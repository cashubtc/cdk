# Wallet API architecture

`cdk::wallet` has one application workflow model. Rust applications and the
generated Swift, Kotlin, Python, Dart, and Go bindings use the same concepts:
requests create resumable sessions or plans, plans expose reviewable amounts
and fees, and execution returns receipts. Protocol internals remain
available through an explicit `advanced()` boundary.

There is no second wallet facade or manually mirrored wallet trait.

## Object model

`Wallet` owns the state for exactly one mint and one currency unit.
`WalletManager` owns a seed, a store, shared transport policy, and any number of
`Wallet` values.

```text
WalletManager
  ├── Wallet(mint A, sat)
  ├── Wallet(mint A, usd)
  └── Wallet(mint B, sat)
```

Open `Wallet` directly when an application has one known mint-and-unit pair.
Use `WalletManager` for discovery, portfolio balances and history, NUT-18
payment requests, or cross-mint transfers. Opening either object loads local
state only; network reconciliation is explicit.

## Functionality map

| Need | Application API |
|---|---|
| Open one wallet | `Wallet::open(WalletOpenRequest)` |
| Open a managed collection | `WalletManagerBuilder` |
| Register or configure mints | `register_mint`, `configure_wallet`, `open_wallet`, `forget_wallet` |
| Balances and startup recovery | `balance`, `balances`, `synchronize`, `synchronize_all` |
| Request and claim incoming value | `request_mint` → `MintSession` |
| Send ecash | `send`, or `plan_send` → `SendPlan` → `execute` / `cancel` |
| Receive ecash | `receive(ReceiveRequest)` |
| Pay Lightning, BOLT12, on-chain, or a custom rail | `quote_payment` → `PaymentSession` → `PaymentPlan` |
| Pay a Lightning/BIP-353 address | `quote_address_payment` |
| Discover durable work after restart | `operations(OperationQuery)` |
| Resume a durable operation | `resume_mint`, `resume_send`, `resume_payment_quote`, `resume_payment`, `resume_pending_payment`, `resume_transfer` |
| Inspect or reclaim an unclaimed send | `send_status`, `reclaim_send` |
| NUT-18 send side | `pay_request`, or `plan_request_payment` for review |
| NUT-18 receive side | `create_payment_request`, `resume_payment_request_receiver` |
| Move funds between mints | `transfer`, or `plan_cross_mint_transfer` for review |
| Restore and history | `restore_from_seed`, `history`, `history_all` |
| Observe application state | `events()` → `WalletEventReceiver` |
| Proofs, keysets, subscriptions, auth, Nostr backup, or custom protocol controls | `advanced()` |

Application types are grouped by domain instead of being flattened into
`cdk::wallet`: `wallet::mint`, `send`, `receive`, `payment`,
`payment_request`, `transfer`, `operation`, `history`, and `events`.
Each domain owns its request types and workflow implementation. Send,
receive, and payment-request application entry points live in their domain's
`api.rs`; protocol engines and durable state machines remain beside them.

## Open and synchronize

```rust,no_run
use std::sync::Arc;

use cdk::nuts::CurrencyUnit;
use cdk::wallet::operation::SyncPolicy;
use cdk::wallet::{Wallet, WalletIdentity, WalletOpenRequest};
use cdk_sqlite::wallet::memory;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let store = Arc::new(memory::empty().await?);
let identity = WalletIdentity::new(
    "https://mint.example.com".parse()?,
    CurrencyUnit::Sat,
);
let wallet = Wallet::open(WalletOpenRequest::new(identity, store, [0; 64]))?;

let local_balance = wallet.balance().await?;
let report = wallet.synchronize(SyncPolicy::Online).await?;
println!("{} available", report.balance.available);
# let _ = local_balance;
# Ok(())
# }
```

`SyncPolicy::LocalOnly` never contacts a mint. `SyncPolicy::Online` recovers
interrupted operations, claims paid mint quotes, reconciles pending proofs, and
finalizes pending payments. `SyncReport::operations` describes each quote or
workflow examined, including its previous and resulting state, stable error
category, and retry guidance. Startup side effects are never hidden in
construction.

`SyncReport::unresolved_amount` is orphaned pending value that the mint still
reports as unspent or pending. It is **not** recovered spendable balance;
only `balance.available` reports funds available to spend. Quote issuance and
payment settlement take precedence over expiry or an earlier transient error
when synchronization reports an operation's final state.

The durable operation index is the source of truth for rebuilding UI after a
restart. It includes quote-backed sessions created before any saga exists, and
every summary identifies its owning mint-and-unit wallet so manager-wide
queries remain unambiguous:

```rust,no_run
use cdk::wallet::operation::{OperationQuery, OperationResume};
use cdk::wallet::Wallet;

# async fn example(wallet: &Wallet) -> Result<(), cdk::Error> {
for operation in wallet.operations(OperationQuery::active()).await? {
    match operation.resume {
        OperationResume::Mint { quote_id } => {
            let _session = wallet.resume_mint(quote_id).await?;
        }
        OperationResume::Send { operation_id } => {
            let _plan = wallet.resume_send(operation_id).await?;
        }
        OperationResume::PaymentQuote { quote_id } => {
            let _session = wallet.resume_payment_quote(quote_id).await?;
        }
        OperationResume::Payment {
            operation_id,
            pending: false,
            ..
        } => {
            let _plan = wallet.resume_payment(operation_id).await?;
        }
        OperationResume::Payment {
            operation_id,
            pending: true,
            ..
        } => {
            let _payment = wallet.resume_pending_payment(operation_id).await?;
        }
        OperationResume::Transfer { .. } | OperationResume::Synchronize => {}
    }
}
# Ok(())
# }
```

For multiple mints, build a manager and register or open wallets explicitly:

```rust,no_run
use std::sync::Arc;

use cdk::wallet::{MintRegistrationRequest, WalletManagerBuilder};
use cdk_sqlite::wallet::memory;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let manager = WalletManagerBuilder::new()
    .with_store(Arc::new(memory::empty().await?))
    .with_seed([0; 64])
    .build()
    .await?;
manager
    .register_mint(MintRegistrationRequest::new(
        "https://mint.example.com".parse()?,
    ))
    .await?;
# Ok(())
# }
```

## Incoming value

```rust,no_run
use std::time::Duration;

use cdk::wallet::mint::MintRequest;
use cdk::wallet::Wallet;
use cdk::Amount;

# async fn example(wallet: &Wallet) -> Result<(), cdk::Error> {
let session = wallet
    .request_mint(MintRequest::bolt11(Amount::from(1_000)))
    .await?;
println!("Pay: {}", session.initial_state().payment_request);

let quote_id = session.id().clone(); // persist before waiting
let receipt = session.wait(Duration::from_secs(300)).await?;
println!("Minted {}", receipt.amount);

// After restart: wallet.resume_mint(quote_id).await?
# let _ = quote_id;
# Ok(())
# }
```

`refresh` checks a session without claiming it. `claim` and `wait` are
idempotent with respect to already-issued quote value, including partial and
multi-part issuance supported by a mint.

For reusable quotes, `claim` checks for subsequent payments and returns the
cumulative issued value. Use `receipts` to receive each newly issued batch.
`wait` returns the next batch, or the locally known total if already issued;
refresh a reusable session before waiting for additional paid value.

## Send and receive ecash

```rust,no_run
use cdk::wallet::receive::ReceiveRequest;
use cdk::wallet::send::SendRequest;
use cdk::wallet::Wallet;
use cdk::Amount;

# async fn example(sender: &Wallet, receiver: &Wallet) -> Result<(), cdk::Error> {
let plan = sender
    .plan_send(SendRequest::new(Amount::from(100)).with_memo("lunch"))
    .await?;
println!("Send {} with fee up to {}", plan.amount(), plan.fee());

let operation_id = plan.operation_id(); // persist before execution
let sent = plan.execute().await?;
let received = receiver
    .receive(ReceiveRequest::new(sent.token.to_string()))
    .await?;
println!("Received {}", received.amount);

// After restart: sender.resume_send(operation_id).await?
# let _ = operation_id;
# Ok(())
# }
```

For the common path, `sender.send(request)` prepares and executes in one call.
An interruption can still leave a durable operation; discover it with
`operations` and resume it before retrying. Planning is for applications that need a
review screen: it reserves proofs until `execute` or `cancel`, and dropping a
plan does not cancel it. An executed token remains inspectable with
`send_status` and can be
reclaimed with `reclaim_send` while it is still unclaimed.

`SendPlan::amount` is the requested transfer value. `SendReceipt::amount` is
the actual token value, which can be higher when receiver fees are included
or the selected send mode permits overpayment.

## Outgoing payments

`PaymentTarget` distinguishes BOLT11 invoices, BOLT12 offers, on-chain
addresses, and custom payment rails. On-chain quoting may return multiple
sessions representing fee/confirmation alternatives.

```rust,no_run
use cdk::wallet::payment::{PaymentConfirmation, PaymentQuoteRequest, PaymentTarget};
use cdk::wallet::Wallet;

# async fn example(wallet: &Wallet, invoice: String) -> Result<(), cdk::Error> {
let session = wallet
    .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice)))
    .await?
    .into_single()?;
println!(
    "Pay {} with at most {} mint fee",
    session.quote().amount,
    session.quote().fee_reserve
);

let plan = session.prepare().await?;
let operation_id = plan.operation_id(); // persist before submission
match plan.submit().await? {
    PaymentConfirmation::Completed(receipt) => {
        println!("Paid with fee {}", receipt.fee_paid);
    }
    PaymentConfirmation::Pending(payment) => {
        let receipt = payment.wait().await?;
        println!("Final fee {}", receipt.fee_paid);
    }
}

// After restart: wallet.resume_payment(operation_id).await?
# let _ = operation_id;
# Ok(())
# }
```

Use `execute` when the caller wants to wait for final settlement. Use `submit`
when the mint may accept the payment for asynchronous processing. The same
verbs are available directly on `PaymentSession` when no review step is
needed. A successful payment is always a `PaymentReceipt`; failed and
indeterminate states remain errors, never success-shaped receipts.

## Durable lifecycle and concurrency

Every fund-reserving workflow follows the same lifecycle:

```text
request → persisted session/plan → execute ──→ receipt
                                ├ submit  ──→ receipt or pending handle
                                ├ cancel  ──→ funds released
                                └ restart ──→ discover and resume by typed ID
```

The database saga is authoritative. Handles contain a wallet reference, stable
identifier, and immutable preview values; they reload persisted state for
execute, submit, cancel, wait, and recovery. Per-operation locks serialize duplicate
actions on the same plan, while distinct operations may proceed concurrently.
Proof reservation prevents concurrently prepared operations from selecting the
same funds.

Quote updates also compare the reservation owner atomically, so a stale
on-chain fee selection cannot release another operation's quote. A competing
selection that changes the reviewed fee fails preparation; canceling a plan
allows selecting another fee option for the same quote.

Persist the quote or operation ID before an external side effect. On startup,
open the wallet or manager, call online synchronization, then reconstruct any
operation still shown to the user. Synchronization preserves a plan that is
still waiting for an explicit execute/cancel decision and reconciles operations
whose execution was interrupted.

## Cross-mint transfers

`WalletManager::plan_cross_mint_transfer` creates one durable source payment
and destination issuance flow. Persist its source operation ID. Execution
returns either:

- `Completed`, when payment and destination issuance both finish; or
- `ClaimPending`, when the source payment succeeded but the already-paid
  destination quote still needs to be claimed.

`ClaimPending` must not be retried as a new transfer. Resume the original plan
with `resume_transfer`, call its `execute` again, or let
`synchronize_all(Online)` claim the destination quote.

## NUT-18 payment requests

The sender can use `pay_request` directly, or use `plan_request_payment` and
review a `RequestPaymentPlan` before execution. The receiver uses
`create_payment_request`; when the
selected transport requires a Nostr listener, persist
`PaymentRequestReceiver::state()` and reconstruct it after restart with
`resume_payment_request_receiver`. `receive_with_timeout` supports bounded
background checks without rebuilding protocol clients in application code.

## Application events

`Wallet::events()` emits balance snapshots, operation transitions,
transaction changes, and incoming mint payments. These are application
events—not NUT-17 wire notifications—and never expose proof identifiers.
Subscribe before starting work when every transition matters. A lagged receiver
returns a typed `WalletEventError::Lagged`; rebuild current state with
`balance`, `operations`, and `history` rather than guessing which events were
lost.

## Normal and advanced APIs

The ordinary `Wallet` and `WalletManager` surfaces expose complete application
workflows. Expert operations are grouped under:

```rust,ignore
wallet.advanced()          // proofs, keysets, subscriptions, auth, raw imports
wallet.advanced_mut()      // mutable connector configuration
manager.advanced()         // token inspection, Nostr backup, npub.cash controls
```

Expert construction is explicit as well. A standalone request accepts
`WalletOpenAdvancedOptions` through `with_advanced`; managed mint requests
accept `MintAdvancedOptions` through `with_advanced`. Proof-shaping, connector,
and metadata-cache knobs therefore remain available without appearing in the
default constructors.

The boundary is organizational, not a second implementation. Advanced methods
delegate to the same wallet, database, transport, and durable state machines.
Use it only when the application deliberately owns protocol-level choices such
as denominations, explicit proofs, spending conditions, authentication, or
subscription filters.

## Rust and generated bindings

`cdk-ffi` exports thin UniFFI records and objects over these same workflows.
Names and lifecycles intentionally match the Rust API: `Wallet`,
`WalletManager`, `MintSession`, `SendPlan`, `PaymentSession`, `PaymentPlan`,
`PendingPayment`, operation discovery, events, receipts, and synchronization.
Protocol-level binding objects are compiled only with the `advanced-wallet`
feature; then `advanced_wallet` / `advanced_manager` expose locked-proof,
explicit-funding, raw-import, and subscription controls. Wallet business rules
belong in `cdk`, not in language-specific bindings.

## Migration from the previous public surface

| Previous operation | Current workflow |
|---|---|
| constructor with positional mint/store/seed arguments | `Wallet::open(WalletOpenRequest)` |
| wallet repository | `WalletManager` |
| raw mint quote + status + mint calls | `request_mint` → `MintSession` |
| prepare/confirm send primitives | `send`, or `plan_send` → `SendPlan::execute` |
| raw receive options | `receive(ReceiveRequest)` |
| raw melt quote + prepared melt variants | `quote_payment` → `PaymentSession` → `PaymentPlan` |
| async melt outcome | `PaymentConfirmation` / `PendingPayment` |
| remembered operation IDs and direct saga recovery | `operations(OperationQuery)` + typed `resume_*`, then `synchronize(SyncPolicy)` |
| raw transactions and reversal calls | `history`, `send_status`, `reclaim_send` |
| proof/keyset/auth/subscription methods on `Wallet` | `wallet.advanced()` |

This is a breaking replacement. Removed names are not retained as deprecated
aliases, compatibility traits, wrappers, or forwarding facades.

Persisted operations are different from public API compatibility: recovery
still reads the `ProofsReserved` send and payment records written by the
previous wallet. Upgrading must not strand existing reservations. Custom
database implementations must honor the owner-and-version checks documented
on `add_melt_quote`; built-in stores enforce them without a schema migration.

## Design references

[cashu-ts](https://github.com/cashubtc/cashu-ts) separates operation builders
from application-wide events, but delegates proof persistence to applications.
CDK keeps those responsibilities in its durable wallet engine because recovery
and reservation safety are part of this SDK's contract.

[BDK](https://github.com/bitcoindevkit/bdk) separates its high-level wallet
from lower-level mechanisms, and [bdk-ffi](https://github.com/bitcoindevkit/bdk-ffi)
uses UniFFI for target-language APIs. These are useful boundaries for CDK:
domain-owned Rust workflows, an explicit expert surface, and shared generated
bindings. They do not require another wallet facade or a second implementation
of payment and recovery policy in the FFI crate.

## Errors and secrets

Rust callers receive `cdk::Error`, whose `wallet_kind()` and `is_retryable()`
methods define the shared application policy. Foreign-language callers receive
the same stable category and retryability values through `FfiError`.
Applications should
branch on structured categories, not parse messages.

When delivery of a NUT-18 payment-request token fails after the token was
created, `FfiError` also carries its durable `operation_id`; use that ID with
`send_status` or `reclaim_send` instead of repeating the payment.

Wallet seeds, bearer tokens, proof secrets, payment preimages, proxy
credentials, and encoded Cashu tokens are redacted from `Debug` output where
their enclosing public types implement it. History is application-facing and
does not expose proof identifiers or secret material. Treat token strings and
serialized receiver state as secrets when persisting or logging them.
