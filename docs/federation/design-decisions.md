# Federation design decisions

These decisions shape how the federation is built. Each states what the code
does and why. They apply on top of two standing rules:

- **Cut-through.** Federation is undeployed. Federation-only formats, schemas,
  wire types and APIs change in place, with regenerated golden vectors and no
  compatibility layer or migration. Standalone CDK behaviour, APIs, defaults
  and serialization are a compatibility boundary and do not change.
- **Reuse before parallel owners.** Prefer the existing CDK owners (mint sagas,
  `MintPayment`, `Transport`, `MintDatabase` transactions) over new parallel
  ones.

## 1. Key types do not change with a feature

`cashu` has one curve-aware `PublicKey`/`SecretKey` enum and always compiles
BLS12-381 and v3 keysets (`KeySetVersion::Version02`), as on the upstream `bls`
branch. No feature changes the shape of a public type; secp256k1-only
operations are fallible `try_*` methods.

Why: Cargo unifies features across a build, so a feature that changes a public
type breaks any downstream crate compiled next to a crate that enables it.
Following upstream `bls` keeps the protocol types from forking.
`scripts/check_standalone_api.py` builds an ordinary consumer in isolation and
checks that it sees the same key types and that the custody crates stay out of
its dependency tree.

## 2. Standalone mint code is not split into validate/plan/commit

Shared standalone mint code stays as it is. Federated application keeps its own
commit paths for swap, mint and melt, the duplicate share-table write and the
two quote-and-settings payment adapters (`cdk-bark`'s and `cdk-mintd`'s); there
is no custody trait. Shared rules are reused where they are already separate
functions (output, input, fee and balance verification through `KeysetView`,
`plan_swap`, quote checks, `finalize_melt_core_in_transaction`).

Why: removing the duplicated commit paths needs a seam in standalone mint code,
and the risk to the standalone mint outweighs the duplication. Instead, a
parity test (`crates/cdk/src/mint/federation/tests/parity.rs`) runs the same
request vectors through the standalone and federated swap, mint and melt paths
and asserts identical signatures, fees, spent proofs, quote states and errors.
A divergence in either path fails that test.

## 3. Bark travels as a generic application operation

Bark operations use the existing `FederationOperation::Application` variant,
with conflict keys and materialized keys in the Bark application's namespace.
There is no process-global codec registry and no start-up registration.

Why: a typed Bark variant in `cdk-common` would require `cdk-common` to name
`cdk-federation-bark`'s types, which depend on `cdk-common`. The generic variant
avoids both that cycle and the registry.

## 4. One node crate; the pure domain stays in `cdk-common`

`cdk-federation` owns the federation node: AlephBFT session, sync and catch-up,
checkpoints, peer client, DKG execution, FROST signing, payment and custody
workers, node construction (`FederationNode::prepare`) and the setup ceremony
library. `cdk-axum` keeps only routes, request extraction and authentication.
The pure protocol domain (operations, envelopes, journal, checkpoints, config,
BLS DKG planner, setup transcript) stays in `cdk-common::federation`.

Why: the node used to live in the HTTP adapter. Extracting it fixed the
dependency direction without a larger crate reorganization.

## 5. Consensus hashing uses canonical JSON

Operation IDs and other consensus hashes use sorted canonical JSON from one
module (`crates/cdk-common/src/federation/canonical.rs`), pinned by golden
vectors. `serde_json`'s `preserve_order` stays enabled workspace-wide.

Why: one module and golden vectors make the encoding stable and reviewable.
Moving to a binary encoding is reconsidered only if hashing shows up in a
profile. The setup ceremony is the exception: its transcript is a fixed binary
encoding (see [setup ceremony](setup-ceremony.md)).

## 6. The wallet persists its federation registry in the KV store

`cdk::wallet::FederationRegistry` stores each discovered federation as JSON in
the wallet KV store (primary namespace `federation`, secondary `registry`, key
= federation ID). There is no `WalletDatabase` trait change and no migration.
Rediscovery through any member refreshes the entry. The CLI, FFI and TUI all use
this registry.

Why: every wallet backend already implements the KV store, so persistence needs
no per-backend work.

## 7. The TUI is a workspace crate; `docs/` holds current design only

`crates/cdk-fed-tui` (binary `fed-cdk-tui`) is a workspace member on the shared
registry, built and linted in CI. Under `docs/`, only condensed current design
documents for shipped behaviour are kept; dated plans, ledgers and reviews live
on an archive branch outside the pull request.

## 8. FROST nonces stay in redb, never restored from backup

FROST signing nonces live in their own redb files
(`frost-nonces/root-<epoch>.redb` under the signing storage root), apart from
the mint's SQL database. They are excluded from backups and never restored. A
lost or suspect nonce file is deleted; the member generates fresh nonces.

Why: a restored nonce store offers consumed nonces again, and a nonce used for
two different messages reveals the member's key share. Keeping the store out of
the SQL database means restoring a database backup cannot bring consumed nonces
back. See [operations](operations.md#frost-nonce-files-never-restore).

## 9. A stale member halts and is reset explicitly

A member that still holds federation state but lies below every peer's
retention floor halts with the typed status `ResetRequired`
(`sync_state = "reset_required"`). The operator stops it and runs
`cdk-mintd federation reset-member --confirm-wipe`, which wipes the federation
and mint rows (keeping key material, configuration and the FROST nonce file).
The member then rejoins by state transfer.

Why: such a member cannot catch up by replay, and state transfer installs only
into an empty member. Deleting a member's data automatically would be a silent
loss; nothing is deleted without the explicit command. See
[storage and retention](storage-and-retention.md#stale-member-halt-and-reset).

## Patterns worth keeping

- The `MintConnector` seam on the wallet side: `FederatedMintConnector` fans out
  to members and returns ordinary aggregate Cashu responses.
- Prepare, persist, commit on the consensus runtime; an operation's application
  effect and its journal status commit in one mint transaction.
- No I/O in the `cdk-common` federation modules; the pure BLS DKG planner.
- `cdk-frost` wraps a FROST library with an explicit nonce lifecycle and
  authorization that re-derives the signed message.
- Iroh is a decorator on the existing `Transport`; one `BdkWalletEngine` serves
  standalone and federated on-chain wallets.
- `FederationTaskGroup`; the capability types `VerifiedFederationSuffix`,
  `VerifiedFederationCheckpointQuorum` and `FederationAcceptedSigningRequest`,
  which can only be obtained through verification; `deny_unknown_fields` with
  explicit size bounds on decoded inputs.
- Lifecycles follow one pattern: one state enum, one transition function in the
  domain, and storage writers that condition on the expected source state.
