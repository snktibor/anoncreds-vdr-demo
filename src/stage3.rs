//! Stage 3: the file boundary between the credx demo and the Fabric VDR.
//!
//! Stage 1–2 kept the accumulator publish/read seams in memory. Stage 3 puts the
//! real ledger behind them, but credx is Rust and the VDR client is Java, so the
//! two halves meet over a JSON file rather than a function call:
//!
//! 1. [`emit`] runs the credx lifecycle and writes `bundle.json`, the material a
//!    publisher needs to drive the VDR transactions, in the `docs/anoncreds-method.md`
//!    §8.1.6 wire shape. Alongside it, `session.json` keeps the credx objects this
//!    process must hold on to in order to check the ledger's answer later.
//! 2. The Java harness reads `bundle.json`, publishes every object, reads each one
//!    back, and writes what the ledger returned to `vdr-report.json`.
//! 3. [`verify`] reassembles an [`IssuerPublic`] from the *returned* values using the
//!    §8.1.5 recipe and verifies real presentations against it.
//!
//! Nothing about revocation itself moves on chain: revoking and refreshing a witness
//! are pure credx operations here exactly as they were in Stage 2. The VDR only ever
//! sees accumulator values.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use credx::credential::CredentialBundle;
use credx::error::Error as CredxError;
use credx::issuer::IssuerPublic;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    apply_refreshed_revocation_handle, create_credential_schema, create_issuer,
    create_presentation, create_presentation_request, issue_credential, obtain_current_accumulator,
    publish_current_accumulator, request_fresh_revocation_handle, revoke_credential,
    verify_presentation, DemoScheme, EmployeeCredentialValues,
};

/// The pinned credx build every artifact in this stage was produced against.
pub const CREDX_COMMIT: &str = "691297a7f9ffcc1f51a5d30741086402d64544c9";

/// `CredDef.type` for the BBS signature scheme, per `VdrTypes.CRED_DEF_BBS_V1`.
pub const CRED_DEF_TYPE: &str = "ANONCREDS_V2/BBS/1.0";
/// `RevRegDef.type` for the VB20 accumulator scheme, per `VdrTypes.REV_REG_VB20_V1`.
pub const REV_REG_DEF_TYPE: &str = "ANONCREDS_V2/VB20/1.0";

const CRED_DEF_TAG: &str = "tag-1";
const SCHEMA_NAME: &str = "employee-credential";
const SCHEMA_VERSION: &str = "1.0";
// VB20 places no cryptographic bound on registry size; this is the issuer's declared
// capacity statement only (docs/anoncreds-method.md §8.2).
const MAX_CRED_NUM: u64 = 1000;

const ALICE_CREDENTIAL_ID: &str = "stage3-alice-credential";
const BOB_CREDENTIAL_ID: &str = "stage3-bob-credential";

/// Accumulator step label for the value published before any revocation.
pub const STEP_GENESIS: &str = "genesis";
/// Accumulator step label for the value published after Alice is revoked.
pub const STEP_AFTER_ALICE_REVOKE: &str = "after-alice-revoke";

/// The publisher-facing artifact the Java harness reads.
pub const BUNDLE_FILE: &str = "bundle.json";
/// The credx-side state this process keeps between `emit` and `verify`.
pub const SESSION_FILE: &str = "session.json";
/// What the ledger returned, written by the Java harness.
pub const REPORT_FILE: &str = "vdr-report.json";

const REVOCATION_VERIFYING_KEY_FIELD: &str = "revocation_verifying_key";
const REVOCATION_REGISTRY_FIELD: &str = "revocation_registry";

/// Anything that can go wrong across the credx, filesystem and JSON layers of this stage.
#[derive(Debug)]
pub enum StageError {
    /// A credx operation failed.
    Credx(CredxError),
    /// A file could not be read or written.
    Io(std::io::Error),
    /// A JSON document could not be parsed, built or converted.
    Json(serde_json::Error),
    /// An invariant this stage exists to prove did not hold.
    Assertion(String),
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credx(error) => write!(f, "credx error: {error:?}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Assertion(message) => write!(f, "assertion failed: {message}"),
        }
    }
}

impl std::error::Error for StageError {}

impl From<CredxError> for StageError {
    fn from(error: CredxError) -> Self {
        Self::Credx(error)
    }
}

impl From<std::io::Error> for StageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for StageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Result type for the artifact-emitting and ledger-verifying paths.
pub type StageResult<T> = Result<T, StageError>;

/// Everything the Java harness needs to publish the AnonCreds objects, and nothing else.
///
/// No DID key material appears here: the harness mints its issuer DID keypair fresh
/// per run, since a P-256 DID key is unrelated to the credx BLS material.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VdrBundle {
    /// The credx build the crypto material came from.
    pub credx_commit: String,
    /// Drives `createSchema`.
    pub schema: BundleSchema,
    /// Drives `createCredDef`.
    pub cred_def: BundleCredDef,
    /// Drives `createRevRegDef`.
    pub rev_reg_def: BundleRevRegDef,
    /// Drives `createRevRegEntry` (first element) then one `updateRevRegEntry` per later element.
    pub accumulators: Vec<BundleAccumulator>,
}

/// `createSchema(id, issuerId, name, version, attrNamesJson, jwt)` payload fields.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleSchema {
    /// Human-readable schema name.
    pub name: String,
    /// Schema version string.
    pub version: String,
    /// Attribute names in claim order — equal to `publicMetadata.schema.claim_indices` (§8.1.6).
    pub attr_names: Vec<String>,
}

/// `createCredDef(id, issuerId, schemaId, type, tag, publicMetadataJson, jwt)` payload fields.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleCredDef {
    /// Allowlisted signature-scheme token.
    #[serde(rename = "type")]
    pub type_name: String,
    /// Issuer-chosen tag.
    pub tag: String,
    /// The §8.1.6 shape: a serialized `IssuerPublic` minus the two first-class fields.
    pub public_metadata: Value,
}

/// `createRevRegDef(id, issuerId, credDefId, type, publicKeysJson, maxCredNum, jwt)` payload fields.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleRevRegDef {
    /// Allowlisted revocation-scheme token.
    #[serde(rename = "type")]
    pub type_name: String,
    /// Strictly validated: exactly one field, nothing else (§8.1.3).
    pub public_keys: BundlePublicKeys,
    /// Issuer-declared capacity statement; carries no cryptographic meaning under VB20.
    pub max_cred_num: u64,
}

/// The revocation registry definition's public key object — exactly one field by specification.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundlePublicKeys {
    /// Compressed BLS12-381 G2 point as 192 lowercase hex characters.
    pub revocation_verifying_key: String,
}

/// One accumulator value and the lifecycle step that produced it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleAccumulator {
    /// Which demo step published this value.
    pub step: String,
    /// Compressed BLS12-381 G1 point as 96 lowercase hex characters.
    pub accum_value: String,
}

/// The credx objects this process must keep in order to check what the ledger returns.
///
/// Only public issuer material and holder credentials are written: the issuer's secret
/// keys never leave the emitting process.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoSession {
    /// The credx build the crypto material came from.
    pub credx_commit: String,
    /// The `IssuerPublic` as credx built it — the reference the reassembly is judged against.
    pub issuer_public: IssuerPublic<DemoScheme>,
    /// The accumulator published before any revocation.
    pub genesis_accumulator: String,
    /// The accumulator published after Alice was revoked.
    pub revoked_accumulator: String,
    /// Revoked Alice's bundle, kept as the negative control.
    pub alice_credential_revoked: CredentialBundle<DemoScheme>,
    /// Bob's bundle while his witness still matches the genesis accumulator.
    pub bob_credential_genesis_witness: CredentialBundle<DemoScheme>,
    /// Bob's bundle after an off-ledger witness refresh against the post-revocation accumulator.
    pub bob_credential_refreshed_witness: CredentialBundle<DemoScheme>,
}

/// What the Java harness read back out of the ledger.
///
/// Unknown fields are ignored, so the harness may record extra provenance (transaction
/// ids, entry history) without this side changing.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VdrReport {
    /// The issuer DID the harness minted for this run.
    pub issuer_did: String,
    /// Minted schema identifier.
    pub schema_id: String,
    /// Minted credential definition identifier.
    pub cred_def_id: String,
    /// Minted revocation registry definition identifier.
    pub rev_reg_def_id: String,
    /// `publicMetadata` exactly as `getCredDef` returned it.
    pub cred_def_public_metadata: Value,
    /// `publicKeys.revocationVerifyingKey` exactly as `getRevRegDef` returned it.
    pub revocation_verifying_key: String,
    /// One element per `getRevRegEntry` read, in ledger version order.
    pub rev_reg_entries: Vec<ReportEntry>,
}

/// One `getRevRegEntry` read.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportEntry {
    /// The demo step whose accumulator this entry holds.
    pub step: String,
    /// The entry's ledger audit version.
    pub version: u64,
    /// The accumulator value the ledger returned.
    pub accum_value: String,
}

/// Runs the credx lifecycle and writes the Java-facing bundle plus this side's session state.
pub fn emit(directory: &Path) -> StageResult<()> {
    println!("[emit 1/5] building the credential definition and issuing Alice and Bob");
    let credential_schema = create_credential_schema()?;
    let (issuer_public, mut issuer) = create_issuer(&credential_schema);

    let alice_credential = issue_credential(
        &mut issuer,
        &EmployeeCredentialValues {
            credential_id: ALICE_CREDENTIAL_ID,
            name: "Alice Example",
            role: "Engineer",
            employee_number: 42,
        },
    )?;
    let mut bob_credential = issue_credential(
        &mut issuer,
        &EmployeeCredentialValues {
            credential_id: BOB_CREDENTIAL_ID,
            name: "Bob Example",
            role: "Designer",
            employee_number: 43,
        },
    )?;

    let genesis_accumulator = obtain_current_accumulator(&publish_current_accumulator(&issuer));
    if genesis_accumulator != issuer_public.revocation_registry {
        return Err(StageError::Assertion(
            "issuing moved the accumulator, so the IssuerPublic snapshot is not the genesis value"
                .to_string(),
        ));
    }

    println!("[emit 2/5] verifying both holders against the genesis accumulator in process");
    for (holder, credential) in [("Alice", &alice_credential), ("Bob", &bob_credential)] {
        let request = create_presentation_request(&issuer_public, genesis_accumulator);
        let presentation = create_presentation(credential, &request)?;
        verify_presentation(&presentation, &request)?;
        println!("      {holder} presentation verified: true");
    }

    // Bob's witness still matches the genesis accumulator here; the ledger will serve
    // that accumulator at entry version 0, so this snapshot is what verifies against it.
    let bob_credential_genesis_witness = bob_credential.clone();

    println!("[emit 3/5] revoking Alice off ledger and taking the new accumulator");
    revoke_credential(&mut issuer, ALICE_CREDENTIAL_ID)?;
    let revoked_accumulator = obtain_current_accumulator(&publish_current_accumulator(&issuer));
    if revoked_accumulator == genesis_accumulator {
        return Err(StageError::Assertion(
            "revocation did not move the accumulator".to_string(),
        ));
    }

    println!("[emit 4/5] refreshing Bob's witness off ledger against the new accumulator");
    let bob_fresh_handle = request_fresh_revocation_handle(&issuer, BOB_CREDENTIAL_ID)?;
    apply_refreshed_revocation_handle(&mut bob_credential, bob_fresh_handle, revoked_accumulator);

    println!("[emit 5/5] writing the wire bundle and the session state");
    let split = split_issuer_public(&issuer_public)?;
    let emitted_genesis = point_hex(&genesis_accumulator)?;
    if split.revocation_registry != emitted_genesis {
        return Err(StageError::Assertion(
            "the accumulator taken from IssuerPublic differs from the published genesis value"
                .to_string(),
        ));
    }
    let revoked_hex = point_hex(&revoked_accumulator)?;
    let attr_names = claim_indices(&split.public_metadata)?;

    let bundle = VdrBundle {
        credx_commit: CREDX_COMMIT.to_string(),
        schema: BundleSchema {
            name: SCHEMA_NAME.to_string(),
            version: SCHEMA_VERSION.to_string(),
            attr_names,
        },
        cred_def: BundleCredDef {
            type_name: CRED_DEF_TYPE.to_string(),
            tag: CRED_DEF_TAG.to_string(),
            public_metadata: split.public_metadata,
        },
        rev_reg_def: BundleRevRegDef {
            type_name: REV_REG_DEF_TYPE.to_string(),
            public_keys: BundlePublicKeys {
                revocation_verifying_key: split.revocation_verifying_key,
            },
            max_cred_num: MAX_CRED_NUM,
        },
        accumulators: vec![
            BundleAccumulator {
                step: STEP_GENESIS.to_string(),
                accum_value: emitted_genesis.clone(),
            },
            BundleAccumulator {
                step: STEP_AFTER_ALICE_REVOKE.to_string(),
                accum_value: revoked_hex.clone(),
            },
        ],
    };

    let session = DemoSession {
        credx_commit: CREDX_COMMIT.to_string(),
        issuer_public,
        genesis_accumulator: emitted_genesis,
        revoked_accumulator: revoked_hex,
        alice_credential_revoked: alice_credential,
        bob_credential_genesis_witness,
        bob_credential_refreshed_witness: bob_credential,
    };

    fs::create_dir_all(directory)?;
    write_json(&directory.join(BUNDLE_FILE), &bundle)?;
    write_json(&directory.join(SESSION_FILE), &session)?;
    println!(
        "      bundle written: {}",
        directory.join(BUNDLE_FILE).display()
    );
    println!(
        "      session written: {}",
        directory.join(SESSION_FILE).display()
    );
    println!("      next: run the Java harness, then `cargo run -- verify`");
    Ok(())
}

/// Reassembles an `IssuerPublic` from what the ledger returned and verifies presentations against it.
///
/// The assertions are stated per entry version on purpose. credx's `IssuerPublic` is a
/// snapshot that keeps the genesis accumulator forever, while the §8.1.5 recipe inserts
/// whatever `getRevRegEntry` currently holds — so only version 0 can be byte-identical.
pub fn verify(directory: &Path) -> StageResult<()> {
    let session: DemoSession = read_json(&directory.join(SESSION_FILE))?;
    let report: VdrReport = read_json(&directory.join(REPORT_FILE))?;

    println!("[verify 1/5] checking what the ledger returned against what was published");
    if report.revocation_verifying_key
        != point_hex(&session.issuer_public.revocation_verifying_key)?
    {
        return Err(StageError::Assertion(
            "getRevRegDef returned a different revocationVerifyingKey than was published"
                .to_string(),
        ));
    }
    let genesis_entry = require_entry(&report, 0)?;
    let updated_entry = require_entry(&report, 1)?;
    if genesis_entry.accum_value != session.genesis_accumulator {
        return Err(StageError::Assertion(
            "entry version 0 does not hold the published genesis accumulator".to_string(),
        ));
    }
    if updated_entry.accum_value != session.revoked_accumulator {
        return Err(StageError::Assertion(
            "entry version 1 does not hold the published post-revocation accumulator".to_string(),
        ));
    }
    println!("      revocationVerifyingKey round-tripped: true");
    println!(
        "      entry v0 = {}… ({})",
        prefix(&genesis_entry.accum_value),
        genesis_entry.step
    );
    println!(
        "      entry v1 = {}… ({})",
        prefix(&updated_entry.accum_value),
        updated_entry.step
    );

    println!("[verify 2/5] reassembling IssuerPublic from the resolved objects (§8.1.5)");
    let document_v0 = reassembly_document(
        &report.cred_def_public_metadata,
        &report.revocation_verifying_key,
        &genesis_entry.accum_value,
    )?;
    let document_v1 = reassembly_document(
        &report.cred_def_public_metadata,
        &report.revocation_verifying_key,
        &updated_entry.accum_value,
    )?;
    probe_from_value(&document_v0);
    let reassembled_v0 = reassemble_issuer_public(&document_v0)?;
    let reassembled_v1 = reassemble_issuer_public(&document_v1)?;
    println!("      deserialized from the JSON text as IssuerPublic<BbsScheme>: true");

    println!("[verify 3/5] version-aware equality against the original IssuerPublic");
    let original_bytes = serde_json::to_string(&session.issuer_public)?;
    let reassembled_v0_bytes = serde_json::to_string(&reassembled_v0)?;
    if original_bytes != reassembled_v0_bytes {
        return Err(StageError::Assertion(format!(
            "at entry version 0 the reassembled IssuerPublic is not byte-identical to the \
             original\n  original:    {original_bytes}\n  reassembled: {reassembled_v0_bytes}"
        )));
    }
    println!("      v0 byte-for-byte equal to the original: true");

    let original_without_accumulator = without_accumulator(&session.issuer_public)?;
    let reassembled_v1_without_accumulator = without_accumulator(&reassembled_v1)?;
    if original_without_accumulator != reassembled_v1_without_accumulator {
        return Err(StageError::Assertion(
            "at entry version 1 the reassembled IssuerPublic differs from the original in a \
             field other than the accumulator"
                .to_string(),
        ));
    }
    if reassembled_v1.revocation_registry == session.issuer_public.revocation_registry {
        return Err(StageError::Assertion(
            "at entry version 1 the reassembled accumulator still equals the genesis value"
                .to_string(),
        ));
    }
    println!("      v1 equal to the original with the accumulator excluded: true");
    println!("      v1 accumulator replaced by the ledger's current value: true");

    println!("[verify 4/5] verifying presentations against the ledger-sourced IssuerPublic");
    let genesis_request =
        create_presentation_request(&reassembled_v0, reassembled_v0.revocation_registry);
    let genesis_presentation =
        create_presentation(&session.bob_credential_genesis_witness, &genesis_request)?;
    verify_presentation(&genesis_presentation, &genesis_request)?;
    println!("      Bob verifies at entry v0 with his genesis witness: true");

    let updated_request =
        create_presentation_request(&reassembled_v1, reassembled_v1.revocation_registry);
    let updated_presentation =
        create_presentation(&session.bob_credential_refreshed_witness, &updated_request)?;
    verify_presentation(&updated_presentation, &updated_request)?;
    println!("      Bob verifies at entry v1 with his refreshed witness: true");

    println!("[verify 5/5] negative control: revoked Alice against the ledger's v1 accumulator");
    let alice_presentation =
        create_presentation(&session.alice_credential_revoked, &updated_request)?;
    match verify_presentation(&alice_presentation, &updated_request) {
        Err(CredxError::InvalidPresentationData(message))
            if message.contains("expected challenge")
                && message.contains("does not match the computed challenge") => {}
        Err(error) => {
            return Err(StageError::Assertion(format!(
                "revoked Alice failed for an unrelated reason: {error:?}"
            )))
        }
        Ok(()) => {
            return Err(StageError::Assertion(
                "revoked Alice verified against the ledger's current accumulator".to_string(),
            ))
        }
    }
    println!("      Alice rejected as expected: true");

    println!("§8.1.6 round-tripped through the ledger into a verifying IssuerPublic: true");
    Ok(())
}

/// The three pieces a serialized `IssuerPublic` splits into across the three VDR objects.
struct IssuerPublicSplit {
    public_metadata: Value,
    revocation_verifying_key: String,
    revocation_registry: String,
}

/// Splits a serialized `IssuerPublic` into the §8.1.5 distribution: two first-class
/// fields, and whatever remains as `publicMetadata`. No renaming or restructuring.
fn split_issuer_public(issuer_public: &IssuerPublic<DemoScheme>) -> StageResult<IssuerPublicSplit> {
    let mut value = serde_json::to_value(issuer_public)?;
    let object = value.as_object_mut().ok_or_else(|| {
        StageError::Assertion("IssuerPublic did not serialize as a JSON object".to_string())
    })?;
    let revocation_verifying_key = take_string(object, REVOCATION_VERIFYING_KEY_FIELD)?;
    let revocation_registry = take_string(object, REVOCATION_REGISTRY_FIELD)?;
    Ok(IssuerPublicSplit {
        public_metadata: value,
        revocation_verifying_key,
        revocation_registry,
    })
}

/// Builds the §8.1.5 reassembly document: the resolved `publicMetadata` with the two
/// first-class fields inserted. No renaming or restructuring, exactly as specified.
fn reassembly_document(
    public_metadata: &Value,
    revocation_verifying_key: &str,
    accum_value: &str,
) -> StageResult<Value> {
    let mut value = public_metadata.clone();
    let object = value.as_object_mut().ok_or_else(|| {
        StageError::Assertion("resolved publicMetadata is not a JSON object".to_string())
    })?;
    object.insert(
        REVOCATION_VERIFYING_KEY_FIELD.to_string(),
        Value::String(revocation_verifying_key.to_string()),
    );
    object.insert(
        REVOCATION_REGISTRY_FIELD.to_string(),
        Value::String(accum_value.to_string()),
    );
    Ok(value)
}

/// Deserializes a reassembly document as an `IssuerPublic`, from its JSON **text**.
///
/// The text matters. `serde_json::from_value` cannot do this: every BLS12-381 point in
/// the document — the accumulator, the revocation verifying key, each `verifying_key.y`
/// entry, the verifiable encryption key — goes through blstrs_plus's human-readable
/// deserializer, which asks for a *borrowed* `&str` (`serde_impl.rs:118`). An owned
/// `Value` can only hand a visitor a transient one, so `from_value` fails on the first
/// curve point with `invalid type: string "...", expected a borrowed string`. Serializing
/// the document and parsing that text gives the visitor a borrow into the input, which is
/// what a resolver holding bytes off the wire has anyway. See [`probe_from_value`].
fn reassemble_issuer_public(document: &Value) -> StageResult<IssuerPublic<DemoScheme>> {
    let text = serde_json::to_string(document)?;
    Ok(serde_json::from_str(&text)?)
}

/// Records what `serde_json::from_value` actually does with a reassembly document.
///
/// Kept in the demo rather than in prose because it is the one interop constraint §8.1.5
/// does not state: the recipe has to be executed on JSON text, not on an in-memory tree.
fn probe_from_value(document: &Value) {
    match serde_json::from_value::<IssuerPublic<DemoScheme>>(document.clone()) {
        Ok(_) => println!("      serde_json::from_value accepted the document: true"),
        Err(error) => println!("      serde_json::from_value rejected the document: {error}"),
    }
}

/// Serializes an `IssuerPublic` and drops the accumulator, for the version 1 comparison.
fn without_accumulator(issuer_public: &IssuerPublic<DemoScheme>) -> StageResult<Value> {
    let mut value = serde_json::to_value(issuer_public)?;
    let object = value.as_object_mut().ok_or_else(|| {
        StageError::Assertion("IssuerPublic did not serialize as a JSON object".to_string())
    })?;
    object.remove(REVOCATION_REGISTRY_FIELD);
    Ok(value)
}

/// Reads `publicMetadata.schema.claim_indices`, which §8.1.6 makes the authoritative
/// claim ordering and therefore the schema's `attrNames`.
fn claim_indices(public_metadata: &Value) -> StageResult<Vec<String>> {
    let indices = public_metadata
        .get("schema")
        .and_then(|schema| schema.get("claim_indices"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            StageError::Assertion("publicMetadata.schema.claim_indices is missing".to_string())
        })?;
    indices
        .iter()
        .map(|entry| {
            entry.as_str().map(str::to_string).ok_or_else(|| {
                StageError::Assertion(
                    "publicMetadata.schema.claim_indices holds a non-string entry".to_string(),
                )
            })
        })
        .collect()
}

/// Serializes a curve point and unwraps credx's bare hex string encoding.
fn point_hex<T: Serialize>(point: &T) -> StageResult<String> {
    match serde_json::to_value(point)? {
        Value::String(hex) => Ok(hex),
        other => Err(StageError::Assertion(format!(
            "expected a bare hex string for a curve point, got {other}"
        ))),
    }
}

fn take_string(object: &mut Map<String, Value>, key: &str) -> StageResult<String> {
    match object.remove(key) {
        Some(Value::String(value)) => Ok(value),
        Some(other) => Err(StageError::Assertion(format!(
            "IssuerPublic.{key} is not a string: {other}"
        ))),
        None => Err(StageError::Assertion(format!(
            "IssuerPublic.{key} is missing"
        ))),
    }
}

/// Shortens a hex value for log lines without risking a slice out of bounds.
fn prefix(value: &str) -> &str {
    let end = value.len().min(16);
    &value[..end]
}

fn require_entry(report: &VdrReport, version: u64) -> StageResult<&ReportEntry> {
    report
        .rev_reg_entries
        .iter()
        .find(|entry| entry.version == version)
        .ok_or_else(|| {
            StageError::Assertion(format!(
                "the harness reported no revocation registry entry at version {version}"
            ))
        })
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> StageResult<()> {
    let mut json = serde_json::to_string_pretty(value)?;
    json.push('\n');
    fs::write(path, json)?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> StageResult<T> {
    let text = fs::read_to_string(path).map_err(|error| {
        StageError::Io(std::io::Error::new(
            error.kind(),
            format!("{}: {error}", path.display()),
        ))
    })?;
    Ok(serde_json::from_str(&text)?)
}

/// The default artifact directory, inside the crate's gitignored build output.
pub fn default_directory() -> PathBuf {
    PathBuf::from("target").join("stage3")
}
