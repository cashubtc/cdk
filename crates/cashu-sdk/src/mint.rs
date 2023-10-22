use std::collections::{HashMap, HashSet};

use cashu::dhke::{sign_message, verify_message};
pub use cashu::error::mint::Error;
use cashu::nuts::nut00::{BlindedMessage, BlindedSignature, Proof};
use cashu::nuts::nut02::mint::KeySet;
use cashu::nuts::nut02::Id;
use cashu::nuts::nut06::{SplitRequest, SplitResponse};
use cashu::nuts::nut07::{CheckSpendableRequest, CheckSpendableResponse};
use cashu::nuts::nut08::{MeltRequest, MeltResponse};
use cashu::nuts::*;
use cashu::secret::Secret;
use cashu::types::KeysetInfo;
use cashu::Amount;
use tracing::debug;

pub struct Mint {
    //    pub pubkey: PublicKey,
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
        derivation_path: &str,
        inactive_keysets: HashSet<KeysetInfo>,
        spent_secrets: HashSet<Secret>,
        max_order: u8,
        min_fee_reserve: Amount,
        percent_fee_reserve: f32,
    ) -> Self {
        let active_keyset = nut02::mint::KeySet::generate(secret, derivation_path, max_order);
        let id = active_keyset.id;

        Self {
            active_keyset,
            inactive_keysets: inactive_keysets.into_iter().map(|ks| (ks.id, ks)).collect(),
            active_keyset_info: KeysetInfo {
                id,
                valid_from: 0,
                valid_to: None,
                secret: secret.to_string(),
                derivation_path: derivation_path.to_string(),
                max_order,
            },
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
    pub fn active_keyset_pubkeys(&self) -> nut01::Response {
        nut01::Response {
            keys: nut02::KeySet::from(self.active_keyset.clone()).keys,
        }
    }

    /// Return a list of all supported keysets
    pub fn keysets(&self) -> nut02::Response {
        let mut keysets: HashSet<_> = self.inactive_keysets.keys().cloned().collect();
        keysets.insert(self.active_keyset.id);
        nut02::Response { keysets }
    }

    pub fn active_keyset(&self) -> nut02::mint::KeySet {
        self.active_keyset.clone()
    }

    pub fn keyset(&self, id: &Id) -> Option<nut02::KeySet> {
        if self.active_keyset.id.eq(id) {
            return Some(self.active_keyset.clone().into());
        }

        self.inactive_keysets.get(id).map(|k| {
            nut02::mint::KeySet::generate(&k.secret, &k.derivation_path, k.max_order).into()
        })
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    pub fn rotate_keyset(
        &mut self,
        secret: impl Into<String>,
        derivation_path: impl Into<String>,
        max_order: u8,
    ) {
        // Add current set to inactive keysets
        self.inactive_keysets
            .insert(self.active_keyset.id, self.active_keyset_info.clone());

        self.active_keyset = KeySet::generate(secret, derivation_path, max_order);
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
        let BlindedMessage { amount, b } = blinded_message;

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
                        &keyset.secret,
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

        let change_target = melt_request.proofs_amount() - total_spent;
        let amounts = change_target.split();
        let mut change = Vec::with_capacity(amounts.len());

        if let Some(outputs) = &melt_request.outputs {
            for (i, amount) in amounts.iter().enumerate() {
                let mut message = outputs[i].clone();

                message.amount = *amount;

                let signature = self.blind_sign(&message)?;
                change.push(signature)
            }
        }

        Ok(MeltResponse {
            paid: true,
            preimage: Some(preimage.to_string()),
            change: Some(change),
        })
    }
}

pub struct FeeReserve {
    pub min_fee_reserve: Amount,
    pub percent_fee_reserve: f32,
}
