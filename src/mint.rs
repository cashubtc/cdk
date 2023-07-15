use std::collections::{HashMap, HashSet};

use crate::dhke::sign_message;
use crate::dhke::verify_message;
use crate::error::Error;
use crate::nuts::nut00::BlindedMessage;
use crate::nuts::nut00::BlindedSignature;
use crate::nuts::nut00::Proof;
use crate::nuts::nut06::SplitRequest;
use crate::nuts::nut06::SplitResponse;
use crate::nuts::nut07::CheckSpendableRequest;
use crate::nuts::nut07::CheckSpendableResponse;
use crate::nuts::nut08::MeltRequest;
use crate::nuts::nut08::MeltResponse;
use crate::nuts::*;
use crate::Amount;

pub struct Mint {
    pub active_keyset: nut02::mint::KeySet,
    pub inactive_keysets: HashMap<String, nut02::mint::KeySet>,
    pub spent_secrets: HashSet<String>,
    pub pending_secrets: HashSet<String>,
}

impl Mint {
    pub fn new(
        secret: &str,
        derivation_path: &str,
        inactive_keysets: HashMap<String, nut02::mint::KeySet>,
        spent_secrets: HashSet<String>,
        max_order: u8,
    ) -> Self {
        Self {
            active_keyset: nut02::mint::KeySet::generate(secret, derivation_path, max_order),
            inactive_keysets,
            spent_secrets,
            pending_secrets: HashSet::new(),
        }
    }

    /// Retrieve the public keys of the active keyset for distribution to
    /// wallet clients
    pub fn active_keyset_pubkeys(&self) -> nut02::KeySet {
        nut02::KeySet::from(self.active_keyset.clone())
    }

    /// Return a list of all supported keysets
    pub fn keysets(&self) -> nut02::Response {
        let mut keysets: HashSet<_> = self.inactive_keysets.keys().cloned().collect();
        keysets.insert(self.active_keyset.id.clone());
        nut02::Response { keysets }
    }

    pub fn active_keyset(&self) -> nut02::mint::KeySet {
        self.active_keyset.clone()
    }

    pub fn keyset(&self, id: &str) -> Option<nut02::KeySet> {
        if self.active_keyset.id == id {
            return Some(self.active_keyset.clone().into());
        }

        self.inactive_keysets.get(id).map(|k| k.clone().into())
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

        let Some(key_pair) = self.active_keyset.keys.0.get(&amount.to_sat()) else {
            // No key for amount
            return Err(Error::AmountKey);
        };

        let c = sign_message(key_pair.secret_key.clone(), b.clone().into())?;

        Ok(BlindedSignature {
            amount: *amount,
            c: c.into(),
            id: self.active_keyset.id.clone(),
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

        let mut secrets = Vec::with_capacity(split_request.proofs.len());
        for proof in &split_request.proofs {
            secrets.push(self.verify_proof(proof)?);
            self.spent_secrets.insert(proof.secret.clone());
        }

        let promises: Vec<BlindedSignature> = split_request
            .outputs
            .iter()
            .map(|b| self.blind_sign(b).unwrap())
            .collect();

        Ok(SplitResponse::new(promises))
    }

    pub fn verify_proof(&self, proof: &Proof) -> Result<String, Error> {
        if self.spent_secrets.contains(&proof.secret) {
            return Err(Error::TokenSpent);
        }

        let keyset = proof.id.as_ref().map_or_else(
            || &self.active_keyset,
            |id| {
                if let Some(keyset) = self.inactive_keysets.get(id) {
                    keyset
                } else {
                    &self.active_keyset
                }
            },
        );

        let Some(keypair) = keyset.keys.0.get(&proof.amount.to_sat()) else {
            return Err(Error::AmountKey);
        };

        verify_message(
            keypair.secret_key.to_owned(),
            proof.c.clone().into(),
            &proof.secret,
        )?;

        Ok(proof.secret.clone())
    }

    pub fn check_spendable(
        &self,
        check_spendable: &CheckSpendableRequest,
    ) -> Result<CheckSpendableResponse, Error> {
        let mut spendable = Vec::with_capacity(check_spendable.proofs.len());
        let mut pending = Vec::with_capacity(check_spendable.proofs.len());

        for proof in &check_spendable.proofs {
            spendable.push(!self.spent_secrets.contains(&proof.secret));
            pending.push(!self.pending_secrets.contains(&proof.secret));
        }

        Ok(CheckSpendableResponse { spendable, pending })
    }

    pub fn verify_melt_request(&mut self, melt_request: &MeltRequest) -> Result<(), Error> {
        let proofs_total = melt_request.proofs_amount();

        // TODO: Fee reserve
        if proofs_total < melt_request.invoice_amount()? {
            return Err(Error::Amount);
        }

        let mut secrets = Vec::with_capacity(melt_request.proofs.len());
        for proof in &melt_request.proofs {
            secrets.push(self.verify_proof(proof)?);
        }

        Ok(())
    }

    pub fn process_melt_request(
        &mut self,
        melt_request: &MeltRequest,
        preimage: &str,
        total_spent: Amount,
    ) -> Result<MeltResponse, Error> {
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
