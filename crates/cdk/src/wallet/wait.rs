use cdk_common::amount::SplitTarget;
use cdk_common::wallet::{MeltQuote, MintQuote};
use cdk_common::{
    Amount, Error, MeltQuoteState, MintQuoteState, NotificationPayload, Proofs, SpendingConditions,
};
use futures::future::BoxFuture;
use tokio::time::{timeout, Duration};

use super::{Wallet, WalletSubscription};

#[allow(private_bounds)]
enum WaitableEvent {
    MeltQuote(String),
    MintQuote(String),
}

impl From<&MeltQuote> for WaitableEvent {
    fn from(event: &MeltQuote) -> Self {
        WaitableEvent::MeltQuote(event.id.to_owned())
    }
}

impl From<&MintQuote> for WaitableEvent {
    fn from(event: &MintQuote) -> Self {
        WaitableEvent::MintQuote(event.id.to_owned())
    }
}

impl From<WaitableEvent> for WalletSubscription {
    fn from(val: WaitableEvent) -> Self {
        match val {
            WaitableEvent::MeltQuote(quote_id) => {
                WalletSubscription::Bolt11MeltQuoteState(vec![quote_id])
            }
            WaitableEvent::MintQuote(quote_id) => {
                WalletSubscription::Bolt11MintQuoteState(vec![quote_id])
            }
        }
    }
}

impl Wallet {
    /// Mints an amount and returns the invoice to be paid, and a BoxFuture that will finalize the
    /// mint once the invoice has been paid
    pub async fn mint_once_paid(
        &self,
        amount: Amount,
        description: Option<String>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
        timeout_duration: Duration,
    ) -> Result<(String, BoxFuture<'_, Result<Proofs, Error>>), Error> {
        let quote = self.mint_quote(amount, description).await?;
        Ok((
            quote.request.to_owned(),
            Box::pin(async move {
                self.wait_for_payment(&quote, timeout_duration).await?;
                self.mint(&quote.id, amount_split_target, spending_conditions)
                    .await
            }),
        ))
    }

    /// Returns a BoxFuture that will wait for payment on the given event with a timeout check
    #[allow(private_bounds)]
    pub fn wait_for_payment<T>(
        &self,
        event: T,
        timeout_duration: Duration,
    ) -> BoxFuture<'_, Result<(), Error>>
    where
        T: Into<WaitableEvent>,
    {
        let subs = self.subscribe::<WalletSubscription>(event.into().into());

        Box::pin(async move {
            timeout(timeout_duration, async {
                let mut subscription = subs.await;
                loop {
                    match subscription.recv().await.ok_or(Error::Internal)? {
                        NotificationPayload::MintQuoteBolt11Response(info) => {
                            if info.state == MintQuoteState::Paid {
                                return Ok(());
                            }
                        }
                        NotificationPayload::MintQuoteBolt12Response(info) => {
                            if info.amount_paid > Amount::ZERO {
                                return Ok(());
                            }
                        }
                        NotificationPayload::MeltQuoteBolt11Response(info) => {
                            if info.state == MeltQuoteState::Paid {
                                return Ok(());
                            }
                        }
                        _ => {}
                    }
                }
            })
            .await
            .map_err(|_| Error::Timeout)?
        })
    }
}
