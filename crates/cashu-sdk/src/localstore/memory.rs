use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use cashu::nuts::{Id, KeySetInfo, Keys, MintInfo};
use cashu::types::{MeltQuote, MintQuote};
use cashu::url::UncheckedUrl;
use tokio::sync::Mutex;

use super::{Error, LocalStore};

#[derive(Default, Debug, Clone)]
pub struct MemoryLocalStore {
    mints: Arc<Mutex<HashMap<UncheckedUrl, Option<MintInfo>>>>,
    mint_keysets: Arc<Mutex<HashMap<UncheckedUrl, HashSet<KeySetInfo>>>>,
    mint_quotes: Arc<Mutex<HashMap<String, MintQuote>>>,
    melt_quotes: Arc<Mutex<HashMap<String, MeltQuote>>>,
    mint_keys: Arc<Mutex<HashMap<Id, Keys>>>,
}

#[async_trait(?Send)]
impl LocalStore for MemoryLocalStore {
    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Error> {
        self.mints.lock().await.insert(mint_url, mint_info);
        Ok(())
    }

    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Error> {
        Ok(self.mints.lock().await.get(&mint_url).cloned().flatten())
    }

    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error> {
        let mut current_keysets = self.mint_keysets.lock().await;

        let current_keysets = current_keysets.entry(mint_url).or_insert(HashSet::new());
        current_keysets.extend(keysets);

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
}
