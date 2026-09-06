//! Durable transfers between mint wallets.

use std::collections::HashMap;
use std::fmt;

use super::mint::MintQuoteId;
use super::operation::{OperationId, OperationKind, OperationReference, OperationState};
use super::{MeltConfirmOptions, PreparedMeltPurpose, Wallet, WalletIdentity, WalletManager};
use crate::{Amount, Error};

/// Destination amount behavior for a cross-mint transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CrossMintTransferAmount {
    /// Move the largest destination amount the source balance and mint limits allow.
    #[default]
    Maximum,
    /// Deliver an exact amount at the destination; source fees are additional.
    Exact(Amount),
}

/// Request to move value between two mint wallets.
#[derive(Debug, Clone)]
pub struct CrossMintTransferRequest {
    /// Wallet that pays the Lightning invoice.
    pub source: WalletIdentity,
    /// Wallet that receives newly issued ecash.
    pub destination: WalletIdentity,
    /// Amount to deliver at the destination.
    pub amount: CrossMintTransferAmount,
    /// Application metadata stored with the source transaction.
    pub metadata: HashMap<String, String>,
}

/// Successful cross-mint transfer result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossMintTransferReceipt {
    /// Durable source operation identifier.
    pub operation_id: OperationId,
    /// Destination wallet.
    pub destination: WalletIdentity,
    /// Quote claimed at the destination.
    pub destination_quote_id: MintQuoteId,
    /// Amount issued at the destination.
    pub amount: Amount,
    /// Actual source-side fee.
    pub source_fee: Amount,
}

/// Source payment succeeded, but destination issuance remains claimable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossMintClaimPending {
    /// Durable source operation identifier.
    pub operation_id: OperationId,
    /// Destination wallet.
    pub destination: WalletIdentity,
    /// Paid destination quote that remains claimable.
    pub destination_quote_id: MintQuoteId,
    /// Amount expected at the destination.
    pub amount: Amount,
    /// Actual source-side fee.
    pub source_fee: Amount,
    /// Why immediate issuance failed.
    pub error_message: String,
    /// Whether retrying this same operation can be useful.
    pub retryable: bool,
}

/// Outcome of confirming a cross-mint transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossMintTransferOutcome {
    /// Source payment and destination issuance both completed.
    Completed(CrossMintTransferReceipt),
    /// Source payment completed; destination issuance is durably recoverable.
    ClaimPending(CrossMintClaimPending),
}

/// Durable, reviewable cross-mint transfer plan.
#[derive(Clone)]
#[must_use = "execute or cancel the plan to release its reserved funds"]
pub struct CrossMintTransferPlan {
    source: Wallet,
    destination: Wallet,
    operation_id: OperationId,
    destination_identity: WalletIdentity,
    destination_quote_id: MintQuoteId,
    amount: Amount,
    maximum_fee: Amount,
    allow_swap: bool,
}

impl fmt::Debug for CrossMintTransferPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CrossMintTransferPlan")
            .field("operation_id", &self.operation_id)
            .field("destination", &self.destination_identity)
            .field("destination_quote_id", &self.destination_quote_id)
            .field("amount", &self.amount)
            .field("maximum_fee", &self.maximum_fee)
            .finish_non_exhaustive()
    }
}

impl CrossMintTransferPlan {
    fn from_operation(
        source: Wallet,
        destination: Wallet,
        operation: &cdk_common::wallet::CrossMintTransferOperation,
    ) -> Result<Self, Error> {
        if source.mint_url != operation.source_mint_url
            || source.unit != operation.source_unit
            || destination.mint_url != operation.destination_mint_url
            || destination.unit != operation.destination_unit
        {
            return Err(Error::InvalidOperationState);
        }

        Ok(Self {
            source,
            destination,
            operation_id: operation.operation_id.into(),
            destination_identity: WalletIdentity {
                mint_url: operation.destination_mint_url.clone(),
                unit: operation.destination_unit.clone(),
            },
            destination_quote_id: MintQuoteId::new(operation.destination_quote_id.clone()),
            amount: operation.amount,
            maximum_fee: operation.maximum_fee,
            allow_swap: operation.allow_swap,
        })
    }

    fn from_prepared(
        source: Wallet,
        destination: Wallet,
        prepared: &crate::wallet::PreparedMelt,
        allow_swap: bool,
    ) -> Result<Self, Error> {
        let PreparedMeltPurpose::CrossMintTransfer {
            destination_mint_url,
            destination_unit,
            destination_quote_id,
        } = prepared.purpose()
        else {
            return Err(Error::InvalidOperationState);
        };
        if destination.mint_url != *destination_mint_url || destination.unit != *destination_unit {
            return Err(Error::InvalidOperationState);
        }
        let maximum_fee = if allow_swap {
            prepared
                .quote()
                .fee_reserve
                .checked_add(prepared.swap_fee())
                .and_then(|fee| fee.checked_add(prepared.input_fee()))
                .ok_or(Error::AmountOverflow)?
        } else {
            prepared
                .quote()
                .fee_reserve
                .checked_add(prepared.input_fee_without_swap())
                .ok_or(Error::AmountOverflow)?
        };
        let operation = cdk_common::wallet::CrossMintTransferOperation {
            operation_id: prepared.operation_id(),
            source_mint_url: source.mint_url.clone(),
            source_unit: source.unit.clone(),
            destination_mint_url: destination_mint_url.clone(),
            destination_unit: destination_unit.clone(),
            destination_quote_id: destination_quote_id.clone(),
            amount: prepared.amount(),
            maximum_fee,
            allow_swap,
        };

        Self::from_operation(source, destination, &operation)
    }

    fn completed(&self, amount: Amount, source_fee: Amount) -> CrossMintTransferOutcome {
        CrossMintTransferOutcome::Completed(CrossMintTransferReceipt {
            operation_id: self.operation_id,
            destination: self.destination_identity.clone(),
            destination_quote_id: self.destination_quote_id.clone(),
            amount,
            source_fee,
        })
    }

    /// Durable operation identifier used to resume this plan.
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    /// Amount expected at the destination.
    pub const fn amount(&self) -> Amount {
        self.amount
    }

    /// Maximum combined mint and input fee.
    pub const fn maximum_fee(&self) -> Amount {
        self.maximum_fee
    }

    /// Destination wallet.
    pub fn destination(&self) -> &WalletIdentity {
        &self.destination_identity
    }

    /// Quote that will issue ecash at the destination.
    pub fn destination_quote_id(&self) -> &MintQuoteId {
        &self.destination_quote_id
    }

    /// Pay the destination quote and claim its ecash.
    pub async fn execute(&self) -> Result<CrossMintTransferOutcome, Error> {
        let options = if self.allow_swap {
            MeltConfirmOptions::default()
        } else {
            MeltConfirmOptions::skip_swap()
        };
        let finalized = match self
            .source
            .confirm_prepared_melt_with_options(self.operation_id.as_uuid(), options)
            .await
        {
            Ok(finalized) => finalized,
            Err(Error::InvalidOperationState) => {
                self.source
                    .pending_melt(self.operation_id.as_uuid())
                    .await?
                    .wait()
                    .await?
            }
            Err(error) => return Err(error),
        };
        let source_fee = finalized.fee_paid();

        // Once the source has paid, every destination-side failure must retain
        // that fact in the outcome, including failure to read its local quote.
        // MintSession owns issuance retry and already-claimed receipt handling.
        let claim = async {
            self.destination
                .resume_mint(self.destination_quote_id.clone())
                .await?
                .claim()
                .await
        };
        let outcome = match claim.await {
            Ok(receipt) => self.completed(receipt.amount, source_fee),
            Err(error) => CrossMintTransferOutcome::ClaimPending(CrossMintClaimPending {
                operation_id: self.operation_id,
                destination: self.destination_identity.clone(),
                destination_quote_id: self.destination_quote_id.clone(),
                amount: self.amount,
                source_fee,
                retryable: error.is_retryable(),
                error_message: error.to_string(),
            }),
        };
        self.publish_outcome(&outcome).await;
        Ok(outcome)
    }

    /// Cancel the local plan and release its source funds.
    pub async fn cancel(&self) -> Result<(), Error> {
        self.source
            .cancel_prepared_melt(self.operation_id.as_uuid())
            .await?;
        self.source.publish_operation_event(
            OperationReference::Workflow(self.operation_id),
            OperationKind::Transfer,
            OperationState::Canceled,
            Some(self.amount),
        );
        self.source.publish_balance_event().await;
        Ok(())
    }

    async fn publish_outcome(&self, outcome: &CrossMintTransferOutcome) {
        let state = match outcome {
            CrossMintTransferOutcome::Completed(_) => OperationState::Completed,
            CrossMintTransferOutcome::ClaimPending(_) => OperationState::Pending,
        };
        self.source.publish_operation_event(
            OperationReference::Workflow(self.operation_id),
            OperationKind::Transfer,
            state,
            Some(self.amount),
        );
        self.source.publish_balance_event().await;
        self.destination.publish_balance_event().await;
        self.source
            .publish_transaction_events(self.operation_id.as_uuid())
            .await;
    }
}

impl WalletManager {
    /// Create a durable maximum-balance transfer plan between two mint wallets.
    pub async fn plan_cross_mint_transfer(
        &self,
        request: CrossMintTransferRequest,
    ) -> Result<CrossMintTransferPlan, Error> {
        let source = self.open_wallet(request.source).await?;
        let destination = self.open_wallet(request.destination).await?;
        let allow_swap = matches!(request.amount, CrossMintTransferAmount::Exact(_));
        let prepared = match request.amount {
            CrossMintTransferAmount::Maximum => {
                source
                    .prepare_cross_mint_transfer(&destination, request.metadata)
                    .await?
            }
            CrossMintTransferAmount::Exact(amount) => {
                source
                    .prepare_cross_mint_transfer_exact(&destination, amount, request.metadata)
                    .await?
            }
        };
        match CrossMintTransferPlan::from_prepared(source, destination, &prepared, allow_swap) {
            Ok(plan) => {
                plan.source.publish_operation_event(
                    OperationReference::Workflow(plan.operation_id()),
                    OperationKind::Transfer,
                    OperationState::AwaitingExecution,
                    Some(plan.amount()),
                );
                plan.source.publish_balance_event().await;
                Ok(plan)
            }
            Err(error) => {
                if let Err(cleanup_error) = prepared.cancel().await {
                    tracing::warn!(
                        "Could not cancel cross-mint plan after construction failed: {}",
                        cleanup_error
                    );
                }
                Err(error)
            }
        }
    }

    /// Prepare and execute a transfer between two mint wallets.
    pub async fn transfer(
        &self,
        request: CrossMintTransferRequest,
    ) -> Result<CrossMintTransferOutcome, Error> {
        self.plan_cross_mint_transfer(request)
            .await?
            .execute()
            .await
    }

    /// Resume a cross-mint transfer from its source operation identifier.
    pub async fn resume_transfer(
        &self,
        operation_id: OperationId,
    ) -> Result<CrossMintTransferPlan, Error> {
        for source in self.get_wallets().await {
            let Some(operation) = source
                .cross_mint_transfer_operation(operation_id.as_uuid())
                .await?
            else {
                continue;
            };
            let destination = self
                .get_or_create_wallet(
                    operation.destination_mint_url.clone(),
                    operation.destination_unit.clone(),
                    None,
                )
                .await?;
            return CrossMintTransferPlan::from_operation(source, destination, &operation);
        }

        Err(Error::OperationNotFound)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::wallet::{
        CrossMintTransferOperation, Transaction, TransactionDirection, TransactionStatus,
    };

    use super::*;
    use crate::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod};
    use crate::wallet::test_utils::{create_test_db, create_test_wallet, test_mint_quote};
    use crate::wallet::WalletOpenRequest;

    #[tokio::test]
    async fn destination_errors_preserve_paid_source_and_allow_claim_retry() {
        let store = create_test_db().await;
        let source = create_test_wallet(Arc::clone(&store)).await;
        let destination_identity = WalletIdentity::new(
            "https://destination.example.com".parse().expect("mint URL"),
            CurrencyUnit::Sat,
        );
        let destination = Wallet::open(WalletOpenRequest::new(
            destination_identity.clone(),
            Arc::clone(&store),
            [42; 64],
        ))
        .expect("destination wallet");
        let id = uuid::Uuid::now_v7();
        store
            .add_transaction(Transaction {
                mint_url: source.mint_url.clone(),
                direction: TransactionDirection::Outgoing,
                amount: Amount::from(100),
                fee: Amount::from(2),
                unit: source.unit.clone(),
                ys: vec![],
                timestamp: 1,
                memo: None,
                metadata: HashMap::new(),
                quote_id: Some("settled-source".to_string()),
                payment_request: None,
                payment_proof: None,
                payment_method: Some(PaymentMethod::BOLT11),
                saga_id: Some(id),
                status: TransactionStatus::Completed,
            })
            .await
            .expect("persist source settlement");

        let mut quote = test_mint_quote(destination_identity.mint_url.clone());
        quote.unit = CurrencyUnit::Msat;
        store
            .add_mint_quote(quote.clone())
            .await
            .expect("persist invalid destination quote");
        let operation = CrossMintTransferOperation {
            operation_id: id,
            source_mint_url: source.mint_url.clone(),
            source_unit: source.unit.clone(),
            destination_mint_url: destination_identity.mint_url,
            destination_unit: destination_identity.unit,
            destination_quote_id: quote.id.clone(),
            amount: Amount::from(100),
            maximum_fee: Amount::from(2),
            allow_swap: true,
        };
        let plan = CrossMintTransferPlan::from_operation(source, destination, &operation)
            .expect("resume transfer");
        let CrossMintTransferOutcome::ClaimPending(pending) =
            plan.execute().await.expect("paid source outcome")
        else {
            panic!("destination validation must not hide source settlement");
        };
        assert_eq!(pending.operation_id, id.into());
        assert_eq!(pending.source_fee, Amount::from(2));

        quote.unit = CurrencyUnit::Sat;
        quote.state = MintQuoteState::Issued;
        quote.amount_paid = Amount::from(100);
        quote.amount_issued = Amount::from(100);
        store
            .add_mint_quote(quote)
            .await
            .expect("repair destination record");
        let CrossMintTransferOutcome::Completed(receipt) =
            plan.execute().await.expect("retry same transfer")
        else {
            panic!("issued destination must complete the transfer");
        };
        assert_eq!(receipt.operation_id, id.into());
        assert_eq!(receipt.amount, Amount::from(100));
        assert_eq!(receipt.source_fee, Amount::from(2));
    }
}
