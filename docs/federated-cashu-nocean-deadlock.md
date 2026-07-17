# Federated Cashu Nocean consensus deadlock

**Observed Nocean deployment build:**
`71a603714c0be9c9a413db133260cdc28330c40a`

**Date investigated:** 2026-07-15

**Date reconfirmed against deployed Git tree:** 2026-07-15

This commit is the exact code boundary for the deployment on which the incident
was observed. It contained checkpoint design documentation, but not the
checkpoint protocol implementation. The later checkpoint implementation commit
`b535b16f7bb33cc233073809f4b6ae2d5f569f8f` and the checkpoint work in the
current working tree had not been deployed to Nocean. Consequently, none of the
incident evidence below should be attributed to, or reinterpreted using, those
newer checkpoint changes.

## Reconfirmation against the deployed build

The findings were re-audited directly from the Git tree at
`71a603714c0be9c9a413db133260cdc28330c40a`, without using the checkpoint-modified
working tree. That commit changes only
`docs/federated-cdk-checkpoint-implementation-plan.md` relative to its parent;
there is no checkpoint protocol source file in the deployed tree.

The deployed source reconfirms the following:

- **Observed deadlock: confirmed.** `local_peer_status` locks
  `materialized_state_digest_cache` at lines 689-693 and retains it while
  calling status, consensus, journal, and materialized-state code at lines
  710-717. `apply_finalized_unit` holds consensus at lines 1706-1711 while
  calling `persist_finalization` at lines 1736-1737. That function commits the
  accepted row at line 1646 and then awaits the same cache mutex through
  invalidation at lines 1647-1650. The inverse lock order exists exactly in the
  deployed binary's source and matches the accepted-but-not-applied production
  boundary.
- **Catch-up-side lock inversion: confirmed but latent in this incident.**
  `apply_verified_catch_up_response` holds consensus from line 1036 through the
  `persist_catch_up` call at lines 1123-1129. `persist_catch_up` invalidates the
  same materialized cache at lines 1682-1683 before consensus is released at
  line 1131. This must be fixed with the observed finalization path, although
  production evidence shows normal finalization triggered the recorded
  deadlock.
- **Journal/finalized-item pagination mismatch: confirmed.** The route first
  selects and truncates a historical journal certificate at lines 86-89 and
  98-169 of `catch_up_routes.rs`, then independently loads the finalized suffix
  and limits it with `min(requested_limit, response.entries.len())` at lines
  218-238. Consensus replay appends only accepted, non-duplicate operation
  items, so finalized-item and journal-entry counts are not one-to-one. This is
  sufficient to produce the observed 76-replayed-versus-92-supplied rejection.
- **Unbounded finalized-history query: confirmed.** The deployed SQL query at
  lines 747-764 of `cdk-sql-common/src/mint/federation/journal.rs` uses
  `fetch_all` for every item at or after the cursor. Route-level truncation
  happens only after the whole suffix is materialized.
- **Finalized-stream authentication gap: confirmed and separate.** The deployed
  certificate fields at lines 2451-2469 of `transport.rs` commit to a journal
  range and state digests, but not the finalized cursor, item bodies, item
  outcomes, session, or unit provenance. Quorum verification explicitly
  returns success with zero signers for an empty journal page at lines
  2856-2867. This is not the cause of the honest-peer pagination failure, but it
  prevents the current range certificate from serving as a complete
  Byzantine-safe recovery proof.
- **Regression coverage gaps: confirmed.** The cache test at lines 5195-5252 is
  sequential. The duplicate-cursor test proves one journal entry can correspond
  to two finalized items at lines 9261-9262 but manually assembles the response.
  The default-limit test equates journal and finalized offsets at lines
  9418-9433. The whole-unit route test explicitly accepts one journal entry with
  zero finalized evidence at lines 23512-23527. There is no deployed test that
  coordinates the cache/consensus cycle or requires the route to return one
  mutually replayable catch-up page.

This reconfirmation is a source-level audit against the exact deployed commit,
not a claim that a new deployment reproduction was run. The production
database and log samples below remain the runtime reproduction. The absence of
a deterministic concurrency regression in this commit is itself one of the
test gaps that the repair must close.

## Executive summary

Two members of the five-member Nocean development federation stopped making
local consensus progress while their processes remained alive. The first
failure left the federation with exactly its four-member consensus threshold.
The second failure reduced it to three responsive members and stopped an
otherwise successfully broadcast and confirmed on-chain melt from reaching its
terminal Cashu state.

The evidence strongly identifies a lock-order deadlock between federation
status calculation and finalized-unit persistence:

```text
status request
  materialized_state_digest_cache
    -> waits for consensus

finalized-unit application
  consensus
    -> waits for materialized_state_digest_cache
```

Both failed members stopped at the same durable boundary: their final journal
entry had been committed as `accepted`, but it was never changed to `applied`.
That is the exact boundary at which `persist_finalization` has committed the
journal entry and attempts to invalidate the materialized-state cache while
`apply_finalized_unit` still holds the consensus mutex.

This is a federated mint liveness bug. It is not a wallet aggregation failure,
a FROST signing failure, or a failure to broadcast the Bitcoin transaction.

No code, database, process, or server state was changed while collecting this
evidence.

## Deployment and safety threshold

The affected deployment had:

- five federation members;
- a three-member FROST signing threshold;
- a four-member consensus and on-chain observation threshold;
- one `cdk-mintd` process and one SQLite database per member;
- the federated BDK on-chain backend on Mutinynet signet.

The distinction between signing and consensus thresholds matters. Three
members can construct a threshold signature, but four matching members are
required for consensus-backed operation and on-chain settlement in this
deployment.

## Code-level deadlock

### Status-side lock order: cache, then consensus

`local_peer_status` in
[`crates/cdk-axum/src/federation.rs`](../crates/cdk-axum/src/federation.rs)
first obtains an order status, then locks
`materialized_state_digest_cache` and retains that guard throughout cache-miss
recomputation:

```rust
let mut cache = state
    .federation
    .materialized_state_digest_cache
    .lock()
    .await;

// The cache guard is still live here.
let (mut status, entries) = local_peer_status_and_entries(state).await?;
let materialized_state_digest = materialized_state_digest(state, &entries).await?;
```

Relevant source in the deployed build: `local_peer_status`, lines 686-719.

`local_peer_status_and_entries` calls `local_finalized_item_cursor`, which calls
`federation_aleph_bft_status`. That ultimately locks the shared consensus
runtime:

```rust
let next_item_index = local_finalized_item_cursor(state).await?;
```

Relevant source in the deployed build: the same file, lines 794-818, and
`FederationNodeRuntime::aleph_bft_status` in
[`crates/cdk-axum/src/federation/node_runtime.rs`](../crates/cdk-axum/src/federation/node_runtime.rs).

On a cache miss, the effective order is therefore:

```text
materialized_state_digest_cache -> consensus
```

Both the periodic peer-sync status request and the authenticated operator
status request use `local_peer_status`. The exact request that won the race was
not observable after the fact; either request type can exercise this lock
order.

### Finalization-side lock order: consensus, then cache

`FederationNodeRuntime::apply_finalized_unit` in
[`crates/cdk-axum/src/federation/node_runtime.rs`](../crates/cdk-axum/src/federation/node_runtime.rs)
locks `operation_application`, then the consensus runtime:

```rust
let _application = self.operation_application.lock().await;
let mut consensus = consensus.lock().await;
```

Relevant source in the deployed build: `apply_finalized_unit`, lines 1702-1742.

While the consensus guard is still held, it calls `persist_finalization`:

```rust
self.persist_finalization(&finalized_items, &entries).await?;
*consensus = next_consensus;
drop(consensus);
```

`persist_finalization` commits finalized items and accepted journal entries,
then invalidates two caches before returning. Materialized-state invalidation
locks the same cache mutex used by status calculation:

```rust
tx.commit().await?;
if !entries.is_empty() {
    self.invalidate_catch_up_certificate_quorum_status().await;
    self.invalidate_materialized_state_digest().await;
}
```

```rust
async fn invalidate_materialized_state_digest(&self) {
    *self.materialized_state_digest_cache.lock().await = None;
}
```

Relevant source in the deployed build: `persist_finalization`, lines 1630-1651,
and `invalidate_materialized_state_digest`, lines 242-244.

The finalization-side order is therefore:

```text
consensus -> materialized_state_digest_cache
```

The same inverse order also exists in stale-member recovery.
`apply_verified_catch_up_response` holds `operation_application` and consensus
while calling `persist_catch_up`; that persistence function invalidates both
caches before its caller releases consensus. Relevant source in the deployed
build:
[`crates/cdk-axum/src/federation/node_runtime.rs`](../crates/cdk-axum/src/federation/node_runtime.rs),
lines 1020-1140 and 1654-1690. The production incident reached
the normal-finalization path, but a repair that changes only
`persist_finalization` would leave the same deadlock available during catch-up.

### Deadlock interleaving

A minimal interleaving is:

1. A status request observes a cache miss and locks
   `materialized_state_digest_cache`.
2. A finalized-unit task locks consensus.
3. The finalized-unit task commits its accepted journal entry.
4. The status request reaches `local_finalized_item_cursor` and waits for
   consensus.
5. The finalized-unit task attempts to invalidate the materialized-state cache
   and waits for the status request.
6. Neither task can proceed. There is no timeout on either mutex acquisition.

The finalized-unit task never installs `next_consensus`, never drops the
consensus guard, and never calls `apply_journal_entries`. This simultaneously
explains the accepted-but-not-applied row, stalled local Aleph creator, hanging
status endpoint, and absence of an error log.

## Durable database evidence

The following samples were read directly from the member SQLite databases in
read-only mode.

### Member 3: first deadlock

Final journal rows:

```text
operation 64  applied   chain_observation
operation 65  applied   chain_observation
operation 66  accepted  chain_observation
```

Final finalized-item rows:

```text
accepted_item_index 78  session 0  finalization 75  accepted
accepted_item_index 79  session 0  finalization 76  accepted
accepted_item_index 80  session 0  finalization 77  accepted
```

Member 3 also retained one pending session-0 `chain_observation`, corresponding
to the next operation that other members subsequently ordered. Its database
contained journal indexes 0 through 66, while the other four members continued
well beyond that point.

The key evidence is operation 66: it was durably accepted but never applied.
That places the freeze after `persist_finalization` committed the journal row
and before `apply_journal_entries` completed.

### Member 2: second deadlock

Final journal rows:

```text
operation 143  applied   chain_observation
operation 144  applied   transaction_broadcast_observation
operation 145  accepted  transaction_broadcast_observation
```

Final finalized-item rows:

```text
accepted_item_index 183  session 1  finalization 51  accepted
accepted_item_index 184  session 1  finalization 52  accepted
accepted_item_index 185  session 1  finalization 53  accepted
```

Operation 145 was member 5's successful broadcast observation for the pending
melt transaction. As with member 3, the last row was durably `accepted` but was
never applied.

This repetition on two independent members, at the same code boundary but on
different operation types and sessions, is strong evidence of a general
runtime deadlock rather than corrupt operation data.

## Runtime and log evidence

### Member 3 creator stopped while peers continued

Representative AlephBFT status reports from member 3:

```text
14:34:05  top row: [(0,225), (1,225), (2,226), (3,226), (4,226)]
14:34:15  top row: [(0,234), (1,234), (2,235), (3,235), (4,235)]
14:34:25  top row: [(0,254), (1,254), (2,236), (3,255), (4,255)]
14:34:35  top row: [(0,274), (1,274), (2,237), (3,275), (4,275)]
14:34:45  top row: [(0,294), (1,294), (2,237), (3,295), (4,295)]
14:34:55  top row: [(0,314), (1,314), (2,237), (3,315), (4,315)]
```

Creator index 2 is member 3. Its creator stopped at round 237, while it
continued receiving and reconstructing units from the other members. Peers
began reporting sustained member-3 status-request timeouts at approximately
14:34:25 UTC.

The Aleph library continued reporting its DAG as `healthy` because network DAG
reconstruction remained active. That health report does not cover the CDK data
provider or finalized-unit application task blocked on the shared consensus
mutex.

### Member 2 stopped during the second melt

Member 2 progressed through readiness counts 137, 141, 143, and 144 while the
second melt was being ordered. Its peers first reported sustained member-2
status timeouts at approximately 15:55:54 UTC.

Its session-1 Aleph creator later remained at round 269 while the other active
creators reached round 270. The final durable operation was the accepted but
unapplied operation 145 described above.

### Negative evidence

Across all five persistent member logs, searches found zero occurrences of:

```text
Failed query
database is locked
Pool exhausted
rollback failure
storage contention
panic / panicked
```

The failed processes remained alive. Their threads were waiting in normal
futex/epoll wait states, and the SQLite files showed no retained OS-level writer
lock. CPU use was modest at the inspection point.

This negative evidence weighs against a SQLite busy lock, process crash,
consensus replay, or CPU saturation. It is consistent with Tokio tasks waiting
indefinitely on mutexes.

## Melt reconstruction

### Earlier melt completed with one failed member

The earlier 12,345-sat melt reached `PAID`. Member 3 was already stuck, but
members 1, 2, 4, and 5 still provided the four members required by the
deployment's consensus and observation policy.

This proves that the first member failure did not immediately stop the
federation. It consumed all remaining fault tolerance.

### Pending 6,123-sat melt

The second melt's durable operation sequence was:

| Operation | Type | Result before the deadlock |
| ---: | --- | --- |
| 136 | `melt_quote` | Applied |
| 137 | `melt` | Applied |
| 138 | `address_allocation` | Applied |
| 139 | `transaction_proposal` | Applied |
| 140 | `transaction_signed` | Applied |
| 141 | `transaction_broadcast_intent` | Applied |
| 142 | `chain_observation` | Applied |
| 143 | `chain_observation` | Applied |
| 144 | `transaction_broadcast_observation` from member 4 | Applied |
| 145 | `transaction_broadcast_observation` from member 5 | Accepted, not applied |

The signed transaction was:

```text
txid: 6779ac74316d2ec34998d0a5f9f4b0c4b623be6b15676a86f3ae0e4626b2e367
recipient amount: 6,123 sat
transaction fee: 401 sat
settlement policy: 1 confirmation from 4 matching observers
```

Members 4 and 5 successfully broadcast it at approximately 15:55:41 UTC. The
transaction subsequently confirmed on Mutinynet at block 3,264,185. Bitcoin
payment success therefore preceded internal Cashu settlement.

## Why the quote remained pending after Bitcoin confirmation

The transaction broadcaster begins each pass with
`ensure_federation_ready_for_authoritative_read` in
[`crates/cdk-axum/src/federation/broadcast.rs`](../crates/cdk-axum/src/federation/broadcast.rs),
currently around lines 49-62. Once member 2 failed, the three responsive
members correctly entered `AwaitingQuorum` and the broadcaster failed closed.

`transaction_settlement_from_observations` in
[`crates/cdk-common/src/federation/transaction_proposal.rs`](../crates/cdk-common/src/federation/transaction_proposal.rs),
currently around lines 2583-2613, requires the configured number of matching
`Confirmed` observations for the same block and sufficient confirmation depth.
The policy embedded in operation 141 required four observers.

The federation had only two accepted broadcast observations and never reached
four consensus-backed confirmation observations. External chain confirmation
alone does not mutate consensus state.

Finally, `probe_federation_melt_quote_payment` in
[`crates/cdk/src/mint/federation.rs`](../crates/cdk/src/mint/federation.rs),
currently around lines 1227-1273, returns an on-chain paid observation only
after `completed_payment_for_quote` yields a terminal receipt. That receipt is
available only after the accepted transaction reaches `Confirmed` state.

The unfinished work therefore has two consensus-backed stages:

1. Four member confirmation observations must produce the completed on-chain
   transaction receipt.
2. Melt-quote payment observations must then finalize the quote, saga, and
   reserved Cashu inputs.

The deadlock stopped the federation before stage 1. The quote correctly
remained `PENDING` even though the Bitcoin transaction was confirmed.

## Restart recovery observations

The two stuck processes were restarted sequentially after the initial
read-only investigation. This confirmed that restarting releases the in-memory
deadlock, but also exposed a separate finalized-item catch-up pagination bug.

### Member 2 recovered and completed the melt

Member 2 was restarted first while members 1, 4, and 5 remained online. Its
durable database already contained session-1 finalized items through operation
145, so startup recovery could reconstruct the local runtime without crossing
a session boundary.

The restart produced the following progression:

1. Member 2 initially reported that it was verifying quorum catch-up
   certificates at operation count 146.
2. Its Aleph session loaded 1,083 units from its retained session-1 backup and
   continued from round 270.
3. Operation 145 changed from `accepted` to `applied`.
4. Members 1, 2, 4, and 5 converged through operation 158, with an `applied`
   journal tip on every member.
5. Operations 146 through 154 supplied additional chain and transaction
   confirmation observations.
6. Operations 155 through 158 were four applied `melt_quote_payment`
   observations.

The formerly pending quote then reached:

```text
quote: 019f667c-d8cd-7b92-8250-601cffd1cda1
amount: 6,123 sat
state: PAID
payment proof: 6779ac74316d2ec34998d0a5f9f4b0c4b623be6b15676a86f3ae0e4626b2e367:1
```

This recovery confirms several parts of the original diagnosis:

- the Bitcoin transaction and FROST signature were valid;
- the accepted operation was recoverable from durable state;
- releasing the in-memory mutex deadlock restored finalization;
- the remaining four members were sufficient to complete both on-chain
  observation settlement and melt-quote payment settlement.

### Member 3 restart exposed a separate catch-up bug

Member 3 was restarted only after the other four members had converged and the
melt had reached `PAID`. Its local state was much further behind:

```text
local journal operations: 67
local finalized items: 81
local Aleph session: 0
healthy peer journal operations: 159
healthy peer Aleph session: 1
```

Unlike member 2, member 3 needed journal and finalized-item catch-up across the
session-0 to session-1 boundary. It contacted each of the four healthy peers,
but every attempt failed deterministically with the same error:

```text
Federation AlephBFT finalized item recovery mismatch:
finalized-item catch-up recovered 76 journal entries but response carried 92
```

Representative attempts were logged against peers 1, 2, 4, and 5. Retrying or
restarting member 3 again cannot resolve this mismatch because it is produced
by the response construction and validation rules, not transient peer state.

### Cause of the catch-up mismatch

`maybe_attach_aleph_bft_finalized_items` in
[`crates/cdk-axum/src/federation/catch_up_routes.rs`](../crates/cdk-axum/src/federation/catch_up_routes.rs),
lines 218-239 in the deployed build, attaches finalized-item evidence to a journal
catch-up response. When the response carries catch-up certificates, it caps the
finalized-item page to the number of journal entries in the response:

```rust
let limit = if response.catch_up_certificates.is_empty() {
    requested_limit
} else {
    requested_limit.min(response.entries.len())
};
let (finalized_items, complete) =
    truncate_to_complete_aleph_bft_units(finalized_items, limit)?;
```

This assumes that one finalized Aleph item corresponds to one appended journal
entry. That assumption is false. Finalized units can contain items that do not
append a new journal operation, including duplicate or invalid operations, and
the finalized-item stream can therefore be longer than the journal suffix it
proves.

For member 3, the response contained all 92 missing journal entries but only
the first page of finalized items. Those items reconstructed 76 new journal
entries; the remaining finalized evidence was omitted from the page.

The client then calls `ensure_catch_up_entries_match_recovered_journal` in
[`crates/cdk-axum/src/lib.rs`](../crates/cdk-axum/src/lib.rs), lines 915-943 in
the deployed build. It correctly requires the recovered journal suffix to match the
entire response entry suffix exactly:

```rust
if response.entries.len() != recovered_entries.len() {
    return Err(aleph_bft_catch_up_recovery_mismatch(format!(
        "finalized-item catch-up recovered {} journal entries but response carried {}",
        recovered_entries.len(),
        response.entries.len()
    )));
}
```

The validation is appropriate, but the server has constructed a response whose
journal page and finalized-evidence page end at different logical boundaries.
The client cannot safely apply such a response and rejects it.

### Operational consequence

A restart is therefore only a partial recovery mechanism:

- it is sufficient for a member whose retained finalized history can fully
  reconstruct its durable journal, as demonstrated by member 2;
- it is insufficient for a sufficiently stale member when catch-up requires
  more finalized items than the response's journal-entry count, as demonstrated
  by member 3.

At the end of this recovery attempt, members 1, 2, 4, and 5 were converged and
the melt was paid. Member 3 remained in catch-up and had not modified its
durable journal beyond operation 66. No manual database repair was performed.

### Required catch-up fix and regression test

Catch-up response construction must page journal entries and finalized items at
a shared recoverable boundary. The finalized-item stream should determine the
page boundary:

1. Fetch a bounded number of complete Aleph units from the requested finalized
   cursor.
2. Replay those units to derive their exact journal contribution.
3. Return precisely that journal prefix and the finalized items that produced
   it.
4. Never cut through an Aleph unit, even when the requested item limit lands in
   the middle of one.
5. Attach a historical range certificate only if its exact journal end is also
   one of the complete-unit boundaries in the selected finalized page.

The item limit must permit progress when the first complete unit is larger than
the requested limit. Since an Aleph unit is bounded, the clean choices are to
treat the limit as soft for one complete unit or to define a separate
finalized-item page limit that is at least the maximum unit size. A response
must never contain a non-empty journal suffix with no replay evidence.

The current database query loads the entire finalized-item suffix before
truncating it in memory. The repair should add a bounded query with enough
lookahead to finish one maximum-sized unit and should enforce both item and byte
bounds on the response.

The implementation must not weaken the client's exact recovered-journal
comparison.

This mismatch does not require a session rollover. A minimal reproduction is a
single session containing an accepted operation A, a duplicate of A, and an
accepted operation B. Two finalized items then reconstruct only one new journal
entry while an independently paged journal can contain two. Invalid operations
and accepted non-operation federation items create the same cardinality
problem.

The mapping is defined by finalized-item classification in
[`crates/cdk-common/src/federation/consensus.rs`](../crates/cdk-common/src/federation/consensus.rs),
lines 1336-1375 in the deployed build: an accepted new operation appends a journal
entry, while a duplicate operation, rejected invalid operation, module fact,
catch-up certificate, or default item does not. Startup recovery follows the
same rule around lines 1571-1615. The route nevertheless fetches the finalized
suffix through
`get_federation_aleph_bft_finalized_items_from` and truncates it with the same
numeric limit used for journal entries.

The existing tests miss or normalize the bug in three ways:

- `aleph_bft_peer_sync_catches_up_finalized_cursor_without_new_operations`
  demonstrates that one journal entry can correspond to two finalized items,
  but manually builds the response and only advances a duplicate-only cursor;
- the default-limit paging test uses the journal operation index as the
  finalized-item index, embedding the invalid one-to-one assumption;
- the whole-unit route test accepts a response with one journal entry and zero
  finalized evidence for a two-item unit, even though the client must reject
  that response as unreplayable.

These cases are in the deployed version of
[`crates/cdk-axum/src/federation/tests.rs`](../crates/cdk-axum/src/federation/tests.rs),
at lines 9199-9364, 9367-9470, and 23482-23549 respectively; the critical
assertions and invalid offset construction are at lines 9261-9262, 9418-9433,
and 23512-23527.

Unit and route-level regression cases must cover:

- accepted A, duplicate A, accepted B;
- an invalid operation mixed with accepted operations;
- an accepted non-operation item mixed with operations;
- a multi-item unit intersected by the requested limit;
- a duplicate-only page that advances the finalized cursor;
- historical certificates whose end is aligned and misaligned with a complete
  unit boundary;
- page sizes immediately below, at, and above a configured boundary;
- rejection without local mutation when entries and recovered history differ;
- bounded query and response sizes, plus a progress guarantee.

A full-process regression should then:

1. stop one member before a session rollover;
2. allow the remaining quorum to rotate sessions and append more operations;
3. restart the stale member;
4. require paginated catch-up to make monotonic progress across the session
   boundary;
5. assert that all journal operations are reconstructed and applied;
6. assert equal operation count, finalized-item cursor, session, order digest,
   and materialized-state digest across all members.

### Authentication gap beyond the pagination bug

The current `FederationCatchUpCertificate` authenticates an exact journal range
and its order and materialized-state digests. It does not commit to the
finalized-item cursor, finalized item bodies and outcomes, or their Aleph
session and unit provenance. In addition,
`verify_catch_up_certificate_quorum` currently permits an empty journal page
with zero certificate signers. A duplicate-only or non-operation-only
finalized page can therefore be accepted from one authenticated peer without
quorum evidence for the finalized stream.

The certificate fields and quorum verification are in
[`crates/cdk-common/src/federation/transport.rs`](../crates/cdk-common/src/federation/transport.rs),
lines 2451-2535 and 2851-2930 in the deployed build. There is no dedicated
deployed regression for the successful zero-signer empty-page branch; add one
before changing it so the protocol decision is explicit.

This did not cause the observed honest-peer 76/92 mismatch, but it is a
Byzantine-safety gap. Correctly pairing pages is an immediate honest-liveness
repair; it is not a complete authenticated recovery protocol.

Add a focused protocol test for an empty journal page that advances the
finalized cursor through duplicate or non-operation items. It should document
the deployed zero-signer acceptance first, then the checkpoint implementation
must change the expected outcome so that one peer cannot advance authenticated
finalized history without a quorum commitment. Also test altered item bodies,
outcomes, session metadata, and unit provenance against the same checkpoint;
each mutation must be rejected before durable state changes.

The durable solution is the checkpoint design in
[`docs/federated-cdk-checkpoint-implementation-plan.md`](federated-cdk-checkpoint-implementation-plan.md):
a quorum certificate at a complete Aleph unit boundary must jointly commit to
the journal cursor and order digest, finalized cursor and digest, application
digest and frontier, session and unit watermarks, materialized audit digest,
and configuration epoch. A batch from one source remains untrusted until its
result matches such a quorum checkpoint, and promotion must be atomic.

The post-incident checkpoint types are directionally appropriate but are not a
fix by themselves. The runtime, persistence, routes, catch-up state machine,
verification, and atomic promotion milestones must all be implemented and
tested before range certificates can be retired safely.

## Consequences that are not the root cause

### Session mismatch log storm

Member 3 remained in Aleph session 0. The other four members eventually reached
the session round limit and automatically rotated to session 1. Member 3 could
not inspect or update its consensus runtime because the mutex remained held.

After rotation:

- member 3 rejected session-1 messages because it expected session 0;
- members 1, 2, 4, and 5 rejected member 3's session-0 messages because they
  expected session 1.

The resulting warnings are a consequence of the original deadlock, not its
trigger.

### Hung deployment status pane

`status_loop` in
[`scripts/nocean_federation_control.sh`](../scripts/nocean_federation_control.sh)
runs `cdk-mintd federation wallet-status --require-ready` without a timeout.
One invocation remained blocked while waiting for the deadlocked status
endpoint, so the pane stopped refreshing.

This is an observability issue and a symptom. Periodic peer status traffic is
already sufficient to exercise the deadlock; the operator status pane is not
required to trigger it.

### Readiness and catch-up failures

After two members became unavailable, the remaining three members reported
`AwaitingQuorum` at operation count 146. The broadcaster, wallet-liveness
worker, and payment observer repeatedly failed closed. Those warnings accurately
describe the loss of quorum; they do not identify why the members stopped
responding.

## Alternative explanations assessed

### Wallet or TUI bug

Unlikely. The melt request was accepted and ordered. The federation built and
threshold-signed the transaction, and multiple mint members broadcast the exact
signed bytes. The failure occurred after those steps inside member consensus
application.

### FROST signing bug

Ruled out for this incident. Operation 140 contains a complete signed
transaction, and the resulting transaction was accepted and confirmed by the
Bitcoin network.

### Federated BDK broadcast or chain-source failure

Ruled out as the primary cause. Members 4 and 5 returned accepted broadcast
results, and the transaction confirmed. The missing state is the federation's
consensus-backed confirmation evidence.

### SQLite writer contention or leaked writer gate

Not supported by the collected evidence. There were no database errors or
writer locks, and both members stopped at the same accepted-but-not-applied
boundary. SQLite remains one of the awaited operations under the consensus
guard and should not be held there, but it does not explain this incident as
precisely as the demonstrated cache/consensus lock inversion.

### AlephBFT replay or consensus CPU saturation

Ruled out. The processes were not replaying from disk, CPU use was modest, and
each affected Aleph instance continued receiving peer units. Only the local
creator/data-provider and status paths that require CDK's consensus mutex
stopped.

## Confidence and remaining uncertainty

Confidence that the failure is the cache/consensus lock inversion is **high**
because:

- the inverse lock order exists directly in the inspected code;
- two independent members failed at the exact same durable boundary;
- the final row on each member is committed as `accepted` but not `applied`;
- creator progress and the status endpoint both stopped, and both require the
  same consensus mutex;
- the processes remained alive without storage or panic errors;
- member 2's failure occurred precisely while finalizing the observation that
  remained accepted.

The remaining uncertainty is which individual inbound status request acquired
the cache mutex first. Production task stacks were not available, and peer-sync
and operator status routes share the same underlying function. This uncertainty
does not change the lock cycle or the required fix.

## Repair plan and acceptance criteria

No fix was applied during this investigation. The following is the required
scope for a later repair. It supplements the ordering and consistency work in
[`docs/federated-cashu-implementation-plan.md`](federated-cashu-implementation-plan.md);
the lock and recovery changes below must preserve the rule that members apply
the same ordered operation history before exposing authoritative state.

### Repair 1: remove the cache/consensus lock cycle

The materialized-state digest cache was introduced together with its
invalidation paths in commit `1e9425e4d6843ec824ef6b57894c688741be19da` to
address a real CPU regression. Deleting the cache would avoid this particular
mutex cycle but would restore repeated full-state digest computation and is not
an acceptable fix.

The cache mutex must protect only short, in-memory get and set operations. On a
cache hit, `local_peer_status` should copy the value and release the guard
immediately. On a miss, it should:

1. release the cache guard;
2. acquire the existing `operation_application` mutex as the snapshot and
   single-flight computation gate;
3. recheck the cache after acquiring that gate;
4. gather status with only short-lived consensus access, then query the
   database and calculate the digest without retaining either the cache or
   consensus mutex;
5. briefly reacquire the cache mutex to publish the result only for the exact
   cache key that was computed.

Using `operation_application` matters for correctness as well as CPU use. The
materialized digest is assembled from multiple database queries rather than a
single read transaction. The gate prevents finalized operation application
from producing a mixed snapshot and prevents concurrent status requests from
duplicating the expensive computation.

The inverse side of the cycle must also be removed. Both
`persist_finalization` and `persist_catch_up` should be persistence-only. Their
callers should install the next consensus state and release the consensus
mutex before invalidating either the catch-up-certificate cache or the
materialized-state-digest cache. Cache invalidation must never acquire a cache
mutex while consensus is held. The catch-up path has the same inverse nesting
as normal finalization and must be changed in the same repair.

If correctness requires invalidation before applying database changes, retain
a pre-application invalidation and invalidate again after reaching the terminal
state, but perform both outside the consensus critical section. The separate
`persist_journal_entries` path can retain its short invalidation because it
does not nest it beneath consensus.

Removing all database awaits from the consensus critical section is desirable
follow-up work, but it is a broader concurrency refactor and is not needed to
prove or fix this incident. It should not be bundled into the minimal repair
unless a separate invariant and regression suite justify the change.

### Deterministic deadlock regressions

The existing materialized-digest cache test in
[`crates/cdk-axum/src/federation/tests.rs`](../crates/cdk-axum/src/federation/tests.rs),
lines 5195-5252 in the deployed build, exercises sequential cache reuse and invalidation
only. It cannot detect inverse lock ordering, duplicate concurrent computation,
or stale publication.

The test must reproduce the old lock cycle deterministically before the repair,
not rely on a probabilistic load test. A test-only barrier should pause a
cache-miss status calculation at the point that makes the old implementation
hold the cache while waiting for consensus. The test should then:

1. start finalized-unit application;
2. verify that the accepted journal row has been committed;
3. release the barrier;
4. require both tasks to finish under a short timeout;
5. assert that the row reaches `applied`, consensus advances, and a subsequent
   status request completes.

Equivalent coverage is required for `persist_catch_up`, since fixing only
normal finalization leaves the second lock inversion intact. Additional cases
must cover consecutive finalizations and several simultaneous status cache
misses. They must prove that only one digest computation runs, no stale cache
value is published, and the digest corresponds to one coherent materialized
snapshot.

The pre-fix version of these tests should time out at the coordinated cycle;
the fixed version should pass without increasing the timeout.

### Repair 2: pair journal and finalized-item catch-up pages

Implement the shared complete-unit paging algorithm described above as the
smallest honest-liveness repair. Keep exact replay equality on the receiver,
make finalized-item retrieval bounded, and only use a historical range
certificate when its journal boundary aligns exactly with the selected complete
units. If no stored certificate aligns, use the existing live-quorum path for
that aligned page rather than trimming evidence independently.

This repair should land with all unit, route, malformed-response, pagination,
and session-rollover tests listed in the catch-up section. It must not claim to
solve the finalized-stream authentication gap.

### Repair 3: complete checkpoint-authenticated recovery

Complete the cut-through checkpoint plan after the two immediate liveness
repairs. A stale member must download untrusted batches, verify the resulting
state against a quorum-signed checkpoint at a complete-unit boundary, and then
atomically promote journal, finalized, application, and materialized state.
Once this path is complete, remove the old range-certificate recovery protocol
rather than maintaining two partially overlapping trust models.

The Nocean federation is a development deployment and may be recreated from a
fresh DKG and fresh member databases. No compatibility adapter or unsafe
fallback to the pre-checkpoint recovery format is required.

### End-to-end acceptance test

After the deterministic local tests pass, run a five-member process test:

1. create accepted, duplicate, invalid, and non-operation finalized history;
2. continuously request peer status while finalizing operations;
3. stop one member before rollover and let the four-member quorum enter the
   next session;
4. perform an on-chain operation while the member is stale;
5. restart the stale member and require bounded, monotonic catch-up;
6. assert equality of journal count and tip, finalized cursor and digest,
   active session, order digest, application status/digest/frontier, and
   materialized-state digest on all five members;
7. assert there are no accepted-but-not-applied rows, hung status requests, or
   finalized-item recovery mismatches.

Only then redeploy to Nocean and repeat the original status traffic and stale
member scenario. Record CPU use and digest-computation frequency to ensure the
repair preserves the cache's performance purpose. The deployment should show
no recurrence of the 76/92 mismatch and should converge without manual database
edits.

### Observability needed for verification

Add bounded metrics or structured logs for:

- materialized digest cache hits, misses, in-flight computations, and compute
  duration;
- catch-up page start and end cursors plus entry, finalized-item, unit, and byte
  counts;
- checkpoint target, verified source, and atomic-promotion result;
- recovery mismatch categories without logging full payloads, secrets, or
  unbounded state.

Adding a timeout to the deployment status loop is useful operational hardening,
but it only prevents a frozen pane and is not evidence that the deadlock is
fixed.

### Non-solutions

The following must not be accepted as fixes:

- increasing timeouts or adding retry storms around status or catch-up;
- deleting the materialized digest cache and restoring the prior CPU cost;
- weakening exact comparison between replayed and supplied journal entries;
- trusting the first authenticated peer for finalized history;
- returning ready before journal, finalized, application, and materialized
  state converge;
- manually editing cursors or journal state as the normal recovery procedure;
- adding checkpoint data structures without wiring persistence, quorum
  verification, state-machine transitions, and atomic promotion.

### Current local test prerequisite

At the end of this review, the federation-enabled `cdk-axum` test target was
blocked by an unrelated pre-existing test connector implementation in
[`crates/cdk-axum/src/custom_handlers.rs`](../crates/cdk-axum/src/custom_handlers.rs),
currently around line 3044, that still defines the removed
`MintConnector::resolve_dns_txt` method. Before running the new federation
regressions, synchronize that test-only connector with the current trait. This
is a build prerequisite, not part of the incident fix, and must not reduce the
federation test surface.
