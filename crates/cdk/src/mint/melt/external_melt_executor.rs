use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::amount::to_unit;
use cdk_common::database::mint::MeltRequestInfo;
use cdk_common::database::{self, DynMintDatabase, MintTransaction};
use cdk_common::mint::MeltQuote;
use cdk_common::nuts::{CurrencyUnit, MeltQuoteState, MeltRequest, State};
use cdk_common::payment::DynMintPayment;
use cdk_common::quote_id::QuoteId;
use tracing::instrument;

use super::payment_executor::PaymentExecutor;
use super::MeltExecutionResult;
use crate::mint::proof_writer::ProofWriter;
use crate::mint::Mint;
use crate::{Amount, Error};

/// Handles external melt operations where payment is made via payment processor
pub struct ExternalMeltExecutor<'a> {
    mint: &'a Mint,
    payment_processors: HashMap<crate::types::PaymentProcessorKey, DynMintPayment>,
}

impl<'a> ExternalMeltExecutor<'a> {
    pub fn new(
        mint: &'a Mint,
        payment_processors: HashMap<crate::types::PaymentProcessorKey, DynMintPayment>,
    ) -> Self {
        Self {
            mint,
            payment_processors,
        }
    }

    /// Execute external melt - spawn payment task and return pending
    /// Returns MeltExecutionResult::Pending to signal async processing
    #[instrument(skip_all)]
    pub async fn execute<'b>(
        &self,
        tx: Box<dyn MintTransaction<'b, database::Error> + Send + Sync + 'b>,
        quote: &MeltQuote,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<MeltExecutionResult<'a>, Error>
    where
        'b: 'a,
    {
        let _partial_amount = match quote.unit {
            CurrencyUnit::Sat | CurrencyUnit::Msat => {
                self.mint
                    .check_melt_expected_ln_fees(quote, melt_request)
                    .await?
            }
            _ => None,
        };

        tx.commit().await?;

        // Clone data needed for background task
        let quote_clone = quote.clone();
        let quote_id = quote.id.clone();
        let payment_processors = self.payment_processors.clone();
        let localstore = self.mint.localstore();
        let pubsub_manager = self.mint.pubsub_manager();
        let signatory = Arc::clone(&self.mint.signatory);

        tokio::spawn(async move {
            if let Err(err) = Self::process_payment_and_finalize_async(
                payment_processors,
                localstore,
                pubsub_manager,
                signatory,
                quote_clone,
            )
            .await
            {
                tracing::error!(
                    "Background payment processing failed for quote {}: {}",
                    quote_id,
                    err
                );
            }
        });

        tracing::info!(
            "External melt payment for quote {} spawned in background, returning pending",
            quote.id
        );

        Ok(MeltExecutionResult::Pending {
            quote: quote.clone(),
        })
    }

    /// Process payment and finalize melt asynchronously in background task
    async fn process_payment_and_finalize_async(
        payment_processors: HashMap<crate::types::PaymentProcessorKey, DynMintPayment>,
        localstore: DynMintDatabase,
        pubsub_manager: Arc<crate::mint::subscription::PubSubManager>,
        signatory: Arc<dyn cdk_signatory::signatory::Signatory + Send + Sync>,
        quote: MeltQuote,
    ) -> Result<(), Error> {
        tracing::debug!(
            "Starting background payment processing for quote {}",
            quote.id
        );

        let payment_executor = PaymentExecutor::new(payment_processors);

        let payment_result = match payment_executor.execute_payment(&quote).await {
            Ok(result) => result,
            Err(err) => {
                tracing::error!("Payment execution failed for quote {}: {}", quote.id, err);

                let mut tx = localstore.begin_transaction().await?;
                tx.update_melt_quote_state(&quote.id, MeltQuoteState::Failed, None)
                    .await?;
                tx.commit().await?;

                pubsub_manager.melt_quote_status(&quote, None, None, MeltQuoteState::Failed);

                return Err(err);
            }
        };

        let amount_spent =
            to_unit(payment_result.total_spent, &quote.unit, &quote.unit).unwrap_or_default();

        tracing::info!(
            "Payment successful for quote {}, amount spent: {}, finalizing melt",
            quote.id,
            amount_spent
        );

        // Now finalize the melt: burn inputs, calculate change, update state
        let mut tx = localstore.begin_transaction().await?;

        let mut updated_quote = quote.clone();
        if Some(payment_result.payment_lookup_id.clone()).as_ref()
            != quote.request_lookup_id.as_ref()
        {
            tracing::debug!(
                "Payment lookup id changed post payment from {:?} to {}",
                &quote.request_lookup_id,
                payment_result.payment_lookup_id
            );

            updated_quote.request_lookup_id = Some(payment_result.payment_lookup_id.clone());

            if let Err(err) = tx
                .update_melt_quote_request_lookup_id(&quote.id, &payment_result.payment_lookup_id)
                .await
            {
                tracing::warn!("Could not update payment lookup id: {}", err);
            }
        }

        // Get proof Ys and burn inputs
        let input_ys: Vec<_> = tx.get_proof_ys_by_quote_id(&quote.id).await?;

        if input_ys.is_empty() {
            tracing::error!("No proofs found for quote {} during finalization", quote.id);
            return Err(Error::Internal);
        }

        tracing::debug!(
            "Updating {} proof states to Spent for quote {}",
            input_ys.len(),
            quote.id
        );

        // Create proof writer for updating states
        let mut proof_writer = ProofWriter::new(localstore.clone(), pubsub_manager.clone());
        proof_writer
            .update_proofs_states(&mut tx, &input_ys, State::Spent)
            .await?;

        tracing::debug!("Successfully updated proof states to Spent");

        // Update quote state to Paid
        tx.update_melt_quote_state(
            &quote.id,
            MeltQuoteState::Paid,
            payment_result.payment_proof.clone(),
        )
        .await?;

        // Get melt request info for change calculation
        let MeltRequestInfo {
            inputs_amount,
            inputs_fee,
            change_outputs,
        } = tx
            .get_melt_request_and_blinded_messages(&quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        // Calculate and sign change if needed
        let change = if inputs_amount - inputs_fee > amount_spent && !change_outputs.is_empty() {
            tracing::debug!(
                "Calculating change for quote {}: inputs {}, spent {}",
                quote.id,
                inputs_amount,
                amount_spent
            );

            // We need to create a temporary Mint-like structure for ChangeProcessor
            // Instead, let's inline the change calculation logic here
            let change_amount = inputs_amount
                .checked_sub(amount_spent + inputs_fee)
                .ok_or(Error::AmountOverflow)?;

            if change_amount > Amount::ZERO {
                let fee_and_amounts = signatory
                    .keysets()
                    .await?
                    .keysets
                    .iter()
                    .filter_map(|keyset| {
                        if keyset.active
                            && Some(keyset.id) == change_outputs.first().map(|x| x.keyset_id)
                        {
                            Some((keyset.input_fee_ppk, keyset.amounts.clone()).into())
                        } else {
                            None
                        }
                    })
                    .next()
                    .unwrap_or_else(|| {
                        (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into()
                    });

                let mut change_amounts = change_amount.split(&fee_and_amounts);

                if change_outputs.len() < change_amounts.len() {
                    tracing::debug!(
                        "Providing change requires {} blinded messages, but only {} provided",
                        change_amounts.len(),
                        change_outputs.len()
                    );

                    // In the case that not enough outputs are provided to return all change
                    // Reverse sort the amounts so that the most amount of change possible is
                    // returned. The rest is burnt
                    change_amounts.sort_by(|a, b| b.cmp(a));
                }

                tracing::debug!(
                    "Signing {} change outputs for amount {}",
                    change_outputs.len(),
                    change_amount
                );

                let mut blinded_messages = vec![];

                for (amount, mut blinded_message) in
                    change_amounts.iter().zip(change_outputs.clone())
                {
                    blinded_message.amount = *amount;
                    blinded_messages.push(blinded_message);
                }

                let blind_signatures = signatory.blind_sign(blinded_messages.clone()).await?;

                // Extract blinded secrets for storage
                let blinded_secrets: Vec<_> = blinded_messages
                    .iter()
                    .map(|bm| bm.blinded_secret)
                    .collect();

                assert!(blinded_secrets.len() == blind_signatures.len());

                // Store change signatures
                tx.add_blind_signatures(
                    &blinded_secrets,
                    &blind_signatures,
                    Some(quote.id.clone()),
                )
                .await?;

                tracing::debug!("Change signed and stored for quote {}", quote.id);

                Some(blind_signatures)
            } else {
                tracing::debug!("Change amount is zero, no change to return");
                None
            }
        } else {
            if inputs_amount > amount_spent {
                tracing::info!(
                    "Inputs for {} {} greater than spent on melt {} but change outputs not provided.",
                    quote.id,
                    inputs_amount,
                    amount_spent
                );
            } else {
                tracing::debug!("No change required for melt {}", quote.id);
            }
            None
        };

        tx.commit().await?;
        proof_writer.commit();

        // Clean up melt request
        let mut tx = localstore.begin_transaction().await?;
        tx.delete_melt_request(&quote.id).await?;
        tx.commit().await?;

        // Publish final status with change via pubsub
        pubsub_manager.melt_quote_status(
            &updated_quote,
            payment_result.payment_proof,
            change.clone(),
            MeltQuoteState::Paid,
        );

        tracing::info!(
            "Background payment processing and finalization completed successfully for quote {}, change: {}",
            quote.id,
            change.is_some()
        );

        Ok(())
    }
}
