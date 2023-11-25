use std::collections::{HashMap, HashSet};

use cashu::dhke::{sign_message, verify_message};
pub use cashu::error::mint::Error;
use cashu::nuts::{
    BlindedMessage, BlindedSignature, MeltRequest, MeltResponse, Proof, SplitRequest,
    SplitResponse, *,
};
#[cfg(feature = "nut07")]
use cashu::nuts::{CheckSpendableRequest, CheckSpendableResponse};
use cashu::secret::Secret;
use cashu::types::KeysetInfo;
use cashu::Amount;
use tracing::{debug, info};

pub struct Mint {
    //    pub pubkey: PublicKey
    secret: String,
    pub active_keyset: nut02::mint::KeySet,
    pub active_keyset_info: KeysetInfo,
    pub inactive_keysets: HashMap<Id, KeysetInfo>,
    pub spent_secrets: HashSet<Secret>,
    pub pending_secrets: HashSet<Secret>,
    pub fee_reserve: FeeReserve,
}

impl Mint {
    pub fn new(
        secret: &str,
        active_keyset_info: KeysetInfo,
        inactive_keysets: HashSet<KeysetInfo>,
        spent_secrets: HashSet<Secret>,
        min_fee_reserve: Amount,
        percent_fee_reserve: f32,
    ) -> Self {
        let active_keyset = nut02::mint::KeySet::generate(
            secret,
            active_keyset_info.symbol.clone(),
            active_keyset_info.derivation_path.clone(),
            active_keyset_info.max_order,
        );

        Self {
            secret: secret.to_string(),
            active_keyset,
            inactive_keysets: inactive_keysets.into_iter().map(|ks| (ks.id, ks)).collect(),
            active_keyset_info,
            spent_secrets,
            pending_secrets: HashSet::new(),
            fee_reserve: FeeReserve {
                min_fee_reserve,
                percent_fee_reserve,
            },
        }
    }

    /// Retrieve the public keys of the active keyset for distribution to
    /// wallet clients
    pub fn active_keyset_pubkeys(&self) -> KeysResponse {
        KeysResponse {
            keys: KeySet::from(self.active_keyset.clone()).keys,
        }
    }

    /// Return a list of all supported keysets
    pub fn keysets(&self) -> KeysetResponse {
        let mut keysets: HashSet<_> = self.inactive_keysets.values().cloned().collect();

        keysets.insert(self.active_keyset_info.clone());

        let keysets = keysets.into_iter().map(|k| k.into()).collect();

        KeysetResponse { keysets }
    }

    pub fn active_keyset(&self) -> MintKeySet {
        self.active_keyset.clone()
    }

    pub fn keyset(&self, id: &Id) -> Option<KeySet> {
        if self.active_keyset.id.eq(id) {
            return Some(self.active_keyset.clone().into());
        }

        self.inactive_keysets.get(id).map(|k| {
            nut02::mint::KeySet::generate(&self.secret, &k.symbol, &k.derivation_path, k.max_order)
                .into()
        })
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    pub fn rotate_keyset(
        &mut self,
        symbol: impl Into<String>,
        derivation_path: impl Into<String>,
        max_order: u8,
    ) {
        // Add current set to inactive keysets
        self.inactive_keysets
            .insert(self.active_keyset.id, self.active_keyset_info.clone());

        self.active_keyset = MintKeySet::generate(&self.secret, symbol, derivation_path, max_order);
    }

    pub fn process_mint_request(
        &mut self,
        mint_request: nut04::MintRequest,
    ) -> Result<nut04::PostMintResponse, Error> {
        let mut blind_signatures = Vec::with_capacity(mint_request.outputs.len());

        for blinded_message in mint_request.outputs {
            blind_signatures.push(self.blind_sign(&blinded_message)?);
        }

        Ok(nut04::PostMintResponse {
            promises: blind_signatures,
        })
    }

    fn blind_sign(&self, blinded_message: &BlindedMessage) -> Result<BlindedSignature, Error> {
        let BlindedMessage {
            amount,
            b,
            keyset_id,
        } = blinded_message;

        if self.active_keyset.id.ne(keyset_id) {
            return Err(Error::InactiveKeyset);
        }

        let Some(key_pair) = self.active_keyset.keys.0.get(amount) else {
            // No key for amount
            return Err(Error::AmountKey);
        };

        let c = sign_message(key_pair.secret_key.clone().into(), b.clone().into())?;

        Ok(BlindedSignature {
            amount: *amount,
            c: c.into(),
            id: self.active_keyset.id,
        })
    }

    pub fn process_split_request(
        &mut self,
        split_request: SplitRequest,
    ) -> Result<SplitResponse, Error> {
        let proofs_total = split_request.proofs_amount();

        let output_total = split_request.output_amount();

        if proofs_total != output_total {
            return Err(Error::Amount);
        }

        let proof_count = split_request.proofs.len();

        let secrets: HashSet<Secret> = split_request
            .proofs
            .iter()
            .map(|p| p.secret.clone())
            .collect();

        // Check that there are no duplicate proofs in request
        if secrets.len().ne(&proof_count) {
            return Err(Error::DuplicateProofs);
        }

        for proof in &split_request.proofs {
            self.verify_proof(proof)?
        }

        for secret in secrets {
            self.spent_secrets.insert(secret);
        }

        match &split_request.amount {
            None => {
                let promises: Vec<BlindedSignature> = split_request
                    .outputs
                    .iter()
                    .map(|b| self.blind_sign(b).unwrap())
                    .collect();

                Ok(SplitResponse::new(promises))
            }
            Some(amount) => {
                let outs_fst = (proofs_total.to_owned() - amount.to_owned()).split();

                // Blinded change messages
                let b_fst = split_request.outputs[0..outs_fst.len()].to_vec();
                let b_snd = split_request.outputs[outs_fst.len()..].to_vec();
                let fst: Vec<BlindedSignature> =
                    b_fst.iter().map(|b| self.blind_sign(b).unwrap()).collect();
                let snd: Vec<BlindedSignature> =
                    b_snd.iter().map(|b| self.blind_sign(b).unwrap()).collect();

                let split_response = SplitResponse::new_from_amount(fst, snd);

                if split_response.target_amount() != split_request.amount {
                    return Err(Error::CustomError("Output order".to_string()));
                }

                Ok(split_response)
            }
        }
    }

    fn verify_proof(&self, proof: &Proof) -> Result<(), Error> {
        if self.spent_secrets.contains(&proof.secret) {
            return Err(Error::TokenSpent);
        }

        let keyset = proof.id.as_ref().map_or_else(
            || self.active_keyset.clone(),
            |id| {
                if let Some(keyset) = self.inactive_keysets.get(id) {
                    nut02::mint::KeySet::generate(
                        &self.secret,
                        &keyset.symbol,
                        &keyset.derivation_path,
                        keyset.max_order,
                    )
                } else {
                    self.active_keyset.clone()
                }
            },
        );

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
    pub fn check_spendable(
        &self,
        check_spendable: &CheckSpendableRequest,
    ) -> Result<CheckSpendableResponse, Error> {
        let mut spendable = Vec::with_capacity(check_spendable.proofs.len());
        let mut pending = Vec::with_capacity(check_spendable.proofs.len());

        for proof in &check_spendable.proofs {
            spendable.push(!self.spent_secrets.contains(&proof.secret));
            pending.push(self.pending_secrets.contains(&proof.secret));
        }

        Ok(CheckSpendableResponse { spendable, pending })
    }

    pub fn verify_melt_request(&mut self, melt_request: &MeltRequest) -> Result<(), Error> {
        let proofs_total = melt_request.proofs_amount();

        let percent_fee_reserve = Amount::from_sat(
            (proofs_total.to_sat() as f32 * self.fee_reserve.percent_fee_reserve) as u64,
        );

        let fee_reserve = if percent_fee_reserve > self.fee_reserve.min_fee_reserve {
            percent_fee_reserve
        } else {
            self.fee_reserve.min_fee_reserve
        };

        let required_total = melt_request
            .invoice_amount()
            .map_err(|_| Error::InvoiceAmountUndefined)?
            + fee_reserve;

        if proofs_total < required_total {
            debug!(
                "Insufficient Proofs: Got: {}, Required: {}",
                proofs_total.to_sat().to_string(),
                required_total.to_sat().to_string()
            );
            return Err(Error::Amount);
        }

        let secrets: HashSet<&Secret> = melt_request.proofs.iter().map(|p| &p.secret).collect();

        // Ensure proofs are unique and not being double spent
        if melt_request.proofs.len().ne(&secrets.len()) {
            return Err(Error::DuplicateProofs);
        }

        for proof in &melt_request.proofs {
            self.verify_proof(proof)?
        }

        Ok(())
    }

    pub fn process_melt_request(
        &mut self,
        melt_request: &MeltRequest,
        preimage: &str,
        total_spent: Amount,
    ) -> Result<MeltResponse, Error> {
        self.verify_melt_request(melt_request)?;

        let secrets = Vec::with_capacity(melt_request.proofs.len());
        for secret in secrets {
            self.spent_secrets.insert(secret);
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

                let signature = self.blind_sign(&blinded_message)?;
                change_sigs.push(signature)
            }

            change = Some(change_sigs);
        } else {
            info!(
                "No change outputs provided. Burnt: {} sats",
                (melt_request.proofs_amount() - total_spent).to_sat()
            );
        }

        Ok(MeltResponse {
            paid: true,
            preimage: Some(preimage.to_string()),
            change,
        })
    }
}

pub struct FeeReserve {
    pub min_fee_reserve: Amount,
    pub percent_fee_reserve: f32,
}
