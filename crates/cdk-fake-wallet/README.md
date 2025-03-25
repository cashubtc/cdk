# CDK Fake Wallet

[![crates.io](https://img.shields.io/crates/v/cdk-fake-wallet.svg)](https://crates.io/crates/cdk-fake-wallet) [![Documentation](https://docs.rs/cdk-fake-wallet/badge.svg)](https://docs.rs/cdk-fake-wallet)

The CDK Fake Wallet is a component of the [Cashu Development Kit](https://github.com/cashubtc/cdk) that provides a simulated Lightning Network backend for testing Cashu mints.

## Overview

This crate implements the `MintPayment` trait with a fake Lightning backend that automatically completes payments without requiring actual Lightning Network transactions. It's designed for development and testing purposes only.

## Features

- Simulated Lightning Network payments
- Automatic completion of payment quotes
- Support for testing mint functionality without real funds
- Implementation of the standard `MintPayment` interface

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-fake-wallet = "0.8.1"
```

### Example

```rust
use cdk_fake_wallet::FakeWallet;
use cdk_common::payment::MintPayment;
use serde_json::Value;

async fn setup_fake_wallet() -> FakeWallet {
    FakeWallet::default()
}

async fn create_test_invoice(wallet: &FakeWallet, amount_msat: u64) {
    let invoice = wallet.create_invoice(
        amount_msat,
        "Test payment",
        Some(600),
    ).await.unwrap();
    
    println!("Test invoice: {}", invoice.payment_request);
    
    // In the fake wallet, payments are automatically marked as paid
    // No need to actually pay the invoice
}
```

## Warning

**This is for testing purposes only!** 

The fake wallet should never be used in production environments as it does not perform actual Lightning Network transactions. It simply simulates the payment flow by automatically marking invoices as paid.

## License

This project is licensed under the [MIT License](../../LICENSE).
