//! Receive Saga - Type State Pattern Implementation
//!
//! This module implements the saga pattern for receive operations using the typestate
//! pattern to enforce valid state transitions at compile-time.
//!
//! # Type State Flow
//!
//! ```text
//! ReceiveSaga<Initial>
//!   └─> prepare() -> ReceiveSaga<Prepared>
//!         └─> execute() -> ReceiveSaga<Finalized>
//! ```

use std::collections::HashMap;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::XOnlyPublicKey;
use cdk_common::util::unix_time;
use cdk_common::wallet::{
    OperationData, ReceiveOperationData, ReceiveSagaState, Transaction, TransactionDirection,
    WalletSaga, WalletSagaState,
};
use tracing::instrument;

use self::compensation::RemovePendingProofs;
use self::state::{Finalized, Initial, Prepared};
use super::ReceiveOptions;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut10::Kind;
use crate::nuts::{Conditions, Proofs, PublicKey, SecretKey, SigFlag, State};
use cdk_common::wallet::ProofInfo;
use crate::util::hex;
use crate::wallet::saga::{
    add_compensation, clear_compensations, execute_compensations, new_compensations, Compensations,
};
use crate::{Amount, Error, Wallet, SECP256K1};

pub(crate) mod compensation;
pub(crate) mod resume;
pub(crate) mod state;

/// Saga pattern implementation for receive operations.
///
/// Uses the typestate pattern to enforce valid state transitions at compile-time.
/// Each state (Initial, Prepared, Finalized) is a distinct type, and operations
/// are only available on the appropriate type.
pub(crate) struct ReceiveSaga<'a, S> {
    /// Wallet reference
    wallet: &'a Wallet,
    /// Compensating actions in LIFO order (most recent first)
    compensations: Compensations,
    /// State-specific data
    state_data: S,
}

impl<'a> ReceiveSaga<'a, Initial> {
    /// Create a new receive saga in the Initial state.
    pub fn new(wallet: &'a Wallet) -> Self {
        let operation_id = uuid::Uuid::new_v4();

        Self {
            wallet,
            compensations: new_compensations(),
            state_data: Initial { operation_id },
        }
    }

    /// Prepare proofs for receiving.
    ///
    /// This is the first step in the saga. It:
    /// 1. Loads mint info if needed
    /// 2. Gets the active keyset
    /// 3. Verifies DLEQ proofs
    /// 4. Signs P2PK proofs if signing keys are provided
    /// 5. Adds HTLC preimages if provided
    ///
    /// No database changes are made in this step.
    #[instrument(skip_all)]
    pub async fn prepare(
        self,
        proofs: Proofs,
        opts: ReceiveOptions,
        memo: Option<String>,
    ) -> Result<ReceiveSaga<'a, Prepared>, Error> {
        tracing::info!(
            "Preparing receive for {} proofs with operation {}",
            proofs.len(),
            self.state_data.operation_id
        );

        let _mint_info = self.wallet.load_mint_info().await?;

        let active_keyset_id = self.wallet.fetch_active_keyset().await?.id;

        let mut proofs = proofs;
        let proofs_amount = proofs.total_amount()?;

        let mut _sig_flag = SigFlag::SigInputs;

        // Map hash of preimage to preimage
        let hashed_to_preimage: HashMap<String, &String> = opts
            .preimages
            .iter()
            .map(|p| {
                let hex_bytes = hex::decode(p)?;
                Ok::<(String, &String), Error>((Sha256Hash::hash(&hex_bytes).to_string(), p))
            })
            .collect::<Result<HashMap<String, &String>, _>>()?;

        let p2pk_signing_keys: HashMap<XOnlyPublicKey, &SecretKey> = opts
            .p2pk_signing_keys
            .iter()
            .map(|s| (s.x_only_public_key(&SECP256K1).0, s))
            .collect();

        // Process each proof: verify DLEQ, handle P2PK/HTLC
        for proof in &mut proofs {
            // Verify that proof DLEQ is valid
            if proof.dleq.is_some() {
                let keys = self.wallet.load_keyset_keys(proof.keyset_id).await?;
                let key = keys.amount_key(proof.amount).ok_or(Error::AmountKey)?;
                proof.verify_dleq(key)?;
            }

            if let Ok(secret) =
                <crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(
                    proof.secret.clone(),
                )
            {
                let conditions: Result<Conditions, _> = secret
                    .secret_data()
                    .tags()
                    .cloned()
                    .unwrap_or_default()
                    .try_into();
                if let Ok(conditions) = conditions {
                    let mut pubkeys = conditions.pubkeys.unwrap_or_default();

                    match secret.kind() {
                        Kind::P2PK => {
                            let data_key = PublicKey::from_str(secret.secret_data().data())?;
                            pubkeys.push(data_key);
                        }
                        Kind::HTLC => {
                            let hashed_preimage = secret.secret_data().data();
                            let preimage = hashed_to_preimage
                                .get(hashed_preimage)
                                .ok_or(Error::PreimageNotProvided)?;
                            proof.add_preimage(preimage.to_string());
                        }
                    }
                    for pubkey in pubkeys {
                        if let Some(signing) = p2pk_signing_keys.get(&pubkey.x_only_public_key()) {
                            proof.sign_p2pk(signing.to_owned().clone())?;
                        }
                    }

                    if conditions.sig_flag.eq(&SigFlag::SigAll) {
                        _sig_flag = SigFlag::SigAll;
                    }
                }
            }
        }

        Ok(ReceiveSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: Prepared {
                operation_id: self.state_data.operation_id,
                options: opts,
                memo,
                proofs,
                proofs_amount,
                active_keyset_id,
            },
        })
    }
}

impl<'a> ReceiveSaga<'a, Prepared> {
    /// Execute the receive operation.
    ///
    /// This completes the receive by:
    /// 1. Storing proofs in Pending state
    /// 2. Creating and executing a swap
    /// 3. Storing new proofs
    /// 4. Recording the transaction
    ///
    /// # Compensation
    ///
    /// Registers a compensation action that will remove pending proofs
    /// if the swap fails.
    #[instrument(skip_all)]
    pub async fn execute(mut self) -> Result<ReceiveSaga<'a, Finalized>, Error> {
        tracing::info!(
            "Executing receive for operation {}",
            self.state_data.operation_id
        );

        let fee_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_by_id(self.state_data.active_keyset_id)
            .await?;

        let keys = self
            .wallet
            .load_keyset_keys(self.state_data.active_keyset_id)
            .await?;

        let proofs = self.state_data.proofs.clone();
        let proofs_ys = proofs.ys()?;

        let fee_breakdown = self.wallet.get_proofs_fee(&proofs).await?;

        let operation_id = self.state_data.operation_id;

        let proofs_info = proofs
            .clone()
            .into_iter()
            .map(|p| {
                ProofInfo::new_with_operations(
                    p,
                    self.wallet.mint_url.clone(),
                    State::Pending,
                    self.wallet.unit.clone(),
                    Some(operation_id),
                    None,
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        self.wallet
            .localstore
            .update_proofs(proofs_info.clone(), vec![])
            .await?;

        let mut saga = WalletSaga::new(
            operation_id,
            WalletSagaState::Receive(ReceiveSagaState::ProofsPending),
            self.state_data.proofs_amount,
            self.wallet.mint_url.clone(),
            self.wallet.unit.clone(),
            OperationData::Receive(ReceiveOperationData {
                token: String::new(),
                counter_start: None,
                counter_end: None,
                amount: Some(self.state_data.proofs_amount),
                blinded_messages: None,
            }),
        );

        self.wallet.localstore.add_saga(saga.clone()).await?;

        add_compensation(
            &mut self.compensations,
            Box::new(RemovePendingProofs {
                localstore: self.wallet.localstore.clone(),
                proof_ys: proofs_info.iter().map(|p| p.y).collect(),
                saga_id: operation_id,
            }),
        )
        .await;

        let mut pre_swap = self
            .wallet
            .create_swap(
                self.state_data.active_keyset_id,
                &fee_and_amounts,
                None,
                self.state_data.options.amount_split_target.clone(),
                proofs,
                None,
                false,
                &fee_breakdown,
            )
            .await?;

        // Determine if SigAll signing is needed
        let sig_flag = self.determine_sig_flag()?;
        if sig_flag == SigFlag::SigAll {
            let p2pk_signing_keys: HashMap<XOnlyPublicKey, &SecretKey> = self
                .state_data
                .options
                .p2pk_signing_keys
                .iter()
                .map(|s| (s.x_only_public_key(&SECP256K1).0, s))
                .collect();

            for blinded_message in pre_swap.swap_request.outputs_mut() {
                for signing_key in p2pk_signing_keys.values() {
                    blinded_message.sign_p2pk(signing_key.to_owned().clone())?
                }
            }
        }

        // Get counter range for recovery (before the swap request is sent)
        let counter_end = self
            .wallet
            .localstore
            .increment_keyset_counter(&self.state_data.active_keyset_id, 0)
            .await?;
        let counter_start = counter_end.saturating_sub(pre_swap.derived_secret_count);

        // Update saga state to SwapRequested BEFORE making the mint call
        // This is write-ahead logging - if we crash after this, recovery knows
        // the swap may have been attempted
        saga.update_state(WalletSagaState::Receive(ReceiveSagaState::SwapRequested));
        if let OperationData::Receive(ref mut data) = saga.data {
            data.counter_start = Some(counter_start);
            data.counter_end = Some(counter_end);
            data.blinded_messages = Some(pre_swap.swap_request.outputs().clone());
        }

        // Update saga state - if this fails due to version conflict, another instance
        // is processing this saga, which should not happen during normal operation
        if !self.wallet.localstore.update_saga(saga).await? {
            return Err(Error::Custom(
                "Saga version conflict during update - another instance may be processing this saga".to_string(),
            ));
        }

        let swap_response = match self.wallet.client.post_swap(pre_swap.swap_request).await {
            Ok(response) => response,
            Err(err) => {
                if err.is_definitive_failure() {
                    tracing::error!("Failed to post swap request (definitive): {}", err);
                    execute_compensations(&mut self.compensations).await?;
                } else {
                    tracing::warn!("Failed to post swap request (ambiguous): {}.", err,);
                }
                return Err(err);
            }
        };

        let recv_proofs = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &keys,
        )?;

        self.wallet
            .localstore
            .increment_keyset_counter(&self.state_data.active_keyset_id, recv_proofs.len() as u32)
            .await?;

        let total_amount = recv_proofs.total_amount()?;
        let fee = self.state_data.proofs_amount - total_amount;

        let recv_proof_infos = recv_proofs
            .into_iter()
            .map(|proof| {
                ProofInfo::new(
                    proof,
                    self.wallet.mint_url.clone(),
                    State::Unspent,
                    self.wallet.unit.clone(),
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        self.wallet
            .localstore
            .update_proofs(
                recv_proof_infos,
                proofs_info.into_iter().map(|p| p.y).collect(),
            )
            .await?;

        self.wallet
            .localstore
            .add_transaction(Transaction {
                mint_url: self.wallet.mint_url.clone(),
                direction: TransactionDirection::Incoming,
                amount: total_amount,
                fee,
                unit: self.wallet.unit.clone(),
                ys: proofs_ys,
                timestamp: unix_time(),
                memo: self.state_data.memo.clone(),
                metadata: self.state_data.options.metadata.clone(),
                quote_id: None,
                payment_request: None,
                payment_proof: None,
                payment_method: None,
                saga_id: None,
            })
            .await?;

        clear_compensations(&mut self.compensations).await;

        if let Err(e) = self.wallet.localstore.delete_saga(&operation_id).await {
            tracing::warn!(
                "Failed to delete receive saga {}: {}. Will be cleaned up on recovery.",
                operation_id,
                e
            );
            // Don't fail the receive if saga deletion fails - orphaned saga is harmless
        }

        Ok(ReceiveSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: Finalized {
                amount: total_amount,
            },
        })
    }

    /// Determine the signature flag based on the proofs
    fn determine_sig_flag(&self) -> Result<SigFlag, Error> {
        for proof in &self.state_data.proofs {
            if let Ok(secret) =
                <crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(
                    proof.secret.clone(),
                )
            {
                let conditions: Result<Conditions, _> = secret
                    .secret_data()
                    .tags()
                    .cloned()
                    .unwrap_or_default()
                    .try_into();
                if let Ok(conditions) = conditions {
                    if conditions.sig_flag == SigFlag::SigAll {
                        return Ok(SigFlag::SigAll);
                    }
                }
            }
        }
        Ok(SigFlag::SigInputs)
    }
}

impl<'a> ReceiveSaga<'a, Finalized> {
    /// Consume the saga and return the received amount
    pub fn into_amount(self) -> Amount {
        self.state_data.amount
    }
}

// Required import for PublicKey::from_str
use std::str::FromStr;

impl<S: std::fmt::Debug> std::fmt::Debug for ReceiveSaga<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReceiveSaga")
            .field("state_data", &self.state_data)
            .finish_non_exhaustive()
    }
}
