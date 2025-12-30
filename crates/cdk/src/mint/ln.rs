use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::amount::to_unit;
use cdk_common::common::PaymentProcessorKey;
use cdk_common::database::DynMintDatabase;
use cdk_common::mint::MintQuote;
use cdk_common::payment::DynMintPayment;
use cdk_common::{Amount, MintQuoteState, PaymentMethod};
use tracing::instrument;

use super::subscription::PubSubManager;
use super::Mint;
use crate::Error;

impl Mint {
    /// Static implementation of check_mint_quote_paid to avoid circular dependency to the Mint
    #[inline(always)]
    pub(crate) async fn check_mint_quote_payments(
        localstore: DynMintDatabase,
        payment_processors: Arc<HashMap<PaymentProcessorKey, DynMintPayment>>,
        pubsub_manager: Option<Arc<PubSubManager>>,
        quote: &mut MintQuote,
    ) -> Result<(), Error> {
        let state = quote.state();

        // We can just return here and do not need to check with ln node.
        // If quote is issued it is already in a final state,
        // If it is paid ln node will only tell us what we already know
        if quote.payment_method == PaymentMethod::Bolt11
            && (state == MintQuoteState::Issued || state == MintQuoteState::Paid)
        {
            return Ok(());
        }

        let ln = match payment_processors.get(&PaymentProcessorKey::new(
            quote.unit.clone(),
            quote.payment_method.clone(),
        )) {
            Some(ln) => ln,
            None => {
                tracing::info!("Could not get ln backend for {}, bolt11 ", quote.unit);

                return Err(Error::UnsupportedUnit);
            }
        };

        let ln_status = ln
            .check_incoming_payment_status(&quote.request_lookup_id)
            .await?;

        if ln_status.is_empty() {
            return Ok(());
        }

        let mut tx = localstore.begin_transaction().await?;

        // reload the quote, as it state may have changed
        let mut new_quote = tx
            .get_mint_quote(&quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let current_state = new_quote.state();

        if new_quote.payment_method == PaymentMethod::Bolt11
            && (current_state == MintQuoteState::Issued || current_state == MintQuoteState::Paid)
        {
            return Ok(());
        }

        for payment in ln_status {
            if !new_quote.payment_ids().contains(&&payment.payment_id)
                && payment.payment_amount > Amount::ZERO
            {
                tracing::debug!(
                    "Found payment of {} {} for quote {} when checking.",
                    payment.payment_amount,
                    payment.unit,
                    new_quote.id
                );

                let amount_paid = to_unit(payment.payment_amount, &payment.unit, &new_quote.unit)?;

                match new_quote.add_payment(amount_paid, payment.payment_id.clone(), None) {
                    Ok(()) => {
                        tx.update_mint_quote(&mut new_quote).await?;
                        if let Some(pubsub_manager) = pubsub_manager.as_ref() {
                            pubsub_manager.mint_quote_payment(&new_quote, new_quote.amount_paid());
                        }
                    }
                    Err(crate::Error::DuplicatePaymentId) => {
                        tracing::debug!(
                            "Payment ID {} already processed (caught race condition in check_mint_quote_paid)",
                            payment.payment_id
                        );
                        // This is fine - another concurrent request already processed this payment
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        tx.commit().await?;

        *quote = new_quote.inner();

        Ok(())
    }

    /// Check the status of an ln payment for a quote
    #[instrument(skip_all)]
    pub async fn check_mint_quote_paid(&self, quote: &mut MintQuote) -> Result<(), Error> {
        Self::check_mint_quote_payments(
            self.localstore.clone(),
            self.payment_processors.clone(),
            Some(self.pubsub_manager.clone()),
            quote,
        )
        .await
    }
}
