//! Nostr payment event stream
//!
//! This stream exposes incoming Nostr payment messages as a standard `Stream<Item = Result<PaymentRequestPayload, Error>>`
//! so callers can `select!`/`next().await`, cancel via `CancellationToken`, or combine with other streams.

use std::task::Poll;

use cdk_common::PaymentRequestPayload;
use futures::{FutureExt, Stream, StreamExt};
use nostr::prelude::{Filter, Keys, Kind, PublicKey, UnwrappedGift};
use nostr_sdk::prelude::{Client, ClientNotification, RelayCapabilities, SignerAuthenticator};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::wallet::streams::RecvFuture;

#[allow(clippy::type_complexity)]
pub struct NostrPaymentEventStream {
    cancel: CancellationToken,
    // Internal channel receiver for parsed payloads
    rx: Option<mpsc::Receiver<Result<PaymentRequestPayload, Error>>>,
    // A future that initializes the client + subscription and spawns the notification pump
    init_fut: Option<RecvFuture<'static, Result<(), Error>>>,
    // Future to detect external cancellation
    cancel_fut: Option<RecvFuture<'static, ()>>,
}

impl NostrPaymentEventStream {
    pub fn new(keys: Keys, relays: Vec<String>, pubkey: PublicKey) -> Self {
        let cancel = CancellationToken::new();
        let (tx, rx) = mpsc::channel::<Result<PaymentRequestPayload, Error>>(32);

        let init_cancel = cancel.clone();
        let init_fut = Box::pin(async move {
            let client = Client::builder()
                .authenticator(SignerAuthenticator::new(keys.clone()))
                .build();

            for r in &relays {
                client
                    .add_relay(r.clone())
                    .capabilities(RelayCapabilities::READ)
                    .await
                    .map_err(|e| Error::Custom(format!("Add relay {r}: {e}")))?;
            }

            client.connect().await;

            // Subscribe to events addressed to `pubkey`
            let filter = Filter::new().pubkey(pubkey).kind(Kind::GiftWrap);
            let notifications = client.notifications();
            client
                .subscribe(filter)
                .await
                .map_err(|e| Error::Custom(format!("Subscribe: {e}")))?;

            // Pump notifications in a background task into the channel until cancelled
            tokio::spawn(pump_notifications(
                client,
                keys,
                notifications,
                tx,
                init_cancel,
            ));

            Ok(())
        });

        Self {
            cancel,
            rx: Some(rx),
            init_fut: Some(init_fut),
            cancel_fut: None,
        }
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

impl Drop for NostrPaymentEventStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn pump_notifications<S>(
    client: Client,
    keys: Keys,
    mut notifications: S,
    tx: mpsc::Sender<Result<PaymentRequestPayload, Error>>,
    cancel: CancellationToken,
) where
    S: Stream<Item = ClientNotification> + Unpin,
{
    loop {
        let notification = tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tx.closed() => break,
            notification = notifications.next() => notification,
        };
        let closed = notification.is_none();
        let item = match notification {
            Some(ClientNotification::Event { event, .. }) => {
                match UnwrappedGift::from_gift_wrap(&keys, &event) {
                    Ok(unwrapped) => {
                        serde_json::from_str::<PaymentRequestPayload>(&unwrapped.rumor.content)
                            .map_err(|e| Error::Custom(format!("Invalid payload JSON: {e}")))
                    }
                    Err(e) => Err(Error::Custom(format!("Unwrap gift wrap failed: {e}"))),
                }
            }
            Some(_) => continue,
            None => Err(Error::Custom("Notification stream closed".to_string())),
        };

        // Every result observes receiver closure and cancellation, including
        // malformed messages and sends blocked by a full channel.
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = tx.send(item) => {
                if result.is_err() || closed {
                    break;
                }
            }
        }
    }

    client.disconnect().await;
}

impl Stream for NostrPaymentEventStream {
    type Item = Result<PaymentRequestPayload, Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Check external cancellation
        if this.cancel_fut.is_none() {
            let cancel = this.cancel.clone();
            this.cancel_fut = Some(Box::pin(async move { cancel.cancelled().await }));
        }
        if let Some(mut fut) = this.cancel_fut.take() {
            if fut.poll_unpin(cx).is_ready() {
                // Drop receiver to end the stream
                this.rx.take();
                this.init_fut.take();
                return Poll::Ready(None);
            }
            this.cancel_fut = Some(fut);
        }

        // Drive initialization
        if let Some(mut init) = this.init_fut.take() {
            match init.poll_unpin(cx) {
                Poll::Pending => {
                    this.init_fut = Some(init);
                    return Poll::Pending;
                }
                Poll::Ready(Err(e)) => {
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(Ok(())) => {
                    // fallthrough
                }
            }
        }

        match this.rx.as_mut() {
            Some(rx) => rx.poll_recv(cx),
            None => Poll::Ready(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::{poll, stream};
    use nostr::prelude::{EventBuilder, FinalizeEvent, RelayUrl, SubscriptionId};
    use nostr_sdk::prelude::RelayStatus;
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn dropping_stream_stops_idle_pump() {
        let keys = Keys::generate();
        let mut stream = NostrPaymentEventStream::new(keys.clone(), vec![], keys.public_key());
        let (tx, rx) = mpsc::channel(1);
        stream.rx = Some(rx);
        let client = Client::default();
        let relay_url = "wss://relay.example.com";
        client.add_relay(relay_url).await.expect("add relay");
        let relay = client
            .relay(relay_url)
            .await
            .expect("lookup relay")
            .expect("relay");
        assert_ne!(relay.status(), RelayStatus::Terminated);
        let pump = pump_notifications(
            client.clone(),
            keys,
            stream::pending(),
            tx,
            stream.cancel_token(),
        );
        tokio::pin!(pump);
        assert!(poll!(&mut pump).is_pending());
        // Keep the receiver alive separately: termination must come from Drop
        // cancellation, not from a failed send or another relay notification.
        let _rx = stream.rx.take().expect("receiver");
        drop(stream);
        timeout(Duration::from_secs(1), pump)
            .await
            .expect("drop must stop idle pump");
        // Retaining the client ensures this checks explicit disconnect cleanup.
        assert_eq!(relay.status(), RelayStatus::Terminated);
    }

    #[tokio::test]
    async fn closing_receiver_stops_idle_pump() {
        let (tx, rx) = mpsc::channel(1);
        let pump = pump_notifications(
            Client::default(),
            Keys::generate(),
            stream::pending(),
            tx,
            CancellationToken::new(),
        );
        tokio::pin!(pump);
        assert!(poll!(&mut pump).is_pending());
        drop(rx);
        timeout(Duration::from_secs(1), pump)
            .await
            .expect("receiver closure must stop idle pump");
    }

    #[tokio::test]
    async fn cancellation_interrupts_backpressured_malformed_message() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::GiftWrap, "invalid ciphertext")
            .finalize(&keys)
            .expect("signed event");
        let notification = ClientNotification::Event {
            relay_url: RelayUrl::parse("wss://relay.example.com").expect("relay URL"),
            subscription_id: SubscriptionId::new("test"),
            event: Box::new(event),
        };
        let (tx, mut rx) = mpsc::channel(1);
        tx.send(Err(Error::Internal)).await.expect("fill channel");
        let cancel = CancellationToken::new();
        let pump = pump_notifications(
            Client::default(),
            keys,
            stream::iter([notification]).chain(stream::pending()),
            tx,
            cancel.clone(),
        );
        tokio::pin!(pump);
        // The notification is ready, so this blocks on sending its parse error.
        assert!(poll!(&mut pump).is_pending());
        cancel.cancel();
        timeout(Duration::from_secs(1), pump)
            .await
            .expect("cancellation must interrupt a blocked send");
        assert!(matches!(rx.recv().await, Some(Err(Error::Internal))));
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn pending_receive_does_not_end_stream() {
        let keys = Keys::generate();
        let mut stream = NostrPaymentEventStream::new(keys.clone(), vec![], keys.public_key());
        stream.init_fut = None;
        let (tx, rx) = mpsc::channel(1);
        stream.rx = Some(rx);
        assert!(poll!(stream.next()).is_pending());
        assert!(poll!(stream.next()).is_pending());
        tx.send(Err(Error::Internal)).await.expect("send item");
        assert!(matches!(stream.next().await, Some(Err(Error::Internal))));
        stream.cancel_token().cancel();
        assert!(stream.next().await.is_none());
        assert!(tx.is_closed());
    }
}
