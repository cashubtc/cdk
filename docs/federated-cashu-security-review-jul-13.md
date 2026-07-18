# Federation Security Review — July 13, 2026

## Verdict

**CHANGES REQUESTED.** The federation feature should not be deployed until the
critical payment-observer authentication defect and the high-severity consensus
availability defects are fixed and covered by adversarial regression tests.

## Scope

This review covers the committed `bls...bls-federation` diff, with emphasis on:

- authenticated private federation REST requests;
- AlephBFT proposal, finalization, persistence, and application;
- mint and melt payment-observation quorum formation;
- quote quarantine and administrative authorization;
- wallet BLS share aggregation and recovery;
- wallet member discovery, caching, timeouts, and fanout; and
- AlephBFT network retry scheduling.

Uncommitted workspace changes were excluded from the reviewed branch diff.

## Reproduction Progress

This table is the durable reproduction ledger. Each finding must move through
`Pending`, `In progress`, and then either `Confirmed`, `Refined`, or
`Not reproduced`. A finding is confirmed only when a focused executable test or
example demonstrates the claimed behavior.

| ID | Finding | Initial severity | Confidence | Status | Reproduction evidence |
|---|---|---:|---:|---|---|
| R1 | Signed member can impersonate payment observers | Critical | 99% | Pending | — |
| R2 | Byzantine finalized Aleph unit can halt honest members | High | 99% | Resolved 2026-07-18 (ledger `F2`); residual apply-time definiteness tracked as `DB-001` | `byzantine_balance_overflow_operation_does_not_halt_finalized_application` |
| R3 | One member can permanently quarantine arbitrary quotes | High | 97% | Pending | — |
| R4 | Melt status recovery aggregates unverified BLS shares | Medium | 99% | Pending | — |
| R5 | Member validation can wait indefinitely | Medium | 98% | Pending | — |
| R6 | Stale valid-member cache pins wallets to a dead threshold | Medium | 97% | Pending | — |
| R7 | Continuous outbound traffic starves Aleph retry delivery | Medium | 95% | Pending | — |

## Findings

### R1: Signed Member Can Impersonate Payment Observers

**Severity:** Critical | **Confidence:** 99%

`post_operation` authenticates a submit request but discards
`FederationVerifiedOperationAdmission::sender` before submitting the operation
envelope to consensus:

- `crates/cdk-axum/src/federation.rs:301-315`
- `crates/cdk-common/src/federation/consensus.rs:450-486`

Payment-observation admission only verifies that the payload's `observer` is a
member of the roster:

- `crates/cdk/src/mint/federation.rs:316-336`
- `crates/cdk/src/mint/federation.rs:437-452`

Quorum calculation counts this payload field as a distinct member, and conflict
keys are scoped by the same claimed observer:

- `crates/cdk-common/src/federation/operation.rs:269-297`
- `crates/cdk-common/src/federation/operation.rs:587-615`
- `crates/cdk-common/src/federation/operation.rs:1431-1454`

A single malicious member can therefore sign multiple submissions that claim
different roster identities. Matching forged observations can satisfy the mint
payment quorum and authorize unbacked issuance. Forged failed or unpaid melt
observations can also roll back a pending saga while an external payment may
still settle, making the input proofs spendable again.

**Required remediation:** Bind the observer to the authenticated REST sender and
to the finalized Aleph unit creator. Prefer deriving observer identity instead
of accepting it from the operation payload.

### R2: Byzantine Finalized Aleph Unit Can Halt Honest Members

**Severity:** High | **Confidence:** 99%

Aleph unit payloads carry self-reported federation, submitter, session, and unit
index fields:

- `crates/cdk-common/src/federation/consensus.rs:135-151`

Canonical decoding verifies the representation and internal body hash, but not
that these fields match the authenticated Aleph unit:

- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs:408-423`

`batch_finalized` discards `OrderedUnit.creator`, round, and hash and forwards
only the untrusted payload:

- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs:637-647`

Wrong federation/session metadata, empty items, or a duplicate self-declared
unit identity cause `finalize_next_unit` to return an error. That error escapes
the finalized-payload apply loop, whose supervisor halts the federation:

- `crates/cdk-common/src/federation/consensus.rs:1261-1300`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1301-1319`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1642-1658`

This turns one Byzantine proposal, which the BFT model is supposed to tolerate,
into a federation-wide availability failure.

**Required remediation:** Derive proposal identity and ordering metadata from
the authenticated `OrderedUnit`. Deterministically reject or ignore invalid
Byzantine application data and continue processing later finalized units.

**Resolution update 2026-07-18:** the structural variant (self-reported unit
metadata) was corrected earlier by reconstructing payload metadata from the
authenticated Aleph unit. The semantic variant is now corrected for pure
operation content: finalized classification runs all pure, deterministic,
operation-local validation and records untrusted contents as per-item
`RejectedInvalidOperation` without aborting the containing unit, with
red-before-green five-member evidence in
`byzantine_balance_overflow_operation_does_not_halt_finalized_application`
(milestone `F2` in `docs/federated-cashu-consensus-deadlocks-2026-07-18.md`).
Deterministic apply-time business failures that still return ambiguous errors
remain open as discovered bug `DB-001` in that ledger.

### R3: One Member Can Permanently Quarantine Arbitrary Quotes

**Severity:** High | **Confidence:** 97%

`PaymentObservationQuarantine` uses the generic authenticated member operation
route. Admission only parses the quote ID and requires a nonempty evidence list;
`OperatorHalt` requires no evidence:

- `crates/cdk/src/mint/federation.rs:477-495`

The evidence IDs are not checked for existence, quote ownership, or an actual
conflict. Once finalized, the quote-scoped conflict key makes the quarantine
permanent, while mint and melt application reject the quote:

- `crates/cdk-common/src/federation/operation.rs:1456-1463`
- `crates/cdk/src/mint/federation.rs:1793-1804`
- `crates/cdk/src/mint/federation.rs:2343-2353`

Consensus ordering does not provide threshold authorization. One member can
therefore censor every quote it learns about.

**Required remediation:** Require a threshold-signed administrative certificate
or deterministically verified conflict evidence. A unilateral emergency halt,
if retained, needs bounded scope, expiry, and a threshold-authorized recovery
transition.

### R4: Melt Status Recovery Aggregates Unverified BLS Shares

**Severity:** Medium | **Confidence:** 99%

Normal melt aggregation verifies member shares against the blinded output and
the member public share:

- `crates/cdk/src/wallet/federation.rs:1690-1726`

Melt status recovery groups responses using metadata that excludes the actual
signature value and then interpolates `c` values without share verification:

- `crates/cdk/src/wallet/federation.rs:1831-1944`
- `crates/cdk/src/wallet/federation.rs:2526-2555`

Collection stops after threshold matching metadata, so a fast invalid response
can poison recovery even when enough slower honest members are available.

**Required remediation:** Persist or retrieve the original blinded change
outputs, verify every member share, ignore invalid shares, and continue until a
valid threshold is available.

### R5: Member Validation Can Wait Indefinitely

**Severity:** Medium | **Confidence:** 98%

`member_request_timeout` is documented as the per-member fanout timeout, but
`validate_member_active_keysets_with_mint_info` does not apply it to
`get_mint_info` or `get_mint_keys`:

- `crates/cdk/src/wallet/federation.rs:60-80`
- `crates/cdk/src/wallet/federation.rs:732-845`

With bounded fanout, a hung early member can occupy a slot indefinitely and
prevent later healthy members from being scheduled. This is a logical deadlock,
not a mutex lock-order cycle.

**Required remediation:** Apply the configured timeout to both metadata calls
and count timeout as a non-definitive member failure.

### R6: Stale Valid-Member Cache Pins Wallets to a Dead Threshold

**Severity:** Medium | **Confidence:** 97%

Once the validation cache contains a threshold of members, it is returned
without health checking, expiry, or consideration of the remaining roster:

- `crates/cdk/src/wallet/federation.rs:741-745`

Subsequent signing and authoritative-read collectors operate on that subset. In
a 3-of-5 federation, losing one member from a cached three-member set prevents
progress even when the other two members are healthy.

**Required remediation:** Expire or invalidate cached membership and retry once
against the complete roster when a cached set cannot produce a threshold.

### R7: Continuous Outbound Traffic Starves Aleph Retry Delivery

**Severity:** Medium | **Confidence:** 95%

The Aleph network pump services retry traffic only when it drains zero new
outbound messages:

- `crates/cdk-axum/src/federation/aleph_bft_network.rs:445-511`

Continuous new traffic therefore prevents the retry queue from being serviced;
its bounded capacity can eventually evict missed deliveries required by a
recovering member.

**Required remediation:** Reserve a retry budget in every tick or fairly
interleave retry and new traffic.

## Validation Notes

The initial source review found no server-side mutex lock-order deadlock. R5 is
an indefinite asynchronous wait caused by missing timeouts. Focused executable
reproductions are tracked in the table above and will determine the final status
and confidence of every finding.
