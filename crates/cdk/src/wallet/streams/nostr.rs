//! Nostr payment event stream
//!
//! This stream exposes incoming Nostr payment messages as a standard `Stream<Item = Result<PaymentRequestPayload, Error>>`
//! so callers can `select!`/`next().await`, cancel via `CancellationToken`, or combine with other streams.

use std::task::Poll;

use cdk_common::PaymentRequestPayload;
use futures::{FutureExt, Stream, StreamExt};
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
    // Future awaiting the next item from `rx`
    rx_future: Option<
        RecvFuture<
            'static,
            (
                Option<Result<PaymentRequestPayload, Error>>,
                mpsc::Receiver<Result<PaymentRequestPayload, Error>>,
            ),
        >,
    >,
}

impl NostrPaymentEventStream {
    pub fn new(
        keys: nostr_sdk::prelude::Keys,
        relays: Vec<String>,
        pubkey: nostr_sdk::prelude::PublicKey,
    ) -> Self {
        let cancel = CancellationToken::new();
        let (tx, rx) = mpsc::channel::<Result<PaymentRequestPayload, Error>>(32);

        let init_cancel = cancel.clone();
        let init_fut = Box::pin(async move {
            let client = nostr_sdk::prelude::Client::builder()
                .authenticator(nostr_sdk::prelude::SignerAuthenticator::new(keys.clone()))
                .build();

            for r in &relays {
                client
                    .add_relay(r.clone())
                    .capabilities(nostr_sdk::prelude::RelayCapabilities::READ)
                    .await
                    .map_err(|e| Error::Custom(format!("Add relay {r}: {e}")))?;
            }

            client.connect().await;

            // Subscribe to events addressed to `pubkey`
            let filter = nostr_sdk::prelude::Filter::new()
                .pubkey(pubkey)
                .kind(nostr_sdk::prelude::Kind::GiftWrap);
            let mut notifications = client.notifications();
            client
                .subscribe(filter)
                .await
                .map_err(|e| Error::Custom(format!("Subscribe: {e}")))?;

            // Pump notifications in a background task into the channel until cancelled
            let _bg = tokio::spawn(async move {
                loop {
                    let notification = tokio::select! {
                        _ = init_cancel.cancelled() => break,
                        notification = notifications.next() => notification,
                    };
                    let event = match notification {
                        Some(nostr_sdk::prelude::ClientNotification::Event { event, .. }) => event,
                        Some(_) => continue,
                        None => {
                            let _ = tx
                                .send(Err(Error::Custom("Notification stream closed".to_string())))
                                .await;
                            break;
                        }
                    };

                    match nostr_sdk::prelude::UnwrappedGift::from_gift_wrap(&keys, &event) {
                        Ok(unwrapped) => {
                            match serde_json::from_str::<PaymentRequestPayload>(
                                &unwrapped.rumor.content,
                            ) {
                                Ok(payload) => {
                                    if tx.send(Ok(payload)).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx
                                        .send(Err(Error::Custom(format!(
                                            "Invalid payload JSON: {e}"
                                        ))))
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send(Err(Error::Custom(format!("Unwrap gift wrap failed: {e}"))))
                                .await;
                        }
                    }
                }

                client.disconnect().await;
            });

            Ok(())
        });

        Self {
            cancel,
            rx: Some(rx),
            init_fut: Some(init_fut),
            cancel_fut: None,
            rx_future: None,
        }
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
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

        // Drive next item from the internal channel
        if this.rx.is_none() {
            return Poll::Ready(None);
        }

        if this.rx_future.is_none() {
            let mut rx = this.rx.take().expect("receiver");
            this.rx_future = Some(Box::pin(async move {
                let item = rx.recv().await;
                (item, rx)
            }));
        }

        let mut fut = this.rx_future.take().ok_or(Error::Internal)?;
        match fut.poll_unpin(cx) {
            Poll::Pending => {
                this.rx_future = Some(fut);
                Poll::Pending
            }
            Poll::Ready((item, rx)) => {
                this.rx = Some(rx);
                match item {
                    None => Poll::Ready(None),
                    Some(item) => Poll::Ready(Some(item)),
                }
            }
        }
    }
}
