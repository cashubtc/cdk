//! Memory Database

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::WalletDatabase;
use crate::cdk_database::Error;
use crate::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, Proofs, PublicKey, SpendingConditions, State,
};
use crate::types::{MeltQuote, MintQuote, ProofInfo};
use crate::url::UncheckedUrl;

#[derive(Default, Debug, Clone)]
pub struct WalletMemoryDatabase {
    mints: Arc<RwLock<HashMap<UncheckedUrl, Option<MintInfo>>>>,
    mint_keysets: Arc<RwLock<HashMap<UncheckedUrl, HashSet<Id>>>>,
    keysets: Arc<RwLock<HashMap<Id, KeySetInfo>>>,
    mint_quotes: Arc<RwLock<HashMap<String, MintQuote>>>,
    melt_quotes: Arc<RwLock<HashMap<String, MeltQuote>>>,
    mint_keys: Arc<RwLock<HashMap<Id, Keys>>>,
    proofs: Arc<RwLock<HashMap<PublicKey, ProofInfo>>>,
    keyset_counter: Arc<RwLock<HashMap<Id, u32>>>,
    #[cfg(feature = "nostr")]
    nostr_last_checked: Arc<RwLock<HashMap<PublicKey, u32>>>,
}

impl WalletMemoryDatabase {
    pub fn new(
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        mint_keys: Vec<Keys>,
        keyset_counter: HashMap<Id, u32>,
        #[cfg(feature = "nostr")] nostr_last_checked: HashMap<PublicKey, u32>,
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
            #[cfg(feature = "nostr")]
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
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
        self.mints.write().await.insert(mint_url, mint_info);
        Ok(())
    }

    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Self::Err> {
        Ok(self.mints.read().await.get(&mint_url).cloned().flatten())
    }

    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Error> {
        Ok(self.mints.read().await.clone())
    }

    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
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

    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Error> {
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

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Error> {
        self.melt_quotes
            .write()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Error> {
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

    async fn add_proofs(&self, proofs_info: Vec<ProofInfo>) -> Result<(), Error> {
        let mut all_proofs = self.proofs.write().await;

        for proof_info in proofs_info.into_iter() {
            all_proofs.insert(proof_info.y, proof_info);
        }

        Ok(())
    }

    async fn get_proofs(
        &self,
        mint_url: Option<UncheckedUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Option<Vec<ProofInfo>>, Error> {
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

        if proofs.is_empty() {
            return Ok(None);
        }

        Ok(Some(proofs))
    }

    async fn remove_proofs(&self, proofs: &Proofs) -> Result<(), Error> {
        let mut mint_proofs = self.proofs.write().await;

        for proof in proofs {
            mint_proofs.remove(&proof.y().map_err(Error::from)?);
        }

        Ok(())
    }

    async fn set_proof_state(&self, y: PublicKey, state: State) -> Result<(), Self::Err> {
        let mint_proofs = self.proofs.read().await;

        let mint_proof = mint_proofs.get(&y).cloned();
        drop(mint_proofs);

        let mut mint_proofs = self.proofs.write().await;

        if let Some(proof_info) = mint_proof {
            let mut proof_info = proof_info.clone();

            proof_info.state = state;
            mint_proofs.insert(y, proof_info);
        }

        Ok(())
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

    #[cfg(feature = "nostr")]
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
    #[cfg(feature = "nostr")]
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
