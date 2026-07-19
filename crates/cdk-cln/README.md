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

### Import and Start

Add the section above to a complete `mint.toml`, then explicitly import it into
the mint database before the first start:

```bash
cdk-mintd config validate --file mint.toml
cdk-mintd config init --file mint.toml
cdk-mintd
```

Environment variables no longer override CLN settings at daemon startup. To
change them later, edit the complete file, run
`cdk-mintd config apply --file mint.toml` through the management RPC (or add
`--offline` while the daemon is stopped), and restart. See the
[`cdk-mintd` configuration guide](../cdk-mintd/README.md#configuration).

## License

This project is licensed under the [MIT License](../../LICENSE).
