//! Memory Database

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::WalletDatabase;
use crate::cdk_database::Error;
use crate::nuts::{Id, KeySetInfo, Keys, MintInfo, Proof, Proofs};
use crate::types::{MeltQuote, MintQuote};
use crate::url::UncheckedUrl;

#[derive(Default, Debug, Clone)]
pub struct WalletMemoryDatabase {
    mints: Arc<Mutex<HashMap<UncheckedUrl, Option<MintInfo>>>>,
    mint_keysets: Arc<Mutex<HashMap<UncheckedUrl, HashSet<KeySetInfo>>>>,
    mint_quotes: Arc<Mutex<HashMap<String, MintQuote>>>,
    melt_quotes: Arc<Mutex<HashMap<String, MeltQuote>>>,
    mint_keys: Arc<Mutex<HashMap<Id, Keys>>>,
    proofs: Arc<Mutex<HashMap<UncheckedUrl, HashSet<Proof>>>>,
    pending_proofs: Arc<Mutex<HashMap<UncheckedUrl, HashSet<Proof>>>>,
    keyset_counter: Arc<Mutex<HashMap<Id, u64>>>,
}

impl WalletMemoryDatabase {
    pub fn new(
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        mint_keys: Vec<Keys>,
        keyset_counter: HashMap<Id, u64>,
    ) -> Self {
        Self {
            mints: Arc::new(Mutex::new(HashMap::new())),
            mint_keysets: Arc::new(Mutex::new(HashMap::new())),
            mint_quotes: Arc::new(Mutex::new(
                mint_quotes.into_iter().map(|q| (q.id.clone(), q)).collect(),
            )),
            melt_quotes: Arc::new(Mutex::new(
                melt_quotes.into_iter().map(|q| (q.id.clone(), q)).collect(),
            )),
            mint_keys: Arc::new(Mutex::new(
                mint_keys.into_iter().map(|k| (Id::from(&k), k)).collect(),
            )),
            proofs: Arc::new(Mutex::new(HashMap::new())),
            pending_proofs: Arc::new(Mutex::new(HashMap::new())),
            keyset_counter: Arc::new(Mutex::new(keyset_counter)),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WalletDatabase for WalletMemoryDatabase {
    type Err = Error;

    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
        self.mints.lock().await.insert(mint_url, mint_info);
        Ok(())
    }

    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Self::Err> {
        Ok(self.mints.lock().await.get(&mint_url).cloned().flatten())
    }

    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Error> {
        Ok(self.mints.lock().await.clone())
    }

    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error> {
        let mut current_keysets = self.mint_keysets.lock().await;

        let mint_keysets = current_keysets.entry(mint_url).or_insert(HashSet::new());
        mint_keysets.extend(keysets);

        Ok(())
    }

    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Error> {
        Ok(self
            .mint_keysets
            .lock()
            .await
            .get(&mint_url)
            .map(|ks| ks.iter().cloned().collect()))
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        self.mint_quotes
            .lock()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Error> {
        Ok(self.mint_quotes.lock().await.get(quote_id).cloned())
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.mint_quotes.lock().await.remove(quote_id);

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Error> {
        self.melt_quotes
            .lock()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Error> {
        Ok(self.melt_quotes.lock().await.get(quote_id).cloned())
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.melt_quotes.lock().await.remove(quote_id);

        Ok(())
    }

    async fn add_keys(&self, keys: Keys) -> Result<(), Error> {
        self.mint_keys.lock().await.insert(Id::from(&keys), keys);
        Ok(())
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Error> {
        Ok(self.mint_keys.lock().await.get(id).cloned())
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), Error> {
        self.mint_keys.lock().await.remove(id);
        Ok(())
    }

    async fn add_proofs(&self, mint_url: UncheckedUrl, proofs: Proofs) -> Result<(), Error> {
        let mut all_proofs = self.proofs.lock().await;

        let mint_proofs = all_proofs.entry(mint_url).or_insert(HashSet::new());
        mint_proofs.extend(proofs);

        Ok(())
    }

    async fn get_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Error> {
        Ok(self
            .proofs
            .lock()
            .await
            .get(&mint_url)
            .map(|p| p.iter().cloned().collect()))
    }

    async fn remove_proofs(&self, mint_url: UncheckedUrl, proofs: &Proofs) -> Result<(), Error> {
        let mut mint_proofs = self.proofs.lock().await;

        if let Some(mint_proofs) = mint_proofs.get_mut(&mint_url) {
            for proof in proofs {
                mint_proofs.remove(proof);
            }
        }

        Ok(())
    }

    async fn add_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: Proofs,
    ) -> Result<(), Error> {
        let mut all_proofs = self.pending_proofs.lock().await;

        let mint_proofs = all_proofs.entry(mint_url).or_insert(HashSet::new());
        mint_proofs.extend(proofs);

        Ok(())
    }

    async fn get_pending_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Error> {
        Ok(self
            .pending_proofs
            .lock()
            .await
            .get(&mint_url)
            .map(|p| p.iter().cloned().collect()))
    }

    async fn remove_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: &Proofs,
    ) -> Result<(), Error> {
        let mut mint_proofs = self.pending_proofs.lock().await;

        if let Some(mint_proofs) = mint_proofs.get_mut(&mint_url) {
            for proof in proofs {
                mint_proofs.remove(proof);
            }
        }

        Ok(())
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u64) -> Result<(), Error> {
        let keyset_counter = self.keyset_counter.lock().await;
        let current_counter = keyset_counter.get(keyset_id).unwrap_or(&0);
        self.keyset_counter
            .lock()
            .await
            .insert(*keyset_id, current_counter + count);
        Ok(())
    }

    async fn get_keyset_counter(&self, id: &Id) -> Result<Option<u64>, Error> {
        Ok(self.keyset_counter.lock().await.get(id).cloned())
    }
}
