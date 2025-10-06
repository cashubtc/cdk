use std::collections::HashMap;

use cdk_common::util::unix_time;
use cdk_common::wallet::{MeltQuote, Transaction, TransactionDirection};
use cdk_common::{Error, MeltQuoteBolt11Response, MeltQuoteState, ProofsMethods};
use tracing::instrument;

use crate::Wallet;

#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
mod melt_bip353;
mod melt_bolt11;
mod melt_bolt12;

impl Wallet {
    /// Check pending melt quotes
    #[instrument(skip_all)]
    pub async fn check_pending_melt_quotes(&self) -> Result<(), Error> {
        let quotes = self.get_pending_melt_quotes().await?;
        for quote in quotes {
            self.melt_quote_status(&quote.id).await?;
        }
        Ok(())
    }

    /// Get all active melt quotes from the wallet
    pub async fn get_active_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes
            .into_iter()
            .filter(|q| {
                q.state == MeltQuoteState::Pending
                    || (q.state == MeltQuoteState::Unpaid && q.expiry > unix_time())
            })
            .collect())
    }

    /// Get pending melt quotes
    pub async fn get_pending_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes
            .into_iter()
            .filter(|q| q.state == MeltQuoteState::Pending)
            .collect())
    }

    pub(crate) async fn add_transaction_for_pending_melt(
        &self,
        quote: &MeltQuote,
        response: &MeltQuoteBolt11Response<String>,
    ) -> Result<(), Error> {
        if quote.state != response.state {
            tracing::info!(
                "Quote melt {} state changed from {} to {}",
                quote.id,
                quote.state,
                response.state
            );
            if response.state == MeltQuoteState::Paid {
                let pending_proofs = self.get_pending_proofs().await?;
                let proofs_total = pending_proofs.total_amount().unwrap_or_default();
                let change_total = response.change_amount().unwrap_or_default();

                self.localstore
                    .add_transaction(Transaction {
                        mint_url: self.mint_url.clone(),
                        direction: TransactionDirection::Outgoing,
                        amount: response.amount,
                        fee: proofs_total
                            .checked_sub(response.amount)
                            .and_then(|amt| amt.checked_sub(change_total))
                            .unwrap_or_default(),
                        unit: quote.unit.clone(),
                        ys: pending_proofs.ys()?,
                        timestamp: unix_time(),
                        memo: None,
                        metadata: HashMap::new(),
                        quote_id: Some(quote.id.clone()),
                        payment_request: Some(quote.request.clone()),
                        payment_proof: response.payment_preimage.clone(),
                    })
                    .await?;
            }
        }
        Ok(())
    }
}
