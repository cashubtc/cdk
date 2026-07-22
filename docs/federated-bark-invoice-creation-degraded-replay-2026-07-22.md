# Federated Bark invoice-creation replay incompatibility

Date: 2026-07-22

Status: root cause confirmed from code history and production replay evidence.
The minimal backward-compatible verifier is implemented, locally tested, and
deployed with preserved state. All five members passed strict federation health
and reported Bark ready at the same accepted frontier with zero lag.

Scope: five-member Nocean federation using the federated Bark BOLT11 backend.

This report omits invoices, payment hashes, action IDs, quote IDs, preimages,
server addresses, federation identifiers, and custody material.

## Executive summary

Invoice creation failed before CDK accepted a new receive intent, asked Ark to
create an invoice, or persisted a mint quote. The immediate gate was federated
Bark readiness: replay of three previously accepted incoming payments failed,
so the mint correctly rejected new work.

The replay failure was caused by a backward-incompatible change to the result
commitment for `ReceiveHtlcsReady`. Historical records committed reconstructed
Bark evidence with `cdk/bark/receive-htlc-evidence/v1`. Commit `53c98ed36`
changed new records and replay to use
`cdk/federation/bark-receive-htlc-package/v1`, but did not bump the federation
Bark operation version, migrate accepted records, or retain a legacy verifier.
Current binaries therefore applied the new formula to old version-1 records and
rejected them.

The minimal fix keeps the current package commitment for all newly generated
records. During replay, an uncertified historical transition may also match the
exact legacy evidence commitment. Certified transitions cannot use the fallback
because certificates were introduced with, and bind, the package commitment.

Temporary production stage diagnostics and an exploratory receive-projection
mutex were useful during investigation but are not part of the retained fix.
No payment-specific recovery behavior or accepted-state mutation is added.

## User-visible failure

The failing endpoint creates a Lightning invoice that can later fund a Cashu
mint quote. Its relevant sequence is:

1. require a ready federated Bark projection;
2. accept a receive intent;
3. prepare the Ark invoice; and
4. persist the ordinary mint quote.

The request stopped at step 1. Consequently:

- no new receive intent was accepted;
- Ark was not asked to create an invoice; and
- no new 1,000-sat BOLT11 mint quote was persisted.

The mint did not receive Lightning funds for that failed invoice-creation
attempt because no invoice was created.

## Production evidence

Before the fix, all five members agreed on the accepted journal and were caught
up to the same frontier with zero Bark projection lag, but Bark readiness was
degraded. This ruled out consensus lag and a member database fork.

The journal contains three completed historical incoming payments. For every
corresponding accepted `ReceiveHtlcsReady` record, staged replay checks showed:

1. the action-transition structure and identity were valid;
2. each artifact SHA-256 matched its stored bytes;
3. every Bark VTXO deserialized canonically and round-tripped byte-for-byte;
4. VTXO IDs, amounts, locators, and expiry heights matched the accepted
   transition;
5. Bark reconstructed the receive evidence successfully; and
6. the request commitment matched.

Only the result commitment comparison failed. Recomputing the historical
formula matched the stored result on all three records and all five members.
After deploying the compatibility verifier, the logs confirmed three legacy
commitment matches per replay and the receive-history failure disappeared.

The deployment also exposed an unrelated, pre-existing outgoing send-plan
replay failure. It is not addressed by this fix, and no code is added to recover
or special-case that payment.

## Root cause

### Historical formula

The accepted records were created with:

```text
SHA256(
  "cdk/bark/receive-htlc-evidence/v1" ||
  action_id || payment_hash || destination_index ||
  gross_amount || net_amount || fee || minimum_expiry ||
  length_prefixed_serialized_vtxos
)
```

This was the authoritative result hash before `53c98ed36`.

### Current formula

The later code uses a domain-separated canonical package commitment:

```text
domain_hash(
  "cdk/federation/bark-receive-htlc-package/v1",
  canonical_json(
    minimum_expiry,
    request_hash,
    vtxo_transitions,
    vtxo_artifacts
  )
)
```

The package formula is stronger for federation certification because members
can authenticate the exact backend-independent artifacts and state transitions.
The defect was not adopting it; the defect was changing replay semantics for
already accepted version-1 records without a version boundary or compatibility
path.

### Why this deterministically blocked replay

Both hashes use different domains and different transcripts, so a valid old
record cannot match the new formula. Full replay always reaches those completed
receives. Failure prevents the readiness routine from clearing its degraded
state, and new invoice creation remains fail-closed at the readiness gate.

Retries and restarts cannot correct a deterministic commitment mismatch.

## Minimal fix

Replay now applies this rule:

1. reconstruct and validate the complete Bark receive evidence exactly as
   before;
2. recompute the current package commitment;
3. accept it when it matches the stored result;
4. otherwise, only if the transition has no evidence certificate, recompute
   and compare the historical evidence commitment; and
5. reject every other mismatch.

New transition generation is unchanged and always emits the current package
commitment. The fallback does not accept arbitrary data: it verifies the exact
previously authoritative digest after all artifact, VTXO, evidence, and request
checks have succeeded.

## Candidate assessment

### Confirmed: commitment migration without a version boundary

Confidence: confirmed. Code history supplies both formulas, and the legacy
formula matches the stored production results after every other validation
stage passes.

### Not retained: receive checkpoint replay race

An earlier production log contained `accepted receive is not materialized`, and
an in-memory persistence interleaving can reproduce that text. This establishes
a plausible concurrency hazard, but it does not establish that the hazard
caused the deterministic all-member outage. Once result-hash diagnostics were
added, every member consistently failed at the commitment comparison instead.
The exploratory runtime-wide receive mutex is therefore removed from the
minimal fix and should be evaluated independently if the checkpoint error
recurs after compatibility is restored.

### Ruled out: malformed historical VTXO evidence

Artifact integrity, canonical decoding, identity, amount, expiry, locator, and
Bark reconstruction all passed before the result-hash comparison.

### Ruled out: request commitment mismatch

The reconstructed request commitment matched every historical accepted record.

### Ruled out: consensus lag or federation split

All members agreed on the accepted operation count, frontier, and projected
state. Bark lag was zero when invoice creation failed.

### Separate issue: outgoing send-plan replay

The existing failed outgoing payment can independently keep strict Bark health
degraded. It is outside this incoming-invoice compatibility fix. It must be
diagnosed from its own canonical replay invariant; it must not be handled by a
payment-specific exception, database rewrite, or recovery shortcut.

## Verification

Local regression coverage verifies:

- current commitments are always accepted when exact;
- the exact legacy commitment is accepted only for an uncertified transition;
- the same legacy commitment is rejected when a certificate is present; and
- unrelated hashes remain rejected.

Commands:

```text
cargo test -p cdk-bark --features federation-bark
cargo check -p cdk-axum --features federation-bark
cargo clippy -p cdk-bark --features federation-bark --lib -- -D warnings
```

Production preserved-state validation confirmed that the three historical
receive commitments pass the legacy verifier. The cleaned deployment then
passed strict readiness on all five members at accepted frontier 122 with zero
Bark lag. The existing outgoing payment remained at its previously accepted
ambiguous phase; the deployment did not alter or special-case it.

## Acceptance criteria

The incoming invoice bug is fixed when:

- all historical uncertified receives replay under their exact legacy digest;
- current and certified receives still require the package digest;
- a new invoice request can advance past Bark receive-history replay;
- no new code special-cases or mutates the existing failed payment; and
- temporary diagnostics and exploratory synchronization are absent from the
  retained production diff.
