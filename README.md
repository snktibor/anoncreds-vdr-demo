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
