# CDK LNBits

[![crates.io](https://img.shields.io/crates/v/cdk-lnbits.svg)](https://crates.io/crates/cdk-lnbits)
[![Documentation](https://docs.rs/cdk-lnbits/badge.svg)](https://docs.rs/cdk-lnbits)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

LNBits backend implementation for the Cashu Development Kit (CDK). This provides integration with [LNBits](https://lnbits.com/) for Lightning Network functionality.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-lnbits = "*"
```

## Example

```rust
use cdk_lnbits::LnbitsLightning;

// Initialize LNBits client
let lnbits = LnbitsLightning::new(
    "https://legend.lnbits.com",  // LNBits instance URL
    "your-admin-key",            // Admin key
    None,                        // Optional invoice read key
).await?;
```

## License

This project is licensed under the [MIT License](../../LICENSE).