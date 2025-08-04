# CDK FFI Bindings

UniFFI bindings for the CDK (Cashu Development Kit) wallet functionality.

## Usage

```rust
use cdk_ffi::*;

// Generate a new mnemonic or use an existing one
let mnemonic = generate_mnemonic()?;

// Create wallet configuration
let config = WalletConfig {
    work_dir: "/path/to/wallet/data".to_string(),
    target_proof_count: Some(3), // or None for default
};

// Create wallet with mnemonic
let wallet = Wallet::new(
    "https://mint.example.com".to_string(),
    CurrencyUnit::Sat,
    mnemonic,
    Some("optional_passphrase".to_string()), // or None for no passphrase
    config
).await?;

// Send and receive tokens
let amount = Amount::new(100);
let token = wallet.send(amount, SendOptions::default(), None).await?;
let received = wallet.receive(Arc::new(token), ReceiveOptions::default()).await?;
```

## Building

This crate uses UniFFI proc macros (not UDL files) for generating bindings.

```bash
# Build the library
cargo build --release --package cdk-ffi

# Generate bindings
cargo run --bin uniffi-bindgen generate \
  --library target/release/libcdk_ffi.so \
  --language python \
  --out-dir target/bindings/python/
```

