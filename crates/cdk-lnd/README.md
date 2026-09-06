# CDK LND

[![crates.io](https://img.shields.io/crates/v/cdk-lnd.svg)](https://crates.io/crates/cdk-lnd)
[![Documentation](https://docs.rs/cdk-lnd/badge.svg)](https://docs.rs/cdk-lnd)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

LND (Lightning Network Daemon) backend implementation for the Cashu Development Kit (CDK).

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-lnd = "*"
```

## Configuration for cdk-mintd

### Config File

```toml
[payment_backend]
backend = "lnd"

[lnd]
address = "https://localhost:10009"
cert_file = "/path/to/.lnd/tls.cert"
macaroon_file = "/path/to/.lnd/data/chain/bitcoin/mainnet/admin.macaroon"
fee_percent = 0.02       # Optional, defaults to 2%
reserve_fee_min = 2      # Optional, defaults to 2 sats
```

### Environment Variables

All configuration can be set via environment variables:

| Variable | Description | Required |
|----------|-------------|----------|
| `CDK_MINTD_PAYMENT_BACKEND` | Set to `lnd` | Yes |
| `CDK_MINTD_LND_ADDRESS` | LND gRPC address (e.g., `https://localhost:10009`) | Yes |
| `CDK_MINTD_LND_CERT_FILE` | Path to LND TLS certificate | Yes |
| `CDK_MINTD_LND_MACAROON_FILE` | Path to LND macaroon file | Yes |
| `CDK_MINTD_LND_FEE_PERCENT` | Fee percentage (default: `0.02`) | No |
| `CDK_MINTD_LND_RESERVE_FEE_MIN` | Minimum fee in sats (default: `2`) | No |

### Example

```bash
export CDK_MINTD_PAYMENT_BACKEND=lnd
export CDK_MINTD_LND_ADDRESS=https://127.0.0.1:10009
export CDK_MINTD_LND_CERT_FILE=/home/user/.lnd/tls.cert
export CDK_MINTD_LND_MACAROON_FILE=/home/user/.lnd/data/chain/bitcoin/mainnet/admin.macaroon
cdk-mintd
```

## Pending outgoing payments

Regular BOLT11 payments return `Pending` when LND reports `INITIATED` or
`IN_FLIGHT`. Status checks return the current payment state without waiting for
settlement. Dispatch acknowledgement and status lookups have a 10-second timeout;
a timeout is an indeterminate result, never a terminal payment failure.

The backend tracks outgoing payments in memory before dispatch. While
`wait_payment_event()` is consumed (as it is by the mint), a background monitor
tracks each payment by hash and emits success or failure events for the mint's
saga handlers. Tracking resumes after stream interruptions within the same
backend instance. After a process restart, callers must reconcile outstanding
payments through status checks.
Cancelling or dropping the event subscription stops its monitors without
cancelling Lightning payments. Applications using the backend directly must
consume the event stream or reconcile payments through status checks.

## Minimum Supported Rust Version (MSRV)

This crate supports Rust version **1.75.0** or higher.

## License

This project is licensed under the [MIT License](../../LICENSE).
