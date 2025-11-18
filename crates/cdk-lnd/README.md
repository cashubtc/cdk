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

### Environment Variables

All configuration can be set via environment variables:

| Variable | Description | Required |
|----------|-------------|----------|
| `CDK_MINTD_LN_BACKEND` | Set to `lnd` | Yes |
| `CDK_MINTD_LND_ADDRESS` | LND gRPC address (e.g., `https://localhost:10009`) | Yes |
| `CDK_MINTD_LND_CERT_FILE` | Path to LND TLS certificate | Yes |
| `CDK_MINTD_LND_MACAROON_FILE` | Path to LND macaroon file | Yes |
| `CDK_MINTD_LND_FEE_PERCENT` | Fee percentage (default: `0.02`) | No |
| `CDK_MINTD_LND_RESERVE_FEE_MIN` | Minimum fee in sats (default: `2`) | No |

### Example

```bash
export CDK_MINTD_LN_BACKEND=lnd
export CDK_MINTD_LND_ADDRESS=https://127.0.0.1:10009
export CDK_MINTD_LND_CERT_FILE=/home/user/.lnd/tls.cert
export CDK_MINTD_LND_MACAROON_FILE=/home/user/.lnd/data/chain/bitcoin/mainnet/admin.macaroon
cdk-mintd
```

## Minimum Supported Rust Version (MSRV)

This crate supports Rust version **1.75.0** or higher.

## License

This project is licensed under the [MIT License](../../LICENSE).