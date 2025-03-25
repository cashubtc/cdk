# Cashu

[![crates.io](https://img.shields.io/crates/v/cashu.svg)](https://crates.io/crates/cashu)
[![Documentation](https://docs.rs/cashu/badge.svg)](https://docs.rs/cashu)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

A Rust implementation of the [Cashu](https://github.com/cashubtc) protocol, providing the core functionality for Cashu e-cash operations.

## Overview

 This crate implements the core Cashu protocol as defined in the [Cashu NUTs (Notation, Usage, and Terminology)](https://github.com/cashubtc/nuts/).

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

### Mandatory

| NUT #    | Description                       |
|----------|-----------------------------------|
| [00][00] | Cryptography and Models           |
| [01][01] | Mint public keys                  |
| [02][02] | Keysets and fees                  |
| [03][03] | Swapping tokens                   |
| [04][04] | Minting tokens                    |
| [05][05] | Melting tokens                    |
| [06][06] | Mint info                         |

### Optional

| # | Description | Status
| --- | --- | --- |
| [07][07] | Token state check | :heavy_check_mark: |
| [08][08] | Overpaid Lightning fees | :heavy_check_mark: |
| [09][09] | Signature restore | :heavy_check_mark: |
| [10][10] | Spending conditions | :heavy_check_mark: |
| [11][11] | Pay-To-Pubkey (P2PK) | :heavy_check_mark: |
| [12][12] | DLEQ proofs | :heavy_check_mark: |
| [13][13] | Deterministic secrets | :heavy_check_mark: |
| [14][14] | Hashed Timelock Contracts (HTLCs) | :heavy_check_mark: |
| [15][15] | Partial multi-path payments (MPP) | :heavy_check_mark: |
| [16][16] | Animated QR codes | :x: |
| [17][17] | WebSocket subscriptions  | :heavy_check_mark: |
| [18][18] | Payment Requests  | :heavy_check_mark: |
| [19][19] | Cached responses  | :heavy_check_mark: |
| [20][20] | Signature on Mint Quote  | :heavy_check_mark: |

## License

This project is licensed under the [MIT License](https://github.com/cashubtc/cdk/blob/main/LICENSE).

[00]: https://github.com/cashubtc/nuts/blob/main/00.md
[01]: https://github.com/cashubtc/nuts/blob/main/01.md
[02]: https://github.com/cashubtc/nuts/blob/main/02.md
[03]: https://github.com/cashubtc/nuts/blob/main/03.md
[04]: https://github.com/cashubtc/nuts/blob/main/04.md
[05]: https://github.com/cashubtc/nuts/blob/main/05.md
[06]: https://github.com/cashubtc/nuts/blob/main/06.md
[07]: https://github.com/cashubtc/nuts/blob/main/07.md
[08]: https://github.com/cashubtc/nuts/blob/main/08.md
[09]: https://github.com/cashubtc/nuts/blob/main/09.md
[10]: https://github.com/cashubtc/nuts/blob/main/10.md
[11]: https://github.com/cashubtc/nuts/blob/main/11.md
[12]: https://github.com/cashubtc/nuts/blob/main/12.md
[13]: https://github.com/cashubtc/nuts/blob/main/13.md
[14]: https://github.com/cashubtc/nuts/blob/main/14.md
[15]: https://github.com/cashubtc/nuts/blob/main/15.md
[16]: https://github.com/cashubtc/nuts/blob/main/16.md
[17]: https://github.com/cashubtc/nuts/blob/main/17.md
[18]: https://github.com/cashubtc/nuts/blob/main/18.md
[19]: https://github.com/cashubtc/nuts/blob/main/19.md
[20]: https://github.com/cashubtc/nuts/blob/main/20.md
