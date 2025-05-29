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
