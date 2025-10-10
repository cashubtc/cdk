# Mining Share Support Migration Plan

This port needs to land in coherent stages so that each batch of changes
builds cleanly before moving on.  Here's the sequence I’ll follow:

1. **Core Type Enablement**
   - Introduce `PaymentMethod::MiningShare`, `MintQuoteMiningShare*` structs,
     and `PaymentIdentifier::MiningShareHash` in the shared `cashu`/`cdk-common`
     crates.
   - Extend public APIs (notification payloads, events) just enough for the
     new types to compile without touching runtime logic yet.
   - `cargo check` to validate the broader workspace still builds.

2. **Persistent Model Updates**
   - Add `keyset_id` support to `cdk-common` wallet/mint structs and update
     SQLite/Postgres backends (`cdk-sql-common`, `cdk-sqlite`), including
     migrations and legacy fixtures.
   - Ensure migrations are copied but not yet wired into build.rs (already
     handles includes) and that DB tests compile.
   - `cargo check`.

3. **Wallet Connector & Issue Flow**
   - Wire the mining-share endpoints into `wallet/mint_connector::{trait,http}`.
   - Add `wallet/issue/issue_mining_share.rs` and hook it into
     `wallet/issue/mod.rs`, plus extend wallet streams where needed.
   - Update CLI/runtime surfaces that assume only Bolt11/12 to handle the new
     payment method minimally.
   - `cargo check`.

4. **Mint Logic & PubSub**
   - Port `Mint::create_mint_mining_share_quote` and any supporting helpers
     (payments, pubsub notifications, LN bypass logic).
   - Extend mint subscription/pubsub code to index/broadcast the new quote
     events.
   - `cargo check`.

5. **API Surface & Ancillary Services**
   - Add axum routes/handlers for `/mint/quote/mining_share` and `/mint/mining_share`.
   - Reconcile payment-processor and any other RPC surface (e.g. mint RPC)
     to understand the new identifier type.
   - `cargo check`.

6. **Translator Integration & Final Wiring**
   - Once the library side is complete, reapply translator-specific glue if
     anything is still missing (e.g. proof sweeper expectations, config tweaks).
   - Run targeted builds/tests (translator role, unit tests) to make sure the
     end-to-end flow is intact.

At each boundary I’ll stop for a review opportunity so you can commit the
chunk before we continue.
