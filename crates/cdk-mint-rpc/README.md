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

## Upgrading manual payment credits

Manual credits remain on `cdk_mint_quote_v1.QuoteService`. The new `amount` and
`payment_id` protobuf fields are optional on the wire. BOLT11 defaults an omitted
amount to the unpaid remainder; all other methods require an explicit nonzero
amount, even for fixed-amount quotes.

The actual backend `payment_id` is required for every method. Old callers that
omit it receive `INVALID_ARGUMENT`; this is an intentional behavioral change.
Use the exact backend payment ID, not an offer/address identifier:

```sh
# BOLT11: credit the unpaid remainder
cdk-mint-cli update-mint-quote-state "$QUOTE_ID" PAID --payment-id "$BACKEND_PAYMENT_ID"
# Other methods: specify the amount in the quote's unit
cdk-mint-cli update-mint-quote-state "$QUOTE_ID" PAID --amount 100 --payment-id "$BACKEND_PAYMENT_ID"
```

An already recorded payment ID returns `ALREADY_EXISTS`, regardless of the
requested amount. Fixed quotes cap credits at their requested amount,
except reusable BOLT12 offers. The recorded amount remains final: mismatched
backend reports warn when processed but never adjust the balance. No database
migration is required. Upgrade the server before using the new credit workflow.
