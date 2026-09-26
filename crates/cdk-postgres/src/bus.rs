//! Cross-process pub/sub bus backed by PostgreSQL `LISTEN`/`NOTIFY`.
//!
//! The mint's NUT-17 notifications are in-process by default: an event
//! published on one instance only reaches subscribers connected to that same
//! instance. [`PostgresBus`] implements [`cdk_common::pub_sub::Bus`] so several
//! mint instances sharing a Postgres database also share notifications.
//!
//! The bus holds two connections of its own, outside the database pool. One
//! stays in `LISTEN` and one is used for nothing but `pg_notify`, so forwarding
//! never contends with reading.
//!
//! Outbound work is one funnel. A publish clones the event into a bounded
//! queue and hands the original to the local fan-out on its own task, as the
//! in-process bus does, so no request handler waits on either. A single task
//! drains that queue, serializes each event and sends it with `pg_notify`
//! sequentially on the publishing connection. Because the enqueue happens in
//! the calling thread, peers receive one instance's events in the order that
//! instance published them: NUT-17 payloads are full-state replacements, so a
//! reordered pair would read as a state regression to a subscriber on another
//! instance.
//!
//! That order holds per origin, not globally. Two instances publishing about
//! the same quote race, and a peer's copy is always the slower one, so a
//! subscriber can see its own instance's `ISSUED` before another instance's
//! `PAID`. The database stays the source of truth; see the ordering section of
//! `docs/adr/0005-cross-process-mint-notifications-bus.md`.
//!
//! Inbound work is one supervised loop: the listener is a
//! [`SupervisedStream`] whose items are the payloads themselves, so reading,
//! decoding and local delivery happen in the task that also owns reconnects.
//! Decoding and fan-out do not await, so they do not hold the read up. A slow
//! peer is absorbed by the connection rather than by a queue.
//!
//! Wire format is JSON. `NOTIFY` payloads are capped by Postgres at 8000 bytes;
//! oversized events are delivered locally but not forwarded (a warning is
//! logged). Mint events are small; this only concerns unusually large melts.
//!
//! Forwarding is best-effort and the database remains the source of truth. A
//! publish while the publishing connection is down is queued, and flushed in
//! order once it reconnects; past the queue's capacity it drops them, counted
//! and reported in one aggregated log line per episode rather than one per
//! event. Queued events are also discarded if the bus is dropped before they
//! reach the wire. Peers recover the state itself from the database, but only
//! when a subscription is created: see [`PostgresBusConnector::new`].
//!
//! Inbound payloads are trusted: they are decoded and handed to local
//! subscribers without being re-validated against the database. `LISTEN` and
//! `NOTIFY` need no privileges beyond connecting, so every role that can reach
//! the mint's database can both read this channel and publish on it, and what
//! it publishes reaches wallets as if the mint had sent it. A payload that does
//! not decode is dropped and reported in aggregate, since a `NOTIFY` reaches
//! every instance and one writer must not cost a log line per payload on each
//! of them. A mint using this bus must own its database; see the trust model in
//! `docs/adr/0005-cross-process-mint-notifications-bus.md`.
//!
//! Both connections are tied to the bus lifetime: dropping the built
//! [`PostgresBus`] stops the listener's supervisor, which drops the stream its
//! connection lives in, and closes the queue the publisher reads, which ends
//! that task and drops its connection. An unbuilt [`PostgresBusConnector`]
//! holds the listening connection directly, so dropping it releases that
//! connection just the same.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use cdk_common::database::Error;
use cdk_common::pub_sub::{Bus, LocalDelivery, Spec};
use cdk_common::stream::{BackoffPolicy, SupervisedStream};
use cdk_sql_common::pool::DatabaseConfig;
use futures_util::{pin_mut, Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_postgres::Client;

use crate::connection::{connect_and_drive, connect_listening, error_chain, AwaitEnd, Payloads};
use crate::PgConfig;

/// Backoff before the first reconnect attempt.
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

/// Upper bound for the reconnect backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Maximum `NOTIFY` payload Postgres accepts is 8000 bytes; stay under it.
const MAX_NOTIFY_PAYLOAD: usize = 7999;

/// Depth of the queue between [`PostgresBus::publish`] and the forwarding task.
///
/// Deep enough that a reconnect at the backoff floor loses nothing. Past it a
/// publish drops its forward rather than blocking a request handler or growing
/// without bound during a database incident.
const FORWARD_QUEUE: usize = 1024;

/// Floor between aggregated reports of dropped events, outbound and inbound, so
/// a database incident or a peer writing garbage costs a bounded number of log
/// lines instead of one per event.
const DROP_REPORT_INTERVAL: Duration = Duration::from_secs(30);

/// Envelope written to the wire. Borrows the event to avoid a clone.
#[derive(Serialize)]
struct OutEnvelope<'a, E> {
    origin: &'a str,
    event: &'a E,
}

/// Envelope read from the wire.
#[derive(Deserialize)]
struct InEnvelope<E> {
    origin: String,
    event: E,
}

/// Outcome of classifying an inbound wire payload.
enum Inbound<E> {
    /// A peer event to deliver locally.
    Deliver(E),
    /// This instance's own event echoed back by Postgres; ignore it.
    SelfEcho,
    /// Undecodable payload; ignore it. Carries the decode error so the listener
    /// can say why a peer's event was dropped.
    Malformed(serde_json::Error),
}

/// Decode an inbound payload and decide what to do with it.
///
/// Pure, so the self-echo and malformed-payload handling are unit-testable
/// without a live connection.
fn classify<E: DeserializeOwned>(payload: &str, our_origin: &str) -> Inbound<E> {
    match serde_json::from_str::<InEnvelope<E>>(payload) {
        Ok(envelope) if envelope.origin == our_origin => Inbound::SelfEcho,
        Ok(envelope) => Inbound::Deliver(envelope.event),
        Err(err) => Inbound::Malformed(err),
    }
}

/// Serialize one event for the wire, or `None` when it cannot be forwarded.
///
/// The inverse of [`classify`], and pure for the same reason: the oversize and
/// serialize-failure branches are unit-testable without a live connection.
fn encode<E: Serialize>(origin: &str, event: &E) -> Option<String> {
    let payload = match serde_json::to_string(&OutEnvelope { origin, event }) {
        Ok(payload) => payload,
        Err(err) => {
            tracing::warn!("postgres bus: failed to serialize event: {err}");
            return None;
        }
    };

    if payload.len() > MAX_NOTIFY_PAYLOAD {
        tracing::warn!(
            "postgres bus: event of {} bytes exceeds NOTIFY limit, not forwarded to peers",
            payload.len()
        );
        return None;
    }

    Some(payload)
}

/// Aggregates malformed inbound payloads, so anything able to `NOTIFY` on the
/// channel costs a bounded number of log lines rather than one per payload.
struct MalformedReport {
    pending: u64,
    /// `None` until the first report, so the first malformed payload is logged
    /// at once and only a sustained flood is held back.
    last_report: Option<Instant>,
}

impl MalformedReport {
    fn new() -> Self {
        Self {
            pending: 0,
            last_report: None,
        }
    }

    /// Count one malformed payload, returning the tally to log once the floor
    /// has elapsed, and `None` while it is still being aggregated.
    ///
    /// A residual tally surfaces with the next malformed payload rather than on
    /// a timer: nothing else wakes the listener, and a channel that went quiet
    /// no longer needs reporting.
    fn record(&mut self) -> Option<u64> {
        self.pending += 1;

        if let Some(last) = self.last_report {
            if last.elapsed() < DROP_REPORT_INTERVAL {
                return None;
            }
        }

        let pending = self.pending;
        self.pending = 0;
        self.last_report = Some(Instant::now());
        Some(pending)
    }
}

/// A Postgres bus, not yet bound to a subscriber set.
///
/// [`PostgresBusConnector::connect`] establishes the listening connection and
/// starts listening; [`PostgresBusConnector::build`] then attaches it to a
/// [`Pubsub`](cdk_common::pub_sub::Pubsub) via its [`LocalDelivery`] handle and
/// opens the publishing side.
#[allow(missing_debug_implementations)]
pub struct PostgresBusConnector {
    connect: PgConnect,
    /// The connection [`PostgresBusConnector::connect`] opened, handed to the
    /// supervisor's first attempt by [`PostgresBusConnector::build`]. Dropping
    /// the connector instead drops it, which closes the connection. `None` from
    /// [`PostgresBusConnector::new`], which leaves the first attempt to the
    /// supervisor.
    listening: Option<Listening>,
    channel: Arc<str>,
    origin: Arc<str>,
}

impl PostgresBusConnector {
    /// Connect to Postgres and start listening on `channel`.
    ///
    /// Returns once the listening connection is established and the `LISTEN` is
    /// in place, so a failure to reach Postgres surfaces here rather than
    /// silently later. That same connection is handed to the supervisor by
    /// [`build`](Self::build), which keeps it alive and reconnects with backoff
    /// until the bus is dropped.
    ///
    /// The publishing connection is not opened here. It is opened by the
    /// forwarding task under its own backoff, so an instance that can listen
    /// but not publish starts and says so in its logs rather than failing.
    ///
    /// `channel` must be a valid Postgres identifier (letters, digits and
    /// underscores, not starting with a digit, at most 63 bytes) because it is
    /// interpolated into the `LISTEN` statement, which cannot be parameterized.
    pub async fn connect(config: PgConfig, channel: &str) -> Result<Self, Error> {
        let mut connector = Self::new(config, channel)?;
        connector.listening = Some(connector.connect.open().await.map_err(Error::Internal)?);
        Ok(connector)
    }

    /// Build a bus without waiting for Postgres.
    ///
    /// The supervisor opens the first connection in the background, so local
    /// delivery works from the start and a database that is not reachable yet is
    /// retried with backoff instead of being fatal.
    ///
    /// Events published while the bus is disconnected still reach local
    /// subscribers, and are queued for peers and flushed in order once it
    /// reconnects. Past the queue's capacity it drops them, and peers never see
    /// those. Recovering them is a matter for the subscriber:
    /// `fetch_events` backfills a subscription when it is created, so a wallet
    /// that subscribes after the reconnect sees current state, while one whose
    /// subscription stayed open on a peer instance throughout misses the
    /// interim events until it polls or reconnects. The database, not this bus,
    /// is the source of truth.
    ///
    /// `channel` has the same requirements as in [`connect`](Self::connect).
    pub fn new(config: PgConfig, channel: &str) -> Result<Self, Error> {
        let channel = validate_channel(channel)?;

        Ok(Self {
            connect: PgConnect {
                config,
                listen_sql: format!("LISTEN \"{channel}\""),
            },
            listening: None,
            channel: Arc::from(channel),
            origin: Arc::from(new_origin_id().as_str()),
        })
    }

    /// Attach the bus to a subscriber set and return it as a [`Bus`].
    ///
    /// Spawns the supervised listener, which delivers peer events through
    /// `local`, skipping this instance's own echoed events, and the forwarding
    /// task, which owns the publishing connection. Both reconnect with backoff
    /// until the returned bus is dropped.
    pub fn build<S>(self, local: LocalDelivery<S>) -> Arc<dyn Bus<S>>
    where
        S: Spec + 'static,
    {
        let PostgresBusConnector {
            connect,
            listening,
            channel,
            origin,
        } = self;

        let (shutdown, mut listen_shutdown) = watch::channel(());
        let mut publish_shutdown = shutdown.subscribe();

        let (outbound, events) = mpsc::channel(FORWARD_QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let name = format!("postgres bus:{channel}");

        let publisher = PgPublish::<S> {
            name: name.clone(),
            config: connect.config.clone(),
            channel: channel.clone(),
            origin: origin.clone(),
            events,
            dropped: dropped.clone(),
            reported: 0,
            last_report: Instant::now(),
        };

        let mut listener = PgListen {
            name,
            connect,
            local: local.clone(),
            origin,
            first: listening,
            malformed: MalformedReport::new(),
        };

        // The reconnect, backoff, and shutdown loop is provided by
        // `SupervisedStream`. It stops once the `watch::Sender` held by the
        // returned `PostgresBus` is dropped, which the receiver's `changed()`
        // observes.
        cdk_common::task::spawn(async move {
            listener
                .supervise(async move {
                    let _ = listen_shutdown.changed().await;
                })
                .await;
        });

        // The forwarding task stops on that same signal, and also when the
        // queue closes because the bus was dropped. The signal is what stops it
        // promptly while it is parked in a backoff.
        cdk_common::task::spawn(async move {
            publisher
                .run(async move {
                    let _ = publish_shutdown.changed().await;
                })
                .await;
        });

        Arc::new(PostgresBus {
            local,
            outbound,
            dropped,
            _shutdown: shutdown,
        })
    }
}

/// Postgres-backed [`Bus`]. See the module docs.
#[allow(missing_debug_implementations)]
pub struct PostgresBus<S>
where
    S: Spec + 'static,
{
    local: LocalDelivery<S>,
    /// Queue the forwarding task drains. Writing to it in the calling thread is
    /// what makes wire order match publish order.
    outbound: mpsc::Sender<S::Event>,
    /// Forwards dropped because `outbound` was full. Read and reported by the
    /// forwarding task, which is the only place that knows when an episode of
    /// dropping has ended.
    dropped: Arc<AtomicU64>,
    /// Dropped when the bus is dropped, which stops the background tasks.
    _shutdown: watch::Sender<()>,
}

impl<S> Bus<S> for PostgresBus<S>
where
    S: Spec + 'static,
{
    /// Enqueues the event for peers in call order, so peers receive this
    /// instance's events in the order it published them, and fans it out
    /// locally on a spawned task like
    /// [`LocalBus`](cdk_common::pub_sub::LocalBus), so a publishing request
    /// handler never pays for delivery or contends with a concurrent
    /// `subscribe`.
    ///
    /// The clone is what buys that ordering: the queue has to be written in the
    /// calling thread, while local fan-out keeps its own task so a slow forward
    /// cannot hold in-process subscribers up. Serialization happens on the
    /// forwarding task, off this path.
    fn publish(&self, event: S::Event) {
        if self.outbound.try_send(event.clone()).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }

        let local = self.local.clone();
        cdk_common::task::spawn(async move {
            local.deliver(event);
        });
    }
}

/// How to open a listening connection.
///
/// Not generic over the subscriber set, so [`PostgresBusConnector::connect`]
/// can open the first connection before a [`LocalDelivery`] handle exists, and
/// so the supervisor can reopen one on every reconnect.
struct PgConnect {
    config: PgConfig,
    listen_sql: String,
}

impl PgConnect {
    /// Open a fresh connection and `LISTEN` on it, bounded by the configured
    /// connection timeout as one budget for the whole attempt. An attempt that
    /// never returns would otherwise park the supervisor and stop it from ever
    /// reconnecting.
    async fn open(&self) -> Result<Listening, String> {
        match timeout(self.config.default_timeout(), self.listen()).await {
            Ok(result) => result,
            Err(_) => Err("timeout opening postgres bus connection".to_string()),
        }
    }

    /// `LISTEN` runs before the connection is handed back, so the supervisor
    /// never serves a connection that is up but not listening. Until it
    /// succeeds both halves are locals, so every failing exit path (a failed
    /// `LISTEN`, the caller's timeout, a shutdown that drops this future) drops
    /// them and closes the connection.
    async fn listen(&self) -> Result<Listening, String> {
        let (client, mut payloads) = connect_listening(&self.config)
            .await
            .map_err(|err| error_chain(&err))?;

        drive(&mut payloads, client.batch_execute(&self.listen_sql))
            .await?
            .map_err(|err| error_chain(&err))?;

        Ok(Listening {
            payloads,
            _client: client,
        })
    }
}

/// A live listening connection.
///
/// The connection lives inside the payload stream, so dropping this closes it.
#[allow(missing_debug_implementations)]
struct Listening {
    payloads: Payloads,
    /// `tokio-postgres` ends a connection once its client is dropped, so the
    /// client is held for the life of the stream even though nothing queries it
    /// after the `LISTEN`.
    _client: Client,
}

impl Stream for Listening {
    type Item = Result<String, String>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().payloads.as_mut().poll_next(cx)
    }
}

/// Await a request on a connection's client while polling that connection,
/// which is what makes the request progress. Payloads arriving meanwhile are
/// dropped: this only runs before the `LISTEN` is in place, so there are none
/// to lose.
async fn drive<T>(payloads: &mut Payloads, request: impl Future<Output = T>) -> Result<T, String> {
    pin_mut!(request);

    loop {
        tokio::select! {
            result = &mut request => return Ok(result),
            payload = payloads.next() => match payload {
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(err),
                None => return Err("connection closed".to_string()),
            },
        }
    }
}

/// The publishing half of the Postgres bus.
///
/// Owns the queue [`PostgresBus::publish`] writes to and a connection used for
/// nothing else, and drains the one onto the other sequentially. That is what
/// keeps wire order equal to publish order, and it bounds the bus to one
/// in-flight `pg_notify` at a time.
#[allow(missing_debug_implementations)]
struct PgPublish<S>
where
    S: Spec + 'static,
{
    config: PgConfig,
    channel: Arc<str>,
    origin: Arc<str>,
    events: mpsc::Receiver<S::Event>,
    dropped: Arc<AtomicU64>,
    /// Drops already named in a log line, so each report covers only what is
    /// new since the last one.
    reported: u64,
    last_report: Instant,
    name: String,
}

impl<S> PgPublish<S>
where
    S: Spec + 'static,
{
    /// Forward queued events until the bus is dropped or `shutdown` resolves,
    /// reconnecting with backoff.
    ///
    /// Hand-rolled rather than a [`SupervisedStream`], which supervises a
    /// stream of inbound items: this loop has to drain a queue while its
    /// connection is merely alive. The backoff follows the same rule as
    /// [`SupervisedStream::supervise`], so the two halves of the bus behave
    /// alike: consecutive connect failures double the delay, while a connection
    /// that was healthy and then dropped retries at the floor.
    ///
    /// Shutdown wins over draining: whatever is still queued when the bus is
    /// dropped is discarded rather than flushed, so a shutdown is not held up
    /// by a backlog or by a database that has stopped answering.
    async fn run(mut self, shutdown: impl Future<Output = ()> + Send) {
        pin_mut!(shutdown);
        let mut backoff = INITIAL_BACKOFF;
        let mut degraded = false;

        'outer: loop {
            let opened = tokio::select! {
                biased;
                _ = &mut shutdown => break 'outer,
                opened = self.open() => opened,
            };

            let wait = match opened {
                Ok((client, mut driver)) => {
                    backoff = INITIAL_BACKOFF;
                    if std::mem::take(&mut degraded) {
                        self.report_resumed();
                    }

                    loop {
                        let event = tokio::select! {
                            biased;
                            _ = &mut shutdown => break 'outer,
                            _ = &mut driver => break,
                            event = self.events.recv() => event,
                        };

                        // The queue closes when the bus is dropped.
                        let Some(event) = event else { break 'outer };

                        let Some(payload) = encode(&self.origin, &event) else {
                            continue;
                        };

                        let channel: &str = &self.channel;
                        if let Err(err) = client
                            .execute("SELECT pg_notify($1, $2)", &[&channel, &payload])
                            .await
                        {
                            // This event left the queue and never reached the
                            // wire, so it counts against the outage like the
                            // ones the queue drops. Not retried after the
                            // reconnect: a failed `execute` does not say
                            // whether the statement committed, and a peer
                            // seeing stale state twice is worse than seeing it
                            // once from the database.
                            self.dropped.fetch_add(1, Ordering::Relaxed);
                            tracing::warn!(
                                name = self.name,
                                "pg_notify failed, reconnecting: {}",
                                error_chain(&err)
                            );
                            break;
                        }

                        self.report_drops();
                    }

                    degraded = true;
                    tracing::warn!(
                        name = self.name,
                        "publishing connection lost, events are not reaching \
                         peers until it is restored"
                    );
                    // The connection was working, so retry at the floor rather
                    // than at a backoff earned by earlier failures.
                    INITIAL_BACKOFF
                }
                Err(err) => {
                    if degraded {
                        tracing::debug!(
                            name = self.name,
                            "publishing connection still unavailable: {err}"
                        );
                    } else {
                        degraded = true;
                        tracing::warn!(
                            name = self.name,
                            "cannot open the publishing connection, events are \
                             not reaching peers: {err}"
                        );
                    }
                    let wait = backoff;
                    backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
                    wait
                }
            };

            tokio::select! {
                biased;
                _ = &mut shutdown => break 'outer,
                _ = sleep(wait) => {}
            }
        }
    }

    /// Open the publishing connection, bounded by the configured connection
    /// timeout as one budget for the whole attempt, like the listener's. An
    /// attempt that never returns would otherwise park this loop and stop it
    /// from ever reconnecting.
    async fn open(&self) -> Result<(Client, JoinHandle<()>), String> {
        let attempt = connect_and_drive(&self.config, AwaitEnd);

        match timeout(self.config.default_timeout(), attempt).await {
            Ok(result) => result.map_err(|err| error_chain(&err)),
            Err(_) => Err("timeout opening postgres bus publishing connection".to_string()),
        }
    }

    /// Announce that forwarding has resumed, naming what the outage cost.
    fn report_resumed(&mut self) {
        let dropped = self.take_drops();
        if dropped > 0 {
            tracing::warn!(
                name = self.name,
                "publishing connection restored; {dropped} events were not \
                 forwarded to peers and reach subscribers only through the \
                 database, when they next subscribe"
            );
        } else {
            tracing::info!(name = self.name, "publishing connection restored");
        }
    }

    /// Report drops accumulated while connected, at most once per
    /// [`DROP_REPORT_INTERVAL`].
    ///
    /// Reaching here means `pg_notify` is slower than publishes arrive rather
    /// than the connection being down, which no reconnect would announce. The
    /// interval is what keeps a sustained backlog from costing one log line per
    /// event, which is the shape this funnel replaced.
    fn report_drops(&mut self) {
        if self.dropped.load(Ordering::Relaxed) == self.reported
            || self.last_report.elapsed() < DROP_REPORT_INTERVAL
        {
            return;
        }

        let dropped = self.take_drops();
        tracing::warn!(
            name = self.name,
            "outbound queue full; {dropped} events were not forwarded to peers"
        );
    }

    /// Drops since the last report, marking them all reported.
    fn take_drops(&mut self) -> u64 {
        let dropped = self.dropped.load(Ordering::Relaxed);
        let new = dropped - self.reported;
        self.reported = dropped;
        self.last_report = Instant::now();
        new
    }
}

/// The listening half of the Postgres bus.
///
/// Carries the wire payloads as its items, so one supervised loop reads them,
/// decodes them and hands them to the local fan-out. The reconnect, backoff and
/// shutdown loop is provided by [`SupervisedStream`].
///
/// `LISTEN` runs inside [`connect`](SupervisedStream::connect), so a failed
/// `LISTEN` is a failed connect: the supervisor backs off and reconnects rather
/// than serving a connection that is up but not listening.
#[allow(missing_debug_implementations)]
struct PgListen<S>
where
    S: Spec + 'static,
{
    connect: PgConnect,
    local: LocalDelivery<S>,
    origin: Arc<str>,
    /// The connection [`PostgresBusConnector::connect`] already opened, taken by
    /// the first attempt so startup opens exactly one.
    first: Option<Listening>,
    name: String,
    /// Malformed inbound payloads, aggregated so a flood stays bounded in the
    /// log.
    malformed: MalformedReport,
}

#[async_trait::async_trait]
impl<S> SupervisedStream for PgListen<S>
where
    S: Spec + 'static,
{
    type Item = String;
    type ConnectError = String;
    type StreamError = String;
    type Stream = Listening;

    fn name(&self) -> &str {
        &self.name
    }

    fn backoff_policy(&self) -> BackoffPolicy {
        BackoffPolicy {
            initial_connect_backoff: INITIAL_BACKOFF,
            max_connect_backoff: MAX_BACKOFF,
        }
    }

    async fn connect(&mut self) -> Result<Self::Stream, Self::ConnectError> {
        match self.first.take() {
            Some(listening) => Ok(listening),
            None => self.connect.open().await,
        }
    }

    /// Decode one payload and fan it out locally. Neither step awaits, so the
    /// read loop is not held up.
    async fn on_message(&mut self, payload: String) {
        match classify::<S::Event>(&payload, &self.origin) {
            Inbound::Deliver(event) => self.local.deliver(event),
            Inbound::SelfEcho => {}
            Inbound::Malformed(err) => {
                if let Some(count) = self.malformed.record() {
                    tracing::warn!(
                        name = self.name,
                        "dropped {count} malformed payloads, most recent: {err}"
                    );
                }
            }
        }
    }
}

/// Generate a per-instance origin id used to skip self-echoed events.
fn new_origin_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Validate that `channel` is a safe Postgres identifier for interpolation into
/// `LISTEN`.
fn validate_channel(channel: &str) -> Result<String, Error> {
    let valid = !channel.is_empty()
        && channel.len() <= 63
        && channel
            .bytes()
            .next()
            .map(|b| b.is_ascii_alphabetic() || b == b'_')
            .unwrap_or(false)
        && channel
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_');

    if valid {
        Ok(channel.to_string())
    } else {
        Err(Error::Internal(format!(
            "invalid postgres bus channel name: {channel:?}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::bus_test;
    use cdk_common::pub_sub::test::{CustomPubSub, Message, SubscriptionReq};
    use cdk_common::pub_sub::Pubsub;

    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Ev {
        foo: u64,
    }

    /// Connection string for the test database, matching the generic database
    /// tests: `CDK_MINTD_DATABASE_URL`, then `PG_DB_URL`, then a local default.
    fn test_db_url() -> String {
        std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL"))
            .unwrap_or_else(|_| {
                "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                    .to_owned()
            })
    }

    /// Derive a valid, unique `LISTEN`/`NOTIFY` channel from a test id.
    ///
    /// The id from [`bus_test!`] begins with `test_<nanos>_`, so truncating to
    /// the 63-byte Postgres identifier limit keeps the unique `<nanos>` prefix.
    /// A unique channel per test means one test never receives another's
    /// `NOTIFY`.
    fn pg_channel(test_id: &str) -> String {
        test_id.chars().take(63).collect()
    }

    /// Connect a Postgres bus on `channel` and wrap it in a `Pubsub`.
    ///
    /// The config carries the channel as its `application_name`, which both of
    /// the instance's connections inherit, so a test that terminates backends
    /// can find its own in `pg_stat_activity` without touching a concurrently
    /// running test's. The listening connection is identifiable by its `LISTEN`
    /// statement, but the publishing one is not: it binds the channel as a
    /// parameter, so `pg_stat_activity.query` is the same string for every
    /// instance.
    async fn pg_pubsub<S>(channel: &str) -> Pubsub<S>
    where
        S: Spec<Context = ()> + 'static,
    {
        let config = PgConfig::from(test_db_url().as_str()).with_application_name(channel);
        let connector = PostgresBusConnector::connect(config, channel)
            .await
            .expect("connect postgres bus");
        Pubsub::new_with_bus(S::new_instance(()), move |local| connector.build(local))
    }

    /// Factory for the generic bus suite: one Postgres-backed node per test.
    async fn provide_pg_bus(test_id: String) -> Pubsub<CustomPubSub> {
        pg_pubsub(&pg_channel(&test_id)).await
    }

    bus_test!(provide_pg_bus);

    /// Two mint instances sharing one Postgres database and channel: an event
    /// published on instance A is delivered through `LISTEN`/`NOTIFY` to a
    /// subscriber on instance B, and A's own subscriber receives it exactly
    /// once (the echo Postgres sends back is dropped as a self-echo).
    #[tokio::test]
    async fn event_crosses_instances_through_postgres() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let channel = pg_channel(&format!("test_{nanos}_cross_node"));

        let instance_a = pg_pubsub::<CustomPubSub>(&channel).await;
        let instance_b = pg_pubsub::<CustomPubSub>(&channel).await;

        let mut sub_a = instance_a.subscribe(SubscriptionReq::Foo(2)).unwrap();
        let mut sub_b = instance_b.subscribe(SubscriptionReq::Foo(2)).unwrap();

        instance_a.publish(Message { foo: 2, bar: 7 });

        // Delivered to the other instance through Postgres.
        let received_b = timeout(Duration::from_secs(5), sub_b.recv())
            .await
            .expect("event delivered to instance B before timeout");
        assert_eq!(received_b.map(|m| m.bar), Some(7));

        // Delivered locally on the publishing instance as well.
        let received_a = timeout(Duration::from_secs(5), sub_a.recv())
            .await
            .expect("event delivered to instance A before timeout");
        assert_eq!(received_a.map(|m| m.bar), Some(7));

        // The self-echo Postgres sends back is dropped: A sees the event once.
        // Wait long enough for a round-trip through the database.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(sub_a.try_recv().is_none());
    }

    /// Events published in sequence on one instance reach a subscriber on
    /// another in that same sequence. NUT-17 payloads are full-state
    /// replacements, so a reordered pair would present to that subscriber as a
    /// state regression. Only one publisher, because that is the whole
    /// guarantee: events from two instances are not ordered against each
    /// other.
    ///
    /// This is what the publish funnel buys: before it, each publish forwarded
    /// on its own task and the order two of them reached the wire was down to
    /// the scheduler. Multi-threaded on purpose, because that scheduler is the
    /// one a mint runs on: on the single-threaded runtime `#[tokio::test]`
    /// gives by default, spawned tasks run in the order they were spawned and
    /// the reordering this guards against cannot happen.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn publish_order_from_one_instance_is_preserved() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let channel = pg_channel(&format!("test_{nanos}_ordering"));

        let instance_a = pg_pubsub::<CustomPubSub>(&channel).await;
        let instance_b = pg_pubsub::<CustomPubSub>(&channel).await;
        let mut sub_b = instance_b.subscribe(SubscriptionReq::Foo(9)).unwrap();

        // Published back to back, so before the funnel these raced each other
        // onto the shared connection.
        for bar in 0..32 {
            instance_a.publish(Message { foo: 9, bar });
        }

        for bar in 0..32 {
            let received = timeout(Duration::from_secs(5), sub_b.recv())
                .await
                .expect("event delivered to instance B before timeout")
                .expect("subscription stays open");
            assert_eq!(
                received.bar, bar,
                "instance B must see events in the order A published them"
            );
        }
    }

    /// A bus built without a reachable database still serves local subscribers,
    /// which is what lets a mint start when its peers are unreachable instead of
    /// refusing to start. The supervisor retries the connection in the
    /// background.
    #[tokio::test]
    async fn local_delivery_works_without_a_reachable_database() {
        let connector = PostgresBusConnector::new(
            PgConfig::new(
                "host=127.0.0.1 port=1 user=nobody dbname=nothing",
                None,
                None,
                Some(1),
            ),
            "cdk_bus_unreachable",
        )
        .expect("a bus is built without connecting");

        let pubsub = Pubsub::new_with_bus(CustomPubSub::new_instance(()), move |local| {
            connector.build(local)
        });
        let mut subscriber = pubsub.subscribe(SubscriptionReq::Foo(5)).unwrap();

        pubsub.publish(Message { foo: 5, bar: 13 });

        let received = timeout(Duration::from_secs(5), subscriber.recv())
            .await
            .expect("event delivered locally while the bus is disconnected");
        assert_eq!(received.map(|m| m.bar), Some(13));
    }

    /// Delivery survives losing the connection. The first supervised attempt
    /// reuses the connection opened at startup, so a reconnect is the first time
    /// the supervisor opens one itself, installs a new client for the publish
    /// path, and resumes listening.
    ///
    /// The drop is forced by terminating the listening backends from a separate
    /// connection, matched on this test's unique channel so no other test's
    /// connection is touched.
    #[tokio::test]
    async fn delivery_resumes_after_the_connection_drops() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let channel = pg_channel(&format!("test_{nanos}_reconnect"));

        let instance_a = pg_pubsub::<CustomPubSub>(&channel).await;
        let instance_b = pg_pubsub::<CustomPubSub>(&channel).await;
        let mut sub_b = instance_b.subscribe(SubscriptionReq::Foo(3)).unwrap();

        let (admin, connection) = tokio_postgres::connect(&test_db_url(), tokio_postgres::NoTls)
            .await
            .expect("admin connect");
        cdk_common::task::spawn(async move {
            let _ = connection.await;
        });
        let listening = format!("%{channel}%");
        let terminated = admin
            .execute(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE query LIKE $1 AND pid <> pg_backend_pid()",
                &[&listening],
            )
            .await
            .expect("terminate the listening backends");
        // Otherwise the test would pass without a reconnect ever happening.
        assert_eq!(terminated, 2, "both listening backends must be terminated");

        // Long enough for the supervisor to notice and reconnect at its floor.
        tokio::time::sleep(Duration::from_secs(2)).await;

        instance_a.publish(Message { foo: 3, bar: 11 });

        let received = timeout(Duration::from_secs(5), sub_b.recv())
            .await
            .expect("event delivered after the reconnect");
        assert_eq!(received.map(|m| m.bar), Some(11));
    }

    /// Build a forwarding task with no connection behind it, for the parts of
    /// it that do not need one.
    fn detached_publisher(
        url: &str,
    ) -> (
        PgPublish<CustomPubSub>,
        mpsc::Sender<Message>,
        Arc<AtomicU64>,
    ) {
        let (outbound, events) = mpsc::channel(FORWARD_QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));

        let publisher = PgPublish {
            name: "test".to_string(),
            config: PgConfig::new(url, None, None, Some(1)),
            channel: Arc::from("test"),
            origin: Arc::from("origin"),
            events,
            dropped: dropped.clone(),
            reported: 0,
            last_report: Instant::now(),
        };

        (publisher, outbound, dropped)
    }

    /// Each report names only the drops since the last one, so the counts an
    /// operator reads add up to the total rather than repeating it.
    #[tokio::test]
    async fn drop_reports_do_not_double_count() {
        let (mut publisher, _outbound, dropped) = detached_publisher("host=127.0.0.1 port=1");

        assert_eq!(publisher.take_drops(), 0);

        dropped.fetch_add(3, Ordering::Relaxed);
        assert_eq!(publisher.take_drops(), 3);
        // Already reported, so a second look finds nothing new.
        assert_eq!(publisher.take_drops(), 0);

        dropped.fetch_add(2, Ordering::Relaxed);
        assert_eq!(publisher.take_drops(), 2);
    }

    /// Drops are reported at most once per interval, which is what keeps a
    /// sustained backlog from costing a log line per event. Within the
    /// interval the count keeps accruing instead of being lost.
    #[tokio::test]
    async fn drop_reports_are_rate_limited() {
        let (mut publisher, _outbound, dropped) = detached_publisher("host=127.0.0.1 port=1");

        dropped.fetch_add(5, Ordering::Relaxed);
        publisher.report_drops();
        assert_eq!(publisher.reported, 0, "too soon to report");

        publisher.last_report = Instant::now()
            .checked_sub(DROP_REPORT_INTERVAL + Duration::from_secs(1))
            .expect("the monotonic clock is past the report interval");
        publisher.report_drops();
        assert_eq!(
            publisher.reported, 5,
            "the whole backlog is reported at once"
        );
    }

    /// The first malformed payload is reported at once, so a single bad writer
    /// is visible without waiting the floor out, while the flood behind it is
    /// held back.
    #[test]
    fn first_malformed_payload_is_reported_at_once() {
        let mut report = MalformedReport::new();

        assert_eq!(report.record(), Some(1));
        assert_eq!(report.record(), None, "too soon to report again");
        assert_eq!(report.record(), None);
        assert_eq!(report.pending, 2, "the held-back payloads keep accruing");
    }

    /// Malformed payloads are reported at most once per interval, which is what
    /// keeps a session flooding the channel from costing a log line per payload
    /// on every listening instance.
    #[test]
    fn malformed_reports_are_rate_limited() {
        let mut report = MalformedReport::new();

        assert_eq!(report.record(), Some(1));
        for _ in 0..4 {
            assert_eq!(report.record(), None);
        }

        report.last_report = Some(
            Instant::now()
                .checked_sub(DROP_REPORT_INTERVAL + Duration::from_secs(1))
                .expect("the monotonic clock is past the report interval"),
        );

        assert_eq!(
            report.record(),
            Some(5),
            "the whole backlog is reported at once"
        );
        assert_eq!(report.pending, 0, "and never reported twice");
    }

    /// The forwarding task stops promptly on shutdown even while parked in a
    /// reconnect backoff against a database it cannot reach. Without that, a
    /// dropped bus would linger for as long as the backoff.
    #[tokio::test]
    async fn forwarding_stops_promptly_while_backing_off() {
        let (publisher, _outbound, _dropped) = detached_publisher("host=127.0.0.1 port=1");
        let (shutdown, mut rx) = watch::channel(());

        let task = cdk_common::task::spawn(async move {
            publisher
                .run(async move {
                    let _ = rx.changed().await;
                })
                .await;
        });

        // Connections to a closed port fail at once, so by now the backoff has
        // doubled its way past the deadline below: a task that only noticed the
        // shutdown after its sleep would miss it.
        tokio::time::sleep(Duration::from_secs(4)).await;
        drop(shutdown);

        timeout(Duration::from_millis(1500), task)
            .await
            .expect("the forwarding task stops without waiting out its backoff")
            .expect("forwarding task");
    }

    /// Forwarding survives losing the publishing connection, which reconnects
    /// on its own rather than riding on the listener's supervisor.
    ///
    /// Only the publishing backends are terminated, matched on the `pg_notify`
    /// statement they last ran, so the listeners stay up and this exercises the
    /// forwarding task's own reconnect. A first publish is needed to make those
    /// backends identifiable: until one runs, their `query` is empty.
    #[tokio::test]
    async fn forwarding_resumes_after_the_publishing_connection_drops() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let channel = pg_channel(&format!("test_{nanos}_publisher_reconnect"));

        let instance_a = pg_pubsub::<CustomPubSub>(&channel).await;
        let instance_b = pg_pubsub::<CustomPubSub>(&channel).await;
        let mut sub_b = instance_b.subscribe(SubscriptionReq::Foo(4)).unwrap();

        instance_a.publish(Message { foo: 4, bar: 1 });
        let received = timeout(Duration::from_secs(5), sub_b.recv())
            .await
            .expect("the first event crosses before the connection is cut");
        assert_eq!(received.map(|m| m.bar), Some(1));

        let (admin, connection) = tokio_postgres::connect(&test_db_url(), tokio_postgres::NoTls)
            .await
            .expect("admin connect");
        cdk_common::task::spawn(async move {
            if let Err(err) = connection.await {
                tracing::debug!("admin connection ended: {err}");
            }
        });
        // Scoped to this test's own instances: an unscoped '%pg_notify%' would
        // also terminate the publishing connection of any test running
        // concurrently, whose in-flight event would then be lost.
        let instance: &str = &channel;
        let terminated = admin
            .execute(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE application_name = $1 AND query LIKE '%pg_notify%' \
                 AND pid <> pg_backend_pid()",
                &[&instance],
            )
            .await
            .expect("terminate the publishing backend");
        // Only A has published, so only A's publishing connection matches.
        // Otherwise the test would pass without a reconnect ever happening.
        assert_eq!(
            terminated, 1,
            "exactly this test's publishing backend must be terminated"
        );

        // Long enough for the forwarding task to notice and reconnect at its
        // floor.
        tokio::time::sleep(Duration::from_secs(2)).await;

        instance_a.publish(Message { foo: 4, bar: 2 });

        let received = timeout(Duration::from_secs(5), sub_b.recv())
            .await
            .expect("event forwarded after the publishing connection reconnected");
        assert_eq!(received.map(|m| m.bar), Some(2));
    }

    /// A failed `LISTEN` on an otherwise healthy connection must surface as a
    /// failed connect, so `SupervisedStream` backs off and reconnects rather
    /// than serving a connection that is up but not listening.
    ///
    /// The failure is forced with an invalid `LISTEN` statement: the connection
    /// opens fine, but the statement errors on the server.
    #[tokio::test]
    async fn listen_failure_errors_connect() {
        let connect = PgConnect {
            config: PgConfig::from(test_db_url().as_str()),
            // Missing channel name: opens a healthy connection, then errors.
            listen_sql: "LISTEN".to_string(),
        };

        let result = connect.open().await;

        assert!(
            result.is_err(),
            "a failed LISTEN must surface as a connect error"
        );
    }

    /// A `LISTEN` that never returns must fail the attempt on the configured
    /// timeout. Without it the supervisor parks inside `connect` and the bus
    /// never reconnects.
    ///
    /// The hang is forced with a statement that sleeps on the server far longer
    /// than the one-second timeout the config carries.
    #[tokio::test]
    async fn listen_hang_times_out_connect() {
        let connect = PgConnect {
            config: PgConfig::new(&test_db_url(), None, None, Some(1)),
            listen_sql: "SELECT pg_sleep(30)".to_string(),
        };

        let started = Instant::now();
        let result = connect.open().await;

        assert!(
            result.is_err(),
            "a hung LISTEN must surface as a connect error"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "connect must give up on the configured timeout, not wait out the statement"
        );
    }

    /// A connect that hangs before the handshake finishes must time out too, and
    /// must release the connection instead of leaving a driver task holding it.
    ///
    /// The fake server accepts and then stays silent, so the client waits on a
    /// startup response that never comes; once the attempt gives up, the server
    /// side sees EOF.
    #[tokio::test]
    async fn connect_hang_times_out_and_releases_the_socket() {
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let server = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = server.local_addr().expect("local addr").port();
        let served = cdk_common::task::spawn(async move {
            let (mut socket, _) = server.accept().await.expect("accept");
            let mut buf = [0u8; 1024];
            while socket.read(&mut buf).await.unwrap_or(0) > 0 {}
        });

        let connect = PgConnect {
            config: PgConfig::new(
                &format!("host=127.0.0.1 port={port} user=cdk_user dbname=cdk_mint"),
                None,
                None,
                Some(1),
            ),
            listen_sql: "LISTEN \"cdk_bus_test\"".to_string(),
        };

        let result = connect.open().await;

        assert!(
            result.is_err(),
            "a silent server must surface as a connect error"
        );
        timeout(Duration::from_secs(5), served)
            .await
            .expect("the timed-out attempt released its socket")
            .expect("server task");
    }

    #[test]
    fn accepts_valid_channel_names() {
        assert!(validate_channel("cdk_mint_events").is_ok());
        assert!(validate_channel("_private").is_ok());
        assert!(validate_channel("a1").is_ok());
    }

    #[test]
    fn rejects_invalid_channel_names() {
        assert!(validate_channel("").is_err());
        assert!(validate_channel("1leading_digit").is_err());
        assert!(validate_channel("has space").is_err());
        assert!(validate_channel("drop;table").is_err());
        assert!(validate_channel(&"x".repeat(64)).is_err());
    }

    #[test]
    fn envelope_roundtrips_as_json() {
        let json = serde_json::to_string(&OutEnvelope {
            origin: "abc",
            event: &Ev { foo: 7 },
        })
        .expect("serialize");

        let parsed: InEnvelope<Ev> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.origin, "abc");
        assert_eq!(parsed.event, Ev { foo: 7 });
    }

    #[test]
    fn classify_delivers_peer_events() {
        let json = serde_json::to_string(&OutEnvelope {
            origin: "peer",
            event: &Ev { foo: 9 },
        })
        .expect("serialize");

        match classify::<Ev>(&json, "self") {
            Inbound::Deliver(event) => assert_eq!(event, Ev { foo: 9 }),
            _ => panic!("expected Deliver"),
        }
    }

    #[test]
    fn classify_skips_self_echo() {
        let json = serde_json::to_string(&OutEnvelope {
            origin: "self",
            event: &Ev { foo: 1 },
        })
        .expect("serialize");

        assert!(matches!(classify::<Ev>(&json, "self"), Inbound::SelfEcho));
    }

    /// The wire path for the mint's own event type, without a database: a
    /// published `MintEvent` must come back out of `classify` unchanged. Before
    /// the wire format carried the subscription kind, every real mint event
    /// decoded as malformed here and was silently dropped.
    #[test]
    fn classify_decodes_real_mint_events() {
        use cdk_common::event::MintEvent;
        use cdk_common::nut07::State;
        use cdk_common::{NotificationPayload, ProofState, PublicKey, QuoteId};

        let event: MintEvent<QuoteId> =
            MintEvent::new(NotificationPayload::ProofState(ProofState {
                y: PublicKey::from_hex(
                    "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
                )
                .expect("valid pubkey"),
                state: State::Spent,
                witness: None,
            }));

        let payload = serde_json::to_string(&OutEnvelope {
            origin: "peer",
            event: &event,
        })
        .expect("serialize");

        match classify::<MintEvent<QuoteId>>(&payload, "self") {
            Inbound::Deliver(decoded) => assert_eq!(decoded, event),
            Inbound::SelfEcho => panic!("expected Deliver, got SelfEcho"),
            Inbound::Malformed(err) => panic!("expected Deliver, got Malformed: {err}"),
        }
    }

    #[test]
    fn classify_rejects_malformed_payload() {
        assert!(matches!(
            classify::<Ev>("not json", "self"),
            Inbound::Malformed(_)
        ));
    }

    /// A [`Spec`] whose event carries an arbitrary-size `String`, so a test can
    /// build a payload past the `NOTIFY` limit. The generic `CustomPubSub` event
    /// is two integers and can never grow that large.
    mod blob {
        use std::sync::Arc;

        use cdk_common::pub_sub::{Error, Event, Spec, Subscriber, SubscriptionRequest};
        use serde::{Deserialize, Serialize};

        #[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
        pub struct BlobEvent {
            pub topic: u64,
            pub blob: String,
        }

        impl Event for BlobEvent {
            type Topic = u64;

            fn get_topics(&self) -> Vec<u64> {
                vec![self.topic]
            }
        }

        #[derive(Debug)]
        pub struct BlobSpec;

        #[async_trait::async_trait]
        impl Spec for BlobSpec {
            type Topic = u64;
            type Event = BlobEvent;
            type SubscriptionId = String;
            type Context = ();

            fn new_instance(_context: ()) -> Arc<Self> {
                Arc::new(BlobSpec)
            }

            async fn fetch_events(
                self: &Arc<Self>,
                _topics: Vec<u64>,
                _reply_to: Subscriber<Self>,
            ) {
            }
        }

        pub struct Sub(pub u64);

        impl SubscriptionRequest for Sub {
            type Topic = u64;
            type SubscriptionId = String;

            fn try_get_topics(&self) -> Result<Vec<u64>, Error> {
                Ok(vec![self.0])
            }

            fn subscription_name(&self) -> Arc<String> {
                Arc::new("blob".to_owned())
            }
        }
    }

    /// An event too large for `NOTIFY` is still delivered to local subscribers;
    /// only the forward to peers is skipped. Skipping happens on the forwarding
    /// task now, so this covers only the local half; `encode` covers the skip
    /// itself.
    #[tokio::test]
    async fn oversized_event_is_still_delivered_locally() {
        use blob::{BlobEvent, BlobSpec, Sub};

        let (outbound, _events) = mpsc::channel(FORWARD_QUEUE);
        let (shutdown, _shutdown_rx) = watch::channel(());
        let pubsub = Pubsub::new_with_bus(BlobSpec::new_instance(()), move |local| {
            Arc::new(PostgresBus {
                local,
                outbound,
                dropped: Arc::new(AtomicU64::new(0)),
                _shutdown: shutdown,
            })
        });

        let mut subscriber = pubsub.subscribe(Sub(1)).unwrap();

        let event = BlobEvent {
            topic: 1,
            blob: "x".repeat(MAX_NOTIFY_PAYLOAD + 1),
        };

        pubsub.publish(event);

        let received = timeout(Duration::from_secs(1), subscriber.recv())
            .await
            .expect("oversized event delivered locally before timeout");
        assert_eq!(received.map(|e| e.blob.len()), Some(MAX_NOTIFY_PAYLOAD + 1));
    }

    /// `encode` refuses an event whose envelope exceeds what `NOTIFY` accepts,
    /// which is what keeps the forwarding task from issuing a statement
    /// Postgres would reject.
    #[test]
    fn encode_skips_oversized_event() {
        use blob::BlobEvent;

        let event = BlobEvent {
            topic: 1,
            blob: "x".repeat(MAX_NOTIFY_PAYLOAD + 1),
        };

        assert!(encode("origin", &event).is_none());
    }

    /// `encode` and `classify` are inverses: what the forwarding task puts on
    /// the wire is what a peer's listener takes off it.
    #[test]
    fn encode_roundtrips_through_classify() {
        let event = Ev { foo: 42 };
        let payload = encode("instance-a", &event).expect("encodes");

        match classify::<Ev>(&payload, "instance-b") {
            Inbound::Deliver(decoded) => assert_eq!(decoded, event),
            _ => panic!("a peer must decode what encode produced"),
        }
        assert!(matches!(
            classify::<Ev>(&payload, "instance-a"),
            Inbound::SelfEcho
        ));
    }

    /// Wire order must match publish order, so a peer never sees a NUT-17
    /// full-state payload regress. `publish` enqueues in the calling thread,
    /// which is what guarantees it; the forwarding task then drains the queue
    /// sequentially.
    #[tokio::test]
    async fn publish_enqueues_in_call_order() {
        use cdk_common::pub_sub::test::{CustomPubSub, Message};

        let (outbound, mut events) = mpsc::channel(FORWARD_QUEUE);
        let (shutdown, _shutdown_rx) = watch::channel(());
        let pubsub = Pubsub::new_with_bus(CustomPubSub::new_instance(()), move |local| {
            Arc::new(PostgresBus {
                local,
                outbound,
                dropped: Arc::new(AtomicU64::new(0)),
                _shutdown: shutdown,
            })
        });

        for bar in 0..64 {
            pubsub.publish(Message { foo: 1, bar });
        }

        for bar in 0..64 {
            let queued = events.recv().await.expect("event queued for peers");
            assert_eq!(queued.bar, bar, "the queue must preserve publish order");
        }
    }

    /// A full queue drops the forward and counts it, rather than blocking the
    /// publishing request handler or growing without bound. The count is what
    /// the forwarding task later reports in one aggregated line.
    #[tokio::test]
    async fn queue_full_drops_are_counted() {
        use cdk_common::pub_sub::test::{CustomPubSub, Message};

        // Nothing drains this, so every publish past the first is dropped.
        let (outbound, _events) = mpsc::channel(1);
        let (shutdown, _shutdown_rx) = watch::channel(());
        let dropped = Arc::new(AtomicU64::new(0));
        let counter = dropped.clone();

        let pubsub = Pubsub::new_with_bus(CustomPubSub::new_instance(()), move |local| {
            Arc::new(PostgresBus {
                local,
                outbound,
                dropped,
                _shutdown: shutdown,
            })
        });

        for bar in 0..5 {
            pubsub.publish(Message { foo: 1, bar });
        }

        assert_eq!(
            counter.load(Ordering::Relaxed),
            4,
            "every publish past the queue's capacity must be counted"
        );
    }

    /// A [`Spec`] carrying the mint's own NUT-17 event type.
    ///
    /// `MintEvent` is not plain serde: its payloads are only decodable together
    /// with the subscription kind that produced them, so a bus that re-encodes
    /// events has to preserve that kind. The synthetic `Message` used by the
    /// generic suite would never catch a wire format that loses it.
    mod mint_events {
        use std::sync::Arc;

        use cdk_common::event::MintEvent;
        use cdk_common::nut17::NotificationId;
        use cdk_common::pub_sub::{Error, Spec, Subscriber, SubscriptionRequest};
        use cdk_common::QuoteId;

        #[derive(Debug)]
        pub struct MintEventSpec;

        #[async_trait::async_trait]
        impl Spec for MintEventSpec {
            type Topic = NotificationId<QuoteId>;
            type Event = MintEvent<QuoteId>;
            type SubscriptionId = String;
            type Context = ();

            fn new_instance(_context: ()) -> Arc<Self> {
                Arc::new(MintEventSpec)
            }

            async fn fetch_events(
                self: &Arc<Self>,
                _topics: Vec<Self::Topic>,
                _reply_to: Subscriber<Self>,
            ) {
            }
        }

        pub struct Sub(pub NotificationId<QuoteId>);

        impl SubscriptionRequest for Sub {
            type Topic = NotificationId<QuoteId>;
            type SubscriptionId = String;

            fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
                Ok(vec![self.0.clone()])
            }

            fn subscription_name(&self) -> Arc<String> {
                Arc::new("mint-events".to_owned())
            }
        }
    }

    /// Real mint notifications cross instances: a proof state and a quote
    /// response published on instance A reach a subscriber on instance B
    /// unchanged.
    #[tokio::test]
    async fn mint_events_cross_instances_through_postgres() {
        use std::time::{SystemTime, UNIX_EPOCH};

        use cdk_common::event::MintEvent;
        use cdk_common::nut07::State;
        use cdk_common::nut17::NotificationId;
        use cdk_common::{
            Amount, CurrencyUnit, MintQuoteBolt11Response, MintQuoteState, NotificationPayload,
            PaymentMethod, ProofState, PublicKey, QuoteId,
        };
        use mint_events::{MintEventSpec, Sub};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let channel = pg_channel(&format!("test_{nanos}_mint_events"));

        let instance_a = pg_pubsub::<MintEventSpec>(&channel).await;
        let instance_b = pg_pubsub::<MintEventSpec>(&channel).await;

        let y = PublicKey::from_hex(
            "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
        )
        .expect("valid pubkey");
        let proof_state: MintEvent<QuoteId> =
            MintEvent::new(NotificationPayload::ProofState(ProofState {
                y,
                state: State::Spent,
                witness: None,
            }));

        let quote_id = QuoteId::new();
        let mint_quote: MintEvent<QuoteId> = MintEvent::new(
            NotificationPayload::MintQuoteBolt11Response(MintQuoteBolt11Response {
                quote: quote_id.clone(),
                request: "lnbc...".to_string(),
                amount: Some(Amount::from(100_000)),
                unit: Some(CurrencyUnit::Sat),
                method: PaymentMethod::BOLT11,
                amount_paid: Amount::from(0),
                amount_issued: Amount::from(0),
                updated_at: 0,
                state: MintQuoteState::Paid,
                expiry: Some(1701704757),
                pubkey: None,
            }),
        );

        let mut proofs_b = instance_b
            .subscribe(Sub(NotificationId::ProofState(y)))
            .expect("subscribe proof state");
        let mut quotes_b = instance_b
            .subscribe(Sub(NotificationId::MintQuoteBolt11(quote_id.clone())))
            .expect("subscribe mint quote");
        let mut proofs_a = instance_a
            .subscribe(Sub(NotificationId::ProofState(y)))
            .expect("subscribe proof state");

        instance_a.publish(proof_state.clone());
        instance_a.publish(mint_quote.clone());

        let received = timeout(Duration::from_secs(5), proofs_b.recv())
            .await
            .expect("proof state delivered to instance B before timeout");
        assert_eq!(received, Some(proof_state.clone()));

        let received = timeout(Duration::from_secs(5), quotes_b.recv())
            .await
            .expect("mint quote delivered to instance B before timeout");
        assert_eq!(received, Some(mint_quote));

        // The publishing instance sees its own event once: the copy Postgres
        // echoes back is dropped, not re-delivered.
        let received = timeout(Duration::from_secs(5), proofs_a.recv())
            .await
            .expect("proof state delivered to instance A before timeout");
        assert_eq!(received, Some(proof_state));

        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(proofs_a.try_recv().is_none());
    }
}
