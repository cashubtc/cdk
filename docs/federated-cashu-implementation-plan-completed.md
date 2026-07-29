# Federated Cashu with CDK: implementation plan

> Historical implementation record. References to catch-up certificates
> describe the superseded experimental design. The current cut-through recovery
> design is
> [`federated-cdk-checkpoint-implementation-plan.md`](federated-cdk-checkpoint-implementation-plan.md).
> The current normative plan is
> [`federated-cashu-implementation-plan.md`](federated-cashu-implementation-plan.md),
> and the 2026-07-29 finding dispositions and proof are in
> [`cleanup-2026-07-29.md`](cleanup-2026-07-29.md). Unchecked payment-evidence
> and real-backend release items in this historical copy remain open; the
> document's fakewallet-scope completion statement is not a production
> readiness claim.

This is a working checklist for adding federated Cashu mint support to CDK while keeping the public Cashu wallet-to-mint protocol as close as possible to today's NUT endpoints. The plan assumes BLS blind signatures are the federation signing scheme, with locally generated key shares first and real DKG later.

Sandbox test note: run localhost-listener and real-network tests with network permission enabled from the first attempt (an escalated `cargo test ...` invocation in Codex), because `127.0.0.1:0` bind failures without it are environmental rather than regressions.

## Grand vision

The grand vision of this project is to evolve Cashu from a single-operator Chaumian eCash mint into a federated, threshold-secured eCash system where multiple mint members can run on independent servers, coordinate all state transitions through consensus, and jointly issue, swap, and melt Cashu tokens using BLS threshold blind signatures. Federation mode should preserve the existing Cashu wallet-to-mint protocol as much as possible while giving users stronger security and fault tolerance: no single operator should be able to inflate, censor, or unilaterally corrupt mint state, and the federation should continue operating when some members are offline as long as the signing and consensus thresholds are met. Every safety-critical mint operation must be serialized by the federation, stored in a shared consensus journal, replayable after outages, and applied deterministically so all honest members converge on the same database state. Wallets must be able to talk to multiple members, verify and aggregate threshold signature shares into ordinary Cashu proofs, and reject inconsistent federation metadata or bad shares. The system should start with static trusted-dealer key generation, fakewallet as a configured development payment backend, private REST networking, and locally testable consensus scaffolding, then grow toward durable storage, catch-up, member authentication, DKG, key rotation, and full mint/swap/melt integration. Federation is explicitly v3/BLS-only, standalone CDK behavior must remain unchanged, and the implementation should favor simple CDK-native designs while borrowing Fedimint's core safety lessons around consensus-before-signing, authenticated peers, deterministic state machines, and robust recovery.

## Scope

- Federate CDK mint issuance, swap, and melt state transitions; serve restore
  and check-state as threshold authoritative reads of accepted state.
- Let a federation-aware CDK wallet talk to multiple mint members and aggregate BLS blind signature shares into normal Cashu proofs.
- Serialize all safety-critical mint state transitions through consensus before any member returns a usable signature share.
- Keep existing standalone mint and wallet behavior unchanged unless federation mode is explicitly enabled.
- Do not preserve backward compatibility for old experimental federated Cashu formats or non-v3 federation keysets; federation is still undeployed, so cut through with the simplest safe v3/BLS-only design. This applies to old in-repo federation fixtures, invite shapes, config defaults, keyset metadata, and route semantics as well as any external-looking development artifacts. Regenerate development configs and test fixtures instead of carrying migration shims for formats that have never been deployed.
- Start with fakewallet as a normal configured CDK payment backend for local development and trusted-dealer key shares, then add payment observation quorum and real DKG.

## Hard keyset constraint

BLS blind signatures are only supported for Cashu keyset v3, represented in CDK as `KeySetVersion::Version02` and serialized with keyset ID prefix `02`. Federation mode must therefore be v3/BLS-only.

- Federation configs must only create, import, advertise, and validate aggregate v3/BLS keysets.
- Federation startup must reject any aggregate keyset, member public share, or local secret share that is not tied to a v3/BLS keyset.
- Federated mint, swap, and change-output signing paths must reject outputs
  targeting non-v3 keysets before consensus signing. Restore must reject
  non-v3 placeholders before reading or returning persisted shares.
- Federation-aware wallets must reject member sets that advertise mismatched aggregate keysets or any non-v3 federation keyset.
- Standalone non-federated mints remain unaffected and may keep supporting whatever keyset versions CDK otherwise supports.
- There is no legacy federation compatibility mode. Old experimental federation configs, invites, keyset metadata, fixtures, and route semantics should be regenerated or removed rather than migrated.

## Implementation philosophy

Fedimint should be treated as the main reference implementation for federation design questions, especially around consensus, DKG, peer authentication, catch-up, state-machine boundaries, and threshold signing safety. When there is ambiguity about which library, protocol pattern, or engineering method to use, inspect the Fedimint code and design first.

Cashu/CDK should still value simplicity over copying architecture wholesale. If a simpler design achieves the same safety property, preserves Cashu protocol compatibility, is easier to test, and fits CDK's existing structure better, prefer the simpler design. Copy invariants and lessons from Fedimint; do not blindly copy complexity.

Private signed REST is the mint-to-mint transport for this plan and can remain
the production transport when operators provide stable reachable federation URLs
through normal HTTPS, VPNs, reverse tunnels, Tor onion services, or equivalent
connectivity. Alternate NAT-traversing transports are out of scope and do not
affect federation correctness milestones.

Plan maintenance checkpoint: speculative transport/config abstractions and
refactor-only placeholders are not active checklist items. The plan marks the
implemented REST limits, status surface, response cache, pending waiter joining,
public mint/swap/melt routes, payment actor selection, and share provenance as
complete. The safety, DKG, and wallet-error recovery checklist is now complete;
remaining work belongs to independent review and deployment validation.

The interface to existing CDK code should be as small and thin as practical. Prefer logically organized federation modules with well-separated concerns over scattering federation conditionals through existing mint, wallet, HTTP, storage, or signatory code. Existing code should expose narrow hooks or adapters where needed, and most federation-specific behavior should live behind explicit feature-gated abstractions so reviews can reason about ownership, invariants, and blast radius.

## Development workflow

Commit changes to Git regularly after each coherent section, milestone, or independently testable checkpoint. Each commit should leave the repository in a reconstructable state: code, migrations, tests, documentation, and test results should make it clear what changed and how to verify it. Prefer small conventional commits over large mixed changes so federation development can be audited, bisected, reverted, or resumed cleanly.

Performance and correctness investigations may add narrowly scoped code instrumentation when existing logs and external profiling do not provide enough evidence. Capture an uninstrumented baseline first whenever practical, keep temporary timing spans, counters, and probes isolated from protocol behavior, and remove them before finalizing the checkpoint. Instrumentation may remain only when it is useful as opt-in, low-overhead production diagnostics with focused tests and documentation; do not retain noisy logging or measurement-only behavior on normal code paths.

Never commit secrets, generated private member configs, private key shares, machine-specific paths, or personally identifiable local environment details. Test fixtures may include deterministic fake keys only when they are clearly marked as non-production material.

## Blocking bugs

If a bug or design issue blocks safe progress, record it here before continuing milestone work. Blocking bugs must be fixed before dependent milestones continue.

A blocker is only resolved when the fix is tested and the code is clean: no debug scaffolding, temporary naming, fake shortcuts, relaxed validation, one-off tools, extra logs, generated artifacts, or other traces of the workaround remain in production paths.

Current blocking bugs:

- None in the AlephBFT idle-liveness checkpoint. The previously open bug was
  reproduced, isolated, repaired, and closed without restoring periodic idle
  unit creation.

Resolved blocking bugs:

- AlephBFT idle data-provider liveness is fixed. A fresh five-member,
  threshold-three process benchmark completed one mint in 7.78s, then left a
  second member's locally queued `mint_quote` and already-paid
  `mint_quote_payment` operations without a `data_provider_yielded` event for
  exactly the 90-second wallet member-fallback timeout. A fallback quote from
  another member yielded immediately; its data-bearing unit woke the stalled
  member, whose two older operations then yielded within 10ms and finalized.
  Upstream creator tracing confirms that `run_session` creates a preunit and
  awaits `DataProvider::get_data()` before sending that unit. CDK's indefinite
  inactive wait can therefore hold parent units needed by creators that are
  not themselves waiting in `get_data`; waking only the local submitter is not
  a sufficient distributed liveness mechanism. This is separate from the
  fixed 100-percent/periodic idle-CPU issue. The clean fix adds an authenticated,
  session- and operation-bound private REST activity wake. A member admitting
  public work wakes its local provider and broadcasts the control signal to the
  configured federation roster; peers use the existing bounded network-activity
  grace rather than pretending to own local work. Finalization shortens both
  body-hash and operation-ID activity to the one-second propagation tail.
  Rejected sessions do not consume sequence numbers, replays are rejected, and
  the wake never enters the accepted operation journal. A fresh fixed build
  completed cold and 15-second-post-idle mints in 7.76s and 4.95s with exactly
  six accepted operations, no fallback quote, no duplicate submission, no
  pending work, and identical member digests. Over a separate 30-second idle
  sample every DAG stayed at exactly 293 units and each debug member consumed
  only 0.64-0.65 CPU seconds, about 2.1% per process.
- AlephBFT pending-finalization liveness across healed partitions is fixed.
  The five-member live partition matrix repeatedly showed the exact operation
  durably pending after a fourth member returned: all queued private-REST
  retries to that member had succeeded, and temporary test-only probes proved
  the submitting member's signed recovery wake reached all three available
  peers, but one 10-second activity lease could still expire before an
  exact-quorum DAG caught up. The activity broadcaster now reloads durable
  pending operation IDs for the active session and renews one representative
  operation-bound lease halfway through the existing network-activity grace.
  It performs no timer, database read, REST request, or unit wake while no
  durable pending work exists, stops refreshing after finality removes the
  pending rows, and restores pending activity after restart. Temporary probes
  and the discarded retry-recovery-specific plumbing were removed. Five
  consecutive full partition-matrix runs passed in 43.41s, 55.72s, 48.01s,
  46.71s, and 54.64s; final cleanup runs passed in 57.20s and 56.52s, including
  an assertion that activity traffic remains flat for a full refresh interval
  after pending work clears. A final deterministic 63.98s run additionally
  held the federation below finality through one complete refresh interval,
  proved activity renewed while quote and journal state remained untouched,
  then restored quorum and converged.

## Recent federation security checkpoint

- Federated mint issuance now requires quorum-satisfied payment observation
  evidence at or before the accepted mint operation index. A locally paid quote
  is not sufficient, and later observations cannot retroactively authorize an
  older mint operation.
- Federated swap apply rejects cross-operation reuse of already-signed outputs
  before fresh proofs are inserted or spent. Exact replay remains tied to the
  completed operation and its persisted signature-share provenance.
- `FederationMeltQuoteOperation` now follows the mint-quote consensus pattern:
  it carries the canonical `MeltQuoteCreateResponse` plus only internal backend
  metadata absent from the public response. The proposer prepares without
  persistence, apply performs no backend call, and all members materialize the
  accepted Bolt11, Bolt12, onchain, or custom terms with deterministic journal
  time.
- Federated melt apply requires a matching earlier successfully applied
  `MeltQuote` journal entry before proof reservation, saga creation, or payment
  work. The check is index-bounded, so a later quote never authorizes an older
  melt on replay.
- Focused regressions cover local-only payment state, temporal authorization,
  signed-output reuse, pre-consensus quote absence, non-proposer backend
  isolation, exact replay, conflicting local quotes, standalone quote
  persistence, and normal quote-then-melt operation. The full federation melt
  module, standalone melt tests, `cdk-common` federation suite, and focused Axum
  catch-up and route tests pass.

## Recent consensus checkpoint

- Consensus-application transaction ownership is centralized without changing
  payment semantics. Every mint federation applier transaction now commits or
  explicitly rolls back through one finalizer; successful domain mutations and
  `Applied` status commit together. Runtime apply failures use one explicit
  persistence path for existing retryable `Failed` and canonical
  `Rejected(ErrorResponse)` outcomes. Outgoing melt payment and saga phases
  remain intentionally staged and replayable rather than being hidden behind a
  fictitious cross-payment transaction. All 100 mint federation tests and all
  420 non-ignored federation Axum tests pass, alongside 28 standalone Axum
  tests.
- Materialized-state projection cleanup is complete. Axum delegates snapshot
  construction to the mint domain, which scans operation effects once and uses
  bounded bulk database reads without loading quote payment/issuance histories
  outside the commitment. At 1,000 quote operations, debug projection improved
  from 44.4ms to 27.4ms and release from 5.9ms to 4.2ms while reducing roughly
  3,000 state reads to five. Incremental durable fingerprints were rejected as
  unnecessary complexity at the measured scale. Results and discarded
  candidates are recorded in
  `docs/federated-cashu-materialized-state-benchmark.md`.

- Milestone 4 is complete. The selected AlephBFT backend is no longer an
  adapter-only placeholder: router startup reconstructs durable adapter state,
  launches upstream `aleph_bft::run_session`, installs the roster-authenticated
  private-REST network pump, and routes finalized units through one persistent
  deterministic apply loop before notifying public request waiters.
- The Milestone 4A production-backend, live data-provider, live finalization,
  durable accepted-evidence, and startup-recovery parent items are now certified
  complete from current code and tests. Focused verification passed 8 bounded
  data-provider tests, 7 strict finalization-handler tests, live two- and
  four-member private-REST finality, a five-member reorder/duplicate/delay fault
  matrix, pre-backup restart replay, post-backup continuation, and startup
  fail-closed/reopen behavior.
- AlephBFT startup status now says the precise condition, live network finality
  is not active yet, instead of the obsolete claim that the backend is not
  production-ready in the build. The fail-closed behavior is unchanged.
- Milestone 4A's release criteria are complete, including bounded
  retry/backpressure certification, round/consensus-height observability,
  atomic mint/swap failure injection, and melt payment-side-effect boundaries. The exact
  self-partitioned proposer, below-quorum value-operation, above-quorum stalled
  value/conflict, and partition restart/catch-up matrices are complete.
- Current verification is explicit: all focused activity-wake tests, the
  default-cadence probe, mintd/CLI feature checks, and fresh five- and
  11-process gates pass. The final federation-enabled Axum suite covers 461 passing
  tests in one permissioned run. This surface includes 4-member and
  5-member one-offline full value-operation/catch-up windows, the self-partition,
  below-quorum full value-operation,
  production-paced above-quorum basic recovery, and below-finality conflicting
  swap plus melt side-effect matrices.
- Undeployed federation wire shapes are now strict. Materialized-state digests
  are required for convergence and signed catch-up evidence while fast
  order-only status explicitly omits them. Journal order/conflict metadata,
  keyset fees, wallet registry policy, and current status/report fields are
  required instead of receiving legacy defaults. Optional setup phases,
  pagination, evidence, payment proof data, and diagnostics remain optional
  only where absence has current meaning.
  Missing-field regressions fail closed, while 221 common federation, 21
  SQLite federation, 11 CLI registry, 99 mint federation, 41 mintd status, and
  332 Axum federation tests pass alongside 28 standalone Axum and 134
  standalone SQLite tests; PostgreSQL and mintd federation checks are clean.
- The undeployed federation SQL history is squashed from 20 incremental files
  per backend to one direct final-schema migration for SQLite and PostgreSQL.
  No pre-federation migration changed. Real catalog tests require exactly one
  federation migration, all federation tables, the saga operation ID,
  required journal order/conflict fields, nullable evidence/rejection metadata,
  and every current index. Verification passes 21 SQLite and 19 PostgreSQL
  federation tests, 29 catch-up and 8 SQLite restart tests, plus 134 SQLite and
  125 PostgreSQL standalone tests.
- Axum federation runtime ownership extraction is complete with a verified
  behavior-neutral boundary: `MintState` now contains one concrete
  `FederationNodeRuntime` instead of 13 consensus, sync, DKG, admission,
  notification, and live-session fields. Every call site explicitly names the
  runtime owner; no trait or event bus was added. Operation
  waiter registration, response-cache policy/statistics, and notification
  delivery are the first methods moved onto the runtime; focused saturation,
  timeout, eviction, cache-bound, and notification tests pass. Duplicate
  finalized-unit verification and response reconstruction now also execute
  there. Operation submission is idempotent by canonical operation ID and no
  longer carries source sequences into finalization. The complete readiness
  state machine and live-session admission check now run on the runtime while
  `MintState` persists returned
  transitions; status restoration/persistence and public-submit gating tests
  pass. Aleph status/mempool reads and local/peer activity signaling now run on
  the runtime as well; all 11 activity-wake tests and status-counter coverage
  pass. Active runtimes now depend directly on the same `Arc<Mint>` used by
  Axum, while inert standalone runtimes hold none; this avoids a one-use trait
  and enables intact submission/replay moves. Effective-keyset admission is the
  first mint-dependent method moved and passes rotated-keyset plus all 28
  standalone Axum regressions. Atomic journal/catch-up certificate,
  pending-operation, finalization, and catch-up transactions now belong to the
  runtime; five finalized-unit and all 29 focused catch-up tests pass. Replay,
  lock retry/backoff, durable application/rejection state, standard
  `ErrorResponse` persistence, and catch-up rejection verification now also run
  there, with failed mint/swap and startup replay regressions green. The complete
  authoritative finalized-unit path now runs as one runtime-owned ordered
  method: duplicate recovery, finalization, atomic persistence, consensus-state
  replacement, mint application, latency recording, and waiter notification.
  Five finalized-unit tests pass, including duplicate
  replay with unmaterialized entries and recovery after lost waiters. Consensus
  submission now belongs to the runtime as one ordered path covering duplicate
  joins, waiter bounds, mempool admission, durable pending persistence, activity
  wake-up, timeout reporting, test-local finality, and submission metrics. Seven focused
  submission tests plus duplicate-pending, notification, cache, duplicate-race,
  and Prometheus metric regressions pass; public readiness/admission and final
  Cashu response validation remain thin `MintState` adapters. Signed catch-up
  quorum verification and finalized-item reconstruction now also run entirely
  on the runtime through atomic persistence, mint replay, and post-replay
  rejection verification. All 29 focused catch-up regressions pass, including
  malformed evidence, digest divergence, stale-overlap retry, rejected-entry
  continuation, and mint/swap/melt materialization. Catch-up now has one
  authoritative replay representation: finalized AlephBFT items. The wire
  response carries only compact operation/order commitments plus application
  status and retained Cashu rejection metadata; full journal envelopes,
  conflict keys, and consensus evidence are reconstructed from finalized
  items. Compact ranges validate every index and digest link before quorum
  evidence is accepted, and response construction requires the actual
  materialized-state digest before signing. The range proof is now named
  `FederationCatchUpCertificate` throughout protocol metadata, private REST,
  storage, readiness, metrics, CLI output, and operator tooling, so it cannot
  be confused with an AlephBFT session result. Certificate JSON derives its
  operation count, last index, and range digest from the first index, previous
  digest, and ordered operation IDs; the unused duplicate SQL columns are gone.
  Readiness transitions now remain in memory on the runtime, and startup
  derives the replayed-journal quorum gate without restoring an operational
  label. Seventeen startup, four quorum-prefix, two lagging-readiness, and one
  materialized-state-mismatch regressions pass. DKG commitment, reveal,
  secret-share, transcript-signature, activation, transcript derivation, and
  finalized-config admission state now belong to the runtime as well. All 50
  focused DKG tests pass across restart, response-loss, partition, conflict,
  transcript, fairness, and bounded-CPU cases. The runtime-owned live-session
  handle is now an explicit `AlephBftSessionSupervisor` with a single owned
  task set and Running/Stopping/Stopped state. Shutdown and rotation join all
  CDK tasks and tracked upstream Aleph children; focused supervisor, start,
  manual/automatic rotation, backup restart, and idle-cadence tests pass. All
  fallible construction now precedes task spawn and state publication. A root
  task panic is caught, stops and joins the supervisor, clears inbound
  admission, and durably halts readiness; partial-start and panic fault tests
  pass. The final complete gates pass all 420 federation-enabled Axum tests and
  all 28 standalone Axum tests, and the federation library compiles without
  warnings.

## Recent performance checkpoint

The original report of 36-second federated `mint-info` and 66-second minting was isolated with clean five-member AlephBFT fakewallet benchmarks. The delay was not direct HTTP, fakewallet settlement, catch-up, database contention, or signature aggregation. It came from avoidable wallet startup work and an idle AlephBFT data-provider/payment-observer wake path; both fixes are implemented and verified.

Current behavior is documented by the latest benchmark checkpoints: federated minting completes through the expected serialized `mint_quote`, `mint_quote_payment`, and `mint` operations, members converge on identical journal and state digests, and idle member CPU remains low. No additional performance work is planned unless a significant new regression appears.

## Sources reviewed

CDK source areas:

- `crates/cdk/src/mint/issue/mod.rs`
- `crates/cdk/src/mint/swap/mod.rs`
- `crates/cdk/src/mint/swap/swap_saga/mod.rs`
- `crates/cdk/src/mint/melt/melt_saga/mod.rs`
- `crates/cdk/src/mint/verification.rs`
- `crates/cdk-common/src/database/mint/mod.rs`
- `crates/cdk-sql-common/src/mint/{proofs,quotes,signatures,saga}.rs`
- `crates/cdk-signatory/src/{signatory,db_signatory}.rs`
- `crates/cashu/src/dhke.rs`
- `crates/cashu/src/nuts/nut01/bls.rs`
- `crates/cdk/src/wallet/{issue,swap}/`
- `crates/cdk/src/wallet/mint_connector/`
- `crates/cdk-axum/src/{router_handlers,custom_handlers}.rs`

Fedimint source areas:

- [README](https://github.com/fedimint/fedimint)
- [fedimint-server-core/src/lib.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-server-core/src/lib.rs)
- [fedimint-core/src/transaction.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-core/src/transaction.rs)
- [fedimint-core/src/epoch.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-core/src/epoch.rs)
- [fedimint-server/src/consensus/engine.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/consensus/engine.rs)
- [fedimint-server/src/consensus/transaction.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/consensus/transaction.rs)
- [fedimint-server/src/consensus/aleph_bft/data_provider.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/consensus/aleph_bft/data_provider.rs)
- [modules/fedimint-mint-server/src/lib.rs](https://github.com/fedimint/fedimint/blob/master/modules/fedimint-mint-server/src/lib.rs)
- [modules/fedimint-mint-common/src/lib.rs](https://github.com/fedimint/fedimint/blob/master/modules/fedimint-mint-common/src/lib.rs)
- [fedimint-server/src/config/dkg_g2.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/config/dkg_g2.rs)

## Key observations

### CDK today

- `Mint::process_mint_request` validates quotes and outputs, calls `blind_sign`, then persists blinded messages, blind signatures, quote issuance, and completed operations in a database transaction.
- `Mint::process_swap_request` uses a saga: TX1 marks input proofs pending and stores blinded outputs, signing happens outside the DB transaction, then TX2 stores signatures and marks inputs spent.
- `MeltSaga` correctly treats external payments as non-transactional and uses write-ahead saga state before payment attempts. This is the right shape for a federated melt, but the state transitions need consensus.
- The mint database already has useful materialized state tables: `proof`, `blind_signature`, `mint_quote`, `melt_quote`, `saga_state`, and `completed_operations`.
- The wallet already depends on `MintConnector`, so federation can start as a new connector implementation without rewriting wallet business logic.
- Initial baseline: BLS primitives could blind, sign, unblind, and verify, but
  threshold share generation, share verification, and interpolation were still
  absent. Those capabilities are implemented and verified by the completed
  milestones below.

### Fedimint patterns to copy

- Client value transfers are modeled as canonical transactions ordered by consensus. Module consensus items are reserved for facts needed to validate transactions, such as time, block height, fees, or external observations.
- The mint module spends input notes and creates output signature shares inside consensus transaction processing.
- Output outcomes contain blind signature shares and are read by clients only after the transaction is accepted.
- The consensus engine accepts an ordered stream of items, processes each item in a DB transaction, records accepted items, audits module balances, and checkpoints DB state.
- Fedimint uses AlephBFT for atomic broadcast and authenticated peer networking. CDK should copy the atomic-broadcast and authentication invariants and can use the same `aleph-bft` crate, but it should not depend on Fedimint's full module stack or require Fedimint's network stack. Signed private REST is sufficient for correctness when federation members are mutually reachable.
- Fedimint DKG for G2 is a direct reference for later dealer-free setup. For CDK MVP, trusted-dealer key share generation is enough.

## Safety invariants

Every implementation phase must preserve these invariants:

- A member never returns a blind signature share unless the exact operation payload was accepted by consensus or was already accepted earlier.
- The operation hash commits to the complete request: quote IDs, input proofs, output blinded messages, amounts, keyset IDs, witnesses, NUT-20 signatures, payment method, and version.
- A quote cannot issue more aggregate output value than its consensus-tracked mintable amount.
- A proof can transition to spent exactly once in the consensus materialized state.
- A blinded output can be signed at most once per accepted operation and amount.
- Federation mode only advertises, accepts, and signs Cashu keyset v3 (`KeySetVersion::Version02`) aggregate BLS keysets.
- All honest members process the same ordered operation log into the same state digest.
- External payment status is never read directly inside deterministic consensus apply logic. It enters consensus as signed observations or designated payment events.
- Standalone CDK behavior remains available and covered by existing tests.

Database equality means equality of consensus-visible federation state, not byte-for-byte equality of each member's entire database file. Private member configuration, local peer state, local sequence trackers, logs, caches, metrics, and other node-local operational data must be excluded from state digests and row-level convergence checks. Any member whose consensus-visible state digest differs from quorum after deterministic replay must halt or enter manual inspection mode before serving signing shares, melt payment attempts, restore responses, or authoritative check-state responses.

## Compatibility strategy

The public Cashu protocol should stay mostly unchanged:

- Existing routes remain: `/v1/mint/quote/{method}`, `/v1/mint/{method}`, `/v1/mint/{method}/batch`, `/v1/swap`, `/v1/melt/{method}`, `/v1/restore`, `/v1/checkstate`, `/v1/keys`, `/v1/keysets`, `/v1/info`.
- Federation-aware wallets use a new `FederatedMintConnector` that internally fans out requests to member URLs and aggregates BLS shares before returning normal `MintResponse` or `SwapResponse` to wallet sagas.
- Federation members may return `MintResponse` or `SwapResponse` whose `BlindSignature.c` is a BLS share, not a final signature. This is acceptable only when the caller is the federated connector and knows the member ID.
- Existing non-federated wallets should learn from mint info that a mint is federated and should refuse unless they support federation.
- Add an optional mint info extension, for example `nuts.federation` or a CDK-specific metadata field, containing federation ID, threshold, members, aggregate keysets, public key shares, consensus version, and operation-hash version.
- All members expose the same aggregate public Cashu v3/BLS keysets to clients. Member public key shares are extra federation metadata, not replacement NUT-01 keys.

## Architecture overview

Recommended new components:

- `cashu` threshold BLS module: key shares, public shares, share verification, interpolation, and aggregate public key derivation.
- `cdk-common` federation types: `FederationId`, `MemberId`, `ThresholdParams`, canonical operation types, operation hashes, accepted outcomes, state digests, and errors.
- `cdk-federation` crate: consensus engine, in-memory test transport, signed private REST transport, optional later NAT-traversing transports, operation mempool, catch-up certificates, DB journal abstractions, and test harness.
- `cdk` mint federation mode: wraps mint operations with consensus submit/apply/await and uses a key-share signatory.
- `cdk` wallet federation connector: fans out requests, validates partial responses against public shares, aggregates threshold shares, and returns ordinary wallet responses.
- `cdk-mintd` federation config and commands: create local trusted-dealer configs, run member nodes, expose status, and start fakewallet-backed development federation tests.

## Federation member identity model

Objective: ensure each mint only accepts federation traffic from configured federation partners.

Key material:

- Each member has a long-lived federation identity keypair used for mint-to-mint authentication.
- Each member also has a threshold BLS signing share for each federated Cashu v3 keyset.
- The federation has aggregate Cashu public keys derived from the DKG or trusted-dealer transcript.
- The member identity key is separate from the Cashu signing share. Network authentication must not reuse Cashu signing secret material.

Public roster:

- Every member stores the complete federation roster: federation ID, member IDs, partner public API URLs, private federation REST URLs, member identity public keys, BLS public shares, and aggregate Cashu keysets.
- Startup validation fails if the local member's private identity key does not match the configured member identity public key.
- Startup validation fails if the local BLS secret share does not match the configured BLS public share.
- Startup validation fails if the roster, aggregate keysets, thresholds, or DKG transcript hash differ from the configured federation.
- MVP membership is static. Unknown members are rejected rather than added dynamically.

Request authentication:

- Every private REST message is wrapped in a signed member envelope.
- The receiver verifies federation ID, sender member ID, receiver member ID, body hash, sequence or nonce, and signature against the sender's configured identity public key.
- Messages from unknown member IDs, unexpected public keys, wrong federation IDs, or mismatched body hashes are rejected before they can enter consensus or mutate state. Effectful DKG messages also reject stale sequences through durable replay state; status, catch-up, readiness, and Aleph traffic use kind-isolated volatile windows. `SubmitOperation` signs but does not order by its transport sequence: canonical operation IDs and durable consensus outcomes provide exact-retry idempotency across response loss and restart.
- DKG and trusted-dealer setup output must bind member IDs to identity public keys and BLS public shares so a public share cannot be silently reassigned to another member.

## Core operation coverage matrix

Objective: make it explicit that federation support covers all three core Cashu eCash operations: minting, swapping, and melting.

Minting coverage:

- Covered by Milestone 8, Milestone 9, and Milestone 10.
- The wallet obtains a mint quote from one member, then submits the same quote ID and blinded outputs to all members through the federated connector.
- The federation serializes the exact mint request by operation hash before any member returns a signature share.
- Every member persists the same mint quote, payment observation, accepted output set, and deterministic signature shares.
- The wallet verifies member shares against public key shares, aggregates any valid threshold subset, unblinds the aggregate signatures, and stores ordinary Cashu proofs.
- Required happy-path tests: fake wallet marks the incoming invoice paid for all members, threshold shares aggregate, and the resulting proofs verify against the aggregate keyset.
- Required unhappy-path tests: unpaid quote, expired quote, amount mismatch, conflicting output set for the same quote, duplicate request replay, insufficient shares, invalid share, one slow member, one offline member that later catches up.

Swapping coverage:

- Covered by Milestone 6, Milestone 7, Milestone 11, and Milestone 12.
- The wallet sends the same input proofs and blinded outputs to all members.
- The federation serializes the exact swap request, verifies inputs once, moves inputs through pending or spent states consistently, and signs only the accepted output set.
- The operation hash commits to inputs, outputs, unit, fees, spending conditions, and signature flags so members cannot sign overlapping output windows.
- Required happy-path tests: valid inputs are spent, threshold output shares aggregate, and new proofs verify.
- Required unhappy-path tests: double spend, conflicting output set, insufficient input amount, fee mismatch, duplicate proof, pending proof conflict, P2PK or HTLC failure, `SIG_ALL` mismatch, and the sliding-window quorum-cover attack.

Melting coverage:

- Covered by Milestone 14, with earlier support from the deterministic state machine in Milestone 6 and the journal in Milestone 5.
- The wallet submits proofs and a melt quote. The federation serializes reserve, development-backend payment observation, finalization, and optional change signing.
- Inputs are not considered finally spent until the accepted melt state says the outgoing payment is final, while pending states remain durable and recoverable.
- With the fake wallet backend, all members can deterministically observe success for happy-path tests, and scripted failures or delays are used for unhappy-path tests.
- Required happy-path tests: fake wallet outgoing payment succeeds, all members finalize the melt, inputs become spent, and change outputs aggregate when present.
- Required unhappy-path tests: fake wallet failure, long pending payment, one member slow to observe, one member offline during payment, payment timeout, quote expiry, duplicate melt retry, crash after payment attempt, and rollback or retry according to the accepted state.

Restore and state-check coverage:

- Covered by Milestone 13.
- Restore and check-state are not value-changing operations, but they must read the federated database consistently and expose the aggregate keyset view expected by wallets.
- Required tests: restored signatures match deterministic shares or aggregate signatures, pending and spent states agree across members, and lagging members refuse to answer as authoritative until caught up.

## Incremental validation ladder

Objective: make the mint-side implementation testable in small, reversible steps before the whole federation exists.

Checkpoint 1: threshold BLS only.

- Implement trusted-dealer shares, share verification, interpolation, aggregate signatures, and wallet-side aggregation without changing mint business logic.
- Success criteria: 1-of-1 equals standalone BLS, 3-of-5 and 4-of-7 aggregate, invalid or duplicate shares fail, randomized property tests pass.

Checkpoint 2: static federation configuration.

- Generate local member configs, aggregate keysets, public shares, private shares, peer identities, and deterministic test fixtures.
- Success criteria: all generated members agree on federation ID and aggregate keyset IDs; each private share validates against public config; malformed configs fail before startup.

Checkpoint 3: operation hashing.

- Convert mint, swap, melt, payment observation, and restore request material into canonical operation encodings.
- Success criteria: semantically identical requests hash identically; reordered outputs or changed amounts hash differently; cross-language fixture vectors can be added later.

Checkpoint 4: networking smoke tests without consensus.

- Start N member processes with the planned private REST transport and exchange authenticated ping, status, and config-digest messages.
- Success criteria: members connect, reject unknown peers, report peer liveness, survive reconnects, and expose status without any mint operation enabled.

Checkpoint 5: consensus on a dummy log.

- Feed counter increments or no-op operations into the consensus engine, persist accepted sessions, and expose height and digest.
- Success criteria: all members apply the same ordered dummy operations, one member can restart and catch up, and state digest mismatch is detected.

Checkpoint 6: database journal and replay.

- Add the federation journal around an isolated test state machine before touching mint issue, swap, or melt.
- Success criteria: crash-before-apply, crash-after-apply, replay, checkpoint, and compaction tests pass across memory and SQLite.

Checkpoint 7: deterministic mint state-machine extraction.

- Refactor existing mint business logic so validation and state transitions can be run as deterministic apply functions.
- Success criteria: standalone mint behavior remains unchanged; federation mode can apply accepted operations without making HTTP responses or payment calls inside consensus apply.

Checkpoint 8: development-backend payment observations.

- Script fake incoming and outgoing payment outcomes as deterministic observations that can be proposed into consensus.
- Success criteria: success, failure, pending, delayed, duplicate, and conflicting observations are all serialized and produce the expected quote state.

Checkpoint 9: federated mint issuance.

- Enable mint quote creation, paid observation, accepted output set, share production, response caching, and wallet aggregation.
- Success criteria: one end-to-end fakewallet-backed development mint succeeds; conflicting mint requests for the same quote fail; one offline member catches up and later serves the cached or regenerated share.

Checkpoint 10: federated swap.

- Enable proof verification, pending/spent transitions, accepted output set signing, and wallet aggregation for swap.
- Success criteria: basic swap succeeds; double spend fails; overlapping-window attack fails; state digests remain equal.

Checkpoint 11: federated melt.

- Enable melt reserve, fake outgoing payment observation, finalization, failure recovery, and optional change signing.
- Success criteria: successful melt finalizes across all members; failed or pending melt does not lose funds; retries are idempotent.

Checkpoint 12: network chaos and catch-up.

- Run the three core operations while members disconnect, restart, lag, and rejoin.
- Success criteria: caught-up members converge to the same digest, lagging members refuse signing until safe, and wallets can complete with any valid threshold subset.

## Fake wallet payment behavior matrix

Objective: prove payment handling is robust before connecting real Lightning backends.

Fake incoming payments for minting:

- Happy path: all members observe the mint quote as paid and the wallet receives aggregate signatures.
- One slow member: threshold members observe paid and sign; the slow member later records the same paid observation and catches up.
- One offline member: the operation finalizes with a threshold or consensus quorum; the offline member replays the accepted quote and payment observation before serving shares.
- Pending payment: members keep the quote pending, do not sign outputs, and later sign only after a paid observation reaches consensus.
- Failed payment: members mark the quote failed or unpaid according to the fake backend script and refuse issuance.
- Conflicting observation: one member proposes a different amount, unit, invoice, or quote state; consensus rejects or records the conflict without issuing signatures.
- Duplicate observation: repeated fake wallet notifications are idempotent and do not change the operation hash or issue twice.

Fake outgoing payments for melting:

- Happy path: all members accept the melt reserve, fake wallet reports success, inputs become spent, and change is signed.
- One slow member: threshold responses are enough for the wallet where safe, but the slow member catches up before serving authoritative state.
- One offline member: accepted melt state and payment observation are replayed on reconnect, and the member does not attempt a second outgoing fake payment.
- Long pending payment: inputs remain pending, no success response is returned as final, and retry or polling returns the same pending state.
- Failed payment: inputs are released or moved to the configured failed state consistently across all members, no payment success is reported, and exact duplicate failed melt retries do not attempt another outgoing payment.
- Timeout then success: the final accepted observation determines state, and repeated wallet polling receives a consistent result.
- Crash after payment attempt: recovery checks accepted observations and idempotency keys before finalizing or retrying.

## Milestone 0: written RFC and feature gate

Objective: make the safety model explicit before code paths start diverging.

Implementation checklist:

- [x] Add an RFC document under `docs/` describing the federated mint threat model.
- [x] Define supported fault models: BFT consensus default, optional lower signing threshold, crash-only test mode.
- [x] Decide the default threshold rule. Recommended: default to Fedimint-style `n - floor((n - 1) / 3)` for both consensus and signing until lower signing thresholds receive separate review.
- [x] If supporting examples like 3-of-5 signing, define it as a separate `signature_threshold`, with consensus quorum still governed by the BFT engine.
- [x] Add a `federation` Cargo feature that is off by default.
- [x] Add `FederationMode::{Standalone, Member}` to mint setup without changing existing constructors initially.
- [x] Document that signature shares must never be produced from pre-consensus local request state.

Tests and success criteria:

- [x] Re-run `cargo test --lib --workspace --exclude cdk-postgres` with the feature disabled and record the result. Passed on 2026-07-05 after the NUT-10 test helper was made keyset-version aware.
- [x] Re-run a feature-enabled workspace compile/check and confirm standalone behavior remains unchanged. `cargo check --workspace --all-targets --exclude cdk-postgres --features federation` passed on 2026-07-05 after fixing the `melt-token` example secp256k1 signing key import and stabilizing `Transport::resolve_dns_txt` across feature combinations.
- [x] Add a dedicated review checklist document or PR template for future federation PRs: determinism, consensus-before-signing, operation hashing, state digest, attack tests.

## Milestone 1: threshold BLS blind signature primitives

Objective: add the cryptographic building blocks without touching mint state logic.

Implementation checklist:

- [x] Add a separate file-backed module for threshold BLS under `crates/cashu/src/nuts/nut01/bls/` or an equivalent protocol-level location.
- [x] Define `BlsSecretKeyShare`, `BlsPublicKeyShare`, `BlsAggregatePublicKey`, `BlsBlindSignatureShare`, and signer index types.
- [x] Implement trusted-dealer polynomial key generation for BLS12-381 G2 public keys and G1 signature shares.
- [x] Implement Lagrange interpolation over BLS12-381 scalar field for arbitrary member IDs.
- [x] Implement blind signing with a secret share: `share = sk_i * blinded_message`.
- [x] Implement share verification: `e(share, G2) == e(blinded_message, public_share_i)`.
- [x] Implement aggregate blind signature construction from `t` valid shares.
- [x] Verify aggregate blind signatures with existing `verify_bls_blind_signature`.
- [x] Unblind aggregate signatures with existing BLS unblinding logic and verify final proofs with existing `verify_bls_message`.
- [x] Add serialization for share types with explicit versioning and no secret key debug leakage.
- [x] Add zeroization for secret shares where practical.

Tests and success criteria:

- [x] Unit test 1-of-1 threshold produces the same final signature as the existing standalone BLS signer.
- [x] Unit test 3-of-5 aggregate signatures verify under aggregate public keys.
- [x] Unit test multi-threshold and order-permutation aggregation, including a 4-of-6 case.
- [x] Add an explicit 4-of-7 aggregate signature test if that exact threshold/member count remains a release criterion.
- [x] Unit test any valid threshold subset aggregates to the same final signature.
- [x] Unit test insufficient shares cannot aggregate.
- [x] Unit test duplicate signer IDs are rejected.
- [x] Unit test invalid share fails verification before aggregation.
- [x] Unit test share/order permutations produce identical aggregate signatures.
- [x] Add deterministic multi-case tests for messages, thresholds, and signer subsets.
- [x] Add randomized/property tests for random messages, thresholds, and signer subsets.
- [x] Existing NUT-00 BLS proof verification passes for aggregate proofs.

## Milestone 2: federation configuration and trusted-dealer setup

Objective: create deterministic federation configs and local key shares for test federations.

Implementation checklist:

- [x] Add `FederationConfig` with federation ID, member set, consensus params,
  signature threshold, keyset metadata, supported units, and one wallet-facing
  protocol version. Member-only versions remain in the payloads they govern.
- [x] Add public member config data with member ID, public API URL, private federation REST URL, federation identity public key, and public BLS shares.
- [x] Add private member config data with member ID, federation identity private key, local secret BLS shares, and config-file/runtime wiring.
- [x] Represent the federation roster deterministically in `FederationConfig`, mapping member IDs to identity public keys, endpoint data, and BLS public shares.
- [x] Add `FederatedKeyset` with aggregate public keys per amount and public shares per member.
- [x] Define deterministic member ordering and reject duplicate member IDs.
- [x] Generate Cashu keyset IDs from aggregate public keys, not member share public keys.
- [x] Force all generated federation keysets to Cashu keyset v3 (`KeySetVersion::Version02`).
- [x] Add the test-only `cdk-mintd federation test-trusted-dealer` command to generate N member config files for local/regtest use.
- [x] Add `cdk-mintd federation inspect` to print non-secret config and keyset IDs.
- [x] Add config validation to ensure each local identity private key matches the configured identity public key.
- [x] Add config validation to ensure secret shares match declared public shares.
- [x] Add config validation that every aggregate federation keyset is v3/BLS and reject v1/v2/non-BLS federation keysets.
- [x] Add validation that all members agree on roster, identity public keys, aggregate keysets, thresholds, units, fees, and NUT settings.
- [x] Reject startup if a member is missing any federation partner public key.

Tests and success criteria:

- [x] Generated 5-member configs have identical federation ID and aggregate keyset IDs.
- [x] Generated 5-member configs have identical roster hashes and complete partner identity public-key allowlists.
- [x] Generated 4-member/threshold-3 local smoke artifacts cover the same
  member-generic convergence, long-partition, pending-melt, quarantine,
  key-rotation, Prometheus, and catch-up certificate checks as the larger matrix.
  Covered by `trusted_dealer_smoke_script_supports_four_member_bft_matrix`.
  Standard and long-partition operation windows now use the smoke command's
  first-class `--operation-batches` option instead of relying on shell-only
  loops, with validation that prevents zero batches, unsupported stateful
  scenarios, and seed-offset wrapping. The generated process script also exposes
  explicit `LONG_PARTITION_MEMBER`, `LONG_PARTITION_SEED_OFFSET`, and
  `LONG_PARTITION_OPERATION_BATCHES` controls so longer member partitions can be
  run without reusing deterministic wallet seeds.
- [x] Each member's private identity key verifies against its public identity key.
- [x] Each member's private share verifies against its public share.
- [x] A config with mismatched thresholds is rejected.
- [x] A config with mismatched aggregate keyset is rejected.
- [x] A config with any non-v3 federation aggregate keyset is rejected.
- [x] A config with mismatched member identity public key is rejected.
- [x] A config missing a federation partner public key is rejected.
- [x] A config with duplicate member IDs is rejected.
- [x] Trusted-dealer tests cover deterministic IDs and validation for generated federation material.
- [x] Add a deterministic test-seed mode where a config generated twice matches byte-for-byte except explicitly random fields.

## Milestone 3: canonical operation encoding and hashes

Objective: make every consensus operation uniquely and deterministically identified.

Implementation checklist:

- [x] Define `FederatedMintOperation` enum with versioned variants.
- [x] Add versioned federation operation variants for mint quote, mint quote payment, mint issuance, swap, melt quote, melt quote payment, melt, restore, payment-observation quarantine, and keyset rotation.
- [x] Decide whether to further split melt into explicit reserve/finalize/rollback operation variants; operation version 1 keeps the current `Melt` reserve/execution operation plus accepted `MeltQuotePayment` observations and saga recovery, as documented in `docs/federated-cashu-melt-state-machine.md`.
- [x] Implement deterministic canonical JSON encoding with sorted object keys for consensus operation IDs.
- [x] Decide canonical sorted JSON remains the operation version 1 encoding; a deterministic binary format must use a future operation version if it becomes a protocol requirement.
- [x] Use `BTreeMap` and sorted vectors wherever ordering matters.
- [x] Define `OperationHash = sha256("cdk-fed-op-v1" || encoded_operation)`.
- [x] Define per-output hashes for blinded messages and per-input proof Y identifiers.
- [x] Make operation IDs for federated consensus hash-based rather than UUIDv7-based.
- [x] Keep wallet-local saga IDs as UUIDs if useful, but never use them as consensus identity.
- [x] Define conflict keys per operation:
  - [x] quote issuance: quote ID plus issued amount window.
  - [x] swap: input proof Ys and output blinded messages.
  - [x] melt reserve: quote ID plus input proof Ys.
  - [x] restore: blinded messages only.
- [x] Bind NUT-20 and SIG_ALL verification to the canonical operation envelope
  and its domain-separated operation ID.

Tests and success criteria:

- [x] Unit tests cover canonical operation encoding behavior.
- [x] Commit cross-version golden fixture vectors for canonical operation encoding.
- [x] Equivalent operations produce the same hash across processes.
- [x] Reordered unordered fields still encode canonically.
- [x] Changing one output blinded message changes the operation hash.
- [x] Add an explicit witness-mutation test proving that changing an input witness changes the operation hash.
- [x] Add size-limit tests for oversized consensus payloads.
- [x] Add fuzz coverage proving operation decoding rejects unknown major versions and malformed payloads.

## Milestone 4: consensus engine abstraction

Objective: add an ordered log before integrating with mint business logic.

Implementation checklist:

- [x] Add a consensus abstraction in `cdk-common::federation::consensus` with submit, journal, catch-up, and state-digest boundaries.
- [x] Keep the consensus boundary in `cdk-common::federation::consensus` rather
  than adding a separate crate. The runtime submit API, durable journal,
  response-waiter registry, finalized-item stream, and catch-up interface supply
  the exact boundaries used by Axum without introducing another workspace crate.
- [x] Add `ConsensusItem::{Operation(FederatedMintOperation), ModuleFact(...), CatchUpCertificate(...), Default(...)}`.
- [x] Retire the early in-memory single-node engine after replacing its useful
  coverage with direct journal tests and the AlephBFT runtime.
- [x] Retire the early in-memory multi-node simulator and its unused parallel
  adapter runtime after real AlephBFT fault and process coverage superseded it.
- [x] Make AlephBFT the mandatory mintd federation backend; remove backend
  selection from config, environment variables, generated scripts, and recipes.
- [x] Implement the AlephBFT-backed engine for multi-member BFT ordering.
  `FederationConsensusRuntime` owns canonical pending/finalized state,
  while router startup wires the adapter through upstream `aleph_bft::run_session`,
  the roster keychain, private-REST network pump, durable backup IO, and the
  single ordered apply loop. Live two-, four-, five-, and seven-member tests plus
  process smokes prove actual multi-member finality rather than adapter-only
  simulation.
- [x] Add a `DataProvider` equivalent that batches pending items, deduplicates operation hashes, and enforces max item size.
- [x] Add a `FinalizationHandler` equivalent that applies ordered items to the DB transactionally.
- [x] Add catch-up certificates signed by member identity keys.
- [x] Store accepted items and signed catch-up certificates in the DB.
- [x] Add operation outcome notification so public HTTP handlers can await acceptance.
- [x] Add bounded mempool and admission checks to avoid DoS by large invalid operations.

Tests and success criteria:

- [x] Single-node engine accepts operations in submit order.
- [x] Multi-node in-memory engine converges on identical accepted sequences.
- [x] Duplicate operation submissions are accepted once.
- [x] Oversized consensus items are rejected before broadcast.
- [x] Restarting a node mid-session replays accepted items without duplicating state changes.
- [x] AlephBFT integration test finalizes sessions for 4 nodes and 5 nodes.
  Covered at the CDK adapter boundary by
  `consensus_runtime_finalizes_four_member_order_with_faulty_duplicates`
  and
  `consensus_runtime_finalizes_five_member_sessions_with_silent_member`.
  Live network/process-level AlephBFT finality remains tracked separately below.
- [x] Consensus halts or rejects when fewer than the required quorum is reachable.

## Milestone 4A: AlephBFT strict-order consensus and idempotent federation APIs

Objective: replace the development consensus path with a production BFT ordering
engine and make federated mint, swap, and melt APIs behave like Fedimint-style
submit-and-await APIs. Public wallet requests must only validate and enqueue
operations. All safety-critical state changes, signature-share creation, proof
spending, quote finalization, payment-attempt state, and response persistence
must happen only after the exact operation envelope comes out of AlephBFT in the
finalized order.

Fedimint lessons to adopt:

- Treat AlephBFT as an atomic broadcast engine, not as mint business logic.
- Submit paths run admission or dry-run validation only; they do not commit
  state and do not produce usable blind-signature shares.
- The ordered consensus stream is the only source of truth for applying
  transactions.
- Exact duplicate submissions are harmless and converge to the same operation
  hash, accepted outcome, and cached response.
- A member can receive a wallet request after it has already processed the same
  operation through consensus; the API must return the persisted result rather
  than trying to process the operation again.
- Signature-share endpoints are lookups of persisted consensus outcomes, never
  fresh local signing decisions.
- Waiting APIs must handle both races: the operation may already be accepted, or
  it may be accepted after the HTTP request starts waiting.
- Signed catch-up certificates, accepted item logs, and state digests are catch-up
  and replay evidence. A lagging member must not serve signing shares or
  authoritative state until replay has certified convergence.

Architecture checklist:

- [x] Add one production `FederationConsensusRuntime`, using Fedimint's AlephBFT
  integration as the reference
  for data provider, finalization handler, keychain, network, catch-up certificate,
  crash recovery, and catch-up design. The runtime directly owns proposal,
  finalized-item, journal, and catch-up state; there is no backend selector or
  parallel adapter type. Axum owns the upstream live-session tasks and durable
  recovery boundary.
- [x] Keep Cashu/CDK simpler than Fedimint where possible: CDK needs strict
  operation ordering and deterministic mint execution, not Fedimint's full module
  system or wallet state-machine stack. The implementation uses one canonical
  federation operation stream and CDK's existing mint/wallet sagas instead of a
  general module framework.
- [x] Define the AlephBFT unit payload as canonical CDK consensus items:
  `Operation(FederationOperation)`, payment observation facts, session
  signatures, default/compatibility items, and future module facts. This is now
  represented by `FederationAlephBftUnitPayload`.
- [x] Ensure every unit payload includes or commits to federation ID, operation
  hash, submitting member, accepted session/index, and body hash. The payload
  has its own exact version, includes member/session/unit metadata and derived
  operation IDs, and hashes the ordered canonical item body. Each consensus item
  carries and validates its own version; the unit does not duplicate an
  aggregate protocol bundle or item version.
- [x] Reuse the existing signed private REST envelope as the authenticated
  admission and transport envelope where practical, but make it explicitly
  mappable into AlephBFT proposals without changing the canonical operation
  bytes. `FederationVerifiedOperationAdmission` now verifies the signed
  `SubmitOperation` REST envelope, enforces the expected message kind and
  configured sender roster, carries sender sequence metadata, computes the
  operation ID, and wraps the exact operation envelope as a consensus item
  without changing its canonical bytes. The Axum private operation route now
  uses this helper before forwarding or local submission so private REST
  transport and the future AlephBFT proposal path share one operation admission
  boundary.
- [x] Add peer identity allowlist validation before any AlephBFT message,
  private REST submit, catch-up response, or signed catch-up certificate is accepted.
  The existing signed private REST envelope verifies sender membership against
  `FederationConfig`, and signed catch-up certificate quorum verification rejects
  unknown signers. The AlephBFT adapter runtime now constructs a roster-bound
  finalization handler from the configured members and rejects finalized units
  whose `submitting_member` is not in the roster before advancing unit indexes,
  marking duplicates, appending journal entries, or recording finalized items.
  Signed submit-operation admission now reuses that private REST roster check
  before producing any consensus item. Signed private REST finalized-unit
  delivery now reuses the same envelope and sequence check before applying an
  externally finalized AlephBFT unit, and also rejects a mismatch between the
  authenticated sender and the unit's `submitting_member` before any finalized
  item, journal, sequence, or mint-state mutation. Private REST submit-operation
  responses are also validated against both the submitted operation ID and the
  embedded accepted journal outcome operation ID before the peer client accepts
  them. AlephBFT finalized-unit acknowledgement responses now apply the same
  embedded-outcome validation to each nested submit response before the peer
  client counts a remote member as having accepted the unit. Future non-REST
  AlephBFT wire messages must reuse the same roster check at the networking
  boundary when those transports are introduced. The outbound private REST peer
  client now rejects unknown members and members whose public config differs
  from the local roster before it signs any request, with focused AlephBFT
  finalized-unit tests proving no request is emitted for unknown or mismatched
  peers.
- [x] Add a Fedimint-like data-provider loop that drains a bounded pending queue,
  deduplicates operation hashes locally per session, enforces max byte limits,
  and batches items without busy loops. The synchronous
  `FederationAlephBftUnitDataProvider` primitive now performs bounded,
  byte-limited, duplicate-aware unit construction; non-destructive previews; and
  exact-match draining so transient broadcast failures do not force a driver to
  consume the queue before it has actually finalized or durably handed off the
  proposal. `CdkAlephBftDataProvider` implements upstream AlephBFT's async
  `DataProvider`, drains canonical units from that runtime, waits on an activity
  notification instead of busy-looping when idle, and is passed through
  `LocalIO` into router-owned `run_session` tasks.
  `FederationConsensusRuntime` now wires the primitive to the CDK-side
  operation submission boundary so tests can verify queued operations become
  canonical unit payloads. The runtime validates roster, threshold, and
  local-member membership before queueing and exposes explicit
  proposal/finalization methods to the AlephBFT task. Axum now has a focused
  broadcast round that exercises
  that handoff over signed private REST, retains the queued proposal when remote
  acknowledgements do not reach the configured consensus threshold, and covers a
  batched finalized unit carrying multiple operation responses through driver
  accounting, finalized-item persistence, accepted-journal order, and
  materialized quote apply.
- [x] Add a Fedimint-like finalization handler that converts finalized AlephBFT
  units into an ordered stream consumed by exactly one deterministic CDK apply
  loop. The `FederationAlephBftFinalizationHandler` primitive now verifies
  finalized unit payloads, assigns accepted item indexes, preserves
  session/unit metadata, rejects wrong federation/session/replayed units, and
  flags duplicate operation IDs. `CdkAlephBftFinalizationHandler` implements
  upstream AlephBFT's data and unit finalization traits and feeds the finalized
  payload channel consumed by exactly one router-owned async apply task. The
  current
  `FederationConsensusRuntime` accepts externally finalized units,
  validates them, appends operation outcomes to the in-memory accepted-operation
  journal, and records finalized item metadata. Axum now exposes a controlled
  finalized-unit apply path that persists the finalized-item delta and newly
  appended journal entries atomically before swapping runtime state, applying
  materialized mint changes, and notifying response waiters. The private REST
  federation router now exposes a signed finalized-unit delivery endpoint that
  feeds this path and atomically stores the peer sequence with accepted
  finalized-unit state. A local driver-facing drain/apply helper now drains a
  previewed local unit and finalizes it only on a cloned runtime, persists the
  finalized items and journal entries first, and swaps runtime state only after
  storage succeeds.
  Generic dispatch-and-finalize calls for the AlephBFT adapter fail closed with
  an external-finalization-required error so local queueing cannot be mistaken
  for BFT acceptance. A guarded production-facing Axum state hook lets the live
  AlephBFT task mark network finality as available only after
  validating federation config, the `aleph_bft` backend selection, and the
  consensus runtime. Focused tests prove that activation opens queue admission
  but still returns retryable pending-finalization and does not persist quote or
  journal state until the exact finalized unit is delivered.
- [x] Store accepted session number, item index, canonical operation/order
  commitment, and signed catch-up certificate before considering an item durable.
  `FederationJournalEntry` stores the immutable order chain and optional
  accepted-item consensus evidence with session, accepted item index, finalized
  unit position, submitting member, and member unit index. SQLite/Postgres
  journal persistence round-trips those fields. A later audit removed the
  duplicate journal state chain and derivable `operation_outcome_hash`; the
  distinct materialized-state commitment remains on signed status and catch-up
  evidence. Full signed catch-up certificate
  quorum linkage remains in the existing catch-up certificate table; core consensus
  now exposes `apply_catch_up_response_with_catch_up_certificate_quorum` so replay can
  require quorum-signed evidence before appending peer entries. Axum REST peer
  sync now collects matching catch-up certificates from other peers for the exact
  catch-up range, applies through the quorum-verified replay primitive, and
  persists the accepted range plus evidence in one database transaction before
  reopening readiness. Startup replay now checks persisted contiguous
  catch-up certificate quorum height after materializing durable entries, blocks
  public value-changing routes when evidence is missing, and restores healthy
  status when quorum evidence already covers the replayed height. Production
  startup now owns durable finalized-item recovery and validates the recovered
  journal before enabling the live session.
- [x] Persist enough AlephBFT unit state and accepted item state to recover after
  crashes without replaying an item twice or losing a finalized item. The
  in-memory adapter now has a tested recovery constructor from retained
  finalized items that rebuilds journal evidence, finalized-unit replay guards,
  duplicate-operation tracking, next item indexes, and next unit-finalization
  indexes while rejecting corrupted retained metadata, split or interleaved
  retained units, non-contiguous unit-finalization order, non-contiguous item
  indexes inside a retained unit, accepted-order item-index mismatches, and
  retained unit body hashes that no longer match the retained item body. Empty
  finalized units are rejected before the finalization handler mutates unit
  replay guards or indexes because they cannot be represented in the retained
  finalized-item log used for recovery. The shared mint database
  trait now persists `FederationAlephBftFinalizedItem` rows, SQLite/Postgres
  migrations create a durable accepted-item table with session/unit and
  operation-ID indexes, SQLite round-trip tests reload the persisted log into
  adapter recovery, and Postgres compiles the same persistence regression.
  Axum startup now loads this finalized-item log when `aleph_bft` is selected,
  rebuilds the recovered journal, compares it against any stored journal rows,
  replays unapplied operations into mint state, and rejects synchronous submits
  with an external-finalization error until the live AlephBFT driver exists.
  Router startup now has coverage proving finalized-item replay happens before
  signed local status can report materialized rows, while public submissions
  remain halted in external-finalization mode.
  Newly applied externally finalized units are persisted through the same
  finalized-item store and journal transaction before local runtime mutation.
  The same retained finalized-item store is now exposed through strict catch-up
  response evidence with whole-unit slicing, and returned evidence is replayed
  through the AlephBFT recovery constructor before any catch-up suffix is
  persisted or applied.
  Raw AlephBFT backup bytes are now persisted per session, and the Axum runtime
  exposes a run-session-compatible DB backup writer/reader: writes stay buffered
  until `flush` or `close`, flushes append bytes through a mint DB transaction,
  and readers load stable persisted snapshots for restart recovery.
  The same runtime now assembles the concrete upstream `LocalIO` bundle from the
  consensus data provider, DB backup IO, and finalized-unit receiver so startup
  can spawn `run_session` without changing the public route semantics. A
  focused live-session starter now builds that `run_session` task, installs the
  private REST inbound network queue, starts the bounded outbound network pump,
  retains failed network-message deliveries in a bounded in-memory retry queue,
  owns task shutdown, forwards finalized units into the CDK apply loop, and
  activates network-finality mode for valid AlephBFT federations with remote
  peers. Router startup now owns that live-session handle for `aleph_bft` mints,
  starts the session before deriving readiness, clears the startup halt for
  empty verified histories, and keeps replayed histories in catch-up certificate
  verification until catch-up/recovery evidence explicitly opens public routes. A focused
  two-member in-process live-session test now injects the private REST transport
  into both router-owned upstream AlephBFT tasks, queues a real mint-quote
  operation on one member, exchanges SCALE AlephBFT network messages through the
  signed `/aleph-bft/messages` route, and proves both members finalize and apply
  the same operation with matching finalized-item counters. The same router
  transport harness can now inject delayed AlephBFT private REST network-message
  posts plus bounded exact signed replay attempts, and a live-session regression
  proves those transport faults still produce one finalized journal entry per
  member. Five-member live-session coverage now proves one unavailable member
  leaves public submission retryably pending without durable mint state mutation,
  then finalizes the exact same operation after the member returns and retained
  network messages are redelivered. The unfinalized live-session restart
  regression now explicitly covers the earlier restart window where pending
  operation rows exist before upstream AlephBFT backup bytes have been flushed;
  after restart, pending-operation replay regenerates protocol traffic, drives
  the member to finality, and clears pending operation rows. Operator
  `cdk-mintd` startup now accepts the `aleph_bft` backend instead of failing the
  config before runtime construction. A deterministic private-REST reorder
  regression now holds one opaque AlephBFT `/messages` POST until a later
  message overtakes it, then proves both members still finalize exactly one
  journal entry. A five-member/threshold-three live-session matrix now combines
  reorder, duplicate signed replay, and delayed private-REST network-message
  faults while proving every member finalizes exactly one journal entry and
  clears pending operation rows. A post-backup restart regression now waits for
  real persisted upstream AlephBFT backup bytes, restarts one member from the
  same DB and seed, and proves the restarted member finalizes a second
  operation without reusing its local CDK unit index. The runtime and
  backend-adapter runtime now expose a guarded empty-session rollover API with
  focused tests for successful rotation, in-memory rejection, and pending-local
  finality rejection. Storage-backed live-session startup now persists a
  monotonic active-session marker, runtime startup recovers the maximum of that
  marker and finalized-item evidence, an Axum regression proves a newly
  rotated empty session survives restart before any finalized item exists, and
  the Axum live-session wrapper can now explicitly rotate a running empty
  session into the next live network pump while refusing non-empty pending
  mempools. Two-member live private-REST coverage now proves that rotation also
  works after a session has finalized real traffic, and the live-session and
  inbound-network slots are shared across router state clones so the original
  private REST routers observe the rotated session without route reinsertion.
  Live `run_session` completion now also supervises automatic rotation into the
  next session; a focused two-member regression proves the auto-rotated session
  stays live and can finalize a later operation through those original routers.
  The process-smoke generator can now opt into a small live-session round cap
  and wait for every member to report an auto-rotated AlephBFT session before
  wallet operation batches. Longer repeated external partition runs remain an
  optional deployment-hardening exercise rather than an unchecked milestone.
- [x] On startup, rebuild the consensus runtime from durable state, fetch missing
  signed catch-up certificates or journal ranges from peers, replay unapplied items,
  and only then enable public federation mint/swap/melt/restore/check-state
  routes. The in-memory path already restores persisted journal/session evidence
  and readiness; the AlephBFT path now restores local finalized-item state and
  can durably apply externally finalized units. Router startup now starts the
  live AlephBFT session before deriving readiness, clears the startup halt for
  empty histories once live finality exists, and sends replayed histories to
  catch-up certificate verification rather than opening public routes immediately.
  Public
  authoritative wallet routes also check the adapter finality mode directly, so
  a same-process healthy readiness value cannot bypass the
  network-finality guard. Peer fetch now uses both
  accepted-operation lag and finalized-item cursor lag, requests finalized-item
  suffixes by cursor, and rejects incomplete whole-unit pages before durable
  mutation. Live pre-backup, post-backup, finalized-before-local-apply, and
  five-member restart tests plus SQLite/Postgres process smokes cover restart,
  reorder, offline catch-up, and readiness reopening after convergence.
- [x] Never silently fall back to local signing or single-member processing.
  `cdk-mintd` always constructs the AlephBFT runtime and rejects the removed
  `consensus_backend` config field. Axum-created
  AlephBFT federation state starts `halted`/not-ready until the live AlephBFT
  session installs network finality, so public wallet submissions fail before
  queue admission if finality is not actually available. Startup restore
  preserves that backend-imposed halt over stale persisted healthy snapshots,
  and Axum rechecks the finality mode before authoritative public routes even
  when sync state is healthy.
- [x] Add backpressure and admission limits for pending wallet submissions,
  consensus messages, catch-up ranges, and response waiters. The AlephBFT submit
  path now enforces a configurable `pending_mempool_item_limit` before committing
  a newly accepted operation to the live runtime; full queues fail fast without
  growing the pending set, duplicate pending operation IDs continue to collapse
  onto the existing entry, and competing pending or drained mint/melt quote
  operations with the same semantic conflict key are rejected before finality.
  Drained local units now remain counted as pending finalization until finality
  decides their exact operation ID or semantic conflict key; peer-finalized
  duplicate/conflicting items release only the affected local reservation and
  keep unrelated drained items reserved.
  It also enforces a configurable
  `response_waiter_limit` before swapping new pending operations into the live
  runtime; waiter slots are released on timeout or cancellation, and signed local
  status reports the pending waiter count. Existing coverage also includes size
  limits, private REST inbound/outbound concurrency limits, catch-up page
  limits, bounded driver ticks, finalized-unit peer request timeouts, and a
  configurable per-unit finalized-unit remote delivery cap that stops after
  quorum and fails before networking if the cap is below the remote quorum
  requirement. The live network retry queue has an overflow regression proving
  the oldest failed delivery is dropped from bounded runtime memory and counted
  when the queue exceeds its cap.
- [x] Expose status for AlephBFT session, pending item
  count, finalized item count, signed catch-up certificate height, catch-up state,
  member lag, response-cache hit rate, and duplicate submissions. The
  `FederationConsensusRuntimeStatus` snapshot now exposes the runtime-visible
  subset through signed local status and `cdk-mintd federation status`: backend,
  finality mode, federation, local member, session, next local unit index, next
  unit finalization index, next global item index, queued item count, drained
  pending-finalization item count, finalized item count, accepted operation
  count, latest accepted operation index, and roster-bound member count.
  `cdk-mintd federation status` now prints all of those runtime-visible fields
  instead of only the older session/queue/finalized/accepted subset. Signed
  local status and `cdk-mintd federation status` now also expose whether the
  router has installed the live AlephBFT session task and inbound network queue,
  so process-level checks can distinguish real live-session ownership from an
  runtime finality flag alone. Cargo registry checks confirm upstream
  `aleph-bft = 0.45.4` and `fedimint-aleph-bft = 0.36.0` are available, and the
  federation-enabled mintd status test compiles through upstream
  `aleph-bft v0.45.4`. Signed local status and `cdk-mintd federation status`
  now also expose response-cache lookup/hit counters,
  response-cache hit rate in basis points, duplicate-submission plus duplicate
  pending-waiter join counters, structured sync peer/operation-count fields for
  lagging, catch-up, catch-up certificate verification, and materialized-state
  mismatch states, and signed
  catch-up certificate quorum range fields (`from_index`, `latest_index`, operation
  count, and signer set). The status surface intentionally reports durable
  finalization indexes rather than fabricating unstable upstream round or
  consensus-height values.

Mint-side restructuring checklist:

- [x] Create explicit admission validation functions for federated operations:
  `validate_federated_mint_submission`, `validate_federated_swap_submission`,
  `validate_federated_melt_submission`, and equivalent quote/payment
  observation admission checks. Current code provides the mint, swap, melt,
  quote, payment-observation, and quarantine admission helpers.
- [x] Admission validation may read local consensus-visible state to reject
  malformed or obviously invalid requests, but it must not commit state, mark
  proofs pending, mark proofs spent, mutate quotes, attempt payments, or create
  signature shares. Current mint/swap/melt helpers are pure request-local
  checks and are exercised before operation-envelope construction. Quote,
  payment-observation, and quarantine helpers now validate operation-local quote
  IDs, method names, observer roster membership, and value consistency without
  touching quote/proof/payment state.
- [x] Keep final consensus apply stricter than admission validation. Admission
  checks are mempool policy; accepted-order apply is the authoritative validity
  decision against the current ordered state.
- [x] Add a single deterministic accepted-operation entrypoint such as
  `apply_accepted_federation_operation` and require journal replay, live
  finalization, and catch-up to use the same path.
- [x] Audit public Axum routes and `Mint::process_*` federation branches so no
  federated mint, swap, melt, restore, or change-output path can call standalone
  signing or mutation helpers before consensus acceptance. Current coverage
  wires quote/payment/quarantine plus mint/swap/melt/restore/keyset-rotation
  admission through public submit, authenticated private submit, and non-primary
  forwarding before journal append or peer forwarding. Public
  Axum mint/swap/melt handlers are source-audited to try federation submission
  before standalone fallback, restore is source-audited to read persisted
  federation shares without direct signing, and member-mode `Mint` guards reject
  direct standalone mint, swap, melt, and raw blind-signing calls.
- [x] Treat external payment reads and payment attempts as non-deterministic side
  effects outside the pure apply path. Their results must enter consensus as
  signed observations or deterministic payment-attempt/finalization operations.
- [x] Select the real outgoing melt payment actor deterministically from accepted
  consensus state, and use an idempotency key so duplicate wallet requests,
  retries, crashes, or catch-up never make two outgoing payments.
- [x] Ensure no blind-signature share is created until the accepted operation
  hash, output index, blinded message, amount, keyset ID, member ID, and
  accepted session/index are known and persisted together. Current coverage
  includes an unpaid-quote regression proving federated mint apply fails before
  storing blind signatures or federation signature-share rows, plus checked
  accepted-share provenance constructors used by mint, swap, and melt-change
  signing paths that reject operation-ID mismatches before persistence. Public
  mint and swap handlers now read wallet-facing responses from those persisted
  rows after accepted apply.
- [x] Make all federation output paths v3/BLS-only: admission rejects non-v3
  output keysets, accepted apply rechecks the same condition, and startup
  rejects non-v3 aggregate federation keysets. Current coverage includes
  mint-side runtime rejection for mint/swap/restore/melt change and wallet-side
  rejection before fanout for mint, batch mint, swap, restore, and melt change.
  Fresh targeted coverage also proves common federation config rejects v1, v2,
  and non-BLS aggregate keysets, Axum public routes reject non-v3 mint, swap,
  and melt-change outputs before consensus work and reject non-v3 restore
  placeholders before the local share read, mintd federation
  startup rejects non-v3/non-BLS aggregate configs, and standalone
  non-federated mint builders still honor legacy keyset-version settings.
- [x] Add a bounded operation response cache keyed by operation hash for mint,
  swap, melt, and quote state responses that need idempotent
  public API behavior. The local accepted-operation notification cache now
  retains a bounded `response_cache_limit` of accepted responses for idempotent
  waiters and status visibility. Active waiters keep a per-operation response
  copy so cache eviction cannot race a notified request. AlephBFT duplicate
  submits now also fall back to the durable accepted journal after cache
  eviction, returning a duplicate response without requeuing work or adding a
  waiter. Public mint, swap, and melt route coverage now bounds the cache to
  one accepted response, evicts the first operation with a second accepted
  operation, retries the first request, and verifies the wallet-facing response
  is rebuilt from persisted accepted state without appending a third operation.
  Durable journal replay remains the source of truth after cache eviction; a
  separate typed cache is unnecessary.
- [x] Add a waiter registry keyed by operation hash so duplicate HTTP requests
  join the same pending consensus wait rather than submitting unbounded copies.
  AlephBFT submits now check the existing operation notification registry after
  durable journal duplicate detection but before consensus admission. If the
  same operation is already pending locally, the duplicate request joins the
  existing waiter instead of re-entering queue admission. The canonical
  operation ID and durable pending/finalized records provide idempotency; no
  private REST sequence metadata is coupled to operation finalization.
- [x] Define retryable timeout semantics: if consensus has not finalized before
  the public API timeout, return a clear pending/retryable error without
  changing operation state; retrying the exact request must later return the
  finalized response.
  AlephBFT submits now return a typed `OperationPendingFinalization` federation
  error instead of a generic string timeout. That error maps to Cashu's
  retryable pending error response, remains non-definitive for wallet recovery,
  leaves the pending operation queued, and a later exact retry can join the
  existing operation and receive the finalized response once the accepted unit is
  applied.
- [x] Ensure accepted conflicting operations produce deterministic rejected
  outcomes that are persisted and returned idempotently where appropriate.
  AlephBFT-finalized operations that conflict with an existing journal conflict
  key now append a `Rejected` journal entry, preserve operation-id duplicate
  replay, rebuild from durable entries without reserving the rejected conflict
  key, and return a rejected submit response instead of aborting finalized-unit
  replay.
- [x] Reject terminal rejected or failed submit-operation responses before
  wallet-facing handlers read persisted mint, swap, or melt state. Submit
  responses now expose a shared accepted-status guard that allows only accepted,
  applying, or applied journal entries through public and forwarded submit
  boundaries. Rejected and failed finalized outcomes become a typed
  `RejectedOperation` federation error. Terminal duplicates are reconstructed
  by operation ID and do not mutate durable DKG replay watermarks.

Federated public API checklist:

- [x] `post_mint` and `post_batch_mint` compute the canonical mint operation
  hash, check for an accepted/cached response, run admission validation if
  needed, submit the operation to AlephBFT, await the accepted outcome, and
  return this member's persisted signature shares in normal Cashu response
  shape. Current in-memory/public route coverage returns persisted accepted
  rows after submit, verifies the response against the stored share record, and
  proves an exact duplicate accepted request returns the same persisted response
  without appending a second operation, including after local response-cache
  eviction. SQLite restart coverage now rebuilds `MintState`, replays the
  persisted runtime state, retries the same public batch mint request, and
  returns the persisted shares without appending another operation. In-memory
  catch-up coverage now applies peer-accepted quote, payment, and mint
  operations before the public request arrives and returns the persisted shares
  without appending another operation. AlephBFT not-ready coverage now proves
  the public batch mint route fails closed before queueing work, appending a
  journal entry, or registering a waiter while network finality is unavailable.
  Batch-mint pending-finalization coverage now proves a timed-out public request
  leaves the operation queued without issuing shares, and a later exact retry
  waits for the same operation and returns the persisted accepted response after
  adapter-level finality. Single-quote `post_mint` pending-finalization coverage
  now proves the same retry behavior while the quote remains paid, unissued, and
  unsigned until the exact queued unit is finalized through the current adapter
  boundary. Live duplicate single-quote `post_mint` requests now join the same
  pending operation like batch mint and both waiters receive the same persisted
  accepted response after adapter-level finality.
- [x] `post_swap` computes the canonical swap operation hash, checks for a
  cached response, validates the request without pending/spent mutations,
  submits to AlephBFT, awaits accepted apply, and returns persisted output
  shares in request-output order. Current in-memory/public route coverage
  returns persisted accepted rows after submit, verifies the response against
  the stored share record, and proves an exact duplicate accepted request
  returns the same persisted response without appending a second operation,
  including after local response-cache eviction. SQLite restart coverage now
  rebuilds `MintState`, replays the persisted runtime state, retries the same
  public swap request, and returns the persisted shares without appending
  another operation. In-memory catch-up coverage now applies a peer-accepted
  swap before the public request arrives and returns the persisted shares
  without appending another operation. AlephBFT not-ready coverage now proves
  the public swap route fails closed before queueing work, appending a journal
  entry, or registering a waiter while network finality is unavailable.
  Pending-finalization coverage now proves a timed-out public swap leaves the
  operation queued without journal entries, proof-state mutation, signature
  shares, or retained waiters, and that a retry joins the pending operation and
  returns persisted shares after the exact queued unit is finalized through the
  current AlephBFT adapter boundary. Live duplicate public swap requests now
  join the same pending operation and both waiters receive the same persisted
  accepted response after adapter-level finality.
- [x] `post_melt` computes the canonical melt operation hash, checks for cached
  reserve/final response, validates proofs and change outputs without payment
  side effects, submits to AlephBFT, awaits accepted reserve/finalization
  according to the configured fake or real payment observation policy, and
  returns the same response on duplicate requests. Current in-memory/public
  route coverage proves an exact duplicate accepted melt with change returns the
  same persisted paid response and change signatures without appending a second
  operation, including after local response-cache eviction. SQLite restart
  coverage now rebuilds `MintState`, replays the persisted runtime state,
  retries the same melt request, and returns the persisted paid response plus
  change signatures without appending another operation. In-memory catch-up
  coverage now applies a peer-accepted melt before the public request arrives
  and returns the persisted paid response plus change signatures without
  appending another operation. AlephBFT not-ready coverage now proves the public
  melt route fails closed before queueing work, appending a journal entry,
  registering a waiter, or moving the melt quote out of `Unpaid` while real
  finality is unavailable. Failed fake-wallet coverage now proves an exact
  duplicate failed melt returns `PaymentFailed`, keeps inputs released, appends
  no second operation, and does not attempt a second outgoing payment. SQLite
  restart and in-memory catch-up coverage now prove that exact duplicate failed
  melts remain terminal and do not attempt another outgoing payment after
  recovery. Pending-finalization coverage now proves a timed-out public melt
  leaves the operation queued without journal entries, proof-state mutation,
  quote-state mutation, change signatures, or retained waiters, and that a
  retry joins the pending operation and returns the persisted paid response plus
  change shares after the exact queued unit is finalized through the current
  AlephBFT adapter boundary. Live duplicate public melt requests now join the
  same pending operation without an outgoing payment attempt before finality, and
  both waiters receive the same persisted paid response after one accepted
  outgoing payment attempt.
- [x] Mint quote creation and melt quote creation remain idempotent around quote
  IDs and quote request payloads; duplicate quote operations return the
  accepted quote response, while conflicting quote payloads are rejected.
  Current route coverage proves AlephBFT-not-ready public mint/melt quote
  routes fail closed before creating local quote state, appending journal
  entries, queueing work, or registering waiters. Consensus-level
  duplicate/conflict coverage exists for quote operations; AlephBFT pending
  queue coverage now rejects competing mint/melt quote operations with the same
  quote conflict key before finality. Federation wallets now generate one quote
  ID per creation attempt and carry it as the federation-only
  `federation_quote_id` query parameter without changing the Cashu JSON body.
  Every member route requires that ID in federation mode, returns the accepted
  response for an identical retry, and rejects a different request under the
  same ID; standalone routes remain unchanged. Sequential member fallback
  reuses the same assigned ID. After an ambiguous HTTP timeout the wallet first
  performs one threshold status read: a threshold-visible accepted quote is
  returned without calling another creation endpoint, while a genuinely
  invisible quote may fall back using the same ID. Mint and melt regressions
  cover timeout recovery without duplicate fallback, invisible-timeout
  fallback, pending submission recovery, exact ID reuse, definitive errors,
  threshold visibility, route-level conflicting payload rejection, and HTTP
  query serialization.
- [x] Payment observation submission is idempotent by payment ID, quote ID,
  observer, method, amount, state, and proof material. Federation observation
  payloads carry no observer-local wall clock, so canonical consensus bytes and
  operation-ID bytes are identical; changed proof material produces a distinct
  conflicting operation ID. Durable mint payment time is derived from accepted
  logical journal order. Covered by
  `payment_observation_operation_ids_track_exact_evidence` and
  `mint_payment_observation_uses_payment_id_and_consensus_logical_time`.
- [x] Restore and check-state remain read-only, but they must refuse
  authoritative answers from lagging members and must only return shares or
  states derived from accepted consensus-visible state. Public Axum restore and
  check-state handlers now verify federation sync readiness after auth and
  before local mint reads; the focused lagging-member regression proves both
  routes fail closed while lagging and resume once healthy. Restore is not a
  consensus operation: it reads signature-share provenance persisted by prior
  accepted mint, swap, or melt-change operations, while the wallet validates
  matching threshold views and aggregates the returned BLS shares.
- [x] Public APIs must support the race where the accepted operation arrives
  between admission validation and queue submission: the submit path should then
  return the cached accepted response. Submit responses now recompute the
  embedded journal entry envelope's operation ID before any accepted response is
  returned to wallet-facing mint, swap, or melt handlers. The focused in-memory
  submit regression now validates an operation, accepts it through a competing
  path, then submits the same envelope through the wallet path and proves the
  durable duplicate response is returned without appending or applying a second
  journal entry.
- [x] Public APIs must support the race where the accepted operation arrives
  before the wallet reaches this member: the handler should compute the same
  operation hash and immediately return the persisted response. AlephBFT submit
  coverage now proves this for an already accepted duplicate after the bounded
  notification response cache has evicted the original response. Public mint,
  swap, and melt handler coverage now proves the same persisted wallet-facing
  response behavior after local response-cache eviction. The mint, swap, and
  melt catch-up regressions now explicitly certify the stronger
  accepted-before-request case where peer-accepted operations are replayed
  before the public request reaches this member and the handler returns
  persisted state without a second append.

Wallet checklist:

- Design constraint: keep the CDK wallet simple. Do not add a Fedimint-style
  persistent client state-machine stack unless a concrete later requirement
  proves it necessary.
- [x] The federated connector continues to fan out the same mint, swap, and melt
  request to all configured members, accepts threshold compatible responses,
  validates member shares, aggregates BLS shares, and returns ordinary Cashu
  wallet results. The connector now keeps all successful value-operation
  responses for aggregation, validates each member's full response, ignores bad
  signature shares when a threshold-compatible valid subset remains available,
  and returns normal aggregate `MintResponse`, `SwapResponse`, and melt-change
  responses.
- [x] The connector treats member timeouts and pending/retryable responses as
  normal retry inputs, not as proof construction failures unless threshold
  cannot be reached. Pending quote and pending-finalization errors remain
  non-definitive, and HTTP 429 rate limits are now classified as retryable so a
  temporarily rate-limited member can still contribute a threshold share after a
  configured retry.
- [x] The connector rejects members advertising mismatched federation IDs,
  mismatched aggregate keysets, mismatched wallet protocol versions, or non-v3/BLS
  aggregate federation keysets before submitting value-changing requests.
  Current code exposes typed Cashu `MintInfo.federation` metadata containing
  federation ID, member ID, config digest, and wallet protocol version. Federation
  mints advertise this metadata in `/v1/info`; wallets require it before
  aggregate keyset discovery and before mint, batch mint, swap, melt, restore,
  check-state, and mint-info fanout. The connector rejects missing or mismatched
  member mint-info metadata, excludes members with mismatched or non-v3 active
  aggregate keysets, fails before signing if a threshold compatible subset
  cannot be reached, rejects unsupported public-config wallet protocol versions at
  connector construction, and rejects non-v3 mint, batch mint, swap, restore,
  and melt change outputs before contacting any members.
- [x] The connector tolerates a member that already processed the operation and
  another member that is still waiting, as long as a threshold of valid final
  responses can be collected. The wallet regression
  `federated_mint_connector_tolerates_mixed_final_pending_and_rejected_value_responses`
  now covers mint, swap, and melt fanout with one already-final member, one
  retryable pending-finalization member that later returns a final response,
  and one definitive rejecting member.

AlephBFT and consensus unit tests:

- [x] A 4-member AlephBFT test finalizes ordered sessions with one faulty or
  offline member and all honest members store identical accepted item order.
  Covered at the adapter boundary by
  `consensus_runtime_finalizes_four_member_order_with_faulty_duplicates`,
  which finalizes accepted, duplicate, and conflicting units across four
  roster-bound adapters and compares their journal and finalized-item
  fingerprints.
- [x] A 5-member AlephBFT test finalizes ordered sessions with one faulty or
  offline member and all honest members store identical accepted item order.
  Covered at the adapter boundary by
  `consensus_runtime_finalizes_five_member_sessions_with_silent_member`,
  which advances from session 0 to session 1, leaves member three silent, and
  proves the other adapters converge.
- [x] Runtime-level AlephBFT session rollover has focused unit coverage.
  `consensus_runtime_begin_aleph_bft_session_rotates_empty_session`,
  `consensus_runtime_begin_aleph_bft_session_waits_for_pending_finality`,
  and `consensus_runtime_begin_aleph_bft_session_can_requeue_pending_finality`
  pass under
  `cargo test -p cdk-common --features federation begin_aleph_bft_session -- --test-threads 1`.
- [x] Durable empty-session AlephBFT recovery has focused storage and Axum
  coverage. `federation_aleph_bft_active_session_persistence` proves the
  active-session marker is durable, rollback-safe, and monotonic under SQLite,
  `federation_mint_migrations_cover_sqlite_and_postgres` covers the migration
  pair, and
  `aleph_bft_live_session_persists_and_recovers_empty_active_session` proves
  live-session startup persists session 1 and restart recovers it before any
  finalized item exists.
- [x] Duplicate proposals from multiple members produce at most one applied
  operation outcome and identical idempotent responses. Covered by the
  4-member and 5-member adapter finalization regressions, which include
  duplicate finalized operations from different members and assert duplicate
  finalized-item status without adding a second journal outcome.
- [x] Delayed, reordered, and duplicated AlephBFT network messages do not change
  the final accepted order seen by honest members.
  Private REST stand-in coverage now proves exact duplicate finalized-unit
  delivery is idempotent at the route/apply boundary and does not append another
  finalized item or journal entry. The broadcast driver also now skips duplicate
  peer entries before delivery attempts or quorum accounting, so one remote
  member cannot be counted twice toward a threshold. Catch-up replay rejects
  reordered AlephBFT finalized-item evidence before quote, journal, or
  finalized-item storage mutation. Router-started upstream AlephBFT sessions now
  have a focused two-member in-process private REST transport test proving a
  real network-message exchange can finalize and apply the same operation on
  both members. Cargo registry checks confirm upstream `aleph-bft` 0.45.4 is
  available and is the workspace dependency used by the federation feature.
  Bounded in-memory redelivery retries failed network-message deliveries to
  recovered peers. Gossip retries are deliberately runtime-only: upstream
  AlephBFT persists its DAG backup, rebroadcasts units, and repeatedly requests
  missing coordinates, parents, and newest units after recovery. The queue
  retains failed peer subsets, prioritizes fresh messages, limits idle retry
  work and per-attempt duration, reports dropped deliveries, and evicts the
  oldest delivery at its hard bound without adding database write contention.
  Durable pending-operation rows now persist locally accepted AlephBFT operation
  envelopes before wallet-facing pending responses, production runtime startup
  replays those pending envelopes back into the live AlephBFT data provider, and
  finalization or catch-up clears them once the operation is durably decided. A
  five-member live-session regression now restarts the submitting member while
  one peer is unavailable and proves finality resumes without wallet resubmission
  when the peer returns.
  A two-member router-started live-session fault regression now injects delayed
  AlephBFT private REST network-message posts plus bounded exact signed replay
  attempts and proves the operation still finalizes once, with one journal entry
  per member and no duplicate durable application.
  Peer delivery is concurrent per message, the first live-session pump tick is
  delayed by one configured interval to avoid startup-router races, live-session
  shutdown now aborts upstream child tasks, and SQLite backup reads preserve
  invalid UTF-8 bytes as blobs. Deterministic opaque AlephBFT message reordering
  and a five-member reorder/duplicate/delay matrix are now covered. A
  five-member/threshold-three post-backup restart regression now waits for real
  upstream AlephBFT backup bytes, restarts the submitting member, verifies the
  recovered local unit index, appends new backup bytes after restart, and proves
  two more operations converge in identical order across all members.
  Route-level same-sequence equivocation coverage now accepts one signed
  AlephBFT network message, rejects a second valid-but-different payload signed
  with the same sender sequence as stale, leaves the sender watermark unchanged,
  and proves the equivocated payload is not delivered to the inbound network
  queue. Router-started live-session coverage now extends that stale-sequence
  guard through the production private REST transport after real AlephBFT
  finality, proving a same-sequence different-body network message is rejected
  as stale without advancing the receiver watermark or adding journal/finalized
  state. Five-member/threshold-three live-session coverage now extends that
  proof to a BFT-sized federation and verifies a later operation still finalizes
  in identical order across all members after the rejected equivocation.
  Adapter-boundary session-rotation coverage now blocks `begin_session` while a
  locally drained unit is still awaiting finality, proves the adapter remains on
  the old session until that unit finalizes, and verifies recovered local unit
  numbering in a rotated session. Router/runtime startup now derives the active
  AlephBFT session from persisted finalized-item evidence instead of hardcoding
  session zero, and live-session network pumps override stale injected pump
  configs with the recovered consensus session before signing private REST
  network-message envelopes. Runtime-level AlephBFT wrappers now expose a
  guarded `begin_aleph_bft_session` API that can rotate to an empty later
  session, rejects in-memory runtimes, and refuses rotation while drained local
  units still await finality. Live-session startup now durably records a
  monotonic active-session marker, startup recovers the maximum of that marker
  and finalized-item evidence, and focused storage/Axum tests cover empty
  newly-rotated session recovery before finalized evidence exists. Runtime
  gossip queues begin empty for each recovered session and Aleph regenerates
  required protocol traffic from its backup.
  Focused long unavailable-window coverage now proves multiple operations can
  remain durably pending without pre-finality state mutation while one member is
  unavailable, then converge in the same accepted order after that member
  returns. The focused duplicate/delay, deterministic reorder, and combined
  five-member reorder/duplicate/delay live-session tests all pass against the
  upstream AlephBFT runtime and compare the honest members' accepted order.
- [x] Byzantine equivocation or malformed unit data from one member is rejected
  or ignored without causing honest members to diverge.
  Private REST stand-in coverage now proves a signed finalized-unit payload
  with tampered derived operation IDs, tampered body hash, a mismatched body
  federation ID, or a future session is rejected before finalized-item, journal,
  sequence, quote, or consensus-runtime mutation. Signed AlephBFT
  network-message route coverage also rejects bad SCALE payloads, valid SCALE
  messages with trailing bytes, session mismatches, and same-sequence
  different-payload equivocation before sender sequence persistence or inbound
  queue delivery. Router-started live-session coverage now sends a malformed
  but correctly signed AlephBFT network message through the private REST route,
  proves the sender sequence is not consumed by the rejection, and then proves
  later valid network traffic still finalizes and applies the same operation
  across both members. It also now replays a different signed body with an
  already accepted live sender sequence after real finality and proves the
  receiver rejects it without watermark, journal, or finalized-item mutation.
  Five-member/threshold-three live-session coverage now applies the same
  equivocation attempt in a BFT-sized federation and proves a subsequent honest
  operation finalizes across every member without order divergence. A separate
  five-member upstream AlephBFT regression now restores two member-one runtimes
  from the same pre-operation backup, permits normal parent/control traffic,
  withholds only their data-bearing messages, and makes them sign different
  Cashu operations at the same local Aleph unit index. The two branch payloads
  are then delivered to alternating peers through fresh authenticated private
  REST requests with distinct increasing sequences. After the duplicate
  runtime is removed, at most one fork variant reaches the journal, a later
  honest operation finalizes, and all five honest states converge on identical
  journal order and materialized-state digest. The malformed, same-sequence,
  combined reorder/duplicate/delay, and partition/catch-up regressions continue
  to pass alongside the increasing-sequence fork test.
- [x] Consensus stalls safely when fewer than the BFT quorum are connected and
  resumes when enough members return.
  Private REST stand-in coverage now proves a 3-member/threshold-3 broadcast
  that reaches only one remote acknowledgement fails closed with the local unit
  retained, then retries the same unit and applies when both remotes accept.
  Router-started upstream AlephBFT coverage now also proves a 5-member live
  session with four started members and one unavailable member leaves the public
  operation retryably pending and writes no mint quote, journal entry, or
  finalized item before finality, then finalizes the exact operation after the
  member returns and the in-memory retry queue redelivers failed messages.
  Expanded five-member coverage now keeps several distinct operations pending
  through the same unavailable-member window, verifies no member mutates quote
  or journal state before finality, and then verifies all members apply the
  same accepted operation order after the missing member returns.
  The live five-member partition matrix now also proves a two-member-offline
  operation remains durably pending with no materialized quote, then finalizes
  autonomously and exactly once after a fourth member returns without client
  resubmission. Durable pending-work activity leases renew only while real work
  remains and become fully idle after finality.
  `aleph_bft_live_sessions_finalize_after_unavailable_member_returns` and
  `aleph_bft_live_partition_matrix_catches_up_and_certifies_lagging_members`
  pass against the router-started upstream runtime. Full-process restart
  matrices remain a separate operational-hardening gate; transient gossip is
  never replayed as durable application state.
- [x] Add a live private-REST AlephBFT regression for the exact
  self-partitioned proposer case: keep the submitting member's public/client
  route usable while its federation transport to the other members is
  unavailable, submit a valid wallet operation to that isolated member, assert
  the operation is durably queued/pending with no quote, journal, finalized
  item, proof-state, signature-share, or payment side effect on any member,
  heal the federation transport, and prove the exact same operation ID
  finalizes exactly once across every member with pending rows cleared and
  accepted order plus state digest converged.
  Covered by
  `aleph_bft_live_partition_matrix_catches_up_and_certifies_lagging_members`:
  an already-paid mint operation is submitted through a member whose outbound
  federation links are blocked, remains durably pending while its quote stays
  paid with no finalized item, completed-operation row, or signature share on
  any member, then finalizes once after heal. Duplicate retry returns the same
  accepted operation, every member has the same ordered journal, finalized
  cursor, and materialized state digest, all pending rows clear, and activity
  traffic stays flat after durable work disappears.
- [x] Add live private-REST AlephBFT below-quorum-offline coverage for a
  federation size/threshold where the number of offline members remains within
  the tolerated fault budget. Submit mint, swap, melt, restore/check-state, and
  payment-observation operations through still-connected members, prove they
  finalize with the available quorum, then reconnect the offline members and
  prove catch-up replays finalized AlephBFT items, journal entries,
  materialized mint state, payment observations, completed-operation rows,
  signature-share provenance, accepted-order digests, and state digests without
  duplicate signing or duplicate outgoing payment attempts.
  Covered by
  `aleph_bft_live_one_member_offline_value_window_catches_up_without_duplicate_payment`.
  Member five goes offline before a 14-operation window containing mint quote,
  four-observer incoming-payment quorum, mint issuance, swap, restore,
  check-state, melt quote, melt reservation/change, and four-observer outgoing
  payment quorum. Four connected members finalize every operation, aggregate
  accepted mint shares into ordinary proofs, restore the accepted swap share,
  and elect exactly one fakewallet outgoing payment actor. The lagging member
  refuses authoritative reads, then replays whole finalized AlephBFT units and
  obtains quorum-signed catch-up certification. All five members converge on
  journal order, finalized cursor, materialized state digest, proof states,
  payment observations, completed-operation rows, and exactly one provenance
  row per mint, swap, and melt-change share. Catch-up leaves the total outgoing
  payment call count at one.
- [x] Add live private-REST AlephBFT above-quorum-offline coverage where too
  many members are disconnected for consensus finality. Submit valid operations
  to reachable members and to an isolated proposer, prove every request fails
  closed or returns retryable pending-finalization, durable pending-operation
  rows are retained, no member applies pre-finality state, no shares or outgoing
  payment side effects leak, and after enough members reconnect the exact
  queued operations either finalize once in deterministic order or are safely
  rejected by consensus-visible conflict rules with all honest members
  converging to the same journal/state digest.
  The original live matrix proves a mint-quote operation with
  two of five members offline times out as retryably pending, creates no quote
  or accepted journal entry anywhere, finalizes autonomously once one member
  returns, and later catches the final member up to identical order, cursor,
  and state digest. The returning fourth member now follows the production
  restart path by reconstructing its consensus runtime and REST sequence state
  from durable storage before rejoining AlephBFT; the final lagging member does
  the same before signed catch-up and quorum certification. The matrix uses the
  production activity-aware cadence because its old fixed 10 ms test cadence
  let surviving members race thousands of artificial rounds ahead during a
  long partition and made recovery depend on test-runner load. Repeated
  isolated runs and the full parallel Axum suite now pass.
  `aleph_bft_live_below_finality_conflicts_and_melt_recover_without_leaking_effects`
  extends that proof with genuine threshold-minted inputs, two swaps that
  conflict on one proof, and an independent melt while only three of five
  members are live. All three operations remain retryably pending with no
  journal entry, proof-state mutation, completed-operation row, signature
  share, blind signature, quote-state change, or outgoing payment. Restoring a
  fourth member deterministically applies one swap, rejects its loser, applies
  the melt, and elects exactly one payment actor. The final restarted lagger
  then catches up and all five members converge on operation IDs and statuses,
  cursor, state digest, spent proofs, signature provenance, completed rows, and
  paid quote state without repeating payment. Combined with the exact isolated
  proposer regression above, this closes the above-quorum-offline gate.
- [x] Extend the partition matrix above with restart/catch-up variants: restart
  the self-partitioned proposer before reconnect, restart at least one
  non-proposer member while below quorum is offline, and restart one lagging
  member after finality but before catch-up completes. Each variant must prove
  public signing/read routes fail closed until catch-up evidence is verified,
  AlephBFT backup/finalized-item cursors replay safely, pending-operation rows
  are neither lost nor duplicated, and all honest members converge after
  reconnect.
  `aleph_bft_live_partition_matrix_catches_up_and_certifies_lagging_members`
  now covers all three variants with production-style durable runtime replay.
  The self-partitioned proposer retains exactly one pending mint operation
  across restart, duplicate retry while still isolated remains retryably
  pending without a signature share or completion row, and reconnect finalizes
  the same operation once. The same matrix restarts a non-proposer while the
  federation is below finality and restarts the final lagger after finality but
  before signed catch-up certification. Every path converges on journal order,
  finalized cursor, materialized state digest, and empty pending rows.
- [x] Restart during unit creation, after finalization before apply, and after
  apply before response persistence all recover to the same journal and state
  digest.
  Storage-backed AlephBFT coverage now proves restart after local unit creation
  but before finalization leaves no durable mint quote, journal entry, or
  finalized-item row, rebuilds with zero accepted operations, and lets a wallet
  retry recreate the same canonical unit bytes. Existing replay coverage also
  exercises accepted-but-unapplied journal replay and lost waiter/cache duplicate
  handling, and router startup now replays persisted finalized-item evidence
  before serving local status. Production AlephBFT startup now also replays
  durable pending-operation rows into the live data provider, so a member that
  restarts after returning a pending wallet response can still finalize the
  operation when the missing quorum member returns. Post-upstream-backup restart
  after applied finality is now covered in both two-member and
  five-member/threshold-three live sessions. Generated process smoke now also
  restarts a configurable member immediately after repeated all-member
  operation batches return and before the explicit convergence wait, then
  requires status, convergence, and quorum-backed catch-up certificates from
  the restarted member. The same generated process smoke now also runs fresh
  all-member wallet-operation batches through the recovered member before the
  later named long-partition window. That window now has explicit generated
  controls for the partitioned member, deterministic seed range, and operation
  batch count, then requires member status, convergence, and quorum-backed
  catch-up certificates after restart. A focused upstream-backup-only crash
  window regression now copies real AlephBFT backup bytes into a fresh member
  database with no CDK finalized-item, journal, or quote rows, proves the live
  session replays and applies the backed-up finalized operation, advances the
  local data-provider unit index past the replayed local unit, and then
  finalizes a later operation. SQLite-backed apply-without-response coverage
  now also proves a timed-out submit can be finalized and applied after its
  waiter is gone, survive restart with no response cache, and return the
  durable duplicate response without requeuing. These three focused windows
  now compare consensus index, operation ID, previous/order digest, and terminal
  status across recovery, plus the complete materialized state digest; the
  recovered state is therefore proven identical rather than inferred from one
  quote row. Live empty-session rotation
  now has Axum wrapper coverage for shutting down the old task set, persisting
  the next active session, restarting the pump, and refusing non-empty pending
  mempools; post-finality rotation coverage now proves two live members can
  finalize session-0 traffic, rotate into session 1, and finalize another
  operation through the router clones that were installed before rotation. The
  automatic completion watcher now rotates a naturally completed `run_session`
  into the next live session and proves the next operation can finalize there.
  The latest crates.io check reconfirmed upstream `aleph-bft = 0.45.4` as the
  direct dependency. Local smoke tooling can now force the generated smoke
  harness onto the `aleph_bft` backend, reserve an isolated warm-up operation
  batch to drive natural session completion, align wallet-member and mint-side
  operation timeouts, shift later deterministic smoke-wallet seed windows past
  that warm-up range, and expose the flow through `just
  fed-regtest-aleph-rotation`. The generated rotation process gate now proves
  target session 2 before wallet batches and continues through the standard
  convergence checks.
- [x] Catch-up from signed catch-up certificates and journal ranges rejects bad
  signatures, wrong federation IDs, unsupported embedded payload versions, truncated ranges,
  and state-digest mismatches. Covered by
  `catch_up_response_rejects_bad_catch_up_certificate_signature`,
  `catch_up_response_rejects_truncated_signed_catch_up_certificate_range`,
  `catch_up_response_rejects_tampered_catch_up_certificate`,
  `dummy_consensus_rejects_wrong_federation_catch_up_response`,
  `dummy_consensus_rejects_future_catch_up_response_version`,
  `dummy_consensus_rejects_future_operation_version_catch_up_response`,
  `dummy_consensus_rejects_tampered_catch_up_digest_chain`, and
  `rest_peer_client_detects_catch_up_materialized_state_mismatch`.
- [x] The data provider never spins at 100 percent CPU when the pending queue is
  empty, full, or repeatedly receiving duplicate operations. Direct
  data-provider coverage now proves an inactive provider waits without
  advancing unit indexes or creating phantom work, and local submissions or
  newly seen authenticated peer payloads wake it. Existing size-limit,
  duplicate-operation, semantic-conflict, pending-mempool backpressure, and
  idle-driver tests cover the full/duplicate/conflicting cases and the Axum
  tick surface. Thirty-second process measurements found and fixed the later
  periodic empty-unit variant: pre-fix members used 27-32% CPU and grew the DAG
  every two seconds; corrected fresh idle members use about 0.9%, post-work
  debug members use about 4-5%, and the DAG stops growing. The CPU-spin issue is
  closed and needs no further work unless it is observed again.
- [x] Order-only peer status is constant-time in accepted history. Process
  sampling found the five-second peer-sync loop repeatedly deserializing every
  durable operation, including BLS points, through
  `local_peer_order_status`. SQL storage now reads only the latest durable
  operation index and order digest. Empty and
  populated tip persistence is covered, all 13 focused peer-sync tests pass,
  and the same 17-entry trace-enabled process set dropped from 15.45-17.05% to
  3.85-3.95% CPU without changing protocol behavior or materialized-state
  diagnostics.
- [x] Federated CLI startup and read-only mint-info paths avoid repeating full
  BLS aggregate-key validation after a federation registry entry has already
  been validated. Profiling showed the slow path was CPU time in repeated
  `FederationConfig::validate` / aggregate public-key reconstruction rather
  than the `/v1/info` HTTP request itself. Focused wallet tests now prove the
  untrusted public-config URL constructor still rejects tampered aggregate
  material, while the trusted registry constructor skips the redundant
  validation pass.
- [x] The 11-member, threshold-eight private-REST and SQLite failure has an
  evidence-backed fix and regression gate. The initiating timeout storm was
  synchronous unpooled `bitreq` fanout, not SQLite: a 2,000-request comparison
  measured about 27.7s through `bitreq` and 57ms through pooled async
  `reqwest`, and the former immediately reproduced synchronized 10-second
  AlephBFT request timeouts. Federation Axum now enables the reqwest transport,
  AlephBFT fanout is concurrent and quorum-aware, SQLite serializes writes per
  database while keeping reads concurrent, dropped explicit transactions roll
  back deterministically, and melt-quote persistence is atomic/idempotent with
  exact request matching. A fresh 11-of-8 sender/receiver run completed two
  mints, receive/swap, and melt with all 11 members converged on nine operations
  and zero request timeouts, database locks, duplicate quote constraints,
  range mismatches, or HTTP 500s. A separate one-member-offline run finalized
  with ten members and caught the restarted member up in about five seconds
  with identical order and state digests.
- [x] Session-outcome `range content mismatch` remains fail-closed. Mismatched
  or stale evidence is rejected before persistence, quorum certification,
  readiness changes, journal mutation, or materialized-state mutation. Focused
  tests prove rejected evidence does not enter the catch-up certificate table.
- [x] AlephBFT backup persistence treats transient locked/busy database writes
  as retryable. Process benchmarking exposed a backup-saver termination after
  SQLite storage contention; the runtime now retries backup-byte persistence
  with bounded backoff and has focused retry-classification coverage.
- [x] Repair AlephBFT live-session tail pacing so it is both pending-work aware
  and live from every quiescent creator phase. The fixed
  always-fast delay experiment was rejected because it reached the 2s idle tail
  before normal wallet traffic and reproduced high CPU. The live session now
  keeps the 2s idle tail while inactive, marks the shared activity hint when a
  local operation is accepted into the AlephBFT mempool or an authenticated
  inbound AlephBFT network message carrying CDK consensus data is queued, and
  uses a 100ms active cadence only while local pending work or tracked
  consensus-data network activity is present. The async data provider waits
  instead of creating empty units once both sources clear. Newly finalized peer
  bodies retain a one-second propagation tail so local finality cannot strand
  slower members; replayed bodies do not refresh the window. Focused coverage
  proves inactive providers wait, active tail rounds use the faster cadence,
  finalization shortens peer activity, authenticated data gossip wakes the
  provider, idle and replayed messages do not extend it, and live private-REST
  finality still completes. Fresh process verification measured a sleeping
  full mint at 8.20s and a warm post-restart mint at 5.45s. Fresh idle CPU is
  about 0.9%; post-work debug CPU is about 4-5% after replacing peer status's
  full finalized-item scan with the maintained runtime cursor. A later fresh
  process benchmark disproved the liveness assumption behind the direct
  data-provider wake test: one primary member queued a quote and payment
  observation for 90 seconds without yielding either, then yielded both within
  10ms of a fallback member's data-bearing unit. Upstream creator traces show
  why: AlephBFT awaits the data provider after creating a preunit, so an
  indefinite empty-queue wait can withhold parent units while another creator
  is waiting for parents and cannot observe its local activity hint. Keep the
  CPU and replay fixes are retained. Newly admitted public work now emits a
  signed member-wide private REST activity request bound to federation,
  receiver, sender, sequence, live session, and operation ID. Every peer marks
  bounded network activity so an empty provider can return once and release its
  already-created preunit; private duplicate admissions wake only the local
  provider, while public retries may rebroadcast. Focused transport, provider,
  finalization-tail, route/replay, and five-member post-idle tests pass. The
  fresh process gate completed 7.76s and 4.95s mints with exactly three accepted
  operations each, matching digests, zero pending work, zero fallback quotes,
  and zero duplicate submissions. A 30-second post-work idle sample held every
  DAG at 293 units and measured 0.64-0.65 CPU seconds per debug member, so the
  indefinite-wait liveness failure is closed without restoring the periodic
  idle-unit/100-percent-CPU regression.
- [x] Non-production AlephBFT latency instrumentation records stage timing
  without changing production protocol behavior. The scoped test-only recorder
  captures operation-specific submit queueing, upstream data-provider yield,
  private REST network-message delivery, upstream finalization, apply-loop
  receive, durable apply, response-cache notification, and waiter return. The
  focused live-session test passed with a fast in-process timeline of
  `SubmitQueued@1ms`, `DataProviderYielded@20ms`,
  `NetworkMessageDelivered@24ms`, `FinalizedDataReceived@68ms`,
  `ApplyLoopReceived@68ms`, `ApplyCompleted@70ms`,
  `OperationNotified@70ms`, and `WaiterReturned@70ms`; the duplicate later
  finalization/apply events come from the second live member applying the same
  accepted operation. The same harness now also covers three serial mint-quote
  operations, with submit-to-waiter deltas around 50-70ms under fast in-process
  options, and duplicate fanout, which records one `SubmitQueued`, one
  `DataProviderYielded`, two `WaiterReturned` events, and one
  duplicate-pending waiter join. A five-member threshold-three BFT-sized probe
  now covers larger-roster private REST gossip and all-member apply, with the
  local waiter returning around 1.0-1.3s under fast in-process options. A
  seven-member threshold-five BFT-sized probe now covers the same stage sequence
  at the next planned roster size, with the local waiter returning around
  2.5-3.3s and all seven members applying by around 3.0-3.8s. A default-cadence
  cold/warm probe now records the idle-tail effect without fast test delay
  overrides: the first operation after an idle pause yielded from the data
  provider after about 2.0s and returned at about 4.0-4.5s, while the immediate
  follow-up yielded after about 0.48s and returned at about 2.5s. Recorder
  installation is exclusive so parallel tests cannot overwrite the scoped active
  recorder. This gives future cadence and batching work a stable non-production
  benchmark surface.
- [x] Public federated mint signing waits for mint-quote payment observation to
  be locally visible before submitting the mint operation to consensus.
  Benchmark traces showed an ordering race where an unpaid mint operation could
  enter AlephBFT before the corresponding payment observation and then be
  rejected definitively by some members. The public route now checks the local
  backend, submits the payment observation when available, waits for local
  Paid/Issued state, and returns a retryable pending quote instead of queueing
  an unpaid mint. The federated mint route suite covers the unpaid-before-queue
  behavior plus duplicate/retry response handling. A fresh five-member
  threshold-three AlephBFT fakewallet timing run measured direct `/v1/info` at
  0.92s, federation import at 1.44s real / 1.42s user, federated `mint-info` at
  1.52s real / 0.57s user, and federated mint of 50 sat at 13.07s real / 2.48s
  user.
- [x] Bounded queues apply backpressure under load and do not drop finalized
  consensus items. The pending-mempool overflow regression now rejects only the
  new operation, keeps the already admitted unit previewable, lets duplicate
  submissions join the existing pending waiter, finalizes the retained unit
  through the bounded driver, and verifies durable finalized-item metadata plus
  the applied journal entry.
- [x] Bounded AlephBFT delivery telemetry reports distinct skipped remote
  members even when duplicate peer entries are present, matching distinct-member
  quorum accounting. Covered by
  `aleph_bft_broadcast_round_reports_distinct_skipped_members`.
- [x] AlephBFT finalized-unit responses and waiters report post-apply terminal
  status, not the pre-apply append status. Covered by
  `federation_aleph_bft_finalized_unit_reports_state_rejected_swap`, which
  finalizes valid, state-conflicting, and later valid swaps in one unit and
  proves the loser response is `Rejected` while valid siblings are `Applied`.
- [x] Restore the complete federation-enabled Axum regression suite after the
  pending-work activity repair without weakening consensus semantics. The two
  real-HTTP wallet tests now use an explicit test-only isolated-member router
  and valid accepted federation history, so they exercise member HTTP routes,
  wallet fanout, threshold aggregation, rotation, and restore without inventing
  a shared P2P history for independent in-memory stores. Private operation
  fixtures construct v3/BLS outputs from the configured keyset. Inline
  consensus finalization persists the journal order chain without a separate
  write-only digest checkpoint table.
  Finalized-unit REST replay watermarks remain liveness-only and in memory,
  including exact idempotent duplicate delivery. The five-member equivocation
  test likewise requires no durable gossip watermark, and empty-session
  rotation is stimulated by bounded activity instead of periodic idle units.
  The live partition/catch-up matrix uses actual threshold catch-up certificate
  evidence, production activity pacing, and production-style runtime replay
  for restarted members. The new below-finality value/conflict regression uses
  genuine threshold-minted proofs and verifies zero pre-finality signature or
  payment leakage followed by deterministic recovery and final-lagger
  convergence. Verification passed all 434 sandbox-compatible tests
  in the federation-enabled Axum suite, while the two real-listener tests
  passed with network permission, for 436 covered
  tests total. Federation-enabled and federation-disabled compile checks also
  remain covered.

Mint operation tests:

- [x] Admission validation for mint rejects malformed requests, duplicate
  outputs, NUT-20 failures, and non-v3/BLS keysets without writing quote
  issuance or signature-share rows; accepted apply authoritatively validates
  quote state, expiry, and amount against ordered consensus state.
  Current admission coverage proves single and batch mint duplicate outputs
  fail before consensus operation construction; apply-layer coverage still
  proves replayed unpaid quotes, expired quotes, duplicate outputs, batch
  quote/output amount mismatches, and NUT-20 missing/mutated signatures fail
  before quote issuance, blind-signature storage, completed-operation storage,
  or federation share persistence; non-v3 output rejection is covered before
  consensus submission.
- [x] Accepted mint apply persists quote issuance, blinded messages, member
  signature shares, completed operation response, and state digest atomically.
  `apply_federation_mint_rolls_back_late_failure_and_replays` injects a
  test-only failure after quote issuance, blinded-message, blind-signature,
  member-share, and completed-operation writes but before the journal becomes
  `Applied`; it proves every write rolls back, the quote remains paid and
  unissued, and an exact replay succeeds. The SQLite integration regression
  `failed_mint_apply_preserves_materialized_digest_and_replays_on_restart`
  independently forces a real unique-row failure after application starts,
  proves the journal remains `Accepted` and the materialized digest is
  unchanged, removes the fault, and verifies startup replay reaches `Applied`
  with the issued quote and signature present.
- [x] Same quote plus same outputs is idempotent before submission, while
  pending, after acceptance, after restart, and after catch-up. Current public
  route coverage proves live AlephBFT duplicates join one pending queue item
  and return the same accepted response without issuing twice, and proves the
  after-acceptance duplicate returns the same persisted `MintResponse` without
  appending another operation, including after local response-cache eviction,
  SQLite restart/runtime replay, and in-memory catch-up replay.
- [x] Same quote plus different outputs is rejected before any threshold of
  shares can be collected. Covered at the consensus-conflict layer by
  `runtime_config_rejects_sliding_window_mint_output_attack` and at the public
  route/storage layer by
  `batch_mint_rejects_same_quote_with_different_outputs_without_signing`, which
  proves a conflicting output set appends no second mint operation and stores no
  signature share for the conflicting blinded output.
- [x] A mint operation already accepted by consensus before the HTTP request
  reaches a member returns the same persisted `MintResponse`. Covered by
  `duplicate_federation_mint_after_catch_up_returns_persisted_shares`, which
  applies peer-accepted mint quote, payment observation, and mint operations
  before calling the public batch mint handler.
- [x] A mint request that times out while pending can be retried and later
  returns the accepted response without issuing twice. Covered by
  `batch_mint_pending_aleph_bft_retry_returns_persisted_response_without_issuing_twice`,
  which proves the first public batch mint times out while queued without
  issuing a share, the exact retry waits for finality, the accepted response is
  read from persisted shares, and duplicate accepted retries do not append or
  issue again.
- [x] Restore of accepted mint outputs returns persisted or deterministically
  regenerated shares tied to the original operation hash and output index.
  Covered by
  `restore_returns_persisted_accepted_mint_shares_by_original_output_index`,
  which restores a subset of accepted mint outputs in request order and verifies
  each returned signature share by original operation ID, accepted index,
  consensus evidence, blinded secret, and output index.
- [x] Federation restore ignores plain compatibility `blind_signature` rows
  that do not have accepted-operation federation share provenance. Covered by
  `restore_ignores_orphan_blind_signature_without_federation_share`, which
  stores an orphan blind-signature row, proves no federation share row exists,
  and verifies restore returns no output or signature for it.

Swap operation tests:

- [x] Admission validation for swap rejects malformed proofs, duplicate inputs,
  duplicate outputs, bad signatures, P2PK/HTLC/SIG_ALL failures, balance/fee
  errors, and non-v3/BLS output keysets without marking proofs pending or spent;
  accepted apply authoritatively decides spent inputs and ordered-state balance.
  Current apply-layer coverage proves oversized/malformed proofs, invalid
  aggregate proof signatures, duplicate inputs, duplicate outputs, P2PK missing
  witnesses, unbalanced outputs, and SIG_ALL output mutation fail before proof
  state storage, blind-signature storage, completed-operation storage, or
  federation share persistence; non-v3 output rejection is covered before
  consensus submission; oversized proof secret content and duplicate input
  proofs plus duplicate blinded outputs are rejected before consensus operation
  construction; conflicting spent-input reuse is covered by the input-reuse
  attack regression.
- [x] Accepted swap apply marks inputs spent and stores output signature shares
  in one deterministic DB transaction.
  `apply_federation_swap_rolls_back_late_failure_and_replays` injects a
  test-only failure after proof spending, proof/output operation linkage,
  blind-signature, member-share, and completed-operation writes but before the
  journal becomes `Applied`; it proves every write rolls back and exact replay
  succeeds. The SQLite integration regression
  `failed_swap_apply_preserves_materialized_digest_and_replays_on_restart`
  forces a real unique-row failure after the proof has been inserted and marked
  spent inside the transaction, proves no proof/share/signature/completed row
  escapes and the materialized digest is unchanged, removes the fault, and
  verifies startup replay reaches one spent proof and one signed output.
- [x] Exact duplicate swaps return the same persisted shares before submission,
  while pending, after acceptance, after restart, and after catch-up. Current
  public route coverage proves the after-acceptance duplicate returns the same
  persisted `SwapResponse` and does not append another operation, including
  after local response-cache eviction, SQLite restart/runtime replay, and
  in-memory catch-up replay.
- [x] Concurrent conflicting swaps spending the same proof result in exactly one
  accepted spend and deterministic rejected outcomes for the losers. Covered at
  the deterministic apply layer by
  `apply_federation_swap_rejects_sliding_window_input_reuse_attack` and at the
  public route race layer by
  `concurrent_conflicting_swaps_spend_one_input_once_without_signing_losers`,
  which sends concurrent same-proof/different-output swap requests and proves
  one applies, losers are terminal rejected/failed journal outcomes, and loser
  outputs remain unsigned.
- [x] The overlapping-window quorum-cover attack fails for mint issuance and
  swap: different output sets cannot be mixed into threshold aggregate notes.
  Mint coverage is provided by
  `runtime_config_rejects_sliding_window_mint_output_attack` and
  `batch_mint_rejects_same_quote_with_different_outputs_without_signing`.
  Swap coverage now includes
  `local_federated_members_reject_sliding_window_swap_output_attack`, which
  uses a 3-of-4 federation with ABC/BCD/CDA/DAB member views over the same input
  proofs and proves only one accepted view can return shares while the
  overlapping views fail before returning enough shares to aggregate.
- [x] A swap accepted by consensus before the wallet reaches a member returns
  the same persisted `SwapResponse`. Covered by
  `duplicate_post_swap_after_catch_up_returns_persisted_signature_shares`,
  which applies the peer-accepted swap before calling the public swap handler.

Melt operation tests:

- [x] Admission validation for melt rejects malformed proofs, duplicate inputs,
  bad spending conditions, invalid change outputs, non-v3/BLS change keysets,
  and balance/fee errors without marking proofs spent or attempting payment;
  accepted apply authoritatively decides quote state and payment progression.
  Current apply-layer coverage proves invalid aggregate proof signatures,
  duplicate inputs, P2PK missing witnesses, non-v3 change outputs, and
  unbalanced inputs fail before proof state storage, saga storage, quote-state
  mutation, change signature storage, federation share persistence, or outgoing
  payment attempts. Pending quote reuse by a different accepted operation now
  fails before proof state storage, completed-operation storage, saga takeover,
  change-share persistence, or another payment attempt, while replay of the
  original pending operation still returns the pending response without another
  payment attempt. Paid quote reuse with different input sets or different
  change outputs now fails before proof state storage, completed-operation
  storage, journal applied status, unsigned change-share storage, or another
  payment attempt. Failed quote state now retries through the accepted melt
  setup path and reaches the payment-attempt boundary with pending proofs,
  pending quote state, operation-tied saga state, and no terminal completed
  operation row. Unknown backend payment observation/status now remains
  transient and keeps the accepted melt pending without mutating durable quote
  state to `UNKNOWN`, storing completed-operation rows, or signing change
  outputs; accepted `Unknown` melt payment observations are persisted as
  evidence without asking the quote table to support `UNKNOWN`. Oversized proof
  secret and witness content now fail with `ProofContentTooLarge` before proof
  state, saga, quote-state, share, or payment side effects, and oversized proof
  content plus duplicate input proofs and duplicate change outputs are also
  rejected before consensus operation construction. Additional malformed-input
  fuzzing remains an external hardening activity.
- [x] Accepted melt reserve records pending inputs and payment-attempt intent
  before any external outgoing payment can be attempted. Covered by
  `accepted_federation_melt_persists_reserve_before_payment_attempt`, whose
  payment backend checks during `make_payment` that proofs are pending, the
  quote is pending, the melt saga is `PaymentAttempted`, and the saga is tied
  to the accepted federation operation; non-elected real-policy reserve remains
  covered by `real_policy_non_elected_melt_member_reserves_without_payment_attempt`.
- [x] Fake-wallet happy path records successful outgoing payment observations,
  finalizes the melt, marks inputs spent, signs change outputs when present, and
  returns the same response to every duplicate API request. Covered by
  `successful_federation_melt_quote_check_records_terminal_payment_observation`
  for the live public happy path plus terminal quote-status observation row, by
  `process_melt_request_returns_federated_change_shares` for change-share
  verification, and by the duplicate public melt tests for direct, cache-evicted,
  SQLite-restarted, and catch-up replayed idempotent responses.
- [x] Fake-wallet failure path records failed observations, rolls back or
  releases pending inputs according to the accepted state machine, and returns
  deterministic idempotent errors. Covered by
  `process_failed_melt_request_releases_inputs_without_change_shares`,
  `duplicate_failed_federation_melt_returns_payment_failed_without_second_payment`,
  `duplicate_failed_federation_melt_after_sqlite_restart_stays_failed_without_payment`,
  `duplicate_failed_federation_melt_after_catch_up_stays_failed_without_payment`,
  `failed_federation_melt_quote_check_records_terminal_unpaid_observation`, and
  `get_check_melt_custom_quote_skips_unattempted_unpaid_observation`.
- [x] Long-pending fake-wallet path keeps inputs pending, returns retryable
  pending status, and later finalizes or rolls back only after an accepted
  observation. Covered by
  `pending_federated_melt_public_quote_check_observes_only_terminal_settlement`,
  `process_pending_melt_request_persists_federation_operation_id`,
  `pending_federated_melt_quote_check_later_finalizes_with_change`,
  `pending_federated_melt_verified_paid_finalizes_with_change`, and
  `pending_federated_melt_check_error_keeps_recovery_state`.
- [x] Only the deterministic payment actor attempts a real outgoing melt payment,
  and retries/crashes cannot cause duplicate outgoing payments. Covered by
  `melt_payment_actor_selection_is_deterministic_for_real_policies`,
  `real_policy_non_elected_melt_member_reserves_without_payment_attempt`,
  `accepted_federation_melt_persists_reserve_before_payment_attempt`,
  `duplicate_pending_federation_melt_returns_pending_without_second_payment`,
  `duplicate_pending_federation_melt_after_sqlite_restart_avoids_second_payment`,
  `duplicate_failed_federation_melt_returns_payment_failed_without_second_payment`,
  `duplicate_failed_federation_melt_after_sqlite_restart_stays_failed_without_payment`,
  and `duplicate_failed_federation_melt_after_catch_up_stays_failed_without_payment`.
- [x] A member offline during payment observation catches up without attempting a
  duplicate payment and then returns the same final melt/change response.
  Covered by
  `catch_up_melt_payment_observations_finalize_without_duplicate_payment`,
  `duplicate_federation_melt_after_catch_up_returns_persisted_response`, and
  `duplicate_failed_federation_melt_after_catch_up_stays_failed_without_payment`.

Race and wallet scenario tests:

- [x] Many concurrent identical mint requests sent to all members produce one
  accepted operation, one response cache entry per member, and threshold
  aggregate proofs. Member-local public route idempotency is covered by
  `concurrent_identical_value_routes_are_idempotent`; multi-member wallet
  fanout with aggregate proof assertions is covered by
  `concurrent_federated_wallet_mint_fanout_aggregates_once_per_member`, which
  sends concurrent identical wallet mint requests through a 3-of-3 direct-route
  member connector and verifies one accepted mint operation, one federation
  response cache entry per member, identical wallet responses, and aggregate
  BLS signatures for every response.
- [x] Many concurrent identical swap requests sent to all members produce one
  accepted operation, one input spend, one response cache entry per member, and
  threshold aggregate proofs. Member-local public route idempotency is covered
  by `concurrent_identical_value_routes_are_idempotent`; multi-member wallet
  fanout with aggregate proof assertions is covered by
  `concurrent_federated_wallet_swap_fanout_aggregates_once_per_member`, which
  sends concurrent identical wallet swap requests through a 3-of-3 direct-route
  member connector and verifies one accepted swap operation, one spent input,
  one federation response cache entry per member, identical wallet responses,
  and aggregate BLS signatures for every response.
- [x] Many concurrent identical melt requests sent to all members produce one
  accepted reserve/finalization path and at most one outgoing payment attempt.
  Member-local public route idempotency and single fake-wallet payment-attempt
  behavior are covered by `concurrent_identical_value_routes_are_idempotent`;
  multi-member wallet fanout with aggregate change assertions is covered by
  `concurrent_federated_wallet_melt_fanout_aggregates_once_per_member`, which
  sends concurrent identical wallet melt requests through a 3-of-3 direct-route
  member connector, verifies one canonical melt operation plus matching paid
  observations on every member, confirms only the elected member attempts the
  outgoing fake-wallet payment, spends the input on every member, returns
  identical wallet responses, and validates aggregate BLS change signatures.
- [x] Conflicting mint, swap, and melt requests submitted concurrently from
  different members are serialized by AlephBFT and converge on identical
  accepted/rejected outcomes on all honest members. Covered by
  `federation_aleph_bft_finalized_unit_reports_state_rejected_swap`, which
  proves finalized state-level swap conflicts are rejected without blocking
  later applied items, and
  `federation_aleph_bft_finalized_units_serialize_cross_member_value_conflicts`,
  which applies winner operations from one member's finalized unit and
  conflicting mint, swap, and melt operations from another member's finalized
  unit to two member states, verifies matching applied/rejected journal
  outcomes, proves loser outputs receive no signature shares, and compares
  final materialized state digests.
- [x] The accepted operation arrives between admission validation and queue
  submission; the handler returns the cached accepted response without
  resubmitting a conflicting operation. Covered by
  `submit_federation_envelope_returns_duplicate_when_acceptance_races_submission`
  for the in-memory submit path and
  `aleph_bft_duplicate_after_admission_race_returns_journal_duplicate` for the
  AlephBFT queue path.
- [x] The accepted operation arrives before the HTTP request reaches the member;
  the handler returns immediately from durable state. Covered for mint, swap,
  and melt by the public catch-up duplicate-response regressions.
- [x] The member crashes after accepted apply but before waking HTTP waiters;
  retry returns the persisted response. Covered by
  `aleph_bft_duplicate_after_lost_waiters_returns_journal_duplicate`, which
  clears in-memory notification/cache state after finality and proves a retry
  returns the durable journal duplicate without registering a waiter.
- [x] The member is catching up while a wallet request arrives; value-changing
  routes return not-ready or retryable errors until catch-up certification
  succeeds. Covered by
  `value_routes_fail_closed_while_catching_up_and_resume_when_healthy`, which
  marks a federation member `CatchingUp`, proves mint, swap, and melt reject
  before journal, queue, or waiter mutation, then marks healthy and verifies the
  same operations complete.
- [x] A wallet can mint, swap, melt, restore, and check state with only a
  threshold of live reachable members while one member is offline. Covered by
  `federated_mint_connector_uses_threshold_live_members_for_wallet_operations`
  in a 3-of-5 federation where one member fails key discovery and the wallet
  completes mint, swap, melt with change, restore, and check-state from the
  reachable threshold.
- [x] A wallet tolerates one slow member, one offline member, and one bad
  signature share as long as enough valid shares are returned. Covered for
  mint, swap, and melt change by
  `federated_mint_connector_tolerates_slow_offline_and_bad_share_members` in a
  3-of-5 federation.
- [x] A full fake-backend scenario runs mint -> swap -> melt with member
  restarts, delayed payment observation, duplicate wallet retries, catch-up, and
  final state-digest equality. Covered by
  `offline_member_catches_up_multiple_operations_and_converges`, which now
  materializes the ordered scenario on a source member, catches up a lagging
  member through a pending melt and a later paid melt observation, rebuilds the
  lagging runtime from durable state, proves duplicate mint/swap/melt retries
  return persisted responses without new journal entries, finalizes melt change
  after delayed settlement, and compares final materialized-state digests. Wallet
  threshold aggregation for the same core operation set remains covered by the
  direct-route and HTTP federated wallet connector tests.

Success criteria:

- [x] No federated mint, swap, melt, restore, or change-output path can return a
  usable signature share unless the exact operation was accepted by AlephBFT or
  was already accepted earlier and retrieved from durable state. Current
  coverage includes submit-response validation that rejects tampered embedded
  journal envelopes before accepted responses can be used for share-returning
  public paths, plus startup readiness coverage proving restore/check-state
  reads fail closed while replayed journal history is still awaiting quorum
  catch-up certificate verification and reopen only after that quorum evidence is
  present. The final route audit also verifies that public mint, swap, melt, and
  restore paths return only persisted responses and that every returned share is
  reconstructed from accepted-journal provenance; orphan signature rows remain
  invisible to restore.
- [x] Every safety-critical federation mutation is reachable from ordered
  consensus apply, replay, or catch-up; no public route commits those mutations
  directly. Public handlers submit canonical envelopes and read durable outcomes,
  while direct standalone-value methods fail closed when federation mode is
  active; the federation-enabled Axum gate exercises apply, replay, restart, and
  catch-up boundaries.
- [x] Mint, swap, and melt public APIs are idempotent across duplicate requests,
  retries, timeouts, accepted-before-request races, member restarts, and catch-up.
  Exact duplicate, joined-waiter, conflicting-operation, SQLite partial-apply,
  restart, and catch-up regressions cover all three operation families without
  duplicate signing, spending, quote mutation, or outgoing payment.
- [x] Wallet behavior remains close to current CDK behavior: request, wait for
  compatible member responses, verify shares, aggregate threshold BLS signatures,
  and return ordinary Cashu proofs/responses. The 84-test wallet federation gate
  and live CLI smokes use the existing wallet connector and saga paths.
- [x] AlephBFT-backed 4-member and 5-member integration tests pass for mint,
  swap, melt, restore, check-state, restart, offline member, catch-up, duplicate
  submission, and conflicting operation scenarios. The shared one-offline value
  window now passes as both 4-member/threshold-3 and 5-member/threshold-3 live
  upstream AlephBFT tests, including certified catch-up, identical journal and
  materialized-state digests, and exactly one outgoing melt payment; the fault,
  duplicate, and conflict matrices remain covered by the surrounding live suite.
- [x] Standalone non-federated mint and wallet tests remain unchanged, including
  non-federation keyset support. The feature-off CDK library gate passes all 513
  tests, including legacy standalone keyset and restore behavior; its three
  localhost listener cases pass when run with network permission.

## Milestone 5: federation DB journal and state digests

Objective: keep replicated operation history separate from materialized mint state.

Implementation checklist:

- [x] Add DB traits for federation journal/database transactions, implemented as `FederationJournalDatabase` and `FederationJournalTransaction`.
- [x] Add SQL migrations for SQLite and Postgres:
  - [x] `federation_config`, storing a durable public config digest snapshot and rejecting conflicting database reuse at federation startup.
  - [x] `federation_catch_up_certificates`.
  - [x] `federation_journal_entries` for accepted items, operation envelopes, and operation outcomes.
  - [x] `federation_signature_share`, implemented as `federation_signature_shares`.
  - [x] `federation_payment_observations`.
  - [x] `federation_dkg_member_sequences` for effectful DKG replay watermarks.
  - [x] Keep runtime member readiness derived and in memory; the obsolete
    `federation_member_status` table and database API were removed.
- [x] Keep existing mint tables as the materialized state used by existing APIs where possible.
- [x] Add `signature_share` rows instead of overloading final `blind_signature` semantics for federated members.
- [x] Add state digest calculation over consensus-visible tables only.
- [x] Exclude local private config, local connection status, logs, and caches from the digest.
- [x] Add an audit summary similar to Fedimint's balance audit after each accepted operation.
- [x] Persist accepted order digests and signed catch-up certificates as lightweight replay/catch-up evidence.
- [x] Keep the materialized-state digest in convergence status, signed session
  outcomes, and catch-up verification without storing it redundantly on every
  journal row or in a write-only digest-history table.

Tests and success criteria:

- [x] Re-run migration tests proving federation migrations apply cleanly on an empty SQLite mint database while preserving standalone mint tables.
- [x] Re-run migration tests proving federation migrations apply cleanly on an empty Postgres mint database with a live Postgres instance. `cargo test -p cdk-postgres --features federation federation_migrations_apply_to_empty_postgres_and_keep_standalone_tables -- --test-threads 1` passed against the local Postgres helper on 2026-07-05.
- [x] Re-run migration tests covering feature-disabled SQLite legacy/standalone migration compatibility.
- [x] Applying the same operation log to two DBs yields identical state digests.
- [x] Local-only fields do not change state digest.
- [x] Negative balance audit causes a hard error in tests.
- [x] Signature shares can be retrieved by operation hash and output index from a dedicated federation share API/table.

## Milestone 6: deterministic mint state machine extraction

Objective: refactor CDK mint operations so consensus can call deterministic apply functions.

Implementation checklist:

- [x] Add deterministic federated mint apply path, implemented as `apply_federation_mint`.
- [x] Add deterministic federated swap apply path, implemented as `apply_federation_swap`.
- [x] Pass operation hashes/IDs, quote IDs, and payment observations explicitly into federation apply paths.
- [x] Add a consensus-provided accepted-time input if any apply path needs accepted time.
- [x] Remove calls to `unix_time()` from consensus apply paths except via consensus-provided accepted time.
- [x] Remove UUID generation from consensus apply paths.
- [x] Ensure apply paths do not spawn tasks or publish pubsub notifications until after commit.
- [x] Make `apply_federation_mint` update quote issuance, blinded message records, completed operation, and output member-share records in one DB transaction using existing blind-signature storage.
- [x] Make `apply_federation_swap` verify inputs, verify spending conditions, check balance, mark inputs spent, record blinded outputs, record completed operation, and request signature shares in one DB transaction.
- [x] Decide how to represent `Pending` in federated mode:
  - [x] Swap should not use local pending state before consensus.
  - [x] Melt reserve can use consensus-visible pending state between reserve and finalization.
- [x] Add idempotency: if the exact same operation was already accepted, return the existing outcome.
- [x] Add conflict detection: if a conflicting operation was accepted first, reject and record a conflict outcome where appropriate.

Tests and success criteria:

- [x] Re-run standalone mint tests after the federation refactor.
- [x] Applying a valid issue operation twice is idempotent.
- [x] Applying a same-quote different-output issue after the first issue fails.
- [x] Applying a swap with already spent inputs fails.
- [x] Applying a swap with duplicate outputs fails.
- [x] Applying operations in the same order on multiple DB instances produces identical materialized state.
- [x] Federation swap pubsub notifications fire after commit.
- [x] Add broad post-commit pubsub tests for standalone mode and all federation operations.

## Milestone 7: federated key-share signatory

Objective: let each member sign only with its own BLS share while advertising aggregate keys.

Implementation checklist:

- [x] Add federation member-share signing helpers on federation config/private config data.
- [x] `keysets()` must return aggregate public keys and aggregate keyset IDs.
- [x] `verify_proofs()` must verify proofs against aggregate public keys.
- [x] `blind_sign_share()` must return member share signatures plus member ID.
- [x] Add share verification against local public share before storing or returning a share.
- [x] Prevent federated share signer from being used in standalone paths that expect final signatures.
- [x] Ensure Cashu keyset v3 (`KeySetVersion::Version02`) BLS responses do not include DLEQ data.
- [x] Reject all non-v3 keysets in federation signing paths unless a future protocol explicitly supports them.
- [x] Persist signature shares in `federation_signature_share`, keyed by operation hash, output index, member ID, blinded message, amount, and keyset ID.
- [x] Persist accepted journal index and consensus evidence with signature-share records so shares can be traced to accepted ordering provenance.
- [x] Build accepted signature-share records through checked constructors that reject mismatched share-set and journal/provenance operation IDs before persistence.
- [x] Return existing share for idempotent repeat requests.

Tests and success criteria:

- [x] A member cannot return a final signature in federation mode.
- [x] A member's share verifies against its public share.
- [x] Aggregate of shares returned by three members verifies as a normal Cashu blind signature when the configured threshold is three.
- [x] Re-run regression coverage proving existing standalone `DbSignatory` behavior is unchanged.
- [x] A federated member rejects non-BLS keysets for federation signing unless explicitly supported later.
- [x] A federated member rejects v1/v2 keysets even if they are otherwise valid standalone Cashu keysets.

## Milestone 8: federated mint quote creation and payment observations

Objective: make all members agree on mint quotes and paid status.

Implementation checklist:

- [x] In federation mode, `get_mint_quote` on the contacted member creates the payment request, then submits a `MintQuote` federation operation to consensus.
- [x] `MintQuote` carries quote ID and Cashu quote request/response payloads, including method, unit, amount, expiry, request, lookup, pubkey, and extra fields carried by those types.
- [x] All members persist the same quote after consensus accepts it.
- [x] Payment observation operations are submitted and processed through consensus.
- [x] Add an always-on payment observer task per member that watches unpaid quotes and submits mint quote payment observations.
- [x] After a `MintQuote` operation is accepted, immediately check the just-created quote once and submit a `MintQuotePayment` observation if the backend already reports it paid, while keeping the periodic observer as the fallback for delayed payments.
- [x] `MintQuotePayment` carries observer, method, state, amount paid, amount issued, and optional payment proof.
- [x] Add explicit payment ID and observed-time fields if optional payment proof plus journal ordering are insufficient.
- [x] Mark quote paid only after the configured observation policy is satisfied.
- [x] For fake wallet MVP, allow all members to observe paid deterministically.
- [x] For real Lightning/onchain, enforce a configured threshold of matching,
  authenticated member observations without claiming an unimplemented backend
  trust model.
- [ ] Add method-specific shared-backend, origin-proof, threshold-gateway, or
  onchain-proof policy only together with its concrete evidence verifier,
  admission rules, and end-to-end failure tests.
- [x] Make duplicate payment observations idempotent by payment ID.
- [x] Ensure a malicious single member cannot mark a quote paid alone.

Tests and success criteria:

- [x] Creating a quote on one member makes it visible on all members after consensus.
- [x] Fake wallet payment observation marks the quote paid on all members.
- [x] Duplicate observations do not double-count `amount_paid`.
- [x] Conflicting observations for the same payment ID are rejected or quarantined.
- [x] A member that did not create the quote can still serve quote status after consensus.
- [x] Quote state digests match across all members after payment.

## Milestone 9: federated mint issuance

Objective: issue eCash from paid quotes only after the exact issuance request is accepted by consensus.

Implementation checklist:

- [x] In federation mode, `process_mint_request` converts `MintInput` into a `Mint` federation operation and submits it to consensus.
- [x] `Mint` includes quote IDs and blinded outputs through `MintRequest`/`BatchMintRequest`; quote amounts are validated from accepted quote state.
- [x] Consensus apply validates quote states, paid amounts, issued amounts, payment methods, units, v3/BLS output keysets, output uniqueness, and NUT-20 signatures.
- [x] Consensus apply records issuance and member signature shares in the same DB transaction via dedicated federation signature-share storage plus the current wallet-facing blind-signature compatibility rows.
- [x] Move member signature shares to a dedicated `federation_signature_share` table if that table is added.
- [x] The public handler waits for the operation outcome and returns this member's signature shares as `MintResponse`.
- [x] If the same operation was already accepted, return cached shares.
- [x] If a conflicting operation spent the quote first, return the existing CDK error that best maps to issued or unbalanced quote.
- [x] For batch minting, record one accepted operation and per-quote issued amounts.
- [x] Ensure returned shares are ordered exactly like request outputs.

Tests and success criteria:

- [x] Single quote paid by fake wallet mints successfully through 3-of-5 or configured threshold.
- [x] Batch mint with two paid quotes mints once and updates both quotes.
- [x] Replaying the same mint request returns the same aggregate final signature after wallet aggregation.
- [x] Same quote with different outputs cannot obtain threshold shares for both output sets.
- [x] Unpaid quote minting fails before blind-signature or federation signature-share storage.
- [x] On member crash after consensus acceptance but before HTTP response, retry returns stored shares.
- [x] On member crash before consensus acceptance, retry submits the same operation hash.
- [x] Add or re-run explicit federated NUT-20 locked quote regression tests.

## Milestone 10: federated wallet connector and aggregation

Objective: make the wallet see normal Cashu responses while the connector handles member fanout.

Implementation checklist:

- [x] Add `FederatedMintConnector` implementing `MintConnector`.
- [x] Load federation config through explicit builder/config construction.
- [x] Load federation config from an invite or from one member's public mint
  URL. Every active member exposes the finalized public `FederationConfig` at
  unauthenticated `GET /v1/config`; standalone mints do not mount the route.
  URL discovery fully validates the config, requires the supplied URL to match
  exactly one roster member, then cross-checks that member's federation ID,
  member ID, wallet protocol version, and config digest against `GET /v1/info` before
  connector construction.
- [x] Wrap one `HttpClient` per member.
- [x] `get_mint_keys`, `get_mint_keysets`, and `get_mint_info` should query multiple members and require matching federation ID and aggregate keysets.
- [x] `post_mint_quote` creates a quote through the first healthy member.
- [x] Optionally verify quorum visibility after quote creation.
- [x] `post_mint` and `post_batch_mint` should fan out the exact same request to all members concurrently.
- [x] Validate each returned share against the member public share and corresponding blinded output.
- [x] Aggregate the first threshold valid shares per output.
- [x] Return normal aggregate `MintResponse` to the wallet saga.
- [x] `post_swap` should do the same for swap signatures.
- [x] Implement first-healthy fallback and threshold fanout error collection.
- [x] Add an explicit retry and timeout policy per member.
- [x] Track per-member errors without leaking them into proof construction unless threshold cannot be reached.
- [x] Add a clear error when too few valid shares arrive.
- [x] Ensure wallet-local `validate_mint_response_signatures` validates aggregate signatures under aggregate keys after aggregation.

Tests and success criteria:

- [x] Federated connector mints proofs accepted by existing wallet proof verification.
- [x] Connector tolerates one offline member when threshold can still be met.
- [x] Connector rejects a bad share from one member and still succeeds if enough good shares remain. The collector now waits for a threshold-valid aggregate rather than stopping at the raw response count, so an early bad share cannot exclude a slightly slower honest response. The deterministic 3-of-5 mint/swap/melt regression and command-level CLI P2PK send regression both exercise this ordering.
- [x] Connector fails if responses have mismatched amounts or keyset IDs.
- [x] Connector fails if members advertise different aggregate keysets or any non-v3 federation keyset.
- [x] Connector rejects missing or mismatched federation mint-info metadata before signing fanout.
- [x] Existing wallet issue saga needs minimal changes beyond connector injection.

## Milestone 11: federated swap

Objective: prevent double-spends and mix-and-match output attacks for swaps.

Implementation checklist:

- [x] In federation mode, public `/swap` submits a canonical `Swap` operation to consensus.
- [x] `Swap` commits to all input proofs, all witnesses, all output blinded messages, and request version.
- [x] Consensus apply verifies:
  - [x] inputs are unique.
  - [x] outputs are unique.
  - [x] input keysets are known and not expired.
  - [x] output keysets are active and not expired.
  - [x] proofs verify under aggregate public keys.
  - [x] Existing spending-condition verification is called inside federated swap apply.
  - [x] Add dedicated federated canonical-transcript tests for P2PK, HTLC, and SIG_ALL.
  - [x] input value equals output value plus fees.
  - [x] no input proof Y is already spent or pending in consensus state.
- [x] Consensus apply marks inputs spent and records output signature shares in one DB transaction.
- [x] Public handler waits for acceptance and returns this member's shares.
- [x] Existing standalone saga remains available for non-federated mints.
- [x] Restore can recover accepted blind-signature rows by blinded output.
- [x] Add explicit operation-hash/output-index restore mapping if the dedicated signature-share table is added.

Tests and success criteria:

- [x] Normal wallet swap succeeds through federated connector.
- [x] Concurrent swaps spending the same input result in exactly one accepted swap.
- [x] Replay of the same swap request is idempotent.
- [x] Different output sets for the same inputs cannot both get threshold shares.
- [x] Add or re-run P2PK and HTLC tests in federated mode.
- [x] Add federated SIG_ALL tests proving the signature commits to the exact canonical operation.

## Milestone 12: explicit sliding-window attack regression suite

Objective: prove that threshold shares cannot be mixed across different output sets.

Implementation checklist:

- [x] Build a malicious test wallet helper that can send different output windows to different members.
- [x] Configure a 4-member test federation with signature threshold 3 if lower-threshold mode is supported.
- [x] Create a paid quote for three notes and attempt the ABC, BCD, CDA, DAB pattern.
- [x] Ensure each member submits its request to consensus and signs only accepted operations.
- [x] Record which operation hash won consensus.
- [x] Attempt to aggregate shares per output across all responses.
- [x] Repeat the same sliding-window attack test for swap inputs instead of mint quote issuance.
- [x] Repeat with delayed members and duplicate submissions.

Tests and success criteria:

- [x] Only one exact operation receives enough honest shares.
- [x] The fourth unpaid output cannot be aggregated into a valid proof.
- [x] Conflicting operations are rejected, not partially signed.
- [x] State digests still match after the attack attempt.
- [x] The same suite fails against an intentionally naive local-sign-before-consensus test double.

## Milestone 13: restore and check-state

Objective: support wallet recovery without reintroducing unsafe signing.

Implementation checklist:

- [x] `checkstate` can remain a read query over local consensus materialized state.
- [x] `restore` should never sign new outputs. It should return signature shares only for blinded messages already signed by accepted operations.
- [x] Restore lookup uses accepted-operation federation share storage by blinded
  message in federation mode, while standalone restore continues to use existing
  blind-signature storage.
- [x] Add `federation_signature_share` lookup by blinded message if the dedicated federation share table is added.
- [x] If a member lacks a local share for an accepted output, it may regenerate it from the accepted operation and key share, then persist it.
- [x] Federated connector calls restore on members, validates shares, aggregates threshold shares, and returns normal `RestoreResponse`.
- [x] Recovery APIs must not leak member-specific private share data.

Tests and success criteria:

- [x] Wallet can restore proofs after losing local DB using threshold member responses.
- [x] Restore of unknown blinded message returns no signature share.
- [x] Restore cannot be used to get shares for an output that never passed consensus,
  including if a plain `blind_signature` row exists without federation share
  provenance. Covered by
  `restore_ignores_orphan_blind_signature_without_federation_share`.
- [x] Checkstate returns identical states from all honest members.

## Milestone 14: melt reserve and payment finalization

Objective: federate melts without losing funds or allowing proof reuse.

Implementation checklist:

- [x] Split melt into consensus-visible stages.
- [x] `MeltQuote` persists the quote across members.
- [x] The accepted melt quote operation carries the canonical public quote
  response and required internal backend metadata. Preparation is provisional,
  and every member materializes identical terms without a local backend call.
- [x] A melt can reserve proofs only when the same quote has an earlier
  successfully applied `MeltQuote` journal entry.
- [x] Federated melt request validates inputs and change outputs and uses consensus-visible saga/pending state.
- [x] Keep the current federated melt request name for operation version 1; document `FederationOperation::Melt` as the accepted reserve/execution operation rather than a terminal finality operation.
- [x] Fake-wallet happy path covers outgoing payment completion.
- [x] Exactly one production payment actor should attempt the outgoing payment, selected deterministically from operation hash or configured as origin member.
- [x] Before attempting payment, consensus must record `PaymentAttempted` or equivalent write-ahead state.
- [x] Payment result enters consensus as `MeltQuotePayment`.
- [x] `FinalizeMelt` after threshold paid observations marks inputs spent, records quote paid, signs change outputs, records completed operation, and deletes pending melt state.
- [x] `RollbackMelt` after threshold failed observations releases pending inputs and records quote failed.
- [x] Unknown or pending backend states keep inputs pending until resolved or an admin policy handles timeout.
- [x] Existing `MeltSaga` recovery concepts should be reused, but consensus state replaces member-local saga state as the source of truth.
- [x] Internal melt-to-mint settlement must become a single consensus operation that updates both quote families atomically.
- [x] A paid federation melt is not locally complete until its deterministic
  melt completed-operation row exists. If payment finalization committed the
  paid quote and spent proofs but crashed before change signing and cleanup,
  replay resumes the existing `Finalizing` saga from its persisted settlement
  data without consulting or paying the backend again.
- [x] Failed-payment rollback atomically releases proofs, resets the quote,
  deletes melt-request state, and deletes the saga. Saga cleanup is not
  best-effort because committing without it would leave a partially rolled-back
  consensus operation.

Tests and success criteria:

- [x] Fake wallet melt succeeds and returns aggregated change signatures.
- [x] Failed fake wallet melt releases inputs across all members, and exact duplicate failed public retries return `PaymentFailed` without attempting a second outgoing payment, including after SQLite restart and in-memory catch-up.
- [x] Pending fake wallet melt keeps inputs pending and later finalizes after observation.
- [x] Crash after payment attempt but before finalize recovers by checking observations and finalizing.
- [x] A late fault after paid/spent state is durable but before change,
  completed-operation, request, and saga cleanup leaves the operation
  replayable; replay produces the change share, records completion, reaches
  `Applied`, and keeps the outgoing backend call count at one. Covered by
  `paid_federation_melt_replays_finalizing_cleanup_without_second_payment`.
- [x] A late failed-payment rollback fault commits none of proof release, quote
  reset, request deletion, or saga deletion. The accepted payment observation
  remains deduplicated, and replay completes rollback without another outgoing
  payment. Covered by
  `failed_federation_melt_rolls_back_atomically_and_replays`.
- [x] Two concurrent melts of the same proofs cannot both pay.
- [x] Internal melt-to-mint updates the mint quote and melt quote in one accepted operation.
- [x] Melt quote preparation leaves no durable proposer-local quote before
  consensus; exact replay is idempotent, conflicting local data fails closed,
  and a later quote does not authorize an older melt.

## Milestone 15: networking and peer authentication

Objective: run federation members as separate processes.

Scope decision: federation networking is signed private REST. Alternative
transports are outside this implementation plan and would require a separate
design and deployment requirement.

Implementation checklist:

- [x] Use private REST client/server and `cdk_http_client::Transport` for MVP federation traffic.
- [x] Implement an in-memory consensus/network simulator for tests.
- [x] Abstract HTTP transport for the private REST peer client.
- [x] Implement private REST transport first using Axum/Reqwest for local, regtest, and reachable-server deployments.
- [x] Keep signed member-to-member federation REST endpoints separate from
  public Cashu wallet endpoints under `/federation/v1/*`. The intentional
  exception is the read-only wallet discovery endpoint `GET /v1/config`, which
  returns only the finalized public federation config and requires no member
  authentication.
- [x] Use signed, canonical member-to-member envelopes on every REST message: federation ID, sender member ID, receiver member ID, message kind, consensus height, nonce or sequence number, body hash, and signature.
- [x] Verify every private REST envelope against the sender's identity public key from the local federation roster.
- [x] Refuse private REST requests from member IDs that are not in the configured roster.
- [x] Refuse private REST requests whose signing key does not match the configured public key for the claimed sender member ID.
- [x] Do not support dynamic peer admission in MVP; member add/remove must be an explicit later governance operation.
- [x] Make REST messages idempotent so retries after timeouts cannot duplicate consensus proposals, payment observations, or configured development-backend outgoing attempts.
- [x] Add REST endpoints for peer status, proposal submission, consensus messages, signed catch-up certificates, catch-up ranges, and state-digest comparison.
- [x] Treat private signed REST as the default federation transport. It is sufficient for all mint-to-mint consensus, catch-up, DKG/setup, status, and AlephBFT message traffic when members have mutually reachable endpoints.
- [x] Document that private REST requires reachable member endpoints or an
  external connectivity solution such as VPN, reverse tunnel, relay, or Tor.
- [x] Authenticate every peer message by member ID.
- [x] Add consensus item size/admission limits.
- [x] Add bounded outbound and inbound REST queues and backpressure.
- [x] Add a periodic peer sync loop.
- [x] Add reconnect with exponential backoff.
- [x] Add periodic config-digest, consensus-height, and state-digest gossip between peers.
- [x] Add admin peer status output.
- [x] Add peer status metrics.
- [x] Separate public Cashu API URL from private federation REST URL.
- [x] Reject messages from unknown members or wrong federation IDs.
- [x] Define maximum message sizes and backpressure behavior for consensus proposals, signature-share responses, and catch-up data.

Tests and success criteria:

- [x] Local process tooling and smoke scripts exist for multi-member private REST federation runs.
- [x] Re-run and record a five-local-process private REST consensus smoke test.
- [x] In-memory private-transport tests deterministically simulate partitions,
  dropped messages, reordered messages, and reconnects.
- [x] REST retry test proves duplicate HTTP deliveries are idempotent.
- [x] REST authentication test rejects wrong federation ID, wrong sender ID, stale sequence, invalid signature, and body-hash mismatch.
- [x] REST allowlist test rejects a well-formed message signed by a valid key that is not in the federation roster.
- [x] REST claimed-sender test rejects a message signed by member A's key while claiming member B's ID.
- [x] Restarted member reconnects and catches up.
- [x] Unknown peer messages are rejected.
- [x] Size/admission tests cover oversized consensus items.
- [x] Queue overflow drops non-critical messages without crashing consensus.
- [x] Signed-envelope identity mismatch prevents message acceptance.
- [x] Peer status shows connected, disconnected, lagging, catching-up, and halted states.

## Milestone 16: member startup, catch-up, and recovery

Objective: make nodes durable and restart-safe.

Implementation checklist:

- [x] On startup, load federation config and validate key shares before serving HTTP.
- [x] Start private REST/consensus runtime before enabling federation signing endpoints.
- [x] Replay accepted items since last checkpoint if needed.
- [x] Persist finalized consensus evidence and the immutable operation/order
  commitment; recompute the materialized-state digest from durable mint rows.
- [x] Persist signed catch-up certificates so lagging members can verify catch-up data without trusting one peer.
- [x] On reconnect, request missing signed catch-up certificates and accepted items from peers starting at the last durable local height.
- [x] Verify quorum or threshold signatures on catch-up data before replaying it locally.
- [x] Replay accepted operations deterministically and recompute state digests after every session boundary.
- [x] Regenerate deterministic signature shares from accepted operations when local share rows are missing.
- [x] Compare local state digest to quorum state digest at session boundaries.
- [x] Expose admin endpoint or CLI to show consensus height, last digest, member lag, and pending operations.
- [x] Keep catch-up replay bounded by accepted journal/session evidence; a
  separate materialized-state checkpoint retention setting was removed after
  confirming it had no production reader.
- [x] Add a safe read-only "catch-up" mode for lagging members.
- [x] Prevent a lagging member from serving signing shares until it catches up to the operation's acceptance height.
- [x] Prevent a lagging member from initiating configured development-backend outgoing payment attempts until it has replayed all accepted melt state.
- [x] Add idempotency keys for fake wallet outgoing payments so recovery cannot pay twice.
- [x] Add a manual halt and inspect mode for state digest mismatch.

Tests and success criteria:

- [x] Crash before applying an item replays cleanly.
- [x] Crash after applying but before HTTP response returns cached shares on retry.
- [x] Crash after signing share but before storing share regenerates the same share.
- [x] Lagging member refuses to sign until caught up.
- [x] Member offline for 10 accepted sessions catches up and reaches the same state digest.
- [x] Member offline during mint quote payment observation catches up and can later return the correct deterministic share.
- [x] Member offline during swap cannot later accept a conflicting spend and catches up to spent state.
- [x] Member offline during melt payment observation catches up without attempting a duplicate configured-backend payment.
- [x] Network partition heals and all non-faulty members converge or halt on digest mismatch.
- [x] State digest mismatch is detected and halts the member in tests.

## Milestone 16A: state convergence and catch-up certification

Objective: prove that any honest federation member that was offline, restarted, or lagging can replay consensus history and converge to the same consensus-visible mint state as the rest of the federation, while refusing unsafe service until catch-up is certified.

Implementation checklist:

- [x] Define the exact consensus-visible database surface: tables and columns included in federation state equality.
- [x] Define the exact local-only database surface: private config, peer connection state, sequence trackers, logs, caches, local metrics, process metadata, and other fields excluded from equality.
- [x] Implement a canonical state snapshot/export over consensus-visible rows.
- [x] Implement a state digest over the canonical snapshot.
- [x] Include state digests in signed catch-up certificates and admin status reporting.
- [x] Include materialized-state digests in signed status/session evidence and
  compare them after catch-up; the write-only digest-history table and retention
  policy were removed.
- [x] Store finalized session/item evidence and the immutable operation/order
  commitment; compute the distinct materialized-state digest from canonical rows.
- [x] Implement an admin command to compare state digests across members.
- [x] Implement an optional admin command to diff consensus-visible rows when digests differ.
- [x] Require catch-up data to include quorum-signed catch-up certificates or equivalent authenticated evidence.
- [x] Reject catch-up ranges with missing, duplicated, reordered, tampered, or conflicting entries.
- [x] Replay accepted operations deterministically from the last durable local height.
- [x] Recompute state digest after replay and compare it to quorum-reported digest.
- [x] Put lagging members into safe catch-up mode.
- [x] In catch-up mode, reject signing shares, melt payment attempts, and authoritative wallet reads.
- [x] Exit catch-up mode only after local height and state digest match quorum evidence.
- [x] Halt or quarantine the member if local replay produces a digest mismatch.
- [x] Regenerate deterministic signature shares from accepted operations if local share rows are missing.
- [x] Ensure configured-backend outgoing payment idempotency prevents duplicate payments after catch-up or restart.

Tests and success criteria:

- [x] Member offline for 10 accepted operations catches up and reaches the same state digest.
- [x] Long-partition tooling exists for extended offline/catch-up scenarios.
  The generated process smoke now drives repeated standard and offline
  wallet-operation batches through
  `cdk-mintd federation smoke-operations --operation-batches`, keeping seed
  progression and validation inside the command while the script controls the
  all-members and offline-member windows. It also restarts a configurable
  member immediately after the repeated all-member operation batches return and
  before the explicit convergence wait, then requires the restarted member to
  recover status, convergence, quorum-backed catch-up certificates, and a
  post-restart all-member wallet-operation batch. Post-crash batches now use a
  generated seed-offset range that defaults after the initial standard range and
  is validated to avoid deterministic smoke-wallet seed reuse. The AlephBFT
  auto-rotation variant now defaults to the `aleph_bft` backend, runs an
  isolated warm-up batch before waiting for the target session, aligns generated
  member operation-submission timeouts with the smoke operation timeout, and
  exposes the path through `just fed-regtest-aleph-rotation`.
- Latest AlephBFT multi-rotation process-smoke checkpoint: generated smoke
  can now pass an explicit AlephBFT live-session rotation limit to each member
  so `ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION=2` exercises two capped natural
  `run_session` completions before restoring the production round cap. The
  startup path still strips capped test settings for recovered nonzero sessions
  and backup-backed restarts, while in-process automatic rotation preserves the
  remaining cap. The `fed-regtest-long-partition`,
  `fed-regtest-aleph-rotation`, and Postgres long-partition `just` targets now
  wire their `offline_batches` argument to the generated script's
  `LONG_PARTITION_OPERATION_BATCHES` variable instead of an unused legacy name.
  A five-member threshold-three AlephBFT process smoke with target session 2
  and six long-partition wallet-operation batches passed through post-rotation
  mint/swap/melt/restore batches, crash-window restart, offline pending paid
  and failed melts, quarantine conflict replay, keyset rotation, restarted
  member catch-up, and final convergence.
- Latest DKG route-probe smoke-target checkpoint: `just fed-regtest-dkg-full`
  and `just fed-regtest-postgres-dkg-full` now generate DKG setup-route probe
  fixtures, set `PROBE_DKG_SETUP_ROUTES=1`, and then continue through the
  normal local federation smoke path instead of exiting after setup-only
  activation. The fast `just fed-regtest-check` gate now also runs the
  `federation_dkg_status` tests so database-backed DKG progress reporting and
  DB-plus-file completion-record recovery stay covered without starting member
  processes. Dry-run checks prove both new full smoke targets wire the DKG
  probes into the generated script with the expected SQLite and Postgres
  command paths. Live five-member threshold-three AlephBFT full-process DKG
  route-probe smokes now pass on both SQLite and Postgres with standard
  mint/swap/melt/restore, member restart, one-member-offline wallet batches,
  paid and failed pending melts, payment-observation quarantine, offline keyset
  rotation, old-proof restore, post-rotation mint, melt, final restore, and
  returned-member catch-up.
- [x] Add AlephBFT live private-REST catch-up certification tests for
  self-partitioned proposers, below-quorum-offline operation windows, and
  above-quorum-offline stall windows. These tests must verify reconnect
  catch-up through finalized-item cursors and signed catch-up certificates,
  state-digest equality across all honest members, lagging-member fail-closed
  behavior before certification, no duplicate signing, no duplicate outgoing
  payment attempts, and deterministic handling of any queued operations that
  were accepted locally while connectivity was unavailable.
  The exact self-partitioned mint case, full one-member-offline value-operation
  window, basic two-member-offline stall/recovery window, and above-quorum
  queued swap-conflict plus melt window are now covered by the five-member live
  matrix. Coverage includes autonomous pending finality, fail-closed
  pre-certification reads, finalized-item replay, signed catch-up certificate
  certification, cursor/journal/state-digest convergence, mint/swap/melt share
  provenance, payment-observation replay, deterministic conflict rejection and
  duplicate retry, one elected outgoing payment attempt, and zero post-finality
  activity refresh. Restart-before-heal now preserves exactly one durable
  pending operation on the self-partitioned proposer, while the below-quorum
  non-proposer and post-finality lagger also rebuild runtime state before
  convergence. The distinct unit/apply/response crash-window item above remains
  open.
- [x] Add or re-run an explicit 100+ accepted-operation offline catch-up test.
- [x] Member offline across minting catches up and can return the correct deterministic mint share.
- [x] Member offline across swapping catches up to spent input state and rejects conflicting swaps.
- [x] Catch-up replay continues past a deterministic rejected conflicting swap
  and still applies later independent swaps. Covered by
  `catch_up_marks_conflicting_swap_rejected_and_continues_replay`.
- [x] Catch-up replay materializes payment-observation quarantine rows, and
  signed local status reports the replayed quarantine. Covered by
  `catch_up_replay_materializes_payment_observation_quarantine_rows`.
- [x] Catch-up replay materializes unresolved payment-observation rows, keeps
  the quote unpaid, and signed local status reports the replayed observation as
  awaiting quorum. Covered by
  `catch_up_replay_materializes_unresolved_payment_observation_rows`.
- [x] Signed local status row-fingerprint output includes unresolved
  payment-observation rows for operator diffing without leaking raw payment
  proof material. Covered by
  `private_local_status_route_includes_unresolved_payment_observation_fingerprints`.
- [x] Operator digest comparison diagnoses missing payment-observation rows at
  equal accepted height as redacted materialized row diffs, including the
  missing member without leaking quote IDs, operation IDs, or payment proof
  material. Covered by
  `digest_comparison_diagnoses_missing_payment_observation_row_without_leaking_payment_material`.
- [x] Operator digest comparison diagnoses divergent payment-observation rows at
  equal accepted height as redacted materialized row variants with stable member
  ordering and without leaking quote IDs, operation IDs, or payment proof
  material. Covered by
  `digest_comparison_diagnoses_payment_observation_row_variant_without_leaking_payment_material`.
- [x] Materialized state snapshots include redacted completed-operation rows for
  accepted mint, swap, and terminal paid melt outcomes, with deterministic
  federated melt saga IDs so the terminal completed-operation key converges
  across members. Covered by
  `materialized_state_digest_tracks_completed_operation_rows_for_value_operations`
  and `accepted_federation_melt_persists_reserve_before_payment_attempt`.
- [x] Catch-up replay materializes completed-operation rows after replaying
  accepted mint issuance history. Covered by
  `catch_up_replay_materializes_completed_operation_rows`.
- [x] Signed local status exposes redacted completed-operation materialized row
  fingerprints after accepted mint issuance without leaking operation IDs,
  completed-operation row UUIDs, or blinded outputs. Covered by
  `private_local_status_route_includes_completed_operation_fingerprints`.
- [x] Operator digest comparison diagnoses missing completed-operation rows at
  equal accepted height as redacted materialized row diffs, including the
  missing member without leaking row keys or operation IDs. Covered by
  `digest_comparison_diagnoses_missing_completed_operation_row`.
- [x] Operator digest comparison diagnoses divergent completed-operation rows at
  equal accepted height as redacted materialized row variants with stable member
  ordering, without leaking row keys, operation IDs, or response material.
  Covered by
  `digest_comparison_diagnoses_completed_operation_row_variant_without_leaking_response_material`.
- [x] AlephBFT finalized-unit apply and catch-up preserve terminal rejected
  entries when the latest consensus item is rejected. Covered by
  `federation_aleph_bft_finalized_unit_applies_rejected_trailing_entry` and
  `aleph_bft_catch_up_applies_rejected_trailing_entry`.
- [x] Member offline across melting catches up without attempting a duplicate outgoing payment.
- [x] Member offline across pending-to-paid melt finalization catches up to paid state.
- [x] Member offline across pending-to-failed melt finalization catches up to rolled-back state.
- [x] Member offline across payment-observation conflict or quarantine catches up to the quarantined state.
- [x] Restart after journal append but before apply replays cleanly.
- [x] Restart after apply but before HTTP response returns cached or regenerated outcome.
- [x] Restart after signing share but before storing share regenerates the same share.
- [x] Lagging member refuses mint, swap, melt, restore, and check-state authority until caught up.
- [x] Catch-up rejects tampered operation IDs.
- [x] Catch-up rejects reordered ranges.
- [x] Catch-up rejects missing entries.
- [x] Catch-up rejects duplicate entries.
- [x] Catch-up rejects ranges signed by insufficient quorum.
- [x] Catch-up rejects ranges signed by unknown members.
- [x] Catch-up halts on state digest mismatch.
- [x] SQLite and Postgres produce the same state digest from the same accepted operation log.
- [x] Optional row-level diff shows the exact consensus-visible row mismatch when digest comparison fails.
- [x] Generated regtest stops one member, runs many mint, swap, melt, restore, check-state, payment-observation, and quarantine operations, restarts the member, waits for catch-up, and verifies height plus state digest equality across all members.

## Milestone 17: `cdk-mintd` integration and regtest tooling

Objective: make the federation easy to run locally.

Implementation checklist:

- [x] Add `federation` section to mintd config.
- [x] Add member-specific config file support.
- [x] Add `just fed-regtest` or similar target that starts N local federation mints with fakewallet configured and a test wallet.
- [x] Add health commands: member status, consensus height, quote status across members, state digest comparison.
- [x] Add log labels for federation ID, member ID, operation hash, and session.
- [x] Add a fixture generator for 4-member and 5-member federations.
- [x] Ensure no generated config intended for examples contains private production material.

Local multi-mint federation harness:

- [x] Generate per-member local configs containing member ID, public wallet API URL, private federation REST URL, DB path, configured backend settings, federation public config path, and local private member config path.
- [x] Assign deterministic localhost ports for every member, with separate public wallet API ports and private federation REST ports.
- [x] Create isolated per-member data directories so databases, logs, and configs cannot overlap.
- [x] Add `just fed-regtest` to launch 4-member and 5-member federations with fakewallet configured as the backend as separate `cdk-mintd` processes.
- [x] Add `just fed-regtest-status` to show public URL, private federation URL, process status, consensus height, last state digest, lag, and member health.
- [x] Add `just fed-regtest-stop-member MEMBER_ID` and `just fed-regtest-start-member MEMBER_ID` to test threshold liveness, reconnect, and catch-up.
- [x] Add `just fed-regtest-clean` to remove generated local configs, DBs, logs, and process state.
- [x] Add a local wallet federation config, invite file, or test connector config that points to the federation roster instead of a single mint URL.
- [x] Make local logs identify federation ID, member ID, public port, private federation port, consensus height, operation hash, and DB path.
- [x] Document how to run the local federation manually without `just`, including the exact `cdk-mintd` command shape for one member.
- [x] Keep generated local private configs out of examples and Git; commit only templates, deterministic non-secret fixtures, and safe documentation.

Tests and success criteria:

- [x] `just fed-regtest` target and generated smoke scripts exist for development-backend federation runs.
- [x] Generated local smoke scripts have explicit 4-member/threshold-3 matrix
  coverage and exercise member-generic convergence, offline operation, pending
  paid/failed melt, quarantine, key-rotation, Prometheus, and catch-up certificate
  gates. Covered by `trusted_dealer_smoke_script_supports_four_member_bft_matrix`;
  repeated standard/long-partition operation batches are now generated-script knobs
  backed by the validated smoke command option and covered by the
  `federation_smoke` and `federation_setup` test filters. The generated script
  also has a configurable process crash-window restart after repeated
  all-member batches and before convergence, then runs fresh all-member
  operations through the recovered member using a collision-free generated
  seed-offset range. The later long-partition operation window now has explicit
  generated member, seed-offset, and batch controls, and that collision-free
  seed range is validated against the standard and post-crash ranges before any
  member processes are started. The generated script also exposes
  `ALEPH_BFT_AUTO_ROTATION_MAX_ROUND` as an opt-in live-session round cap and
  waits for every member to report the configured target session before running
  wallet operation batches. Covered by `federation_setup` artifact assertions.
- [x] Re-run `just fed-regtest` and record that it starts a 5-member federation with fakewallet configured as the backend.
- [x] Connector and local smoke coverage exist for mint, swap, restore, and check-state flows.
- [x] Re-run full regtest proving a CDK wallet can mint, swap, restore, and check state against the federation.
- [x] Code and tests exist for development-backend federated melt and aggregated change signatures.
- [x] Re-run full regtest proving a CDK wallet can melt through the development-backend federation and receive aggregated change signatures when change is expected.
- [x] Stop/start tooling and catch-up tests exist for threshold liveness and member recovery.
- [x] Re-run regtest proving stopping one member still allows operations if threshold and consensus quorum allow.
- [x] Re-run regtest proving restarting the stopped member reconnects, catches up from peers, and reaches the same state digest.
- [x] State digest comparison command exists.
- [x] Re-run regtest proving state digest comparison reports all members equal after each test.
- [x] Local private REST peer authentication rejects unknown members, wrong federation IDs, wrong claimed member IDs, and invalid signatures on localhost.
- [x] Local status commands clearly show connected, offline, lagging, catching-up, and healthy members.
- [x] `just fed-regtest-clean` target exists.
- [x] Re-run `just fed-regtest-clean` and verify it leaves no running member processes or generated local state.

## Milestone 18: storage backend coverage

Objective: support the same storage expectations as CDK does today.

Implementation checklist:

- [x] Implement federation DB traits for SQLite first.
- [x] Implement Postgres after SQLite semantics are stable.
- [x] Decide whether Redb is wallet-only and therefore out of scope for mint federation.
- [x] Keep migrations additive and compatible with standalone mode.
- [x] Conflict keys and concurrent request stress tests exist.
- [x] Document SQL lock ordering rules for all conflict keys to avoid deadlocks.
- [x] Add federation journal/payment/session indexes for operation/session/payment hot paths.
- [x] Review and add exact indexes for quote ID, proof Y, blinded message, and dedicated signature-share rows after the final federation schema settles.

Tests and success criteria:

- [x] Re-run SQLite unit and integration tests.
- [x] Re-run Postgres integration tests with a live Postgres instance. `cargo test -p cdk-postgres --features federation -- --test-threads 1` passed 138 tests against the local Postgres helper on 2026-07-05.
- [x] Concurrent request stress tests do not deadlock.
- [x] Verify query plans for hot paths use indexes.

## Milestone 19: real payment backend policy

Objective: move beyond fake wallet without centralizing payment trust accidentally.

Implementation checklist:

- [x] Write a payment trust model document before enabling real funds.
- [x] Keep the implemented public policy limited to the required number of
  authenticated, matching observations; do not serialize unenforced backend
  trust-model labels.
- [ ] Before real funds, implement and test the concrete Lightning or onchain
  evidence verifier selected by operators.
- [x] Define observation quorum and conflict handling for each method.
- [x] Keep unenforced pending-quote limits out of consensus config; classify
  unresolved observations for operator status with one local 15-minute
  default.
- [x] Add admin tooling to inspect unresolved payment observations.

Tests and success criteria:

- [x] Fake wallet policy and real backend policy are separate code paths.
- [x] A single malicious observation cannot mark a quote paid.
- [x] Conflicting payment amounts do not silently update quote state.
- [x] Stale pending quotes are visible to operators and do not block consensus.

## Milestone 20: distributed key generation

Objective: replace trusted-dealer setup with dealer-free DKG.

Implementation checklist:

- [x] Add DKG setup phases, commitments/reveals, secret-share contributions, transcript signatures, and local member keyset-share derivation.
- [x] Complete a production-ready dealer-free Pedersen DKG replacement for BLS G2, using Fedimint's DKG as a reference.
  - [x] Add an explicit DKG keyset specification binding unit, activation, input fee, expiry, and denomination set into every participant commitment/reveal and the final derived-key transcript.
  - [x] Add a versioned, restartable private participant state that generates only the local member's random polynomials, emits per-recipient contributions, drops and zeroizes the original coefficients, validates its own reload, and redacts secret material from `Debug`.
  - [x] Simulate five independent members at threshold three and prove every receiver derives the same v3 aggregate keyset after private-state serialization/reload.
  - [x] Add a unique 32-byte ceremony ID to every participant commitment, reveal, private contribution, restartable participant state, and setup-state phase gate. The ID is inside the pre-reveal commitment hash, cross-ceremony records fail before setup mutation or authenticated sender-sequence advancement, and the undeployed v1 experimental record shape is intentionally unsupported.
  - [x] Bind the ceremony ID and complete participant commitment/reveal record into the final identity-signed transcript. Each durable local member-keyset-share record carries the public ceremony that produced it; final-config assembly and startup signature admission rederive the aggregate keyset from that record before accepting the transcript. The transcript hash moved to v2 without an undeployed-federation compatibility path.
  - [x] Add the live per-member ceremony driver with durable local state, phase gates, authenticated REST fanout, bounded retries, restart/resume, and operator-visible progress.
    - [x] Persist the versioned local participant state by ceremony ID in both SQL backends. Exact replay is idempotent, regeneration under the same ceremony is rejected, multiple ceremony IDs can coexist, and status exposes only record counts, member IDs, and public ceremony IDs.
    - [x] Add a pure phase planner over the local participant state plus durable commitments, reveals, incoming contributions, keyset shares, and transcript signatures. It emits commitment, reveal, per-recipient shares, local keyset persistence, deterministic transcript signature, or complete only when the preceding all-member gate is durable. Later phases retain earlier outbound artifacts so lagging peers can recover without a separate acknowledgement protocol.
    - [x] Execute planned phases over authenticated private REST with bounded retries, self-delivery, restart/resume, and explicit terminal/conflict handling. Phase batches are ordered, all configured receivers are attempted concurrently within a phase, every send uses the existing signed roster-authenticated route, and a completed member keeps a cumulative rebroadcast plan until one failure-free terminal round prevents asymmetric final-signature loss from stranding a peer.
    - [x] Add operator configuration and process-level status for starting and monitoring one live ceremony. Mintd accepts one shared ceremony ID, exact keyset policy, bounded tick/retry controls, and an explicit loopback-development switch; database-backed status applies the same pure planner to durable records and reports only the redacted current phase or redacted validation failure. Once complete, an explicit status option exports the independently assembled final public client config and local private member config with overwrite protection and `0600` private-file permissions, matching the setup-to-runtime handoff used by Fedimint without coupling wallets to mint storage.
  - [x] Require confidential transport for private secret-share contributions in production (HTTPS/mTLS or recipient-encrypted envelopes), while retaining explicit localhost development support. The default policy validates every member endpoint as HTTPS before participant state is created or any ceremony message is sent. Plain HTTP is accepted only under the explicit development switch and only when every endpoint is `localhost`, IPv4 loopback, or IPv6 loopback; the switch cannot authorize a remote HTTP host.
  - [x] Replace centralized identity bootstrap with independent operator enrollment. `dkg-member` creates exactly one local secp256k1 identity and emits a public enrollment plus a restrictive `0600` unbound private config; `dkg-roster` consumes only public enrollments, derives the roster/federation ID, commits a production payment-observation policy, and emits the setup-only public config plus a fresh ceremony TOML; `dkg-bind` verifies and binds each local identity to that roster without ever exposing it to the roster coordinator. Duplicate members, mismatched identities, embedded URL credentials, remote plaintext peer APIs, invalid thresholds, duplicate denominations, and non-setup configs fail before output.
- [x] Run DKG over authenticated private REST setup routes, accepting DKG messages only from identity public keys in the federation roster.
- [x] Include federation ID, unique ceremony ID, member roster, member identity public keys, participant order, commitments/reveals, keyset policy, BLS public shares, and aggregate public keys in the final DKG transcript. Pedersen transcripts require exactly one unique, complete public ceremony per keyset in canonical keyset order; trusted-dealer transcripts require none.
- [x] Require each participant to sign the final DKG transcript with its federation identity key. Private REST transcript-signature acceptance now also requires the receiver to have a locally derived DKG keyset-share record, rejects transcript hashes that do not match that local final public config before advancing the sender sequence, and rejects alternate valid randomized signatures for an already accepted signer/transcript pair before private REST sequence persistence.
- [x] Commit hash of polynomial commitments before revealing commitments.
- [x] Verify secret shares against public commitments.
- [x] Derive public shares and aggregate public keys deterministically.
- [x] Persist DKG transcript for audit and recovery.
- [x] Persist the final roster hash and DKG transcript hash in each member config.
- [x] Database-backed DKG status merges persisted completion records with file-supplied recovery records idempotently, so reusing the same keyset-share or transcript-signature record during migration does not inflate readiness counts.
- [x] Support key rotation through consensus as a federation operation.
- [x] Make trusted-dealer mode test-only once production DKG is complete. The generator, CLI argument types, and implementation compile only for tests or the explicit `federation-test-utils` feature; local recipes use the renamed `test-trusted-dealer` command. Production startup rejects a final config whose signed setup transcript declares `trusted_dealer`, while the explicit test-utils build retains local/regtest admission.

Tests and success criteria:

- [x] DKG produces keys equivalent in behavior to trusted-dealer keygen.
- [x] DKG rejects a participant whose identity public key is not in the configured roster.
- [x] DKG rejects a transcript where a BLS public share is reassigned to another member ID.
- [x] DKG transcript signatures verify for every member identity public key.
- [x] Private REST DKG setup conflicts for commitments, reveals, secret shares, and transcript signatures fail before advancing the sender sequence or replacing the accepted setup record.
- [x] Partial database-backed DKG setup status reports visible commitment, reveal, and secret-share progress without marking finalization ready or leaking secret material.
- [x] Invalid share is detected and setup fails.
- [x] Reloaded private participant state rejects a contribution that no longer matches its public polynomial commitments and does not expose secret scalar bytes through `Debug`.
- [x] Participants that commit to different unit/fee/expiry/activation/denomination policies cannot complete one DKG round.
- [x] Commitments, reveals, and private contributions from another ceremony fail before setup-state mutation; the authenticated private contribution route also proves rejection does not advance the sender sequence or persist secret material.
- [x] A same-roster ceremony relabeled with a fresh ID but deriving identical aggregate keys produces a different transcript hash, and signatures from the original ceremony do not authenticate it; direct reveal tampering also invalidates the signed payload.
- [x] Restart activation and database-backed status reconstruction rederive each keyset from its persisted public ceremony before accepting transcript signatures, with no new table or consensus phase.
- [x] Local participant state survives SQL restart without exposing scalar material through status; exact replay succeeds, conflicting same-ceremony regeneration fails, and a later ceremony persists independently.
- [x] The pure driver planner advances through commitment, reveal, secret-share, local-keyset, transcript-signature, and complete phases only from a valid durable view; out-of-order and cross-ceremony records fail closed, repeated signature planning is deterministic, and planner `Debug` output excludes private shares.
- [x] Three independent members complete the ceremony through authenticated private REST, persist one valid local share each, and independently assemble an identical final public config. A partitioned sender attempts every configured receiver, remains incomplete, reloads its exact private participant state from SQL after restart, and converges after the links heal.
- [x] Restart during setup can resume or safely restart.
- [x] Key rotation creates a new aggregate keyset accepted by all members.
- [x] Old keysets remain verifiable until final expiry.

Verified production-bootstrap closure:

- `cargo check --release -p cdk-mintd --features federation`: passed with the
  trusted-dealer implementation excluded from the production build.
- Five focused production setup tests pass for independent three-member
  enrollment/roster/binding, restrictive private permissions, duplicate-member
  rejection, wrong-identity rejection, remote plaintext-peer rejection, and
  colliding-output refusal before any file is written.
- All 15 production federation CLI parser tests pass, including the three DKG
  setup commands and proof that `test-trusted-dealer` is not exposed.
- Production startup rejects a trusted-dealer transcript; the explicit
  `federation-test-utils` startup and renamed CLI command accept it for regtest.
  Both trusted-dealer core generator regressions still pass behind that feature.
- All 35 focused `cdk-common` dealer-free DKG tests pass unchanged, covering
  derivation, reload, transcript, transport, size bounds, invalid shares, and
  ceremony/policy binding.

Verified dealer-free participant checkpoint:

- `cargo test -p cdk-common --features federation -- --test-threads=1`: 366 passed, plus 3 ignored documentation examples.
- `cargo test -p cdk-axum --features federation dkg -- --test-threads=1`: 17 passed.
- `cargo test -p cdk-mintd --features federation,fakewallet dkg -- --test-threads=1`: 10 passed.
- `cargo test -p cdk-sqlite --features federation dkg -- --test-threads=1`: 4 passed.
- `cargo check -p cdk-axum --features federation` and `cargo check -p cdk-mintd --features federation,fakewallet`: passed.
- `cargo check -p cashu -p cdk-common` and the standalone `test_generate_bls_is_canonical_and_non_zero` regression: passed without federation features.
- `cargo clippy -p cdk-common --features federation --lib --no-deps -- -D warnings` remains blocked by 19 pre-existing federation lint findings; the DKG panic surfaced in this checkpoint was removed, and no new participant/spec lint remains.

Verified ceremony-domain checkpoint:

- `cargo test -p cdk-common --features federation dkg -- --test-threads=1`: 25 focused DKG tests passed.
- `cargo test -p cdk-axum --features federation dkg -- --test-threads=1`: 18 focused DKG tests passed, including cross-ceremony rejection before sender-sequence persistence.
- `cargo test -p cdk-mintd --features federation,fakewallet dkg -- --test-threads=1`: 10 focused DKG tests passed.
- `cargo test -p cdk-sqlite --features federation dkg -- --test-threads=1`: 4 focused DKG persistence tests passed.

Verified public-ceremony transcript checkpoint:

- `cargo test -p cdk-common --features federation dkg -- --test-threads=1`: 26 focused DKG tests passed, including ceremony substitution, missing-ceremony, reveal-tamper, and stale-signature rejection.
- `cargo test -p cdk-common --features federation trusted_dealer -- --test-threads=1`: 11 trusted-dealer transcript regressions passed with an explicitly empty ceremony list.
- `cargo test -p cdk-axum --features federation dkg -- --test-threads=1`: 18 DKG route, transcript-signature, and restart-activation tests passed.
- `cargo test -p cdk-mintd --features federation,fakewallet dkg -- --test-threads=1`: 10 DKG setup-probe and status-reconstruction tests passed.
- `cargo test -p cdk-sqlite --features federation dkg -- --test-threads=1`: 4 DKG persistence tests passed; `cargo test -p cdk-postgres --features federation dkg --no-run` also compiled the same JSON-backed record shape.
- `cargo test -p cdk --features federation dkg_completion_records -- --test-threads=1`: 3 startup completion/incomplete/mismatched-member tests passed, and the focused active-aggregate-keyset test passed separately.
- `cargo check -p cashu -p cdk-common`: passed without federation features.
- `cargo clippy -p cdk-common --features federation --lib --no-deps -- -D warnings` remains blocked by the same 19 pre-existing federation findings and reports no new public-ceremony finding.

Verified durable driver-state checkpoint:

- `cargo test -p cdk-sqlite --features federation dkg -- --test-threads=1`: all 5 DKG persistence tests passed, including exact participant-state replay, same-ceremony conflict rejection, and multi-ceremony reload.
- The focused SQLite empty-database migration and federation-index inventory tests passed with `federation_dkg_participant_states` and its member index present.
- `cargo test -p cdk-sql-common --features federation federation_mint_migrations_cover_sqlite_and_postgres`: passed for both migration definitions.
- `cargo test -p cdk-mintd --features federation,fakewallet federation_dkg_status -- --test-threads=1`: all 9 status/reconstruction tests passed; output reports only participant-state counts, member IDs, and ceremony IDs.

Verified resumable phase-planner checkpoint:

- `cargo test -p cdk-common --features federation dkg_driver -- --test-threads=1`: all 3 planner progression, fail-closed ordering/ceremony, and redacted-debug tests passed.
- `cargo test -p cdk-common --features federation dkg -- --test-threads=1`: all 29 focused DKG tests passed with the planner included.
- `cargo check -p cashu -p cdk-common`: passed without federation features.
- `cargo clippy -p cdk-common --features federation --lib --no-deps -- -D warnings` remains blocked by the same 19 pre-existing federation findings and reports no planner finding.

Verified authenticated REST driver checkpoint:

- `cargo test -p cdk-axum --features federation dkg_driver -- --nocapture`: all 5 bounded-config, participant-policy, authenticated three-member completion, all-peer fanout, partition, restart, and convergence tests passed.
- `cargo test -p cdk-common --features federation dkg_driver`: all 3 pure planner tests passed after extending the complete phase with cumulative idempotent rebroadcast artifacts.
- `cargo test -p cdk-axum --features federation dkg -- --test-threads=1`: all 23 driver, signed-route, transcript-signature, restart-activation, peer-client, and HTTP DKG tests passed.
- `cargo test -p cdk-common --features federation dkg -- --test-threads=1`: all 29 DKG cryptography, planner, transcript, ceremony-domain, and signed-transport tests passed.
- `cargo check -p cdk-axum --features federation`: passed before the focused route tests.
- `cargo check -p cdk-axum`: passed without federation features.
- The driver returns only phase/progress counters and member IDs, never secret-share values; transport failures remain retryable and no acknowledgement table, consensus item, or extra DKG phase was added.
- `cargo clippy -p cdk-axum --features federation --lib --no-deps -- -D warnings` reaches the new driver without a new finding and remains blocked by 15 pre-existing Axum findings elsewhere.

Verified secure operator auto-start checkpoint:

- Mintd now maps `[federation.dkg]` into one bounded Axum auto-start configuration. It requires federation mode, a valid shared ceremony ID, an exact valid keyset specification, nonzero tick and retry bounds, unique denominations, and the confidential-transport policy before spawning the driver.
- Router startup validates the setup-only public config before spawning. A restart that has already activated finalized key material skips auto-start cleanly, so ceremony completion remains a durable setup transition followed by explicit process restart rather than an in-place live signing-key mutation.
- `cdk-mintd federation dkg-status --from-database` reconstructs the planner phase from durable SQL records and reports `commitment`, `reveal`, `share_delivery`, `local_keyset_share`, `transcript_signature`, or `complete` without exposing private contributions.
- `cdk-mintd federation dkg-status --final-config-dir <dir>` writes the transcript-verified public and local-private configs only after complete assembly. It refuses partial records and existing targets by default, requires explicit `--force` to replace them, and protects the private file with mode `0600` on Unix.
- Local federation generation now emits a separate `dkg-live-config.toml` and durable `dkg-live-work` path per member. These configs share one generated ceremony ID and exact active keyset policy, reference only setup-only public/private identity material, enable plaintext transport only for explicit loopback hosts, and remain separate from both trusted-dealer runtime configs and manually injected DKG route-probe configs.
- `cargo test -p cdk-axum --features federation dkg -- --test-threads=1`: all 25 DKG driver, transport-policy, signed-route, partition/restart, transcript, and activation tests passed.
- `cargo test -p cdk-mintd --features federation,fakewallet,sqlite dkg -- --test-threads=1`: all 13 DKG configuration, status, and setup-generation tests passed.
- `cargo test -p cdk-mintd --features federation,fakewallet,sqlite --lib -- --test-threads=1`: all 189 mintd library tests passed.
- The focused `federation_dkg_status` gate now passes 11 tests, including complete export, incomplete-record rejection, overwrite protection, private-file permissions, database reconstruction, and redacted status output.
- The focused `federation_setup` gate passes all 14 tests with SQLite and Postgres live-DKG config parsing, one shared valid ceremony ID, exact denomination/timing policy, distinct member storage paths, and unchanged route-probe defaults.
- `cargo check -p cdk-axum` and `cargo check -p cdk-mintd --features fakewallet,sqlite`: passed without federation features.
- `cargo clippy -p cdk-axum --features federation --lib --no-deps -- -D warnings` and the equivalent mintd command reach this checkpoint without a new finding; they remain blocked by the existing 15 Axum and 9 mintd findings in unrelated older modules.

Verified DKG hot-path checkpoint:

- A fresh five-member process attempt exposed sustained full-core CPU use during
  ceremony completion. Process sampling identified two repeated cryptographic
  hot paths rather than AlephBFT or SQL locking: status/planner calls repeatedly
  reevaluated every member polynomial for every denomination, and each
  transcript-signature request independently rebuilt the same aggregate public
  keyset.
- The driver now derives and fully verifies a local keyset share once at the
  durable `LocalKeysetShare` transition, caches pure plans while the durable
  record view is unchanged, and moves the expensive derivation/planner work to
  blocking workers so it cannot starve private REST. The persisted locally
  derived share is the hot-path cache boundary for transcript hashing. Final
  config export and startup activation still independently rederive all public
  shares and reject any mismatch.
- `cdk-mintd federation dkg-status --from-database --progress-only` reports
  explicitly unchecked, redacted durable record counts without invoking the
  planner or finalization verifier. Process polling can wait for one local
  keyset share and all transcript-signature records, then perform the full
  cryptographic check exactly once during export.
- The sampled status command dropped from sustained full-core work under
  contention to 0.80 user CPU seconds in the focused progress-only measurement.
  The complete Axum DKG test group dropped from 128.63 seconds to 55.60 seconds
  while retaining all full finalization checks.
- `cargo test -p cdk-common --features federation dkg_driver -- --test-threads=1`:
  all 3 planner tests passed, including equality between the cached transcript
  hash and the fully rederived hash plus keyset-policy tamper rejection.
- `cargo test -p cdk-axum --features federation dkg_transcript_signature -- --test-threads=1`:
  all 5 transcript-signature route tests passed.
- `cargo test -p cdk-mintd --features federation,fakewallet,sqlite federation_dkg_status -- --test-threads=1`:
  all 12 status, progress, reconstruction, and export tests passed.
- `cargo check -p cdk-mintd --features federation,fakewallet,sqlite`: passed.

Verified live-DKG process-harness checkpoint:

- Local federation generation now emits one executable
  `smoke-live-dkg-federation.sh` alongside the existing trusted-dealer smoke.
  It starts all but one member from setup-only configs, waits for durable partial
  commitments, interrupts and restarts a second member, proves the ceremony
  cannot complete while a required participant is offline, then waits for the
  resumed durable ceremony records.
- The harness exports every member's independently verified final public/private
  config, byte-compares all public configs, restarts the same databases and
  configs under AlephBFT, waits for runtime keyset activation and state-digest
  convergence, and invokes the standard mint/swap/melt/restore wallet smoke.
- `just fed-regtest-live-dkg` and `just fed-regtest-postgres-live-dkg` provide
  SQLite and isolated-schema Postgres entry points. Both targets pass `just
  --dry-run`; the generated script passes `bash -n` on Unix.
- `cargo test -p cdk-mintd --features federation,fakewallet,sqlite federation_setup::tests -- --test-threads=1`:
  all 12 generation tests passed, including generated live-DKG shell syntax and
  SQLite/Postgres config parsing.
- The first real SQLite attempt verified partial commitment collection,
  interruption, offline non-completion, and restart before exposing the repeated
  public-key derivation hot path fixed in `c932b152d`. A clean post-fix process
  rerun is still required; no end-to-end process success is claimed by this
  checkpoint.
- Remaining production-DKG gate: execute an interrupted and resumed multi-process ceremony with the generated live configs, restart every member onto the finalized keyset, and run mint/swap/melt/restore against it on SQLite and Postgres.

## Milestone 20A: optimize DKG performance and robustness

Objective: make dealer-free DKG fast, measurable, resource-bounded, and
operationally safe for federations of considerable size without weakening any
cryptographic verification or ceremony-domain invariant.

This milestone is benchmark-driven. The current five-member DKG checkpoint is
not sufficient evidence for a production-sized federation. A manual 20-member,
threshold-14, eight-denomination debug-process attempt saturated a powerful
development machine, took about five minutes to reach its first delivery
results, and remained incomplete after more than eighteen minutes. All members
had persisted commitments, members had only partial reveal sets, no member had
reached secret-share completion, and even self-directed REST requests timed out.
Treat this as a reproducible warning and harness input, not as a certified
benchmark: the first controlled baseline must separately measure optimized
release binaries and unoptimized debug binaries before attributing costs.

Benchmark and decision rules:

Verified harness checkpoint (2026-07-11):

- `just dkg-perf` now runs a versioned benchmark-only `cdk-common` example with
  no AlephBFT, wallet, mint daemon, REST, or database dependency. It supports
  explicit profile, denomination, build, iteration, phase, full-ceremony, and
  timeout controls and emits one machine-readable JSON report.
- Harness v4 measures every requested pure phase, modeled canonical bytes and
  durable record counts, and final transcript/key/share equality. An external
  permissioned `/usr/bin/time -lp` wrapper currently supplies wall, user,
  system, average CPU, and peak RSS. Peak-CPU sampling and real REST counters
  remain part of the optional process-runner work.
- The first unmodified 5-of-3 one-denomination baseline is recorded in
  `docs/federated-cashu-minting-performance-benchmark.md`: three complete
  release ceremonies have a 1.308s median versus 30.792s in debug, with every
  correctness flag true. No production optimization has been made or selected.
- The unmodified 11-of-8 release profile completed with a 32.084s ceremony
  median and every correctness flag true. The 20-of-14 one-denomination profile
  was interrupted after 1,116.72s without a complete report; this is enough to
  fail the current implementation as a quick iteration gate. Do not spend CPU
  on the eight-denomination 20-of-14 acceptance run until focused optimization
  makes the one-denomination profile complete within the quick gate.
- Candidate 1 is **Kept**. A non-serializable validated-participant wrapper now
  performs full validation once after generation or durable reload, binds the
  proof to the complete public-config digest, and reuses immutable participant
  state across driver ticks. Ten-sample release medians fell from 9.842ms to
  0.223ms at 5-of-3 (44.1x) and from 48.160ms to 0.844ms at 11-of-8 (57.1x),
  with all transcript, keyset, public-share, and local secret/public equality
  checks true. This is a planner-tick result, not a full-ceremony speed claim.
- A bounded five-member debug process smoke reached all commitments/reveals but
  repeatedly timed out the final member's secret-share request at the existing
  ten-second peer deadline while every receiver performed final key derivation
  before committing that contribution. The run was interrupted rather than
  extended. This does not contradict Candidate 1, but it confirms that final
  public-polynomial derivation and request-lifetime coupling remain the next
  robustness bottleneck; no timeout or protocol behavior was changed.
- Candidate 2 is **Kept**. The final public polynomial now sums each coefficient
  across participants once and evaluates that aggregate polynomial once per
  member, matching Fedimint's algebraic shape. An independent benchmark-only
  copy of the original algorithm proves byte-identical keysets. Release medians
  improved from 38.493ms to 9.919ms at 5-of-3 (3.9x), 466.559ms to 47.510ms at
  11-of-8 (9.8x), and 2.676s to 143.897ms at 20-of-14 (18.6x).
- The improvement carries through complete one-denomination ceremonies:
  5-of-3 fell from 1.308s to 0.457s, 11-of-8 from 32.084s to 4.498s, and a
  20-of-14 ceremony now completes in 23.725s with every final equality check
  true instead of remaining incomplete when the baseline was interrupted after
  1,116.72s. The eight-denomination and real-process responsiveness gates
  remain deferred rather than inferred from these pure-state-machine results.
- Candidate 3 is **Kept**. Fresh live-driver participant state now receives a
  non-serializable validated-by-construction proof instead of immediately
  converting every generated secret share back to a public point and
  reevaluating every public polynomial. Side-by-side release generation medians
  improved from 70.569ms to 20.461ms at 5-of-3 (3.4x), 645.072ms to 106.529ms
  at 11-of-8 (6.1x), and 3.299s to 347.621ms at 20-of-14 (9.5x).
- The ordinary public generator still performs complete self-validation.
  Persisted/reloaded state can only regain the in-process proof through the full
  validator, and the harness independently validates one generated candidate
  before reporting success. Thirty-one common DKG tests and seven permissioned
  release Axum driver/route tests pass, including fresh persistence, restart,
  tamper rejection, changed policy, and authenticated delivery.
- Candidate 4 is **Kept** as a responsiveness/load-bound change, not a speed
  optimization. Fresh generation, persisted-state validation, planner rebuilds,
  and final member-share derivation now use one process-local DKG CPU admission
  permit and `spawn_blocking`. The permit is owned by the blocking closure, so
  cancelling or timing out the awaiting request cannot admit overlapping work
  while the uncancellable CPU task continues.
- A five-sample current-thread release probe kept generation wall time unchanged
  at 3.944ms direct versus 3.945ms on the worker, while an Axum route probe fell
  from 3.971ms to 21us. A cancellation regression proves a second job cannot
  start until an abandoned first closure actually exits. Twenty-seven focused
  DKG driver, route, persistence, transcript, invalid-share, and restart tests
  pass. This scheduler-level result does not replace the real multi-process
  REST responsiveness and process-CPU gate.
- Candidate 5 is **Kept**. Commitment, reveal, secret-share, and transcript
  signature persistence now detect an exact durable record before rebuilding
  accumulated DKG state. They persist only the newer authenticated member
  sequence; conflicting records still take the existing fail-closed path before
  sequence advancement. Reveal replays additionally revalidate their durable
  prior commitment.
- The dominant completed secret-share replay fell from a 7.579ms ten-sample
  release median to 4us (about 1,895x) and completes while the DKG CPU permit is
  deliberately held, proving no keyset derivation occurs. Twenty-nine focused
  release DKG tests pass, including ten repeated completed-share replays,
  signed-route duplicates for every setup phase, one durable record per
  artifact, monotonic sequence advancement, immutable derived keysets, and all
  prior conflict/invalid-share checks.
- Candidate 6 is **Kept**. The live driver now tracks verified acknowledgements
  in memory for one commitment, reveal, transcript signature, and per-recipient
  secret share. A peer is suppressed only after its response passes the existing
  peer-client validation; timeout/error targets remain pending. Changed payloads
  reset the relevant acknowledgements, and driver restart starts with an empty
  tracker and safely rebroadcasts all artifacts.
- A three-member authenticated ceremony fell from 126 to exactly 36 delivery
  attempts, the unique artifact-to-peer minimum (71.4% fewer), while preserving
  five-round completion and identical final configs. Partition/restart coverage
  proves failed peers retry and sender restart forgets prior acknowledgements.
  Thirty focused release DKG tests pass.
- Candidate 7 is **Rejected**. The interrupted 20-of-14, eight-denomination
  release process run recorded 422 retryable delivery failures: 382 commitment
  failures followed the harness's two-second sequential listener startup, and
  40 secret-share failures occurred as two all-target waves roughly 130 seconds
  apart. The run used a 60-second peer request timeout and a one-second driver
  retry delay, so request/handler lifetime dwarfed the configured retry delay.
  Adding a few seconds of jitter cannot remove the startup race or the expensive
  first-time secret-share work, while a large backoff would directly slow
  recovery. No production retry behavior was changed; the retained
  acknowledgement tracker already suppresses successful targets.
- Candidate 8 is **Kept** for startup-race elimination. A fresh ceremony now
  sends an authenticated readiness probe carrying the exact federation,
  ceremony ID, and keyset policy to every member before generating participant
  state or sending commitments. Successful probes are remembered only for that
  driver run; mismatched configuration fails closed, and no readiness probe is
  persisted as a ceremony artifact or consensus item.
- In the focused staggered three-member transport test, the first round reached
  two members, failed one unavailable route, and wrote zero commitments. After
  the route appeared, the second round attempted only that member and completed
  the barrier. A durable participant state skips the volatile barrier after
  restart, preserving recovery if already-ready peers later activate or go
  offline. Forty-two common transport tests, 32 release Axum DKG tests, and
  three mintd auto-start tests pass. The interrupted 20-member process ceremony
  was not rerun, so its completion and secret-share responsiveness gates remain
  open.
- Candidate 9 is **Kept** for bounded operator diagnostics, with no speed claim.
  DKG delivery retries now emit a fixed `failure_kind`, member, phase, and only
  an optional numeric HTTP status. Typed local errors distinguish timeout and
  outbound admission; existing HTTP and federation errors distinguish
  connection, remote admission, remote validation, remote storage, remote
  server, response decode/validation, transport configuration, transport, and
  unknown failures. Raw error text, URLs, response bodies, and DKG payloads are
  not retained in the sender diagnostic.
- DKG database errors return a generic retryable HTTP 503 while other internal
  processing failures remain generic HTTP 500, allowing storage/server
  classification without parsing response text. The receiver log passes the
  underlying error through the existing operator redactor. A focused regression
  injects private-looking details into every error class and proves the
  classified value contains none of them. All 34 focused release Axum DKG tests
  pass; retry behavior, ceremony messages, persistence, phases, and activation
  are unchanged.
- Harness v5 adds isolated single-contribution verification and single-member
  final-derivation phases. At eight denominations, release medians are
  15.094ms versus 159.791ms for 5-of-3, 33.756ms versus 752.266ms for 11-of-8,
  and 56.148ms versus 2.276s for 20-of-14. The 20-member final derivation is
  40.5x slower than validating one incoming contribution before any
  multi-process CPU contention. This establishes the pre-change baseline for
  decoupling durable contribution acknowledgement from final local derivation;
  no production behavior changed in this harness checkpoint.
- Candidate 10 is **Kept** for request liveness and earlier cryptographic
  rejection, not for lower total CPU. Every first-time contribution is now
  verified against its sender's committed reveal on the bounded CPU worker
  before the contribution and authenticated sequence are committed atomically.
  Invalid shares fail before persistence even when they are not the final
  contribution. The HTTP response then acknowledges that durable contribution;
  only the existing restartable driver derives and persists the final member
  keyset from the complete durable record set.
- Repeated harness v5 eight-denomination medians put the new route-critical
  verification at 15.286ms, 34.399ms, and 56.793ms for 5-of-3, 11-of-8, and
  20-of-14. The corresponding pre-change final derivations were 159.791ms,
  752.266ms, and 2.276s, so the cryptographic acknowledgement path is 10.5x,
  21.9x, and 40.1x shorter. Driver derivation remains 161.116ms, 771.336ms,
  and 2.322s and fully revalidates durable records for restart safety. Thirty-two
  common DKG tests and all 34 release Axum DKG tests pass, including invalid
  first-share rejection, acknowledgement-before-derivation, driver completion,
  restart activation, transcript equality, and prior fault coverage.
- Candidate 11 is **Kept** for activation-request liveness and removal of
  repeated immutable validation. An unmodified release 11-of-8 process run
  reached activation, then every receiver independently rebuilt and fully
  validated the same finalized member config for each of 11 concurrent
  senders. Initial requests exceeded the ten-second peer deadline, retries hit
  the still-held per-sender admission slots with HTTP 429, and at least one
  driver remained unfinished after more than 90 seconds in activation even
  though durable activation rows were converging. The representative member
  log showed one 25ms slow commit and `/v1/info` stayed responsive, isolating
  repeated synchronous finalized-config reconstruction rather than database or
  event-loop failure.
- Each setup process now uses one fallible `OnceCell` beside its existing DKG
  admission state. The first activation request after complete transcript
  records builds and validates the immutable finalized public/private config;
  concurrent requests await and reuse that result. An early incomplete lookup
  is not cached, and every request still validates its member signature and
  executes the unchanged conflict, sequence, and atomic persistence checks.
  The identical 11-of-8/eight-denomination rerun completed setup in 34 seconds
  with zero delivery timeouts, zero post-start info failures, and exact final
  config equality, then passed activation restart and mint/swap/melt/restore.
  All 50 focused release Axum DKG tests pass.
- Candidate 12 is **Kept** for eliminating repeated trust-boundary validation.
  The first controlled release 20-of-14/eight-denomination process run reached
  activation but failed after 467 seconds with 6,989.29 user CPU seconds and
  1,216 request timeouts. The cached finalized config was being fully validated
  again inside every sender's activation check. Harness v6 measures the ordinary
  checked path versus the explicit already-validated path at 116.204ms versus
  161us for 5-of-3, 463.958ms versus 555us for 11-of-8, and 1.300s versus
  1.527ms for 20-of-14.
- The checked public signing/validation APIs remain unchanged. Only callers that
  just assembled or retrieved a fully validated immutable finalized config use
  the new variants; federation ID, ceremony, transcript hash, config digest,
  sender signature, extension shape, sequence, conflict, and persistence checks
  remain mandatory. The fresh 20-of-14 run completed setup in 116 seconds with
  1,724.13 user CPU seconds, 243 recovered timeouts, zero runtime info failures,
  byte-identical final configs, and a 7.834-second mint/swap/check-state/melt/
  restore smoke after restart. The documented release budget is 180 seconds.
- Candidate 13, caching the locally derived transcript hash, is **Rejected**:
  the exact 5-of-3/eight-denomination derivation is only 200us and cannot explain
  a ten-second timeout. Candidate 14, limiting each daemon to two Tokio workers,
  is **Rejected** after increasing the 20-member timeout count from 243 to 516.
  Candidate 15, proactively rebuilding finalized configs before activation, is
  **Rejected** after an unfinished 139-second run accumulated 540 timeouts and
  one runtime info-probe failure. All three implementations were removed.
- Candidate 16 is **Kept** for bounded artifact fanout. Each driver now has at
  most four commitment, reveal, secret-share, transcript-signature, or activation
  requests in flight while still attempting every unacknowledged member each
  tick. The controlled 20-member run completed in 130 seconds with 37 timeouts,
  down 84.8% from 243, and 76 failures, down 73.9% from 291. User CPU fell from
  1,724.13 to 1,418.67 seconds and peak RSS from 96,544 to 93,248 KiB. Rotated
  target order increased timeouts to 76; serial delivery took 248 seconds and
  still recorded 12 late timeouts, so both variants are **Rejected**. The
  zero-self-timeout success criterion remained open at this checkpoint.
- Candidate 17 is **Kept** for activation-only replanning. Harness v7 reduces a
  complete incremental activation sweep from 2.259s to 2.499ms at 5-of-3/eight
  and from 22.581s to 39.049ms at 11-of-8/eight. The first 20-of-14 process run
  completed in 110 seconds with 1,304.15 user CPU seconds, 39 failures, and nine
  timeouts. The cache is process-local, bound to the validated setup digest,
  and is used only when every non-activation durable record is byte-identical;
  all changed activations still receive the ordinary signature/digest checks.
- Candidate 18 is **Kept as a fail-fast trust boundary**. Activation routes use
  only the finalized config already validated and seeded by the local driver;
  an early request receives retryable HTTP 425 and cannot trigger a duplicate
  config reconstruction or advance sequence/persistence. Diagnostics classify
  this separately as `remote_not_ready`. In isolation it reduced setup to 101
  seconds and activation timeouts to zero, but produced about 400 quick retries,
  so it required the phase-handoff improvement below rather than being accepted
  as a standalone load optimization.
- Candidate 19 is **Kept** for prior-tick acknowledgement gating. Transcript
  signatures are sent only after every matching secret share was acknowledged
  before the current tick; activations follow the same rule for the sender's
  transcript signature. No message, durable phase, timeout, or cryptographic
  rule changes, and restart safely rebroadcasts because the tracker is volatile.
  The final 20-of-14/eight run completed in 89 seconds using 1,071.00 user and
  7.24 system CPU seconds, 83,408 KiB peak RSS, zero runtime info failures, and
  zero private-REST timeouts. Its 285 bounded failures were 19 deliberate
  interruption connections, 25 early transcript rejections, and 241 retryable
  activation-readiness responses. All 20 exports were byte-identical and the
  restart mint/swap/check-state/melt/restore smoke passed in 7.447 seconds.
- The bounded-admission robustness checkpoint is **Kept** without a speed
  claim. Dealer-free DKG now accepts at most 64 roster members and 64
  denomination keys, while broader standalone and trusted-dealer configuration
  formats remain unchanged. Every DKG private REST route has an independent
  1 MiB decoded-body cap and requires an active local ceremony. Commitment,
  reveal, and secret-share requests must match that ceremony ID and its exact
  keyset/amount shape before expensive reveal/share validation or persistence.
  The maximum supported signed reveal shape (64 amounts by 64 G2
  coefficients) serializes to 800,039 bytes, leaving 248,537 bytes of route
  headroom. An authenticated request padded past the cap returns HTTP 413 with
  no sequence or DKG record mutation. Thirty-five common, 40 permissioned
  release Axum, and three mintd startup DKG tests pass. Remote deployment still
  requires operator-provided HTTPS or an equivalent confidential tunnel.
- The fair-member-admission checkpoint is **Kept** without a speed claim. Each
  configured sender now has one nonblocking post-authentication DKG slot at
  each receiver, matching the protocol's sequential per-sender phase flow. A
  sender with an in-flight request receives HTTP 429 for another DKG request;
  it cannot occupy another authenticated member's slot or race its own DKG
  state mutation. The focused three-member regression holds one sender's slot,
  proves its request leaves sequence and commitments unchanged, accepts a
  second sender's commitment, then accepts the first sender's exact retry after
  release. Active ceremony state without its per-member map fails closed with
  HTTP 503. The existing global 128-request admission remains the aggregate
  bound. A bounded token bucket now supplies the sustained per-member policy
  described in the next checkpoint.
- The restrictive-secret-file checkpoint is **Kept** without claiming complete
  at-rest encryption. Generated final/setup private JSON, member TOML files
  containing mnemonics or database credentials, and signed DKG route-probe
  files now use a dedicated private writer. On Unix, first creation uses
  `create_new` at mode `0600`; forced replacement writes and syncs a same-
  directory `0600` temporary file and atomically renames it, so a pre-existing
  symlink is replaced rather than followed. Mint startup rejects private
  federation config files with any group/other permission bits. Thirteen local
  generator, two backend-agnostic core-generator, and six startup release tests
  pass, including forced mode repair, symlink-target preservation, all generated
  secret-bearing artifacts, valid startup, and permissive-file rejection.
  Non-Unix ACL enforcement and database payload encryption remain recorded as
  out-of-scope residual risks below.
- The sustained authenticated-member admission checkpoint is **Kept** without
  a speed claim. The existing per-member in-flight slot now carries an
  independent token bucket created only for a validated active ceremony. The
  default permits a 16-request burst and restores one token every 250ms;
  startup restricts bursts to 1..=64 and refuses refill intervals below 10ms.
  Exhaustion returns retryable HTTP 429 after roster authentication but before
  ceremony validation, sequence advancement, expensive DKG work, or database
  mutation. A focused route regression exhausts one member, proves another
  member remains admissible, verifies the rejected retry leaves its sequence
  unchanged, and accepts that exact retry after refill. All 42 release Axum DKG
  tests, three mintd DKG policy tests, and 13 generated-config tests pass.
- The explicit all-member activation barrier is **Kept** as a safety checkpoint,
  without a speed claim. Complete transcript signatures now justify an
  `Activation` phase rather than immediate completion. Each member signs a
  domain-separated confirmation binding federation ID, ceremony ID, signer,
  complete transcript hash, and final public-config digest; that transcript
  and digest commit the roster, threshold/keyset policy, keyset ID, aggregate
  keys, every public share, and the exact collected transcript signatures.
  Confirmations use the existing authenticated private REST fanout, one new
  idempotent ceremony/member SQL record, and no AlephBFT item or wallet phase.
  Durable confirmations are verified against the signed setup-time finalized
  config reconstructed from the transcript, so later permitted keyset-state
  changes do not invalidate the original ceremony evidence.
  Only all roster confirmations make the driver complete or let startup
  advertise/sign with the keyset. A setup-only member remains inert while one
  confirmation is missing, and even a directly supplied finalized Pedersen-DKG
  config fails startup without matching durable evidence. Trusted-dealer and
  standalone activation remain unchanged. The three-member authenticated
  artifact minimum rises from 36 to 45 deliveries. Thirty-five common DKG, 43
  Axum DKG, five focused builder, 13 mintd status, 13 generator, six SQLite DKG,
  and SQL migration-parity tests pass; the Postgres activation test compiles.

- [x] Add a focused DKG performance harness that can run independently of
  AlephBFT, wallet, mint/swap/melt, and the long full-federation suites. It must
  support fast phase-level probes and an optional real multi-process ceremony,
  use disposable isolated work directories and ports, stop every child process
  on success/failure/interruption, and never require an unrelated long-running
  test before reporting a result.
- [x] Give the harness explicit profiles for at least 5-of-3, 11-of-8, and
  20-of-14 federations, with one-denomination and standard eight-denomination
  variants. The one-denomination profile is the quick iteration gate; the
  20-of-14 eight-denomination profile is the large-federation acceptance gate.
- [x] Measure fresh participant generation, persisted participant reload and
  validation, one unchanged planner tick, one changed-record planner tick,
  commitment/reveal ingestion, duplicate ingestion, secret-share verification,
  local keyset derivation, transcript derivation, and complete multi-process
  ceremony time separately so one fast aggregate number cannot hide a hot
  phase.
- [x] Record wall, user, and system time; average and peak CPU; peak RSS;
  per-phase elapsed time; REST attempt/success/timeout counts; duplicate request
  counts; durable record counts; bytes sent; and whether every member completed
  with the same transcript hash, keyset ID, aggregate keys, and local public
  share mapping. Sampling must be low overhead and removable or explicitly
  benchmark-only.
  The generated live-DKG runner now has opt-in `DKG_CAPTURE_METRICS=1`
  accounting. It preserves logs across setup restarts, samples each member's
  CPU percentage, RSS, user time, and system time, and sums CPU time across PID
  changes caused by intentional setup restarts. It probes the primary member's
  `/v1/info` latency/status, records setup wall time, and prints exact
  attempt/success/failure, timeout, and duplicate-success totals from opt-in
  per-tick events. Raw process and REST CSV files are retained under
  `dkg-metrics`, and the generated scripts pass `bash -n`. The EXIT trap now
  takes a final sample, stops the monitor and every member, and emits the same
  summary exactly once with an explicit `setup_failure` marker when setup exits
  early. A bounded synthetic failure using a non-starting binary exited nonzero,
  cleaned up all three child PIDs, and reported REST plus zeroed delivery totals
  without launching a real federation. INT/TERM now exit with status 130 and
  invoke EXIT cleanup exactly once; a live process run confirmed immediate
  shutdown instead of continuing after Ctrl-C.

  A release 5-of-3, eight-denomination validation completed interrupted setup
  and restart in 10 seconds, then passed activation plus mint/swap/melt/restore.
  All members produced the same transcript, keyset, aggregate keys, and local
  public-share mapping. Across five members it used 36.14 user CPU seconds and
  0.30 system CPU seconds, peaked at 42,352 KiB RSS per member, and completed
  146 artifact deliveries: 130 successful, 16 expected connection failures
  while the interrupted member was offline, zero timeouts, and five exact
  duplicate successes after its in-memory acknowledgement tracker restarted.
  Nine post-listener `/v1/info` probes succeeded with zero runtime failures;
  peak latency under setup load was 0.500 seconds.

  The next release scaling checkpoint used 11 members, threshold eight, and
  eight denominations. Its first run exposed the repeated activation-validation
  bottleneck above and failed the driver-completion boundary. After Candidate
  11, setup completed in 34 seconds using 345.53 user CPU seconds and 1.92
  system CPU seconds, with 52,864 KiB peak per-member RSS. The collector counted
  636 attempts, 616 successes, 20 expected offline connection failures, zero
  timeouts, and 11 exact restart duplicates. Thirty-one post-listener info
  probes succeeded with zero runtime failures and 91ms peak latency. All 11
  members exported the same transcript, keyset, aggregate keys, and local
  eight-amount public-share mapping; the standard wallet smoke passed in 7.963
  seconds.

  The final release acceptance profile used 20 members, threshold 14, and eight
  denominations. Its pre-fix attempt failed after 467 seconds; after Candidate
  12, setup completed in 116 seconds with 1,724.13 user and 9.28 system CPU
  seconds, 96,544 KiB peak per-member RSS, 2,311 deliveries, 291 failures, 243
  recovered timeouts, and zero runtime `/v1/info` failures. Every member
  exported the same final public config and restarted with its own private
  share; mint, swap, check-state, melt, and restore passed in 7.834 seconds.
  Candidate 16 then bounded artifact fanout at four concurrent requests and
  completed in 130 seconds with 1,418.67 user CPU seconds, 93,248 KiB peak RSS,
  2,096 deliveries, 76 failures, and 37 timeouts. Candidates 17-19 reuse the
  validated activation plan, fail fast until that config is locally ready, and
  hand later artifacts across prior-tick acknowledgement boundaries. The final
  run completed in 89 seconds with 1,071.00 user CPU seconds, 83,408 KiB peak
  RSS, zero runtime info failures, and zero timeouts; all exported configs,
  restart activation, and wallet operations passed.
- [x] Run the primary performance gate with release binaries. Keep one debug
  profile as a developer-experience regression detector, but never use a debug
  result alone to accept or reject a cryptographic optimization.
- [x] Establish an unmodified baseline before each optimization series. For
  noisy process measurements, run enough repetitions to report a median and
  range; preserve the exact member/threshold/denomination profile, build mode,
  machine power mode, and concurrency settings for before/after comparisons.
- [x] Treat "baseline before production-code changes" as a sequencing rule,
  not a blanket ban on instrumentation. After the untouched baseline, the agent
  may add the smallest behavior-neutral timing spans, counters, tracing, or
  profiling hooks needed to answer a specific DKG performance question,
  including narrowly scoped probes around production code paths. Instrumentation
  must not change cryptographic verification, ceremony messages, phase gates,
  persistence, REST admission, retry policy, or activation behavior. Remove
  temporary instrumentation as soon as the issue is resolved, the probe becomes
  obsolete, or the candidate decision is recorded, and always before closing
  the checkpoint. Retain instrumentation only when it has focused tests and is
  intentionally useful as normal low-noise production diagnostics.
- [x] Use
  `docs/federated-cashu-minting-performance-benchmark.md` to log DKG observations
  in a clearly separated DKG section: command/harness version, commit, profile,
  build mode, raw measurements, bottleneck classification, retained/rejected
  decision, and remaining risk. A dedicated DKG benchmark document may be split
  out later only if the combined benchmark note becomes difficult to navigate.
- [x] Evaluate one optimization at a time whenever practical. A candidate may
  be retained only after the focused harness shows a repeatable improvement in
  its intended metric and all cryptographic/correctness gates remain unchanged.
  If it does not improve the intended metric, regressions outweigh the gain, or
  the hypothesis is disproved, check it off as **Rejected** and record the
  evidence instead of retaining speculative complexity.
- [x] Keep benchmark tooling deterministic and cheap enough that a coding agent
  can test a candidate in minutes with the quick profile before spending CPU on
  the 20-of-14 acceptance profile. The harness must support phase selection,
  bounded timeouts, early failure on lack of progress, and machine-readable
  output suitable for before/after comparison.

Optimization candidates were evaluated in expected priority order. Every item
below is closed as **Kept** or **Rejected** with recorded measurements:

- [x] Validate immutable participant state once at the trust boundary rather
  than on every driver tick and every planner rebuild. Preserve full validation
  after deserialization/restart and reject any persisted-state mismatch.
  Outcome: **Kept**. Harness v2 measured 44.1x and 57.1x lower planner rebuild
  medians at 5-of-3 and 11-of-8 respectively. The reusable proof is
  non-serializable, is recreated after generation/reload, binds the complete
  config digest, and preserves full validation in the legacy planner API.
- [x] Aggregate public polynomial coefficients across participants before
  evaluating the aggregate polynomial for each federation member. Prove the
  optimized result is byte-for-byte equivalent to summing independently
  evaluated participant polynomials, including malformed-commitment rejection.
  Outcome: **Kept**. Harness v3 retains the original derivation as an independent
  reference and reports exact keyset equality at 5-of-3, 11-of-8, and 20-of-14;
  isolated release speedups are 3.9x, 9.8x, and 18.6x respectively. Existing
  reveal validation still runs first, and focused malformed-coefficient,
  transcript, member-share, restart, and authenticated-route tests pass.
- [x] Avoid an expensive public-polynomial round-trip immediately after freshly
  generating state from known coefficients, while retaining construction
  invariants and full validation when state crosses a persistence or transport
  boundary. Outcome: **Kept**. Harness v4 reports 3.4x, 6.1x, and 9.5x lower
  fresh-generation medians at 5-of-3, 11-of-8, and 20-of-14. Only the fresh
  live-driver path can receive the non-serializable construction proof; the
  public raw generator and every deserialized/restarted state still perform
  full polynomial/share validation.
- [x] Move participant generation and every remaining expensive derivation to a
  bounded blocking/CPU worker path so Axum and private REST stay responsive.
  Measure both total CPU time and request latency; do not call a pure scheduling
  move a speed improvement if it only improves responsiveness. Outcome:
  **Kept** for responsiveness and bounded load. The same generation closure took
  3.944ms direct and 3.945ms on the worker, while a five-sample Axum probe median
  improved from 3.971ms to 21us. One process-local permit covers generation,
  reload validation, planning, and member-share derivation and remains held
  across waiter cancellation. No CPU-time reduction is claimed; real-process
  CPU and private REST latency remain an acceptance gate below.
- [x] Add exact-duplicate fast paths for commitment, reveal, secret-share, and
  transcript-signature ingestion before rebuilding accumulated setup state.
  Conflicting duplicates must still fail before sequence advancement or record
  replacement. Outcome: **Kept**. A completed secret-share replay improves from
  7.579ms to 4us in the same release fixture and succeeds with the DKG CPU
  permit held. Exact replays preserve only the new authenticated sequence;
  durable artifact/keyset counts remain unchanged, reveal replays recheck their
  prior commitment, and all conflicting duplicate tests remain fail-closed.
- [x] Stop rebroadcasting phase artifacts to peers that returned a verified
  acknowledgement, while preserving idempotent recovery after ambiguous timeout
  or restart. Prefer a simple bounded delivery tracker over a new consensus
  phase. Outcome: **Kept**. A bounded in-memory tracker reduces a three-member
  authenticated ceremony from 126 to 36 attempts. Only validated `Ok` responses
  suppress a target; timeout/error targets retry, changed payloads reset their
  phase tracker, and process/restarted-driver state intentionally rebroadcasts
  because acknowledgements are not persisted.
- [x] Benchmark a slower, jittered retry policy and phase-aware backoff to avoid
  synchronized all-to-all retry waves. Treat this as load mitigation, not a
  substitute for removing repeated cryptographic work. Outcome: **Rejected**.
  In the interrupted 20-of-14 release process run, 382 commitment failures
  tracked the two-second sequential listener startup and 40 secret-share
  failures formed two all-target waves about 130 seconds apart under a
  60-second request timeout. The existing one-second retry delay was not the
  dominant cadence. A bounded jitter model therefore either changes almost
  nothing or adds material recovery latency without addressing the measured
  causes. Candidate 6 already removes acknowledged targets, so no speculative
  retry scheduler was added.
- [x] Add an explicit all-members-ready/start barrier before the first DKG phase
  so participant generation does not race process startup. Measure separately:
  a release 20-of-14 harness run started listeners over a 38-second interval and
  produced 382 commitment failures, with later-starting targets accumulating
  progressively more failures. Outcome: **Kept**. Fresh members now authenticate
  an exact ceremony/keyset readiness probe with every peer before participant
  generation or commitment delivery. A focused staggered three-member test
  records two successes and one unavailable route, zero commitment writes, then
  exactly one pending probe after the missing route appears. Verified successes
  are volatile and suppressed within the run; an existing durable participant
  skips the barrier after restart. No consensus phase or durable readiness state
  was added. The later 20-of-14 process acceptance run passed in 89 seconds.
- [x] Improve redacted DKG delivery diagnostics so timeout, HTTP status,
  admission, validation, and storage failures are distinguishable. This has no
  direct speed claim; retain it only if it materially shortens diagnosis or
  enables trustworthy benchmark classification without exposing secret data.
  Outcome: **Kept**. Sender logs now contain only member, phase, a fixed failure
  class, and an optional numeric HTTP status. Local typed errors distinguish
  request timeout and outbound admission; remote 429, 4xx, 503, and other 5xx
  responses distinguish admission, validation, storage, and server failures;
  connection, decode, response-validation, transport-configuration, transport,
  and unknown classes are separate. DKG database responses use generic 503 and
  other internal failures generic 500 without exposing response details. A
  redaction regression proves private-looking source messages are absent from
  every classified value. Outcome does not claim lower latency.
- [x] Validate each first-time secret-share contribution against its sender's
  revealed polynomial before durable acceptance, then acknowledge the durable
  contribution without coupling that HTTP request to final local keyset
  derivation. Let the existing driver derive and persist the final member share
  from complete durable records. Harness v5 baseline: at eight denominations,
  one contribution verification versus one member derivation is 15.094ms versus
  159.791ms (5-of-3), 33.756ms versus 752.266ms (11-of-8), and 56.148ms versus
  2.276s (20-of-14). Outcome: **Kept**. Repeated after medians for the shared
  verifier are 15.286ms, 34.399ms, and 56.793ms, making the route-critical
  cryptographic path 10.5x, 21.9x, and 40.1x shorter than the former final
  request derivation. The driver still spends 161.116ms, 771.336ms, and 2.322s
  deriving and fully revalidating the durable record set, so no total-CPU
  reduction is claimed. Invalid first shares fail before persistence, final
  contributions are acknowledged before keyset creation, and restart derives
  and activates the identical keyset.
- [x] Reuse the immutable finalized member config across concurrent activation
  requests instead of rebuilding and fully validating it once per sender.
  Outcome: **Kept**. The first release 11-of-8/eight-denomination process run
  produced repeated ten-second activation timeouts and HTTP 429 retry
  amplification while databases converged. A fallible process-local `OnceCell`
  now caches only a successfully finalized config; incomplete early lookups
  remain retryable. The identical rerun completed setup in 34 seconds with zero
  delivery timeouts and zero runtime info failures, and all 50 focused release
  Axum DKG tests preserve activation signature/conflict/sequence/write checks.
- [x] Reuse already-validated setup and finalized configs inside activation
  signing and sender checks instead of invoking full config validation again
  for every activation request. Outcome: **Kept**. Harness v6 eight-denomination
  medians improve from 116.204ms to 161us at 5-of-3, 463.958ms to 555us at
  11-of-8, and 1.300s to 1.527ms at 20-of-14. The pre-fix 20-member process run
  failed after 467 seconds with 1,216 timeouts; the fresh after-run completed in
  116 seconds with 243 recovered timeouts, zero runtime info failures, identical
  final configs, and passing wallet operations. The ordinary checked APIs remain
  available and unchanged, while the validated-config variants retain every
  activation binding and signature check.
- [x] Cache the locally derived transcript hash if exact phase timing shows it
  is material to delivery latency. Outcome: **Rejected**. Harness v6 measures
  the exact 5-of-3/eight-denomination derivation at only 200us, far below the
  ten-second request deadline. The cache and its temporary probes were removed.
- [x] Reduce per-daemon Tokio worker counts to limit one-host CPU contention.
  Outcome: **Rejected**. Two workers increased the controlled 20-of-14 timeout
  count from 243 to 516 and regressed mean/peak info latency to
  351.051ms/6.072s. No runtime setting or production change was retained.
- [x] Proactively prepare the finalized config before activation fanout.
  Outcome: **Rejected**. The controlled run remained incomplete at 139 seconds,
  accumulated 540 delivery timeouts, and produced one runtime info-probe
  failure because it duplicated expensive finalized-config work during phase
  convergence. The preparation code was removed.
- [x] Bound each driver's artifact fanout while retaining delivery to every
  unacknowledged member. Outcome: **Kept** at four concurrent requests in
  deterministic roster order. The controlled 20-of-14 run completed in 130
  seconds and reduced recovered timeouts from 243 to 37 and total failures from
  291 to 76, while user CPU fell from 1,724.13 to 1,418.67 seconds and peak RSS
  from 96,544 to 93,248 KiB. Rotated order increased timeouts to 76; serial
  delivery took 248 seconds and still recorded 12 late timeouts, so both
  variants are **Rejected**. The zero-self-timeout criterion remained open at
  this checkpoint and is closed by Candidates 17-19 below.
- [x] Reuse the validated finalized config when only activation records change.
  Outcome: **Kept**. Harness v7 measures 904x lower 5-of-3/eight sweep latency
  and 578x lower 11-of-8/eight latency. The fast path verifies its setup digest
  and every activation, matches the full plan, and falls back on any immutable
  record change.
- [x] Keep activation request handling out of duplicate finalized-config work.
  Outcome: **Kept as a fail-fast trust boundary**. The driver seeds one
  validated process-local config; early routes return classified retryable HTTP
  425 without persistence. The immediate policy alone amplified quick retries,
  so it is paired with the measured acknowledgement handoff below.
- [x] Gate transcript-signature and activation fanout on complete prerequisite
  acknowledgements from the prior tick. Outcome: **Kept**. The final release
  20-of-14/eight run completes in 89 seconds with zero self-request timeouts,
  identical final configs, passing restart/wallet operations, and unchanged
  message, persistence, deadline, and cryptographic semantics.

Production robustness checklist:

- [x] Verify each received secret-share contribution against the sender's
  revealed public polynomial before durable acceptance. Invalid contributions
  must identify the offending member, fail before sequence advancement, and
  never wait until final all-member derivation to surface. The shared reveal
  verifier checks exact ceremony, receiver, denomination set, signer ID, and
  every scalar-derived public share against the sender polynomial on the bounded
  CPU worker before opening the write transaction. A bad first contribution
  leaves both contribution storage and sender sequence unchanged.
- [x] Protect participant state, received scalar contributions, final private
  shares, identity keys, mnemonics, and backups at rest with restrictive
  permissions, redaction, and an explicit secret-data inventory. Restrictive
  Unix file permissions are complete for generated identity/final-share JSON,
  setup-only identity JSON, mnemonic/database-credential member TOML, and
  signed DKG probes; forced writes are atomic and symlink-safe, and startup
  rejects permissive private federation JSON. Database encryption at rest is
  explicitly out of scope and not required for Milestone 20A; the plaintext
  database payload inventory is recorded for a future production-hardening
  milestone. Non-Unix ACL policy remains separate future hardening work; no
  backup-encryption or backup requirement is part of the Milestone 20A gate.
- [x] Harden confidential peer transport beyond signed payload authentication:
  verified HTTPS and an explicit mTLS/pinning policy where appropriate, no
  secret-bearing insecure redirects, bounded request bodies, and per-member
  admission/rate limits. Localhost plaintext remains an explicit development
  exception only. The request-body portion is complete for DKG: every setup
  route independently caps decoded JSON at 1 MiB and the maximum supported
  64-by-64 reveal is 800,039 bytes. The production DKG driver uses the default
  HTTP transport, which verifies WebPKI TLS certificates and disables redirects;
  it never enables invalid-certificate acceptance. The initial protocol policy
  deliberately does not require mTLS or static TLS-key pinning: federation
  identity signatures authenticate the exact configured sender at the
  application layer, the config digest commits every identity key and HTTPS
  endpoint, and verified TLS supplies confidentiality and endpoint server
  authentication. Deployments may terminate TLS at a controlled reverse proxy;
  mTLS/SPKI pinning remains optional deployment hardening rather than a second
  protocol identity system. Plain HTTP is accepted only when explicitly enabled
  and every member endpoint is loopback. One nonblocking authenticated DKG slot
  and one bounded token bucket per roster member prevent a member with concurrent
  or sustained setup traffic from occupying another member's DKG capacity.
- [x] Separate DKG completion from keyset activation with an explicit barrier.
  Every member must confirm the same federation ID, ceremony ID, transcript
  hash, roster, keyset policy, keyset ID, aggregate keys, and public shares
  before any member advertises or signs with the keyset. Partial activation
  fails closed and is operator-visible. The final signed confirmation binds the
  complete transcript and final-config digest, is persisted by ceremony/member,
  and is required from the full roster by the planner, `dkg-status`, config
  export, setup-record startup activation, and direct finalized-config startup.
  A digest/signature mismatch fails before sequence or activation-record
  mutation, while exact retry remains idempotent.
- [x] Define explicit abort and roster-reconfiguration semantics. This
  cooperative DKG requires every configured participant, not merely the signing
  threshold; one unavailable or malicious member must lead to a clear durable
  aborted state and a new ceremony, never silent completion with fewer dealers.
  Each local database now records an exact `active`, `aborted`, or `complete`
  ceremony lifecycle binding federation ID, local member, keyset policy, and
  predecessor/successor IDs. Changing an unfinished ceremony requires a new
  ceremony ID plus explicit `federation.dkg.replaces_ceremony_id`; startup
  compare-and-sets the old record to `aborted` and inserts the replacement in
  one transaction. Exact restart is idempotent, an implicit or conflicting
  replacement fails before mutation, and a completed ceremony cannot be
  aborted through this path: operators must use key rotation. A member newly
  added by an explicit roster reconfiguration may declare a predecessor absent
  from its fresh local database, while existing members must match their
  durable predecessor chain. The replacement still uses the existing exact
  all-member readiness barrier for its new roster/federation and never lowers
  the dealer requirement. Completion compare-and-sets `active` to `complete`;
  `dkg-status` exposes the lifecycle chain, startup ignores all aborted ceremony
  artifacts, and no AlephBFT item or additional DKG network phase is added.
  Focused SQL compare-and-set, mintd replacement/restart, mint-builder stale
  activation, authenticated completion, and status regressions pass.
  The current verified gate is 50/50 release Axum DKG tests, 19/19 release mintd
  DKG/status tests, six release builder DKG tests, seven release SQLite DKG
  persistence tests, migration parity, and a successful Postgres lifecycle
  test compile with `--no-run`.
- [x] Scope every durable setup message, contribution, acknowledgement,
  transcript signature, and activation record by ceremony and keyset so aborted
  attempts, audits, rotations, and later ceremonies cannot collide or require
  unsafe record deletion. Incoming commitment, reveal, and contribution routes
  now reject messages outside the exact active in-memory ceremony/keyset policy
  before persistence. Commitment/reveal and private-contribution SQL coordinates
  now include the ceremony ID; participant state and activation records were
  already ceremony-scoped, while keyset IDs and transcript hashes provide the
  existing cryptographic scope for derived shares and signatures. The live
  driver filters every loaded artifact to its configured ceremony, so retained
  historical records cannot collide with or contaminate a replacement. This is
  an intentional pre-deployment schema cut-through with no compatibility
  migration for disposable earlier federation databases. Seven SQLite DKG tests,
  migration parity, and an authenticated three-member prior-record isolation
  regression pass.
- [x] Bound CPU, memory, blocking workers, SQLite writers, and queued work per
  authenticated peer. A compromised roster member must not exhaust a receiver
  with validly signed duplicate or oversized DKG requests. Ceremony dimensions
  are now capped at 64 members and 64 denominations, DKG route bodies at 1 MiB,
  expensive DKG CPU work remains semaphore-bounded, exact duplicates use their
  cheap paths, and one post-authentication slot serializes each member's DKG
  mutations without blocking another member. The per-member token bucket
  bounds sustained traffic at a configurable 1..=64-request burst and a refill
  interval of at least 10ms. With the 64-member roster cap, at most 64 DKG
  requests can wait behind the single CPU worker; the existing global private
  REST cap is 128, admission never queues on either nonblocking request
  semaphore, and SQLite's shared per-database write gate admits one explicit
  transaction writer at a time.
- [x] Encrypted backup and clean-machine restore procedures are explicitly out
  of scope for Milestone 20A. No backup or backup-encryption requirement is
  part of this milestone's production gate; future backup work must still prove
  that a recovered private share maps to the transcript's configured public
  share before a member rejoins.
- [x] Add a focused production failure matrix covering interruption/restart in
  every phase, ambiguous timeout after durable persistence, duplicate and
  reordered delivery, one slow/offline member, malformed and invalid shares,
  conflicting reveals, disk-full/write failure, transcript mismatch, partial
  activation, abort/replacement, and failure recovery. No test may log scalar
  material.
  The first matrix checkpoint is complete: a three-member ceremony now restarts
  one member immediately at commitment, reveal, secret-share delivery, local
  keyset-share persistence, transcript signature, and activation boundaries,
  then reaches the identical final config. A post-delivery transport fault also
  drops one commitment response only after the receiver durably commits it;
  the sender reports one failed attempt, retries the exact request, completes in
  46 rather than 45 deliveries, and every database retains exactly three
  current-ceremony commitments. Existing focused tests cover offline partition
  recovery, exact duplicates, invalid shares, conflicting reveals, transcript
  mismatch, partial activation, and explicit abort/replacement. A deterministic
  slow-member case now delays one commitment delivery beyond a 20ms peer
  deadline: both fast members persist immediately, only the slow member is
  reported failed, and the next retry catches it up to the identical final
  config. The final SQLite gate installs a disposable engine-level abort trigger
  on the sequence-watermark write. The commitment insert succeeds first in the
  same transaction, the second write fails, explicit rollback leaves neither
  commitment nor sequence durable, and removing the trigger lets the exact
  retry commit both. This exercises the same atomic rollback/recovery boundary
  required for storage exhaustion without a production fault switch. The full
  focused gate is 50/50 Axum DKG tests and eight SQLite DKG persistence/failure
  tests; no test or error output contains scalar material.

Fedimint DKG comparison and source references:

- [Fedimint G2 DKG state machine](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/config/dkg_g2.rs)
  (`fedimint-server/src/config/dkg_g2.rs` in the local Fedimint checkout): use as
  the main reference for commit-before-reveal, event-driven phase progression,
  immediate incoming-share verification, scalar-polynomial Horner evaluation,
  and aggregating public polynomial coefficients once all shares arrive.
- [Fedimint DKG peer-handle integration](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/config/dkg.rs)
  (`fedimint-server/src/config/dkg.rs` locally): compare the small state-machine
  boundary and transport-independent DKG API with CDK's planner/REST split.
- [Fedimint distributed setup orchestration](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/config/mod.rs)
  (`fedimint-server/src/config/mod.rs` locally): use the wait-for-all-P2P-
  connections barrier and explicit setup transition as references, while
  retaining CDK's simpler signed-REST transport when it satisfies the same
  safety and liveness requirements.
- [Fedimint large setup test](https://github.com/fedimint/fedimint/blob/master/scripts/tests/large-setup-test.sh)
  (`scripts/tests/large-setup-test.sh` locally): use as a reference for a
  bounded large-federation setup harness, not as code to copy blindly.
- [x] Document which Fedimint properties CDK adopts, simplifies, or deliberately
  rejects. CDK should prefer the smallest design that preserves commit-before-
  reveal, authenticated/confidential shares, immediate verification,
  deterministic final keys, restart safety, and bounded resource use.
- CDK **adopts** Fedimint's commit-before-reveal ordering, all-participant
  dealer requirement, immediate polynomial verification of received shares,
  deterministic aggregate public derivation, all-peer final-config equality
  barrier, and fail-closed setup transition.
- CDK **simplifies** orchestration into a pure restartable planner plus signed
  private REST artifacts persisted in the existing mint database. It uses the
  configured identity keys for request authentication and final-config
  confirmations instead of introducing a separate setup P2P identity or live
  checksum exchange, and uses the existing generated federation roster rather
  than Fedimint's setup UI state machine.
- CDK **deliberately rejects** threshold-only dealer completion, unsigned or
  plaintext non-loopback secret delivery, automatic roster shrinkage, durable
  volatile delivery acknowledgements, mandatory iroh/P2P transport, a second
  consensus round for setup errors, and speculative retry/backoff complexity.
  Explicit ceremony replacement retains failed history and must pass the full
  new roster's readiness and activation barriers.
- Fedimint exchanges and compares every peer's consensus-config checksum before
  config generation returns. CDK adopts that all-peer checksum barrier in a
  restart-safe REST form: identity-signed final-config-digest confirmations are
  durable and revalidated at startup instead of relying on one live P2P call.

Tests and success criteria:

- [x] The quick harness reports a complete phase breakdown in minutes and can
  compare two implementations without running wallet, AlephBFT, or full
  workspace tests.
- [x] The release 20-of-14 eight-denomination ceremony completes within a
  documented, repeatable budget without sustained event-loop starvation,
  self-request timeouts, unbounded retries, or divergent final transcripts.
  Set the numeric budget only after recording the first controlled release
  baseline and machine-independent relative improvements.
- [x] Axum remains responsive throughout CPU-heavy generation and derivation;
  bounded health/private-status probes complete within their documented timeout
  while the ceremony advances.
- [x] Every retained optimization has before/after evidence and every rejected
  candidate is checked off with its disproving measurement. No candidate is
  retained solely because a microbenchmark improves while the process ceremony
  regresses.
- [x] Optimized and reference derivations produce identical aggregate keys,
  public shares, member secret/public correspondence, keyset IDs, public
  ceremony, and transcript hash over randomized 5-of-3, 11-of-8, and 20-of-14
  fixtures.
- [x] Invalid-share, conflicting-record, cross-ceremony, wrong-roster,
  policy-mismatch, duplicate-idempotency, restart/resume, and transcript-
  signature tests remain unchanged or stronger.
- [x] Standalone and trusted-dealer behavior remains unaffected, and federation
  mode remains v3/BLS-only.
- [x] The benchmark harness, benchmark observations, accepted/rejected decisions,
  focused test commands, and remaining production risks are documented before
  Milestone 20A is marked complete.

## Milestone 21: key rotation and federation upgrades

Objective: keep long-running federations maintainable.

Implementation checklist:

- [x] Add consensus operation `KeysetRotation`.
- [x] New aggregate keysets become active only after consensus acceptance.
- [x] Validate configured key shares against public shares.
- [x] Add an explicit rotation-time proof-of-possession gate before activating a new keyset.
- [x] Wallet sees NUT-02 keyset changes consistently across members.
- [x] Add wallet protocol version negotiation in public federation metadata;
  keep operation, consensus-item, private-REST, Aleph-unit, and catch-up versions
  local to their member-facing payloads.
- [x] Add migration plan for changing operation encoding versions.
- [x] Add member add/remove as a later, explicit governance operation. Do not include in MVP.

Tests and success criteria:

- [x] Wallet can mint with new active keyset after rotation.
- [x] Old proofs remain spendable.
- [x] Config mismatch validation exists for federation membership/configuration.
- [x] Add stale-upgrade consensus tests proving members with stale config refuse to join upgraded consensus.
- [x] Version fields and some version checks exist for federation data.
- [x] Add mixed-version federation tests proving unsupported major versions are rejected cleanly.

Verified setup-transcript rotation-lineage checkpoint:

- The signed DKG/trusted-dealer transcript is now treated as an immutable
  historical setup attestation rather than a mutable snapshot of current
  activation state. Validation reconstructs the exact setup-time keyset list
  from the signed payload and verifies its hash, member signatures, roster,
  threshold, ceremonies, aggregate keys, and public shares without weakening
  the original transcript check.
- A later consensus keyset rotation may deactivate setup keysets and append a
  separately validated keyset. Every setup keyset must remain present with the
  same ID, unit, fee, expiry, aggregate public keys, and member public shares;
  removal or immutable-material changes fail closed. Private member configs
  remain bound to the original setup transcript hash.
- `cargo test -p cdk-common --features federation trusted_dealer -- --test-threads=1`:
  all 12 trusted-dealer generation, transcript-tamper, private-binding, and
  rotation-lineage tests passed.
- `cargo test -p cdk-common --features federation dkg -- --test-threads=1`:
  all 29 dealer-free DKG, transcript, planner, and transport tests passed.
- `cargo test -p cdk --features federation wallet::federation::tests -- --test-threads=1`:
  all 84 wallet connector tests passed, including rotated fake-wallet/HTTP
  shares, stale-output mapping, and spending restored old-keyset proofs.

Operation encoding migration plan:

- Treat `wallet_protocol_version` as the public wallet compatibility gate. Treat
  operation-envelope, consensus-item, private-REST, Aleph-unit, and catch-up
  certificate versions as member-facing gates at the payloads they govern. Mint
  startup and wallet metadata must reject only a wallet protocol mismatch;
  member routes, consensus admission, replay, and catch-up reject incompatible
  embedded artifacts at their own boundary.
- Because federation has not been deployed, federation JSON must be strict: do
  not silently default a missing wallet protocol version or embedded member
  payload version, missing
  payment-observation policy, partial payment-observation contexts, implicit
  removed payment-observation operator-limit fields, missing invite versions, or old
  development aliases. Generate fresh configs instead of accepting old
  experimental shapes. Until a real federation deployment exists, remove old
  experimental federated Cashu compatibility code rather than maintaining it.
- For a future operation encoding change, introduce the new decoder and encoder behind a new `FEDERATION_OPERATION_VERSION` plus golden vectors before any federation advertises it. The operation hash transcript must remain domain-separated by the versioned envelope bytes so old and new encodings cannot collide.
- For a future consensus item encoding change, bump `FEDERATION_CONSENSUS_ITEM_VERSION`, add item-level admission tests for `Operation`, `ModuleFact`, `Default`, and any production BFT items, and require stale members to reject queue admission before dispatch.
- A wallet-protocol rollout must be all-or-nothing for a federation roster until
  dynamic upgrade governance exists: generate a new public config, require all
  members to load it, and keep old members halted. Member-only protocol changes
  do not alter wallet metadata, but mixed member payload versions still fail at
  private REST, consensus, or catch-up admission until every member is upgraded.
- Catch-up/replay is the last safety gate. Persisted journal entries and peer catch-up responses must be revalidated through the embedded operation envelope version and catch-up certificate version before updating local journal state or materialized mint state.
- Every encoding migration needs checked-in canonical vectors, stale-config startup tests, consensus item admission rejection tests, catch-up/replay rejection tests, and wallet metadata rejection tests before release.

## Milestone 22: observability and operator safety

Objective: make failures diagnosable without exposing private user data.

Implementation checklist:

- [x] Add admin/status reporting for consensus height, digests, lag, and peer state.
- [x] Add comprehensive metrics for consensus height, pending mempool items, accepted operations, rejected operations, signing share latency, aggregation failures, state digest mismatches, and peer connectivity.
- [x] Add structured logs with short operation hashes, not full secrets or PII.
- [x] Add admin command to compare state digests across members.
- [x] Add admin command to list pending payment observations by quote ID.
- [x] Add alert when a member is behind by more than configured sessions.
- [x] Add alert when too many invalid operations are submitted by one client IP or auth token.
- [x] Redact proof secrets, witnesses, invoices, local paths, and private keys from logs.

Tests and success criteria:

- [x] Log snapshot tests verify redaction of secrets and private config.
- [x] Metrics are emitted in fake regtest.
- [x] Digest mismatch alert fires in an injected-divergence test.

## Milestone 23: production cleanup and payment-backend isolation

Objective: remove MVP/dev scaffolding from production federation semantics before real-funds deployment.

### Refactoring and cleanup policy

Use the [Federated Cashu refactoring plan](./federated-cashu-refactoring-plan.md)
as the detailed source of refactoring instructions, architectural opportunities,
target module boundaries, safety invariants, test requirements, and suggested
commit checkpoints. Its ranked items are candidates to investigate, not
instructions to apply mechanically. The implementation may have changed since
an opportunity was recorded, and a locally elegant rewrite is not valuable if
it does not improve ownership, correctness, maintainability, performance, or
testability in the current code.

Before applying any refactoring-plan item:

1. Verify against the current branch that the described duplication, ownership
   problem, invalid state, performance cost, or review burden still exists.
2. Identify the invariant owner before and after the change, the exact files and
   APIs affected, and the standalone and federation behavior that must remain
   unchanged.
3. Capture a relevant baseline. Depending on the item, this can include lines of
   implementation code, duplicated branches, public API surface, dependency
   edges, compile time, focused test time, operation latency, idle CPU, memory,
   database queries/lock duration, network requests, retry volume, or failure
   recovery time. Line count alone is never sufficient evidence.
4. Define the expected impact and a concrete success threshold before editing.
   High-risk consensus, journal, catch-up, signing, and application-state changes
   require a behavior-preserving characterization test or fault-injection test
   before implementation.
5. Confirm that the proposed abstraction has one real owner and removes more
   lifecycle or decision duplication than it introduces. Reject or narrow the
   item when measurement does not justify its cost or blast radius.

After applying an item:

1. Re-run the baseline and the item's focused tests, then compare measured
   before/after results.
2. Prove that canonical bytes, operation IDs, accepted ordering, threshold
   decisions, state transitions, digests, error behavior, and standalone CDK
   behavior remain unchanged unless the checkpoint explicitly intends a tested
   change.
3. Record the measured impact and any residual risk in the commit, progress
   documentation, or an adjacent benchmark/review document.
4. Remove temporary instrumentation and speculative abstractions. Keep
   instrumentation only when it is low-overhead, generally useful production
   observability with tests.
5. Commit the refactor as its own reconstructable checkpoint, separate from
   unrelated features and protocol changes.

The refactoring plan's AlephBFT-only architecture and
`crates/cdk/src/mint/federation/` target layout are accepted project direction,
but their individual removal and move steps still require the same baseline,
focused verification, and impact report before they are applied.

Implementation checklist:

- [x] Ensure federation crates and production federation modules do not depend on, import, or semantically special-case `cdk-fake-wallet`.
- [x] Keep `FakeWallet` only as a normal CDK mint payment backend used by `cdk-mintd` for local development, smoke tests, and regtest.
- [x] Rename or replace any federation policy type, method, error, config field, or test helper whose production meaning is currently expressed as `FakeWallet`.
- [x] Remove the permissive payment-policy `Default`: general
  `FederationConfig::new` derives `MemberQuorum` from the consensus threshold,
  trusted-dealer/test setup opts into development single-observation finality
  by name, and mintd rejects missing policy fields or development policy without
  its explicit local override.
- [x] Reject production federation startup when a config relies on implicit fakewallet defaults, single-observation dev policies, or test-only payment assumptions.
- [x] Model deterministic single-observation behavior, if still needed for local tests, as an explicit dev/test payment observation policy rather than as fakewallet-specific federation logic.
- [x] Make federation minting and melting depend only on generic payment observations, quote state transitions, and existing CDK payment backend traits.
- [x] Make federated minting and melting use the mint's configured payment backend in the same way standalone mint mode does; the federation layer must not know whether the backend is CLN, LND, LNBits, LDK Node, fakewallet, or a future backend.
- [x] Derive payment observations from generic quote/payment state returned by the configured backend, not from backend-specific federation branches.
- [x] Split trusted-dealer federation key/config generation from local fakewallet regtest config generation.
- [x] Allow `cdk-mintd federation trusted-dealer` or its successor setup command to generate member identities, public federation config, private member configs, BLS shares, and aggregate keysets without enabling the `fakewallet` feature.
- [x] Keep fakewallet-specific member config generation in a dev/regtest wrapper command, generated smoke harness, or `just` target rather than in the key generation path.
- [x] Add federation config for peer sync interval, peer request timeout, operation submission timeout, catch-up limits, digest recheck attempts, digest recheck delay, catch-up certificates chunk size, and related retry policy.
- [x] Add wallet federation connector configuration for per-member fallback timeout, member request timeout, member request retries, quote-status polling timeout, and fanout concurrency.
- [x] Replace production reads from in-memory federation journal/runtime state with durable mint database reads for peer status, catch-up responses, state digests, and catch-up certificates.
- [x] On `cdk-mintd` startup, reconstruct federation runtime state from the database and replay unapplied accepted journal entries before serving signing shares or authoritative status.
- [x] Ensure a crash after consensus acceptance but before deterministic apply recovers from the database and converges to the same state digest.
- [x] Extract federation code and tests into logical submodules where it improves
  ownership and test focus. The current layout separates mint runtime,
  admission, issuance, quote/payment observation, keyset/restore, swap, melt,
  wallet validation, private REST peer client, sync loop, status, catch-up, DKG
  routes, and smoke/status test modules.
- Maintenance note: evaluate further splitting and architectural cleanup through
  the linked refactoring plan and its measurement policy. Refactoring candidates
  do not become correctness or release gates merely by being listed; each must
  demonstrate current applicability and measurable value before implementation.
- [x] Gate any remaining smoke/regtest-only federation helpers behind test-only code or explicit dev features.
- [x] Audit `cdk`, `cdk-common`, `cdk-axum`, `cdk-mintd`, and storage crates for accidental fakewallet coupling in federation code.
- [x] Audit generated example configs so fakewallet-backed development federation examples are clearly marked development-only and cannot be mistaken for production templates.
- [x] Audit timeout constants, fallback behavior, and member retry policies introduced for smoke tests, and make production-relevant values configurable.
- [x] Remove stale "scaffolding", "placeholder", and "MVP wiring" labels from production federation modules once the corresponding feature is implemented; keep such language only in explicit dev/test code.
- [x] Provide fakewallet federation smoke tooling through focused `cargo test`
  coverage with `just` targets as convenience wrappers. Smoke-operation argument
  tests cover invalid amounts, batch bounds, stateful scenario validation, and
  required wallet/private-config prerequisites before generated scripts reach
  networked member operations; `just fed-regtest-check` runs the fast
  smoke/setup test gate without starting member processes.
- [x] Remove or rewrite any comments, docs, metrics, or status labels that imply fakewallet is part of federation rather than a replaceable mint backend.
- [x] Confirm standalone fakewallet mint behavior remains unchanged outside federation mode.

Tests and success criteria:

- [x] `rg "cdk_fake_wallet|FakeWallet|fake_wallet|fake-wallet"` in production federation modules only finds test-only code, mintd backend setup, smoke tooling, or documentation that explicitly marks fakewallet as development-only.
- [x] A federation built with a real payment backend can start without enabling the `fakewallet` feature.
- [x] `cdk-mintd` can generate trusted-dealer federation configs with `federation,sqlite` features and without the `fakewallet` feature.
- [x] Federated mint and melt tests pass through the same federation payment-observation path with fakewallet and with a backend-agnostic mock payment backend.
- [x] A production-mode federation config without an explicit payment observation policy fails startup with a clear error.
- [x] A production-mode federation config with a dev/test single-observation policy fails startup unless an explicit development override is set.
- [x] Custom peer sync, catch-up, digest recheck, operation submission, and wallet fallback timeouts are honored in unit tests.
- [x] Restart tests prove accepted-but-unapplied federation operations are replayed from the database and converge to the same state digest.
- [x] Private REST status, catch-up, digest, and catch-up certificate tests read from durable database state rather than an in-memory journal scaffold.
- [x] Fake-wallet regtest still passes through the normal CDK payment backend interface.
- [x] Mint, melt, and swap federation tests still pass after fakewallet-specific naming is removed from federation semantics.
- [x] Module split/refactor keeps focused federation tests passing without behavioral changes. `cargo test -p cdk-common --features federation federation::consensus::tests -- --test-threads 1` passed 33 tests after extracting the consensus test module, `cargo test -p cdk --features federation runtime_config_ -- --test-threads 1` passed 23 tests after extracting the mint federation runtime/admission test module, `cargo test -p cdk --features federation mint::federation::tests::melt -- --test-threads 1` passed 22 tests after extracting the stateful melt test module, and `cargo test -p cdk --features federation mint::federation::tests::swap -- --test-threads 1` passed 12 tests after extracting the stateful swap test module.
- [x] Federation persistence is grouped into journal/runtime, post-consensus
  application, and DKG trait and SQL-module families. Complex journal and
  finalized-Aleph row shapes use one compile-time static SELECT definition and
  one shared decoder each. All 156 SQLite tests, including 22 federation
  persistence cases, plus 41 catch-up/restart tests and 21 authenticated DKG
  driver tests pass; the full
  PostgreSQL federation test target compiles, while live execution still needs
  a running local PostgreSQL test service.
- [x] Private local-status serialization has one owner in `cdk-common` transport.
  Axum derives and emits the shared wire record, while mintd imports it for
  validation and display without duplicate DTOs or compatibility defaults. All
  44 transport tests, ten focused status-route tests, and 42 mintd status
  command tests pass.
- [x] Stable local federation development artifacts are external templates:
  trusted-dealer and live-DKG Bash programs plus member, live-DKG, and
  PostgreSQL TOML templates live under `cdk-mintd/dev/federation`. Rust retains
  validated value generation, TOML encoding, shell quoting, secret-safe writes,
  and a fail-closed exact-marker renderer. Both raw and generated scripts pass
  Bash syntax checks, and all 16 setup tests pass with paths containing spaces.
- [x] `rg "scaffold|placeholder|MVP"` in production federation modules only finds intentional dev/test documentation or open future-work comments.
- [x] Standalone fakewallet mint tests remain unchanged and prove fakewallet is still available as a non-federation development backend.
- [x] Documentation states that federation is payment-backend agnostic and fakewallet is only a development backend for mintd/regtest.

## Milestone 24: security review checklist

Objective: make the design reviewable before mainnet use.

Implementation checklist:

- [x] Implement default threshold rules and validation.
- [x] Complete formal review of threshold choice and BFT assumptions.
- [x] Implement canonical JSON operation hashing.
- [x] Complete formal review of operation canonicalization and transcript binding, including the binary-encoding decision and dedicated SIG_ALL transcript behavior.
- [x] Complete final review of all paths that can call `blind_sign` or `blind_sign_share`.
- [x] Public handlers are wired through federation paths in federation mode.
- [x] Complete final security review proving no public endpoint can bypass consensus in federation mode.
- [x] Complete final review and/or tests for NUT-20 signature verification in federated batch issuance.
- [x] Complete final review and/or tests for P2PK, HTLC, and SIG_ALL transcript construction.
- [x] Review real payment observation policies.
- [x] Implement DB conflict keys and initial federation indexes.
- [x] Complete final DB conflict-key and index review after the signature-share schema and query-plan checks are done.
- [x] Review member identity authentication.
- [x] Implement size/admission limits for oversized consensus items.
- [x] Complete full DoS review for huge witnesses, huge batches, repeated invalid ops, slow consensus, and restore spam.

Tests and success criteria:

- [x] Operation IDs are tied to share generation in federation apply paths.
- [x] Ensure an independent reviewer can trace every signature share to an accepted operation hash, output index, keyset, member ID, accepted journal index, and consensus evidence after the signature-share persistence decision is finalized.
- [x] Add federation operation fuzz targets for operation decoding and request conversion.
- [x] Attack regression tests exist.
- [x] Re-run and record the full attack regression suite.
- [x] Code paths appear consensus-gated in federation mode.
- [x] Add executable source audit proving production Axum public handlers do not call direct signing helpers and preserve consensus-before-response ordering for mint, swap, and melt.
- [x] Complete final security-review signoff that no route signs directly in federation mode.

## Milestone 25: CLI wallet federation support

Objective: make `cdk-cli` usable as a federation-aware wallet, not only a consumer of the wallet library connector in tests or downstream applications.

Implementation checklist:

- [x] Enable federation support in `cdk-cli` dependencies behind a CLI
  `federation` feature.
- [x] Add a `cdk-cli` configuration path for the first federation wallet use
  case by accepting a federation public config file and persisting it as a
  public-metadata sidecar in the CLI work directory.
- [x] Add an explicit CLI import command for the first test path:
  `cdk-cli federation add --public-config <federation-public.json>` or
  `cdk-cli federation add --invite-code <cashufed...>`. The invite-code path
  decodes the same public federation config carried by `FederationInvite`, then
  reuses the existing registry validation and connector policy path.
- [x] Add URL-only federation discovery:
  `cdk-cli federation add --mint-url <member-public-url>`. The CLI fetches
  `<member-public-url>/v1/config`, performs full external config validation,
  requires the supplied URL to identify exactly one roster member, fetches
  `/v1/info` from that same member, and verifies matching federation ID, member
  ID, wallet protocol version, and config digest before mutating the registry. The
  discovery member URL becomes the default local wallet mint URL unless
  `--local-mint-url` is supplied. File and invite imports remain available for
  offline setup.
- [x] Define how the first CLI path identifies a federation wallet in local
  state and token workflows: it maps the federation to the first member's public
  mint URL by default, with an override for the local wallet mint URL. A later
  logical federation URL/alias should still be revisited before user-facing
  deployments to avoid confusing token identity semantics.
- [x] Teach CLI wallet construction to build a `FederatedMintConnector` when
  federation wallet mode is configured, while keeping the existing single-mint
  HTTP connector path unchanged for standalone wallets.
- [x] Wire CLI wallet lookup and `get_or_create_wallet` flows so they detect
  stored federation metadata and inject the `FederatedMintConnector` instead of
  the normal single-mint HTTP connector.
- [x] Persist enough federation metadata in the CLI wallet state to reopen the
  same wallet without repeatedly passing the full config, without storing member
  private configs or mint-side private material.
- [x] CLI federation wallet activation reuses one constructed federated wallet
  connector across all active units in an imported federation during a single
  command process. This avoids rebuilding the same member connector set for
  multi-unit federations during startup activation while keeping the persisted
  registry JSON unchanged. Focused coverage proves `Sat` and `Usd` wallets
  activated from one imported federation share the same connector `Arc`.
- [x] Wire every `cdk-cli` command through the federation connector when
  federation mode is active, or classify it explicitly as local-only or
  standalone-only. The command-by-command audit is recorded in
  `docs/federated-cashu-cli-routing-audit.md`. NpubCash and explicit transfer
  wallet lookup now resolve imported member aliases through
  `get_or_create_wallet`; CAT login, CAT device login, blind-auth minting, and
  mint-URL mutation reject imported federations before direct member transport
  or local mutation. Pure decode commands skip eager federation activation and
  wallet recovery. Repository startup also reconstructs wallets from persisted
  mint metadata instead of making a direct `/v1/info` request before federation
  connector activation.
- [x] Add all CLI options for wallet-side federation request policy. The CLI
  import path now exposes member fallback timeout, member request timeout,
  quote-status timeout, retry count, fanout concurrency, mint-quote visibility
  controls, and `--quote-coordinator-member-id` for the first quote-creation
  attempt. The persisted policy is validated against the public federation
  roster before the wallet connector is constructed and does not influence
  status quorums or unrelated first-healthy calls.
- [x] Ensure CLI output remains compatible with normal Cashu tokens and proofs
  after threshold aggregation; the first CLI smoke returned an ordinary Cashu
  token after online send/swap, not member-share artifacts.
- [x] Validate federation mint-info metadata and v3/BLS aggregate keysets before
  CLI mint/swap/melt/restore fanout by using the wallet-library
  `FederatedMintConnector` validation path.
- [x] Make received-token handling resolve federation metadata from the token
  mint/federation identity, or return a clear actionable error when the CLI has
  not imported the corresponding federation config. The first path resolves
  tokens whose mint URL matches the imported local federation URL or any member
  public mint URL in the imported federation config. A focused receive test now
  proves an unimported token mint URL fails before receive/network work with an
  error that tells the user to run `cdk-cli federation add --mint-url
  <member-url>` for federated tokens or `--allow-untrusted` for standalone
  mints.
  A second focused receive test now proves a token whose mint URL is an imported
  member public URL reaches federation wallet resolution rather than the
  untrusted-mint gate, without needing live member HTTP.
- [x] Add clear CLI errors for missing federation metadata, mismatched member rosters, insufficient threshold responses, non-v3 aggregate keysets, stale invite/config versions, and all-members-unreachable cases. The receive path tells users to import the federation public config when a token mint URL is not trusted or recognized as an imported federation member. Focused CLI registry/import tests cover malformed registry JSON, malformed public-config JSON, stale registry versions, stale public-config wallet protocol versions, mismatched roster metadata, inactive keysets, and non-v3/BLS keysets before connector construction. Process smokes cover a reachable mismatched member, insufficient valid advertisements, missing federation mint-info metadata, all members unreachable, insufficient consensus threshold, and slow-member quote behavior without duplicate quote churn. Non-v3 raw outputs are intentionally unrepresentable in the CLI; crafted requests are rejected at the wallet pre-fanout and mint pre-consensus boundaries.
- [x] Document how to use `cdk-cli` against a locally generated federation from
  the regtest/smoke tooling for the first AlephBFT fakewallet smoke path.

Latest checkpoint:

- The first real CLI smoke passed against a five-member, threshold-three
  AlephBFT fakewallet federation. The smoke imported `federation-public.json`
  with `cdk-cli federation add`, minted 20 sat through the federation connector,
  performed an online `send`/swap for 7 sat and received the ordinary Cashu
  token back, melted a 10 sat BOLT11 invoice with 1 sat fee paid, verified the
  final CLI balance was 9 sat, and verified all five federation members were
  healthy and converged at 6 accepted operations with the same order digest and
  state digest. A follow-up CLI routing checkpoint now resolves imported
  federations by any member public mint URL, uses the federation connector for
  `mint-info`, and treats imported member URLs as trusted federation metadata
  during receive. The smoke is now scripted as `just fed-regtest-cli-smoke` and
  adds CLI `check-state`, generated fake invoices, copied-seed restore into a
  fresh CLI database, final balance verification, and final signed convergence
  status. A five-member threshold-three AlephBFT run passed with 6 accepted
  operations, matching order digest, and matching state digest. The follow-up
  no-change melt checkpoint extends `just fed-regtest-cli-smoke` with a
  separate exact-input wallet that imports the federation config, mints 11 sat,
  melts a 10 sat fake BOLT11 invoice with the 1 sat fakewallet fee reserve, and
  verifies `check-state` reports no remaining local proofs after the melt.
  The local federation start helpers now also launch member daemons with `nohup`
  and closed stdin so the short-lived recipe shell cannot terminate member
  processes before the CLI smoke reaches readiness polling. A later performance
  benchmark showed that some one-shot automation shells can still reap
  background benchmark members after the shell exits even without mintd panic
  logs; live timing runs should keep the harness shell alive or use the scripted
  smoke recipe for the duration of the benchmark. The mismatched-member
  CLI smoke target now starts four honest members plus one reachable member
  running with a different federation config, imports the honest public config
  with sequential fanout, and proves a 4-of-5 wallet can still mint using the
  valid member set while the honest members remain converged. A live run of
  `just fed-regtest-cli-smoke-mismatched-member` passed after minting 12 sat
  through the threshold-valid member set. The insufficient-valid-advertisements
  target now starts three honest members plus two reachable wrong-federation
  members for a 4-of-5 wallet, asserts `mint-info` fails with insufficient
  federation keyset advertisements, and verifies no local proofs exist. A live
  run passed with `required 4, received 3` and explicit member-ID mismatch
  diagnostics for the two wrong-federation members. The missing-metadata target
  now starts three honest members plus two reachable standalone fakewallet mints
  with federation disabled for a 4-of-5 wallet, asserts `mint-info` fails with
  insufficient federation keyset advertisements, and verifies no local proofs
  exist. A live run passed with `required 4, received 3` and explicit missing
  federation mint-info diagnostics for members 2 and 5. The follow-up
  `just fed-regtest-cli-smoke-offline` target now repeats the same CLI flow
  while member 5 is offline, restarts that member without rebuilding mintd,
  waits until the member's public API is reachable, and requires catch-up to the
  same 6 accepted operations plus matching order and state digests before
  passing. The `just fed-regtest-cli-smoke-insufficient-threshold` target now
  stops two members in the same five-member setup, verifies CLI minting fails
  while consensus threshold is unavailable, and checks that the wallet has no
  local proofs afterward. The CLI mint command now also exits with an error when
  its proof stream surfaces an error before completion, instead of silently
  reporting a zero-value receive. Focused CLI unit tests now also prove malformed
  registry/public-config JSON, stale registry/public-config wallet protocol versions,
  mismatched roster metadata, inactive keysets, and non-v3/BLS keysets are
  rejected before any federated connector is constructed. A repository-level CLI
  utility test now proves imported federation wallets are activated into the
  wallet repository, `get_or_create_wallet` maps both the local federation URL
  and imported member public URLs to a `FederatedMintConnector`, unsupported
  federation units fail before wallet creation, and unrelated mint URLs still use
  the standalone wallet path. A focused receive test now proves unimported
  federation-token mint URLs fail with an actionable import-config error before
  network receive work starts. The wallet connector and CLI registry persist
  and validate an explicit quote coordinator preference: the connector tries
  that member first only for quote creation, falls back to the rest of the
  validated roster when needed, and rejects coordinator IDs outside the
  federation roster. Mint, melt, and batch quote-status reads always require a
  matching threshold view; generic first-healthy calls use roster order.
  Focused tests passed with `cargo test -p cdk --features federation
  wallet::federation::tests -- --test-threads 1` and `cargo test -p cdk-cli
  --features federation federation -- --test-threads 1`. The next CLI receive
  checkpoint proves imported member public mint URLs resolve to the imported
  federation metadata path before wallet construction: `cargo test -p cdk-cli
  --features federation receive_token -- --test-threads 1` passed both the
  actionable error for an unimported federation and the imported-member URL
  resolution test. The invite-code CLI import
  checkpoint now accepts either `--public-config` or `--invite-code`, rejects
  bad invite codes before registry mutation, and passed `cargo test -p cdk-cli
  --features federation federation -- --test-threads 1` with 27 focused tests.
  The slow-member CLI smoke target now starts a five-member threshold-four
  AlephBFT fakewallet federation with member 5 configured for a 30-second
  fakewallet incoming-payment delay. A fast-coordinator wallet mints and forces
  a P2PK swap; a second wallet uses slow member 5 as quote coordinator, proves
  the configured delay behaviorally, consumes its four 1 sat proofs offline, and
  melts 10 sat plus the 1 sat reserve so change signing is mandatory. A live
  run on 2026-07-10 measured the P2PK swap at 3.55s, the delayed-coordinator
  mint at 37.60s, and the forced-change melt at 6.74s. All five members
  converged on the exact 10-operation sequence with matching digests, zero
  duplicate submissions, and no alerts, so fallback quote churn is excluded
  without depending on log text.
  A direct value-operation benchmark on 2026-07-09 then forced a P2PK swap and
  a melt with real change output rather than relying on denomination luck. The
  all-members-live swap took 4.71s and the change melt took 8.06s; the same
  paths with one member offline took 4.90s and 8.45s. The melt journal sequence
  was exactly `melt_quote -> swap -> melt`, the returned member caught up to
  the same 17-entry order/state digests, and a post-optimization swap took
  5.45s before all members converged on entry 18. The command-routing audit is
  now complete: every CLI command is classified as federation-aware,
  local-only, or standalone-only; NpubCash and transfer member aliases use the
  federation connector; and undefined per-mint auth/URL-mutation operations
  fail closed for imported federations.
  Command-level restore/check-state disagreement coverage is now complete,
  leaving a full external process variant optional rather than required for the
  CLI wiring invariant.

Scripted local CLI smoke command:

```bash
just fed-regtest-cli-smoke
just fed-regtest-cli-smoke-offline
just fed-regtest-cli-smoke-insufficient-threshold
just fed-regtest-cli-smoke-mismatched-member
just fed-regtest-cli-smoke-insufficient-valid-advertisements
just fed-regtest-cli-smoke-missing-metadata
just fed-regtest-cli-smoke-slow-member
```

Manual local CLI smoke command shape:

```bash
CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS=60 \
just fed-regtest-start target/cdk-cli-fed-smoke-aleph-live 5 3 62705

target/debug/cdk-cli --work-dir target/cdk-cli-fed-smoke-wallet --non-interactive \
  federation add --mint-url http://127.0.0.1:62705 \
  --quote-coordinator-member-id 1 \
  --member-request-timeout-secs 60 \
  --member-fallback-timeout-secs 60 \
  --quote-status-member-timeout-secs 60

target/debug/cdk-cli --work-dir target/cdk-cli-fed-smoke-wallet --non-interactive --unit sat \
  mint http://127.0.0.1:62705 20 --wait-duration 90

target/debug/cdk-cli --work-dir target/cdk-cli-fed-smoke-wallet --non-interactive --unit sat \
  send --mint-url http://127.0.0.1:62705 --amount 7

target/debug/cdk-cli --work-dir target/cdk-cli-fed-smoke-wallet --non-interactive --unit sat \
  receive <token-from-send>

target/debug/cdk-cli --work-dir target/cdk-cli-fed-smoke-wallet --non-interactive --unit sat \
  check-state --token <token-from-send>

target/debug/cdk-mintd federation fake-invoice --amount-msat 10000

target/debug/cdk-cli --work-dir target/cdk-cli-fed-smoke-wallet --non-interactive --unit sat \
  melt --mint-url http://127.0.0.1:62705 --method bolt11 --invoice <generated-fake-invoice>

cp target/cdk-cli-fed-smoke-wallet/seed target/cdk-cli-fed-smoke-wallet-restore/seed
target/debug/cdk-cli --work-dir target/cdk-cli-fed-smoke-wallet-restore --non-interactive --unit sat \
  restore http://127.0.0.1:62705
```

Tests and success criteria:

- [x] CLI unit tests prove federation wallet public-config and registry parsing
  accepts a valid config and rejects malformed JSON, stale registry versions,
  stale public-config wallet protocol versions, mismatched rosters, inactive keysets,
  and non-v3/BLS federation metadata before constructing a wallet connector.
  Focused tests also cover valid public config persistence, member public mint
  URL resolution, connector construction, explicit quote-coordinator policy
  persistence, unknown coordinator rejection, and CLI parse shape.
- [x] CLI invite-code import parsing tests prove `cdk-cli federation add
  --invite-code <cashufed...>` reaches the federation add command, a valid
  `FederationInvite` decodes to the expected public config, and malformed
  invite codes fail closed before registry mutation.
- [x] Public discovery route coverage proves unauthenticated `GET /v1/config`
  returns the exact finalized `FederationConfig`, sets `Cache-Control: no-store`,
  and serializes no identity-secret or BLS secret-share fields. CLI validation
  tests accept a fully matching member, reject a config that does not contain
  the supplied URL, and reject mismatched member config-digest metadata. CLI
  parsing covers `federation add --mint-url <member-public-url>`, whose discovery
  implementation requests `/v1/config` and `/v1/info` from that member.
- [x] CLI command tests prove `cdk-cli federation add --public-config
  <federation-public.json>` stores only public federation metadata and never
  persists member private configs or mint-side private material.
- [x] CLI wallet construction tests prove wallet lookup and
  `get_or_create_wallet` choose `FederatedMintConnector` for imported
  federations and the existing HTTP connector for standalone mints. Current
  focused tests cover connector construction, registry activation into an
  in-memory wallet repository, local federation URL lookup, imported member
  public URL lookup, unsupported federation unit rejection, and standalone URL
  fallback.
- [x] CLI integration or command-level tests prove `cdk-cli` can mint through a
  local federation and receive aggregate-verifiable ordinary Cashu proofs.
- [x] CLI integration or command-level tests prove `cdk-cli` can send/swap
  existing federated proofs and cannot aggregate shares for a conflicting or
  sliding-window output set. Live process smoke and the command-level P2PK send
  regression exercise the real CLI swap path and aggregate ordinary tokens.
  The honest CLI constructs one canonical `SwapRequest`, so per-member sliding
  windows are not representable at the command layer; malicious-client
  coverage lives at the actual trust boundaries in
  `local_federated_members_reject_sliding_window_swap_output_attack`,
  `apply_federation_swap_rejects_sliding_window_input_reuse_attack`, and the
  concurrent conflicting-output Axum regression.
- [x] CLI integration or command-level tests prove `cdk-cli` can melt through a
  local federation, including the no-change case and a change-output case with
  aggregate-verifiable change proofs. Current live smoke proves the
  change-output path on the main wallet and the no-change exact-input path on a
  separate wallet in `just fed-regtest-cli-smoke`.
- [x] CLI integration or command-level tests prove `cdk-cli` restore and check-state commands use threshold member responses and reject mismatched member state when no threshold agreement exists. `command_rejects_restore_without_threshold_member_agreement` and `command_rejects_check_state_without_threshold_member_agreement` install a real `FederatedMintConnector` with validated v3 keysets and member mint-info into a real CLI wallet repository, invoke the actual command functions, and require the explicit required-two no-quorum errors for three distinct member views.
  `cargo test -p cdk-cli --no-default-features --features federation --
  --test-threads 1` passed all 28 tests on 2026-07-10. The default-feature
  run passed 28 of 29; the only failure was the pre-existing npubcash
  `set_mint_keeps_local_active_mint_when_server_update_fails` timeout, unrelated
  to federation state handling. CLI-only clippy passed after allowing that
  crate's pre-existing `match_like_matches_macro` warning in startup routing.
  Current scripted smoke proves happy-path local proof check-state, spent-token
  check-state after receive, and copied-seed restore through the federation
  connector; the command-level tests now cover mismatched/no-threshold member
  views for both restore and check-state.
- [x] CLI receive tests prove received tokens resolve to imported federation
  metadata, and missing federation metadata produces a clear error instead of
  silently treating the token as a standalone mint token. Current live smoke
  proves receive when the token mint URL matches the imported federation URL,
  and focused registry coverage proves imported member public URLs resolve to
  the same federation metadata before wallet construction. Focused receive
  coverage now proves a token from an unimported mint URL fails with an explicit
  `cdk-cli federation add --public-config <path>` hint before network receive
  work starts, and a token from an imported member public URL reaches federation
  wallet resolution instead of the untrusted-mint gate.
- [x] Add a fakewallet federation CLI smoke test that starts N local federation
  members, imports `federation-public.json` into `cdk-cli`, mints with fake paid
  quotes, performs send/swap, melts through the fake successful backend, and
  verifies balances and aggregate-verifiable proofs. The smoke is available as
  `just fed-regtest-cli-smoke` and now also asserts local proof check-state,
  spent-token check-state, generated fake-invoice melt, copied-seed restore, and
  final signed convergence status.
- [x] Repeat the fakewallet CLI smoke with one member stopped when the configured member count and thresholds can still make progress. `just fed-regtest-cli-smoke-offline` now stops one member during CLI mint, check-state, send/receive, spent-token check-state, melt, balance, and copied-seed restore, then restarts it and requires final signed convergence.
- [x] CLI tests prove one offline member, one slow member, or one invalid-share member is tolerated when the configured threshold remains reachable. Process smokes prove one-offline-member restart/catch-up and a behaviorally observed 30-second incoming-payment delay across threshold mint, forced swap, and forced-change melt with exact all-member convergence. `command_send_waits_for_valid_threshold_after_invalid_share` forces a wrong signer share to arrive before one delayed honest share, asserts the invalid response was served, and proves the actual CLI P2PK send succeeds with the later valid threshold and correct remaining balance.
- [x] CLI tests prove insufficient threshold fails before wallet fanout can produce spendable proofs. `just fed-regtest-cli-smoke-insufficient-threshold` stops two members, expects CLI mint quote creation to fail, rejects any output claiming minted or received positive value, and verifies `check-state` finds no local proofs.
- [x] CLI tests prove missing federation mint-info metadata, mismatched aggregate keysets, and non-v3 output requests fail before wallet fanout can produce spendable proofs. Process targets cover reachable mismatched and missing-metadata members, insufficient valid advertisements, and all members unreachable. CLI import rejects non-v3/BLS configs before connector construction. Because CLI commands expose no raw keyset/output selector, all normal outputs derive from the validated v3 aggregate keyset; crafted non-v3 mint, batch mint, swap, restore, and melt-change requests are covered directly at the wallet pre-fanout boundary, with matching mint pre-consensus tests and a passing standalone legacy-keyset regression.
- [x] First-test success criterion: a developer can run generated fakewallet
  federation mints and complete `mint -> send/swap -> melt` using only
  `cdk-cli`, without calling the Rust wallet connector test harness directly.
- [x] Existing standalone `cdk-cli` tests keep passing with federation disabled
  and do not require federation config.

## Milestone 26: unified federated wallet errors and recovery

Objective: make federation-mode wallet and CLI error behavior match the existing
standalone Cashu behavior whenever consensus has decided an ordinary Cashu
business outcome. Reuse existing wallet error types, saga recovery, and public
Cashu error codes rather than creating a parallel federation-specific business
error system.

Implementation checklist:

- [x] Keep the existing CDK error model as the single authority for error
  classification and wallet behavior. Do not add a federation-specific error
  whitelist, definitive-error classifier, or parallel business-error hierarchy.
- [x] Extend the existing federation journal entry's application state with
  `rejection: Option<ErrorResponse>`. Do not create a separate result journal,
  rejection table, consensus item, or AlephBFT round, and do not use successful
  `completed_operations` rows to store rejected operations.
- [x] Enforce one persisted-state invariant: a deterministic application
  rejection carries its standard Cashu `ErrorResponse`, while `Accepted`,
  `Applying`, `Applied`, and `Failed` cannot carry one. A generic journal-level
  conflict may remain `Rejected` with no business response. Persist application
  rejection status and response atomically through one operation-state
  transition.
- [x] When the existing mint logic durably rejects an ordered operation, store
  the `ErrorResponse` that the standalone public API would return. A replay or
  duplicate request must return that same response instead of reducing it to
  the generic `RejectedOperation` error.
- [x] Reuse the existing `Error -> ErrorResponse` and `ErrorResponse -> Error`
  conversions unchanged. This includes errors such as `TokenAlreadySpent`,
  `TokenPending`, `BlindedMessageAlreadySigned`, quote and payment errors,
  keyset errors, balance errors, duplicate inputs/outputs, and witness or
  authentication errors whenever the existing non-federated path produces them.
- [x] Treat the rejection as post-consensus application metadata, not as part of
  the immutable operation envelope or original ordering hash. Keep it
  deterministic and include it in existing catch-up data, materialized-state
  digests, checkpoints, and explicit catch-up application-outcome verification.
- [x] Do not increase mint-to-mint consensus traffic for error handling. Wallet
  fanout may wait for threshold agreement using its existing member requests,
  but rejection persistence must not submit another consensus operation.
- [x] Keep federation liveness and consistency conditions distinct from Cashu
  business errors: pending AlephBFT finalization, unavailable quorum, catch-up
  gates, timeouts, HTTP `429`/`5xx`, invalid shares, mismatched member views,
  insufficient threshold responses, and non-terminal apply failures remain
  retryable or ambiguous and do not receive a persisted business rejection.
- [x] Preserve the existing `OperationPendingFinalization` to pending/retry
  mapping. Do not reinterpret a timeout or unavailable member as a rejected
  mint, swap, receive, or melt operation.
- [x] Update `FederatedMintConnector` fanout collection so it returns an
  existing wallet error when a signing threshold returns the same persisted
  `ErrorResponse`, aggregates threshold shares when successful, and
  returns an ambiguous federation-consistency error when neither outcome has
  threshold agreement.
- [x] Do not allow a single stale, faulty, or malicious member response to
  determine a consensus-dependent definitive wallet error. Preserve the normal
  direct error path for validation and authentication failures that occur before
  consensus, and preserve existing quote-creation fallback behavior.
- [x] Preserve the existing public `ErrorResponse -> Error` route for mint,
  swap, melt, and restore. Private member-to-member REST failures remain
  internal transport errors and must not become wallet-facing Cashu errors.
- [x] Reuse existing wallet saga recovery rules without federation-only copies:
  definitive business errors may compensate when the standalone saga already
  does so; pending or ambiguous errors retain recoverable state and use the
  existing replay, check-state, quote-status, and restore paths.
- [x] Keep the CLI thin: it should surface the wallet's typed error and exit
  nonzero, with clear text for pending/retryable federation state, rather than
  reimplementing mint, swap, receive, or melt recovery policy.
- [x] Keep standalone mint, wallet, and CLI error behavior unchanged.

Tests and success criteria:

- [x] Add end-to-end federation connector and Axum tests showing a threshold
  `TokenAlreadySpent` rejection during receive/swap reaches
  `Error::TokenAlreadySpent`, while a spent proof caused by this wallet's own
  interrupted receive follows the existing restore-based recovery path.
- [x] Add focused round-trip tests for representative mint, swap/receive, and
  melt business errors. Batch mint uses the same mint collection path, and melt
  change remains part of the melt response rather than a separate error system.
- [x] Add generic connector tests for threshold success, threshold matching
  rejection, mixed member errors, insufficient agreement, timeout, and invalid
  shares. Conflicting responses must remain ambiguous without producing
  spendable proofs or prematurely releasing local proofs.
- [x] Add tests for `TokenPending`, pending finalization, timeout, unavailable
  quorum, catch-up, and HTTP `429`/`5xx` that prove the existing sagas retain
  recoverable state and do not classify them as definitive failures.
- [x] Add only the saga bridge tests needed to prove the connector's ordinary
  CDK errors enter the existing receive, mint, swap, and melt recovery paths;
  do not duplicate the standalone saga test matrix with federation-only copies.
- [x] Preserve restore and check-state threshold-view behavior and test that
  mismatched member views remain ambiguous rather than inventing business
  errors for these read paths.
- [x] Add one CLI smoke test for a consensus-decided business error and one for
  an ambiguous federation condition. Re-run relevant standalone tests with
  federation disabled to prove existing behavior is unchanged.

Verified implementation result:

- `FederationJournalEntry` now stores `rejection: Option<ErrorResponse>` as
  application metadata. SQLite and Postgres migrations add the nullable JSON
  column; the shared database contract verifies atomic `Rejected` plus response
  persistence and round-trip reads. Ordinary status updates clear stale
  rejection data, and journal validation rejects a payload on a non-rejected
  state.
- Deterministic apply failures use the unchanged `Error::is_definitive_failure`
  decision, persist the unchanged `Error -> ErrorResponse` result, and return
  through the unchanged `ErrorResponse -> Error` conversion. Replays and
  duplicates return the stored error; rejected operations never enter
  `completed_operations`.
- Rejection metadata does not alter operation IDs or the order digest. It does
  alter materialized-state digests, is restored into runtime
  state, and is compared exactly during overlap and post-apply catch-up
  verification.
- Signing fanout now retains member failures. Mint/swap and melt return valid
  aggregate responses at signature threshold, return one existing Cashu error
  only at exact response threshold, and otherwise return ambiguous
  `InvalidMintResponse`. Federation member HTTP clients preserve the original
  4xx response for this comparison; same-code/different-detail responses do not
  combine, and structured HTTP 429/5xx responses remain infrastructure errors.
  Quote visibility, restore, and check-state retain their established
  threshold-view and fallback behavior.
- Verification passed: 363 `cdk-common` tests, 81 complete federation wallet
  tests, 153 `cdk-sqlite` tests, four receive recovery tests, and all 436
  federation-enabled Axum tests (434 sandbox-compatible plus the two real
  loopback-listener tests with network permission). A fresh
  five-member/threshold-three live AlephBFT CLI smoke returned
  `Token Already Spent` on repeated receive and then
  completed melt, restore, balance, and full convergence. The paired
  insufficient-threshold smoke timed out ambiguously and produced no proofs.

## Post-review critical restart revalidation

- [x] Preserve the materialized-state digest authenticated by each verified
  catch-up certificate quorum and carry the latest digest across a fully
  contiguous certified prefix.
- [x] Keep startup fail-closed while loading the journal, verifying certificate
  coverage, projecting local materialized state, and comparing the recomputed
  digest with certified evidence. Publish one final status only if no newer
  readiness transition superseded the startup check.
- [x] Prove matching evidence opens Healthy and a valid quorum for a different
  digest remains `MaterializedStateMismatch` while submission, restore, and
  check-state stay blocked. Seven focused shared-core certificate tests, 18
  Axum startup tests, five Axum quorum tests, all 28 standalone Axum tests, and
  the no-default-features `cdk-common` check pass.

## Post-review pending-admission atomicity

- [x] Reserve the bounded response waiter after deterministic admission but
  before durable pending persistence. Reuse the existing RAII waiter so
  persistence failure or cancellation releases the reservation automatically.
- [x] Publish the prepared consensus clone only after the pending row commits,
  with no fallible await between durable commit and runtime publication.
- [x] Prove waiter exhaustion and injected persistence failure leave neither a
  durable row nor queued operation, exact retry succeeds once, and the existing
  pre-backup restart path recovers durable pending work without resubmission.
  All seven focused submission tests and all 28 standalone Axum tests pass.

## Post-review melt-observation concurrency

- [x] Remove the federation-wide melt observer mutex so backend and consensus
  latency for one quote cannot block unrelated quote IDs.
- [x] Use a quote-scoped RAII active-payment guard for the local in-flight
  window and remove it on success, error, or cancellation.
- [x] Make the saga transition the authoritative initiation claim: atomically
  compare-and-set `SetupComplete -> PaymentAttempted`, and allow only the
  durable winner to call the payment backend. Keep standalone melt behavior
  unchanged.
- [x] Prove a concurrent same-quote duplicate cannot make a second payment and
  a second quote reaches its backend while the first backend call remains
  blocked. The generic saga database claim regression also passes.

## Post-review session-rotation pending recovery

- [x] Make durable active-session advancement and older pending-row rebinding
  one transaction. Fully decode every pending envelope, verify its stored
  operation ID, and fail closed on a row from a future session.
- [x] Serialize pending insertion against the same marker row, validate the
  operation ID before insertion, and reject stale-session records.
- [x] Run the atomic session advance before startup selects pending operations,
  so an advanced marker plus old-session rows is repaired and queued in the
  same boot.
- [x] Keep queued and dispatched-but-unfinalized consensus items across natural
  rotation, and do not publish the next live session before durable session
  advancement commits.
- [x] Prove transaction rollback preserves the old marker and row bindings;
  prove an advanced-marker crash fixture rebinds, finalizes, and removes two
  pending operations without wallet resubmission.

## Post-review essential-task supervision and terminal teardown

- [x] Propagate non-retryable finalized-payload application errors to the
  session supervisor instead of skipping the payload and continuing Healthy.
- [x] Treat root, apply, and network-pump outcomes as generation-scoped
  essential-task events; distinguish normal root-coupled completion from an
  isolated exit or panic.
- [x] Route task failure, automatic-rotation failure, peer digest mismatch, and
  no-progress catch-up through one teardown that clears inbound admission,
  publishes a generic Halted reason, and stops/joins the matching supervisor.
- [x] Barrier publication of the complete supervisor and reject publication if
  a terminal halt advanced the monotonic halt epoch during setup.
- [x] Prove non-retryable apply and root/network failure halt fail-closed,
  normal rotation remains healthy, stale generation failure cannot kill the
  replacement, rotation failure leaves no resources, and peer-sync halt tears
  down a live generation.

## Final completion audit

Status: complete for the implementation plan's fakewallet federation scope.
Independent security review, real payment-backend validation, and multi-host
operator deployment drills remain release gates, not unchecked implementation
milestones.

- [x] Run the broad release test matrix: 379 `cdk-common` federation tests, 703
  `cdk` tests, 157 SQLite federation tests, 461 federation-enabled Axum tests,
  205 mintd tests, and 32 CLI federation tests all pass.
- [x] Verify standalone compatibility with federation disabled: mintd and CLI
  release builds pass, 28 standalone Axum tests pass, and Cashu passes 546 unit
  tests plus 11 doctests. Standalone keyset behavior remains unchanged.
- [x] Preserve precise restore/check-state threshold-view failures. When enough
  members reply but their views conflict, the connector now returns the
  existing matching-view validation error instead of incorrectly reporting
  too few responses; all 73 focused federation connector tests pass.
- [x] Run a fresh five-member, threshold-three dealer-free DKG process smoke.
  The ceremony survives an interrupted member, exports identical final public
  configurations, restarts every member under AlephBFT, and completes mint,
  swap, check-state, melt, and restore. The operation phase completed in 4.94
  seconds with converged final state.
- [x] Run a separate release CLI wallet against the restarted DKG federation.
  Federation import, mint-info, mint, send/receive swap, melt, restore, and
  balances succeed. An immediate post-operation readiness check correctly
  fails closed while catch-up certificate coverage is behind; after process restart,
  signed catch-up and full digest convergence complete in 7 seconds with no
  digest or invalid-operation alerts.
- [x] Keep the generated live-DKG harness valid when optional metrics are
  disabled. Metrics sampling now exits before touching metrics files unless
  capture is enabled, with generator regression coverage and the full 205-test
  mintd gate passing.

## MVP cut lines

MVP 1: local deterministic federation

- Trusted-dealer key shares.
- In-memory or single-process consensus.
- Fake wallet mint quote payment.
- Mint issuance and swap.
- Fake-wallet melt state-machine tests without networked member processes.
- Federated wallet connector aggregation.
- Sliding-window regression test.

MVP 2: networked development-backend federation

- Separate member processes.
- AlephBFT strict-order consensus and idempotent mint/swap/melt APIs from Milestone 4A.
- SQLite federation DB.
- Member restart and catch-up.
- Fake-wallet melt reserve, pending, failure, finalize, and change signing.
- Restore and check-state.
- Regtest tooling.

MVP 3: payments and production hardening

- Real payment observation policy.
- Production cleanup and payment-backend isolation.
- Postgres support.
- DKG.
- Key rotation.
- Full observability and security review.

## Initial implementation order

1. Add threshold BLS tests and types.
2. Add federation config and trusted-dealer generator.
3. Add canonical operation encoding and operation hash tests.
4. Add in-memory consensus and federation DB journal.
5. Refactor mint issue apply path.
6. Add federated signatory share storage.
7. Implement federated mint issuance end to end with fake wallet.
8. Implement federated wallet connector aggregation for mint.
9. Add sliding-window mint attack regression.
10. Refactor and implement federated swap.
11. Add sliding-window swap attack regression.
12. Add Milestone 4A AlephBFT strict-order consensus, idempotent federation APIs, and the cdk-mintd fake federation harness.
13. Add development-backend melt reserve, pending, failure, finalize, and change signing.
14. Add restore/check-state.
15. Add state convergence, catch-up certification, and database-equivalence gates.
16. Add network chaos, catch-up, and payment-observation recovery tests.
17. Add real payment observation policy and DKG.
18. Add production cleanup and payment-backend isolation.
19. Add CLI wallet federation support.
20. Add unified federated wallet error handling and saga recovery.
