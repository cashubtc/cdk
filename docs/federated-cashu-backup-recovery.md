# Federated Cashu backup and recovery

This guide defines the version-1 operator contract for checkpoint startup,
database backup restore, empty-database bootstrap, retention, and explicit
materialized-state audits. A signed checkpoint is a commitment to state; it is
not a database snapshot.

## Recovery choices

Use an ordinary restart when the member database is intact. The mint verifies
its bounded trusted checkpoint quorum, initializes consensus at that checkpoint,
and validates only the retained suffix and active AlephBFT session backup. An
exact-checkpoint restart reads no journal or finalized-item prefix.

Use a checkpoint-bound database backup for disaster recovery. The database
backup must be paired with:

- the exact public federation config;
- the same member identity and BLS private config generation;
- the FROST private-root/sealed-storage generation already contained in or
  restored with that database backup; and
- the generated backup manifest.

An empty database cannot start from the materialized state named by a
checkpoint. It must obtain and replay ordered journal and finalized-item bodies
from genesis in bounded pages. It also needs the member's existing identity,
BLS shares, and, when the joint on-chain wallet is enabled, its valid FROST
private-root material. Checkpoints authenticate replayed boundaries but do not
reconstruct missing secret shares. Do not replace lost member secrets with a
different member's shares.

## Creating a backup manifest

Stop the mint cleanly before taking the database backup. Keep it stopped while
the database snapshot and manifest are created so the recorded durable frontier
and active backup metadata cannot move independently.

For SQLite, point `--work-dir` and `--config` at the quiesced database copy. For
PostgreSQL, restore or expose the quiesced snapshot through the configured
database connection. Then run:

```text
cdk-mintd --work-dir <member-work-dir> --config <config.toml> \
  federation backup-manifest \
  --public-config <federation-public.json> \
  --private-config <federation-private.json> \
  --output <database-backup-manifest.json>
```

The manifest is written as a private file. It contains no secret keys or local
paths. It binds the backup to the federation ID, complete public-config digest,
member ID, trusted checkpoint ID, exact durable frontier, active AlephBFT
session backup size, federation database schema contract, and a domain-separated
commitment to the member's external secret config.

Store the database backup, manifest, and member secrets under the same backup
generation. Protect all of them as sensitive operator material.

## Restoring a database backup

Restore the database and matching secret/config files while the mint is stopped.
Set the following only for the restored startup:

```toml
[federation]
restore_manifest_path = "/path/to/database-backup-manifest.json"
```

Before DKG reconciliation, mint construction, or route creation, startup
validates the manifest against the restored database and supplied member
secrets. A wrong federation, config digest, checkpoint, durable frontier,
schema contract, active backup, member ID, or secret generation fails startup.
There is no bypass flag. The setting may remain enabled for later restarts of
the same unchanged backup generation, but a new database state requires a new
quiesced backup and manifest.

After startup, run the explicit operator checkpoint audit and require a matched
result before returning the restored member to service:

```text
cdk-mintd federation audit-checkpoint \
  --public-config <federation-public.json> \
  --private-config <federation-private.json> \
  --member <member-id>
```

## Retention contract

Version 1 advertises genesis as the oldest bootstrap recovery floor:

- accepted journal bodies are retained from index 0;
- finalized AlephBFT item bodies are retained from item index 0;
- all scheduled checkpoint quorums are retained as reusable historical proofs;
- a covered recovery checkpoint may be pruned after a later scheduled checkpoint
  makes it redundant, unless it remains trusted or audited; and
- AlephBFT backups older than the active session may be pruned, but the active
  session backup is protected even if a newer pruning boundary is requested.

There is no journal/finalized-body pruning API in version 1. A future nonzero
recovery floor must be introduced as an explicit protocol and operator contract;
it must not silently make empty-database genesis replay impossible.

Signed local status exposes the trusted checkpoint, genesis recovery floor,
current in-memory suffix item counts and canonical bytes, active-session backup
chunk/byte totals, and audit latch. It never includes backup contents, secrets,
credentials, or filesystem paths.

## Audit mismatch recovery

A materialized-state audit mismatch is a durable one-way latch. It is never
cleared by restart, checkpoint promotion, peer agreement, or an automatic
background task. There is intentionally no operator clear command.

When a mismatch occurs:

1. Stop the member and preserve the failed database for investigation.
2. Identify the cause without modifying or clearing the latch in place.
3. Restore a known-good checkpoint-bound database backup with its exact manifest
   and member secrets, or create a fresh database and perform bounded genesis
   replay with the valid pre-existing member secret material.
4. Run the explicit operator checkpoint audit.
5. Return the member to service only after the audit matches and signed status
   reports no mismatch latch.

This recovery replaces the failed database; it does not mutate the latched copy
back into service.
