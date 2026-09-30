# CDK Turso

Native embedded [Turso](https://turso.tech/) storage for CDK, using the shared
CDK SQL schema and wallet, mint, and authentication implementations. This crate
uses the `turso` Rust engine; it does not connect to Turso Cloud through libSQL.

```rust,no_run
# async fn example() -> Result<(), cdk_common::database::Error> {
let wallet = cdk_turso::wallet::open("wallet.turso").await?;
let memory = cdk_turso::wallet::open(":memory:").await?;
# Ok(())
# }
```

Turso requires Rust 1.88 or newer and is outside CDK’s Rust 1.85 MSRV coverage.
The `wallet` and `mint` features are enabled by default. Operations in a database
pool are serialized, and transactions acquire the write lock before reading. Database
files and WAL files must be kept together. SQLCipher passwords are not supported.

The CLI supports `--engine turso` when built with `--features turso` and stores
its wallet in `cdk-cli.turso` inside the configured work directory.

Open mint storage with `cdk_turso::mint::open("mint.turso").await?` and a
separate authentication database with `cdk_turso::mint::open_auth("auth.turso").await?`.
The mint includes proof, quote, keyset, saga, completed-operation, and configuration
storage from the shared SQL implementation.

Build `cdk-mintd` with `--features turso` and set `[database] engine = "turso"`
(or `CDK_MINTD_DATABASE=turso`). Data is stored in `cdk-mintd.turso` and, when
authentication is enabled, `cdk-mintd-auth.turso` in the work directory.

Run both shared database contract suites with `cargo test -p cdk-turso`.
Pure integration tests support `CDK_TEST_DB_TYPE=turso`.
