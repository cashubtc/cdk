//! LocalStorage-backed wallet database for WASM
//!
//! A write-through cache implementation: all data lives in memory (HashMaps/Vecs)
//! for fast reads, and every write is persisted to a JS storage backend
//! (localStorage by default). On construction the cache is hydrated from storage.

use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::database::{Error, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{
    CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use cdk_common::wallet::{
    MeltQuote, MintQuote, ProofInfo, Transaction, TransactionDirection, TransactionId, WalletSaga,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::RwLock;
use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// JS FFI – imported from js/storage.js
// ---------------------------------------------------------------------------
#[wasm_bindgen(module = "/js/storage.js")]
extern "C" {
    #[wasm_bindgen(js_name = "storageGet")]
    fn storage_get(key: &str) -> Option<String>;

    #[wasm_bindgen(js_name = "storageSet")]
    fn storage_set(key: &str, value: &str);

    #[wasm_bindgen(js_name = "storageRemove")]
    fn storage_remove(key: &str);

    #[wasm_bindgen(js_name = "setStorageBackend")]
    pub fn set_storage_backend(backend: JsValue);
}

// ---------------------------------------------------------------------------
// Re-export setStorageBackend to JS consumers of the WASM module
// ---------------------------------------------------------------------------
#[wasm_bindgen(js_name = "setStorageBackend")]
pub fn set_storage_backend_wasm(backend: JsValue) {
    set_storage_backend(backend);
}

// ---------------------------------------------------------------------------
// Storage keys (one JSON blob per "table")
// ---------------------------------------------------------------------------
const KEY_MINTS: &str = "mints";
const KEY_MINT_KEYSETS: &str = "mint_keysets";
const KEY_KEYSET_COUNTER: &str = "keyset_counter";
const KEY_KEYS: &str = "keys";
const KEY_PROOFS: &str = "proofs";
const KEY_MINT_QUOTES: &str = "mint_quotes";
const KEY_MELT_QUOTES: &str = "melt_quotes";
const KEY_TRANSACTIONS: &str = "transactions";
const KEY_SAGAS: &str = "sagas";
const KEY_PROOF_RESERVATIONS: &str = "proof_reservations";
const KEY_MELT_QUOTE_RESERVATIONS: &str = "melt_quote_reservations";
const KEY_MINT_QUOTE_RESERVATIONS: &str = "mint_quote_reservations";
const KEY_KV: &str = "kv";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
fn load_or_default<T: DeserializeOwned + Default>(key: &str) -> T {
    storage_get(key)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn persist<T: Serialize>(key: &str, value: &T) {
    if let Ok(json) = serde_json::to_string(value) {
        storage_set(key, &json);
    }
}

// ---------------------------------------------------------------------------
// Inner state – same shape as the old MemoryDatabase
// ---------------------------------------------------------------------------
// KV uses a single String key (components joined by \0) because JSON object
// keys must be strings — tuple keys like (String, String, String) cannot
// round-trip through serde_json as map keys.

type Db = Arc<dyn WalletDatabase<Error> + Send + Sync>;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Inner {
    mints: HashMap<MintUrl, Option<MintInfo>>,
    mint_keysets: HashMap<MintUrl, Vec<KeySetInfo>>,
    keyset_counter: HashMap<Id, u32>,
    keys: HashMap<Id, Keys>,
    proofs: Vec<ProofInfo>,
    mint_quotes: Vec<MintQuote>,
    melt_quotes: Vec<MeltQuote>,
    transactions: Vec<Transaction>,
    sagas: HashMap<String, WalletSaga>,
    proof_reservations: HashMap<uuid::Uuid, Vec<PublicKey>>,
    melt_quote_reservations: HashMap<uuid::Uuid, String>,
    mint_quote_reservations: HashMap<uuid::Uuid, String>,
    /// Composite key: "primary\0secondary\0key" → bytes (hex-encoded for JSON)
    kv: HashMap<String, Vec<u8>>,
}

impl Inner {
    /// Hydrate from JS storage, falling back to empty defaults.
    fn load() -> Self {
        Self {
            mints: load_or_default(KEY_MINTS),
            mint_keysets: load_or_default(KEY_MINT_KEYSETS),
            keyset_counter: load_or_default(KEY_KEYSET_COUNTER),
            keys: load_or_default(KEY_KEYS),
            proofs: load_or_default(KEY_PROOFS),
            mint_quotes: load_or_default(KEY_MINT_QUOTES),
            melt_quotes: load_or_default(KEY_MELT_QUOTES),
            transactions: load_or_default(KEY_TRANSACTIONS),
            sagas: load_or_default(KEY_SAGAS),
            proof_reservations: load_or_default(KEY_PROOF_RESERVATIONS),
            melt_quote_reservations: load_or_default(KEY_MELT_QUOTE_RESERVATIONS),
            mint_quote_reservations: load_or_default(KEY_MINT_QUOTE_RESERVATIONS),
            kv: load_or_default(KEY_KV),
        }
    }
}

fn kv_key(primary: &str, secondary: &str, key: &str) -> String {
    format!("{}\0{}\0{}", primary, secondary, key)
}

// ---------------------------------------------------------------------------
// Public database type
// ---------------------------------------------------------------------------

/// LocalStorage-backed wallet database with in-memory write-through cache
#[derive(Debug)]
pub struct LocalStorageDatabase {
    inner: RwLock<Inner>,
}

impl LocalStorageDatabase {
    /// Create a new database, loading persisted state from JS storage.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner::load()),
        }
    }

    pub fn into_arc(self) -> Db {
        Arc::new(self)
    }
}

// ---------------------------------------------------------------------------
// WalletDatabase trait implementation
// ---------------------------------------------------------------------------
#[async_trait::async_trait(?Send)]
impl WalletDatabase<Error> for LocalStorageDatabase {
    // ---- reads (from memory only) -----------------------------------------

    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Error> {
        let db = self.inner.read().await;
        Ok(db.mints.get(&mint_url).cloned().flatten())
    }

    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Error> {
        let db = self.inner.read().await;
        Ok(db.mints.clone())
    }

    async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<Option<Vec<KeySetInfo>>, Error> {
        let db = self.inner.read().await;
        Ok(db.mint_keysets.get(&mint_url).cloned())
    }

    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Error> {
        let db = self.inner.read().await;
        for keysets in db.mint_keysets.values() {
            if let Some(ks) = keysets.iter().find(|k| &k.id == keyset_id) {
                return Ok(Some(ks.clone()));
            }
        }
        Ok(None)
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Error> {
        let db = self.inner.read().await;
        Ok(db.mint_quotes.iter().find(|q| q.id == quote_id).cloned())
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let db = self.inner.read().await;
        Ok(db.mint_quotes.clone())
    }

    async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let db = self.inner.read().await;
        Ok(db
            .mint_quotes
            .iter()
            .filter(|q| q.amount_issued == cdk_common::Amount::ZERO)
            .cloned()
            .collect())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Error> {
        let db = self.inner.read().await;
        Ok(db.melt_quotes.iter().find(|q| q.id == quote_id).cloned())
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let db = self.inner.read().await;
        Ok(db.melt_quotes.clone())
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Error> {
        let db = self.inner.read().await;
        Ok(db.keys.get(id).cloned())
    }

    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        _spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Error> {
        let db = self.inner.read().await;
        Ok(db
            .proofs
            .iter()
            .filter(|p| mint_url.as_ref().is_none_or(|u| &p.mint_url == u))
            .filter(|p| unit.as_ref().is_none_or(|u| &p.unit == u))
            .filter(|p| state.as_ref().is_none_or(|s| s.contains(&p.state)))
            .cloned()
            .collect())
    }

    async fn get_proofs_by_ys(&self, ys: Vec<PublicKey>) -> Result<Vec<ProofInfo>, Error> {
        let db = self.inner.read().await;
        Ok(db
            .proofs
            .iter()
            .filter(|p| ys.contains(&p.y))
            .cloned()
            .collect())
    }

    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
    ) -> Result<u64, Error> {
        let proofs = self.get_proofs(mint_url, unit, state, None).await?;
        Ok(proofs.iter().map(|p| u64::from(p.proof.amount)).sum())
    }

    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, Error> {
        let db = self.inner.read().await;
        Ok(db
            .transactions
            .iter()
            .find(|t| t.id() == transaction_id)
            .cloned())
    }

    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, Error> {
        let db = self.inner.read().await;
        Ok(db
            .transactions
            .iter()
            .filter(|t| mint_url.as_ref().is_none_or(|u| &t.mint_url == u))
            .filter(|t| direction.as_ref().is_none_or(|d| &t.direction == d))
            .filter(|t| unit.as_ref().is_none_or(|u| &t.unit == u))
            .cloned()
            .collect())
    }

    // ---- writes (memory + persist) ----------------------------------------

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.proofs.retain(|p| !removed_ys.contains(&p.y));
        db.proofs.extend(added);
        persist(KEY_PROOFS, &db.proofs);
        Ok(())
    }

    async fn update_proofs_state(&self, ys: Vec<PublicKey>, state: State) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        for proof in db.proofs.iter_mut() {
            if ys.contains(&proof.y) {
                proof.state = state;
            }
        }
        persist(KEY_PROOFS, &db.proofs);
        Ok(())
    }

    async fn add_transaction(&self, transaction: Transaction) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.transactions.push(transaction);
        persist(KEY_TRANSACTIONS, &db.transactions);
        Ok(())
    }

    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        if let Some(info) = db.mints.remove(&old_mint_url) {
            db.mints.insert(new_mint_url.clone(), info);
        }
        if let Some(keysets) = db.mint_keysets.remove(&old_mint_url) {
            db.mint_keysets.insert(new_mint_url, keysets);
        }
        persist(KEY_MINTS, &db.mints);
        persist(KEY_MINT_KEYSETS, &db.mint_keysets);
        Ok(())
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<u32, Error> {
        let mut db = self.inner.write().await;
        let counter = db.keyset_counter.entry(*keyset_id).or_insert(0);
        let old = *counter;
        *counter += count;
        persist(KEY_KEYSET_COUNTER, &db.keyset_counter);
        Ok(old)
    }

    async fn add_mint(&self, mint_url: MintUrl, mint_info: Option<MintInfo>) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.mints.insert(mint_url, mint_info);
        persist(KEY_MINTS, &db.mints);
        Ok(())
    }

    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.mints.remove(&mint_url);
        db.mint_keysets.remove(&mint_url);
        persist(KEY_MINTS, &db.mints);
        persist(KEY_MINT_KEYSETS, &db.mint_keysets);
        Ok(())
    }

    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.mint_keysets.entry(mint_url).or_default().extend(keysets);
        persist(KEY_MINT_KEYSETS, &db.mint_keysets);
        Ok(())
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.mint_quotes.retain(|q| q.id != quote.id);
        db.mint_quotes.push(quote);
        persist(KEY_MINT_QUOTES, &db.mint_quotes);
        Ok(())
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.mint_quotes.retain(|q| q.id != quote_id);
        persist(KEY_MINT_QUOTES, &db.mint_quotes);
        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.melt_quotes.retain(|q| q.id != quote.id);
        db.melt_quotes.push(quote);
        persist(KEY_MELT_QUOTES, &db.melt_quotes);
        Ok(())
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.melt_quotes.retain(|q| q.id != quote_id);
        persist(KEY_MELT_QUOTES, &db.melt_quotes);
        Ok(())
    }

    async fn add_keys(&self, keyset: KeySet) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.keys.insert(keyset.id, keyset.keys);
        persist(KEY_KEYS, &db.keys);
        Ok(())
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.keys.remove(id);
        persist(KEY_KEYS, &db.keys);
        Ok(())
    }

    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.transactions.retain(|t| t.id() != transaction_id);
        persist(KEY_TRANSACTIONS, &db.transactions);
        Ok(())
    }

    async fn add_saga(&self, saga: WalletSaga) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.sagas.insert(saga.id.to_string(), saga);
        persist(KEY_SAGAS, &db.sagas);
        Ok(())
    }

    async fn get_saga(&self, id: &uuid::Uuid) -> Result<Option<WalletSaga>, Error> {
        let db = self.inner.read().await;
        Ok(db.sagas.get(&id.to_string()).cloned())
    }

    async fn update_saga(&self, saga: WalletSaga) -> Result<bool, Error> {
        let mut db = self.inner.write().await;
        let key = saga.id.to_string();
        if let Some(existing) = db.sagas.get(&key) {
            if existing.version == saga.version.saturating_sub(1) {
                db.sagas.insert(key, saga);
                persist(KEY_SAGAS, &db.sagas);
                return Ok(true);
            }
            return Ok(false);
        }
        Ok(false)
    }

    async fn delete_saga(&self, id: &uuid::Uuid) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.sagas.remove(&id.to_string());
        persist(KEY_SAGAS, &db.sagas);
        Ok(())
    }

    async fn get_incomplete_sagas(&self) -> Result<Vec<WalletSaga>, Error> {
        let db = self.inner.read().await;
        Ok(db.sagas.values().cloned().collect())
    }

    async fn reserve_proofs(
        &self,
        ys: Vec<PublicKey>,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.proof_reservations.insert(*operation_id, ys);
        persist(KEY_PROOF_RESERVATIONS, &db.proof_reservations);
        Ok(())
    }

    async fn release_proofs(&self, operation_id: &uuid::Uuid) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.proof_reservations.remove(operation_id);
        persist(KEY_PROOF_RESERVATIONS, &db.proof_reservations);
        Ok(())
    }

    async fn get_reserved_proofs(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<ProofInfo>, Error> {
        let db = self.inner.read().await;
        if let Some(ys) = db.proof_reservations.get(operation_id) {
            Ok(db
                .proofs
                .iter()
                .filter(|p| ys.contains(&p.y))
                .cloned()
                .collect())
        } else {
            Ok(vec![])
        }
    }

    async fn reserve_melt_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.melt_quote_reservations
            .insert(*operation_id, quote_id.to_string());
        persist(KEY_MELT_QUOTE_RESERVATIONS, &db.melt_quote_reservations);
        Ok(())
    }

    async fn release_melt_quote(&self, operation_id: &uuid::Uuid) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.melt_quote_reservations.remove(operation_id);
        persist(KEY_MELT_QUOTE_RESERVATIONS, &db.melt_quote_reservations);
        Ok(())
    }

    async fn reserve_mint_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.mint_quote_reservations
            .insert(*operation_id, quote_id.to_string());
        persist(KEY_MINT_QUOTE_RESERVATIONS, &db.mint_quote_reservations);
        Ok(())
    }

    async fn release_mint_quote(&self, operation_id: &uuid::Uuid) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.mint_quote_reservations.remove(operation_id);
        persist(KEY_MINT_QUOTE_RESERVATIONS, &db.mint_quote_reservations);
        Ok(())
    }

    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        let db = self.inner.read().await;
        Ok(db
            .kv
            .get(&kv_key(primary_namespace, secondary_namespace, key))
            .cloned())
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Error> {
        let db = self.inner.read().await;
        let prefix = format!("{}\0{}\0", primary_namespace, secondary_namespace);
        Ok(db
            .kv
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix).map(|s| s.to_string()))
            .collect())
    }

    async fn kv_write(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.kv.insert(
            kv_key(primary_namespace, secondary_namespace, key),
            value.to_vec(),
        );
        persist(KEY_KV, &db.kv);
        Ok(())
    }

    async fn kv_remove(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.kv
            .remove(&kv_key(primary_namespace, secondary_namespace, key));
        persist(KEY_KV, &db.kv);
        Ok(())
    }
}
