# Federated Cashu Performance and Issue Report

Date: 2026-07-15

## Relationship to the federation implementation plan

This document is the investigation, remediation, and verification plan for the
performance, reliability, and robustness work that remains after the core
federation implementation. It is governed by
[federated-cashu-implementation-plan.md](federated-cashu-implementation-plan.md).
The implementation plan is the design and safety contract; this document must
not weaken or work around its invariants to make a benchmark pass.

The most important concepts carried forward from the implementation plan are:

- Federation mode preserves the public Cashu wallet-to-mint protocol while
  requiring consensus ordering and BLS threshold authorization. Standalone CDK
  behavior remains unchanged unless federation mode is explicitly enabled.
- Consensus must precede signature-share creation and externally visible
  safety-critical state changes. No member may independently authorize mint,
  swap, or melt results.
- The federation journal is the authoritative operation history. Operations
  require canonical identities, durable ordering, deterministic and idempotent
  application, exact replay, and matching materialized state.
- Local time, randomness, unordered data, network I/O, and member-local backend
  results must not influence deterministic application unless consensus commits
  the relevant value.
- Payment side effects use the designated replay-safe, quorum-authorized flow.
  Member-local payment or database state is not federation evidence.
- Wallet quorum behavior belongs in the centralized federation connector:
  member fanout, view agreement, error classification, fallback, share
  validation, aggregation, and ambiguous-result handling must not be
  reimplemented independently in each wallet flow.
- The mint domain continues to own Cashu validation and business state changes.
  Federation orchestration owns ordering, quorum authorization, replay, and the
  point at which shared domain behavior runs. Public handlers remain thin.
- Startup, catch-up, DKG, rotation, roster, keyset, transcript, configuration,
  and live-session mismatches fail closed. Local consistency alone is not proof
  that a member is current or able to participate in consensus.
- Federation keysets are v3/BLS-only and must reject aggregate-key, public-share,
  local-share, or keyset-version mismatch without falling back to standalone
  behavior.
- Prefer simplification and existing CDK patterns before adding new types,
  queues, retries, background systems, or parallel implementations. Do not hide
  liveness failures with larger timeouts, unbounded queues, retry storms, or
  serialized tests.
- Federation development remains cut-through because no production federation
  format has been deployed. Change experimental federation members, clients,
  schemas, tests, and fixtures together instead of adding compatibility layers.
- Temporary instrumentation is allowed for investigation only when it cannot
  influence protocol behavior. Remove it before production unless it becomes
  documented, opt-in or low-overhead, bounded observability that exposes no
  secrets or sensitive payloads.

## Milestone working agreement

Work proceeds one milestone at a time in the order below unless a documented
dependency or newly confirmed critical issue requires reprioritization.

Every milestone follows the same evidence-first gate:

1. Reproduce or invalidate the reported behavior under a bounded, documented
   workload.
2. Add temporary measurement or tracing only where existing evidence is
   insufficient.
3. Identify the causal mechanism with hard process, journal, database, network,
   client, or code-path evidence.
4. Do not implement a fix while the milestone remains unconfirmed or the root
   cause is only speculative.
5. Once the cause is demonstrated, choose the smallest fix that fits the
   existing owning layer and reuses established CDK patterns.
6. Verify the fix against the minimal regression, relevant adverse cases,
   restart/replay behavior, and unchanged standalone behavior.
7. Remove temporary instrumentation unless it satisfies the production
   observability rules above.

After every milestone, all code, tests, verification results, and this document
must be updated together and committed to git with a conventional commit. The
document update must record the final status, root-cause evidence, chosen fix,
commands run, meaningful results, instrumentation removed or retained, and any
tests not run. A milestone is not complete until that commit exists.

If investigation or stress work exposes a previously unidentified networking,
consensus, mint, wallet, storage, or operational issue, do not fold an
unconfirmed fix into the active milestone. Append a new `U` milestone after all
existing numbered and `U` milestones at the end of the milestone execution
plan. Record which milestone exposed it, the raw evidence, why it might matter,
alternative explanations, and the stricter validation required. Its status
remains **Unconfirmed** and no fix may begin until independent reproduction
establishes that the issue is real.

### Status values

| Status | Meaning |
| --- | --- |
| **Pending** | Defined work has not started because earlier ordered milestones still own the active investigation. |
| **Unconfirmed** | Observation or hypothesis exists, but a bounded independent reproduction has not established a real defect. |
| **Confirmed historically; revalidation pending** | A prior revision reproduced the failure, but current code must reproduce it again before any fix is attempted. |
| **Confirmed; investigation pending** | The failure is reproducible, but the causal mechanism is not yet proven. |
| **Root cause confirmed; fix not started** | Evidence identifies the mechanism and owning layer; implementation may begin. |
| **Fix in progress** | A narrow fix is being implemented with its regression. |
| **Verification in progress** | The fix exists and the required adverse, restart, and compatibility checks are running. |
| **Complete** | Success criteria pass, temporary instrumentation is resolved, the document is current, and a milestone commit exists. |
| **Blocked** | A named prerequisite prevents useful progress; the blocking evidence and required external change are documented. |

Checked boxes record evidence or work already completed. Unchecked boxes are
requirements for milestone completion; they are not assumptions that the
proposed fix is correct.

Each milestone intentionally has one short work checklist. Treat a checkbox as
a phase gate, not as an invitation to exhaust every conceivable diagnostic or
test permutation. Once a minimal reproduction proves the cause, stop
investigating, implement the smallest fitting fix, run the minimum tests that
prove the success criteria and protect adjacent invariants, then close and
commit the milestone. The scenarios listed under **Acceptance** define what the
result must prove; they are not additional independent tasks.

### Milestone roadmap

| Order | Milestone | Current status | Detailed issues |
| ---: | --- | --- | --- |
| 1 | Fresh-member recovery, readiness, and live-session convergence | Complete | 1, 2 |
| 2 | Cancellation-safe server application and bounded wallet completion | Complete | 3, 4, 5 |
| 3 | Exactly-once melt completion under repetition and member faults | Complete | 6 |
| 4 | AlephBFT cadence, network backpressure, and HTTP liveness | Complete | 8 |
| 5 | Signed private-status liveness and bounded operator reporting | Complete | 7, 12 |
| 6 | Federation discovery and import completion | Complete | 9 |
| 7 | Wallet token-preparation and fragmentation scaling | Complete | 10 |
| 8 | Controlled local and remote capacity characterization | Pending | 11 and performance baselines |
| 9 | Production release validation and operator readiness | Pending | Remaining release work |
| U1 | Validate payment-report quote-age semantics | Unconfirmed | 13 |
| U2 | Validate network-pump retry duplication | Unconfirmed | Secondary Issue 8 hypothesis |
| U3 | Validate stale or unusable AlephBFT backup replacement and fallback | Partially confirmed; stale-backup fix verified | Found during Milestone 1 and U4 verification |
| U4 | Validate catch-up certificate deadlock with two stale members | Resolved and verified | Found during Milestone 1 two-member-behind verification |
| U5 | Validate paged catch-up full-tip digest mismatch | Closed; hypothesis not reproduced | Found during Milestone U4 certificate-boundary tracing |
| U6 | Validate large-history catch-up page-cost scaling | Unconfirmed | Found during Milestone U5 scale verification |
| U7 | Validate BLS transaction-identifier persistence | Resolved and verified | Found during Milestone 2 cancellation recovery |
| U8 | Validate finalized-item catch-up response-count mismatch | Confirmed transient mismatch; cause unconfirmed | Found during Milestone 2 parallel receive verification; reproduced during U9 verification |
| U9 | Validate intermittent 60-second melt-quote creation latency | Resolved and verified | Found during Milestone 3 process verification |
| U10 | Validate parallel quote-state observation failures | Unconfirmed | Found during Milestone 5 status-load verification |
| U11 | Validate release-only status fixture quote failures | Unconfirmed | Found during Milestone 5 focused verification |
| U12 | Validate federation registry save interruption atomicity | Unconfirmed | Found during Milestone 6 interruption verification |

## Executive summary

Milestone 1 has closed the highest-priority recovery and readiness defects in
the current federation implementation. Empty and stale members now remain
non-ready, recover quorum-authoritative journal, finalized-item, certificate,
materialized-state, and AlephBFT backup state, and join only a quorum-compatible
live session. Intact and full-federation restarts remain bounded. The existing
local performance baseline remains: a five-member federation reached healthy
state in 1.303 seconds, processed 100 parallel receives at 25.09 committed
swaps per second, restarted all five members at a 316-operation height in 2.958
seconds, and restarted one member with an intact database in 1.673 seconds.

Milestone 2 established that current server application is already independent
of HTTP request cancellation and that the centralized wallet connector already
returns at threshold without waiting for slow surplus members. The reproducible
defect was instead at wallet recovery: the CLI recovered a completed receive
but recovery did not record its incoming transaction, so command dispatch then
submitted the token again with a new saga identity and received `Token Already
Spent`. Recovery now records the same input-proof transaction identity as the
normal receive path, and receive retries return the persisted incoming result.
A related shared-SQL assumption that every transaction identifier was a
33-byte secp256k1 key was independently reproduced for 48-byte federation BLS
identifiers and replaced by self-describing serialization with legacy 33- and
48-byte read compatibility.

Milestone 3 proved that repeated melt completion failed because payment actor
selection hashed the operation ID across the full configured roster, including
offline members. In the minimal failure, the first melt selected active member
4 and completed; the second selected stopped member 5, so its accepted reserve
remained pending with no backend attempt or payment observation. Melt payment
ownership now comes from the authenticated author of the consensus-accepted
journal envelope. The accepted author is a deterministic, quorum-ordered value
already covered by operation authentication, while payment observations remain
the only quorum evidence of the outcome. The obsolete roster election and
wallet melt-only preflight were removed. The same two-batch process workload
now completed both melts with member 5 offline, converged on 14 operations and
four payment reports, and retained that exact state across a four-member
restart without duplicate payment.

Before the fix, a member whose database was erased did not recover. After
approximately 155 seconds it held only 1 of 316 accepted operations while
reporting `sync=healthy` and `ready=true`. Its logs showed an AlephBFT
backup/collection round mismatch followed by a new session that the other four
members rejected as the wrong session. This established both a catch-up failure
and an incorrect readiness claim.

A focused local confirmation run had reproduced both defects from a smaller
35-operation history. The erased member recovered only through operation 10,
remained at 11 of 35 operations for more than 15 minutes, entered session 1
while the other four members remained in session 0, and continued to report
`sync=healthy ready=true`. A second load run exposed the complementary case:
four members rotated to session 1 while one stalled member remained in session
0. After restart the stalled member recovered the exact 144-entry journal, but
the strict ready/converged/certificate status command still returned success
with the live-session split present. Readiness therefore fails to prove both
federation currency and live-session compatibility.

The same local run also found two load-sensitive failures: nine of ten repeated
standard-operation workers returned `Payment failed` during their second melt,
while the tenth stalled at the same step; and one heavily used member stopped
answering signed private status requests even though its public information
route remained fast. A restart cleared the status failure.

The focused confirmation run reproduced the melt-confirmation stall and private
status failure independently. It also sharpened the wallet result: 43 parallel
one-second-deadline receives all committed and persisted one sat in their
wallets, but none of the 43 CLI processes exited within the two-minute bound
after printing `Received: 1`. Four members applied all 144 journal entries while
the unresponsive fifth member remained at 100 entries, with one accepted and
unapplied. This did not reproduce the earlier exact client-reported-failure
shape, but it confirmed an adjacent post-success completion hang and another
minority application divergence.

A fresh Nocean deployment exposed a separate consensus liveness and
backpressure failure. The first receive case, configured for only one concurrent
wallet, caused all five public HTTPS endpoints to become temporarily
unresponsive. Only 10 accepted journal operations produced approximately 744
AlephBFT DAG units. `UnitBroadcast` queues grew to roughly 80-160 messages per
member while receive requests timed out, then the public endpoints recovered
without a restart after the queues drained. Mint CPU remained far below host
saturation. Code tracing confirmed that the fixed 100 ms active schedule
overrides AlephBFT's normal 500 ms steady delay and that the data provider
permits empty units while payload activity remains pending. This amplifies a
small workload into enough consensus traffic to starve operation and HTTP work.

Earlier Nocean testing found wallet fanout, retry, and ambiguous-result problems
under member catch-up pressure. Those findings remain relevant even though the
new local receive-only result is much faster. The local and remote throughput
numbers are not directly comparable because the network, host capacity,
software revisions, and workload shapes differ.

## Milestone execution plan

### Milestone 1: Fresh-member recovery, readiness, and live-session convergence

**Status:** Complete

**Covers:** Issues 1 and 2

**Objective:** Make recovery from an empty or stale database deterministic, and
make readiness prove that a member is current and compatible with the live
federation session rather than merely locally consistent.

#### Existing evidence

- An erased member recovered only 1 of 316 accepted operations and still
  reported `sync=healthy` and `ready=true` after approximately 155 seconds.
- A smaller independent run recovered only 11 of 35 operations, then
  remained stalled for more than 15 minutes.
- The erased member entered session 1 while the other four members remained
  in session 0 and rejected its traffic as the wrong session.
- The complementary split was observed under load: four members entered
  session 1 while one remained in session 0, yet strict ready, convergence, and
  certificate checks succeeded because heights and digests matched.
- An intact single-member restart and a full-federation restart recovered
  quickly, separating the empty/stale recovery problem from ordinary replay.

#### Root-cause evidence

A bounded current-revision process reproduction started five members, committed
one standard batch, and converged all members at seven operations in AlephBFT
session 0. Member 5 was stopped, its work directory was preserved outside the
active path, and it was restarted with an empty work directory. Within ten
seconds it reported zero operations, no finalized items, no catch-up certificate
evidence, `session=1`, `sync=healthy`, and `ready=true`. Those values remained
unchanged in every five-second sample for 90 seconds while the other four
members remained at seven operations in session 0. Both sides repeatedly
rejected AlephBFT traffic with `expected=0, found=1` or `expected=1, found=0`.
The same causal shape was previously reproduced at histories of 35 and 316
operations, so it is not a large-journal or single-timing-window effect.

The code path explains every observed transition:

1. Router construction creates an empty consensus runtime at session 0 and
   starts the live AlephBFT task before startup readiness revalidation or the
   peer-sync loop.
2. `restore_startup_readiness` treats an empty journal as trusted and changes
   the startup halt directly to `Healthy` without any peer comparison,
   certificate evidence, finalized-item history, or session evidence.
3. The fresh session lacks the persisted backup and finalized prefix needed to
   rejoin peers already advanced in session 0. It terminates and rotates the
   empty local runtime to session 1.
4. Each peer-status response proves the local journal is behind, but
   `should_defer_catch_up_to_live_aleph_bft` returns true solely because local
   sync is `Healthy` and the local live-session and inbound handles exist. This
   return occurs before the code marks the member lagging or sends a journal
   range request, so recovery is intentionally skipped on every round.
5. `sync_ready` is derived only from the local sync enum. Operator
   `--require-ready` checks only that boolean, convergence checks only height and
   state digests, and zero-operation certificate checks require no evidence.
   No guard compares the active AlephBFT session, so an equal-height session
   split can pass every existing strict guard.

The owning layers are the AlephBFT/router startup ordering, peer-sync readiness
transition and deferral policy, peer status evidence, and the operator status
guards. The evidence supports one coherent correction: an untrusted startup
checkpoint must remain non-ready and must not start live consensus until peer
sync establishes and recovers a quorum-compatible journal/session view; strict
status must reject session disagreement. The temporary reproduction wrapper is
git-ignored and must be removed after the permanent regression passes.

The first implementation pass removed premature startup and readiness and was
then re-run as a five-member process test. Member 5 stayed non-ready at zero
operations, recovered the exact seven-operation journal, finalized cursor,
order and materialized-state digests, and quorum catch-up certificate, and
briefly became healthy in session 0. It then moved alone to session 1 and
correctly returned to `awaiting_quorum`; strict convergence failed for the
remaining 75-second observation window. The member log supplies the missing
causal boundary: upstream AlephBFT inferred collection round 62 from the peers
but backup round 0 locally, reported `Backup state behind unit collection
state`, stopped its creator, and returned from `run_session`. CDK classified
that indistinguishably returned task as normal session completion and rotated
unilaterally. Therefore journal recovery and fail-closed readiness are working,
but an erased member cannot resume a progressed session until its signed DAG
backup is recovered or the quorum moves to a new session. Any accepted fix must
address that continuity boundary; suppressing the readiness error or merely
delaying unilateral rotation is insufficient.

The corrected path has now passed erased-member recovery at independent
seven- and 36-operation histories. A restart matrix then processed a standard
seven-operation mint/swap/melt/restore batch, recovered one intact member in 21
seconds, and recovered a full federation restart in 28 seconds. Two members
were restored concurrently from seven-operation snapshots while the other
three held 15 operations; both recovered the exact journal, finalized cursor,
backup, certificate quorum, materialized state, and session 0 in 46 seconds.
The first version of that test exposed the historical-certificate and stale
backup defects now tracked in U4 and U3. After their narrow fixes, the same
test passed without relaxing readiness or certificate thresholds.

An interruption test next restored member 5 from 15 operations while the
quorum held 51. Before catch-up mutation it was observed as
`awaiting_quorum`, `ready=false`, with no live session; the process was killed
at that boundary. Restart converged all five members to journal indices 0-50,
59 finalized items, session 0, and 2,923 persisted backup chunks totaling
459,984 bytes in 59 seconds. All five strict status records were healthy and
ready with the same order and materialized-state digests. This verifies
fail-closed interruption and durable restart at a larger independent suffix;
the paging-boundary and standalone regressions below complete Milestone 1.
The distinct release-mode large-history cost question remains unconfirmed and
is isolated in Milestone U6 rather than hidden by a timeout or page-size change.

#### Verification results

- The five-member process matrix passed erased recovery at seven and 36
  operations, intact-member restart, full-federation restart, concurrent
  two-stale-member recovery from 7 to 15 operations, and interrupted recovery
  from 15 to 51 operations. Every final member had the exact journal, finalized
  cursor, order and materialized-state digests, certificate quorum, backup, and
  session 0; strict health/readiness passed and new work committed.
- `cargo test -p cdk-axum --features federation startup_` passed 21 tests;
  the `historical` filter passed 2; and `rest_peer_client_` passed all 24,
  including atomic backup recovery, malformed-page rejection, two-stale-member
  certificate relay, full-tip digest rejection, and 99/100/101 paging.
- Focused `transcript`, `keyset`, `roster`, and `configuration` filters passed
  5, 9, 1, and 1 tests respectively. These retain fail-closed validation at the
  configuration and cryptographic boundaries adjacent to startup.
- `cargo test -p cdk-mintd --lib --features
  federation-test-utils,sqlite,prometheus federation_status::tests` passed all
  42 status tests, including inactive live transport and equal-height session
  disagreement. The focused SQLite AlephBFT backup persistence test passed its
  append, bounded-page, replacement rollback, commit, and session-isolation
  assertions.
- `cargo test -p cdk --features federation standalone` passed all 10 standalone
  startup, mint, BLS issue, persistence, and builder tests. `cargo check -p
  cdk-axum --features federation`, `cargo fmt --all -- --check`, and `git diff
  --check` passed.
- An additional `cdk-common` `catch_up` test-filter attempt did not run because
  unrelated existing test fixtures no longer initialize current Cashu quote
  fields (15 compile errors across consensus, journal, operation, signing, and
  transport test modules). Production `cdk-common` code compiled through the
  successful `cdk-axum` check; this unrelated fixture drift was not folded into
  Milestone 1.
- Temporary timing cases, fault injection, process wrappers, generated
  snapshots, and the stale-backup probe were removed. The permanent bounded
  regressions and normal low-overhead status fields remain.

#### Work checklist

- [x] Reproduce the failure and confirm the owning causal path with hard
  evidence; record it above before changing behavior.
- [x] Complete the smallest fix in the existing startup, peer-sync, status, or
  session-supervisor path so an empty/stale member recovers quorum-authoritative
  state before joining the live session and readiness requires current,
  compatible signed quorum evidence. Do not add a parallel recovery system.
- [x] Add the narrow regression and verify the acceptance criteria, including
  fail-closed roster, transcript, keyset, configuration, journal, and session
  disagreement behavior.
- [x] Remove temporary instrumentation, record sanitized commands/results and
  any tests not run, update this milestone's status, and commit it with a
  conventional commit.

#### Acceptance

**Success criteria:** An empty or stale member converges exactly to the quorum
journal and materialized state, joins the quorum's live session, and never
reports ready while lagging or incompatible. Intact restart stays fast,
standalone startup is unchanged, and recovery creates no conflicting operation,
digest, certificate, or session.

**Minimum verification:** Focused startup/readiness unit tests; a five-member
erased-member process regression at a representative non-empty height; an
equal-state but wrong-session strict-status regression; interrupted and intact
restart coverage; and relevant standalone startup/mint tests. Add larger-height
or multi-member-behind cases only if the proven cause depends on those shapes.

### Milestone 2: Cancellation-safe server application and bounded wallet completion

**Status:** Complete

**Covers:** Issues 3, 4, and 5

**Depends on:** Milestone 1 for final catch-up fault classification

**Objective:** Once an operation is accepted, make its server-side completion
independent of request cancellation, and make wallet quorum operations return a
bounded, semantically correct definitive or ambiguous result.

#### Existing evidence

- Earlier Nocean testing recorded 25 client failures whose operations still
  committed and whose valid shares existed on all members.
- In the focused local run, 43 of 43 wallets printed `Received: 1` and stored
  one sat, but none of their CLI processes exited within two minutes.
- Four members applied all 144 operations while the fifth stopped at 100,
  with operation 100 accepted but not applied; restart replay completed it.
- Direct receive overlap against an intact member completed 20 of 20 cases,
  showing that failure depends on load, peer state, or post-success cleanup.
- The exact earlier failure-after-commit client result was not reproduced in
  the focused local run; the adjacent post-success completion hang was.

#### Root-cause evidence

A current-revision five-member process reproduction separated the server and
wallet lifecycles at the durable boundary. The receive client was killed only
after a new row appeared in `federation_aleph_bft_pending_operations`. All five
members then applied the operation, converged with no pending rows, and advanced
the journal by exactly one. Code tracing agrees with the process result:
`submit_envelope` persists pending work and an AlephBFT-owned task applies it;
the HTTP handler owns only a bounded result waiter. Dropping the request future
does not own or cancel accepted application. The earlier server-cancellation
hypothesis is therefore invalidated on the current revision, and no server
lifecycle change was justified.

The first wallet retry recovered one receive saga and restored one sat, but
then dispatched `receive` again and submitted a second federation operation,
which terminated as `Token Already Spent`. The causal path was:

1. CLI startup calls `recover_incomplete_sagas` before command dispatch.
2. `resume_receive_saga` restored the output proofs and deleted the saga but,
   unlike normal receive and the existing recovered-issue pattern, recorded no
   incoming transaction for the original input proof identifiers.
3. Command dispatch could not recognize the recovered result. `ReceiveSaga::new`
   created a new operation ID and outputs and submitted the same spent inputs
   again.

The smallest fitting correction records a recovered incoming transaction using
the original input proof identifiers, recovered amount, fee, and saga ID.
`receive_proofs` checks that canonical transaction identity and returns the
persisted incoming amount before creating another saga. It adds no retry loop,
queue, schema, fanout path, or alternate application system.

That correction exposed a separate shared-SQL defect before the retry could
return: both a normal federation receive and a recovered receive persisted a
48-byte BLS G1 identifier, while `sql_row_to_transaction` split every `ys` blob
into 33-byte chunks. A focused SQLite conformance test failed with an invalid
secp256k1 public key before the storage fix. New writes now serialize the key
vector as JSON in the existing blob column; reads first decode that
self-describing representation and then accept legacy homogeneous 33-byte
secp256k1 or 48-byte BLS G1 rows. This is isolated as Milestone U7 below, with
the independent evidence that allowed it to be fixed during M2.

#### Verification results

- With one member offline, a receive returned `Received: 1` and exited in two
  seconds. Ten parallel receive processes under the same four-member quorum
  all exited before a 60-second watchdog, all ten wallets held exactly one sat,
  the active members shared the same 109-operation journal and state digests,
  and the journal delta was exactly ten.
- In the killed-client case, a durable pending row was observed before
  termination. The operation converged with journal delta one and zero pending
  rows. Wallet restart reported one recovered operation; retry returned
  `Received: 1` in one to two seconds, retained a one-sat balance, and did not
  advance the 67- or 93-operation journal in two independent runs.
- Connector tests passed for return-after-threshold success, matching
  threshold rejection without slow-member delay, disagreeing rejection
  ambiguity, and timeout/invalid-share ambiguity.
- The receive idempotence test, recovered-transaction identity test, and all
  four existing `test_recover_receive` tests passed. The shared encoding test
  passed new mixed-key serialization plus legacy 33- and 48-byte decoding; the
  SQLite BLS transaction round trip passed after failing before the fix, the
  complete SQLite `transaction` filter passed 18 tests, and the shared Redb BLS
  transaction conformance test passed.
- `startup_replays_accepted_but_unapplied_journal_entry_to_same_state_digest`
  and
  `aleph_bft_live_session_replays_pending_operation_when_restart_precedes_backup`
  passed, covering both accepted-journal and pending-operation restart
  boundaries. The ten-test standalone filter passed, and `cargo check -p cdk
  --features federation -p cdk-sql-common -p cdk-sqlite` passed.
- No temporary instrumentation was added. The git-ignored process wrapper and
  generated wallets were removed after recording sanitized aggregate results.
  A restarted offline member exposed the separate unconfirmed U8 catch-up
  mismatch; M2's final regression intentionally held that member offline and
  asserted the available quorum rather than hiding or fixing U8 in this
  milestone.

The focused commands were `cargo test -p cdk --features federation
test_recover_receive`, the two new receive test-name filters, the four connector
test-name filters recorded above, `cargo test -p cdk-sql-common
transaction_ys_decode_new_and_legacy_encodings`, `cargo test -p cdk-sqlite
transaction`, the two `cdk-axum` replay test-name filters, `cargo test -p cdk
--features federation standalone`, `cargo check -p cdk --features federation
-p cdk-sql-common -p cdk-sqlite`, and `cargo fmt --all -- --check`. The CLI used
by the process regression was built with `cargo build -p cdk-cli
--no-default-features --features federation`; `cargo test -p cdk-redb
wallet_add_and_get_bls_transaction` also passed. A full workspace test, clippy,
and live PostgreSQL conformance run were not repeated because this milestone
changes only wallet receive and shared wallet-transaction persistence; the
focused compile, database, recovery, connector, replay, and standalone coverage
exercised every changed boundary without requiring an external database.

#### Work checklist

- [x] Reproduce the smallest current client-exit and/or accepted-but-unapplied
  failure, instrument only the boundary needed, and record the confirmed cause
  and owning wallet/server layer. Close any unreproduced shape without fixing it.
- [x] Implement the smallest existing connector, saga, journal/application, or
  cancellation-lifecycle fix. Preserve stable operation IDs, idempotent replay,
  typed ambiguous outcomes, and request-independent accepted work; do not add
  per-flow fanout or application systems.
- [x] Add the narrow regression and verify bounded threshold completion,
  cancellation/restart recovery, minority-member faults, and no duplicate side
  effects or false definitive failures.
- [x] Remove temporary instrumentation, record sanitized commands/results and
  any tests not run, update the status/root cause/fix, and commit with a
  conventional commit.

#### Acceptance

**Success criteria:** A threshold-valid wallet operation returns and exits
within a documented bound despite one or two slow members. Client cancellation
cannot strand accepted work; live application or replay reaches one durable
terminal result; retrying the same identity returns that result without a
duplicate issuance, spend, or payment. Standalone behavior remains unchanged.

**Minimum verification:** Focused connector and application tests for the
confirmed boundary; one parallel receive process regression asserting wallet
balances, process exit, and member application state; cancellation plus
restart/replay at the causal boundary; typed rejection/ambiguity coverage; and
relevant standalone wallet/mint tests. Test additional lifecycle boundaries
only when the root cause can affect them.

### Milestone 3: Exactly-once melt completion under repetition and member faults

**Status:** Complete

**Covers:** Issue 6

**Depends on:** Milestone 2 wallet completion semantics

**Objective:** Make repeated melt operations reach one threshold-consistent,
exactly-once terminal result without hangs when members are slow or offline.

#### Existing evidence

- Nine of ten concurrent standard-operation workers returned
  `Payment failed` during their second melt while the tenth stalled there.
- A focused sequential run with one member offline completed the first melt
  and stalled during the second melt-confirmation path for more than 120
  seconds.
- The repeated second-operation shape was reproduced independently of the
  larger mixed workload.

#### Root cause

A fresh five-member, 3-of-5 run stopped member 5 and executed two sequential
standard batches. The first batch completed. The second reached
`standard.melt-confirm.start` and exceeded the 90-second scenario bound while
members 1-4 remained healthy, ready, and converged with empty pending queues.
Their journals contained the accepted second melt reserve and their databases
held its quote and saga in `Pending`, but there was no corresponding payment
observation.

The first melt operation ID selected member 4 under
`elected_melt_payment_member` and completed. The second selected stopped member
5. That function reduced the first eight operation-ID bytes modulo all five
configured roster members without considering whether the selected member was
live. `probe_federation_melt_quote_payment` allowed only that member to call the
backend, so none of the four active members could advance the accepted second
melt. The wallet repeated the same static selection to wait for or validate the
chosen connector. This establishes the mint payment-actor rule as the cause;
the backend was never called, so fakewallet state handling was not causal.

The accepted second operation had the same authenticated author, member 2, in
the durable journal on every active member. Operation authorization is verified
before consensus admission, the accepted envelope is quorum ordered, and the
author is therefore a deterministic value already committed before external
effects. It is also a member that was live when it submitted the accepted
operation, unlike a hash over the static roster.

#### Fix

The payment probe now loads the authoritative journal entry for the saga's
operation ID, verifies its durable authorization, and permits only that accepted
author to initiate the external payment. The existing per-quote active-payment
claim, persisted saga, backend lookup identity, and payment-observation quorum
continue to provide local at-most-once execution, restart recovery, and
threshold-authoritative terminal state. Deterministic application still never
calls a payment backend.

The obsolete roster election API and its test-only brute-force request
selection were removed. The centralized wallet connector now uses the normal
validated threshold fanout and returns any aggregatable matching threshold
view, including `Pending`, without waiting for a statically selected surplus
member. Production's request-independent payment observer remains responsible
for advancing accepted pending work. The local connector fixture was updated
to model that existing lifecycle explicitly rather than tying payment progress
to a cancellable mock request.

This is smaller than the previous implementation: it removes the special
melt-connector validation path and static election code, adds no clock, lease,
failure detector, retry queue, schema, wire field, or alternate orchestration
system, and uses the existing authenticated journal envelope.

#### Verification and measurements

- Before the fix, the minimal two-batch run timed out after 90 seconds during
  the second melt. Its second operation was accepted and applied on members
  1-4, but the selected payment member was offline and no payment observation
  existed.
- After the fix, the same fresh run with member 5 stopped exited successfully.
  The first and second melt-confirmation phases completed in 5.151 seconds and
  1.024 seconds. Both batches reported `melted=1`; members 1-4 converged on 14
  journal operations, identical order and materialized-state digests, four
  payment reports, zero payment conflicts or unresolved reports, and empty
  pending queues.
- Restarting all four active members against the same persisted state retained
  the exact 14-operation journal, digests, four payment reports, and zero
  pending work. No replay payment was added.
- `cargo test -p cdk --features federation
  mint::federation::tests::melt:: -- --nocapture` passed all 33 focused melt
  tests, including accepted-author exclusivity, concurrent duplicate probes,
  backend `Unknown`, failed rollback, paid finalization replay, catch-up
  observations, and standalone quote persistence.
- `cargo test -p cdk --features federation
  wallet::federation::tests::federated_mint_connector_ -- --nocapture` passed
  all 77 connector tests, including the new threshold-pending/slow-surplus
  regression and the request-independent local observer fixture.
- `cargo test -p cdk --features federation pending_melt_ -- --nocapture`
  passed all 14 standalone and wallet pending-melt lifecycle tests.
- The five-member one-offline AlephBFT value-window test passed in 123.46
  seconds and asserted one outgoing payment total before and after catch-up.
  The below-finality/restart test passed in 20.60 seconds and asserted no
  pre-consensus payment or signatures plus one payment after recovery.
- Federation-enabled `cdk-axum` tests compiled, and standalone
  `cargo check -p cdk` plus `cargo check -p cdk-axum` passed. A focused clippy
  invocation was attempted but stopped in unchanged `cashu` code on existing
  Rust 1.95 `useless_conversion`, `missing_panics_doc`, and
  `large_enum_variant` findings before reaching the changed crates.
- A real Lightning process check was not run: this workspace has neither the
  documented Nix regtest shell nor Bitcoin/CLN/LND binaries. The causal path is
  backend-independent and occurs before backend dispatch, but real-backend
  validation remains part of Milestone 9 release work.
- No protocol instrumentation was added. The ignored process harness and its
  outputs were used only for the bounded reproduction and were removed before
  commit.

#### Work checklist

- [x] Reproduce the smallest two-melt failure, add only enough instrumentation
  to locate it, and record the proven cause and owning layer. Separate
  fake-wallet behavior from a general payment defect before fixing anything.
- [x] Implement the smallest change using the existing payment observation,
  payer election, saga, journal, and quote-state patterns; preserve quorum
  authorization, deterministic replay, and at-most-once external payment.
- [x] Add the narrow regression and verify repeated completion, member/payer
  faults, backend ambiguity, cancellation, and restart without duplicate
  payment or loss of the terminal result.
- [x] Remove temporary instrumentation, record sanitized commands/results and
  tests not run, update the milestone, and commit with a conventional commit.

#### Acceptance

**Success criteria:** Consecutive melts complete within a documented bound while
a quorum remains. Each has one payer decision, at most one external payment,
one threshold-consistent terminal quote state, exact replay, and a correct typed
result for pending, failed, timeout, or already-paid observations. Standalone
melt behavior is unchanged.

**Minimum verification:** The minimal two-sequential-melt regression; the fault
or lifecycle case that triggers the confirmed cause; restart/replay; focused
federation and standalone melt tests; and one real Lightning backend check when
the cause is not demonstrably fake-wallet-only. Run a larger concurrent mixed
workload only as the final process confirmation, not as a debugging loop.

### Milestone 4: AlephBFT cadence, network backpressure, and HTTP liveness

**Status:** Complete

**Covers:** Issue 8

**Objective:** Prevent a small application workload from creating excessive
empty consensus traffic, queue growth, and public/private HTTP starvation while
retaining prompt consensus for real work.

#### Existing evidence

- A fresh Nocean deployment became temporarily unresponsive during the
  first one-wallet receive workload and recovered after queues drained without
  a restart.
- Ten accepted journal operations produced approximately 744 DAG units.
- `UnitBroadcast` queues grew to roughly 80-160 messages per member while
  requests timed out.
- Mint CPU remained far below host saturation, arguing against simple host
  CPU exhaustion.
- Code tracing confirmed the fixed 100 ms active schedule replaces the
  upstream 500 ms steady delay and that empty units remain possible while
  payload activity is pending.
- A separate possible network-pump retry amplifier remains unconfirmed and
  is tracked independently as Milestone U2.

#### Current-revision root-cause evidence

A fresh five-member local process run submitted five sequential mint-quote
requests, producing exactly ten accepted and finalized journal operations: one
quote and one payment observation per request. Before the fix, the member-1
AlephBFT report moved from zero to 314 reconstructed DAG units in one reporting
interval. Its `UnitBroadcast` count reached 314, then fell through 207 to the
five-member idle floor of 5. All five journals converged at ten applied entries
with zero pending operations. All 750 concurrent public `/v1/info` probes still
returned HTTP 200 locally, with 46 ms maximum latency, so this machine
reproduced the cadence amplification and queue pressure but not the stronger
remote HTTP starvation.

The production schedule supplied a fixed 100 ms delay for every active round,
while AlephBFT's existing schedule uses 5 seconds for round zero and 500 ms for
the later rounds reached by this workload. While any real operation remained
active, members with no payload returned empty data to AlephBFT so they could
help finalize peer work. The fixed override therefore let the five members
create roughly 50 units per second, five times the supported steady cadence,
and outrun dissemination. Empty units do not renew activity, and the local
retry queue was not shown to be necessary for the failure; U2 remains separate.

#### Chosen fix

Active sessions now use AlephBFT's existing round-aware unit-creation schedule.
The existing activity hint still wakes an idle session, inactive tail rounds
still use the two-second slowdown, and a fully idle data provider still waits
without creating periodic empty units. This removes the fixed 100 ms constant
and adds no queue, timeout, retry, protocol message, or alternate scheduling
system.

#### Local verification results

- With the old schedule, ten operations produced 314 reconstructed units and a
  peak `UnitBroadcast` count of 314. With the upstream active schedule, the
  identical all-member case produced 258 units and peaked at 179, reductions of
  approximately 18% and 43%. The final two reports had the same unit count,
  proving unit creation returned to the event-driven idle state.
- The all-member post-fix case completed 750 of 750 public probes with 105 ms
  maximum and 10.8 ms mean latency. Forty of forty signed-status probes passed;
  their maximum was 716 ms and mean was 698 ms including command startup and
  local federation-config validation.
- The explicit latency tradeoff is material: the five sequential quote
  requests completed in 25.272 seconds after the fix. Before the fix, their
  persisted quote timestamps spanned five seconds. The safer schedule is
  intentionally favored over a custom intermediate magic delay. Further
  capacity and latency optimization, after liveness is protected, belongs in
  Milestone 8.
- With member 5 offline, the active quorum applied all ten operations in 28.791
  seconds. The DAG stopped at 235 units, `UnitBroadcast` peaked at 135 and
  drained to the four-member idle floor of 4, 600 of 600 public probes returned
  HTTP 200 with 21 ms maximum latency, and all 40 signed-status probes passed
  with 701 ms maximum. Restarted member 5 caught up to the same ten-entry
  journal; all members had ten finalized items and zero pending operations.
- Focused tests passed for the upstream active delay, idle data-provider wake
  and drain behavior, the slow-peer quorum fast path, and five-member finality
  under reordering, duplicate delivery, and delay. All 28 standalone
  `cdk-axum` tests and the standalone build check passed. Formatting and diff
  checks passed.
- Focused Clippy with `-D warnings` was attempted, but Rust 1.95 reports 22
  pre-existing `cdk-axum` findings, including large enums, derivable impls,
  result size, argument count, and newer style lints. None points to the changed
  delay closure or regression assertion.
- No temporary production instrumentation or experimental switch was added.
  The git-ignored process harness and generated data were removed after the
  aggregate results above were recorded.
- The `typos` executable was unavailable in the current shell; formatting and
  whitespace validation did run successfully.

#### Nocean final validation

The state-preserving deployment installed the current binaries on all five
members, retained rollback binaries, and passed process, public-route, config,
signed-status, readiness, and journal-convergence checks at operation 152. The
benchmark preparation advanced the journal to 158 before the timed workload.
The exact ten-operation receive case then completed 10 of 10 operations with no
client failures and advanced every member to operation 168:

- elapsed time was 91.649 seconds and committed throughput was 0.109
  operations per second;
- successful receive latency was 9.221 seconds at p50, 9.340 seconds at p95 and
  p99, and 9.409 seconds maximum;
- the comparable successful pre-fix Nocean run was 6.562 seconds at p50 and
  6.704 seconds maximum, so the safer cadence increased both figures by about
  40%; this is the explicit stability-for-latency tradeoff accepted here;
- member 1's reconstructed DAG grew from 7,327 to 7,744 units during the timed
  receive window, an increase of 417, while its reported `UnitBroadcast` count
  peaked at 90 and returned to the five-member idle floor of 5; three late
  units then propagated and all members agreed at 7,747 without sustained
  growth;
- no public endpoint outage or timeout occurred, the post-run signed comparison
  reported all five members healthy, ready, and converged at operation 168;
  and a separate post-run sample returned HTTP 200 for 25 of 25 fresh-connection
  `/v1/info` probes, with 1.023 seconds mean and 1.093 seconds maximum latency.

The current schedule therefore does not eliminate consensus traffic or make
remote latency cheap, but it removed the unsupported fivefold active cadence,
kept HTTP reachable, bounded the observed broadcast burst, and returned to the
idle queue floor. Capacity optimization now belongs to Milestone 8. The
separate network-pump retry hypothesis remains unconfirmed in U2 because this
trace still did not expose evidence that retry duplication was necessary for
the former outage.

#### Work checklist

- [x] Reproduce the smallest starvation case on one Nocean timeline, measure
  only the suspected cadence/queue/scheduling boundaries, and record the
  complete causal chain. Keep the separate U2 retry hypothesis out unless
  independently proven.
- [x] Implement the smallest supported AlephBFT configuration, activity-hint,
  or bounded-backpressure fix for that cause. Do not use larger timeouts, fork
  upstream logic unnecessarily, or drop correctness-critical traffic.
- [x] Add the narrow regression and verify bounded idle/burst traffic, HTTP
  liveness, return to idle cadence, slow-peer behavior, and the measured
  payload-latency/throughput tradeoff.
- [x] Remove experimental switches/high-volume tracing, record sanitized
  before/after results and tests not run, complete the authorized Nocean final
  validation, update the milestone, and commit with a conventional commit.

#### Acceptance

**Success criteria:** Empty-unit production, network queues, and public/private
HTTP latency stay within documented bounds; activity returns to steady idle
cadence; a slow peer cannot cause unbounded growth; and useful payload latency
does not regress beyond an explicit measured tradeoff. Standalone behavior is
unchanged.

**Minimum verification:** Focused tests for the confirmed cadence or
backpressure mechanism; a five-member ten-operation process regression with
concurrent HTTP sampling; idle/burst plus the one adverse network shape that
exercises the cause; comparison with the previous schedule; and relevant
standalone HTTP tests. Use the Nocean workload once for final validation.

### Milestone 5: Signed private-status liveness and bounded operator reporting

**Status:** Complete

**Covers:** Issues 7 and 12

**Depends on:** Milestone 1 readiness semantics; coordinate measurements with
Milestone 4

**Objective:** Keep authenticated diagnostic status responsive and accurate
under load, catch-up, restart, and session divergence without weakening access
control or masking application liveness failures.

#### Existing evidence

- Signed private-status requests hung on heavily used members while the same
  members' public `/v1/info` routes returned HTTP 200 quickly.
- Peer logs recorded private-status timeouts and exponential backoff.
- Restart cleared the status failure.
- One affected member was also behind at 100 of 144 entries and in a
  different live session, so correlation with application/consensus lag remains
  possible but unproven.

#### Current-revision investigation result

The historical hang did not reproduce on the current revision after Milestone
4 restored AlephBFT's upstream active cadence. Code inspection also invalidated
the suspected admission-queue mechanism: private REST admission uses
`try_acquire_owned`, so saturation returns HTTP 429 immediately rather than
waiting, and its owned permit is released by normal Rust cancellation.

The already-loaded Nocean federation first completed 100 concurrent signed
status requests with no failures, 255.1 ms mean latency, and 366 ms maximum
latency while all five members were healthy and converged at operation 168. A
higher-load remote rerun was not authorized, so no additional remote
transactions were created for this milestone.

A fresh local five-member, threshold-three production-path DKG federation then
provided the causal process check:

- At operation 9, 50 of 50 concurrent signed-status requests passed with 271.7
  ms mean and 286 ms maximum latency, including CLI startup, config validation,
  request signing, and response validation.
- From operation 49 to 89, 20 parallelism-five quote/payment-observation pairs
  completed successfully. All 85 signed-status samples passed with 284.1 ms
  mean and 304 ms maximum latency; all 85 public samples returned HTTP 200 with
  8.8 ms mean and 18.3 ms maximum latency.
- A later parallelism-five batch advanced the federation from operation 109 to
  the historical failure height of 149. All 20 quote pairs, 80 signed-status
  samples, and 80 public samples passed; maximum latencies were 317 ms and 12
  ms respectively. All members remained healthy, ready, in the compatible live
  session, and exactly converged.
- With member 5 offline, the active quorum advanced from operation 149 to 159.
  All five quote pairs and all 20 status samples against the active members
  passed, with 262 ms maximum status latency. Restarted member 5 caught up
  before the first poll, returned healthy/ready signed status in 156 ms, and all
  five members converged at operation 159.

Status construction still performs database and cache reads, but the default
request already returns summary reports and leaves materialized row
fingerprints opt-in. No measured handler, lock, report, response-size, or
event-loop boundary approached the five-second cancellation bound. Status lag
was therefore neither a cause nor an effect in these current runs, and there is
no hard evidence for a production locking, caching, admission, or report-detail
fix.

#### Chosen resolution and verification

No production behavior changed. A narrow regression holds the certificate
status cache after the authenticated local-status handler has recorded its
sequence, cancels the handler three times, and verifies after every cancellation
that the sole inbound admission permit is restored. A fourth signed request
then completes. The release-profile regression passed in 0.03 seconds.

The focused release-profile status filters also passed the admission overflow,
non-persistent liveness sequence, consensus/sync reporting, lagging, catching
up, certificate verification, redacted halted state, payment-conflict,
completed-operation, alert, and materialized-row tests. Two unrelated fixtures
failed deterministically with `Unknown quote` before exercising their status
assertions; U11 records that release-only observation for independent debug and
fixture validation. Temporary process state, identities, wallets, logs, and
measurement files were removed, and all local listeners were stopped.

#### Work checklist

- [x] Reproduce the smallest signed-status hang, instrument only the suspected
  permit/lock/task/report boundary, and record the blocking mechanism, owning
  layer, and whether status lag is cause, effect, or merely correlated. The
  current revision invalidated the hang and admission-wait hypothesis, so no
  production fix is permitted.
- [x] Implement the smallest existing admission, locking, snapshot, caching, or
  bounded-detail fix only if confirmed. No fix was implemented because all
  measured boundaries stayed bounded and default detail is already summarized.
- [x] Add the narrow regression and verify bounded status under the causal load,
  cancellation cleanup, accurate readiness/session reporting, and continued
  public-route plus consensus/application liveness.
- [x] Remove temporary tracing, record sanitized latency/results and tests not
  run, update the milestone/operator semantics, and commit conventionally.

#### Acceptance

**Success criteria:** Signed status returns within a documented bound under the
confirmed stress condition, timed-out clients leak no permit/lock/task, status
agrees with strict readiness and live-session state, public and consensus work
remain responsive at supported polling rates, and authentication/disclosure
behavior is unchanged.

**Minimum verification:** Focused authenticated-route and causal-boundary tests;
repeated cancellation/admission cleanup; continuous status polling during the
single process workload that triggers the cause; readiness/session transition
coverage; and unchanged public/standalone routes. Add large-report or cache
tests only if report construction or caching is the proven cause.

### Milestone 6: Federation discovery and import completion

**Status:** Complete

**Covers:** Issue 9

**Depends on:** Milestones 1 and 2 for current readiness and connector semantics

**Objective:** Make URL discovery and federation import complete within a bound
and behave consistently with importing the same validated configuration from a
file.

#### Existing evidence

- Earlier testing fetched the discovery configuration successfully with
  HTTP 200.
- Adding the federation by discovery URL hung afterward.
- Importing the equivalent generated configuration file succeeded.
- The failure has not yet been revalidated against the current code and
  local process harness.

#### Current-revision investigation result

The historical hang did not reproduce. The current URL path performs two
sequential fetches from only the selected discovery member: the public
federation config and public mint info. It then validates the source member,
advertised federation ID/config digest, active v3 keyset, roster, and protocol
metadata before entering the same registry `upsert` and save path as
file and invite imports. It does not fan out to member metadata or keyset
endpoints after discovery, so the historical post-fetch fanout hypothesis no
longer describes this code.

A fresh local five-member, threshold-three production-path DKG federation
produced the following bounded results:

- Ten fresh URL imports completed with no failures, 296.2 ms mean latency, and
  300 ms maximum latency. Ten imports of the same immutable public config from
  a file also completed with no failures, 170.9 ms mean latency, and 175 ms
  maximum latency.
- Every URL/file pair produced byte-equivalent normalized registries. Repeating
  both imports in place kept exactly one wallet entry, proving idempotent
  upsert rather than duplicate persistence.
- With member 5 offline, five additional imports through healthy member 1 all
  completed, with 328.4 ms mean and 418 ms maximum latency. Both a URL-imported
  and file-imported wallet then used the stored connector successfully while
  that minority member remained unavailable, in 74 ms and 68 ms respectively.
- Interrupting URL import after 50 ms left no federation registry. Two retries
  completed and retained exactly one entry.
- Addressing the same discovery server through an unlisted source identity was
  rejected with the expected source-URL mismatch and created no registry.
- A standalone fakewallet mint returned HTTP 404 for the federation config;
  URL import failed in 35 ms and created no federation registry.

No current phase hung or approached its ten-second bound, and the tested
pre-save interruption plus retry left no partial or duplicate registry. The
historical Issue 9 failure is therefore closed on this revision without a
production change. Direct-write crash atomicity is not proven and is tracked
separately in U12 without a speculative persistence change.

#### Verification and cleanup

All 18 focused release-profile CLI federation tests passed, covering matching
and mismatched discovery identity/digest, active v3 keysets, roster and protocol
metadata, registry schema and privacy, invite/file loading, coordinator
selection, and connector-policy reconstruction. The healthy, offline-member,
conflicting-source, interruption/retry, connector-use, and standalone process
checks above cover the remaining discovery boundaries. All generated member
identities, databases, wallet seeds, registries, configs, logs, and measurement
files were removed, and every temporary listener was stopped.

#### Work checklist

- [x] Revalidate URL and file import against the same current local federation.
  If URL import no longer fails in repeated bounded runs, record the evidence
  and close without code; otherwise isolate and record the causal phase.
- [x] Implement the smallest discovery/import or centralized-connector fix only
  if confirmed. No production fix was permitted because the current path did
  not reproduce the historical hang.
- [x] Use the existing narrow regressions and verify healthy URL/file
  equivalence, the minority-member condition that exercises the cause,
  conflicting-identity failure, interruption/retry, and standalone import
  behavior.
- [x] Remove temporary instrumentation, record sanitized results and tests not
  run, update the status with invalidation or fix evidence, and commit.

#### Acceptance

**Success criteria:** URL discovery/import exits within a documented bound and
produces the same usable normalized federation identity as file import. A
minority slow/unreachable member cannot block valid threshold import,
conflicting identity fails closed, and retries cannot leave partial or duplicate
wallet registry state.

**Minimum verification:** One healthy URL-versus-file process test; focused
tests for the confirmed phase and its relevant minority/conflict condition;
interruption plus idempotent retry; and standalone single-mint import tests.

### Milestone 7: Wallet token-preparation and fragmentation scaling

**Status:** Complete

**Covers:** Issue 10

**Objective:** Determine whether large wallet proof sets cause a real local
token-preparation or storage bottleneck, distinct from federation consensus and
network latency, and fix only the measured owning path.

#### Existing evidence

- Token preparation slowed in a 240-payment workflow while only 84
  operations entered federation consensus.
- Proof fragmentation, CLI startup, SQLite access, and coin selection are
  plausible contributors, but none is yet established as the cause.
- A controlled proof-count scaling curve has not yet confirmed a defect.

#### Root cause

A temporary release-profile diagnostic held the useful set at 64 unspent
one-unit proofs for one mint while adding unspent proofs for another mint. Each
point contained 15 filtered reads and selections. Median filtered-read latency
rose from 0.528 ms with no unrelated proofs to 5.815 ms at 1,000, 27.493 ms at
5,000, and 108.921 ms at 20,000; p95 at 20,000 was 109.809 ms. Selection of the
unchanged 32 proofs remained 0.038-0.054 ms. Code inspection then confirmed
that the shared SQL wallet query selected every proof row, decoded every proof,
and only afterwards applied mint, unit, and state predicates in Rust. The
database already had suitable SQLite indexes. This proves full wallet-table
retrieval and decoding, rather than consensus or network work, as one owning
storage defect.

A second 15-sample curve used only matching one-unit proofs and selected half
the wallet. At 64, 1,024, and 5,000 proofs, median selection took 0.036 ms,
7.286 ms, and 181.707 ms; the respective p95 values were 0.040 ms, 7.449 ms,
and 190.885 ms. The selector scanned the sorted proof vector from its beginning
for every denomination attempt, repeatedly crossing proofs it had already
selected. In the same curve, median token serialization was only 0.011 ms,
0.161 ms, and 0.819 ms. This separately proves a coin-selection rescan defect
under severe same-denomination fragmentation and rules out serialization as
its cause.

The curve contained no online calls. Existing send-boundary tests confirmed
that offline preparation uses cached keysets without contacting the mint and
that online preparation loads missing keysets through the connector. A warm
release `cdk-cli --help` process-start sample had a 4.08 ms median across 14
runs; the first cold-cache run was 702.512 ms. Startup is therefore a separate,
proof-count-independent fixed cost in this measurement. Network and federation
latency remain intentionally outside this local curve and belong to Milestone
8.

#### Fix

The shared SQL wallet query now pushes its supported mint URL, currency unit,
and proof-state predicates into SQL before rows are transferred and decoded.
Only structured spending-condition matching remains in Rust, preserving its
existing semantics. An explicit empty state list still returns no proofs. No
schema, index, backend-specific branch, or federation logic was added.

Proof selection now builds one ordered queue per available denomination after
the existing proof sort and inactive-proof selection. Exact requests consume
the same first proof for a denomination, while over-value requests consume the
same last proof from the smallest higher denomination. This preserves the
selection policy and privacy behavior while replacing repeated full-vector
rescans with ordered denomination lookup.

#### Verification and measurements

- The identical unrelated-row curve after predicate pushdown measured median
  filtered reads of 0.529, 0.389, 0.436, and 0.710 ms at 0, 1,000, 5,000, and
  20,000 unrelated proofs; p95 at 20,000 was 1.059 ms. The 20,000-row case
  improved from 108.921 ms to 0.710 ms, while still returning exactly the same
  64 proofs.
- The identical 64/1,024/5,000 matching-proof curve measured median selection
  of 0.014/0.237/1.406 ms and p95 of 0.019/0.358/1.881 ms. The 5,000-proof case
  improved from 181.707 ms to 1.406 ms. Matching-row SQL decoding remained
  proportional to rows actually returned, and serialization remained below
  1 ms median at all points.
- The focused shared-query regression passed and asserts that mint, unit, and
  multi-state predicates are bound into the generated SQL. All 136
  `cdk-sqlite` library tests passed, including wallet mint/unit/state filtering,
  proof state transitions, reservations, sagas, and BLS persistence.
- All 42 proof-selection tests passed, including exact, closest-over, inactive
  keyset, multiple-keyset, fragmented, fee-convergence, and many-one-unit proof
  shapes.
- Three offline preparation tests passed for empty versus primed keyset caches
  and locked proofs; the online empty-cache preparation test passed; and all
  seven interrupted-send recovery tests passed for reserved, token-created,
  rolling-back, spent, and unspent outcomes.
- `cargo check -p cdk-postgres` passed. The query change is shared by SQLite and
  PostgreSQL, but no PostgreSQL service was available for a live performance
  curve. No PostgreSQL index change was made without backend-specific evidence.
- `cargo build -p cdk-cli --release` passed. The process-start measurements
  above used the resulting standalone CLI binary.
- Targeted Clippy was attempted. The normal invocation stopped in unchanged
  `cashu` code on 28 existing Rust 1.95 `useless_conversion`,
  `missing_panics_doc`, and `large_enum_variant` findings; `--no-deps` then
  stopped in unchanged shared database code on the existing `single_match`
  finding before providing a clean changed-crate result.
- The temporary ignored diagnostic test and all temporary SQLite files were
  removed. No production instrumentation was retained. A live federated
  capacity run was not repeated here because it mixes the now-isolated local
  path with network and consensus latency; Milestone 8 owns that controlled
  process matrix.

#### Work checklist

- [x] Build one controlled proof-count curve that separates startup, storage,
  selection, serialization, and online calls. If no material product defect is
  confirmed, record the curve and close without code; otherwise record the
  causal algorithm/query and owning layer.
- [x] Implement the smallest existing wallet/storage/query fix, avoiding
  federation-specific logic for a general wallet problem and preserving proof
  state, selection, fees, privacy, and restart safety.
- [x] Add the narrow regression/benchmark and verify the improved scaling curve,
  online/offline correctness, bounded resources, interrupted restart, and any
  other backend affected by shared code.
- [x] Remove profiling artifacts, record sanitized distributions and tests not
  run, update the milestone with cause or invalidation, and commit.

#### Acceptance

**Success criteria:** Token-preparation latency has a documented acceptable
curve and materially improves at any confirmed bottleneck without correctness,
privacy, fee, proof-state, resource-growth, or restart regression.

**Minimum verification:** Repeated small/medium/large fragmented-proof samples;
the focused wallet/storage test for the causal path; offline and online
preparation plus restart; standalone behavior; and another backend only when
shared storage/query code changed.

### Milestone 8: Controlled local and remote capacity characterization

**Status:** Pending

**Covers:** Issue 11 and the local/Nocean performance baselines

**Objective:** Establish repeatable throughput, latency, convergence, resource,
and saturation limits only after known correctness and liveness failures can no
longer invalidate benchmark results.

#### Work checklist

- [ ] After Milestones 1-5 are complete, pin a sanitized, reproducible local and
  remote benchmark setup with explicit workload and measurement boundaries.
- [ ] Run the minimum concurrency ladders needed to locate repeatable saturation
  for receive, swap, melt, and mixed work; attribute the first constraint with
  evidence and stop any run that violates correctness.
- [ ] Verify convergence, minority-fault behavior, restart after saturation,
  sustained resource bounds, and a standalone baseline. Put any new suspected
  defect in a separate unconfirmed `U` milestone instead of fixing it here.
- [ ] Remove benchmark-only instrumentation/artifacts, publish sanitized
  distributions and limitations, update affected claims, and commit.

#### Acceptance

**Success criteria:** Local and remote capacity tables are repeatable, include
uncertainty, define all latency/throughput boundaries, end successful runs in
exact federation convergence, identify the saturated resource or bounded queue,
and distinguish protocol, client, backend, and deployment costs.

**Minimum verification:** A short concurrency ladder for each distinct workload
shape; one quorum-preserving slow/offline-member case; restart after saturation;
one sustained run; and a standalone baseline. Local and remote results are
compared only when their revisions, configurations, workloads, and boundaries
match.

### Milestone 9: Production release validation and operator readiness

**Status:** Pending

**Covers:** Remaining release work from the implementation plan

**Depends on:** Milestones 1-8 and every confirmed `U` milestone

**Objective:** Demonstrate that the federation is safe, interoperable,
recoverable, operable, and understandable in production-like conditions before
any production-readiness claim.

#### Work checklist

- [ ] Map implementation-plan claims to existing evidence and identify only the
  missing release blockers; put every newly observed defect in a separate
  evidence-first `U` milestone rather than patching it during release work.
- [ ] Complete the independent review, real-backend, multi-host, recovery,
  upgrade, partition, sustained-load, backup/restore, and fault-model evidence
  needed to close those blockers.
- [ ] Finish operator procedures and monitoring for bootstrap, readiness,
  recovery, rotation, backup, limits, safe abort/rollback, and incidents; verify
  them through operator drills.
- [ ] Remove temporary/private artifacts, publish the sanitized evidence matrix
  and limitations, update both plans, run the final compatibility/protocol
  suite, and commit the release milestone.

#### Acceptance

**Success criteria:** Independent review has no unresolved critical/high issue;
required real-backend, multi-host, recovery, upgrade, fault, and sustained-load
evidence converges exactly; operators can execute the documented drills;
monitoring distinguishes the important federation states; and no confirmed
correctness, liveness, security, or data-loss issue is hidden by a readiness
claim.

**Minimum verification:** The sanitized release matrix, independent review and
resolution record, real-backend exactly-once evidence, multi-host
recovery/upgrade/partition evidence, Milestone 8 capacity report, operator
drills, and final standalone plus public Cashu protocol suites.

### Milestone U1: Validate payment-report quote-age semantics

**Status:** Unconfirmed

**Found during:** Focused local confirmation work associated with Milestones 1,
2, and 5

**Related issue:** Issue 13

**Observation:** Fake-wallet payment reports showed
`quote_age_secs` near 1.784 billion while `stale=false`.

#### Work checklist

- [ ] Independently reproduce the timestamp inputs and determine the cause,
  impact, and whether correctness consumes the value. Close without production
  code if this is only missing fake-wallet fixture metadata.
- [ ] If confirmed, apply the smallest timestamp/reporting fix with explicit
  unknown-age semantics and add the narrow regression.
- [ ] Verify fresh/stale/unknown behavior across persistence and the relevant
  fake/real backend, remove instrumentation, update the milestone, and commit.

#### Acceptance

**Success criteria:** Fresh, stale, and unknown quotes report consistent age and
staleness across restart, and no decision interprets missing time as a precise
epoch-sized age.

**Minimum verification:** Focused boundary timestamp tests and one persisted
fake/real backend comparison sufficient to prove the confirmed cause.

### Milestone U2: Validate network-pump retry duplication

**Status:** Unconfirmed

**Found during:** Nocean Issue 8 investigation, now Milestone 4

**Observation:** The network pump emits one outbound message per iteration; after
three remote sends satisfy threshold, only 25 ms remains for the fourth send,
and unfinished work may be retried. This could amplify traffic, but the observed
AlephBFT `UnitBroadcast` queue is not itself proof that CDK retried requests.

#### Work checklist

- [ ] Independently measure attempt identity/count and delivery with one
  controlled delayed peer, separating CDK retries from expected consensus and
  transport retransmission. Close without code if duplication is immaterial.
- [ ] If confirmed, apply the smallest bounded attempt-ownership/classified-retry
  fix without unbounded queues/concurrency or arbitrary timeout increases.
- [ ] Verify delay and reconnect delivery, queue/attempt bounds, quorum progress,
  and HTTP latency; remove diagnostics, update the milestone, and commit.

#### Acceptance

**Success criteria:** A delayed healthy peer receives required messages without
duplicate application work or retry storms, while attempts, queues, delivery,
HTTP latency, and quorum progress remain bounded.

**Minimum verification:** Focused completion/cancellation/retry tests and one
five-member delayed-peer plus reconnect process regression.

### Milestone U3: Validate stale or unusable AlephBFT backup replacement and fallback

**Status:** Partially confirmed; stale-backup fix verified, opaque-source
poisoning remains unconfirmed

**Found during:** Milestone 1 recovery-fix verification and Milestone U4's
two-stale-member process regression

**Observation:** Live-session backup bytes are authenticated in transit but are
opaque, append-order snapshots that legitimately differ between honest peers.
The Milestone 1 recovery path therefore downloads one peer's complete snapshot
after quorum-authoritative journal and state convergence. Its page shape and
size are validated and the snapshot is committed atomically, but a non-empty
local backup currently suppresses other sources. Upstream AlephBFT reports
decode, session, or DAG-consistency failures only inside `run_session`, whose
public API returns no typed startup result. A Byzantine peer might therefore
pin a recovered member to unusable bytes. This is code-path evidence, not an
independent reproduction of a real liveness failure.

The two-stale-member regression independently confirmed the adjacent concrete
failure. Both members recovered the exact 15-operation journal and finalized
cursor, but their non-empty session-0 backups came from the earlier
seven-operation snapshots. Sync skipped backup recovery solely because bytes
already existed. Both members then advanced alone to session 1 while members
1-3 remained in session 0; every member stayed fail-closed in
`awaiting_quorum`. Persisted state showed identical journal indices, order and
materialized digests, and finalized cursor 27, isolating the stale backup as
the remaining continuity input. A focused injected-transport test independently
proved that a non-empty local backup suppressed every honest backup request.

The smallest fix keeps intact restarts unchanged but treats any finalized-item
catch-up as proof that the pre-existing backup predates the recovered cursor.
It downloads one complete bounded authenticated peer snapshot and atomically
replaces the old session chunks. The transaction preserves the old snapshot on
rollback; malformed pages still fail before mutation. The two-stale-member
process test then converged all five members at 15 operations in session 0, and
the interrupted 15-to-51 recovery also converged in session 0. This does not
prove fallback from a syntactically valid but internally unusable remote
snapshot because upstream still provides no typed startup rejection.

#### Work checklist

- [x] Independently inject a non-empty stale local backup, prove that it
  suppresses honest recovery, and correlate it with a five-member session
  split after exact finalized catch-up.
- [x] Apply the smallest fail-closed peer-sync and backup-persistence fix for
  that confirmed case: replace the snapshot atomically only after finalized
  catch-up proves the local bytes stale.
- [x] Verify focused replacement, rollback, malformed-page atomicity,
  two-stale-member recovery, interrupted restart, readiness/session safety,
  and new consensus work.
- [ ] Independently inject one syntactically valid but internally unusable
  remote backup followed by an honest source and prove whether it can
  persistently block recovery. Do not require byte-identical honest snapshots;
  close without code if fallback works.
- [ ] If the remaining opaque-source hypothesis is confirmed, apply the
  smallest fail-closed peer-sync,
  session-supervisor, or backup-persistence fix, preserving quorum-authoritative
  journal/state and bounded authenticated-source attempts.
- [ ] Verify unusable-source fallback and remove the remaining fault injection;
  update this milestone and commit with its owning work.

#### Acceptance

**Success criteria:** One malicious, corrupt, unavailable, or incomplete source
cannot make a member falsely ready, rotate it alone, or permanently block
bounded recovery from honest sources in the quorum session.

**Minimum verification:** Focused unusable/incomplete-source fallback and
restart tests plus one five-member healthy-quorum process regression that
commits new work after recovery.

### Milestone U4: Validate catch-up certificate deadlock with two stale members

**Status:** Resolved and verified

**Found during:** Milestone 1 two-member-behind verification

**Observation:** Members 4 and 5 were restored concurrently to the same
seven-operation snapshot while members 1-3 retained 15 operations. Both stale
members correctly remained non-ready, but each repeatedly reported catch-up
certificate quorum `required=4, found=3`; the other stale member returned zero
entries where the eight-operation suffix was expected. A current member still
stored four independently signed certificate chunks for that suffix, but the
catch-up response exposed only its serving member's fresh signature. This is
one bounded process reproduction, not yet independent proof of the owning
mechanism.

A focused five-member-threshold transport test reproduced the exact mechanism:
the recovering member received three current signatures, the other stale
member returned no suffix, and recovery failed with `required=4, found=3`
without mutating its journal. Injecting the persisted historical signatures
from members 1-4 for the exact range and materialized digest made the same
request succeed. Duplicate, unknown, invalid, wrong-range, and wrong-digest
relays were ignored and never reduced the threshold.

The fix adds an explicit historical-certificate-page request used only by
behind-member recovery. A serving peer may then return a bounded exact-range
checkpoint and relay persisted signatures that independently verify against
that page and its reachable materialized digest. Ordinary evidence refresh
keeps returning the requested full range, avoiding an earlier test iteration
that opportunistically truncated normal restart responses. The final
five-member process regression recovered both seven-operation snapshots to 15
operations concurrently in 46 seconds, with all members healthy and ready in
session 0. The stale-backup session split found by the first fixed run was
tracked separately in U3 and is no longer masked as a certificate failure.

#### Work checklist

- [x] Independently reproduce the four-of-five signing deadlock with an
  injected transport, including historical certificate chunk boundaries and
  the bound materialized-state digest.
- [x] Apply the smallest existing catch-up/persistence fix that
  safely relays and verifies bounded historical evidence; do not weaken the
  threshold or let a stale member attest to state it has not materialized.
- [x] Verify success with two stale members and fail-closed behavior for
  missing, insufficient, duplicate, invalid, wrong-range, and wrong-digest
  evidence; remove generated snapshots and update this milestone and
  Milestone 1.
- [x] Commit the verified U4 change with its owning Milestone 1 work after the
  remaining Milestone 1 acceptance checks pass.

#### Acceptance

**Success criteria:** Two stale members can recover concurrently through the
three current peers when an authentic historical certificate quorum exists;
without that exact quorum, neither journal nor materialized state mutates and
the member remains non-ready.

**Minimum verification:** Focused relay-success and insufficient-evidence
tests, historical multi-chunk coverage, restart, and the five-member
two-stale-snapshot process regression.

### Milestone U5: Validate paged catch-up full-tip digest mismatch

**Status:** Closed; hypothesis not reproduced, no fix warranted

**Found during:** Milestone U4 certificate-boundary tracing

**Observation:** `from_journal` truncates returned entries to the catch-up page
limit while retaining the peer's full-tip order and materialized-state digests.
Catch-up certificates bind both the page entries and those full-tip digests,
which initially appeared to imply that recovery rechecked the full-tip digest
after each page. If true, the first page of a larger suffix would be asked to
equal a state reachable only after later pages. That initial reading was the
hypothesis investigated here, not established behavior.

Code tracing and an independent full-sync test disproved the suspected failure
mechanism. The client applies bounded pages in a loop until its operation and
finalized-item cursors reach the advertised peer tip; only then does it perform
the materialized-state digest recheck. It does not require an intermediate
page to materialize the full-tip digest. A real peer state and recovering state
were exercised with quorum-signed page evidence, exact finalized-item slices,
backup recovery, and final order/materialized digest comparison at 99, 100,
101, and 1,000 operations. All cases converged exactly, including the first
two-page 101-operation case.

A temporary 10,000-operation debug-build run also converged exactly with the
already supported 1,000-entry page setting in 113.66 seconds. The same workload
with the default 100-entry setting exceeded a 180-second sync bound, but did
not report a digest, certificate, cursor, or mutation error. That separate
page-cost result is tracked as unconfirmed Milestone U6; it is not evidence for
the U5 correctness hypothesis. The heavy temporary cases were removed and the
fast 99/100/101 boundary regression remains.

#### Work checklist

- [x] Trace the full client loop and verify that materialized-state comparison
  occurs after all pages reach the advertised peer tip, not after each page.
- [x] Independently run exact-boundary and over-boundary catch-up at 99, 100,
  101, 1,000, and 10,000 operations with signed evidence, finalized cursors,
  backup recovery, and exact final digests.
- [x] Close the disproved digest hypothesis without a production fix, retain
  the fast 99/100/101 regression, remove heavy temporary cases, and move the
  distinct 10,000-operation page-cost observation to U6.

#### Acceptance

**Success criteria:** Multi-page recovery reaches the exact peer operation and
finalized-item tip before comparing the advertised full-tip materialized
digest, remains bounded by the configured page limit, and does not introduce an
intermediate-page digest failure.

**Minimum verification:** Focused 99/100/101 real-state boundary regression,
temporary 1,000/10,000 scale runs, and the interrupted 15-to-51 process
recovery recorded in Milestone 1.

### Milestone U6: Validate large-history catch-up page-cost scaling

**Status:** Unconfirmed; one debug-build injected reproduction requires
profiling and production-representative validation

**Found during:** Milestone U5 scale verification

**Observation:** A real-state injected catch-up at 10,000 operations exceeded
180 seconds when using the default 100-entry page size. The identical workload
completed in 113.66 seconds with the existing 1,000-entry setting, while the
default setting completed through 1,000 operations and all 99/100/101 boundary
cases. No digest, certificate, cursor, readiness, or mutation failure occurred.
This suggests repeated per-page authentication, runtime reconstruction,
projection, persistence, or application cost, but the single debug-build run
does not establish production severity or justify increasing the default:
finalized items can contain large operation envelopes, so larger entry-count
pages may raise response and memory bounds substantially.

#### Work checklist

- [ ] Independently profile 1,000- and 10,000-operation catch-up with 100- and
  1,000-entry pages in release mode, recording per-page time, response bytes,
  finalized-item bytes, transactions, peak memory, and total convergence.
- [ ] Use temporary timing/counter instrumentation to separate certificate
  verification, finalized-runtime reconstruction, persistence, operation
  application, projection, and backup recovery; remove it after measurement.
- [ ] Confirm the result in a five-member process with one stale member and
  interruption/restart. Treat debug-only or fixture-dominated cost as a closed
  non-issue rather than changing production behavior.
- [ ] If confirmed, apply the smallest bounded owning-layer fix using existing
  replay/page patterns. Do not mask the cost with a longer timeout or raise the
  entry limit without an explicit byte and memory bound.
- [ ] Verify exact replay, certificate/digest rejection, bounded response and
  memory use, interruption/restart, readiness/session continuity, and
  standalone behavior; update the milestone and commit.

#### Acceptance

**Success criteria:** Large-history recovery scales predictably without
repeated superlinear reconstruction, preserves exact quorum-authoritative
state, and keeps every page, allocation, transaction, and retry bounded.

**Minimum verification:** Release-mode 1,000/10,000 comparison at both page
sizes, page-level cost attribution, one five-member stale/restart regression,
and focused malformed/certificate/digest boundary tests.

### Milestone U7: Validate BLS transaction-identifier persistence

**Status:** Resolved and verified during Milestone 2

**Found during:** Milestone 2 killed-client receive recovery

**Observation and independent confirmation:** The first fixed recovery run
restored one sat and avoided a duplicate federation operation, but CLI retry
failed with `malformed public key`. Both a normal receive wallet and the
recovered wallet held an incoming transaction whose `ys` blob was 48 bytes.
Code inspection showed that shared SQL concatenated variable-width public keys
on write but always decoded 33-byte chunks on read. Before any storage change,
a focused SQLite database conformance test persisted one BLS G1 transaction
identifier and failed while decoding it as secp256k1. The normal and recovered
rows plus the independent test established a shared storage defect rather than
a recovery-only artifact.

The smallest fix retains the existing blob column, writes the public-key vector
in self-describing JSON, and reads both that format and legacy homogeneous
33-byte secp256k1 or 48-byte BLS G1 rows. It adds no schema migration or
federation-specific database path and preserves standalone wallet rows.

#### Work checklist

- [x] Independently reproduce normal and recovery BLS transaction reads before
  changing storage behavior; identify the shared fixed-width decoder.
- [x] Implement one shared self-describing encoding with bounded legacy reads;
  do not add a federation transaction table or migration-only parallel path.
- [x] Verify new mixed-key encoding, legacy secp256k1 and BLS reads, SQLite BLS
  round trip, receive recovery, and standalone behavior.
- [x] Record the finding and verification with Milestone 2 and commit it in the
  same conventional milestone commit because it directly blocked M2 retry.

#### Acceptance

**Success criteria:** Transaction identifiers round-trip without assuming one
cryptographic key width; existing secp256k1 and experimental BLS rows remain
readable; malformed encodings fail closed.

**Minimum verification:** Shared new/legacy decoder test, SQLite BLS database
conformance test, the real recovered-receive retry, and standalone wallet/mint
coverage. All passed as recorded in Milestone 2.

### Milestone U8: Validate finalized-item catch-up response-count mismatch

**Status:** Confirmed transient mismatch; cause unconfirmed

**Found during:** Milestone 2 parallel receive verification

**Observation:** In one five-member run, member 5 was offline while ten
parallel receives completed on the four-member quorum. After restart, its
signed private status returned `503` throughout the 120-second bound. Its peer
sync attempts against three different members each failed with the same
message: finalized-item catch-up recovered 6 journal entries while the response
carried 10 items. The ten wallet processes had already succeeded and the active
four members were healthy, so M2 was rerun and passed with member 5 intentionally
offline. No catch-up code was changed.

The count mismatch could indicate an invalid equality check between finalized
items and payload-bearing journal operations, a response construction error, a
cursor/range boundary defect, or state left by the long-lived experimental
fixture. One occurrence does not distinguish these explanations. This issue
must receive more scrutiny than an ordinary regression failure because relaxing
the check without proving the intended mapping could admit incomplete replay.

Fresh U9 offline-member verification independently reproduced the mismatch on
clean state. During successive member restarts, peer sync logged recovered
response-count pairs of zero-versus-two and one-versus-two. Unlike the original
120-second failure, every affected member retried, reached the exact current
operation height, became healthy, and the final five-member digest comparison
converged at 77 operations. This confirms that the mismatch is real and can be
transient, but does not establish whether the rejected response or the local
reconstruction count was wrong. U9 therefore did not weaken or alter this
check; the invariant still requires the dedicated investigation below.

#### Work checklist

- [ ] Independently reproduce from fresh state with controlled offline suffixes
  containing known payload and payload-free finalized items; record cursors,
  item counts, operation counts, and response boundaries on both peers.
- [ ] Trace the invariant from response construction through finalized-runtime
  reconstruction and journal persistence. Confirm whether six-versus-ten is
  valid compression or actual missing application before attempting a fix.
- [ ] If confirmed, implement the smallest correction in the existing paged
  finalized-item recovery path; do not suppress the mismatch, weaken digest or
  certificate checks, or lengthen the reconnect delay as a workaround.
- [ ] Verify exact journal/finalized-item/digest convergence, 99/100/101 paging,
  interruption/restart, mixed empty/payload units, and standalone behavior;
  update this milestone and commit separately.

#### Acceptance

**Success criteria:** A member offline across a mixed finalized-item suffix
either recovers the exact quorum-authoritative journal and runtime state or
fails closed for a proven malformed response. Valid payload-free finalized
items never create a false count mismatch.

**Minimum verification:** Two fresh independent reproductions with controlled
item composition, a focused response-boundary unit test, one five-member
offline/rejoin process test, and relevant malformed/digest/certificate tests.

### Milestone U9: Validate intermittent 60-second melt-quote creation latency

**Status:** Resolved and verified

**Found during:** Milestone 3 post-fix process verification

**Observation:** In the successful fresh two-batch run with member 5 offline,
the first melt-quote creation completed in 442 ms. The second took 60.461
seconds, from 3.971 seconds to 64.432 seconds in its batch timeline. Quote
visibility then completed in 8 ms and melt confirmation in 1.024 seconds. The
workload still exited within its bound, and members 1-4 ended healthy and
converged with empty pending queues. The earlier pre-fix minimal run created its
second melt quote in well under one second, so the long delay is not yet a
stable part of the original Issue 6 reproduction.

#### Reproduction and root cause

Fresh five-member all-healthy runs reproduced both mint- and melt-quote
outliers at 62-65 seconds. Temporary lifecycle, handler, consensus-lock, and
SQLite write-gate instrumentation established one shared timeline:

- normal submissions reached AlephBFT's data provider within about 500 ms and
  finalized in roughly two to three seconds;
- during an outlier, the preferred member entered the public quote handler but
  its readiness check observed a temporarily non-ready sync state and returned
  before quote preparation or consensus admission;
- no SQLite write-gate wait exceeded one second, consensus locks were acquired
  immediately, and the next member's identical submission finalized in about
  2.5 seconds; and
- the wallet nevertheless waited for the entire 60-second member-fallback
  budget. On a non-definitive returned error, it repeatedly polled threshold
  quote visibility until the remaining budget expired. Because the first
  member had rejected the request before admission, no quote could become
  visible; only then did the wallet try the next member.

This rejects the initial transport-reuse, fakewallet-preparation, SQLite
contention, consensus-lock, and AlephBFT-finality hypotheses. The owning defect
was the wallet's ambiguous-creation recovery policy after an immediate member
error, not a slow server or an undersized timeout.

#### Fix

Federation quote IDs are assigned once by the wallet and reused across member
attempts. After a non-definitive member error, the connector now performs one
threshold-visibility check to recover an already accepted ambiguous request;
if the quote is not visible, it immediately tries the next member with the same
ID. Actual in-flight member calls retain their configured timeout, and timed-out
submissions retain the existing final visibility check. The change removes the
unbounded recovery loop without adding a retry, weakening validation, or
creating a second quote identity.

#### Verification results

- The complete federation-wallet test group passed: 88 tests, including
  delayed-submission recovery, timeout fallback, pending-quote recovery,
  definitive-error handling, and the new mint/melt regressions. The regressions
  use a paused 30-second fallback budget and prove that an immediate error falls
  through before that budget while both members receive the same assigned quote
  ID.
- Standalone HTTP mint- and melt-quote serialization tests passed unchanged.
- A clean release five-member all-healthy run completed 20 standard batches.
  The maximum observed mint-quote time was about 5.3 seconds, the maximum
  melt-quote time about 3.0 seconds, and the slowest complete batch about 20.0
  seconds. The run ended with 147 converged operations, all five members ready,
  and no pending work.
- A second clean federation completed ten one-member-offline batches. Member 5
  was offline for six batches, and members 4, 3, 2, and the preferred quote
  coordinator member 1 were each offline for one batch. The preferred-
  coordinator-offline batch created its mint quote in about 4.6 seconds and its
  melt quote in about 2.5 seconds. Every restarted member caught up and became
  healthy; the final five-member comparison converged at 77 operations.
- All temporary instrumentation was removed before the final builds and runs.
  The restart exercise also produced the fresh U8 evidence recorded in that
  milestone; no U8 behavior was changed here.
- Formatting checks pass for both changed Rust files. Workspace-wide formatting
  remains blocked by a pre-existing difference in the federation test module,
  and strict Clippy remains blocked in `cashu` by the existing Rust 1.95
  conversion, panic-documentation, and enum-size findings; neither blocker is
  in the U9 change.

#### Work checklist

- [x] Run fresh bounded quote-only and standard-batch samples with all members
  healthy and with each member offline in turn. Record coordinator choice,
  member request/fallback phases, consensus submission/finality, public handler
  latency, and quote visibility without changing protocol behavior.
- [x] If the delay reproduces, isolate one owning phase and compare its latency
  with the configured fallback, member-request, submission, and observer
  bounds. Reject correlated queue or status symptoms as causes without a shared
  timeline.
- [x] If confirmed, implement the smallest correction in the existing quote
  coordinator, connector, handler, or consensus boundary. Do not increase a
  timeout, add retries, or special-case an offline roster position to mask it.
- [x] Verify a latency distribution across repeated all-healthy and
  one-offline runs, cancellation cleanup, quote idempotency, restart, and
  standalone quote creation; update this milestone and commit separately.

#### Acceptance

**Success criteria:** Either repeated controlled evidence invalidates the
observation, or the confirmed owning phase completes within a documented bound
without duplicate quote creation, partial persistence, reduced quorum
validation, or standalone regression.

**Minimum verification:** At least ten fresh quote creations per topology, one
controlled slow/unreachable coordinator case, focused coverage of the proven
boundary, interruption plus idempotent retry, and unchanged standalone quote
behavior.

### Milestone U10: Validate parallel quote-state observation failures

**Status:** Unconfirmed

**Found during:** Milestone 5 signed-status load verification

**Observation:** A fresh local five-member federation completed two
parallelism-five batches of 20 quote/payment-observation pairs with no failures.
An intermediate 30-quote batch at parallelism ten reported only 10 pairs in the
expected paid state and advanced the converged journal by exactly 20 operations.
Signed status remained responsive for 25 of 25 samples with 318 ms maximum
latency, every public probe passed, all five members remained healthy, and
member 1 recorded an invalid-operation alert count of 20 from the benchmark
client source.

The temporary harness retained only the final pair result, not the create
response, immediate quote-state response, HTTP status, or alert operation ID.
It therefore cannot distinguish create rejection, a valid unpaid-to-paid
observation race, identifier validation, overload admission, or a harness
expectation that polled payment state too early. The following
parallelism-five batch succeeded and reached operation 149. No quote,
consensus, alert, timeout, or retry change is justified from this one lossy
sample.

#### Work checklist

- [ ] Reproduce from fresh state at parallelism five and ten while retaining
  bounded sanitized phase timing, HTTP status, typed error category, quote
  state transitions, operation IDs, journal admission, and alert attribution.
- [ ] Poll accepted quotes through a documented payment-state bound instead of
  treating the first non-paid response as failure; compare valid identifier
  generation and exact operation deltas.
- [ ] If a product failure remains after the harness race is excluded, identify
  its owning quote, observation, admission, consensus, or alert boundary before
  changing code. Do not suppress alerts or add retries/timeouts speculatively.
- [ ] Verify the proven boundary at adjacent parallelism, interruption/retry,
  exact journal convergence, bounded status/public liveness, and standalone
  quote behavior; remove artifacts, update the milestone, and commit.

#### Acceptance

**Success criteria:** Either the result is explained and closed as an immediate
polling/harness artifact, or every accepted quote reaches a correct bounded
state and every rejected quote produces an accurate typed result without false
invalid-operation alerts or inconsistent journal application.

**Minimum verification:** Two fresh retained-phase runs at parallelism five and
ten, bounded quote-state polling, exact alert-to-operation correlation, focused
coverage of the confirmed boundary, and standalone comparison.

### Milestone U11: Validate release-only status fixture quote failures

**Status:** Unconfirmed

**Found during:** Milestone 5 focused authenticated-status verification

**Observation:** The new cancellation regression and 12 adjacent status tests
passed in the release profile. Two other existing tests failed deterministically
before reaching their status assertions:
`private_status_routes_read_persisted_journal_after_runtime_reset` and
`private_local_status_route_reports_stale_payment_observations` both received
`Unknown quote` while submitting their setup operation. Repeating each test in
isolation produced the same result. The Milestone 5 change adds only a separate
test and cannot alter either fixture.

The debug federation test target entered a compiler/linker stall and was
stopped after process inspection showed no active linker or CPU use, so this
turn did not establish whether the failures are release-only, fixture-ordering
defects, or real optimized-build behavior. They do not prove a status-liveness
defect, but setup operations failing by profile merit independent scrutiny
before either test is weakened or ignored.

#### Work checklist

- [ ] Run both tests on a clean current toolchain in debug and release with
  retained setup-phase ordering, quote persistence, operation IDs, and
  application errors; first resolve or bypass only the build-environment stall.
- [ ] Determine whether quote fixtures omit durable quote state, depend on
  debug timing/order, or expose a real release behavior. Do not change status
  assertions or convert `Unknown quote` into an accepted setup result.
- [ ] If a product or fixture defect is confirmed, apply the smallest existing
  quote-setup/application correction and keep the tests faithful to production
  ordering.
- [ ] Verify both profiles, related quote/payment-observation tests, status
  assertions, standalone behavior, and deterministic repetition; update the
  milestone and commit separately.

#### Acceptance

**Success criteria:** Both fixtures either pass deterministically in debug and
release with valid persisted quote setup or fail closed for an intentionally
invalid fixture whose expectation has been corrected without masking product
behavior.

**Minimum verification:** Repeated clean debug/release runs, setup-state and
operation-order evidence, the two focused tests, adjacent payment-observation
tests, and standalone quote coverage if shared production code changes.

### Milestone U12: Validate federation registry save interruption atomicity

**Status:** Unconfirmed

**Found during:** Milestone 6 interruption/retry verification

**Observation:** Interrupting one URL import after 50 ms left no federation
registry, and two retries produced exactly one valid entry. Code inspection
then showed that `FederationWalletRegistry::save` serializes the validated
registry and writes the destination directly with `fs::write`; it does not use
a temporary file plus atomic rename. The observed interruption occurred before
registry creation and therefore did not exercise open/truncate/write failure.

No partial registry or data loss was observed. A synchronous write of the
current bounded registry may complete too quickly for ordinary process timing
to reach the vulnerable interval, and filesystem behavior is another variable.
The code shape alone is insufficient evidence for a persistence change, but
the possibility of a truncated registry during process or host failure merits
fault-injected validation rather than an atomicity claim.

#### Work checklist

- [ ] Add test-only filesystem fault injection around open/truncate/write and
  process interruption, preserving the previous valid registry as the oracle.
  Test an absent registry, replacement, and a bounded multi-wallet registry.
- [ ] Compare the registry behavior with existing CDK durable-file patterns and
  record filesystem assumptions, permissions, cleanup, and restart results.
- [ ] If corruption or loss reproduces, apply the smallest existing bounded
  temp-file, sync, and rename pattern. Do not invent a general storage layer or
  claim durability beyond the proven filesystem contract.
- [ ] Verify malformed temporary/destination files, retry, concurrent command
  exclusion if relevant, secret-free permissions, and unchanged file/invite/URL
  imports; remove fault injection, update the milestone, and commit separately.

#### Acceptance

**Success criteria:** An interrupted registry save either leaves the previous
validated registry intact or leaves no registry on first creation, and retry
produces exactly one normalized entry without accepting partial JSON or
weakening validation.

**Minimum verification:** Deterministic faults at each write boundary, previous
file preservation, first-create and replacement cases, idempotent retry, and
all three import sources.

## Scope and evidence labels

This report consolidates:

- the 2026-07-13 five-member Nocean receive stress test documented in
  [federated-cashu-nocean-stress-test.md](federated-cashu-nocean-stress-test.md);
- the 2026-07-14 local five-member restart, recovery, and transaction benchmark;
- the 2026-07-14 focused local confirmation of Issues 1, 2, 3, and 5, including
  the incidental revalidation of Issues 6 and 7;
- the Milestone 2 current-revision cancellation, recovery retry, offline-member,
  and ten-wallet parallel receive regressions;
- the Milestone 3 accepted-author payment regression, one-offline sequential
  two-melt run, and restart/replay verification;
- the local CPU regression investigation already recorded in that stress-test
  document.

The labels used below are:

- **Confirmed:** directly reproduced with process, journal, database, or client
  evidence.
- **Most likely cause:** consistent with the trace and code path, but not yet
  proven by a minimal failing regression.
- **Resolved and verified:** reproduced before the fix and measured or tested
  after the fix.
- **Needs revalidation:** previously observed, but affected code has changed and
  the exact process workload has not yet been repeated.

## Test configurations

### Local benchmark

- Five members on one local machine.
- BLS signature threshold: 3 of 5.
- Consensus threshold: 4 of 5.
- AlephBFT consensus, SQLite databases, and fakewallet payment processors.
- Current release-mode `cdk-mintd` and federation-enabled `cdk-cli` built from
  the same working tree before the run.
- Strict restart health required ready members, one converged height and digest,
  catch-up-certificate coverage, and no digest or invalid-operation alerts.
- Temporary processes were stopped after the run.

### Focused local issue-confirmation run

- Two disposable five-member local federations using AlephBFT, SQLite, and
  fakewallet payment processors.
- BLS signature threshold 3 of 5 and consensus threshold 4 of 5.
- Release-mode CDK 0.17.0 binaries. The `cdk-mintd` SHA-256 was
  `ce7ecb1e78f28add3020f23d3d15652e40efdebe7986ce29f2a1a90d1fe8c779` and
  the `cdk-cli` SHA-256 was
  `ff1df5e97aaea6837168d188c6adcb90b3da852f943c24f6bf31883899fa6e29`.
- The first federation created a converged 35-operation history, stopped member
  5, deleted only that member's SQLite database, WAL, and shared-memory files,
  and restarted it while the other four members remained live.
- The second federation stopped member 5 with its database intact, advanced the
  other four members, restarted member 5, and overlapped its startup with 20
  isolated wallet receives. A later 43-wallet receive wave used a deliberately
  adverse one-second per-member deadline and a two-minute wall-clock bound.
- Signed status, journal row counts and states, AlephBFT active sessions, wallet
  balances, public-route availability, client output, and process completion
  were recorded. Bearer-token artifacts remained private.
- All temporary mint processes were stopped and all test listeners were closed
  after evidence collection.

### Nocean benchmark

- Five co-located members on a four-logical-CPU development server.
- Remote load generation over HTTPS.
- The same 3-of-5 signature and 4-of-5 consensus thresholds.
- Full details, binary identities, method, and raw result tables are in the
  dedicated Nocean stress-test document.

### Fresh Nocean backpressure reproduction

- The current local working tree was compiled for Linux on the development
  machine and copied to the server with `scripts/deploy_nocean_federation.sh`.
- The remote federation was stopped, all member databases, AlephBFT backup
  state, and DKG state were erased, and a new five-member federation was set up
  and started. This was an intentionally destructive reset of a development
  deployment.
- Before load, all five members were healthy and converged at zero operations,
  with empty mempools and finalized cursors at zero.
- Idle interval CPU was approximately 1.1-1.2% per mint and each new SQLite
  database occupied approximately 4 KiB.
- The default `scripts/benchmark_nocean_federation.sh` matrix was started. The
  failure appeared during its first timed case, `N=10, parallelism=1`, after
  token and receiver-wallet preparation had completed.
- CPU, public and loopback endpoint latency, signed federation status, journal
  height, DAG statistics, and AlephBFT repeat counters were observed while the
  case ran and after it was interrupted.

## Performance results

### Current local results

| Measurement | Result | Qualification |
| --- | ---: | --- |
| Initial five-member startup | 1.303 s | Zero-operation federation to five healthy live sessions |
| Sequential standard batch | 7 entries in 4.797 s | Mint, swap, melt, restore; not a raw swap benchmark |
| Parallel receive workload | 100/100 successful | One accepted swap per receive |
| Parallelism | 20 | Isolated receiver wallet per token |
| Client-completion journal rate | 25.09 entries/s | 100 entries in 3.986 s |
| Post-client convergence delay | 1.710 s | All five members converged after clients completed |
| Commit-to-convergence rate | 17.56 entries/s | 100 entries over the full 5.696 s interval |
| Final converged height | 316 entries | Matching order and materialized-state digests |
| Full five-member restart | 2.958 s | Exact timed rerun at height 316 |
| One-member intact restart | 1.673 s | Height 316 with certificate coverage |
| One-member erased recovery | Failed | Only height 1 after approximately 155 s |

The 100-receive result is the cleanest current local throughput measurement. It
includes CLI process startup, HTTP fanout, consensus, threshold-share
collection, wallet verification, and wallet persistence. Wallet creation and
federation import occurred before the timed interval.

### Focused local issue-confirmation results

| Measurement | Result | Qualification |
| --- | ---: | --- |
| Erased-member source height | 35 operations | Five-member signed convergence before erasure |
| Erased-member recovered height | 11 operations | Stalled through index 10 for more than 15 minutes |
| Erased-member status | `healthy`, `ready=true` | Local certificate covered only operations 0-10 |
| Erased-member session | 1 versus peer session 0 | Repeated bidirectional session-mismatch rejection |
| Intact-member overlap receives | 20/20 successful | 0.56-0.81 s per receive while member 5 restarted |
| Post-overlap convergence | 57 operations on all five | Matching journal and materialized-state digests |
| Adverse-deadline receive wave | 43/43 printed success | Every wallet persisted a one-sat balance |
| Client process completion | 0/43 within two minutes | Every process hung after printing `Received: 1` |
| Healthy-majority journal | 144 applied entries | Members 1, 2, 4, and 5 |
| Stalled minority journal | 100 entries | 99 applied and one accepted on member 3 |
| Intact member 3 restart | Journal recovered to 144 applied | Private status responsiveness also returned |
| Post-restart sessions | Four in session 1, member 3 in session 0 | Strict ready/converged status still exited successfully |

The one-second deadline case is an intentionally adverse correctness probe, not
a production throughput result. Its important result is the separation between
threshold success, wallet persistence, process completion, minority
application, and live-session convergence.

### Earlier Nocean results

| Workload | Observed result |
| --- | ---: |
| Sustained confirmed receive plateau | Approximately 0.27 swaps/s |
| Highest confirmed one-wave burst | 0.566 swaps/s |
| Highest journal-rate estimate | Approximately 0.566 entries/s |
| Concurrency-5 failure rate | 30.0% |
| Concurrency-10 failure rate | 47.5% |

These numbers must not be compared as if they were two revisions of one
controlled benchmark. The local run removed public-network latency and used a
different machine, newer code, a bounded one-wave receive test, and a corrected
commit-through-convergence timer. The improvement is encouraging, but it does
not by itself prove a 40-90x production throughput increase.

### Fresh Nocean backpressure incident

The fresh-state run did not complete a benchmark row because it was stopped
when the public endpoints became unavailable. Its partial measurements are
still diagnostically important:

| Observation | Result |
| --- | ---: |
| Timed case | `N=10, parallelism=1` |
| Operations before timed case | 6 |
| Operations after interruption and convergence | 10 |
| First successful receive | 6.587 s |
| Failed receive | 108.233 s |
| Failure detail | 0 of 3 required shares; all five member requests timed out after 90 s |
| Later successful receive | 35.082 s |
| Finalized items after recovery | 40 per member |
| AlephBFT DAG units after 10 accepted operations | Approximately 744 |
| Peak observed `UnitBroadcast` backlog | Approximately 80-160 per member |
| Mint CPU during observed bursts | Generally 4-12%, versus approximately 1-2% idle |
| Recovery | HTTPS recovered after backlog drainage; no restart required |

The interrupted case must not be presented as a completed TPS result. It does
show that the federation can enter a severe latency and availability failure at
parallelism 1, so this incident is not explained by excessive client
concurrency.

### Nocean cadence-amplification rerun

A subsequent run started from a healthy federation and increased concurrency
one bounded step at a time. The default matrix did not reproduce a complete
HTTPS outage, but it reproduced the unit amplification, periodic CPU
saturation, latency increase, and prolonged wallet preparation stalls:

| Case | Result | Confirmed TPS | Successful receive latency |
| --- | ---: | ---: | ---: |
| `N=10, parallelism=1` | 10/10 | 0.151 | p50 6.562 s, max 6.704 s |
| `N=20, parallelism=5` | 20/20 | 0.739 | p50 6.671 s, max 6.878 s |
| `N=20, parallelism=10` | 20/20 | 1.388 | p50 6.901 s, max 7.474 s |
| `N=20, parallelism=20` | 20/20 | 2.456 | p50 6.959 s, max 8.118 s |
| `N=40, parallelism=40` | Stopped during preparation at 20/40 tokens | Not timed | Preparation repeatedly paused for minutes |

During the final preparation run, aggregate mint CPU peaked at 369% on the
four-logical-CPU host, the reconstructed DAG reached 6,489 units at only 144
accepted operations, and `UnitBroadcast` reached approximately 81-83 per
member. Public endpoint latency increased, but lightweight server-local
`/v1/info` remained responsive. After stopping the benchmark, queues drained to
the normal five repeatable broadcasts and CPU returned to approximately 1% per
mint without a restart.

## Detailed correctness and liveness evidence

The issue records below preserve the raw observations, reproduction notes, and
earlier analysis. The milestone status and evidence gates above govern whether
implementation may begin; a `Most likely cause` in this section is a hypothesis,
not authorization to fix it.

### 1. Fresh-database member does not catch up

**Severity:** Critical  
**Status:** Resolved and verified in Milestone 1

Member 5 was stopped at converged height 316 and its SQLite database, WAL, and
shared-memory files were deleted. The other four members remained live. The
fresh member was then given approximately 155 seconds to reach the existing
height. It reached only height 1.

The member did not merely recover slowly. Its logs showed:

- AlephBFT backup state behind the unit-collection state;
- the AlephBFT creator and consensus service terminating early;
- the member starting or advertising session 1 while the existing federation
  remained in session 0;
- repeated non-retryable session-mismatch responses in both directions;
- the private AlephBFT network receiver closing.

The focused confirmation reproduced the same failure from a converged
35-operation history. The erased member initially recovered through index 10,
then remained at 11 of 35 operations for more than 15 minutes. It entered
session 1 while all four peers remained in session 0 and repeatedly received
non-retryable `expected=0, found=1` responses; it also rejected peer traffic as
`expected=1, found=0`. The smaller history proves that the defect does not
require hundreds of operations or a long-running database.

**Most likely cause:** startup ordering allows an empty member to install a live
AlephBFT session and mark itself healthy before peer-height discovery and REST
journal recovery. Once it is `Healthy` with a live session and inbound network,
`should_defer_catch_up_to_live_aleph_bft` defers REST catch-up. The empty member
then tries to participate without the persisted AlephBFT backup and finalized
history required to join the peers' current session. Session rotation makes the
split explicit rather than repairing it.

**Implemented fix:** a member with no trustworthy local checkpoint now begins
fail-closed in startup recovery, compares signed peer status, recovers journal,
finalized-item, application, certificate, and AlephBFT backup state, and only
then installs the live session and becomes healthy. Deferral to live AlephBFT is
valid only when the local live session is known to be continuous with the peer
session and history.

**Verified regression:** erased-member runs at seven and 36 operations, the
two-stale-member 7-to-15 case, and interrupted 15-to-51 recovery all remained
non-ready until exact state and quorum-session recovery. The permanent focused
tests cover startup gating, signed peer-view reconciliation, backup paging and
atomic persistence, certificate/digest rejection, and live-session continuity.

### 2. A behind member can falsely report healthy and ready

**Severity:** Critical  
**Status:** Resolved and verified in Milestone 1

At local height 1 versus peer height 316, the erased member reported:

```text
sync=healthy ready=true operations=1
```

Its catch-up certificate covered its one local operation, so
`--require-ready --require-catch-up-certificate-quorum` was also insufficient to
detect the 315-operation deficit. The benchmark had to impose the expected
height explicitly.

The focused confirmation reproduced the same claim at 11 local operations
versus 35 peer operations:

```text
sync=healthy ready=true operations=11 session=1
```

The member's certificate covered only indices 0-10 while the peers' certificates
covered indices 0-34. More importantly, the second federation demonstrated
that equal height and digest are also insufficient. After member 3 restarted,
all five members held the same 144 applied journal entries and matching order
and materialized-state digests, but member 3 remained in AlephBFT session 0
while the other four were in session 1. The strict command using
`--require-ready`, `--require-converged`, catch-up-certificate quorum, and both
alert checks exited successfully. All five members reported healthy and ready.

This is a distinct missing invariant: the operator can prove historical state
convergence while one member is unable to participate in the quorum's current
live consensus session.

**Most likely cause:** readiness proves internal consistency only against the
member's local tip. A fresh runtime defaults to healthy before it has established
a recent signed peer-height view, and the live-session catch-up deferral returns
without first marking the node lagging.

**Implemented fix:** readiness now distinguishes local consistency from
federation currency. A
member must not advertise authoritative readiness until it has compared itself
with quorum peer evidence during startup. The `awaiting_quorum` state keeps a
single-member health check fail-closed until startup peer validation completes,
and readiness also requires an active live session and inbound consensus
transport.

Readiness and strict convergence must additionally require a quorum-compatible
AlephBFT session view. Peer status exposes the locally represented session and
strict status compares it across members. A session mismatch now
make the member non-ready even when height, digests, application state, and
certificate coverage match.

**Verified regression:** the 42-test operator-status suite rejects inactive
consensus transport and equal-height session splits. Process interruption,
empty/stale recovery, intact restart, and two-member-behind cases observed no
false-ready interval and ended with all five members healthy in session 0.

### 3. Wallet operations can commit while the client reports failure

**Severity:** High  
**Status:** Closed on the current revision; historical Nocean failure shape was
not reproduced, and the reproducible recovery retry is fixed and verified

In the Nocean concurrency-5 and concurrency-10 cases, every attempted swap
advanced the converged journal even though 25 wallet processes exited without
enough shares. Database inspection later showed that all five mints applied all
25 operations and persisted operation-bound signature shares.

This creates an ambiguous client result: blind retry can encounter spent inputs
even though the original command reported failure.

The focused local revalidation used 43 isolated receiver wallets, full
five-member fanout, zero additional member retries, and a deliberately adverse
one-second per-member timeout. All 43 operations advanced the healthy majority
from height 101 to 144. Every CLI printed `Received: 1`, and every wallet
database persisted a one-sat balance, so the exact earlier shape of a normal
failure after commit did not recur. However, none of the 43 CLI processes exited
within the two-minute wall-clock bound. They were stopped only after their
success output and wallet persistence had been verified.

At the same checkpoint, four members had 144 applied entries while member 3
held only 100 entries, comprising 99 applied and one accepted. Thus threshold
success, wallet persistence, minority application, and client-process
completion can still diverge. The local result must not be described as 43
clean client successes merely because success text was printed.

**Historical hypothesis:** the wallet previously cached only the first
threshold-sized compatible member subset. One temporary catch-up/readiness error
therefore reduced a nominal 3-of-5 operation to 2-of-3. Even after scheduling all
members, dropped unfinished requests were suspected of leaving minority
handlers accepted-but-unapplied. Current tracing and cancellation reproduction
invalidated that second mechanism; accepted server work has an independent
owner. Catch-up errors remain availability rather than rejection evidence.

**Required behavior:** all validated roster members remain eligible; retryable
member states do not count as rejection votes; the identical deterministic
request remains recoverable until threshold success, threshold definitive
rejection, or an overall deadline; and a deadline returns a typed
pending/ambiguous result rather than an ordinary failure.

Success must also have a bounded completion contract. After threshold success
and durable wallet persistence, surplus member work should be detached or
drained under a separate bound that cannot prevent the API or CLI process from
returning. Detachment must not cancel accepted server operations. Record
separate timestamps for threshold decision, wallet commit, user-visible output,
surplus-fanout completion, and process exit.

Milestone 2 killed a current client only after its operation was durably
pending. Server application still converged on every member, which invalidates
request cancellation as the owner of accepted work. The reproducible wallet
failure occurred on restart instead: recovery restored the value without
recording the original incoming transaction, after which command dispatch
submitted the spent token again. Recording the recovered transaction and
returning an existing incoming result made two independent process retries
return success without a second journal entry.

The historical Nocean failure-after-commit result remains a useful remote
regression, but no current code change is accepted for that unreproduced shape.
Milestones 4 and 5 own the remaining backpressure and status-unresponsiveness
conditions that can invalidate remote results.

### 4. Threshold completion can cancel minority member application

**Severity:** High  
**Status:** Request-cancellation hypothesis invalidated on the current revision;
durable application and restart replay verified

A local full-fanout follow-up found four members with an applied operation while
one member retained the same operation as accepted but unapplied. The client had
already obtained a threshold result.

The focused confirmation produced a larger variant. Four members reached 144
applied entries while member 3 remained at 100 entries with one row still
accepted and unapplied. The 43 affected clients had already printed success and
persisted their received balances. Restarting member 3 replayed the journal to
144 applied entries, proving that the durable history was recoverable but live
application had stopped.

**Historical hypothesis, now invalidated on the current revision:** pending
HTTP request futures are dropped once the wallet has enough shares, and this
was suspected of cancelling local application after journal acceptance.

Current code tracing and a durable-boundary process test show that the wallet is
not the owner of accepted application. The server persists pending work before
the handler waits, and an AlephBFT-owned application task continues after the
HTTP future is dropped. Killing the client after the pending row appeared led
to one applied operation on all five members and no residual pending row.
Accepted-but-unapplied and pending-before-backup restart tests also replayed to
the expected terminal state.

The historical minority divergence was real, but this causal hypothesis is
closed without a server fix. Remaining ways for a member to stop applying under
load belong to their independently evidenced startup/catch-up, consensus
backpressure, or status-liveness milestones. U8 records the new catch-up
mismatch found while testing member rejoin.

### 5. Wallet catch-up handling remains too brittle

**Severity:** High  
**Status:** Current threshold and ambiguity behavior verified; historical
load-induced failures remain assigned to the owning backpressure/status work

Wallet errors named members that were lagging, catching up, or verifying
catch-up certificates. These are retryable availability states, not definitive
transaction rejection votes. A catching-up member caused the command to fail
even when enough unused federation members could eventually provide shares.

The wallet must preserve operation IDs and blinded outputs, continue fanout to
all eligible members, retry with bounded backoff, and retain its saga on an
overall deadline. The mint should use a typed federation catch-up error and may
be able to return a strictly operation-bound persisted share for an exact replay
before applying the general new-request readiness gate.

The focused local process run produced two materially different results:

- Member 5 was stopped with an intact database while the other four advanced.
  After it restarted, 20 isolated receives overlapped its startup and all 20
  completed successfully in 0.56-0.81 seconds. All five members then converged
  at 57 operations. This is positive revalidation of the ordinary one-behind-
  member case.
- Under the later 43-wallet wave, member 3 stopped applying at height 100 while
  four members reached 144. Peers timed out its signed private status, all 43
  clients printed success but did not exit within two minutes, and the cluster
  split across AlephBFT sessions. Restart restored member 3's journal but not
  its live session. This is not the original typed `catching_up` error, but it
  confirms that one non-completing member can still prevent bounded client
  completion despite a healthy signature and consensus quorum.

Milestone 2 confirmed that the centralized connector starts all configured
member requests and returns as soon as threshold success or a matching
threshold rejection is established. Focused tests passed for slow surplus
members, agreeing definitive rejections, disagreeing rejection ambiguity, and
timeouts mixed with invalid shares. A real four-of-five process test completed
one receive in two seconds and ten parallel receives with zero process or
balance failures and an exact ten-operation journal delta.

No connector change was justified. The earlier ordinary intact-restart case
and the current offline-minority case both pass. Load-induced HTTPS starvation,
signed-status unresponsiveness, session incompatibility, and the newly observed
finalized-item catch-up mismatch remain separate owning-layer defects in
Milestones 4, 5, and U8; they must not be hidden with wallet retries or larger
timeouts.

### 6. Repeated concurrent melts fail or stall

**Severity:** High for federation melt availability  
**Status:** Resolved and verified in Milestone 3

> **Update 2026-07-18:** the residual accepted-author veto noted below ("no
> member was permitted to call the backend" when the accepted author is
> offline) is now resolved. Payment authority rotates through the roster
> after a bounded, consensus-derived attempt lease
> (`FEDERATION_MELT_PAYMENT_ACTOR_LEASE_ITEMS`), and the takeover member
> safely continues the same payment under the durable caller-supplied
> idempotency key. See milestone `P1` in
> `docs/federated-cashu-consensus-deadlocks-2026-07-18.md` for the
> red-before-green evidence.

Ten concurrent standard-operation workers each completed one full mint, swap,
melt, and restore batch. During the second batch:

- nine workers returned `Payment failed` from melt confirmation;
- one worker stalled at `standard.melt-confirm.start` until the benchmark was
  stopped;
- mint, swap, check-state, and melt-quote stages had already succeeded.

This is not evidence of insufficient consensus throughput: the receive-only
test later committed 100 swaps successfully at parallelism 20.

The focused confirmation incidentally reproduced the stall without ten
concurrent workers. With member 5 offline but a valid four-member consensus
quorum, the first standard mint/swap/melt/restore batch completed. The second
batch progressed through mint, swap, check-state, melt-quote creation, and quote
visibility, then remained at `standard.melt-confirm.start` until the explicit
120-second scenario bound expired. Receive/swap-only work was used afterward so
this independent melt failure would not contaminate the wallet catch-up test.

This smaller reproduction suggests that high wallet concurrency is not required;
consecutive melt state or payment-observation history may be sufficient. It
also strengthens the need to test quorum operation with one member offline.

The minimal current-revision run established the cause. Static operation-ID
hashing selected payment actors from all configured members, including stopped
members. Its first melt selected active member 4 and completed; its second
selected stopped member 5. The active quorum had accepted and applied the
second reserve, but no member was permitted to call the backend and no payment
observation followed. The accepted journal envelope instead consistently named
active author member 2 on all four members.

Payment ownership now uses that authenticated, consensus-accepted author. The
wallet no longer computes a private copy of static actor election or waits for
that surplus connector. The post-fix sequential regression completed both
melts, produced one payment observation per melt, converged with the stopped
member excluded, and retained the exact result across restart. Focused
concurrent, failed, unknown, replay, catch-up, wallet threshold, and standalone
tests passed as recorded in Milestone 3.

The larger ten-wallet/two-melt workload remains useful capacity coverage, but
it is no longer needed to prove this causal fix. Any future failure in that
larger shape must be diagnosed independently rather than attributed to the
removed selector.

### 7. Signed private status can become unresponsive under load

**Severity:** High operationally  
**Status:** Historical failure not reproduced on the current revision; closed
without a production fix in Milestone 5

After the concurrent standard-operation workload, member 1 continued to answer
the public information endpoint in approximately 30 ms but did not answer signed
private status within 20 seconds. Other members logged repeated 10-second status
timeouts against member 1 and increased reconnect backoff to 60 seconds. A
normal restart restored member 1 status immediately and it was healthy at the
existing height in 0.172 seconds.

The focused confirmation reproduced the same route-specific failure on member
3. During token preparation, all five public `/v1/info` routes returned HTTP
200, but peers repeatedly logged ten-second signed-status timeouts against
member 3 with reconnect backoff increasing through one, two, four, and eight
seconds. A direct signed status request produced no response within a 15-second
bound. Database inspection showed member 3 stalled at 100 journal entries while
the other four reached 144. Restart restored private status immediately and
replayed all 144 entries as applied.

The failure coincided with member 3 remaining in AlephBFT session 0 while the
other four moved to session 1. The trace does not prove whether status
unresponsiveness caused application/session lag, resulted from it, or shared a
lock/permit cause. These events must be correlated in the next instrumented
run.

**Most likely cause:** a private-status handler, admission permit, or state lock
was held or starved, rather than the HTTP runtime being generally unavailable.
Member 1 was also the preferred quote coordinator during the failing workload,
which concentrated quote and payment-observation work there. Status generation
serializes increasingly large payment reports and inspects several federation
caches, so lock duration and response size are plausible amplifiers. No stack
sample captured the exact holder.

**Required regression:** continuously issue bounded signed status requests while
running repeated parallel mint/swap/melt batches. Every member must retain a
bounded status latency, and a timed-out client must not leak a private REST
admission permit or retain a state lock.

The regression must continue through AlephBFT session rotation and assert that
status responsiveness, journal application, and live-session membership remain
coupled. Capture handler admission wait, lock wait/hold time, serialization
time, response size, event-loop delay, and the operation/session state at every
timeout.

Milestone 5 revalidated the current revision with 100 remote and 260 local
signed-status requests. No request failed, local latency remained below 318 ms
including CLI overhead, the federation stayed converged at the historical
144-operation region, and an offline member caught up to operation 159 before
its first signed poll. Admission cannot create the historical silence because
it is fail-fast, and the new cancellation regression proves that three blocked
and cancelled handlers each restore the owned permit before a fourth request
completes. The historical failure remains useful context, but there is no
current causal evidence that permits, locks, report construction, or status
handling require a production change.

### 8. Small receive load can amplify empty consensus units and starve HTTPS

**Severity:** Critical for remote availability  
**Status:** Resolved and verified in Milestone 4; retry amplification remains a
separate unconfirmed U2 hypothesis

On a freshly reset federation, the first sequential receive case temporarily
made all five public mint endpoints unavailable. Loopback and public HTTP
recovered without a process restart after the AlephBFT broadcast backlog
drained. This rules out a persistent reverse-proxy configuration failure and
makes protocol/runtime backpressure much more likely than raw CPU exhaustion.

The following evidence was captured:

- only 10 accepted journal operations corresponded to approximately 744 total
  DAG units and 40 finalized items per member;
- reconstructed-DAG reports showed approximately 666 units during the first
  affected period and approximately 744 after a second burst;
- `UnitBroadcast` repeat counters reached approximately 161, fell through
  roughly 56-68 to approximately 5, then rose again to approximately 80-83 and
  drained again;
- creator logs printed `Delay passed.` at approximately 100 ms intervals during
  the bursts;
- all five wallet requests in one receive timed out after 90 seconds and the
  wallet returned `insufficient federation signature shares: required 3,
  received 0`;
- interval CPU generally remained around 4-12% per mint, with no host CPU
  saturation; and
- once the repeat queues drained, all five public `/v1/info` routes returned
  HTTP 200 again without restarting the federation.

The controlled rerun added stronger evidence:

- the default concurrency matrix completed successfully, proving that a full
  outage is intermittent rather than a deterministic threshold at concurrency
  20;
- each successful receive still took approximately 6.6-8.1 seconds;
- aggregate mint CPU repeatedly reached 300-369% on the four-logical-CPU host;
- the above-default run became operationally stalled during token preparation,
  before its timed parallel receive began;
- the federation remained journal-converged while the wallet waited, showing
  that consensus acceptance and client completion can separate; and
- stopping load allowed CPU and broadcast queues to return to idle without a
  process restart.

After restoring AlephBFT's upstream active schedule, the final state-preserving
Nocean validation completed the same ten-operation case without a public-route
outage or client failure. All members advanced from operation 158 to 168 and
converged healthy and ready. The timed window added 417 reconstructed units on
member 1, its reported `UnitBroadcast` count peaked at 90 and drained to the
idle floor of 5, and successful receive latency was 9.221 seconds at p50 and
9.409 seconds maximum. This is slower than the comparable pre-fix successful
run, whose p50 was 6.562 seconds, but it is bounded and explicitly accepted in
exchange for removing the unsupported cadence and protecting liveness.

**Confirmed source:** before the Milestone 4 fix, production overrode
AlephBFT's normal unit-creation schedule with a fixed 100 ms delay whenever the
shared activity hint was active. Upstream AlephBFT normally waits 5 seconds for
round zero and 500 ms for subsequent rounds through the range relevant here.
The active override therefore created units five times faster than the normal
steady schedule and fifty times faster for round zero.

At the same time, the CDK data provider explicitly returns `None` when it has no
transaction payload but the activity hint remains active. AlephBFT interprets
that as permission to create an empty unit. While a real payload is awaiting
finalization, all five members can therefore create roughly ten units per
second, or approximately fifty federation units per second, most without Cashu
payloads. Every unit creates dissemination, signature-verification, backup, and
private-REST work. The measured 100 ms creator logs, DAG growth, CPU bursts, and
`UnitBroadcast` queues match this mechanism.

This behavior came from the change named `fix: bound active AlephBFT cadence`:
the previous code retained AlephBFT's default schedule while active and used the
two-second slowdown only for idle tail rounds. The change instead applies 100
ms to every active round. The intent was to bound proposal latency, but the
result is a fivefold increase in normal active unit creation.

The principal code paths investigated were:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs`, especially the active
  use of the upstream delay schedule, `CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY`,
  the activity-broadcast loop, and delay configuration;
- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs`, especially
  `CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE` and
  `CdkAlephBftActivityHint`, `CdkAlephBftDataProvider::get_data`, and
  `mark_network_activity_finalized`; and
- `crates/cdk-axum/src/federation/aleph_bft_network.rs`, where inbound
  consensus messages extract activity identifiers and call
  `mark_network_activity`.

**Correction to the original hypothesis:** empty units do not directly renew
the activity lease. The inbound activity marker extracts IDs only from included
payload data, so a payload-free unit contributes no new activity ID.
Finalization also shortens matching body-hash and operation-ID leases to one
second. The defect is sustained fast empty-unit creation while legitimate
payload activity is pending, not a self-renewing empty-unit loop.

**Secondary likely amplifier:** the CDK network pump drains one outbound message
per iteration. It submits that message to all remote peers, but after reaching
the three-remote-member threshold it waits only 25 ms for unfinished peers.
Unfinished deliveries are classified as retryable and queued again. Under load,
a healthy but slightly slower fourth peer can therefore create duplicate
private-REST delivery work. This follows from the code, but the Nocean trace did
not expose the CDK retry-queue length, so it is not yet proven necessary for the
failure. AlephBFT's reported `UnitBroadcast` task count must not be treated as
the same queue.

**Reproduction:** on the development deployment only, start from a fresh
federation and then run the smallest benchmark case while monitoring every
member:

```bash
# Build locally, upload with SCP, install, and verify the current binaries.
scripts/deploy_nocean_federation.sh

# Destructive: erase only the disposable remote development federation.
ssh nocean 'cd ~/cdk-federation && \
  bash scripts/stop-federation.sh && \
  bash scripts/reset-federation-data.sh --yes && \
  bash scripts/setup-federation.sh && \
  bash scripts/start-federation.sh'

# Use a single sequential receive case before increasing concurrency.
CASES='10:1' scripts/benchmark_nocean_federation.sh

# If healthy, increase one bounded step at a time.
CASES='20:5' scripts/benchmark_nocean_federation.sh
CASES='20:10' scripts/benchmark_nocean_federation.sh
CASES='20:20' scripts/benchmark_nocean_federation.sh

# Optional above-default escalation. Stop on a bounded deadline; preparation
# itself may reproduce the stall before the timed receive begins.
CASES='40:40' scripts/benchmark_nocean_federation.sh
```

Before the timed receive, require five healthy converged members, record an
idle interval CPU baseline, and confirm all public and loopback information
routes answer. During the receive, sample process CPU rather than relying on a
single instantaneous value; continuously time `/v1/info`; retain signed status,
operation height, pending mempool size, finalized cursor, reconstructed-DAG
size, and AlephBFT repeat counters. Stop the case after a bounded deadline if
HTTPS becomes unavailable. Keep the benchmark directory private because it can
contain bearer tokens.

The amplification bug is reproduced even if every wallet command succeeds: CPU
bursts, DAG growth, and `UnitBroadcast` depth must be compared with the number
of accepted operations. The stronger liveness failure is reproduced when
operation or public requests time out, or wallet preparation stops making
progress while the journal is already converged. Continue observing after
stopping load: recovery synchronized with queue drainage, without a mint
restart, distinguishes this failure from proxy misconfiguration, database
corruption, or permanent deadlock.

**Required instrumentation and regression:** expose active versus idle unit
counts, empty versus payload-bearing units, activity-ID cardinality and lease
age, CDK retry-queue depth, per-peer send-queue depth, event-loop delay, and HTTP
handler latency. A five-member process test should submit a small bounded number
of sequential
swaps, require a bounded number of empty units per payload and bounded queue
depth, and continuously require public and signed private routes to respond.
The test must wait beyond the network-activity grace period and prove that all
members return to the normal idle cadence. A separate network-pump test should
delay one healthy peer beyond the 25 ms completion grace and measure whether
the retry policy creates avoidable duplicate delivery work.

### 9. Federation discovery/import can hang after a healthy config response

**Severity:** Medium  
**Status:** Historical failure not reproduced on the current revision; closed
without a production fix in Milestone 6

`GET /federation/v1/config` returned HTTP 200 with a valid configuration, but
`cdk-cli federation add --mint-url ...` hung during post-discovery
import/validation. Importing the same immutable config from a file succeeded.

**Most likely cause:** the problem is after config retrieval, probably member
metadata/keyset validation or incomplete fanout cancellation, not the discovery
route itself. The exact import path must be rerun after the wallet fanout work.

Milestone 6 showed that this hypothesis no longer matches the implementation:
current discovery fetches config and mint info only from the selected member,
validates them, and then uses the same registry path as file import. Ten URL
imports and ten file imports completed within 300 ms and produced equivalent
registries. Minority-member unavailability, interruption/retry, conflicting
source identity, connector use, and standalone rejection were all bounded and
left no partial or duplicate state.

## Detailed performance evidence

### 10. Sequential token preparation degrades with wallet fragmentation

**Status:** Resolved and verified in Milestone 7

The bounded local preparation run produced 240 independent tokens before it was
stopped. Progress slowed materially over the run. Only 84 new consensus entries
were added while producing those 240 tokens, because many sends could use exact
local proofs without an online swap.

The slowdown could not be attributed solely to consensus journal growth.
Milestone 7 isolated and fixed two local proof-count defects: shared SQL wallet
reads decoded the complete proof table before filtering, and same-denomination
coin selection repeatedly rescanned already-selected proofs. At 20,000
unrelated rows, filtered-read median improved from 108.921 ms to 0.710 ms. At
5,000 matching one-unit proofs, selection median improved from 181.707 ms to
1.406 ms. Serialization and warm process startup were independently small.

This does not retroactively assign every pause in the original mixed federated
workflow to these local defects: that run did not record per-send phase timing.
Milestone 8 must count offline versus online preparation and correlate remaining
latency with proof count, journal height, and federation calls rather than
combining those phases again.

### 11. Remote sustained throughput and latency remain poor

**Status:** Confirmed for the tested Nocean revision

The Nocean run sustained only about 0.27 confirmed swaps/s at concurrency 5 and
10, with failure rates rising to 30-47.5%. A one-wave concurrency-20 burst
reached 0.566 swaps/s but had approximately 32-35 second successful latency.

The dominant observed limitation was not raw rejection by consensus. Operations
continued to enter the journal while wallets failed to collect threshold shares
through member catch-up/readiness churn. Wallet fanout and catch-up orchestration
must be fixed and the same remote matrix repeated before treating these numbers
as a consensus-engine capacity limit.

### 12. Status and recovery work can scale with journal and report size

**Status:** Partly resolved; residual status risk not reproduced on the current
revision

Status, digest, certificate, and startup paths previously reloaded or replayed
unchanged journal state. Large payment-observation reports also make signed
status responses grow with workload history. The new caches remove major
repeated computation, but the member-1 status failure shows that bounded status
latency still needs a process-level load test.

Operator endpoints should expose summaries by default and retrieve detailed
rows or reports only on demand. Cache invalidation must remain tied to journal,
application, certificate, and observation mutation rather than polling.

Current local status requests already omit materialized row fingerprints by
default and return payment-observation summaries rather than underlying rows.
At operation 149, with roughly the same journal scale as the historical hang,
continuous signed status stayed below 318 ms including CLI overhead. This does
not establish an unlimited scaling claim, but it invalidates Issue 12 as the
cause of the current Milestone 5 workload; larger characterization belongs to
Milestone 8 unless a controlled report-size curve proves a separate defect.

### 13. Payment-report quote age can be nonsensical

**Status:** Confirmed with the local fakewallet configuration; production
payment backends need revalidation

Signed status in both focused local federations reported newly created mint and
melt quotes with `quote_age_secs` values around 1.784 billion seconds, close to
the current Unix timestamp rather than the quotes' actual age of minutes. The
same reports marked the quotes `stale=false`.

The most likely explanation is a missing or zero creation timestamp in the
fakewallet quote/report path, followed by age calculation against the Unix
epoch. The trace did not establish whether real payment backends populate the
field correctly. Even if test-only, the value makes local stale-payment
diagnostics misleading and can hide age-calculation regressions.

Status generation should represent an unavailable creation timestamp as
unknown rather than computing an epoch-sized age. Add unit and process coverage
for fresh, stale, terminal, and missing-timestamp quotes across fakewallet and at
least one real payment-backend fixture, and assert consistency between
`quote_age_secs` and `stale`.

## Resolved and verified performance defects

### Healthy live consensus was mistaken for recovery-worthy lag

Under modest swap load, a normal one-operation propagation gap caused the
peer-sync loop to mark a member lagging and start REST journal catch-up even
though its healthy live AlephBFT session was already delivering that operation.
In the corrected process-level reproduction, one member entered lagging 27
times, catch-up 15 times, and certificate verification 3 times during otherwise
convergent load.

The peer-sync loop now leaves delivery to AlephBFT while readiness is healthy
and both the live session and network input are installed. Startup and genuinely
non-healthy states retain fail-closed REST recovery. The post-fix five-member
regression converged with zero height skew and zero lag/catch-up transitions.

The erased-database failure in Issue 1 exposes the necessary boundary on this
fix: the optimization is correct only after startup has established that the
local persisted history and live session are continuous with quorum peer state.
A new empty member must not default into the healthy condition that suppresses
its required recovery.

### Effective federation config replayed the full journal on every read

The cache was consulted only after loading and deserializing the journal. BLS
key decoding and subgroup checks made repeated config reads increasingly
expensive. The cache is now checked before database access and invalidated after
an applied keyset rotation. Focused regressions verify one replay for repeated
unchanged reads and correct rotation invalidation.

### AlephBFT backup persistence had quadratic write amplification

Every backup flush appended to one growing SQL BLOB. At 130 operations, roughly
1.3 MB of logical backup produced about 3.34 GB of cumulative SQL expression
output per member. Backups are now stored as ordered independent rows, with
legacy migration and exact restart-prefix suppression. Identical chunks remain
valid and are not content-deduplicated.

### Repeated digest and certificate verification reloaded unchanged state

Materialized-state digests and validated catch-up-certificate quorum results are
now cached until relevant mutation. Detailed materialized rows are fetched only
after a digest mismatch. Focused cache, mutation, and migration tests pass.

### CPU runaway during idle/retry state

Before these fixes, the reproduced problematic state used approximately 8-17%
CPU per mint. After the fixes, local idle and accepted-but-unapplied states used
approximately 0.0-0.1% per mint, including repeated signed digest checks. This
proves the runaway CPU defect is fixed; it does not prove that all throughput or
liveness defects are fixed.

The fresh Nocean failure in Issue 8 is distinct. It showed consensus-message
amplification, queue backpressure, and HTTP starvation at moderate CPU rather
than journal-length-dependent idle CPU. Fixing the earlier replay and cache
costs therefore does not resolve or contradict the new observation.

## Recommendations from the focused local confirmation

### Make readiness a quorum-derived claim

Treat `ready` as a lease derived from recent signed quorum evidence, not as a
default local state. The lease must cover peer height, order and materialized
digests, catch-up certificates, active AlephBFT session, finalized cursor, and
successful traffic exchange in that session. Expiry, missing startup evidence,
height lag, or session mismatch must fail closed. Preserve a separate local-
consistency field so operators can distinguish internally valid storage from a
member that is current and able to participate.

### Separate recovery from live consensus activation

Startup should first classify the local checkpoint. A missing or untrusted
checkpoint must recover journal, finalized items, application state,
certificates, session state, and AlephBFT backup before installing live network
input. A trusted but behind checkpoint may use live AlephBFT only after proving
session continuity with quorum peers. Session rotation must require quorum
evidence and must not allow one member to unilaterally advertise a session that
its peers reject.

### Make server completion cancellation-safe and client completion bounded

Once a server has durably accepted an operation, request cancellation must not
control whether application and response persistence finish. On the client,
threshold success and wallet persistence should permit a bounded return while
surplus member work continues independently. A surplus request may be detached,
but never cancelled in a way that strands an accepted journal entry. Add a hard
post-success process-exit bound and expose separate metrics for threshold
decision, wallet commit, response drain, and exit.

### Treat session and application skew as first-class alerts

The strict status command should fail when members disagree on active session,
when any member has accepted/applying rows older than a short bound, or when a
member's signed status is unavailable even if public information remains
healthy. Summaries should show per-member session, journal height, applied
height, accepted/applying counts, peer-view age, and status latency without
requiring large payment-report serialization.

### Keep correctness probes separate from capacity benchmarks

Retain a small deterministic matrix for erased recovery, intact recovery,
session rotation, accepted-but-unapplied replay, slow/unresponsive surplus
members, consecutive melts, and post-success CLI exit. Run capacity curves only
after those invariants pass. Every process benchmark should impose a hard wall
clock and report partial client output, wallet persistence, journal state,
application state, and session state when the bound expires.

## Positive performance findings

- Initial empty-federation startup is fast at 1.303 seconds.
- Intact local restart replay is fast: 1.673 seconds for one member and 2.958
  seconds for all five at 316 operations.
- Parallel local receive throughput reached 25.09 client-completed committed
  swaps/s with no failures at parallelism 20.
- All five members converged 1.710 seconds after those clients completed.
- In the focused run, 20 receives overlapping an intact behind member's restart
  all completed in 0.56-0.81 seconds and converged on all five members.
- The CPU fixes eliminated journal-length-dependent idle spin in the reproduced
  state.

These results establish that normal restart, local swap throughput, and the
confirmed static melt-actor failure are not the remaining immediate blockers.
Consensus/HTTP backpressure, signed-status liveness, discovery/import, and the
unconfirmed scaling findings remain.

## Cross-milestone dependency summary

The roadmap and milestone checklists above are authoritative. In condensed
order, the dependencies are:

1. Investigate and fix fresh-member startup recovery, then make readiness depend on recent quorum
   height, digest, certificate, and live-session evidence.
2. Make strict operator status reject AlephBFT session divergence once the
   readiness evidence model is proven, even when
   journal height and digests match.
3. Prove, then implement, cancellation-safe server application and bounded client exit after
   threshold success; eliminate accepted-but-unapplied minority stalls.
4. Keep the resolved accepted-author melt regression: one wallet, one offline
   member, two consecutive melts, and exact restart/replay without duplicate
   payment.
5. Complete the cadence/backpressure causal chain, then bound empty-unit
   amplification and preserve HTTP liveness under small remote
   workloads.
6. Reproduce and explain the private-status hang with lock, permit, handler-latency, session,
   and application-state instrumentation.
7. Revalidate typed catch-up/ambiguous wallet outcomes with one and two slow,
   catching-up, unresponsive, and session-incompatible members.
8. Revalidate federation discovery/import after wallet fanout changes and make
   no production change if the current code invalidates the report.
9. Repeat controlled local and remote capacity curves only after the correctness
   and liveness fixes above.

## Cross-milestone regression matrix

This matrix is a shared reference. A test counts toward milestone completion
only when its checklist records the exact command, result, and relevant state
assertions.

- Fresh member at histories of 1, 100, 1,000, and 10,000 operations; measure
  time through exact digest/certificate convergence and assert non-healthy
  startup state.
- A cheap approximately 35-operation erased-member CI regression that waits
  beyond partial recovery and rejects any unsupported session transition.
- Intact one-member and full-cluster restart at the same heights.
- Equal-height, equal-digest members with deliberately mismatched active
  sessions; strict status and single-member readiness must fail.
- Session rotation under load with continuous assertions that every member
  remains in a quorum-compatible session and continues applying journal rows.
- Repeated 3-of-5 fanout with one and two slow/catching-up members.
- Offline-member catch-up across controlled mixed payload and payload-free
  finalized items, asserting response-item, recovered-operation, cursor, and
  digest relationships before accepting any count-check change.
- Repeated 3-of-5 fanout with one accepted-but-unapplied, status-unresponsive,
  or session-incompatible member. Threshold success must return within a
  bounded post-success interval without cancelling server application.
- A 43-wallet-style adverse-deadline wave that separately asserts user-visible
  result, wallet persistence, CLI exit, journal commitment, per-member
  application, and session convergence.
- Ten or more wallets performing at least two consecutive melts concurrently.
- One wallet performing two consecutive standard batches while one member is
  offline, with a hard melt-confirmation deadline and cross-member observation
  comparison.
- Continuous signed status polling during mint, swap, melt, restart, and catch-up
  load, with bounded latency and admission-permit accounting.
- Fresh and stale payment reports with valid and missing quote timestamps;
  reported age and stale classification must remain consistent.
- Fixed-wave receive tests with at least five repetitions per concurrency,
  measuring client completion and signed convergence separately.
- A fresh-state `N=10, parallelism=1` remote receive test with continuous public
  route probes, event-loop delay, activity-hint cardinality, empty/payload unit
  counts, and per-peer `UnitBroadcast` queue depth.
- Synchronized CPU, RSS, SQLite lock time, disk I/O, journal height, AlephBFT
  session/round, response-cache, and catch-up-transition telemetry.

Hard wall-clock limits should guard exploratory measurements. A single case
should normally stop after two minutes and report its partial height and state;
longer recovery tests should be explicit soak tests rather than accidental
benchmark stalls.
