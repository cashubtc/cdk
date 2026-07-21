# CDK Mint RPC

[![crates.io](https://img.shields.io/crates/v/cdk-mint-rpc.svg)](https://crates.io/crates/cdk-mint-rpc)
[![Documentation](https://docs.rs/cdk-mint-rpc/badge.svg)](https://docs.rs/cdk-mint-rpc)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

gRPC server and CLI client for managing Cashu mints in the Cashu Development Kit (CDK).

## Components

This crate includes:
- gRPC server for mint management
- CLI client for interacting with the gRPC server
- Protocol definitions for mint management

## Installation

From crates.io:
```bash
cargo install cdk-mint-rpc
```

As a library:
```toml
[dependencies]
cdk-mint-rpc = "*"
```

## Usage

### CLI

```bash
# Show available commands
cdk-mint-cli --help

# Get mint info
cdk-mint-cli info

# Manage keysets
cdk-mint-cli keysets list
```




## License

This project is licensed under the [MIT License](../../LICENSE).
