pub mod stage3;

use std::collections::BTreeSet;

use credx::claim::{ClaimType, ClaimValidator, HashedClaim, NumberClaim, RevocationClaim};
use credx::credential::{ClaimSchema, CredentialBundle, CredentialSchema};
use credx::indexmap::IndexMap;
use credx::issuer::{Issuer, IssuerPublic};
use credx::knox::accumulator::vb20::{Accumulator, MembershipWitness};
use credx::knox::bbs::BbsScheme;
use credx::presentation::{Presentation, PresentationSchema};
use credx::statement::{RevocationStatement, SignatureStatement};
use credx::CredxResult;
use rand::{thread_rng, RngCore};

pub type DemoScheme = BbsScheme;
pub type PresentationNonce = [u8; 32];

const CREDENTIAL_STATEMENT_ID: &str = "employee-credential-signature";
const REVOCATION_STATEMENT_ID: &str = "employee-credential-revocation";
const REVOCATION_CLAIM_INDEX: usize = 0;

/// Values supplied by the issuer when creating the demo credential.
pub struct EmployeeCredentialValues<'a> {
    pub credential_id: &'a str,
    pub name: &'a str,
    pub role: &'a str,
    pub employee_number: u64,
}

/// Public verifier request passed directly to a holder.
///
/// A later transport layer can carry this data without changing the crypto calls.
pub struct PresentationRequest {
    pub credential_statement_id: String,
    pub schema: PresentationSchema<DemoScheme>,
    pub nonce: PresentationNonce,
}

/// The accumulator value made available by the issuer.
///
/// This wrapper is the in-memory stand-in for the value Stage 3 will publish to
/// Fabric. Its field stays private so holders obtain it through the read seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublishedAccumulator {
    value: Accumulator,
}

/// Build the ordered claim schema used by both issuer and verifier.
pub fn create_credential_schema() -> CredxResult<CredentialSchema> {
    let claims = [
        ClaimSchema {
            claim_type: ClaimType::Revocation,
            label: "credential_id".to_string(),
            print_friendly: false,
            validators: vec![],
        },
        ClaimSchema {
            claim_type: ClaimType::Hashed,
            label: "name".to_string(),
            print_friendly: true,
            validators: vec![ClaimValidator::Length {
                min: Some(1),
                max: Some(128),
            }],
        },
        ClaimSchema {
            claim_type: ClaimType::Hashed,
            label: "role".to_string(),
            print_friendly: true,
            validators: vec![ClaimValidator::Length {
                min: Some(1),
                max: Some(128),
            }],
        },
        ClaimSchema {
            claim_type: ClaimType::Number,
            label: "employee_number".to_string(),
            print_friendly: true,
            validators: vec![ClaimValidator::Range {
                min: Some(1),
                max: None,
            }],
        },
    ];

    CredentialSchema::new(
        Some("employee-credential"),
        Some("In-memory AnonCreds v2 employee credential"),
        &[],
        &claims,
    )
}

/// Mint issuer secret keys and the corresponding public credential definition.
pub fn create_issuer(schema: &CredentialSchema) -> (IssuerPublic<DemoScheme>, Issuer<DemoScheme>) {
    Issuer::<DemoScheme>::new(schema)
}

/// Sign claim values and return the credential bundle delivered to the holder.
pub fn issue_credential(
    issuer: &mut Issuer<DemoScheme>,
    values: &EmployeeCredentialValues<'_>,
) -> CredxResult<CredentialBundle<DemoScheme>> {
    issuer.sign_credential(&[
        RevocationClaim::from(values.credential_id).into(),
        HashedClaim::from(values.name).into(),
        HashedClaim::from(values.role).into(),
        NumberClaim::from(values.employee_number).into(),
    ])
}

/// Revoke one credential and move the issuer's accumulator.
pub fn revoke_credential(issuer: &mut Issuer<DemoScheme>, credential_id: &str) -> CredxResult<()> {
    issuer.revoke_credentials(&[RevocationClaim::from(credential_id)])
}

/// Publish the issuer's exact current accumulator value.
///
/// Stage 3 can put its Fabric write at this boundary.
pub fn publish_current_accumulator(issuer: &Issuer<DemoScheme>) -> PublishedAccumulator {
    PublishedAccumulator {
        value: issuer.revocation_registry.value,
    }
}

/// Obtain the current accumulator from the publication channel.
///
/// Stage 3 can put its Fabric read at this boundary.
pub fn obtain_current_accumulator(publication: &PublishedAccumulator) -> Accumulator {
    publication.value
}

/// Ask the issuer for a fresh membership witness for an unrevoked credential.
///
/// This is intentionally separate from applying the witness so Stage 3 can put
/// the issuer-side, off-ledger refresh interaction at this boundary.
pub fn request_fresh_revocation_handle(
    issuer: &Issuer<DemoScheme>,
    credential_id: &str,
) -> CredxResult<MembershipWitness> {
    issuer.update_revocation_handle(RevocationClaim::from(credential_id))
}

/// Apply an issuer-provided witness and its matching accumulator to a holder's bundle.
pub fn apply_refreshed_revocation_handle(
    credential: &mut CredentialBundle<DemoScheme>,
    refreshed_handle: MembershipWitness,
    current_accumulator: Accumulator,
) {
    credential.update_revocation_handle(refreshed_handle, current_accumulator);
}

/// Build the verifier's disclosure and non-revocation request with a fresh nonce.
pub fn create_presentation_request(
    issuer_public: &IssuerPublic<DemoScheme>,
    current_accumulator: Accumulator,
) -> PresentationRequest {
    let signature_statement = SignatureStatement {
        disclosed: BTreeSet::from(["name".to_string(), "role".to_string()]),
        id: CREDENTIAL_STATEMENT_ID.to_string(),
        issuer: issuer_public.clone(),
    };
    let revocation_statement = RevocationStatement {
        id: REVOCATION_STATEMENT_ID.to_string(),
        reference_id: signature_statement.id.clone(),
        accumulator: current_accumulator,
        verification_key: issuer_public.revocation_verifying_key,
        claim: REVOCATION_CLAIM_INDEX,
    };

    let mut nonce = [0_u8; 32];
    thread_rng().fill_bytes(&mut nonce);

    PresentationRequest {
        credential_statement_id: signature_statement.id.clone(),
        schema: PresentationSchema::new(&[signature_statement.into(), revocation_statement.into()]),
        nonce,
    }
}

/// Have the holder create a presentation for the verifier's request.
pub fn create_presentation(
    credential: &CredentialBundle<DemoScheme>,
    request: &PresentationRequest,
) -> CredxResult<Presentation<DemoScheme>> {
    let mut credentials = IndexMap::new();
    credentials.insert(
        request.credential_statement_id.clone(),
        credential.credential.clone().into(),
    );

    Presentation::create(&credentials, &request.schema, &request.nonce)
}

/// Verify the holder's presentation against the original request.
pub fn verify_presentation(
    presentation: &Presentation<DemoScheme>,
    request: &PresentationRequest,
) -> CredxResult<()> {
    presentation.verify(&request.schema, &request.nonce)
}
