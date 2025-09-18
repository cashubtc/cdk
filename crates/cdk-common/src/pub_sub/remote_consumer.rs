//! Pub-sub consumer
//!
//! Consumers are designed to connect to a producer, through a transport, and subscribe to events.
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use tokio::sync::mpsc;

use super::pubsub::Topic;
use super::subscriber::{ActiveSubscription, SubscriptionRequest};
use super::{Error, Indexable, Pubsub};

type ActiveSubscriptions<T> = RwLock<
    HashMap<
        <T as Topic>::SubscriptionName,
        (
            Vec<<<T as Topic>::Event as Indexable>::Index>,
            ActiveSubscription<T>,
        ),
    >,
>;

type InternalSender<T> = mpsc::Sender<(<T as Topic>::SubscriptionName, <T as Topic>::Event)>;

/// Subscription consumer
pub struct Consumer<T>
where
    T: Transport + 'static,
{
    transport: T,
    pubsub_internal_sender: InternalSender<T::Topic>,
    inner_pubsub: Pubsub<T::Topic>,
    subscriptions: ActiveSubscriptions<T::Topic>,
    send_to_transport_loop: RwLock<mpsc::Sender<MessageToTransportLoop<T::Topic>>>,
    still_running: AtomicBool,
}

impl<T> Consumer<T>
where
    T: Transport + 'static,
{
    /// Creates a new instance
    pub async fn new(transport: T) -> Arc<Self> {
        let (sender, _) = mpsc::channel(10_000);
        let this = Arc::new(Self {
            transport,
            inner_pubsub: T::new_pubsub().await,
            pubsub_internal_sender: mpsc::channel(10_000).0,
            subscriptions: Default::default(),
            send_to_transport_loop: RwLock::new(sender),
            still_running: true.into(),
        });

        tokio::spawn(Self::connection_loop(this.clone()));

        this
    }

    async fn connection_loop(instance: Arc<Self>) {
        loop {
            let (sender, receiver) = mpsc::channel(10_000);

            {
                let mut shared_sender = instance.send_to_transport_loop.write().unwrap();
                *shared_sender = sender;
            }

            instance.transport.long_connection(receiver).await;
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
    ///
    ///
    pub fn subscribe<I>(&self, request: I) -> Result<(), Error>
    where
        I: SubscriptionRequest<
            Index = <<T::Topic as Topic>::Event as Indexable>::Index,
            SubscriptionName = <T::Topic as Topic>::SubscriptionName,
        >,
    {
        let transport_loop = self
            .send_to_transport_loop
            .read()
            .map_err(|_| Error::Poison)?;
        let mut subscriptions = self.subscriptions.write().map_err(|_| Error::Poison)?;
        let subscription_name = request.subscription_name();
        let indexes = request.try_get_indexes()?;

        if subscriptions.get(&subscription_name).is_some() {
            return Err(Error::AlreadySubscribed);
        }

        subscriptions.insert(
            subscription_name.clone(),
            (
                indexes.clone(),
                self.inner_pubsub.subscribe_with(
                    request,
                    self.pubsub_internal_sender.clone(),
                    None,
                )?,
            ),
        );
        drop(subscriptions);

        let _ = transport_loop.try_send(MessageToTransportLoop::Subscribe((
            subscription_name,
            indexes,
        )));

        Ok(())
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
    Subscribe((T::SubscriptionName, Vec<<T::Event as Indexable>::Index>)),
    /// Desuscribe
    Desuscribe(T::SubscriptionName),
    /// Exit the loop
    Stop,
}

/// Subscription transport trait
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// Topic
    type Topic: Topic + Clone + Sync + Send;

    /// Creates a new pubsub topic producer
    async fn new_pubsub() -> Pubsub<Self::Topic>;

    /// Open a long connection
    async fn long_connection(
        &self,
        subscribe_changes: mpsc::Receiver<MessageToTransportLoop<Self::Topic>>,
    ) where
        Self: Sized;

    /// Poll on demand
    async fn poll(
        &self,
        index: Vec<<<Self::Topic as Topic>::Event as Indexable>::Index>,
    ) -> Result<Vec<Self::Topic>, Error>;
}
