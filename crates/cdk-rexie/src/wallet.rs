//! Rexie Browser Database

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::result::Result;

use async_trait::async_trait;
use cdk::cdk_database::WalletDatabase;
use cdk::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, Proofs, PublicKey, SpendingConditions, State,
};
use cdk::types::{MeltQuote, MintQuote, ProofInfo};
use cdk::url::UncheckedUrl;
use cdk::util::unix_time;
use rexie::*;
use thiserror::Error;
use tokio::sync::Mutex;

// Tables
const MINTS: &str = "mints";
const MINT_KEYSETS: &str = "keysets_by_mint";
const KEYSETS: &str = "keysets";
const MINT_KEYS: &str = "mint_keys";
const MINT_QUOTES: &str = "mint_quotes";
const MELT_QUOTES: &str = "melt_quotes";
const PROOFS: &str = "proofs";
const CONFIG: &str = "config";
const KEYSET_COUNTER: &str = "keyset_counter";
const NOSTR_LAST_CHECKED: &str = "nostr_last_check";

const DATABASE_VERSION: u32 = 3;

/// Rexie Database Error
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
    /// NUT00 Error
    #[error(transparent)]
    NUT00(cdk::nuts::nut00::Error),
}
impl From<Error> for cdk::cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}

// These are okay because we never actually send across threads in the browser
unsafe impl Send for Error {}
unsafe impl Sync for Error {}

/// Wallet Rexie Database
#[derive(Debug, Clone)]
pub struct WalletRexieDatabase {
    db: Rc<Mutex<Rexie>>,
}

// These are okay because we never actually send across threads in the browser
unsafe impl Send for WalletRexieDatabase {}
unsafe impl Sync for WalletRexieDatabase {}

impl WalletRexieDatabase {
    /// Create new [`WalletRexieDatabase`]
    pub async fn new() -> Result<Self, Error> {
        let rexie = Rexie::builder("cdk")
            .version(DATABASE_VERSION)
            .add_object_store(
                ObjectStore::new(PROOFS)
                    .add_index(Index::new("y", "y").unique(true))
                    .add_index(Index::new("mint_url", "mint_url"))
                    .add_index(Index::new("state", "state"))
                    .add_index(Index::new("unit", "unit")),
            )
            .add_object_store(
                ObjectStore::new(MINTS).add_index(Index::new("mint_url", "mint_url").unique(true)),
            )
            .add_object_store(ObjectStore::new(MINT_KEYSETS))
            .add_object_store(
                ObjectStore::new(KEYSETS)
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(
                ObjectStore::new(MINT_KEYS)
                    .add_index(Index::new("keyset_id", "keyset_id").unique(true)),
            )
            .add_object_store(ObjectStore::new(MINT_QUOTES))
            .add_object_store(ObjectStore::new(MELT_QUOTES))
            .add_object_store(ObjectStore::new(CONFIG))
            .add_object_store(ObjectStore::new(KEYSET_COUNTER))
            .add_object_store(ObjectStore::new(NOSTR_LAST_CHECKED))
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
impl WalletDatabase for WalletRexieDatabase {
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

    async fn remove_mint(&self, mint_url: UncheckedUrl) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINTS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let mints_store = transaction.store(MINTS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;

        mints_store.delete(&mint_url).await.map_err(Error::from)?;

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

    async fn update_mint_url(
        &self,
        old_mint_url: UncheckedUrl,
        new_mint_url: UncheckedUrl,
    ) -> Result<(), Self::Err> {
        let proofs = self
            .get_proofs(Some(old_mint_url), None, None, None)
            .await
            .map_err(Error::from)?;

        if let Some(proofs) = proofs {
            let updated_proofs: Vec<ProofInfo> = proofs
                .clone()
                .into_iter()
                .map(|mut p| {
                    p.mint_url = new_mint_url.clone();
                    p
                })
                .collect();

            self.add_proofs(updated_proofs).await?;
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
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_KEYSETS, KEYSETS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let mint_keysets_store = transaction.store(MINT_KEYSETS).map_err(Error::from)?;
        let keysets_store = transaction.store(KEYSETS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;

        let mint_keysets = mint_keysets_store
            .get(&mint_url)
            .await
            .map_err(Error::from)?;

        let mut mint_keysets: Option<HashSet<Id>> =
            serde_wasm_bindgen::from_value(mint_keysets).map_err(Error::from)?;

        let new_keyset_ids: Vec<Id> = keysets.iter().map(|k| k.id).collect();

        mint_keysets
            .as_mut()
            .unwrap_or(&mut HashSet::new())
            .extend(new_keyset_ids);

        let mint_keysets = serde_wasm_bindgen::to_value(&mint_keysets).map_err(Error::from)?;

        mint_keysets_store
            .put(&mint_keysets, Some(&mint_url))
            .await
            .map_err(Error::from)?;

        for keyset in keysets {
            let id = serde_wasm_bindgen::to_value(&keyset.id).map_err(Error::from)?;
            let keyset = serde_wasm_bindgen::to_value(&keyset).map_err(Error::from)?;

            keysets_store
                .put(&keyset, Some(&id))
                .await
                .map_err(Error::from)?;
        }

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_KEYSETS, KEYSETS], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let mints_store = transaction.store(MINT_KEYSETS).map_err(Error::from)?;

        let mint_url = serde_wasm_bindgen::to_value(&mint_url).map_err(Error::from)?;
        let mint_keysets = mints_store.get(&mint_url).await.map_err(Error::from)?;

        let mint_keysets: Option<HashSet<Id>> =
            serde_wasm_bindgen::from_value(mint_keysets).map_err(Error::from)?;

        let keysets_store = transaction.store(KEYSETS).map_err(Error::from)?;

        let keysets = match mint_keysets {
            Some(mint_keysets) => {
                let mut keysets = vec![];

                for mint_keyset in mint_keysets {
                    let id = serde_wasm_bindgen::to_value(&mint_keyset).map_err(Error::from)?;

                    let keyset = keysets_store.get(&id).await.map_err(Error::from)?;

                    let keyset = serde_wasm_bindgen::from_value(keyset).map_err(Error::from)?;

                    keysets.push(keyset);
                }

                Some(keysets)
            }
            None => None,
        };

        transaction.done().await.map_err(Error::from)?;

        Ok(keysets)
    }

    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[KEYSETS], TransactionMode::ReadOnly)
            .map_err(Error::from)?;
        let keysets_store = transaction.store(KEYSETS).map_err(Error::from)?;

        let keyset_id = serde_wasm_bindgen::to_value(keyset_id).map_err(Error::from)?;

        let keyset = keysets_store.get(&keyset_id).await.map_err(Error::from)?;

        Ok(serde_wasm_bindgen::from_value(keyset).map_err(Error::from)?)
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
            .put(&quote, Some(&quote_id))
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
        let quote = quotes_store.get(&quote_id).await.map_err(Error::from)?;

        let quote: Option<MintQuote> =
            serde_wasm_bindgen::from_value(quote).map_err(Error::from)?;

        Ok(quote)
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[MINT_QUOTES], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let quotes_store = transaction.store(MINT_QUOTES).map_err(Error::from)?;

        let quotes = quotes_store
            .get_all(None, None, None, None)
            .await
            .map_err(Error::from)?;

        Ok(quotes
            .into_iter()
            .flat_map(|(_id, q)| serde_wasm_bindgen::from_value(q))
            .collect())
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
            .put(&quote, Some(&quote_id))
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

    async fn add_proofs(&self, proofs: Vec<ProofInfo>) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[PROOFS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let proofs_store = transaction.store(PROOFS).map_err(Error::from)?;

        for proof in proofs {
            let y = proof.y;
            let y = serde_wasm_bindgen::to_value(&y).map_err(Error::from)?;
            let proof = serde_wasm_bindgen::to_value(&proof).map_err(Error::from)?;

            proofs_store
                .put(&proof, Some(&y))
                .await
                .map_err(Error::from)?;
        }

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_proofs(
        &self,
        mint_url: Option<UncheckedUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Option<Vec<ProofInfo>>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[PROOFS], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let proofs_store = transaction.store(PROOFS).map_err(Error::from)?;

        let proofs = proofs_store
            .get_all(None, None, None, None)
            .await
            .map_err(Error::from)?;

        let proofs: Vec<ProofInfo> = proofs
            .into_iter()
            .filter_map(|(_k, v)| {
                let mut proof = None;

                if let Ok(proof_info) = serde_wasm_bindgen::from_value::<ProofInfo>(v) {
                    proof = match proof_info.matches_conditions(
                        &mint_url,
                        &unit,
                        &state,
                        &spending_conditions,
                    ) {
                        true => Some(proof_info),
                        false => None,
                    };
                }

                proof
            })
            .collect();

        transaction.done().await.map_err(Error::from)?;

        if proofs.is_empty() {
            return Ok(None);
        }

        Ok(Some(proofs))
    }

    async fn remove_proofs(&self, proofs: &Proofs) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[PROOFS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let proofs_store = transaction.store(PROOFS).map_err(Error::from)?;

        for proof in proofs {
            let y = serde_wasm_bindgen::to_value(&proof.y()?).map_err(Error::from)?;

            proofs_store.delete(&y).await.map_err(Error::from)?;
        }

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn set_proof_state(&self, y: PublicKey, state: State) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[PROOFS], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let proofs_store = transaction.store(PROOFS).map_err(Error::from)?;

        let y = serde_wasm_bindgen::to_value(&y).map_err(Error::from)?;

        let proof = proofs_store.get(&y).await.map_err(Error::from)?;
        let mut proof: ProofInfo = serde_wasm_bindgen::from_value(proof).map_err(Error::from)?;

        proof.state = state;

        let proof = serde_wasm_bindgen::to_value(&proof).map_err(Error::from)?;

        proofs_store
            .put(&proof, Some(&y))
            .await
            .map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[KEYSET_COUNTER], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let counter_store = transaction.store(KEYSET_COUNTER).map_err(Error::from)?;

        let keyset_id = serde_wasm_bindgen::to_value(keyset_id).map_err(Error::from)?;

        let current_count = counter_store.get(&keyset_id).await.map_err(Error::from)?;
        let current_count: Option<u32> =
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

    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u32>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[KEYSET_COUNTER], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let counter_store = transaction.store(KEYSET_COUNTER).map_err(Error::from)?;

        let keyset_id = serde_wasm_bindgen::to_value(keyset_id).map_err(Error::from)?;

        let current_count = counter_store.get(&keyset_id).await.map_err(Error::from)?;
        let current_count: Option<u32> =
            serde_wasm_bindgen::from_value(current_count).map_err(Error::from)?;

        Ok(current_count)
    }

    async fn add_nostr_last_checked(
        &self,
        verifying_key: PublicKey,
        last_checked: u32,
    ) -> Result<(), Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[NOSTR_LAST_CHECKED], TransactionMode::ReadWrite)
            .map_err(Error::from)?;

        let counter_store = transaction.store(NOSTR_LAST_CHECKED).map_err(Error::from)?;

        let verifying_key = serde_wasm_bindgen::to_value(&verifying_key).map_err(Error::from)?;

        let last_checked = serde_wasm_bindgen::to_value(&last_checked).map_err(Error::from)?;

        counter_store
            .put(&last_checked, Some(&verifying_key))
            .await
            .map_err(Error::from)?;

        transaction.done().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_nostr_last_checked(
        &self,
        verifying_key: &PublicKey,
    ) -> Result<Option<u32>, Self::Err> {
        let rexie = self.db.lock().await;

        let transaction = rexie
            .transaction(&[NOSTR_LAST_CHECKED], TransactionMode::ReadOnly)
            .map_err(Error::from)?;

        let nostr_last_check_store = transaction.store(NOSTR_LAST_CHECKED).map_err(Error::from)?;

        let verifying_key = serde_wasm_bindgen::to_value(verifying_key).map_err(Error::from)?;

        let last_checked = nostr_last_check_store
            .get(&verifying_key)
            .await
            .map_err(Error::from)?;
        let last_checked: Option<u32> =
            serde_wasm_bindgen::from_value(last_checked).map_err(Error::from)?;

        Ok(last_checked)
    }
}
