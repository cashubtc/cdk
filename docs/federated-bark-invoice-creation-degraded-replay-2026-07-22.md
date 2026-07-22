# Federated Bark invoice-creation failure and replay-compatibility report

Date: 2026-07-22

Status: two receive-replay defects are confirmed. The concurrency defect has a
deterministic local reproduction and a locally verified containment fix. A
second, persistent commitment-version incompatibility was isolated through a
staged production replay diagnostic; its backward-compatible verifier is
implemented and awaiting validation and deployment.

Scope: five-member Nocean federation using the federated Bark BOLT11 backend.

Related documents:

- `docs/federated-bark-outgoing-lightning-failure-2026-07-20.md`
- `docs/federated-cashu-bark-consensus-review-2026-07-21.md`
- `docs/federated-cashu-bark-optimization.md`

This report deliberately omits invoices, payment hashes, action IDs, quote IDs,
preimages, server addresses, federation identifiers, and custody material. The
retained amounts, phases, operation indexes, timestamps, aggregate database
state, and code paths are sufficient to reproduce and fix the issue.

## Executive summary

The latest BOLT11 invoice-creation request did not reach invoice preparation.
It failed at the federated Bark readiness gate before CDK accepted a receive
intent, called Ark to create an invoice, or persisted an ordinary mint quote.
There is no new 1,000-sat BOLT11 mint quote in any of the five member databases.

All five federation members are consensus-healthy and fully caught up through
operation index 119. Their Bark projections also report local frontier 119, so
the failure is not consensus lag. Nevertheless, every member reports:

```text
readiness=degraded
degraded_reason=projection_invalid
projection_lag_operations=0
```

The latest outgoing 1,000-sat payment action remains accepted at
`payment_initiated`, reserving 1,025 sats. Only one member failure observation
has reached consensus, while the policy requires four matching observations.
Every member-local Bark database contains the exact corresponding
`PaymentInitiated` checkpoint. This is evidence against a currently missing or
mis-keyed checkpoint for that outgoing action.

One confirmed defect is an uncoordinated member-local incoming receive
projection mutation racing terminal incoming-receive cleanup. Production
recorded its exact signature on one member:

```text
Federated Bark accepted receive evidence replay failed
error=accepted receive is not materialized
```

The Bark replay method first creates the receive checkpoint, performs several
awaited imports and movement operations, and then reads the checkpoint again.
A concurrent replay of the terminal `ReceiveClaimed` record, or live terminal
acknowledgement, removes that same checkpoint. CDK had separate locks for some
incoming and maintenance drivers, but no lock at the member-local Bark runtime
boundary. Therefore live receive processing and any historical replay could
mutate the same receive checkpoint concurrently. A deterministic in-memory
Bark persistence harness reproduces the exact error under this interleaving.

Once any transient replay error sets `ProjectionInvalid`, the health state is
sticky. It is cleared only after another complete journal replay and maintenance
replay succeeds. New invoice creation, outgoing-payment observation, incoming
settlement, and automatic maintenance all call the same readiness gate first.
Consequently, the degraded state blocks the work that could otherwise reconcile
or terminalize it. Production maintenance has logged `Internal Error` with no
progress for more than 100 consecutive rounds.

The concurrency fix adds one mutex to each `FederatedBarkRuntime` and holds it
across every operation that reads or mutates member-local incoming receive
projection state. This includes accepted-history application, live HTLC
observation/preparation, claim planning/execution, and terminal
acknowledgement. It changes no federation consensus, evidence, payment, or
custody semantics; it only removes invalid local interleavings.

After deploying that fix, replay progressed beyond the missing-checkpoint race
but consistently rejected all three historical `ReceiveHtlcsReady` records.
Bounded stage diagnostics proved that artifact hashes, canonical VTXO decoding,
VTXO identities, values, expiries, Bark evidence reconstruction, and request
commitments all match. Only the result commitment differs.

Code history identifies the persistent incompatibility. Historical records use
`cdk/bark/receive-htlc-evidence/v1`, which commits the reconstructed Bark
evidence fields and serialized VTXOs. A later change replaced it with
`cdk/federation/bark-receive-htlc-package/v1`, which commits the request hash,
CDK VTXO transitions, and artifacts. The accepted operation version remained
unchanged and replay retained no legacy verifier. New binaries consequently
validated old, valid records with the new formula and failed closed on every
startup. The minimal compatibility fix accepts the exact historical digest
only for transitions without an evidence certificate; certified evidence and
newly generated transitions remain bound to the current package digest.

## User-visible failure

The latest request was an incoming operation: create a Lightning invoice that
can later fund a Cashu mint quote. It is distinct from the earlier outgoing
operation that attempted to pay a supplied Lightning invoice.

The request path is:

1. Derive the deterministic federated quote identity.
2. Enter `accepted_invoice`.
3. Require a ready Bark projection.
4. Create and accept a receive intent.
5. Ask the selected actor's Bark wallet to create the Ark invoice.
6. Accept `ReceivePrepared`.
7. Persist the ordinary CDK mint quote.

The request stopped at step 3. In
`crates/cdk-axum/src/federation/bark_incoming.rs`, `accepted_invoice` calls
`federation_bark_ready_projection` before creating a fresh receive intent. A
degraded result is mapped to the generic Bark-unavailable error. Because the
request never reached steps 4-7:

- no new receive action exists in the accepted projection;
- no new receive checkpoint exists in Bark;
- Ark was not asked to create the invoice; and
- no new ordinary 1,000-sat BOLT11 mint quote exists.

The mint therefore did not receive or retain the attempted invoice request as a
durable quote.

## Production evidence

### Federation and Bark health

All five members report the same accepted state:

| Field | Observed value |
|---|---:|
| Consensus health | healthy |
| Accepted operations | 120 |
| Latest operation index | 119 |
| Bark accepted frontier | 119 |
| Bark local frontier | 119 |
| Bark projection lag | 0 |
| Bark readiness | degraded |
| Bark degraded reason | `projection_invalid` |
| Ambiguous Bark actions | 1 |
| Ambiguous value | 1,025 sats |
| Active receive actions | 0 |

The accepted action and VTXO projections agree across all five members. This is
not a consensus split, catch-up failure, or one-member database fork.

### Latest outgoing action

The latest accepted action sequence is:

| Operation index | Accepted operation |
|---:|---|
| 114 | 1,000-sat melt quote |
| 115 | melt request |
| 116 | Bark outgoing action plan |
| 117 | `SendHtlcsCosigned` |
| 118 | `PaymentInitiated` |
| 119 | one member observation reporting `Failed` |

The outgoing fee reserve is 25 sats, so the accepted pending-spend value is
1,025 sats. The action needs four matching terminal observations. It currently
has one. It is therefore unresolved rather than consensus-failed.

Every member-local Bark database has the checkpoint for this exact accepted
action. On every member:

- the checkpoint key matches the action ID stored in its payload;
- the checkpoint variant is `LightningSend`; and
- its progress is `PaymentInitiated`.

This does not prove that every replay sub-operation is valid, but it rules out
the simplest explanation that the latest action is degraded because its base
checkpoint is absent.

### Completed incoming actions

Consensus contains three completed incoming actions:

| Receive | Origin index | Prepared | HTLC ready | Claim planned | Preimage disclosed | Claimed |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 0 | 1 | 6 | 7 | 8 | 9 |
| 2 | 41 | 42 | 47 | 48 | 49 | 50 |
| 3 | 73 | 74 | 79 | 80 | 81 | 82 |

All five Bark databases contain three successful receive movements and three
settled-receive records. They contain no receive action checkpoints, which is
the expected result after terminal acknowledgement. The missing checkpoint is
therefore correct at rest; replay must tolerate or correctly reconstruct this
terminal state.

### Quote persistence

The only persisted BOLT11 mint quotes on each member are two earlier 50,000-sat
quotes and one earlier 250-sat quote. There is no later 1,000-sat BOLT11 mint
quote. This confirms that the latest invoice-creation request failed before
ordinary quote persistence rather than creating an invoice and losing its HTTP
response.

## Timeline

All timestamps are UTC on 2026-07-21.

| Time or index | Event |
|---|---|
| 20:00 | The third accepted incoming receive settled successfully. |
| Before 20:04 | Automatic Bark maintenance had already entered repeated no-progress/error backoff. The retained log at 20:04 was failure 52. |
| 22:40-22:42 | Earlier outgoing work changed local Bark send checkpoints; all members remained database-consistent. |
| 23:22-23:23 | The latest 1,000-sat outgoing action advanced through accepted `PaymentInitiated`; all five members persisted the matching local checkpoint. |
| Index 119 | Member 3 submitted one accepted `Failed` observation. No four-member terminal quorum followed. |
| 23:24:48 | A request routed to member 4 triggered full readiness reconciliation and logged `accepted receive is not materialized` while replaying accepted receive evidence. |
| Afterward | All five members reported `projection_invalid` at zero lag. Maintenance continued failing every five minutes at the maximum backoff. |
| Latest request | Incoming invoice creation failed at the readiness gate; no receive intent, invoice, or mint quote was persisted. |

## Confirmed causal chain

The user-visible cause is confirmed even though the first event that latched
`ProjectionInvalid` is not recoverable from current logs:

1. A Bark replay path marked the runtime `ProjectionInvalid`.
2. The accepted and local frontiers later converged at index 119, but the health
   reason remained set.
3. `accepted_invoice` called `federation_bark_ready_projection`.
4. That helper attempted a full applied-journal replay because readiness was
   degraded.
5. The replay did not complete and therefore did not reach the only
   `clear_degraded(ProjectionInvalid)` call.
6. `accepted_invoice` returned Bark unavailable.
7. No receive intent, Ark invoice, or mint quote was created.

Two observations show that the current degraded flag is at least partly stale:

- every member is at the required frontier; and
- every member has the exact local checkpoint and phase for the only active
  action.

The runtime may still contain a replay defect, but `projection_invalid` no
longer identifies a specific persistent row mismatch.

## Root cause and remaining candidates

### Confirmed root cause: receive result commitment changed without migration

Confidence: confirmed by production record inspection, bounded replay
diagnostics, and code history.

The three historical accepted `ReceiveHtlcsReady` transitions were created with
this result commitment:

```text
SHA256(
  "cdk/bark/receive-htlc-evidence/v1" ||
  action_id || payment_hash || destination ||
  gross_amount || net_amount || fee || minimum_expiry ||
  length_prefixed_serialized_vtxos
)
```

Commit `53c98ed36` changed new receive transitions and replay verification to a
canonical package commitment:

```text
domain_hash(
  "cdk/federation/bark-receive-htlc-package/v1",
  canonical_json(minimum_expiry, request_hash, transitions, artifacts)
)
```

Both formulas are internally sound, but the change did not bump
`FEDERATION_BARK_OPERATION_VERSION`, migrate accepted journal records, or retain
a legacy replay verifier. Production therefore retains version-1 records made
with the first formula while the current binary assumes every version-1 record
uses the second.

The diagnostic sequence narrowed the failure without logging payment or
custody identifiers:

1. action-transition validation passed;
2. VTXO artifact lookup and SHA-256 verification passed;
3. canonical Bark VTXO deserialization and byte round-trip passed;
4. VTXO ID, amount, and expiry comparisons passed;
5. Bark receive evidence reconstruction passed;
6. the request commitment passed; and
7. only the result commitment comparison failed.

The backward-compatible rule is deliberately narrow: replay first requires the
current package digest. If it differs, an uncertified transition may match the
exact historical evidence digest. A transition carrying an evidence
certificate never receives the fallback because certificates were introduced
with, and explicitly bind, the package digest. This restores replay of records
that were valid when accepted without weakening current certified evidence.

### Confirmed defect: receive replay races terminal checkpoint deletion

Confidence: confirmed by code inspection and deterministic local reproduction.
This defect directly produces the specific production error.

`reconcile_federation_bark_runtime` is called from multiple independent paths:

- startup reconciliation;
- post-consensus operation application;
- request-time `federation_bark_ready_projection`;
- automatic maintenance through the same readiness helper; and
- the authenticated operator reconcile endpoint.

There was no common member-local receive-projection mutex. The incoming and
maintenance mutexes protect their own drivers but do not serialize
post-application replay, request-time full replay, operator replay, and live
terminal acknowledgement with one another.

In the Bark dependency, incoming HTLC replay does this:

1. `apply_lightning_receive` creates or validates the action checkpoint.
2. Import each accepted HTLC VTXO.
3. Create or reload the movement.
4. Read the action checkpoint again.
5. Advance it to `HtlcsReady` if needed.

Steps 1 and 4 are separated by many `await` points. Terminal acknowledgement
does this independently:

1. Read the claimed checkpoint.
2. Validate `Claimed`.
3. Delete the checkpoint.

A valid historical-replay interleaving is:

```text
replay A: materialize ReceivePrepared/checkpoint
replay B: replay through ReceiveClaimed
replay A: begin ReceiveHtlcsReady replay and recreate/read checkpoint
replay B: acknowledge ReceiveClaimed and delete checkpoint
replay A: perform the second checkpoint read -> missing
```

The same race can occur when the cleanup side is live incoming processing
rather than a second full replay. That broader case is why serializing only
`reconcile_federation_bark_runtime` is insufficient.

The exact failure text comes from that second read in the Bark federation
facade. A genuinely absent checkpoint before the method starts is self-healed
by its initial `apply_lightning_receive` call, so the only code-consistent way
to observe the checkpoint missing at the second read is deletion after that
materialization. The local harness forces precisely that ordering and obtains
the identical error text.

### Candidate 2: sticky, all-or-nothing degradation recovery

Confidence: high. This is confirmed as the mechanism that turns a transient
replay error into an outage.

`reconcile_federation_bark_runtime` clears `ProjectionInvalid` only after:

1. every applied journal entry has replayed;
2. all historical maintenance actions have replayed;
3. the local frontier has been recorded.

Any unrelated failure before the end leaves the old reason latched. The health
record contains only a bounded enum, not the failed phase, operation index, or
error class, so a later operator cannot distinguish an active deterministic
divergence from a transient race that already disappeared.

The clear should be scoped to the invariant it proves. Successful deterministic
journal replay should clear `ProjectionInvalid` even if a separate maintenance
rehydration step reports its own recoverable error. Conversely, a persistent
projection mismatch should record a new generation-specific failure and remain
closed.

### Candidate 3: degraded readiness blocks the recovery lane

Confidence: high. This is confirmed by the call graph and current action state.

The following internal progress paths require `federation_bark_ready_projection`
before doing any work:

- outgoing payment polling and terminal observation;
- incoming payment polling and claim progress;
- receive-intent retry; and
- automatic maintenance.

The latest outgoing action has one accepted failure observation but needs four.
Once every member is degraded, the remaining observers fail at the readiness
gate before polling the already-accepted action. This is a liveness cycle:

```text
projection error -> degraded readiness -> no terminal observations
-> action remains ambiguous -> no normal convergence -> degraded readiness
```

Starting new money-moving actions should remain fail-closed. Replaying accepted
facts and gathering read-only terminal evidence need a separate recovery
readiness policy so they can converge the system.

### Candidate 4: replay or maintenance sub-error is hidden

Confidence: high as an observability defect; unknown as the original trigger.

Several reconciliation branches use `.is_err()` or map all Bark errors to one
generic CDK error. Production therefore records `projection_invalid` or
`Internal Error` without identifying whether failure occurred in:

- send-plan materialization;
- HTLC evidence replay;
- payment-initiation replay;
- successful/failed send settlement;
- refresh-plan replay;
- refresh-result replay; or
- terminal receive replay.

The incoming replay path has one useful warning and exposed the missing
checkpoint error. Equivalent bounded diagnostics are absent from most outgoing
and maintenance paths. The initial event that set `ProjectionInvalid` on all
members cannot be selected from retained production logs.

### Candidate 5: non-atomic Bark replay/acknowledgement contract

Confidence: medium-high. This is the dependency-level form of the confirmed
race above.

Even if CDK serializes full reconciliation, Bark exposes multi-call methods
whose correctness assumes no concurrent terminal acknowledgement for the same
action. The database operations are individually durable but not atomic as an
action-level state transition. Another caller of the Bark facade could recreate
the same race.

The robust dependency-level fix is per-action serialization or a terminal
tombstone that makes replay after acknowledgement an idempotent success. The
implemented CDK runtime mutex is the smallest safe containment fix, but Bark
should eventually own this invariant too.

## Deterministic local reproduction

The regression harness uses Bark's real in-memory `BarkPersister` and the real
`LightningReceive` checkpoint representation. It does not need Ark, Bitcoin,
Lightning liquidity, or a running five-member federation because the failure
is wholly inside the member-local persistence interleaving.

The unguarded half of the test coordinates two tasks with explicit barriers:

1. Replay upserts a valid `LightningReceive` checkpoint.
2. Replay pauses at the same logical awaited window that exists between Bark's
   first and second checkpoint access.
3. Terminal cleanup removes the checkpoint.
4. Replay performs its second lookup.
5. The lookup returns the exact production error:
   `accepted receive is not materialized`.

The guarded half runs the same replay and cleanup operations under the new
receive-projection mutex. Cleanup attempts to start as soon as replay
materializes the checkpoint, but it cannot enter until replay finishes its
second read. Replay succeeds, cleanup then removes the terminal checkpoint, and
the final durable state remains correct.

This harness establishes both halves of the causal claim: the existing
interleaving is sufficient to reproduce the incident signature, and mutual
exclusion prevents that interleaving without retaining stale checkpoints.

## Implemented minimal fix

`FederatedBarkRuntime` now owns a Tokio mutex named `receive_projection`. The
runtime acquires it around every public operation that can persist, reload, or
remove incoming receive state:

- materializing `ReceivePrepared`;
- applying accepted `ReceiveHtlcsReady` evidence;
- polling/preparing/hydrating live HTLC evidence;
- reloading accepted member evidence;
- planning and validating a receive claim;
- applying accepted claim plans and transitions;
- executing a receive claim; and
- acknowledging terminal claim completion.

The lock is member-local, in-memory, and intentionally covers all receive
actions rather than only one call site. A per-action lock would allow more
parallelism but requires a lifecycle-safe lock registry and additional cleanup
logic. Incoming receive volume is low, and correctness is more important than
that premature concurrency. The mutex is also placed below Axum so startup,
request, post-application, operator, and live worker paths cannot bypass it.

The invoice-preparation Ark RPC is not locked because Bark documents and
implements it as having no local projection mutation. Pure accepted-artifact
validation is likewise outside the critical section. A private validation
helper avoids recursively acquiring the non-reentrant mutex during claim
execution.

Replay now also recomputes both receive-result commitment formats. The current
package commitment is checked first. The legacy evidence commitment is accepted
only when the transition has no evidence certificate, exactly matching the
records created before certified package commitments existed. New transition
creation is unchanged and always emits the current package commitment.

Local verification:

```text
cargo test -p cdk-bark --features federation-bark
18 passed; 0 failed

cargo check -p cdk-axum --features federation-bark
succeeded

cargo clippy -p cdk-bark --features federation-bark --lib -- -D warnings
succeeded
```

The workspace-wide formatting check is currently blocked by pre-existing
format differences in the local Bark path dependency. The modified CDK source
file was formatted directly. Bark also emits two pre-existing compiler warnings
in its chain source; neither is related to this change. Clippy with
`--all-targets` reaches the pre-existing `items_after_test_module` lint in this
runtime file; the production-library Clippy target succeeds with warnings
denied.

## Why normal retries do not heal the incident

Each failed user request sees `ProjectionInvalid` and launches another complete
replay. Without serialization, multiple requests increase the chance of the
receive replay/acknowledgement race. If a replay fails for any reason, it exits
before clearing the degradation reason.

The maintenance task is not an independent healing path. It also calls
`federation_bark_ready_projection` first, maps the failure to `Internal Error`,
and backs off. The repeated maintenance warnings are therefore primarily a
symptom of the degraded latch, not evidence of 100 distinct Ark or chain
failures.

A restart may temporarily remove the in-memory latch and startup replay has
less concurrency, but restart is not a fix. Traffic or post-application replay
can reproduce the race, and an unresolved `PaymentInitiated` action remains.

## Impact and safety assessment

- New BOLT11 mint invoices cannot be created while all members fail the Bark
  readiness gate.
- The latest attempted incoming invoice was not created or persisted.
- The latest outgoing 1,000-sat action remains ambiguous at
  `PaymentInitiated`; 1,025 sats remain pending-spend.
- There is one accepted failure observation, not enough evidence to declare the
  outgoing payment failed.
- No consensus divergence was found. All five accepted projections match.
- No member-local divergence was found for the active outgoing checkpoint.
- The three prior incoming payments are recorded as successfully settled on all
  members.
- Manual database deletion or state rewriting is unsafe and unnecessary.

The fail-closed behavior protected funds from starting new work on a runtime it
believed invalid. The bug is that a transient/local replay problem can become a
permanent federation-wide liveness failure even after durable state converges.

## Remaining correction plan

### Implemented: serialize member-local receive projection

The minimal fix is implemented at the `FederatedBarkRuntime` boundary. One
member-local mutex covers both historical replay and live incoming operations,
including terminal acknowledgement. This scope is necessary because a
reconciliation-only lock would still allow live cleanup to race replay.

### Implemented: verify legacy receive commitments during replay

Historical uncertified `ReceiveHtlcsReady` records may match the exact
`receive-htlc-evidence/v1` digest that was authoritative when they were
accepted. Certified transitions and all new transition generation continue to
require `bark-receive-htlc-package/v1`. A regression test verifies that the
legacy digest is rejected whenever a certificate is present.

### Priority 1: make terminal receive replay idempotent inside Bark

In Bark, protect receive replay and terminal acknowledgement with a per-action
lock or transaction-like state machine. Acceptable designs include:

- retain a compact terminal receive tombstone after deleting the retry
  checkpoint;
- make acknowledged terminal replay validate the settled-receive record and
  return success without recreating an intermediate checkpoint;
- serialize all operations for one federation action ID; or
- defer checkpoint deletion until the outer reconciliation pass has completed.

The invariant must be: once `ReceiveClaimed` is accepted, replaying any accepted
prefix or the complete action cannot produce `accepted receive is not
materialized`.

### Priority 1: separate recovery readiness from admission readiness

Keep new invoice creation and new outgoing plans blocked while projection
integrity is unproven. Allow these bounded recovery operations to proceed:

- deterministic replay of already-accepted records;
- read-only polling of already-initiated payments;
- submission of matching terminal observations; and
- terminal acknowledgement after accepted evidence.

This lets the four-observer policy terminalize the current outgoing action
without permitting new value-moving plans.

### Priority 1: make degradation scoped and recoverable

Record at least:

- reason;
- replay stage;
- semantic phase;
- operation index;
- whether the error is deterministic or retryable; and
- the frontier generation at which it occurred.

Do not log payment identifiers, invoices, preimages, or artifact bytes.

Clear `ProjectionInvalid` immediately after a serialized deterministic journal
replay proves the projection. Track maintenance replay failures under a
separate bounded reason. Do not let a stale reason from an older frontier mask
the successful validation of a newer frontier.

### Priority 1: preserve bounded causal errors

Replace `.is_err()` and blanket error collapsing at reconciliation boundaries
with redacted structured diagnostics such as:

```text
stage=receive_htlcs_ready phase=receive_htlcs_ready index=<n>
class=checkpoint_missing retryable=true
```

The current generic `Internal Error` is insufficient for production recovery.

### Priority 2: consolidate replay by current action head

Full journal replay currently replays every historical prefix and then deletes
terminal receive checkpoints again. Consider replaying a validated consolidated
action state derived from the deterministic projection:

- materialize only the accepted terminal receive result for claimed actions;
- replay non-terminal prefixes only for active actions; and
- use append-only artifacts to validate the consolidated result.

This reduces work and removes intermediate-state races without weakening
consensus validation.

## Verification status and remaining regression coverage

Completed in this change:

1. Deterministically reproduce the checkpoint deletion window using Bark's
   real in-memory persistence layer and receive checkpoint type. Assert the
   exact production error without serialization.
2. Run the identical ordering under the member-local receive mutex. Assert the
   replay read succeeds, terminal cleanup runs afterward, and no checkpoint is
   retained.
3. Run the complete `cdk-bark` feature test suite and compile the Axum
   federation-Bark integration.

Recommended follow-up coverage:

1. Run two complete reconciliations concurrently over a journal containing
   `ReceivePrepared -> ReceiveHtlcsReady -> ClaimPlanned ->
   ReceivePreimageDisclosed -> ReceiveClaimed`. Both must succeed, and the final
   local state must contain the settled receive with no retry checkpoint.
2. Insert a barrier after `apply_lightning_receive` and race
   `acknowledge_lightning_receive_claim` against the second checkpoint read.
   The result must be idempotent success, never checkpoint-missing.
3. Replay the same terminal receive history repeatedly after checkpoint
   cleanup. Every pass must produce identical local state.
4. Call request-time readiness, post-application reconciliation, maintenance,
   and operator reconciliation concurrently. Verify that only one local replay
   mutates Bark at a time.
5. Inject a transient replay error that sets `ProjectionInvalid`, remove the
   fault, and verify that the next serialized replay clears the reason at the
   same frontier.
6. Inject a maintenance-only replay error after successful deterministic
   journal replay. Verify that projection readiness is not mislabeled
   `projection_invalid` and that the maintenance reason is separately visible.
7. Start from `PaymentInitiated` with one accepted `Failed` observation and a
   degraded admission state. Verify that selected observers can still poll and
   reach the required terminal quorum while creation of new actions remains
   blocked.
8. Verify that a rejected invoice-creation request performs no Ark RPC and
   persists no receive intent or ordinary mint quote.
9. Run the above tests with five member databases and compare action heads,
   VTXO states, checkpoints, movements, and settled-receive records.
10. Verify logs expose only bounded stage/index/error-class fields and never
    invoices, hashes, quote IDs, preimages, server addresses, or custody data.

## Production recovery constraints

Recovery should be performed only after capturing database backups and should
use code-level replay, not manual row edits.

The safe operational sequence after a fix is available is:

1. Quiesce public Bark payment traffic.
2. Back up all five CDK, Bark, and receive-secret databases.
3. Deploy the serialized/idempotent replay fix.
4. Reconcile one member at a time and verify frontier equality and ready health.
5. Allow the accepted `PaymentInitiated` action to gather terminal observations.
6. Verify all five members agree on its terminal action head and VTXO state.
7. Re-enable invoice creation.
8. Create a small test invoice and verify `ReceiveIntent`, `ReceivePrepared`,
   quote persistence, payment settlement, and terminal checkpoint cleanup.

Do not manually mark the outgoing action failed, delete its checkpoint, release
its pending VTXOs, or insert a mint quote. One failure observation is not the
required terminal evidence.

## Acceptance criteria

The incident is fixed when all of the following hold:

- every member reports Bark ready at the accepted frontier;
- repeated and concurrent full replay is deterministic and idempotent;
- terminal receive cleanup cannot race accepted-prefix replay;
- the current outgoing action reaches a quorum-backed terminal state;
- recovery progress remains possible while new-action admission is degraded;
- a new 1,000-sat invoice request creates one accepted receive intent, one
  accepted prepared invoice, and one ordinary mint quote; and
- no sensitive payment or deployment material appears in logs or diagnostics.
