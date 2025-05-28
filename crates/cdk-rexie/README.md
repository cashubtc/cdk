# CDK Rexie

[![crates.io](https://img.shields.io/crates/v/cdk-rexie.svg)](https://crates.io/crates/cdk-rexie)
[![Documentation](https://docs.rs/cdk-rexie/badge.svg)](https://docs.rs/cdk-rexie)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

[Rexie](https://github.com/SaltyAom/rexie) (IndexedDB) storage backend implementation for the Cashu Development Kit (CDK). This provides browser-based storage for web applications.

## Features

This crate provides a Rexie-based storage implementation for browser environments:
- Wallet storage
- Transaction history
- Proof tracking
- IndexedDB persistence

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-rexie = "*"
```


## WASM Support

This crate is specifically designed for use in WebAssembly environments and requires the `wasm32-unknown-unknown` target.

## License

This project is licensed under the [MIT License](../../LICENSE).