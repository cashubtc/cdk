# CDK LND

[![crates.io](https://img.shields.io/crates/v/cdk-lnd.svg)](https://crates.io/crates/cdk-lnd)
[![Documentation](https://docs.rs/cdk-lnd/badge.svg)](https://docs.rs/cdk-lnd)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

LND (Lightning Network Daemon) backend implementation for the Cashu Development Kit (CDK).

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-lnd = "*"
```

## Example

```rust
use cdk_lnd::LndLightning;

// Initialize LND client
let lnd = LndLightning::new(
    "https://localhost:8080",    // LND REST API endpoint
    "path/to/tls.cert",         // TLS cert path
    "path/to/macaroon",         // Macaroon path
).await?;
```

## Minimum Supported Rust Version (MSRV)

This crate supports Rust version **1.75.0** or higher.

## License

This project is licensed under the [MIT License](../../LICENSE).