# CDK SQLite

[![crates.io](https://img.shields.io/crates/v/cdk-sqlite.svg)](https://crates.io/crates/cdk-sqlite)
[![Documentation](https://docs.rs/cdk-sqlite/badge.svg)](https://docs.rs/cdk-sqlite)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

SQLite storage backend implementation for the Cashu Development Kit (CDK).

## Closing a database

Stop wallet or mint operations, release any transactions, then call
`database.close().await` before backing up or moving its SQLite file. Closing
permanently rejects new operations through every clone of that database handle,
waits for borrowed connections and pending cleanup, and closes connections on
blocking workers. The close can be retried if its caller is cancelled.

Other independently opened connections to the same file must also be closed
before relying on SQLite's final WAL checkpoint for a main-file-only backup.
For live backups, use a SQLite-aware backup mechanism instead of copying files.

Dropping the last handle inside Tokio schedules connection disposal but does not
wait for it. Outside a Tokio runtime, disposal is synchronous. Explicit close is
the completion boundary; dropping a handle alone is not a backup boundary.

The FFI `WalletSqliteDatabase` also exposes async `close()` with the same semantics.

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

SQLite connections are pooled by Deadpool through a small rusqlite manager in
this crate, so the supported rusqlite range and SQLCipher compatibility are
unchanged. File databases use up to 20 connections; in-memory databases keep
one connection and never evict it while healthy. Returning a connection does
not run a health query, so a cancelled checkout cannot discard it.
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
Create the backend inside a Tokio runtime. Blocking work runs on the caller's
runtime; backend handles retain the creating runtime's handle as a fallback for
disposing of connections when dropped by synchronous FFI callers. They do not
create or own another runtime.

`close()` rejects new checkouts and waits until every connection, including one
detached after failed cleanup, has finished closing. Await it before copying a
database file.

Cleanup diagnostics use the same outcome/disposal counters and structured warning
fields as PostgreSQL; see [cleanup metrics](../cdk-postgres/README.md#cancellation-and-errors).
