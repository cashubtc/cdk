//! Resume logic for melt sagas after crash recovery.
//!
//! This module handles resuming incomplete melt sagas that were interrupted
//! by a crash. It determines the payment status by querying the mint and
//! either completes the operation or compensates.

use cdk_common::wallet::{MeltOperationData, MeltSagaState, OperationData, WalletSaga};
use cdk_common::{Amount, MeltQuoteState};
use tracing::instrument;

use crate::nuts::State;
use crate::types::FinalizedMelt;
use crate::wallet::melt::saga::compensation::ReleaseMeltQuote;
use crate::wallet::recovery::RecoveryHelpers;
use crate::wallet::saga::{CompensatingAction, RevertProofReservation};
use crate::{Error, Wallet};

impl Wallet {
    /// Resume an incomplete melt saga after crash recovery.
    ///
    /// # Recovery Logic
    ///
    /// - **ProofsReserved**: Proofs reserved but melt not executed.
    ///   Safe to compensate by releasing the reserved proofs. Returns `None`
    ///   since no payment was attempted.
    ///
    /// - **MeltRequested/PaymentPending**: Melt request was sent or payment is pending.
    ///   Check quote state to determine if payment succeeded.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(FinalizedMelt))` - The melt was finalized (paid), compensated (failed/unpaid/never started)
    /// - `Ok(None)` - The melt was skipped (still pending, mint unreachable)
    /// - `Err(e)` - An error occurred during recovery
    #[instrument(skip(self, saga))]
    pub async fn resume_melt_saga(
        &self,
        saga: &WalletSaga,
    ) -> Result<Option<FinalizedMelt>, Error> {
        let state = match &saga.state {
            cdk_common::wallet::WalletSagaState::Melt(s) => s,
            _ => {
                return Err(Error::Custom(format!(
                    "Invalid saga state type for melt saga {}",
                    saga.id
                )))
            }
        };

        let data = match &saga.data {
            OperationData::Melt(d) => d,
            _ => {
                return Err(Error::Custom(format!(
                    "Invalid operation data type for melt saga {}",
                    saga.id
                )))
            }
        };

        match state {
            MeltSagaState::ProofsReserved => {
                // No melt was executed - safe to compensate
                // Return FinalizedMelt with Unpaid state so caller counts it as compensated
                tracing::info!(
                    "Melt saga {} in ProofsReserved state - compensating",
                    saga.id
                );
                self.compensate_melt(&saga.id).await?;
                Ok(Some(FinalizedMelt::new(
                    data.quote_id.clone(),
                    MeltQuoteState::Unpaid,
                    None,
                    data.amount,
                    Amount::ZERO,
                    None,
                )))
            }
            MeltSagaState::MeltRequested | MeltSagaState::PaymentPending => {
                // Melt was requested or payment is pending - check quote state
                tracing::info!(
                    "Melt saga {} in {:?} state - checking quote state",
                    saga.id,
                    state
                );
                self.recover_or_compensate_melt(&saga.id, data).await
            }
        }
    }

    /// Check quote status and either complete melt or compensate.
    ///
    /// Returns `Some(FinalizedMelt)` for finalized melts (paid or failed),
    /// `None` for still-pending melts that should be retried later.
    async fn recover_or_compensate_melt(
        &self,
        saga_id: &uuid::Uuid,
        data: &MeltOperationData,
    ) -> Result<Option<FinalizedMelt>, Error> {
        // Check quote state with the mint
        match self.client.get_melt_quote_status(&data.quote_id).await {
            Ok(quote_status) => match quote_status.state {
                MeltQuoteState::Paid => {
                    // Payment succeeded - mark proofs as spent and recover change
                    tracing::info!("Melt saga {} - payment succeeded, finalizing", saga_id);
                    let melted = self
                        .complete_melt_from_restore(saga_id, data, &quote_status)
                        .await?;
                    Ok(Some(melted))
                }
                MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                    // Payment failed - compensate and return FinalizedMelt with failed state
                    tracing::info!("Melt saga {} - payment failed, compensating", saga_id);
                    self.compensate_melt(saga_id).await?;
                    Ok(Some(FinalizedMelt::new(
                        data.quote_id.clone(),
                        quote_status.state,
                        None,
                        data.amount,
                        Amount::ZERO,
                        None,
                    )))
                }
                MeltQuoteState::Pending | MeltQuoteState::Unknown => {
                    // Still pending or unknown - skip and retry later
                    tracing::info!("Melt saga {} - payment pending/unknown, skipping", saga_id);
                    Ok(None)
                }
            },
            Err(e) => {
                tracing::warn!(
                    "Melt saga {} - can't check quote state ({}), skipping",
                    saga_id,
                    e
                );
                Ok(None)
            }
        }
    }

    /// Complete a melt by marking proofs as spent and restoring change.
    async fn complete_melt_from_restore(
        &self,
        saga_id: &uuid::Uuid,
        data: &MeltOperationData,
        quote_status: &cdk_common::MeltQuoteBolt11Response<String>,
    ) -> Result<FinalizedMelt, Error> {
        // Mark input proofs as spent
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;
        let input_amount =
            Amount::try_sum(reserved_proofs.iter().map(|p| p.proof.amount)).unwrap_or(Amount::ZERO);

        if !reserved_proofs.is_empty() {
            let proof_ys: Vec<_> = reserved_proofs.iter().map(|p| p.y).collect();
            self.localstore
                .update_proofs_state(proof_ys, State::Spent)
                .await?;
        }

        // Try to recover change proofs using stored blinded messages
        let change_proofs = if let Some(ref change_blinded_messages) = data.change_blinded_messages
        {
            if !change_blinded_messages.is_empty() {
                match self
                    .restore_outputs(
                        saga_id,
                        "Melt",
                        Some(change_blinded_messages.as_slice()),
                        data.counter_start,
                        data.counter_end,
                    )
                    .await
                {
                    Ok(Some(change_proof_infos)) => {
                        let proofs: Vec<_> =
                            change_proof_infos.iter().map(|p| p.proof.clone()).collect();
                        self.localstore
                            .update_proofs(change_proof_infos, vec![])
                            .await?;
                        Some(proofs)
                    }
                    Ok(None) => {
                        tracing::warn!(
                            "Melt saga {} - couldn't restore change proofs. \
                             Run wallet.restore() to recover any missing change.",
                            saga_id
                        );
                        None
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Melt saga {} - failed to recover change: {}. \
                             Run wallet.restore() to recover any missing change.",
                            saga_id,
                            e
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            tracing::warn!(
                "Melt saga {} - payment succeeded but no change blinded messages stored. \
                 Run wallet.restore() to recover any missing change.",
                saga_id
            );
            None
        };

        // Calculate fee paid
        let change_amount = change_proofs
            .as_ref()
            .and_then(|p| Amount::try_sum(p.iter().map(|proof| proof.amount)).ok())
            .unwrap_or(Amount::ZERO);
        let fee_paid = input_amount
            .checked_sub(data.amount + change_amount)
            .unwrap_or(Amount::ZERO);

        // Delete the saga record
        self.localstore.delete_saga(saga_id).await?;

        Ok(FinalizedMelt::new(
            data.quote_id.clone(),
            MeltQuoteState::Paid,
            quote_status.payment_preimage.clone(),
            data.amount,
            fee_paid,
            change_proofs,
        ))
    }

    /// Compensate a melt saga by releasing proofs and the melt quote.
    async fn compensate_melt(&self, saga_id: &uuid::Uuid) -> Result<(), Error> {
        // Release melt quote (best-effort, continue on error)
        if let Err(e) = (ReleaseMeltQuote {
            localstore: self.localstore.clone(),
            operation_id: *saga_id,
        }
        .execute()
        .await)
        {
            tracing::warn!(
                "Failed to release melt quote for saga {}: {}. Continuing with saga cleanup.",
                saga_id,
                e
            );
        }

        // Release proofs and delete saga
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;
        let proof_ys = reserved_proofs.iter().map(|p| p.y).collect();

        RevertProofReservation {
            localstore: self.localstore.clone(),
            proof_ys,
            saga_id: *saga_id,
        }
        .execute()
        .await
    }
}

#[cfg(test)]
mod tests {
    // Tests will be moved here from recovery.rs
}
