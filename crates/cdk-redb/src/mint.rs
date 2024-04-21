use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk::cdk_database::MintDatabase;
use cdk::dhke::hash_to_curve;
use cdk::nuts::{
    BlindSignature, CurrencyUnit, Id, MintInfo, MintKeySet as KeySet, Proof, PublicKey,
};
use cdk::secret::Secret;
use cdk::types::{MeltQuote, MintQuote};
use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::Mutex;
use tracing::debug;

use super::error::Error;

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
    type Err = Error;

    async fn set_mint_info(&self, mint_info: &MintInfo) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(CONFIG_TABLE)?;
            table.insert("mint_info", serde_json::to_string(mint_info)?.as_str())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(CONFIG_TABLE)?;

        let mint_info = table.get("mint_info")?.ok_or(Error::UnknownMintInfo)?;

        Ok(serde_json::from_str(mint_info.value())?)
    }

    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(ACTIVE_KEYSETS_TABLE)?;
            table.insert(unit.to_string().as_str(), id.to_string().as_str())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(ACTIVE_KEYSETS_TABLE)?;

        if let Some(id) = table.get(unit.to_string().as_str())? {
            return Ok(Some(Id::from_str(id.value())?));
        }

        Ok(None)
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(ACTIVE_KEYSETS_TABLE)?;

        let mut active_keysets = HashMap::new();

        for (unit, id) in (table.iter()?).flatten() {
            let unit = CurrencyUnit::from(unit.value());
            let id = Id::from_str(id.value())?;

            active_keysets.insert(unit, id);
        }

        Ok(active_keysets)
    }

    async fn add_keyset(&self, keyset: KeySet) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(KEYSETS_TABLE)?;
            table.insert(
                Id::from(keyset.clone()).to_string().as_str(),
                serde_json::to_string(&keyset)?.as_str(),
            )?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_keyset(&self, keyset_id: &Id) -> Result<Option<KeySet>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(KEYSETS_TABLE)?;

        match table.get(keyset_id.to_string().as_str())? {
            Some(keyset) => Ok(serde_json::from_str(keyset.value())?),
            None => Ok(None),
        }
    }

    async fn get_keysets(&self) -> Result<Vec<KeySet>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(KEYSETS_TABLE)?;

        let mut keysets = Vec::new();

        for (_id, keyset) in (table.iter()?).flatten() {
            let keyset = serde_json::from_str(keyset.value())?;

            keysets.push(keyset)
        }

        Ok(keysets)
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MINT_QUOTES_TABLE)?;
            table.insert(quote.id.as_str(), serde_json::to_string(&quote)?.as_str())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MINT_QUOTES_TABLE)?;

        match table.get(quote_id)? {
            Some(quote) => Ok(serde_json::from_str(quote.value())?),
            None => Ok(None),
        }
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MINT_QUOTES_TABLE)?;

        let mut quotes = Vec::new();

        for (_id, quote) in (table.iter()?).flatten() {
            let quote = serde_json::from_str(quote.value())?;

            quotes.push(quote)
        }

        Ok(quotes)
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MINT_QUOTES_TABLE)?;
            table.remove(quote_id)?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MELT_QUOTES_TABLE)?;
            table.insert(quote.id.as_str(), serde_json::to_string(&quote)?.as_str())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MELT_QUOTES_TABLE)?;

        let quote = table.get(quote_id)?;

        Ok(quote.map(|q| serde_json::from_str(q.value()).unwrap()))
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MELT_QUOTES_TABLE)?;

        let mut quotes = Vec::new();

        for (_id, quote) in (table.iter()?).flatten() {
            let quote = serde_json::from_str(quote.value())?;

            quotes.push(quote)
        }

        Ok(quotes)
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MELT_QUOTES_TABLE)?;
            table.remove(quote_id)?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn add_spent_proof(&self, proof: Proof) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(SPENT_PROOFS_TABLE)?;
            let y: PublicKey = hash_to_curve(&proof.secret.to_bytes())?;
            table.insert(y.to_bytes(), serde_json::to_string(&proof)?.as_str())?;
        }
        write_txn.commit()?;
        debug!("Added spend secret: {}", proof.secret.to_string());

        Ok(())
    }

    async fn get_spent_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(SPENT_PROOFS_TABLE)?;

        match table.get(y.to_bytes())? {
            Some(proof) => Ok(serde_json::from_str(proof.value())?),
            None => Ok(None),
        }
    }

    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(SPENT_PROOFS_TABLE)?;

        let y: PublicKey = hash_to_curve(&secret.to_bytes())?;

        match table.get(y.to_bytes())? {
            Some(proof) => Ok(serde_json::from_str(proof.value())?),
            None => Ok(None),
        }
    }

    async fn add_pending_proof(&self, proof: Proof) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(PENDING_PROOFS_TABLE)?;
            table.insert(
                hash_to_curve(&proof.secret.to_bytes())?.to_bytes(),
                serde_json::to_string(&proof)?.as_str(),
            )?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_pending_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(PENDING_PROOFS_TABLE)?;

        match table.get(y.to_bytes())? {
            Some(proof) => Ok(serde_json::from_str(proof.value())?),
            None => Ok(None),
        }
    }

    async fn get_pending_proof_by_secret(
        &self,
        secret: &Secret,
    ) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(PENDING_PROOFS_TABLE)?;

        let secret_hash = hash_to_curve(&secret.to_bytes())?;

        match table.get(secret_hash.to_bytes())? {
            Some(proof) => Ok(serde_json::from_str(proof.value())?),
            None => Ok(None),
        }
    }

    async fn remove_pending_proof(&self, secret: &Secret) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(PENDING_PROOFS_TABLE)?;
            let secret_hash = hash_to_curve(&secret.to_bytes())?;
            table.remove(secret_hash.to_bytes())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn add_blinded_signature(
        &self,
        blinded_message: PublicKey,
        blinded_signature: BlindSignature,
    ) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(BLINDED_SIGNATURES)?;
            table.insert(
                blinded_message.to_bytes(),
                serde_json::to_string(&blinded_signature)?.as_str(),
            )?;
        }

        write_txn.commit()?;

        Ok(())
    }

    async fn get_blinded_signature(
        &self,
        blinded_message: &PublicKey,
    ) -> Result<Option<BlindSignature>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(BLINDED_SIGNATURES)?;

        match table.get(blinded_message.to_bytes())? {
            Some(blind_signature) => Ok(serde_json::from_str(blind_signature.value())?),
            None => Ok(None),
        }
    }

    async fn get_blinded_signatures(
        &self,
        blinded_messages: Vec<PublicKey>,
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(BLINDED_SIGNATURES)?;

        let mut signatures = Vec::with_capacity(blinded_messages.len());

        for blinded_message in blinded_messages {
            match table.get(blinded_message.to_bytes())? {
                Some(blind_signature) => {
                    signatures.push(Some(serde_json::from_str(blind_signature.value())?))
                }
                None => signatures.push(None),
            }
        }

        Ok(signatures)
    }
}
