# Cross-process mint notifications via a pluggable bus

* Status: accepted
* Authors: Cesar Rodas
* Date: 2026-07-24
* Targeted modules: cdk-common (pub_sub), cdk-postgres, cdk (mint)
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
any publish call site or the WebSocket layer?

## Decision Drivers

* Quote and proof state is already consistent across instances through the
  shared database and `SELECT ... FOR UPDATE` row locks. The gap is only the
  real-time push, so the fix should target the fan-out, not storage.
* The ~15 `PubSubManager` publish helpers, the mint spec, and the WS handlers
  should not change. Distribution is a cross-cutting concern, not a per-event
  one.
* Local delivery must keep working even when the cross-process transport is
  down. A distributed backend is an addition, never a dependency for the
  single-instance case.
* Reuse what exists: `Spec::Event` and `Spec::Topic` already require
  `Serialize + DeserializeOwned`, and `cdk-postgres` already speaks
  `tokio-postgres` with TLS handling.
* Not every deployment wants an extra moving part. The default must stay
  in-process with no new dependency.

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

#### Postgres `LISTEN`/`NOTIFY` bus behind a trait

Introduce a `Bus` trait between publish and local fan-out. The default keeps
everything in-process; a Postgres-backed implementation forwards events to peers
over `LISTEN`/`NOTIFY`.

**Pros:**

* Good, because multi-instance mints already share a Postgres database, so the
  transport needs no new infrastructure.
* Good, because the trait keeps the in-process default unchanged and confines
  distribution to one seam.
* Good, because a Redis or message-queue backend can be added later as another
  `Bus` implementation without touching the seam.

**Cons:**

* Bad, because `NOTIFY` caps payloads at 8000 bytes, so unusually large events
  cannot be forwarded verbatim.
* Bad, because it costs a dedicated Postgres connection per instance and a
  reconnect loop.

## Decision Outcome

Chosen option: "Postgres `LISTEN`/`NOTIFY` bus behind a trait", because it
distributes events without new infrastructure for a Postgres-backed mint, keeps
the single-instance path dependency-free, and leaves room for other backends.

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

### The Postgres implementation

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

Each instance has a per-process `origin` id (a v4 UUID). Publishing delivers
locally at once and forwards to peers; the origin check drops the copy Postgres
echoes back, so a locally-published event is delivered exactly once on its own
instance and once on every peer. `LocalDelivery` feeds both local and peer
events through one path, so remote events are indistinguishable from local ones
downstream.

Wire format is JSON, because cashu types do not round-trip through CBOR. The
`channel` name is validated as a Postgres identifier before it is interpolated
into `LISTEN` (which cannot be parameterized); `NOTIFY` itself uses bound
parameters. Events larger than the 8000-byte `NOTIFY` limit are delivered
locally and skipped for peers with a warning; mint events (quote responses,
proof states) are well under it, so this only concerns unusually large melts.

### Mint integration

```rust
let connector = PostgresBusConnector::connect(pg_config, "cdk_mint_pubsub").await?;
let manager = PubSubManager::new_with_bus(ctx, move |local| connector.build(local));
```

A mint left on the default `PubSubManager::new` behaves exactly as before.
`cdk-mintd` installs the bus automatically when the database engine is
Postgres; the channel is configurable via `pubsub_channel` /
`CDK_MINTD_POSTGRES_PUBSUB_CHANNEL`.

### Positive Consequences

* A multi-instance mint sharing Postgres gets real-time notifications on every
  instance with no new infrastructure.
* The single-instance path is unchanged and dependency-free; the bus is opt-in.
* Publish call sites, the mint spec, and the WS handlers are untouched.
* Another backend (Redis, a message queue) is a new `Bus` implementation, not a
  change to the seam.

### Negative Consequences

* `NOTIFY`'s 8000-byte payload cap means very large events are not forwarded to
  peers; those subscribers fall back to the existing on-read/reconnect backfill.
* Each instance holds a dedicated Postgres connection and a reconnect loop, plus
  one inbound dispatcher task.
* A brief reconnect window can drop peer events; subscribers recover current
  state on their next `fetch_events` backfill, so state is not lost, only the
  live push during the gap. The same holds if the bounded inbound queue
  overflows under a flood: excess notifications are dropped, not buffered.
* Inbound events are delivered to subscribers without re-validating against the
  database. This is sound because publishing to the channel requires a
  connection to the mint's own database, so a publisher is already inside the
  trust boundary (it could write mint state directly). It is not a new attack
  surface, but it is an assumption: all instances on a channel must be the same
  trust domain.

`cdk-mintd` installs the bus automatically when the database engine is Postgres
and keeps the in-process default for every other backend.

## Links

* Builds on the pub/sub primitives in `crates/cdk-common/src/pub_sub/`
* Reuses the reconnect pattern also used by the signatory client in
  [ADR-0002](0002-signatory-keyset-subscription.md)
