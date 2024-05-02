use std::collections::HashMap;
use std::num::ParseIntError;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk::cdk_database::{self, MintDatabase};
use cdk::dhke::hash_to_curve;
use cdk::mint::MintKeySetInfo;
use cdk::nuts::{BlindSignature, CurrencyUnit, Id, MintInfo, Proof, PublicKey};
use cdk::secret::Secret;
use cdk::types::{MeltQuote, MintQuote};
use redb::{Database, ReadableTable, TableDefinition};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::debug;

const ACTIVE_KEYSETS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("active_keysets");
const KEYSETS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("keysets");
const MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");
const MELT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("melt_quotes");
const PENDING_PROOFS_TABLE: TableDefinition<[u8; 33], &str> =
    TableDefinition::new("pending_proofs");
const SPENT_PROOFS_TABLE: TableDefinition<[u8; 33], &str> = TableDefinition::new("spent_proofs");
const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
// Key is hex blinded_message B_ value is blinded_signature
const BLINDED_SIGNATURES: TableDefinition<[u8; 33], &str> =
    TableDefinition::new("blinded_signatures");

const DATABASE_VERSION: u64 = 0;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Redb(#[from] redb::Error),
    #[error(transparent)]
    Database(#[from] redb::DatabaseError),
    #[error(transparent)]
    Transaction(#[from] redb::TransactionError),
    #[error(transparent)]
    Commit(#[from] redb::CommitError),
    #[error(transparent)]
    Table(#[from] redb::TableError),
    #[error(transparent)]
    Storage(#[from] redb::StorageError),
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    #[error(transparent)]
    ParseInt(#[from] ParseIntError),
    #[error(transparent)]
    CDKDatabase(#[from] cdk_database::Error),
    #[error(transparent)]
    CDK(#[from] cdk::error::Error),
    #[error(transparent)]
    CDKNUT02(#[from] cdk::nuts::nut02::Error),
    #[error(transparent)]
    CDKNUT00(#[from] cdk::nuts::nut00::Error),
    #[error("Unknown Mint Info")]
    UnknownMintInfo,
}

impl From<Error> for cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}

#[derive(Debug, Clone)]
pub struct MintRedbDatabase {
    db: Arc<Mutex<Database>>,
}

impl MintRedbDatabase {
    pub fn new(path: &str) -> Result<Self, Error> {
        let db = Database::create(path)?;

        let write_txn = db.begin_write()?;
        // Check database version
        {
            let _ = write_txn.open_table(CONFIG_TABLE)?;
            let mut table = write_txn.open_table(CONFIG_TABLE)?;

            let db_version = table.get("db_version")?;
            let db_version = db_version.map(|v| v.value().to_owned());

            match db_version {
                Some(db_version) => {
                    let current_file_version = u64::from_str(&db_version)?;
                    if current_file_version.ne(&DATABASE_VERSION) {
                        // Database needs to be upgraded
                        todo!()
                    }
                }
                None => {
                    // Open all tables to init a new db
                    let _ = write_txn.open_table(ACTIVE_KEYSETS_TABLE)?;
                    let _ = write_txn.open_table(KEYSETS_TABLE)?;
                    let _ = write_txn.open_table(MINT_QUOTES_TABLE)?;
                    let _ = write_txn.open_table(MELT_QUOTES_TABLE)?;
                    let _ = write_txn.open_table(PENDING_PROOFS_TABLE)?;
                    let _ = write_txn.open_table(SPENT_PROOFS_TABLE)?;
                    let _ = write_txn.open_table(BLINDED_SIGNATURES)?;

                    table.insert("db_version", "0")?;
                }
            }
        }

        write_txn.commit()?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
        })
    }
}

#[async_trait]
impl MintDatabase for MintRedbDatabase {
    type Err = cdk_database::Error;

    async fn set_mint_info(&self, mint_info: &MintInfo) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(CONFIG_TABLE).map_err(Error::from)?;
            table
                .insert(
                    "mint_info",
                    serde_json::to_string(mint_info)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(CONFIG_TABLE).map_err(Error::from)?;

        let mint_info = table
            .get("mint_info")
            .map_err(Error::from)?
            .ok_or(Error::UnknownMintInfo)?;

        Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?)
    }

    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(ACTIVE_KEYSETS_TABLE)
                .map_err(Error::from)?;
            table
                .insert(unit.to_string().as_str(), id.to_string().as_str())
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(ACTIVE_KEYSETS_TABLE)
            .map_err(Error::from)?;

        if let Some(id) = table.get(unit.to_string().as_str()).map_err(Error::from)? {
            return Ok(Some(Id::from_str(id.value()).map_err(Error::from)?));
        }

        Ok(None)
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(ACTIVE_KEYSETS_TABLE)
            .map_err(Error::from)?;

        let mut active_keysets = HashMap::new();

        for (unit, id) in (table.iter().map_err(Error::from)?).flatten() {
            let unit = CurrencyUnit::from(unit.value());
            let id = Id::from_str(id.value()).map_err(Error::from)?;

            active_keysets.insert(unit, id);
        }

        Ok(active_keysets)
    }

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;
            table
                .insert(
                    keyset.id.to_string().as_str(),
                    serde_json::to_string(&keyset)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_keyset_info(&self, keyset_id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

        match table
            .get(keyset_id.to_string().as_str())
            .map_err(Error::from)?
        {
            Some(keyset) => Ok(serde_json::from_str(keyset.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

        let mut keysets = Vec::new();

        for (_id, keyset) in (table.iter().map_err(Error::from)?).flatten() {
            let keyset = serde_json::from_str(keyset.value()).map_err(Error::from)?;

            keysets.push(keyset)
        }

        Ok(keysets)
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    quote.id.as_str(),
                    serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        match table.get(quote_id).map_err(Error::from)? {
            Some(quote) => Ok(serde_json::from_str(quote.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        let mut quotes = Vec::new();

        for (_id, quote) in (table.iter().map_err(Error::from)?).flatten() {
            let quote = serde_json::from_str(quote.value()).map_err(Error::from)?;

            quotes.push(quote)
        }

        Ok(quotes)
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table.remove(quote_id).map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    quote.id.as_str(),
                    serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MELT_QUOTES_TABLE)
            .map_err(Error::from)?;

        let quote = table.get(quote_id).map_err(Error::from)?;

        Ok(quote.map(|q| serde_json::from_str(q.value()).unwrap()))
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MELT_QUOTES_TABLE)
            .map_err(Error::from)?;

        let mut quotes = Vec::new();

        for (_id, quote) in (table.iter().map_err(Error::from)?).flatten() {
            let quote = serde_json::from_str(quote.value()).map_err(Error::from)?;

            quotes.push(quote)
        }

        Ok(quotes)
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table.remove(quote_id).map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_spent_proof(&self, proof: Proof) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(SPENT_PROOFS_TABLE)
                .map_err(Error::from)?;
            let y: PublicKey = hash_to_curve(&proof.secret.to_bytes()).map_err(Error::from)?;
            table
                .insert(
                    y.to_bytes(),
                    serde_json::to_string(&proof).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        debug!("Added spend secret: {}", proof.secret.to_string());

        Ok(())
    }

    async fn get_spent_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(SPENT_PROOFS_TABLE)
            .map_err(Error::from)?;

        match table.get(y.to_bytes()).map_err(Error::from)? {
            Some(proof) => Ok(serde_json::from_str(proof.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(SPENT_PROOFS_TABLE)
            .map_err(Error::from)?;

        let y: PublicKey = hash_to_curve(&secret.to_bytes())?;

        match table.get(y.to_bytes()).map_err(Error::from)? {
            Some(proof) => Ok(serde_json::from_str(proof.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn add_pending_proof(&self, proof: Proof) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(PENDING_PROOFS_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    hash_to_curve(&proof.secret.to_bytes())?.to_bytes(),
                    serde_json::to_string(&proof).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_pending_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(PENDING_PROOFS_TABLE)
            .map_err(Error::from)?;

        match table.get(y.to_bytes()).map_err(Error::from)? {
            Some(proof) => Ok(serde_json::from_str(proof.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn get_pending_proof_by_secret(
        &self,
        secret: &Secret,
    ) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(PENDING_PROOFS_TABLE)
            .map_err(Error::from)?;

        let secret_hash = hash_to_curve(&secret.to_bytes())?;

        match table.get(secret_hash.to_bytes()).map_err(Error::from)? {
            Some(proof) => Ok(serde_json::from_str(proof.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn remove_pending_proof(&self, secret: &Secret) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(PENDING_PROOFS_TABLE)
                .map_err(Error::from)?;
            let secret_hash = hash_to_curve(&secret.to_bytes()).map_err(Error::from)?;
            table.remove(secret_hash.to_bytes()).map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_blinded_signature(
        &self,
        blinded_message: PublicKey,
        blinded_signature: BlindSignature,
    ) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(BLINDED_SIGNATURES)
                .map_err(Error::from)?;
            table
                .insert(
                    blinded_message.to_bytes(),
                    serde_json::to_string(&blinded_signature)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_blinded_signature(
        &self,
        blinded_message: &PublicKey,
    ) -> Result<Option<BlindSignature>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(BLINDED_SIGNATURES)
            .map_err(Error::from)?;

        match table.get(blinded_message.to_bytes()).map_err(Error::from)? {
            Some(blind_signature) => {
                Ok(serde_json::from_str(blind_signature.value()).map_err(Error::from)?)
            }
            None => Ok(None),
        }
    }

    async fn get_blinded_signatures(
        &self,
        blinded_messages: Vec<PublicKey>,
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(BLINDED_SIGNATURES)
            .map_err(Error::from)?;

        let mut signatures = Vec::with_capacity(blinded_messages.len());

        for blinded_message in blinded_messages {
            match table.get(blinded_message.to_bytes()).map_err(Error::from)? {
                Some(blind_signature) => signatures.push(Some(
                    serde_json::from_str(blind_signature.value()).map_err(Error::from)?,
                )),
                None => signatures.push(None),
            }
        }

        Ok(signatures)
    }
}
