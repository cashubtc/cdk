//! Ecash send plans and their application-facing lifecycle.

use std::collections::HashMap;
use std::fmt;

use crate::nuts::Token;
use crate::wallet::advanced::SendAdvancedOptions;
use crate::wallet::operation::{OperationId, OperationKind, OperationReference, OperationState};
use crate::wallet::{SendMemo, SendOptions, Wallet};
use crate::{Amount, Error};

/// Whether a send may contact the mint to obtain suitable denominations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SendMode {
    /// Contact the mint when an exact local proof selection is unavailable.
    #[default]
    OnlineExact,
    /// Prefer local proofs whose overpayment stays within `tolerance`, then
    /// contact the mint when necessary.
    OnlineTolerant {
        /// Maximum acceptable local overpayment.
        tolerance: Amount,
    },
    /// Never contact the mint and require an exact local proof selection.
    OfflineExact,
    /// Never contact the mint and allow local overpayment up to `tolerance`.
    OfflineTolerant {
        /// Maximum acceptable local overpayment.
        tolerance: Amount,
    },
}

impl From<SendMode> for crate::wallet::SendKind {
    fn from(value: SendMode) -> Self {
        match value {
            SendMode::OnlineExact => Self::OnlineExact,
            SendMode::OnlineTolerant { tolerance } => Self::OnlineTolerance(tolerance),
            SendMode::OfflineExact => Self::OfflineExact,
            SendMode::OfflineTolerant { tolerance } => Self::OfflineTolerance(tolerance),
        }
    }
}

/// High-level request to send an encoded ecash token.
#[derive(Debug, Clone)]
pub struct SendRequest {
    /// Value to transfer.
    pub amount: Amount,
    /// Online/offline proof selection behavior.
    pub mode: SendMode,
    /// Memo embedded in the token.
    pub memo: Option<String>,
    /// Add input fees so the receiver obtains the exact requested value.
    pub include_fee: bool,
    /// Application metadata stored with the transaction.
    pub metadata: HashMap<String, String>,
    /// Protocol-specific controls omitted from ordinary sends.
    pub(crate) advanced: SendAdvancedOptions,
}

impl SendRequest {
    /// Create an online send with exact receiver value and no memo.
    pub fn new(amount: Amount) -> Self {
        Self {
            amount,
            mode: SendMode::default(),
            memo: None,
            include_fee: true,
            metadata: HashMap::new(),
            advanced: SendAdvancedOptions::default(),
        }
    }

    /// Embed a memo in the resulting token.
    pub fn with_memo(mut self, memo: impl Into<String>) -> Self {
        self.memo = Some(memo.into());
        self
    }

    /// Apply proof-locking or denomination controls for an expert workflow.
    pub fn with_advanced(mut self, advanced: SendAdvancedOptions) -> Self {
        self.advanced = advanced;
        self
    }
}

/// Receipt for a confirmed ecash send.
#[derive(Clone)]
pub struct SendReceipt {
    /// Durable operation identifier.
    pub operation_id: OperationId,
    /// Encoded token value.
    pub amount: Amount,
    /// Fee reserved for this send.
    pub fee: Amount,
    /// Token to deliver to the receiver.
    pub token: Token,
}

impl fmt::Debug for SendReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendReceipt")
            .field("operation_id", &self.operation_id)
            .field("amount", &self.amount)
            .field("fee", &self.fee)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

/// Status of a confirmed send whose token may still be reclaimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SendStatus {
    /// The receiver has not yet spent the token.
    Unclaimed,
    /// The receiver has claimed the token.
    Claimed,
}

impl fmt::Display for SendStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unclaimed => "unclaimed",
            Self::Claimed => "claimed",
        })
    }
}

/// Reviewable, durable ecash send plan.
#[derive(Clone)]
#[must_use = "execute or cancel the plan to release its reserved funds"]
pub struct SendPlan {
    wallet: Wallet,
    operation_id: OperationId,
    amount: Amount,
    fee: Amount,
}

impl fmt::Debug for SendPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendPlan")
            .field("operation_id", &self.operation_id)
            .field("amount", &self.amount)
            .field("fee", &self.fee)
            .finish_non_exhaustive()
    }
}

impl SendPlan {
    pub(super) fn from_prepared(
        wallet: Wallet,
        prepared: &crate::wallet::PreparedSend,
    ) -> Result<Self, Error> {
        Ok(Self {
            wallet,
            operation_id: prepared.operation_id().into(),
            amount: prepared.amount(),
            fee: prepared.fee()?,
        })
    }

    /// Durable operation identifier used to resume this plan.
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    /// Requested value, before receiver fees or tolerated overpayment.
    pub const fn amount(&self) -> Amount {
        self.amount
    }

    /// Maximum fee reserved by this plan.
    pub const fn fee(&self) -> Amount {
        self.fee
    }

    /// Execute the plan and create the token.
    pub async fn execute(&self) -> Result<SendReceipt, Error> {
        let token = self
            .wallet
            .confirm_send(self.operation_id.as_uuid(), None)
            .await?;
        let receipt = SendReceipt {
            operation_id: self.operation_id,
            amount: token.value()?,
            fee: self.fee,
            token,
        };
        self.wallet.publish_operation_event(
            OperationReference::Workflow(self.operation_id),
            OperationKind::Send,
            OperationState::Pending,
            Some(self.amount),
        );
        self.wallet.publish_balance_event().await;
        self.wallet
            .publish_transaction_events(self.operation_id.as_uuid())
            .await;
        Ok(receipt)
    }

    /// Cancel the plan and release its reserved funds.
    pub async fn cancel(&self) -> Result<(), Error> {
        self.wallet.cancel_send(self.operation_id.as_uuid()).await?;
        self.wallet.publish_operation_event(
            OperationReference::Workflow(self.operation_id),
            OperationKind::Send,
            OperationState::Canceled,
            Some(self.amount),
        );
        self.wallet.publish_balance_event().await;
        Ok(())
    }
}

impl Wallet {
    /// Select and reserve funds for an ecash transfer.
    pub async fn plan_send(&self, request: SendRequest) -> Result<SendPlan, Error> {
        let options = SendOptions {
            memo: request.memo.map(|memo| SendMemo::for_token(&memo)),
            send_kind: request.mode.into(),
            include_fee: request.include_fee,
            metadata: request.metadata,
            conditions: request.advanced.conditions,
            amount_split_target: request.advanced.amount_split_target,
            max_proofs: request.advanced.max_proofs,
            use_p2bk: request.advanced.use_p2bk,
            p2pk_signing_keys: request.advanced.p2pk_signing_keys,
            p2pk_locked_proof_send_mode: request.advanced.locked_proof_policy.into(),
        };
        let prepared = self.prepare_send(request.amount, options).await?;
        match SendPlan::from_prepared(self.clone(), &prepared) {
            Ok(plan) => {
                self.publish_operation_event(
                    OperationReference::Workflow(plan.operation_id()),
                    OperationKind::Send,
                    OperationState::AwaitingExecution,
                    Some(plan.amount()),
                );
                self.publish_balance_event().await;
                Ok(plan)
            }
            Err(error) => {
                if let Err(cleanup_error) = prepared.cancel().await {
                    tracing::warn!(
                        "Could not cancel send plan after construction failed: {}",
                        cleanup_error
                    );
                }
                Err(error)
            }
        }
    }

    /// Prepare and execute an ecash send with the requested policy.
    pub async fn send(&self, request: SendRequest) -> Result<SendReceipt, Error> {
        self.plan_send(request).await?.execute().await
    }

    /// Resume a prepared send after a process restart.
    pub async fn resume_send(&self, operation_id: OperationId) -> Result<SendPlan, Error> {
        let saga = self
            .localstore
            .get_saga(&operation_id.as_uuid())
            .await?
            .ok_or(Error::OperationNotFound)?;
        if saga.mint_url != self.mint_url || saga.unit != self.unit {
            return Err(Error::InvalidOperationState);
        }
        match saga.state {
            cdk_common::wallet::WalletSagaState::Send(
                cdk_common::wallet::SendSagaState::Prepared,
            ) => {
                let prepared = self.prepared_send(operation_id.as_uuid()).await?;
                SendPlan::from_prepared(self.clone(), &prepared)
            }
            cdk_common::wallet::WalletSagaState::Send(
                cdk_common::wallet::SendSagaState::TokenCreated,
            ) => {
                let fee = self
                    .transactions_for_operation(operation_id.as_uuid())
                    .await?
                    .into_iter()
                    .next()
                    .map(|transaction| transaction.fee)
                    .unwrap_or_default();
                Ok(SendPlan {
                    wallet: self.clone(),
                    operation_id,
                    amount: saga.amount,
                    fee,
                })
            }
            _ => Err(Error::InvalidOperationState),
        }
    }

    /// Check whether a confirmed send has been claimed by its receiver.
    /// Completed sends remain queryable while their transaction history is retained.
    pub async fn send_status(&self, operation_id: OperationId) -> Result<SendStatus, Error> {
        if self.check_send_status(operation_id.as_uuid()).await? {
            self.publish_operation_event(
                OperationReference::Workflow(operation_id),
                OperationKind::Send,
                OperationState::Completed,
                None,
            );
            self.publish_balance_event().await;
            self.publish_transaction_events(operation_id.as_uuid())
                .await;
            Ok(SendStatus::Claimed)
        } else {
            Ok(SendStatus::Unclaimed)
        }
    }

    /// Reclaim an unclaimed send and return the restored value.
    pub async fn reclaim_send(&self, operation_id: OperationId) -> Result<Amount, Error> {
        let amount = self.revoke_send(operation_id.as_uuid()).await?;
        self.publish_operation_event(
            OperationReference::Workflow(operation_id),
            OperationKind::Send,
            OperationState::Canceled,
            Some(amount),
        );
        self.publish_balance_event().await;
        self.publish_transaction_events(operation_id.as_uuid())
            .await;
        Ok(amount)
    }

    /// List confirmed sends whose tokens can still be checked or reclaimed.
    pub async fn pending_send_ids(&self) -> Result<Vec<OperationId>, Error> {
        Ok(self
            .get_pending_sends()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }
}
