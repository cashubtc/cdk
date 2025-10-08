//! Pub-sub consumer
//!
//! Consumers are designed to connect to a producer, through a transport, and subscribe to events.
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tokio::sync::mpsc;
use tokio::time::{sleep, Instant};

use super::subscriber::{ActiveSubscription, SubscriptionRequest};
use super::{Error, Event, Pubsub, Spec};

const STREAM_CONNECTION_BACKOFF: Duration = Duration::from_millis(2_000);

const STREAM_CONNECTION_MAX_BACKOFF: Duration = Duration::from_millis(30_000);

const INTERNAL_POLL_SIZE: usize = 1_000;

const POLL_SLEEP: Duration = Duration::from_millis(2_000);

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures;

struct UniqueSubscription<S>
where
    S: Spec,
{
    name: S::SubscriptionId,
    total_subscribers: usize,
}

type UniqueSubscriptions<S> = RwLock<HashMap<<S as Spec>::Topic, UniqueSubscription<S>>>;

type ActiveSubscriptions<S> =
    RwLock<HashMap<Arc<<S as Spec>::SubscriptionId>, Vec<<S as Spec>::Topic>>>;

type CacheEvent<S> = HashMap<<<S as Spec>::Event as Event>::Topic, <S as Spec>::Event>;

/// Subscription consumer
pub struct Consumer<T>
where
    T: Transport + 'static,
{
    transport: T,
    inner_pubsub: Arc<Pubsub<T::Spec>>,
    remote_subscriptions: UniqueSubscriptions<T::Spec>,
    subscriptions: ActiveSubscriptions<T::Spec>,
    stream_ctrl: RwLock<Option<mpsc::Sender<StreamCtrl<T::Spec>>>>,
    still_running: AtomicBool,
    prefer_polling: bool,
    /// Cached events
    ///
    /// The cached events are useful to share events. The cache is automatically evicted it is
    /// disconnected from the remote source, meaning the cache is only active while there is an
    /// active subscription to the remote source, and it remembers the latest event.
    cached_events: Arc<RwLock<CacheEvent<T::Spec>>>,
}

/// Remote consumer
pub struct RemoteActiveConsumer<T>
where
    T: Transport + 'static,
{
    inner: ActiveSubscription<T::Spec>,
    previous_messages: VecDeque<<T::Spec as Spec>::Event>,
    consumer: Arc<Consumer<T>>,
}

impl<T> RemoteActiveConsumer<T>
where
    T: Transport + 'static,
{
    /// Receives the next event
    pub async fn recv(&mut self) -> Option<<T::Spec as Spec>::Event> {
        if let Some(event) = self.previous_messages.pop_front() {
            Some(event)
        } else {
            self.inner.recv().await
        }
    }

    /// Try receive an event or return Noen right away
    pub fn try_recv(&mut self) -> Option<<T::Spec as Spec>::Event> {
        if let Some(event) = self.previous_messages.pop_front() {
            Some(event)
        } else {
            self.inner.try_recv()
        }
    }

    /// Get the subscription name
    pub fn name(&self) -> &<T::Spec as Spec>::SubscriptionId {
        self.inner.name()
    }
}

impl<T> Drop for RemoteActiveConsumer<T>
where
    T: Transport + 'static,
{
    fn drop(&mut self) {
        let _ = self.consumer.unsubscribe(self.name().clone());
    }
}

/// Struct to relay events from Poll and Streams from the external subscription to the local
/// subscribers
pub struct InternalRelay<S>
where
    S: Spec + 'static,
{
    inner: Arc<Pubsub<S>>,
    cached_events: Arc<RwLock<CacheEvent<S>>>,
}

impl<S> InternalRelay<S>
where
    S: Spec + 'static,
{
    /// Relay a remote event locally
    pub fn send<X>(&self, event: X)
    where
        X: Into<S::Event>,
    {
        let event = event.into();
        let mut cached_events = self.cached_events.write();

        for topic in event.get_topics() {
            cached_events.insert(topic, event.clone());
        }

        self.inner.publish(event);
    }
}

impl<T> Consumer<T>
where
    T: Transport + 'static,
{
    /// Creates a new instance
    pub fn new(
        transport: T,
        prefer_polling: bool,
        context: <T::Spec as Spec>::Context,
    ) -> Arc<Self> {
        let this = Arc::new(Self {
            transport,
            prefer_polling,
            inner_pubsub: Arc::new(Pubsub::new(T::Spec::new_instance(context))),
            subscriptions: Default::default(),
            remote_subscriptions: Default::default(),
            stream_ctrl: RwLock::new(None),
            cached_events: Default::default(),
            still_running: true.into(),
        });

        #[cfg(not(target_arch = "wasm32"))]
        tokio::spawn(Self::stream(this.clone()));

        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(Self::stream(this.clone()));

        this
    }

    async fn stream(instance: Arc<Self>) {
        let mut stream_supported = true;
        let mut poll_supported = true;

        let mut backoff = STREAM_CONNECTION_BACKOFF;
        let mut retry_at = None;

        loop {
            if (!stream_supported && !poll_supported)
                || !instance
                    .still_running
                    .load(std::sync::atomic::Ordering::Relaxed)
            {
                break;
            }

            if instance.remote_subscriptions.read().is_empty() {
                sleep(Duration::from_millis(100)).await;
                continue;
            }

            if stream_supported
                && !instance.prefer_polling
                && retry_at
                    .map(|retry_at| retry_at < Instant::now())
                    .unwrap_or(true)
            {
                let (sender, receiver) = mpsc::channel(INTERNAL_POLL_SIZE);

                {
                    *instance.stream_ctrl.write() = Some(sender);
                }

                let current_subscriptions = {
                    instance
                        .remote_subscriptions
                        .read()
                        .iter()
                        .map(|(key, name)| (name.name.clone(), key.clone()))
                        .collect::<Vec<_>>()
                };

                if let Err(err) = instance
                    .transport
                    .stream(
                        receiver,
                        current_subscriptions,
                        InternalRelay {
                            inner: instance.inner_pubsub.clone(),
                            cached_events: instance.cached_events.clone(),
                        },
                    )
                    .await
                {
                    retry_at = Some(Instant::now() + backoff);
                    backoff =
                        (backoff + STREAM_CONNECTION_BACKOFF).min(STREAM_CONNECTION_MAX_BACKOFF);

                    if matches!(err, Error::NotSupported) {
                        stream_supported = false;
                    }
                    tracing::error!("Long connection failed with error {:?}", err);
                } else {
                    backoff = STREAM_CONNECTION_BACKOFF;
                }

                // remove sender to stream, as there is no stream
                let _ = instance.stream_ctrl.write().take();
            }

            if poll_supported {
                let current_subscriptions = {
                    instance
                        .remote_subscriptions
                        .read()
                        .iter()
                        .map(|(key, name)| (name.name.clone(), key.clone()))
                        .collect::<Vec<_>>()
                };

                if let Err(err) = instance
                    .transport
                    .poll(
                        current_subscriptions,
                        InternalRelay {
                            inner: instance.inner_pubsub.clone(),
                            cached_events: instance.cached_events.clone(),
                        },
                    )
                    .await
                {
                    if matches!(err, Error::NotSupported) {
                        poll_supported = false;
                    }
                    tracing::error!("Polling failed with error {:?}", err);
                }

                sleep(POLL_SLEEP).await;
            }
        }
    }

    /// Unsubscribe from a topic, this is called automatically when RemoteActiveSubscription<T> goes
    /// out of scope
    fn unsubscribe(
        self: &Arc<Self>,
        subscription_name: <T::Spec as Spec>::SubscriptionId,
    ) -> Result<(), Error> {
        let topics = self
            .subscriptions
            .write()
            .remove(&subscription_name)
            .ok_or(Error::NoSubscription)?;

        let mut remote_subscriptions = self.remote_subscriptions.write();

        for topic in topics {
            let mut remote_subscription =
                if let Some(remote_subscription) = remote_subscriptions.remove(&topic) {
                    remote_subscription
                } else {
                    continue;
                };

            remote_subscription.total_subscribers = remote_subscription
                .total_subscribers
                .checked_sub(1)
                .unwrap_or_default();

            if remote_subscription.total_subscribers == 0 {
                let mut cached_events = self.cached_events.write();

                cached_events.remove(&topic);

                self.message_to_stream(StreamCtrl::Unsubscribe(remote_subscription.name.clone()))?;
            } else {
                remote_subscriptions.insert(topic, remote_subscription);
            }
        }

        if remote_subscriptions.is_empty() {
            self.message_to_stream(StreamCtrl::Stop)?;
        }

        Ok(())
    }

    #[inline(always)]
    fn message_to_stream(&self, message: StreamCtrl<T::Spec>) -> Result<(), Error> {
        let to_stream = self.stream_ctrl.read();

        if let Some(to_stream) = to_stream.as_ref() {
            Ok(to_stream.try_send(message)?)
        } else {
            Ok(())
        }
    }

    /// Creates a subscription
    ///
    /// The subscriptions have two parts:
    ///
    /// 1. Will create the subscription to the remote Pubsub service, Any events will be moved to
    ///    the internal pubsub
    ///
    /// 2. The internal subscription to the inner Pubsub. Because all subscriptions are going the
    ///    transport, once events matches subscriptions, the inner_pubsub will receive the message and
    ///    broadcasat the event.
    pub fn subscribe<I>(self: &Arc<Self>, request: I) -> Result<RemoteActiveConsumer<T>, Error>
    where
        I: SubscriptionRequest<
            Topic = <T::Spec as Spec>::Topic,
            SubscriptionId = <T::Spec as Spec>::SubscriptionId,
        >,
    {
        let subscription_name = request.subscription_name();
        let topics = request.try_get_topics()?;

        let mut remote_subscriptions = self.remote_subscriptions.write();
        let mut subscriptions = self.subscriptions.write();

        if subscriptions.get(&subscription_name).is_some() {
            return Err(Error::NoSubscription);
        }

        let mut previous_messages = Vec::new();
        let cached_events = self.cached_events.read();

        for topic in topics.iter() {
            if let Some(subscription) = remote_subscriptions.get_mut(topic) {
                subscription.total_subscribers += 1;

                if let Some(v) = cached_events.get(topic).cloned() {
                    previous_messages.push(v);
                }
            } else {
                let internal_sub_name = self.transport.new_name();
                remote_subscriptions.insert(
                    topic.clone(),
                    UniqueSubscription {
                        total_subscribers: 1,
                        name: internal_sub_name.clone(),
                    },
                );

                // new subscription is created, so the connection worker should be notified
                self.message_to_stream(StreamCtrl::Subscribe((internal_sub_name, topic.clone())))?;
            }
        }

        subscriptions.insert(subscription_name, topics);
        drop(subscriptions);

        Ok(RemoteActiveConsumer {
            inner: self.inner_pubsub.subscribe(request)?,
            previous_messages: previous_messages.into(),
            consumer: self.clone(),
        })
    }
}

impl<T> Drop for Consumer<T>
where
    T: Transport + 'static,
{
    fn drop(&mut self) {
        self.still_running
            .store(false, std::sync::atomic::Ordering::Release);
        if let Some(to_stream) = self.stream_ctrl.read().as_ref() {
            let _ = to_stream.try_send(StreamCtrl::Stop).inspect_err(|err| {
                tracing::error!("Failed to send message LongPoll::Stop due to {err:?}")
            });
        }
    }
}

/// Subscribe Message
pub type SubscribeMessage<S> = (<S as Spec>::SubscriptionId, <S as Spec>::Topic);

/// Messages sent from the [`Consumer`] to the [`Transport`] background loop.
pub enum StreamCtrl<S>
where
    S: Spec + 'static,
{
    /// Add a subscription
    Subscribe(SubscribeMessage<S>),
    /// Desuscribe
    Unsubscribe(S::SubscriptionId),
    /// Exit the loop
    Stop,
}

impl<S> Clone for StreamCtrl<S>
where
    S: Spec + 'static,
{
    fn clone(&self) -> Self {
        match self {
            Self::Subscribe(s) => Self::Subscribe(s.clone()),
            Self::Unsubscribe(u) => Self::Unsubscribe(u.clone()),
            Self::Stop => Self::Stop,
        }
    }
}

/// Transport abstracts how the consumer talks to the remote pubsub.
///
/// Implement this on your HTTP/WebSocket client. The transport is responsible for:
/// - creating unique subscription names,
/// - keeping a long connection via `stream` **or** performing on-demand `poll`,
/// - forwarding remote events to `InternalRelay`.
///
/// ```ignore
/// struct WsTransport { /* ... */ }
/// #[async_trait::async_trait]
/// impl Transport for WsTransport {
///     type Topic = MyTopic;
///     fn new_name(&self) -> <Self::Topic as Topic>::SubscriptionName { 0 }
///     async fn stream(/* ... */) -> Result<(), Error> { Ok(()) }
///     async fn poll(/* ... */) -> Result<(), Error> { Ok(()) }
/// }
/// ```
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait Transport: Send + Sync {
    /// Spec
    type Spec: Spec;

    /// Create a new subscription name
    fn new_name(&self) -> <Self::Spec as Spec>::SubscriptionId;

    /// Opens a persistent connection and continuously streams events.
    /// For protocols that support server push (e.g. WebSocket, SSE).
    async fn stream(
        &self,
        subscribe_changes: mpsc::Receiver<StreamCtrl<Self::Spec>>,
        topics: Vec<SubscribeMessage<Self::Spec>>,
        reply_to: InternalRelay<Self::Spec>,
    ) -> Result<(), Error>;

    /// Performs a one-shot fetch of any currently available events.
    /// Called repeatedly by the consumer when streaming is not available.
    async fn poll(
        &self,
        topics: Vec<SubscribeMessage<Self::Spec>>,
        reply_to: InternalRelay<Self::Spec>,
    ) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use tokio::sync::{mpsc, Mutex};
    use tokio::time::{timeout, Duration};

    use super::{
        InternalRelay, RemoteActiveConsumer, StreamCtrl, SubscribeMessage, Transport,
        INTERNAL_POLL_SIZE,
    };
    use crate::pub_sub::remote_consumer::Consumer;
    use crate::pub_sub::test::{CustomPubSub, IndexTest, Message};
    use crate::pub_sub::{Error, Spec, SubscriptionRequest};

    // ===== Test Event/Topic types =====

    #[derive(Clone, Debug)]
    enum SubscriptionReq {
        Foo(String, u64),
        Bar(String, u64),
    }

    impl SubscriptionRequest for SubscriptionReq {
        type Topic = IndexTest;

        type SubscriptionId = String;

        fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
            Ok(vec![match self {
                SubscriptionReq::Foo(_, n) => IndexTest::Foo(*n),
                SubscriptionReq::Bar(_, n) => IndexTest::Bar(*n),
            }])
        }

        fn subscription_name(&self) -> Arc<Self::SubscriptionId> {
            Arc::new(match self {
                SubscriptionReq::Foo(n, _) => n.to_string(),
                SubscriptionReq::Bar(n, _) => n.to_string(),
            })
        }
    }

    // ===== A controllable in-memory Transport used by tests =====

    /// TestTransport relays messages from a broadcast channel to the Consumer via `InternalRelay`.
    /// It also forwards Subscribe/Unsubscribe/Stop signals to an observer channel so tests can assert them.
    struct TestTransport {
        name_ctr: AtomicUsize,
        // We forward all transport-loop control messages here so tests can observe them.
        observe_ctrl_tx: mpsc::Sender<StreamCtrl<CustomPubSub>>,
        // Whether stream / poll are supported.
        support_long: bool,
        support_poll: bool,
        rx: Mutex<mpsc::Receiver<Message>>,
    }

    impl TestTransport {
        fn new(
            support_long: bool,
            support_poll: bool,
        ) -> (
            Self,
            mpsc::Sender<Message>,
            mpsc::Receiver<StreamCtrl<CustomPubSub>>,
        ) {
            let (events_tx, rx) = mpsc::channel::<Message>(INTERNAL_POLL_SIZE);
            let (observe_ctrl_tx, observe_ctrl_rx) =
                mpsc::channel::<StreamCtrl<_>>(INTERNAL_POLL_SIZE);

            let t = TestTransport {
                name_ctr: AtomicUsize::new(1),
                rx: Mutex::new(rx),
                observe_ctrl_tx,
                support_long,
                support_poll,
            };

            (t, events_tx, observe_ctrl_rx)
        }
    }

    #[async_trait::async_trait]
    impl Transport for TestTransport {
        type Spec = CustomPubSub;

        fn new_name(&self) -> <Self::Spec as Spec>::SubscriptionId {
            format!("sub-{}", self.name_ctr.fetch_add(1, Ordering::Relaxed))
        }

        async fn stream(
            &self,
            mut subscribe_changes: mpsc::Receiver<StreamCtrl<Self::Spec>>,
            topics: Vec<SubscribeMessage<Self::Spec>>,
            reply_to: InternalRelay<Self::Spec>,
        ) -> Result<(), Error> {
            if !self.support_long {
                return Err(Error::NotSupported);
            }

            // Each invocation creates a fresh broadcast receiver
            let mut rx = self.rx.lock().await;
            let observe = self.observe_ctrl_tx.clone();

            for topic in topics {
                observe.try_send(StreamCtrl::Subscribe(topic)).unwrap();
            }

            loop {
                tokio::select! {
                    // Forward any control (Subscribe/Unsubscribe/Stop) messages so the test can assert them.
                    Some(ctrl) = subscribe_changes.recv() => {
                        observe.try_send(ctrl.clone()).unwrap();
                        if matches!(ctrl, StreamCtrl::Stop) {
                            break;
                        }
                    }
                    // Relay external events into the inner pubsub
                    Some(msg) = rx.recv() => {
                        reply_to.send(msg);
                    }
                }
            }

            Ok(())
        }

        async fn poll(
            &self,
            _topics: Vec<SubscribeMessage<Self::Spec>>,
            reply_to: InternalRelay<Self::Spec>,
        ) -> Result<(), Error> {
            if !self.support_poll {
                return Err(Error::NotSupported);
            }

            // On each poll call, drain anything currently pending and return.
            // (The Consumer calls this repeatedly; first call happens immediately.)
            let mut rx = self.rx.lock().await;
            // Non-blocking drain pass: try a few times without sleeping to keep tests snappy
            for _ in 0..32 {
                match rx.try_recv() {
                    Ok(msg) => reply_to.send(msg),
                    Err(mpsc::error::TryRecvError::Empty) => continue,
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                }
            }
            Ok(())
        }
    }

    // ===== Helpers =====

    async fn recv_next<T: Transport>(
        sub: &mut RemoteActiveConsumer<T>,
        dur_ms: u64,
    ) -> Option<<T::Spec as Spec>::Event> {
        timeout(Duration::from_millis(dur_ms), sub.recv())
            .await
            .ok()
            .flatten()
    }

    async fn expect_ctrl(
        rx: &mut mpsc::Receiver<StreamCtrl<CustomPubSub>>,
        dur_ms: u64,
        pred: impl Fn(&StreamCtrl<CustomPubSub>) -> bool,
    ) -> StreamCtrl<CustomPubSub> {
        timeout(Duration::from_millis(dur_ms), async {
            loop {
                if let Some(msg) = rx.recv().await {
                    if pred(&msg) {
                        break msg;
                    }
                }
            }
        })
        .await
        .expect("timed out waiting for control message")
    }

    // ===== Tests =====

    #[tokio::test]
    async fn stream_delivery_and_unsubscribe_on_drop() {
        // stream supported, poll supported (doesn't matter; prefer long)
        let (transport, events_tx, mut ctrl_rx) = TestTransport::new(true, true);

        // prefer_polling = false so connection loop will try stream first.
        let consumer = Consumer::new(transport, false, ());

        // Subscribe to Foo(7)
        let mut sub = consumer
            .subscribe(SubscriptionReq::Foo("t".to_owned(), 7))
            .expect("subscribe ok");

        // We should see a Subscribe(name, topic) forwarded to transport
        let ctrl = expect_ctrl(
            &mut ctrl_rx,
            1000,
            |m| matches!(m, StreamCtrl::Subscribe((_, idx)) if *idx == IndexTest::Foo(7)),
        )
        .await;
        match ctrl {
            StreamCtrl::Subscribe((name, idx)) => {
                assert_ne!(name, "t".to_owned());
                assert_eq!(idx, IndexTest::Foo(7));
            }
            _ => unreachable!(),
        }

        // Send an event that matches Foo(7)
        events_tx.send(Message { foo: 7, bar: 1 }).await.unwrap();
        let got = recv_next::<TestTransport>(&mut sub, 1000)
            .await
            .expect("got event");
        assert_eq!(got, Message { foo: 7, bar: 1 });

        // Dropping the RemoteActiveConsumer should trigger an Unsubscribe(name)
        drop(sub);
        let _ctrl = expect_ctrl(&mut ctrl_rx, 1000, |m| {
            matches!(m, StreamCtrl::Unsubscribe(_))
        })
        .await;

        // Drop the Consumer -> Stop is sent so the transport loop exits cleanly
        drop(consumer);
        let _ = expect_ctrl(&mut ctrl_rx, 1000, |m| matches!(m, StreamCtrl::Stop)).await;
    }

    #[tokio::test]
    async fn test_cache_and_invalation() {
        // stream supported, poll supported (doesn't matter; prefer long)
        let (transport, events_tx, mut ctrl_rx) = TestTransport::new(true, true);

        // prefer_polling = false so connection loop will try stream first.
        let consumer = Consumer::new(transport, false, ());

        // Subscribe to Foo(7)
        let mut sub_1 = consumer
            .subscribe(SubscriptionReq::Foo("t".to_owned(), 7))
            .expect("subscribe ok");

        // We should see a Subscribe(name, topic) forwarded to transport
        let ctrl = expect_ctrl(
            &mut ctrl_rx,
            1000,
            |m| matches!(m, StreamCtrl::Subscribe((_, idx)) if *idx == IndexTest::Foo(7)),
        )
        .await;
        match ctrl {
            StreamCtrl::Subscribe((name, idx)) => {
                assert_ne!(name, "t1".to_owned());
                assert_eq!(idx, IndexTest::Foo(7));
            }
            _ => unreachable!(),
        }

        // Send an event that matches Foo(7)
        events_tx.send(Message { foo: 7, bar: 1 }).await.unwrap();
        let got = recv_next::<TestTransport>(&mut sub_1, 1000)
            .await
            .expect("got event");
        assert_eq!(got, Message { foo: 7, bar: 1 });

        // Subscribe to Foo(7), should receive the latest message and future messages
        let mut sub_2 = consumer
            .subscribe(SubscriptionReq::Foo("t2".to_owned(), 7))
            .expect("subscribe ok");

        let got = recv_next::<TestTransport>(&mut sub_2, 1000)
            .await
            .expect("got event");
        assert_eq!(got, Message { foo: 7, bar: 1 });

        // Dropping the RemoteActiveConsumer but not unsubscribe, since sub_2 is still active
        drop(sub_1);

        // Subscribe to Foo(7), should receive the latest message and future messages
        let mut sub_3 = consumer
            .subscribe(SubscriptionReq::Foo("t3".to_owned(), 7))
            .expect("subscribe ok");

        // receive cache message
        let got = recv_next::<TestTransport>(&mut sub_3, 1000)
            .await
            .expect("got event");
        assert_eq!(got, Message { foo: 7, bar: 1 });

        // Send an event that matches Foo(7)
        events_tx.send(Message { foo: 7, bar: 2 }).await.unwrap();

        // receive new message
        let got = recv_next::<TestTransport>(&mut sub_2, 1000)
            .await
            .expect("got event");
        assert_eq!(got, Message { foo: 7, bar: 2 });

        let got = recv_next::<TestTransport>(&mut sub_3, 1000)
            .await
            .expect("got event");
        assert_eq!(got, Message { foo: 7, bar: 2 });

        drop(sub_2);
        drop(sub_3);

        let _ctrl = expect_ctrl(&mut ctrl_rx, 1000, |m| {
            matches!(m, StreamCtrl::Unsubscribe(_))
        })
        .await;

        // The cache should be dropped, so no new messages
        let mut sub_4 = consumer
            .subscribe(SubscriptionReq::Foo("t4".to_owned(), 7))
            .expect("subscribe ok");

        assert!(
            recv_next::<TestTransport>(&mut sub_4, 1000).await.is_none(),
            "Should have not receive any update"
        );

        drop(sub_4);

        // Drop the Consumer -> Stop is sent so the transport loop exits cleanly
        let _ = expect_ctrl(&mut ctrl_rx, 2000, |m| matches!(m, StreamCtrl::Stop)).await;
    }

    #[tokio::test]
    async fn falls_back_to_poll_when_stream_not_supported() {
        // stream NOT supported, poll supported
        let (transport, events_tx, _) = TestTransport::new(false, true);
        // prefer_polling = true nudges the connection loop to poll first, but even if it
        // tried stream, our transport returns NotSupported and the loop will use poll.
        let consumer = Consumer::new(transport, true, ());

        // Subscribe to Bar(5)
        let mut sub = consumer
            .subscribe(SubscriptionReq::Bar("t".to_owned(), 5))
            .expect("subscribe ok");

        // Inject an event; the poll path should relay it on the first poll iteration
        events_tx.send(Message { foo: 9, bar: 5 }).await.unwrap();
        let got = recv_next::<TestTransport>(&mut sub, 1500)
            .await
            .expect("event relayed via polling");
        assert_eq!(got, Message { foo: 9, bar: 5 });
    }

    #[tokio::test]
    async fn multiple_subscribers_share_single_remote_subscription() {
        // This validates the "coalescing" behavior in Consumer::subscribe where multiple local
        // subscribers to the same Topic should only create one remote subscription.
        let (transport, events_tx, mut ctrl_rx) = TestTransport::new(true, true);
        let consumer = Consumer::new(transport, false, ());

        // Two local subscriptions to the SAME topic/name pair (different names)
        let mut a = consumer
            .subscribe(SubscriptionReq::Foo("t".to_owned(), 1))
            .expect("subscribe A");
        let _ = expect_ctrl(
            &mut ctrl_rx,
            1000,
            |m| matches!(m, StreamCtrl::Subscribe((_, idx)) if  *idx == IndexTest::Foo(1)),
        )
        .await;

        let mut b = consumer
            .subscribe(SubscriptionReq::Foo("b".to_owned(), 1))
            .expect("subscribe B");

        // No second Subscribe should be forwarded for the same topic (coalesced).
        // Give a little time; if one appears, we'll fail explicitly.
        if let Ok(Some(StreamCtrl::Subscribe((_, idx)))) =
            timeout(Duration::from_millis(400), ctrl_rx.recv()).await
        {
            assert_ne!(idx, IndexTest::Foo(1), "should not resubscribe same topic");
        }

        // Send one event and ensure BOTH local subscribers receive it.
        events_tx.send(Message { foo: 1, bar: 42 }).await.unwrap();
        let got_a = recv_next::<TestTransport>(&mut a, 1000)
            .await
            .expect("A got");
        let got_b = recv_next::<TestTransport>(&mut b, 1000)
            .await
            .expect("B got");
        assert_eq!(got_a, Message { foo: 1, bar: 42 });
        assert_eq!(got_b, Message { foo: 1, bar: 42 });

        // Drop B: no Unsubscribe should be sent yet (still one local subscriber).
        drop(b);
        if let Ok(Some(StreamCtrl::Unsubscribe(_))) =
            timeout(Duration::from_millis(400), ctrl_rx.recv()).await
        {
            panic!("Should NOT unsubscribe while another local subscriber exists");
        }

        // Drop A: now remote unsubscribe should occur.
        drop(a);
        let _ = expect_ctrl(&mut ctrl_rx, 1000, |m| {
            matches!(m, StreamCtrl::Unsubscribe(_))
        })
        .await;

        let _ = expect_ctrl(&mut ctrl_rx, 1000, |m| matches!(m, StreamCtrl::Stop)).await;
    }
}
