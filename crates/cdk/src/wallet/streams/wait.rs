use cdk_common::amount::SplitTarget;
use cdk_common::wallet::MintQuote;
use cdk_common::{Amount, Error, Proofs, SpendingConditions};
use futures::future::BoxFuture;
use futures::StreamExt;
use tokio::time::{timeout, Duration};

use super::Wallet;

impl Wallet {
    #[inline(always)]
    /// Mints a mint quote once it is paid
    pub async fn wait_and_mint_quote(
        &self,
        quote: MintQuote,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
        timeout_duration: Duration,
    ) -> Result<Proofs, Error> {
        let mut stream = self.proof_stream(quote, amount_split_target, spending_conditions);

        timeout(timeout_duration, async move {
            stream.next().await.ok_or(Error::Internal)?
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    /// Returns a BoxFuture that will wait for payment on the given event with a timeout check
    #[allow(private_bounds)]
    pub fn wait_for_payment(
        &self,
        event: &MintQuote,
        timeout_duration: Duration,
    ) -> BoxFuture<'_, Result<Option<Amount>, Error>> {
        let mut stream = self.payment_stream(event);

        Box::pin(async move {
            timeout(timeout_duration, async {
                stream
                    .next()
                    .await
                    .ok_or(Error::Internal)?
                    .map(|(_quote, amount)| amount)
            })
            .await
            .map_err(|_| Error::Timeout)?
        })
    }
}
