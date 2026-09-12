# Multiple signatory instances sharing one database

* Status: accepted
* Authors: Cesar Rodas
* Date: 2026-07-28
* Targeted modules: cdk-signatory (DbSignatory), cdk-common
  (KeysDatabaseTransaction), cdk-sql-common

## Context and Problem Statement

ADR-0003 made the keys database persistence-only and, as a direct consequence,
assumed a single writer: "correctness depends on this process being the only
writer. An out-of-band database mutation is not observed until the next boot."
That rules out running more than one signatory (or embedded mint) against the
same database for high availability or horizontal scale.

Two concrete things break with multiple instances:

1. A rotation performed by one instance is invisible to the others until they
   restart. They keep signing with the old active keyset and reject the new
   one as unknown.
2. `rotate_keyset` computed the next derivation index from process-local
   memory. The in-process `rotation_lock` only serializes rotations within one
   process, so two instances could read the same index and derive divergent
   keysets for the same slot.

The enabling fact is that keys are deterministic from the seed. Every instance
shares the same seed, so two instances given the same `MintKeySetInfo` rows
derive byte-identical keys. Only metadata (which keysets exist, which is
active, the next index) has to be shared, never secret material.

## Decision Drivers

* Keep the hot paths (`blind_sign`, `verify_proofs`, `keysets`) memory-only.
  The shared database must not land on the signing path (ADR-0003).
* Rotation must allocate a unique derivation index even when two instances
  rotate the same unit at the same time.
* Propagating a peer's rotation should need no inter-node messaging
  infrastructure to operate or deploy.

## Considered Options

#### Event-driven notification (Postgres LISTEN/NOTIFY, poll elsewhere)

Rotation emits a notification; peers subscribe and reload on the event.

* Good, because propagation is near-instant and idle cost is zero.
* Bad, because it needs a per-backend pub/sub seam: a dedicated LISTEN
  connection and a `NOTIFY` on commit for Postgres, a polling fallback for
  SQLite, and a subscription method threaded through the database trait. More
  surface than the problem needs.

#### Epoch-aligned periodic reload (chosen)

Every instance reloads keysets from the database on a shared wall-clock epoch
(`unix_time` floored to a fixed interval). Rotation still writes through the
database; peers pick the change up on the next reload.

* Good, because it needs no notification channel, no extra connection, and no
  backend-specific pub/sub. One periodic task per instance.
* Good, because aligning to a shared epoch means the fleet reloads at about the
  same moment, so their views converge within one interval rather than drifting
  by arbitrary offsets.
* Bad, because propagation is bounded by the interval, not instant. For keyset
  rotation, which is rare and not latency-sensitive, this is acceptable.

## Decision Outcome

Chosen option: "epoch-aligned periodic reload", because it closes the
staleness gap with the least machinery and rotation is not latency-sensitive.

Two changes, one for each break above.

### One global keyset advisory lock

Every keyset transaction (rotation, reload, boot reactivation) takes a single
global advisory lock when it begins, held to commit, so all keyset reads and
writes serialize across processes. The lock is an internal helper on
`SQLTransaction`, `lock_keysets`, run from the keys `begin_transaction`: Postgres
runs at `START TRANSACTION` isolation, which does not serialize concurrent reads,
so it takes a `pg_advisory_xact_lock(hashtext('cdk:keysets'))`; SQLite serializes
writers with `BEGIN IMMEDIATE`, so it is a no-op there. Dispatch is by driver
name, the same way migrations are.

An earlier revision keyed the lock per unit to allow concurrent rotations of
different units. One global lock is simpler and closes a gap the per-unit lock
left open: two rotations of *different* units whose currency strings hash to the
same derivation slot could both pass the cross-unit collision check
concurrently. Under the global lock no two keyset transactions overlap, so that
race cannot happen, and rotations (which are rare and not latency-sensitive)
simply serialize.

The next derivation index is allocated inside the rotation transaction through
`KeysDatabaseTransaction::next_derivation_index`, not from memory. It returns
`COALESCE(MAX(derivation_path_index), 0) + 1` for the unit, so a unit with no
keysets yet starts at `1`; the global lock makes that read authoritative.
`get_keyset_infos_by_unit` relies on the same lock, so a concurrent rotation
cannot commit between that read and the caller's active-pointer reassignment.
`rotate_keyset` opens the transaction first, allocates the index, derives the
keyset, then writes and commits. The in-process `rotation_lock` stays as a cheap
local optimization but is no longer the correctness boundary; the database is.

### Periodic reload

The reload is opt-in. `DbSignatory::spawn_keyset_refresh` takes the interval;
`None` (the default) or zero disables it and spawns no task. A single process
owns its database and never polls; enabling the reload is what lets several
processes run against one shared database. Both construction sites, the
embedded mint
(`build_with_seed`, via `MintBuilder::with_keyset_refresh_interval`) and the
standalone gRPC server (via `--keyset-refresh-interval-ms`), pass the configured
interval; both default to off. When enabled, the task sleeps until the next epoch
boundary (Unix time in milliseconds floored to the interval), then runs
`load_keys_from_db`.

The in-memory keysets live in an `ArcSwap<KeysetSnapshot>`: the hot read paths
(`blind_sign`, `verify_proofs`, `keysets`) load it lock-free, and the reload
builds a fresh snapshot off to the side and swaps it in atomically, so a reload
never blocks signing. The snapshot carries the keyset epoch it was built from,
so no separate lock guards it: the swap is a compare-and-swap loop that gives up
when the pointer already holds an equal-or-newer epoch, so two reloads racing (a
refresh tick against a rotation's post-commit reload) settle on the newer
database view regardless of arrival order. Because the reload loads
authoritative database state and swaps wholesale, it needs no `rotation_lock`:
it can neither block signing nor interleave destructively with a local rotation.

`load_keys_from_db` is cheap in the steady state. It first asks the storage for
a `u64` keyset epoch, `KeysDatabase::keysets_epoch`, with a single lock-free
autocommit read (one statement is consistent on its own), and returns early when
it matches the loaded snapshot. The value is whatever the backend can compute
cheaply; the SQL backend keeps a single-row `keyset_epoch` table (one `epoch`
column), bumped inside every keyset-writing transaction (`add_keyset_info` and
`set_active_keyset`), so it moves on any change, an insert or a pure
active-pointer reassignment. Only when the epoch changed does it open a keyset
transaction (which takes the global lock) and rebuild through it, and even then
it reuses the keys already derived in the current snapshot (a memory copy, no
crypto), deriving only the new keysets. Because the transaction holds the global
lock, no other keyset transaction can commit between the active-pointer and
keyset-infos reads, so they are a consistent snapshot with no epoch bracketing.
On a real change it re-publishes the snapshot to the watch channel. The mint side
is unchanged; it drains the signatory's watch channel into its keyset cache, so a
peer's rotation reaches it once the reload republishes.

A local `rotate_keyset` refreshes memory through this same `load_keys_from_db`
after committing, rather than patching the maps by hand. That picks up any
concurrent peer rotation and records the loaded epoch, so the next refresh tick
sees no change and does not reload. Recording the epoch from a bare
post-commit read would be unsafe: it could already reflect a peer change this
instance has not loaded, and the instance would then skip it.

### Auto-rotation across instances

Keyset auto-rotation (`spawn_auto_rotation`, off when the interval is zero) adds
a third writer to the picture: every instance sweeps its own keysets and rotates
the ones older than the interval.

The sweep needs no coordination of its own because it runs entirely inside one
rotation transaction. `begin_rotation` opens that transaction, which takes the
global keyset lock, and reloads the in-memory keysets through it; only then does
the sweep read its due set. A unit a peer already rotated therefore carries that
peer's fresh `valid_from` and is not due, and the lock is held until commit, so
no peer can rotate in between. Age alone decides, with no identity check and no
lost-race error to distinguish from a real failure.

Because the loop polls faster than the interval (age has to drive rotation, not
process uptime), nearly every tick has nothing to do, and taking the global lock
that often would tax every instance in the fleet. So a tick first checks the
in-memory snapshot lock-free and returns when nothing looks due. That cannot
skip a due keyset: the snapshot only ever lags the database, and a lagging
snapshot carries an older `valid_from`, so it over-reports due-ness and never
under-reports. A false positive costs one transaction that the authoritative
under-lock read then discards.

Reading the due set outside the transaction was what made an identity check
necessary, and it is also what forced a transaction per unit. Staging every due
unit into the one transaction makes the sweep all-or-nothing: if any unit fails,
none of them rotate and the tick is retried later. That is not a preference,
Postgres aborts a transaction on the first failed statement, so continuing past
a failure is not available once the units share a transaction. The realistic
failure is a database error, which would have failed every unit anyway.

`rotate_keyset` runs through the same two pieces with a single unit, so a
mint-initiated rotation and a sweep share the loading and the commit.

### Positive Consequences

* Multiple signatory instances can run active/active against one database.
* The signing paths still never touch the database. Rotation coordinates and
  reloads through the database, but remains outside the signing hot path.
* No notification channel, extra connection, or backend pub/sub to operate.

### Negative Consequences

* A peer's rotation is visible only after the next reload (bounded by the
  interval), not immediately.
* Multi-process sharing is opt-in: it works only when the reload is enabled. Run
  more than one process against a shared database without setting an interval and
  a peer's rotation stays invisible until the next boot, the pre-ADR behavior.
* When the reload is enabled, each process runs one cheap epoch-token query
  against the database each interval. When the token is unchanged that is the
  whole cost: no row fetch, no key derivation, no swap. Only when it moves does
  the process fetch rows and rebuild, reusing the already-derived keys, deriving
  only the new keysets, and swapping the snapshot in atomically. A lone process
  leaves the reload off and never polls.

## Links

* Supersedes the single-writer assumption in
  [ADR-0003](0003-signatory-database-persistence-only.md); the persistence-only
  model and strict boot are unchanged.
* Builds on [ADR-0002](0002-signatory-keyset-subscription.md): the watch
  publish contract is preserved, now gated on an actual change.
