# Cashu

[![crates.io](https://img.shields.io/crates/v/cashu.svg)](https://crates.io/crates/cashu)
[![Documentation](https://docs.rs/cashu/badge.svg)](https://docs.rs/cashu)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

A Rust implementation of the [Cashu](https://github.com/cashubtc) protocol, providing the core functionality for Cashu e-cash operations.

## Overview

Cashu is a privacy-focused, token-based digital cash system built on the Bitcoin Lightning Network. This crate implements the core Cashu protocol as defined in the [Cashu NUTs (Notation, Usage, and Terminology)](https://github.com/cashubtc/nuts/).

## Features

- **Cryptographic Operations**: Secure blind signatures and verification
- **Token Management**: Creation, validation, and manipulation of Cashu tokens
- **NUTs Implementation**: Support for the core Cashu protocol specifications
- **Type-safe API**: Strongly-typed interfaces for working with Cashu primitives

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
cashu = "0.8.1"
```

### Basic Example

```rust
use cashu::amount::Amount;
use cashu::nuts::nut00::Token;
use std::str::FromStr;

// Parse a Cashu token from a string
let token_str = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vZXhhbXBsZS5jb20iLCJwcm9vZnMiOlt7ImlkIjoiMDAwMDAwMDAwMDAwMDAwMCIsImFtb3VudCI6MX1dfV19";
let token = Token::from_str(token_str).expect("Valid token");

// Get the total amount
let amount: Amount = token.amount();
println!("Token amount: {}", amount);
```

## Implemented NUTs

This crate implements the core Cashu protocol specifications (NUTs):

- **NUT-00**: Cryptography and Models
- **NUT-01**: Mint public keys
- **NUT-02**: Keysets and fees
- **NUT-03**: Swapping tokens
- **NUT-04**: Minting tokens
- **NUT-05**: Melting tokens
- **NUT-06**: Mint info
- And more...

## License

This project is licensed under the [MIT License](../LICENSE).
