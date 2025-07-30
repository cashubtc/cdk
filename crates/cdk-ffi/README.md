# CDK FFI Bindings

This crate provides UniFFI bindings for the CDK (Cashu Development Kit) wallet functionality, enabling the use of CDK from other programming languages like Python, Swift, Kotlin, and others.

## Features

- **Wallet Management**: Create and manage Cashu wallets
- **Balance Operations**: Check wallet balances (total, pending, reserved)  
- **Send/Receive**: Send and receive Cashu tokens
- **Mint Operations**: Interact with Cashu mints
- **Token Verification**: Verify DLEQ proofs and token validity
- **Restore Functionality**: Restore wallets from seed
- **Procedural Macros**: Uses UniFFI proc macros for better type safety and compile-time validation

## Basic Usage

### Creating a Wallet

```rust
use cdk_ffi::*;

// Generate a random seed
let seed = generate_seed();

// Create a new wallet
let wallet = Wallet::new(
    "https://mint.example.com".to_string(),
    CurrencyUnit::Sat,
    seed,
    Some(3) // target proof count
)?;
```

### Sending Tokens

```rust
let amount = Amount::new(100); // 100 sats
let options = SendOptions { offline: false };
let memo = Some("Payment for coffee".to_string());

let token = wallet.send(amount, options, memo)?;
println!("Token: {}", token.token);
```

### Receiving Tokens

```rust
let token = Token { token: "cashuA...".to_string() };
let options = ReceiveOptions { check_spendable: true };

let received_amount = wallet.receive(token, options)?;
println!("Received: {} sats", received_amount.value);
```

### Checking Balance

```rust
let balance = wallet.total_balance()?;
println!("Balance: {} sats", balance.value);
```

## Building

This crate uses UniFFI proc macros (not UDL files) for generating bindings. 

First, build the library:
```bash
cargo build --release
```

Then generate bindings for your target language:

```bash
# For Python
cargo run --bin uniffi-bindgen generate --library target/release/libcdk_ffi.so --language python --out-dir bindings/

# For Swift  
cargo run --bin uniffi-bindgen generate --library target/release/libcdk_ffi.so --language swift --out-dir bindings/

# For Kotlin
cargo run --bin uniffi-bindgen generate --library target/release/libcdk_ffi.so --language kotlin --out-dir bindings/
```

Or use the provided example:
```bash
cargo run --example generate_bindings
```

## Current Status

This is a work-in-progress implementation. The basic wallet functionality is implemented, but some advanced features may be missing or simplified for FFI compatibility.

### Known Limitations

- PreparedSend functionality is simplified due to serialization constraints
- Some advanced configuration options are not exposed
- Error handling could be more granular

## Contributing

Contributions are welcome! Please ensure that any new functionality maintains compatibility with the UniFFI interface and follows the existing patterns.