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
cdk-lnbits = "*"
```

## License

This project is licensed under the [MIT License](https://github.com/cashubtc/cdk/blob/main/LICENSE).
