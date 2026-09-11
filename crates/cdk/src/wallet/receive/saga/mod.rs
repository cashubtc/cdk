//! Receive Saga - Type State Pattern Implementation
//!
//! This module implements the saga pattern for receive operations using the typestate
//! pattern to enforce valid state transitions at compile-time.
//!
//! # State Flow
//!
//! ```text
//! [saga created] ──► ProofsPending ──► SwapRequested ──► [completed]
//!                         │                  │
//!                         │                  ├─ replay succeeds ───► [completed]
//!                         │                  ├─ proofs spent ──────► [completed] (via /restore)
//!                         │                  ├─ proofs not spent ──► [compensated]
//!                         │                  └─ mint unreachable ──► [skipped]
//!                         │
//!                         └─ recovery ─────────────────────────────► [compensated]
//! ```
//!
//! # States
//!
//! | State | Description |
//! |-------|-------------|
//! | `ProofsPending` | Input proofs validated and stored as pending, ready to swap for new proofs |
//! | `SwapRequested` | Swap request sent to mint, awaiting signatures for new proofs |
//!
//! # Recovery Outcomes
//!
//! | Outcome | Description |
//! |---------|-------------|
//! | `[completed]` | Receive succeeded, new proofs saved to wallet |
//! | `[compensated]` | Receive failed, pending input proofs removed |
//! | `[skipped]` | Recovery deferred (mint unreachable), will retry on next recovery |

use std::collections::HashMap;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::XOnlyPublicKey;
use cdk_common::util::unix_time;
use cdk_common::wallet::{
    OperationData, ProofInfo, ReceiveOperationData, ReceiveSagaState, Transaction,
    TransactionDirection, TransactionStatus, WalletSaga, WalletSagaState,
};
use tracing::instrument;

use self::compensation::RemovePendingProofs;
use self::state::{Finalized, Initial, Prepared};
use super::ReceiveOptions;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut10::Kind;
use crate::nuts::{Conditions, Proofs, PublicKey, SecretKey, SigFlag, State};
use crate::util::hex;
use crate::wallet::blind_signature::{
    validate_mint_response_signatures, SignatureAmountValidation,
};
use crate::wallet::saga::{
    add_compensation, clear_compensations, execute_compensations, new_compensations, Compensations,
};
use crate::wallet::swap::ProofReservation;
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

    /// Prepare proofs for receiving.
    ///
    /// Verifies DLEQ proofs, signs P2PK proofs if keys provided, and adds HTLC preimages.
    /// No database changes are made in this step.
    #[instrument(skip_all)]
    pub async fn prepare(
        self,
        proofs: Proofs,
        opts: ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<ReceiveSaga<'a, Prepared>, Error> {
        tracing::info!(
            "Preparing receive for {} proofs with operation {}",
            proofs.len(),
            self.state_data.operation_id
        );

        let _mint_info = self.wallet.load_mint_info().await?;

        let keyset_policy = self.state_data.keyset_policy;
        let active_keyset_id = self
            .wallet
            .active_keyset_with_policy(keyset_policy)
            .await?
            .id;

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

        let mut p2pk_signing_keys: HashMap<XOnlyPublicKey, SecretKey> = opts
            .p2pk_signing_keys
            .iter()
            .map(|s| (s.x_only_public_key(&SECP256K1).0, s.clone()))
            .collect();

        // Process each proof: verify DLEQ, handle P2PK/HTLC
        for proof in &mut proofs {
            // Verify that proof DLEQ is valid
            if proof.dleq.is_some() {
                let keys = self
                    .wallet
                    .keyset_with_policy(proof.keyset_id, keyset_policy)
                    .await?
                    .keys;
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
                    let mut pubkeys = Vec::new();

                    match secret.kind() {
                        Kind::P2PK => {
                            let data_key = PublicKey::from_str(secret.secret_data().data())?;
                            pubkeys.push(data_key);
                        }
                        Kind::HTLC => {
                            // HTLC data is a hash, not a pubkey.
                            // Add the pre-image and skip slot 0 pubkey.
                            let hashed_preimage = secret.secret_data().data();
                            let preimage = hashed_to_preimage
                                .get(hashed_preimage)
                                .ok_or(Error::PreimageNotProvided)?;
                            proof.add_preimage(preimage.to_string());

                            // For HTLC, there is no slot 0 pubkey. But slot index for the tags still starts at 1!
                        }
                    }
                    if let Some(mut cond_pubkeys) = conditions.pubkeys {
                        pubkeys.append(&mut cond_pubkeys);
                    }
                    if let Some(mut refund_keys) = conditions.refund_keys {
                        pubkeys.append(&mut refund_keys);
                    }

                    for (i, pubkey) in pubkeys.iter().enumerate() {
                        let slot = match secret.kind() {
                            Kind::P2PK => i as u8,
                            _ => (i + 1) as u8, // HTLC skips slot 0 since it's a hash, not a pubkey
                        };
                        let x_only_pubkey = pubkey.x_only_public_key();

                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            p2pk_signing_keys.entry(x_only_pubkey)
                        {
                            if let Some(secret_key) = self.wallet.get_signing_key(pubkey).await? {
                                entry.insert(secret_key.clone());
                            }
                        }

                        if let Some(ephemeral_key) = proof.p2pk_e {
                            for signing_key in p2pk_signing_keys.values() {
                                if let Ok(r) =
                                    crate::nuts::nut28::ecdh_kdf(signing_key, &ephemeral_key, slot)
                                {
                                    if let Ok(derived_key) =
                                        crate::nuts::nut28::derive_signing_key_bip340(
                                            signing_key,
                                            &r,
                                            pubkey,
                                        )
                                    {
                                        proof.sign_p2pk(derived_key)?;
                                        break;
                                    }
                                }
                            }
                        } else if let Some(signing) = p2pk_signing_keys.get(&x_only_pubkey) {
                            proof.sign_p2pk(signing.to_owned().clone())?;
                        }
                    }

                    match secret.kind() {
                        Kind::P2PK => proof.verify_p2pk()?,
                        Kind::HTLC => proof.verify_htlc()?,
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
                token,
                proofs,
                proofs_amount,
                active_keyset_id,
                p2pk_signing_keys,
            },
        })
    }
}

impl<'a> ReceiveSaga<'a, Prepared> {
    /// Execute the receive operation.
    ///
    /// Stores proofs in Pending state, executes the swap, stores new proofs,
    /// and records the transaction. On failure, removes pending proofs.
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
            .keyset(self.state_data.active_keyset_id)
            .await?
            .keys;

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
                token: self.state_data.token.clone(),
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
                &operation_id,
                self.state_data.active_keyset_id,
                &fee_and_amounts,
                None,
                self.state_data.options.amount_split_target.clone(),
                proofs,
                None,
                false,
                false,
                &fee_breakdown,
                ProofReservation::Skip,
            )
            .await?;

        // Determine if SigAll signing is needed
        let sig_flag = self.determine_sig_flag()?;
        if sig_flag == SigFlag::SigAll {
            for blinded_message in pre_swap.swap_request.outputs_mut() {
                for signing_key in self.state_data.p2pk_signing_keys.values() {
                    // Sign the outputs of the swap using standard P2PK since output
                    // P2BK requires ephemeral keys which is handled at creation.
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

        // Update saga state to SwapRequested BEFORE making the mint call.
        // This is write-ahead logging - if a crash occurs after this, recovery knows
        // the swap may have been attempted.
        saga.update_state(WalletSagaState::Receive(ReceiveSagaState::SwapRequested));
        if let OperationData::Receive(ref mut data) = saga.data {
            data.counter_start = Some(counter_start);
            data.counter_end = Some(counter_end);
            data.blinded_messages = Some(pre_swap.swap_request.outputs().clone());
        }

        // Update saga state - if this fails due to version conflict, another instance
        // is processing this saga, which should not happen during normal operation.
        if !self.wallet.localstore.update_saga(saga).await? {
            return Err(Error::ConcurrentUpdate);
        }

        let total_amount = self
            .state_data
            .proofs_amount
            .checked_sub(fee_breakdown.total)
            .unwrap_or(Amount::ZERO);
        self.wallet
            .upsert_transaction(Transaction {
                mint_url: self.wallet.mint_url.clone(),
                direction: TransactionDirection::Incoming,
                amount: total_amount,
                fee: fee_breakdown.total,
                unit: self.wallet.unit.clone(),
                ys: proofs_ys.clone(),
                timestamp: unix_time(),
                memo: self.state_data.memo.clone(),
                metadata: self.state_data.options.metadata.clone(),
                quote_id: None,
                payment_request: None,
                payment_proof: None,
                payment_method: None,
                saga_id: Some(operation_id),
                status: TransactionStatus::Pending,
            })
            .await?;

        let swap_response = match self.wallet.client.post_swap(pre_swap.swap_request).await {
            Ok(response) => response,
            Err(err) => {
                if err.is_definitive_failure() {
                    tracing::error!("Failed to post swap request (definitive): {}", err);
                    self.wallet.mark_transaction_failed(operation_id).await?;
                    execute_compensations(&mut self.compensations).await?;
                } else {
                    tracing::warn!("Failed to post swap request (ambiguous): {}.", err,);
                }
                return Err(err);
            }
        };

        // Preserve the pending saga on an invalid response: the mint may have
        // already spent the inputs, so recovery must still be able to retry.
        validate_mint_response_signatures(
            self.wallet,
            &swap_response.signatures,
            pre_swap
                .pre_mint_secrets
                .secrets
                .iter()
                .map(|premint| &premint.blinded_message),
            SignatureAmountValidation::Exact,
        )
        .await?;

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
            .zip(&pre_swap.pre_mint_secrets.secrets)
            .map(|(proof, premint)| {
                let proof_info = ProofInfo::new(
                    proof,
                    self.wallet.mint_url.clone(),
                    State::Unspent,
                    self.wallet.unit.clone(),
                )?;
                Ok::<_, Error>(match premint.derivation_index {
                    Some(index) => proof_info.with_derivation_index(index),
                    None => proof_info,
                })
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
            .upsert_transaction(Transaction {
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
                saga_id: Some(operation_id),
                status: TransactionStatus::Completed,
            })
            .await?;

        clear_compensations(&mut self.compensations).await;

        if let Err(e) = self.wallet.localstore.delete_saga(&operation_id).await {
            tracing::warn!(
                "Failed to delete receive saga {}: {}. Will be cleaned up on recovery.",
                operation_id,
                e
            );
            // Don't fail the receive if saga deletion fails - orphaned saga is harmless.
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::wallet::{ReceiveSagaState, TransactionId, TransactionStatus, WalletSagaState};

    use crate::amount::SplitTarget;
    use crate::dhke::{hash_to_curve, sign_message};
    use crate::nuts::{BlindSignature, PreMintSecrets, Proof, SecretKey, State, SwapResponse};
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock_seed, test_keyset_id, test_proof,
        MockMintConnector,
    };
    use crate::wallet::ReceiveOptions;
    use crate::{Amount, Error, Wallet};

    async fn receive_fixture() -> (Wallet, Arc<MockMintConnector>, Proof, BlindSignature) {
        let db = create_test_db().await;
        let mock = Arc::new(MockMintConnector::new());
        mock.enable_mint_signing();
        let keyset_id = mock.keysets.lock().unwrap()[0].id;
        let seed = [42; 64];
        let wallet = create_test_wallet_with_mock_seed(db, mock.clone(), seed).await;
        let amount = Amount::from(2);
        let mint_key = mock.mint_signing_keys.lock().unwrap().as_ref().unwrap()[&amount].clone();

        let mut input = test_proof(keyset_id, 2);
        input.c =
            sign_message(&mint_key, &hash_to_curve(input.secret.as_bytes()).unwrap()).unwrap();

        let premints = PreMintSecrets::restore_batch(keyset_id, &seed, 0, 1).unwrap();
        let blinded_message = premints.blinded_messages()[0].blinded_secret;
        let signature = BlindSignature::new(
            amount,
            sign_message(&mint_key, &blinded_message).unwrap(),
            keyset_id,
            &blinded_message,
            &mint_key,
        )
        .unwrap();

        (wallet, mock, input, signature)
    }

    async fn receive_response(
        wallet: &Wallet,
        mock: &MockMintConnector,
        input: &Proof,
        signatures: Vec<BlindSignature>,
    ) -> Result<Amount, Error> {
        mock.set_post_swap_response(Ok(SwapResponse { signatures }));
        wallet
            .receive_proofs(
                vec![input.clone()],
                ReceiveOptions {
                    amount_split_target: SplitTarget::Values(vec![Amount::from(2)]),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
    }

    async fn assert_receive_pending(wallet: &Wallet, input: &Proof) {
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        let sagas = wallet.localstore.get_incomplete_sagas().await.unwrap();
        assert_eq!(sagas.len(), 1);
        assert_eq!(
            sagas[0].state,
            WalletSagaState::Receive(ReceiveSagaState::SwapRequested)
        );
        let proofs = wallet
            .localstore
            .get_reserved_proofs(&sagas[0].id)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].proof, *input);
        assert_eq!(proofs[0].state, State::Pending);
        let transaction = wallet
            .localstore
            .get_transaction(TransactionId::from_saga_id(sagas[0].id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(transaction.status, TransactionStatus::Pending);
    }

    #[tokio::test]
    async fn test_receive_rejects_invalid_dleq_and_preserves_recovery() {
        let (wallet, mock, input, valid_signature) = receive_fixture().await;
        let mut invalid_signature = valid_signature.clone();
        invalid_signature.dleq.as_mut().unwrap().e = SecretKey::from_slice(&[3; 32]).unwrap();

        let result = receive_response(&wallet, &mock, &input, vec![invalid_signature]).await;
        assert!(
            matches!(result, Err(Error::CouldNotVerifyDleq)),
            "{result:?}"
        );
        assert_receive_pending(&wallet, &input).await;

        // The mint may have spent the inputs even though its response was invalid.
        // Keep the request so recovery can obtain valid signatures later.
        mock.set_post_swap_response(Ok(SwapResponse {
            signatures: vec![valid_signature],
        }));
        let report = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(2));
        assert!(wallet
            .localstore
            .get_incomplete_sagas()
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_receive_rejects_signature_with_mismatched_amount() {
        let (wallet, mock, input, mut signature) = receive_fixture().await;
        signature.amount = Amount::from(1);
        signature.dleq = None;

        let result = receive_response(&wallet, &mock, &input, vec![signature]).await;
        assert!(
            matches!(result, Err(Error::InvalidMintResponse(_))),
            "{result:?}"
        );
        assert_receive_pending(&wallet, &input).await;
    }

    #[tokio::test]
    async fn test_receive_rejects_signature_with_mismatched_keyset() {
        let (wallet, mock, input, mut signature) = receive_fixture().await;
        signature.keyset_id = test_keyset_id();
        signature.dleq = None;

        let result = receive_response(&wallet, &mock, &input, vec![signature]).await;
        assert!(
            matches!(result, Err(Error::InvalidMintResponse(_))),
            "{result:?}"
        );
        assert_receive_pending(&wallet, &input).await;
    }

    #[tokio::test]
    async fn test_receive_rejects_signature_count_mismatch() {
        for extra_signature in [false, true] {
            let (wallet, mock, input, signature) = receive_fixture().await;
            let signatures = match extra_signature {
                true => vec![signature.clone(), signature],
                false => vec![],
            };
            let result = receive_response(&wallet, &mock, &input, signatures).await;
            assert!(
                matches!(result, Err(Error::InvalidMintResponse(_))),
                "{result:?}"
            );
            assert_receive_pending(&wallet, &input).await;
        }
    }

    #[tokio::test]
    async fn test_receive_accepts_valid_signatures_with_optional_dleq() {
        for include_dleq in [true, false] {
            let (wallet, mock, input, mut signature) = receive_fixture().await;
            if !include_dleq {
                signature.dleq = None;
            }

            let amount = receive_response(&wallet, &mock, &input, vec![signature])
                .await
                .unwrap();
            assert_eq!(amount, Amount::from(2));
            assert_eq!(wallet.total_balance().await.unwrap(), amount);
            assert!(wallet
                .localstore
                .get_incomplete_sagas()
                .await
                .unwrap()
                .is_empty());
            let proofs = wallet.get_unspent_proofs().await.unwrap();
            assert_eq!(proofs.len(), 1);
            let mint_key =
                mock.mint_signing_keys.lock().unwrap().as_ref().unwrap()[&amount].clone();
            crate::dhke::verify_message(&mint_key, proofs[0].c, proofs[0].secret.as_bytes())
                .unwrap();
        }
    }
}
