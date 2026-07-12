# Federated materialized-state projection benchmark

Date: 2026-07-12

This benchmark measures full federation materialized-state snapshot and digest
construction as accepted journal history grows. It was captured before changing
production behavior, then repeated after each candidate. The ignored Axum test
`benchmark_materialized_state_snapshot_growth` is the bounded reproduction
harness; it creates SQLite-backed states containing 100, 500, and 1,000 mint
quotes and matching accepted journal operations.

Absolute timings depend on the host. Query growth, relative timings, digest
equality, and unchanged row counts are the acceptance criteria.

## Baseline

The original Axum-owned projector performed one full journal scan and one
`get_mint_quote` call per referenced quote. Each quote load also read its
payments and issuance history, producing roughly `3N` SQL reads for this
quote-only profile.

| Operations | Debug | Release |
| ---: | ---: | ---: |
| 100 | 5,070 us | 1,101 us |
| 500 | 22,452 us | 3,342 us |
| 1,000 | 44,419 us | 5,917 us |

## Accepted implementation

Projection ownership now lives in `cdk::mint::federation::projection`. Axum
only requests a snapshot or digest. The mint projector scans accepted entries
once, collects deterministic affected-row references, and uses bounded bulk
reads for quote state, completed operations, melt quotes, payment observations,
proof states, and blind signatures.

Mint quote projection has a dedicated minimal durable-state read. It does not
load payment IDs or issuance-history vectors because those local histories are
not fields in the convergence commitment. Quote-state reads are chunked at 500
IDs, preserve requested order, and preserve missing rows. The 1,000-quote
profile therefore falls from about 3,000 SQL reads to five: two bounded quote
state batches and one read each for the other three relevant state collections.

| Operations | Debug | Release | Debug change | Release change |
| ---: | ---: | ---: | ---: | ---: |
| 100 | 3,083 us | 703 us | -39.2% | -36.1% |
| 500 | 14,319 us | 2,343 us | -36.2% | -29.9% |
| 1,000 | 27,366 us | 4,231 us | -38.4% | -28.5% |

A repeat release sample measured 575 us, 2,405 us, and 4,091 us respectively.
All samples retained exactly one materialized row per operation and a non-genesis
digest.

## Rejected candidates

### Reuse the full multi-quote API

Rejected. The existing `get_mint_quotes_by_ids` call still loaded payment and
issuance histories per quote. At 1,000 operations it measured 39,451 us in
debug and 7,179 us in release. It reduced connection churn but retained the
child-table N+1 pattern and regressed the release baseline.

### Batch full payment and issuance histories

Rejected. Loading complete history vectors in two large `IN` queries still did
work outside the digest contract. The 1,000-operation release result regressed
to 10,051 us. The candidate was removed rather than changing a general quote
API for a federation-only projection need.

### Durable incremental row fingerprints

Rejected for now. The bounded full recomputation is linear, takes about 4 ms at
1,000 quote operations in release, and remains the direct audit of durable mint
state. An incremental table would add persistence, restart, repair, and
full-versus-incremental verification machinery without evidence that it is
needed. Reconsider only if process-level measurements show full projection is a
material bottleneck at realistic retained-history sizes.

## Verification

- Materialized-state digest tests cover rejection, keyset rotation, unresolved
  payment observations, quarantine, swap, mint, melt, missing rows, restart,
  and catch-up replay.
- The SQL quote-state regression verifies input ordering, missing rows, paid and
  issued counters, and derived state.
- Common tests pass 327/327 and SQLite tests pass 156/156.
- Federation-enabled Axum passes 418 in the restricted sandbox plus both
  loopback HTTP tests with network permission; one manual benchmark is ignored.
- Standalone Axum passes 28/28. PostgreSQL and mintd federation checks pass.
