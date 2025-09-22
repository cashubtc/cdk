//! Pub-sub consumer
//!
//! Consumers are designed to connect to a producer, through a transport, and subscribe to events.
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::sleep;

use super::pubsub::Topic;
use super::subscriber::{ActiveSubscription, SubscriptionRequest};
use super::{Error, Event, Pubsub};

struct UniqueSubscription<T>
where
    T: Topic,
{
    name: T::SubscriptionName,
    total_subscribers: usize,
}

type UniqueSubscriptions<T> =
    RwLock<HashMap<<<T as Topic>::Event as Event>::Topic, UniqueSubscription<T>>>;

type ActiveSubscriptions<T> =
    RwLock<HashMap<<T as Topic>::SubscriptionName, Vec<<<T as Topic>::Event as Event>::Topic>>>;

const LONG_CONNECTION_SLEEP_MS: u64 = 10;
const POLL_SLEEP_MS: u64 = 2000;

/// Subscription consumer
pub struct Consumer<T>
where
    T: Transport + 'static,
{
    transport: T,
    inner_pubsub: Arc<Pubsub<T::Topic>>,
    remote_subscriptions: UniqueSubscriptions<T::Topic>,
    subscriptions: ActiveSubscriptions<T::Topic>,
    send_to_transport_loop: RwLock<mpsc::Sender<MessageToTransportLoop<T::Topic>>>,
    still_running: AtomicBool,
    prefer_http: bool,
}

/// Remote consumer
pub struct RemoteActiveConsumer<T>
where
    T: Transport + 'static,
{
    inner: ActiveSubscription<T::Topic>,
    consumer: Arc<Consumer<T>>,
}

impl<T> Drop for RemoteActiveConsumer<T>
where
    T: Transport + 'static,
{
    fn drop(&mut self) {
        let _ = self.consumer.unsubscribe(self.name().clone());
    }
}

impl<T> Deref for RemoteActiveConsumer<T>
where
    T: Transport + 'static,
{
    type Target = ActiveSubscription<T::Topic>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for RemoteActiveConsumer<T>
where
    T: Transport + 'static,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

struct InternalConversion<T>
where
    T: Transport + 'static,
{
    name: <T::Topic as Topic>::SubscriptionName,
    index: <<T::Topic as Topic>::Event as Event>::Topic,
}

impl<T> Clone for InternalConversion<T>
where
    T: Transport + 'static,
{
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            index: self.index.clone(),
        }
    }
}

impl<T> SubscriptionRequest for InternalConversion<T>
where
    T: Transport + 'static,
{
    type Topic = <<T::Topic as Topic>::Event as Event>::Topic;

    type SubscriptionName = <T::Topic as Topic>::SubscriptionName;

    fn subscription_name(&self) -> Self::SubscriptionName {
        self.name.to_owned()
    }

    fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
        Ok(vec![self.index.clone()])
    }
}

impl<T> Consumer<T>
where
    T: Transport + 'static,
{
    /// Creates a new instance
    pub fn new(transport: T, prefer_http: bool, inner_pubsub: Pubsub<T::Topic>) -> Arc<Self> {
        let this = Arc::new(Self {
            transport,
            prefer_http,
            inner_pubsub: Arc::new(inner_pubsub),
            subscriptions: Default::default(),
            remote_subscriptions: Default::default(),
            send_to_transport_loop: RwLock::new(mpsc::channel(10_000).0),
            still_running: true.into(),
        });

        tokio::spawn(Self::connection_loop(this.clone()));

        this
    }

    async fn connection_loop(instance: Arc<Self>) {
        let mut long_connection_supported = true;
        let mut poll_supported = true;

        loop {
            if (!long_connection_supported && !poll_supported)
                || !instance
                    .still_running
                    .load(std::sync::atomic::Ordering::Relaxed)
            {
                break;
            }

            if long_connection_supported && !instance.prefer_http {
                let (sender, receiver) = mpsc::channel(10_000);

                {
                    let mut shared_sender = instance.send_to_transport_loop.write().unwrap();
                    *shared_sender = sender;
                }

                let current_subscriptions = {
                    instance
                        .remote_subscriptions
                        .read()
                        .expect("xxx")
                        .keys()
                        .map(|x| x.clone())
                        .collect::<Vec<_>>()
                };

                if let Err(err) = instance
                    .transport
                    .long_connection(receiver, current_subscriptions)
                    .await
                {
                    if matches!(err, Error::NotSupported) {
                        long_connection_supported = false;
                    }
                    tracing::error!("Long connection failed with error {:?}", err);
                }
                sleep(Duration::from_millis(LONG_CONNECTION_SLEEP_MS)).await;
            }

            if poll_supported {
                let current_subscriptions = {
                    instance
                        .remote_subscriptions
                        .read()
                        .expect("xxx")
                        .keys()
                        .map(|x| x.clone())
                        .collect::<Vec<_>>()
                };

                if let Err(err) = instance
                    .transport
                    .poll(current_subscriptions, instance.inner_pubsub.clone())
                    .await
                {
                    if matches!(err, Error::NotSupported) {
                        poll_supported = false;
                    }
                    tracing::error!("Polling failed with error {:?}", err);
                }

                sleep(Duration::from_millis(POLL_SLEEP_MS)).await;
            }
        }
    }

    /// Unsubscribe from a topic, this is called automatically when RemoteActiveSubscription<T> goes out of scope
    fn unsubscribe(
        self: &Arc<Self>,
        subscription_name: <T::Topic as Topic>::SubscriptionName,
    ) -> Result<(), Error> {
        let topics = self
            .subscriptions
            .write()
            .map_err(|_| Error::Poison)?
            .remove(&subscription_name)
            .ok_or(Error::AlreadySubscribed)?;

        let mut remote_subscriptions = self
            .remote_subscriptions
            .write()
            .map_err(|_| Error::Poison)?;

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
                let _ = self
                    .send_to_transport_loop
                    .read()
                    .map_err(|_| Error::Poison)?
                    .try_send(MessageToTransportLoop::Unsubscribe(
                        remote_subscription.name.clone(),
                    ));
            } else {
                remote_subscriptions.insert(topic, remote_subscription);
            }
        }

        Ok(())
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
            Topic = <<T::Topic as Topic>::Event as Event>::Topic,
            SubscriptionName = <T::Topic as Topic>::SubscriptionName,
        >,
    {
        let subscription_name = request.subscription_name();
        let indexes = request.try_get_topics()?;

        let mut remote_subscriptions = self
            .remote_subscriptions
            .write()
            .map_err(|_| Error::Poison)?;
        let mut subscriptions = self.subscriptions.write().map_err(|_| Error::Poison)?;

        if subscriptions.get(&subscription_name).is_some() {
            return Err(Error::AlreadySubscribed);
        }

        for index in indexes.iter() {
            if let Some(subscription) = remote_subscriptions.get_mut(&index) {
                subscription.total_subscribers = subscription.total_subscribers + 1;
            } else {
                remote_subscriptions.insert(
                    index.clone(),
                    UniqueSubscription {
                        total_subscribers: 1,
                        name: subscription_name.clone(),
                    },
                );

                // new subscription is created, so the connection worker should be notified
                let _ = self
                    .send_to_transport_loop
                    .read()
                    .map_err(|_| Error::Poison)?
                    .try_send(MessageToTransportLoop::Subscribe((
                        subscription_name.clone(),
                        indexes.clone(),
                    )));
            }
        }

        subscriptions.insert(subscription_name, indexes);
        drop(subscriptions);

        Ok(RemoteActiveConsumer {
            inner: self.inner_pubsub.subscribe(request)?,
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
        let r = self.send_to_transport_loop.read().unwrap();
        let _ = r.try_send(MessageToTransportLoop::Stop);
    }
}

///Internal message to transport loop
pub enum MessageToTransportLoop<T>
where
    T: Topic + 'static,
{
    /// Add a subscription
    Subscribe((T::SubscriptionName, Vec<<T::Event as Event>::Topic>)),
    /// Desuscribe
    Unsubscribe(T::SubscriptionName),
    /// Exit the loop
    Stop,
}

/// Subscription transport trait
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// Topic
    type Topic: Topic + Clone + Sync + Send;

    /// Create a new subscription name
    fn new_name(&self) -> <Self::Topic as Topic>::SubscriptionName;

    /// Open a long connection
    async fn long_connection(
        &self,
        subscribe_changes: mpsc::Receiver<MessageToTransportLoop<Self::Topic>>,
        topics: Vec<<<Self::Topic as Topic>::Event as Event>::Topic>,
    ) -> Result<(), Error>
    where
        Self: Sized;

    /// Poll on demand
    async fn poll(
        &self,
        topics: Vec<<<Self::Topic as Topic>::Event as Event>::Topic>,
        reply_to: Arc<Pubsub<Self::Topic>>,
    ) -> Result<(), Error>;
}
