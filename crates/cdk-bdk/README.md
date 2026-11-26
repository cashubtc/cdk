# CDK BDK

CDK onchain payment backend using [BDK (Bitcoin Development Kit)](https://bitcoindevkit.org/), providing on-chain Bitcoin payment functionality for CDK mint operations.

## Features

- On-chain Bitcoin payments (receive and send)
- BIP84 (Native SegWit) key derivation from a BIP39 mnemonic
- SQLite-backed wallet persistence via BDK
- Blockchain sync via Bitcoin Core RPC, Electrum, or Esplora
- Confirmation tracking for incoming and outgoing transactions
- Pending transaction persistence via KV store

## Chain Sources

| Source | Status |
|---|---|
| Bitcoin Core RPC | Supported |
| Electrum | Supported |
| Esplora | Supported |

Electrum sources accept `tcp://` and `ssl://` server URLs:

```rust,ignore
let chain_source = ChainSource::Electrum(ElectrumConfig {
    url: "ssl://electrum.example.com:50002".to_string(),
    batch_size: 5,
});
```

## Usage

```rust,ignore
use cdk_bdk::{BatchConfig, BitcoinRpcConfig, CdkBdk, ChainSource, SyncConfig};

let chain_source = ChainSource::BitcoinRpc(BitcoinRpcConfig {
    host: "127.0.0.1".to_string(),
    port: 18443,
    user: "user".to_string(),
    password: "password".to_string(),
    wallet_rescan_from_height: None,
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
    min_send_amount_sat,
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

Esplora sync, broadcast, and fee-estimation requests have a 10-second timeout.
Esplora's `parallel_requests` must be greater than zero. Missed polling ticks
are skipped so a slow sync does not trigger a burst of catch-up polls.

Fresh Bitcoin Core wallets checkpoint at the current chain tip instead of
scanning from genesis. When restoring from a mnemonic, set
`BitcoinRpcConfig::wallet_rescan_from_height` to the wallet's known birthday
height. This setting only affects wallet creation and never rewinds persisted
wallet state.

On reorgs, Bitcoin Core sync follows the orphaned branch's block headers to
find the actual common ancestor, including ancestors absent from the wallet's
stored checkpoints. It then replays all replacement blocks after that ancestor,
even below the original birthday. The birthday controls wallet creation, not
reorg recovery. Normal catch-up still resumes from the latest checkpoint.
Missing headers or required block data fail the sync tick and are retried; sync
does not skip affected blocks or substitute a genesis rescan for missing history.
A rollback with no replacement block is retried rather than reported as completed
recovery.

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

## Payjoin v2

`cdk-bdk` uses PDK (`payjoin` 1.2.0) for BIP77 sending and receiving. The Cashu
wire fields follow the [draft Payjoin extension](https://github.com/cashubtc/nuts/pull/376).
Enable it with `CdkBdk::with_payjoin(PayjoinConfig { relay_url, directory_url })`.
The relay must use HTTPS. Setting `directory_url` also enables receiving and
requires the Bitcoin RPC chain source, which validates original transactions
and input ownership against Bitcoin Core. Sending works with every chain source.

For `cdk-mintd`, add:

```toml
[bdk.payjoin]
relay_url = "https://your-ohttp-relay.example"
# Omit this field for sending only.
directory_url = "https://your-payjoin-directory.example"
```

Use a relay operated independently of the directory. Receiving quotes publish
fresh PDK receiver parameters with a 24-hour session lifetime. The receiver
contributes a P2WPKH input and requires the sender to cover the additional fee;
it does not charge that fee to the mint deposit. Deposit accounting subtracts
the mint's contributed coins, including when a sender uses multiple outputs or
a legacy input changes the final transaction ID. Only confirmed deposits count.

Payjoin melts run individually, outside ordinary payment batches. Quotes reserve
an additional native SegWit input fee. PDK validates proposals, output
substitution is disabled, and the final sender expenditure is checked against
the selected quote's fee limit. Transport failures are retried with fresh OHTTP
requests. Negotiation failure or expiration uses the original transaction as a
fallback, subject to the same durable broadcast and confirmation handling as
ordinary melts. Separate sender and receiver workers keep relay waits out of the
ordinary batch processor and prevent idle incoming quotes from delaying outgoing
negotiations.
Each worker processes up to eight sessions concurrently; coin selection remains
serialized by the wallet lock.

### Persistence and recovery

PDK event logs and input reservations live in the backend KV database. The BDK
wallet and derivation state live in `bdk_wallet/bdk_wallet.sqlite`. Back up both.
A session and its selected inputs are committed before the first network
request. Receiver reservations are committed before signatures are exposed.
A signed final or fallback transaction is committed before broadcast. On restart,
the backend replays PDK events and resumes the existing transaction. It also
watches the original fallback after a proposal is persisted, so the payment
receipt identifies the transaction that actually confirmed.

The sender hands negotiation to the send saga through a typed transition. A
single KV transaction stores the signed batch and assigns its pending intent;
stale intents and attempts to replace an existing batch are rejected. Normal
execution advances through the typed broadcast transitions. Interrupted handoffs
are retried by the Payjoin worker as well as during startup recovery.

Before a session is exposed, a failed payment can be marked `Failed` and retried.
Once a valid transaction may have been exposed, errors leave it pending; a
network timeout or rejected broadcast never authorizes a refund. Receiver inputs
remain reserved until a conflicting transaction or the Payjoin confirms, even
if negotiation has expired. Unexposed receiver sessions can release inputs when
they expire. Closed session records are retained for accounting and recovery.
Removing the Payjoin configuration stops new negotiations, but existing senders
still complete using their saved proposal or fallback transaction. Receivers
continue fallback and confirmation recovery, retaining signed input reservations
until settlement.
Malformed session data stops coin selection instead of silently losing locks.
Do not delete session records to retry an ambiguous payment.

### CLI

An onchain mint quote prints a `bitcoin:` Payjoin URI when the backend offers one:

```sh
cdk-cli mint <mint-url> 10000 --method onchain --wait-duration 0
```

Pay that URI through an onchain melt:

```sh
cdk-cli melt --method onchain --address 'bitcoin:...?...&pj=...'
```

Alternatively, pass a plain address, `--amount`, and `--payjoin-endpoint` containing
the complete BIP77 endpoint (including its fragment). The explicit Payjoin flow
rejects a quote that does not acknowledge those parameters. Acknowledging Payjoin
does not prohibit transaction fallback after negotiation fails.

The SDK exposes `quote_onchain_melt_options_with_payjoin` and
`onchain_mint_quote_payjoin`; both are also exported through `cdk-ffi`.

Run the focused recovery tests with `cargo test -p cdk-bdk payjoin --lib`.
These include cancellation before and after every KV write/commit in the sender
handoff and subprocess exits at the same boundaries, followed by reopening both
databases and recovering the exact transaction.

Run the local end-to-end tests with:

```sh
nix develop .#regtest -c cargo test -p cdk-bdk --lib \
  --features payjoin-regtest live_payjoin -- --ignored --nocapture
```

They start an isolated Bitcoin Core regtest node and separate HTTPS relay and
directory instances, exercise real OHTTP negotiation and Bitcoin Core validation,
and confirm both Payjoin and fallback transactions after reopening the wallets.
They also interrupt sender and receiver POSTs after the directory has accepted
them but before the response is saved locally. A private test certificate is
trusted only by these test instances. Set `BITCOIND_EXE` when the daemon is not
on `PATH`; temporary node and database files use the configured temporary directory.
