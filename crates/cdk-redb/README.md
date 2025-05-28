# CDK Redb

[![crates.io](https://img.shields.io/crates/v/cdk-redb.svg)](https://crates.io/crates/cdk-redb)
[![Documentation](https://docs.rs/cdk-redb/badge.svg)](https://docs.rs/cdk-redb)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

[Redb](https://github.com/cberner/redb) storage backend implementation for the Cashu Development Kit (CDK).

## Features

This crate provides a Redb-based storage implementation for:
- Wallet storage
- Mint storage
- Proof tracking
- Transaction history

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-redb = "*"
```

## Example

```rust
use cdk_redb::Store;

// Create a new Redb store
let store = Store::new("wallet.redb")?;

// Use the store with a CDK wallet or mint
let wallet = Wallet::new(mint_url, unit, store, &seed, None)?;
```

## License

This project is licensed under the [MIT License](../../LICENSE).