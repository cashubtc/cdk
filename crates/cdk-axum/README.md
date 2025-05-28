# CDK Axum

[![crates.io](https://img.shields.io/crates/v/cdk-axum.svg)](https://crates.io/crates/cdk-axum)
[![Documentation](https://docs.rs/cdk-axum/badge.svg)](https://docs.rs/cdk-axum)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

Axum web server implementation for the Cashu Development Kit (CDK). This provides the HTTP API for Cashu mints.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-axum = "*"
```

## Example

```rust
use cdk_axum::MintServer;

// Initialize the mint server
let mint_server = MintServer::new(
    mint,           // Your configured CDK mint
    Some(keysets), // Optional keysets configuration
    options,       // Server options
).await?;

// Start the server
mint_server.serve().await?;
```

## License

This project is licensed under the [MIT License](../../LICENSE).