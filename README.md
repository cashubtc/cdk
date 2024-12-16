> **Warning**
> This project is in early development, it does however work with real sats! Always use amounts you don't mind losing.


# Cashu Development Kit

CDK is a collection of rust crates for [Cashu](https://github.com/cashubtc) wallets and mints written in Rust.

**ALPHA** This library is in early development, the api will change and should be used with caution.


## Project structure

The project is split up into several crates in the `crates/` directory:

* Libraries:
    * [**cdk**](./crates/cdk/): Rust implementation of Cashu protocol.
    * [**cdk-sqlite**](./crates/cdk-sqlite/): SQLite Storage backend.
    * [**cdk-redb**](./crates/cdk-redb/): Redb Storage backend.
    * [**cdk-rexie**](./crates/cdk-rexie/): Rexie Storage backend for browsers.
    * [**cdk-axum**](./crates/cdk-axum/): Axum webserver for mint.
    * [**cdk-cln**](./crates/cdk-cln/): CLN Lightning backend for mint.
    * [**cdk-lnd**](./crates/cdk-lnd/): Lnd Lightning backend for mint.
    * [**cdk-strike**](./crates/cdk-strike/): Strike Lightning backend for mint.
    * [**cdk-lnbits**](./crates/cdk-lnbits/): [LNbits](https://lnbits.com/) Lightning backend for mint.
    * [**cdk-phoenixd**](./crates/cdk-phoenixd/): Phoenixd Lightning backend for mint.
    * [**cdk-fake-wallet**](./crates/cdk-fake-wallet/): Fake Lightning backend for mint. To be used only for testing, quotes are automatically filled.
* Binaries:
    * [**cdk-cli**](./crates/cdk-cli/): Cashu wallet CLI.
    * [**cdk-mintd**](./crates/cdk-mintd/): Cashu Mint Binary.


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


## Bindings

Experimental JS bindings can be found in the [bindings repository](https://github.com/thesimplekid/cdk-js).

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
