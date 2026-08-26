use std::collections::BTreeSet;

use credx::claim::{ClaimType, ClaimValidator, HashedClaim, NumberClaim, RevocationClaim};
use credx::credential::{ClaimSchema, CredentialBundle, CredentialSchema};
use credx::indexmap::IndexMap;
use credx::issuer::{Issuer, IssuerPublic};
use credx::knox::bbs::BbsScheme;
use credx::presentation::{Presentation, PresentationSchema};
use credx::statement::SignatureStatement;
use credx::CredxResult;
use rand::{thread_rng, RngCore};

pub type DemoScheme = BbsScheme;
pub type PresentationNonce = [u8; 32];

const CREDENTIAL_STATEMENT_ID: &str = "employee-credential-signature";

/// Values supplied by the issuer when creating the demo credential.
pub struct EmployeeCredentialValues<'a> {
    pub credential_id: &'a str,
    pub name: &'a str,
    pub role: &'a str,
    pub employee_number: u64,
}

/// Public verifier request passed directly to the holder in Stage 1.
///
/// A later transport layer can carry this data without changing the crypto calls.
pub struct PresentationRequest {
    pub credential_statement_id: String,
    pub schema: PresentationSchema<DemoScheme>,
    pub nonce: PresentationNonce,
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
        Some("Stage 1 in-memory AnonCreds v2 credential"),
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

/// Build the verifier's disclosure request and fresh challenge nonce.
pub fn create_presentation_request(
    issuer_public: &IssuerPublic<DemoScheme>,
) -> PresentationRequest {
    let statement = SignatureStatement {
        disclosed: BTreeSet::from(["name".to_string(), "role".to_string()]),
        id: CREDENTIAL_STATEMENT_ID.to_string(),
        issuer: issuer_public.clone(),
    };

    let mut nonce = [0_u8; 32];
    thread_rng().fill_bytes(&mut nonce);

    PresentationRequest {
        credential_statement_id: statement.id.clone(),
        schema: PresentationSchema::new(&[statement.into()]),
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
) -> CredxResult<bool> {
    presentation.verify(&request.schema, &request.nonce)?;
    Ok(true)
}
