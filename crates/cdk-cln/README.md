# CDK CLN

[![crates.io](https://img.shields.io/crates/v/cdk-cln.svg)](https://crates.io/crates/cdk-cln)
[![Documentation](https://docs.rs/cdk-cln/badge.svg)](https://docs.rs/cdk-cln)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

Core Lightning (CLN) backend implementation for the Cashu Development Kit (CDK).

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-cln = "*"
```

## Example

```rust
use cdk_cln::ClnLightning;

// Initialize CLN client
let cln = ClnLightning::new(
    "unix://path/to/lightning-rpc",  // Socket path
    None,                           // Optional network
).await?;
```

## License

This project is licensed under the [MIT License](../../LICENSE).