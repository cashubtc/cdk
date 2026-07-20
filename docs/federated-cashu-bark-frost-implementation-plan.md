# Federated CDK Bark-FROST BOLT11 backend implementation plan

Date: 2026-07-19

Status: implementation in progress; Milestones 0-5 and 7-17 are complete.
Milestone 6 is locally complete but retains the dependency-publication gate
recorded below. Production incoming
quote preparation, observation, threshold claim, and settlement bridging now
exist; outgoing quotes, consensus planning, atomic reservation, exact Ark
execution, success/failure reconciliation, revocation, refresh, emergency exit,
and melt settlement bridging now exist. Accepted Bark state now replays through
the complete receive, send, revocation, refresh, and exit lifecycles before a
member is ready for more side effects. Operations tooling is complete; the
release gate remains in Milestone 18.

Reviewed Bark baseline: `frost-wallet` at `ff0c04457137fe7506ffcf361214ea0fb25d3999`

Related documents:

- `docs/federated-cashu-implementation-plan.md`
- `docs/federated-cashu-bdk-frost-implementation-plan.md`
- `bark:cdk-implementation-plan.md`

## Implementation progress

This table is the implementation ledger. A milestone is marked complete only
after its exit criteria and named verification have passed. The conventional
commit subject identifies the milestone commit without embedding repository- or
machine-specific metadata in this document.

| Milestone | Status | Completion evidence | Commit subject |
|---|---|---|---|
| 0. Protocol and threat model | Complete (2026-07-19) | Decision record below; targeted Bark cryptography tests and standalone CDK regression baseline passed | `docs: freeze federated Bark protocol model` |
| 1. Bark federation facade | Complete (2026-07-19) | Exact locator/import/send/receive APIs, deterministic custody sessions, stable replay results, two-clean-database and per-step restart tests, standalone and minimal-feature Bark checks passed | `feat: add explicit federation wallet facade` |
| 2. Bark wire and authorization | Complete (2026-07-19) | Versioned bounded encodings for FROST, virtual MuSig2, and threshold DH; typed accepted-operation references; exact transcript reconstruction and adversarial mutation tests passed | `feat: define canonical Bark custody wire` |
| 3. CDK Bark application | Complete (2026-07-19) | Frozen registration/locator adapter, v2 Bark-capable config, dual-DKG activation, sealed-root registration, 2-of-3 and 3-of-5 vectors, Bark byte-agreement and scope rejection tests passed | `feat: register Bark FROST application` |
| 4. Durable MuSig2 and DH | Complete (2026-07-19) | Protocol-separated durable ledgers, atomic attempt/nonces, restart-safe exact retries, write-failure matrix, transparent-crypto agreement, and ordinary-FROST coexistence passed | `feat: persist federated custody protocols` |
| 5. Custody transport | Complete (2026-07-19) | Signed bounded REST routes, durable participant handlers, timeout/backpressure-aware bounded collection, exact retries, stale/mixed-attempt rejection, and higher-attempt signer replacement passed | `feat: add federated Bark custody transport` |
| 6. `cdk-bark` skeleton | In progress (dependency publication gate) | Local construction, custody, redaction, feature-isolation, strict-clippy, and standalone regression gates pass; immutable Bark source is not remotely fetchable yet | `feat: add federated Bark runtime skeleton` (local-only progress commit) |
| 7. Bark operations | Complete (2026-07-19) | Nested canonical operation family, typed authorization, conflict/idempotency tests, result lookup, full common regression suite, and workspace compatibility passed | `feat: add typed Bark federation operations` |
| 8. SQL projection | Complete (2026-07-20) | New paired-only migrations, all 83 historical migrations byte-locked, deterministic action/VTXO projection, retry-state hardening, SQLite's 168 tests, strict changed-crate clippy, workspace checks, PostgreSQL lifecycle parity, and PostgreSQL pre-Bark forward upgrade passed | `test: verify Bark PostgreSQL projection parity` |
| 9. Receive secrets | Complete (2026-07-19) | Authenticated 4 KiB private transport, AES-GCM persistence-before-acknowledgement and crash recovery, threshold/actor signature checks, verified-checkpoint retention, encrypted backup/restore, and feature-isolation checks passed | `feat: protect federated Bark receive secrets` |
| 10. Incoming preparation | Complete (2026-07-19; public-Ark compatibility corrected 2026-07-20) | Consensus-first secret protection and actor failover, exact signed-invoice validation, stable NUT-19/payment-hash retry identity over Bark's original protocol, ordinary `MintQuote` reuse, focused CDK/Bark tests, and production feature wiring passed | `feat: complete federated Bark quote preparation` |
| 11. Incoming claim | Complete (2026-07-19) | Independent payment-hash polling, threshold-matched HTLC evidence, accepted exact claim plans, FROST/virtual-MuSig custody, durable preimage/result checkpoints, independently validated claimed VTXOs, two-layer result/payment quorums, and focused restart/adversarial tests passed | `feat: complete federated Bark incoming claims` |
| 12. Outgoing planning | Complete (2026-07-19) | Conservative accepted-projection quotes, exact deterministic plans, adversarial binding checks, atomic competing-reservation rollback, local Bark `Start` checkpoint reconstruction, full common/CDK regressions, and production feature builds passed | `feat: complete federated Bark outgoing planning` |
| 13. Outgoing execution | Complete (2026-07-19) | Accepted phase-at-a-time Ark execution, exact multi-input custody authorization, canonical HTLC/initiation/success transitions, restart replay, verified preimage proof, distinct-member result quorum, and existing melt-saga finalization passed | CDK: `feat: execute federated Bark outgoing payments`; Bark: `feat: add federated send phase execution` |
| 14. Failure recovery | Complete (2026-07-19) | Threshold failure evidence, exact accepted revocation, safe recovery observations, startup replay, amount conservation, and focused/full regression checks passed | CDK: `feat: recover failed federated Bark payments`; Bark: `feat: recover failed federated Bark payments` |
| 15. Refresh and exit | Complete (2026-07-19) | Earliest-expiry planning, accepted-plan custody gates, threshold trigger/confirmation evidence, BDK recovery allocation, replay-only startup reconstruction, focused/full regression checks, and production feature builds passed | CDK: `feat: bind federated Bark maintenance plans`, `feat: execute federated Bark maintenance`; Bark: `feat: add federated Bark maintenance primitives`, `feat: complete federated Bark maintenance execution` |
| 16. Restart and replay | Complete (2026-07-19) | Full-frontier readiness, durable-journal reconstruction, exact replay-only receive materialization, missing-secret fail-closed behavior, clean-database Bark rebuilds, 33 CDK restart/catch-up tests, and FROST nonce restart tests passed | CDK: `fix: complete federated Bark restart replay`; Bark: `feat: replay accepted federated Bark receives` |
| 17. Operations tooling | Complete (2026-07-19) | Bounded signed status, fixed-label metrics, authenticated replay-only reconcile and exact-action retry commands, targeted tests, and feature-isolation checks passed | `feat: add federated Bark operator tooling` |
| 18. Release gate | In progress (2026-07-20) | Feature-boundary, FROST adversarial suite, public-Ark protocol compatibility restoration, and focused Bark/CDK checks passed; database parity and full payment gates remain | `feat: checkpoint federated Bark release gate` (checkpoint) |

## Repository path notation

This document deliberately avoids machine-specific absolute paths.

- `cdk:<path>` means a path in the CDK repository.
- `bark:<path>` means a path in the Bark repository on the reviewed `frost-wallet` branch.

## Executive outcome

The integration is feasible, and the existing Bark branch has already completed the most invasive Bark-wide refactor: money-moving code now uses the signer-neutral `VtxoCustody` boundary instead of assuming a local seed-backed keypair. Its standard FROST, virtual MuSig2, and threshold-DH cryptographic tests pass for the reviewed branch.

It is not yet safe to connect that prototype directly to a CDK federation. The remaining blockers are architectural rather than cosmetic:

1. Bark participant nonces, completed shares, burned sessions, and coordinator MuSig state are process-local.
2. Bark has no complete bounded canonical wire contract for a remote participant transport.
3. Bark authorizes custody requests with an opaque byte vector and internally creates random session IDs that cannot be reconstructed from an accepted CDK operation.
4. Bark allocates VTXO indexes from a member-local database and mutates local locks and movements before federation consensus.
5. Incoming invoice creation creates the preimage, contacts the Ark server, and only then persists the action checkpoint. Loss of that member can make a paid invoice unclaimable.
6. Bark action checkpoints provide useful local crash recovery, but raw Bark SQLite state is neither deterministic consensus state nor safely replicable state.
7. Bark's current application derivation is not the same contract as CDK's registered `cdk-frost` application derivation. Exporting the CDK root share into Bark would violate the existing non-exporting root boundary.
8. Bark deducts a receive fee from the invoice amount before creating HTLC VTXOs. Issuing the nominal Cashu quote amount without accounting for that deduction would undercollateralize the mint.

The recommended design is therefore:

- add a new `cdk-bark` crate that owns all Bark-specific planning, validation, projection, execution, and `MintPayment` adaptation;
- keep the ordinary Cashu quote, mint, melt, swap, and saga logic unchanged;
- add one feature-gated, nested Bark operation family to the federation journal instead of a generic opaque plugin framework or many top-level CDK operation variants;
- add a small Bark-specific incoming BOLT11 route seam and a small Bark driver seam in the existing federation payment probes;
- implement Bark's `VtxoCustody` in `cdk-bark` using the existing CDK root share and an extended `cdk-frost` durable protocol ledger;
- use Bark's wallet and action state machines as a member-local execution engine after an exact plan has been accepted;
- treat the CDK journal and its deterministic Bark projection as authority, and treat each member's Bark SQLite database as a rebuildable execution cache;
- require a fresh Bark application registration and fresh Bark wallet database for the first release; and
- add only new forward migrations. Previously shipped standalone and federation migrations must never be edited.

This is intentionally similar to the federated BDK integration in authority and recovery semantics, but it does not force Ark's VTXO model into the on-chain UTXO/PSBT model.

## Integration constraints

These constraints are release requirements, not implementation preferences.

### Preserve Cashu behavior

- Public NUT-04 and NUT-05 BOLT11 request and response shapes remain unchanged.
- Standalone CLN, LND, LNbits, LDK Node, fake-wallet, and gRPC processor behavior remains unchanged.
- Existing `MintPayment` implementations remain unchanged unless a small additive trait clarification is unavoidable.
- Existing mint, melt, swap, proof-state, and saga rules remain the business authority.
- Federation-specific failures must not introduce new states or semantics for standalone mints.

### Preserve federation invariants

- Consensus accepts an exact payment plan before any threshold signature, preimage disclosure, VTXO reservation with external consequences, or Ark server payment side effect.
- No Ark server call, chain call, clock read, random selection, or local database observation occurs during deterministic apply.
- Actor failover changes the executor, not the accepted invoice, payment hash, VTXOs, derivation indexes, fees, Ark server, signer attempt, or recovery path.
- Backend observations are evidence. They become CDK payment authority only through accepted federation operations and the configured observation policy.
- Restart, replay, catch-up, checkpoint restore, and local Bark projection rebuild converge to the same public state.

### Preserve the root-share boundary

- The canonical `cdk-frost` root share remains sealed and non-exporting.
- No code path reconstructs the federation root secret or imports its plaintext share into `frost_federation_core::ParticipantPackage`.
- Bark is a separately registered application under the reusable root, with an independent application ID, key epoch, derivation version, authorization policy, and transcript domains.
- Ordinary BDK signing, Bark FROST signing, Bark virtual MuSig2, Bark threshold DH, and receive-secret acknowledgement use disjoint protocol domains and session namespaces.

### Keep the CDK change reviewable

- Bark protocol and runtime code lives in `cdk-bark` whenever it does not have to be in `cdk-common`, `cdk`, `cdk-axum`, or `cdk-mintd`.
- The federation journal receives one nested `FederationBarkOperation` family, not a large collection of unrelated top-level variants.
- CDK core stores stable federation-domain records, not serialized Bark implementation checkpoints.
- Do not add a dynamic payment-backend consensus plugin registry for one backend. A generic opaque payload would make deterministic semantic admission harder to audit and would expand the federation core more than an explicit nested Bark family.
- Bark runtime behavior and Bark dependencies are behind `federation-bark`; `federation` without Bark and all non-federation builds continue to compile and behave as before.
- Lightweight Bark federation wire/domain types may remain under `cdk-common/federation` so tools can decode the operation format without linking Bark. Admission of Bark operations still requires a Bark-enabled public config and runtime protocol version.

### Migration rule

Only new, forward-only migrations may be added.

- Do not modify any pre-existing SQLite migration.
- Do not modify any pre-existing PostgreSQL migration.
- Do not fold Bark tables into `20260712000000_create_federation_schema.sql` or any other historical migration.
- Add paired new SQLite and PostgreSQL migration files with a timestamp later than the current latest migration.
- Keep and extend checksum tests that prove historical standalone and federation migration bytes are unchanged.
- Test empty-database install and upgrade from the latest pre-Bark schema for both SQL backends.

## 1. Review scope and evidence

### CDK material reviewed

The review traced the following boundaries rather than relying only on the existing plans:

- reusable root, application derivation, sealed share, signing session, wire, and durable nonce code in `cdk:crates/cdk-frost/`;
- BIP32/BDK application configuration in `cdk:crates/cdk-frost-bip32/` and `cdk:crates/cdk-common/src/federation/frost_config.rs`;
- watch-only federated BDK construction and scoping in `cdk:crates/cdk-bdk/src/federation.rs`;
- the non-mutating BDK payment adapter in `cdk:crates/cdk-mintd/src/federated_bdk_backend.rs`;
- federation operation envelopes, conflict keys, journal application, payment observations, actor leases, convergence projections, checkpoints, and private transport in `cdk:crates/cdk-common/src/federation/`, `cdk:crates/cdk/src/mint/federation/`, and `cdk:crates/cdk-axum/src/federation/`;
- the federation-aware on-chain mint-quote seam in `cdk:crates/cdk-axum/src/custom_handlers.rs` and `cdk:crates/cdk/src/mint/issue/mod.rs`;
- Lightning backend configuration and construction in `cdk:crates/cdk-mintd/src/config.rs` and `cdk:crates/cdk-mintd/src/lib.rs`;
- federation database traits, SQL application code, materialized-state digest code, and migrations in `cdk:crates/cdk-common/src/database/mint/`, `cdk:crates/cdk-sql-common/src/mint/`, `cdk:crates/cdk-sqlite/`, and `cdk:crates/cdk-postgres/`.

### Bark material reviewed

The review covered the full `master..frost-wallet` change shape and the relevant implementation paths:

- `bark:bark/src/custody.rs`;
- `bark:bark-frost/src/{authorization,builder,coordinator,participant,transport}.rs`;
- `bark:frost-federation-core/src/`;
- wallet construction, key allocation, and key-source persistence in `bark:bark/src/lib.rs` and `bark:bark/src/persist/`;
- Lightning send and receive state machines in `bark:bark/src/actions/lightning/` and their public wrappers in `bark:bark/src/lightning/`;
- action checkpoint execution in `bark:bark/src/actions/mod.rs`;
- arkoor, round, board, offboard, unilateral-exit, mailbox, and VTXO signing paths;
- Ark server Lightning initiation, status, co-sign, receive, and claim paths in `bark:server/src/ln/`;
- the Bark-side integration analysis in `bark:cdk-implementation-plan.md`.

### Verification run

The following targeted Bark command passed on the reviewed revision:

```text
cargo test -p frost-federation-core -p bark-frost --locked
```

Results:

- `bark-frost`: 4 passed;
- `frost-federation-core`: 4 passed;
- doc tests: passed.

This verifies the current cryptographic prototype and in-process adapter tests. It does not verify durable participants, remote transport, CDK authorization, multi-process failover, or Ark/CDK end-to-end behavior because those facilities do not exist yet.

## 2. Existing architecture to preserve

### CDK federation authority

CDK currently separates four important concerns:

1. AlephBFT orders bounded, signed federation operations.
2. The federation application deterministically validates and applies those operations in one database transaction.
3. Side-effecting workers operate outside apply and report results through new operations or member observations.
4. Existing Cashu quotes, sagas, proof states, and signatures remain the public business model.

The Bark backend must fit this arrangement. It must not make Bark SQLite, a mailbox notification, or one member's Ark client response authoritative.

### Federated BDK pattern to reuse

The BDK integration provides the following useful pattern:

- derive a separate application from a reusable FROST root;
- keep the local wallet watch-only;
- allocate addresses and reserve spend state through accepted operations;
- authorize a signature only by reconstructing an accepted proposal;
- keep nonce material durable and participant-local;
- perform network and signing coordination outside deterministic apply;
- project accepted state independently on every member;
- expose a payment adapter that cannot accidentally perform standalone custody mutations.

The Bark implementation should copy these authority boundaries, not BDK's UTXO, descriptor, PSBT, fee-bump, or confirmation data model.

### Bark machinery to reuse

The Bark branch already provides valuable pieces:

- `VtxoCustody`, which removes local-key assumptions from the relevant signing paths;
- `KeySourceDescriptor`, which prevents opening a Frost wallet database under the wrong custody source;
- deterministic public VTXO key derivation through `VtxoKeyLocator`;
- ordinary BIP340 FROST signing;
- a virtual user MuSig2 partial produced by a threshold federation for Bark's outer user/server MuSig2;
- threshold DH with DLEQ verification for mailbox blinding;
- Lightning send and receive action state machines;
- durable SQLite action checkpoints, VTXO locks, movements, paid-invoice records, and settled-receive records;
- payment-hash-based Ark server lookup and retry behavior.

These should be hardened and exposed through a restricted federation facade, not reimplemented in CDK.

## 3. Detailed readiness findings

| ID | Finding | Consequence | Required disposition |
|---|---|---|---|
| B1 | `FrostParticipant` keeps pending/completed FROST and MuSig sessions and burned IDs in process-local maps. | A crash can lose a returned nonce commitment, lose retry data, or reopen a session unsafely. | Do not use it as CDK's production participant. Extend `cdk-frost` and reuse its durable store. |
| B2 | `FrostVtxoCustody` keeps pending coordinator MuSig sessions in a process-local map. | The coordinator cannot safely resume after a returned aggregate nonce or fail over to another member. | Make all coordinator inputs reconstructible from accepted state and durable participant replies; persist only rebuildable local execution state. |
| B3 | `ParticipantTransport` has only an in-process implementation, and several response types lack complete public validating wire APIs. | A production member-to-member transport would need unsafe ad hoc serialization or access to private fields. | Add bounded versioned canonical encodings and public decoders before remote use. |
| B4 | `BarkAuthorizationRequest.authorization` is an opaque `Vec<u8>`. | A participant cannot prove that a digest, locator, signer set, or phase belongs to one exact accepted CDK operation. | Replace it with a typed accepted-operation reference and transcript commitment. |
| B5 | Bark constructs `CustodySessionId::random()` inside many money-moving paths. | Consensus cannot pre-bind the session or distinguish retries and actor failover deterministically. | Inject deterministic session context derived from accepted operation ID, phase, object ordinal, signer set, and attempt. |
| B6 | The Bark coordinator hardcodes attempt `1`, uses a fixed initial signer subset, and calls participants sequentially. | One unavailable selected signer can stall progress, and retry/failover lineage is not explicit. | Use consensus-derived attempt numbers, deterministic eligible subsets, bounded parallel collection, and accepted lease rotation. |
| B7 | Bark's `peek_next_vtxo_public_key` and `derive_store_next_vtxo_public_key` advance a member-local frontier. | Members can choose different destination keys and collide indexes. | Add explicit `derive/reveal/store at locator` APIs and allocate indexes in consensus. |
| B8 | Lightning send selects and locks local VTXOs, allocates two keys, and creates a movement before any accepted plan. | Members can double-select VTXOs or execute a different payment plan. | Split planning from apply; accept exact inputs, outputs, keys, fee, and expiry before local locks or server I/O. |
| B9 | Lightning receive generates the preimage, contacts the Ark server, allocates a local key, and only then persists its checkpoint. | A crash can lose the only claim secret or destination allocation for a live invoice. | Persist and acknowledge protected receive-secret material and accept the intent before Ark invoice creation. |
| B10 | `WalletAction::advance` can mutate Bark state or call the server before the returned next checkpoint is persisted. | A crash can leave local state ahead of its checkpoint. | Reconstruct from the accepted CDK plan and Bark's existing payment-hash/artifact identity; tighten local Bark persistence and remain conservative where an RPC result is ambiguous. |
| B11 | Bark action checkpoints contain implementation-specific state and plaintext receive preimages. | Replicating checkpoints would couple consensus to Bark versions and leak secrets into public federation state. | Replicate stable semantic records only; keep checkpoints and secrets member-local. |
| B12 | Bark's operational `WalletSeed` still owns mailbox identity, database fingerprint, and ephemeral operational keys. | Sharing it would create an unnecessary common secret; keeping it local makes mailbox-only state differ. | Keep per-member operational seeds; make mailbox a notification source, not authority, and add explicit receive-secret recovery. |
| B13 | Bark's application/root derivation formula is independent of CDK's `ApplicationRegistration`. | The same root input does not imply identical Bark keys under the two implementations. | Freeze one CDK-owned Bark application derivation and add cross-implementation vectors. |
| B14 | `frost_federation_core::ParticipantPackage::import` needs a raw share, while `CanonicalRootKeyPackage` intentionally does not export one. | Directly constructing Bark participants would break the root isolation model. | Implement Bark `VtxoCustody` on top of `ParticipantKeyManager`; never export the share. |
| B15 | Bark needs ordinary FROST, virtual MuSig2, and threshold DH, while `cdk-frost` currently owns only ordinary FROST session persistence. | A second nonce manager would duplicate sensitive lifecycle logic. | Add protocol-separated MuSig2 and threshold-DH records to the same CDK participant ledger. |
| B16 | The Ark server mostly keys Lightning status by payment hash, but exact plan deduplication is not a complete federation contract. | Different actors could retry the same payment hash with different VTXO sets or output plans. | Keep action IDs and exact request commitments inside accepted CDK state; prove each original Bark RPC is safe for an identical exact retry, and retain `Pending`/`RecoveryRequired` when ambiguity cannot be resolved. Do not extend the public Ark protocol. |
| B17 | VTXOs expire and failed Lightning HTLCs require revocation or exit. | A backend that only sends and receives can still strand federation funds. | Include refresh, revocation, and emergency exit in the first production scope. |
| B18 | The reviewed Bark code is on an unreleased work branch. | An absolute path or floating branch dependency is not reproducible. | Upstream required APIs and consume a released crate or an explicitly reviewed immutable revision. |
| B19 | Receive fees are deducted from the gross invoice amount before HTLC VTXOs are granted. | Treating gross Lightning payment as net federation assets can issue more ecash than the Bark wallet receives. | Commit gross amount, receive fee, and minimum net VTXO value; require net accepted value to cover Cashu issuance. |
| B20 | Receive anti-DoS currently selects a local VTXO attestation opportunistically, uses an optional token, and may retry without either. | An empty federation wallet may be unable to claim its first receive, and local selection would bypass consensus. | Make anti-DoS policy explicit; reserve an exact attestation VTXO or configure a protected operator token/no-proof policy accepted by the Ark server. |

## 4. Goals

The first production release must:

- expose BOLT11 mint and melt support backed by one configured Ark server;
- custody every Bark VTXO under a threshold-derived Bark application key;
- reuse the federation's existing sealed FROST root share without exporting it;
- keep ordinary CDK Lightning backends and business logic intact;
- accept exact receive, send, claim, revocation, refresh, and exit plans before side effects;
- support deterministic member-local projection and VTXO reservation;
- support actor failover without changing a plan;
- survive participant, coordinator, mint, Bark SQLite, and network restarts;
- make incoming preimages recoverable by an eligible replacement set;
- use authenticated bounded member transport and durable protocol state;
- classify paid, failed, pending, and ambiguous states conservatively;
- integrate with existing mint/melt payment-observation quorum and saga finalization;
- provide startup validation, readiness, metrics, operator inspection, backups, and recovery procedures;
- run on SQLite and PostgreSQL federation databases; and
- retain CDK's MSRV and lint requirements.

## 5. Non-goals for the first production release

- General-purpose user-facing Bark wallet APIs through CDK.
- Arbitrary arkoor sends unrelated to Cashu payments.
- User-selected offboards through the BOLT11 backend.
- Multiple Ark servers in one Bark application epoch.
- Automatic Ark server migration.
- Root or Bark application key rotation.
- Migrating an existing seed-custodied Bark wallet into federation custody.
- Importing a database produced by the current experimental Bark derivation.
- A generic dynamic federation payment-backend plugin framework.
- Replicating raw Bark SQLite files or Bark action checkpoint JSON through consensus.
- Hiding incoming payment preimages from every individual member in v1. The recommended v1 scheme provides threshold availability but allows acknowledged members to know the preimage; this is an explicit griefing limitation, not a custody break.
- Running a trusted dealer in production. Dealer-backed examples remain test-only.
- BOLT12 in the initial integration. It may reuse the architecture after BOLT11 is complete.

## 6. Security and correctness requirements

### Custody

- Fewer than the configured signature threshold cannot sign a Bark asset key.
- No participant API signs an arbitrary digest without an accepted Bark authorization reference.
- The Ark server key and network are part of the immutable Bark application configuration.
- No local `WalletSeed` keypair fallback is reachable for VTXO custody in federation mode.
- Operational seeds, transport identities, sealed-storage keys, and Bark application shares have distinct purposes and domains.

### Consensus before action

- Receive intent is accepted before invoice RPC.
- Receive claim plan is accepted before claim signing or preimage disclosure.
- Send plan is accepted before input reservation is mirrored into Bark and before HTLC co-sign RPC.
- Failed-payment recovery is accepted before revocation signing.
- Refresh and emergency exit plans are accepted before signing or publication.
- A coordinator may only request transcripts derivable from the currently accepted plan and attempt.

### Deterministic apply

- Apply reads only the accepted operation, prior materialized state, immutable public config, and deterministic policy constants.
- Canonical VTXO and Ark artifact decoders reject malformed or non-canonical bytes.
- All vectors are bounded and canonically ordered.
- All amounts use checked arithmetic and explicit satoshi units.
- State transitions are monotonic, idempotent, and conflict-detecting.
- Terminal paid/failed transitions cannot be reversed by stale observations.

### External ambiguity

- A lost response is not treated as a failed payment.
- An outgoing payment becomes `Failed` only after the Ark payment is terminally failed and the federation has either restored spendable value through accepted revocation or committed an accepted recovery path.
- An incoming quote becomes paid only after the accepted Bark state and configured observation quorum prove the required value is claimable or settled under federation custody.
- A preimage-revealed receive remains pending/recovery-required until its claim or exit is accepted as terminal.

### Availability

- Actor leases rotate deterministically over accepted journal progress.
- Signer attempts and subsets are explicit and durable.
- A failed actor cannot force re-selection of VTXOs or keys.
- A clean member can rebuild its Bark public projection from accepted operations.
- A participant restart burns unsafe incomplete nonce material and can return cached identical completed shares.

### Balance and fee accounting

Bark's receive fee changes the usual Lightning-backend accounting. Define:

- `cashu_amount_sat`: the amount of ecash the quote may issue;
- `invoice_amount_sat`: the gross BOLT11 amount paid by the user;
- `receive_fee_sat`: the Ark/Bark receive fee calculated on the gross amount;
- `net_vtxo_amount_sat`: the accepted value under federation-controlled VTXOs.

The receive intent must choose the smallest policy-valid gross invoice amount such that:

```text
invoice_amount_sat - receive_fee_sat >= cashu_amount_sat
```

The public quote continues to represent `cashu_amount_sat`; the invoice may be higher to cover the disclosed backend fee. Mint issuance is not permitted until accepted VTXO value, net of Bark fees, is at least the quote amount. Any bounded excess is accounted as federation fee/surplus, never silently counted twice.

Milestone 0 must verify that existing NUT-04 wallets correctly handle an invoice whose gross amount is higher than the requested ecash amount. If that is not interoperable, the first release must require the Ark receive fee to be zero or fund it from an explicit, auditable federation revenue reserve. Silently issuing against the smaller net VTXO amount is never an acceptable fallback.

For outgoing melts:

- the accepted input VTXO value covers invoice amount plus actual Bark/Ark fee;
- actual fee must not exceed the Cashu melt fee reserve or accepted fee policy;
- `total_spent` is derived from accepted VTXO balance effects, not an unverified Ark response;
- unused Cashu fee reserve follows the existing melt change path.

The Bark application config pins a maximum fee formula and CLTV policy. If the Ark server advertises values outside it, the backend degrades rather than silently changing quote economics. A future fee-policy update requires an explicit accepted config/policy epoch.

## 7. Target architecture

```text
Cashu HTTP routes and ordinary mint/melt sagas
                 |
                 | existing quote/mint/melt/payment-observation operations
                 v
CDK federation application ---- AlephBFT journal/checkpoints/backups
                 |
                 | nested, typed FederationBarkOperation records
                 v
            cdk-bark
      +----------+-----------+
      |                      |
pure validation/projection   member-local worker
      |                      |
federation SQL state         Bark federation facade ---- Ark server/chain
                             |
                             +---- member-local Bark SQLite cache
                             |
                             +---- CdkFrostVtxoCustody
                                        |
                                  authenticated private REST
                                        |
                                  cdk-frost participant manager
                                  and one durable protocol ledger
```

### Authority boundaries

| Concern | Authority | Non-authoritative evidence/cache |
|---|---|---|
| Cashu quote and proof state | Existing accepted CDK operations and sagas | HTTP request owner, local task state |
| Bark application identity | Signed federation public config | Local TOML |
| VTXO key indexes | Accepted Bark records | Bark's local last-key index |
| Spendable/reserved/spent VTXOs | Deterministic Bark projection | Bark SQLite VTXO state |
| Send/claim/recovery plan | Accepted Bark plan | Actor candidate plan |
| Payment result | Accepted action results plus existing member-observation policy | One Ark response or mailbox event |
| Threshold signature authorization | Accepted plan and exact signing transcript | Coordinator request label |
| Nonce lifecycle | Participant-local durable `cdk-frost` ledger | In-memory maps |
| Worker lease | Deterministic journal-derived actor selection | Process locks |
| Incoming preimage availability | Protected member records plus accepted signed acknowledgements | Invoice actor memory |
| External execution recovery | Accepted plan, protocol-native payment/artifact identity, and accepted results | Bark checkpoint alone |

### Four persistence layers

1. **Federation SQL authority**
   - accepted Bark operations;
   - materialized VTXO, derivation, action, reservation, and result rows;
   - included in materialized-state digest, checkpoint, backup, and catch-up.

2. **Member-local Bark SQLite execution cache**
   - Bark action checkpoints, movements, locks, server metadata, and imported VTXOs;
   - independently rebuilt from accepted Bark records;
   - never copied between members and never authoritative for consensus.

3. **Member-local `cdk-frost` protocol ledger**
   - sealed ordinary FROST nonces and cached shares;
   - sealed virtual MuSig2 nonce state and cached partials;
   - threshold-DH completed results or tombstones;
   - attempt high-water marks and permanent replay tombstones.

4. **Member-local protected receive-secret store**
   - preimage and binding metadata;
   - encrypted at rest;
   - keyed by receive intent and commitment;
   - backed up under the existing private federation backup policy, not the public materialized-state snapshot.

## 8. Bark FROST application and key model

### Application registration

Add a distinct immutable Bark application under the existing root:

```text
canonical federation FROST root epoch
  +-- CDK on-chain application epoch
  +-- Bark VTXO application epoch
```

The Bark registration includes:

- stable Bark application ID;
- application key epoch;
- application derivation version;
- authorization-policy commitment;
- root epoch;
- federation ID;
- network;
- exact Ark server public key;
- VTXO derivation policy version;
- Lightning fee/CLTV/recovery policy versions;
- gross-to-net receive fee formula and maximum outgoing fee formula;
- receive anti-DoS policy;
- threshold and complete verification-share roster.

Do not overload `FederationFrostApplicationConfig`, which currently contains BDK-specific descriptors and confirmation policy. Add a Bark-specific config that contains a small shared application identity record plus Bark policy. Avoid reshaping the existing on-chain config in the same pull request.

For the first release, the Bark application must be present in the finalized setup config. Adding Bark to an already-active federation requires a separately designed, signed federation-config upgrade and is not silently inferred from local TOML.

### Canonical VTXO derivation

Freeze one derivation specification owned by CDK:

- derive the Bark application from `ApplicationRegistration` using `cdk-frost`;
- derive each VTXO public key from an explicit `u32` locator and a Bark VTXO role domain;
- bind network, root epoch, application key epoch, and derivation version;
- define point parity and full compressed-key conversion explicitly;
- reject overflow, zero, invalid, or unsupported locators;
- publish test vectors for 2-of-3 and 3-of-5 roots;
- compare CDK-derived public group keys and every verification share with Bark's expected `VtxoCustody::derive_public_key` result.

Do not assume the reviewed `frost-federation-core` derivation is byte-for-byte compatible. Either adapt Bark's custody implementation to the CDK formula or upstream a shared formula after vectors prove equivalence. The canonical CDK root package remains the source of truth.

### Protocols under the Bark application

The custody adapter requires three separately namespaced protocols:

1. **BIP340 FROST** for direct Schnorr requests.
2. **Virtual MuSig2** for the threshold federation's partial contribution to Bark's outer user/Ark-server MuSig2.
3. **Threshold DH with DLEQ** for blinded mailbox identifiers.

Each protocol context binds:

- federation and Bark application IDs;
- root and application epochs;
- accepted operation ID and journal index;
- Bark action ID and phase;
- exact object ordinal;
- locator;
- message/transcript commitment;
- signer set;
- attempt;
- Ark server and network;
- protocol namespace.

Ordinary FROST and MuSig2 nonce IDs must never collide even if all other fields are equal.

### `CdkFrostVtxoCustody`

Implement Bark's `VtxoCustody` in `cdk-bark` rather than constructing `bark-frost::FrostParticipant`:

- `descriptor()` returns the accepted Bark application identity;
- `derive_public_key(locator)` uses CDK's registered application package;
- `sign_schnorr()` authorizes and coordinates existing durable CDK FROST;
- `begin_musig2()` and `finish_musig2()` use the extended durable CDK MuSig protocol;
- `blind_mailbox()` uses authorized threshold-DH shares and verifies DLEQ proofs;
- `abort_session()` burns the exact protocol attempt and retains a tombstone;
- `local_keypair()` always returns `None`.

No method accepts an untyped arbitrary authorization payload.

## 9. Typed authorization and session design

Add a canonical `FederationBarkAuthorizationReference` containing at least:

- authorization version;
- federation ID;
- Bark application ID and epochs;
- accepted operation ID and journal index;
- action ID;
- semantic phase;
- object ordinal;
- operation kind;
- exact plan or result hash;
- VTXO locator;
- protocol kind;
- message/transcript hash;
- signer set;
- attempt.

Participant authorization must:

1. Load the referenced accepted journal entry and current materialized Bark state.
2. Verify the operation is applied and not superseded or terminally incompatible.
3. Reconstruct the expected Bark object, locator, message, Ark server, and policy.
4. Reconstruct the exact protocol transcript.
5. Compare every request field to the reconstruction.
6. Check that this participant is in the accepted signer set.
7. Only then reserve a nonce, return a commitment, release a share, or return a DH contribution.

The deterministic session slot is derived from the authorization reference without the attempt or signer set. The full session ID includes the attempt and signer set. A higher accepted attempt burns an older active attempt; an equal attempt must reproduce the same full context; a lower attempt is stale.

Bark must stop creating random custody sessions in federation mode. Add an explicit session provider or pass the typed context through the restricted action APIs. Avoid thread-local or ambient global context.

## 10. Consensus operation model

Add one top-level journal variant:

```text
FederationOperation::Bark(FederationBarkOperation)
```

`FederationBarkOperation` is a stable CDK federation-domain enum. It must not contain Bark's internal `WalletActionCheckpoint` JSON. The recommended compact family is:

### 10.1 `ReceiveIntent`

Commits to:

- Cashu quote ID;
- Cashu amount, gross invoice amount, receive fee, minimum net VTXO amount, unit, and description commitment;
- payment hash and preimage commitment;
- signed receive-secret acknowledgements;
- exact VTXO destination locator;
- mailbox scope or explicit mailbox ID;
- Ark server and network;
- minimum CLTV and fee policy versions;
- explicit anti-DoS policy and either a protected-token commitment or an exact accepted attestation VTXO reference;
- intent expiry;
- deterministic action ID.

Acceptance reserves the quote, payment hash, destination locator, and action ID. It does not call the Ark server.

### 10.2 `ReceivePrepared`

Commits to:

- accepted receive intent;
- exact canonical BOLT11 invoice;
- payment hash, amount, network, expiry, and description validation;
- Ark server response/request commitment;
- actual fee and CLTV values within accepted policy;
- proof that the invoice's gross amount and expected net VTXO amount satisfy the accepted Cashu collateral invariant;
- stable accepted CDK action and request commitment; these are not sent as new Ark fields.

Acceptance makes the invoice eligible to become the ordinary CDK mint quote request. It does not mark the quote paid.

### 10.3 `ActionPlan`

A typed sub-enum covers:

- outgoing Lightning send;
- incoming HTLC claim;
- failed outgoing HTLC revocation;
- VTXO refresh;
- emergency unilateral exit.

Every plan commits to exact input VTXOs, exact output policies and canonical artifact hashes, derivation locators, fee bounds, expiries, recovery policy, signer attempt, and predecessor state.

Acceptance atomically reserves every input VTXO and locator. A retry cannot select replacements unless a separately accepted superseding plan first releases or transitions the old reservation according to policy.

### 10.4 `ActionTransition`

Records one externally observed semantic transition with exact result artifacts, for example:

- receive HTLCs ready;
- receive preimage disclosed;
- receive claimed;
- send HTLC package co-signed;
- payment initiated;
- payment succeeded with proof/preimage commitment;
- payment terminally failed;
- revocation completed;
- refresh completed;
- exit registered/published/confirmed.

The transition references its plan, predecessor transition, actor, attempt, external request commitment, and canonical result. Apply validates the transition without network access and updates the authoritative Bark projection.

### 10.5 `MemberObservation`

Records bounded member evidence needed to reach an internal Bark transition or an existing CDK payment observation:

- observer;
- source scope and observation epoch;
- quote/action/payment hash;
- Ark or chain status;
- exact artifact/result commitment;
- amount and fee where relevant.

Use existing `MintQuotePayment` and `MeltQuotePayment` operations for final Cashu payment authority. Do not duplicate their quorum and saga rules in Bark operations.

### 10.6 `RecoveryPlan`

Use a separate explicit recovery operation only when the normal `ActionPlan` lineage cannot express the transition clearly. It covers:

- coordinator-attempt replacement;
- actor takeover after an ambiguous response;
- receive-claim retry after preimage disclosure;
- revocation escalation;
- emergency exit;
- terminal abandonment after value is proven safe.

Prefer `ActionPlan` with a recovery subtype when possible; do not add a variant per error string.

### Conflict keys

Derive conflict keys from typed data, including:

- Cashu quote ID;
- Bark action ID and phase;
- payment hash;
- VTXO ID;
- derivation locator;
- input reservation;
- predecessor transition;
- accepted CDK action ID;
- signing slot and attempt.

Never accept caller-supplied opaque conflict keys.

### Admission rules

Every Bark operation must have:

- bounded canonical serialization;
- stateless envelope validation;
- deterministic stateful validation in apply;
- checked balance conservation where applicable;
- policy-version validation;
- exact predecessor validation;
- idempotent identical replay;
- conflict rejection for different contents under the same keys;
- materialized-row key registration and digest coverage;
- operation result lookup for route/worker waiting.

## 11. Authoritative Bark projection

The minimum deterministic projection contains:

- Bark application scope and policy version;
- derivation-locator ownership and high-water state;
- VTXO records keyed by canonical VTXO ID;
- VTXO state: observed, spendable, reserved, pending-spend, spent, exiting, exited;
- complete accepted VTXO bytes and ancestry needed for independent validation;
- action records and semantic phase;
- plan and predecessor hashes;
- quote/payment-hash/action indexes;
- reservation owner and operation index;
- accepted external request/result commitments;
- receive preparation and settlement state;
- outgoing payment, revocation, and recovery state;
- gross, fee, net, and issued-liability accounting for every receive/send action;
- materialized-row fingerprints.

Projection rows are consensus authority and must be included in `federation_materialized_state_snapshot()`. Bark SQLite rows are checked against this projection during readiness but are not included directly.

The projection must answer, without Ark I/O:

- Is this receive intent accepted and prepared?
- Is this payment hash already owned by another quote/action?
- Which VTXOs are accepted as spendable?
- Which plan reserves each VTXO and locator?
- Which semantic action phase is accepted?
- Is a custody request authorized at the current attempt?
- Is a paid or failed CDK payment observation now admissible?
- What exact records must a clean Bark SQLite database replay?

## 12. Bark-side prerequisite changes

These changes should land upstream in Bark before CDK consumes the integration.

### P1. Explicit locator APIs

Add public idempotent APIs equivalent to:

- `derive_vtxo_public_key(locator)`;
- `reveal_vtxo_public_key(locator)`;
- `import_accepted_vtxo(locator, vtxo, origin)`;
- `validate_accepted_vtxo(locator, vtxo)`.

Federation mode must never call `peek_next_vtxo_public_key` or `derive_store_next_vtxo_public_key`.

### P2. Federation-safe receive preparation

Split current invoice creation so the caller supplies:

- payment preimage or a protected secret handle;
- payment hash;
- destination locator;
- mailbox identity/scope;
- exact amount, description, CLTV, policy, and accepted CDK action identity.
- exact Cashu amount, gross invoice amount, receive fee bound, and minimum net amount;
- an explicit anti-DoS token handle, accepted attestation input, or configured no-proof policy.

The API must validate supplied values, call the Ark server only after CDK accepts the intent, and persist or reconstruct a local checkpoint from the accepted intent. It must never silently generate a replacement secret or locator on retry.

### P3. Pure send planning and accepted-plan apply

Add a two-stage interface:

1. Build a candidate from an explicit accepted VTXO view without locks, movements, key advancement, or server I/O.
2. Apply an accepted exact plan idempotently, mirror its locks/movement/checkpoint locally, and drive it without reselecting inputs or locators.

Expose independent plan validation so every member can validate the candidate before voting/apply.

### P4. Typed custody context

Replace opaque authorization bytes in federation mode with an embedding-defined typed reference or canonical byte type that Bark treats as required. Every internal signing call must receive the caller-supplied deterministic session and authorization context. Keep local-wallet convenience generation only for `LocalVtxoCustody` paths.

### P5. Stable semantic action results

Expose versioned result types that `cdk-bark` can translate into accepted transitions without serializing implementation checkpoints. The facade should export:

- exact request commitment;
- semantic phase before and after;
- canonical VTXO/artifact bytes;
- amount/fee/expiry data;
- payment status and proof commitments;
- whether retry is safe, terminal, or ambiguous.

### P6. Original-protocol retry audit

For every Ark RPC used by the backend, document and test the retry behavior of
the original Bark protocol. CDK persists its action ID, exact request
commitment, plan, and result locally, but does not add those values to Ark RPCs
or require server-side federation support. A retry is permitted only when the
original RPC is keyed by the same payment hash or exact artifact and the
accepted request can be reproduced byte-for-byte. Otherwise the action remains
`Pending` or `RecoveryRequired` until status or chain evidence resolves it.

This includes invoice creation, receive status/claim/cancel, HTLC co-sign, payment initiation/status, revocation, refresh, and exit registration.

### P7. Canonical remote protocol types

Provide bounded versioned encoders/decoders and public constructors for:

- FROST request, commitment set, and signature share;
- virtual MuSig request, commitment set, and partial share;
- threshold-DH request, share, and DLEQ proof;
- authorization reference;
- abort and completed-session acknowledgement.

Decoders reject unknown versions, trailing data, oversized collections, unsorted or duplicate members, invalid points/scalars, cross-protocol substitutions, and mismatched sessions.

### P8. Projection rebuild hooks

The restricted facade must support:

- opening a member-scoped Frost wallet database;
- clearing/rebuilding only the rebuildable accepted projection;
- applying accepted VTXOs, locators, reservations, and actions idempotently;
- reporting divergence without automatically trusting local rows;
- keeping member-local operational seed state separate.

## 13. `cdk-frost` extensions

Extend the existing key manager instead of adding a second production participant manager.

### Ordinary FROST

Reuse the existing `SigningAuthorizationScope`, `SigningSessionContext`, durable attempt high-water logic, sealed nonce records, cached shares, restart burn, and Redb store. Add only the Bark derivation and authorization adapters needed by the new application.

### Virtual MuSig2

Add a separate record and API family with lifecycle:

```text
generated -> reserved -> commitment_sent -> partial_signing
          -> partial_produced -> consumed -> tombstone
          \-> burned ---------------------> tombstone
```

Requirements:

- different record/table namespace from ordinary FROST;
- same atomic attempt and slot rules;
- sealed secret nonce persisted before commitment return;
- secret removed or irreversibly burned before partial return;
- completed identical partial cached for retry;
- verification against the participant's derived verification share and final outer MuSig transcript;
- restart and failure-injection tests.

Add new Redb tables rather than changing the serialized meaning of existing ordinary-FROST tables. Opening an existing BDK nonce database must remain compatible.

### Threshold DH

Threshold DH has no reusable signing nonce, but it still needs:

- typed authorization;
- domain-separated derivation;
- DLEQ proof generation and verification;
- bounded wire encoding;
- durable completed-result cache or terminal request tombstone for exact retry/audit;
- request conflict rejection.

### Private REST

Extend the existing authenticated federation private API with protocol-specific endpoints or one tagged custody endpoint. Reuse:

- member identity authentication;
- federation and member routing;
- request body bounds;
- timeouts and concurrency limits;
- replay protection and response validation;
- metrics and redaction.

Do not expose participant custody endpoints on the public Cashu router.

## 14. `cdk-bark` crate design

Add `cdk:crates/cdk-bark/` with no dependency on `cdk` itself, preventing a dependency cycle. It may depend on:

- `cashu` and `cdk-common` for payment and federation domain types;
- `cdk-frost` for application and participant custody;
- the reviewed/released Bark wallet and Ark libraries;
- narrow crypto/serialization dependencies already shared by the workspace.

Suggested modules:

```text
src/
  adapter.rs          MintPayment adapter
  config.rs           accepted and local configuration validation
  custody.rs          CdkFrostVtxoCustody
  authorization.rs    accepted-state transcript reconstruction
  protocol.rs         canonical Bark federation records and hashing
  policy.rs           pure fee/CLTV/VTXO validation
  projection.rs       deterministic apply and query model
  runtime.rs          member-local Bark facade and readiness
  receive.rs          receive intent/preparation/claim worker
  send.rs             send planning/execution/status worker
  recovery.rs         revocation/refresh/exit worker
  secret_store.rs     protected receive secrets and acknowledgements
  transport.rs        authenticated CDK participant client
  error.rs            typed errors and ambiguity classification
  tests/
```

### Runtime API

The runtime should expose a narrow API similar in spirit to federated BDK:

- `scope()`;
- `ensure_ready(frontier)`;
- `validate_operation(operation, prior_state)`;
- `apply_accepted_operation(transaction, entry)`;
- `replay_accepted_operations(entries)`;
- `prepare_receive_intent(...)`;
- `execute_accepted_receive_intent(...)`;
- `prepare_send_plan(...)`;
- `execute_accepted_action(...)`;
- `observe_action(...)`;
- `prepare_recovery_plan(...)`;
- `reconcile_local_projection()`;
- `projection_report()`;
- `degraded_reason()`.

Network-capable methods are never called by deterministic apply.

### Payment adapter behavior

`FederatedBarkPaymentAdapter` advertises only the configured BOLT11 unit and limits.

| `MintPayment` method | Behavior |
|---|---|
| `get_settings` | Return supported BOLT11 settings and accepted limits. |
| `create_incoming_payment_request` | Return a consensus-only error; the federation route owns receive intent and preparation. |
| `get_payment_quote` | Parse the invoice and return a conservative fee reserve derived from the accepted maximum fee formula and accepted public VTXO view. No VTXO reservation. |
| `make_payment` | Return a consensus-only error; the accepted Bark worker owns the exact send plan. |
| `wait_payment_event` | Pending stream initially; add a projection-backed event bridge only if it is simple and deterministic. |
| `check_incoming_payment_status` | Return projection-derived evidence only, or a consensus-only error if the federation observer calls the runtime directly. |
| `check_outgoing_payment` | Return projection-derived evidence only, or a consensus-only error if the federation observer calls the runtime directly. |

Keep the adapter incapable of falling back to a standalone seed-backed Bark wallet.

## 15. Minimal CDK integration seams

### Feature and dependency wiring

Add:

```text
federation-bark -> federation + cdk-bark + required Bark dependencies
```

Do not add Bark to `cdk-mintd` default features initially. `federation` without Bark must not pull Bark into its build graph.

### Public config

Extend the FROST wallet config with an optional Bark application config using `serde(default, skip_serializing_if = "Option::is_none")`. Validate it against the root and federation roster. Preserve byte-for-byte interpretation and validation of existing version-1 BDK-only snapshots. Introduce and require a new aggregate/protocol version when the Bark application is present; do not silently reinterpret a version-1 snapshot as Bark-capable. Older binaries must fail before joining a Bark-enabled federation, not after receiving an unknown accepted operation.

Add local `[federation.bark]` settings for:

- enabled flag;
- network;
- Ark endpoint and expected server public key;
- Bark SQLite path or member-local data directory;
- operational seed provider/reference;
- chain source;
- receive-secret store;
- worker concurrency/timeouts;
- local policy values that must equal accepted public policy.

Never put endpoints, credentials, local paths, operational seed material, or preimages in public config or debug output.

### Lightning backend selection

Add `LnBackend::FederatedBark` behind `federation-bark`. Startup rejects:

- federation disabled;
- missing accepted Bark application;
- network, Ark server, threshold, roster, or policy mismatch;
- standalone Bark VTXO seed custody;
- missing durable Frost store or receive-secret store;
- Bark database bound to another custody descriptor;
- trusted-dealer setup outside test-only builds;
- Bark backend configured for multiple units or unsupported units.

The initial backend supports `CurrencyUnit::Sat` only. Do not emulate millisatoshi accounting by rounding Bark's satoshi-denominated VTXOs.

### Incoming quote seam

Add one federation-only BOLT11 branch next to the existing federated on-chain branch in `create_mint_quote_for_route`:

1. Allocate a quote ID before backend work.
2. Validate the ordinary request.
3. Ask `cdk-bark` to prepare, submit, and await `ReceiveIntent` and `ReceivePrepared`.
4. Call a new narrow `prepare_bolt11_mint_quote_with_invoice(quote_id, request, invoice)` helper.
5. Submit the existing `MintQuote` operation and return the ordinary response.

Do not add a quote ID to every standalone `Bolt11IncomingPaymentOptions` solely for Bark.

### Federation payment probes

Add feature-gated Bark branches in:

- `probe_federation_mint_quote_payment`;
- `probe_federation_melt_quote_payment`.

The branches delegate to `cdk-bark`. They return the existing `FederationMintQuotePaymentObservation` and `FederationMeltQuotePaymentObservation` only when the Bark projection permits them. The ordinary observation quorum and saga finalization code remains unchanged.

### Apply dispatch

Add one Bark dispatch arm in federation admission/apply and one Bark projection section in materialized-state snapshot generation. Keep all Bark semantic validation functions in `cdk-bark` or Bark-specific modules.

### Expected file-level change map

| Repository area | Expected change |
|---|---|
| `cdk:Cargo.toml` | Add `cdk-bark` and reviewed Bark/Ark workspace dependencies. |
| `cdk:crates/cdk-common/src/federation/operation.rs` plus a new Bark domain module | Add one nested Bark operation family, kinds, conflict keys, canonical bounds, and authorization claims. |
| `cdk:crates/cdk-common/src/federation/frost_config.rs` | Add Bark application public config without reshaping the existing BDK application type. |
| `cdk:crates/cdk-common/src/federation/config.rs` | Validate Bark-enabled protocol/config compatibility. |
| `cdk:crates/cdk-common/src/database/mint/mod.rs` | Add narrowly scoped Bark projection transaction/query traits. |
| `cdk:crates/cdk-frost/src/{key_manager,nonce,wire}.rs` and new protocol modules | Add durable virtual MuSig2, threshold-DH records, and canonical wire APIs while preserving existing FROST records. |
| `cdk:crates/cdk-bark/` | Own Bark custody, policy, projection, runtime, workers, secret store, and payment adapter. |
| `cdk:crates/cdk/src/mint/federation/` | Add the Bark apply/probe dispatch and existing-observation bridge. |
| `cdk:crates/cdk/src/mint/issue/mod.rs` | Add a narrow prepared BOLT11 quote helper using an already accepted invoice. |
| `cdk:crates/cdk-axum/src/custom_handlers.rs` | Add the federation-only Bark incoming quote branch. |
| `cdk:crates/cdk-axum/src/federation/` | Wire Bark workers, private custody/secret endpoints, readiness, and actor leases. |
| `cdk:crates/cdk-mintd/src/{config,lib}.rs` | Add opt-in backend selection, local settings, startup checks, and runtime construction. |
| `cdk:crates/cdk-sql-common/src/mint/federation/` | Implement Bark projection persistence and snapshot queries. |
| `cdk:crates/cdk-sql-common/src/mint/migrations/{sqlite,postgres}/` | Add new paired forward migrations only. |
| `cdk:crates/cdk-sqlite/` and `cdk:crates/cdk-postgres/` | Add backend parity, empty-install, and upgrade tests. |
| `bark:bark/src/{custody,lib}.rs` | Add typed deterministic custody contexts and explicit locator APIs. |
| `bark:bark/src/actions/lightning/` and `bark:bark/src/lightning/` | Split pure planning, accepted-plan apply, supplied receive intent, and semantic result export. |
| `bark:bark/src/persist/` | Add only new migrations/metadata needed for accepted-operation projection and rebuild. |
| `bark:bark-frost/` and `bark:frost-federation-core/` | Complete public wire types/vectors; retain as reference/test code where CDK's durable participant supersedes it. |

## 16. Incoming BOLT11 flow

### Phase A: protect the receive secret

1. The request owner allocates a quote ID, random preimage, payment hash, action ID, and candidate destination locator, and computes the smallest accepted gross invoice amount whose net value covers the Cashu quote, all without Ark I/O.
2. It sends the preimage and complete binding metadata over authenticated private member transport to eligible members.
3. Each member persists the preimage encrypted at rest before signing an acknowledgement over quote, payment hash, preimage commitment, locator, Ark server, network, Cashu amount, gross amount, fee/net invariant, anti-DoS policy, and intent expiry.
4. The owner collects at least the configured receive-secret acknowledgement threshold.
5. Orphan protected records expire if no intent is accepted.

Recommended v1 policy: use the signature threshold and select future receive actors only from acknowledged members until the secret is re-replicated. Every acknowledging member can learn/reveal the preimage, which permits griefing but does not permit redirecting or spending the federated VTXO. Document this explicitly.

### Phase B: accept the intent and create the invoice

6. Submit `ReceiveIntent` containing acknowledgements but not the plaintext preimage.
7. Deterministic apply verifies acknowledgement signatures and reserves quote, payment hash, locator, and action.
8. The lease actor loads the protected preimage and any protected anti-DoS token, reconstructs the accepted request, and calls the Ark invoice RPC with the stable action ID.
9. The actor validates invoice signature, network, amount, payment hash, description, expiry, and CLTV/fee bounds.
10. Submit `ReceivePrepared` with the exact invoice and result commitment.
11. Every member validates it deterministically.
12. The public route creates and submits the ordinary `MintQuote` using the accepted invoice, then returns the standard response.

### Phase C: observe and claim

13. Members poll by payment hash; mailbox notifications only trigger polling.
14. Members submit bounded evidence for ready HTLC VTXOs.
15. Once policy is satisfied, propose an exact receive-claim `ActionPlan` containing HTLC VTXOs, destination locator, package, fee, expiry safety, and signing attempt.
16. Consensus accepts the plan before any claim signature or preimage disclosure.
17. The actor obtains threshold custody results, executes the exact claim, and submits semantic `ActionTransition` records.
18. If the preimage has been disclosed and the claim is ambiguous, the action remains pending and follows an accepted retry or exit plan.
19. Members emit the existing mint-quote paid observation only when accepted policy considers at least `cashu_amount_sat` of net value safely claimable/settled under federation custody.
20. Existing CDK quorum and mint issuance logic completes the quote.

## 17. Outgoing BOLT11 flow

1. CDK parses the invoice and creates the ordinary melt quote.
2. `cdk-bark` computes a conservative fee reserve from accepted policy without reserving VTXOs.
3. The wallet submits the melt; the existing accepted `Melt` operation moves proofs to pending.
4. The existing deterministic actor lease selects a Bark planning actor, or a Bark-specific lease derived by the same rule selects one.
5. The actor reads the accepted Bark projection and builds a candidate with exact input VTXOs, payment amount, fee, HTLC/change locator, revocation locator, expiry, and Ark artifact commitment. Planning makes no mutation.
6. Every member can independently validate the candidate against the invoice, melt, VTXO state, amount conservation, fee reserve, and policy.
7. Consensus accepts the send `ActionPlan`, atomically reserving inputs and locators.
8. The actor applies the accepted plan to its local Bark execution cache and creates the exact checkpoint/locks/movement idempotently.
9. Every participant reconstructs the plan before returning FROST or virtual MuSig material.
10. The actor submits the exact HTLC co-sign request and records the accepted result transition.
11. The actor initiates payment using the stable payment hash/action ID and records the accepted initiation transition.
12. All members poll status and submit observations.
13. On success, an accepted transition records the proof/preimage commitment, exact VTXO effects, and actual total spent. Existing `MeltQuotePayment` quorum finalizes the Cashu melt and returns any unused fee reserve through the ordinary path.
14. On terminal external failure, consensus first accepts a revocation plan. Only after value is restored or an accepted recovery path owns it may members emit terminal failed observations.
15. If status is ambiguous, proofs remain pending and actor failover repeats the exact request; it never creates a different send plan.

## 18. Maintenance and emergency recovery

Bark VTXO expiry makes maintenance part of custody safety.

### Refresh

- Members observe expiry/tip data from independent sources.
- An actor proposes exact VTXOs, round context, outputs, locators, fees, and timing.
- Consensus reserves inputs and outputs before round participation or signatures.
- Bark drives the accepted round plan and records exact result artifacts.
- Missed rounds do not silently generate replacement plans.

### Failed outgoing revocation

- Terminal payment-failure evidence is accepted.
- A revocation plan fixes HTLC VTXOs, revocation locator, destination, fee, expiry, and attempt.
- Threshold signing and Ark RPC occur only after acceptance.
- Failure to revoke remains recovery-required; it is not reported to Cashu as safely failed while value is unresolved.

### Incoming claim exit

- Once a preimage is disclosed, cancellation is impossible.
- If cooperative claim cannot complete, consensus accepts an exact unilateral-exit plan.
- The plan binds HTLC VTXOs, preimage commitment, exit transactions, fee policy, destination, and chain context.
- Publication and confirmation are observed and accepted before terminal accounting.

### General emergency exit

- Operators can propose exits only for accepted federation-owned VTXOs.
- Exit destinations should use the existing federated BDK application when configured; otherwise the Bark application needs a separately accepted threshold-controlled on-chain recovery destination.
- This dependency must be resolved in Milestone 0. A production Bark backend cannot promise emergency recovery to a member-local seed address.

## 19. Failure and ambiguity matrix

| Failure point | Required behavior |
|---|---|
| Actor crashes before secret acknowledgements | No accepted state or Ark invoice; orphan member secret records expire. |
| Actor crashes after `ReceiveIntent`, before invoice RPC | Lease rotates to an acknowledged member, which uses the same preimage and locator. |
| Invoice RPC succeeds but response is lost | Retry by stable action ID/payment hash or query status; never generate another preimage. |
| `ReceivePrepared` accepted but HTTP owner disconnects | Quote/result lookup returns the same invoice on retry. |
| Incoming HTLC observed by one member only | No payment authority until required accepted evidence exists. |
| Preimage disclosed, claim response lost | Remain pending; retry exact claim or accept exit. Never mark unpaid. |
| Planner crashes before send plan acceptance | No external side effect; another actor may propose a plan from current state. |
| Send plan accepted, actor crashes before HTLC RPC | Another actor applies and executes the same plan. |
| FROST commitment returned, participant crashes | Restart burns unsafe incomplete state or returns the exact durable commitment/share according to lifecycle. |
| One selected signer is unavailable | Accept a higher attempt with a deterministic new eligible subset; old attempt is burned. |
| HTLC RPC succeeds, response lost | Query/retry by exact action ID; do not select different inputs. |
| Payment initiation response lost | Treat as pending/unknown and query by payment hash. |
| Payment terminally fails, revocation pending | Cashu melt remains pending until value recovery is accepted. |
| Bark SQLite is deleted | Reopen scoped wallet and rebuild accepted public state from journal; protected secrets and Frost ledger restore separately. |
| Member is far behind | Disable Bark worker/signing readiness until journal, projection, and policy frontier catch up. |
| Ark server key/config changes | Fail closed; require a new application/config epoch. |

## 20. Persistence and migration plan

### New federation SQL state

Add new forward migrations for tables such as:

- `federation_bark_actions`;
- `federation_bark_vtxos`;
- `federation_bark_derivation_allocations`;
- `federation_bark_reservations`;
- `federation_bark_transitions`;
- `federation_bark_observations`.

The final schema can consolidate these where it improves integrity, but must retain explicit unique constraints for quote, payment hash, action/phase, VTXO reservation, locator, predecessor, and operation ID.

Requirements:

- paired new SQLite and PostgreSQL files;
- no historical migration edits;
- foreign keys or equivalent deterministic validation to accepted operations where practical;
- idempotent identical insert behavior and conflict on different bytes;
- rows registered in `federation_materialized_state_keys`;
- complete projection snapshot/digest support;
- empty install, pre-Bark upgrade, rollback-on-failure, and backend parity tests.

### Bark SQLite

Use Bark's own forward migration system for member-local execution-cache changes. Never edit an already released Bark migration. Add a new migration if the restricted facade needs accepted-operation IDs, locators, request commitments, or projection origin metadata.

### Frost Redb

Open existing stores compatibly and create new protocol-specific tables. Do not change the meaning of existing ordinary-FROST serialized records or table keys. Test opening an existing BDK-only store, using Bark, restarting, and continuing BDK signing.

### Protected secret store

Store:

- receive intent/action ID;
- quote ID;
- payment hash and preimage commitment;
- encrypted preimage;
- locator and Ark server binding;
- accepted operation reference when available;
- acknowledgement state;
- lifecycle and retention metadata.

Never include plaintext secrets in logs, public checkpoints, materialized-state rows, or operator status.

## 21. Configuration, dependencies, and release provenance

### Bark dependency

Preferred order:

1. Upstream the restricted facade and required custody/wire changes.
2. Consume a released Bark wallet/Ark crate version.
3. If release timing blocks development, use an immutable reviewed git revision temporarily and record it in the plan/lockfile.

Forbidden:

- absolute path dependencies;
- a floating `frost-wallet` branch;
- copying Bark source into CDK;
- production `[patch]` entries that depend on a developer checkout.

### Compatibility gates

- CDK MSRV 1.85.0 with the Bark dependency graph;
- Rust 2021/2024 boundary review;
- one compatible `bitcoin`/`lightning` type graph or explicit checked conversions;
- no duplicate incompatible secp/MuSig types crossing public interfaces;
- license and supply-chain review;
- reproducible locked build.

## 22. Implementation milestones

### Milestone 0: freeze protocol and threat model

Status: complete on 2026-07-19.

#### Frozen v1 decisions

These decisions are normative for the remaining milestones. Changing one
requires a new explicitly versioned policy or protocol epoch; it must not be
silently changed under an existing accepted configuration.

##### Authority and side-effect boundary

- The accepted CDK journal and deterministic Bark projection are the only
  federation authority. Bark SQLite is a rebuildable member-local execution
  cache and Ark responses are untrusted evidence.
- Deterministic apply performs no Ark RPC, chain RPC, signing, random choice,
  local-clock read, Bark SQLite read, or receive-secret read.
- `ReceiveIntent` precedes invoice creation. `ReceiveClaimPlan` precedes claim
  signing and preimage disclosure. `SendPlan` precedes input locking, HTLC
  co-sign, or payment initiation. Accepted revocation, refresh, or exit plans
  precede their respective signatures and publications.
- A read-only status observation may query the configured Ark or chain source
  only after the referenced accepted action exists. The result has no authority
  until it is admitted through the existing observation policy.
- Actor failover may change only the executor. It cannot change the accepted
  request, action ID, payment hash, locator, VTXOs, fee, expiry, signer attempt,
  destination, or recovery path.

##### Emergency-exit destination

- Production `federation-bark` v1 requires the existing federated BDK
  application on the same Bitcoin network, federation root epoch, and public
  federation configuration. Startup fails closed if this dependency is absent
  or inconsistent.
- Bark VTXO custody remains under the Bark application while an exit is being
  authorized. Exit transactions, CPFP funding, and final drain destinations
  are accepted as Bark recovery plans and target a consensus-allocated address
  from the federated BDK application.
- Test-only harnesses may use an explicit mock recovery wallet. A member-local
  seed address is never a production destination, and v1 does not introduce a
  second threshold on-chain wallet implementation.

##### Bark application registration and locator derivation

The CDK registration is:

| Field | Frozen v1 value |
|---|---|
| Application label | `org.cashubtc.cdk.federated-bark` |
| `ApplicationId` | `ApplicationId::from_label` result `592b0360e90867b31ba58f0277e1d78485ee2ca5fa20945512f4ba01f4b61586` |
| Application key epoch | `1` |
| Application derivation version | `1` |
| Authorization policy domain | `cdk/frost/federated-bark-authorization/v1` |
| Authorization policy commitment | SHA-256 result `8a012e3c91d8dbb625c210a0c254e7b9307f76629bbd8325057bcbf05b06fcc6` |
| Locator version | `1` |
| Supported v1 role | `VtxoCustody = 1` |
| Locator index | consensus-allocated unsigned 32-bit integer, beginning at `0` |
| Signature mode | plain BIP340/FROST; no BIP341 key-path post-tweak |

The existing CDK application derivation is applied first. A VTXO signing key is
then derived from that application public package, never directly from the root
package. For locator `(role, index, version)`, derive a non-zero additive scalar
by reducing this SHA-256 transcript modulo secp256k1 order and incrementing the
big-endian `counter` from zero only if reduction produces zero:

```text
"cdk/frost/federated-bark-additive-derivation/v1"
|| root_epoch_u64_be
|| application_id_32
|| application_key_epoch_u64_be
|| application_derivation_version_u16_be
|| authorization_policy_32
|| application_group_key_compressed_33
|| role_u8
|| index_u32_be
|| locator_version_u16_be
|| counter_u32_be
```

The `SigningDerivation.context` is SHA-256 over the same transcript using the
domain `cdk/frost/federated-bark-signing-target/v1` and omitting `counter`; its
single tweak is the scalar above. Milestone 3 must implement this once in a
small CDK adapter and prove byte-for-byte public-key agreement with Bark test
vectors. Bark's current direct-root derivation is not the v1 CDK contract.

The following independent formats begin at version `1`: nested Bark operation,
Bark authorization, Bark custody wire, protected receive-secret record and
acknowledgement, deterministic action ID, and Bark projection schema. The
existing outer federation operation envelope remains version `5`: adding a
typed nested Bark family does not invalidate historical v5 journal entries.
Bark admission is instead gated by an explicit Bark capability/config version;
old binaries and Bark-disabled configurations reject it.

##### Receive-secret availability and retention

- A receive secret is usable only after at least the federation signature
  threshold of distinct eligible members have durably persisted its encrypted
  record and signed identical acknowledgements. The invoice actor must be one
  of those acknowledged members.
- Persist completes before acknowledgement. Acknowledgements bind the accepted
  quote/action reference, payment hash, preimage commitment, locator, network,
  Ark key, amounts, policy versions, and expiry; they never contain the
  preimage.
- An unaccepted orphan may be garbage-collected no sooner than 24 hours after
  local receipt. Wall-clock cleanup has no consensus effect.
- An accepted secret is retained while any receive, claim, refresh, or exit can
  require it. It is eligible for deletion only after terminal accepted value
  accounting, after a quorum-certified checkpoint covers that terminal entry,
  and after a further 30-day operator-configurable retention period. The
  configured period may be increased but not reduced below 30 days in v1.
- Deletion is local, auditable, and never inferred merely from a Bark SQLite
  checkpoint. Backup/restore must preserve still-required encrypted records.

##### Receive fees and first-receive anti-DoS

- The general checked formula is the smallest gross `G` within configured
  bounds for which `G - (base_fee + floor(G * ppm / 1_000_000)) >=
  cashu_amount_sat`. Arithmetic overflow, a fee greater than or equal to `G`,
  or failure to find a bounded `G` is a hard quote failure.
- CDK's bundled wallet does not compare a NUT-04 requested amount to the BOLT11
  amount returned in the payment request. That local fact is not sufficient
  evidence that the wider NUT-04 ecosystem accepts gross invoices. Therefore
  production v1 pins the accepted Ark Lightning receive fee to zero: base fee
  `0`, ppm `0`, `G = cashu_amount_sat`, and net VTXO value must be at least
  `cashu_amount_sat`. Non-zero advertised receive fees fail readiness.
- Supporting a non-zero gross invoice requires a later fee-policy epoch plus
  ecosystem interoperability tests. Funding fees from an operator reserve is
  not part of v1, and undercollateralized issuance is never a fallback.
- Production v1 supports the Ark server's explicit no-proof receive policy and
  requires `ln_receive_anti_dos_required = false` in the pinned Ark capability
  snapshot. The federation never opportunistically selects a local VTXO and
  never retries without a proof after committing to a proof mode.
- Protected one-time tokens and consensus-reserved VTXO attestations require a
  later anti-DoS policy version. This v1 restriction is an operator-visible
  availability/abuse tradeoff, not an implicit fallback.

##### Terminal payment rules

- Incoming is `Paid` only when accepted semantic artifacts prove either (a)
  spendable federation-custodied pubkey VTXOs with net value at least the Cashu
  quote amount or (b) a confirmed accepted emergency exit of at least that net
  value into federated BDK custody. `Accepted`, `HtlcsReady`, preimage
  disclosure, one member's Bark state, or merely claimable value is not paid.
- Outgoing is `Paid` only when the Ark/Lightning payment is terminally
  successful, a valid preimage or equivalent success proof is accepted, and
  the exact accepted VTXO balance effects show amount conservation and a fee no
  larger than the Cashu reserve and Bark plan bound.
- Outgoing is `Failed` only when terminal failure evidence is accepted and an
  accepted revocation has restored spendable federation VTXOs, or an accepted
  emergency exit has confirmed equivalent value in federated BDK custody.
- Timeout, transport loss, actor loss, unknown Ark status, preimage disclosure
  without a terminal receive result, and revocation/exit in progress remain
  `Pending` or `RecoveryRequired`. Terminal states are monotonic.

##### Ark RPC compatibility and retry contract

CDK derives and persists these values as federation-internal consensus data:

```text
action_id = SHA256(
    "cdk/federation/bark-action-id/v1"
    || federation_id_32
    || accepted_operation_id_32
    || phase_u16_be
    || object_ordinal_u32_be
)
request_commitment = SHA256(
    "cdk/federation/bark-ark-request/v1"
    || canonical_typed_request_bytes
)
```

The action ID is stable across actor and transport retries, but neither value is
added to Bark protobuf messages or advertised by the Ark server. The public
server remains unmodified infrastructure.

Invoice creation uses the accepted payment hash and Bark's existing behavior:
an identical retry returns the existing `Created` subscription. CDK validates
the returned signed invoice against the accepted payment hash, exact amount,
description, network, and policy before accepting `ReceivePrepared`; a changed
HTTP request already conflicts in CDK before Ark I/O.

Receive claim retries reproduce the exact accepted HTLC inputs, output policy,
preimage, custody attempt, and Arkoor package. Bark validates the exact VTXO
tree update, rejects conflicting spends, and produces deterministic MuSig
partial signatures for the same package. The local preimage-disclosure and
claimed-result checkpoints are persisted around the RPC. A lost response is
never interpreted as failure: the action stays `Pending` or
`RecoveryRequired` and is reconciled by exact retry, Ark status, or chain
evidence.

Every other mutating RPC must receive the same audit. CDK may replay only an
exact accepted request with a protocol-native payment hash or artifact
identity. If the original Bark protocol cannot safely classify or repeat an
operation, production keeps the result ambiguous and escalates to the accepted
recovery path. No production behavior may require a custom Ark capability,
field, endpoint, or database migration.

##### Frozen resource limits

| Resource | v1 limit |
|---|---:|
| Existing outer federation operation envelope | 1 MiB, unchanged |
| Canonical nested Bark operation | 512 KiB |
| Candidate/result canonical artifact aggregate | 512 KiB |
| One canonical VTXO or Ark artifact | 64 KiB |
| VTXO inputs, outputs, HTLCs, or commitment entries per action | 64 each |
| Federation/FROST participants | 64 |
| Bark participant authorization payload | 16 KiB |
| Complete authenticated custody request or response | 64 KiB |
| Protected receive-secret request or response | 4 KiB |
| User-supplied invoice description | 4 KiB UTF-8 |

The tighter nested limits are enforced before allocation and again after
canonical encoding. The outer 1 MiB limit and version are not increased. Counts
are checked before conversion, all sums are checked, canonical ordering rejects
duplicates, and decompression or unbounded opaque extension fields are not part
of v1.

##### Non-goals and database rule

The non-goals in section 5 are approved for v1. In particular, v1 has one Ark
server, one Bark application epoch, BOLT11 only, no generic backend plugin, no
root/application key rotation, no raw Bark database replication, and no import
of seed-custodied or experimental-FROST Bark databases. Every member creates a
fresh application-scoped Bark SQLite execution database. CDK persistence uses
new forward-only SQLite and PostgreSQL migrations; no historical migration may
be edited.

#### Milestone 0 verification record

The threat-model review traced each side-effecting Bark receive, send,
revocation, and exit transition against the acceptance boundary above. The
frozen design requires neither root-share export nor a shared operational Bark
seed. The remaining independent cryptographic and adversarial review is a
Milestone 18 release gate, not permission to relax these decisions.

Verification executed with a documentation-only worktree diff:

```text
cargo test -p frost-federation-core -p bark-frost --locked
cargo test -p cdk-common --lib
cargo test -p cdk --lib --no-default-features --features mint
cargo check -p cdk-mintd --no-default-features --features fakewallet,sqlite
```

The first command is the Bark FROST cryptographic baseline. The remaining
commands are the explicit standalone-CDK regression baseline and deliberately
do not enable federation or Bark features. Results: Bark `bark-frost` 4 passed,
Bark `frost-federation-core` 4 passed, `cdk-common` 112 passed, `cdk` 217
passed, and the standalone `cdk-mintd` configuration compiled successfully.

Deliverables:

- approve the authority/state model in this document;
- decide emergency-exit destination dependency on federated BDK;
- fix Bark application ID, epochs, derivation, locator, and policy versions;
- choose receive-secret acknowledgement threshold and retention;
- choose the gross-to-net receive fee formula and explicit first-receive anti-DoS policy;
- define terminal paid/failed rules;
- define original-protocol Ark retry and ambiguity requirements;
- freeze operation and wire size limits;
- record non-goals and fresh-database requirement.

Exit criteria:

- security review agrees no external side effect precedes the required accepted plan;
- no design requires root-share export or shared Bark operational seed;
- standalone CDK behavior has an explicit regression baseline.

### Milestone 1: upstream explicit Bark federation facade

Status: complete on 2026-07-19.

Implementation commit in Bark: `4fe76da4` (`feat: add explicit federation wallet facade`).

Implementation record:

- added a FROST-only `FederationWallet` facade with explicit locator derive,
  reveal, accepted-VTXO import, pure send planning, exact accepted-plan apply,
  receive creation from supplied identity and secret material, and deterministic
  local projection rebuild;
- kept ordinary Bark flows on their existing random-session and next-key paths,
  while federation actions carry an accepted action ID, request commitment,
  exact locators, and a typed deterministic custody-session context;
- persisted only stable federation bindings in existing key and action storage,
  added an index-to-public-key lookup to both Bark persisters, and added no
  migration or historical-migration change;
- made outgoing projection use the accepted timestamp and exact VTXO set, with
  no local clock read or reselection, and made incoming projection preserve the
  Cashu liability, gross amount, receive-fee bound, minimum net amount, policy,
  mailbox, locator, and payment-hash bindings across restart;
- retained the plaintext incoming preimage only in the protected member-local
  Bark action checkpoint. The exported semantic result and projection result
  expose only the payment hash; and
- added independent plan validators and stable public phases/retry classes so a
  clean projection rebuild returns the same semantic state without signing or
  consulting local secret keys.

Verification evidence:

```text
cargo test -p bark-wallet federation --locked
  2 passed

cargo test -p bark-wallet --lib --tests --locked
  105 library tests and 1 integration test passed

cargo check -p bark-wallet --no-default-features --locked
  passed (existing warnings only)

cargo test -p frost-federation-core -p bark-frost --locked
  8 tests passed; doc tests passed
```

The replay test applies identical accepted records to two clean Bark databases,
reconstructs the projection at every outgoing and incoming action phase, and
asserts that `local_keypair()` is never called. Static review also confirms the
federation facade contains no next-key allocation call. Mutation tests reject a
changed send plan and an undercollateralized receive plan, and custody-session
tests cover stable replay plus protocol and transcript domain separation.

A broader `cargo test -p bark-wallet --locked` run passed the same library and
integration suites and 21 doc tests, but exposed two existing doctest
feature-gating failures outside the changed files: the exit example imports the
feature-gated on-chain wallet without enabling `onchain-bdk`, and the chain
example supplies the feature-gated SOCKS argument. They do not affect this
milestone's facade or its exit criteria and remain visible for upstream cleanup.

Deliverables:

- explicit locator derive/reveal/import APIs;
- pure send candidate planning;
- accepted-plan apply without reselection;
- receive start with supplied secret, locator, mailbox, and action ID;
- typed custody session injection;
- stable semantic result exports;
- local projection rebuild hooks.

Tests:

- two clean Bark databases replay identical accepted records;
- no federation path calls local next-key allocation;
- no federation path reaches `local_keypair()`;
- restart between every action step returns to the same semantic plan.

### Milestone 2: canonical Bark wire and authorization contract

Status: complete on 2026-07-19.

Implementation commit in Bark: `6fd88165` (`feat: define canonical Bark custody wire`).

Implementation record:

- added custody wire version 1 with a 64 KiB complete request/response limit and
  a 64-participant/commitment limit, plus canonical decode-by-reencode checks;
- exposed validating public encoders and decoders for standard-FROST requests,
  nonce commitments, complete ordered commitment sets, and signature shares;
  virtual-MuSig2 commitment requests, commitments, complete ordered sets,
  final BIP327 transcripts, and partial shares; and threshold-DH requests,
  shares, and standalone DLEQ proofs;
- made threshold-DH requests and DLEQ transcripts bind the accepted attempt and
  canonical signer set instead of deriving the signer set from whichever
  responses happened to arrive;
- replaced `BarkAuthorizationRequest`'s opaque application byte vector with a
  canonical `FederationBarkAuthorizationReference`, capped at 16 KiB, that
  binds the federation and application identities/epochs, accepted operation
  and journal index, Bark action/phase/object, operation kind, exact plan hash,
  session, locator, protocol family, message/transcript hash, signer set,
  attempt, network, and Ark server;
- replaced the inline MuSig commitment fields with a public validating
  `VirtualMusigCommitmentRequest`, and made every authorization protocol round
  independently encodable with an explicit kind and version;
- defined the exact protocol commitment as
  `SHA256("bark-frost/authorization-protocol-transcript/v1" ||
  canonical_protocol_bytes)`, so FROST commitment/signature rounds and all
  three protocol families are domain separated; and
- documented the participant reconstruction algorithm. A reference is only an
  identity claim: an honest authorizer must load the accepted journal entry,
  reconstruct the reference and full protocol transcript, compare canonical
  bytes and transcript commitment, and verify participant membership before
  releasing cryptographic material.

Verification evidence:

```text
cargo test -p frost-federation-core -p bark-frost --locked
  bark-frost: 5 passed
  frost-federation-core: 5 passed
  doc tests: passed

cargo check -p frost-federation-core -p bark-frost --all-targets --locked
  passed (existing bark-wallet warnings only)

cargo clippy -p frost-federation-core --all-targets --locked -- -D warnings
  passed

cargo clippy -p bark-frost --all-targets --locked --no-deps -- -D warnings
  passed for bark-frost (existing dependency warnings only)
```

The wire tests round-trip every public request, commitment/set, share, final
MuSig transcript, and DLEQ proof. They reject oversized input, trailing bytes,
too many participants, malformed values through validating constructors, and
cross-protocol decoding. The exact-authorizer test reconstructs one accepted
request for every selected participant, then proves that plan-hash, accepted
operation, object ordinal, protocol round, message, and Bark operation
mutations are denied. No migration was added or modified.

Deliverables:

- public bounded encodings for all three custody protocols;
- typed accepted-operation authorization reference;
- exact transcript reconstruction specification;
- cross-protocol and malformed-input rejection tests.

Exit criteria:

- a remote transport can be implemented without private-field access;
- changing any plan field causes every honest participant to deny the request.

### Milestone 3: register Bark under the CDK root

Status: complete on 2026-07-19.

Implementation commits:

- CDK: `feat: register Bark FROST application`
- Bark: `2450be25` (`test: verify CDK Bark derivation vectors`)

Implementation record:

- added the small application-specific `cdk-frost-bark` crate. It owns the
  frozen application label/ID, key epoch and authorization-policy commitment,
  the version-1 VTXO locator, the exact SHA-256/reduce-mod-order additive
  scalar transcript, and the plain BIP340 `SigningDerivation`; it has no Bark
  runtime or secret-share dependency;
- added a Bark-specific public application config without changing the
  descriptor-bearing BDK application type. It commits the root/application
  epochs, complete derived verification-share map, group key, network, exact
  compressed Ark server key, locator version, and explicit fee, CLTV,
  refresh/exit, receive-secret-retention, and first-receive anti-DoS policy;
- extended `FederationFrostWalletConfig` with a serde-defaulted, omitted-when-
  absent Bark application. BDK-only snapshots remain aggregate version 1 and
  retain their prior serialized digest vector; a Bark application requires
  aggregate version 2. Consequently an older `deny_unknown_fields` binary
  rejects a Bark-enabled snapshot before startup, while the current binary
  rejects version 1 with Bark or version 2 without Bark;
- added initial DKG export activation flags for Bark network, Ark server key,
  application epoch, and an explicit public-policy JSON file. Bark activation
  requires the existing federated BDK recovery application on the same network
  and derives from the already completed reusable FROST root; no second DKG or
  secret field is created;
- kept participant persistence at the existing boundary: the participant-local
  root remains encrypted in `FederationFrostRootPrivateConfig`, the complete
  public Bark application persists through the signed federation-config
  snapshot, and startup derives/registers Bark inside the existing
  `ParticipantKeyManager` and durable nonce namespace;
- added a registration-only Bark authorization adapter. It proves application
  construction without exporting a root or application share, but denies every
  signing request until the accepted-state authorizer in Milestone 7 is
  installed; and
- froze deterministic 2-of-3 and 3-of-5 application group keys, every member's
  verification share, and locator `0`/`u32::MAX` public keys. Bark independently
  reconstructs the transcript and returns the same keys through its
  `VtxoCustody::derive_public_key` contract, with `local_keypair()` unavailable.

Verification evidence:

```text
cargo test -p cdk-frost-bark
  2 passed; doc tests passed

cargo test -p cdk-common --features federation --test frost_dkg
  4 passed, including 2-of-3 and 3-of-5 DKG/config/no-export vectors

cargo test -p cdk-mintd --features federation completed_dual_dkg_ --lib
  2 passed

cargo test -p cdk-mintd --features federation parses_federation_dkg_status_subcommand --lib
  1 passed

cargo test -p cdk-axum --features federation frost_signing --lib
  8 passed

cargo clippy -p cdk-frost-bark -p cdk-common --all-targets \
  --features federation --no-deps -- -D warnings
  passed

bark: cargo test -p frost-federation-core -p bark-frost --locked
  bark-frost: 6 passed; frost-federation-core: 5 passed; doc tests passed

bark: cargo clippy -p bark-frost --all-targets --locked --no-deps -- -D warnings
  passed for bark-frost (existing bark-wallet warnings only)
```

An additional strict all-target clippy run including `cdk-axum` reached
unrelated pre-existing Rust 1.95 lints in unchanged federation/AlephBFT files;
none referenced the Milestone 3 changes. The focused Axum FROST signing suite
and all-target compile gate passed. No migration was added or modified.

Deliverables:

- Bark application config and validation;
- setup/DKG activation derivation from the existing root;
- public and private config persistence;
- application derivation vectors for 2-of-3 and 3-of-5;
- no-export custody construction.

Exit criteria:

- BDK and Bark derive different application keys from the same root;
- every member derives the same Bark group key and verification-share map;
- opening with wrong network/server/epoch fails.

### Milestone 4: durable virtual MuSig2 and threshold DH in `cdk-frost`

Status: complete on 2026-07-19.

Implementation commit:

- CDK: `feat: persist federated custody protocols`

Implementation record:

- added explicit `VirtualMusig2` and `ThresholdDh` custody protocol domains to
  the existing signing-session model while retaining the byte-identical
  ordinary-FROST slot/session derivation domains. A single authorization slot
  can no longer collide across ordinary signing, virtual MuSig2, or
  threshold-DH;
- added typed virtual-MuSig2 nonce IDs, two-column public commitments, complete
  canonical commitment sets, finalized BIP327 transcripts, and partial shares.
  Partial generation and verification use the same libsecp-compatible MuSig2
  equations exercised by Bark, and aggregate tests compare the resulting
  signature against the transparent group key;
- added typed threshold-DH requests, shares, and Chaum-Pedersen DLEQ proofs.
  Each result binds the exact accepted session, request, participant, and
  signer set; combination verifies every proof before interpolation and agrees
  with the transparent group operation for 2-of-3 and 3-of-5 subsets;
- kept both new operations behind the registered-application authorization
  adapter. The key manager exposes only protocol-specific methods: it does not
  expose the root share, an application share, arbitrary scalar multiplication,
  or an unrestricted digest-signing API;
- added independent in-memory and Redb ledgers with versioned, bounded records
  and new `frost_virtual_musig2_*_v1`, `frost_threshold_dh_*_v1`, and
  `frost_protocol_meta_v1` tables. Existing ordinary-FROST table names and
  record bytes were not changed, and an ordinary-only Redb database is opened
  and upgraded by creating empty protocol tables;
- made attempt activation, older-attempt burning, slot reservation,
  compare-and-swap transitions, terminal ordering, and tombstone compaction
  atomic. Attempt high-water marks permanently reject stale attempts and reject
  a different signer set or session at the same attempt;
- implemented the virtual-nonce lifecycle `Generated -> Reserved ->
  CommitmentSent -> PartialSigning -> PartialProduced -> Consumed`, with
  terminal `Burned` transitions. Secret nonces are sealed with protocol,
  session, nonce, and participant AAD, removed before partial computation, and
  never reloaded after the durable `PartialSigning` boundary;
- durably cache completed virtual partials and threshold-DH results. An exact
  retry after restart verifies the cached cryptographic result against the
  caller's reconstructed transcript before returning byte-identical output;
  conflicting requests for the same slot/session remain rejected, including
  after detail compaction; and
- restart recovery burns every ambiguous nonce-bearing state, promotes an
  already persisted `PartialProduced` record to terminal `Consumed`, and burns
  pending threshold-DH work. Failure-injection tests stop at every persistence
  transition and prove that recovery never exposes a reusable virtual nonce or
  recomputes an ambiguous DH result.

Verification evidence:

```text
cargo test -p cdk-frost
  29 passed; doc tests passed

cargo clippy -p cdk-frost --all-targets --no-deps -- -D warnings
  passed

cargo check --workspace --all-targets
  passed

bark: cargo test -p frost-federation-core -p bark-frost --locked
  bark-frost: 6 passed; frost-federation-core: 5 passed; doc tests passed
```

The focused suite covers exact completed retries across Redb reopen, same-slot
and same-attempt substitution, stale attempts, kill-at-every-write virtual
MuSig2 recovery, threshold-DH write failures, terminal compaction, and an
ordinary FROST signing session before and after protocol-ledger use. Formatting
passes with the repository's available stable formatter, which reports that
the repository's nightly-only formatting options are ignored. The configured
environment has no `rustup` executable, so a separate explicit MSRV 1.85 run
could not be performed; the whole-workspace all-target compile gate passed on
the installed stable compiler. No migration was added or modified.

Deliverables:

- protocol-separated durable record types and Redb tables;
- atomic nonce lifecycle and attempt high-water behavior;
- completed retry cache and tombstones;
- threshold-DH authorization/result audit records;
- failure-injection, restart, and BDK coexistence tests.

Exit criteria:

- kill-at-every-write tests cannot reuse a nonce;
- an identical completed request returns the same safe result after restart;
- a conflicting request for the same slot is rejected permanently.

### Milestone 5: authenticated Bark custody transport

Status: complete on 2026-07-19.

Implementation commit:

- CDK: `feat: add federated Bark custody transport`

Implementation record:

- extended the canonical `cdk-frost` wire boundary for protocol-separated
  signing contexts, virtual MuSig2 commitments/sets/transcripts/partials, and
  threshold-DH requests/shares with DLEQ proofs. Ordinary FROST continues to
  emit the exact version-1 `CDKFSC01` context bytes; only virtual MuSig2 and
  threshold-DH use the new explicitly tagged `CDKFSC02` context format;
- added four signature-domain-separated private REST message kinds and routes:
  virtual commitment, finalized virtual partial, threshold-DH share, and
  protocol abort. Every payload is versioned, `deny_unknown_fields`, bounded
  by the existing 512-KiB private signing body limit, and decoded through the
  canonical bounded `cdk-frost` types rather than ad hoc field access;
- bound every request and response to the federation, application registration,
  protocol, root/session, exact derivation, authorization, participant, sorted
  signer set, accepted attempt, nonce or request ID, and finalized transcript.
  Mixed-session, mixed-attempt, wrong-signer, changed-transcript, trailing-byte,
  oversized, and cross-protocol inputs fail before cryptographic output;
- implemented participant handlers on the existing authenticated federation
  REST envelope and long-lived `ParticipantKeyManager`. The handlers require a
  healthy authoritative-read state before producing material and call the
  durable Milestone 4 nonce/DH APIs directly. Abort requests are authorized
  before they may burn state;
- kept the production Bark application fail closed until Milestone 7 supplies
  the accepted-operation revalidator. The transport is live and fully tested
  with exact accepted-state fixtures, but the registration-only Bark adapter
  still denies production custody requests rather than trusting coordinator
  claims;
- added public-only coordinator collection for an exact accepted signer set.
  Requests are collected with bounded unordered concurrency capped by the
  existing outbound semaphore, inherit the configured per-peer timeout and
  fail-fast backpressure, verify every signed response, then independently
  verify and combine the virtual partial or threshold-DH point;
- on an unavailable or ambiguous participant response, the coordinator sends
  best-effort authenticated aborts to the entire selected set. It never swaps a
  signer inside the same attempt: the accepted-state caller must provide a
  higher attempt and its deterministic replacement subset. Participant
  high-water records reject both same-attempt substitution and delayed stale
  traffic; and
- reused the existing private-REST observability boundary: handlers use
  `#[instrument(skip_all)]`, peer errors and timeout labels contain only the
  member and operation class, and externally returned signing failures remain
  fail-closed/redacted. No nonce, authorization payload, transcript, endpoint
  credential, or cryptographic result is logged or added to metric labels.

Verification evidence:

```text
cargo test -p cdk-frost
  29 passed; doc tests passed

cargo test -p cdk-common --features federation
  430 unit tests passed, 1 ignored
  4 federation-operation authorization tests passed
  4 FROST DKG/config tests passed
  doc tests passed

cargo test -p cdk-axum --features federation frost_signing --lib
  9 passed

cargo clippy -p cdk-frost --all-targets --no-deps -- -D warnings
  passed

cargo clippy -p cdk-common --features federation --all-targets \
  --no-deps -- -D warnings
  passed

cargo check --workspace --all-targets
  passed
```

The live three-member route test deliberately omits one selected signer. It
proves that the first attempt fails and is aborted, replacing that signer at
the same attempt is rejected, a higher accepted attempt with the new subset
succeeds under an outbound concurrency limit of one, exact commitment/partial
and threshold-DH retries return identical verified results, a mutated
transcript binding is rejected, and delayed old-attempt traffic remains stale.
The same test performs all participant work through signed private routes; it
does not construct an in-process participant transport.

Strict all-target clippy for `cdk-axum` still reaches unrelated pre-existing
Rust 1.95 lints in unchanged AlephBFT, status, handler, and test code; none
referenced the Bark custody module, peer methods, handlers, or transport test.
Formatting passes with the available stable formatter and its previously
documented nightly-option warnings. No migration was added or modified.

Deliverables:

- private REST request/response types;
- participant handlers backed by `cdk-frost`;
- bounded parallel coordinator client;
- timeouts, concurrency limits, replay protection, and redacted metrics;
- accepted signer-attempt/subset handling.

Exit criteria:

- no in-process aggregate participant set is used in a multi-member deployment;
- unavailable signer replacement uses a higher accepted attempt;
- stale and mixed-attempt shares are rejected.

### Milestone 6: `cdk-bark` skeleton and fail-closed construction

Status: implementation and local verification complete; publishable milestone
completion and final provenance evidence are waiting for a remotely fetchable
immutable Bark revision.

Local progress commit subject: `feat: add federated Bark runtime skeleton`.
This commit is intentionally not publishable while it contains the temporary
developer-checkout dependency; it preserves reviewed work without representing
the dependency-publication exit criterion as complete.

In-progress implementation record:

- added an empty-by-default `cdk-bark` crate and a separate
  `federation-bark` mintd feature, so neither the crate's Bark-backed modules
  nor `bark-wallet` enter ordinary federation or standalone dependency graphs;
- added typed, redacted errors; an immutable federation/application/custody
  scope; explicit member-local public bindings; store readiness; validated
  satoshi limits; and a manual local-config `Debug` implementation that never
  formats Bark endpoints, credentials, paths, or the operational seed;
- derived a stable Bark `KeySourceDescriptor::frost` fingerprint from the
  accepted federation, application registration, threshold roster, verification
  shares, group key, and locator version;
- implemented `CdkFrostVtxoCustody::descriptor()` and public locator derivation
  through `cdk-frost-bark`, while every signature- or DH-releasing method stays
  consensus-only until Milestone 7 installs accepted-state authorization;
- opened Bark through `Wallet::open_with_custody` using a separate member-local
  operational seed and the restricted `FederationWallet` facade, with daemon
  mutation disabled at construction;
- validated the persisted Bark network, operational-seed fingerprint, exact
  FROST custody descriptor, Ark server key, and mailbox key before exposing the
  runtime;
- added bounded ready/behind/degraded reporting and monotonic local-frontier
  tracking; and
- added a non-mutating `MintPayment` adapter that obtains its limits from the
  validated runtime and rejects invoice creation, quoting, payment, and status
  mutation until the corresponding consensus operation families exist.

Local verification evidence:

```text
cargo test -p cdk-bark --features federation-bark --offline
  4 passed; doc tests passed

cargo clippy -p cdk-bark --all-targets --features federation-bark --offline -- -D warnings
  passed for cdk-bark; two pre-existing Bark dependency warnings remain visible

cargo check -p cdk-mintd --no-default-features --features federation-bark,sqlite --offline
  passed (existing unrelated mintd warnings only)

cargo check -p cdk-mintd --no-default-features --features federation,sqlite --offline
cargo check -p cdk-mintd --no-default-features --features fakewallet,sqlite --offline
  both passed; neither built bark-wallet

cargo tree -p cdk-mintd --no-default-features --features federation -i bark-wallet --offline
  bark-wallet is absent

cargo tree -p cdk-mintd --no-default-features --features federation-bark -i bark-wallet --offline
  bark-wallet -> cdk-bark -> cdk-mintd is the only enabling path
```

The four tests cover exact public derivation, fail-closed signing, every local
federation/network/server/root/application/threshold/roster/policy/unit/store/
chain-source binding, standalone-seed custody rejection, persisted descriptor
and operational-seed mismatch rejection, missing application rejection, limit
validation, and debug redaction.

Publication gate:

- the required Bark facade/wire/vector commit is currently available only in
  the local `frost-wallet` branch at `2450be256343`;
- the temporary development dependency is therefore deliberately suitable
  only for the documented local progress commit, not a publishable milestone
  commit; and
- completion requires publishing that commit (or an equivalent reviewed
  immutable revision/release), replacing the temporary source with the exact
  immutable dependency, regenerating the lockfile, and rerunning the gates
  above.

Deliverables:

- crate, features, typed errors, config, and runtime scope;
- `CdkFrostVtxoCustody`;
- member-local Bark wallet open with separate operational seed;
- custody descriptor validation;
- readiness/degraded reporting;
- non-mutating payment adapter.

Exit criteria:

- no Bark dependency is built without `federation-bark`;
- startup rejects every config/custody mismatch;
- debug output contains no path, endpoint credential, secret, or nonce.

### Milestone 7: typed Bark federation operations

Status: complete on 2026-07-19.

Implementation record:

- added one `FederationOperation::Bark(FederationBarkOperation)` variant while
  retaining outer operation-envelope version `5` and the existing non-Bark
  canonical golden vector;
- kept the nested family to `ReceiveIntent`, `ReceivePrepared`, `ActionPlan`,
  `ActionTransition`, and `MemberObservation`; signer replacement, revocation,
  refresh, and exit use typed plan/transition phases instead of a parallel
  recovery operation hierarchy;
- added non-zero typed action, digest, VTXO, and execution-request IDs;
  versioned locators; satoshi-only checked receive and action accounting;
  sorted unique input, output, signer, acknowledgement, and artifact sets; and
  the frozen 64-entry, 64 KiB artifact, 512 KiB aggregate/nested, 64 KiB
  invoice, and 16 KiB authorization bounds;
- derived the frozen action ID from federation ID, accepted operation ID,
  semantic phase, and object ordinal, with a fixed vector, and derived disjoint
  signing-slot and full-session commitments so signer-set/attempt rotation
  changes the session without changing the economic slot;
- added conflict keys for initial Cashu quote and payment-hash ownership, VTXO
  reservations, locator allocations, action phase/attempt, predecessor,
  execution-request ID, custody signing attempt, and observer epoch; typed
  references in later phases do not incorrectly re-reserve an origin key;
- bound operation authorship to the actor or observer and reject Bark envelopes
  under a public config without an accepted Bark application;
- added a typed accepted-operation authorization reference covering federation,
  application/root epochs and derivation, accepted journal position, action,
  phase, object, operation kind, canonical plan/result commitment, locator,
  protocol, transcript, signer set, attempt, network, and Ark server;
- added pure accepted-entry authorization checks for applied status, exact
  envelope authorization, application scope, operation commitment, signer set,
  attempt, threshold, and roster. Locator ownership and exact transcript
  reconstruction remain deliberately assigned to the deterministic SQL
  projection in Milestone 8;
- added compact result lookup by action and semantic phase, preserving result
  status and commitment across journal serialization and replay;
- initially kept Bark materialized-row registration empty and CDK
  admission/apply fail-closed; Milestone 8 has now replaced those temporary
  guards with the deterministic SQL projection without changing the outer
  operation-envelope version or the pre-existing empty-transition wire; and
- changed no migration and added no Bark implementation checkpoint or
  member-local endpoint/database data to consensus.

Verification evidence:

```text
cargo test -p cdk-common --features federation --offline
  438 passed, 1 ignored
  federation operation authorization: 4 passed
  FROST DKG/config: 4 passed
  doc tests: passed

cargo test -p cdk --features federation --lib --offline
  779 passed; 3 loopback-listener tests were blocked by the filesystem/network sandbox

cargo test -p cdk --features federation --lib wallet::wallet_repository::tests:: --offline
  12 passed with loopback binding permitted, including the 3 sandbox-blocked tests

cargo clippy -p cdk-common --features federation --all-targets --offline -- -D warnings
  passed

cargo check --workspace --all-targets --offline
  passed

cargo fmt -p cdk-common -p cdk -- --check
git diff --check
  passed
```

Strict whole-`cdk` clippy still reaches ten unrelated pre-existing Rust 1.95
warnings in unchanged federation helpers, melt types, mint construction, and
wallet federation code. None is in the Bark operation module or the two small
fail-closed match arms added by this milestone.

Deliverables:

- nested operation family and conflict keys;
- canonical bounds and hashes;
- authorization/author rules;
- result lookup;
- journal, transport, checkpoint, suffix-verifier, and backup compatibility.

Exit criteria:

- replay and duplicate operations are idempotent;
- conflicting quote, payment hash, VTXO, locator, phase, and attempt operations are rejected;
- non-Bark federation operation behavior is unchanged.

### Milestone 8: Bark SQL projection and append-only migrations

Status: complete on 2026-07-20. Implementation and SQLite verification
completed on 2026-07-19; the remaining PostgreSQL runtime gate passed against
an isolated PostgreSQL 16 instance on 2026-07-20.

Implementation record:

- added exactly one new paired forward migration,
  `20260719000000_create_federation_bark_projection.sql`, and did not modify
  any prior SQLite or PostgreSQL migration;
- added explicit action, VTXO, derivation-allocation, reservation, transition,
  and observation tables with unique quote, payment hash, action/phase/attempt,
  VTXO, locator, predecessor, operation ID, and observer-epoch ownership;
- checksum-locked every pre-Bark migration as ordered filename-and-byte
  transcripts: 30 PostgreSQL files and 53 SQLite files, in addition to the
  existing representative per-file locks;
- added backend-neutral typed projection records and a canonical complete
  snapshot. Milestone 10 deliberately extended the previously frozen receive
  intent with its exact direct description and mailbox bytes before activating
  invoice creation; the shared SQLite/PostgreSQL lifecycle test now produces
  fixed SHA-256 vector
  `81cea4507814d6365ef32f3b2250e87269ef2256d72a978fc1a0b2bde0a738bb`;
- added a bounded, sorted, defaulted `vtxo_transitions` field to Bark action
  transitions. It explicitly maps canonical VTXO artifacts to locator,
  amount, prior state, and new state, so CDK never parses Bark internals or
  guesses state changes from opaque bytes. Empty pre-projection v1 records
  retain their exact canonical wire and golden vector;
- implemented deterministic stateful checks for action lineage, quote and
  payment-hash ownership, exact predecessor and current plan, semantic phase,
  current attempt, locator ownership, input spendability and reservation,
  immutable VTXO identity/amount/artifact, and the allowed VTXO lifecycle;
- retained the semantic phase underneath `SigningAttemptAdvanced`, so a
  higher-attempt retry can continue only along the original phase's valid
  next edge and can never reopen a completed action or jump between flows;
- made accepted-operation replay idempotent at both the mint apply boundary
  and direct projection boundary, while stale predecessors and different
  locator/VTXO/reservation contents fail before mutation;
- registered every Bark action, VTXO, locator, reservation, transition, and
  observation row through `federation_materialized_state_keys`, and included
  the complete Bark projection in the federation materialized-state snapshot
  and digest;
- added the Bark transaction/database traits and one shared SQL
  implementation used by both SQLite and PostgreSQL; the only mint business
  logic changes are one Bark admission arm, one transactional apply arm, and
  one snapshot section; and
- added empty-install schema/index tests, simulated pre-Bark forward-upgrade
  tests for both backends, an atomic migration-failure rollback test, exact
  replay and stale-predecessor tests, VTXO reserve/spend/replacement coverage,
  and fixed cross-backend projection bytes.

Verification evidence:

```text
cargo test -p cdk-common --features federation --offline
  438 passed, 1 ignored
  federation operation authorization: 4 passed
  FROST DKG/config: 4 passed
  doc tests: passed

cargo test -p cdk-sqlite --features federation --offline
  168 passed; doc tests passed
  includes clean install, pre-Bark upgrade, atomic migration rollback,
  projection lifecycle/replay/conflict, and fixed snapshot vector

cargo test -p cdk-sql-common --features federation migration_tests --offline
  4 passed, including all 83 historical migration bytes locked

cargo test -p cdk-postgres --features federation --offline --no-run
  passed

cargo test -p cdk-postgres --features federation \
  federation_bark_projection_persistence --offline
  not runnable locally: configured CDK PostgreSQL test role is absent

cargo test -p cdk --features federation --lib --offline
  779 passed; 3 loopback-listener tests were sandbox-blocked

cargo test -p cdk --features federation --lib \
  wallet::wallet_repository::tests:: --offline
  12 passed with loopback binding permitted, including those 3 tests

cargo clippy -p cdk-common -p cdk-sql-common -p cdk-sqlite -p cdk-postgres \
  --features federation --all-targets --offline -- -D warnings
  passed

cargo check --workspace --all-targets --offline
  passed

cargo fmt -p cdk-common -p cdk-sql-common -p cdk-sqlite -p cdk-postgres -p cdk -- --check
git diff --check
  passed
```

PostgreSQL runtime verification completed on 2026-07-20:

```text
cargo test -p cdk-postgres --features federation \
  federation_bark_projection_persistence --offline
  passed: 1 passed; 152 filtered out

cargo test -p cdk-postgres --features federation \
  pre_bark_postgres_database_upgrades_with_only_the_forward_migration --offline
  passed: 1 passed; 152 filtered out
```

Both tests ran against an isolated PostgreSQL 16 instance with a dedicated
test database. The first executed the same backend-neutral Bark lifecycle and
fixed snapshot-vector fixture used by SQLite. The second rewound only the new
Bark schema objects and migration row, reapplied the single forward Bark
migration, and verified the historical standalone schema remained present.
The disposable database was removed after the tests.

Deliverables:

- new paired SQL migrations only;
- database traits and SQLite/PostgreSQL implementations;
- deterministic state machine and materialized digest rows;
- migration checksum locks for all historical migrations.

Exit criteria:

- empty and pre-Bark databases reach the same schema;
- SQLite and PostgreSQL projections have byte-equivalent snapshots;
- no previously existing migration byte changes.

### Milestone 9: protected receive-secret distribution

Status: complete on 2026-07-19. The feature-gated private service and runtime
handle are implemented; Milestone 10 will attach the already-opened Bark
runtime to production mint-daemon construction when it activates the incoming
backend.

Implementation record:

- added one canonical public binding containing the federation, action, quote,
  actor, amounts, payment/preimage commitments, locator, mailbox scope, Ark
  server, network, policies, proof mode, and expiry, while keeping the preimage
  exclusively in the authenticated private request;
- bounded the complete protected-secret request at the frozen 4 KiB limit,
  authenticated sender and recipient through the existing signed federation
  transport, and added a bounded collector that accepts only a sorted configured
  recipient set and requires the signature threshold including the invoice
  actor;
- replaced placeholder acknowledgement bytes with deterministic member
  identity signatures over a domain-separated federation/member/record-hash
  transcript. The client verifies both signatures and that the acknowledgement
  signer is the member contacted;
- added a separate member-local Redb store encrypted with AES-256-GCM and an
  independently backed-up operator key. The binding, record hash,
  acknowledgement, receipt time, and accepted/terminal lifecycle metadata are
  all AEAD-authenticated, keys and plaintext owners zeroize on drop, and debug
  and error surfaces remain redacted;
- split preparation into two durable transactions: encrypted persistence first,
  acknowledgement persistence second. A crash after the first commit leaves an
  unacknowledged but decryptable record, and an exact retry completes the same
  deterministic acknowledgement without replacing the secret or locator;
- made action conflicts fail closed, exact retries idempotent, and backup import
  bounded, fully authenticated, decrypt-checked, atomic, and idempotent. Exported
  backups contain ciphertext and public bindings, never plaintext or the
  encryption key;
- retain unaccepted orphans for at least 24 hours, retain accepted intents
  indefinitely, and permit terminal cleanup only after at least the configured
  30-day policy and a cryptographically verified quorum checkpoint that covers
  the terminal journal index; and
- verify acknowledgement signatures, sorted uniqueness, threshold, and actor
  eligibility both at operation admission and deterministic replay/application,
  so catch-up cannot bypass the protected-secret gate.

Verification evidence:

```text
cargo test -p cdk-bark --features federation-bark --offline
  7 passed; doc tests passed
  includes persistence-before-ack crash/reopen, exact retry, two-member
  threshold recovery, forgery/replay/duplicate rejection, cleanup boundaries,
  verified-checkpoint retention, redaction, and encrypted backup/restore

cargo test -p cdk-common --features federation --offline
  438 passed, 1 ignored; doc tests passed

cargo test -p cdk --features federation --lib mint::federation:: --offline
  139 passed

cargo check -p cdk-axum --features federation-bark --all-targets --offline
cargo check -p cdk-mintd --features federation-bark --offline -j 1
cargo check -p cdk-bark --no-default-features --offline
cargo check -p cdk-axum --features federation --no-default-features --offline
cargo check --workspace --all-targets --offline -j 1
  passed; ordinary federation and empty cdk-bark feature surfaces remain clean

cargo clippy -p cdk-common --features federation --all-targets --offline -- -D warnings
cargo clippy -p cdk-bark --features federation-bark --all-targets --offline -- -D warnings
  passed; the reviewed Bark dependency retains its two pre-existing warnings

cargo fmt -p cdk-common -p cdk-bark -p cdk-axum -p cdk-mintd -p cdk -- --check
git diff --check
  passed
```

Strict whole-`cdk` and whole-`cdk-axum` clippy still reach unrelated
pre-existing Rust 1.95 warnings in unchanged code. No warning reported by those
runs points to the receive-secret modules or their integration arms.

Deliverables:

- authenticated prepare-secret request;
- encrypted local persistence-before-acknowledgement;
- signed canonical acknowledgements;
- orphan cleanup and accepted-intent retention;
- backup/restore and actor-eligibility rules.

Exit criteria:

- permanent loss of the invoice creator does not lose an accepted invoice secret;
- acknowledgement cannot be forged or replayed for another quote/hash/locator;
- plaintext never enters public federation state.

### Milestone 10: incoming quote preparation

Status: complete on 2026-07-19. This milestone stops at preparation: it can
return one consensus-bound invoice and ordinary Cashu mint quote, but it cannot
mark that quote paid. Independent HTLC observation and threshold claim remain
the Milestone 11 gate.

Implementation record:

- added one `federation-bark` BOLT11 route branch alongside the existing
  federated on-chain seam. The branch is active only when a validated Bark
  runtime is installed; ordinary and standalone BOLT11 paths retain their
  existing payment-backend flow;
- require the existing optional NUT-19 quote public key for federated Bark and
  derive a federation-scoped UUIDv8 quote ID from it. The key is both the
  ordinary quote minting lock and the stable client retry identity: identical
  retries recover the same action/invoice, while reusing the key with changed
  amount or description conflicts against the accepted intent;
- generate the preimage and candidate custody locator without Ark I/O, persist
  the secret encrypted on a signature-threshold set, accept `ReceiveIntent`,
  and only then permit invoice creation. Any acknowledging member may take over
  before the accepted intent deadline, so loss of the original HTTP owner does
  not lose the secret or force a new payment hash;
- bind the exact direct description, mailbox bytes/scope, satoshi amount,
  payment hash/preimage commitment, Ark server, network, CLTV, fee/anti-DoS
  profile, locator, quote, and accepted intent operation ID into the canonical
  request commitment. `ReceivePrepared` admission re-parses the signed BOLT11
  invoice and verifies every corresponding field plus expiry, result, and
  external retry commitments;
- deliberately constrain production v1 to zero receive fees and `NoProof`
  anti-DoS. Startup rejects other accepted policies, unit mismatch, missing
  durable stores, Bark/roster/threshold/config mismatch, or the original Ark
  server's advertised network, key, fee, CLTV, amount, and anti-DoS policy;
- use Bark's original `StartLightningReceiveRequest` unchanged. The supplied
  secret receive facade sends only payment hash, amount, CLTV, mailbox, and
  description. Federation action IDs and request commitments remain local CDK
  consensus data and are never required from the public Ark server;
- rely on the original server's payment-hash lookup for an identical invoice
  retry. CDK independently validates the signed returned invoice and rejects a
  changed NUT-19 request or accepted plan before external I/O. No Bark server
  migration, capability flag, or federation-specific RPC field is required;
- keep restart recovery in the federation-owned layers: the protected receive
  secret, accepted intent/prepared result, exact invoice and local Bark action
  checkpoint. Ark states beyond an existing `Created` invoice are reconciled
  conservatively rather than re-injected through a custom server path;
- add a narrow CDK mint helper that constructs the ordinary BOLT11 `MintQuote`
  from the already accepted invoice without proposer-local persistence. The
  existing accepted `MintQuote` operation remains the only authoritative quote
  persistence path; and
- recover every preparation boundary through existing durable layers: protected
  secret lookup by quote, accepted Bark projection lookup, Ark action/request
  identity, `ReceivePrepared` result lookup, and ordinary local quote lookup.

Original local Bark commits (partly superseded by the compatibility correction
below):

- `feat: bind federated receive retries`;
- `fix: validate federated receive reinjection`.

Verification evidence:

```text
cargo check -p bark-wallet -p bark-server -p bark-server-rpc -p bark-json --offline
cargo check -p bark-server --offline
  passed; Bark retains two pre-existing wallet warnings outside this change

cargo test -p cdk-common --features federation --offline bark_operation
  9 passed; includes the signed invoice mutation matrix and exact commitments

cargo test -p cdk-sqlite --features federation --offline \
  federation_bark_projection_persistence
  1 passed; changed prepared request commitment rolls back transactionally

cargo test -p cdk-bark \
  --features federation-bark,federation-test-utils --offline
  8 passed; doc tests passed; includes secret-by-quote crash recovery and
  unsupported receive fee/anti-DoS startup rejection

cargo test -p cdk --features federation --lib \
  federation_bolt11_quote_uses_exact_accepted_invoice_without_local_persistence \
  --offline
  1 passed

cargo test -p cdk-axum --features federation-bark --offline \
  federated_bark_quote_id -- --nocapture
  2 passed; stable retry, federation/requester separation, and NUT-19 gate

cargo test -p cdk-axum --features federation-bark --offline \
  post_mint_custom_quote_ -- --nocapture
  3 passed; existing public quote behavior remains green

cargo test -p cdk-axum --features federation-bark --offline --no-run
cargo check -p cdk-mintd --features federation-bark,sqlite --offline
  passed

cargo clippy -p cdk-common --features federation --all-targets \
  --offline -- -D warnings
cargo clippy -p cdk-bark \
  --features federation-bark,federation-test-utils --all-targets \
  --offline -- -D warnings
  passed; the reviewed Bark dependency retains its two pre-existing warnings

cargo clippy -p cdk-axum --features federation-bark --all-targets \
  --offline -- -D warnings
  reaches ten pre-existing Rust 1.95 lints in unchanged cdk federation/melt/
  wallet code; no diagnostic points to this milestone's files

cargo fmt -p cdk-common -p cdk-sql-common -p cdk-bark -p cdk \
  -p cdk-axum -p cdk-mintd -- --check
git diff --check
  passed
```

Compatibility correction on 2026-07-20:

- inspection of the deployed public Signet Ark server showed that it implements
  the original Bark protobuf and does not advertise the experimental receive
  capability. Requiring a server upgrade was invalid because Ark is public
  infrastructure outside CDK's control;
- removed the experimental ArkInfo capability bit, federation-only start/claim
  request fields, server reservation/reinjection logic, and Bark migrations V54
  and V55 from the local `frost-wallet` branch. The resulting protobuf is
  byte-for-byte the original Bark protocol in these messages;
- removed CDK's custom capability startup gate. The remaining readiness checks
  cover only original advertised Ark properties that the backend actually
  requires: network, pinned server key, zero receive fee, no-proof receive,
  CLTV/fee bounds, and amount limits;
- retained the necessary federation-local safeguards: NUT-19 request identity,
  protected preimage, accepted action and request commitments, exact plans,
  preimage-disclosure/claim checkpoints, result quorum, and conservative
  ambiguity states. None requires an Ark server change;
- verified the affected Bark wallet, server, RPC, and JSON crates together
  against the restored protobuf. The branch retains two pre-existing wallet
  warnings outside this correction.

Deliverables:

- federation-only BOLT11 route seam;
- receive intent and prepared operations;
- supplied-secret Bark invoice API;
- exact invoice validation;
- gross/fee/net collateral validation and anti-DoS handling;
- ordinary `MintQuote` reuse and idempotent response lookup.

Exit criteria:

- crash before and after every invoice boundary returns one invoice/payment hash;
- no Ark invoice exists without an accepted intent;
- standalone BOLT11 quote tests are unchanged.

### Milestone 11: incoming observation and claim

Status: complete on 2026-07-19.

Bark support commit: `a882840d` (`feat: persist federated receive claim results`).

Implementation record:

- added an idempotent incoming worker under one member-local action lock. Its
  five-second repair scan and existing payment-event wake-up both invoke the
  same independent payment-hash poll; a notification can only make that poll
  happen sooner and is never accepted as settlement evidence;
- made every member poll and validate the exact Ark invoice, status, HTLC VTXO
  chain, destination, amount, fee, expiry, and canonical evidence bytes. A
  `ReceiveHtlcsReady` transition is admissible only after distinct matching
  `Accepted` observations reach the configured signature threshold;
- construct the receive-claim package without signing, reserve its exact HTLC
  inputs and output locator in an accepted `ActionPlan`, and reconstruct the
  complete package from member-local Bark evidence before execution. The plan
  fixes actor, signer set, attempt, inputs, destination, expiry, artifacts, and
  request commitment;
- replaced the Bark FROST registration's fail-closed placeholder with a typed
  authorization that carries canonical derivation bytes, the exact BIP340
  message, accepted operation reference, and bounded request transcript.
  Every participant reloads the applied plan and projection, checks the
  reserved input bytes/locator/amount, re-derives the application key, and
  verifies every signing-session field before ordinary FROST or virtual MuSig2
  can release a contribution;
- connected the Bark custody adapter to the authenticated CDK peer transport.
  Ordinary Schnorr signing uses the existing threshold coordinator; virtual
  MuSig2 uses durable participant commitments and partial-share ledgers, binds
  the outer keys/nonces/message/tweak, and burns ambiguous sessions through the
  existing abort protocol;
- persist the `PreimageRevealed` checkpoint before the claim RPC and retain a
  new durable `Claimed` checkpoint containing the output VTXO IDs until CDK
  accepts the terminal transition. Exact retries hydrate the already stored
  full outputs, while an error after disclosure returns `RecoveryRequired`
  and can never be translated back to unpaid;
- split the semantic result into ordered `Reserved -> PendingSpend` and
  `PendingSpend -> Spent` transitions plus newly `Spendable` output VTXOs.
  CDK admission requires the exact accepted predecessor/input set and at least
  the Cashu amount, without exceeding the accepted output allocation;
- added an independent result-attestation layer after the actor's claimed
  transition. Each member decodes the opaque output bytes and validates their
  canonical encoding, VTXO ID, amount, configured Ark key, FROST-derived
  locator ownership, chain anchor, and complete signed transaction chain.
  Only a threshold of distinct matching `Succeeded` Bark observations enables
  the ordinary mint-payment observation bridge;
- retained the existing `FederationMintQuotePaymentObservation` policy as the
  only quote-paid finalizer. Consequently one member cannot mark a quote paid
  either by forging a Bark result report or by emitting the outer CDK payment
  observation; and
- kept standalone payment backends and mint business logic unchanged. The
  only core seam is a federation-Bark branch in the existing quote probe, and
  it emits the ordinary quote-ID settlement identity after both inner and
  outer evidence policies are satisfied.

Verification record:

```text
bark:
cargo check -p bark-wallet -p bark-server --offline
  passed; two pre-existing bark/src/chain.rs warnings remain

cargo test -p bark-wallet \
  clean_databases_replay_identical_records_without_local_keys --offline
  1 passed; includes the durable Claimed restart phase

cdk:
cargo check -p cdk -p cdk-bark -p cdk-axum \
  --features federation-bark --offline
  passed

cargo test -p cdk-common --features federation \
  frost_authorization_binds_exact_canonical_transcript --offline
  1 passed

cargo test -p cdk --features federation --lib \
  one_false_bark_result_observer_cannot_form_the_inner_quorum --offline
  1 passed

cargo test -p cdk --features federation --lib \
  proposer_only_invoice_visibility_cannot_form_payment_quorum --offline
  1 passed

cargo test -p cdk-bark \
  --features federation-bark,federation-test-utils --offline
  8 passed

cargo test -p cdk-axum --features federation-bark --offline --no-run
  passed

cargo test -p cdk-axum --features federation-bark --offline \
  bark_custody_transport_rotates_attempts_and_retries_exact_results -- --nocapture
  1 passed

cargo clippy -p cdk-common --features federation --all-targets \
  --offline -- -D warnings
cargo clippy -p cdk-bark \
  --features federation-bark,federation-test-utils --all-targets \
  --offline -- -D warnings
  passed; the reviewed Bark dependency retains its two pre-existing warnings

cargo clippy -p cdk --features federation --lib --offline -- -D warnings
cargo clippy -p cdk-axum --features federation-bark --all-targets \
  --offline -- -D warnings
  reach the same ten pre-existing Rust 1.95 lints in unchanged CDK federation,
  melt, mint-construction, and wallet code; no diagnostic points to Milestone 11

cargo fmt -p cdk-common -p cdk-bark -p cdk -p cdk-axum -- --check
git diff --check
  passed
```

Deliverables:

- independent status polling and mailbox-triggered polling;
- accepted HTLC evidence;
- exact receive-claim plan;
- FROST/MuSig authorization;
- accepted result transitions;
- existing mint-payment observation bridge.

Exit criteria:

- one member's false paid report cannot mark a quote paid;
- preimage disclosure cannot precede claim-plan acceptance;
- post-disclosure ambiguity follows retry/exit rather than unpaid.

### Milestone 12: outgoing planning and reservation

Status: complete on 2026-07-19.

Implementation record:

- The Bark payment adapter now accepts only signed, exact-satoshi BOLT11
  invoices on the configured network. It computes the accepted maximum fee as
  `base + ceil(amount * ppm / 1_000_000)` and rejects unsupported units,
  amountless invoices, MPP options, out-of-range amounts, overflow, or
  insufficient accepted spendable balance before any Ark or chain call.
- Quote balance checks read only the accepted public Bark projection. They do
  not reserve VTXOs, allocate locators, inspect member-local Bark state, or
  perform payment side effects.
- The deterministic current melt lease actor builds one canonical send plan
  from the applied melt operation, exact pending quote, accepted policy, and
  accepted projection. Selection consumes spendable VTXOs in canonical VTXO-ID
  order until the exact amount plus fee is covered; the next two unallocated
  locator indexes become the HTLC/change and revocation locators.
- The plan binds the exact invoice, payment hash, predecessor operation,
  action/external IDs, actor, first-attempt signer set, input VTXO IDs, input
  custody locators and values, change value and locator, revocation locator,
  fee and fee bound, and absolute HTLC expiry. The request artifact is a
  canonical commitment to those fields and the invoice is a separate exact
  canonical artifact.
- Stateful admission independently reconstructs the actor, threshold signer
  set, candidate selection, change, both locator allocations, fee, invoice,
  and expiry from accepted CDK state. A proposer cannot change any of them by
  presenting a different locally convenient Bark plan.
- Action insertion, every input reservation, the HTLC/change allocation, the
  revocation allocation, transition insertion, and materialized-state rows are
  applied in one existing mint database transaction. The VTXO compare-and-swap
  update remains the concurrency authority; no new reservation subsystem was
  introduced.
- After consensus acceptance, every member imports the exact accepted VTXO
  artifacts as their spendable origins and applies Bark's canonical send plan.
  Bark atomically creates the movement, locks the inputs to the action, reveals
  the accepted derivation locators locally, and persists the action at
  `LightningSendProgress::Start`. Replay and startup reconciliation rebuild the
  same checkpoint idempotently.
- Milestone 12 deliberately never starts Bark's action executor. Ark HTLC
  co-signing, payment initiation, status observation, and semantic result
  transitions remain Milestone 13 work.

Verification evidence:

```text
cargo test -p cdk-common --features federation,mint
  passed: 442 unit tests, 8 integration tests, and the compile-fail doc test;
  one benchmark-style unit test and three documentation examples were ignored

cargo test -p cdk --features federation --lib
  passed: 784 tests
  note: the first sandboxed run reached 781 passes and only the three tests
  requiring loopback listeners were denied by the sandbox; the permitted local
  rerun passed all 784

cargo test -p cdk-bark --features federation-bark
  passed: 8 tests and doc tests

cargo test -p cdk-sqlite --features federation \
  federation_bark_projection_persistence -- --nocapture
  passed: the projection lifecycle and competing outgoing reservation rollback

cargo test -p cdk-axum --features federation-bark --no-run
cargo check -p cdk-axum --features federation-bark
cargo check -p cdk-mintd --features federation-bark
  passed; the reviewed Bark dependency retains its two pre-existing warnings

cargo clippy -p cdk-common --all-targets \
  --features federation,mint --no-deps -- -D warnings
cargo clippy -p cdk-bark --all-targets \
  --features federation-bark --no-deps -- -D warnings
  passed; the separate Bark dependency retains its two pre-existing warnings

cargo clippy -p cdk-axum --all-targets \
  --features federation-bark --no-deps -- -D warnings
  remains blocked by the existing crate-wide Rust 1.95 lint backlog; no
  diagnostic points to the new outgoing planner module
```

Deliverables:

- conservative quote fee calculation;
- pure candidate selection;
- complete send-plan validation;
- atomic VTXO/locator reservation;
- local Bark plan apply/rebuild.

Exit criteria:

- two concurrent melts cannot reserve one VTXO;
- failover cannot alter inputs, keys, fee, expiry, or invoice;
- insufficient-balance and fee-bound errors occur before external I/O.

### Milestone 13: outgoing execution and finalization

Status: complete on 2026-07-19.

Implementation commits:

- CDK: `feat: execute federated Bark outgoing payments`
- Bark: `feat: add federated send phase execution`

Implementation record:

- Bark now exposes narrow federation-only methods for exactly one accepted
  outgoing phase: execute the HTLC co-sign, apply accepted HTLC evidence,
  initiate the already co-signed payment, apply accepted initiation, poll
  without mutation, and settle an accepted success proof. The federation path
  never starts Bark's general action executor or advances more than one phase.
- The exact HTLC result commits the action and accepted request, payment hash,
  mailbox, canonical full HTLC VTXOs, and canonical change VTXOs. Bark validates
  the Ark server, policies, key, payment hash, expiry, total HTLC amount,
  change amount, ordering, and uniqueness before applying it. Followers apply
  accepted evidence without repeating the Ark co-sign request.
- Bark checkpoints now retain the accepted change VTXO IDs. The field is
  serde-defaulted and has a compatibility test proving that checkpoints from
  before this field remain readable; no Bark migration was added or changed.
- The accepted plan commitment prefixes every outgoing custody transcript.
  CDK authorizes only the exact plan input locators and Lightning-send
  operation. Each participant reconstructs the complete send authorization
  prefix from the accepted plan (commitment, payment hash, amount, fee, and
  ordered input IDs) before releasing a FROST or virtual-MuSig share.
- The deterministic actor submits one consensus-visible phase at a time:
  `SendHtlcsCosigned`, `PaymentInitiated`, then `PaymentSucceeded`. Stateful
  admission binds every transition to the exact plan, actor, attempt, signer
  set, external ID, predecessor operation, request/result evidence, reserved
  inputs, allocated change, and accepted VTXO state.
- HTLC acceptance moves every exact reserved input to `PendingSpend` and adds
  only the accepted change as spendable. Initiation mutates no VTXO state.
  Success requires one exact 32-byte preimage whose SHA-256 is the accepted
  invoice payment hash, then moves every original pending input to `Spent`.
- Payment polling treats pending, missing/transport-unknown status, and terminal
  server failure as non-final for this milestone. No failed observation is
  emitted and Cashu value remains locked; accepted revocation and failure
  release are deliberately owned by Milestone 14.
- Every member replays the accepted plan and phases into its local Bark database
  at startup. Accepted success settlement is idempotent through Bark's paid
  invoice, VTXO-state, and movement records; members then submit independently
  bound success observations.
- The existing federation melt observer remains the only bridge into the Cashu
  melt saga. It does not call the ordinary Lightning backend for a Bark-enabled
  BOLT11 quote. It reports `Paid` only after the accepted success proof and a
  threshold of distinct, exact Bark observations matching action, source,
  payment hash, amount, fee, and result. Existing outer payment-observation
  quorum and saga logic then finalize the melt.
- No SQL migration was added or modified. Changes outside the Bark adapter,
  Bark-specific orchestration, and federation projection/observation branches
  are limited to the typed semantic validation required by the accepted
  operation family; unfederated CDK payment behavior is unchanged.

Verification evidence:

```text
cargo check -p bark-wallet --all-targets --locked
  passed; Bark retains two pre-existing warnings

cargo test -p bark-wallet --lib --locked -- --test-threads 1
  passed: 106 tests
  note: an initial parallel run made two process-lock tests interfere; both
  passed independently and the complete serial suite passed

cargo test -p cdk-common --features federation --lib
  passed: 442 tests; one explicit benchmark-style test ignored

cargo test -p cdk-bark --features federation-bark --lib
  passed: 8 tests

cargo test -p cdk-sqlite --features federation \
  federation_bark_projection_persistence --lib
  passed: outgoing reserve -> HTLC -> initiation -> success and exact replay

cargo test -p cdk --features mint,federation \
  outgoing_success_proof_binds_preimage_hash_and_predecessor --lib
cargo test -p cdk --features mint,federation \
  one_false_bark_result_observer_cannot_form_the_inner_quorum --lib
  passed: proof/predecessor mutations and amount/fee/distinct-member quorum

cargo check -p cdk-bark --features federation-bark --all-targets
cargo check -p cdk --features mint,federation --all-targets
cargo check -p cdk-axum --features federation,federation-bark --all-targets
  passed; Bark retains its two pre-existing warnings

cargo fmt -p cdk-common -p cdk-bark -p cdk -p cdk-axum -- --check
cargo clippy -p cdk-common --features federation --all-targets \
  --no-deps -- -D warnings
cargo clippy -p cdk-bark --features federation-bark --all-targets \
  --no-deps -- -D warnings
  passed

cargo clippy -p cdk --features mint,federation --all-targets \
  --no-deps -- -D warnings
cargo clippy -p cdk-axum --features federation,federation-bark --all-targets \
  --no-deps -- -D warnings
  remain blocked by the existing crate-wide Rust 1.95 lint backlog; no
  diagnostic identifies the new outgoing execution or finalization code
```

Deliverables:

- exact HTLC co-sign execution;
- payment initiation/status reconciliation;
- accepted semantic transitions;
- success proof handling;
- existing melt-payment observation and saga bridge.

Exit criteria:

- lost responses never produce a second logical payment;
- paid payment finalizes exactly once;
- unknown remains pending.

### Milestone 14: revocation and failure safety

Status: complete on 2026-07-19.

Implementation commits:

- CDK: `feat: recover failed federated Bark payments`
- Bark: `582d4190 feat: recover failed federated Bark payments`

Implementation record:

- Terminal payment failure is now a consensus fact rather than an actor-local
  decision. A `PaymentFailed` result is accepted only after threshold distinct
  members submit exact failure observations for the same action, payment hash,
  accepted send result, amount, fee, and predecessor phase. Pending, missing,
  and transport-unknown Bark status remain non-terminal.
- The accepted failure does not release Cashu proofs. It changes the action to
  recovery-required and deterministically derives one
  `FailedSendRevocation` request from the original accepted send plan. That
  request reuses only its preallocated recovery locator and commits the exact
  canonical HTLC VTXOs, payment hash, predecessor, policy, actor, and attempt.
- Revocation custody authorization is domain separated and binds the accepted
  revocation request commitment, payment hash, and ordered HTLC IDs. Every
  signer reconstructs this transcript from consensus state; a failover member
  cannot change the recovery inputs, key, or destination.
- Bark's federation surface separates requesting a revocation from applying its
  accepted result. The actor performs the single external revocation request;
  followers validate and replay the canonical recovered VTXOs without issuing
  a second request. Applying the result finishes the failed movement, spends
  the exact HTLCs, stores the recovered spendable VTXOs, and is idempotent.
- VTXO accounting now follows the actual Bark lifecycle. Creating the HTLCs
  consumes the original reserved inputs, introduces the exact HTLC VTXOs as
  `PendingSpend`, and introduces change as `Spendable`. Success consumes those
  HTLCs. Revocation consumes the same HTLCs and introduces only the accepted
  recovered outputs, preserving value conservation in either terminal branch.
- Every member independently validates and applies the accepted revocation
  result before submitting a safe `Failed` observation bound to
  `RevocationCompleted`. The existing melt bridge exposes a failed payment and
  releases reserved Cashu proofs only after threshold distinct safe recovery
  observations. Failure votes from `PaymentInitiated` cannot substitute for
  recovery-complete votes.
- Startup reconciliation replays accepted payment failure and revocation
  records into Bark. An unresolved or locally invalid recovery frontier leaves
  the Bark backend in `RecoveryRequired`; successful replay clears that state.
  This is the explicit emergency-exit/operator escalation point and prevents
  automatic compensation while Bark value is unresolved.
- The current, not-yet-released Bark projection migration was adjusted so one
  accepted allocation may materialize the multiple VTXOs present in canonical
  Bark evidence. No previous migration was modified. The exact action/locator
  ownership checks remain in the stateful SQL projection.
- The unfederated payment path is unchanged. The new logic remains confined to
  Bark's federation adapter, federation-gated orchestration, semantic
  projection, and the existing consensus payment-observation bridge.

Verification evidence:

```text
cargo check -p bark-wallet --all-targets --locked
  passed; Bark retains two pre-existing warnings

cargo test -p bark-wallet --lib --locked -- --test-threads 1
  passed: 106 tests

cargo test -p cdk-bark --features federation-bark --lib
  passed: 8 tests

cargo clippy -p cdk-bark --features federation-bark --all-targets \
  --no-deps -- -D warnings
  passed

cargo test -p cdk-common --features federation \
  failed_send_revocation_reuses_only_the_preallocated_recovery_locator --lib
  passed

cargo test -p cdk-sqlite --features mint,federation \
  federation_bark_projection_persistence --lib
  passed: reserve -> failure -> revocation -> safe recovered state, including
  idempotent replay

cargo test -p cdk --lib --features mint,federation -- --test-threads 1
  federation and new recovery tests passed as part of 783 passing tests;
  the only three failures were unrelated wallet-repository tests for which the
  execution sandbox denied binding a local test listener

cargo check -p cdk-bark --features federation-bark --all-targets
cargo check -p cdk --features mint,federation --all-targets
cargo check -p cdk-axum --features federation,federation-bark --all-targets
  passed; Bark retains its two pre-existing warnings

cargo fmt -p cdk-common -p cdk-bark -p cdk -p cdk-axum -- --check
cargo clippy -p cdk-common --features federation --all-targets \
  --no-deps -- -D warnings
cargo clippy -p cdk-sql-common --features federation --all-targets \
  --no-deps -- -D warnings
  passed

cargo clippy -p cdk --features mint,federation --all-targets \
  --no-deps -- -D warnings
cargo clippy -p cdk-axum --features federation,federation-bark --all-targets \
  --no-deps -- -D warnings
  remain blocked by the existing crate-wide Rust 1.95 lint backlog; no
  diagnostic identifies the new failure or revocation code
```

Deliverables:

- terminal-failure evidence rules;
- accepted revocation plan and execution;
- revocation-stuck readiness/alerting;
- emergency exit escalation;
- safe failed-observation gate.

Exit criteria:

- Cashu proofs are not released/compensated while Bark value is unresolved;
- revocation retry uses exact accepted VTXOs and key;
- failure paths preserve amount conservation.

### Milestone 15: refresh and emergency exit

Status: complete on 2026-07-19.

Implementation commits:

- CDK: `feat: bind federated Bark maintenance plans` and
  `feat: execute federated Bark maintenance`
- Bark: `feat: add federated Bark maintenance primitives` and
  `feat: complete federated Bark maintenance execution`

Implementation record:

- Maintenance candidates are deterministic federation facts. Every member
  selects the complete sorted group of spendable VTXOs at the earliest expiry,
  derives the same predecessor, action ID, actor, threshold signer set, fresh
  output locator, refresh trigger, emergency trigger, attempt, and policy fee
  bound, then asks Bark only for the exact round output and fee. No candidate
  reserves a VTXO, reveals the output, joins a round, or requests a signature.
- Refresh and emergency-exit requests are canonical artifacts embedded in the
  accepted Bark action plan. They bind every input VTXO and locator, amounts,
  output or recovery destination, timing, fee policy, accepted action ID,
  and custody attempt. Stateful admission independently reconstructs these
  fields from the accepted projection and rejects a changed or locally selected
  plan before it can enter custody.
- Members submit source-scoped trigger observations only after their chain view
  crosses the immutable accepted height. The actor may execute a refresh or
  register an exit only after a threshold of distinct members has accepted the
  exact plan commitment. FROST and virtual-MuSig authorization now admit refresh
  and emergency exit only through the accepted action-plan reference and exact
  plan-commitment transcript; an unplanned maintenance signature remains
  impossible.
- Bark's federation facade separates preview, accepted-plan replay, execution,
  and accepted-result replay. Refresh execution joins only the exact accepted
  input/output round and returns either pending or canonical confirmed
  replacement evidence. The result must spend every reservation, materialize
  exactly the accepted output amount at the accepted locator, and bind the
  confirmed funding transaction before consensus advances to
  `RefreshCompleted`.
- Emergency exit reuses the existing accepted refresh reservation instead of
  releasing and re-reserving it. The destination is one ordinary
  consensus-allocated internal address from the federated BDK application,
  derived and checked against the accepted on-chain application, network, and
  confirmation policy. The v1 drain fee rate is frozen at 2,500 sat/kwu and the
  maximum recovery fee remains bounded by the Bark application policy.
- Exit progression is split into `ExitPlanned`, `ExitRegistered`,
  `ExitPublished`, and `ExitConfirmed`. Registration binds every exact exit
  transaction and moves all inputs from reserved to exiting. Publication binds
  the deterministic one-output BDK drain without changing VTXO state.
  Confirmation requires matching distinct-member chain observations and moves
  every input from exiting to exited. Bark's drain builder filters only the
  accepted plan inputs, so unrelated claimable exits cannot enter the drain.
- Response loss is safe at each side-effect boundary. Refresh retries retain
  the same action and attempt. Exit registration is idempotent and normalized
  to the accepted registration phase even when the local exit manager already
  observes later progress. Drain bytes and txid are deterministic for the
  accepted plan, so a publish retry cannot create a second logical recovery.
- A dedicated bounded worker advances at most one consensus-visible maintenance
  phase per five-second tick. Automatic refresh is enabled by default;
  automatic emergency exit is deliberately disabled by default and must be
  enabled by an operator. These switches can stop local automation but cannot
  relax consensus timing, destination, fee, signer, or confirmation bounds.
- Startup reconciliation reconstructs refresh plans/results and exit
  plans/registrations from the applied journal and final Bark projection. It
  performs no Ark request, round participation, signature, exit progression,
  or broadcast. Pending work resumes only through the normal threshold-gated
  worker after local replay succeeds.
- No SQL migration was added or modified. The existing Bark projection state
  machine gained only the narrow refresh-to-exit escalation and terminal
  maintenance edges, retaining the exact existing reservation during
  escalation. Changes outside Bark-specific and federation-gated code are
  limited to typed validation and startup wiring; unfederated CDK behavior is
  unchanged.

Verification evidence:

```text
cargo test -p bark-wallet --lib \
  maintenance_plans_rebuild_identically_without_signing_or_publication
  passed

cargo test -p bark-wallet --lib
  new federation tests passed; 106 of 107 total tests passed
  the only failure was the existing same-process pid_fcntl lock test

cargo test -p cdk-bark --features federation-bark --lib
  passed: 9 tests

cargo test -p cdk-common --features federation --lib
  passed: 444 tests; one explicit benchmark-style test ignored

cargo test -p cdk-sql-common --features federation --lib
  passed: 11 tests, including the frozen maintenance lifecycle and all
  historical-migration checksum locks

cargo test -p cdk --features mint,federation --lib \
  mint::federation:: -- --test-threads 1
  passed: 142 tests

cargo test -p cdk-mintd --features federation,federation-bark \
  federated_bark_maintenance_defaults_to_refresh_without_exit --lib
  passed

cargo check -p cdk-axum --features federation,federation-bark --all-targets
cargo check -p cdk-mintd --features federation,federation-bark --all-targets
  passed; Bark retains its two pre-existing warnings

cargo clippy -p cdk-common --features federation --all-targets \
  --no-deps -- -D warnings
cargo clippy -p cdk-sql-common --features federation --all-targets \
  --no-deps -- -D warnings
cargo clippy -p cdk-bark --features federation-bark --all-targets \
  --no-deps -- -D warnings
  passed

cargo clippy -p cdk --features mint,federation --all-targets \
  --no-deps -- -D warnings
cargo clippy -p cdk-axum --features federation,federation-bark --all-targets \
  --no-deps -- -D warnings
cargo clippy -p cdk-mintd --features federation,federation-bark --all-targets \
  --no-deps -- -D warnings
  remain blocked by the existing crate-wide Rust 1.95 lint backlogs; no
  diagnostic identifies the new maintenance validation, worker, or config

cargo fmt -p cdk-common -p cdk-sql-common -p cdk -p cdk-bark \
  -p cdk-axum -p cdk-mintd -- --check
git diff --check
  passed
```

Deliverables:

- expiry observations and maintenance candidate planning;
- accepted refresh plans and round execution;
- accepted exit plans, publication, and confirmation evidence;
- operator controls and policy bounds.

Exit criteria:

- expiring VTXOs are visible before safety margin;
- no maintenance signature occurs without an accepted plan;
- clean replay reconstructs pending maintenance/recovery.

### Milestone 16: full restart, replay, and catch-up

Status: complete on 2026-07-19.

Implementation record:

- added one canonical Bark projection frontier that takes the maximum accepted
  operation index across actions, VTXOs, derivation allocations, reservations,
  transitions, and member observations. Observation-only progress therefore
  cannot be omitted from the readiness target;
- made startup and live reconciliation advance the member-local Bark frontier
  only after replaying the accepted projection. Incoming, outgoing, and
  maintenance workers now request the exact current frontier, retry a complete
  idempotent journal-to-Bark catch-up when behind, and remain blocked when the
  replay is degraded;
- changed maintenance reconstruction to resolve its accepted plan and required
  predecessor transitions from the durable journal by projected operation ID.
  Replay is no longer coupled to whichever single operation or bounded catch-up
  page triggered reconciliation;
- added replay-only Bark APIs for accepted incoming HTLC and claim phases. They
  import the exact accepted VTXO bytes, reconstruct the action checkpoint and
  movement, preserve later local states during repeated replay, and never poll
  Ark, allocate a key, create a nonce, sign, or disclose a preimage;
- reconstruct the canonical incoming claim plan entirely from the accepted
  plan and artifacts before applying it. HTLC-ready, preimage-disclosed, and
  claimed transitions are checked against their accepted operation lineage,
  hashes, VTXO IDs, amounts, locators, server, policy, and output bounds;
- terminal receive replay restores spendable outputs, spent inputs, settled
  invoice state, and the successful movement before acknowledging the accepted
  terminal result. Non-terminal replay restores the exact durable retry point;
- fail closed with `RecoveryRequired` when a member named by an accepted
  receive-secret acknowledgement has lost that secret before terminal
  settlement. A member that never acknowledged the secret is not falsely
  treated as a secret holder; and
- kept this recovery work inside the Bark facade, `cdk-bark`, and the
  feature-gated federation runtime. No standalone payment backend, ordinary
  Cashu business rule, SQL schema, or migration was changed.

Restart coverage is intentionally layered. Bark's clean-database tests cover
deterministic local projection and every durable send/receive checkpoint;
`cdk-frost` covers nonce/session crash boundaries; and CDK's federation suite
covers journal response loss, paged catch-up, checkpoint suffix recovery,
SQLite restart, and live AlephBFT member restart. The local Ark/Lightning
multi-process end-to-end restart scenario remains part of the Milestone 18
release gate rather than being represented as a unit test.

Test restarts at:

- receive secret persisted, acknowledgement not returned;
- intent accepted, invoice not created;
- invoice created, response lost;
- receive prepared, quote not returned;
- HTLC ready, claim plan not accepted;
- claim nonce committed/shared;
- preimage disclosed, claim result lost;
- send plan accepted, local Bark plan absent;
- MuSig commitment returned;
- HTLC result lost;
- payment initiated, status unknown;
- revocation/refresh/exit in every phase;
- checkpoint restore and projection deletion.

Exit criteria:

- no nonce reuse, double payment, VTXO double reservation, secret loss, or divergent projection;
- a rebuilt member becomes ready only after all local layers match the accepted frontier.

Verification record:

```text
cargo check -p bark-wallet --offline
  passed; Bark retains two pre-existing warnings in bark/src/chain.rs

cargo test -p bark-wallet \
  clean_databases_replay_identical_records_without_local_keys --offline
cargo test -p bark-wallet \
  maintenance_plans_rebuild_identically_without_signing_or_publication --offline
  passed: 1 focused test in each invocation

cargo test -p cdk-common --features federation \
  projection_frontier_covers_observation_only_progress --lib
  passed: 1 focused test

cargo test -p cdk-bark --features federation-bark --lib
  passed: 9 tests, including persist-before-ack crash recovery, protected
  secret backup/retention, and replay-binding rejection

cargo test -p cdk-frost --lib restart -- --test-threads 1
  passed: 4 restart tests covering nonce burns, write failures, threshold DH,
  application separation, and ordinary-FROST coexistence

RUST_MIN_STACK=8388608 cargo test -p cdk-axum \
  --features federation,federation-bark --lib restart -- --test-threads 1
  passed: 33 restart, response-loss, SQLite, checkpoint, catch-up, DKG, and
  AlephBFT tests

RUST_MIN_STACK=8388608 cargo test -p cdk-axum \
  --features federation,federation-bark --lib \
  lost_post_signer_response_burns_nonce_and_retry_attempt_succeeds \
  -- --test-threads 1
RUST_MIN_STACK=8388608 cargo test -p cdk-axum \
  --features federation,federation-bark --lib \
  bark_custody_transport_rotates_attempts_and_retries_exact_results \
  -- --test-threads 1
RUST_MIN_STACK=8388608 cargo test -p cdk-axum \
  --features federation,federation-bark --lib \
  startup_replays_accepted_but_unapplied_journal_entry_to_same_state_digest \
  -- --test-threads 1
  passed: all 3 focused tests

cargo check -p cdk-bark --features federation-bark --all-targets
cargo check -p cdk-axum --features federation,federation-bark --all-targets
cargo clippy -p cdk-common --features federation --all-targets \
  --no-deps -- -D warnings
cargo clippy -p cdk-bark --features federation-bark --all-targets \
  --no-deps -- -D warnings
cargo fmt -p cdk-common -p cdk-bark -p cdk-axum -- --check
git diff --check
  passed

cargo clippy -p cdk-axum --features federation,federation-bark \
  --all-targets --no-deps -- -D warnings
  remains blocked by the existing crate-wide Rust 1.95 lint backlog; filtered
  diagnostics contain no new Milestone 16 line
```

The default Rust test-thread stack overflowed in an existing large SQLite melt
restart test under the Bark-enabled feature set. Isolating that test with an
8 MiB test-thread stack passed, and the complete 33-test restart filter then
passed with the same setting. This is recorded as a test-harness constraint,
not hidden as a successful default-stack run.

### Milestone 17: observability and operator tooling

Status: complete on 2026-07-19.

Implementation record:

- added one bounded, versioned federated-Bark operator-status model in
  `cdk-common`. It reports readiness/degradation, accepted and local projection
  frontiers, accepted VTXO count/value buckets, expiry exposure, active action
  phase/accepted actor/signer attempt/operation age/value, aggregate signer
  protocol record counts, and receive-secret acknowledgement coverage;
- derived the view from the accepted Bark projection and existing protected
  receive-secret store. No new status database, journal scan, secret mirror, or
  migration was added. The action actor is the actor fixed by the accepted
  action head, and age is deterministic accepted-operation distance rather than
  fabricated wall-clock lease time;
- exposed only the Bark wallet's current chain height through a narrow read-only
  facade method. Status commits to the Ark server identity, labels the Ark
  source as shared/correlated, and labels the chain source as
  undeclared/correlated until independent-source configuration is implemented;
- added the bounded Bark summary to the existing signed local federation status
  response and CLI. `--require-ready` now independently rejects Bark projection
  lag and a locally acknowledged nonterminal receive whose protected record is
  missing, while consensus synchronization remains visible through the existing
  federation status fields;
- added fixed-label Prometheus gauges for readiness, projection lag, VTXO
  state/value, action phase/max operation age, expiring/ambiguous/recovery/signer
  retry exposure, aggregate protocol records, and protected-secret coverage.
  Action IDs, quote IDs, invoices, payment hashes, endpoints, credentials,
  paths, nonces, shares, and session IDs are never metric labels;
- added authenticated signed `bark-reconcile` and `bark-retry` operator
  commands. Reconcile is replay-only and cannot perform Ark RPCs, allocate keys,
  or start signing. Retry requires an exact accepted action ID and explicit
  side-effect acknowledgement, then advances only that action through the
  existing incoming, outgoing, receive-intent, or maintenance worker;
- made targeted maintenance retry unable to create or advance unrelated
  maintenance, and added the missing targeted receive-intent path without
  duplicating payment-processor business logic;
- centralized stable readiness, degradation, VTXO-state, action-kind, and
  semantic-phase labels on their owning enums so status, CLI, and metrics cannot
  drift into separate string taxonomies;
- kept all routes, daemon wiring, CLI commands, and the Bark dependency behind
  the existing `federation-bark` feature. No unfederated mint/payment behavior
  or historical migration was changed.

Verification record:

```text
cargo check -p cdk-bark --features federation-bark --all-targets
cargo check -p cdk-axum \
  --features federation,federation-bark,prometheus --lib
cargo check -p cdk-mintd \
  --features federation,federation-bark,prometheus --all-targets
  passed

cargo test -p cdk-common --features federation bark_operator_status --lib
  passed: 3 tests
cargo test -p cdk-mintd \
  --features federation,federation-bark,prometheus bark_ --lib
  passed: 6 tests
cargo test -p cdk-prometheus \
  federation_bark_metrics_use_only_fixed_redacted_labels --lib
  passed

cargo clippy -p cdk-common --features federation --all-targets -- -D warnings
cargo clippy -p cdk-bark \
  --features federation-bark --all-targets -- -D warnings
cargo clippy -p cdk-prometheus --all-targets -- -D warnings
cargo fmt -p cdk-common -p cdk-bark -p cdk-axum \
  -p cdk-mintd -p cdk-prometheus -- --check
git diff --check
  passed

cargo check -p bark-wallet --offline
  passed for the read-only Bark chain-height facade
```

The Bark dependency still emits its two pre-existing `chain.rs` warnings. A
workspace-wide Bark format check also remains noisy because that branch was
formatted with a different rustfmt/import-order configuration; the single
facade addition follows the surrounding file style and introduces no unrelated
format rewrite.

Two pre-existing feature/test-harness defects remain release-gate inputs rather
than Milestone 17 scope expansion: federation-without-Bark currently reaches an
unconditional `bark_custody` reference to the optional dependency, and the
Axum all-target build has an existing test call to `CdkAlephBftSessionIo::from_db`
with the old argument count. Strict whole-crate Axum and mintd clippy also remain
blocked by the recorded Rust 1.95 lint backlog. The Milestone 17 library and
daemon feature surfaces above compile and their focused tests pass.

Deliverables:

- scoped readiness and degraded reasons;
- VTXO balance by accepted state;
- expiring VTXO and recovery-required counts;
- action phase, actor lease, signer attempt, and age;
- protocol nonce/session counts without secrets;
- receive-secret acknowledgement coverage;
- Ark/chain source scope warnings;
- safe retry/reconcile commands.

Exit criteria:

- operators can distinguish consensus lag, Bark projection lag, signer failure, Ark ambiguity, and recovery-required value;
- tooling cannot print preimages, nonces, shares, credentials, or private paths.

### Milestone 18: adversarial release gate

Status: in progress as of 2026-07-19.

Progress checkpoint:

- restored the required federation-without-Bark build boundary by compiling the
  Bark custody module, imports, and private routes only with
  `federation-bark`. The ordinary FROST routes retain the same bounded body
  limit and remain available with federation alone;
- removed the resulting feature-disabled dead imports/methods and kept Bark's
  participant registration validation available without constructing a Bark
  runtime;
- updated the Prometheus synthetic-checkpoint test fixture for the already
  accepted AlephBFT channel-capacity constructor argument. The Bark-enabled
  Axum all-target build now compiles instead of failing in that stale fixture;
- ran the complete `cdk-frost` unit suite single-threaded: all 29 tests passed,
  including 2-of-3/3-of-5 BIP340 interoperability, canonical-wire mutation,
  signer substitution, stale attempts, durable nonce restart/CAS failures,
  replay tombstones, virtual MuSig2, and threshold-DH failure injection;
- verified `cargo check -p cdk-axum --features federation --lib` and
  `cargo check -p cdk-axum --features federation,federation-bark,prometheus
  --all-targets`.
- refreshed the independently locked `fed-cdk-tui` dependency graph after the
  federation feature gained the lightweight `cdk-frost-bark` protocol crate,
  and verified the tool still builds offline with `--locked`. The lockfile
  does not include `bark-wallet` or enable the `federation-bark` runtime;
- audited the deployed public Signet Ark endpoint against Bark's current
  protobuf and confirmed it exposes the original protocol, not the experimental
  federation receive capability. Restored the local Bark `frost-wallet` branch
  to that original ArkInfo/start/claim wire contract, deleted the two
  federation-only server migrations and reservation/reinjection paths, and
  removed CDK's custom capability gate;
- retained all federation-local accepted identities, plans, protected secrets,
  custody bindings, preimage-disclosure checkpoints, and conservative recovery
  states. Invoice retries use the original payment-hash identity, while claims
  reproduce the exact accepted Arkoor package; neither requires a public server
  change;
- compiled the affected Bark wallet, server, RPC, and JSON packages together
  against the restored wire protocol. CDK feature checks and the local Signet
  invoice demonstration passed as recorded below;
- started a fresh local three-member, two-signature-threshold federation with
  an interrupted/resumed dealer-free BLS+FROST DKG, finalized both the BDK and
  Bark applications for Signet, and opened one independent member-local Bark
  wallet per mint against the existing public Signet Ark and Esplora services;
- imported the finalized public config into a clean `cdk-cli` wallet and
  requested a 100 sat BOLT11 mint quote. The wallet received a valid `lntbs`
  invoice and reported the quote `UNPAID`; all three members returned the same
  quote ID, amount, unit, expiry, and state. The CLI then timed out after the
  configured one-second unpaid wait, as expected. No local Ark server was
  launched and the public server was not modified;
- diagnosed the first paid Nocean receive at the accepted-state boundary: the
  public Ark server had produced one exact HTLC-receive VTXO, but CDK made all
  members call the mutating prepare RPC before accepting observations. Only
  the member whose prepare call succeeded retained local evidence, so the
  four-member observation threshold could never be reached;
- split initial receive preparation from replica evidence retrieval. Only the
  replaceable lease actor may turn Ark `Accepted` into `HtlcsReady`, avoiding
  concurrent first-prepare races. After `HtlcsReady`, every member calls the
  original `PrepareLightningReceiveClaim` endpoint, whose existing ready-state
  branch returns the same already-created complete HTLC VTXOs idempotently;
- corrected an interoperability mistake found by the paid Nocean test: the
  original `CheckLightningReceive` response contains HTLC VTXO IDs, not
  serialized VTXOs. Treating those IDs as complete VTXOs prevented non-actor
  members from observing the receive. Bark status polling now returns status
  metadata only; exact evidence always comes from the original prepare
  endpoint. No Ark protobuf, capability, database, or server change is needed;
- diagnosed the next paid Nocean retry from exact member-local errors: the
  original idempotent prepare endpoint returned the HTLC VTXOs created by the
  first caller, but later replicas compared their original absolute expiry to
  a newly calculated `current_tip + receive_delta` and rejected valid evidence.
  Repeated failed hydration then reached the public Ark's rate limit. Bark now
  keeps strict requested-expiry validation for the initial `Accepted` prepare,
  while the explicit `HtlcsReady` hydration path accepts the first caller's
  exact VTXOs only while their expiry remains in the future. Hydrated evidence
  is persisted before return, so successful replicas stop calling prepare. The
  fix uses the original status and prepare RPCs and requires no public Ark
  server or protocol change;
- diagnosed the accepted replay failure independently from the stored journal
  commitments: receive evidence commits to the expiry in each
  `ServerHtlcRecv` policy, while CDK replay incorrectly substituted the longer
  enclosing VTXO lifetime. Bark now exposes its existing policy-aware evidence
  reconstruction as a local, non-mutating facade operation, and CDK uses that
  single source of truth when validating accepted artifacts. A Bark regression
  vector asserts that the two expiries differ and that the HTLC policy expiry
  is retained. This changes no Ark RPC or wire protocol;
- traced the remaining claim failure to Bark's local checkpoint namespace.
  Ordinary receives are stored as `ln_recv.<payment-hash>`, while accepted
  federated operations are deliberately stored as `federation.<action-id>`.
  The settled preimage was present in the latter checkpoint, but the claim
  lookup searched only the former. Bark now falls back to its existing active
  action checkpoints and returns the preimage only from a matching
  `LightningReceive` checkpoint. This is a local wallet lookup and adds no Ark
  request, polling loop, persistence format, or protocol extension;
- removed a CDK signer condition that required an incoming-claim transcript to
  contain the complete serialized VTXO as a literal byte substring. The
  original Bark transcript binds the accepted plan commitment plus the Arkoor
  inputs, outputs, and intents; the accepted plan already binds the exact
  artifact, while CDK separately verifies the operation, locator, amount, and
  reservation. The literal-substring condition was not an original Bark
  authorization requirement and made a valid claim impossible;
- reproduced the next rejection at the exact serialization boundary: a valid
  outgoing Bark FROST authorization reached 21,422 bytes because the private
  participant envelope encoded Bark's already-bounded binary transcript as
  JSON/hex, exceeding CDK's existing 16,384-byte limit. Only that private
  envelope now uses deterministic compact CBOR with a native byte string. The
  exact Bark transcript remains intact, the bound was not raised, and a
  12,000-byte canonical round-trip regression case proves the compact
  representation;
- reproduced outgoing execution through input hydration, Ark registration,
  FROST authorization, and virtual-MuSig package construction. The public Ark
  server then rejected the accepted HTLC expiry as too low. Its live ArkInfo
  advertises `htlc_send_expiry_delta = 256`; CDK had derived the height from the
  selected input's expiry (`minimum_input_expiry - emergency_margin`) instead
  of Bark's required `current_tip + htlc_send_expiry_delta` rule;
- added one narrow Bark federation-facade query that performs the same outgoing
  expiry calculation as ordinary Bark. The replaceable proposal actor reads it
  before consensus, and CDK commits the returned absolute height into the exact
  send plan and authorization transcript. Deterministic replay performs no
  chain or Ark I/O and enforces a fixed two-week upper extension relative to
  the earliest accepted input. The Ark server retains the live lower-bound
  check. No Ark RPC, protobuf, database, or public-server behavior changed;
- identified the final paid-quote race from accepted history and the live Bark
  projection. After the receive claim made its output spendable, ordinary
  automatic Bark maintenance could immediately reserve that output for a
  refresh. CDK's payment probe incorrectly required the output's *current*
  state to remain `Spendable`, even though it also verified the immutable
  accepted claim transition and threshold-success observations. The probe now
  permits later accepted operations to advance the output state while retaining
  every identity, locator, amount, artifact-hash, accepted-transition, and
  observation check;
- retained consensus-frontier lease rotation for both outgoing payment workers
  and the incoming first-prepare worker. Every replica validates the full
  evidence returned by the ready-state prepare call before submitting its
  bounded observation, and exact accepted VTXOs are imported through the
  existing deterministic apply path;
- reran the three Bark federation tests, all nine `cdk-bark` tests, the existing
  CDK actor-lease test, and the exact production `cdk-mintd` feature check.

Compatibility verification executed on 2026-07-20:

```text
bark:
cargo check -p bark-wallet -p bark-server -p bark-server-rpc -p bark-json --offline
cargo test -p bark-wallet federation --offline --lib
  passed: affected packages compile; 3 focused federation tests passed

cdk:
cargo test -p cdk --features federation --lib \
  accepted_melt_author_is_the_only_payment_actor
cargo check -p cdk-bark --features federation-bark --all-targets
cargo test -p cdk-bark --features federation-bark --lib
cargo check -p cdk-mintd --no-default-features \
  --features fakewallet,sqlite,info-page,federation-bark,prometheus
cargo check -p cdk-axum --features federation,federation-bark --all-targets
  passed: 9 cdk-bark tests and all named production feature checks

locked federation operator tool:
cargo check --manifest-path scripts/fed-cdk-tui/Cargo.toml \
  --locked --offline
  passed; the refreshed lock resolves the federation-only lightweight FROST
  protocol crates without enabling bark-wallet

fresh local Signet demonstration:
3 members; BLS/FROST threshold 2; interrupted DKG resumed and finalized
cdk-cli federation add <final-public-config> --local-mint-url <member-1>
cdk-cli mint --wait-duration 1 <member-1> 100 \
  "federated Bark Signet compatibility"
  result: valid Signet BOLT11 invoice; quote UNPAID on all 3 members
  expected final result: unpaid one-second wallet timeout

fresh paid local Signet demonstration after the receive fixes:
3 members; BLS/FROST threshold 2; BDK on-chain and Bark BOLT11 enabled
external Signet faucet paid a fresh 323 sat federation invoice
  result: Bark reached HtlcsReady and ReceiveClaimed; the mint quote became
  PAID and then ISSUED; the clean cdk-cli wallet minted and received 323 sat
  without a member restart, operator retry, or recovery-only code path
  compatibility: existing public Ark service and original Bark protocol only

outgoing Signet diagnosis and verification:
public ArkInfo: htlc_send_expiry_delta=256
pre-fix accepted plan: input expiry 314076; outgoing expiry 314058
public Ark response: requested expiry too low; server tip 314023
post-fix rule: Bark facade returns current tip + advertised send delta and CDK
  commits that exact bounded height before custody
cargo test -p cdk-common --features federation bark_operation::tests --offline
  passed: 15 focused Bark operation tests, including the 12 KB authorization
  and outgoing-expiry bounds
cargo check -p cdk-bark --offline
cargo check -p cdk-axum --features federation-bark --offline
  passed
live post-fix Cashu melt: blocked before Bark planning by an independent fresh
  federation BLS issuance failure. A single-output 2,048-sat mint response still
  failed the wallet's aggregate pairing check, after every member share had
  passed member-share validation. No proof-verification bypass was added. This
  is not evidence that the outgoing gate has passed; the local mint-signing
  setup must be repaired or replaced before the final Signet melt is rerun.
```

This is a checkpoint, not completion of the release gate. The remaining
database parity, complete adversarial/workspace suites, deterministic local
regtest payment gate, opt-in Signet payment gate, and independent review below
must still pass before this milestone or production readiness can be claimed.

Run:

- Byzantine conflicting proposals and observations;
- malformed canonical artifacts and oversized messages;
- stale attempt, mixed signer set, and cross-protocol substitution;
- actor and signer partitions;
- Ark timeout/rejection/duplicate/ambiguous response;
- chain-source disagreement;
- database transaction and disk failures;
- SQLite/PostgreSQL parity;
- MSRV, fmt, clippy, docs, and workspace tests;
- deterministic regtest end-to-end CLI tests with a local
  Bark/Ark/Lightning stack, covering federated BOLT11 mint and melt;
- an opt-in Signet end-to-end CLI test against the existing Bark Signet
  infrastructure, covering both federated BOLT11 mint and melt;
- independent cryptographic and federation-state review.

Exit criteria:

- all definition-of-done items below pass;
- no unresolved critical/high finding remains;
- production enablement remains opt-in.

## 23. Test matrix

### Cryptography

- BIP340 signatures for 2-of-3 and 3-of-5.
- Virtual MuSig2 partial accepted byte-for-byte by Bark's MuSig implementation.
- Threshold-DH combined result and every DLEQ proof.
- Application and locator derivation vectors.
- Parity/full compressed-key edge cases.
- Cross-application and cross-protocol rejection.

### Durable protocol state

- duplicate commitment request;
- conflicting same-session request;
- completed-share retry after restart;
- crash before/after every CAS transition;
- higher attempt burns lower attempt atomically;
- tombstone compaction retains replay denial;
- BDK and Bark share one Redb database without record collision.

### Consensus and projection

- operation canonicalization and size bounds;
- every conflict-key class;
- invalid predecessor/phase;
- VTXO and amount conservation;
- locator collision and high-water behavior;
- deterministic replay under shuffled local observations;
- snapshot/digest equality across members and SQL backends;
- checkpoint restore and suffix verification.

### Incoming

- secret acknowledgement threshold and invalid acknowledgements;
- invoice actor loss before/after RPC;
- invoice validation mismatch;
- gross invoice, receive fee, and net VTXO accounting boundaries;
- first receive into an empty wallet with each accepted anti-DoS mode;
- duplicate HTTP request returns same invoice;
- HTLC pending/ready/canceled;
- claim success, rejection, timeout, and response loss;
- preimage disclosure followed by cooperative recovery or exit;
- false/missing observer reports.

### Outgoing

- amountless/fixed invoice rules;
- insufficient funds and fee overflow;
- concurrent coin selection;
- actor failover before/after every RPC;
- signer subset failover;
- success, failure, pending, and unknown status;
- revocation success/stuck/exit;
- exact-once Cashu saga finalization.

### Regtest CLI end-to-end release test

Run the complete two-direction CLI payment scenario deterministically on
regtest. Start a local multi-process CDK federation, one member-local Bark
wallet per federation member, a local Bark/Ark server, its Lightning backend,
and the required Bitcoin/chain-source processes. Drive the Cashu side through
`cdk-cli`: mint ecash after paying the federation-created BOLT11 invoice, then
melt that ecash to a BOLT11 invoice created by an independent local Lightning
receiver.

The regtest harness must provision funds and mine blocks automatically, verify
the same liability/projection/backend-selection invariants as the Signet test,
and exercise at least one restart or actor-failover boundary in each payment
direction. This is the deterministic CI release gate and must not depend on a
user, faucet, public Ark server, or public Lightning liquidity.

### Signet CLI end-to-end release test

In addition to regtest, run a real-payment interoperability smoke test on
Signet without launching a local Ark server. Start a local multi-process CDK
federation with `federation-bark`; each member opens its own member-local Bark
wallet and connects to the same existing Bark Signet Ark endpoint and pinned
server public key. Drive the Cashu side exclusively through `cdk-cli`, as an
external wallet would.

The test must complete both directions with small, configurable amounts:

1. `cdk-cli` requests a federated BOLT11 mint quote and receives the exact
   accepted invoice.
2. A separately funded Signet payer pays that invoice. The federation reaches
   its payment-observation quorum and `cdk-cli` mints the expected ecash exactly
   once.
3. A separate Signet receiver creates a BOLT11 invoice.
4. `cdk-cli` melts ecash to that invoice through the federated Bark backend and
   observes terminal settlement, the actual fee, and any ordinary melt change.
5. The test reconciles the Cashu liability, accepted Bark projection, and every
   member-local Bark projection, and proves that no standalone seed-custodied
   Bark or non-federated Lightning backend was selected.

Funding has two supported modes. In interactive mode, print only the public
Signet address or invoice and wait with a bounded timeout for the operator to
fund the external payer or pay the requested invoice. In unattended mode,
first use the existing Bark Signet faucet to fund the isolated external Bark
test wallet. If that faucet is unavailable, use an explicitly configured
alternative Signet faucet to fund an on-chain address and board those funds
into the external Bark wallet before continuing. Faucet URLs, credentials,
rate-limit tokens, wallet seeds, and local paths are member-local test
configuration and must be redacted from output.

The test is an opt-in release gate because it depends on shared Signet
infrastructure. It must use a unique run identifier, bounded retries and
timeouts, faucet-friendly amounts, and idempotent payment-hash lookup. If no
operator and no configured faucet can supply funds, the gate reports a funding
blocker; it must not report the payment test as passed or silently replace it
with a mocked or regtest payment.

### Maintenance and recovery

- expiry threshold boundaries;
- rejected VTXO in round;
- missed round retry with a new accepted plan;
- forced exit and fee policy;
- local Bark database deletion;
- private secret restore;
- member catch-up from stale frontier.

### Standalone regression

- existing CDK library and integration tests without `federation-bark`;
- all existing Lightning backends;
- existing federated BDK tests;
- existing migration checksum and upgrade tests;
- build with federation disabled;
- build with federation enabled and Bark disabled.

## 24. Observability requirements

Metrics and status should expose identifiers only in bounded/redacted form:

- Bark application/root epoch and accepted config digest;
- accepted journal and local Bark projection frontier;
- spendable/reserved/pending/recovery/exiting VTXO counts and amounts;
- oldest action by semantic phase;
- current actor lease and attempt number;
- participant response counts and latency;
- pending/burned/completed protocol sessions;
- receive-secret acknowledgement count;
- Ark status/latency/error class;
- ambiguous result and recovery-required counters;
- refresh deadline margin.

Never expose:

- root/application shares;
- nonce material;
- receive preimages;
- unredacted invoices in high-cardinality labels;
- operational seeds;
- credentials, tokens, private endpoints, or local paths.

## 25. Suggested pull request sequence

1. Bark: explicit locators, supplied receive intent, pure send plan, accepted-plan apply.
2. Bark: typed custody contexts, deterministic sessions, semantic result API.
3. Bark/core: complete canonical wire APIs and idempotency tests.
4. CDK: Bark application registration and derivation vectors.
5. CDK: durable MuSig2/threshold-DH extensions and private transport.
6. CDK: `cdk-bark` skeleton, custody adapter, fail-closed config.
7. CDK: nested Bark operations and pure projection.
8. CDK: new append-only SQL migrations and digest/backup support.
9. CDK: receive-secret distribution and incoming invoice preparation.
10. CDK: incoming observation/claim and mint-payment bridge.
11. CDK: outgoing planning/reservation/execution and melt bridge.
12. CDK/Bark: revocation, refresh, and emergency exit.
13. Recovery, operator tooling, adversarial tests, and independent review.

Keep Bark and CDK changes independently reviewable. Do not combine cryptographic-store redesign, public route integration, SQL schema, and full payment execution in one pull request.

## 26. Recommended decisions for open design points

| Question | Recommended first-release decision |
|---|---|
| Where does production participant state live? | Extend the existing `cdk-frost` Redb ledger; do not use Bark's process-local participant. |
| How is the CDK root reused? | Register a separate Bark application; never export/import the root share. |
| How are VTXO indexes allocated? | In accepted Bark intents/plans, with explicit conflict keys and high-water validation. |
| What is authoritative Bark state? | CDK journal plus deterministic Bark SQL projection. |
| What is Bark SQLite? | Independent member-local, rebuildable execution cache. |
| How are incoming preimages recovered? | Private persist-before-ack distribution to at least the signature threshold before `ReceiveIntent`; accepted acknowledgements prove availability. |
| Does one acknowledged member know the preimage? | Yes in v1, explicitly documented as a griefing limitation; custody still requires threshold VTXO signing. |
| How are external retries identified? | Internally by stable accepted action/request commitment; on the unchanged Bark protocol by the native payment hash or exact artifact identity. |
| Should CDK use an opaque backend operation? | No. Use one nested typed Bark operation family. |
| Should existing `MintPayment` be widened globally? | No unless implementation proves the narrow federation seams cannot work. |
| Are existing Bark Frost databases migrated? | No; start with a fresh Bark application-scoped database. |
| Are historical CDK migrations edited? | Never. Add forward-only migrations and checksum locks. |
| Is federated BDK required? | Decide at Milestone 0 for emergency-exit destination; production Bark must have a threshold-controlled exit destination either way. |

## 27. Definition of done

The federated Bark backend is production-ready only when all of the following are true:

- Bark is a distinct, validated application under the existing CDK FROST root.
- No plaintext root/application share is exported, reconstructed, or copied into Bark.
- No production code uses process-local Bark participant nonce maps.
- Ordinary FROST, virtual MuSig2, and threshold DH use authenticated bounded protocols and durable participant state.
- Every custody request is reconstructed from one exact accepted operation and attempt.
- All VTXO locators are consensus allocated.
- Incoming receive secrets survive permanent invoice-actor loss.
- Invoice creation follows accepted intent and returns one stable invoice.
- Accepted net Bark value covers the Cashu amount issued after receive fees.
- Outgoing inputs, keys, fees, expiries, and Ark artifacts are accepted before side effects.
- Outgoing actual spend never exceeds the accepted fee reserve and is derived from accepted balance effects.
- Actor and signer failover preserve the exact plan.
- Paid, failed, pending, and ambiguous results follow conservative accepted-state rules.
- Revocation, refresh, and emergency exit are implemented and tested.
- CDK journal/projection is authority; Bark SQLite can be deleted and rebuilt.
- SQLite and PostgreSQL migrations and projections agree.
- No prior migration file was modified.
- Existing federation checkpoint, backup, recovery, and materialized-state audits include Bark rows.
- Existing Cashu business logic and standalone Lightning backends remain unchanged.
- `federation-bark` is optional and absent from default builds.
- MSRV, formatting, clippy, unit, integration, doc, restart, adversarial,
  deterministic regtest, and opt-in Signet CLI payment gates pass.
- Operator status is sufficient to diagnose lag, ambiguity, signer failure, expiring funds, and recovery-required value without leaking secrets.
- An independent review covers application derivation, virtual MuSig math, threshold DH, nonce persistence, typed authorization, VTXO accounting, payment ambiguity, and emergency recovery.

## 28. First implementation slice

The first coding slice should not attempt a payment. It should prove the irreversible foundations with a small diff:

1. Add Bark application public config derived from the existing root.
2. Add stable application/locator derivation vectors and confirm they can implement Bark's `derive_public_key` expectation.
3. Add `cdk-bark` with `CdkFrostVtxoCustody::descriptor()` and `derive_public_key()` only.
4. Open a fresh Bark wallet using that external custody descriptor and a separate local operational seed.
5. Add fail-closed startup checks and a read-only readiness report.
6. Keep signing and all `MintPayment` mutation methods disabled.

That slice validates dependency compatibility, root reuse, database scoping, and Bark construction without risking funds or forcing premature consensus/schema decisions. The next slice should be durable virtual MuSig2/threshold-DH and typed authorization; only after those pass should incoming or outgoing payment work begin.
