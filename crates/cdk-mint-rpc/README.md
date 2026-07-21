# CDK Mint RPC

[![crates.io](https://img.shields.io/crates/v/cdk-mint-rpc.svg)](https://crates.io/crates/cdk-mint-rpc)
[![Documentation](https://docs.rs/cdk-mint-rpc/badge.svg)](https://docs.rs/cdk-mint-rpc)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

gRPC server and client library for managing Cashu mints in the Cashu Development Kit (CDK).

## Components

This crate includes:

- gRPC server for mint management
- Generated and ergonomic clients for interacting with the gRPC server
- Protocol definitions for mint management
- A transport-independent configuration-management interface
- The `cdk-mint-cli` binary for operators

## Installation

As a library:

```toml
[dependencies]
cdk-mint-rpc = "*"
```

## Usage

### Operator CLI (`cdk-mint-cli`)

```bash
# Show available commands
cdk-mint-cli --help

# Get mint info
cdk-mint-cli get-info

# Update an immediately applied field
cdk-mint-cli update-motd "Scheduled maintenance"

# Rotate a keyset
cdk-mint-cli rotate-next-keyset --unit sat --use-keyset-v2 true
```

When client TLS credentials are not provided, the address must use `http://`.
With TLS credentials (explicit `--tls-dir` or `<work-dir>/tls`), the address must
use `https://`. Defaults:

```bash
cdk-mint-cli get-info \
  --addr https://mint.example:8086 \
  --tls-dir /var/lib/cdk-mintd/tls
```

### Configuration

Authoritative mint configuration is managed by the `cdk-mintd` binary against the
database **directly**. It is not an RPC client:

```bash
cdk-mintd config validate --file mint.toml
cdk-mintd config init --file mint.toml
cdk-mintd
cdk-mintd config apply --file changed.toml
cdk-mintd config show
cdk-mintd config export --file backup.toml
cdk-mintd config discard-pending
```

See the [`cdk-mintd` configuration guide](../cdk-mintd/README.md#configuration)
for bootstrap settings, secret references, and activate/rollback behavior.

The management RPC server still exposes full-document configuration methods
(`GetConfiguration`, `ApplyConfiguration`, `DiscardPendingConfiguration`) for
programmatic clients. Immediate field RPCs used by `cdk-mint-cli` remain
available when no complete document is pending.

## License

This project is licensed under the [MIT License](../../LICENSE).
