//! Cashu Wallet

use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use bitcoin::bip32::ExtendedPrivKey;
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::XOnlyPublicKey;
use bitcoin::Network;
use error::Error;
#[cfg(feature = "nostr")]
use nostr_sdk::nips::nip04;
#[cfg(feature = "nostr")]
use nostr_sdk::{Filter, Timestamp};
use tokio::sync::RwLock;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::cdk_database::{self, WalletDatabase};
use crate::dhke::{construct_proofs, hash_to_curve};
use crate::nuts::{
    nut10, nut12, Conditions, CurrencyUnit, Id, KeySet, KeySetInfo, Keys, Kind,
    MeltQuoteBolt11Response, MintInfo, MintQuoteBolt11Response, PreMintSecrets, PreSwap, Proof,
    ProofState, Proofs, PublicKey, RestoreRequest, SecretKey, SigFlag, SpendingConditions, State,
    SwapRequest, Token,
};
use crate::types::{MeltQuote, Melted, MintQuote, ProofInfo};
use crate::url::UncheckedUrl;
use crate::util::{hex, unix_time};
use crate::{Amount, Bolt11Invoice, HttpClient};

pub mod client;
pub mod error;

#[derive(Clone)]
pub struct Wallet {
    pub client: HttpClient,
    pub localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync>,
    xpriv: ExtendedPrivKey,
    p2pk_signing_keys: Arc<RwLock<HashMap<XOnlyPublicKey, SecretKey>>>,
    #[cfg(feature = "nostr")]
    nostr_client: nostr_sdk::Client,
}

impl Wallet {
    pub fn new(
        localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync>,
        seed: &[u8],
        p2pk_signing_keys: Vec<SecretKey>,
    ) -> Self {
        let xpriv = ExtendedPrivKey::new_master(Network::Bitcoin, seed)
            .expect("Could not create master key");

        Self {
            client: HttpClient::new(),
            localstore,
            xpriv,
            p2pk_signing_keys: Arc::new(RwLock::new(
                p2pk_signing_keys
                    .into_iter()
                    .map(|s| (s.public_key().x_only_public_key(), s))
                    .collect(),
            )),
            #[cfg(feature = "nostr")]
            nostr_client: nostr_sdk::Client::default(),
        }
    }

    /// Add P2PK signing key to wallet
    #[instrument(skip_all)]
    pub async fn add_p2pk_signing_key(&self, signing_key: SecretKey) {
        self.p2pk_signing_keys
            .write()
            .await
            .insert(signing_key.public_key().x_only_public_key(), signing_key);
    }

    /// Remove P2PK signing key from wallet
    #[instrument(skip_all)]
    pub async fn remove_p2pk_signing_key(&self, x_only_pubkey: &XOnlyPublicKey) {
        self.p2pk_signing_keys.write().await.remove(x_only_pubkey);
    }

    /// P2PK keys available in wallet
    #[instrument(skip(self))]
    pub async fn available_p2pk_signing_keys(&self) -> HashMap<XOnlyPublicKey, SecretKey> {
        self.p2pk_signing_keys.read().await.deref().clone()
    }

    /// Add nostr relays to client
    #[cfg(feature = "nostr")]
    #[instrument(skip(self))]
    pub async fn add_nostr_relays(&self, relays: Vec<String>) -> Result<(), Error> {
        self.nostr_client.add_relays(relays).await?;
        Ok(())
    }

    /// Total Balance of wallet for given unit
    #[instrument(skip(self))]
    pub async fn unit_balance(&self, unit: CurrencyUnit) -> Result<Amount, Error> {
        let balance = match self
            .localstore
            .get_proofs(None, Some(unit), Some(vec![State::Unspent]), None)
            .await?
        {
            Some(proofs) => proofs.iter().map(|p| p.proof.amount).sum(),
            None => Amount::ZERO,
        };

        Ok(balance)
    }

    /// Total pending and reserved balance of wallet for given unit
    #[instrument(skip(self))]
    pub async fn pending_unit_balance(&self, unit: CurrencyUnit) -> Result<Amount, Error> {
        let balance = match self
            .localstore
            .get_proofs(
                None,
                Some(unit),
                Some(vec![State::Pending, State::Reserved]),
                None,
            )
            .await?
        {
            Some(proofs) => proofs.iter().map(|p| p.proof.amount).sum(),
            None => Amount::ZERO,
        };

        Ok(balance)
    }

    /// Total Balance of wallet
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<HashMap<CurrencyUnit, Amount>, Error> {
        let mut balances = HashMap::new();

        if let Some(proofs) = self
            .localstore
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await?
        {
            for proof in proofs {
                balances
                    .entry(proof.unit)
                    .and_modify(|ps| *ps += proof.proof.amount)
                    .or_insert(proof.proof.amount);
            }
        }

        Ok(balances)
    }

    /// Total Balance of wallet
    #[instrument(skip(self))]
    pub async fn total_pending_balance(&self) -> Result<HashMap<CurrencyUnit, Amount>, Error> {
        let mut balances = HashMap::new();

        if let Some(proofs) = self
            .localstore
            .get_proofs(
                None,
                None,
                Some(vec![State::Pending, State::Reserved]),
                None,
            )
            .await?
        {
            for proof in proofs {
                balances
                    .entry(proof.unit)
                    .and_modify(|ps| *ps += proof.proof.amount)
                    .or_insert(proof.proof.amount);
            }
        }

        Ok(balances)
    }

    #[instrument(skip(self))]
    pub async fn mint_balances(
        &self,
    ) -> Result<HashMap<UncheckedUrl, HashMap<CurrencyUnit, Amount>>, Error> {
        let mints = self.localstore.get_mints().await?;

        let mut mint_balances = HashMap::new();

        for (mint, _) in mints {
            if let Some(proofs) = self
                .localstore
                .get_proofs(Some(mint.clone()), None, Some(vec![State::Unspent]), None)
                .await?
            {
                let mut balances = HashMap::new();

                for proof in proofs {
                    balances
                        .entry(proof.unit)
                        .and_modify(|ps| *ps += proof.proof.amount)
                        .or_insert(proof.proof.amount);
                }

                mint_balances.insert(mint, balances);
            } else {
                mint_balances.insert(mint, HashMap::new());
            }
        }

        Ok(mint_balances)
    }

    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn get_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Error> {
        Ok(self
            .localstore
            .get_proofs(Some(mint_url), None, Some(vec![State::Unspent]), None)
            .await?
            .map(|p| p.into_iter().map(|p| p.proof).collect()))
    }

    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn add_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Error> {
        let mint_info = match self
            .client
            .get_mint_info(mint_url.clone().try_into()?)
            .await
        {
            Ok(mint_info) => Some(mint_info),
            Err(err) => {
                tracing::warn!("Could not get mint info {}", err);
                None
            }
        };

        self.localstore
            .add_mint(mint_url, mint_info.clone())
            .await?;

        Ok(mint_info)
    }

    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn get_keyset_keys(
        &self,
        mint_url: &UncheckedUrl,
        keyset_id: Id,
    ) -> Result<Keys, Error> {
        let keys = if let Some(keys) = self.localstore.get_keys(&keyset_id).await? {
            keys
        } else {
            let keys = self
                .client
                .get_mint_keyset(mint_url.try_into()?, keyset_id)
                .await?;

            self.localstore.add_keys(keys.keys.clone()).await?;

            keys.keys
        };

        Ok(keys)
    }

    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn get_mint_keysets(
        &self,
        mint_url: &UncheckedUrl,
    ) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self.client.get_mint_keysets(mint_url.try_into()?).await?;

        self.localstore
            .add_mint_keysets(mint_url.clone(), keysets.keysets.clone())
            .await?;

        Ok(keysets.keysets)
    }

    /// Get active mint keyset
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn get_active_mint_keys(
        &self,
        mint_url: &UncheckedUrl,
    ) -> Result<Vec<KeySet>, Error> {
        let keysets = self.client.get_mint_keys(mint_url.try_into()?).await?;

        for keyset in keysets.clone() {
            self.localstore.add_keys(keyset.keys).await?;
        }

        let k = self.client.get_mint_keysets(mint_url.try_into()?).await?;

        self.localstore
            .add_mint_keysets(mint_url.clone(), k.keysets)
            .await?;

        Ok(keysets)
    }

    /// Refresh Mint keys
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn refresh_mint_keys(&self, mint_url: &UncheckedUrl) -> Result<(), Error> {
        let current_mint_keysets_info = self
            .client
            .get_mint_keysets(mint_url.try_into()?)
            .await?
            .keysets;

        match self.localstore.get_mint_keysets(mint_url.clone()).await? {
            Some(stored_keysets) => {
                let mut unseen_keysets = current_mint_keysets_info.clone();
                unseen_keysets.retain(|ks| !stored_keysets.contains(ks));

                for keyset in unseen_keysets {
                    let keys = self
                        .client
                        .get_mint_keyset(mint_url.try_into()?, keyset.id)
                        .await?;

                    self.localstore.add_keys(keys.keys).await?;
                }
            }
            None => {
                let mint_keys = self.client.get_mint_keys(mint_url.try_into()?).await?;

                for keys in mint_keys {
                    self.localstore.add_keys(keys.keys).await?;
                }
            }
        }

        self.localstore
            .add_mint_keysets(mint_url.clone(), current_mint_keysets_info)
            .await?;

        Ok(())
    }

    /// Check if a proof is spent
    #[instrument(skip(self, proofs), fields(mint_url = %mint_url))]
    pub async fn check_proofs_spent(
        &self,
        mint_url: UncheckedUrl,
        proofs: Proofs,
    ) -> Result<Vec<ProofState>, Error> {
        let spendable = self
            .client
            .post_check_state(
                mint_url.try_into()?,
                proofs
                    .into_iter()
                    // Find Y for the secret
                    .flat_map(|p| hash_to_curve(p.secret.as_bytes()))
                    .collect::<Vec<PublicKey>>(),
            )
            .await?;

        Ok(spendable.states)
    }

    /// Checks pending proofs for spent status
    #[instrument(skip(self))]
    pub async fn check_all_pending_proofs(
        &self,
        mint_url: Option<UncheckedUrl>,
    ) -> Result<Amount, Error> {
        let mints = match mint_url {
            Some(mint_url) => HashMap::from_iter(vec![(mint_url, None)]),
            None => self.localstore.get_mints().await?,
        };

        let mut balance = Amount::ZERO;

        for (mint, _) in mints {
            if let Some(proofs) = self
                .localstore
                .get_proofs(
                    Some(mint.clone()),
                    None,
                    Some(vec![State::Pending, State::Reserved]),
                    None,
                )
                .await?
            {
                let states = self
                    .check_proofs_spent(
                        mint.clone(),
                        proofs.clone().into_iter().map(|p| p.proof).collect(),
                    )
                    .await?;

                // Both `State::Pending` and `State::Unspent` should be included in the pending table.
                // This is because a proof that has been crated to send will be stored in the pending table
                // in order to avoid accidentally double spending but to allow it to be explicitly reclaimed
                let pending_states: HashSet<PublicKey> = states
                    .into_iter()
                    .filter(|s| s.state.ne(&State::Spent))
                    .map(|s| s.y)
                    .collect();

                let (pending_proofs, non_pending_proofs): (Vec<ProofInfo>, Vec<ProofInfo>) = proofs
                    .into_iter()
                    .partition(|p| pending_states.contains(&p.y));

                let amount = pending_proofs.iter().map(|p| p.proof.amount).sum();

                self.localstore
                    .remove_proofs(&non_pending_proofs.into_iter().map(|p| p.proof).collect())
                    .await?;

                balance += amount;
            }
        }

        Ok(balance)
    }

    /// Mint Quote
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn mint_quote(
        &self,
        mint_url: UncheckedUrl,
        amount: Amount,
        unit: CurrencyUnit,
    ) -> Result<MintQuote, Error> {
        let quote_res = self
            .client
            .post_mint_quote(mint_url.clone().try_into()?, amount, unit.clone())
            .await?;

        let quote = MintQuote {
            mint_url,
            id: quote_res.quote.clone(),
            amount,
            unit: unit.clone(),
            request: quote_res.request,
            paid: quote_res.paid,
            expiry: quote_res.expiry.unwrap_or(0),
        };

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Mint quote status
    #[instrument(skip(self, quote_id), fields(mint_url = %mint_url))]
    pub async fn mint_quote_status(
        &self,
        mint_url: UncheckedUrl,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response, Error> {
        let response = self
            .client
            .get_mint_quote_status(mint_url.try_into()?, quote_id)
            .await?;

        match self.localstore.get_mint_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                quote.paid = response.paid;
                self.localstore.add_mint_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote mint {} unknown", quote_id);
            }
        }

        Ok(response)
    }

    /// Check status of pending mint quotes
    pub async fn check_all_mint_quotes(&self) -> Result<Amount, Error> {
        let mint_quotes = self.localstore.get_mint_quotes().await?;
        let mut total_amount = Amount::ZERO;

        for mint_quote in mint_quotes {
            let mint_quote_response = self
                .mint_quote_status(mint_quote.mint_url.clone(), &mint_quote.id)
                .await?;

            if mint_quote_response.paid {
                let amount = self
                    .mint(
                        mint_quote.mint_url,
                        &mint_quote.id,
                        SplitTarget::default(),
                        None,
                    )
                    .await?;
                total_amount += amount;
            } else if mint_quote.expiry.le(&unix_time()) {
                self.localstore.remove_mint_quote(&mint_quote.id).await?;
            }
        }
        Ok(total_amount)
    }

    #[instrument(skip(self), fields(mint_url = %mint_url))]
    async fn active_mint_keyset(
        &self,
        mint_url: &UncheckedUrl,
        unit: &CurrencyUnit,
    ) -> Result<Id, Error> {
        if let Some(keysets) = self.localstore.get_mint_keysets(mint_url.clone()).await? {
            for keyset in keysets {
                if keyset.unit.eq(unit) && keyset.active {
                    return Ok(keyset.id);
                }
            }
        }

        let keysets = self.client.get_mint_keysets(mint_url.try_into()?).await?;

        self.localstore
            .add_mint_keysets(
                mint_url.clone(),
                keysets.keysets.clone().into_iter().collect(),
            )
            .await?;
        for keyset in &keysets.keysets {
            if keyset.unit.eq(unit) && keyset.active {
                return Ok(keyset.id);
            }
        }

        Err(Error::NoActiveKeyset)
    }

    #[instrument(skip(self), fields(mint_url = %mint_url))]
    async fn active_keys(
        &self,
        mint_url: &UncheckedUrl,
        unit: &CurrencyUnit,
    ) -> Result<Option<Keys>, Error> {
        let active_keyset_id = self.active_mint_keyset(mint_url, unit).await?;

        let keys;

        if let Some(k) = self.localstore.get_keys(&active_keyset_id).await? {
            keys = Some(k.clone())
        } else {
            let keyset = self
                .client
                .get_mint_keyset(mint_url.try_into()?, active_keyset_id)
                .await?;

            self.localstore.add_keys(keyset.keys.clone()).await?;
            keys = Some(keyset.keys);
        }

        Ok(keys)
    }

    /// Mint
    #[instrument(skip(self, quote_id), fields(mint_url = %mint_url))]
    pub async fn mint(
        &self,
        mint_url: UncheckedUrl,
        quote_id: &str,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Amount, Error> {
        // Check that mint is in store of mints
        if self.localstore.get_mint(mint_url.clone()).await?.is_none() {
            self.add_mint(mint_url.clone()).await?;
        }

        let quote_info = self.localstore.get_mint_quote(quote_id).await?;

        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) && quote.expiry.ne(&0) {
                return Err(Error::QuoteExpired);
            }

            quote.clone()
        } else {
            return Err(Error::QuoteUnknown);
        };

        let active_keyset_id = self.active_mint_keyset(&mint_url, &quote_info.unit).await?;

        let count = self
            .localstore
            .get_keyset_counter(&active_keyset_id)
            .await?;

        let count = count.map_or(0, |c| c + 1);

        let premint_secrets = match &spending_conditions {
            Some(spending_conditions) => PreMintSecrets::with_conditions(
                active_keyset_id,
                quote_info.amount,
                &amount_split_target,
                spending_conditions,
            )?,
            None => PreMintSecrets::from_xpriv(
                active_keyset_id,
                count,
                self.xpriv,
                quote_info.amount,
                false,
                &amount_split_target,
            )?,
        };

        let mint_res = self
            .client
            .post_mint(
                mint_url.clone().try_into()?,
                quote_id,
                premint_secrets.clone(),
            )
            .await?;

        let keys = self.get_keyset_keys(&mint_url, active_keyset_id).await?;

        // Verify the signature DLEQ is valid
        {
            for (sig, premint) in mint_res.signatures.iter().zip(&premint_secrets.secrets) {
                let keys = self.get_keyset_keys(&mint_url, sig.keyset_id).await?;
                let key = keys.amount_key(sig.amount).ok_or(Error::UnknownKey)?;
                match sig.verify_dleq(key, premint.blinded_message.blinded_secret) {
                    Ok(_) | Err(nut12::Error::MissingDleqProof) => (),
                    Err(_) => return Err(Error::CouldNotVerifyDleq),
                }
            }
        }

        let proofs = construct_proofs(
            mint_res.signatures,
            premint_secrets.rs(),
            premint_secrets.secrets(),
            &keys,
        )?;

        let minted_amount = proofs.iter().map(|p| p.amount).sum();

        // Remove filled quote from store
        self.localstore.remove_mint_quote(&quote_info.id).await?;

        if spending_conditions.is_none() {
            // Update counter for keyset
            self.localstore
                .increment_keyset_counter(&active_keyset_id, proofs.len() as u32)
                .await?;
        }

        let proofs = proofs
            .into_iter()
            .flat_map(|proof| {
                ProofInfo::new(
                    proof,
                    mint_url.clone(),
                    State::Unspent,
                    quote_info.unit.clone(),
                )
            })
            .collect();

        // Add new proofs to store
        self.localstore.add_proofs(proofs).await?;

        Ok(minted_amount)
    }

    /// Swap
    #[instrument(skip(self, input_proofs), fields(mint_url = %mint_url))]
    pub async fn swap(
        &self,
        mint_url: &UncheckedUrl,
        unit: &CurrencyUnit,
        amount: Option<Amount>,
        amount_split_target: &SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Option<Proofs>, Error> {
        let pre_swap = self
            .create_swap(
                mint_url,
                unit,
                amount,
                amount_split_target,
                input_proofs.clone(),
                spending_conditions,
            )
            .await?;

        let swap_response = self
            .client
            .post_swap(mint_url.clone().try_into()?, pre_swap.swap_request)
            .await?;

        let active_keys = self.active_keys(mint_url, unit).await?.unwrap();

        let mut post_swap_proofs = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &active_keys,
        )?;

        let active_keyset_id = self.active_mint_keyset(mint_url, unit).await?;

        // FIXME: Should not increment keyset counter for condition proofs
        self.localstore
            .increment_keyset_counter(&active_keyset_id, post_swap_proofs.len() as u32)
            .await?;

        let mut keep_proofs = Proofs::new();
        let proofs_to_send;

        match amount {
            Some(amount) => {
                post_swap_proofs.reverse();

                let mut left_proofs = vec![];
                let mut send_proofs = vec![];

                for proof in post_swap_proofs {
                    let nut10: Result<nut10::Secret, _> = proof.secret.clone().try_into();

                    match nut10 {
                        Ok(_) => send_proofs.push(proof),
                        Err(_) => left_proofs.push(proof),
                    }
                }

                for proof in left_proofs {
                    if (proof.amount + send_proofs.iter().map(|p| p.amount).sum()).gt(&amount) {
                        keep_proofs.push(proof);
                    } else {
                        send_proofs.push(proof);
                    }
                }

                let send_amount: Amount = send_proofs.iter().map(|p| p.amount).sum();

                if send_amount.ne(&amount) {
                    tracing::warn!(
                        "Send amount proofs is {:?} expected {:?}",
                        send_amount,
                        amount
                    );
                }

                let send_proofs_info = send_proofs
                    .clone()
                    .into_iter()
                    .flat_map(|proof| {
                        ProofInfo::new(proof, mint_url.clone(), State::Reserved, unit.clone())
                    })
                    .collect();

                self.localstore.add_proofs(send_proofs_info).await?;

                proofs_to_send = Some(send_proofs);
            }
            None => {
                keep_proofs = post_swap_proofs;
                proofs_to_send = None;
            }
        }

        for proof in input_proofs {
            self.localstore
                .set_proof_state(proof.y()?, State::Reserved)
                .await?;
        }

        let keep_proofs = keep_proofs
            .into_iter()
            .flat_map(|proof| ProofInfo::new(proof, mint_url.clone(), State::Unspent, unit.clone()))
            .collect();

        self.localstore.add_proofs(keep_proofs).await?;

        Ok(proofs_to_send)
    }

    /// Create Swap Payload
    #[instrument(skip(self, proofs), fields(mint_url = %mint_url))]
    async fn create_swap(
        &self,
        mint_url: &UncheckedUrl,
        unit: &CurrencyUnit,
        amount: Option<Amount>,
        amount_split_target: &SplitTarget,
        proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<PreSwap, Error> {
        let active_keyset_id = self.active_mint_keyset(mint_url, unit).await.unwrap();

        // Desired amount is either amount passwed or value of all proof
        let proofs_total = proofs.iter().map(|p| p.amount).sum();

        let desired_amount = amount.unwrap_or(proofs_total);
        let change_amount = proofs_total - desired_amount;

        let (mut desired_messages, change_messages) = match spending_conditions {
            Some(conditions) => {
                let count = self
                    .localstore
                    .get_keyset_counter(&active_keyset_id)
                    .await?;

                let count = count.map_or(0, |c| c + 1);

                let change_premint_secrets = PreMintSecrets::from_xpriv(
                    active_keyset_id,
                    count,
                    self.xpriv,
                    change_amount,
                    false,
                    amount_split_target,
                )?;

                (
                    PreMintSecrets::with_conditions(
                        active_keyset_id,
                        desired_amount,
                        amount_split_target,
                        &conditions,
                    )?,
                    change_premint_secrets,
                )
            }
            None => {
                let count = self
                    .localstore
                    .get_keyset_counter(&active_keyset_id)
                    .await?;

                let mut count = count.map_or(0, |c| c + 1);

                let premint_secrets = PreMintSecrets::from_xpriv(
                    active_keyset_id,
                    count,
                    self.xpriv,
                    desired_amount,
                    false,
                    amount_split_target,
                )?;

                count += premint_secrets.len() as u32;

                let change_premint_secrets = PreMintSecrets::from_xpriv(
                    active_keyset_id,
                    count,
                    self.xpriv,
                    change_amount,
                    false,
                    amount_split_target,
                )?;

                (premint_secrets, change_premint_secrets)
            }
        };

        // Combine the BlindedMessages totaling the desired amount with change
        desired_messages.combine(change_messages);
        // Sort the premint secrets to avoid finger printing
        desired_messages.sort_secrets();

        let swap_request = SwapRequest::new(proofs, desired_messages.blinded_messages());

        Ok(PreSwap {
            pre_mint_secrets: desired_messages,
            swap_request,
        })
    }

    /// Send
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn send(
        &self,
        mint_url: &UncheckedUrl,
        unit: CurrencyUnit,
        memo: Option<String>,
        amount: Amount,
        amount_split_target: &SplitTarget,
        conditions: Option<SpendingConditions>,
    ) -> Result<String, Error> {
        let (condition_input_proofs, input_proofs) = self
            .select_proofs(
                mint_url.clone(),
                unit.clone(),
                amount,
                conditions.clone().map(|p| vec![p]),
            )
            .await?;

        let send_proofs = match conditions {
            Some(_) => {
                let condition_input_proof_total = condition_input_proofs
                    .iter()
                    .map(|p| p.amount)
                    .sum::<Amount>();
                assert!(condition_input_proof_total.le(&amount));
                let needed_amount = amount - condition_input_proof_total;

                let top_up_proofs = match needed_amount > Amount::ZERO {
                    true => {
                        self.swap(
                            mint_url,
                            &unit,
                            Some(needed_amount),
                            amount_split_target,
                            input_proofs,
                            conditions,
                        )
                        .await?
                    }
                    false => Some(vec![]),
                };

                Some(
                    [
                        condition_input_proofs,
                        top_up_proofs.ok_or(Error::InsufficientFunds)?,
                    ]
                    .concat(),
                )
            }
            None => {
                match input_proofs
                    .iter()
                    .map(|p| p.amount)
                    .sum::<Amount>()
                    .eq(&amount)
                {
                    true => Some(input_proofs),
                    false => {
                        self.swap(
                            mint_url,
                            &unit,
                            Some(amount),
                            amount_split_target,
                            input_proofs,
                            conditions,
                        )
                        .await?
                    }
                }
            }
        };

        let send_proofs = send_proofs.ok_or(Error::InsufficientFunds)?;
        for proof in send_proofs.iter() {
            self.localstore
                .set_proof_state(proof.y()?, State::Reserved)
                .await?;
        }

        Ok(self
            .proof_to_token(mint_url.clone(), send_proofs, memo, Some(unit.clone()))?
            .to_string())
    }

    /// Melt Quote
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn melt_quote(
        &self,
        mint_url: UncheckedUrl,
        unit: CurrencyUnit,
        request: String,
    ) -> Result<MeltQuote, Error> {
        let quote_res = self
            .client
            .post_melt_quote(
                mint_url.clone().try_into()?,
                unit.clone(),
                Bolt11Invoice::from_str(&request.clone())?,
            )
            .await?;

        let quote = MeltQuote {
            id: quote_res.quote,
            amount: quote_res.amount,
            request,
            unit,
            fee_reserve: quote_res.fee_reserve,
            paid: quote_res.paid,
            expiry: quote_res.expiry,
        };

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Melt quote status
    #[instrument(skip(self, quote_id), fields(mint_url = %mint_url))]
    pub async fn melt_quote_status(
        &self,
        mint_url: UncheckedUrl,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let response = self
            .client
            .get_melt_quote_status(mint_url.try_into()?, quote_id)
            .await?;

        match self.localstore.get_melt_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                quote.paid = response.paid;
                self.localstore.add_melt_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote melt {} unknown", quote_id);
            }
        }

        Ok(response)
    }

    // Select proofs
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn select_proofs(
        &self,
        mint_url: UncheckedUrl,
        unit: CurrencyUnit,
        amount: Amount,
        conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<(Proofs, Proofs), Error> {
        let mut condition_mint_proofs = Vec::new();

        if conditions.is_some() {
            condition_mint_proofs = self
                .localstore
                .get_proofs(
                    Some(mint_url.clone()),
                    Some(unit.clone()),
                    Some(vec![State::Unspent]),
                    conditions,
                )
                .await?
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.proof)
                .collect();
        }

        let mint_keysets = match self.localstore.get_mint_keysets(mint_url.clone()).await? {
            Some(keysets) => keysets,
            None => self.get_mint_keysets(&mint_url).await?,
        };

        let (active, inactive): (HashSet<KeySetInfo>, HashSet<KeySetInfo>) = mint_keysets
            .into_iter()
            .filter(|p| p.unit.eq(&unit.clone()))
            .partition(|x| x.active);

        let active: HashSet<Id> = active.iter().map(|k| k.id).collect();
        let inactive: HashSet<Id> = inactive.iter().map(|k| k.id).collect();

        let (mut condition_active_proofs, mut condition_inactive_proofs): (Proofs, Proofs) =
            condition_mint_proofs
                .into_iter()
                .partition(|p| active.contains(&p.keyset_id));

        condition_active_proofs.sort_by(|a, b| b.cmp(a));
        condition_inactive_proofs.sort_by(|a: &Proof, b: &Proof| b.cmp(a));

        let condition_proofs = [condition_inactive_proofs, condition_active_proofs].concat();

        let mut condition_selected_proofs: Proofs = Vec::new();

        for proof in condition_proofs {
            let mut condition_selected_proof_total = condition_selected_proofs
                .iter()
                .map(|p| p.amount)
                .sum::<Amount>();

            if condition_selected_proof_total + proof.amount <= amount {
                condition_selected_proof_total += proof.amount;
                condition_selected_proofs.push(proof);
            }

            if condition_selected_proof_total == amount {
                return Ok((condition_selected_proofs, vec![]));
            }
        }

        condition_selected_proofs.sort();

        let condition_proof_total = condition_selected_proofs.iter().map(|p| p.amount).sum();

        let mint_proofs: Proofs = self
            .localstore
            .get_proofs(
                Some(mint_url.clone()),
                Some(unit.clone()),
                Some(vec![State::Unspent]),
                None,
            )
            .await?
            .ok_or(Error::InsufficientFunds)?
            .into_iter()
            .map(|p| p.proof)
            .collect();

        let mut active_proofs: Proofs = Vec::new();
        let mut inactive_proofs: Proofs = Vec::new();

        for proof in mint_proofs {
            if active.contains(&proof.keyset_id) {
                active_proofs.push(proof);
            } else if inactive.contains(&proof.keyset_id) {
                inactive_proofs.push(proof);
            }
        }

        active_proofs.sort_by(|a: &Proof, b: &Proof| b.cmp(a));
        inactive_proofs.sort_by(|a: &Proof, b: &Proof| b.cmp(a));

        let mut selected_proofs: Proofs = Vec::new();

        for proof in [inactive_proofs, active_proofs].concat() {
            if selected_proofs.iter().map(|p| p.amount).sum::<Amount>() + condition_proof_total
                <= amount
            {
                selected_proofs.push(proof);
            } else {
                break;
            }
        }

        if selected_proofs.iter().map(|p| p.amount).sum::<Amount>() + condition_proof_total < amount
        {
            return Err(Error::InsufficientFunds);
        }

        selected_proofs.sort();

        Ok((condition_selected_proofs, selected_proofs))
    }

    /// Melt
    #[instrument(skip(self, quote_id), fields(mint_url = %mint_url))]
    pub async fn melt(
        &self,
        mint_url: &UncheckedUrl,
        quote_id: &str,
        amount_split_target: SplitTarget,
    ) -> Result<Melted, Error> {
        let quote_info = self.localstore.get_melt_quote(quote_id).await?;

        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) {
                return Err(Error::QuoteExpired);
            }

            quote.clone()
        } else {
            return Err(Error::QuoteUnknown);
        };

        let proofs = self
            .select_proofs(
                mint_url.clone(),
                quote_info.unit.clone(),
                quote_info.amount,
                None,
            )
            .await?
            .1;

        let proofs_amount = proofs.iter().map(|p| p.amount).sum();

        let active_keyset_id = self.active_mint_keyset(mint_url, &quote_info.unit).await?;

        let count = self
            .localstore
            .get_keyset_counter(&active_keyset_id)
            .await?;

        let count = count.map_or(0, |c| c + 1);

        let premint_secrets = PreMintSecrets::from_xpriv(
            active_keyset_id,
            count,
            self.xpriv,
            proofs_amount,
            true,
            &amount_split_target,
        )?;

        let melt_response = self
            .client
            .post_melt(
                mint_url.clone().try_into()?,
                quote_id.to_string(),
                proofs.clone(),
                Some(premint_secrets.blinded_messages()),
            )
            .await?;

        let change_proofs = match melt_response.change {
            Some(change) => Some(construct_proofs(
                change,
                premint_secrets.rs(),
                premint_secrets.secrets(),
                &self
                    .active_keys(mint_url, &quote_info.unit)
                    .await?
                    .ok_or(Error::UnknownKey)?,
            )?),
            None => None,
        };

        let melted = Melted {
            paid: true,
            preimage: melt_response.payment_preimage,
            change: change_proofs.clone(),
        };

        if let Some(change_proofs) = change_proofs {
            tracing::debug!(
                "Change amount returned from melt: {}",
                change_proofs.iter().map(|p| p.amount).sum::<Amount>()
            );

            // Update counter for keyset
            self.localstore
                .increment_keyset_counter(&active_keyset_id, change_proofs.len() as u32)
                .await?;

            let change_proofs_info = change_proofs
                .into_iter()
                .flat_map(|proof| {
                    ProofInfo::new(
                        proof,
                        mint_url.clone(),
                        State::Unspent,
                        quote_info.unit.clone(),
                    )
                })
                .collect();

            self.localstore.add_proofs(change_proofs_info).await?;
        }

        self.localstore.remove_melt_quote(&quote_info.id).await?;

        self.localstore.remove_proofs(&proofs).await?;

        Ok(melted)
    }

    /// Receive
    #[instrument(skip_all)]
    pub async fn receive(
        &self,
        encoded_token: &str,
        amount_split_target: &SplitTarget,
        preimages: Option<Vec<String>>,
    ) -> Result<Amount, Error> {
        let token_data = Token::from_str(encoded_token)?;

        let unit = token_data.unit.unwrap_or_default();

        let mut received_proofs: HashMap<UncheckedUrl, Proofs> = HashMap::new();
        for token in token_data.token {
            if token.proofs.is_empty() {
                continue;
            }

            // Add mint if it does not exist in the store
            if self
                .localstore
                .get_mint(token.mint.clone())
                .await?
                .is_none()
            {
                self.add_mint(token.mint.clone()).await?;
            }

            let active_keyset_id = self.active_mint_keyset(&token.mint, &unit).await?;

            let keys = self.get_keyset_keys(&token.mint, active_keyset_id).await?;

            // Sum amount of all proofs
            let amount: Amount = token.proofs.iter().map(|p| p.amount).sum();

            let mut proofs = token.proofs;

            let mut sig_flag = SigFlag::SigInputs;

            // Map hash of preimage to preimage
            let hashed_to_preimage = match preimages {
                Some(ref preimages) => preimages
                    .iter()
                    .flat_map(|p| match hex::decode(p) {
                        Ok(hex_bytes) => Some((Sha256Hash::hash(&hex_bytes).to_string(), p)),
                        Err(_) => None,
                    })
                    .collect(),
                None => HashMap::new(),
            };

            let p2pk_signing_keys = self.p2pk_signing_keys.read().await;

            for proof in &mut proofs {
                // Verify that proof DLEQ is valid
                if proof.dleq.is_some() {
                    let keys = self.get_keyset_keys(&token.mint, proof.keyset_id).await?;
                    let key = keys.amount_key(proof.amount).ok_or(Error::UnknownKey)?;
                    proof.verify_dleq(key)?;
                }

                if let Ok(secret) =
                    <crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(
                        proof.secret.clone(),
                    )
                {
                    let conditions: Result<Conditions, _> = secret.secret_data.tags.try_into();
                    if let Ok(conditions) = conditions {
                        let mut pubkeys = conditions.pubkeys.unwrap_or_default();

                        match secret.kind {
                            Kind::P2PK => {
                                let data_key = PublicKey::from_str(&secret.secret_data.data)?;

                                pubkeys.push(data_key);
                            }
                            Kind::HTLC => {
                                let hashed_preimage = &secret.secret_data.data;
                                let preimage = hashed_to_preimage
                                    .get(hashed_preimage)
                                    .ok_or(Error::PreimageNotProvided)?;
                                proof.add_preimage(preimage.to_string());
                            }
                        }
                        for pubkey in pubkeys {
                            if let Some(signing) =
                                p2pk_signing_keys.get(&pubkey.x_only_public_key())
                            {
                                proof.sign_p2pk(signing.to_owned().clone())?;
                            }
                        }

                        if conditions.sig_flag.eq(&SigFlag::SigAll) {
                            sig_flag = SigFlag::SigAll;
                        }
                    }
                }
            }

            let mut pre_swap = self
                .create_swap(
                    &token.mint,
                    &unit,
                    Some(amount),
                    amount_split_target,
                    proofs,
                    None,
                )
                .await?;

            if sig_flag.eq(&SigFlag::SigAll) {
                for blinded_message in &mut pre_swap.swap_request.outputs {
                    for signing_key in p2pk_signing_keys.values() {
                        blinded_message.sign_p2pk(signing_key.to_owned().clone())?
                    }
                }
            }

            let swap_response = self
                .client
                .post_swap(token.mint.clone().try_into()?, pre_swap.swap_request)
                .await?;

            // Proof to keep
            let p = construct_proofs(
                swap_response.signatures,
                pre_swap.pre_mint_secrets.rs(),
                pre_swap.pre_mint_secrets.secrets(),
                &keys,
            )?;
            let mint_proofs = received_proofs.entry(token.mint).or_default();

            self.localstore
                .increment_keyset_counter(&active_keyset_id, p.len() as u32)
                .await?;

            mint_proofs.extend(p);
        }

        let mut total_amount = Amount::ZERO;
        for (mint, proofs) in received_proofs {
            total_amount += proofs.iter().map(|p| p.amount).sum();
            let proofs = proofs
                .into_iter()
                .flat_map(|proof| ProofInfo::new(proof, mint.clone(), State::Unspent, unit.clone()))
                .collect();
            self.localstore.add_proofs(proofs).await?;
        }

        Ok(total_amount)
    }

    #[cfg(feature = "nostr")]
    #[instrument(skip_all)]
    pub async fn nostr_receive(
        &self,
        nostr_signing_key: SecretKey,
        since: Option<u64>,
        amount_split_target: SplitTarget,
    ) -> Result<Amount, Error> {
        use nostr_sdk::{Keys, Kind};

        let verifying_key = nostr_signing_key.public_key();

        let x_only_pubkey = verifying_key.x_only_public_key();

        let nostr_pubkey = nostr_sdk::PublicKey::from_hex(x_only_pubkey.to_string())?;

        let keys = Keys::from_str(&(nostr_signing_key).to_secret_hex())?;
        self.add_p2pk_signing_key(nostr_signing_key).await;

        let since = match since {
            Some(since) => Some(Timestamp::from(since)),
            None => self
                .localstore
                .get_nostr_last_checked(&verifying_key)
                .await?
                .map(|s| Timestamp::from(s as u64)),
        };

        let filter = match since {
            Some(since) => Filter::new()
                .pubkey(nostr_pubkey)
                .kind(Kind::EncryptedDirectMessage)
                .since(since),
            None => Filter::new()
                .pubkey(nostr_pubkey)
                .kind(Kind::EncryptedDirectMessage),
        };

        self.nostr_client.connect().await;

        let events = self.nostr_client.get_events_of(vec![filter], None).await?;

        let mut tokens: HashSet<String> = HashSet::new();

        for event in events {
            if event.kind() == Kind::EncryptedDirectMessage {
                if let Ok(msg) =
                    nip04::decrypt(keys.secret_key()?, event.author_ref(), event.content())
                {
                    if let Some(token) = Self::token_from_text(&msg) {
                        tokens.insert(token.to_string());
                    }
                } else {
                    tracing::error!("Impossible to decrypt direct message");
                }
            }
        }

        let mut total_received = Amount::ZERO;
        for token in tokens.iter() {
            match self.receive(token, &amount_split_target, None).await {
                Ok(amount) => total_received += amount,
                Err(err) => {
                    tracing::error!("Could not receive token: {}", err);
                }
            }
        }

        self.localstore
            .add_nostr_last_checked(verifying_key, unix_time() as u32)
            .await?;

        Ok(total_received)
    }

    #[cfg(feature = "nostr")]
    fn token_from_text(text: &str) -> Option<&str> {
        let text = text.trim();
        if let Some(start) = text.find("cashu") {
            match text[start..].find(' ') {
                Some(end) => return Some(&text[start..(end + start)]),
                None => return Some(&text[start..]),
            }
        }

        None
    }

    #[instrument(skip(self, proofs), fields(mint_url = %mint_url))]
    pub fn proof_to_token(
        &self,
        mint_url: UncheckedUrl,
        proofs: Proofs,
        memo: Option<String>,
        unit: Option<CurrencyUnit>,
    ) -> Result<String, Error> {
        Ok(Token::new(mint_url, proofs, memo, unit)?.to_string())
    }

    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn restore(&self, mint_url: UncheckedUrl) -> Result<Amount, Error> {
        // Check that mint is in store of mints
        if self.localstore.get_mint(mint_url.clone()).await?.is_none() {
            self.add_mint(mint_url.clone()).await?;
        }

        let keysets = self.get_mint_keysets(&mint_url).await?;

        let mut restored_value = Amount::ZERO;

        for keyset in keysets {
            let keys = self.get_keyset_keys(&mint_url, keyset.id).await?;
            let mut empty_batch = 0;
            let mut start_counter = 0;

            while empty_batch.lt(&3) {
                let premint_secrets = PreMintSecrets::restore_batch(
                    keyset.id,
                    self.xpriv,
                    start_counter,
                    start_counter + 100,
                )?;

                tracing::debug!(
                    "Attempting to restore counter {}-{} for mint {} keyset {}",
                    start_counter,
                    start_counter + 100,
                    mint_url,
                    keyset.id
                );

                let restore_request = RestoreRequest {
                    outputs: premint_secrets.blinded_messages(),
                };

                let response = self
                    .client
                    .post_restore(mint_url.clone().try_into()?, restore_request)
                    .await?;

                if response.signatures.is_empty() {
                    empty_batch += 1;
                    start_counter += 100;
                    continue;
                }

                let premint_secrets: Vec<_> = premint_secrets
                    .secrets
                    .iter()
                    .filter(|p| response.outputs.contains(&p.blinded_message))
                    .collect();

                let premint_secrets: Vec<_> = premint_secrets
                    .iter()
                    .filter(|p| response.outputs.contains(&p.blinded_message))
                    .collect();

                // the response outputs and premint secrets should be the same after filtering
                // blinded messages the mint did not have signatures for
                assert_eq!(response.outputs.len(), premint_secrets.len());

                let proofs = construct_proofs(
                    response.signatures,
                    premint_secrets.iter().map(|p| p.r.clone()).collect(),
                    premint_secrets.iter().map(|p| p.secret.clone()).collect(),
                    &keys,
                )?;

                tracing::debug!("Restored {} proofs", proofs.len());

                self.localstore
                    .increment_keyset_counter(&keyset.id, proofs.len() as u32)
                    .await?;

                let states = self
                    .check_proofs_spent(mint_url.clone(), proofs.clone())
                    .await?;

                let unspent_proofs: Vec<Proof> = proofs
                    .iter()
                    .zip(states)
                    .filter(|(_, state)| !state.state.eq(&State::Spent))
                    .map(|(p, _)| p)
                    .cloned()
                    .collect();

                restored_value += unspent_proofs.iter().map(|p| p.amount).sum();

                let unspent_proofs = unspent_proofs
                    .into_iter()
                    .flat_map(|proof| {
                        ProofInfo::new(proof, mint_url.clone(), State::Unspent, keyset.unit.clone())
                    })
                    .collect();

                self.localstore.add_proofs(unspent_proofs).await?;

                empty_batch = 0;
                start_counter += 100;
            }
        }
        Ok(restored_value)
    }

    /// Verify all proofs in token have meet the required spend
    /// Can be used to allow a wallet to accept payments offline while reducing
    /// the risk of claiming back to the limits let by the spending_conditions
    #[instrument(skip(self, token))]
    pub fn verify_token_p2pk(
        &self,
        token: &Token,
        spending_conditions: SpendingConditions,
    ) -> Result<(), Error> {
        let (refund_keys, pubkeys, locktime, num_sigs) = match spending_conditions {
            SpendingConditions::P2PKConditions { data, conditions } => {
                let mut pubkeys = vec![data];

                pubkeys.extend(conditions.pubkeys.unwrap_or_default());

                (
                    conditions.refund_keys,
                    Some(pubkeys),
                    conditions.locktime,
                    conditions.num_sigs,
                )
            }
            SpendingConditions::HTLCConditions { conditions, .. } => (
                conditions.refund_keys,
                conditions.pubkeys,
                conditions.locktime,
                conditions.num_sigs,
            ),
        };

        if refund_keys.is_some() && locktime.is_none() {
            tracing::warn!(
                "Invalid spending conditions set: Locktime must be set if refund keys are allowed"
            );
            return Err(Error::InvalidSpendConditions(
                "Must set locktime".to_string(),
            ));
        }

        for mint_proof in &token.token {
            for proof in &mint_proof.proofs {
                let secret: nut10::Secret = (&proof.secret).try_into()?;

                let proof_conditions: SpendingConditions = secret.try_into()?;

                if num_sigs.ne(&proof_conditions.num_sigs()) {
                    tracing::debug!(
                        "Spending condition requires: {:?} sigs proof secret specifies: {:?}",
                        num_sigs,
                        proof_conditions.num_sigs()
                    );

                    return Err(Error::P2PKConditionsNotMet(
                        "Num sigs did not match spending condition".to_string(),
                    ));
                }

                let spending_condition_pubkeys = pubkeys.clone().unwrap_or_default();
                let proof_pubkeys = proof_conditions.pubkeys().unwrap_or_default();

                // Check the Proof has the required pubkeys
                if proof_pubkeys.len().ne(&spending_condition_pubkeys.len())
                    || !proof_pubkeys
                        .iter()
                        .all(|pubkey| spending_condition_pubkeys.contains(pubkey))
                {
                    tracing::debug!("Proof did not included Publickeys meeting condition");
                    tracing::debug!("{:?}", proof_pubkeys);
                    tracing::debug!("{:?}", spending_condition_pubkeys);
                    return Err(Error::P2PKConditionsNotMet(
                        "Pubkeys in proof not allowed by spending condition".to_string(),
                    ));
                }

                // If spending condition refund keys is allowed (Some(Empty Vec))
                // If spending conition refund keys is allowed to restricted set of keys check
                // it is one of them Check that proof locktime is > condition
                // locktime

                if let Some(proof_refund_keys) = proof_conditions.refund_keys() {
                    let proof_locktime = proof_conditions
                        .locktime()
                        .ok_or(Error::LocktimeNotProvided)?;

                    if let (Some(condition_refund_keys), Some(condition_locktime)) =
                        (&refund_keys, locktime)
                    {
                        // Proof locktime must be greater then condition locktime to ensure it
                        // cannot be claimed back
                        if proof_locktime.lt(&condition_locktime) {
                            return Err(Error::P2PKConditionsNotMet(
                                "Proof locktime less then required".to_string(),
                            ));
                        }

                        // A non empty condition refund key list is used as a restricted set of keys
                        // returns are allowed to An empty list means the
                        // proof can be refunded to anykey set in the secret
                        if !condition_refund_keys.is_empty()
                            && !proof_refund_keys
                                .iter()
                                .all(|refund_key| condition_refund_keys.contains(refund_key))
                        {
                            return Err(Error::P2PKConditionsNotMet(
                                "Refund Key not allowed".to_string(),
                            ));
                        }
                    } else {
                        // Spending conditions does not allow refund keys
                        return Err(Error::P2PKConditionsNotMet(
                            "Spending condition does not allow refund keys".to_string(),
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Verify all proofs in token have a valid DLEQ proof
    #[instrument(skip(self, token))]
    pub async fn verify_token_dleq(&self, token: &Token) -> Result<(), Error> {
        let mut keys_cache: HashMap<Id, Keys> = HashMap::new();

        for mint_proof in &token.token {
            for proof in &mint_proof.proofs {
                let mint_pubkey = match keys_cache.get(&proof.keyset_id) {
                    Some(keys) => keys.amount_key(proof.amount),
                    None => {
                        let keys = self
                            .get_keyset_keys(&mint_proof.mint, proof.keyset_id)
                            .await?;

                        let key = keys.amount_key(proof.amount);
                        keys_cache.insert(proof.keyset_id, keys);

                        key
                    }
                }
                .ok_or(Error::UnknownKey)?;

                proof
                    .verify_dleq(mint_pubkey)
                    .map_err(|_| Error::CouldNotVerifyDleq)?;
            }
        }

        Ok(())
    }
}

#[cfg(feature = "nostr")]
#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_token_from_text() {
        let text = " Here is some ecash: cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJhbW91bnQiOjIsInNlY3JldCI6ImI2Zjk1ODIxYmZlNjUyYjYwZGQ2ZjYwMDU4N2UyZjNhOTk4MzVhMGMyNWI4MTQzODNlYWIwY2QzOWFiNDFjNzUiLCJDIjoiMDI1YWU4ZGEyOTY2Y2E5OGVmYjA5ZDcwOGMxM2FiZmEwZDkxNGUwYTk3OTE4MmFjMzQ4MDllMjYxODY5YTBhNDJlIiwicmVzZXJ2ZWQiOmZhbHNlLCJpZCI6IjAwOWExZjI5MzI1M2U0MWUifSx7ImFtb3VudCI6Miwic2VjcmV0IjoiZjU0Y2JjNmNhZWZmYTY5MTUyOTgyM2M1MjU1MDkwYjRhMDZjNGQ3ZDRjNzNhNDFlZTFkNDBlM2ExY2EzZGZhNyIsIkMiOiIwMjMyMTIzN2JlYjcyMWU3NGI1NzcwNWE5MjJjNjUxMGQwOTYyYzAzNzlhZDM0OTJhMDYwMDliZTAyNjA5ZjA3NTAiLCJyZXNlcnZlZCI6ZmFsc2UsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSJ9LHsiYW1vdW50IjoxLCJzZWNyZXQiOiJhNzdhM2NjODY4YWM4ZGU3YmNiOWMxMzJmZWI3YzEzMDY4Nzg3ODk5Yzk3YTk2NWE2ZThkZTFiMzliMmQ2NmQ3IiwiQyI6IjAzMTY0YTMxNWVhNjM0NGE5NWI2NzM1NzBkYzg0YmZlMTQ2NDhmMTQwM2EwMDJiZmJlMDhlNWFhMWE0NDQ0YWE0MCIsInJlc2VydmVkIjpmYWxzZSwiaWQiOiIwMDlhMWYyOTMyNTNlNDFlIn1dLCJtaW50IjoiaHR0cHM6Ly90ZXN0bnV0LmNhc2h1LnNwYWNlIn1dLCJ1bml0Ijoic2F0In0= fdfdfg
        sdfs";
        let token = Wallet::token_from_text(text).unwrap();

        let token_str = "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJhbW91bnQiOjIsInNlY3JldCI6ImI2Zjk1ODIxYmZlNjUyYjYwZGQ2ZjYwMDU4N2UyZjNhOTk4MzVhMGMyNWI4MTQzODNlYWIwY2QzOWFiNDFjNzUiLCJDIjoiMDI1YWU4ZGEyOTY2Y2E5OGVmYjA5ZDcwOGMxM2FiZmEwZDkxNGUwYTk3OTE4MmFjMzQ4MDllMjYxODY5YTBhNDJlIiwicmVzZXJ2ZWQiOmZhbHNlLCJpZCI6IjAwOWExZjI5MzI1M2U0MWUifSx7ImFtb3VudCI6Miwic2VjcmV0IjoiZjU0Y2JjNmNhZWZmYTY5MTUyOTgyM2M1MjU1MDkwYjRhMDZjNGQ3ZDRjNzNhNDFlZTFkNDBlM2ExY2EzZGZhNyIsIkMiOiIwMjMyMTIzN2JlYjcyMWU3NGI1NzcwNWE5MjJjNjUxMGQwOTYyYzAzNzlhZDM0OTJhMDYwMDliZTAyNjA5ZjA3NTAiLCJyZXNlcnZlZCI6ZmFsc2UsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSJ9LHsiYW1vdW50IjoxLCJzZWNyZXQiOiJhNzdhM2NjODY4YWM4ZGU3YmNiOWMxMzJmZWI3YzEzMDY4Nzg3ODk5Yzk3YTk2NWE2ZThkZTFiMzliMmQ2NmQ3IiwiQyI6IjAzMTY0YTMxNWVhNjM0NGE5NWI2NzM1NzBkYzg0YmZlMTQ2NDhmMTQwM2EwMDJiZmJlMDhlNWFhMWE0NDQ0YWE0MCIsInJlc2VydmVkIjpmYWxzZSwiaWQiOiIwMDlhMWYyOTMyNTNlNDFlIn1dLCJtaW50IjoiaHR0cHM6Ly90ZXN0bnV0LmNhc2h1LnNwYWNlIn1dLCJ1bml0Ijoic2F0In0=";

        assert_eq!(token, token_str)
    }
}
