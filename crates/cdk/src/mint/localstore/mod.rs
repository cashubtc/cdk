pub mod memory;
#[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
pub mod redb_store;

use std::collections::HashMap;
use std::num::ParseIntError;

use async_trait::async_trait;
pub use memory::MemoryLocalStore;
#[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
pub use redb_store::RedbLocalStore;
use thiserror::Error;

use crate::nuts::nut02::mint::KeySet;
use crate::nuts::{BlindSignature, CurrencyUnit, Id, MintInfo, Proof, PublicKey};
use crate::secret::Secret;
use crate::types::{MeltQuote, MintQuote};

#[derive(Debug, Error)]
pub enum Error {
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error(transparent)]
    Redb(#[from] redb::Error),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error(transparent)]
    Database(#[from] redb::DatabaseError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error(transparent)]
    Transaction(#[from] redb::TransactionError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error(transparent)]
    Commit(#[from] redb::CommitError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error(transparent)]
    Table(#[from] redb::TableError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error(transparent)]
    Storage(#[from] redb::StorageError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "redb"))]
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    #[error(transparent)]
    ParseInt(#[from] ParseIntError),
    #[error("Unknown Mint Info")]
    UnknownMintInfo,
    #[error(transparent)]
    Cashu(#[from] crate::error::Error),
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    #[error(transparent)]
    CashuNut02(#[from] crate::nuts::nut02::Error),
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
}

#[async_trait]
pub trait LocalStore {
    async fn set_mint_info(&self, mint_info: &MintInfo) -> Result<(), Error>;
    async fn get_mint_info(&self) -> Result<MintInfo, Error>;

    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Error>;
    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Error>;
    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Error>;

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error>;
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Error>;
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Error>;
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error>;

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Error>;
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Error>;
    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error>;
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error>;

    async fn add_keyset(&self, keyset: KeySet) -> Result<(), Error>;
    async fn get_keyset(&self, id: &Id) -> Result<Option<KeySet>, Error>;
    async fn get_keysets(&self) -> Result<Vec<KeySet>, Error>;

    async fn add_spent_proof(&self, proof: Proof) -> Result<(), Error>;
    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Error>;
    async fn get_spent_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Error>;

    async fn add_pending_proof(&self, proof: Proof) -> Result<(), Error>;
    async fn get_pending_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Error>;
    async fn get_pending_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Error>;
    async fn remove_pending_proof(&self, secret: &Secret) -> Result<(), Error>;

    async fn add_blinded_signature(
        &self,
        blinded_message: PublicKey,
        blinded_signature: BlindSignature,
    ) -> Result<(), Error>;
    async fn get_blinded_signature(
        &self,
        blinded_message: &PublicKey,
    ) -> Result<Option<BlindSignature>, Error>;
    async fn get_blinded_signatures(
        &self,
        blinded_messages: Vec<PublicKey>,
    ) -> Result<Vec<Option<BlindSignature>>, Error>;
}
