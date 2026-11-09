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
| `KeysetService` | `cdk_mint_keyset_v1` | Keyset rotation and ecash issuance/redemption totals |
| `PaymentMethodService` | `cdk_mint_payment_method_v1` | Mint (NUT-04) and melt (NUT-05) method settings, and the mint-wide disabled flags |
| `QuoteService` | `cdk_mint_quote_v1` | Quote time-to-live settings and mint quote state overrides |
| `WalletService` | `cdk_mint_wallet_v1` | On-chain wallet balance, deposit addresses, and transactions |

Each package name carries its API version (`_v1`). Incompatible changes go in a
new package version, served alongside the old one.

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

# Query ecash issued and redeemed totals
cdk-mint-cli get-keyset-totals
cdk-mint-cli get-keyset-totals --unit sat
cdk-mint-cli get-keyset-totals --keyset-id "$KEYSET_ID"

# Point at a specific mint
cdk-mint-cli --addr https://127.0.0.1:8086 get-info
```

### Keyset and ecash accounting totals

`KeysetService.GetKeysetTotals` is a read-only management RPC returning ecash
issuance and redemption totals across the mint.

Accounting is grouped and aggregated per currency unit in `unit_totals` (keeping
currency units distinct, e.g. `sat` and `usd` are never summed together), and also
provides a granular breakdown per keyset in `keyset_totals`. Filtering by `--unit`
or `--keyset-id` is supported. The endpoint is read-only and remains available
during pending restarts.

> **Migration Note for Legacy `GetInfo` Consumers:** In earlier versions,
> legacy monolithic `CdkMint.GetInfo` returned `total_issued` and `total_redeemed`
> as a flat sum across all keysets regardless of unit, which produced invalid
> totals on multi-unit mints. When `CdkMint` was split into domain services,
> `MintInfoService.GetInfo` was scoped strictly to public NUT-06 metadata.
> Applications and dashboards tracking mint ecash accounting should migrate to
> `KeysetService.GetKeysetTotals` (using `unit_totals`).

### TLS

When the working directory (`--work-dir`, default `~/.cdk-mint-rpc-cli`)
contains a `tls/` directory with `ca.pem`, `client.pem`, and `client.key`, the
CLI connects with mutual TLS. Without it, the CLI connects in plaintext, which
the mint only accepts when explicitly configured to allow it. See
[CERTIFICATES.md](CERTIFICATES.md) for generating the certificates.

## License

This project is licensed under the [MIT License](../../LICENSE).
