#![doc = include_str!("./README.md")]

use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::amount::FeeAndAmounts;
use cdk_common::database::{self, DynWalletDatabaseTransaction, WalletDatabase};
use cdk_common::parking_lot::RwLock;
use cdk_common::subscription::WalletParams;
use getrandom::getrandom;
use subscription::{ActiveSubscription, SubscriptionManager};
#[cfg(feature = "auth")]
use tokio::sync::RwLock as TokioRwLock;
use tracing::instrument;
use zeroize::Zeroize;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::error::Error;
use crate::fees::calculate_fee;
use crate::mint_url::MintUrl;
use crate::nuts::nut00::token::Token;
use crate::nuts::nut17::Kind;
use crate::nuts::{
    nut10, CurrencyUnit, Id, Keys, MintInfo, MintQuoteState, PreMintSecrets, Proof, Proofs,
    RestoreRequest, SpendingConditions, State,
};
use crate::types::ProofInfo;
use crate::util::unix_time;
use crate::wallet::mint_metadata_cache::MintMetadataCache;
use crate::Amount;
#[cfg(feature = "auth")]
use crate::OidcClient;

#[cfg(feature = "auth")]
mod auth;
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub use mint_connector::TorHttpClient;
mod balance;
mod builder;
mod issue;
mod keysets;
mod melt;
mod mint_connector;
mod mint_metadata_cache;
pub mod multi_mint_wallet;
pub mod payment_request;
mod proofs;
mod receive;
mod reclaim;
mod send;
#[cfg(not(target_arch = "wasm32"))]
mod streams;
pub mod subscription;
mod swap;
mod transactions;
pub mod util;

#[cfg(feature = "auth")]
pub use auth::{AuthMintConnector, AuthWallet};
pub use builder::WalletBuilder;
pub use cdk_common::wallet as types;
#[cfg(feature = "auth")]
pub use mint_connector::http_client::AuthHttpClient as BaseAuthHttpClient;
pub use mint_connector::http_client::HttpClient as BaseHttpClient;
pub use mint_connector::transport::Transport as HttpTransport;
#[cfg(feature = "auth")]
pub use mint_connector::AuthHttpClient;
pub use mint_connector::{HttpClient, LnurlPayInvoiceResponse, LnurlPayResponse, MintConnector};
pub use multi_mint_wallet::{MultiMintReceiveOptions, MultiMintSendOptions, MultiMintWallet};
pub use receive::ReceiveOptions;
pub use send::{PreparedSend, SendMemo, SendOptions};
pub use types::{MeltQuote, MintQuote, SendKind};

use crate::nuts::nut00::ProofsMethods;

/// CDK Wallet
///
/// The CDK [`Wallet`] is a high level cashu wallet.
///
/// A [`Wallet`] is for a single mint and single unit.
#[derive(Debug, Clone)]
pub struct Wallet {
    /// Mint Url
    pub mint_url: MintUrl,
    /// Unit
    pub unit: CurrencyUnit,
    /// Storage backend
    pub localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    /// Mint metadata cache for this mint (lock-free cached access to keys, keysets, and mint info)
    pub metadata_cache: Arc<MintMetadataCache>,
    /// The targeted amount of proofs to have at each size
    pub target_proof_count: usize,
    metadata_cache_ttl: Arc<RwLock<Option<Duration>>>,
    #[cfg(feature = "auth")]
    auth_wallet: Arc<TokioRwLock<Option<AuthWallet>>>,
    seed: [u8; 64],
    client: Arc<dyn MintConnector + Send + Sync>,
    subscription: SubscriptionManager,
    in_error_swap_reverted_proofs: Arc<AtomicBool>,
}

const ALPHANUMERIC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Wallet Subscription filter
#[derive(Debug, Clone)]
pub enum WalletSubscription {
    /// Proof subscription
    ProofState(Vec<String>),
    /// Mint quote subscription
    Bolt11MintQuoteState(Vec<String>),
    /// Melt quote subscription
    Bolt11MeltQuoteState(Vec<String>),
    /// Mint bolt12 quote subscription
    Bolt12MintQuoteState(Vec<String>),
}

impl From<WalletSubscription> for WalletParams {
    fn from(val: WalletSubscription) -> Self {
        let mut buffer = vec![0u8; 10];

        getrandom(&mut buffer).expect("Failed to generate random bytes");

        let id = Arc::new(
            buffer
                .iter()
                .map(|&byte| {
                    let index = byte as usize % ALPHANUMERIC.len(); // 62 alphanumeric characters (A-Z, a-z, 0-9)
                    ALPHANUMERIC[index] as char
                })
                .collect::<String>(),
        );

        match val {
            WalletSubscription::ProofState(filters) => WalletParams {
                filters,
                kind: Kind::ProofState,
                id,
            },
            WalletSubscription::Bolt11MintQuoteState(filters) => WalletParams {
                filters,
                kind: Kind::Bolt11MintQuote,
                id,
            },
            WalletSubscription::Bolt11MeltQuoteState(filters) => WalletParams {
                filters,
                kind: Kind::Bolt11MeltQuote,
                id,
            },
            WalletSubscription::Bolt12MintQuoteState(filters) => WalletParams {
                filters,
                kind: Kind::Bolt12MintQuote,
                id,
            },
        }
    }
}

impl Wallet {
    /// Create new [`Wallet`] using the builder pattern
    /// # Synopsis
    /// ```rust
    /// use bitcoin::bip32::Xpriv;
    /// use std::sync::Arc;
    ///
    /// use cdk::nuts::CurrencyUnit;
    /// use cdk::wallet::{Wallet, WalletBuilder};
    /// use cdk_sqlite::wallet::memory;
    /// use rand::random;
    ///
    /// async fn test() -> anyhow::Result<()> {
    ///     let seed = random::<[u8; 64]>();
    ///     let mint_url = "https://fake.thesimplekid.dev";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = memory::empty().await?;
    ///     let wallet = WalletBuilder::new()
    ///         .mint_url(mint_url.parse().unwrap())
    ///         .unit(unit)
    ///         .localstore(Arc::new(localstore))
    ///         .seed(seed)
    ///         .build();
    ///     Ok(())
    /// }
    /// ```
    pub fn new(
        mint_url: &str,
        unit: CurrencyUnit,
        localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        seed: [u8; 64],
        target_proof_count: Option<usize>,
    ) -> Result<Self, Error> {
        let mint_url = MintUrl::from_str(mint_url)?;

        WalletBuilder::new()
            .mint_url(mint_url)
            .unit(unit)
            .localstore(localstore)
            .seed(seed)
            .target_proof_count(target_proof_count.unwrap_or(3))
            .build()
    }

    /// Subscribe to events
    pub async fn subscribe<T: Into<WalletParams>>(&self, query: T) -> ActiveSubscription {
        self.subscription
            .subscribe(self.mint_url.clone(), query.into())
            .expect("FIXME")
    }

    /// Fee required for proof set
    #[instrument(skip_all)]
    pub async fn get_proofs_fee(&self, proofs: &Proofs) -> Result<Amount, Error> {
        let proofs_per_keyset = proofs.count_by_keyset();
        self.get_proofs_fee_by_count(proofs_per_keyset).await
    }

    /// Fee required for proof set by count
    pub async fn get_proofs_fee_by_count(
        &self,
        proofs_per_keyset: HashMap<Id, u64>,
    ) -> Result<Amount, Error> {
        let mut fee_per_keyset = HashMap::new();
        let metadata = self
            .metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?;

        for keyset_id in proofs_per_keyset.keys() {
            let mint_keyset_info = metadata
                .keysets
                .get(keyset_id)
                .ok_or(Error::UnknownKeySet)?;
            fee_per_keyset.insert(*keyset_id, mint_keyset_info.input_fee_ppk);
        }

        let fee = calculate_fee(&proofs_per_keyset, &fee_per_keyset)?;

        Ok(fee)
    }

    /// Get fee for count of proofs in a keyset
    #[instrument(skip_all)]
    pub async fn get_keyset_count_fee(&self, keyset_id: &Id, count: u64) -> Result<Amount, Error> {
        let input_fee_ppk = self
            .metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?
            .keysets
            .get(keyset_id)
            .ok_or(Error::UnknownKeySet)?
            .input_fee_ppk;

        let fee = (input_fee_ppk * count).div_ceil(1000);

        Ok(Amount::from(fee))
    }

    /// Update Mint information and related entries in the event a mint changes
    /// its URL
    #[instrument(skip(self))]
    pub async fn update_mint_url(&mut self, new_mint_url: MintUrl) -> Result<(), Error> {
        // Update the mint URL in the wallet DB
        let mut tx = self.localstore.begin_db_transaction().await?;
        tx.update_mint_url(self.mint_url.clone(), new_mint_url.clone())
            .await?;
        tx.commit().await?;

        // Update the mint URL in the wallet struct field
        self.mint_url = new_mint_url;

        Ok(())
    }

    /// Query mint for current mint information
    #[instrument(skip(self))]
    pub async fn fetch_mint_info(&self) -> Result<Option<MintInfo>, Error> {
        let mint_info = self
            .metadata_cache
            .load_from_mint(&self.localstore, &self.client)
            .await?
            .mint_info
            .clone();

        // If mint provides time make sure it is accurate
        if let Some(mint_unix_time) = mint_info.time {
            let current_unix_time = unix_time();
            if current_unix_time.abs_diff(mint_unix_time) > 30 {
                tracing::warn!(
                    "Mint time does match wallet time. Mint: {}, Wallet: {}",
                    mint_unix_time,
                    current_unix_time
                );
                return Err(Error::MintTimeExceedsTolerance);
            }
        }

        // Create or update auth wallet
        #[cfg(feature = "auth")]
        {
            let mut auth_wallet = self.auth_wallet.write().await;
            match &*auth_wallet {
                Some(auth_wallet) => {
                    let mut protected_endpoints = auth_wallet.protected_endpoints.write().await;
                    *protected_endpoints = mint_info.protected_endpoints();

                    if let Some(oidc_client) = mint_info
                        .openid_discovery()
                        .map(|url| OidcClient::new(url, None))
                    {
                        auth_wallet.set_oidc_client(Some(oidc_client)).await;
                    }
                }
                None => {
                    tracing::info!("Mint has auth enabled creating auth wallet");

                    let oidc_client = mint_info
                        .openid_discovery()
                        .map(|url| OidcClient::new(url, None));
                    let new_auth_wallet = AuthWallet::new(
                        self.mint_url.clone(),
                        None,
                        self.localstore.clone(),
                        self.metadata_cache.clone(),
                        mint_info.protected_endpoints(),
                        oidc_client,
                    );
                    *auth_wallet = Some(new_auth_wallet.clone());

                    self.client.set_auth_wallet(Some(new_auth_wallet)).await;
                }
            }
        }

        tracing::trace!("Mint info updated for {}", self.mint_url);

        Ok(Some(mint_info))
    }

    /// Load mint info from cache
    ///
    /// This is a helper function that loads the mint info from the metadata cache
    /// using the configured TTL. Unlike `fetch_mint_info()`, this does not make
    /// a network call if the cache is fresh.
    #[instrument(skip(self))]
    pub async fn load_mint_info(&self) -> Result<MintInfo, Error> {
        let mint_info = self
            .metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?
            .mint_info
            .clone();

        Ok(mint_info)
    }

    /// Get amounts needed to refill proof state
    #[instrument(skip(self, tx))]
    pub(crate) async fn amounts_needed_for_state_target(
        &self,
        tx: &mut DynWalletDatabaseTransaction<'_>,
        fee_and_amounts: &FeeAndAmounts,
    ) -> Result<Vec<Amount>, Error> {
        let unspent_proofs = self
            .get_proofs_with(Some(tx), Some(vec![State::Unspent]), None)
            .await?;

        let amounts_count: HashMap<u64, u64> =
            unspent_proofs
                .iter()
                .fold(HashMap::new(), |mut acc, proof| {
                    let amount = proof.amount;
                    let counter = acc.entry(u64::from(amount)).or_insert(0);
                    *counter += 1;
                    acc
                });

        let needed_amounts =
            fee_and_amounts
                .amounts()
                .iter()
                .fold(Vec::new(), |mut acc, amount| {
                    let count_needed = (self.target_proof_count as u64)
                        .saturating_sub(*amounts_count.get(amount).unwrap_or(&0));

                    for _i in 0..count_needed {
                        acc.push(Amount::from(*amount));
                    }

                    acc
                });
        Ok(needed_amounts)
    }

    /// Determine [`SplitTarget`] for amount based on state
    #[instrument(skip(self, tx))]
    async fn determine_split_target_values(
        &self,
        tx: &mut DynWalletDatabaseTransaction<'_>,
        change_amount: Amount,
        fee_and_amounts: &FeeAndAmounts,
    ) -> Result<SplitTarget, Error> {
        let mut amounts_needed_refill = self
            .amounts_needed_for_state_target(tx, fee_and_amounts)
            .await?;

        amounts_needed_refill.sort();

        let mut values = Vec::new();

        for amount in amounts_needed_refill {
            let values_sum = Amount::try_sum(values.clone().into_iter())?;
            if values_sum + amount <= change_amount {
                values.push(amount);
            }
        }

        Ok(SplitTarget::Values(values))
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
            self.fetch_mint_info().await?;
        }

        let keysets = self.load_mint_keysets().await?;

        let mut restored_value = Amount::ZERO;

        for keyset in keysets {
            let keys = self.load_keyset_keys(keyset.id).await?;
            let mut empty_batch = 0;
            let mut start_counter = 0;

            while empty_batch.lt(&3) {
                let premint_secrets = PreMintSecrets::restore_batch(
                    keyset.id,
                    &self.seed,
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

                let response = self.client.post_restore(restore_request).await?;

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

                let mut tx = self.localstore.begin_db_transaction().await?;
                tx.increment_keyset_counter(&keyset.id, proofs.len() as u32)
                    .await?;
                tx.commit().await?;

                let states = self.check_proofs_spent(proofs.clone()).await?;

                let unspent_proofs: Vec<Proof> = proofs
                    .iter()
                    .zip(states)
                    .filter(|(_, state)| !state.state.eq(&State::Spent))
                    .map(|(p, _)| p)
                    .cloned()
                    .collect();

                restored_value += unspent_proofs.total_amount()?;

                let unspent_proofs = unspent_proofs
                    .into_iter()
                    .map(|proof| {
                        ProofInfo::new(
                            proof,
                            self.mint_url.clone(),
                            State::Unspent,
                            keyset.unit.clone(),
                        )
                    })
                    .collect::<Result<Vec<ProofInfo>, _>>()?;

                let mut tx = self.localstore.begin_db_transaction().await?;
                tx.update_proofs(unspent_proofs, vec![]).await?;
                tx.commit().await?;

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
    pub async fn verify_token_p2pk(
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
        if token.mint_url()? != self.mint_url {
            return Err(Error::IncorrectWallet(format!(
                "Should be {} not {}",
                self.mint_url,
                token.mint_url()?
            )));
        }
        // We need the keysets information to properly convert from token proof to proof
        let keysets_info = self.load_mint_keysets().await?;
        let proofs = token.proofs(&keysets_info)?;

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

        Ok(())
    }

    /// Verify all proofs in token have a valid DLEQ proof
    #[instrument(skip(self, token))]
    pub async fn verify_token_dleq(&self, token: &Token) -> Result<(), Error> {
        let mut keys_cache: HashMap<Id, Keys> = HashMap::new();

        // TODO: Get mint url
        // if mint_url != &self.mint_url {
        //     return Err(Error::IncorrectWallet(format!(
        //         "Should be {} not {}",
        //         self.mint_url, mint_url
        //     )));
        // }

        // We need the keysets information to properly convert from token proof to proof
        let keysets_info = self.load_mint_keysets().await?;
        let proofs = token.proofs(&keysets_info)?;
        for proof in proofs {
            let mint_pubkey = match keys_cache.get(&proof.keyset_id) {
                Some(keys) => keys.amount_key(proof.amount),
                None => {
                    let keys = self.load_keyset_keys(proof.keyset_id).await?;

                    let key = keys.amount_key(proof.amount);
                    keys_cache.insert(proof.keyset_id, keys);

                    key
                }
            }
            .ok_or(Error::AmountKey)?;

            proof
                .verify_dleq(mint_pubkey)
                .map_err(|_| Error::CouldNotVerifyDleq)?;
        }

        Ok(())
    }

    /// Set the client (MintConnector) for this wallet
    ///
    /// This allows updating the connector without recreating the wallet.
    pub fn set_client(&mut self, client: Arc<dyn MintConnector + Send + Sync>) {
        self.client = client;
    }

    /// Set the target proof count for this wallet
    ///
    /// This controls how many proofs of each denomination the wallet tries to maintain.
    pub fn set_target_proof_count(&mut self, count: usize) {
        self.target_proof_count = count;
    }
}

impl Drop for Wallet {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
}
