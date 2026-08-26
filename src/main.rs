use anoncreds_vdr_demo::{
    apply_refreshed_revocation_handle, create_credential_schema, create_issuer,
    create_presentation, create_presentation_request, issue_credential, obtain_current_accumulator,
    publish_current_accumulator, request_fresh_revocation_handle, revoke_credential,
    verify_presentation, EmployeeCredentialValues,
};
use credx::error::Error;
use credx::CredxResult;

const ALICE_CREDENTIAL_ID: &str = "stage2-alice-credential";
const BOB_CREDENTIAL_ID: &str = "stage2-bob-credential";

fn assert_expected_revocation_failure(result: CredxResult<()>, holder: &str) {
    // credx reports both a revoked witness and a never-revoked but stale witness
    // as this same Fiat-Shamir challenge mismatch. The error proves verification
    // failed for the expected crypto reason, but does not distinguish the two cases.
    match result {
        Err(Error::InvalidPresentationData(message))
            if message.contains("expected challenge")
                && message.contains("does not match the computed challenge") => {}
        Err(error) => panic!("{holder} failed for an unrelated reason: {error:?}"),
        Ok(()) => panic!("{holder}'s non-revocation presentation unexpectedly verified"),
    }
}

fn main() -> CredxResult<()> {
    println!("[1/6] issuing Alice and Bob credentials; verifying both before revocation");
    let credential_schema = create_credential_schema()?;
    let (issuer_public, mut issuer) = create_issuer(&credential_schema);

    let alice_values = EmployeeCredentialValues {
        credential_id: ALICE_CREDENTIAL_ID,
        name: "Alice Example",
        role: "Engineer",
        employee_number: 42,
    };
    let bob_values = EmployeeCredentialValues {
        credential_id: BOB_CREDENTIAL_ID,
        name: "Bob Example",
        role: "Designer",
        employee_number: 43,
    };
    let alice_credential = issue_credential(&mut issuer, &alice_values)?;
    let mut bob_credential = issue_credential(&mut issuer, &bob_values)?;

    let initial_publication = publish_current_accumulator(&issuer);
    let initial_accumulator = obtain_current_accumulator(&initial_publication);

    let alice_request = create_presentation_request(&issuer_public, initial_accumulator);
    let alice_presentation = create_presentation(&alice_credential, &alice_request)?;
    verify_presentation(&alice_presentation, &alice_request)?;

    let bob_request = create_presentation_request(&issuer_public, initial_accumulator);
    let bob_presentation = create_presentation(&bob_credential, &bob_request)?;
    verify_presentation(&bob_presentation, &bob_request)?;
    println!("      Alice presentation verified: true");
    println!("      Bob presentation verified: true");

    println!("[2/6] issuer revokes Alice and publishes the updated accumulator");
    revoke_credential(&mut issuer, ALICE_CREDENTIAL_ID)?;
    let updated_publication = publish_current_accumulator(&issuer);
    let updated_accumulator = obtain_current_accumulator(&updated_publication);
    assert_ne!(
        initial_accumulator, updated_accumulator,
        "revocation must move the accumulator"
    );
    println!("      Alice revoked: true");
    println!("      Accumulator changed: true");

    println!("[3/6] revoked Alice presents against the current accumulator");
    let alice_current_accumulator = obtain_current_accumulator(&updated_publication);
    let alice_request = create_presentation_request(&issuer_public, alice_current_accumulator);
    let alice_presentation = create_presentation(&alice_credential, &alice_request)?;
    let alice_verification = verify_presentation(&alice_presentation, &alice_request);
    assert_expected_revocation_failure(alice_verification, "revoked Alice");
    println!("      Alice verification rejected as expected: true");

    println!("[4/6] never-revoked Bob presents with his stale handle");
    let bob_current_accumulator = obtain_current_accumulator(&updated_publication);
    let bob_request = create_presentation_request(&issuer_public, bob_current_accumulator);
    let bob_presentation = create_presentation(&bob_credential, &bob_request)?;
    let bob_stale_verification = verify_presentation(&bob_presentation, &bob_request);
    assert_expected_revocation_failure(bob_stale_verification, "stale Bob");
    println!("      Bob stale-handle verification rejected as expected: true");

    println!("[5/6] Bob requests and applies a fresh handle, then presents again");
    let bob_fresh_handle = request_fresh_revocation_handle(&issuer, BOB_CREDENTIAL_ID)?;
    let bob_current_accumulator = obtain_current_accumulator(&updated_publication);
    apply_refreshed_revocation_handle(
        &mut bob_credential,
        bob_fresh_handle,
        bob_current_accumulator,
    );
    let bob_request = create_presentation_request(&issuer_public, bob_current_accumulator);
    let bob_presentation = create_presentation(&bob_credential, &bob_request)?;
    verify_presentation(&bob_presentation, &bob_request)?;
    println!("      Bob refreshed-handle verification succeeded: true");

    println!("[6/6] Alice requests a fresh handle and the issuer refuses");
    match request_fresh_revocation_handle(&issuer, ALICE_CREDENTIAL_ID) {
        Err(Error::InvalidRevocationRegistryRevokeOperation) => {}
        Err(error) => panic!("Alice refresh failed for an unrelated reason: {error:?}"),
        Ok(_) => panic!("issuer unexpectedly refreshed revoked Alice's handle"),
    }
    println!("      Alice refresh refused as expected: true");

    println!("revocation lifecycle verified: true");
    Ok(())
}
