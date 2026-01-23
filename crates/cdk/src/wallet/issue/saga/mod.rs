//! Mint (Issue) Saga - Type State Pattern Implementation
//!
//! This module implements the saga pattern for mint operations using the typestate
//! pattern to enforce valid state transitions at compile-time.
//!
//! # Type State Flow
//!
//! ```text
//! MintSaga<Initial>
//!   └─> prepare() -> MintSaga<Prepared>
//!         └─> execute() -> MintSaga<Finalized>
//! ```

use std::collections::HashMap;

use cdk_common::nut00::KnownMethod;
use cdk_common::wallet::{
    IssueSagaState, MintOperationData, OperationData, Transaction, TransactionDirection,
    WalletSaga, WalletSagaState,
};
use cdk_common::PaymentMethod;
use tracing::instrument;

use self::compensation::{MintCompensation, ReleaseMintQuote};
use self::state::{Finalized, Initial, Prepared};
use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{nut12, MintRequest, PreMintSecrets, Proofs, SpendingConditions, State};
use cdk_common::wallet::ProofInfo;
use crate::util::unix_time;
use crate::wallet::saga::{
    add_compensation, clear_compensations, new_compensations, Compensations,
};
use crate::{Amount, Error, Wallet};

pub(crate) mod compensation;
pub(crate) mod resume;
pub(crate) mod state;

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
        let operation_id = uuid::Uuid::new_v4();

        Self {
            wallet,
            compensations: new_compensations(),
            state_data: Initial { operation_id },
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
                    .determine_split_target_values(amount, &fee_and_amounts)
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
                &fee_and_amounts,
            )?,
            None => {
                let amount_split = amount.split_targeted(&split_target, &fee_and_amounts)?;
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
                    &fee_and_amounts,
                )?
            }
        };

        let mut request = MintRequest {
            quote: quote_id.to_string(),
            outputs: premint_secrets.blinded_messages(),
            signature: None,
        };

        if let Some(secret_key) = &quote_info.secret_key {
            request.sign(secret_key.clone())?;
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
            OperationData::Mint(MintOperationData {
                quote_id: quote_id.to_string(),
                amount,
                counter_start: Some(counter_start),
                counter_end: Some(counter_end),
                blinded_messages: Some(request.outputs.clone()),
            }),
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

        // Transition to Prepared state
        Ok(MintSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: Prepared {
                operation_id: self.state_data.operation_id,
                quote_id: quote_id.to_string(),
                quote_info: quote_info.clone(),
                active_keyset_id,
                premint_secrets,
                mint_request: request,
                payment_method: quote_info.payment_method.clone(),
                saga,
            },
        })
    }

    /// Prepare the mint operation.
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
        let quote_info = self
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

        let amount = quote_info.amount_mintable();

        let active_keyset_id = self.wallet.fetch_active_keyset().await?.id;
        let fee_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
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
}

impl<'a> MintSaga<'a, Prepared> {
    /// Execute the mint operation.
    ///
    /// This completes the mint by:
    /// 1. Posting the mint request to the mint
    /// 2. Verifying DLEQ proofs
    /// 3. Constructing proofs
    /// 4. Updating quote state
    /// 5. Storing proofs
    /// 6. Recording transaction
    ///
    /// On success, compensations are cleared.
    #[instrument(skip_all)]
    pub async fn execute(self) -> Result<MintSaga<'a, Finalized>, Error> {
        let MintSaga {
            wallet,
            mut compensations,
            state_data,
        } = self;

        let Prepared {
            operation_id,
            quote_id,
            quote_info,
            active_keyset_id,
            premint_secrets,
            mint_request,
            payment_method,
            saga,
        } = state_data;

        tracing::info!(
            "Executing mint for quote {} with operation {}",
            quote_id,
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

            // Update saga state to MintRequested BEFORE making the mint call
            // This is write-ahead logging - if we crash after this, recovery knows
            // the mint request may have been sent
            let mut updated_saga = saga.clone();
            updated_saga.update_state(WalletSagaState::Issue(IssueSagaState::MintRequested));
            if let OperationData::Mint(ref mut data) = updated_saga.data {
                data.counter_start = Some(counter_start);
                data.counter_end = Some(counter_end);
                data.blinded_messages = Some(mint_request.outputs.clone());
            }

            if !wallet.localstore.update_saga(updated_saga).await? {
                return Err(Error::Custom(
                    "Saga version conflict during update - another instance may be processing this saga".to_string(),
                ));
            }

            let mint_res = wallet
                .client
                .post_mint(&payment_method, mint_request.clone())
                .await?;

            let keys = wallet.load_keyset_keys(active_keyset_id).await?;

            for (sig, premint) in mint_res.signatures.iter().zip(&premint_secrets.secrets) {
                let keys = wallet.load_keyset_keys(sig.keyset_id).await?;
                let key = keys.amount_key(sig.amount).ok_or(Error::AmountKey)?;
                match sig.verify_dleq(key, premint.blinded_message.blinded_secret) {
                    Ok(_) | Err(nut12::Error::MissingDleqProof) => (),
                    Err(_) => return Err(Error::CouldNotVerifyDleq),
                }
            }

            let proofs = construct_proofs(
                mint_res.signatures,
                premint_secrets.rs(),
                premint_secrets.secrets(),
                &keys,
            )?;

            let minted_amount = proofs.total_amount()?;

            let mut quote_info = quote_info;

            if payment_method == PaymentMethod::Known(KnownMethod::Bolt11) {
                quote_info.state = cdk_common::MintQuoteState::Issued;
            }

            quote_info.amount_issued += minted_amount;
            wallet.localstore.add_mint_quote(quote_info.clone()).await?;

            let proof_infos = proofs
                .iter()
                .map(|proof| {
                    ProofInfo::new(
                        proof.clone(),
                        wallet.mint_url.clone(),
                        State::Unspent,
                        quote_info.unit.clone(),
                    )
                })
                .collect::<Result<Vec<ProofInfo>, _>>()?;

            wallet.localstore.update_proofs(proof_infos, vec![]).await?;

            wallet
                .localstore
                .add_transaction(Transaction {
                    mint_url: wallet.mint_url.clone(),
                    direction: TransactionDirection::Incoming,
                    amount: minted_amount,
                    fee: Amount::ZERO,
                    unit: wallet.unit.clone(),
                    ys: proofs.ys()?,
                    timestamp: unix_time(),
                    memo: None,
                    metadata: HashMap::new(),
                    quote_id: Some(quote_id.clone()),
                    payment_request: Some(quote_info.request.clone()),
                    payment_proof: None,
                    payment_method: Some(payment_method.clone()),
                    saga_id: None,
                })
                .await?;

            // Release the mint quote reservation - operation completed successfully
            // This is important for Bolt12 partial minting where the same quote
            // may be used for multiple mint operations.
            if let Err(e) = wallet.localstore.release_mint_quote(&operation_id).await {
                tracing::warn!(
                    "Failed to release mint quote for operation {}: {}. Quote may remain marked as reserved.",
                    operation_id,
                    e
                );
                // Don't fail the mint - proofs are already stored
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
                    // Don't fail the mint if saga deletion fails - orphaned saga is harmless
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
                    use crate::wallet::saga::execute_compensations;
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
