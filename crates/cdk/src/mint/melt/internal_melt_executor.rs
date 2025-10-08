use cdk_common::database::{self, MintTransaction};
use cdk_common::mint::MeltQuote;
use cdk_common::nuts::{MeltRequest, MintQuoteState, PaymentMethod};
use cdk_common::quote_id::QuoteId;
use tracing::instrument;

use crate::mint::Mint;
use crate::{Amount, Error};

/// Handles internal melt operations where payment is settled internally
/// against a matching mint quote.
pub struct InternalMeltExecutor<'a> {
    mint: &'a Mint,
}

impl<'a> InternalMeltExecutor<'a> {
    pub fn new(mint: &'a Mint) -> Self {
        Self { mint }
    }

    /// Check if the melt quote can be settled internally against a mint quote
    #[instrument(skip_all)]
    pub async fn is_internal_settlement(
        &self,
        tx: &mut Box<dyn MintTransaction<'_, database::Error> + Send + Sync + '_>,
        melt_quote: &MeltQuote,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<Option<QuoteId>, Error> {
        let mint_quote = match tx
            .get_mint_quote_by_request(&melt_quote.request.to_string())
            .await
        {
            Ok(Some(mint_quote)) if mint_quote.unit == melt_quote.unit => mint_quote,
            // Not an internal melt -> mint or unit mismatch
            Ok(_) => return Ok(None),
            Err(err) => {
                tracing::debug!("Error attempting to get mint quote: {}", err);
                return Err(Error::Internal);
            }
        };

        let inputs_amount_quote_unit = melt_request.inputs_amount().map_err(|_| {
            tracing::error!("Proof inputs in melt quote overflowed");
            Error::AmountOverflow
        })?;

        if let Some(amount) = mint_quote.amount {
            if amount > inputs_amount_quote_unit {
                tracing::debug!(
                    "Not enough inputs provided: {} needed {}",
                    inputs_amount_quote_unit,
                    amount
                );
                return Err(Error::InsufficientFunds);
            }
        }

        Ok(Some(mint_quote.id))
    }

    /// Execute internal melt - settle with matching mint quote
    /// Returns (preimage, amount_spent, quote)
    #[instrument(skip_all)]
    pub async fn execute(
        &self,
        melt_quote: &MeltQuote,
        mint_quote_id: &QuoteId,
    ) -> Result<(Option<String>, Amount, MeltQuote), Error> {
        let mut tx = self.mint.localstore.begin_transaction().await?;

        let mint_quote = tx
            .get_mint_quote(mint_quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        // Mint quote has already been settled, proofs should not be burned or held.
        if (mint_quote.state() == MintQuoteState::Issued
            || mint_quote.state() == MintQuoteState::Paid)
            && mint_quote.payment_method == PaymentMethod::Bolt11
        {
            return Err(Error::RequestAlreadyPaid);
        }

        let amount = melt_quote.amount;

        tracing::info!(
            "Executing internal melt for quote {} with amount {}",
            melt_quote.id,
            amount
        );

        tracing::info!(
            "Mint quote {} paid {} from internal payment.",
            mint_quote.id,
            amount
        );

        let total_paid = tx
            .increment_mint_quote_amount_paid(&mint_quote.id, amount, melt_quote.id.to_string())
            .await?;

        self.mint
            .pubsub_manager
            .mint_quote_payment(&mint_quote, total_paid);

        tracing::info!(
            "Melt quote {} paid Mint quote {}",
            melt_quote.id,
            mint_quote.id
        );

        tx.commit().await?;

        Ok((None, amount, melt_quote.clone()))
    }
}
