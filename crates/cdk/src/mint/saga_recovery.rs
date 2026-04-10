//! Shared saga recovery logic for melt operations.
//!
//! This module contains functions used by both startup recovery and on-demand quote checking
//! to process melt saga outcomes consistently.

use cdk_common::mint::{MeltFinalizationData, MeltQuote, MeltSagaState, Saga, SagaStateEnum};
use cdk_common::nuts::MeltQuoteState;
use cdk_common::payment::MakePaymentResponse;
use tracing::instrument;

use crate::mint::subscription::PubSubManager;
use crate::mint::Mint;
use crate::Error;

/// Process the outcome of a melt saga based on LN payment status.
///
/// This function handles the shared logic for deciding whether to finalize, compensate, or skip
/// a melt operation based on the payment response from the LN backend.
///
/// For the `Paid` case, this delegates to [`super::melt::shared::finalize_melt_quote`] which
/// is the single finalization path — it handles operation recording, saga deletion, and all
/// cleanup atomically.
///
/// # Arguments
/// * `saga` - The melt saga being processed
/// * `quote` - The melt quote associated with the saga
/// * `payment_response` - The payment status from the LN backend
/// * `db` - Database handle
/// * `pubsub` - PubSub manager for notifications
/// * `mint` - Mint instance for signing operations
///
/// # Returns
/// Ok(()) on success, or an error if processing fails
#[instrument(skip_all)]
pub(crate) async fn process_melt_saga_outcome(
    saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
) -> Result<(), Error> {
    match payment_response.status {
        MeltQuoteState::Paid => {
            tracing::info!(
                "Finalizing paid melt quote {} (saga {})",
                quote.id,
                saga.operation_id
            );

            // Persist the exact payment result before finalizing so recovery uses
            // the same durable Finalizing handoff as the in-process finalize path.
            let total_spent = payment_response
                .total_spent
                .convert_to(&quote.unit)
                .map_err(|e| {
                    tracing::error!(
                        "Failed to convert recovered total_spent for quote {}: {:?}",
                        quote.id,
                        e
                    );
                    Error::UnitMismatch
                })?;

            let mut tx = db.begin_transaction().await?;
            let finalization_data = MeltFinalizationData {
                total_spent: total_spent.clone(),
                payment_lookup_id: payment_response.payment_lookup_id.clone(),
                payment_proof: payment_response.payment_proof.clone(),
            };
            tx.update_saga_with_finalization_data(
                &saga.operation_id,
                SagaStateEnum::Melt(MeltSagaState::Finalizing),
                Some(&finalization_data),
            )
            .await?;
            tx.commit().await?;

            // finalize_melt_quote handles the rest of the atomic cleanup:
            // operation recording, saga deletion, and melt request cleanup.
            super::melt::shared::finalize_melt_quote(
                mint,
                db,
                pubsub,
                quote,
                total_spent,
                payment_response.payment_proof.clone(),
                &payment_response.payment_lookup_id,
                Some(saga.operation_id),
            )
            .await?;
        }
        MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
            tracing::info!(
                "Compensating failed melt quote {} (saga {})",
                quote.id,
                saga.operation_id
            );
            let input_ys = db.get_proof_ys_by_operation_id(&saga.operation_id).await?;
            let blinded_secrets = db
                .get_blinded_secrets_by_operation_id(&saga.operation_id)
                .await?;
            super::melt::shared::rollback_melt_quote(
                db,
                pubsub,
                &quote.id,
                &input_ys,
                &blinded_secrets,
                &saga.operation_id,
            )
            .await?;

            quote.state = MeltQuoteState::Unpaid;
        }
        MeltQuoteState::Pending | MeltQuoteState::Unknown => {
            tracing::debug!(
                "Melt quote {} (saga {}) payment status still {}, skipping action",
                quote.id,
                saga.operation_id,
                payment_response.status
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use cdk_common::mint::{OperationKind, Saga};
    use cdk_common::nut00::KnownMethod;
    use cdk_common::nuts::{CurrencyUnit, MeltQuoteBolt11Request, ProofsMethods, State};
    use cdk_common::payment::PaymentIdentifier;
    use cdk_common::{Amount, PaymentMethod};
    use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};

    use super::*;
    use crate::mint::melt::melt_saga::MeltSaga;
    use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};

    #[tokio::test]
    async fn test_paid_outcome_finalizes_and_records_completed_operation() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let (payment_saga, decision) = setup_saga
            .attempt_internal_settlement(&melt_request)
            .await
            .unwrap();
        let _confirmed_saga = payment_saga.make_payment(decision).await.unwrap();

        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let saga = assert_saga_exists(&mint, &operation_id).await;
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("paid_outcome_lookup".to_string()),
            payment_proof: Some("paid_outcome_preimage".to_string()),
            status: MeltQuoteState::Paid,
            total_spent: Amount::from(9_250).with_unit(CurrencyUnit::Sat),
        };

        process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        assert_saga_not_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, Some(State::Spent)).await;

        let completed_operation = mint
            .localstore
            .get_completed_operation(&operation_id)
            .await
            .unwrap()
            .expect("completed operation should be recorded");
        assert_eq!(completed_operation.kind(), OperationKind::Melt);
        assert_eq!(completed_operation.id(), &operation_id);

        let paid_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should exist after finalization");
        assert_eq!(paid_quote.state, MeltQuoteState::Paid);
    }

    #[tokio::test]
    async fn test_failed_outcome_rolls_back_and_deletes_saga() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        drop(setup_saga);

        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let saga = assert_saga_exists(&mint, &operation_id).await;
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("failed_outcome_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Failed,
            total_spent: quote.amount(),
        };

        process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        assert_saga_not_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, None).await;

        let recovered_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist after rollback");
        assert_eq!(recovered_quote.state, MeltQuoteState::Unpaid);
    }

    #[tokio::test]
    async fn test_pending_outcome_leaves_state_unchanged() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let _setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let saga = assert_saga_exists(&mint, &operation_id).await;
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("pending_outcome_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: quote.amount(),
        };

        process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        assert_saga_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;

        let pending_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist");
        assert_eq!(pending_quote.state, MeltQuoteState::Pending);
    }

    #[tokio::test]
    async fn test_unknown_outcome_leaves_state_unchanged() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let _setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let saga = assert_saga_exists(&mint, &operation_id).await;
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("unknown_outcome_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Unknown,
            total_spent: quote.amount(),
        };

        process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        assert_saga_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;

        let pending_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist");
        assert_eq!(pending_quote.state, MeltQuoteState::Pending);
    }

    #[tokio::test]
    async fn test_paid_outcome_with_unit_mismatch_returns_error_without_mutation() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let _setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let saga = assert_saga_exists(&mint, &operation_id).await;
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("unit_mismatch_lookup".to_string()),
            payment_proof: Some("unit_mismatch_preimage".to_string()),
            status: MeltQuoteState::Paid,
            total_spent: Amount::from(9_250).with_unit(CurrencyUnit::Usd),
        };

        let err = process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, Error::UnitMismatch));
        assert_saga_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;

        let still_pending_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist");
        assert_eq!(still_pending_quote.state, MeltQuoteState::Pending);

        let completed_operation = mint
            .localstore
            .get_completed_operation(&operation_id)
            .await
            .unwrap();
        assert!(completed_operation.is_none());
    }

    async fn create_test_melt_quote(mint: &crate::mint::Mint, amount: Amount) -> MeltQuote {
        use cdk_common::melt::MeltQuoteRequest;

        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Paid,
            check_payment_state: MeltQuoteState::Paid,
            pay_err: false,
            check_err: false,
        };

        let amount_msats: u64 = amount.into();
        let invoice = create_fake_invoice(
            amount_msats,
            serde_json::to_string(&fake_description).unwrap(),
        );

        let request = MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        });

        let quote_response = mint.get_melt_quote(request).await.unwrap();
        let quote_id = quote_response
            .quote()
            .expect("expected single quote response");

        mint.localstore
            .get_melt_quote(quote_id)
            .await
            .unwrap()
            .expect("quote should exist in database")
    }

    fn create_test_melt_request(
        proofs: &cdk_common::nuts::Proofs,
        quote: &MeltQuote,
    ) -> cdk_common::nuts::MeltRequest<cdk_common::QuoteId> {
        cdk_common::nuts::MeltRequest::new(quote.id.clone(), proofs.clone(), None)
    }

    async fn assert_saga_exists(mint: &crate::mint::Mint, operation_id: &uuid::Uuid) -> Saga {
        mint.localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.operation_id == *operation_id)
            .expect("saga should exist in database")
    }

    async fn assert_single_melt_saga_operation_id(mint: &crate::mint::Mint) -> uuid::Uuid {
        let sagas = mint
            .localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await
            .unwrap();

        assert_eq!(sagas.len(), 1, "expected exactly one melt saga");
        sagas[0].operation_id
    }

    async fn assert_saga_not_exists(mint: &crate::mint::Mint, operation_id: &uuid::Uuid) {
        let sagas = mint
            .localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await
            .unwrap();

        assert!(
            !sagas.iter().any(|s| s.operation_id == *operation_id),
            "saga should not exist in database"
        );
    }

    async fn assert_proofs_state(
        mint: &crate::mint::Mint,
        ys: &[cdk_common::PublicKey],
        expected_state: Option<State>,
    ) {
        let states = mint.localstore.get_proofs_states(ys).await.unwrap();

        for state in states {
            assert_eq!(state, expected_state, "proof state mismatch");
        }
    }
}
