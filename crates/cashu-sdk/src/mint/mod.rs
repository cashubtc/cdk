use std::collections::{HashMap, HashSet};

use cashu::dhke::{sign_message, verify_message};
use cashu::nuts::{
    BlindedMessage, BlindedSignature, MeltBolt11Request, MeltBolt11Response, Proof, SwapRequest,
    SwapResponse, *,
};
#[cfg(feature = "nut07")]
use cashu::nuts::{CheckSpendableRequest, CheckSpendableResponse};
use cashu::secret::Secret;
use cashu::Amount;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info};

use crate::utils::unix_time;
use crate::Mnemonic;

mod localstore;

use localstore::LocalStore;

#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Keyset
    #[error("Unknown Keyset")]
    UnknownKeySet,
    /// Inactive Keyset
    #[error("Inactive Keyset")]
    InactiveKeyset,
    #[error("No key for amount")]
    AmountKey,
    #[error("Amount")]
    Amount,
    #[error("Duplicate proofs")]
    DuplicateProofs,
    #[error("Token Spent")]
    TokenSpent,
    #[error("Token Pending")]
    TokenPending,
    #[error("`{0}`")]
    Custom(String),
    #[error("`{0}`")]
    Cashu(#[from] cashu::error::mint::Error),
    #[error("`{0}`")]
    Localstore(#[from] localstore::Error),
    #[error("Unknown quote")]
    UnknownQuote,
    #[error("Cannot have multiple units")]
    MultipleUnits,
}

pub struct Mint<L: LocalStore> {
    //    pub pubkey: PublicKey
    pub keysets_info: HashMap<Id, MintKeySetInfo>,
    //    pub pubkey: PublicKey,
    mnemonic: Mnemonic,
    pub fee_reserve: FeeReserve,
    localstore: L,
}

impl<L: LocalStore> Mint<L> {
    pub async fn new(
        localstore: L,
        mnemonic: Mnemonic,
        keysets_info: HashSet<MintKeySetInfo>,
        min_fee_reserve: Amount,
        percent_fee_reserve: f32,
    ) -> Result<Self, Error> {
        let mut info = HashMap::default();

        let mut active_units: HashSet<CurrencyUnit> = HashSet::default();

        // Check that there is only one active keyset per unit
        for keyset_info in keysets_info {
            if keyset_info.active && !active_units.insert(keyset_info.unit.clone()) {
                // TODO: Handle Error
                todo!()
            }

            let keyset = nut02::mint::KeySet::generate(
                &mnemonic.to_seed_normalized(""),
                keyset_info.unit.clone(),
                &keyset_info.derivation_path.clone(),
                keyset_info.max_order,
            );

            info.insert(keyset_info.id, keyset_info);

            localstore.add_keyset(keyset).await?;
        }

        Ok(Self {
            localstore,
            mnemonic,
            keysets_info: info,
            fee_reserve: FeeReserve {
                min_fee_reserve,
                percent_fee_reserve,
            },
        })
    }

    /// Retrieve the public keys of the active keyset for distribution to
    /// wallet clients
    pub async fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<Option<KeysResponse>, Error> {
        let keyset = match self.localstore.get_keyset(keyset_id).await? {
            Some(keyset) => keyset.clone(),
            None => {
                return Ok(None);
            }
        };

        Ok(Some(KeysResponse {
            keysets: vec![keyset.into()],
        }))
    }

    /// Return a list of all supported keysets
    pub fn keysets(&self) -> KeysetResponse {
        let keysets = self
            .keysets_info
            .values()
            .map(|k| k.clone().into())
            .collect();

        KeysetResponse { keysets }
    }

    pub async fn keyset(&self, id: &Id) -> Result<Option<KeySet>, Error> {
        Ok(self
            .localstore
            .get_keyset(id)
            .await?
            .map(|ks| ks.clone().into()))
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    pub async fn rotate_keyset(
        &mut self,
        unit: CurrencyUnit,
        derivation_path: &str,
        max_order: u8,
    ) -> Result<(), Error> {
        let new_keyset = MintKeySet::generate(
            &self.mnemonic.to_seed_normalized(""),
            unit.clone(),
            derivation_path,
            max_order,
        );

        self.localstore.add_keyset(new_keyset.clone()).await?;

        for mint_keyset_info in self.keysets_info.values_mut() {
            if mint_keyset_info.active && mint_keyset_info.unit.eq(&unit) {
                mint_keyset_info.active = false;
            }
        }

        let mint_keyset_info = MintKeySetInfo {
            id: new_keyset.id,
            unit,
            derivation_path: derivation_path.to_string(),
            active: true,
            valid_from: unix_time(),
            valid_to: None,
            max_order,
        };

        self.keysets_info.insert(new_keyset.id, mint_keyset_info);
        Ok(())
    }

    pub async fn process_mint_request(
        &mut self,
        mint_request: nut04::MintBolt11Request,
    ) -> Result<nut04::MintBolt11Response, Error> {
        let mut blind_signatures = Vec::with_capacity(mint_request.outputs.len());

        for blinded_message in mint_request.outputs {
            blind_signatures.push(self.blind_sign(&blinded_message).await?);
        }

        Ok(nut04::MintBolt11Response {
            signatures: blind_signatures,
        })
    }

    async fn blind_sign(
        &self,
        blinded_message: &BlindedMessage,
    ) -> Result<BlindedSignature, Error> {
        let BlindedMessage {
            amount,
            b,
            keyset_id,
        } = blinded_message;

        let keyset = self
            .localstore
            .get_keyset(keyset_id)
            .await?
            .ok_or(Error::UnknownKeySet)?;

        // Check that the keyset is active and should be used to sign
        if !self
            .keysets_info
            .get(keyset_id)
            .ok_or(Error::UnknownKeySet)?
            .active
        {
            return Err(Error::InactiveKeyset);
        }

        let Some(key_pair) = keyset.keys.0.get(amount) else {
            // No key for amount
            return Err(Error::AmountKey);
        };

        let c = sign_message(key_pair.secret_key.clone().into(), b.clone().into())?;

        Ok(BlindedSignature {
            amount: *amount,
            c: c.into(),
            keyset_id: keyset.id,
        })
    }

    pub async fn process_swap_request(
        &mut self,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        let proofs_total = swap_request.input_amount();

        let output_total = swap_request.output_amount();

        if proofs_total != output_total {
            return Err(Error::Amount);
        }

        let proof_count = swap_request.inputs.len();

        let secrets: HashSet<Secret> = swap_request
            .inputs
            .iter()
            .map(|p| p.secret.clone())
            .collect();

        // Check that there are no duplicate proofs in request
        if secrets.len().ne(&proof_count) {
            return Err(Error::DuplicateProofs);
        }

        for proof in &swap_request.inputs {
            self.verify_proof(proof).await?
        }

        let input_keyset_ids: HashSet<Id> =
            swap_request.inputs.iter().map(|p| p.keyset_id).collect();

        let mut keyset_units = Vec::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset(&id)
                .await?
                .ok_or(Error::UnknownKeySet)?;
            keyset_units.push(keyset.unit);
        }

        let output_keyset_ids: HashSet<Id> =
            swap_request.outputs.iter().map(|p| p.keyset_id).collect();

        for id in &output_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset(&id)
                .await?
                .ok_or(Error::UnknownKeySet)?;

            // Get the active keyset for the unit
            let active_keyset_id = self
                .localstore
                .get_active_keyset_id(&keyset.unit)
                .await?
                .ok_or(Error::InactiveKeyset)?;

            // Check output is for current active keyset
            if id.ne(&active_keyset_id) {
                return Err(Error::InactiveKeyset);
            }
            keyset_units.push(keyset.unit);
        }

        // Check that all input and output proofs are the same unit
        let seen_units: HashSet<CurrencyUnit> = HashSet::new();
        if keyset_units.iter().any(|unit| !seen_units.contains(unit)) && seen_units.len() != 1 {
            return Err(Error::MultipleUnits);
        }

        for proof in swap_request.inputs {
            self.localstore
                .add_spent_proof(proof.secret.clone(), proof)
                .await
                .unwrap();
        }

        let mut promises = Vec::with_capacity(swap_request.outputs.len());

        for output in swap_request.outputs {
            let promise = self.blind_sign(&output).await?;
            promises.push(promise);
        }

        Ok(SwapResponse::new(promises))
    }

    async fn verify_proof(&self, proof: &Proof) -> Result<(), Error> {
        if self
            .localstore
            .get_spent_proof(&proof.secret)
            .await?
            .is_some()
        {
            return Err(Error::TokenSpent);
        }

        if self
            .localstore
            .get_pending_proof(&proof.secret)
            .await?
            .is_some()
        {
            return Err(Error::TokenPending);
        }

        let keyset = self
            .localstore
            .get_keyset(&proof.keyset_id)
            .await?
            .ok_or(Error::UnknownKeySet)?;

        let Some(keypair) = keyset.keys.0.get(&proof.amount) else {
            return Err(Error::AmountKey);
        };

        verify_message(
            keypair.secret_key.clone().into(),
            proof.c.clone().into(),
            &proof.secret,
        )?;

        Ok(())
    }

    #[cfg(feature = "nut07")]
    pub async fn check_spendable(
        &self,
        check_spendable: &CheckSpendableRequest,
    ) -> Result<CheckSpendableResponse, Error> {
        let mut spendable = Vec::with_capacity(check_spendable.proofs.len());
        let mut pending = Vec::with_capacity(check_spendable.proofs.len());

        for proof in &check_spendable.proofs {
            spendable.push(
                self.localstore
                    .get_spent_proof(&proof.secret)
                    .await
                    .unwrap()
                    .is_none(),
            );
            pending.push(
                self.localstore
                    .get_pending_proof(&proof.secret)
                    .await
                    .unwrap()
                    .is_some(),
            );
        }

        Ok(CheckSpendableResponse { spendable, pending })
    }

    pub async fn verify_melt_request(
        &mut self,
        melt_request: &MeltBolt11Request,
    ) -> Result<(), Error> {
        let quote = self
            .localstore
            .get_melt_quote(&melt_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let proofs_total = melt_request.proofs_amount();

        let required_total = quote.amount + quote.fee_reserve;

        if proofs_total < required_total {
            debug!(
                "Insufficient Proofs: Got: {}, Required: {}",
                proofs_total, required_total
            );
            return Err(Error::Amount);
        }

        let input_keyset_ids: HashSet<Id> =
            melt_request.inputs.iter().map(|p| p.keyset_id).collect();

        let mut keyset_units = Vec::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset(&id)
                .await?
                .ok_or(Error::UnknownKeySet)?;
            keyset_units.push(keyset.unit);
        }

        if let Some(outputs) = &melt_request.outputs {
            let output_keysets_ids: HashSet<Id> = outputs.iter().map(|b| b.keyset_id).collect();
            for id in output_keysets_ids {
                let keyset = self
                    .localstore
                    .get_keyset(&id)
                    .await?
                    .ok_or(Error::UnknownKeySet)?;

                // Get the active keyset for the unit
                let active_keyset_id = self
                    .localstore
                    .get_active_keyset_id(&keyset.unit)
                    .await?
                    .ok_or(Error::InactiveKeyset)?;

                // Check output is for current active keyset
                if id.ne(&active_keyset_id) {
                    return Err(Error::InactiveKeyset);
                }
                keyset_units.push(keyset.unit);
            }
        }

        // Check that all input and output proofs are the same unit
        let seen_units: HashSet<CurrencyUnit> = HashSet::new();
        if keyset_units.iter().any(|unit| !seen_units.contains(unit)) && seen_units.len() != 1 {
            return Err(Error::MultipleUnits);
        }

        let secrets: HashSet<&Secret> = melt_request.inputs.iter().map(|p| &p.secret).collect();

        // Ensure proofs are unique and not being double spent
        if melt_request.inputs.len().ne(&secrets.len()) {
            return Err(Error::DuplicateProofs);
        }

        for proof in &melt_request.inputs {
            self.verify_proof(proof).await?
        }

        Ok(())
    }

    pub async fn process_melt_request(
        &mut self,
        melt_request: &MeltBolt11Request,
        preimage: &str,
        total_spent: Amount,
    ) -> Result<MeltBolt11Response, Error> {
        self.verify_melt_request(melt_request).await?;

        for input in &melt_request.inputs {
            self.localstore
                .add_spent_proof(input.secret.clone(), input.clone())
                .await
                .unwrap();
        }

        let mut change = None;

        if let Some(outputs) = melt_request.outputs.clone() {
            let change_target = melt_request.proofs_amount() - total_spent;
            let mut amounts = change_target.split();
            let mut change_sigs = Vec::with_capacity(amounts.len());

            if outputs.len().lt(&amounts.len()) {
                debug!(
                    "Providing change requires {} blinded messages, but only {} provided",
                    amounts.len(),
                    outputs.len()
                );

                // In the case that not enough outputs are provided to return all change
                // Reverse sort the amounts so that the most amount of change possible is
                // returned. The rest is burnt
                amounts.sort_by(|a, b| b.cmp(a));
            }

            for (amount, blinded_message) in amounts.iter().zip(outputs) {
                let mut blinded_message = blinded_message;
                blinded_message.amount = *amount;

                let signature = self.blind_sign(&blinded_message).await?;
                change_sigs.push(signature)
            }

            change = Some(change_sigs);
        } else {
            info!(
                "No change outputs provided. Burnt: {:?} sats",
                (melt_request.proofs_amount() - total_spent)
            );
        }

        Ok(MeltBolt11Response {
            paid: true,
            payment_preimage: Some(preimage.to_string()),
            change,
        })
    }
}

pub struct FeeReserve {
    pub min_fee_reserve: Amount,
    pub percent_fee_reserve: f32,
}

#[derive(Debug, Hash, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeySetInfo {
    pub id: Id,
    pub unit: CurrencyUnit,
    pub active: bool,
    pub valid_from: u64,
    pub valid_to: Option<u64>,
    pub derivation_path: String,
    pub max_order: u8,
}

impl From<MintKeySetInfo> for KeySetInfo {
    fn from(keyset_info: MintKeySetInfo) -> Self {
        Self {
            id: keyset_info.id,
            unit: keyset_info.unit,
            active: keyset_info.active,
        }
    }
}
