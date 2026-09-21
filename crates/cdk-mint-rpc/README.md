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
| `QuoteService` | `cdk_mint_quote_v1` | Melt quote inspection and resolution, quote time-to-live settings, and mint quote state overrides |
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


### Investigate a reported pending melt

```bash
# Find the reported quote, even if it is already paid or unpaid
cdk-mint-cli list-melt-quotes --quote-id "$QUOTE_ID"
cdk-mint-cli list-melt-quotes --payment-lookup-id "$PAYMENT_LOOKUP_ID"
cdk-mint-cli list-melt-quotes --payment-request "$INVOICE_OR_OFFER_OR_ADDRESS"

# List the mint's pending melts, including expired quotes
cdk-mint-cli list-melt-quotes --state pending
cdk-mint-cli list-melt-quotes --state pending --limit 100 --offset 100
```

`QuoteService.ListMeltQuotes` inspects stored melt quotes without polling the
payment backend or triggering recovery. State filtering is optional: a lookup
without `--state` includes all matching attempts, so a completed payment or an
unpaid retry is visible to the operator. All supplied filters must match.

Results include the current stored state, amount, fee reserve, unit, payment
method, original payment request, creation/expiry/payment timestamps (Unix
seconds), payment proof, and payment lookup ID and kind when known. An incomplete
recovery operation also includes its operation ID for finding mint logs, stage,
and creation/update timestamps. `updated_at` records the last persisted saga
update, not the last backend status check.

For example, `payment_pending` means the backend acknowledged an attempt;
`finalizing` means payment was recorded and change signing/cleanup remains.
A pending quote with no stored saga warrants investigation in the mint logs;
absence of a saga alone does not establish payment success or failure. Quote
and saga are read separately and can change during inspection.

`--payment-lookup-id` aliases `--request-lookup-id`. Lookup IDs and payment
requests match exactly and case-sensitively; use lowercase hexadecimal for
payment hashes and the original invoice, offer, address, or custom request.
A payment request can match multiple attempts; an address is not necessarily a
unique payment. Unknown IDs and requests return an empty list. Empty filters,
malformed quote IDs, and invalid state filters are rejected.

Results are oldest first, with ties sorted by quote ID. The default page size is
100 and the maximum is 1000; `total` counts matches before pagination. Pages can
shift while payments progress. Listing without a quote ID currently loads melt
quotes through the existing mint database API and filters them in memory.

### Resolve an investigated melt

`QuoteService.ResolveMeltQuote` applies the operator's verified payment outcome.
Use the quote ID, operation ID, and recovery stage returned by inspection.
It does not query, send, cancel, or stop retries at the payment backend.

For a verified successful payment, provide the actual total spent including
payment fees, the quote's unit, backend payment identifier and kind, and payment
proof when available:

```bash
cdk-mint-cli resolve-melt-quote "$QUOTE_ID" \
  --operation-id "$OPERATION_ID" --expected-saga-state payment_pending \
  --reason "Verified settled payment in backend; incident 123" \
  finalize --total-spent 1005 --unit sat \
  --payment-lookup-id "$PAYMENT_HASH" --payment-lookup-id-kind payment_hash \
  --payment-proof "$PREIMAGE"
```

This marks the quote paid, spends reserved inputs, signs change, records the
completed operation, and cleans up recovery state. Total spent must cover the
quote amount and fit within reserved inputs after input fees. For an operation
already finalizing, the supplied payment details must match its stored result.

To compensate, first establish that the payment failed and that no backend
attempt or retry can still pay it. Then explicitly assert that finding:

```bash
cdk-mint-cli resolve-melt-quote "$QUOTE_ID" \
  --operation-id "$OPERATION_ID" --expected-saga-state payment_pending \
  --reason "Backend payment permanently failed; retries stopped; incident 123" \
  compensate --payment-failure-confirmed
```

Compensation releases the reserved proofs for reuse, removes change reservations,
marks the quote unpaid, and cleans up the operation. Paid or finalizing operations
cannot be compensated. Busy quotes, changed operation IDs/stages, missing setup
records, and conflicting decisions are rejected; inspect again before proceeding.

The decision, reason, and acceptance timestamp are stored in the mint database's
`cdk_mint/melt_resolutions` key-value namespace, keyed by operation ID, in the same
transaction as the durable recovery handoff. If cleanup is interrupted, retry the
**identical command**, including its original expected stage and reason. Startup
recovery can also finish the handoff. Repeating a completed resolution is a no-op;
an old command cannot resolve a new operation for the same quote. There is no
direct quote-state-only override for melts.

### TLS

When the working directory (`--work-dir`, default `~/.cdk-mint-rpc-cli`)
contains a `tls/` directory with `ca.pem`, `client.pem`, and `client.key`, the
CLI connects with mutual TLS. Without it, the CLI connects in plaintext, which
the mint only accepts when explicitly configured to allow it. See
[CERTIFICATES.md](CERTIFICATES.md) for generating the certificates.

## License

This project is licensed under the [MIT License](../../LICENSE).
