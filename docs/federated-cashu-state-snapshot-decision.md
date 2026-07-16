# Federated Cashu peer state snapshot decision

Status: deferred outside checkpoint version 1

## Decision

Do not add peer-to-peer materialized-state snapshots to the current checkpoint
implementation. Continue to use two recovery paths:

- checkpoint-based restart from an intact or checkpoint-bound restored database,
  with work proportional to the post-checkpoint suffix; and
- bounded page-streamed replay from genesis for an empty database.

A quorum-signed checkpoint authenticates cumulative order, application, and
materialized-state commitments, but it does not authenticate a physical database
image by itself. Treating a peer-provided database dump as if it were a
checkpoint would create a second state-transfer and validation protocol, expand
the trusted surface, and risk making checkpoint data an alternate ordering path.

## When to revisit

Revisit peer snapshots only if measured disaster-recovery objectives cannot be
met by checkpoint-bound operator backups plus bounded genesis replay. A future
proposal must independently specify:

- a canonical, complete materialized-state snapshot format;
- quorum authentication and configuration/schema binding;
- proof that the snapshot plus ordered suffix reconstructs exactly the signed
  checkpoint frontier;
- secret and member-local state exclusions;
- bounded transfer, staging, crash recovery, and denial-of-service limits; and
- retention and compatibility rules.

That proposal requires a separate security/protocol review and an explicit
scope amendment. No snapshot wire type, route, database table, or compatibility
placeholder is reserved in version 1.
