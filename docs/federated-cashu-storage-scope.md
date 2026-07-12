# Federated Cashu Storage Scope

This note records the current storage-backend scope for federated mint work.
Federation mint storage is in scope for backends that implement mint database
traits and can persist consensus-visible mint state, accepted journal entries,
private REST sequence watermarks, catch-up certificates, payment observations, and
future signature-share rows.

## Current Decision

`cdk-redb` is wallet-only today and is out of scope for mint federation storage.

Evidence in the current crate:

- `crates/cdk-redb/Cargo.toml` exposes a `wallet` feature and enables it by
  default.
- `crates/cdk-redb/src/lib.rs` exports `WalletRedbDatabase` only when the
  `wallet` feature is enabled.
- `crates/cdk-redb/src/wallet/mod.rs` implements `WalletDatabase` for
  `WalletRedbDatabase`.
- The crate does not implement `MintDatabase` or the federation journal,
  catch-up certificate, payment-observation, DKG, or setup storage APIs needed by a
  federated mint member.
- `crates/cdk-redb/README.md` describes the crate as a Redb storage backend for
  wallet functionality.

The name `mint_quotes` inside Redb code refers to wallet-side tracking of mint
quotes for wallets, not mint-server quote state.

## Implication For Federation

Federated mint storage work remains scoped to SQLite, Postgres, and in-memory
test storage unless a future Redb mint backend is introduced. Redb wallet
storage can still be used by wallets that talk to federated mints, but it does
not need federation journal migrations, SQL lock-ordering rules, or
mint-member catch-up semantics.

If Redb later adds mint storage, it must enter the federation checklist as a new
mint backend and provide equivalent tests for:

- Additive migrations and standalone-mode compatibility.
- Accepted journal persistence and replay.
- Private REST sequence watermarks.
- Session-outcome persistence and range queries.
- Payment-observation rows and quorum evaluation inputs.
- DKG/setup state if setup routes are backed by Redb.
- Dedicated federation signature-share rows if that schema is added.
- Restart catch-up and materialized-state digest convergence.

Until then, Redb remains a wallet database backend and is not a blocker for
federated mint storage parity.
