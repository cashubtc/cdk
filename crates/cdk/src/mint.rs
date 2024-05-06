use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, ExtendedPrivKey};
use bitcoin::secp256k1::{self, Secp256k1};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use self::nut11::enforce_sig_flag;
use crate::cdk_database::{self, MintDatabase};
use crate::dhke::{hash_to_curve, sign_message, verify_message};
use crate::error::ErrorResponse;
use crate::nuts::*;
use crate::types::{MeltQuote, MintQuote};
use crate::util::unix_time;
use crate::Amount;

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
    #[error("Quote not paid")]
    UnpaidQuote,
    #[error("`{0}`")]
    Custom(String),
    #[error(transparent)]
    Cashu(#[from] crate::error::Error),
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    #[error(transparent)]
    NUT11(#[from] crate::nuts::nut11::Error),
    #[error(transparent)]
    Nut12(#[from] crate::nuts::nut12::Error),
    #[error(transparent)]
    Nut14(#[from] crate::nuts::nut14::Error),
    /// Database Error
    #[error(transparent)]
    Database(#[from] crate::cdk_database::Error),
    #[error("Unknown quote")]
    UnknownQuote,
    #[error("Unknown secret kind")]
    UnknownSecretKind,
    #[error("Cannot have multiple units")]
    MultipleUnits,
    #[error("Blinded Message is already signed")]
    BlindedMessageAlreadySigned,
}

impl From<Error> for cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}

impl From<Error> for ErrorResponse {
    fn from(err: Error) -> ErrorResponse {
        ErrorResponse {
            code: 9999,
            error: Some(err.to_string()),
            detail: None,
        }
    }
}

impl From<Error> for (StatusCode, ErrorResponse) {
    fn from(err: Error) -> (StatusCode, ErrorResponse) {
        (StatusCode::NOT_FOUND, err.into())
    }
}

#[derive(Clone)]
pub struct Mint {
    keysets: Arc<RwLock<HashMap<Id, MintKeySet>>>,
    secp_ctx: Secp256k1<secp256k1::All>,
    xpriv: ExtendedPrivKey,
    pub fee_reserve: FeeReserve,
    pub localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
}

impl Mint {
    pub async fn new(
        seed: &[u8],
        localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
        min_fee_reserve: Amount,
        percent_fee_reserve: f32,
    ) -> Result<Self, Error> {
        let secp_ctx = Secp256k1::new();
        let xpriv =
            ExtendedPrivKey::new_master(bitcoin::Network::Bitcoin, seed).expect("RNG busted");

        let mut keysets = HashMap::new();
        let keysets_info = localstore.get_keyset_infos().await?;
        if keysets_info.is_empty() {
            let derivation_path = DerivationPath::from(vec![
                ChildNumber::from_hardened_idx(0).expect("0 is a valid index")
            ]);
            let (keyset, keyset_info) =
                create_new_keyset(&secp_ctx, xpriv, derivation_path, CurrencyUnit::Sat, 64);
            let id = keyset_info.id;
            localstore.add_active_keyset(CurrencyUnit::Sat, id).await?;
            localstore.add_keyset_info(keyset_info).await?;
            keysets.insert(id, keyset);
        }

        Ok(Self {
            keysets: Arc::new(RwLock::new(keysets)),
            secp_ctx,
            xpriv,
            localstore,
            fee_reserve: FeeReserve {
                min_fee_reserve,
                percent_fee_reserve,
            },
        })
    }

    pub async fn new_mint_quote(
        &self,
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        expiry: u64,
    ) -> Result<MintQuote, Error> {
        let quote = MintQuote::new(request, unit, amount, expiry);

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    pub async fn check_mint_quote(&self, quote_id: &str) -> Result<MintQuoteBolt11Response, Error> {
        let quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        Ok(MintQuoteBolt11Response {
            quote: quote.id,
            request: quote.request,
            paid: quote.paid,
            expiry: quote.expiry,
        })
    }

    pub async fn update_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        self.localstore.add_mint_quote(quote).await?;
        Ok(())
    }

    pub async fn mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let quotes = self.localstore.get_mint_quotes().await?;
        Ok(quotes)
    }

    pub async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.localstore.remove_mint_quote(quote_id).await?;

        Ok(())
    }

    pub async fn new_melt_quote(
        &self,
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        fee_reserve: Amount,
        expiry: u64,
    ) -> Result<MeltQuote, Error> {
        let quote = MeltQuote::new(request, unit, amount, fee_reserve, expiry);

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Retrieve the public keys of the active keyset for distribution to
    /// wallet clients
    pub async fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.ensure_keyset_loaded(keyset_id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(keyset_id).ok_or(Error::UnknownKeySet)?.clone();
        Ok(KeysResponse {
            keysets: vec![keyset.into()],
        })
    }

    /// Retrieve the public keys of the active keyset for distribution to
    /// wallet clients
    pub async fn pubkeys(&self) -> Result<KeysResponse, Error> {
        let keyset_infos = self.localstore.get_keyset_infos().await?;
        for keyset_info in keyset_infos {
            self.ensure_keyset_loaded(&keyset_info.id).await?;
        }
        let keysets = self.keysets.read().await;
        Ok(KeysResponse {
            keysets: keysets.values().map(|k| k.clone().into()).collect(),
        })
    }

    /// Return a list of all supported keysets
    pub async fn keysets(&self) -> Result<KeysetResponse, Error> {
        let keysets = self.localstore.get_keyset_infos().await?;
        let active_keysets: HashSet<Id> = self
            .localstore
            .get_active_keysets()
            .await?
            .values()
            .cloned()
            .collect();

        let keysets = keysets
            .into_iter()
            .map(|k| KeySetInfo {
                id: k.id,
                unit: k.unit,
                active: active_keysets.contains(&k.id),
            })
            .collect();

        Ok(KeysetResponse { keysets })
    }

    pub async fn keyset(&self, id: &Id) -> Result<Option<KeySet>, Error> {
        self.ensure_keyset_loaded(id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(id).map(|k| k.clone().into());
        Ok(keyset)
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    pub async fn rotate_keyset(
        &mut self,
        unit: CurrencyUnit,
        derivation_path: DerivationPath,
        max_order: u8,
    ) -> Result<(), Error> {
        let (keyset, keyset_info) = create_new_keyset(
            &self.secp_ctx,
            self.xpriv,
            derivation_path,
            unit.clone(),
            max_order,
        );
        let id = keyset_info.id;
        self.localstore.add_keyset_info(keyset_info).await?;
        self.localstore.add_active_keyset(unit, id).await?;

        let mut keysets = self.keysets.write().await;
        keysets.insert(id, keyset);

        Ok(())
    }

    pub async fn process_mint_request(
        &mut self,
        mint_request: nut04::MintBolt11Request,
    ) -> Result<nut04::MintBolt11Response, Error> {
        for blinded_message in &mint_request.outputs {
            if self
                .localstore
                .get_blinded_signature(&blinded_message.blinded_secret)
                .await?
                .is_some()
            {
                error!(
                    "Output has already been signed: {}",
                    blinded_message.blinded_secret
                );
                return Err(Error::BlindedMessageAlreadySigned);
            }
        }

        let quote = self
            .localstore
            .get_mint_quote(&mint_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;

        if !quote.paid {
            return Err(Error::UnpaidQuote);
        }

        let mut blind_signatures = Vec::with_capacity(mint_request.outputs.len());

        for blinded_message in mint_request.outputs.into_iter() {
            let blinded_signature = self.blind_sign(&blinded_message).await?;
            self.localstore
                .add_blinded_signature(blinded_message.blinded_secret, blinded_signature.clone())
                .await?;
            blind_signatures.push(blinded_signature);
        }

        Ok(nut04::MintBolt11Response {
            signatures: blind_signatures,
        })
    }

    async fn blind_sign(&self, blinded_message: &BlindedMessage) -> Result<BlindSignature, Error> {
        let BlindedMessage {
            amount,
            blinded_secret,
            keyset_id,
            ..
        } = blinded_message;
        self.ensure_keyset_loaded(keyset_id).await?;

        let keyset_info = self
            .localstore
            .get_keyset_info(keyset_id)
            .await?
            .ok_or(Error::UnknownKeySet)?;

        let active = self
            .localstore
            .get_active_keyset_id(&keyset_info.unit)
            .await?
            .ok_or(Error::InactiveKeyset)?;

        // Check that the keyset is active and should be used to sign
        if keyset_info.id.ne(&active) {
            return Err(Error::InactiveKeyset);
        }

        let keysets = self.keysets.read().await;
        let keyset = keysets.get(keyset_id).ok_or(Error::UnknownKeySet)?;
        let Some(key_pair) = keyset.keys.get(amount) else {
            // No key for amount
            return Err(Error::AmountKey);
        };

        let c = sign_message(&key_pair.secret_key, blinded_secret)?;

        let blinded_signature = BlindSignature::new(
            *amount,
            c,
            keyset_info.id,
            &blinded_message.blinded_secret,
            key_pair.secret_key.clone(),
        )?;

        Ok(blinded_signature)
    }

    pub async fn process_swap_request(
        &mut self,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        for blinded_message in &swap_request.outputs {
            if self
                .localstore
                .get_blinded_signature(&blinded_message.blinded_secret)
                .await?
                .is_some()
            {
                error!(
                    "Output has already been signed: {}",
                    blinded_message.blinded_secret
                );
                return Err(Error::BlindedMessageAlreadySigned);
            }
        }

        let proofs_total = swap_request.input_amount();

        let output_total = swap_request.output_amount();

        if proofs_total != output_total {
            return Err(Error::Amount);
        }

        let proof_count = swap_request.inputs.len();

        let secrets: HashSet<[u8; 33]> = swap_request
            .inputs
            .iter()
            .flat_map(|p| hash_to_curve(&p.secret.to_bytes()))
            .map(|p| p.to_bytes())
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

        let mut keyset_units = HashSet::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset_info(&id)
                .await?
                .ok_or(Error::UnknownKeySet)?;
            keyset_units.insert(keyset.unit);
        }

        let output_keyset_ids: HashSet<Id> =
            swap_request.outputs.iter().map(|p| p.keyset_id).collect();

        for id in &output_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset_info(id)
                .await?
                .ok_or(Error::UnknownKeySet)?;

            keyset_units.insert(keyset.unit);
        }

        // Check that all proofs are the same unit
        // in the future it maybe possible to support multiple units but unsupported for
        // now
        if keyset_units.len().gt(&1) {
            error!("Only one unit is allowed in request: {:?}", keyset_units);
            return Err(Error::MultipleUnits);
        }

        let (sig_flag, pubkeys) = enforce_sig_flag(swap_request.inputs.clone());

        if sig_flag.eq(&SigFlag::SigAll) {
            let pubkeys = pubkeys.into_iter().collect();
            for blinded_messaage in &swap_request.outputs {
                blinded_messaage.verify_p2pk(&pubkeys, 1)?;
            }
        }

        for proof in swap_request.inputs {
            self.localstore.add_spent_proof(proof).await?;
        }

        let mut promises = Vec::with_capacity(swap_request.outputs.len());

        for blinded_message in swap_request.outputs {
            let blinded_signature = self.blind_sign(&blinded_message).await?;
            self.localstore
                .add_blinded_signature(blinded_message.blinded_secret, blinded_signature.clone())
                .await?;
            promises.push(blinded_signature);
        }

        Ok(SwapResponse::new(promises))
    }

    async fn verify_proof(&self, proof: &Proof) -> Result<(), Error> {
        // Check if secret is a nut10 secret with conditions
        if let Ok(secret) =
            <&crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(&proof.secret)
        {
            // Checks and verifes known secret kinds.
            // If it is an unknown secret kind it will be treated as a normal secret.
            // Spending conditions will **not** be check. It is up to the wallet to ensure
            // only supported secret kinds are used as there is no way for the mint to enforce
            // only signing supported secrets as they are blinded at that point.
            match secret.kind {
                Kind::P2PK => {
                    proof.verify_p2pk()?;
                }
                Kind::HTLC => {
                    proof.verify_htlc()?;
                }
            }
        }

        let y: PublicKey = hash_to_curve(&proof.secret.to_bytes())?;

        if self.localstore.get_spent_proof_by_y(&y).await?.is_some() {
            return Err(Error::TokenSpent);
        }

        if self.localstore.get_pending_proof_by_y(&y).await?.is_some() {
            return Err(Error::TokenPending);
        }

        self.ensure_keyset_loaded(&proof.keyset_id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(&proof.keyset_id).ok_or(Error::UnknownKeySet)?;
        let Some(keypair) = keyset.keys.get(&proof.amount) else {
            return Err(Error::AmountKey);
        };

        verify_message(&keypair.secret_key, proof.c, proof.secret.as_bytes())?;

        Ok(())
    }

    pub async fn check_state(
        &self,
        check_state: &CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let mut states = Vec::with_capacity(check_state.ys.len());

        for y in &check_state.ys {
            let state = if self.localstore.get_spent_proof_by_y(y).await?.is_some() {
                State::Spent
            } else if self.localstore.get_pending_proof_by_y(y).await?.is_some() {
                State::Pending
            } else {
                State::Unspent
            };

            states.push(ProofState {
                y: *y,
                state,
                witness: None,
            })
        }
        Ok(CheckStateResponse { states })
    }

    pub async fn verify_melt_request(
        &mut self,
        melt_request: &MeltBolt11Request,
    ) -> Result<MeltQuote, Error> {
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

        let mut keyset_units = HashSet::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset_info(&id)
                .await?
                .ok_or(Error::UnknownKeySet)?;
            keyset_units.insert(keyset.unit);
        }

        if let Some(outputs) = &melt_request.outputs {
            let (sig_flag, pubkeys) = enforce_sig_flag(melt_request.inputs.clone());

            if sig_flag.eq(&SigFlag::SigAll) {
                let pubkeys = pubkeys.into_iter().collect();
                for blinded_messaage in outputs {
                    blinded_messaage.verify_p2pk(&pubkeys, 1)?;
                }
            }

            let output_keysets_ids: HashSet<Id> = outputs.iter().map(|b| b.keyset_id).collect();
            for id in output_keysets_ids {
                let keyset = self
                    .localstore
                    .get_keyset_info(&id)
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
                keyset_units.insert(keyset.unit);
            }
        }

        // Check that all input and output proofs are the same unit
        if keyset_units.len().gt(&1) {
            return Err(Error::MultipleUnits);
        }

        let secrets: HashSet<[u8; 33]> = melt_request
            .inputs
            .iter()
            .flat_map(|p| hash_to_curve(&p.secret.to_bytes()))
            .map(|p| p.to_bytes())
            .collect();

        // Ensure proofs are unique and not being double spent
        if melt_request.inputs.len().ne(&secrets.len()) {
            return Err(Error::DuplicateProofs);
        }

        for proof in &melt_request.inputs {
            self.verify_proof(proof).await?;
        }

        Ok(quote)
    }

    pub async fn process_melt_request(
        &mut self,
        melt_request: &MeltBolt11Request,
        preimage: &str,
        total_spent: Amount,
    ) -> Result<MeltBolt11Response, Error> {
        self.verify_melt_request(melt_request).await?;

        if let Some(outputs) = &melt_request.outputs {
            for blinded_message in outputs {
                if self
                    .localstore
                    .get_blinded_signature(&blinded_message.blinded_secret)
                    .await?
                    .is_some()
                {
                    error!(
                        "Output has already been signed: {}",
                        blinded_message.blinded_secret
                    );
                    return Err(Error::BlindedMessageAlreadySigned);
                }
            }
        }

        for input in &melt_request.inputs {
            self.localstore.add_spent_proof(input.clone()).await?;
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

                let blinded_signature = self.blind_sign(&blinded_message).await?;
                self.localstore
                    .add_blinded_signature(
                        blinded_message.blinded_secret,
                        blinded_signature.clone(),
                    )
                    .await?;
                change_sigs.push(blinded_signature)
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

    pub async fn check_melt_quote(&self, quote_id: &str) -> Result<MeltQuoteBolt11Response, Error> {
        let quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        Ok(MeltQuoteBolt11Response {
            quote: quote.id,
            paid: quote.paid,
            expiry: quote.expiry,
            amount: quote.amount,
            fee_reserve: quote.fee_reserve,
        })
    }

    pub async fn mint_info(&self) -> Result<MintInfo, Error> {
        Ok(self.localstore.get_mint_info().await?)
    }

    pub async fn restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        let output_len = request.outputs.len();

        let mut outputs = Vec::with_capacity(output_len);
        let mut signatures = Vec::with_capacity(output_len);

        let blinded_message: Vec<PublicKey> =
            request.outputs.iter().map(|b| b.blinded_secret).collect();

        let blinded_signatures = self
            .localstore
            .get_blinded_signatures(blinded_message)
            .await?;

        assert_eq!(blinded_signatures.len(), output_len);

        for (blinded_message, blinded_signature) in
            request.outputs.into_iter().zip(blinded_signatures)
        {
            if let Some(blinded_signature) = blinded_signature {
                outputs.push(blinded_message);
                signatures.push(blinded_signature);
            }
        }

        Ok(RestoreResponse {
            outputs,
            signatures,
        })
    }

    async fn ensure_keyset_loaded(&self, id: &Id) -> Result<(), Error> {
        let keysets = self.keysets.read().await;
        if keysets.contains_key(id) {
            return Ok(());
        }

        let mut keysets = self.keysets.write().await;
        let keyset_info = self
            .localstore
            .get_keyset_info(id)
            .await?
            .ok_or(Error::UnknownKeySet)?;
        let id = keyset_info.id;
        keysets.insert(id, self.generate_keyset(keyset_info));
        Ok(())
    }

    fn generate_keyset(&self, keyset_info: MintKeySetInfo) -> MintKeySet {
        MintKeySet::generate_from_xpriv(&self.secp_ctx, self.xpriv, keyset_info)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub derivation_path: DerivationPath,
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

fn create_new_keyset<C: secp256k1::Signing>(
    secp: &secp256k1::Secp256k1<C>,
    xpriv: ExtendedPrivKey,
    derivation_path: DerivationPath,
    unit: CurrencyUnit,
    max_order: u8,
) -> (MintKeySet, MintKeySetInfo) {
    let keyset = MintKeySet::generate(
        secp,
        xpriv
            .derive_priv(secp, &derivation_path)
            .expect("RNG busted"),
        unit,
        max_order,
    );
    let keyset_info = MintKeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: true,
        valid_from: unix_time(),
        valid_to: None,
        derivation_path,
        max_order,
    };
    (keyset, keyset_info)
}
