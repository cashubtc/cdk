# CDK LND

[![crates.io](https://img.shields.io/crates/v/cdk-lnd.svg)](https://crates.io/crates/cdk-lnd)
[![Documentation](https://docs.rs/cdk-lnd/badge.svg)](https://docs.rs/cdk-lnd)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

LND (Lightning Network Daemon) backend implementation for the Cashu Development Kit (CDK).

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-lnd = "*"
```

## Configuration for cdk-mintd

### Config File

```toml
[ln]
ln_backend = "lnd"

[lnd]
address = "https://localhost:10009"
cert_file = "/path/to/.lnd/tls.cert"
macaroon_file = "/path/to/.lnd/data/chain/bitcoin/mainnet/admin.macaroon"
fee_percent = 0.02       # Optional, defaults to 2%
reserve_fee_min = 2      # Optional, defaults to 2 sats
```

### Import and Start

Add the section above to a complete `mint.toml`, then explicitly import it into
the mint database before the first start:

```bash
cdk-mintd config validate --file mint.toml
cdk-mintd config init --file mint.toml
cdk-mintd
```

Environment variables no longer override LND settings at daemon startup. To
change them later, edit the complete file, run
`cdk-mintd config apply --file mint.toml` through the management RPC (or add
`--offline` while the daemon is stopped), and restart. See the
[`cdk-mintd` configuration guide](../cdk-mintd/README.md#configuration).

## Minimum Supported Rust Version (MSRV)

This crate supports Rust version **1.75.0** or higher.

## License

This project is licensed under the [MIT License](../../LICENSE).
