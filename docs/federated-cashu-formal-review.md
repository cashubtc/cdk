# Federated Cashu Formal Review

This document records the current formal review of threshold choice, BFT
assumptions, canonical operation encoding, and operation-ID binding for
CDK federated Cashu. It complements `docs/federated-cashu-review-checklist.md`,
which is the per-PR checklist.

## Scope

Reviewed code and design surfaces:

- `ThresholdParams` and federation config validation in
  `crates/cdk-common/src/federation.rs`.
- Consensus item admission and local consensus boundaries in
  `crates/cdk-common/src/federation/consensus.rs`.
- Operation envelopes, operation IDs, conflict keys, and balance audits in
  `crates/cdk-common/src/federation/operation.rs`.
- Mint-side federation apply/signing paths in `crates/cdk/src/mint/federation.rs`.
- Current design notes in `docs/federated-cashu-consensus-current.md` and the
  review checklist in `docs/federated-cashu-review-checklist.md`.

## Review Outcome

The current threshold and operation-ID design is acceptable for continued
development behind the federation feature gate, with one hard production
condition: CDK must not treat the current local deterministic consensus driver as
a Byzantine-fault-tolerant decision source. Production federation mode still
requires a real replicated BFT ordering backend, or an explicit operator
configuration that accepts a weaker trust model.

The current canonical sorted JSON encoding is acceptable for the current CDK
federation operation version. Deterministic binary encoding is not required for
this version as long as receivers continue to derive operation IDs locally from
submitted envelopes, golden vectors remain in tests, and any later binary format
is introduced with a new operation version rather than silently replacing the
hash input for version 1.

## Threshold And BFT Assumptions

CDK separates two thresholds:

- The signature threshold is the number of BLS shares needed to create a Cashu
  aggregate blind signature.
- The consensus threshold is the number of members needed to commit an ordered
  operation.

The validation rule `consensus_threshold >= signature_threshold` is required.
It prevents a federation from configuring a consensus quorum that cannot produce
enough signing shares for the operation it accepted.

The helper `ThresholdParams::with_bft_consensus(member_count, signature_threshold)`
uses `f = floor((n - 1) / 3)` and `consensus_threshold = n - f`. This gives the
usual quorum size for a fixed-roster BFT system with up to `f` Byzantine members
once a real BFT protocol provides agreement and finality. Examples:

| Members | Faults Tolerated | Consensus Threshold |
|---:|---:|---:|
| 4 | 1 | 3 |
| 5 | 1 | 4 |
| 7 | 2 | 5 |
| 10 | 3 | 7 |

The signature threshold may be lower than the consensus threshold, for example
3-of-5 signing with 4-of-5 BFT consensus. That is acceptable only because
signature shares are produced after the exact operation has been accepted. If
members sign before consensus, a lower signing threshold reintroduces the
overlapping-window mix-and-match attack.

## Required Safety Invariants

- No member returns a usable mint, swap, melt-change, or restore share before the
  exact operation is accepted by the federation decision source.
- Every signing share is traceable to one `FederationOperationId`, output index,
  member ID, keyset ID, amount, and blinded message.
- A member that is lagging, catching up, or halted on digest mismatch must fail
  closed on public signing paths.
- Accepted operations are replayed deterministically from durable storage before
  serving authoritative status or signing routes after restart.
- Payment state enters consensus through accepted observations and accepted
  payment operations, not nondeterministic payment-backend reads during replay.

## BFT Production Blockers

These are not optional production-hardening items; they are the line between the
current development implementation and a production federation:

- Replace the local deterministic decision source with a replicated BFT ordering
  backend, such as the planned AlephBFT adapter or an equivalent consensus
  engine.
- Ensure the BFT backend produces one total order for operation envelopes and
  consensus module facts under partitions, equivocation, duplicate delivery, and
  member restarts.
- Keep signed session outcomes and materialized state digests as recovery and
  operator evidence, but do not use them as a substitute for the actual BFT
  agreement protocol.
- Run multi-process tests for at least 4-node and 5-node federations once the
  BFT backend is implemented.

## Canonical Operation Encoding Decision

Current version 1 operation IDs use:

```text
operation_id = sha256(
  len("cdk-federation-operation-v1") ||
  "cdk-federation-operation-v1" ||
  len(canonical_envelope_json) ||
  canonical_envelope_json
)
```

The canonical envelope is produced by converting the operation envelope to a
Serde JSON value, recursively sorting all JSON object keys, and serializing it
without trusting any caller-supplied operation ID.

Decision:

- Keep sorted canonical JSON for the current federation operation version.
- Treat the existing golden vectors as compatibility fixtures.
- If CDK later needs a deterministic binary format for cross-language protocol
  stability, introduce a new operation version and new domain separator instead
  of changing version 1 semantics.
- Do not accept operation IDs from wallets, private REST peers, or config files
  as authority; always recompute the operation ID from the envelope bytes.

This decision favors CDK simplicity today while preserving an explicit upgrade
path for a stricter binary format later.

## Operation-ID Binding Review

`FederationOperationEnvelope::operation_id()` computes a domain-separated hash
of the complete canonical operation envelope. Consensus, the journal, signing
share provenance, duplicate detection, conflicts, catch-up, and persisted
responses use this operation ID directly. There is no second request-transcript
hash over the same bytes.

This means the operation ID changes when any consensus-relevant wallet request
detail changes, including:

- Quote IDs.
- Input proofs and witnesses.
- P2PK and HTLC witness data.
- SIG_ALL output sets.
- Blinded output messages, keyset IDs, and amounts.
- NUT-20 mint request signatures.

The tests cover NUT-20 output mutations, input witness mutations, P2PK/HTLC
witness mutations, exact SIG_ALL output binding, canonical key sorting,
operation-size limits, and golden vector stability. Restore is intentionally
absent because it is a threshold read, not a consensus operation.

## Open Risks

- Production deployment still needs independent external security review and
  repeated geographically distributed AlephBFT, TLS, payment-backend, and
  disaster-recovery drills beyond the local/regtest matrices.
- External independent implementations should not treat operation version 1 as a
  permanent wire standard until the federation protocol is explicitly frozen.
- Postgres live integration coverage still depends on an available Postgres
  service.

## Review Conclusion

The reviewed invariants, AlephBFT decision source, and dealer-free DKG are
sufficient for continued feature-gated development and production-readiness
testing. They are not by themselves a mainnet launch approval; the independent
review and deployment exercises above remain required.
