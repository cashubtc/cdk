//! Cashu Wallet
//!
//! Each wallet is single mint and single unit

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use bitcoin::bip32::ExtendedPrivKey;
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::key::XOnlyPublicKey;
use bitcoin::Network;
use error::Error;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::cdk_database::{self, WalletDatabase};
use crate::dhke::{construct_proofs, hash_to_curve};
use crate::nuts::nut00::token::Token;
use crate::nuts::{
    nut10, nut12, Conditions, CurrencyUnit, Id, KeySetInfo, Keys, Kind, MeltQuoteBolt11Response,
    MeltQuoteState, MintInfo, MintQuoteBolt11Response, MintQuoteState, PreMintSecrets, PreSwap,
    Proof, ProofState, Proofs, PublicKey, RestoreRequest, SecretKey, SigFlag, SpendingConditions,
    State, SwapRequest,
};
use crate::types::{Melted, ProofInfo};
use crate::url::UncheckedUrl;
use crate::util::{hex, unix_time};
use crate::{Amount, Bolt11Invoice, HttpClient, SECP256K1};

pub mod client;
pub mod error;
pub mod multi_mint_wallet;
pub mod types;
pub mod util;

pub use multi_mint_wallet::MultiMintWallet;
pub use types::{MeltQuote, MintQuote, SendKind};

/// CDK Wallet
#[derive(Debug, Clone)]
pub struct Wallet {
    /// Mint Url
    pub mint_url: UncheckedUrl,
    /// Unit
    pub unit: CurrencyUnit,
    /// Storage backend
    pub localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync>,
    /// The targeted amount of proofs to have at each size
    pub target_proof_count: usize,
    xpriv: ExtendedPrivKey,
    client: HttpClient,
}

impl Wallet {
    /// Create new [`Wallet`]
    pub fn new(
        mint_url: &str,
        unit: CurrencyUnit,
        localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync>,
        seed: &[u8],
        target_proof_count: Option<usize>,
    ) -> Self {
        let xpriv = ExtendedPrivKey::new_master(Network::Bitcoin, seed)
            .expect("Could not create master key");

        Self {
            mint_url: UncheckedUrl::from(mint_url),
            unit,
            client: HttpClient::new(),
            localstore,
            xpriv,
            target_proof_count: target_proof_count.unwrap_or(3),
        }
    }

    /// Fee required for proof set
    #[instrument(skip_all)]
    pub async fn get_proofs_fee(&self, proofs: &Proofs) -> Result<Amount, Error> {
        let mut sum_fee = 0;

        for proof in proofs {
            let input_fee_ppk = self
                .localstore
                .get_keyset_by_id(&proof.keyset_id)
                .await?
                .ok_or(Error::UnknownKey)?;

            sum_fee += input_fee_ppk.input_fee_ppk;
        }

        let fee = (sum_fee + 999) / 1000;

        Ok(Amount::from(fee))
    }

    /// Get fee for count of proofs in a keyset
    #[instrument(skip_all)]
    pub async fn get_keyset_count_fee(&self, keyset_id: &Id, count: u64) -> Result<Amount, Error> {
        let input_fee_ppk = self
            .localstore
            .get_keyset_by_id(keyset_id)
            .await?
            .ok_or(Error::UnknownKey)?
            .input_fee_ppk;

        let fee = (input_fee_ppk * count + 999) / 1000;

        Ok(Amount::from(fee))
    }

    /// Total unspent balance of wallet
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<Amount, Error> {
        let proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Unspent]),
                None,
            )
            .await?;
        let balance = proofs.iter().map(|p| p.proof.amount).sum::<Amount>();

        Ok(balance)
    }

    /// Total pending balance
    #[instrument(skip(self))]
    pub async fn total_pending_balance(&self) -> Result<HashMap<CurrencyUnit, Amount>, Error> {
        let proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Pending]),
                None,
            )
            .await?;

        let balances = proofs.iter().fold(HashMap::new(), |mut acc, proof| {
            *acc.entry(proof.unit).or_insert(Amount::ZERO) += proof.proof.amount;
            acc
        });

        Ok(balances)
    }

    /// Total reserved balance
    #[instrument(skip(self))]
    pub async fn total_reserved_balance(&self) -> Result<HashMap<CurrencyUnit, Amount>, Error> {
        let proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Reserved]),
                None,
            )
            .await?;

        let balances = proofs.iter().fold(HashMap::new(), |mut acc, proof| {
            *acc.entry(proof.unit).or_insert(Amount::ZERO) += proof.proof.amount;
            acc
        });

        Ok(balances)
    }

    /// Update Mint information and related entries in the event a mint changes its URL
    #[instrument(skip(self))]
    pub async fn update_mint_url(&mut self, new_mint_url: UncheckedUrl) -> Result<(), Error> {
        self.mint_url = new_mint_url.clone();
        // Where the mint_url is in the database it must be updated
        self.localstore
            .update_mint_url(self.mint_url.clone(), new_mint_url)
            .await?;

        self.localstore.remove_mint(self.mint_url.clone()).await?;
        Ok(())
    }

    /// Get unspent proofs for mint
    #[instrument(skip(self))]
    pub async fn get_proofs(&self) -> Result<Proofs, Error> {
        Ok(self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Unspent]),
                None,
            )
            .await?
            .into_iter()
            .map(|p| p.proof)
            .collect())
    }

    /// Get pending [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_pending_proofs(&self) -> Result<Proofs, Error> {
        Ok(self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Pending]),
                None,
            )
            .await?
            .into_iter()
            .map(|p| p.proof)
            .collect())
    }

    /// Get reserved [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_reserved_proofs(&self) -> Result<Proofs, Error> {
        Ok(self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Reserved]),
                None,
            )
            .await?
            .into_iter()
            .map(|p| p.proof)
            .collect())
    }

    /// Return proofs to unspent allowing them to be selected and spent
    #[instrument(skip(self))]
    pub async fn unreserve_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Error> {
        for y in ys {
            self.localstore.set_proof_state(y, State::Unspent).await?;
        }

        Ok(())
    }

    /// Add mint to wallet
    #[instrument(skip(self))]
    pub async fn get_mint_info(&self) -> Result<Option<MintInfo>, Error> {
        let mint_info = match self
            .client
            .get_mint_info(self.mint_url.clone().try_into()?)
            .await
        {
            Ok(mint_info) => Some(mint_info),
            Err(err) => {
                tracing::warn!("Could not get mint info {}", err);
                None
            }
        };

        self.localstore
            .add_mint(self.mint_url.clone(), mint_info.clone())
            .await?;

        tracing::trace!("Mint info fetched for {}", self.mint_url);

        Ok(mint_info)
    }

    /// Get keys for mint keyset
    #[instrument(skip(self))]
    pub async fn get_keyset_keys(&self, keyset_id: Id) -> Result<Keys, Error> {
        let keys = if let Some(keys) = self.localstore.get_keys(&keyset_id).await? {
            keys
        } else {
            let keys = self
                .client
                .get_mint_keyset(self.mint_url.clone().try_into()?, keyset_id)
                .await?;

            self.localstore.add_keys(keys.keys.clone()).await?;

            keys.keys
        };

        Ok(keys)
    }

    /// Get keysets for mint
    #[instrument(skip(self))]
    pub async fn get_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self
            .client
            .get_mint_keysets(self.mint_url.clone().try_into()?)
            .await?;

        self.localstore
            .add_mint_keysets(self.mint_url.clone(), keysets.keysets.clone())
            .await?;

        Ok(keysets.keysets)
    }

    /// Get active keyset for mint
    /// Quieries mint for current keysets then gets Keys for any unknown keysets
    #[instrument(skip(self))]
    pub async fn get_active_mint_keyset(&self) -> Result<KeySetInfo, Error> {
        let keysets = self
            .client
            .get_mint_keysets(self.mint_url.clone().try_into()?)
            .await?;
        let keysets = keysets.keysets;

        self.localstore
            .add_mint_keysets(self.mint_url.clone(), keysets.clone())
            .await?;

        let active_keysets = keysets
            .clone()
            .into_iter()
            .filter(|k| k.active && k.unit == self.unit)
            .collect::<Vec<KeySetInfo>>();

        match self
            .localstore
            .get_mint_keysets(self.mint_url.clone())
            .await?
        {
            Some(known_keysets) => {
                let unknown_keysets: Vec<&KeySetInfo> = keysets
                    .iter()
                    .filter(|k| known_keysets.contains(k))
                    .collect();

                for keyset in unknown_keysets {
                    self.get_keyset_keys(keyset.id).await?;
                }
            }
            None => {
                for keyset in keysets {
                    self.get_keyset_keys(keyset.id).await?;
                }
            }
        }

        active_keysets.first().ok_or(Error::NoActiveKeyset).cloned()
    }

    /// Reclaim unspent proofs
    #[instrument(skip(self, proofs))]
    pub async fn reclaim_unspent(&self, proofs: Proofs) -> Result<(), Error> {
        let proof_ys = proofs
            .iter()
            // Find Y for the secret
            .flat_map(|p| hash_to_curve(p.secret.as_bytes()))
            .collect::<Vec<PublicKey>>();

        let spendable = self
            .client
            .post_check_state(self.mint_url.clone().try_into()?, proof_ys)
            .await?
            .states;

        let unspent: Proofs = proofs
            .into_iter()
            .zip(spendable)
            .filter_map(|(p, s)| (s.state == State::Unspent).then_some(p))
            .collect();

        self.swap(None, SplitTarget::default(), unspent, None, false)
            .await?;

        Ok(())
    }

    /// Check if a proof is spent
    #[instrument(skip(self, proofs))]
    pub async fn check_proofs_spent(&self, proofs: Proofs) -> Result<Vec<ProofState>, Error> {
        let spendable = self
            .client
            .post_check_state(
                self.mint_url.clone().try_into()?,
                proofs
                    .iter()
                    // Find Y for the secret
                    .flat_map(|p| hash_to_curve(p.secret.as_bytes()))
                    .collect::<Vec<PublicKey>>(),
            )
            .await?;

        Ok(spendable.states)
    }

    /// Checks pending proofs for spent status
    #[instrument(skip(self))]
    pub async fn check_all_pending_proofs(&self) -> Result<Amount, Error> {
        let mut balance = Amount::ZERO;

        let proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Pending, State::Reserved]),
                None,
            )
            .await?;

        if proofs.is_empty() {
            return Ok(Amount::ZERO);
        }

        let states = self
            .check_proofs_spent(proofs.clone().into_iter().map(|p| p.proof).collect())
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

        Ok(balance)
    }

    /// Mint Quote
    #[instrument(skip(self))]
    pub async fn mint_quote(&self, amount: Amount) -> Result<MintQuote, Error> {
        let mint_url = self.mint_url.clone();
        let unit = self.unit;
        let quote_res = self
            .client
            .post_mint_quote(mint_url.clone().try_into()?, amount, unit)
            .await?;

        let quote = MintQuote {
            mint_url,
            id: quote_res.quote.clone(),
            amount,
            unit,
            request: quote_res.request,
            state: quote_res.state,
            expiry: quote_res.expiry.unwrap_or(0),
        };

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Mint quote status
    #[instrument(skip(self, quote_id))]
    pub async fn mint_quote_state(&self, quote_id: &str) -> Result<MintQuoteBolt11Response, Error> {
        let response = self
            .client
            .get_mint_quote_status(self.mint_url.clone().try_into()?, quote_id)
            .await?;

        match self.localstore.get_mint_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                quote.state = response.state;
                self.localstore.add_mint_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote mint {} unknown", quote_id);
            }
        }

        Ok(response)
    }

    /// Check status of pending mint quotes
    #[instrument(skip(self))]
    pub async fn check_all_mint_quotes(&self) -> Result<Amount, Error> {
        let mint_quotes = self.localstore.get_mint_quotes().await?;
        let mut total_amount = Amount::ZERO;

        for mint_quote in mint_quotes {
            let mint_quote_response = self.mint_quote_state(&mint_quote.id).await?;

            if mint_quote_response.state == MintQuoteState::Paid {
                let amount = self
                    .mint(&mint_quote.id, SplitTarget::default(), None)
                    .await?;
                total_amount += amount;
            } else if mint_quote.expiry.le(&unix_time()) {
                self.localstore.remove_mint_quote(&mint_quote.id).await?;
            }
        }
        Ok(total_amount)
    }

    /// Mint
    #[instrument(skip(self))]
    pub async fn mint(
        &self,
        quote_id: &str,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Amount, Error> {
        // Check that mint is in store of mints
        if self
            .localstore
            .get_mint(self.mint_url.clone())
            .await?
            .is_none()
        {
            self.get_mint_info().await?;
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

        let active_keyset_id = self.get_active_mint_keyset().await?.id;

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
                &amount_split_target,
            )?,
        };

        let mint_res = self
            .client
            .post_mint(
                self.mint_url.clone().try_into()?,
                quote_id,
                premint_secrets.clone(),
            )
            .await?;

        let keys = self.get_keyset_keys(active_keyset_id).await?;

        // Verify the signature DLEQ is valid
        {
            for (sig, premint) in mint_res.signatures.iter().zip(&premint_secrets.secrets) {
                let keys = self.get_keyset_keys(sig.keyset_id).await?;
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
                    self.mint_url.clone(),
                    State::Unspent,
                    quote_info.unit,
                )
            })
            .collect();

        // Add new proofs to store
        self.localstore.add_proofs(proofs).await?;

        Ok(minted_amount)
    }

    /// Get amounts needed to refill proof state
    #[instrument(skip(self))]
    pub async fn amounts_needed_for_state_target(&self) -> Result<Vec<Amount>, Error> {
        let unspent_proofs = self.get_proofs().await?;

        let amounts_count: HashMap<usize, usize> =
            unspent_proofs
                .iter()
                .fold(HashMap::new(), |mut acc, proof| {
                    let amount = proof.amount;
                    let counter = acc.entry(u64::from(amount) as usize).or_insert(0);
                    *counter += 1;
                    acc
                });

        let all_possible_amounts: Vec<usize> = (0..32).map(|i| 2usize.pow(i as u32)).collect();

        let needed_amounts = all_possible_amounts
            .iter()
            .fold(Vec::new(), |mut acc, amount| {
                let count_needed: usize = self
                    .target_proof_count
                    .saturating_sub(*amounts_count.get(amount).unwrap_or(&0));

                for _i in 0..count_needed {
                    acc.push(Amount::from(*amount as u64));
                }

                acc
            });
        Ok(needed_amounts)
    }

    /// Determine [`SplitTarget`] for amount based on state
    #[instrument(skip(self))]
    async fn determine_split_target_values(
        &self,
        change_amount: Amount,
    ) -> Result<SplitTarget, Error> {
        let mut amounts_needed_refill = self.amounts_needed_for_state_target().await?;

        amounts_needed_refill.sort();

        let mut values = Vec::new();

        for amount in amounts_needed_refill {
            let values_sum: Amount = values.clone().into_iter().sum();
            if values_sum + amount <= change_amount {
                values.push(amount);
            }
        }

        Ok(SplitTarget::Values(values))
    }

    /// Create Swap Payload
    #[instrument(skip(self, proofs))]
    pub async fn create_swap(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<PreSwap, Error> {
        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        // Desired amount is either amount passed or value of all proof
        let proofs_total: Amount = proofs.iter().map(|p| p.amount).sum();

        for proof in proofs.iter() {
            self.localstore
                .set_proof_state(proof.y()?, State::Pending)
                .await
                .ok();
        }

        let fee = self.get_proofs_fee(&proofs).await?;

        let change_amount: Amount = proofs_total - amount.unwrap_or(Amount::ZERO) - fee;

        let (send_amount, change_amount) = match include_fees {
            true => {
                let split_count = amount
                    .unwrap_or(Amount::ZERO)
                    .split_targeted(&SplitTarget::default())
                    .unwrap()
                    .len();

                let fee_to_redeam = self
                    .get_keyset_count_fee(&active_keyset_id, split_count as u64)
                    .await?;

                (
                    amount.map(|a| a + fee_to_redeam),
                    change_amount - fee_to_redeam,
                )
            }
            false => (amount, change_amount),
        };

        // If a non None split target is passed use that
        // else use state refill
        let change_split_target = match amount_split_target {
            SplitTarget::None => self.determine_split_target_values(change_amount).await?,
            s => s,
        };

        let derived_secret_count;

        let count = self
            .localstore
            .get_keyset_counter(&active_keyset_id)
            .await?;

        let mut count = count.map_or(0, |c| c + 1);

        let (mut desired_messages, change_messages) = match spending_conditions {
            Some(conditions) => {
                let change_premint_secrets = PreMintSecrets::from_xpriv(
                    active_keyset_id,
                    count,
                    self.xpriv,
                    change_amount,
                    &change_split_target,
                )?;

                derived_secret_count = change_premint_secrets.len();

                (
                    PreMintSecrets::with_conditions(
                        active_keyset_id,
                        send_amount.unwrap_or(Amount::ZERO),
                        &SplitTarget::default(),
                        &conditions,
                    )?,
                    change_premint_secrets,
                )
            }
            None => {
                let premint_secrets = PreMintSecrets::from_xpriv(
                    active_keyset_id,
                    count,
                    self.xpriv,
                    send_amount.unwrap_or(Amount::ZERO),
                    &SplitTarget::default(),
                )?;

                count += premint_secrets.len() as u32;

                let change_premint_secrets = PreMintSecrets::from_xpriv(
                    active_keyset_id,
                    count,
                    self.xpriv,
                    change_amount,
                    &change_split_target,
                )?;

                derived_secret_count = change_premint_secrets.len() + premint_secrets.len();

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
            derived_secret_count: derived_secret_count as u32,
            fee,
        })
    }

    /// Swap
    #[instrument(skip(self, input_proofs))]
    pub async fn swap(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<Option<Proofs>, Error> {
        let mint_url = &self.mint_url;
        let unit = &self.unit;

        let pre_swap = self
            .create_swap(
                amount,
                amount_split_target,
                input_proofs.clone(),
                spending_conditions.clone(),
                include_fees,
            )
            .await?;

        let swap_response = self
            .client
            .post_swap(mint_url.clone().try_into()?, pre_swap.swap_request)
            .await?;

        let active_keyset_id = pre_swap.pre_mint_secrets.keyset_id;

        let active_keys = self
            .localstore
            .get_keys(&active_keyset_id)
            .await?
            .ok_or(Error::NoActiveKeyset)?;

        let post_swap_proofs = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &active_keys,
        )?;

        self.localstore
            .increment_keyset_counter(&active_keyset_id, pre_swap.derived_secret_count)
            .await?;

        let change_proofs;
        let send_proofs;

        match amount {
            Some(amount) => {
                let (proofs_with_condition, proofs_without_condition): (Proofs, Proofs) =
                    post_swap_proofs.into_iter().partition(|p| {
                        let nut10_secret: Result<nut10::Secret, _> = p.secret.clone().try_into();

                        nut10_secret.is_ok()
                    });

                let (proofs_to_send, proofs_to_keep) = match spending_conditions {
                    Some(_) => (proofs_with_condition, proofs_without_condition),
                    None => {
                        let mut all_proofs = proofs_without_condition;
                        all_proofs.reverse();

                        let mut proofs_to_send: Proofs = Vec::new();
                        let mut proofs_to_keep = Vec::new();

                        for proof in all_proofs {
                            let proofs_to_send_amount =
                                proofs_to_send.iter().map(|p| p.amount).sum::<Amount>();
                            if proof.amount + proofs_to_send_amount <= amount + pre_swap.fee {
                                proofs_to_send.push(proof);
                            } else {
                                proofs_to_keep.push(proof);
                            }
                        }

                        (proofs_to_send, proofs_to_keep)
                    }
                };

                let send_amount: Amount = proofs_to_send.iter().map(|p| p.amount).sum();

                if send_amount.ne(&(amount + pre_swap.fee)) {
                    tracing::warn!(
                        "Send amount proofs is {:?} expected {:?}",
                        send_amount,
                        amount
                    );
                }

                let send_proofs_info = proofs_to_send
                    .clone()
                    .into_iter()
                    .flat_map(|proof| {
                        ProofInfo::new(proof, mint_url.clone(), State::Reserved, *unit)
                    })
                    .collect();

                self.localstore.add_proofs(send_proofs_info).await?;

                change_proofs = proofs_to_keep;
                send_proofs = Some(proofs_to_send);
            }
            None => {
                change_proofs = post_swap_proofs;
                send_proofs = None;
            }
        }

        let keep_proofs = change_proofs
            .into_iter()
            .flat_map(|proof| ProofInfo::new(proof, mint_url.clone(), State::Unspent, *unit))
            .collect();

        self.localstore.add_proofs(keep_proofs).await?;

        // Remove spent proofs used as inputs
        self.localstore.remove_proofs(&input_proofs).await?;

        Ok(send_proofs)
    }

    #[instrument(skip(self))]
    async fn swap_from_unspent(
        &self,
        amount: Amount,
        conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<Proofs, Error> {
        let available_proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Unspent]),
                None,
            )
            .await?;

        let (available_proofs, proofs_sum) = available_proofs.into_iter().map(|p| p.proof).fold(
            (Vec::new(), Amount::ZERO),
            |(mut acc1, mut acc2), p| {
                acc2 += p.amount;
                acc1.push(p);
                (acc1, acc2)
            },
        );

        if proofs_sum < amount {
            return Err(Error::InsufficientFunds);
        }

        let proofs = self.select_proofs_to_swap(amount, available_proofs).await?;

        self.swap(
            Some(amount),
            SplitTarget::default(),
            proofs,
            conditions,
            include_fees,
        )
        .await?
        .ok_or(Error::InsufficientFunds)
    }

    /// Send specific proofs
    #[instrument(skip(self))]
    pub async fn send_proofs(&self, memo: Option<String>, proofs: Proofs) -> Result<Token, Error> {
        for proof in proofs.iter() {
            self.localstore
                .set_proof_state(proof.y()?, State::Reserved)
                .await?;
        }

        Ok(Token::new(
            self.mint_url.clone(),
            proofs,
            memo,
            Some(self.unit),
        ))
    }

    /// Send
    #[instrument(skip(self))]
    pub async fn send(
        &self,
        amount: Amount,
        memo: Option<String>,
        conditions: Option<SpendingConditions>,
        amount_split_target: &SplitTarget,
        send_kind: &SendKind,
        include_fees: bool,
    ) -> Result<Token, Error> {
        // If online send check mint for current keysets fees
        if matches!(
            send_kind,
            SendKind::OnlineExact | SendKind::OnlineTolerance(_)
        ) {
            if let Err(e) = self.get_active_mint_keyset().await {
                tracing::error!(
                    "Error fetching active mint keyset: {:?}. Using stored keysets",
                    e
                );
            }
        }

        let mint_url = &self.mint_url;
        let unit = &self.unit;
        let available_proofs = self
            .localstore
            .get_proofs(
                Some(mint_url.clone()),
                Some(*unit),
                Some(vec![State::Unspent]),
                conditions.clone().map(|c| vec![c]),
            )
            .await?;

        let (available_proofs, proofs_sum) = available_proofs.into_iter().map(|p| p.proof).fold(
            (Vec::new(), Amount::ZERO),
            |(mut acc1, mut acc2), p| {
                acc2 += p.amount;
                acc1.push(p);
                (acc1, acc2)
            },
        );
        let available_proofs = if proofs_sum < amount {
            match &conditions {
                Some(conditions) => {
                    let available_proofs = self
                        .localstore
                        .get_proofs(
                            Some(mint_url.clone()),
                            Some(*unit),
                            Some(vec![State::Unspent]),
                            None,
                        )
                        .await?;

                    let available_proofs = available_proofs.into_iter().map(|p| p.proof).collect();

                    let proofs_to_swap =
                        self.select_proofs_to_swap(amount, available_proofs).await?;

                    let proofs_with_conditions = self
                        .swap(
                            Some(amount),
                            SplitTarget::default(),
                            proofs_to_swap,
                            Some(conditions.clone()),
                            include_fees,
                        )
                        .await?;
                    proofs_with_conditions.ok_or(Error::InsufficientFunds)?
                }
                None => {
                    return Err(Error::InsufficientFunds);
                }
            }
        } else {
            available_proofs
        };

        let selected = self
            .select_proofs_to_send(amount, available_proofs, include_fees)
            .await;

        let send_proofs: Proofs = match (send_kind, selected, conditions.clone()) {
            // Handle exact matches offline
            (SendKind::OfflineExact, Ok(selected_proofs), _) => {
                let selected_proofs_amount =
                    selected_proofs.iter().map(|p| p.amount).sum::<Amount>();

                let amount_to_send = match include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };

                if selected_proofs_amount == amount_to_send {
                    selected_proofs
                } else {
                    return Err(Error::InsufficientFunds);
                }
            }

            // Handle exact matches
            (SendKind::OnlineExact, Ok(selected_proofs), _) => {
                let selected_proofs_amount =
                    selected_proofs.iter().map(|p| p.amount).sum::<Amount>();

                let amount_to_send = match include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };

                if selected_proofs_amount == amount_to_send {
                    selected_proofs
                } else {
                    tracing::info!("Could not select proofs exact while offline.");
                    tracing::info!("Attempting to select proofs and swapping");

                    self.swap_from_unspent(amount, conditions, include_fees)
                        .await?
                }
            }

            // Handle offline tolerance
            (SendKind::OfflineTolerance(tolerance), Ok(selected_proofs), _) => {
                let selected_proofs_amount =
                    selected_proofs.iter().map(|p| p.amount).sum::<Amount>();

                let amount_to_send = match include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };
                if selected_proofs_amount - amount_to_send <= *tolerance {
                    selected_proofs
                } else {
                    tracing::info!("Selected proofs greater than tolerance. Must swap online");
                    return Err(Error::InsufficientFunds);
                }
            }

            // Handle online tolerance when selection fails and conditions are present
            (SendKind::OnlineTolerance(_), Err(_), Some(_)) => {
                tracing::info!("Could not select proofs with conditions while offline.");
                tracing::info!("Attempting to select proofs without conditions and swapping");

                self.swap_from_unspent(amount, conditions, include_fees)
                    .await?
            }

            // Handle online tolerance with successful selection
            (SendKind::OnlineTolerance(tolerance), Ok(selected_proofs), _) => {
                let selected_proofs_amount =
                    selected_proofs.iter().map(|p| p.amount).sum::<Amount>();
                let amount_to_send = match include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };
                if selected_proofs_amount - amount_to_send <= *tolerance {
                    selected_proofs
                } else {
                    tracing::info!("Could not select proofs while offline. Attempting swap");
                    self.swap_from_unspent(amount, conditions, include_fees)
                        .await?
                }
            }

            // Handle all other cases where selection fails
            (
                SendKind::OfflineExact
                | SendKind::OnlineExact
                | SendKind::OfflineTolerance(_)
                | SendKind::OnlineTolerance(_),
                Err(_),
                _,
            ) => {
                tracing::debug!("Could not select proofs");
                return Err(Error::InsufficientFunds);
            }
        };

        self.send_proofs(memo, send_proofs).await
    }

    /// Melt Quote
    #[instrument(skip(self))]
    pub async fn melt_quote(
        &self,
        request: String,
        mpp: Option<Amount>,
    ) -> Result<MeltQuote, Error> {
        let invoice = Bolt11Invoice::from_str(&request)?;

        let request_amount = invoice
            .amount_milli_satoshis()
            .ok_or(Error::InvoiceAmountUndefined)?;

        let amount = match self.unit {
            CurrencyUnit::Sat => Amount::from(request_amount / 1000),
            CurrencyUnit::Msat => Amount::from(request_amount),
            _ => return Err(Error::UnitNotSupported),
        };

        let quote_res = self
            .client
            .post_melt_quote(self.mint_url.clone().try_into()?, self.unit, invoice, mpp)
            .await?;

        if quote_res.amount != amount {
            return Err(Error::IncorrectQuoteAmount);
        }

        let quote = MeltQuote {
            id: quote_res.quote,
            amount,
            request,
            unit: self.unit,
            fee_reserve: quote_res.fee_reserve,
            state: quote_res.state,
            expiry: quote_res.expiry,
            payment_preimage: quote_res.payment_preimage,
        };

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Melt quote status
    #[instrument(skip(self, quote_id))]
    pub async fn melt_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let response = self
            .client
            .get_melt_quote_status(self.mint_url.clone().try_into()?, quote_id)
            .await?;

        match self.localstore.get_melt_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                quote.state = response.state;
                self.localstore.add_melt_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote melt {} unknown", quote_id);
            }
        }

        Ok(response)
    }

    /// Melt specific proofs
    #[instrument(skip(self))]
    pub async fn melt_proofs(&self, quote_id: &str, proofs: Proofs) -> Result<Melted, Error> {
        let quote_info = self.localstore.get_melt_quote(quote_id).await?;
        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) {
                return Err(Error::QuoteExpired);
            }

            quote.clone()
        } else {
            return Err(Error::QuoteUnknown);
        };

        for proof in proofs.iter() {
            self.localstore
                .set_proof_state(proof.y()?, State::Pending)
                .await?;
        }

        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        let count = self
            .localstore
            .get_keyset_counter(&active_keyset_id)
            .await?;

        let count = count.map_or(0, |c| c + 1);

        let premint_secrets = PreMintSecrets::from_xpriv_blank(
            active_keyset_id,
            count,
            self.xpriv,
            quote_info.fee_reserve,
        )?;

        let melt_response = self
            .client
            .post_melt(
                self.mint_url.clone().try_into()?,
                quote_id.to_string(),
                proofs.clone(),
                Some(premint_secrets.blinded_messages()),
            )
            .await;

        let melt_response = match melt_response {
            Ok(melt_response) => melt_response,
            Err(err) => {
                tracing::error!("Could not melt: {}", err);
                tracing::info!("Checking status of input proofs.");

                self.reclaim_unspent(proofs).await?;

                return Err(err);
            }
        };

        let active_keys = self
            .localstore
            .get_keys(&active_keyset_id)
            .await?
            .ok_or(Error::NoActiveKeyset)?;

        let change_proofs = match melt_response.change {
            Some(change) => Some(construct_proofs(
                change,
                premint_secrets.rs(),
                premint_secrets.secrets(),
                &active_keys,
            )?),
            None => None,
        };

        let state = match melt_response.paid {
            true => MeltQuoteState::Paid,
            false => MeltQuoteState::Unpaid,
        };

        let melted = Melted {
            state,
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
                        self.mint_url.clone(),
                        State::Unspent,
                        quote_info.unit,
                    )
                })
                .collect();

            self.localstore.add_proofs(change_proofs_info).await?;
        }

        self.localstore.remove_melt_quote(&quote_info.id).await?;

        self.localstore.remove_proofs(&proofs).await?;

        Ok(melted)
    }

    /// Melt
    #[instrument(skip(self))]
    pub async fn melt(&self, quote_id: &str) -> Result<Melted, Error> {
        let quote_info = self.localstore.get_melt_quote(quote_id).await?;

        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) {
                return Err(Error::QuoteExpired);
            }

            quote.clone()
        } else {
            return Err(Error::QuoteUnknown);
        };

        let inputs_needed_amount = quote_info.amount + quote_info.fee_reserve;

        let available_proofs = self.get_proofs().await?;

        let input_proofs = self
            .select_proofs_to_swap(inputs_needed_amount, available_proofs)
            .await?;

        self.melt_proofs(quote_id, input_proofs).await
    }

    /// Select proofs to send
    #[instrument(skip_all)]
    pub async fn select_proofs_to_send(
        &self,
        amount: Amount,
        proofs: Proofs,
        include_fees: bool,
    ) -> Result<Proofs, Error> {
        // TODO: Check all proofs are same unit

        if proofs.iter().map(|p| p.amount).sum::<Amount>() < amount {
            return Err(Error::InsufficientFunds);
        }

        let (mut proofs_larger, mut proofs_smaller): (Proofs, Proofs) =
            proofs.into_iter().partition(|p| p.amount > amount);

        let next_bigger_proof = proofs_larger.first().cloned();

        let mut selected_proofs: Vec<Proof> = Vec::new();
        let mut remaining_amount = amount;

        while remaining_amount > Amount::ZERO {
            proofs_larger.sort();
            // Sort smaller proofs in descending order
            proofs_smaller.sort_by(|a: &Proof, b: &Proof| b.cmp(a));

            let selected_proof = if let Some(next_small) = proofs_smaller.clone().first() {
                next_small.clone()
            } else if let Some(next_bigger) = proofs_larger.first() {
                next_bigger.clone()
            } else {
                break;
            };

            let proof_amount = selected_proof.amount;

            selected_proofs.push(selected_proof);

            let fees = match include_fees {
                true => self.get_proofs_fee(&selected_proofs).await?,
                false => Amount::ZERO,
            };

            if proof_amount >= remaining_amount + fees {
                remaining_amount = Amount::ZERO;
                break;
            }

            remaining_amount =
                amount + fees - selected_proofs.iter().map(|p| p.amount).sum::<Amount>();
            (proofs_larger, proofs_smaller) = proofs_smaller
                .into_iter()
                .skip(1)
                .partition(|p| p.amount > remaining_amount);
        }

        if remaining_amount > Amount::ZERO {
            if let Some(next_bigger) = next_bigger_proof {
                return Ok(vec![next_bigger.clone()]);
            }

            return Err(Error::InsufficientFunds);
        }

        Ok(selected_proofs)
    }

    /// Select proofs to send
    #[instrument(skip_all)]
    pub async fn select_proofs_to_swap(
        &self,
        amount: Amount,
        proofs: Proofs,
    ) -> Result<Proofs, Error> {
        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        let (mut active_proofs, mut inactive_proofs): (Proofs, Proofs) = proofs
            .into_iter()
            .partition(|p| p.keyset_id == active_keyset_id);

        let mut selected_proofs: Proofs = Vec::new();
        inactive_proofs.sort_by(|a: &Proof, b: &Proof| b.cmp(a));

        for inactive_proof in inactive_proofs {
            selected_proofs.push(inactive_proof);
            let selected_total = selected_proofs.iter().map(|p| p.amount).sum::<Amount>();
            let fees = self.get_proofs_fee(&selected_proofs).await?;

            if selected_total >= amount + fees {
                return Ok(selected_proofs);
            }
        }

        active_proofs.sort_by(|a: &Proof, b: &Proof| b.cmp(a));

        for active_proof in active_proofs {
            selected_proofs.push(active_proof);
            let selected_total = selected_proofs.iter().map(|p| p.amount).sum::<Amount>();
            let fees = self.get_proofs_fee(&selected_proofs).await?;

            if selected_total >= amount + fees {
                return Ok(selected_proofs);
            }
        }

        Err(Error::InsufficientFunds)
    }

    /// Receive proofs
    #[instrument(skip_all)]
    pub async fn receive_proofs(
        &self,
        proofs: Proofs,
        amount_split_target: SplitTarget,
        p2pk_signing_keys: &[SecretKey],
        preimages: &[String],
    ) -> Result<Amount, Error> {
        let mut received_proofs: HashMap<UncheckedUrl, Proofs> = HashMap::new();
        let mint_url = &self.mint_url;
        // Add mint if it does not exist in the store
        if self
            .localstore
            .get_mint(self.mint_url.clone())
            .await?
            .is_none()
        {
            tracing::debug!(
                "Mint not in localstore fetching info for: {}",
                self.mint_url
            );
            self.get_mint_info().await?;
        }

        let _ = self.get_active_mint_keyset().await?;

        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        let keys = self.get_keyset_keys(active_keyset_id).await?;

        let mut proofs = proofs;

        let mut sig_flag = SigFlag::SigInputs;

        // Map hash of preimage to preimage
        let hashed_to_preimage: HashMap<String, &String> = preimages
            .iter()
            .flat_map(|p| match hex::decode(p) {
                Ok(hex_bytes) => Some((Sha256Hash::hash(&hex_bytes).to_string(), p)),
                Err(_) => None,
            })
            .collect();

        let p2pk_signing_keys: HashMap<XOnlyPublicKey, &SecretKey> = p2pk_signing_keys
            .iter()
            .map(|s| (s.x_only_public_key(&SECP256K1).0, s))
            .collect();

        for proof in &mut proofs {
            // Verify that proof DLEQ is valid
            if proof.dleq.is_some() {
                let keys = self.get_keyset_keys(proof.keyset_id).await?;
                let key = keys.amount_key(proof.amount).ok_or(Error::UnknownKey)?;
                proof.verify_dleq(key)?;
            }

            if let Ok(secret) =
                <crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(
                    proof.secret.clone(),
                )
            {
                let conditions: Result<Conditions, _> =
                    secret.secret_data.tags.unwrap_or_default().try_into();
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
                        if let Some(signing) = p2pk_signing_keys.get(&pubkey.x_only_public_key()) {
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
            .create_swap(None, amount_split_target, proofs, None, false)
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
            .post_swap(mint_url.clone().try_into()?, pre_swap.swap_request)
            .await?;

        // Proof to keep
        let p = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &keys,
        )?;
        let mint_proofs = received_proofs.entry(mint_url.clone()).or_default();

        self.localstore
            .increment_keyset_counter(&active_keyset_id, p.len() as u32)
            .await?;

        mint_proofs.extend(p);

        let mut total_amount = Amount::ZERO;
        for (mint, proofs) in received_proofs {
            total_amount += proofs.iter().map(|p| p.amount).sum();
            let proofs = proofs
                .into_iter()
                .flat_map(|proof| ProofInfo::new(proof, mint.clone(), State::Unspent, self.unit))
                .collect();
            self.localstore.add_proofs(proofs).await?;
        }

        Ok(total_amount)
    }

    /// Receive
    #[instrument(skip_all)]
    pub async fn receive(
        &self,
        encoded_token: &str,
        amount_split_target: SplitTarget,
        p2pk_signing_keys: &[SecretKey],
        preimages: &[String],
    ) -> Result<Amount, Error> {
        let token_data = Token::from_str(encoded_token)?;

        let unit = token_data.unit().unwrap_or_default();

        if unit != self.unit {
            return Err(Error::UnitNotSupported);
        }

        let proofs = token_data.proofs();
        if proofs.len() != 1 {
            return Err(Error::MultiMintTokenNotSupported);
        }

        let (mint_url, proofs) = proofs.into_iter().next().expect("Token has proofs");

        if self.mint_url != mint_url {
            return Err(Error::IncorrectMint);
        }

        let amount = self
            .receive_proofs(proofs, amount_split_target, p2pk_signing_keys, preimages)
            .await?;

        Ok(amount)
    }

    /// Restore
    #[instrument(skip(self))]
    pub async fn restore(&self) -> Result<Amount, Error> {
        // Check that mint is in store of mints
        if self
            .localstore
            .get_mint(self.mint_url.clone())
            .await?
            .is_none()
        {
            self.get_mint_info().await?;
        }

        let keysets = self.get_mint_keysets().await?;

        let mut restored_value = Amount::ZERO;

        for keyset in keysets {
            let keys = self.get_keyset_keys(keyset.id).await?;
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
                    self.mint_url,
                    keyset.id
                );

                let restore_request = RestoreRequest {
                    outputs: premint_secrets.blinded_messages(),
                };

                let response = self
                    .client
                    .post_restore(self.mint_url.clone().try_into()?, restore_request)
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

                let states = self.check_proofs_spent(proofs.clone()).await?;

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
                        ProofInfo::new(proof, self.mint_url.clone(), State::Unspent, keyset.unit)
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

                match conditions {
                    Some(conditions) => {
                        pubkeys.extend(conditions.pubkeys.unwrap_or_default());

                        (
                            conditions.refund_keys,
                            Some(pubkeys),
                            conditions.locktime,
                            conditions.num_sigs,
                        )
                    }
                    None => (None, Some(pubkeys), None, None),
                }
            }
            SpendingConditions::HTLCConditions {
                conditions,
                data: _,
            } => match conditions {
                Some(conditions) => (
                    conditions.refund_keys,
                    conditions.pubkeys,
                    conditions.locktime,
                    conditions.num_sigs,
                ),
                None => (None, None, None, None),
            },
        };

        if refund_keys.is_some() && locktime.is_none() {
            tracing::warn!(
                "Invalid spending conditions set: Locktime must be set if refund keys are allowed"
            );
            return Err(Error::InvalidSpendConditions(
                "Must set locktime".to_string(),
            ));
        }

        for (mint_url, proofs) in &token.proofs() {
            if mint_url != &self.mint_url {
                return Err(Error::IncorrectWallet(format!(
                    "Should be {} not {}",
                    self.mint_url, mint_url
                )));
            }
            for proof in proofs {
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

        for (mint_url, proofs) in &token.proofs() {
            if mint_url != &self.mint_url {
                return Err(Error::IncorrectWallet(format!(
                    "Should be {} not {}",
                    self.mint_url, mint_url
                )));
            }
            for proof in proofs {
                let mint_pubkey = match keys_cache.get(&proof.keyset_id) {
                    Some(keys) => keys.amount_key(proof.amount),
                    None => {
                        let keys = self.get_keyset_keys(proof.keyset_id).await?;

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
