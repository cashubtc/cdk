//! Wallet in memory database

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::{Error, WalletDatabase};
use tokio::sync::RwLock;

use crate::mint_url::MintUrl;
use crate::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use crate::types::ProofInfo;
use crate::util::unix_time;
use crate::wallet;
use crate::wallet::types::MintQuote;

/// Wallet in Memory Database
#[derive(Debug, Clone, Default)]
pub struct WalletMemoryDatabase {
    mints: Arc<RwLock<HashMap<MintUrl, Option<MintInfo>>>>,
    mint_keysets: Arc<RwLock<HashMap<MintUrl, HashSet<Id>>>>,
    keysets: Arc<RwLock<HashMap<Id, KeySetInfo>>>,
    mint_quotes: Arc<RwLock<HashMap<String, MintQuote>>>,
    melt_quotes: Arc<RwLock<HashMap<String, wallet::MeltQuote>>>,
    mint_keys: Arc<RwLock<HashMap<Id, Keys>>>,
    proofs: Arc<RwLock<HashMap<PublicKey, ProofInfo>>>,
    keyset_counter: Arc<RwLock<HashMap<Id, u32>>>,
    nostr_last_checked: Arc<RwLock<HashMap<PublicKey, u32>>>,
}

impl WalletMemoryDatabase {
    /// Create new [`WalletMemoryDatabase`]
    pub fn new(
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<wallet::MeltQuote>,
        mint_keys: Vec<Keys>,
        keyset_counter: HashMap<Id, u32>,
        nostr_last_checked: HashMap<PublicKey, u32>,
    ) -> Self {
        Self {
            mints: Arc::new(RwLock::new(HashMap::new())),
            mint_keysets: Arc::new(RwLock::new(HashMap::new())),
            keysets: Arc::new(RwLock::new(HashMap::new())),
            mint_quotes: Arc::new(RwLock::new(
                mint_quotes.into_iter().map(|q| (q.id.clone(), q)).collect(),
            )),
            melt_quotes: Arc::new(RwLock::new(
                melt_quotes.into_iter().map(|q| (q.id.clone(), q)).collect(),
            )),
            mint_keys: Arc::new(RwLock::new(
                mint_keys.into_iter().map(|k| (Id::from(&k), k)).collect(),
            )),
            proofs: Arc::new(RwLock::new(HashMap::new())),
            keyset_counter: Arc::new(RwLock::new(keyset_counter)),
            nostr_last_checked: Arc::new(RwLock::new(nostr_last_checked)),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WalletDatabase for WalletMemoryDatabase {
    type Err = Error;

    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
        self.mints.write().await.insert(mint_url, mint_info);
        Ok(())
    }

    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), Self::Err> {
        let mut mints = self.mints.write().await;
        mints.remove(&mint_url);

        Ok(())
    }

    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Self::Err> {
        Ok(self.mints.read().await.get(&mint_url).cloned().flatten())
    }

    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Error> {
        Ok(self.mints.read().await.clone())
    }

    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Self::Err> {
        let proofs = self
            .get_proofs(Some(old_mint_url), None, None, None)
            .await?;

        // Update proofs
        {
            let updated_proofs: Vec<ProofInfo> = proofs
                .into_iter()
                .map(|mut p| {
                    p.mint_url = new_mint_url.clone();
                    p
                })
                .collect();

            self.update_proofs(updated_proofs, vec![]).await?;
        }

        // Update mint quotes
        {
            let quotes = self.get_mint_quotes().await?;

            let unix_time = unix_time();

            let quotes: Vec<MintQuote> = quotes
                .into_iter()
                .filter_map(|mut q| {
                    if q.expiry < unix_time {
                        q.mint_url = new_mint_url.clone();
                        Some(q)
                    } else {
                        None
                    }
                })
                .collect();

            for quote in quotes {
                self.add_mint_quote(quote).await?;
            }
        }

        Ok(())
    }

    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error> {
        let mut current_mint_keysets = self.mint_keysets.write().await;
        let mut current_keysets = self.keysets.write().await;

        for keyset in keysets {
            current_mint_keysets
                .entry(mint_url.clone())
                .and_modify(|ks| {
                    ks.insert(keyset.id);
                })
                .or_insert(HashSet::from_iter(vec![keyset.id]));

            current_keysets.insert(keyset.id, keyset);
        }

        Ok(())
    }

    async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<Option<Vec<KeySetInfo>>, Error> {
        match self.mint_keysets.read().await.get(&mint_url) {
            Some(keyset_ids) => {
                let mut keysets = vec![];

                let db_keysets = self.keysets.read().await;

                for id in keyset_ids {
                    if let Some(keyset) = db_keysets.get(id) {
                        keysets.push(keyset.clone());
                    }
                }

                Ok(Some(keysets))
            }
            None => Ok(None),
        }
    }

    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Error> {
        Ok(self.keysets.read().await.get(keyset_id).cloned())
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        self.mint_quotes
            .write()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Error> {
        Ok(self.mint_quotes.read().await.get(quote_id).cloned())
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let quotes = self.mint_quotes.read().await;
        Ok(quotes.values().cloned().collect())
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.mint_quotes.write().await.remove(quote_id);

        Ok(())
    }

    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), Error> {
        self.melt_quotes
            .write()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Error> {
        Ok(self.melt_quotes.read().await.get(quote_id).cloned())
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.melt_quotes.write().await.remove(quote_id);

        Ok(())
    }

    async fn add_keys(&self, keys: Keys) -> Result<(), Error> {
        self.mint_keys.write().await.insert(Id::from(&keys), keys);
        Ok(())
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Error> {
        Ok(self.mint_keys.read().await.get(id).cloned())
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), Error> {
        self.mint_keys.write().await.remove(id);
        Ok(())
    }

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Error> {
        let mut all_proofs = self.proofs.write().await;

        for proof_info in added.into_iter() {
            all_proofs.insert(proof_info.y, proof_info);
        }

        for y in removed_ys.into_iter() {
            all_proofs.remove(&y);
        }

        Ok(())
    }

    async fn set_pending_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Error> {
        let mut all_proofs = self.proofs.write().await;

        for y in ys.into_iter() {
            if let Some(proof_info) = all_proofs.get_mut(&y) {
                proof_info.state = State::Pending;
            }
        }

        Ok(())
    }

    async fn reserve_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Error> {
        let mut all_proofs = self.proofs.write().await;

        for y in ys.into_iter() {
            if let Some(proof_info) = all_proofs.get_mut(&y) {
                proof_info.state = State::Reserved;
            }
        }

        Ok(())
    }

    async fn set_unspent_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Error> {
        let mut all_proofs = self.proofs.write().await;

        for y in ys.into_iter() {
            if let Some(proof_info) = all_proofs.get_mut(&y) {
                proof_info.state = State::Unspent;
            }
        }

        Ok(())
    }

    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Error> {
        let proofs = self.proofs.read().await;

        let proofs: Vec<ProofInfo> = proofs
            .clone()
            .into_values()
            .filter_map(|proof_info| {
                match proof_info.matches_conditions(&mint_url, &unit, &state, &spending_conditions)
                {
                    true => Some(proof_info),
                    false => None,
                }
            })
            .collect();

        Ok(proofs)
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Error> {
        let keyset_counter = self.keyset_counter.read().await;
        let current_counter = keyset_counter.get(keyset_id).cloned().unwrap_or(0);
        drop(keyset_counter);

        self.keyset_counter
            .write()
            .await
            .insert(*keyset_id, current_counter + count);
        Ok(())
    }

    async fn get_keyset_counter(&self, id: &Id) -> Result<Option<u32>, Error> {
        Ok(self.keyset_counter.read().await.get(id).cloned())
    }

    async fn get_nostr_last_checked(
        &self,
        verifying_key: &PublicKey,
    ) -> Result<Option<u32>, Self::Err> {
        Ok(self
            .nostr_last_checked
            .read()
            .await
            .get(verifying_key)
            .cloned())
    }
    async fn add_nostr_last_checked(
        &self,
        verifying_key: PublicKey,
        last_checked: u32,
    ) -> Result<(), Self::Err> {
        self.nostr_last_checked
            .write()
            .await
            .insert(verifying_key, last_checked);

        Ok(())
    }
}
