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

The operator CLI is part of the single `cdk-mintd` binary. This crate does not
ship a separate command-line binary.

## Installation

As a library:

```toml
[dependencies]
cdk-mint-rpc = "*"
```

## Usage

### Operator commands

```bash
# Show available commands
cdk-mintd --help

# Get mint info
cdk-mintd get-info

# Update an immediately applied field
cdk-mintd update-motd "Scheduled maintenance"

# Rotate a keyset
cdk-mintd rotate-next-keyset --unit sat --use-keyset-v2 true
```

When `--rpc-address` is omitted, management commands use
`http://127.0.0.1:8086` without a TLS directory and
`https://127.0.0.1:8086` when `--rpc-tls-dir` is set or `<work-dir>/tls`
exists. An explicit address must likewise use `http://` without client TLS
credentials and `https://` with them. Select another server or client
certificate directory with the global options:

```bash
cdk-mintd get-info \
  --rpc-address https://mint.example:8086 \
  --rpc-tls-dir /var/lib/cdk-mintd/tls
```

### Configuration RPC

After one explicit direct initialization, the database is authoritative:

```bash
cdk-mintd config validate --file mint.toml
cdk-mintd config init --file mint.toml
cdk-mintd
```

Normal mintd startup never reads TOML or applies operational environment
overrides. Configuration commands access the authoritative database directly
by default. A later full-document replacement must be requested explicitly
using direct database access:

```bash
# Validate against persisted configuration constraints without changing state
cdk-mintd config apply --file changed.toml --validate-only

# Stage directly; this also works beside a running daemon
cdk-mintd config apply --file changed.toml

# Inspect/export active state and inspect pending state
cdk-mintd config show
cdk-mintd config export --file backup.toml

# Cancel the staged replacement before restart
cdk-mintd config discard-pending

# Explicitly stage through a management endpoint instead
cdk-mintd config apply --file changed.toml --rpc https://mint.example:8086 \
  --rpc-tls-dir /var/lib/cdk-mintd/tls
```

`config apply`, `show`, `export`, and `discard-pending` use RPC only when an
endpoint is supplied with `--rpc`. They never probe RPC and never fall back to
direct database access. Direct access uses a short-lived configuration lock and
can run beside a steady daemon. Direct commands, startup activation, full-file
RPC operations, and immediate RPC configuration updates all acquire the same
database-scoped lock. `--rpc <endpoint>` explicitly selects RPC transport; it is
not required merely because RPC is enabled.

Full-document applies are staged until a successful restart. There are no
configuration revisions or expected-revision parameters in this iteration.
Field-specific RPCs, such as mint-info and quote-TTL updates, remain immediate
when no complete document is pending. Activate or discard the pending document
before issuing one of those updates.

Database/work-directory/SQLCipher inputs and RPC connection options are
bootstrap exceptions because they are required before stored configuration can
be read or the management server can be contacted. The global `--rpc-address`
option remains for field-specific management commands; configuration commands
use their transport-selecting `--rpc <endpoint>` option.

Configuration RPC payloads persist secret references only. Secret fields must
use `env:VARIABLE` or `file:/absolute/path`; the resolved contents are never
stored or returned by configuration RPCs. See the
[`cdk-mintd` configuration guide](../cdk-mintd/README.md#configuration) for the
complete lifecycle.

## License

This project is licensed under the [MIT License](../../LICENSE).
