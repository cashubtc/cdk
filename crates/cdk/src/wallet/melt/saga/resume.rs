//! Resume logic for melt sagas after crash recovery.
//!
//! This module handles resuming incomplete melt sagas that were interrupted
//! by a crash. It determines the payment status by querying the mint and
//! either completes the operation or compensates.

use std::collections::HashMap;

use cdk_common::wallet::{
    MeltOperationData, MeltSagaState, OperationData, Transaction, TransactionDirection,
    TransactionId, WalletSaga,
};
use cdk_common::{Amount, MeltQuoteState};
use tracing::instrument;

use crate::nuts::State;
use crate::types::FinalizedMelt;
use crate::util::unix_time;
use crate::wallet::melt::saga::compensation::ReleaseMeltQuote;
use crate::wallet::melt::MeltQuoteStatusResponse;
use crate::wallet::recovery::RecoveryHelpers;
use crate::wallet::saga::{CompensatingAction, RevertProofReservation};
use crate::{Error, Wallet};

impl Wallet {
    /// Resume an incomplete melt saga after crash recovery.
    ///
    /// Determines the payment status by querying the mint and either
    /// completes the operation or compensates.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(FinalizedMelt))` - The melt was finalized or compensated
    /// - `Ok(None)` - The melt was skipped (still pending, mint unreachable)
    /// - `Err(e)` - An error occurred during recovery
    #[instrument(skip(self, saga))]
    pub(crate) async fn resume_melt_saga(
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
        match self.internal_check_melt_status(&data.quote_id).await {
            Ok(quote_status) => match quote_status.state() {
                MeltQuoteState::Paid => {
                    // Payment succeeded - mark proofs as spent and recover change
                    tracing::info!("Melt saga {} - payment succeeded, finalizing", saga_id);
                    let melted = self
                        .complete_melt_from_restore(saga_id, data, &quote_status)
                        .await?;
                    Ok(melted)
                }
                MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                    // Safety: refuse to compensate while the mint still
                    // reports a payment proof (Lightning preimage or
                    // Onchain outpoint). Treat it as pending so the next
                    // recovery pass can re-check. The outpoint is the
                    // onchain equivalent of the preimage for this rule.
                    if quote_status.payment_proof().is_some() {
                        tracing::warn!(
                            "Melt saga {} - payment reported {:?} but mint holds \
                             a payment proof; keeping pending to avoid loss",
                            saga_id,
                            quote_status.state()
                        );
                        return Ok(None);
                    }
                    // Payment failed - compensate and return FinalizedMelt with failed state
                    tracing::info!("Melt saga {} - payment failed, compensating", saga_id);
                    self.compensate_melt(saga_id).await?;
                    Ok(Some(FinalizedMelt::new(
                        data.quote_id.clone(),
                        quote_status.state(),
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
        quote_status: &MeltQuoteStatusResponse,
    ) -> Result<Option<FinalizedMelt>, Error> {
        // Mark input proofs as spent
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;
        if reserved_proofs.is_empty() {
            tracing::warn!(
                "Melt saga {} - payment succeeded but no melt inputs were found; \
                 skipping final transaction recording.",
                saga_id
            );
            return Ok(None);
        }

        let proof_ys: Vec<_> = reserved_proofs.iter().map(|p| p.y).collect();
        let transaction_id = TransactionId::new(proof_ys.clone());
        let status_payment_proof = quote_status.payment_proof();
        if let Some(existing_transaction) = self.localstore.get_transaction(transaction_id).await? {
            let is_recovered_melt = existing_transaction.direction
                == TransactionDirection::Outgoing
                && existing_transaction.quote_id.as_deref() == Some(data.quote_id.as_str());

            if is_recovered_melt {
                self.localstore
                    .update_proofs_state(proof_ys, State::Spent)
                    .await?;

                let mut payment_proof = status_payment_proof.clone();
                if let Some(mut quote) = self.localstore.get_melt_quote(&data.quote_id).await? {
                    quote.state = MeltQuoteState::Paid;
                    if payment_proof.is_none() {
                        payment_proof = quote.payment_proof.clone();
                    }
                    if payment_proof.is_none() {
                        payment_proof = existing_transaction.payment_proof.clone();
                    }
                    quote.payment_proof = payment_proof.clone();
                    self.localstore.add_melt_quote(quote).await?;
                } else if payment_proof.is_none() {
                    payment_proof = existing_transaction.payment_proof.clone();
                }

                if let Err(e) = self.localstore.release_melt_quote(saga_id).await {
                    tracing::warn!(
                        "Failed to release melt quote for saga {} after recovery finalization: {}",
                        saga_id,
                        e
                    );
                }

                self.localstore.delete_saga(saga_id).await?;

                return Ok(Some(FinalizedMelt::new(
                    data.quote_id.clone(),
                    MeltQuoteState::Paid,
                    payment_proof,
                    data.amount,
                    existing_transaction.fee,
                    None,
                )));
            }
        }

        let input_amount =
            Amount::try_sum(reserved_proofs.iter().map(|p| p.proof.amount)).unwrap_or(Amount::ZERO);

        self.localstore
            .update_proofs_state(proof_ys.clone(), State::Spent)
            .await?;

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
            .checked_sub(data.amount.checked_add(change_amount).unwrap_or_default())
            .unwrap_or(Amount::ZERO);

        let mut payment_request = None;
        let mut payment_proof = status_payment_proof;
        let mut payment_method = None;

        if let Some(mut quote) = self.localstore.get_melt_quote(&data.quote_id).await? {
            quote.state = MeltQuoteState::Paid;
            if payment_proof.is_none() {
                payment_proof = quote.payment_proof.clone();
            }
            quote.payment_proof = payment_proof.clone();
            payment_request = Some(quote.request.clone());
            payment_method = Some(quote.payment_method.clone());
            self.localstore.add_melt_quote(quote).await?;
        }

        self.localstore
            .add_transaction(Transaction {
                mint_url: self.mint_url.clone(),
                direction: TransactionDirection::Outgoing,
                amount: data.amount,
                fee: fee_paid,
                unit: self.unit.clone(),
                ys: proof_ys,
                timestamp: unix_time(),
                memo: None,
                metadata: HashMap::new(),
                quote_id: Some(data.quote_id.clone()),
                payment_request,
                payment_proof: payment_proof.clone(),
                payment_method,
                saga_id: Some(*saga_id),
            })
            .await?;

        if let Err(e) = self.localstore.release_melt_quote(saga_id).await {
            tracing::warn!(
                "Failed to release melt quote for saga {} after recovery finalization: {}",
                saga_id,
                e
            );
        }

        self.localstore.delete_saga(saga_id).await?;

        Ok(Some(FinalizedMelt::new(
            data.quote_id.clone(),
            MeltQuoteState::Paid,
            payment_proof,
            data.amount,
            fee_paid,
            change_proofs,
        )))
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
    use std::collections::HashMap;
    use std::sync::Arc;

    use cdk_common::nuts::{CurrencyUnit, PaymentMethod, State};
    use cdk_common::wallet::{
        MeltOperationData, MeltSagaState, OperationData, Transaction, TransactionDirection,
        WalletSaga, WalletSagaState,
    };
    use cdk_common::{Amount, MeltQuoteBolt11Response, MeltQuoteState};

    use crate::wallet::saga::test_utils::{
        create_test_db, test_keyset_id, test_mint_url, test_proof_info,
    };
    use crate::wallet::test_utils::{
        create_test_wallet_with_mock, test_melt_quote, MockMintConnector,
    };

    #[tokio::test]
    async fn test_recover_melt_proofs_reserved() {
        // Compensate: proofs released, quote released
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_melt_quote_{}", uuid::Uuid::new_v4());

        // Create and reserve proofs
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Store melt quote before reserving it
        let mut melt_quote = test_melt_quote();
        melt_quote.id = quote_id.clone();
        db.add_melt_quote(melt_quote).await.unwrap();
        db.reserve_melt_quote(&quote_id, &saga_id).await.unwrap();

        // Create saga in ProofsReserved state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::ProofsReserved),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id,
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Create wallet and recover
        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_melt_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .unwrap();

        // Verify compensation
        assert!(result.is_some());
        let finalized = result.unwrap();
        assert_eq!(finalized.state(), MeltQuoteState::Unpaid);

        // Proofs should be back to Unspent
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        // Saga should be deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_melt_proofs_reserved_without_operation_link_leaves_reserved_proof() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_melt_quote_{}", uuid::Uuid::new_v4());

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.update_proofs_state(vec![proof_y], State::Reserved)
            .await
            .unwrap();

        let mut melt_quote = test_melt_quote();
        melt_quote.id = quote_id.clone();
        db.add_melt_quote(melt_quote).await.unwrap();
        db.reserve_melt_quote(&quote_id, &saga_id).await.unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::ProofsReserved),
            Amount::from(100),
            mint_url,
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id,
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_melt_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .unwrap();

        assert!(result.is_some());
        assert_eq!(result.unwrap().state(), MeltQuoteState::Unpaid);

        let reserved = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(reserved.len(), 1);
        assert_eq!(reserved[0].state, State::Reserved);
        assert_eq!(reserved[0].used_by_operation, None);

        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_melt_melt_requested_quote_paid() {
        // Mock: quote Paid → complete melt, get change
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_melt_quote_{}", uuid::Uuid::new_v4());

        // Create and reserve proofs
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create saga in MeltRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Store melt quote
        let mut melt_quote = test_melt_quote();
        melt_quote.id = quote_id.clone();
        db.add_melt_quote(melt_quote).await.unwrap();

        // Mock: quote is Paid
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id.clone(),
            state: MeltQuoteState::Paid,
            expiry: 9999999999,
            fee_reserve: Amount::from(10),
            amount: Amount::from(100),
            request: Some("lnbc100...".to_string()),
            payment_preimage: Some("preimage123".to_string()),
            change: None,
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::BOLT11,
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_melt_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .unwrap();

        // Verify melt completed
        assert!(result.is_some());
        let finalized = result.unwrap();
        assert_eq!(finalized.state(), MeltQuoteState::Paid);

        // Proofs should be marked spent
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Spent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        // Saga should be deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());

        // Transaction history should contain the recovered paid melt.
        let transactions = wallet
            .list_transactions(Some(TransactionDirection::Outgoing))
            .await
            .unwrap();
        assert_eq!(transactions.len(), 1);

        let transaction = &transactions[0];
        assert_eq!(transaction.quote_id.as_deref(), Some(quote_id.as_str()));
        assert_eq!(transaction.payment_request.as_deref(), Some("lnbc1000..."));
        assert_eq!(transaction.payment_proof.as_deref(), Some("preimage123"));
        assert_eq!(transaction.saga_id, Some(saga_id));
        assert_eq!(transaction.amount, Amount::from(100));
        assert_eq!(transaction.fee, Amount::ZERO);

        let quote = db.get_melt_quote(&quote_id).await.unwrap().unwrap();
        assert_eq!(quote.state, MeltQuoteState::Paid);
        assert_eq!(quote.payment_proof.as_deref(), Some("preimage123"));
        assert_eq!(quote.used_by_operation, None);
    }

    #[tokio::test]
    async fn test_recover_melt_paid_preserves_stored_quote_payment_proof_when_status_has_none() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_melt_quote_{}", uuid::Uuid::new_v4());

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(100),
            mint_url,
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        let mut melt_quote = test_melt_quote();
        melt_quote.id = quote_id.clone();
        melt_quote.payment_proof = Some("stored-preimage".to_string());
        db.add_melt_quote(melt_quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id.clone(),
            state: MeltQuoteState::Paid,
            expiry: 9999999999,
            fee_reserve: Amount::from(10),
            amount: Amount::from(100),
            request: Some("lnbc100...".to_string()),
            payment_preimage: None,
            change: None,
            unit: Some(CurrencyUnit::Sat),
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let finalized = wallet
            .resume_melt_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .unwrap()
            .expect("paid melt should finalize");

        assert_eq!(finalized.payment_proof(), Some("stored-preimage"));

        let transactions = wallet
            .list_transactions(Some(TransactionDirection::Outgoing))
            .await
            .unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(
            transactions[0].payment_proof.as_deref(),
            Some("stored-preimage")
        );

        let quote = db.get_melt_quote(&quote_id).await.unwrap().unwrap();
        assert_eq!(quote.state, MeltQuoteState::Paid);
        assert_eq!(quote.payment_proof.as_deref(), Some("stored-preimage"));
        assert_eq!(quote.used_by_operation, None);
    }

    #[tokio::test]
    async fn test_recover_melt_paid_skips_transaction_when_inputs_missing() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_melt_quote_{}", uuid::Uuid::new_v4());

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(100),
            mint_url,
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        let mut melt_quote = test_melt_quote();
        melt_quote.id = quote_id.clone();
        melt_quote.used_by_operation = Some(saga_id.to_string());
        db.add_melt_quote(melt_quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id,
            state: MeltQuoteState::Paid,
            expiry: 9999999999,
            fee_reserve: Amount::from(10),
            amount: Amount::from(100),
            request: Some("lnbc100...".to_string()),
            payment_preimage: Some("preimage123".to_string()),
            change: None,
            unit: Some(CurrencyUnit::Sat),
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_melt_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .unwrap();

        assert!(result.is_none());
        assert!(db.get_saga(&saga_id).await.unwrap().is_some());
        assert!(db
            .list_transactions(None, None, None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_recover_melt_paid_preserves_existing_transaction() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_melt_quote_{}", uuid::Uuid::new_v4());

        let mut proof_info = test_proof_info(keyset_id, 1200, mint_url.clone(), State::Pending);
        proof_info.used_by_operation = Some(saga_id);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(1000),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(1000),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        let mut melt_quote = test_melt_quote();
        melt_quote.id = quote_id.clone();
        melt_quote.used_by_operation = Some(saga_id.to_string());
        let payment_method = melt_quote.payment_method.clone();
        db.add_melt_quote(melt_quote).await.unwrap();

        let mut existing_metadata = HashMap::new();
        existing_metadata.insert("label".to_string(), "original metadata".to_string());
        db.add_transaction(Transaction {
            mint_url,
            direction: TransactionDirection::Outgoing,
            amount: Amount::from(1000),
            fee: Amount::from(50),
            unit: CurrencyUnit::Sat,
            ys: vec![proof_y],
            timestamp: 42,
            memo: Some("original memo".to_string()),
            metadata: existing_metadata.clone(),
            quote_id: Some(quote_id.clone()),
            payment_request: Some("original request".to_string()),
            payment_proof: Some("original proof".to_string()),
            payment_method: Some(payment_method),
            saga_id: Some(saga_id),
        })
        .await
        .unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id.clone(),
            state: MeltQuoteState::Paid,
            expiry: 9999999999,
            fee_reserve: Amount::from(10),
            amount: Amount::from(1000),
            request: Some("lnbc1000...".to_string()),
            payment_preimage: Some("preimage123".to_string()),
            change: None,
            unit: Some(CurrencyUnit::Sat),
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_melt_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .unwrap();

        let finalized = result.expect("existing transaction should finalize cleanup");
        assert_eq!(finalized.state(), MeltQuoteState::Paid);
        assert_eq!(finalized.fee_paid(), Amount::from(50));
        assert!(finalized.change().is_none());

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].timestamp, 42);
        assert_eq!(transactions[0].fee, Amount::from(50));
        assert_eq!(transactions[0].memo.as_deref(), Some("original memo"));
        assert_eq!(transactions[0].metadata, existing_metadata);
        assert_eq!(
            transactions[0].payment_request.as_deref(),
            Some("original request")
        );
        assert_eq!(
            transactions[0].payment_proof.as_deref(),
            Some("original proof")
        );

        let stored_input = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored_input.len(), 1);
        assert_eq!(stored_input[0].state, State::Spent);

        let quote = db.get_melt_quote(&quote_id).await.unwrap().unwrap();
        assert_eq!(quote.state, MeltQuoteState::Paid);
        assert_eq!(quote.payment_proof.as_deref(), Some("preimage123"));
        assert_eq!(quote.used_by_operation, None);
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_melt_paid_uses_existing_transaction_payment_proof_fallback() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_melt_quote_{}", uuid::Uuid::new_v4());

        let mut proof_info = test_proof_info(keyset_id, 1200, mint_url.clone(), State::Pending);
        proof_info.used_by_operation = Some(saga_id);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(1000),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(1000),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        let mut melt_quote = test_melt_quote();
        melt_quote.id = quote_id.clone();
        melt_quote.used_by_operation = Some(saga_id.to_string());
        let payment_method = melt_quote.payment_method.clone();
        db.add_melt_quote(melt_quote).await.unwrap();

        db.add_transaction(Transaction {
            mint_url,
            direction: TransactionDirection::Outgoing,
            amount: Amount::from(1000),
            fee: Amount::from(50),
            unit: CurrencyUnit::Sat,
            ys: vec![proof_y],
            timestamp: 42,
            memo: Some("original memo".to_string()),
            metadata: HashMap::new(),
            quote_id: Some(quote_id.clone()),
            payment_request: Some("original request".to_string()),
            payment_proof: Some("transaction proof".to_string()),
            payment_method: Some(payment_method),
            saga_id: Some(saga_id),
        })
        .await
        .unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id.clone(),
            state: MeltQuoteState::Paid,
            expiry: 9999999999,
            fee_reserve: Amount::from(10),
            amount: Amount::from(1000),
            request: Some("lnbc1000...".to_string()),
            payment_preimage: None,
            change: None,
            unit: Some(CurrencyUnit::Sat),
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let finalized = wallet
            .resume_melt_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .unwrap()
            .expect("existing transaction should finalize cleanup");

        assert_eq!(finalized.state(), MeltQuoteState::Paid);
        assert_eq!(finalized.payment_proof(), Some("transaction proof"));
        assert_eq!(finalized.fee_paid(), Amount::from(50));

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(
            transactions[0].payment_proof.as_deref(),
            Some("transaction proof")
        );

        let quote = db.get_melt_quote(&quote_id).await.unwrap().unwrap();
        assert_eq!(quote.state, MeltQuoteState::Paid);
        assert_eq!(quote.payment_proof.as_deref(), Some("transaction proof"));
        assert_eq!(quote.used_by_operation, None);
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_melt_paid_ignores_existing_incoming_transaction() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_melt_quote_{}", uuid::Uuid::new_v4());

        let mut proof_info = test_proof_info(keyset_id, 1200, mint_url.clone(), State::Pending);
        proof_info.used_by_operation = Some(saga_id);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(1000),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(1000),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        let mut melt_quote = test_melt_quote();
        melt_quote.id = quote_id.clone();
        melt_quote.used_by_operation = Some(saga_id.to_string());
        let payment_method = melt_quote.payment_method.clone();
        db.add_melt_quote(melt_quote).await.unwrap();

        db.add_transaction(Transaction {
            mint_url,
            direction: TransactionDirection::Incoming,
            amount: Amount::from(1200),
            fee: Amount::ZERO,
            unit: CurrencyUnit::Sat,
            ys: vec![proof_y],
            timestamp: 42,
            memo: Some("original incoming".to_string()),
            metadata: HashMap::new(),
            quote_id: Some("mint_quote".to_string()),
            payment_request: Some("mint request".to_string()),
            payment_proof: None,
            payment_method: None,
            saga_id: Some(uuid::Uuid::new_v4()),
        })
        .await
        .unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id.clone(),
            state: MeltQuoteState::Paid,
            expiry: 9999999999,
            fee_reserve: Amount::from(10),
            amount: Amount::from(1000),
            request: Some("lnbc1000...".to_string()),
            payment_preimage: Some("preimage123".to_string()),
            change: None,
            unit: Some(CurrencyUnit::Sat),
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_melt_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .unwrap();

        let finalized = result.expect("paid melt should finalize");
        assert_eq!(finalized.state(), MeltQuoteState::Paid);
        assert_eq!(finalized.fee_paid(), Amount::from(200));

        let incoming_transactions = wallet
            .list_transactions(Some(TransactionDirection::Incoming))
            .await
            .unwrap();
        assert!(incoming_transactions.is_empty());

        let outgoing_transactions = wallet
            .list_transactions(Some(TransactionDirection::Outgoing))
            .await
            .unwrap();
        assert_eq!(outgoing_transactions.len(), 1);

        let transaction = &outgoing_transactions[0];
        assert_eq!(transaction.direction, TransactionDirection::Outgoing);
        assert_eq!(transaction.amount, Amount::from(1000));
        assert_eq!(transaction.fee, Amount::from(200));
        assert_eq!(transaction.quote_id.as_deref(), Some(quote_id.as_str()));
        assert_eq!(transaction.payment_request.as_deref(), Some("lnbc1000..."));
        assert_eq!(transaction.payment_proof.as_deref(), Some("preimage123"));
        assert_eq!(transaction.payment_method, Some(payment_method));
        assert_eq!(transaction.saga_id, Some(saga_id));

        let stored_input = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored_input.len(), 1);
        assert_eq!(stored_input[0].state, State::Spent);

        let quote = db.get_melt_quote(&quote_id).await.unwrap().unwrap();
        assert_eq!(quote.state, MeltQuoteState::Paid);
        assert_eq!(quote.payment_proof.as_deref(), Some("preimage123"));
        assert_eq!(quote.used_by_operation, None);
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_melt_melt_requested_quote_unpaid() {
        // Mock: quote Unpaid → compensate
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_melt_quote_{}", uuid::Uuid::new_v4());

        // Create and reserve proofs
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create saga in MeltRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Store melt quote
        let mut melt_quote = test_melt_quote();
        melt_quote.id = quote_id.clone();
        db.add_melt_quote(melt_quote).await.unwrap();

        // Mock: quote is Unpaid
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id,
            state: MeltQuoteState::Unpaid,
            expiry: 9999999999,
            fee_reserve: Amount::from(10),
            amount: Amount::from(100),
            request: Some("lnbc100...".to_string()),
            payment_preimage: None,
            change: None,
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::BOLT11,
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_melt_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .unwrap();

        // Verify compensation
        assert!(result.is_some());
        let finalized = result.unwrap();
        assert!(
            finalized.state() == MeltQuoteState::Unpaid
                || finalized.state() == MeltQuoteState::Failed
        );

        // Proofs should be released
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        // Saga should be deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_melt_melt_requested_quote_pending() {
        // Mock: quote Pending → skip
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_melt_quote_{}", uuid::Uuid::new_v4());

        // Create and reserve proofs
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create saga in MeltRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Store melt quote
        let mut melt_quote = test_melt_quote();
        melt_quote.id = quote_id.clone();
        db.add_melt_quote(melt_quote).await.unwrap();

        // Mock: quote is Pending (no payment_proof)
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id,
            state: MeltQuoteState::Pending,
            expiry: 9999999999,
            fee_reserve: Amount::from(10),
            amount: Amount::from(100),
            request: Some("lnbc100...".to_string()),
            payment_preimage: None,
            change: None,
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::BOLT11,
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_melt_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .unwrap();

        // Should skip (None returned for pending)
        assert!(result.is_none());

        // Proofs should still be reserved
        let reserved = db.get_reserved_proofs(&saga_id).await.unwrap();
        assert_eq!(reserved.len(), 1);

        // Saga should still exist
        assert!(db.get_saga(&saga_id).await.unwrap().is_some());
    }
}
