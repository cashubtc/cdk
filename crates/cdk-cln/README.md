# CDK CLN

[![crates.io](https://img.shields.io/crates/v/cdk-cln.svg)](https://crates.io/crates/cdk-cln)
[![Documentation](https://docs.rs/cdk-cln/badge.svg)](https://docs.rs/cdk-cln)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

Core Lightning (CLN) backend implementation for the Cashu Development Kit (CDK).

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-cln = "*"
```

## Configuration for cdk-mintd

### Config File

```toml
[ln]
ln_backend = "cln"

[cln]
rpc_path = "/path/to/.lightning/bitcoin/lightning-rpc"
bolt12 = true            # Optional, defaults to true
fee_percent = 0.02       # Optional, defaults to 2%
reserve_fee_min = 2      # Optional, defaults to 2 sats
```

### Environment Variables

All configuration can be set via environment variables:

| Variable | Description | Required |
|----------|-------------|----------|
| `CDK_MINTD_LN_BACKEND` | Set to `cln` | Yes |
| `CDK_MINTD_CLN_RPC_PATH` | Path to CLN RPC socket | Yes |
| `CDK_MINTD_CLN_BOLT12` | Enable BOLT12 support (default: `true`) | No |
| `CDK_MINTD_CLN_FEE_PERCENT` | Fee percentage (default: `0.02`) | No |
| `CDK_MINTD_CLN_RESERVE_FEE_MIN` | Minimum fee in sats (default: `2`) | No |

### Example

```bash
export CDK_MINTD_LN_BACKEND=cln
export CDK_MINTD_CLN_RPC_PATH=/home/user/.lightning/bitcoin/lightning-rpc
cdk-mintd
```

## License

This project is licensed under the [MIT License](../../LICENSE).
