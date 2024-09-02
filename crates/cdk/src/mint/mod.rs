//! Cashu Mint

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, ExtendedPrivKey};
use bitcoin::secp256k1::{self, Secp256k1};
use error::Error;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::instrument;

use self::nut05::QuoteState;
use self::nut11::EnforceSigFlag;
use crate::cdk_database::{self, MintDatabase};
use crate::dhke::{hash_to_curve, sign_message, verify_message};
use crate::mint_url::MintUrl;
use crate::nuts::nut11::enforce_sig_flag;
use crate::nuts::*;
use crate::util::unix_time;
use crate::Amount;

pub mod error;
pub mod types;

pub use types::{MeltQuote, MintQuote};

/// Cashu Mint
#[derive(Clone)]
pub struct Mint {
    /// Mint Url
    pub mint_url: MintUrl,
    /// Mint Info
    pub mint_info: MintInfo,
    /// Mint Storage backend
    pub localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
    /// Active Mint Keysets
    keysets: Arc<RwLock<HashMap<Id, MintKeySet>>>,
    secp_ctx: Secp256k1<secp256k1::All>,
    xpriv: ExtendedPrivKey,
}

impl Mint {
    /// Create new [`Mint`]
    pub async fn new(
        mint_url: &str,
        seed: &[u8],
        mint_info: MintInfo,
        localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
        // Hashmap where the key is the unit and value is (input fee ppk, max_order)
        supported_units: HashMap<CurrencyUnit, (u64, u8)>,
    ) -> Result<Self, Error> {
        let secp_ctx = Secp256k1::new();
        let xpriv =
            ExtendedPrivKey::new_master(bitcoin::Network::Bitcoin, seed).expect("RNG busted");

        let mut active_keysets = HashMap::new();
        let keysets_infos = localstore.get_keyset_infos().await?;

        let mut active_keyset_units = vec![];

        if !keysets_infos.is_empty() {
            tracing::debug!("Setting all saved keysets to inactive");
            for keyset in keysets_infos.clone() {
                // Set all to in active
                let mut keyset = keyset;
                keyset.active = false;
                localstore.add_keyset_info(keyset).await?;
            }

            let keysets_by_unit: HashMap<CurrencyUnit, Vec<MintKeySetInfo>> =
                keysets_infos.iter().fold(HashMap::new(), |mut acc, ks| {
                    acc.entry(ks.unit).or_default().push(ks.clone());
                    acc
                });

            for (unit, keysets) in keysets_by_unit {
                let mut keysets = keysets;
                keysets.sort_by(|a, b| b.derivation_path_index.cmp(&a.derivation_path_index));

                let highest_index_keyset = keysets
                    .first()
                    .cloned()
                    .expect("unit will not be added to hashmap if empty");

                let keysets: Vec<MintKeySetInfo> = keysets
                    .into_iter()
                    .filter(|ks| ks.derivation_path_index.is_some())
                    .collect();

                if let Some((input_fee_ppk, max_order)) = supported_units.get(&unit) {
                    let derivation_path_index = if keysets.is_empty() {
                        1
                    } else if &highest_index_keyset.input_fee_ppk == input_fee_ppk
                        && &highest_index_keyset.max_order == max_order
                    {
                        let id = highest_index_keyset.id;
                        let keyset = MintKeySet::generate_from_xpriv(
                            &secp_ctx,
                            xpriv,
                            highest_index_keyset.max_order,
                            highest_index_keyset.unit,
                            highest_index_keyset.derivation_path.clone(),
                        );
                        active_keysets.insert(id, keyset);
                        let mut keyset_info = highest_index_keyset;
                        keyset_info.active = true;
                        localstore.add_keyset_info(keyset_info).await?;
                        localstore.set_active_keyset(unit, id).await?;
                        continue;
                    } else {
                        highest_index_keyset.derivation_path_index.unwrap_or(0) + 1
                    };

                    let derivation_path = derivation_path_from_unit(unit, derivation_path_index);

                    let (keyset, keyset_info) = create_new_keyset(
                        &secp_ctx,
                        xpriv,
                        derivation_path,
                        Some(derivation_path_index),
                        unit,
                        *max_order,
                        *input_fee_ppk,
                    );

                    let id = keyset_info.id;
                    localstore.add_keyset_info(keyset_info).await?;
                    localstore.set_active_keyset(unit, id).await?;
                    active_keysets.insert(id, keyset);
                    active_keyset_units.push(unit);
                }
            }
        }

        for (unit, (fee, max_order)) in supported_units {
            if !active_keyset_units.contains(&unit) {
                let derivation_path = derivation_path_from_unit(unit, 0);

                let (keyset, keyset_info) = create_new_keyset(
                    &secp_ctx,
                    xpriv,
                    derivation_path,
                    Some(0),
                    unit,
                    max_order,
                    fee,
                );

                let id = keyset_info.id;
                localstore.add_keyset_info(keyset_info).await?;
                localstore.set_active_keyset(unit, id).await?;
                active_keysets.insert(id, keyset);
            }
        }

        let mint_url = MintUrl::from(mint_url);

        Ok(Self {
            mint_url,
            keysets: Arc::new(RwLock::new(active_keysets)),
            secp_ctx,
            xpriv,
            localstore,
            mint_info,
        })
    }

    /// Set Mint Url
    #[instrument(skip_all)]
    pub fn set_mint_url(&mut self, mint_url: MintUrl) {
        self.mint_url = mint_url;
    }

    /// Get Mint Url
    #[instrument(skip_all)]
    pub fn get_mint_url(&self) -> &MintUrl {
        &self.mint_url
    }

    /// Set Mint Info
    #[instrument(skip_all)]
    pub fn set_mint_info(&mut self, mint_info: MintInfo) {
        self.mint_info = mint_info;
    }

    /// Get Mint Info
    #[instrument(skip_all)]
    pub fn mint_info(&self) -> &MintInfo {
        &self.mint_info
    }

    /// New mint quote
    #[instrument(skip_all)]
    pub async fn new_mint_quote(
        &self,
        mint_url: MintUrl,
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        expiry: u64,
        ln_lookup: String,
    ) -> Result<MintQuote, Error> {
        let nut04 = &self.mint_info.nuts.nut04;

        if nut04.disabled {
            return Err(Error::MintingDisabled);
        }

        match nut04.get_settings(&unit, &PaymentMethod::Bolt11) {
            Some(settings) => {
                if settings
                    .max_amount
                    .map_or(false, |max_amount| amount > max_amount)
                {
                    return Err(Error::MintOverLimit);
                }

                if settings
                    .min_amount
                    .map_or(false, |min_amount| amount < min_amount)
                {
                    return Err(Error::MintUnderLimit);
                }
            }
            None => {
                return Err(Error::UnsupportedUnit);
            }
        }

        let quote = MintQuote::new(mint_url, request, unit, amount, expiry, ln_lookup.clone());
        tracing::debug!(
            "New mint quote {} for {} {} with request id {}",
            quote.id,
            amount,
            unit,
            &ln_lookup
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Check mint quote
    #[instrument(skip(self))]
    pub async fn check_mint_quote(&self, quote_id: &str) -> Result<MintQuoteBolt11Response, Error> {
        let quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let paid = quote.state == MintQuoteState::Paid;

        // Since the pending state is not part of the NUT it should not be part of the
        // response. In practice the wallet should not be checking the state of
        // a quote while waiting for the mint response.
        let state = match quote.state {
            MintQuoteState::Pending => MintQuoteState::Paid,
            s => s,
        };

        Ok(MintQuoteBolt11Response {
            quote: quote.id,
            request: quote.request,
            paid: Some(paid),
            state,
            expiry: Some(quote.expiry),
        })
    }

    /// Update mint quote
    #[instrument(skip_all)]
    pub async fn update_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        self.localstore.add_mint_quote(quote).await?;
        Ok(())
    }

    /// Get mint quotes
    #[instrument(skip_all)]
    pub async fn mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let quotes = self.localstore.get_mint_quotes().await?;
        Ok(quotes)
    }

    /// Get pending mint quotes
    #[instrument(skip_all)]
    pub async fn get_pending_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let mint_quotes = self.localstore.get_mint_quotes().await?;

        Ok(mint_quotes
            .into_iter()
            .filter(|p| p.state == MintQuoteState::Pending)
            .collect())
    }

    /// Get pending mint quotes
    #[instrument(skip_all)]
    pub async fn get_unpaid_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let mint_quotes = self.localstore.get_mint_quotes().await?;

        Ok(mint_quotes
            .into_iter()
            .filter(|p| p.state == MintQuoteState::Unpaid)
            .collect())
    }

    /// Remove mint quote
    #[instrument(skip_all)]
    pub async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.localstore.remove_mint_quote(quote_id).await?;

        Ok(())
    }

    /// Flag mint quote as paid
    #[instrument(skip_all)]
    pub async fn pay_mint_quote_for_request_id(
        &self,
        request_lookup_id: &str,
    ) -> Result<(), Error> {
        if let Ok(Some(mint_quote)) = self
            .localstore
            .get_mint_quote_by_request_lookup_id(request_lookup_id)
            .await
        {
            tracing::debug!(
                "Quote {} paid by lookup id {}",
                mint_quote.id,
                request_lookup_id
            );
            self.localstore
                .update_mint_quote_state(&mint_quote.id, MintQuoteState::Paid)
                .await?;
        }
        Ok(())
    }

    /// New melt quote
    #[instrument(skip_all)]
    pub async fn new_melt_quote(
        &self,
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        fee_reserve: Amount,
        expiry: u64,
        request_lookup_id: String,
    ) -> Result<MeltQuote, Error> {
        let nut05 = &self.mint_info.nuts.nut05;

        if nut05.disabled {
            return Err(Error::MeltingDisabled);
        }

        match nut05.get_settings(&unit, &PaymentMethod::Bolt11) {
            Some(settings) => {
                if settings
                    .max_amount
                    .map_or(false, |max_amount| amount > max_amount)
                {
                    return Err(Error::MeltOverLimit);
                }

                if settings
                    .min_amount
                    .map_or(false, |min_amount| amount < min_amount)
                {
                    return Err(Error::MeltUnderLimit);
                }
            }
            None => {
                return Err(Error::UnsupportedUnit);
            }
        }

        let quote = MeltQuote::new(
            request,
            unit,
            amount,
            fee_reserve,
            expiry,
            request_lookup_id.clone(),
        );

        tracing::debug!(
            "New melt quote {} for {} {} with request id {}",
            quote.id,
            amount,
            unit,
            request_lookup_id
        );

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Fee required for proof set
    #[instrument(skip_all)]
    pub async fn get_proofs_fee(&self, proofs: &Proofs) -> Result<Amount, Error> {
        let mut sum_fee = 0;

        for proof in proofs {
            let input_fee_ppk = self
                .localstore
                .get_keyset_info(&proof.keyset_id)
                .await?
                .ok_or(Error::UnknownKeySet)?;

            sum_fee += input_fee_ppk.input_fee_ppk;
        }

        let fee = (sum_fee + 999) / 1000;

        Ok(Amount::from(fee))
    }

    /// Check melt quote status
    #[instrument(skip(self))]
    pub async fn check_melt_quote(&self, quote_id: &str) -> Result<MeltQuoteBolt11Response, Error> {
        let quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        Ok(MeltQuoteBolt11Response {
            quote: quote.id,
            paid: Some(quote.state == QuoteState::Paid),
            state: quote.state,
            expiry: quote.expiry,
            amount: quote.amount,
            fee_reserve: quote.fee_reserve,
            payment_preimage: quote.payment_preimage,
            change: None,
        })
    }

    /// Update melt quote
    #[instrument(skip_all)]
    pub async fn update_melt_quote(&self, quote: MeltQuote) -> Result<(), Error> {
        self.localstore.add_melt_quote(quote).await?;
        Ok(())
    }

    /// Get melt quotes
    #[instrument(skip_all)]
    pub async fn melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes)
    }

    /// Remove melt quote
    #[instrument(skip(self))]
    pub async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.localstore.remove_melt_quote(quote_id).await?;

        Ok(())
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip(self))]
    pub async fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.ensure_keyset_loaded(keyset_id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(keyset_id).ok_or(Error::UnknownKeySet)?.clone();
        Ok(KeysResponse {
            keysets: vec![keyset.into()],
        })
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
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
    #[instrument(skip_all)]
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
                input_fee_ppk: k.input_fee_ppk,
            })
            .collect();

        Ok(KeysetResponse { keysets })
    }

    /// Get keysets
    #[instrument(skip(self))]
    pub async fn keyset(&self, id: &Id) -> Result<Option<KeySet>, Error> {
        self.ensure_keyset_loaded(id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(id).map(|k| k.clone().into());
        Ok(keyset)
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[instrument(skip(self))]
    pub async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        derivation_path_index: u32,
        max_order: u8,
        input_fee_ppk: u64,
    ) -> Result<(), Error> {
        let derivation_path = derivation_path_from_unit(unit, derivation_path_index);
        let (keyset, keyset_info) = create_new_keyset(
            &self.secp_ctx,
            self.xpriv,
            derivation_path,
            Some(derivation_path_index),
            unit,
            max_order,
            input_fee_ppk,
        );
        let id = keyset_info.id;
        self.localstore.add_keyset_info(keyset_info).await?;
        self.localstore.set_active_keyset(unit, id).await?;

        let mut keysets = self.keysets.write().await;
        keysets.insert(id, keyset);

        Ok(())
    }

    /// Process mint request
    #[instrument(skip_all)]
    pub async fn process_mint_request(
        &self,
        mint_request: nut04::MintBolt11Request,
    ) -> Result<nut04::MintBolt11Response, Error> {
        let state = self
            .localstore
            .update_mint_quote_state(&mint_request.quote, MintQuoteState::Pending)
            .await?;

        match state {
            MintQuoteState::Unpaid => {
                return Err(Error::UnpaidQuote);
            }
            MintQuoteState::Pending => {
                return Err(Error::PendingQuote);
            }
            MintQuoteState::Issued => {
                return Err(Error::IssuedQuote);
            }
            MintQuoteState::Paid => (),
        }

        let blinded_messages: Vec<PublicKey> = mint_request
            .outputs
            .iter()
            .map(|b| b.blinded_secret)
            .collect();

        if self
            .localstore
            .get_blind_signatures(&blinded_messages)
            .await?
            .iter()
            .flatten()
            .next()
            .is_some()
        {
            tracing::info!("Output has already been signed",);
            tracing::info!(
                "Mint {} did not succeed returning quote to Paid state",
                mint_request.quote
            );

            self.localstore
                .update_mint_quote_state(&mint_request.quote, MintQuoteState::Paid)
                .await?;
            return Err(Error::BlindedMessageAlreadySigned);
        }

        let mut blind_signatures = Vec::with_capacity(mint_request.outputs.len());

        for blinded_message in mint_request.outputs.iter() {
            let blind_signature = self.blind_sign(blinded_message).await?;
            blind_signatures.push(blind_signature);
        }

        self.localstore
            .add_blind_signatures(
                &mint_request
                    .outputs
                    .iter()
                    .map(|p| p.blinded_secret)
                    .collect::<Vec<PublicKey>>(),
                &blind_signatures,
            )
            .await?;

        self.localstore
            .update_mint_quote_state(&mint_request.quote, MintQuoteState::Issued)
            .await?;

        Ok(nut04::MintBolt11Response {
            signatures: blind_signatures,
        })
    }

    /// Blind Sign
    #[instrument(skip_all)]
    pub async fn blind_sign(
        &self,
        blinded_message: &BlindedMessage,
    ) -> Result<BlindSignature, Error> {
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

    /// Process Swap
    #[instrument(skip_all)]
    pub async fn process_swap_request(
        &self,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        let blinded_messages: Vec<PublicKey> = swap_request
            .outputs
            .iter()
            .map(|b| b.blinded_secret)
            .collect();

        if self
            .localstore
            .get_blind_signatures(&blinded_messages)
            .await?
            .iter()
            .flatten()
            .next()
            .is_some()
        {
            tracing::info!("Output has already been signed",);

            return Err(Error::BlindedMessageAlreadySigned);
        }

        let proofs_total = swap_request.input_amount();

        let output_total = swap_request.output_amount();

        let fee = self.get_proofs_fee(&swap_request.inputs).await?;

        if proofs_total < output_total + fee {
            tracing::info!(
                "Swap request without enough inputs: {}, outputs {}, fee {}",
                proofs_total,
                output_total,
                fee
            );
            return Err(Error::InsufficientInputs(
                proofs_total.into(),
                output_total.into(),
                fee.into(),
            ));
        }

        let proof_count = swap_request.inputs.len();

        let input_ys = swap_request
            .inputs
            .iter()
            .map(|p| hash_to_curve(&p.secret.to_bytes()))
            .collect::<Result<Vec<PublicKey>, _>>()?;

        self.localstore
            .add_proofs(swap_request.inputs.clone())
            .await?;
        self.check_ys_spendable(&input_ys, State::Pending).await?;

        // Check that there are no duplicate proofs in request
        if input_ys
            .iter()
            .collect::<HashSet<&PublicKey>>()
            .len()
            .ne(&proof_count)
        {
            self.localstore
                .update_proofs_states(&input_ys, State::Unspent)
                .await?;
            return Err(Error::DuplicateProofs);
        }

        for proof in &swap_request.inputs {
            if let Err(err) = self.verify_proof(proof).await {
                tracing::info!("Error verifying proof in swap");
                self.localstore
                    .update_proofs_states(&input_ys, State::Unspent)
                    .await?;
                return Err(err);
            }
        }

        let input_keyset_ids: HashSet<Id> =
            swap_request.inputs.iter().map(|p| p.keyset_id).collect();

        let mut keyset_units = HashSet::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            match self.localstore.get_keyset_info(&id).await? {
                Some(keyset) => {
                    keyset_units.insert(keyset.unit);
                }
                None => {
                    tracing::info!("Swap request with unknown keyset in inputs");
                    self.localstore
                        .update_proofs_states(&input_ys, State::Unspent)
                        .await?;
                }
            }
        }

        let output_keyset_ids: HashSet<Id> =
            swap_request.outputs.iter().map(|p| p.keyset_id).collect();

        for id in &output_keyset_ids {
            match self.localstore.get_keyset_info(id).await? {
                Some(keyset) => {
                    keyset_units.insert(keyset.unit);
                }
                None => {
                    tracing::info!("Swap request with unknown keyset in outputs");
                    self.localstore
                        .update_proofs_states(&input_ys, State::Unspent)
                        .await?;
                }
            }
        }

        // Check that all proofs are the same unit
        // in the future it maybe possible to support multiple units but unsupported for
        // now
        if keyset_units.len().gt(&1) {
            tracing::error!("Only one unit is allowed in request: {:?}", keyset_units);
            self.localstore
                .update_proofs_states(&input_ys, State::Unspent)
                .await?;
            return Err(Error::MultipleUnits);
        }

        let EnforceSigFlag {
            sig_flag,
            pubkeys,
            sigs_required,
        } = enforce_sig_flag(swap_request.inputs.clone());

        if sig_flag.eq(&SigFlag::SigAll) {
            let pubkeys = pubkeys.into_iter().collect();
            for blinded_message in &swap_request.outputs {
                if let Err(err) = blinded_message.verify_p2pk(&pubkeys, sigs_required) {
                    tracing::info!("Could not verify p2pk in swap request");
                    self.localstore
                        .update_proofs_states(&input_ys, State::Unspent)
                        .await?;
                    return Err(err.into());
                }
            }
        }

        let mut promises = Vec::with_capacity(swap_request.outputs.len());

        for blinded_message in swap_request.outputs.iter() {
            let blinded_signature = self.blind_sign(blinded_message).await?;
            promises.push(blinded_signature);
        }

        self.localstore
            .update_proofs_states(&input_ys, State::Spent)
            .await?;

        self.localstore
            .add_blind_signatures(
                &swap_request
                    .outputs
                    .iter()
                    .map(|o| o.blinded_secret)
                    .collect::<Vec<PublicKey>>(),
                &promises,
            )
            .await?;

        Ok(SwapResponse::new(promises))
    }

    /// Verify [`Proof`] meets conditions and is signed
    #[instrument(skip_all)]
    pub async fn verify_proof(&self, proof: &Proof) -> Result<(), Error> {
        // Check if secret is a nut10 secret with conditions
        if let Ok(secret) =
            <&crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(&proof.secret)
        {
            // Checks and verifes known secret kinds.
            // If it is an unknown secret kind it will be treated as a normal secret.
            // Spending conditions will **not** be check. It is up to the wallet to ensure
            // only supported secret kinds are used as there is no way for the mint to
            // enforce only signing supported secrets as they are blinded at
            // that point.
            match secret.kind {
                Kind::P2PK => {
                    proof.verify_p2pk()?;
                }
                Kind::HTLC => {
                    proof.verify_htlc()?;
                }
            }
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

    /// Check state
    #[instrument(skip_all)]
    pub async fn check_state(
        &self,
        check_state: &CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let states = self.localstore.get_proofs_states(&check_state.ys).await?;

        let states = states
            .iter()
            .zip(&check_state.ys)
            .map(|(state, y)| {
                let state = match state {
                    Some(state) => *state,
                    None => State::Unspent,
                };

                ProofState {
                    y: *y,
                    state,
                    witness: None,
                }
            })
            .collect();

        Ok(CheckStateResponse { states })
    }

    /// Check Tokens are not spent or pending
    #[instrument(skip_all)]
    pub async fn check_ys_spendable(
        &self,
        ys: &[PublicKey],
        proof_state: State,
    ) -> Result<(), Error> {
        let proofs_state = self
            .localstore
            .update_proofs_states(ys, proof_state)
            .await?;

        let proofs_state = proofs_state.iter().flatten().collect::<HashSet<&State>>();

        if proofs_state.contains(&State::Pending) {
            return Err(Error::TokenPending);
        }

        if proofs_state.contains(&State::Spent) {
            return Err(Error::TokenAlreadySpent);
        }

        Ok(())
    }

    /// Verify melt request is valid
    #[instrument(skip_all)]
    pub async fn verify_melt_request(
        &self,
        melt_request: &MeltBolt11Request,
    ) -> Result<MeltQuote, Error> {
        let state = self
            .localstore
            .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Pending)
            .await?;

        match state {
            MeltQuoteState::Unpaid => (),
            MeltQuoteState::Pending => {
                return Err(Error::PendingQuote);
            }
            MeltQuoteState::Paid => {
                return Err(Error::PaidQuote);
            }
        }

        let ys = melt_request
            .inputs
            .iter()
            .map(|p| hash_to_curve(&p.secret.to_bytes()))
            .collect::<Result<Vec<PublicKey>, _>>()?;

        // Ensure proofs are unique and not being double spent
        if melt_request.inputs.len() != ys.iter().collect::<HashSet<_>>().len() {
            return Err(Error::DuplicateProofs);
        }

        self.localstore
            .add_proofs(melt_request.inputs.clone())
            .await?;
        self.check_ys_spendable(&ys, State::Pending).await?;

        for proof in &melt_request.inputs {
            self.verify_proof(proof).await?;
        }

        let quote = self
            .localstore
            .get_melt_quote(&melt_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let proofs_total = melt_request.proofs_amount();

        let fee = self.get_proofs_fee(&melt_request.inputs).await?;

        let required_total = quote.amount + quote.fee_reserve + fee;

        if proofs_total < required_total {
            tracing::info!(
                "Swap request without enough inputs: {}, quote amount {}, fee_reserve: {} fee {}",
                proofs_total,
                quote.amount,
                quote.fee_reserve,
                fee
            );
            return Err(Error::InsufficientInputs(
                proofs_total.into(),
                (quote.amount + quote.fee_reserve).into(),
                fee.into(),
            ));
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

        let EnforceSigFlag { sig_flag, .. } = enforce_sig_flag(melt_request.inputs.clone());

        if sig_flag.eq(&SigFlag::SigAll) {
            return Err(Error::SigAllUsedInMelt);
        }

        if let Some(outputs) = &melt_request.outputs {
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

        tracing::debug!("Verified melt quote: {}", melt_request.quote);
        Ok(quote)
    }

    /// Process unpaid melt request
    /// In the event that a melt request fails and the lighthing payment is not
    /// made The [`Proofs`] should be returned to an unspent state and the
    /// quote should be unpaid
    #[instrument(skip_all)]
    pub async fn process_unpaid_melt(&self, melt_request: &MeltBolt11Request) -> Result<(), Error> {
        let input_ys = melt_request
            .inputs
            .iter()
            .map(|p| hash_to_curve(&p.secret.to_bytes()))
            .collect::<Result<Vec<PublicKey>, _>>()?;

        self.localstore
            .update_proofs_states(&input_ys, State::Unspent)
            .await?;

        self.localstore
            .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Unpaid)
            .await?;

        Ok(())
    }

    /// Process melt request marking [`Proofs`] as spent
    /// The melt request must be verifyed using [`Self::verify_melt_request`]
    /// before calling [`Self::process_melt_request`]
    #[instrument(skip_all)]
    pub async fn process_melt_request(
        &self,
        melt_request: &MeltBolt11Request,
        payment_preimage: Option<String>,
        total_spent: Amount,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        tracing::debug!("Processing melt quote: {}", melt_request.quote);

        let quote = self
            .localstore
            .get_melt_quote(&melt_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let input_ys = melt_request
            .inputs
            .iter()
            .map(|p| hash_to_curve(&p.secret.to_bytes()))
            .collect::<Result<Vec<PublicKey>, _>>()?;

        self.localstore
            .update_proofs_states(&input_ys, State::Spent)
            .await?;

        self.localstore
            .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Paid)
            .await?;

        let mut change = None;

        // Check if there is change to return
        if melt_request.proofs_amount() > total_spent {
            // Check if wallet provided change outputs
            if let Some(outputs) = melt_request.outputs.clone() {
                let blinded_messages: Vec<PublicKey> =
                    outputs.iter().map(|b| b.blinded_secret).collect();

                if self
                    .localstore
                    .get_blind_signatures(&blinded_messages)
                    .await?
                    .iter()
                    .flatten()
                    .next()
                    .is_some()
                {
                    tracing::info!("Output has already been signed");

                    return Err(Error::BlindedMessageAlreadySigned);
                }

                let change_target = melt_request.proofs_amount() - total_spent;
                let mut amounts = change_target.split();
                let mut change_sigs = Vec::with_capacity(amounts.len());

                if outputs.len().lt(&amounts.len()) {
                    tracing::debug!(
                        "Providing change requires {} blinded messages, but only {} provided",
                        amounts.len(),
                        outputs.len()
                    );

                    // In the case that not enough outputs are provided to return all change
                    // Reverse sort the amounts so that the most amount of change possible is
                    // returned. The rest is burnt
                    amounts.sort_by(|a, b| b.cmp(a));
                }

                let mut outputs = outputs;

                for (amount, blinded_message) in amounts.iter().zip(&mut outputs) {
                    blinded_message.amount = *amount;

                    let blinded_signature = self.blind_sign(blinded_message).await?;
                    change_sigs.push(blinded_signature)
                }

                self.localstore
                    .add_blind_signatures(
                        &outputs[0..change_sigs.len()]
                            .iter()
                            .map(|o| o.blinded_secret)
                            .collect::<Vec<PublicKey>>(),
                        &change_sigs,
                    )
                    .await?;

                change = Some(change_sigs);
            }
        }

        Ok(MeltQuoteBolt11Response {
            amount: quote.amount,
            paid: Some(true),
            payment_preimage,
            change,
            quote: quote.id,
            fee_reserve: quote.fee_reserve,
            state: QuoteState::Paid,
            expiry: quote.expiry,
        })
    }

    /// Restore
    #[instrument(skip_all)]
    pub async fn restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        let output_len = request.outputs.len();

        let mut outputs = Vec::with_capacity(output_len);
        let mut signatures = Vec::with_capacity(output_len);

        let blinded_message: Vec<PublicKey> =
            request.outputs.iter().map(|b| b.blinded_secret).collect();

        let blinded_signatures = self
            .localstore
            .get_blind_signatures(&blinded_message)
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

    /// Ensure Keyset is loaded in mint
    #[instrument(skip(self))]
    pub async fn ensure_keyset_loaded(&self, id: &Id) -> Result<(), Error> {
        let keysets = self.keysets.read().await;
        if keysets.contains_key(id) {
            return Ok(());
        }
        drop(keysets);

        let keyset_info = self
            .localstore
            .get_keyset_info(id)
            .await?
            .ok_or(Error::UnknownKeySet)?;
        let id = keyset_info.id;
        let mut keysets = self.keysets.write().await;
        keysets.insert(id, self.generate_keyset(keyset_info));
        Ok(())
    }

    /// Generate [`MintKeySet`] from [`MintKeySetInfo`]
    #[instrument(skip_all)]
    pub fn generate_keyset(&self, keyset_info: MintKeySetInfo) -> MintKeySet {
        MintKeySet::generate_from_xpriv(
            &self.secp_ctx,
            self.xpriv,
            keyset_info.max_order,
            keyset_info.unit,
            keyset_info.derivation_path,
        )
    }

    /// Get the total amount issed by keyset
    #[instrument(skip_all)]
    pub async fn total_issued(&self) -> Result<HashMap<Id, Amount>, Error> {
        let keysets = self.localstore.get_keyset_infos().await?;

        let mut total_issued = HashMap::new();

        for keyset in keysets {
            let blinded = self
                .localstore
                .get_blind_signatures_for_keyset(&keyset.id)
                .await?;

            let total = blinded.iter().map(|b| b.amount).sum();

            total_issued.insert(keyset.id, total);
        }

        Ok(total_issued)
    }

    /// Total redeemed for keyset
    #[instrument(skip_all)]
    pub async fn total_redeemed(&self) -> Result<HashMap<Id, Amount>, Error> {
        let keysets = self.localstore.get_keyset_infos().await?;

        let mut total_redeemed = HashMap::new();

        for keyset in keysets {
            let (proofs, state) = self.localstore.get_proofs_by_keyset_id(&keyset.id).await?;

            let total_spent = proofs
                .iter()
                .zip(state)
                .filter_map(|(p, s)| match s == Some(State::Spent) {
                    true => Some(p.amount),
                    false => None,
                })
                .sum();

            total_redeemed.insert(keyset.id, total_spent);
        }

        Ok(total_redeemed)
    }
}

/// Mint Fee Reserve
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeeReserve {
    /// Absolute expected min fee
    pub min_fee_reserve: Amount,
    /// Percentage expected fee
    pub percent_fee_reserve: f32,
}

/// Mint Keyset Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeySetInfo {
    /// Keyset [`Id`]
    pub id: Id,
    /// Keyset [`CurrencyUnit`]
    pub unit: CurrencyUnit,
    /// Keyset active or inactive
    /// Mint will only issue new [`BlindSignature`] on active keysets
    pub active: bool,
    /// Starting unix time Keyset is valid from
    pub valid_from: u64,
    /// When the Keyset is valid to
    /// This is not shown to the wallet and can only be used internally
    pub valid_to: Option<u64>,
    /// [`DerivationPath`] keyset
    pub derivation_path: DerivationPath,
    /// DerivationPath index of Keyset
    pub derivation_path_index: Option<u32>,
    /// Max order of keyset
    pub max_order: u8,
    /// Input Fee ppk
    #[serde(default = "default_fee")]
    pub input_fee_ppk: u64,
}

fn default_fee() -> u64 {
    0
}

impl From<MintKeySetInfo> for KeySetInfo {
    fn from(keyset_info: MintKeySetInfo) -> Self {
        Self {
            id: keyset_info.id,
            unit: keyset_info.unit,
            active: keyset_info.active,
            input_fee_ppk: keyset_info.input_fee_ppk,
        }
    }
}

/// Generate new [`MintKeySetInfo`] from path
#[instrument(skip_all)]
fn create_new_keyset<C: secp256k1::Signing>(
    secp: &secp256k1::Secp256k1<C>,
    xpriv: ExtendedPrivKey,
    derivation_path: DerivationPath,
    derivation_path_index: Option<u32>,
    unit: CurrencyUnit,
    max_order: u8,
    input_fee_ppk: u64,
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
        unit: keyset.unit,
        active: true,
        valid_from: unix_time(),
        valid_to: None,
        derivation_path,
        derivation_path_index,
        max_order,
        input_fee_ppk,
    };
    (keyset, keyset_info)
}

fn derivation_path_from_unit(unit: CurrencyUnit, index: u32) -> DerivationPath {
    DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(0).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(unit.derivation_index()).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(index).expect("0 is a valid index"),
    ])
}

#[cfg(test)]
mod tests {
    use bitcoin::Network;
    use secp256k1::Secp256k1;

    use super::*;

    #[test]
    fn mint_mod_generate_keyset_from_seed() {
        let seed = "test_seed".as_bytes();
        let keyset = MintKeySet::generate_from_seed(
            &Secp256k1::new(),
            seed,
            2,
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0),
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 2);

        let expected_amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = vec![
            (
                Amount::from(1),
                PublicKey::from_hex(
                    "0257aed43bf2c1cdbe3e7ae2db2b27a723c6746fc7415e09748f6847916c09176e",
                )
                .unwrap(),
            ),
            (
                Amount::from(2),
                PublicKey::from_hex(
                    "03ad95811e51adb6231613f9b54ba2ba31e4442c9db9d69f8df42c2b26fbfed26e",
                )
                .unwrap(),
            ),
        ]
        .into_iter()
        .collect();

        let amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = keyset
            .keys
            .iter()
            .map(|(amount, pair)| (*amount, pair.public_key))
            .collect();

        assert_eq!(amounts_and_pubkeys, expected_amounts_and_pubkeys);
    }

    #[test]
    fn mint_mod_generate_keyset_from_xpriv() {
        let seed = "test_seed".as_bytes();
        let network = Network::Bitcoin;
        let xpriv = ExtendedPrivKey::new_master(network, seed).expect("Failed to create xpriv");
        let keyset = MintKeySet::generate_from_xpriv(
            &Secp256k1::new(),
            xpriv,
            2,
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0),
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 2);

        let expected_amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = vec![
            (
                Amount::from(1),
                PublicKey::from_hex(
                    "0257aed43bf2c1cdbe3e7ae2db2b27a723c6746fc7415e09748f6847916c09176e",
                )
                .unwrap(),
            ),
            (
                Amount::from(2),
                PublicKey::from_hex(
                    "03ad95811e51adb6231613f9b54ba2ba31e4442c9db9d69f8df42c2b26fbfed26e",
                )
                .unwrap(),
            ),
        ]
        .into_iter()
        .collect();

        let amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = keyset
            .keys
            .iter()
            .map(|(amount, pair)| (*amount, pair.public_key))
            .collect();

        assert_eq!(amounts_and_pubkeys, expected_amounts_and_pubkeys);
    }

    use cdk_database::mint_memory::MintMemoryDatabase;

    #[derive(Default)]
    struct MintConfig<'a> {
        active_keysets: HashMap<CurrencyUnit, Id>,
        keysets: Vec<MintKeySetInfo>,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        pending_proofs: Proofs,
        spent_proofs: Proofs,
        blinded_signatures: HashMap<[u8; 33], BlindSignature>,
        mint_url: &'a str,
        seed: &'a [u8],
        mint_info: MintInfo,
        supported_units: HashMap<CurrencyUnit, (u64, u8)>,
    }

    async fn create_mint(config: MintConfig<'_>) -> Result<Mint, Error> {
        let localstore = Arc::new(
            MintMemoryDatabase::new(
                config.active_keysets,
                config.keysets,
                config.mint_quotes,
                config.melt_quotes,
                config.pending_proofs,
                config.spent_proofs,
                config.blinded_signatures,
            )
            .unwrap(),
        );

        Mint::new(
            config.mint_url,
            config.seed,
            config.mint_info,
            localstore,
            config.supported_units,
        )
        .await
    }

    #[tokio::test]
    async fn mint_mod_new_mint() -> Result<(), Error> {
        let config = MintConfig::<'_> {
            mint_url: "http://example.com",
            ..Default::default()
        };
        let mint = create_mint(config).await?;

        assert_eq!(mint.get_mint_url().to_string(), "http://example.com");
        let info = mint.mint_info();
        assert!(info.name.is_none());
        assert!(info.pubkey.is_none());
        assert_eq!(
            mint.pubkeys().await.unwrap(),
            KeysResponse {
                keysets: Vec::new()
            }
        );

        assert_eq!(
            mint.keysets().await.unwrap(),
            KeysetResponse {
                keysets: Vec::new()
            }
        );

        assert_eq!(
            mint.total_issued().await.unwrap(),
            HashMap::<nut02::Id, Amount>::new()
        );

        assert_eq!(
            mint.total_redeemed().await.unwrap(),
            HashMap::<nut02::Id, Amount>::new()
        );

        Ok(())
    }

    #[tokio::test]
    async fn mint_mod_rotate_keyset() -> Result<(), Error> {
        let config: MintConfig = Default::default();
        let mint = create_mint(config).await?;

        let keysets = mint.keysets().await.unwrap();
        assert!(keysets.keysets.is_empty());

        // generate the first keyset and set it to active
        mint.rotate_keyset(CurrencyUnit::default(), 0, 1, 1).await?;

        let keysets = mint.keysets().await.unwrap();
        assert!(keysets.keysets.len().eq(&1));
        assert!(keysets.keysets[0].active);
        let first_keyset_id = keysets.keysets[0].id;

        // set the first keyset to inactive and generate a new keyset
        mint.rotate_keyset(CurrencyUnit::default(), 1, 1, 1).await?;

        let keysets = mint.keysets().await.unwrap();

        assert!(keysets.keysets.len().eq(&2));
        for keyset in &keysets.keysets {
            if keyset.id == first_keyset_id {
                assert!(!keyset.active);
            } else {
                assert!(keyset.active);
            }
        }

        Ok(())
    }
}
