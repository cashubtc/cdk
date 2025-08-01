# CDK FFI Bindings

This crate provides UniFFI bindings for the CDK (Cashu Development Kit) wallet functionality, enabling the use of CDK from other programming languages like Python, Swift, Kotlin, Ruby, and others.

## Features

- **Async Wallet Operations**: All wallet methods are properly async
- **Wallet Management**: Create and manage Cashu wallets with builder pattern
- **Balance Operations**: Check wallet balances (total, pending, reserved)
- **Send/Receive**: Send and receive Cashu tokens with structured options
- **Mint Operations**: Interact with Cashu mints with proper quote handling
- **Token Management**: Comprehensive token and proof handling with structured types
- **Spending Conditions**: Support for P2PK and HTLC spending conditions
- **Mint Information**: Structured mint info with NUT support details
- **Token Verification**: Verify DLEQ proofs and token validity
- **Restore Functionality**: Restore wallets from seed
- **Type Safety**: Proper FFI types with conversion logic, no JSON string parsing

## Basic Usage

### Creating a Wallet

```rust
use cdk_ffi::*;

// Generate a random seed
let seed = generate_seed();

// Create a new wallet (async)
let wallet = Wallet::new(
    "https://mint.example.com".to_string(),
    CurrencyUnit::Sat,
    seed,
    Some(3) // target proof count
).await?;

// Or use the builder pattern
let wallet = WalletBuilder::new()?
    .mint_url("https://mint.example.com".to_string())?
    .unit(CurrencyUnit::Sat)
    .seed(seed)
    .target_proof_count(3)
    .build().await?;
```

### Sending Tokens

```rust
let amount = Amount::new(100); // 100 sats
let options = SendOptions {
    memo: Some(SendMemo {
        memo: "Payment for coffee".to_string(),
        include_memo: true,
    }),
    conditions: None,
    amount_split_target: SplitTarget::None,
    send_kind: SendKind::OnlineExact,
    include_fee: false,
    max_proofs: None,
    metadata: std::collections::HashMap::new(),
};
let memo = Some("Payment for coffee".to_string());

let token = wallet.send(amount, options, memo).await?;
println!("Token: {}", token.to_string());
```

### Receiving Tokens

```rust
let token = Token::from_string("cashuA...".to_string())?;
let options = ReceiveOptions {
    amount_split_target: SplitTarget::None,
    p2pk_signing_keys: Vec::new(),
    preimages: Vec::new(),
    metadata: std::collections::HashMap::new(),
};

let received_amount = wallet.receive(Arc::new(token), options).await?;
println!("Received: {} sats", received_amount.value);
```

### Checking Balance

```rust
let balance = wallet.total_balance().await?;
println!("Balance: {} sats", balance.value);

let pending = wallet.total_pending_balance().await?;
let reserved = wallet.total_reserved_balance().await?;
println!("Pending: {} sats, Reserved: {} sats", pending.value, reserved.value);
```

## Building

This crate uses UniFFI proc macros (not UDL files) for generating bindings.

### Using Just (Recommended)

This repository includes a `justfile` in the root with convenient FFI commands (all prefixed with `ffi-`):

```bash
# Build the library and generate bindings for all languages
just ffi-generate-all

# Generate bindings for specific language
just ffi-generate-python
just ffi-generate-swift  
just ffi-generate-kotlin
just ffi-generate-ruby

# Quick development cycle
just ffi-dev-python      # Build debug + generate Python bindings
just ffi-test-python     # Test Python bindings
just ffi-watch-python    # Auto-regenerate on file changes

# Development cycle
just ffi-dev-cycle       # format + check + test + generate
```

### Manual Build

If you prefer manual commands:

```bash
# Build the library
cargo build --release --package cdk-ffi

# Generate bindings (replace .so with .dylib on macOS)
cargo run --bin uniffi-bindgen generate \
  --library target/release/libcdk_ffi.so \
  --language python \
  --out-dir target/bindings/python/
```

### Example Generator

```bash
cargo run --example generate_bindings
```

## Current Status

The FFI bindings are feature-complete and production-ready with the following capabilities:

### Recent Improvements

- **Full Async Support**: All wallet operations are properly async (no more blocking runtime calls)
- **Structured Types**: Proper FFI types for all major components (no JSON string parsing)
- **Spending Conditions**: Complete P2PK and HTLC spending conditions support
- **Mint Info**: Structured mint information with NUT capabilities
- **Type Safety**: Comprehensive From/Into conversions with proper error handling
- **Melted Type**: Automatic conversion for melt operations

### Supported Features

- ✅ Async wallet operations
- ✅ Structured send/receive options
- ✅ Spending conditions (P2PK, HTLC)
- ✅ Mint information and capabilities
- ✅ Token and proof management
- ✅ Balance checking (total, pending, reserved)
- ✅ Wallet builder pattern
- ✅ DLEQ proof verification
- ✅ Wallet restoration
- ✅ Quote management (mint/melt)

### Language Support

- 🐍 **Python**: Full support with cffi
- 🍎 **Swift**: iOS/macOS integration ready
- 🎯 **Kotlin**: Android integration ready  
- 💎 **Ruby**: Complete bindings
- 🦀 **Rust**: Native FFI crate

### Getting Started

1. **Install Just**: `cargo install just`
2. **Build and test**: `just ffi-dev-cycle`
3. **Generate bindings**: `just ffi-generate-all`
4. **Test specific language**: `just ffi-test-python`

### Development Workflow

```bash
# Watch for changes and auto-regenerate Python bindings
just ffi-watch-python

# Quick development iteration
just ffi-dev-python

# Full CI check
just ffi-ci-check
```

## Advanced Usage

### Spending Conditions

```rust
// P2PK spending condition
let p2pk_condition = SpendingConditions::P2PK {
    pubkey: "02abc123...".to_string(),
    conditions: Some(Conditions {
        locktime: Some(1640000000),
        pubkeys: vec![],
        refund_keys: vec![],
        num_sigs: Some(1),
        sig_flag: 0, // SigInputs
        num_sigs_refund: Some(1),
    }),
};

// Use in mint operation
let proofs = wallet.mint(quote_id, SplitTarget::None, Some(p2pk_condition)).await?;
```

### Mint Information

```rust
let mint_info = wallet.get_mint_info().await?;
if let Some(info) = mint_info {
    println!("Mint: {}", info.name.unwrap_or("Unknown".to_string()));
    println!("NUT07 supported: {}", info.nuts.nut07_supported);
    println!("Supported units: {:?}", info.nuts.mint_units);
}
```

## Contributing

Contributions are welcome! Please ensure that:

- All new functionality maintains UniFFI compatibility
- Proper async patterns are used (no blocking runtime calls)
- Structured types are preferred over string-based APIs
- Comprehensive tests are included
- Documentation is updated

### Code Style

- Use `just ffi-format` to format FFI code
- Run `just ffi-dev-cycle` before submitting PRs
- Ensure `just ffi-ci-check` passes