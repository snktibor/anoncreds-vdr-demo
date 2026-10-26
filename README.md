# AnonCreds v2 VDR demo

A real [`credx`](https://github.com/hyperledger/anoncreds-v2-rs) AnonCreds v2 credential
definition published to a **live Hyperledger Fabric ledger**, a holder revoked on that
ledger, and the ledger's own accumulator deciding who verifies and who does not.

Nothing is mocked. The BBS keys, the VB20 accumulator and the presentations are genuine
credx cryptography; the network is a real four-node Fabric 2.5 deployment running the
[`fabric-vdr-core`](https://github.com/snktibor/fabric-vdr-core) `vdrchaincode` Java
chaincode. This repository holds both the Rust half of the demo and the network recipe
that stands the ledger up, so the only thing it needs from outside is the chaincode.

## Run it

```bash
git clone https://github.com/snktibor/anoncreds-vdr-demo.git anoncreds-vdr-demo
cd anoncreds-vdr-demo
git clone https://github.com/snktibor/fabric-vdr-core.git fabric-vdr-core
./run-anoncreds-demo.sh
```

About ten minutes on a cold machine — it builds the chaincode image, generates the Fabric
network and compiles credx from source. Roughly a minute on a network that is already up.

The second clone must be named exactly `fabric-vdr-core` and sit in this directory:
`fablo-config.json` deploys the chaincode from `./fabric-vdr-core`, which Fablo resolves
against the directory holding `fablo-config.json`. The clone is gitignored here; it stays
its own repository.

### Prerequisites

| Tool | Version | Why |
|---|---|---|
| Docker | any recent engine (verified on 29.4 via OrbStack) | runs the Fabric network |
| **JDK 17** | **exactly 17** (verified on Temurin 17.0.19) | JaCoCo 0.8.12 is bound to `prepare-agent` in `fabric-vdr-core/pom.xml` and injects its agent into the forked test JVM. On JDK 21/25 it aborts with `Unsupported class file major version` before the integration test starts. The script refuses to run on anything else. |
| Maven | 3.9+ (verified on 3.9.15) | builds and packages the chaincode |
| Rust | stable (verified on 1.97.1) | builds this crate |
| Network egress | ghcr.io, Docker Hub, Maven Central, crates.io, GitHub | Fablo, the Fabric images, and credx are all fetched |

If Maven is not already on JDK 17:

```bash
export JAVA_HOME="$(/usr/libexec/java_home -v 17)"   # macOS
export PATH="$JAVA_HOME/bin:$PATH"
```

`cargo` is found on `PATH`, or failing that at `~/.cargo/bin/cargo`.

### What you should see

Four numbered steps, then the sentinel. Anything short of it is a failure, and the exit
code is non-zero:

```
==> [0/4] Preflight
==> [1/4] Fabric network
    peer0.org1.example.com:7041 is serving VdrContract on my-channel1
==> [2/4] credx: issue, revoke, refresh — and write the wire bundle
==> [3/4] Java harness: publish to the live VDR and read it back
==> [4/4] credx: reassemble IssuerPublic from the ledger and verify presentations

==> DEMO PASSED — AnonCreds v2 revocation round-tripped through a live Fabric VDR
```

Step by step, with the output of each leg, in
[`docs/anoncreds-demo.md`](docs/anoncreds-demo.md).

### Tear it down

```bash
./stop-anoncreds-demo.sh                   # stop the network, delete target/stage3/
./stop-anoncreds-demo.sh --keep-artifacts  # stop, but keep the JSON artifacts
./stop-anoncreds-demo.sh --down            # destroy the network entirely
```

The default is a **stop**: the next `./run-anoncreds-demo.sh` resumes in seconds. `--down`
removes the generated crypto material and world state, so the next run rebuilds from
scratch.

## What is in here

| | |
|---|---|
| `run-anoncreds-demo.sh` | the whole demo, in one command |
| `stop-anoncreds-demo.sh` | teardown |
| `bootstrap.sh` | builds the chaincode image and brings the network up; the runner calls it |
| `fablo`, `fablo-config.json` | pinned [Fablo](https://github.com/hyperledger-labs/fablo) 2.6.0 CLI and the network topology: Fabric 2.5.15, TLS, one `solo` orderer, `Org1MSP` with two peers, channel `my-channel1`, `vdrchaincode` 0.0.7 deployed as Chaincode-as-a-Service |
| `Cargo.toml`, `src/` | the Rust demo — credx crypto, the wire bundle, the verification |
| `docs/anoncreds-demo.md` | the walkthrough: prerequisites, expected output, clean-vs-resume, failure injection |

`fablo-target/`, `chaincode-packages/`, `target/` and the `fabric-vdr-core/` clone are all
gitignored and regenerated.

---

## The Rust demo on its own

Stage 2 exercises the credx happy path and revocation lifecycle in one Rust
process, with no ledger involved:

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

Between them, from the `fabric-vdr-core` directory, with the Fabric network up:

```console
mvn verify -Dit.test=AnonCredsDemoIT \
    -Dtest=none -Dsurefire.failIfNoSpecifiedTests=false
```

Do **not** pass `-DskipTests` there. Failsafe binds its own `skipTests` parameter
to the same `${skipTests}` property surefire uses, so it would skip the
integration test as well and the build would pass having run nothing.

`run-anoncreds-demo.sh` runs all three legs in order, and additionally exports the two
paths the harness needs from outside its own repository — `VDR_E2E_CRYPTO_PATH` and
`VDR_DEMO_STAGE3_DIR` — which Maven's forked test JVM inherits.

### The three files

`bundle.json` is the publisher-facing artifact, in the
[Hyperledger Fabric AnonCreds Method](https://github.com/snktibor/fabric-vdr-core/blob/main/docs/anoncreds-method.md) §8.1.6 wire shape. It
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

## License

Apache License 2.0 — see [LICENSE](LICENSE) and [NOTICE](NOTICE). Report security issues
as [SECURITY.md](SECURITY.md) describes; to cite this demo, use [CITATION.cff](CITATION.cff).
