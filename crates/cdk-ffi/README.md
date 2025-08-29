# CDK FFI Bindings

UniFFI bindings for the CDK (Cashu Development Kit) wallet functionality, providing async wallet operations for mobile and desktop applications.

## Usage

The CDK FFI provides async bindings for wallet operations. All wallet methods are async and use UniFFI's automatic runtime management.

```rust
use cdk_ffi::*;
use std::sync::Arc;

// Generate a new mnemonic or use an existing one
let mnemonic = generate_mnemonic()?;

// Create a database instance
let database = WalletSqliteDatabase::new("/path/to/wallet/data".to_string()).await?;

// Create wallet configuration
let config = WalletConfig {
    target_proof_count: Some(3), // or None for default
};

// Create wallet with mnemonic (no passphrase used)
let wallet = Wallet::new(
    "https://mint.example.com".to_string(),
    CurrencyUnit::Sat,
    mnemonic,
    database,
    config
).await?;

// Get wallet balance
let balance = wallet.total_balance().await?;

// Receive tokens
let received = wallet.receive(token, ReceiveOptions::default()).await?;
```

## Building

This crate uses UniFFI proc macros (not UDL files) for generating bindings.

```bash
# Build the library
cargo build --release --package cdk-ffi
```

## Supported Language Bindings

Pre-built language bindings are available in separate repositories:

- **Swift**: [cdk-swift](https://github.com/cashubtc/cdk-swift) - iOS and macOS bindings
- **Kotlin**: [cdk-kotlin](https://github.com/cashubtc/cdk-kotlin) - Android and JVM bindings  
- **Python**: [cdk-python](https://github.com/cashubtc/cdk-python) - Python bindings

These repositories contain the generated bindings and provide language-specific packaging and distribution.

## Features

- **Async/Await Support**: All wallet operations are async and integrate with native async runtimes
- **Mobile Optimized**: Runtime configured for mobile battery efficiency and performance
- **Cross-Platform**: Works on iOS, Android, and desktop platforms
- **Type Safety**: Full type safety with automatic conversion between Rust and foreign types

