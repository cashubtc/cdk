mod memory;

#[cfg(not(target_arch = "wasm32"))]
pub mod redb_store;

use async_trait::async_trait;
use cashu::nuts::{Id, KeySetInfo, Keys, MintInfo, Proofs};
use cashu::types::{MeltQuote, MintQuote};
use cashu::url::UncheckedUrl;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("`{0}`")]
    Redb(#[from] redb::Error),
    #[error("`{0}`")]
    Database(#[from] redb::DatabaseError),
    #[error("`{0}`")]
    Transaction(#[from] redb::TransactionError),
    #[error("`{0}`")]
    Commit(#[from] redb::CommitError),
    #[error("`{0}`")]
    Table(#[from] redb::TableError),
    #[error("`{0}`")]
    Storage(#[from] redb::StorageError),
    #[error("`{0}`")]
    Serde(#[from] serde_json::Error),
}

#[async_trait(?Send)]
pub trait LocalStore {
    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Error>;
    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Error>;

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
}
