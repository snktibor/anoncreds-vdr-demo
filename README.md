# AnonCreds v2 VDR demo

Stage 2 exercises the credx happy path and revocation lifecycle in one Rust
process:

1. issue credentials to Alice and Bob and verify both presentations;
2. revoke Alice and publish the changed accumulator in memory;
3. reject Alice's non-revocation presentation;
4. also reject never-revoked Bob while his witness is stale;
5. obtain an issuer-refreshed witness for Bob and verify him successfully; and
6. confirm the issuer refuses to refresh revoked Alice.

The accumulator publication/read and issuer-mediated witness refresh are explicit
function boundaries in `src/lib.rs`. They pass native credx objects directly in
memory. There is no Fabric, VDR, HTTP, wire serialization, batch revocation, or
holder-side coefficient update in this stage.

credx returns the same `InvalidPresentationData` challenge-mismatch error for a
revoked witness and an unrevoked stale witness. The binary checks that precise
error shape for both expected verification failures; the error itself does not
differentiate the two states.

The dependency is pinned to credx commit
`691297a7f9ffcc1f51a5d30741086402d64544c9`.

Run it with:

```console
cargo run
```

The reusable crypto and future transport boundaries live in `src/lib.rs`;
`src/main.rs` only orchestrates and asserts the runnable demonstration.

## Stage 3 — the same lifecycle over the real VDR

Stage 3 puts the Fabric ledger behind the publish/obtain accumulator seams. credx
is Rust and the VDR client is Java, so the two halves meet over JSON files in
`target/stage3/` rather than a function call. `src/stage3.rs` owns this side of
the boundary; the in-process demo above is untouched and still runs.

```console
cargo run -- emit                       # writes bundle.json + session.json
                                        # then run the Java harness (below)
cargo run -- verify                     # reads vdr-report.json and checks it
```

Both modes take an optional directory argument; the default is `target/stage3`.

Between them, from the `fabric-vdr-core` module, with the Fabric network up:

```console
mvn verify -Dit.test=AnonCredsDemoIT \
    -Dtest=none -Dsurefire.failIfNoSpecifiedTests=false
```

Do **not** pass `-DskipTests` there. Failsafe binds its own `skipTests` parameter
to the same `${skipTests}` property surefire uses, so it would skip the
integration test as well and the build would pass having run nothing.

### The three files

`bundle.json` is the publisher-facing artifact, in the
[`docs/anoncreds-method.md`](../docs/anoncreds-method.md) §8.1.6 wire shape. It
carries only what the harness needs to drive transactions — and deliberately no
DID key material, since a P-256 DID key is unrelated to the credx BLS material
and the harness mints its issuer DID fresh per run:

```jsonc
{
  "credxCommit": "691297a7f9ffcc1f51a5d30741086402d64544c9",
  "schema":    { "name": …, "version": …, "attrNames": [ … ] },
  "credDef":   { "type": "ANONCREDS_V2/BBS/1.0", "tag": …, "publicMetadata": { … } },
  "revRegDef": { "type": "ANONCREDS_V2/VB20/1.0",
                 "publicKeys": { "revocationVerifyingKey": "<192 hex>" },
                 "maxCredNum": 1000 },
  "accumulators": [ { "step": …, "accumValue": "<96 hex>" }, … ]
}
```

`publicMetadata` is a serialized `IssuerPublic` with the two first-class fields
removed, and `attrNames` is read straight out of its `schema.claim_indices`, so
the §8.1.6 ordering requirement holds by construction rather than by convention.
`accumulators[0]` drives `createRevRegEntry`; every later element drives one
`updateRevRegEntry`.

`session.json` is this side's own state: the original `IssuerPublic` to judge the
reassembly against, and the credential bundles needed to present later. The
issuer's secret keys are never written.

`vdr-report.json` is what the harness read back off the ledger — the resolved
`publicMetadata`, the resolved `revocationVerifyingKey`, and one entry per
`getRevRegEntry` with its ledger version.

### What `verify` proves

It applies the §8.1.5 reassembly recipe to the *returned* values and asserts
per entry version, because credx's `IssuerPublic` is a snapshot that keeps the
genesis accumulator forever while the recipe inserts whatever `getRevRegEntry`
currently holds:

- at **version 0** the reassembled `IssuerPublic` is byte-for-byte the original;
- at **version 1** it equals the original with the accumulator excluded, and the
  accumulator is the ledger's new value.

Then it verifies real presentations against the ledger-sourced issuer: Bob at
version 0 with his genesis witness, Bob at version 1 with his refreshed witness,
and revoked Alice rejected against the version 1 accumulator.

### One interop constraint §8.1.5 does not state

The recipe has to be executed on the reassembled JSON **text**, not on an
in-memory `serde_json::Value`:

```
serde_json::from_value::<IssuerPublic<BbsScheme>>(document)
  -> invalid type: string "b1524cf9…", expected a borrowed string
```

Every BLS12-381 point in the document — the accumulator, the revocation verifying
key, each `verifying_key.y` entry, the verifiable encryption key — deserializes
through blstrs_plus's human-readable path, which asks for a *borrowed* `&str`. An
owned `Value` can only hand the visitor a transient one, so `from_value` fails on
the first curve point it reaches. `serde_json::from_str` over the serialized
document works, and is what a resolver holding bytes off the wire has anyway.
`stage3::probe_from_value` runs the failing call on every `verify` so the
constraint stays visible rather than becoming folklore.
