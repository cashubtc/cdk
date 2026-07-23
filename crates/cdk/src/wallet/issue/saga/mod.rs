//! Mint (Issue) Saga - Type State Pattern Implementation
//!
//! This module implements the saga pattern for mint operations using the typestate
//! pattern to enforce valid state transitions at compile-time.
//!
//! # State Flow
//!
//! ```text
//! [saga created] ──► SecretsPrepared ──► MintRequested ──► [completed]
//!                         │                    │
//!                         │                    ├─ replay succeeds ────► [completed]
//!                         │                    ├─ restore succeeds ────► [completed]
//!                         │                    └─ restore fails ──────► [compensated] (proofs may be lost*)
//!                         │
//!                         └─ recovery ────────────────────────────────► [compensated]
//! ```
//!
//! *Note: If restore fails after MintRequested, proofs may have been issued but not recovered.
//! Run `wallet.restore()` to attempt full recovery.
//!
//! # States
//!
//! | State | Description |
//! |-------|-------------|
//! | `SecretsPrepared` | Pre-mint secrets created and counter incremented, ready to request signatures |
//! | `MintRequested` | Mint request sent to mint, awaiting signatures for new proofs |
//!
//! # Recovery Outcomes
//!
//! | Outcome | Description |
//! |---------|-------------|
//! | `[completed]` | Minting succeeded, new proofs saved to wallet |
//! | `[compensated]` | Minting failed or rolled back, quote released |

use std::collections::HashMap;

use cdk_common::nut00::KnownMethod;
use cdk_common::wallet::{
    IssueSagaState, MintOperationData, OperationData, ProofInfo, Transaction, TransactionDirection,
    TransactionStatus, WalletSaga, WalletSagaState,
};
use cdk_common::{PaymentMethod, SecretKey};
use tracing::instrument;

use self::compensation::{MintCompensation, ReleaseMintQuote};
use self::state::{Finalized, Initial, Prepared, PreparedMintRequest};
use crate::amount::SplitTarget;
use crate::dhke::{construct_proofs, hash_to_curve};
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{MintRequest, PreMintSecrets, Proofs, SpendingConditions, State};
use crate::util::unix_time;
use crate::wallet::blind_signature::{
    validate_mint_response_signatures, SignatureAmountValidation,
};
use crate::wallet::saga::{
    add_compensation, clear_compensations, execute_compensations, new_compensations, Compensations,
};
use crate::wallet::MintQuote;
use crate::{Amount, Error, Wallet};

pub(crate) mod compensation;
pub(crate) mod resume;
pub(crate) mod state;

fn should_retry_with_legacy_quote_signature(error: &Error) -> bool {
    matches!(
        error,
        Error::SignatureMissingOrInvalid
            | Error::NUT20(crate::nuts::nut20::Error::InvalidSignature)
            | Error::NUT20(crate::nuts::nut20::Error::SignatureMissing)
    )
}

async fn post_mint_request_with_legacy_fallback(
    wallet: &Wallet,
    payment_method: &PaymentMethod,
    mint_request: &PreparedMintRequest,
) -> Result<crate::nuts::MintResponse, Error> {
    match mint_request {
        PreparedMintRequest::Single {
            request,
            quote_info,
            ..
        } => match wallet
            .client
            .post_mint(payment_method, request.clone())
            .await
        {
            Ok(response) => Ok(response),
            Err(error) if should_retry_with_legacy_quote_signature(&error) => {
                let secret_key = match wallet.mint_quote_signing_key(quote_info).await {
                    Ok(Some(secret_key)) => secret_key,
                    Ok(None) => return Err(error),
                    Err(fallback_error) => {
                        tracing::warn!(
                            original_error = %error,
                            fallback_error = %fallback_error,
                            "Could not prepare legacy mint quote signature retry; returning original mint error"
                        );
                        return Err(error);
                    }
                };

                tracing::info!(
                    "Mint request rejected with new NUT-20 signature format; retrying legacy format"
                );

                let mut retry_request = request.clone();
                if let Err(fallback_error) = retry_request.sign_legacy(secret_key) {
                    tracing::warn!(
                        original_error = %error,
                        fallback_error = %fallback_error,
                        "Could not sign legacy mint quote retry; returning original mint error"
                    );
                    return Err(error);
                }

                match wallet.client.post_mint(payment_method, retry_request).await {
                    Ok(response) => Ok(response),
                    Err(fallback_error) => {
                        tracing::warn!(
                            original_error = %error,
                            fallback_error = %fallback_error,
                            "Legacy mint quote signature retry failed; returning original mint error"
                        );
                        Err(error)
                    }
                }
            }
            Err(error) => Err(error),
        },
        PreparedMintRequest::Batch {
            request,
            quote_infos,
            ..
        } => {
            match wallet
                .client
                .post_batch_mint(payment_method, request.clone())
                .await
            {
                Ok(response) => Ok(response),
                Err(error) if should_retry_with_legacy_quote_signature(&error) => {
                    let legacy_signatures = match legacy_batch_signatures(
                        wallet,
                        request,
                        quote_infos,
                    )
                    .await
                    {
                        Ok(Some(legacy_signatures)) => legacy_signatures,
                        Ok(None) => return Err(error),
                        Err(fallback_error) => {
                            tracing::warn!(
                                original_error = %error,
                                fallback_error = %fallback_error,
                                "Could not prepare legacy batch mint quote signature retry; returning original mint error"
                            );
                            return Err(error);
                        }
                    };

                    tracing::info!(
                        "Batch mint request rejected with new NUT-20 signature format; retrying legacy format"
                    );

                    let mut retry_request = request.clone();
                    retry_request.signatures = Some(legacy_signatures);

                    wallet
                        .client
                        .post_batch_mint(payment_method, retry_request)
                        .await
                        .map_err(|fallback_error| {
                            tracing::warn!(
                                original_error = %error,
                                fallback_error = %fallback_error,
                                "Legacy batch mint quote signature retry failed; returning original mint error"
                            );
                            error
                        })
                }
                Err(error) => Err(error),
            }
        }
    }
}

async fn legacy_batch_signatures(
    wallet: &Wallet,
    request: &crate::nuts::BatchMintRequest<String>,
    quote_infos: &[MintQuote],
) -> Result<Option<Vec<Option<String>>>, Error> {
    let Some(signatures) = &request.signatures else {
        return Ok(None);
    };

    if signatures.len() != request.quotes.len() || signatures.len() != quote_infos.len() {
        return Ok(None);
    }

    let mut legacy_signatures = Vec::with_capacity(signatures.len());
    for ((quote_id, quote_info), signature) in
        request.quotes.iter().zip(quote_infos).zip(signatures)
    {
        if quote_info.id.as_str() != quote_id.as_str() {
            return Ok(None);
        }

        if signature.is_some() {
            let Some(secret_key) = wallet.mint_quote_signing_key(quote_info).await? else {
                return Ok(None);
            };
            let legacy_signature = request
                .sign_quote_legacy(quote_id, &secret_key)
                .map_err(|e| Error::Custom(format!("NUT-20 legacy signing failed: {}", e)))?;
            legacy_signatures.push(Some(legacy_signature));
        } else {
            legacy_signatures.push(None);
        }
    }

    Ok(Some(legacy_signatures))
}

/// Saga pattern implementation for mint (issue) operations.
///
/// Uses the typestate pattern to enforce valid state transitions at compile-time.
/// Each state (Initial, Prepared, Finalized) is a distinct type, and operations
/// are only available on the appropriate type.
pub(crate) struct MintSaga<'a, S> {
    /// Wallet reference
    wallet: &'a Wallet,
    /// Compensating actions in LIFO order (most recent first)
    compensations: Compensations,
    /// State-specific data
    state_data: S,
}

impl<'a> MintSaga<'a, Initial> {
    /// Create a new mint saga in the Initial state.
    pub fn new(wallet: &'a Wallet) -> Self {
        let operation_id = uuid::Uuid::now_v7();

        Self {
            wallet,
            compensations: new_compensations(),
            state_data: Initial {
                operation_id,
                keyset_policy: Default::default(),
            },
        }
    }

    /// Prepare common logic for all mint types
    #[allow(clippy::too_many_arguments)]
    async fn prepare_common(
        mut self,
        quote_id: &str,
        quote_info: cdk_common::wallet::MintQuote,
        amount: Amount,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
        fee_and_amounts: cdk_common::amount::FeeAndAmounts,
        active_keyset_id: cdk_common::nut02::Id,
    ) -> Result<MintSaga<'a, Prepared>, Error> {
        // Reserve the quote to prevent concurrent operations from using it
        self.wallet
            .localstore
            .reserve_mint_quote(quote_id, &self.state_data.operation_id)
            .await?;

        // Register compensation to release quote on failure
        add_compensation(
            &mut self.compensations,
            Box::new(ReleaseMintQuote {
                localstore: self.wallet.localstore.clone(),
                operation_id: self.state_data.operation_id,
            }),
        )
        .await;

        // All work after this point has registered compensations.
        // If any step fails, we must run compensations to release the quote
        // rather than leaving it reserved.
        let prepare_result = self
            .prepare_after_reserve(
                quote_id,
                &quote_info,
                amount,
                amount_split_target,
                spending_conditions,
                &fee_and_amounts,
                active_keyset_id,
            )
            .await;

        match prepare_result {
            Ok(prepared) => {
                // Transition to Prepared state
                Ok(MintSaga {
                    wallet: self.wallet,
                    compensations: self.compensations,
                    state_data: prepared,
                })
            }
            Err(e) => {
                if e.is_definitive_failure() {
                    tracing::warn!(
                        "Mint saga prepare failed (definitive): {}. Running compensations.",
                        e
                    );
                    if let Err(comp_err) = execute_compensations(&mut self.compensations).await {
                        tracing::error!("Compensation failed during prepare: {}", comp_err);
                    }
                } else {
                    tracing::warn!("Mint saga prepare failed (ambiguous): {}.", e);
                }
                Err(e)
            }
        }
    }

    /// Fallible prepare logic that runs after the quote has been reserved.
    ///
    /// Separated from `prepare_common` so that the caller can execute
    /// compensations (releasing the reserved quote) if this method fails.
    #[allow(clippy::too_many_arguments)]
    async fn prepare_after_reserve(
        &mut self,
        quote_id: &str,
        quote_info: &cdk_common::wallet::MintQuote,
        amount: Amount,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
        fee_and_amounts: &cdk_common::amount::FeeAndAmounts,
        active_keyset_id: cdk_common::nut02::Id,
    ) -> Result<Prepared, Error> {
        if amount == Amount::ZERO {
            tracing::debug!("Amount mintable 0.");
            return Err(Error::AmountUndefined);
        }

        let unix_time = unix_time();
        if quote_info.expiry < unix_time && quote_info.expiry != 0 {
            tracing::warn!("Attempting to mint with expired quote.");
        }

        let split_target = match amount_split_target {
            SplitTarget::None => {
                self.wallet
                    .determine_split_target_values(amount, fee_and_amounts)
                    .await?
            }
            s => s,
        };

        let premint_secrets = match &spending_conditions {
            Some(spending_conditions) => PreMintSecrets::with_conditions(
                active_keyset_id,
                amount,
                &split_target,
                spending_conditions,
                fee_and_amounts,
            )?,
            None => {
                let amount_split = amount.split_targeted(&split_target, fee_and_amounts)?;
                let num_secrets = amount_split.len() as u32;

                tracing::debug!(
                    "Incrementing keyset {} counter by {}",
                    active_keyset_id,
                    num_secrets
                );

                let new_counter = self
                    .wallet
                    .localstore
                    .increment_keyset_counter(&active_keyset_id, num_secrets)
                    .await?;

                let count = new_counter - num_secrets;

                PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.wallet.seed,
                    amount,
                    &split_target,
                    fee_and_amounts,
                )?
            }
        };

        let mut request = MintRequest {
            quote: quote_id.to_string(),
            outputs: premint_secrets.blinded_messages(),
            signature: None,
        };

        if let Some(secret_key) = self.wallet.mint_quote_signing_key(quote_info).await? {
            request.sign(&secret_key)?;
        } else if quote_info.payment_method.is_bolt12() {
            // Bolt12 requires signature
            tracing::error!("Signature is required for bolt12.");
            return Err(Error::SignatureMissingOrInvalid);
        }

        let operation_id = self.state_data.operation_id;

        // Get counter range for recovery
        let counter_end = self
            .wallet
            .localstore
            .increment_keyset_counter(&active_keyset_id, 0)
            .await?;
        let counter_start = counter_end.saturating_sub(premint_secrets.secrets.len() as u32);

        // Persist saga state for crash recovery
        let saga = WalletSaga::new(
            operation_id,
            WalletSagaState::Issue(IssueSagaState::SecretsPrepared),
            amount,
            self.wallet.mint_url.clone(),
            self.wallet.unit.clone(),
            OperationData::Mint(MintOperationData::new_single(
                quote_id.to_string(),
                amount,
                Some(counter_start),
                Some(counter_end),
                Some(request.outputs.clone()),
            )),
        );

        self.wallet.localstore.add_saga(saga.clone()).await?;

        // Register compensation (deletes saga on failure)
        add_compensation(
            &mut self.compensations,
            Box::new(MintCompensation {
                localstore: self.wallet.localstore.clone(),
                quote_id: quote_id.to_string(),
                saga_id: operation_id,
            }),
        )
        .await;

        Ok(Prepared {
            operation_id: self.state_data.operation_id,
            active_keyset_id,
            premint_secrets,
            mint_request: PreparedMintRequest::Single {
                quote_id: quote_id.to_string(),
                quote_info: quote_info.clone(),
                request,
            },
            payment_method: quote_info.payment_method.clone(),
            keyset_policy: self.state_data.keyset_policy,
            saga,
        })
    }

    /// Prepare the mint operation (single quote).
    ///
    /// This is the first step in the saga. It:
    /// 1. Validates the quote
    /// 2. Creates premint secrets (increments counter if needed)
    /// 3. Prepares the mint request
    #[instrument(skip_all)]
    pub async fn prepare(
        self,
        quote_id: &str,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<MintSaga<'a, Prepared>, Error> {
        let mut quote_info = self
            .wallet
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        tracing::info!(
            "Preparing mint for quote {} with operation {} method {}",
            quote_id,
            self.state_data.operation_id,
            quote_info.payment_method
        );

        let mut amount = quote_info.amount_mintable();

        if amount == Amount::ZERO {
            self.wallet
                .inner_check_mint_quote_status(quote_info.clone())
                .await?;

            quote_info = self
                .wallet
                .localstore
                .get_mint_quote(quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;

            amount = quote_info.amount_mintable();
        }

        let keyset_policy = self.state_data.keyset_policy;
        let active_keyset_id = self
            .wallet
            .active_keyset_with_policy(keyset_policy)
            .await?
            .id;
        let fee_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_by_id_with_policy(active_keyset_id, keyset_policy)
            .await?;

        self.prepare_common(
            quote_id,
            quote_info,
            amount,
            amount_split_target,
            spending_conditions,
            fee_and_amounts,
            active_keyset_id,
        )
        .await
    }

    /// Prepare a batch mint operation for multiple quotes.
    ///
    /// Validates all quotes, reserves them, creates premint secrets for the total amount,
    /// builds a BatchMintRequest with NUT-20 signatures, and persists the saga.
    #[instrument(skip_all)]
    pub async fn prepare_batch(
        mut self,
        quote_ids: &[&str],
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
        external_keys: Option<&std::collections::HashMap<String, SecretKey>>,
    ) -> Result<MintSaga<'a, Prepared>, Error> {
        use crate::nuts::BatchMintRequest;

        if quote_ids.is_empty() {
            return Err(Error::UnknownQuote);
        }

        // Check for duplicates
        let unique: std::collections::HashSet<_> = quote_ids.iter().collect();
        if unique.len() != quote_ids.len() {
            return Err(Error::DuplicateInputs);
        }

        // Load all quotes
        let mut quote_infos: Vec<MintQuote> = Vec::new();
        for quote_id in quote_ids {
            let quote = self
                .wallet
                .localstore
                .get_mint_quote(quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;
            quote_infos.push(quote);
        }

        // Validate all quotes share the same payment method and unit
        let payment_method = quote_infos[0].payment_method.clone();
        let unit = quote_infos[0].unit.clone();

        for quote in &quote_infos {
            if quote.payment_method != payment_method {
                return Err(Error::InvalidPaymentMethod);
            }
            if quote.unit != unit {
                return Err(Error::UnsupportedUnit);
            }
        }

        // Calculate total mintable amount and canonical per-quote amounts.
        // If we refresh a quote state, keep quote_infos and quote_amounts in sync.
        let mut total_amount = Amount::ZERO;
        let mut quote_amounts: Vec<Amount> = Vec::with_capacity(quote_infos.len());
        for quote in &mut quote_infos {
            let mut mintable = quote.amount_mintable();
            if mintable == Amount::ZERO {
                // Refresh quote status
                self.wallet
                    .inner_check_mint_quote_status(quote.clone())
                    .await?;

                let refreshed = self
                    .wallet
                    .localstore
                    .get_mint_quote(&quote.id)
                    .await?
                    .ok_or(Error::UnknownQuote)?;

                mintable = refreshed.amount_mintable();
                *quote = refreshed;
            }

            total_amount += mintable;
            quote_amounts.push(mintable);
        }

        if total_amount == Amount::ZERO {
            return Err(Error::AmountUndefined);
        }

        // Reserve all quotes (with rollback on failure)
        for quote_id in quote_ids {
            self.wallet
                .localstore
                .reserve_mint_quote(quote_id, &self.state_data.operation_id)
                .await?;
        }

        // Register compensation to release all quotes on failure
        add_compensation(
            &mut self.compensations,
            Box::new(ReleaseMintQuote {
                localstore: self.wallet.localstore.clone(),
                operation_id: self.state_data.operation_id,
            }),
        )
        .await;

        // Get active keyset
        let keyset_policy = self.state_data.keyset_policy;
        let active_keyset_id = self
            .wallet
            .active_keyset_with_policy(keyset_policy)
            .await?
            .id;
        let fee_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_by_id_with_policy(active_keyset_id, keyset_policy)
            .await?;

        // Create premint secrets for total amount
        let split_target = match amount_split_target {
            SplitTarget::None => {
                self.wallet
                    .determine_split_target_values(total_amount, &fee_and_amounts)
                    .await?
            }
            s => s,
        };

        let premint_secrets = match &spending_conditions {
            Some(sc) => PreMintSecrets::with_conditions(
                active_keyset_id,
                total_amount,
                &split_target,
                sc,
                &fee_and_amounts,
            )?,
            None => {
                let amount_split = total_amount.split_targeted(&split_target, &fee_and_amounts)?;
                let num_secrets = amount_split.len() as u32;

                tracing::debug!(
                    "Incrementing keyset {} counter by {}",
                    active_keyset_id,
                    num_secrets
                );

                let new_counter = self
                    .wallet
                    .localstore
                    .increment_keyset_counter(&active_keyset_id, num_secrets)
                    .await?;

                let count = new_counter - num_secrets;

                PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.wallet.seed,
                    total_amount,
                    &split_target,
                    &fee_and_amounts,
                )?
            }
        };

        let outputs = premint_secrets.blinded_messages();

        // Create batch mint request
        let mut batch_request = BatchMintRequest {
            quotes: quote_ids.iter().map(|s| s.to_string()).collect(),
            quote_amounts: Some(quote_amounts),
            outputs: outputs.clone(),
            signatures: None,
        };

        // Build signatures for each quote (NUT-20)
        let mut signatures: Vec<Option<String>> = Vec::new();

        for quote in &quote_infos {
            let secret_key = match self.wallet.mint_quote_signing_key(quote).await? {
                Some(secret_key) => Some(secret_key),
                None => external_keys.and_then(|keys| keys.get(&quote.id)).cloned(),
            };

            let requires_signature = secret_key.is_some() || quote.payment_method.is_bolt12();

            if requires_signature {
                let sk = secret_key.ok_or(Error::SignatureMissingOrInvalid)?;
                let sig = batch_request
                    .sign_quote(&quote.id, &sk)
                    .map_err(|e| Error::Custom(format!("NUT-20 signing failed: {}", e)))?;
                signatures.push(Some(sig));
            } else {
                // Quote is unlocked
                signatures.push(None);
            }
        }

        // Check if any quote requires a signature.
        let has_locked = signatures.iter().any(Option::is_some);
        let signatures_to_send = if has_locked { Some(signatures) } else { None };
        batch_request.signatures = signatures_to_send;

        // Get counter range for recovery
        let counter_end = self
            .wallet
            .localstore
            .increment_keyset_counter(&active_keyset_id, 0)
            .await?;
        let counter_start = counter_end.saturating_sub(premint_secrets.secrets.len() as u32);

        // Persist saga state
        let saga = WalletSaga::new(
            self.state_data.operation_id,
            WalletSagaState::Issue(IssueSagaState::SecretsPrepared),
            total_amount,
            self.wallet.mint_url.clone(),
            self.wallet.unit.clone(),
            OperationData::Mint(MintOperationData::new_batch(
                quote_ids.iter().map(|s| s.to_string()).collect(),
                total_amount,
                Some(counter_start),
                Some(counter_end),
                Some(outputs),
            )),
        );

        self.wallet.localstore.add_saga(saga.clone()).await?;

        // Register compensation
        add_compensation(
            &mut self.compensations,
            Box::new(MintCompensation {
                localstore: self.wallet.localstore.clone(),
                quote_id: quote_ids.first().cloned().unwrap_or_default().to_string(),
                saga_id: self.state_data.operation_id,
            }),
        )
        .await;

        Ok(MintSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: Prepared {
                operation_id: self.state_data.operation_id,
                active_keyset_id,
                premint_secrets,
                mint_request: PreparedMintRequest::Batch {
                    quote_ids: quote_ids.iter().map(|s| s.to_string()).collect(),
                    quote_infos,
                    request: batch_request,
                },
                payment_method,
                keyset_policy,
                saga,
            },
        })
    }
}

impl<'a> MintSaga<'a, Prepared> {
    /// Execute the mint operation.
    ///
    /// Posts mint request, verifies DLEQ proofs, constructs and stores proofs,
    /// updates quote state, and records transaction. On success, compensations
    /// are cleared.
    #[instrument(skip_all)]
    pub async fn execute(self) -> Result<MintSaga<'a, Finalized>, Error> {
        let MintSaga {
            wallet,
            mut compensations,
            state_data,
        } = self;

        let Prepared {
            operation_id,
            active_keyset_id,
            premint_secrets,
            mint_request,
            payment_method,
            keyset_policy,
            saga,
        } = state_data;

        let (quote_ids, quote_infos, batch_quote_amounts) = match &mint_request {
            PreparedMintRequest::Single {
                quote_id,
                quote_info,
                ..
            } => (vec![quote_id.clone()], vec![quote_info.clone()], None),
            PreparedMintRequest::Batch {
                quote_ids,
                quote_infos,
                request,
                ..
            } => (
                quote_ids.clone(),
                quote_infos.clone(),
                request.quote_amounts.clone(),
            ),
        };

        tracing::info!(
            "Executing mint for quotes {:?} with operation {}",
            quote_ids,
            operation_id
        );

        let logic_res = async {
            // Get counter range for recovery
            let counter_end = wallet
                .localstore
                .increment_keyset_counter(&active_keyset_id, 0)
                .await?;
            let counter_start =
                counter_end.saturating_sub(premint_secrets.secrets.len() as u32);

            // Get outputs for saga update and for mint call
            let outputs = premint_secrets.blinded_messages();

            // Update saga state to MintRequested BEFORE making the mint call
            // This is write-ahead logging - if we crash after this, recovery knows
            // the mint request may have been sent
            let mut updated_saga = saga.clone();
            updated_saga.update_state(WalletSagaState::Issue(IssueSagaState::MintRequested));
            if let OperationData::Mint(ref mut data) = updated_saga.data {
                data.counter_start = Some(counter_start);
                data.counter_end = Some(counter_end);
                data.blinded_messages = Some(outputs.clone());
            }

            if !wallet.localstore.update_saga(updated_saga).await? {
                return Err(Error::ConcurrentUpdate);
            }

            let transaction_ys = premint_secrets
                .secrets
                .iter()
                .map(|pre_mint| hash_to_curve(pre_mint.secret.as_bytes()))
                .collect::<Result<Vec<_>, _>>()?;
            let first_quote_id = quote_ids.first().cloned();
            let first_quote_request = quote_infos
                .first()
                .map(|quote| quote.request.clone())
                .unwrap_or_default();

            wallet
                .upsert_transaction(Transaction {
                    mint_url: wallet.mint_url.clone(),
                    direction: TransactionDirection::Incoming,
                    amount: saga.amount,
                    fee: Amount::ZERO,
                    unit: wallet.unit.clone(),
                    ys: transaction_ys,
                    timestamp: unix_time(),
                    memo: None,
                    metadata: HashMap::new(),
                    quote_id: first_quote_id.clone(),
                    payment_request: Some(first_quote_request.clone()),
                    payment_proof: None,
                    payment_method: Some(payment_method.clone()),
                    saga_id: Some(operation_id),
                    status: TransactionStatus::Pending,
                })
                .await?;

            let mint_res =
                post_mint_request_with_legacy_fallback(wallet, &payment_method, &mint_request)
                    .await?;

            let keys = wallet
                .keyset_with_policy(active_keyset_id, keyset_policy)
                .await?
                .keys;

            validate_mint_response_signatures(
                wallet,
                &mint_res.signatures,
                premint_secrets.secrets.iter().map(|p| &p.blinded_message),
                SignatureAmountValidation::Exact,
            )
            .await?;

            let proofs = construct_proofs(
                mint_res.signatures,
                premint_secrets.rs(),
                premint_secrets.secrets(),
                &keys,
            )?;

            let minted_amount = proofs.total_amount()?;

            // Extract first quote info before consuming quote_infos
            // Update quote states - for batch, update each quote with its own amount.
            for (index, mut quote_info) in quote_infos.into_iter().enumerate() {
                if payment_method == PaymentMethod::Known(KnownMethod::Bolt11) {
                    quote_info.state = cdk_common::MintQuoteState::Issued;
                }

                let amount_issued = if let Some(ref quote_amounts) = batch_quote_amounts {
                    quote_amounts
                        .get(index)
                        .cloned()
                        .ok_or(Error::AmountUndefined)?
                } else {
                    minted_amount
                };

                quote_info.amount_issued += amount_issued;
                wallet.localstore.add_mint_quote(quote_info.clone()).await?;
            }

            let proof_infos = proofs
                .iter()
                .map(|proof| {
                    ProofInfo::new(
                        proof.clone(),
                        wallet.mint_url.clone(),
                        State::Unspent,
                        wallet.unit.clone(),
                    )
                })
                .collect::<Result<Vec<ProofInfo>, _>>()?;

            wallet.localstore.update_proofs(proof_infos, vec![]).await?;

            // For transaction, use the first quote's request
            wallet
                .upsert_transaction(Transaction {
                    mint_url: wallet.mint_url.clone(),
                    direction: TransactionDirection::Incoming,
                    amount: minted_amount,
                    fee: Amount::ZERO,
                    unit: wallet.unit.clone(),
                    ys: proofs.ys()?,
                    timestamp: unix_time(),
                    memo: None,
                    metadata: HashMap::new(),
                    quote_id: first_quote_id,
                    payment_request: Some(first_quote_request),
                    payment_proof: None,
                    payment_method: Some(payment_method.clone()),
                    saga_id: Some(operation_id),
                    status: TransactionStatus::Completed,
                })
                .await?;

            // Release all mint quote reservations - operation completed successfully
            if let Err(e) = wallet.localstore.release_mint_quote(&operation_id).await {
                tracing::warn!(
                    "Failed to release mint quotes for operation {}: {}. Quotes may remain marked as reserved.",
                    operation_id,
                    e
                );
            }

            Ok(Finalized { proofs })
        }
        .await;

        match logic_res {
            Ok(finalized_data) => {
                clear_compensations(&mut compensations).await;

                if let Err(e) = wallet.localstore.delete_saga(&operation_id).await {
                    tracing::warn!(
                        "Failed to delete mint saga {}: {}. Will be cleaned up on recovery.",
                        operation_id,
                        e
                    );
                }

                Ok(MintSaga {
                    wallet,
                    compensations,
                    state_data: finalized_data,
                })
            }
            Err(e) => {
                if e.is_definitive_failure() {
                    tracing::warn!(
                        "Mint saga execution failed (definitive): {}. Running compensations.",
                        e
                    );
                    wallet
                        .update_transaction_status_by_saga_id(
                            operation_id,
                            TransactionStatus::Failed,
                        )
                        .await?;
                    if let Err(comp_err) = execute_compensations(&mut compensations).await {
                        tracing::error!("Compensation failed: {}", comp_err);
                    }
                } else {
                    tracing::warn!("Mint saga execution failed (ambiguous): {}.", e,);
                }
                Err(e)
            }
        }
    }
}

impl<'a> MintSaga<'a, Finalized> {
    /// Consume the saga and return the minted proofs
    pub fn into_proofs(self) -> Proofs {
        self.state_data.proofs
    }
}

impl<S: std::fmt::Debug> std::fmt::Debug for MintSaga<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintSaga")
            .field("state_data", &self.state_data)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;

    use bitcoin::secp256k1::schnorr::Signature;
    use cdk_common::nuts::MintQuoteState;

    use super::*;
    use crate::nuts::{BatchMintRequest, BlindSignature, BlindedMessage, MintResponse};
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock, test_mint_quote, test_mint_url,
        MockMintConnector,
    };

    fn legacy_mint_quote_msg_to_sign(quote_id: &str, outputs: &[BlindedMessage]) -> Vec<u8> {
        let capacity = quote_id.len() + (outputs.len() * 66);
        let mut msg = Vec::with_capacity(capacity);

        msg.extend_from_slice(quote_id.as_bytes());
        for output in outputs {
            msg.extend_from_slice(output.blinded_secret.to_hex().as_bytes());
        }

        msg
    }

    fn parse_signature(signature: &Option<String>) -> Signature {
        Signature::from_str(signature.as_ref().expect("signature is present"))
            .expect("valid schnorr signature")
    }

    fn paid_signed_mint_quote(
        mint_url: cdk_common::mint_url::MintUrl,
        amount: Amount,
        signing_key: SecretKey,
    ) -> MintQuote {
        let mut mint_quote = test_mint_quote(mint_url);
        mint_quote.state = MintQuoteState::Paid;
        mint_quote.amount = Some(amount);
        mint_quote.amount_paid = amount;
        mint_quote.secret_key = Some(signing_key);
        mint_quote
    }

    #[tokio::test]
    async fn test_execute_retries_single_mint_with_legacy_quote_signature() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client.clone()).await;

        let signing_key =
            SecretKey::from_hex("50d7fd7aa2b2fe4607f41f4ce6f8794fc184dd47b8cdfbe4b3d1249aa02d35aa")
                .expect("valid signing key");
        let mint_quote = paid_signed_mint_quote(mint_url, Amount::from(64), signing_key.clone());
        let quote_id = mint_quote.id.clone();
        db.add_mint_quote(mint_quote).await.expect("add mint quote");

        let prepared = MintSaga::new(&wallet)
            .prepare(&quote_id, SplitTarget::Values(vec![Amount::from(64)]), None)
            .await
            .expect("prepare mint saga");

        mock_client.push_post_mint_response(Err(Error::SignatureMissingOrInvalid));
        mock_client.push_post_mint_response(Err(Error::Custom(
            "legacy retry should not replace original error".to_string(),
        )));

        let result = prepared.execute().await;

        assert!(matches!(result, Err(Error::SignatureMissingOrInvalid)));

        let requests = mock_client.post_mint_requests();
        assert_eq!(requests.len(), 2);

        let first_request = &requests[0].1;
        let legacy_request = &requests[1].1;

        let pubkey = signing_key.public_key();
        let new_signature = parse_signature(&first_request.signature);
        let legacy_signature = parse_signature(&legacy_request.signature);
        let legacy_msg =
            legacy_mint_quote_msg_to_sign(&legacy_request.quote, &legacy_request.outputs);

        assert!(pubkey
            .verify(&first_request.msg_to_sign(), &new_signature)
            .is_ok());
        assert!(pubkey.verify(&legacy_msg, &legacy_signature).is_ok());
        assert!(pubkey.verify(&legacy_msg, &new_signature).is_err());
        assert!(pubkey
            .verify(&legacy_request.msg_to_sign(), &legacy_signature)
            .is_err());
        assert_ne!(first_request.signature, legacy_request.signature);
        assert_eq!(first_request.outputs, legacy_request.outputs);
    }

    #[tokio::test]
    async fn test_execute_retries_batch_mint_with_legacy_quote_signature() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client.clone()).await;

        let signing_key =
            SecretKey::from_hex("50d7fd7aa2b2fe4607f41f4ce6f8794fc184dd47b8cdfbe4b3d1249aa02d35aa")
                .expect("valid signing key");
        let mint_quote = paid_signed_mint_quote(mint_url, Amount::from(64), signing_key.clone());
        let quote_id = mint_quote.id.clone();
        db.add_mint_quote(mint_quote).await.expect("add mint quote");

        let prepared = MintSaga::new(&wallet)
            .prepare_batch(
                &[quote_id.as_str()],
                SplitTarget::Values(vec![Amount::from(64)]),
                None,
                None,
            )
            .await
            .expect("prepare batch mint saga");

        mock_client.push_post_batch_mint_response(Err(Error::SignatureMissingOrInvalid));
        mock_client.push_post_batch_mint_response(Err(Error::Custom(
            "legacy retry should not replace original error".to_string(),
        )));

        let result = prepared.execute().await;

        assert!(matches!(result, Err(Error::SignatureMissingOrInvalid)));

        let requests = mock_client.post_batch_mint_requests();
        assert_eq!(requests.len(), 2);

        let first_request = &requests[0].1;
        let legacy_request = &requests[1].1;
        let quote = first_request
            .quotes
            .first()
            .expect("batch request has quote");

        let new_signature = first_request
            .signatures
            .as_ref()
            .and_then(|signatures| signatures.first())
            .and_then(Option::as_ref)
            .expect("new signature is present");
        let legacy_signature = legacy_request
            .signatures
            .as_ref()
            .and_then(|signatures| signatures.first())
            .and_then(Option::as_ref)
            .expect("legacy signature is present");
        let new_signature =
            Signature::from_str(new_signature).expect("valid new schnorr signature");
        let legacy_signature =
            Signature::from_str(legacy_signature).expect("valid legacy schnorr signature");
        let legacy_msg = legacy_mint_quote_msg_to_sign(quote, &legacy_request.outputs);
        let pubkey = signing_key.public_key();

        assert!(pubkey
            .verify(&first_request.msg_to_sign(quote), &new_signature)
            .is_ok());
        assert!(pubkey.verify(&legacy_msg, &legacy_signature).is_ok());
        assert!(pubkey.verify(&legacy_msg, &new_signature).is_err());
        assert!(pubkey
            .verify(&legacy_request.msg_to_sign(quote), &legacy_signature)
            .is_err());
        assert_ne!(new_signature, legacy_signature);
        assert_eq!(first_request.outputs, legacy_request.outputs);
    }

    #[tokio::test]
    async fn test_legacy_batch_signatures_rejects_misaligned_quote_infos() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        let request = BatchMintRequest {
            quotes: vec!["request-quote-id".to_string()],
            quote_amounts: None,
            outputs: vec![],
            signatures: Some(vec![Some("signature-placeholder".to_string())]),
        };
        let quote_infos = vec![test_mint_quote(mint_url)];

        let result = legacy_batch_signatures(&wallet, &request, &quote_infos)
            .await
            .expect("alignment check should not fail");

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_execute_rejects_signature_with_mismatched_amount() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client.clone()).await;

        let mut mint_quote = test_mint_quote(mint_url);
        mint_quote.state = MintQuoteState::Paid;
        mint_quote.amount = Some(Amount::from(64));
        mint_quote.amount_paid = Amount::from(64);
        let quote_id = mint_quote.id.clone();
        db.add_mint_quote(mint_quote).await.expect("add mint quote");

        let prepared = MintSaga::new(&wallet)
            .prepare(&quote_id, SplitTarget::Values(vec![Amount::from(64)]), None)
            .await
            .expect("prepare mint saga");

        let outputs = match &prepared.state_data.mint_request {
            PreparedMintRequest::Single { request, .. } => request.outputs.clone(),
            PreparedMintRequest::Batch { .. } => panic!("expected single mint request"),
        };

        let bad_signatures = outputs
            .iter()
            .map(|blinded_message| BlindSignature {
                amount: Amount::from(1),
                keyset_id: blinded_message.keyset_id,
                c: blinded_message.blinded_secret,
                dleq: None,
            })
            .collect();

        mock_client.set_post_mint_response(Ok(MintResponse {
            signatures: bad_signatures,
        }));

        let result = prepared.execute().await;

        assert!(matches!(result, Err(Error::InvalidMintResponse(_))));
    }
}
