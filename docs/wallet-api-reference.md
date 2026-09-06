# CDK Wallet API reference

This document describes the wallet-facing API on the current branch. The Rust
wallet and `cdk-ffi` bindings use the same durable wallet engine. The FFI layer
converts types and owns foreign-language object lifetimes; it does not implement
separate payment or recovery rules.

The API is intentionally shaped as:

```text
request -> session/plan -> execute or cancel -> receipt
                         \-> restart and resume by durable ID
```

## Object model

`Wallet` represents one mint URL and one currency unit. `WalletManager` owns a
collection of those wallets and shares the seed, storage, transport, and
rate-limit infrastructure. Use a standalone wallet when the mint and unit are
already known; use a manager for mint discovery, portfolios, NUT-18 routing, or
cross-mint transfers.

Opening is local. After opening, applications should call online
`synchronize`, discover unfinished work with `operations`, and resume by the
reported quote or operation ID.

## Standalone Rust wallet

```rust
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{Wallet, WalletIdentity, WalletOpenRequest};
use cdk::wallet::operation::SyncPolicy;

let wallet = Wallet::open(WalletOpenRequest::new(
    WalletIdentity::new("https://mint.example.com".parse()?, CurrencyUnit::Sat),
    store,
    seed, // an existing [u8; 64] seed
))?;

let balance = wallet.balance().await?;
let report = wallet.synchronize(SyncPolicy::Online).await?;
```

Public construction/configuration types:

- `WalletIdentity { mint_url, unit }`
- `WalletOpenRequest::new(identity, store, [u8; 64])`
- `WalletOpenRequest::with_advanced(WalletOpenAdvancedOptions)`
- `Wallet::open(request)`
- `Wallet::identity()`
- `RestoreRequest { batch_size, max_gap }` and `Wallet::restore_from_seed`

`WalletBalance` contains `available`, `pending`, and `reserved`. Only
`available` is immediately spendable.

## Manager Rust API

```rust
use cdk::wallet::WalletManagerBuilder;

let manager = WalletManagerBuilder::new()
    .with_store(store)
    .with_seed(seed)
    .build()
    .await?;
```

Builder options include `with_proxy`, `with_rate_limiting_config`,
`with_rate_limiting_disabled`, and feature-gated `with_tor`. The debugging-only
`with_danger_accept_invalid_certs` disables TLS verification for proxied HTTPS
connections.

Manager operations:

| Method | Purpose |
| --- | --- |
| `register_mint(request)` | Fetch mint capabilities and register wallets for its supported units. |
| `configure_wallet(request)` | Create or replace one mint/unit wallet configuration. |
| `wallet(identity)` | Get an already configured wallet. |
| `open_wallet(identity)` | Get or locally create a wallet. |
| `wallets()` / `wallets_for_mint(url)` | List configured wallets. |
| `contains_wallet(identity)` / `contains_mint(url)` | Test configuration presence. |
| `forget_wallet(identity)` | Remove a wallet from the manager without deleting persisted mint data. |
| `mint_info(url)` | Fetch public mint capabilities. |
| `balances()` | Return balances for every wallet. |
| `available_balances()` | Return spendable amounts keyed by identity. |
| `balance_totals()` | Sum spendable amounts by currency unit. |
| `claim_pending_mints(mint?)` | Claim paid but unissued quotes. |
| `synchronize_all(policy)` | Synchronize every configured wallet. |
| `operations(query)` | Discover operations across all wallets. |
| `history_all(query)` | Return combined transaction history. |
| `plan_request_payment` / `pay_request` | Select a compatible wallet for NUT-18 payment. |
| `create_payment_request` | Create a receiver-side NUT-18 request. |
| `resume_payment_request_receiver` | Rebuild a persisted Nostr receiver. |
| `plan_cross_mint_transfer` / `transfer` | Move value between two configured wallets. |
| `resume_transfer(operation_id)` | Continue a cross-mint transfer. |

There is no manager-wide event receiver; subscribe to each wallet's events.

## Incoming minting

`MintRequest` contains a payment `method`, optional fixed `amount`, optional
`description`, and method-specific `extra` JSON. The convenience constructor
`MintRequest::bolt11(amount)` creates a fixed BOLT11 request.

`Wallet::request_mint` returns a durable `MintSession`:

```text
id()
initial_state()
refresh()
claim() / claim_with(options)
wait(timeout) / wait_with(options, timeout)
receipts(options)       // native Rust stream
```

`MintSessionState` reports the quote ID, payment request, `Unpaid`/`Paid`/
`Issued` state, requested amount, cumulative `amount_paid`, cumulative
`amount_claimed`, expiry, and payment method.

`MintReceipt.amount` is cumulative for `claim()`. The native `receipts` stream
reports each newly issued batch. `wait()` may return a new batch or a locally
known cumulative result; use `refresh().amount_claimed` when an explicit
cumulative total is needed.

## Ecash send and receive

`SendRequest` contains `amount`, `mode`, optional token `memo`, `include_fee`,
and application `metadata`. Modes are:

- `OnlineExact`
- `OnlineTolerant { tolerance }`
- `OfflineExact`
- `OfflineTolerant { tolerance }`

`plan_send` returns a durable `SendPlan` with `operation_id()`, `amount()`, and
`fee()`. Call `execute()` for a `SendReceipt` or `cancel()` to release the
reservation. `send(request)` is the prepare-and-execute convenience method.

`SendReceipt` contains the durable operation ID, token value, reserved fee,
and encoded `Token`. The token value may exceed the requested amount when
receiver fees or tolerated overpayment apply.

Other send methods:

- `resume_send(operation_id)` reconstructs a prepared/token-created plan.
- `pending_send_ids()` lists confirmed sends still being tracked.
- `send_status(operation_id)` returns `Unclaimed` or `Claimed`.
- `reclaim_send(operation_id)` restores an unclaimed send's amount.

`ReceiveRequest::new(encoded_token)` and `Wallet::receive` redeem an encoded
token. `ReceiveRequest::from_bytes` is Rust-only convenience; the receipt
contains credited amount and wallet identity.

## Outgoing payments

`PaymentTarget` supports:

- BOLT11 invoice, amountless invoice, and MPP amount;
- BOLT12 offer and amountless offer;
- on-chain address with optional maximum fee;
- custom mint-advertised method, request, amount, and JSON extras.

`PaymentQuoteRequest::new(target)` is quoted by `quote_payment`. The result is
`PaymentQuoteResult::Single(PaymentSession)` for one quote or
`PaymentQuoteResult::Options(Vec<PaymentSession>)` for on-chain fee/confirmation
alternatives. Choose an option explicitly; `into_single()` rejects `Options`.

`PaymentSession` exposes `id`, `quote`, `refresh`, `prepare`, `execute`, and
`submit`. Its `PaymentQuote` contains ID, amount, fee reserve, state, expiry,
optional estimated blocks, and payment method.

`prepare()` returns a durable `PaymentPlan`:

```text
operation_id()
quote_id()
amount()
maximum_fee()
wallet_identity()
execute() / execute_with(options)
submit() / submit_with(options)
cancel()
```

`execute` waits for final settlement. `submit` returns either
`PaymentConfirmation::Completed(PaymentReceipt)` or
`PaymentConfirmation::Pending(PendingPayment)`. A pending payment has
`quote_id`, `operation_id`, and `wait()`.

`maximum_fee` includes quote, swap, and input fees. `PaymentReceipt` includes
operation/quote IDs, delivered amount, actual fee, and an optional settlement
proof such as a Lightning preimage.

`AddressPaymentRequest` supports Lightning addresses and, when enabled,
BIP-353 or automatic route selection. Its amount is in millisatoshis.

## NUT-18 payment requests

Senders use `RequestPayment` with a payment request, optional amount, optional
mint selection, and fee/debit limits. `plan_request_payment` returns a
`RequestPaymentPlan`; `pay_request` executes it directly.

The plan exposes:

```text
operation_id(), wallet(), requested_amount(), method(), method_fee(),
payment_amount(), input_fee(), total_amount(), execute(), cancel()
```

Receivers use `CreatePaymentRequest`, which selects amount/unit, description,
P2PK/HTLC lock, out-of-band/HTTP/Nostr transport, mint policy, and supported
methods. It returns `CreatedPaymentRequest` with the encoded request and an
optional `PaymentRequestReceiver`. A Nostr receiver supports `state()`,
`receive()`, and `receive_with_timeout(...)`; persist its state securely before
process termination.

## Cross-mint transfers

`CrossMintTransferRequest` identifies source and destination wallets and uses
either `Exact(amount)` or `Maximum`. A transfer plan exposes its operation ID,
amount, maximum fee, destination, destination quote ID, `execute`, and `cancel`.

Execution returns:

- `Completed`, when source payment and destination issuance finish; or
- `ClaimPending`, when the source already paid and the destination quote still
  needs claiming.

`ClaimPending` must resume the original operation; it must not be retried as a
new transfer.

## Operations, synchronization, history, and events

`OperationQuery` filters by `OperationKind`, `OperationState`, and limit.
Kinds are `Mint`, `Send`, `Receive`, `Payment`, `Transfer`, and `Reissue`.
States include `AwaitingPayment`, `Ready`, `AwaitingExecution`, `Processing`,
`Pending`, `NeedsRecovery`, `Completed`, `Canceled`, and `Failed`.

Each `OperationSummary` includes its wallet, stable quote/workflow reference,
kind, state, amount/timestamps where known, and a typed `OperationResume`
instruction (`Mint`, `Send`, `PaymentQuote`, `Payment`, `Transfer`, or
`Synchronize`).

`SyncPolicy::LocalOnly` never contacts the mint. `SyncPolicy::Online` reconciles
quotes, proofs, interrupted sagas, and pending payments. `SyncReport` includes
the resulting balance, recovery/compensation/failure counts, claimed amount,
finalized payments, unresolved amount, and per-operation updates. Unresolved
amount remains unavailable; it is not spendable recovery.

`HistoryQuery` filters by direction and limit. `HistoryEntry` contains
transaction ID, wallet, direction, amount, fee, timestamp, memo, metadata,
quote/operation linkage, payment method, and status. It intentionally does not
expose proof identifiers or settlement secrets.

`Wallet::events()` produces `WalletEventReceiver`. Events are
`BalanceChanged`, `OperationChanged`, `TransactionChanged`, and
`MintPaymentReceived`. Rust consumes with `receiver.next().await`. A lagged
receiver must be repaired by querying current balance, operations, and history.

## Advanced Rust API

Expert controls are grouped under `wallet.advanced()` and mutable connector
configuration under `wallet.advanced_mut()`. This keeps ordinary application
workflows small without removing protocol functionality.

Advanced wallet capabilities include:

- coherent mint metadata/keyset snapshots and cache controls;
- proof queries, NUT-07 checks, synchronization, release, and reconciliation;
- explicit proof reissue/import and fee estimation;
- token validation and decoded/raw transaction inspection/recovery;
- P2PK signing-key creation/listing and authentication token management;
- low-level quote/proof subscriptions and native proof/payment streams;
- raw mint quote import, NUT-29 batch refresh/claim, and local quote selection;
- optional npub.cash controls and NWC handler/key derivation.

Advanced options control output denominations, spending conditions, P2PK/HTLC
credentials, P2BK protection, locked-proof pass-through/reissue, explicit
payment funding (`Wallet`, `Proofs`, or encoded `Token`), and payment swap
policy.

## FFI API

`cdk-ffi` is generated with UniFFI for Swift, Kotlin, Python, Dart, and Go.
The normal FFI surface mirrors the workflow objects, but uses binding-safe
records/enums and reference-counted objects.

### Opening

```python
import cdk_ffi as cdk

wallet = cdk.Wallet.open(cdk.WalletOpenRequest(
    mint_url="https://mint.example.com",
    unit=cdk.CurrencyUnit.SAT(),
    mnemonic=existing_mnemonic,
    store=cdk.WalletStore.SQLITE(path="wallet.sqlite"),
    config=None,
))

balance = await wallet.balance()
await wallet.synchronize(cdk.SyncPolicy.ONLINE)
```

`WalletManager.open` accepts `WalletManagerOpenRequest` with mnemonic, store,
optional proxy URL, and optional rate limit.

FFI representations differ in these important ways:

| Rust | FFI |
| --- | --- |
| `[u8; 64]` seed | BIP-39 mnemonic string |
| `OperationId` / quote ID newtypes | `String` |
| `Amount` | `{ value: u64 }` record |
| `Token` in a send receipt | Encoded token `String` |
| `Duration` | Integer seconds on exported timeout methods |
| Borrowed advanced handles | `Arc`/reference-counted binding objects |
| Tuples/maps in aggregates | Entry records/lists |

The FFI mnemonic path derives with an empty BIP-39 passphrase. It does not
currently expose a raw-seed or passphrase constructor. `mnemonic_to_entropy`
returns mnemonic entropy, not the derived 64-byte seed.

### Normal FFI methods

The FFI `Wallet` exports `open`, `identity`, rate-limit controls, `balance`,
`synchronize`, `operations`, `events`, all mint/send/receive/payment methods,
history, restore, and the typed resume methods. `WalletManager` exports mint
registration/configuration, wallet lookup/listing, balances, pending mint
claims, NUT-18 request workflows, cross-mint transfers, synchronization,
operations, and combined history.

FFI session/plan objects expose the same lifecycle methods as Rust. Streams are
adapted as objects with `next()` rather than Rust `Stream` implementations.

### FFI advanced feature

The `advanced-wallet` feature adds `wallet.advanced_wallet()` and
`manager.advanced_manager()`. The advanced wallet bridge exports explicit mint
claim options, advanced send/receive, explicit payment funding, payment swap
policy, metadata, proof inspection/reconciliation, reissue/import, fee
estimation, token validation, raw transaction recovery, subscriptions, signing
keys, authentication, and npub.cash methods.

Some native Rust methods intentionally remain Rust-only, including mutable
connector replacement, raw quote/batch helpers, native streams, and several
transport/auth plumbing methods.

## Storage and errors

FFI storage supports SQLite, optional PostgreSQL, and custom foreign-language
`WalletDatabase` callbacks. Built-in helpers are `sqlite_wallet_store`,
`postgres_wallet_store` (feature-gated), `custom_wallet_store`, and
`create_wallet_db`. Custom implementations must preserve durable saga,
proof-reservation, quote-reservation, and optimistic-locking semantics.

Rust callers receive `cdk::Error`; inspect `wallet_kind()` and `is_retryable()`.
FFI callers receive structured `FfiError` categories:

```text
InvalidInput, NotFound, InsufficientFunds, Payment, Conflict,
Authentication, Unsupported, Network, Storage, Internal
```

`FfiError::Cdk` also includes the protocol/local error code, message,
retryability, and sometimes a durable `operation_id`. Do not parse error text
or blindly repeat a payment after a retryable error; rediscover and resume the
original operation.

## Security and lifecycle rules

- Persist quote IDs and operation IDs before external payment side effects.
- Dropping a plan does not cancel it; call `cancel` explicitly.
- Treat seeds, tokens, receiver state, auth tokens, signing keys, preimages,
  and settlement proofs as secrets.
- Use online synchronization after restart before presenting balances as
  settled.
- Subscribe to events before beginning work when every transition matters.

Implementation sources:

- [Rust wallet module](/Users/asm/.codex/worktrees/71b2/cdk/crates/cdk/src/wallet/mod.rs)
- [Rust workflow operations](/Users/asm/.codex/worktrees/71b2/cdk/crates/cdk/src/wallet/operation.rs)
- [Rust advanced API](/Users/asm/.codex/worktrees/71b2/cdk/crates/cdk/src/wallet/advanced.rs)
- [FFI workflow API](/Users/asm/.codex/worktrees/71b2/cdk/crates/cdk-ffi/src/wallet_api.rs)
- [FFI advanced API](/Users/asm/.codex/worktrees/71b2/cdk/crates/cdk-ffi/src/wallet_advanced.rs)
