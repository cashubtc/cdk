# CDK BDK

CDK onchain payment backend using [BDK (Bitcoin Development Kit)](https://bitcoindevkit.org/), providing on-chain Bitcoin payment functionality for CDK mint operations.

## Features

- On-chain Bitcoin payments (receive and send)
- BIP84 (Native SegWit) key derivation from a BIP39 mnemonic
- SQLite-backed wallet persistence via BDK
- Blockchain sync via Bitcoin Core RPC
- Confirmation tracking for incoming and outgoing transactions
- Pending transaction persistence via KV store

## Chain Sources

| Source | Status |
|---|---|
| Bitcoin Core RPC | Supported |
| Esplora | Supported |

## Usage

```rust,ignore
use cdk_bdk::{BatchConfig, BitcoinRpcConfig, CdkBdk, ChainSource, SyncConfig};

let chain_source = ChainSource::BitcoinRpc(BitcoinRpcConfig {
    host: "127.0.0.1".to_string(),
    port: 18443,
    user: "user".to_string(),
    password: "password".to_string(),
});

let backend = CdkBdk::new(
    mnemonic,
    Network::Regtest,
    chain_source,
    "/path/to/storage".to_string(),
    fee_reserve,
    kv_store,
    Some(BatchConfig::default()),
    num_confs,
    min_receive_amount_sat,
    sync_interval_secs,
    Some(30),                    // shutdown_timeout_secs
    Some(SyncConfig::default()),
)?;
```

## Shutdown

`stop()` cancels background sync and batch tasks, then awaits their exit up
to a bounded timeout (default 30 seconds; configurable via
`shutdown_timeout_secs`). If the timeout is exceeded the tasks are aborted.
`start()` returns `Error::AlreadyStarted` if called while tasks are already
running.

## Fee Estimation

Fee rates are estimated per-tier from the configured chain source:

| Tier      | Target blocks |
|-----------|--------------:|
| Immediate | 1             |
| Standard  | 6             |
| Economy   | 144           |

Rates are cached with a configurable TTL (default 60 seconds). On
estimation failure, the configured fallback sat/vB value is used and a
warning is logged -- `get_payment_quote` does not fail due to transient
estimation outages.

## Sync

The blockchain sync loop applies blocks in configurable chunks (default 16
blocks per wallet-lock acquisition) so user-facing operations like address
reveal and batch construction are not blocked during long chain catch-ups.
Chain-source clients are reused across sync iterations and rebuilt only on
error.

## Finality and Confirmation Policy

- Finalization is policy-based: once a send or receive reaches `num_confs`, it is
  treated as final and is not reopened after deep reorgs
- Incoming payment tracking is confirmed-only; unconfirmed tracked outputs are
  intentionally ignored until they satisfy the configured confirmation threshold

## Known Limitations

- **Tiered batching is scaffolded but not reachable via the standard melt
  flow.** `PaymentTier::Standard` and `PaymentTier::Economy` are accepted by
  `make_payment` and drive the internal batch processor, but the cdk melt
  pipeline currently passes `tier: None` at execute time. All melts are
  effectively treated as `Immediate` until the upstream plumbing lands in
  cdk-common and cdk. See `TODO(#TBD)` in
  `crates/cdk-common/src/payment.rs` (`from_melt_quote_with_fee`) and
  `crates/cdk/src/mint/melt/mod.rs` (`get_melt_onchain_quote_impl`).
