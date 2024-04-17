use std::collections::HashMap;
use std::result::Result;
use std::sync::Arc;

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

const DATABASE_VERSION: u32 = 0;

#[derive(Debug, Error)]
pub enum Error {
    /// CDK Database Error
    #[error(transparent)]
    CDKDatabase(#[from] cdk::cdk_database::Error),
    /// Rexie Error
    #[error(transparent)]
    Redb(#[from] rexie::Error),
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
    db: Arc<Mutex<Rexie>>,
}

// These are okay because we never actually send across threads in the browser
//unsafe impl Send for RexieWalletDatabase {}
//unsafe impl Sync for RexieWalletDatabase {}

impl RexieWalletDatabase {
    pub async fn new() -> Result<Self, Error> {
        let rexie = Rexie::builder("cdk")
            // Set the version of the database to 1.0
            .version(DATABASE_VERSION)
            // Add an object store named `employees`
            .add_object_store(
                ObjectStore::new(PROOFS)
                    // Set the key path to `id`
                    .key_path("y")
                    // Add an index named `email` with the key path `email` with unique enabled
                    .add_index(Index::new("y", "y").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MINTS)
                    // Set the key path to `id`
                    .key_path("mint_url")
                    // Add an index named `email` with the key path `email` with unique enabled
                    .add_index(Index::new("mint_url", "mint_url").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MINT_KEYSETS)
                    .key_path("keyset_id")
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MINT_KEYS)
                    .key_path("keyset_id")
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MINT_QUOTES)
                    .key_path("keyset_id")
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MELT_QUOTES)
                    .key_path("keyset_id")
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(PENDING_PROOFS)
                    .key_path("keyset_id")
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(CONFIG)
                    .key_path("keyset_id")
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(KEYSET_COUNTER)
                    .key_path("keyset_id")
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            // Build the database
            .build()
            .await
            .unwrap();

        Ok(Self {
            db: Arc::new(Mutex::new(rexie)),
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WalletDatabase for RexieWalletDatabase {
    type Err = Error;
    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINTS], TransactionMode::ReadWrite)?;

        let mints_store = transaction.store(MINTS)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url)?;
        let mint_info = serde_wasm_bindgen::to_value(&mint_info)?;

        mints_store.add(&mint_info, Some(&mint_url)).await?;

        transaction.done().await?;

        Ok(())
    }

    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINTS], TransactionMode::ReadOnly)?;

        let mints_store = transaction.store(MINTS)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url)?;
        let mint_info = mints_store.get(&mint_url).await?;

        let mint_info: Option<MintInfo> = serde_wasm_bindgen::from_value(mint_info)?;

        Ok(mint_info)
    }

    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINTS], TransactionMode::ReadOnly)?;

        let mints_store = transaction.store(MINTS)?;

        let mints = mints_store.get_all(None, None, None, None).await?;

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
    ) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINT_KEYSETS], TransactionMode::ReadWrite)?;

        let keysets_store = transaction.store(MINT_KEYSETS)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url)?;
        let keysets = serde_wasm_bindgen::to_value(&keysets)?;

        keysets_store.add(&keysets, Some(&mint_url)).await?;

        transaction.done().await?;

        Ok(())
    }

    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINT_KEYSETS], TransactionMode::ReadOnly)?;

        let mints_store = transaction.store(MINT_KEYSETS)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url)?;
        let keysets = mints_store.get(&mint_url).await?;

        let keysets: Option<Vec<KeySetInfo>> = serde_wasm_bindgen::from_value(keysets)?;

        Ok(keysets)
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINT_QUOTES], TransactionMode::ReadWrite)?;

        let quotes_store = transaction.store(MINT_QUOTES)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote.id)?;
        let quote = serde_wasm_bindgen::to_value(&quote)?;

        quotes_store.add(&quote, Some(&quote_id)).await?;

        transaction.done().await?;

        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINT_QUOTES], TransactionMode::ReadOnly)?;

        let quotes_store = transaction.store(MINT_QUOTES)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote_id)?;
        let keysets = quotes_store.get(&quote_id).await?;

        let quote: Option<MintQuote> = serde_wasm_bindgen::from_value(keysets)?;

        Ok(quote)
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINT_QUOTES], TransactionMode::ReadWrite)?;

        let quotes_store = transaction.store(MINT_QUOTES)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote_id)?;

        quotes_store.delete(&quote_id).await?;

        transaction.done().await?;

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MELT_QUOTES], TransactionMode::ReadWrite)?;

        let quotes_store = transaction.store(MELT_QUOTES)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote.id)?;
        let quote = serde_wasm_bindgen::to_value(&quote)?;

        quotes_store.add(&quote, Some(&quote_id)).await?;

        transaction.done().await?;

        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MELT_QUOTES], TransactionMode::ReadOnly)?;

        let quotes_store = transaction.store(MELT_QUOTES)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote_id)?;
        let keysets = quotes_store.get(&quote_id).await?;

        let quote: Option<MeltQuote> = serde_wasm_bindgen::from_value(keysets)?;

        Ok(quote)
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MELT_QUOTES], TransactionMode::ReadWrite)?;

        let quotes_store = transaction.store(MELT_QUOTES)?;

        let quote_id = serde_wasm_bindgen::to_value(&quote_id)?;

        quotes_store.delete(&quote_id).await?;

        transaction.done().await?;

        Ok(())
    }

    async fn add_keys(&self, keys: Keys) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINT_KEYS], TransactionMode::ReadWrite)?;

        let keys_store = transaction.store(MINT_KEYS)?;

        let keyset_id = serde_wasm_bindgen::to_value(&Id::from(&keys))?;
        let keys = serde_wasm_bindgen::to_value(&keys)?;

        keys_store.add(&keys, Some(&keyset_id)).await?;

        transaction.done().await?;

        Ok(())
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINT_KEYS], TransactionMode::ReadOnly)?;

        let keys_store = transaction.store(MINT_KEYS)?;

        let keyset_id = serde_wasm_bindgen::to_value(id)?;
        let keys = keys_store.get(&keyset_id).await?;

        let keys: Option<Keys> = serde_wasm_bindgen::from_value(keys)?;

        Ok(keys)
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[MINT_KEYS], TransactionMode::ReadWrite)?;

        let keys_store = transaction.store(MINT_KEYS)?;

        let keyset_id = serde_wasm_bindgen::to_value(id)?;
        keys_store.delete(&keyset_id).await?;

        Ok(())
    }

    async fn add_proofs(&self, mint_url: UncheckedUrl, proofs: Proofs) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[PROOFS], TransactionMode::ReadWrite)?;

        let proofs_store = transaction.store(PROOFS)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url)?;

        let current_proofs = proofs_store.get(&mint_url).await?;

        let current_proofs: Proofs = serde_wasm_bindgen::from_value(current_proofs)?;

        let all_proofs: Proofs = current_proofs
            .into_iter()
            .chain(proofs.into_iter())
            .collect();

        let all_proofs = serde_wasm_bindgen::to_value(&all_proofs)?;

        proofs_store.add(&all_proofs, Some(&mint_url)).await?;

        transaction.done().await?;

        Ok(())
    }

    async fn get_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[PROOFS], TransactionMode::ReadOnly)?;

        let proofs_store = transaction.store(PROOFS)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url)?;
        let proofs = proofs_store.get(&mint_url).await?;

        transaction.done().await?;

        let proofs: Option<Proofs> = serde_wasm_bindgen::from_value(proofs)?;

        Ok(proofs)
    }

    async fn remove_proofs(&self, mint_url: UncheckedUrl, proofs: &Proofs) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[PROOFS], TransactionMode::ReadWrite)?;

        let proofs_store = transaction.store(PROOFS)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url)?;
        let current_proofs = proofs_store.get(&mint_url).await?;

        let current_proofs: Option<Proofs> = serde_wasm_bindgen::from_value(current_proofs)?;

        if let Some(current_proofs) = current_proofs {
            let proofs: Proofs = current_proofs
                .into_iter()
                .filter(|p| !proofs.contains(p))
                .collect();

            let proofs = serde_wasm_bindgen::to_value(&proofs)?;

            proofs_store.add(&proofs, Some(&mint_url)).await?;
        }

        transaction.done().await?;

        Ok(())
    }

    async fn add_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: Proofs,
    ) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[PENDING_PROOFS], TransactionMode::ReadWrite)?;

        let proofs_store = transaction.store(PENDING_PROOFS)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url)?;

        let current_proofs = proofs_store.get(&mint_url).await?;

        let current_proofs: Proofs = serde_wasm_bindgen::from_value(current_proofs)?;

        let all_proofs: Proofs = current_proofs
            .into_iter()
            .chain(proofs.into_iter())
            .collect();

        let all_proofs = serde_wasm_bindgen::to_value(&all_proofs)?;

        proofs_store.add(&all_proofs, Some(&mint_url)).await?;

        transaction.done().await?;

        Ok(())
    }

    async fn get_pending_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[PENDING_PROOFS], TransactionMode::ReadOnly)?;

        let proofs_store = transaction.store(PENDING_PROOFS)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url)?;
        let proofs = proofs_store.get(&mint_url).await?;

        transaction.done().await?;

        let proofs: Option<Proofs> = serde_wasm_bindgen::from_value(proofs)?;

        Ok(proofs)
    }

    async fn remove_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: &Proofs,
    ) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[PENDING_PROOFS], TransactionMode::ReadWrite)?;

        let proofs_store = transaction.store(PENDING_PROOFS)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url)?;
        let current_proofs = proofs_store.get(&mint_url).await?;

        let current_proofs: Option<Proofs> = serde_wasm_bindgen::from_value(current_proofs)?;

        if let Some(current_proofs) = current_proofs {
            let proofs: Proofs = current_proofs
                .into_iter()
                .filter(|p| !proofs.contains(p))
                .collect();

            let proofs = serde_wasm_bindgen::to_value(&proofs)?;

            proofs_store.add(&proofs, Some(&mint_url)).await?;
        }

        transaction.done().await?;

        Ok(())
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u64) -> Result<(), Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[KEYSET_COUNTER], TransactionMode::ReadWrite)?;

        let counter_store = transaction.store(KEYSET_COUNTER)?;

        let keyset_id = serde_wasm_bindgen::to_value(keyset_id)?;

        let current_count = counter_store.get(&keyset_id).await?;
        let current_count: Option<u64> = serde_wasm_bindgen::from_value(current_count)?;

        let new_count = current_count.unwrap_or_default() + count;

        let new_count = serde_wasm_bindgen::to_value(&new_count)?;

        counter_store.add(&new_count, Some(&keyset_id)).await?;

        transaction.done().await?;

        Ok(())
    }

    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u64>, Error> {
        let rexie = self.db.lock().await;

        let transaction = rexie.transaction(&[KEYSET_COUNTER], TransactionMode::ReadWrite)?;

        let counter_store = transaction.store(KEYSET_COUNTER)?;

        let keyset_id = serde_wasm_bindgen::to_value(keyset_id)?;

        let current_count = counter_store.get(&keyset_id).await?;
        let current_count: Option<u64> = serde_wasm_bindgen::from_value(current_count)?;

        Ok(current_count)
    }
}
