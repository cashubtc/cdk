# Federated BDK-FROST milestone 0 baseline

This note records the behavior frozen before production implementation begins.
It accompanies `docs/federated-cashu-bdk-frost-architecture-decision.md` and the
milestone tracker. Measurements use base revision `dfe01b7f9d92`, SQLite,
Bitcoin Core 30.2 on regtest, and Rust/Cargo 1.95.0.

## Existing behavior

`CdkBdk` is currently a standalone, mnemonic-backed BIP84 wallet. It owns its
private descriptors, persistent BDK database, address revelation, chain sync,
receive detection, fee estimation, coin selection, PSBT construction, local
signing, finalization, broadcast, send batching, and recovery. Its
`MintPayment` implementation exposes ordinary on-chain mint and melt behavior.

The existing `federation` and `bdk` features can be compiled together, but the
configured BDK backend is still constructed from the local member's mnemonic.
There is no shared descriptor, consensus address allocation, FROST signing, or
federated UTXO state. Consequently this combination is not a federated custody
backend: independently configured members own unrelated Bitcoin wallets. The
new `federation-bdk` path must fail closed until shared custody is ready and
must not silently select this member-local behavior.

## Reproducible commands and results

| Command | Result | Observed time |
|---|---:|---:|
| `cargo test -p cdk-bdk` | 99 passed, 0 failed; 1 doc test ignored | 5.21 s test, 36.49 s compile |
| `cargo test -p cdk --features federation mint::federation` | 109 passed, 0 failed | 5.41 s test |
| `cargo test -p cdk-integration-tests block_generation_is_split_into_bounded_rpc_calls` | 1 passed, 0 failed | 1 min 14 s including compile |
| `PATH=<bitcoin-core-bin>:$PATH just itest-onchain SQLITE` | 14 passed, 0 failed | 39.03 s test phase |
| `cargo check -p cdk-bdk` | passed | 13.01 s |
| `cargo check -p cdk --features federation` | passed | 34.96 s |
| `cargo check -p cdk-integration-tests` | passed | 1 min 01 s |

The on-chain suite covers mint, melt, standard fee-index selection, restore,
multiple deposits, asynchronous melt preference, underpayment, unique and
concurrent quotes, quote recovery/status checks, issued-amount tracking, and
attempted unpaid minting.

The first regtest setup attempted to mine all 200 initialization blocks in one
RPC call and exceeded the client's 30-second socket timeout. The integration
Bitcoin client now bounds `generatetoaddress` calls to 100 blocks. Its focused
unit test and the complete rerun above preserve the fix; production wallet or
mint behavior is unchanged.

`rustfmt --check --edition 2021` passes for the changed integration client and
`git diff --check` passes. The workspace-wide formatting check continues to
report formatting-only differences in four untouched federation source files;
those pre-existing differences are intentionally excluded from this milestone.

## SQLite and transaction snapshot

The successful complete on-chain run ended at wallet height 219 with this BDK
snapshot:

| BDK table | Rows |
|---|---:|
| `bdk_wallet` | 1 |
| `bdk_blocks` | 219 |
| `bdk_txs` | 21 |
| `bdk_anchors` | 21 |
| `bdk_descriptor_last_revealed` | 2 |
| `bdk_txouts` | 0 |
| `bdk_descriptor_derived_spks` | 0 |
| `bdk_wallet_locked_outpoints` | 0 |

All 21 transaction records retained raw bytes: 5,846 bytes total, with
individual serialized transactions ranging from 222 to 518 bytes. Raw
transactions and test-generated identifiers are deliberately not copied into
this document.

The mint database retained 17 mint quotes, 6 melt quotes, and 65 proofs. The
BDK KV namespace retained 17 receive-address mappings, 15 finalized receive
intents plus their outpoint and quote indexes, and 6 finalized send intents
plus quote indexes. This is an end-of-suite diagnostic snapshot, not a stable
schema-count assertion; the durable baseline is the namespaces and state
shapes.

Send batches persist explicit `Built`, `Signed`, and `Broadcast` states. Built
state stores PSBT bytes and intent membership; signed and broadcast states
store transaction bytes, payment assignments, and the fee, with broadcast also
storing the txid. Send intents progress through pending, batched, awaiting
confirmation, and failed states before finalized tombstones. Receive intents
progress from detected records to finalized tombstones.

## Recovery baseline

The unit suite verifies pre-broadcast rollback, repair of pending intents from
a signed batch, application of broadcast transaction bytes to the BDK graph,
computed-txid recovery, orphan and missing-record shapes, receive finalization
atomicity, and duplicate quote/outpoint rejection. The regtest restore case
additionally proves the public mint recovery contract against a live chain.

The federated implementation must preserve these standalone contracts while
moving shared address, UTXO, accepted-transaction, signature, and confirmation
authority into the federation state machine described by the architecture
decision.

## Milestone 22: static custody-key scope

**Status: invalidated — not planned.** Root, application, and federation key
rotation are out of scope for this implementation. Supporting rotation would
require substantially more lifecycle and recovery complexity, including a new
coordinated DKG for the other ceremony outputs such as BLS shares. There is
nothing to implement for this milestone; it must be skipped and must not be
treated as deferred implementation work.
