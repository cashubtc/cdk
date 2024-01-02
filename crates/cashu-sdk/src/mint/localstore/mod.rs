mod memory;

use async_trait::async_trait;
use cashu::nuts::nut02::mint::KeySet;
use cashu::nuts::{Id, Proof};
use cashu::secret::Secret;
use cashu::types::{MeltQuote, MintQuote};
use thiserror::Error;

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
}

#[async_trait(?Send)]
pub trait LocalStore {
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error>;
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Error>;
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error>;

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Error>;
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Error>;
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error>;

    async fn add_keyset(&self, keyset: KeySet) -> Result<(), Error>;
    async fn get_keyset(&self, id: &Id) -> Result<Option<KeySet>, Error>;

    async fn add_spent_proof(&self, secret: Secret, proof: Proof) -> Result<(), Error>;
    async fn get_spent_proof(&self, secret: &Secret) -> Result<Option<Proof>, Error>;

    async fn add_pending_proof(&self, secret: Secret, proof: Proof) -> Result<(), Error>;
    async fn get_pending_proof(&self, secret: &Secret) -> Result<Option<Proof>, Error>;
    async fn remove_pending_proof(&self, secret: &Secret) -> Result<(), Error>;
}
