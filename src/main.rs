use anoncreds_vdr_demo::{
    create_credential_schema, create_issuer, create_presentation, create_presentation_request,
    issue_credential, verify_presentation, EmployeeCredentialValues,
};
use credx::CredxResult;

fn main() -> CredxResult<()> {
    println!("[1/6] creating credential schema");
    let credential_schema = create_credential_schema()?;

    println!("[2/6] creating issuer keys and credential definition");
    let (issuer_public, mut issuer) = create_issuer(&credential_schema);

    println!("[3/6] issuing credential to holder");
    let values = EmployeeCredentialValues {
        credential_id: "stage1-cred-0001",
        name: "Alice Example",
        role: "Engineer",
        employee_number: 42,
    };
    let holder_credential = issue_credential(&mut issuer, &values)?;

    println!("[4/6] building presentation request and statements");
    let presentation_request = create_presentation_request(&issuer_public);

    println!("[5/6] holder creating presentation");
    let presentation = create_presentation(&holder_credential, &presentation_request)?;

    println!("[6/6] verifier checking presentation");
    let verified = verify_presentation(&presentation, &presentation_request)?;
    assert!(verified, "credx rejected the presentation");

    println!("presentation verified: {verified}");
    Ok(())
}
