use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::common::PaymentProcessorKey;
use cdk_common::database::DynMintDatabase;
use cdk_common::mint::MintQuote;
use cdk_common::payment::DynMintPayment;
use cdk_common::util::unix_time;
use cdk_common::MintQuoteState;
use tracing::instrument;

use super::subscription::PubSubManager;
use super::Mint;
use crate::Error;

/// Minimum delay between payment backend status checks for the same mint quote.
pub(super) const MINT_QUOTE_PAYMENT_CHECK_INTERVAL_SECS: u64 = 10;

impl Mint {
    /// Static implementation of check_mint_quote_paid to avoid circular dependency to the Mint
    pub(crate) async fn check_mint_quote_payments(
        localstore: DynMintDatabase,
        payment_processors: Arc<HashMap<PaymentProcessorKey, DynMintPayment>>,
        pubsub_manager: Option<Arc<PubSubManager>>,
        quote: &mut MintQuote,
    ) -> Result<(), Error> {
        let state = quote.state();

        // We can just return here and do not need to check with the payment
        // backend. If quote is issued it is already in a final state,
        // If it is paid the payment backend will only tell us what we already know
        if quote.payment_method.is_bolt11()
            && (state == MintQuoteState::Issued || state == MintQuoteState::Paid)
        {
            return Ok(());
        }

        // Claim this check before contacting the backend. The conditional update prevents
        // concurrent HTTP or WebSocket status requests, including requests handled by different
        // mint processes, from issuing duplicate backend calls. Recording the attempt first also
        // throttles retries while a payment backend is failing.
        let now = unix_time();
        let claimed = localstore
            .try_update_mint_quote_last_checked(
                &quote.id,
                now,
                MINT_QUOTE_PAYMENT_CHECK_INTERVAL_SECS,
            )
            .await?;
        if !claimed {
            tracing::trace!(
                quote_id = %quote.id,
                request_lookup_id = %quote.request_lookup_id,
                check_interval_seconds = MINT_QUOTE_PAYMENT_CHECK_INTERVAL_SECS,
                "mint quote payment check skipped because another recent check holds the rate limit",
            );
            return Ok(());
        }
        quote.set_last_checked(now);

        let payment_backend = match payment_processors.get(&PaymentProcessorKey::new(
            quote.unit.clone(),
            quote.payment_method.clone(),
        )) {
            Some(payment_backend) => payment_backend,
            None => {
                tracing::info!("Could not get payment backend for {}, bolt11 ", quote.unit);

                return Err(Error::UnsupportedUnit);
            }
        };

        let payment_status = payment_backend
            .check_incoming_payment_status(&quote.request_lookup_id)
            .await
            .inspect_err(|err| {
                tracing::warn!(
                    quote_id = %quote.id,
                    method = %quote.payment_method,
                    unit = %quote.unit,
                    request_lookup_id = %quote.request_lookup_id,
                    error = %err,
                    "mint quote payment status check failed; quote state is unchanged",
                );
            })?;

        if payment_status.is_empty() {
            tracing::trace!(
                quote_id = %quote.id,
                method = %quote.payment_method,
                unit = %quote.unit,
                request_lookup_id = %quote.request_lookup_id,
                quote_state = %quote.state(),
                "mint quote payment check found no new payments",
            );
            return Ok(());
        }

        let mut tx = localstore.begin_transaction().await?;

        // reload the quote, as it state may have changed
        let mut new_quote = tx
            .get_mint_quote(&quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let current_state = new_quote.state();

        if new_quote.payment_method.is_bolt11()
            && (current_state == MintQuoteState::Issued || current_state == MintQuoteState::Paid)
        {
            *quote = new_quote.inner();
            return Ok(());
        }

        let mut should_notify = false;
        let mut recorded_payment_count = 0usize;

        for payment in payment_status {
            if !new_quote.payment_ids().contains(&&payment.payment_id)
                && payment.payment_amount.value() > 0
            {
                tracing::debug!(
                    "Found payment of {} {:?} for quote {} when checking.",
                    payment.payment_amount.value(),
                    payment.unit(),
                    new_quote.id
                );

                let amount_paid = payment.payment_amount.convert_to(&new_quote.unit)?;

                match new_quote.add_payment(amount_paid, payment.payment_id.clone(), None) {
                    Ok(()) => {
                        tx.update_mint_quote(&mut new_quote).await?;
                        should_notify = true;
                        recorded_payment_count += 1;
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

        if should_notify {
            tracing::info!(
                quote_id = %new_quote.id,
                method = %new_quote.payment_method,
                unit = %new_quote.unit,
                request_lookup_id = %new_quote.request_lookup_id,
                previous_state = %current_state,
                new_state = %new_quote.state(),
                recorded_payment_count,
                amount_paid = %new_quote.amount_paid(),
                amount_issued = %new_quote.amount_issued(),
                amount_mintable = %new_quote.amount_mintable(),
                "mint quote payment committed after backend status check",
            );
        }

        // Publish notification AFTER transaction commits so subscribers
        // see the committed state when they query.
        if should_notify {
            match pubsub_manager.as_ref() {
                Some(pubsub_manager) => {
                    pubsub_manager.mint_quote_payment(&new_quote, new_quote.amount_paid())
                }
                None => tracing::warn!(
                    quote_id = %new_quote.id,
                    "mint quote payment committed without a pub/sub manager; no NUT-17 notification was published",
                ),
            }
        }

        *quote = new_quote.inner();

        Ok(())
    }

    /// Check the status of a payment for a quote with the payment backend
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
