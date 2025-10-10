use std::collections::HashMap;
use std::sync::Arc;

use anyhow::bail;
use cdk_common::amount::to_unit;
use cdk_common::mint::MeltQuote;
use cdk_common::payment::{DynMintPayment, PaymentIdentifier};
use cdk_common::MeltQuoteState;
use tracing::instrument;

use crate::cdk_payment::MakePaymentResponse;
use crate::types::PaymentProcessorKey;
use crate::Error;

/// Result of executing a payment through a payment processor
pub struct PaymentResult {
    /// Payment preimage if available (proof of payment)
    pub payment_proof: Option<String>,
    /// Total amount spent including fees
    pub total_spent: cdk_common::Amount,
    /// Identifier for tracking the payment with the payment processor
    pub payment_lookup_id: PaymentIdentifier,
}

/// Executes payments through configured payment processors with robust error handling
pub struct PaymentExecutor {
    payment_processors: HashMap<PaymentProcessorKey, DynMintPayment>,
}

impl PaymentExecutor {
    pub fn new(payment_processors: HashMap<PaymentProcessorKey, DynMintPayment>) -> Self {
        Self { payment_processors }
    }

    /// Check the current state of a payment with the payment processor
    ///
    /// This is used as a fallback when the initial payment attempt returns an
    /// ambiguous state (Unknown/Failed) or when an error occurs. It queries the
    /// payment processor directly to get the authoritative payment status.
    ///
    /// # Critical Behavior
    /// If this check fails, proofs remain stuck in pending state as we cannot
    /// determine the true payment status.
    #[instrument(skip_all)]
    async fn check_payment_state(
        ln: DynMintPayment,
        lookup_id: &PaymentIdentifier,
    ) -> anyhow::Result<MakePaymentResponse> {
        match ln.check_outgoing_payment(lookup_id).await {
            Ok(response) => Ok(response),
            Err(check_err) => {
                tracing::error!(
                    "Could not check the status of payment for {}. Proofs stuck as pending",
                    lookup_id
                );
                tracing::error!("Checking payment error: {}", check_err);
                bail!("Could not check payment status")
            }
        }
    }

    /// Execute a payment through the configured payment processor
    ///
    /// This function handles payment execution with comprehensive error recovery:
    /// 1. Attempts to make the payment through the payment processor
    /// 2. If status is Unknown/Failed, double-checks with the backend
    /// 3. If an error occurs, checks payment state to determine if it actually succeeded
    /// 4. Returns appropriate error types for different failure scenarios
    ///
    /// # Error Handling Strategy
    /// - `RequestAlreadyPaid`: Invoice was already paid (caller should reset quote)
    /// - `PaymentFailed`: Payment definitively failed (caller should reset quote)
    /// - `PendingQuote`: Payment is still pending (proofs remain in pending state)
    /// - `Internal`: Inconsistent state detected (proofs stuck as pending)
    ///
    /// The robust checking prevents both false failures (payment succeeded but we think
    /// it failed) and false successes (payment failed but we think it succeeded).
    #[instrument(skip_all)]
    pub async fn execute_payment(&self, quote: &MeltQuote) -> Result<PaymentResult, Error> {
        use crate::cdk_payment;

        let ln = self
            .payment_processors
            .get(&PaymentProcessorKey::new(
                quote.unit.clone(),
                quote.payment_method.clone(),
            ))
            .ok_or_else(|| {
                tracing::info!("Could not get ln backend for {}, bolt11 ", quote.unit);
                Error::UnsupportedUnit
            })?;

        let pre = match ln
            .make_payment(&quote.unit, quote.clone().try_into()?)
            .await
        {
            Ok(pay)
                if pay.status == MeltQuoteState::Unknown
                    || pay.status == MeltQuoteState::Failed =>
            {
                tracing::warn!(
                    "Got {} status when paying melt quote {} for {} {}. Checking with backend...",
                    pay.status,
                    quote.id,
                    quote.amount,
                    quote.unit
                );

                let check_response =
                    Self::check_payment_state(Arc::clone(ln), &pay.payment_lookup_id)
                        .await
                        .map_err(|_| Error::Internal)?;

                if check_response.status == MeltQuoteState::Paid {
                    tracing::warn!(
                        "Pay invoice returned {} but check returned {}. Proofs stuck as pending",
                        pay.status,
                        check_response.status
                    );
                    return Err(Error::Internal);
                }

                check_response
            }
            Ok(pay) => pay,
            Err(err) => {
                if matches!(err, cdk_payment::Error::InvoiceAlreadyPaid) {
                    tracing::debug!("Invoice already paid, resetting melt quote");
                    return Err(Error::RequestAlreadyPaid);
                }

                tracing::error!("Error returned attempting to pay: {} {}", quote.id, err);

                let lookup_id = quote.request_lookup_id.as_ref().ok_or_else(|| {
                    tracing::error!(
                        "No payment id could not lookup payment for {} after error.",
                        quote.id
                    );
                    Error::Internal
                })?;

                let check_response = Self::check_payment_state(Arc::clone(ln), lookup_id)
                    .await
                    .map_err(|_| Error::Internal)?;

                if check_response.status == MeltQuoteState::Paid {
                    tracing::warn!(
                        "Pay invoice returned an error but check returned {}. Proofs stuck as pending",
                        check_response.status
                    );
                    return Err(Error::Internal);
                }

                check_response
            }
        };

        match pre.status {
            MeltQuoteState::Paid => (),
            MeltQuoteState::Unpaid | MeltQuoteState::Unknown | MeltQuoteState::Failed => {
                return Err(Error::PaymentFailed);
            }
            MeltQuoteState::Pending => {
                tracing::warn!(
                    "LN payment pending, proofs are stuck as pending for quote: {}",
                    quote.id
                );
                return Err(Error::PendingQuote);
            }
        }

        let total_spent = to_unit(pre.total_spent, &pre.unit, &quote.unit)?;

        Ok(PaymentResult {
            payment_proof: pre.payment_proof,
            total_spent,
            payment_lookup_id: pre.payment_lookup_id,
        })
    }
}
