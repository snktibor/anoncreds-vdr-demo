# AnonCreds v2 VDR demo

Stage 1 exercises the credx happy path in one Rust process:

1. create a credential schema;
2. create issuer keys and public credential-definition material;
3. issue a credential to a holder;
4. create an in-memory presentation request;
5. build a presentation; and
6. verify it.

The demo intentionally performs no revocation check and has no Fabric, VDR, HTTP,
or wire serialization. credx requires every directly signed credential to contain
a revocation claim, so the credential has an opaque ID and `sign_credential`
initializes its handle internally. Stage 1 creates no `RevocationStatement`, calls
no revoke or witness-refresh API, and performs no accumulator verification.

The dependency is pinned to credx commit
`691297a7f9ffcc1f51a5d30741086402d64544c9`.

Run it with:

```console
cargo run
```

The reusable crypto boundaries live in `src/lib.rs`; `src/main.rs` only orchestrates
the runnable demonstration.
