//! Cross-process pub/sub bus backed by PostgreSQL `LISTEN`/`NOTIFY`.
//!
//! The mint's NUT-17 notifications are in-process by default: an event
//! published on one instance only reaches subscribers connected to that same
//! instance. [`PostgresBus`] implements [`cdk_common::pub_sub::Bus`] so several
//! mint instances sharing a Postgres database also share notifications.
//!
//! A publish hands the event to a background task, as the in-process bus does,
//! which fans it out to local subscribers and sends it to peers with
//! `pg_notify`, so no request handler waits on either. Each instance keeps a
//! dedicated connection in `LISTEN`, deserializes inbound events, and injects
//! them into its own local fan-out. Messages carry the origin instance id so a
//! bus skips the copy of its own event that Postgres echoes back.
//!
//! Inbound work is one supervised loop: [`PgListen`] is a
//! [`SupervisedStream`] whose items are the payloads themselves, so reading,
//! decoding and local delivery happen in the task that also owns reconnects.
//! Decoding and fan-out do not await, so they do not hold the read up. A slow
//! peer is absorbed by the connection rather than by a queue.
//!
//! Wire format is JSON. `NOTIFY` payloads are capped by Postgres at 8000 bytes;
//! oversized events are delivered locally but not forwarded (a warning is
//! logged). Mint events are small; this only concerns unusually large melts.
//!
//! Inbound payloads are trusted: they are decoded and handed to local
//! subscribers without being re-validated against the database. `LISTEN` and
//! `NOTIFY` need no privileges beyond connecting, so every role that can reach
//! the mint's database can both read this channel and publish on it, and what
//! it publishes reaches wallets as if the mint had sent it. A mint using this
//! bus must own its database; see the trust model in
//! `docs/adr/0005-cross-process-mint-notifications-bus.md`.
//!
//! The connection is tied to the bus lifetime: dropping the built
//! [`PostgresBus`] stops the supervisor, which drops the stream the connection
//! lives in. An unbuilt [`PostgresBusConnector`] holds that stream directly, so
//! dropping it releases the connection just the same.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;

use cdk_common::database::Error;
use cdk_common::pub_sub::{Bus, LocalDelivery, Spec};
use cdk_common::stream::{BackoffPolicy, SupervisedStream};
use cdk_sql_common::pool::DatabaseConfig;
use futures_util::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::time::timeout;
use tokio_postgres::Client;

use crate::connection::{connect_listening, Payloads};
use crate::PgConfig;

/// Backoff before the first reconnect attempt.
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

/// Upper bound for the reconnect backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Maximum `NOTIFY` payload Postgres accepts is 8000 bytes; stay under it.
const MAX_NOTIFY_PAYLOAD: usize = 7999;

/// A live Postgres client shared between the publisher and the listener.
/// `None` while disconnected; a successful connect installs it and dropping the
/// connection clears it.
type SharedClient = Arc<RwLock<Option<Arc<Client>>>>;

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

/// A connected Postgres bus, not yet bound to a subscriber set.
///
/// [`PostgresBusConnector::connect`] establishes the connection and starts
/// listening; [`PostgresBusConnector::build`] then attaches it to a
/// [`Pubsub`](cdk_common::pub_sub::Pubsub) via its [`LocalDelivery`] handle.
#[allow(missing_debug_implementations)]
pub struct PostgresBusConnector {
    connect: PgConnect,
    /// The connection [`PostgresBusConnector::connect`] opened, handed to the
    /// supervisor's first connect attempt by [`PostgresBusConnector::build`].
    /// Dropping the connector instead drops it, which closes the connection.
    notifications: Notifications,
    channel: Arc<str>,
    origin: Arc<str>,
}

impl PostgresBusConnector {
    /// Connect to Postgres and start listening on `channel`.
    ///
    /// Returns once the connection is established and the `LISTEN` is in place,
    /// so a failure to reach Postgres surfaces here rather than silently later.
    /// That same connection is handed to the supervisor by
    /// [`build`](Self::build), which keeps it alive and reconnects with backoff
    /// until the bus is dropped.
    ///
    /// `channel` must be a valid Postgres identifier (letters, digits and
    /// underscores, not starting with a digit, at most 63 bytes) because it is
    /// interpolated into the `LISTEN` statement, which cannot be parameterized.
    pub async fn connect(config: PgConfig, channel: &str) -> Result<Self, Error> {
        let channel = validate_channel(channel)?;

        let connect = PgConnect {
            config,
            listen_sql: format!("LISTEN \"{channel}\""),
            client: Arc::new(RwLock::new(None)),
        };
        let notifications = connect.open().await.map_err(Error::Internal)?;

        Ok(Self {
            connect,
            notifications,
            channel: Arc::from(channel),
            origin: Arc::from(new_origin_id().as_str()),
        })
    }

    /// Attach the bus to a subscriber set and return it as a [`Bus`].
    ///
    /// Spawns the supervised listener, which delivers peer events through
    /// `local`, skipping this instance's own echoed events, and reconnects with
    /// backoff until the returned bus is dropped.
    pub fn build<S>(self, local: LocalDelivery<S>) -> Arc<dyn Bus<S>>
    where
        S: Spec + 'static,
    {
        let PostgresBusConnector {
            connect,
            notifications,
            channel,
            origin,
        } = self;

        let (shutdown, mut supervise_shutdown) = watch::channel(());
        let client = connect.client.clone();

        let mut listener = PgListen {
            name: format!("postgres bus:{channel}"),
            connect,
            local: local.clone(),
            origin: origin.clone(),
            first: Some(notifications),
        };

        // The reconnect, backoff, and shutdown loop is provided by
        // `SupervisedStream`. It stops once the `watch::Sender` held by the
        // returned `PostgresBus` is dropped, which the receiver's `changed()`
        // observes.
        cdk_common::task::spawn(async move {
            listener
                .supervise(async move {
                    let _ = supervise_shutdown.changed().await;
                })
                .await;
        });

        Arc::new(PostgresBus {
            local,
            client,
            channel,
            origin,
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
    client: SharedClient,
    channel: Arc<str>,
    origin: Arc<str>,
    /// Dropped when the bus is dropped, which stops the background tasks.
    _shutdown: watch::Sender<()>,
}

impl<S> Bus<S> for PostgresBus<S>
where
    S: Spec + 'static,
{
    /// Serialization and local fan-out run on the spawned task, like
    /// [`LocalBus`](cdk_common::pub_sub::LocalBus), so a publishing request
    /// handler never pays for them or contends with a concurrent `subscribe`.
    fn publish(&self, event: S::Event) {
        let local = self.local.clone();
        let origin = self.origin.clone();
        let client = self.client.clone();
        let channel = self.channel.clone();

        cdk_common::task::spawn(async move {
            let payload = match serde_json::to_string(&OutEnvelope {
                origin: origin.as_ref(),
                event: &event,
            }) {
                Ok(payload) => Some(payload),
                Err(err) => {
                    tracing::warn!("postgres bus: failed to serialize event: {err}");
                    None
                }
            };

            local.deliver(event);

            let Some(payload) = payload else { return };
            if payload.len() > MAX_NOTIFY_PAYLOAD {
                tracing::warn!(
                    "postgres bus: event of {} bytes exceeds NOTIFY limit, not forwarded to peers",
                    payload.len()
                );
                return;
            }

            let client = client.read().ok().and_then(|guard| guard.clone());
            match client {
                Some(client) => {
                    let channel: &str = &channel;
                    if let Err(err) = client
                        .execute("SELECT pg_notify($1, $2)", &[&channel, &payload])
                        .await
                    {
                        tracing::warn!("postgres bus: pg_notify failed: {err}");
                    }
                }
                None => {
                    tracing::warn!("postgres bus: disconnected, event not forwarded to peers");
                }
            }
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
    /// Shared with the publisher: each successful connect installs its client
    /// here so `pg_notify` reuses the live connection.
    client: SharedClient,
}

impl PgConnect {
    /// Open a fresh connection and `LISTEN` on it, bounded by the configured
    /// connection timeout as one budget for the whole attempt. An attempt that
    /// never returns would otherwise park the supervisor and stop it from ever
    /// reconnecting.
    async fn open(&self) -> Result<Notifications, String> {
        match timeout(self.config.default_timeout(), self.listen()).await {
            Ok(result) => result,
            Err(_) => Err("timeout opening postgres bus connection".to_string()),
        }
    }

    /// `LISTEN` runs before the client is installed, so the publish path never
    /// sees a connection that is not listening. [`Notifications`] is built
    /// first, so every later exit path (a failed `LISTEN`, the caller's timeout,
    /// a shutdown that drops this future) closes the connection and leaves the
    /// client uninstalled through its `Drop`.
    async fn listen(&self) -> Result<Notifications, String> {
        let (client, payloads) = connect_listening(&self.config)
            .await
            .map_err(|err| err.to_string())?;

        let mut notifications = Notifications {
            payloads,
            client: self.client.clone(),
        };

        let client = Arc::new(client);
        notifications
            .drive(client.batch_execute(&self.listen_sql))
            .await?
            .map_err(|err| err.to_string())?;

        if let Ok(mut slot) = self.client.write() {
            *slot = Some(client);
        }

        Ok(notifications)
    }
}

/// The notification payloads of one connection.
///
/// The connection lives inside the stream, so dropping this closes it, and the
/// `Drop` also uninstalls the client so the publish path stops using a
/// connection that is gone.
#[allow(missing_debug_implementations)]
struct Notifications {
    payloads: Payloads,
    client: SharedClient,
}

impl Notifications {
    /// Await a request on this connection's client while polling the
    /// connection, which is what makes the request progress. Payloads arriving
    /// meanwhile are dropped: this only runs before the `LISTEN` is in place, so
    /// there are none to lose.
    async fn drive<T>(&mut self, request: impl Future<Output = T>) -> Result<T, String> {
        tokio::pin!(request);

        loop {
            tokio::select! {
                result = &mut request => return Ok(result),
                payload = self.next() => match payload {
                    Some(Ok(_)) => {}
                    Some(Err(err)) => return Err(err),
                    None => return Err("connection closed".to_string()),
                },
            }
        }
    }
}

impl Stream for Notifications {
    type Item = Result<String, String>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().payloads.as_mut().poll_next(cx)
    }
}

impl Drop for Notifications {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.client.write() {
            *slot = None;
        }
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
    first: Option<Notifications>,
    name: String,
}

#[async_trait::async_trait]
impl<S> SupervisedStream for PgListen<S>
where
    S: Spec + 'static,
{
    type Item = String;
    type ConnectError = String;
    type StreamError = String;
    type Stream = Notifications;

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
            Some(notifications) => Ok(notifications),
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
                tracing::warn!("postgres bus: dropping malformed payload: {err}");
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
    async fn pg_pubsub<S>(channel: &str) -> Pubsub<S>
    where
        S: Spec<Context = ()> + 'static,
    {
        let connector =
            PostgresBusConnector::connect(PgConfig::from(test_db_url().as_str()), channel)
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

    /// A failed `LISTEN` on an otherwise healthy connection must surface as a
    /// failed connect, so `SupervisedStream` backs off and reconnects rather
    /// than serving a connection that is up but not listening. It must also
    /// leave the client uninstalled, so the publish path never uses a
    /// non-listening connection.
    ///
    /// The failure is forced with an invalid `LISTEN` statement: the connection
    /// opens fine, but the statement errors on the server.
    #[tokio::test]
    async fn listen_failure_errors_connect_and_leaves_client_uninstalled() {
        let client_slot: SharedClient = Arc::new(RwLock::new(None));

        let connect = PgConnect {
            config: PgConfig::from(test_db_url().as_str()),
            // Missing channel name: opens a healthy connection, then errors.
            listen_sql: "LISTEN".to_string(),
            client: client_slot.clone(),
        };

        let result = connect.open().await;

        assert!(
            result.is_err(),
            "a failed LISTEN must surface as a connect error"
        );
        // The client is never installed, so publishers do not use a
        // non-listening connection.
        assert!(client_slot.read().unwrap().is_none());
    }

    /// A `LISTEN` that never returns must fail the attempt on the configured
    /// timeout. Without it the supervisor parks inside `connect` and the bus
    /// never reconnects.
    ///
    /// The hang is forced with a statement that sleeps on the server far longer
    /// than the one-second timeout the config carries.
    #[tokio::test]
    async fn listen_hang_times_out_connect_and_leaves_client_uninstalled() {
        let client_slot: SharedClient = Arc::new(RwLock::new(None));

        let connect = PgConnect {
            config: PgConfig::new(&test_db_url(), None, None, Some(1)),
            listen_sql: "SELECT pg_sleep(30)".to_string(),
            client: client_slot.clone(),
        };

        let started = std::time::Instant::now();
        let result = connect.open().await;

        assert!(
            result.is_err(),
            "a hung LISTEN must surface as a connect error"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "connect must give up on the configured timeout, not wait out the statement"
        );
        assert!(client_slot.read().unwrap().is_none());
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

        let client_slot: SharedClient = Arc::new(RwLock::new(None));

        let connect = PgConnect {
            config: PgConfig::new(
                &format!("host=127.0.0.1 port={port} user=cdk_user dbname=cdk_mint"),
                None,
                None,
                Some(1),
            ),
            listen_sql: "LISTEN \"cdk_bus_test\"".to_string(),
            client: client_slot.clone(),
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
        assert!(client_slot.read().unwrap().is_none());
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
    /// only the forward to peers is skipped. The bus is built by hand with no
    /// live connection, so the oversized branch returns before it would touch
    /// the absent client, and the local delivery is all that is observed.
    #[tokio::test]
    async fn oversized_event_is_still_delivered_locally() {
        use blob::{BlobEvent, BlobSpec, Sub};

        let (shutdown, _shutdown_rx) = watch::channel(());
        let pubsub = Pubsub::new_with_bus(BlobSpec::new_instance(()), move |local| {
            Arc::new(PostgresBus {
                local,
                client: Arc::new(RwLock::new(None)),
                channel: Arc::from("test"),
                origin: Arc::from("origin"),
                _shutdown: shutdown,
            })
        });

        let mut subscriber = pubsub.subscribe(Sub(1)).unwrap();

        let event = BlobEvent {
            topic: 1,
            blob: "x".repeat(MAX_NOTIFY_PAYLOAD + 1),
        };
        // The serialized envelope really exceeds the forwarding cap, so this
        // exercises the skip-forward branch rather than a normal publish.
        let payload = serde_json::to_string(&OutEnvelope {
            origin: "origin",
            event: &event,
        })
        .expect("serialize");
        assert!(payload.len() > MAX_NOTIFY_PAYLOAD);

        pubsub.publish(event);

        let received = timeout(Duration::from_secs(1), subscriber.recv())
            .await
            .expect("oversized event delivered locally before timeout");
        assert_eq!(received.map(|e| e.blob.len()), Some(MAX_NOTIFY_PAYLOAD + 1));
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
