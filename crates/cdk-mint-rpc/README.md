# Cashu Mint Management RPC

This crate is a gRPC client and server to control and manage a CDK mint. This crate exposes a server component that can be imported as a library component, see its usage in `cdk-mintd`. The client can be used as a CLI by running `cargo r --bin cdk-mint-cli`.

The server can be run with or without certificate authentication. For running with authentication, see the [Certificate Generation Guide](./CERTIFICATES.md) for instructions on creating the necessary certificates using the included `generate_certs.sh` script.

## Overview

The cdk-mint-rpc crate provides:

1. A gRPC server for managing Cashu mints
2. A CLI client (`cdk-mint-cli`) for interacting with the gRPC server

This allows mint operators to manage their Cashu mint instances remotely through a secure gRPC interface.

## Features

- Remote mint management via gRPC
- Secure authentication
- Command-line interface for common mint operations
- Integration with other CDK components

## Usage

### CLI Client

The `cdk-mint-cli` provides a command-line interface for interacting with the mint:

```bash
# Using cargo to run the CLI with a specific address
cargo r --bin cdk-mint-cli -- --addr https://127.0.0.1:8086 get-info
```

## Related Crates

This crate is part of the Cashu Development Kit (CDK) ecosystem:

- [cdk](../cdk/): Core Cashu protocol implementation
- [cdk-mintd](../cdk-mintd/): Cashu Mint Binary

## License

MIT License
