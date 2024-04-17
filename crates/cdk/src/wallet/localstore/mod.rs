use std::collections::HashMap;
use std::num::ParseIntError;

use async_trait::async_trait;
use thiserror::Error;

mod memory;
#[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
mod redb_store;

pub use self::memory::MemoryLocalStore;
#[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
pub use self::redb_store::RedbLocalStore;
use crate::nuts::{Id, KeySetInfo, Keys, MintInfo, Proofs};
use crate::types::{MeltQuote, MintQuote};
use crate::url::UncheckedUrl;

#[derive(Debug, Error)]
pub enum Error {
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error("`{0}`")]
    Redb(#[from] redb::Error),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error("`{0}`")]
    Database(#[from] redb::DatabaseError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error("`{0}`")]
    Transaction(#[from] redb::TransactionError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error("`{0}`")]
    Commit(#[from] redb::CommitError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error("`{0}`")]
    Table(#[from] redb::TableError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error("`{0}`")]
    Storage(#[from] redb::StorageError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error("`{0}`")]
    Serde(#[from] serde_json::Error),
    #[error("`{0}`")]
    ParseInt(#[from] ParseIntError),
}

#[async_trait]
pub trait LocalStore {
    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Error>;
    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Error>;
    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Error>;

    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error>;
    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Error>;

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error>;
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Error>;
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error>;

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Error>;
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Error>;
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error>;

    async fn add_keys(&self, keys: Keys) -> Result<(), Error>;
    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Error>;
    async fn remove_keys(&self, id: &Id) -> Result<(), Error>;

    async fn add_proofs(&self, mint_url: UncheckedUrl, proof: Proofs) -> Result<(), Error>;
    async fn get_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Error>;
    async fn remove_proofs(&self, mint_url: UncheckedUrl, proofs: &Proofs) -> Result<(), Error>;

    async fn add_pending_proofs(&self, mint_url: UncheckedUrl, proof: Proofs) -> Result<(), Error>;
    async fn get_pending_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Error>;
    async fn remove_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: &Proofs,
    ) -> Result<(), Error>;

    #[cfg(feature = "nut13")]
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u64) -> Result<(), Error>;
    #[cfg(feature = "nut13")]
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u64>, Error>;
}
