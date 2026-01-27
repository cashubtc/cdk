#![doc = include_str!("./README.md")]

use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::amount::FeeAndAmounts;
use cdk_common::database::{self, WalletDatabase};
use cdk_common::parking_lot::RwLock;
use cdk_common::subscription::WalletParams;
use getrandom::getrandom;
use subscription::{ActiveSubscription, SubscriptionManager};
#[cfg(any(feature = "auth", feature = "npubcash"))]
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
    nut10, CurrencyUnit, Id, Keys, MintInfo, MintQuoteState, PreMintSecrets, Proofs,
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
#[cfg(feature = "nostr")]
mod nostr_backup;
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
#[cfg(feature = "npubcash")]
mod npubcash;
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
pub use multi_mint_wallet::{WalletConfig, WalletRepository};

#[cfg(feature = "nostr")]
pub use nostr_backup::{BackupOptions, BackupResult, RestoreOptions, RestoreResult};
pub use payment_request::CreateRequestParams;
#[cfg(feature = "nostr")]
pub use payment_request::NostrWaitInfo;
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
    pub localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    /// Mint metadata cache for this mint (lock-free cached access to keys, keysets, and mint info)
    pub metadata_cache: Arc<MintMetadataCache>,
    /// The targeted amount of proofs to have at each size
    pub target_proof_count: usize,
    metadata_cache_ttl: Arc<RwLock<Option<Duration>>>,
    #[cfg(feature = "auth")]
    auth_wallet: Arc<TokioRwLock<Option<AuthWallet>>>,
    #[cfg(feature = "npubcash")]
    npubcash_client: Arc<TokioRwLock<Option<Arc<cdk_npubcash::NpubCashClient>>>>,
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

/// Amount that are recovered during restore operation
#[derive(Debug, Hash, PartialEq, Eq, Default)]
pub struct Restored {
    /// Amount in the restore that has already been spent
    pub spent: Amount,
    /// Amount restored that is unspent
    pub unspent: Amount,
    /// Amount restored that is pending
    pub pending: Amount,
}

impl Wallet {
    /// Create new [`Wallet`] using the builder pattern
    /// # Synopsis
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use bitcoin::bip32::Xpriv;
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
        localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
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
    pub async fn subscribe<T: Into<WalletParams>>(
        &self,
        query: T,
    ) -> Result<ActiveSubscription, Error> {
        self.subscription
            .subscribe(self.mint_url.clone(), query.into())
            .map_err(|e| Error::SubscriptionError(e.to_string()))
    }

    /// Fee required to redeem proof set
    #[instrument(skip_all)]
    pub async fn get_proofs_fee(
        &self,
        proofs: &Proofs,
    ) -> Result<crate::fees::ProofsFeeBreakdown, Error> {
        let proofs_per_keyset = proofs.count_by_keyset();
        self.get_proofs_fee_by_count(proofs_per_keyset).await
    }

    /// Fee required to redeem proof set by count
    pub async fn get_proofs_fee_by_count(
        &self,
        proofs_per_keyset: HashMap<Id, u64>,
    ) -> Result<crate::fees::ProofsFeeBreakdown, Error> {
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

        let fee_breakdown = calculate_fee(&proofs_per_keyset, &fee_per_keyset)?;

        Ok(fee_breakdown)
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
        self.localstore
            .update_mint_url(self.mint_url.clone(), new_mint_url.clone())
            .await?;

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

                    self.client
                        .set_auth_wallet(Some(new_auth_wallet.clone()))
                        .await;

                    if let Err(e) = new_auth_wallet.refresh_keysets().await {
                        tracing::error!("Could not fetch auth keysets: {}", e);
                    }
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
    #[instrument(skip(self))]
    pub(crate) async fn amounts_needed_for_state_target(
        &self,
        fee_and_amounts: &FeeAndAmounts,
    ) -> Result<Vec<Amount>, Error> {
        let unspent_proofs = self
            .get_proofs_with(Some(vec![State::Unspent]), None)
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
    #[instrument(skip(self))]
    async fn determine_split_target_values(
        &self,
        change_amount: Amount,
        fee_and_amounts: &FeeAndAmounts,
    ) -> Result<SplitTarget, Error> {
        let mut amounts_needed_refill = self
            .amounts_needed_for_state_target(fee_and_amounts)
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
    pub async fn restore(&self) -> Result<Restored, Error> {
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

        let mut restored_result = Restored::default();

        for keyset in keysets {
            let keys = self.load_keyset_keys(keyset.id).await?;
            let mut empty_batch = 0;
            let mut start_counter = 0;
            // Track the highest counter value that had a signature
            let mut highest_counter: Option<u32> = None;

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

                // Build a map from blinded_secret to signature for O(1) lookup
                // This ensures we match signatures to secrets correctly regardless of response order
                let signature_map: HashMap<_, _> = response
                    .outputs
                    .iter()
                    .zip(response.signatures.iter())
                    .map(|(output, sig)| (output.blinded_secret, sig.clone()))
                    .collect();

                // Enumerate secrets to track their original index (which corresponds to counter value)
                // and match signatures by blinded_secret to ensure correct pairing
                let matched_secrets: Vec<_> = premint_secrets
                    .secrets
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, p)| {
                        signature_map
                            .get(&p.blinded_message.blinded_secret)
                            .map(|sig| (idx, p, sig.clone()))
                    })
                    .collect();

                // Update highest counter based on matched indices
                if let Some(&(max_idx, _, _)) = matched_secrets.last() {
                    let counter_value = start_counter + max_idx as u32;
                    highest_counter =
                        Some(highest_counter.map_or(counter_value, |c| c.max(counter_value)));
                }

                // the response outputs and premint secrets should be the same after filtering
                // blinded messages the mint did not have signatures for
                if response.outputs.len() != matched_secrets.len() {
                    return Err(Error::InvalidMintResponse(format!(
                        "restore response outputs ({}) does not match premint secrets ({})",
                        response.outputs.len(),
                        matched_secrets.len()
                    )));
                }

                // Extract signatures, rs, and secrets in matching order
                // Each tuple (idx, premint, signature) ensures correct pairing
                let proofs = construct_proofs(
                    matched_secrets
                        .iter()
                        .map(|(_, _, sig)| sig.clone())
                        .collect(),
                    matched_secrets
                        .iter()
                        .map(|(_, p, _)| p.r.clone())
                        .collect(),
                    matched_secrets
                        .iter()
                        .map(|(_, p, _)| p.secret.clone())
                        .collect(),
                    &keys,
                )?;

                tracing::debug!("Restored {} proofs", proofs.len());

                let states = self.check_proofs_spent(proofs.clone()).await?;

                let (unspent_proofs, updated_restored) = proofs
                    .into_iter()
                    .zip(states)
                    .filter_map(|(p, state)| {
                        ProofInfo::new(p, self.mint_url.clone(), state.state, keyset.unit.clone())
                            .ok()
                    })
                    .try_fold(
                        (Vec::new(), restored_result),
                        |(mut proofs, mut restored_result), proof_info| {
                            match proof_info.state {
                                State::Spent => {
                                    restored_result.spent += proof_info.proof.amount;
                                }
                                State::Unspent =>  {
                                    restored_result.unspent += proof_info.proof.amount;
                                    proofs.push(proof_info);
                                }
                                State::Pending => {
                                    restored_result.pending += proof_info.proof.amount;
                                    proofs.push(proof_info);
                                }
                                _ => {
                                    unreachable!("These states are unknown to the mint and cannot be returned")
                                }
                            }
                            Ok::<(Vec<ProofInfo>, Restored), Error>((proofs, restored_result))
                        },
                    )?;

                restored_result = updated_restored;

                self.localstore
                    .update_proofs(unspent_proofs, vec![])
                    .await?;

                empty_batch = 0;
                start_counter += 100;
            }

            // Set counter to highest found + 1 to avoid reusing any counter values
            // that already have signatures at the mint
            if let Some(highest) = highest_counter {
                self.localstore
                    .increment_keyset_counter(&keyset.id, highest + 1)
                    .await?;
                tracing::debug!(
                    "Set keyset {} counter to {} after restore",
                    keyset.id,
                    highest + 1
                );
            }
        }
        Ok(restored_result)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nuts::{BlindSignature, BlindedMessage, PreMint, PreMintSecrets};
    use crate::secret::Secret;

    /// Test that restore signature matching works correctly when response is in order
    #[test]
    fn test_restore_signature_matching_in_order() {
        // Create test data with 3 premint secrets
        let keyset_id = Id::from_bytes(&[0u8; 8]).unwrap();

        // Generate deterministic keys for testing
        let secret1 = Secret::generate();
        let secret2 = Secret::generate();
        let secret3 = Secret::generate();

        let (blinded1, r1) = crate::dhke::blind_message(&secret1.to_bytes(), None).unwrap();
        let (blinded2, r2) = crate::dhke::blind_message(&secret2.to_bytes(), None).unwrap();
        let (blinded3, r3) = crate::dhke::blind_message(&secret3.to_bytes(), None).unwrap();

        let premint1 = PreMint {
            blinded_message: BlindedMessage::new(Amount::from(1), keyset_id, blinded1),
            secret: secret1.clone(),
            r: r1.clone(),
            amount: Amount::from(1),
        };
        let premint2 = PreMint {
            blinded_message: BlindedMessage::new(Amount::from(2), keyset_id, blinded2),
            secret: secret2.clone(),
            r: r2.clone(),
            amount: Amount::from(2),
        };
        let premint3 = PreMint {
            blinded_message: BlindedMessage::new(Amount::from(4), keyset_id, blinded3),
            secret: secret3.clone(),
            r: r3.clone(),
            amount: Amount::from(4),
        };

        let premint_secrets = PreMintSecrets {
            secrets: vec![premint1.clone(), premint2.clone(), premint3.clone()],
            keyset_id,
        };

        // Create mock signatures (just need the structure, not real signatures)
        let sig1 = BlindSignature {
            amount: Amount::from(1),
            keyset_id,
            c: blinded1, // Using blinded as placeholder for signature
            dleq: None,
        };
        let sig2 = BlindSignature {
            amount: Amount::from(2),
            keyset_id,
            c: blinded2,
            dleq: None,
        };
        let sig3 = BlindSignature {
            amount: Amount::from(4),
            keyset_id,
            c: blinded3,
            dleq: None,
        };

        // Response in same order as request
        let response_outputs = vec![
            premint1.blinded_message.clone(),
            premint2.blinded_message.clone(),
            premint3.blinded_message.clone(),
        ];
        let response_signatures = vec![sig1.clone(), sig2.clone(), sig3.clone()];

        // Apply the matching logic (same as in restore)
        let signature_map: HashMap<_, _> = response_outputs
            .iter()
            .zip(response_signatures.iter())
            .map(|(output, sig)| (output.blinded_secret, sig.clone()))
            .collect();

        let matched_secrets: Vec<_> = premint_secrets
            .secrets
            .iter()
            .enumerate()
            .filter_map(|(idx, p)| {
                signature_map
                    .get(&p.blinded_message.blinded_secret)
                    .map(|sig| (idx, p, sig.clone()))
            })
            .collect();

        // Verify all 3 matched
        assert_eq!(matched_secrets.len(), 3);

        // Verify correct pairing by checking amounts match
        assert_eq!(matched_secrets[0].2.amount, Amount::from(1));
        assert_eq!(matched_secrets[1].2.amount, Amount::from(2));
        assert_eq!(matched_secrets[2].2.amount, Amount::from(4));

        // Verify indices are preserved
        assert_eq!(matched_secrets[0].0, 0);
        assert_eq!(matched_secrets[1].0, 1);
        assert_eq!(matched_secrets[2].0, 2);
    }

    /// Test that restore signature matching works correctly when response is OUT of order
    /// This is the critical test that verifies the fix for TokenNotVerified
    #[test]
    fn test_restore_signature_matching_out_of_order() {
        let keyset_id = Id::from_bytes(&[0u8; 8]).unwrap();

        let secret1 = Secret::generate();
        let secret2 = Secret::generate();
        let secret3 = Secret::generate();

        let (blinded1, r1) = crate::dhke::blind_message(&secret1.to_bytes(), None).unwrap();
        let (blinded2, r2) = crate::dhke::blind_message(&secret2.to_bytes(), None).unwrap();
        let (blinded3, r3) = crate::dhke::blind_message(&secret3.to_bytes(), None).unwrap();

        let premint1 = PreMint {
            blinded_message: BlindedMessage::new(Amount::from(1), keyset_id, blinded1),
            secret: secret1.clone(),
            r: r1.clone(),
            amount: Amount::from(1),
        };
        let premint2 = PreMint {
            blinded_message: BlindedMessage::new(Amount::from(2), keyset_id, blinded2),
            secret: secret2.clone(),
            r: r2.clone(),
            amount: Amount::from(2),
        };
        let premint3 = PreMint {
            blinded_message: BlindedMessage::new(Amount::from(4), keyset_id, blinded3),
            secret: secret3.clone(),
            r: r3.clone(),
            amount: Amount::from(4),
        };

        let premint_secrets = PreMintSecrets {
            secrets: vec![premint1.clone(), premint2.clone(), premint3.clone()],
            keyset_id,
        };

        let sig1 = BlindSignature {
            amount: Amount::from(1),
            keyset_id,
            c: blinded1,
            dleq: None,
        };
        let sig2 = BlindSignature {
            amount: Amount::from(2),
            keyset_id,
            c: blinded2,
            dleq: None,
        };
        let sig3 = BlindSignature {
            amount: Amount::from(4),
            keyset_id,
            c: blinded3,
            dleq: None,
        };

        // Response in REVERSED order (simulating out-of-order response from mint)
        let response_outputs = vec![
            premint3.blinded_message.clone(), // index 2 first
            premint1.blinded_message.clone(), // index 0 second
            premint2.blinded_message.clone(), // index 1 third
        ];
        let response_signatures = vec![sig3.clone(), sig1.clone(), sig2.clone()];

        // Apply the matching logic (same as in restore)
        let signature_map: HashMap<_, _> = response_outputs
            .iter()
            .zip(response_signatures.iter())
            .map(|(output, sig)| (output.blinded_secret, sig.clone()))
            .collect();

        let matched_secrets: Vec<_> = premint_secrets
            .secrets
            .iter()
            .enumerate()
            .filter_map(|(idx, p)| {
                signature_map
                    .get(&p.blinded_message.blinded_secret)
                    .map(|sig| (idx, p, sig.clone()))
            })
            .collect();

        // Verify all 3 matched
        assert_eq!(matched_secrets.len(), 3);

        // Critical: Even though response was out of order, signatures should be
        // correctly paired with their corresponding premint secrets
        // matched_secrets should be in premint order (0, 1, 2) with correct signatures
        assert_eq!(matched_secrets[0].0, 0); // First premint (amount 1)
        assert_eq!(matched_secrets[0].2.amount, Amount::from(1)); // Correct signature

        assert_eq!(matched_secrets[1].0, 1); // Second premint (amount 2)
        assert_eq!(matched_secrets[1].2.amount, Amount::from(2)); // Correct signature

        assert_eq!(matched_secrets[2].0, 2); // Third premint (amount 4)
        assert_eq!(matched_secrets[2].2.amount, Amount::from(4)); // Correct signature
    }

    /// Test that restore handles partial responses correctly
    #[test]
    fn test_restore_signature_matching_partial_response() {
        let keyset_id = Id::from_bytes(&[0u8; 8]).unwrap();

        let secret1 = Secret::generate();
        let secret2 = Secret::generate();
        let secret3 = Secret::generate();

        let (blinded1, r1) = crate::dhke::blind_message(&secret1.to_bytes(), None).unwrap();
        let (blinded2, r2) = crate::dhke::blind_message(&secret2.to_bytes(), None).unwrap();
        let (blinded3, r3) = crate::dhke::blind_message(&secret3.to_bytes(), None).unwrap();

        let premint1 = PreMint {
            blinded_message: BlindedMessage::new(Amount::from(1), keyset_id, blinded1),
            secret: secret1.clone(),
            r: r1.clone(),
            amount: Amount::from(1),
        };
        let premint2 = PreMint {
            blinded_message: BlindedMessage::new(Amount::from(2), keyset_id, blinded2),
            secret: secret2.clone(),
            r: r2.clone(),
            amount: Amount::from(2),
        };
        let premint3 = PreMint {
            blinded_message: BlindedMessage::new(Amount::from(4), keyset_id, blinded3),
            secret: secret3.clone(),
            r: r3.clone(),
            amount: Amount::from(4),
        };

        let premint_secrets = PreMintSecrets {
            secrets: vec![premint1.clone(), premint2.clone(), premint3.clone()],
            keyset_id,
        };

        let sig1 = BlindSignature {
            amount: Amount::from(1),
            keyset_id,
            c: blinded1,
            dleq: None,
        };
        let sig3 = BlindSignature {
            amount: Amount::from(4),
            keyset_id,
            c: blinded3,
            dleq: None,
        };

        // Response only has signatures for premint1 and premint3 (gap at premint2)
        // Also out of order
        let response_outputs = vec![
            premint3.blinded_message.clone(),
            premint1.blinded_message.clone(),
        ];
        let response_signatures = vec![sig3.clone(), sig1.clone()];

        let signature_map: HashMap<_, _> = response_outputs
            .iter()
            .zip(response_signatures.iter())
            .map(|(output, sig)| (output.blinded_secret, sig.clone()))
            .collect();

        let matched_secrets: Vec<_> = premint_secrets
            .secrets
            .iter()
            .enumerate()
            .filter_map(|(idx, p)| {
                signature_map
                    .get(&p.blinded_message.blinded_secret)
                    .map(|sig| (idx, p, sig.clone()))
            })
            .collect();

        // Only 2 should match
        assert_eq!(matched_secrets.len(), 2);

        // Verify correct pairing despite gap and out-of-order response
        assert_eq!(matched_secrets[0].0, 0); // First premint (amount 1)
        assert_eq!(matched_secrets[0].2.amount, Amount::from(1));

        assert_eq!(matched_secrets[1].0, 2); // Third premint (amount 4), index 1 skipped
        assert_eq!(matched_secrets[1].2.amount, Amount::from(4));
    }
}
