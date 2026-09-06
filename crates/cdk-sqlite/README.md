# CDK SQLite

[![crates.io](https://img.shields.io/crates/v/cdk-sqlite.svg)](https://crates.io/crates/cdk-sqlite)
[![Documentation](https://docs.rs/cdk-sqlite/badge.svg)](https://docs.rs/cdk-sqlite)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

SQLite storage backend implementation for the Cashu Development Kit (CDK).

## Features

The following crate feature flags are available:

| Feature     | Default | Description                        |
|-------------|:-------:|------------------------------------|
| `wallet`    |   Yes   | Enable cashu wallet features       |
| `mint`      |   Yes   | Enable cashu mint wallet features  |
| `sqlcipher` |   No    | Enable encrypted database          |

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-sqlite = "*"
```


## Minimum Supported Rust Version (MSRV)

This crate supports Rust version **1.75.0** or higher.

To build and test with the MSRV you will need to pin the below dependency versions:

```shell
cargo update -p half --precise 2.2.1
cargo update -p home --precise 0.5.5
cargo update -p tokio --precise 1.38.1
cargo update -p serde_with --precise 3.1.0
cargo update -p reqwest --precise 0.12.4
```

## License

This project is licensed under the [MIT License](../../LICENSE).

## Connection pooling and cancellation

SQLite connections are managed by `deadpool-sqlite` 0.8.1, retaining rusqlite
0.31 and SQLCipher compatibility. File databases use up to 20 connections;
in-memory databases keep one connection and never evict it while healthy.
The acquisition wait timeout remains five seconds. Opening hooks, SQL, row
conversion, and transaction commands run through `interact()` on blocking
workers. The encryption key is applied before database pragmas access contents.
WAL, full durability, a ten-second busy timeout, and `BEGIN IMMEDIATE` are retained.

Cancelling a query does not cancel its running blocking closure. The checkout
stays unavailable until that work finishes and rollback succeeds. Cleanup has a
ten-second deadline including the outstanding work; failure detaches the
connection. If an initialized in-memory connection is lost, the backend closes
and returns errors instead of creating a new empty database. Explicitly opening
a new backend is required to start a new in-memory database.

Normal constructors and path/password arguments are unchanged. Low-level users
should migrate to `SqliteBackend` and the shared `SqlBackend`, `SqlConnection`,
and `SqlTransaction` traits; see the
[low-level API migration guide](../cdk-postgres/README.md#low-level-api-migration).
Create the backend inside a Tokio runtime. Backend handles retain the runtime
handle needed to safely dispose of blocking connections, including when dropped
by synchronous FFI callers. They do not create or own another runtime.
