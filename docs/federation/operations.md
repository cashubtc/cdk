# Operating a federation member

This is the operator workflow for one member: build, configure, create the
federation, run, monitor, back up and recover. Commands are `cdk-mintd`
subcommands (`crates/cdk-mintd/src/cli.rs`); configuration fields are in
`crates/cdk-mintd/example.config.toml`.

Federation formats are undeployed and follow the cut-through policy: start a
federation from fresh state for each revision. Old federation databases,
configs and wire formats are not migrated.

## Build

| Feature | Adds |
|---|---|
| `federation` | Consensus, BLS DKG, checkpoints, setup ceremony |
| `federation-bdk` | Joint on-chain custody (`onchain_backend = "federated-bdk"`) |
| `federation-bark` | Ark-backed Lightning (`backend = "federated-bark"`) |
| `iroh` | Iroh transport for the mint and the federation API |
| `federation-test-utils` | Test-only: trusted-dealer fixtures, fake-wallet smoke commands. Not for production |

For example: `cargo build -p cdk-mintd --release --no-default-features --features sqlite,federation-bdk,federation-bark,iroh`.

## Configuration

Runtime configuration is stored in the member's database, as for a standalone
mint. Keep the work directory explicit on every command:

```text
cdk-mintd --work-dir <member-dir> config init --new-mint --file <runtime.toml>
cdk-mintd --work-dir <member-dir> config apply --file <runtime.toml>
cdk-mintd --work-dir <member-dir>
```

`config apply` stages a replacement for the next start; `config rollback`
restores the last applied document. The daemon reads no federation environment
variables. `-c/--config` is a global flag that only the `federation setup`
subcommands read, as their protected setup config; every other command rejects
it as an obsolete startup input.

At startup the federation config is read once (`FederationStartup::load`):
either the public and private config files, or the certified setup handoff
while `[federation.dkg]` is enabled. The Bitcoin network, confirmation depth,
fee policy and Ark server come from the accepted applications in that config
and are not restated in `[federation.onchain_wallet]` or `[federation.bark]`;
the sections deny unknown fields, so a document that sets them is rejected.

The `[federation]` section:

| Field | Meaning |
|---|---|
| `enabled` | Run as a federation member |
| `public_config_path`, `private_config_path` | Activated public config and this member's private config |
| `setup_config_path` | Protected setup config, for the certified DKG handoff |
| `restore_manifest_path` | Only when starting a restored database backup |
| `allow_development_payment_observation_policy` | Permit development single-observation finality and sub-BFT thresholds; keep `false` in production |
| `[federation.peer_sync]` | Peer sync interval, timeouts, catch-up page limit, backoff, concurrency and AlephBFT cadence |
| `[federation.dkg]` | Certified DKG auto-start and its tick, retry and rate limits |
| `[federation.onchain_wallet]` | Member-local chain source for the joint on-chain wallet |
| `[federation.bark]` | Member-local Ark endpoint, chain source, storage and secrets |

Secrets (mint signing secrets, Bark operational and receive keys, access
tokens, RPC passwords) use `file:` or `env:` references. Keep secret files
outside the database and in the member's protected backup.

## Creating a federation

The protocol is described in [setup ceremony](setup-ceremony.md). Each command
takes the member's protected setup config with `--config <setup.json>`.

1. Creator: `federation setup create --state-dir <dir> --database <setup.db>
   --members <n> --threshold <t> --network <net> --public-mint-url <url>
   --federation-api-url <url> --coordinator-url <url>` plus keyset, checkpoint,
   FROST and optional Bark policy arguments (`--frost-bark-*`). It writes one
   protected invitation per member; `federation setup invite --member <id>
   --output <file> [--replace]` reissues one.
2. Creator: `federation setup serve --listen <addr>` runs the setup-only
   coordinator.
3. Each member: `federation setup join --invitation-file <file> --state-dir
   <dir> --database <setup.db> --public-mint-url <url> --federation-api-url
   <url>`.
4. Creator: `federation setup propose`.
5. Each member: `federation setup review`, then
   `federation setup approve --proposal-hash <hash>` for exactly the hash shown.
6. Each member: `federation setup status` until it reports the certified
   manifest; it persists the manifest.
7. Each member: `federation setup serve --listen <federation-api-addr>` runs the
   certified BLS and FROST DKG (bounded by `--dkg-max-ticks`,
   `--dkg-retry-delay-ms`, `--dkg-ceremony-timeout-seconds`); `setup status`
   reports when it is finalized.
8. Each member: `federation setup activate` publishes the final public and
   private config under `<state-dir>/activated/`. Point `public_config_path`
   and `private_config_path` at them and start the member.

`--allow-insecure-localhost` permits plain HTTP only when every URL is local.
Copying a public config does not grant membership; protect private configs and
the setup state directory. `federation dkg-status` and `federation inspect`
show redacted DKG records and config material.

For Iroh, run `cdk-mintd --work-dir <dir> iroh init` on each member before setup
and use the printed `iroh://` URL as its federation API URL (and optionally its
public mint URL). Back up the endpoint key: replacing it changes the URL.

## Running

The daemon starts federation services when its listeners are ready. A failed
essential worker stops serving. Shutdown stops scheduling, waits up to 30
seconds for tracked work, then aborts what remains and joins the AlephBFT
session. Workers do not restart an external action on failure; recovery
inspects the durable payment, signing or broadcast identity.

## Monitoring

Operator commands sign requests with the member identity. They take
`--public-config` and `--private-config` together, or neither, in which case
the paths come from the stored config in the work directory.

| Command | Use |
|---|---|
| `federation status` | Signed local status of members. `--require-ready`, `--require-converged`, `--max-operation-lag <n>`, `--require-checkpoint-quorum`, `--require-no-digest-alerts`, `--require-no-payment-observation-alerts`, `--require-no-invalid-operation-alerts` make it a health check |
| `federation compare-digests` | Compare signed order and current-state digests across quiesced members |
| `federation payment-observations` | Redacted payment-observation reports; `--context`, `--quote`, `--include-resolved` |
| `federation audit-checkpoint --member <id>` | Full materialized-state audit of one member against its trusted checkpoint |
| `federation wallet-status` | Joint on-chain wallet health, counts and epochs (no addresses, scripts or transaction contents) |
| `federation transaction-status --proposal-id <id>` | One accepted on-chain transaction |

A digest mismatch is a halt that needs investigation; restarting a worker or
changing display text does not clear it. `sync_state = "reset_required"`
means the member is below every peer's retention floor (see below).

## Joint on-chain wallet

Select `onchain_backend = "federated-bdk"` under `[onchain]` and enable
`[federation.onchain_wallet]` once the public config activates the CDK on-chain
application. Standalone `[bdk]` settings and a mnemonic are rejected in this
mode. The wallet stores its data under `<work-dir>/federation-wallet`.

Each member chooses its own chain source; it never enters consensus:

```toml
[federation.onchain_wallet]
enabled = true
chain_source_type = "bitcoinrpc"   # or "esplora" with esplora_url
chain_source_scope = "independent" # "shared" for common infrastructure
chain_source_id = "member-core"    # opaque label, never an endpoint or token
bitcoind_rpc_host = "127.0.0.1"
bitcoind_rpc_port = 8332
bitcoind_rpc_user = "<secret reference>"
bitcoind_rpc_password = "<secret reference>"
request_timeout_secs = 15
sync_interval_secs = 30
```

Prefer independently operated Bitcoin Core nodes. An Esplora service learns
the scripts a member queries and can omit transactions or serve stale data; a
public one also correlates members. Members on the same provider should use
`shared` scope and the same `chain_source_id`: status then reports a
correlated-source warning. A shared source does not change the observation
threshold. Esplora concurrency is 1 to 16, timeouts 1 to 120 seconds, retries
at most 6; repeated sync failures back off up to five minutes.

`federation rebroadcast --proposal-id <id> --acknowledge-exact-byte-retry`
retries the exact persisted signed bytes of one accepted transaction through
the normal broadcast intent and observation path. It cannot build a new spend,
change an output, release a reservation or bypass signing.

After a reorg, do not edit the chain-observation data, force a UTXO spendable
or run a new DKG. Before liability, the quorum's reconfirmation of the same
block recovers the UTXO; after liability, the alarm stays (see
[custody](custody.md#chain-observation-and-reorgs)).

## Bark

Add a `[[payment_backend]]` entry with `backend = "federated-bark"` and
configure `[federation.bark]`: `enabled`, `server_address` (Ark gRPC), exactly
one of `esplora_url` or `bitcoind_address` (with user and password or cookie
file), `operational_seed_hex`, `receive_secret_key_hex`,
`frost_protocol_storage_root` (must be an existing directory),
`create_if_missing` for a fresh projection, and optionally `data_directory`
(default `<work-dir>/federation-bark`), `receive_secret_database_path`,
`automatic_refresh` (default on) and `automatic_emergency_exit`.

| Command | Use |
|---|---|
| `federation bark-reconcile` | Replay accepted Bark state into a member's local Bark wallet |
| `federation bark-recovery-address --reserve-id <key>` | Allocate an idempotent Bark-owned CPFP fee-reserve address |
| `federation bark-retry --action-id <id> --acknowledge-bark-side-effects` | Retry one accepted Bark action |

## What to back up

| Item | Back up? |
|---|---|
| Mint database (federation and mint state) | Yes, as a checkpoint-bound backup with its manifest |
| Private member config, setup state directory | Yes, protected |
| Secret files (mint, Bark operational and receive keys, RPC credentials) | Yes, protected |
| Bark receive-secret store | Yes, protected |
| Iroh endpoint key (`<work-dir>/iroh/endpoint-secret`) | Yes |
| FROST nonce files (`frost-nonces/`) | **Never** |
| Bark SQLite cache | Optional; it is rebuilt from accepted state |

## FROST nonce files: never restore

FROST signing nonces are redb files, `frost-nonces/root-<epoch>.redb`, kept apart
from the SQL database. The signing runtime places them under
`<work-dir>/federation-wallet/` when the joint on-chain wallet is configured,
and otherwise under `<bark data_directory>/frost-signing/`.

Exclude them from backups and never restore them. A restored nonce store offers
consumed nonces again, and a nonce used for two different messages reveals the
member's key share. If a nonce file is lost or suspect, stop the member and
delete it; the member generates fresh nonces, and a signing session that
referenced a deleted nonce is started again. Restoring the SQL database from a
backup does not affect this rule, and `reset-member` never touches the nonce
files.

## Backup manifest and restore

A signed checkpoint commits to state; it is not a backup. A database backup is
bound to its checkpoint by a manifest:

1. Stop the member cleanly and keep it stopped while the backup and manifest
   are taken, so the frontier cannot move.
2. Point the work directory at the quiesced copy (SQLite) or expose the
   snapshot through the configured connection (PostgreSQL), and run
   `cdk-mintd --work-dir <dir> federation backup-manifest --output <manifest.json>`
   (with `--public-config` and `--private-config` if they are not in the stored
   config).
3. Store the backup, manifest and member secrets together as one generation.

The manifest contains no secrets or paths. It binds the federation ID, public
config digest, member ID, trusted checkpoint, exact durable frontier, active
AlephBFT backup, schema contract and a commitment to the member's secret
config.

To restore, put back the database and matching secrets while stopped, set
`restore_manifest_path`, and start. Before DKG reconciliation, mint
construction or routes, startup checks the manifest against the database and
secrets; any mismatch fails startup, with no bypass. Then run
`federation audit-checkpoint --member <id>` and return the member to service
only after the audit matches. A new database state needs a new manifest.

## Empty member

A member started with an empty database but its existing identity, key shares
and (with on-chain custody) FROST root material catches up from its peers:
by replay if a peer still holds the history, or by installing a peer's latest
checkpoint state when every peer has pruned it. Checkpoints and state transfer
never recreate lost secret shares; never substitute another member's shares.

## Member below every peer's retention floor

A member that holds state but was offline past every peer's retention floor
halts with `sync_state = "reset_required"` and reports its local journal index.
To return it to service:

1. Stop the daemon. The command cannot detect a running daemon.
2. Run `cdk-mintd --work-dir <member-dir> federation reset-member --confirm-wipe`.
   Without `--confirm-wipe` it refuses. It deletes the member's federation
   consensus state and mint business rows (quotes, proofs, blind signatures,
   signature shares, sagas) in one transaction and keeps the federation config,
   DKG and setup records, FROST key material, keysets and mint configuration.
   It does not touch the FROST nonce files.
3. Start the member. It installs a peer's latest scheduled checkpoint state and
   catches up.

Nothing is deleted without this command. Restarting without it halts the
member again at its first sync round. Details are in
[storage and retention](storage-and-retention.md#stale-member-halt-and-reset).

## Audit mismatch

A materialized-state audit mismatch is a durable one-way latch. Restart,
checkpoint promotion, peer agreement and background tasks never clear it, and
there is no clear command.

1. Stop the member and preserve the failed database for investigation.
2. Find the cause without modifying the latched database.
3. Restore a known-good checkpoint-bound backup with its manifest and secrets,
   or start from an empty database with the member's existing secret material.
4. Run `federation audit-checkpoint`.
5. Return the member to service only when the audit matches and status reports
   no latch.
