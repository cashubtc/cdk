# CDK Mint RPC

[![crates.io](https://img.shields.io/crates/v/cdk-mint-rpc.svg)](https://crates.io/crates/cdk-mint-rpc)
[![Documentation](https://docs.rs/cdk-mint-rpc/badge.svg)](https://docs.rs/cdk-mint-rpc)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

gRPC server and CLI client for managing Cashu mints in the Cashu Development Kit (CDK).

## Components

This crate includes:
- gRPC server for mint management, embedded in `cdk-mintd`
- `cdk-mint-cli`, a CLI client for the gRPC server
- Protocol definitions for mint management

## Services

The management API is a set of per-domain gRPC services. Each lives in its own
versioned package under `src/proto/` and is served on the same port.

| Service | Package | Scope |
|---|---|---|
| `MintInfoService` | `cdk_mint_info_v1` | Mint metadata: name, descriptions, MOTD, icon and terms-of-service URLs, mint URLs, contacts |
| `KeysetService` | `cdk_mint_keyset_v1` | Keyset rotation |
| `PaymentMethodService` | `cdk_mint_payment_method_v1` | Mint (NUT-04) and melt (NUT-05) method settings, and the mint-wide disabled flags |
| `QuoteService` | `cdk_mint_quote_v1` | Quote time-to-live settings and mint quote state overrides |
| `WalletService` | `cdk_mint_wallet_v1` | On-chain wallet balance, deposit addresses, and transactions |

Every request must carry the `x-cdk-protocol-version` header set to
`cdk_common::MINT_RPC_PROTOCOL_VERSION`. The CLI adds it for you.

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
cdk-mint-cli get-info

# Update the message of the day
cdk-mint-cli update-motd "Maintenance tonight at 22:00 UTC"

# Rotate to the next keyset for a unit
cdk-mint-cli rotate-next-keyset --unit sat

# Point at a specific mint
cdk-mint-cli --addr https://127.0.0.1:8086 get-info
```

### TLS

When the working directory (`--work-dir`, default `~/.cdk-mint-rpc-cli`)
contains a `tls/` directory with `ca.pem`, `client.pem`, and `client.key`, the
CLI connects with mutual TLS. Without it, the CLI connects in plaintext, which
the mint only accepts when explicitly configured to allow it. See
[CERTIFICATES.md](CERTIFICATES.md) for generating the certificates.

## License

This project is licensed under the [MIT License](../../LICENSE).
