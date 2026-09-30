# CDK Turso

Native embedded [Turso](https://turso.tech/) storage for CDK, using the shared
CDK SQL schema and wallet implementation. This crate uses the `turso` Rust
engine; it does not connect to Turso Cloud through libSQL.

```rust,no_run
# async fn example() -> Result<(), cdk_common::database::Error> {
let wallet = cdk_turso::wallet::open("wallet.turso").await?;
let memory = cdk_turso::wallet::open(":memory:").await?;
# Ok(())
# }
```

Turso requires Rust 1.88 or newer and is outside CDK’s Rust 1.85 MSRV coverage.
The `wallet` feature is enabled by default. Operations in a database pool are
serialized, and transactions acquire the write lock before reading. Database
files and WAL files must be kept together. SQLCipher passwords are not supported.

The CLI supports `--engine turso` when built with `--features turso` and stores
its wallet in `cdk-cli.turso` inside the configured work directory.

Run the shared wallet database contract tests with `cargo test -p cdk-turso`.
