# Cross-process mint notifications via a pluggable bus

* Status: accepted
* Authors: Cesar Rodas
* Date: 2026-07-31
* Targeted modules: cdk-common (pub_sub), cdk-postgres, cdk-sql-common (bus), cdk-mintd (config), cdk (mint)
* Associated tickets/PRs: n/a

## Context and Problem Statement

The mint's NUT-17 notifications are in-process. Subscribers live in an
in-memory index inside a single `Pubsub` instance and are fed over tokio `mpsc`
channels (`crates/cdk-common/src/pub_sub/pubsub.rs`). When several mint
instances run behind a load balancer sharing one database, the instance whose
Lightning stream sees an invoice paid updates the shared quote row and publishes
the event, but that live event only reaches WebSocket subscribers connected to
that same instance. A wallet whose socket is pinned to another instance sees the
change only on reconnect, through the database-backed `fetch_events` backfill.

How does a published event reach subscribers on every instance without changing
any publish call site or the WebSocket layer, across deployments that differ in
database engine and connection topology (direct Postgres, Postgres behind a
transaction-pooling proxy such as PgBouncer, or SQLite)?

## Decision Drivers

* Quote and proof state is already consistent across instances through the
  shared database and `SELECT ... FOR UPDATE` row locks. The gap is only the
  real-time push, so the fix should target the fan-out, not storage.
* Correctness must not depend on the transport. The mint reconciles state from
  the database on every read (`fetch_events`), so a transport is a latency
  optimization for the live push, never the source of truth.
* The ~15 `PubSubManager` publish helpers, the mint spec, and the WS handlers
  should not change. Distribution is a cross-cutting concern, not a per-event
  one.
* Local delivery must keep working even when the cross-process transport is
  down. A distributed backend is an addition, never a dependency for the
  single-instance case, which must stay the zero-configuration default.
* Reuse what exists: `Spec::Event` and `Spec::Topic` already require
  `Serialize + DeserializeOwned`; `cdk-postgres` already speaks `tokio-postgres`
  with TLS handling; the SQL layer already runs plain queries against a shared
  pool.
* A transport must exist that works on SQLite and through connection poolers,
  not only on directly-connected Postgres.

## Considered Options

#### Sticky sessions at the load balancer

Pin each WebSocket to one backend and rely on database reconciliation for
correctness.

**Pros:**

* Good, because it needs no code change.

**Cons:**

* Bad, because a cross-instance flow (paid on A, subscribed on B) still misses
  the live push; it only papers over the symptom for the connection's lifetime.
* Bad, because it constrains the deployment and breaks on rebalancing.

#### Redis pub/sub bus

Fan events out through a Redis channel.

**Pros:**

* Good, because Redis pub/sub is purpose-built for fan-out.

**Cons:**

* Bad, because it adds an operational dependency most mints do not otherwise
  run. Redis appears in the tree today only as an optional HTTP cache.
* Bad, because a mint that already runs Postgres would take on a second
  datastore purely for notifications.

#### A single wire transport (Postgres `LISTEN`/`NOTIFY` only)

Ship one distributed backend behind a trait and tell pooled deployments to route
its connection directly to Postgres.

**Pros:**

* Good, because it needs the least code and keeps the lowest latency.

**Cons:**

* Bad, because `LISTEN` needs a session-pinned connection, so it fails behind a
  transaction-pooling proxy unless the operator carves out a direct connection.
* Bad, because it does nothing for SQLite, which then has no cross-process
  transport at all.

#### A `Bus` trait seam with selectable transports

Introduce a `Bus` trait between publish and local fan-out. The default keeps
everything in-process; wire implementations forward events to peers. Ship both a
Postgres `LISTEN`/`NOTIFY` bus and a portable SQL polling bus, and let the mint
pick per deployment.

**Pros:**

* Good, because the trait keeps the in-process default unchanged and confines
  distribution to one seam.
* Good, because `LISTEN`/`NOTIFY` serves a directly-connected Postgres with no
  new infrastructure, while the SQL polling bus serves SQLite and pooled Postgres
  using only plain `INSERT`/`SELECT`.
* Good, because a Redis or message-queue backend can be added later as another
  `Bus` implementation without touching the seam.

**Cons:**

* Bad, because `NOTIFY` caps payloads at 8000 bytes, so the `LISTEN`/`NOTIFY`
  transport cannot forward unusually large events verbatim.
* Bad, because polling latency is bounded by the interval, and the outbox adds a
  table, a write per event, and a periodic prune.

## Decision Outcome

Chosen option: "A `Bus` trait seam with selectable transports". It distributes
events without new infrastructure, keeps the single-instance path
dependency-free, covers every supported database and connection topology, and
leaves room for other backends.

### The seam

`Pubsub::publish` previously fanned out directly to the in-memory subscriber
index. That is split into local fan-out (unchanged) and distribution (behind a
trait), in `crates/cdk-common/src/pub_sub/bus.rs`:

```rust
/// Distributes a published event. LocalBus keeps it in-process; a wire bus
/// also forwards to peers and injects peer events through the same handle.
pub trait Bus<S: Spec>: Send + Sync {
    fn publish(&self, event: S::Event);
}

/// Handle a Bus uses to deliver into this process's subscribers.
pub struct LocalDelivery<S: Spec> { /* wraps the subscriber index */ }
impl<S: Spec> LocalDelivery<S> {
    pub fn deliver(&self, event: S::Event); // the existing fan-out
}
```

`publish` stays sync fire-and-forget (matching today's contract), so no publish
call site changes. `Pubsub::new` keeps the in-process `LocalBus`; a distributed
bus is opted into through `Pubsub::new_with_bus` and, for the mint,
`PubSubManager::new_with_bus`. The builder is a closure
`FnOnce(LocalDelivery<S>) -> Arc<dyn Bus<S>>` because a bus needs the delivery
handle, which only exists once the subscriber index is created, and a wire bus
spawns its inbound listener at that point.

`LocalDelivery` feeds both local and peer events through one path, so remote
events are indistinguishable from local ones downstream. Each instance carries a
per-process `origin` id (a v4 UUID) so a bus can drop the copy of its own event
that a transport hands back.

### The Postgres `LISTEN`/`NOTIFY` transport

`PostgresBus<S>` lives in `crates/cdk-postgres/src/bus.rs`, where the
`tokio-postgres` client and TLS handling already exist. It is generic over any
`Spec`, not tied to the mint, and reuses the crate's `PgConfig`/`SslMode`.

Construction is two steps, keeping `new_with_bus`'s sync closure intact:

* `PostgresBusConnector::connect(config, channel).await` opens a dedicated
  connection, issues `LISTEN`, and returns only after the first connect and
  `LISTEN` succeed, so an unreachable database fails here rather than silently
  later. A background driver polls the connection with `poll_message`, forwards
  `AsyncMessage::Notification` payloads to an inbound channel, and reconnects
  with capped exponential backoff when the connection drops. It stops once every
  bus handle is gone.
* `connector.build(local)` spawns the inbound dispatcher and returns
  `Arc<dyn Bus<S>>`.

Delivery model:

```text
publish(event):
  serialize { origin, event } as JSON
  local.deliver(event)                     // immediate, never blocked on Postgres
  spawn: SELECT pg_notify(channel, payload)

inbound payload:
  parse { origin, event }
  if origin == self: skip                  // Postgres echoes our own NOTIFY back
  else: local.deliver(event)
```

Publishing delivers locally at once and forwards to peers; the origin check
drops the copy Postgres echoes back, so a locally-published event is delivered
exactly once on its own instance and once on every peer. Wire format is JSON,
because cashu types do not round-trip through CBOR. The `channel` name is
validated as a Postgres identifier before it is interpolated into `LISTEN`
(which cannot be parameterized); `NOTIFY` itself uses bound parameters. Events
larger than the 8000-byte `NOTIFY` limit are delivered locally and skipped for
peers with a warning; mint events (quote responses, proof states) are well under
it, so this only concerns unusually large melts.

### The SQL polling transport

`SqlBus` lives in `crates/cdk-sql-common/src/mint/bus.rs`. It is the portable
alternative: it uses only plain `INSERT`/`SELECT`, so it works on any backend
the SQL layer supports (SQLite, Postgres) and, because it never holds a
session-pinned connection, it also works when Postgres is reached through a
transaction-pooling proxy such as PgBouncer, where `LISTEN` cannot. It reuses the
mint's existing connection pool through a new `SQLMintDatabase::pool()` accessor,
so it opens no connection of its own.

Delivery model:

* On publish the event is delivered to local subscribers immediately and, in the
  background, appended as a row to `pubsub_outbox (id, origin, payload,
  created_time)`. `payload` is the JSON-serialized event; `origin` is the
  per-instance id.
* Each instance polls `WHERE id > :cursor AND origin <> :origin ORDER BY id`,
  delivers each row through the same local fan-out, and advances its cursor. The
  `origin` filter drops an instance's own rows, so there is no self-echo to
  detect after the fact.
* The starting cursor is the current `MAX(id)`, so history is not replayed; an
  instance only sees events published after it came up, matching
  `LISTEN`/`NOTIFY`.
* Rows older than a retention window are pruned on a coarse cadence. Pruning is
  idempotent, so every instance can run it.

Relative to `LISTEN`/`NOTIFY`, the SQL bus trades near-instant latency for a
poll interval, but removes the 8000-byte payload cap (the event lives in a row)
and resumes from its cursor after a missed poll or restart instead of dropping
the event, as long as the row is still within the retention window.

### Mint integration and transport selection

```rust
let connector = PostgresBusConnector::connect(pg_config, "cdk_mint_pubsub").await?;
let manager = PubSubManager::new_with_bus(ctx, move |local| connector.build(local));
```

A mint left on the default `PubSubManager::new` behaves exactly as before.
`cdk-mintd` selects a transport through `[database.pubsub].transport` (or
`CDK_MINTD_PUBSUB_TRANSPORT`):

* `in-memory` (default): `LocalBus`. Correct for a single instance.
* `sql`: `SqlBus`. Works on SQLite and Postgres, and through PgBouncer. Tuned
  with `poll_interval_ms` / `retention_seconds`.
* `postgres-listen-notify`: `PostgresBus`. Lowest latency, Postgres only, needs a
  session-pinned connection. The channel is set by `channel` /
  `CDK_MINTD_PUBSUB_CHANNEL`.

Choosing `postgres-listen-notify` on a non-Postgres engine is a startup error.
For the `sql` transport `cdk-mintd` reuses the mint database's pool; for
`postgres-listen-notify` it opens the dedicated `LISTEN` connection described
above.

When mintd runs from a database-backed configuration document, the bus is
created while opening that database, before the document can be read. The
transport therefore comes from the same bootstrap environment that selects the
database (`CDK_MINTD_PUBSUB_*`), like `CDK_MINTD_DATABASE` and the Postgres URL.
A `[database.pubsub]` block in the stored document is reported at startup and
the bootstrap value is used.

### Positive Consequences

* A multi-instance mint gets real-time notifications on every instance with no
  new infrastructure: `LISTEN`/`NOTIFY` for directly-connected Postgres, SQL
  polling for SQLite and pooled Postgres.
* The single-instance path is unchanged and dependency-free; any bus is opt-in.
* Publish call sites, the mint spec, and the WS handlers are untouched.
* Another backend (Redis, a message queue) is a new `Bus` implementation, not a
  change to the seam.
* The operator states intent once (`transport = "..."`) instead of reasoning
  about connection routing.

### Negative Consequences

* `NOTIFY`'s 8000-byte payload cap means the `LISTEN`/`NOTIFY` transport does not
  forward very large events to peers; those subscribers fall back to the existing
  on-read/reconnect backfill. The SQL transport has no such cap.
* The `LISTEN`/`NOTIFY` transport holds a dedicated Postgres connection and a
  reconnect loop per instance, plus one inbound dispatcher task. The SQL
  transport adds an outbox table, a write per event, and a periodic prune, and
  its latency is bounded by the poll interval.
* A brief `LISTEN`/`NOTIFY` reconnect window, a bounded-inbound-queue overflow,
  or an SQL cursor that skips a row committed out of id order can drop the live
  push. In every case subscribers recover current state on their next
  `fetch_events` backfill, so state is not lost, only the live push during the
  gap.
* Inbound events are delivered to subscribers without re-validating against the
  database. This is sound because publishing requires a connection to the mint's
  own database, so a publisher is already inside the trust boundary (it could
  write mint state directly). It is not a new attack surface, but it is an
  assumption: all instances on a channel or outbox must be the same trust domain.

## Links

* Builds on the pub/sub primitives in `crates/cdk-common/src/pub_sub/`
* `PostgresBus` lives in `crates/cdk-postgres/src/bus.rs`; `SqlBus` lives in
  `crates/cdk-sql-common/src/mint/bus.rs`
* Reuses the reconnect pattern also used by the signatory client in
  [ADR-0002](0002-signatory-keyset-subscription.md)
