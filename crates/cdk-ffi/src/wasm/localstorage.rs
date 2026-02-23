//! LocalStorage-based WalletDatabase for WASM
//!
//! Uses the browser's `localStorage` API to persist wallet data as JSON.
//! Each record type uses a key prefix for namespacing.

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Mutex;

use async_trait::async_trait;
use cdk_common::database::{self, Error};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{
    CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use cdk_common::wallet::{
    self, MintQuote as WalletMintQuote, ProofInfo, Transaction, TransactionDirection,
    TransactionId, WalletSaga,
};

/// Key prefixes for localStorage namespacing
const PREFIX_MINT: &str = "cdk:mint:";
const PREFIX_KEYSET_BY_MINT: &str = "cdk:keysets_by_mint:";
const PREFIX_KEYSET: &str = "cdk:keyset:";
const PREFIX_KEYS: &str = "cdk:keys:";
const PREFIX_MINT_QUOTE: &str = "cdk:mint_quote:";
const PREFIX_MELT_QUOTE: &str = "cdk:melt_quote:";
const PREFIX_PROOF: &str = "cdk:proof:";
const PREFIX_TRANSACTION: &str = "cdk:tx:";
const PREFIX_KEYSET_COUNTER: &str = "cdk:counter:";
const PREFIX_SAGA: &str = "cdk:saga:";
const PREFIX_RESERVE_PROOFS: &str = "cdk:reserve_proofs:";
const PREFIX_RESERVE_MELT: &str = "cdk:reserve_melt:";
const PREFIX_RESERVE_MINT: &str = "cdk:reserve_mint:";
const PREFIX_KV: &str = "cdk:kv:";

/// Get the browser's localStorage
fn get_storage() -> Result<web_sys::Storage, Error> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .ok_or_else(|| Error::Database("localStorage not available".to_string().into()))
}

/// Helper to read and deserialize a JSON value from localStorage
fn read_json<T: serde::de::DeserializeOwned>(
    storage: &web_sys::Storage,
    key: &str,
) -> Result<Option<T>, Error> {
    match storage.get_item(key) {
        Ok(Some(json)) => {
            let val = serde_json::from_str(&json)
                .map_err(|e| Error::Database(format!("deserialize {key}: {e}").into()))?;
            Ok(Some(val))
        }
        Ok(None) => Ok(None),
        Err(_) => Err(Error::Database(format!("read {key}").into())),
    }
}

/// Helper to serialize and write a JSON value to localStorage
fn write_json<T: serde::Serialize>(
    storage: &web_sys::Storage,
    key: &str,
    value: &T,
) -> Result<(), Error> {
    let json = serde_json::to_string(value)
        .map_err(|e| Error::Database(format!("serialize {key}: {e}").into()))?;
    storage
        .set_item(key, &json)
        .map_err(|_| Error::Database(format!("write {key}").into()))
}

/// Collect all localStorage keys that start with a given prefix
fn keys_with_prefix(storage: &web_sys::Storage, prefix: &str) -> Result<Vec<String>, Error> {
    let len = storage
        .length()
        .map_err(|_| Error::Database("get length".to_string().into()))?;
    let mut result = Vec::new();
    for i in 0..len {
        if let Ok(Some(key)) = storage.key(i) {
            if key.starts_with(prefix) {
                result.push(key);
            }
        }
    }
    Ok(result)
}

/// Read all values whose keys start with a given prefix
fn read_all_with_prefix<T: serde::de::DeserializeOwned>(
    storage: &web_sys::Storage,
    prefix: &str,
) -> Result<Vec<T>, Error> {
    let keys = keys_with_prefix(storage, prefix)?;
    let mut items = Vec::new();
    for key in keys {
        if let Some(val) = read_json::<T>(storage, &key)? {
            items.push(val);
        }
    }
    Ok(items)
}

/// LocalStorage-backed wallet database for WASM
///
/// All data is stored as JSON strings in the browser's localStorage.
/// A `Mutex` is used for keyset counter atomicity (single-threaded in WASM).
pub struct LocalStorageDatabase {
    /// Mutex for keyset counter operations
    counter_lock: Mutex<()>,
}

impl Debug for LocalStorageDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalStorageDatabase").finish()
    }
}

impl LocalStorageDatabase {
    /// Create a new LocalStorageDatabase
    pub fn new() -> Result<Self, Error> {
        // Verify localStorage is available
        let _ = get_storage()?;
        Ok(Self {
            counter_lock: Mutex::new(()),
        })
    }
}

impl Default for LocalStorageDatabase {
    fn default() -> Self {
        Self {
            counter_lock: Mutex::new(()),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl database::WalletDatabase<Error> for LocalStorageDatabase {
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_MINT}{mint_url}");
        read_json(&storage, &key)
    }

    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Error> {
        let storage = get_storage()?;
        let keys = keys_with_prefix(&storage, PREFIX_MINT)?;
        let mut result = HashMap::new();
        for key in keys {
            let url_str = &key[PREFIX_MINT.len()..];
            if let Ok(url) = url_str.parse::<MintUrl>() {
                let info = read_json::<MintInfo>(&storage, &key)?;
                result.insert(url, info);
            }
        }
        Ok(result)
    }

    async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<Option<Vec<KeySetInfo>>, Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_KEYSET_BY_MINT}{mint_url}");
        let ids: Option<Vec<String>> = read_json(&storage, &key)?;
        match ids {
            Some(ids) => {
                let mut keysets = Vec::new();
                for id in ids {
                    let ks_key = format!("{PREFIX_KEYSET}{id}");
                    if let Some(ks) = read_json::<KeySetInfo>(&storage, &ks_key)? {
                        keysets.push(ks);
                    }
                }
                Ok(Some(keysets))
            }
            None => Ok(None),
        }
    }

    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_KEYSET}{keyset_id}");
        read_json(&storage, &key)
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<WalletMintQuote>, Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_MINT_QUOTE}{quote_id}");
        read_json(&storage, &key)
    }

    async fn get_mint_quotes(&self) -> Result<Vec<WalletMintQuote>, Error> {
        let storage = get_storage()?;
        read_all_with_prefix(&storage, PREFIX_MINT_QUOTE)
    }

    async fn get_unissued_mint_quotes(&self) -> Result<Vec<WalletMintQuote>, Error> {
        let quotes = self.get_mint_quotes().await?;
        Ok(quotes
            .into_iter()
            .filter(|q| {
                // Include bolt11 quotes with amount_issued = 0 and all bolt12/custom quotes
                if q.payment_method == cdk_common::nuts::PaymentMethod::BOLT11 {
                    q.amount_issued == 0.into()
                } else {
                    true
                }
            })
            .collect())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_MELT_QUOTE}{quote_id}");
        read_json(&storage, &key)
    }

    async fn get_melt_quotes(&self) -> Result<Vec<wallet::MeltQuote>, Error> {
        let storage = get_storage()?;
        read_all_with_prefix(&storage, PREFIX_MELT_QUOTE)
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_KEYS}{id}");
        read_json(&storage, &key)
    }

    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Error> {
        let storage = get_storage()?;
        let all_proofs: Vec<ProofInfo> = read_all_with_prefix(&storage, PREFIX_PROOF)?;
        Ok(all_proofs
            .into_iter()
            .filter(|p| {
                if let Some(ref url) = mint_url {
                    if &p.mint_url != url {
                        return false;
                    }
                }
                if let Some(ref u) = unit {
                    if &p.unit != u {
                        return false;
                    }
                }
                if let Some(ref states) = state {
                    if !states.contains(&p.state) {
                        return false;
                    }
                }
                if let Some(ref conditions) = spending_conditions {
                    match &p.spending_condition {
                        Some(pc) => {
                            if !conditions.contains(pc) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
                true
            })
            .collect())
    }

    async fn get_proofs_by_ys(&self, ys: Vec<PublicKey>) -> Result<Vec<ProofInfo>, Error> {
        let storage = get_storage()?;
        let mut result = Vec::new();
        for y in ys {
            let key = format!("{PREFIX_PROOF}{y}");
            if let Some(proof) = read_json::<ProofInfo>(&storage, &key)? {
                result.push(proof);
            }
        }
        Ok(result)
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
        let storage = get_storage()?;
        let key = format!("{PREFIX_TRANSACTION}{transaction_id}");
        read_json(&storage, &key)
    }

    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, Error> {
        let storage = get_storage()?;
        let all_txs: Vec<Transaction> = read_all_with_prefix(&storage, PREFIX_TRANSACTION)?;
        Ok(all_txs
            .into_iter()
            .filter(|tx| {
                if let Some(ref url) = mint_url {
                    if &tx.mint_url != url {
                        return false;
                    }
                }
                if let Some(ref dir) = direction {
                    if &tx.direction != dir {
                        return false;
                    }
                }
                if let Some(ref u) = unit {
                    if &tx.unit != u {
                        return false;
                    }
                }
                true
            })
            .collect())
    }

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Error> {
        let storage = get_storage()?;
        for proof in added {
            let key = format!("{PREFIX_PROOF}{}", proof.y);
            write_json(&storage, &key, &proof)?;
        }
        for y in removed_ys {
            let key = format!("{PREFIX_PROOF}{y}");
            let _ = storage.remove_item(&key);
        }
        Ok(())
    }

    async fn update_proofs_state(&self, ys: Vec<PublicKey>, state: State) -> Result<(), Error> {
        let storage = get_storage()?;
        for y in ys {
            let key = format!("{PREFIX_PROOF}{y}");
            if let Some(mut proof) = read_json::<ProofInfo>(&storage, &key)? {
                proof.state = state;
                write_json(&storage, &key, &proof)?;
            }
        }
        Ok(())
    }

    async fn add_transaction(&self, transaction: Transaction) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_TRANSACTION}{}", transaction.id());
        write_json(&storage, &key, &transaction)
    }

    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Error> {
        let storage = get_storage()?;

        // Move mint info
        let old_key = format!("{PREFIX_MINT}{old_mint_url}");
        let new_key = format!("{PREFIX_MINT}{new_mint_url}");
        if let Some(info) = read_json::<MintInfo>(&storage, &old_key)? {
            write_json(&storage, &new_key, &info)?;
            let _ = storage.remove_item(&old_key);
        }

        // Move keysets
        let old_ks_key = format!("{PREFIX_KEYSET_BY_MINT}{old_mint_url}");
        let new_ks_key = format!("{PREFIX_KEYSET_BY_MINT}{new_mint_url}");
        if let Some(ids) = read_json::<Vec<String>>(&storage, &old_ks_key)? {
            write_json(&storage, &new_ks_key, &ids)?;
            let _ = storage.remove_item(&old_ks_key);
        }

        // Update mint_url in all proofs for this mint
        let all_proofs: Vec<ProofInfo> = read_all_with_prefix(&storage, PREFIX_PROOF)?;
        for mut proof in all_proofs {
            if proof.mint_url == old_mint_url {
                proof.mint_url = new_mint_url.clone();
                let key = format!("{PREFIX_PROOF}{}", proof.y);
                write_json(&storage, &key, &proof)?;
            }
        }

        Ok(())
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<u32, Error> {
        let _guard = self
            .counter_lock
            .lock()
            .map_err(|e| Error::Database(format!("counter lock: {e}").into()))?;

        let storage = get_storage()?;
        let key = format!("{PREFIX_KEYSET_COUNTER}{keyset_id}");
        let current: u32 = read_json(&storage, &key)?.unwrap_or(0);
        let new_val = current + count;
        write_json(&storage, &key, &new_val)?;
        Ok(current)
    }

    async fn add_mint(&self, mint_url: MintUrl, mint_info: Option<MintInfo>) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_MINT}{mint_url}");
        if let Some(info) = mint_info {
            write_json(&storage, &key, &info)
        } else {
            // Store empty marker so get_mints returns this URL
            storage
                .set_item(&key, "null")
                .map_err(|_| Error::Database(format!("write {key}").into()))
        }
    }

    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_MINT}{mint_url}");
        let _ = storage.remove_item(&key);
        let ks_key = format!("{PREFIX_KEYSET_BY_MINT}{mint_url}");
        let _ = storage.remove_item(&ks_key);
        Ok(())
    }

    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error> {
        let storage = get_storage()?;

        // Store each keyset individually
        let mut ids: Vec<String> = Vec::new();
        for ks in &keysets {
            let ks_key = format!("{PREFIX_KEYSET}{}", ks.id);
            write_json(&storage, &ks_key, ks)?;
            ids.push(ks.id.to_string());
        }

        // Update the mint -> keyset_ids index (merge with existing)
        let idx_key = format!("{PREFIX_KEYSET_BY_MINT}{mint_url}");
        let mut existing: Vec<String> = read_json(&storage, &idx_key)?.unwrap_or_default();
        for id in ids {
            if !existing.contains(&id) {
                existing.push(id);
            }
        }
        write_json(&storage, &idx_key, &existing)
    }

    async fn add_mint_quote(&self, quote: WalletMintQuote) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_MINT_QUOTE}{}", quote.id);
        write_json(&storage, &key, &quote)
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_MINT_QUOTE}{quote_id}");
        let _ = storage.remove_item(&key);
        Ok(())
    }

    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_MELT_QUOTE}{}", quote.id);
        write_json(&storage, &key, &quote)
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_MELT_QUOTE}{quote_id}");
        let _ = storage.remove_item(&key);
        Ok(())
    }

    async fn add_keys(&self, keyset: KeySet) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_KEYS}{}", keyset.id);
        write_json(&storage, &key, &keyset.keys)
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_KEYS}{id}");
        let _ = storage.remove_item(&key);
        Ok(())
    }

    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_TRANSACTION}{transaction_id}");
        let _ = storage.remove_item(&key);
        Ok(())
    }

    async fn add_saga(&self, saga: WalletSaga) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_SAGA}{}", saga.id);
        write_json(&storage, &key, &saga)
    }

    async fn get_saga(&self, id: &uuid::Uuid) -> Result<Option<WalletSaga>, Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_SAGA}{id}");
        read_json(&storage, &key)
    }

    async fn update_saga(&self, saga: WalletSaga) -> Result<bool, Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_SAGA}{}", saga.id);

        // Optimistic locking: read current version
        if let Some(current) = read_json::<WalletSaga>(&storage, &key)? {
            if current.version != saga.version.saturating_sub(1) {
                return Ok(false); // Version mismatch
            }
        }

        write_json(&storage, &key, &saga)?;
        Ok(true)
    }

    async fn delete_saga(&self, id: &uuid::Uuid) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_SAGA}{id}");
        let _ = storage.remove_item(&key);
        Ok(())
    }

    async fn get_incomplete_sagas(&self) -> Result<Vec<WalletSaga>, Error> {
        let storage = get_storage()?;
        let all: Vec<WalletSaga> = read_all_with_prefix(&storage, PREFIX_SAGA)?;
        // Return all sagas - the caller filters by state
        Ok(all)
    }

    async fn reserve_proofs(
        &self,
        ys: Vec<PublicKey>,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Error> {
        let storage = get_storage()?;

        // Store Y values for this operation
        let key = format!("{PREFIX_RESERVE_PROOFS}{operation_id}");
        let y_strings: Vec<String> = ys.iter().map(|y| y.to_string()).collect();
        write_json(&storage, &key, &y_strings)?;

        // Mark proofs as reserved
        self.update_proofs_state(ys, State::Reserved).await
    }

    async fn release_proofs(&self, operation_id: &uuid::Uuid) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_RESERVE_PROOFS}{operation_id}");

        if let Some(y_strings) = read_json::<Vec<String>>(&storage, &key)? {
            let ys: Vec<PublicKey> = y_strings.iter().filter_map(|s| s.parse().ok()).collect();
            self.update_proofs_state(ys, State::Unspent).await?;
            let _ = storage.remove_item(&key);
        }
        Ok(())
    }

    async fn get_reserved_proofs(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<ProofInfo>, Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_RESERVE_PROOFS}{operation_id}");

        match read_json::<Vec<String>>(&storage, &key)? {
            Some(y_strings) => {
                let ys: Vec<PublicKey> = y_strings.iter().filter_map(|s| s.parse().ok()).collect();
                self.get_proofs_by_ys(ys).await
            }
            None => Ok(Vec::new()),
        }
    }

    async fn reserve_melt_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_RESERVE_MELT}{operation_id}");
        write_json(&storage, &key, &quote_id.to_string())
    }

    async fn release_melt_quote(&self, operation_id: &uuid::Uuid) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_RESERVE_MELT}{operation_id}");
        let _ = storage.remove_item(&key);
        Ok(())
    }

    async fn reserve_mint_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_RESERVE_MINT}{operation_id}");
        write_json(&storage, &key, &quote_id.to_string())
    }

    async fn release_mint_quote(&self, operation_id: &uuid::Uuid) -> Result<(), Error> {
        let storage = get_storage()?;
        let key = format!("{PREFIX_RESERVE_MINT}{operation_id}");
        let _ = storage.remove_item(&key);
        Ok(())
    }

    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        let storage = get_storage()?;
        let storage_key = format!("{PREFIX_KV}{primary_namespace}:{secondary_namespace}:{key}");
        match read_json::<String>(&storage, &storage_key)? {
            Some(b64) => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&b64)
                    .map_err(|e| Error::Database(format!("base64 decode: {e}").into()))?;
                Ok(Some(bytes))
            }
            None => Ok(None),
        }
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Error> {
        let storage = get_storage()?;
        let prefix = format!("{PREFIX_KV}{primary_namespace}:{secondary_namespace}:");
        let keys = keys_with_prefix(&storage, &prefix)?;
        Ok(keys
            .into_iter()
            .map(|k| k[prefix.len()..].to_string())
            .collect())
    }

    async fn kv_write(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), Error> {
        let storage = get_storage()?;
        let storage_key = format!("{PREFIX_KV}{primary_namespace}:{secondary_namespace}:{key}");
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(value);
        write_json(&storage, &storage_key, &b64)
    }

    async fn kv_remove(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), Error> {
        let storage = get_storage()?;
        let storage_key = format!("{PREFIX_KV}{primary_namespace}:{secondary_namespace}:{key}");
        let _ = storage.remove_item(&storage_key);
        Ok(())
    }
}
