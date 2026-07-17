# Federated Cashu checkpoint architecture

This guide describes the consolidated checkpoint, restart, and catch-up design.
AlephBFT remains the only ordering authority. A checkpoint authenticates an
already ordered and terminally applied prefix; it never orders operations,
authorizes an AlephBFT backup, or supplies a physical database snapshot.

## Object model

| Object | Owner and authority |
|---|---|
| `FederationCommittedFrontier` | Canonical journal, finalized-item, terminal-application, and AlephBFT recovery cursors. It owns exact-match, monotonic coverage, and complete-unit rules. |
| `FederationCheckpointFrontier` | Durable database projection of the committed cursors plus trusted-checkpoint and operator-audit metadata. It projects into the canonical committed frontier instead of defining parallel comparison rules. |
| `FederationCheckpointBoundary` | A complete finalized-unit boundary captured only after its accepted operations are terminally applied. |
| `FederationCheckpoint` | Versioned, federation/config-bound, cumulative signed commitment to one boundary, lineage, policy, and checkpoint-time materialized-state digest. |
| `VerifiedFederationCheckpointQuorum` | Quorum proof returned only after config, signer, threshold, signature, and checkpoint validation. Promotion and trusted recovery require this type. |
| `FederationSuffixVerifier` | The single incremental verifier used by checkpoint startup, genesis reconstruction, and peer catch-up. It buffers at most one incomplete unit and advances rolling commitments only through valid complete units. |
| `VerifiedFederationSuffix` | Opaque verifier result containing the recovered runtime and exact proved frontier. `FederationConsensusRuntime::recover_verified_suffix` is the production recovery entry point. |
| `FederationConsensusRuntime` | In-memory live consensus suffix rooted at genesis or a verified checkpoint. Submission and finalization prepare bounded deltas, persist them, then commit them without cloning the full runtime. |
| `FederationDatabaseBackupManifest` | Operator-only binding between a quiesced database backup, its exact durable frontier and trusted checkpoint, active backup metadata, schema contract, config, member, and external secret generation. It is not a consensus proof. |

The component frontier structs are value types inside
`FederationCommittedFrontier`; they do not provide a second aggregate authority.
Wire summaries are bounded projections and must be checked against a stored or
verified checkpoint before they affect trust.

## Runtime ownership

`FederationNodeRuntime` composes focused owners:

- `FederationOperationPipeline` owns the outer application-serialization lock
  shared by live finality and catch-up;
- `FederationCheckpointManager` owns recovery demand and the bounded local
  recovery-barrier proposal lease;
- `FederationReadinessCoordinator` owns local readiness transitions;
- `FederationDkgCoordinator` owns DKG configuration and admission;
- `FederationWalletReconciler` owns on-chain and FROST reconciliation handles;
- `sync_loop` owns peer selection, retries, source failover, and network paging;
  and
- `checkpoint_catch_up` owns page verification, durable staging, deterministic
  application, exact promotion, and runtime rebasing.

History catch-up and opaque current-session AlephBFT backup recovery have
different signed message kinds, request/response types, routes, size limits, and
mutation paths. Backup bytes can resume AlephBFT but cannot advance journal,
application, or checkpoint authority.

## Startup sequence

1. `cdk-mintd` opens/migrates the configured database. When a restore manifest
   is configured, it validates the restored config snapshot, trusted quorum,
   durable frontier, active backup metadata, member secrets, and schema before
   DKG reconciliation, mint construction, or routes.
2. Axum persists/validates the federation config snapshot and loads the active
   AlephBFT session, durable frontier, and optional trusted quorum.
3. A stored quorum is reverified against the configured federation. Its ID,
   session, and durable binding must match before it can become a recovery base.
4. At an exact checkpoint frontier, startup creates a verified empty suffix and
   reads no journal or finalized-item prefix.
5. With a local post-checkpoint tail, startup loads only rows at or after the
   checkpoint cursors. The shared verifier proves the tail. If a crash left a
   valid ordered tail only partially applied, the existing idempotent
   application path repairs the missing terminal suffix and rechecks the exact
   durable frontier.
6. Without a trusted checkpoint, the same verifier reconstructs from genesis.
   An empty member later downloads retained bodies in bounded checkpoint pages;
   a checkpoint alone cannot create its materialized database state.
7. Startup restores only pending operations not already accepted, reconciles
   local wallet projections, loads only the bounded active-session AlephBFT
   backup, revalidates checkpoint candidates, and computes a per-member replay
   target from the trusted checkpoint plus compatible same-session peer status.
   It counts every finalized round, including empty units, and keeps
   authoritative routes behind readiness until every target component is met.
   A generation-aware watcher prevents an old or halted session from publishing
   readiness. Automatic rotation clears the prior session's target.

Normal startup performs no full materialized-state scan. The full audit is an
explicit operator command.

## Catch-up sequence

1. Peer sync compares signed status/summary metadata and selects the next stored
   quorum checkpoint on the local checkpoint lineage. A recovery checkpoint
   anchored to an older scheduled checkpoint is ignored after a newer scheduled
   checkpoint is trusted, even if the stale recovery frontier is later; the
   member requests a replacement recovery barrier on the current lineage.
2. Network code requests one bounded history page from one authenticated peer.
   HTTP body limits apply before JSON decoding; response item and canonical-byte
   limits apply again after decoding.
3. The page must name the selected target checkpoint, begin at the exact durable
   local cursors, contain contiguous operation commitments, and contain only
   complete finalized units except for an explicitly incomplete trailing page.
4. After network work finishes, catch-up acquires the application lock and then
   the consensus lock. The shared verifier extends the already verified local
   suffix and proves that the page is a prefix of, or exactly reaches, the
   target checkpoint.
5. Finalized bodies and accepted journal entries are persisted, deterministic
   business effects are applied idempotently, and the in-memory runtime is
   replaced with the verifier result.
6. Incomplete pages release the locks and fetch the next page. No aggregate
   response grows with total checkpoint history.
7. A complete page may promote the target only when the durable journal,
   finalized-item, and terminal-application frontiers exactly equal the verified
   checkpoint. Promotion is transactional and the runtime is rebased to an
   empty suffix at the promoted checkpoint.
8. If the active AlephBFT backup is missing, its separate bounded recovery path
   may fetch it from a compatible peer. Invalid/unavailable sources fail over
   without changing application or checkpoint trust.

## Checkpoint lifecycle

Scheduled checkpoints are sparse consequences of finalized progress. After a
complete unit is terminally applied, the checkpoint manager evaluates the
consensus-configured journal/finalized intervals, persists one deterministic
candidate, collects member signatures over that exact payload, verifies a
distinct configured-member quorum, and transactionally promotes the quorum.
Unchanged polling does not recreate signatures or rescan history.

When sustained traffic prevents a lagging member from reaching the scheduled
tip, the member requests recovery. One local member proposes a versioned
recovery barrier through ordinary AlephBFT ordering under a bounded lease. The
barrier's complete-unit terminal boundary deterministically creates the
recovery candidate. The recovery checkpoint uses the same signature, quorum,
verification, promotion, and lineage machinery as a scheduled checkpoint; it
does not introduce a second ordering path.

## Lock order and publication

Peer requests always complete before runtime mutation locks are acquired. Short
readiness and recovery-proposal locks are independent. The mutation order is:

1. operation application;
2. consensus runtime; and
3. database transaction.

A caller must not acquire an earlier lock while holding a later one. Database
reads needed to build a deterministic plan may occur under the consensus lock;
network requests may not. Submission and finalization prepare non-mutating
deltas, durably persist first, and publish to the runtime only after persistence
succeeds. Checkpoint authority advances only in the database promotion
transaction after every committed frontier is exact.

## Failure recovery

- A crash after finalized/journal persistence but before application leaves the
  trusted checkpoint unchanged. Startup verifies the suffix, idempotently
  applies only its missing terminal portion, and rechecks the frontier.
- A catch-up page or promotion failure can leave only authenticated bodies below
  the old trusted checkpoint. Retry resumes from durable cursors; trust never
  moves ahead of application state.
- Wrong signatures, signers, federation/config bindings, lineage, cursors,
  digests, units, envelopes, or outcomes fail before promotion.
- Invalid current-session backup data may prevent AlephBFT recovery but cannot
  authorize mint state.
- A materialized-state audit mismatch durably latches the member. No restart,
  background task, peer response, or operator clear command removes it. Recovery
  uses a known-good checkpoint-bound database backup or fresh genesis replay,
  followed by the explicit operator audit.

Version 1 retains journal and finalized bodies from genesis and protects the
active AlephBFT backup. Peer-provided physical state snapshots remain outside
this architecture.
