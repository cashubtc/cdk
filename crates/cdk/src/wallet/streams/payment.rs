//! Payment Stream
//!
//! This future Stream will wait events for a Mint Quote be paid. If it is for a Bolt12 it will not stop
//! but it will eventually error on a Timeout.
//!
//! Bolt11 will emit a single event.
use std::pin::Pin;
use std::task::Poll;

use cdk_common::{Amount, Error, MeltQuoteState, MintQuoteState, NotificationPayload};
use futures::{FutureExt, Stream};
use tokio::time::{sleep, Duration, Sleep};

use super::RecvFuture;
use crate::wallet::subscription::ActiveSubscription;
use crate::{Wallet, WalletSubscription};

/// PaymentWaiter
pub struct PaymentStream<'a> {
    wallet: Option<(&'a Wallet, WalletSubscription)>,
    is_finalized: bool,
    active_subscription: Option<ActiveSubscription>,
    timeout: Duration,

    // Future events
    subscriber_future: Option<RecvFuture<'a, ActiveSubscription>>,
    subscription_receiver_future:
        Option<RecvFuture<'static, (Option<NotificationPayload<String>>, ActiveSubscription)>>,
    timeout_future: Option<Pin<Box<Sleep>>>,
}

impl<'a> PaymentStream<'a> {
    /// Creates a new instance of the
    pub fn new(wallet: &'a Wallet, filter: WalletSubscription, timeout: Duration) -> Self {
        Self {
            wallet: Some((wallet, filter)),
            is_finalized: false,
            active_subscription: None,
            timeout,
            subscriber_future: None,
            subscription_receiver_future: None,
            timeout_future: None,
        }
    }

    /// Creating a wallet subscription is an async event, this may change in the future, but for now,
    /// creating a new Subscription should be polled, as any other async event. This function will
    /// return None if the subscription is already active, Some(()) otherwise
    fn poll_init_subscription(&mut self, cx: &mut std::task::Context<'_>) -> Option<()> {
        if let Some((wallet, filter)) = self.wallet.take() {
            self.subscriber_future = Some(Box::pin(async move { wallet.subscribe(filter).await }));
        }

        let mut subscriber_future = self.subscriber_future.take()?;

        match subscriber_future.poll_unpin(cx) {
            Poll::Pending => {
                self.subscriber_future = Some(subscriber_future);
                Some(())
            }
            Poll::Ready(active_subscription) => {
                self.active_subscription = Some(active_subscription);
                None
            }
        }
    }

    /// Checks if the timeout has been reached, or starts a new timer that will be executed if no
    /// event is produced before
    ///
    /// When an event is produced this timeout is dropped and it starts again whenever the new event
    /// is polled.
    fn poll_timeout(&mut self, cx: &mut std::task::Context<'_>) -> bool {
        let mut timeout = self
            .timeout_future
            .take()
            .unwrap_or_else(|| Box::pin(sleep(self.timeout)));

        if timeout.poll_unpin(cx).is_ready() {
            self.subscription_receiver_future = None;
            true
        } else {
            self.timeout_future = Some(timeout);
            false
        }
    }

    /// Polls the subscription for any new event
    fn poll_event(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<Option<Amount>, Error>>> {
        let (subscription_receiver_future, active_subscription) = (
            self.subscription_receiver_future.take(),
            self.active_subscription.take(),
        );

        if subscription_receiver_future.is_none() && active_subscription.is_none() {
            // Unexpected state, we should have an in-flight future or the active_subscription to
            // create the future to read an event
            return Poll::Ready(Some(Err(Error::Internal)));
        }

        let mut receiver = subscription_receiver_future.unwrap_or_else(|| {
            let mut subscription_receiver =
                active_subscription.expect("active subscription object");

            Box::pin(async move { (subscription_receiver.recv().await, subscription_receiver) })
        });

        match receiver.poll_unpin(cx) {
            Poll::Pending => {
                self.subscription_receiver_future = Some(receiver);
                Poll::Pending
            }
            Poll::Ready((notification, subscription)) => {
                tracing::debug!("Receive payment notification {:?}", notification);
                // This future is now fulfilled, put the active_subscription again back to object. Next time next().await is called,
                // the future will be created in subscription_receiver_future.
                self.active_subscription = Some(subscription);
                self.timeout_future = None; // resets timeout
                match notification {
                    None => {
                        self.is_finalized = true;
                        Poll::Ready(None)
                    }
                    Some(info) => {
                        match info {
                            NotificationPayload::MintQuoteBolt11Response(info) => {
                                if info.state == MintQuoteState::Paid {
                                    self.is_finalized = true;
                                    return Poll::Ready(Some(Ok(None)));
                                }
                            }
                            NotificationPayload::MintQuoteBolt12Response(info) => {
                                let to_be_issued = info.amount_paid - info.amount_issued;
                                if to_be_issued > Amount::ZERO {
                                    return Poll::Ready(Some(Ok(Some(to_be_issued))));
                                }
                            }
                            NotificationPayload::MeltQuoteBolt11Response(info) => {
                                if info.state == MeltQuoteState::Paid {
                                    self.is_finalized = true;
                                    return Poll::Ready(Some(Ok(None)));
                                }
                            }
                            _ => {}
                        }

                        // We got an event but it is not what was expected, we need to call `recv`
                        // again, and to copy-paste this is a recursive call that should be resolved
                        // to a Poll::Pending *but* will trigger the future execution
                        self.poll_event(cx)
                    }
                }
            }
        }
    }
}

impl Stream for PaymentStream<'_> {
    type Item = Result<Option<Amount>, Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.is_finalized {
            // end of stream
            return Poll::Ready(None);
        }

        if this.poll_timeout(cx) {
            return Poll::Ready(Some(Err(Error::Timeout)));
        }

        if this.poll_init_subscription(cx).is_some() {
            return Poll::Pending;
        }

        this.poll_event(cx)
    }
}
