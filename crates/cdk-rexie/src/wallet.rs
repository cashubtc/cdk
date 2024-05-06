use std::collections::HashMap;
use std::rc::Rc;
use std::result::Result;

use async_trait::async_trait;
use cdk::cdk_database::WalletDatabase;
use cdk::nuts::{Id, KeySetInfo, Keys, MintInfo, Proofs};
use cdk::types::{MeltQuote, MintQuote};
use cdk::url::UncheckedUrl;
use rexie::*;
use thiserror::Error;
use tokio::sync::Mutex;

// Tables
const MINTS: &str = "mints";
const MINT_KEYSETS: &str = "mint_keysets";
const MINT_KEYS: &str = "mint_keys";
const MINT_QUOTES: &str = "mint_quotes";
const MELT_QUOTES: &str = "melt_quotes";
const PROOFS: &str = "proofs";
const PENDING_PROOFS: &str = "pending_proofs";
const CONFIG: &str = "config";
const KEYSET_COUNTER: &str = "keyset_counter";

const DATABASE_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum Error {
    /// CDK Database Error
    #[error(transparent)]
    CDKDatabase(#[from] cdk::cdk_database::Error),
    /// Rexie Error
    #[error(transparent)]
    Rexie(#[from] rexie::Error),
    /// Serde Wasm Error
    #[error(transparent)]
    SerdeBindgen(#[from] serde_wasm_bindgen::Error),
}
impl From<Error> for cdk::cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}

// These are okay because we never actually send across threads in the browser
unsafe impl Send for Error {}
unsafe impl Sync for Error {}

#[derive(Debug, Clone)]
pub struct RexieWalletDatabase {
    db: Rc<Mutex<Rexie>>,
}

// These are okay because we never actually send across threads in the browser
unsafe impl Send for RexieWalletDatabase {}
unsafe impl Sync for RexieWalletDatabase {}

impl RexieWalletDatabase {
    pub async fn new() -> Result<Self, Error> {
        let rexie = Rexie::builder("cdk")
            // Set the version of the database to 1.0
            .version(DATABASE_VERSION)
            // Add an object store named `employees`
            .add_object_store(
                ObjectStore::new(PROOFS)
                    // Add an index named `email` with the key path `email` with unique enabled
                    .add_index(Index::new("y", "y").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MINTS)
                    // Add an index named `email` with the key path `email` with unique enabled
                    .add_index(Index::new("mint_url", "mint_url").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MINT_KEYSETS)
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MINT_KEYS)
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MINT_QUOTES)
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MELT_QUOTES)
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(PENDING_PROOFS)
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(CONFIG)
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(KEYSET_COUNTER)
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            // Build the database
            .build()
            .await
            .unwrap();

        Ok(Self {
            db: Rc::new(Mutex::new(rexie)),
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WalletDatabase for RexieWalletDatabase {
    type Err = cdk::cdk_database::Error;

    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINTS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let mints_store = transaction.store(MINTS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;
        let mint_info = serde_wasm_bindgen::to_value(&mint_info).map_err(Error::from)?;

        mints_store
            .put(&mint_info, Some(&mint_url))
            .await
            .map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINTS], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let mints_store = transaction.store(MINTS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;
        let mint_info = mints_store.get(&mint_url).await.map_err(Error::from)?;

        let mint_info: Option<MintInfo> =
            serde_wasm_bindgen::from_value(mint_info).map_err(Error::from)?;

        Ok(mint_info)
    }

    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINTS], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let mints_store = transaction.store(MINTS).map_err(Error::from)?;

        let mints = mints_store
            .get_all(None, None, None, None)
            .await
            .map_err(Error::from)?;

        let mints: HashMap<UncheckedUrl, Option<MintInfo>> = mints
            .into_iter()
            .map(|(url, info)| {
                (
                    serde_wasm_bindgen::from_value(url).unwrap(),
                    serde_wasm_bindgen::from_value(info).unwrap(),
                )
            })
            .collect();

        Ok(mints)
    }

    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_KEYSETS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let keysets_store = transaction.store(MINT_KEYSETS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;
        let keysets = serde_wasm_bindgen::to_value(&keysets).map_err(Error::from)?;

        keysets_store
            .put(&keysets, Some(&mint_url))
            .await
            .map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_KEYSETS], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let mints_store = transaction.store(MINT_KEYSETS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;
        let keysets = mints_store.get(&mint_url).await.map_err(Error::from)?;

        let keysets: Option<Vec<KeySetInfo>> =
            serde_wasm_bindgen::from_value(keysets).map_err(Error::from)?;

        Ok(keysets)
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_QUOTES], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let quotes_store = transaction.store(MINT_QUOTES).map_err(Error::from)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote.id).map_err(Error::from)?;
        let quote = serde_wasm_bindgen::to_value(&quote).map_err(Error::from)?;

        quotes_store
            .add(&quote, Some(&quote_id))
            .await
            .map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_QUOTES], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let quotes_store = transaction.store(MINT_QUOTES).map_err(Error::from)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote_id).map_err(Error::from)?;
        let keysets = quotes_store.get(&quote_id).await.map_err(Error::from)?;

        let quote: Option<MintQuote> =
            serde_wasm_bindgen::from_value(keysets).map_err(Error::from)?;

        Ok(quote)
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_QUOTES], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let quotes_store = transaction.store(MINT_QUOTES).map_err(Error::from)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote_id).map_err(Error::from)?;

        quotes_store.delete(&quote_id).await.map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MELT_QUOTES], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let quotes_store = transaction.store(MELT_QUOTES).map_err(Error::from)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote.id).map_err(Error::from)?;
        let quote = serde_wasm_bindgen::to_value(&quote).map_err(Error::from)?;

        quotes_store
            .add(&quote, Some(&quote_id))
            .await
            .map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MELT_QUOTES], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let quotes_store = transaction.store(MELT_QUOTES).map_err(Error::from)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote_id).map_err(Error::from)?;
        let keysets = quotes_store.get(&quote_id).await.map_err(Error::from)?;

        let quote: Option<MeltQuote> =
            serde_wasm_bindgen::from_value(keysets).map_err(Error::from)?;

        Ok(quote)
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MELT_QUOTES], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let quotes_store = transaction.store(MELT_QUOTES).map_err(Error::from)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote_id).map_err(Error::from)?;

        quotes_store.delete(&quote_id).await.map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn add_keys(&self, keys: Keys) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_KEYS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let keys_store = transaction.store(MINT_KEYS).map_err(Error::from)?;

        let keyset_id = serde_wasm_bindgen::to_value(&Id::from(&keys)).map_err(Error::from)?;
        let keys = serde_wasm_bindgen::to_value(&keys).map_err(Error::from)?;

        keys_store
            .put(&keys, Some(&keyset_id))
            .await
            .map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_KEYS], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let keys_store = transaction.store(MINT_KEYS).map_err(Error::from)?;

        let keyset_id = serde_wasm_bindgen::to_value(id).map_err(Error::from)?;
        let keys = keys_store.get(&keyset_id).await.map_err(Error::from)?;

        let keys: Option<Keys> = serde_wasm_bindgen::from_value(keys).map_err(Error::from)?;

        Ok(keys)
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_KEYS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let keys_store = transaction.store(MINT_KEYS).map_err(Error::from)?;

        let keyset_id = serde_wasm_bindgen::to_value(id).map_err(Error::from)?;
        keys_store.delete(&keyset_id).await.map_err(Error::from)?;

        Ok(())
    }

    async fn add_proofs(&self, mint_url: UncheckedUrl, proofs: Proofs) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[PROOFS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let proofs_store = transaction.store(PROOFS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;

        let current_proofs = proofs_store.get(&mint_url).await.map_err(Error::from)?;

        let current_proofs: Proofs =
            serde_wasm_bindgen::from_value(current_proofs).unwrap_or_default();

        let all_proofs: Proofs = current_proofs
            .into_iter()
            .chain(proofs.into_iter())
            .collect();

        let all_proofs = serde_wasm_bindgen::to_value(&all_proofs).map_err(Error::from)?;

        web_sys::console::log_1(&all_proofs);

        proofs_store
            .put(&all_proofs, Some(&mint_url))
            .await
            .map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[PROOFS], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let proofs_store = transaction.store(PROOFS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;
        let proofs = proofs_store.get(&mint_url).await.map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        let proofs: Option<Proofs> = serde_wasm_bindgen::from_value(proofs).map_err(Error::from)?;

        Ok(proofs)
    }

    async fn remove_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: &Proofs,
    ) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[PROOFS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let proofs_store = transaction.store(PROOFS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;
        let current_proofs = proofs_store.get(&mint_url).await.map_err(Error::from)?;

        let current_proofs: Option<Proofs> =
            serde_wasm_bindgen::from_value(current_proofs).map_err(Error::from)?;

        if let Some(current_proofs) = current_proofs {
            let proofs: Proofs = current_proofs
                .into_iter()
                .filter(|p| !proofs.contains(p))
                .collect();

            let proofs = serde_wasm_bindgen::to_value(&proofs).map_err(Error::from)?;

            proofs_store
                .put(&proofs, Some(&mint_url))
                .await
                .map_err(Error::from)?;
        }

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn add_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: Proofs,
    ) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[PENDING_PROOFS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let proofs_store = transaction.store(PENDING_PROOFS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;

        let current_proofs = proofs_store.get(&mint_url).await.map_err(Error::from)?;

        let current_proofs: Proofs =
            serde_wasm_bindgen::from_value(current_proofs).unwrap_or_default();

        let all_proofs: Proofs = current_proofs
            .into_iter()
            .chain(proofs.into_iter())
            .collect();

        let all_proofs = serde_wasm_bindgen::to_value(&all_proofs).map_err(Error::from)?;

        proofs_store
            .put(&all_proofs, Some(&mint_url))
            .await
            .map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Proofs>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[PENDING_PROOFS], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let proofs_store = transaction.store(PENDING_PROOFS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;
        let proofs = proofs_store.get(&mint_url).await.map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        let proofs: Option<Proofs> = serde_wasm_bindgen::from_value(proofs).unwrap_or(None);

        Ok(proofs)
    }

    async fn remove_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: &Proofs,
    ) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[PENDING_PROOFS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let proofs_store = transaction.store(PENDING_PROOFS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;
        let current_proofs = proofs_store.get(&mint_url).await.map_err(Error::from)?;

        let current_proofs: Option<Proofs> =
            serde_wasm_bindgen::from_value(current_proofs).map_err(Error::from)?;

        if let Some(current_proofs) = current_proofs {
            let proofs: Proofs = current_proofs
                .into_iter()
                .filter(|p| !proofs.contains(p))
                .collect();

            let proofs = serde_wasm_bindgen::to_value(&proofs).map_err(Error::from)?;

            proofs_store
                .add(&proofs, Some(&mint_url))
                .await
                .map_err(Error::from)?;
        }

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u64) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[KEYSET_COUNTER], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let counter_store = transaction.store(KEYSET_COUNTER).map_err(Error::from)?;

        let keyset_id = serde_wasm_bindgen::to_value(keyset_id).map_err(Error::from)?;

        let current_count = counter_store.get(&keyset_id).await.map_err(Error::from)?;
        let current_count: Option<u64> =
            serde_wasm_bindgen::from_value(current_count).map_err(Error::from)?;

        let new_count = current_count.unwrap_or_default() + count;

        let new_count = serde_wasm_bindgen::to_value(&new_count).map_err(Error::from)?;

        counter_store
            .put(&new_count, Some(&keyset_id))
            .await
            .map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u64>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[KEYSET_COUNTER], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let counter_store = transaction.store(KEYSET_COUNTER).map_err(Error::from)?;

        let keyset_id = serde_wasm_bindgen::to_value(keyset_id).map_err(Error::from)?;

        let current_count = counter_store.get(&keyset_id).await.map_err(Error::from)?;
        let current_count: Option<u64> =
            serde_wasm_bindgen::from_value(current_count).map_err(Error::from)?;

        Ok(current_count)
    }
}
