# CDK LNbits

[![crates.io](https://img.shields.io/crates/v/cdk-lnbits.svg)](https://crates.io/crates/cdk-lnbits) [![Documentation](https://docs.rs/cdk-lnbits/badge.svg)](https://docs.rs/cdk-lnbits)

The CDK LNbits crate is a component of the [Cashu Development Kit](https://github.com/cashubtc/cdk) that provides integration with [LNbits](https://lnbits.com/) as a Lightning Network backend for Cashu mints.

## Overview

This crate implements the `MintPayment` trait for LNbits, allowing Cashu mints to use LNbits as a payment backend for handling Lightning Network transactions.

## Features

- Create and pay Lightning invoices via LNbits
- Handle webhook callbacks for payment notifications
- Manage fee reserves for Lightning transactions
- Support for invoice descriptions
- MPP (Multi-Path Payment) support

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-lnbits = "0.8.1"
```

### Example

```rust
use cdk_lnbits::{LNbits, LNBitsConfig};
use cdk_common::payment::MintPayment;

async fn setup_lnbits() -> LNbits {
    let config = LNBitsConfig {
        admin_api_key: "your_admin_key".to_string(),
        invoice_api_key: "your_invoice_key".to_string(),
        lnbits_api: "https://legend.lnbits.com".to_string(),
        fee_percent: 0.1,
        reserve_fee_min: 1000.into(),
    };
    
    LNbits::new(config, "https://your-mint.com/webhook").await.unwrap()
}

async fn create_invoice(lnbits: &LNbits, amount_msat: u64) {
    let invoice = lnbits.create_invoice(
        amount_msat,
        "Payment for Cashu tokens",
        Some(600),
    ).await.unwrap();
    
    println!("Invoice: {}", invoice.payment_request);
}
```

## License

This project is licensed under the [MIT License](../../LICENSE).
