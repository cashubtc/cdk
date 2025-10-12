> [!Warning]
> This project is in early development, it does however work with real sats! Always use amounts you don't mind losing.

[![crates.io](https://img.shields.io/crates/v/cdk.svg)](https://crates.io/crates/cdk) [![Documentation](https://docs.rs/cdk/badge.svg)](https://docs.rs/cdk) [![License](https://img.shields.io/github/license/cashubtc/cdk)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

# Cashu Development Kit

CDK is a collection of rust crates for [Cashu](https://github.com/cashubtc) wallets and mints written in Rust.

**ALPHA** This library is in early development, the api will change and should be used with caution.


## Project structure

The project is split up into several crates in the `crates/` directory:

* Libraries:
    * [**cashu**](./crates/cashu/): Core Cashu protocol implementation.
    * [**cdk**](./crates/cdk/): Rust implementation of Cashu protocol.
    * [**cdk-sqlite**](./crates/cdk-sqlite/): SQLite Storage backend.
    * [**cdk-postgres**](./crates/cdk-postgres/): PostgreSQL Storage backend.
    * [**cdk-redb**](./crates/cdk-redb/): Redb Storage backend.
    * [**cdk-axum**](./crates/cdk-axum/): Axum webserver for mint.
    * [**cdk-cln**](./crates/cdk-cln/): CLN Lightning backend for mint.
    * [**cdk-lnd**](./crates/cdk-lnd/): Lnd Lightning backend for mint.
    * [**cdk-lnbits**](./crates/cdk-lnbits/): [LNbits](https://lnbits.com/) Lightning backend for mint. **Note: Only LNBits v1 API is supported.**
    * [**cdk-ldk-node**](./crates/cdk-ldk-node/): LDK Node Lightning backend for mint.
    * [**cdk-fake-wallet**](./crates/cdk-fake-wallet/): Fake Lightning backend for mint. To be used only for testing, quotes are automatically filled.
    * [**cdk-common**](./crates/cdk-common/): Common utilities and shared code.
    * [**cdk-sql-common**](./crates/cdk-sql-common/): Common SQL utilities for storage backends.
    * [**cdk-signatory**](./crates/cdk-signatory/): Signing utilities and cryptographic operations.
    * [**cdk-payment-processor**](./crates/cdk-payment-processor/): Payment processing functionality.
    * [**cdk-prometheus**](./crates/cdk-prometheus/): Prometheus metrics integration.
    * [**cdk-ffi**](./crates/cdk-ffi/): Foreign Function Interface bindings for other languages.
    * [**cdk-integration-tests**](./crates/cdk-integration-tests/): Integration test suite.
    * [**cdk-mint-rpc**](./crates/cdk-mint-rpc/): Mint management gRPC server and cli.
* Binaries:
    * [**cdk-cli**](./crates/cdk-cli/): Cashu wallet CLI.
    * [**cdk-mintd**](./crates/cdk-mintd/): Cashu Mint Binary.
    * [**cdk-mint-cli**](./crates/cdk-mint-rpc/): Cashu Mint management gRPC client cli.


## Development 

For a guide to settings up a development environment see [DEVELOPMENT.md](./DEVELOPMENT.md)

## Implemented [NUTs](https://github.com/cashubtc/nuts/):

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
| [21][21] | Clear Authentication | :heavy_check_mark: |
| [22][22] | Blind Authentication  | :heavy_check_mark: |
| [23][23] | Payment Method: BOLT11 | :heavy_check_mark: |
| [25][25] | Payment Method: BOLT12 | :heavy_check_mark: |


## License

Code is under the [MIT License](LICENSE)

## Contribution

All contributions are welcome.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, shall be licensed as above, without any additional terms or conditions.

Please see the [development guide](DEVELOPMENT.md).


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
[21]: https://github.com/cashubtc/nuts/blob/main/21.md
[22]: https://github.com/cashubtc/nuts/blob/main/22.md
[23]: https://github.com/cashubtc/nuts/blob/main/23.md
[25]: https://github.com/cashubtc/nuts/blob/main/25.md
