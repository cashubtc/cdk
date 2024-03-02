use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cashu::dhke::hash_to_curve;
use cashu::k256;
use cashu::nuts::{CurrencyUnit, Id, MintInfo, MintKeySet as KeySet, Proof};
use cashu::secret::Secret;
use cashu::types::{MeltQuote, MintQuote};
use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::Mutex;
use tracing::debug;

use super::{Error, LocalStore};

const ACTIVE_KEYSETS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("active_keysets");
const KEYSETS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("keysets");
const MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");
const MELT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("melt_quotes");
const PENDING_PROOFS_TABLE: TableDefinition<&[u8], &str> = TableDefinition::new("pending_proofs");
const SPENT_PROOFS_TABLE: TableDefinition<&[u8], &str> = TableDefinition::new("spent_proofs");
const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");

#[derive(Debug, Clone)]
pub struct RedbLocalStore {
    db: Arc<Mutex<Database>>,
}

impl RedbLocalStore {
    pub fn new(path: &str) -> Result<Self, Error> {
        let db = Database::create(path)?;

        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(ACTIVE_KEYSETS_TABLE)?;
            let _ = write_txn.open_table(KEYSETS_TABLE)?;
            let _ = write_txn.open_table(MINT_QUOTES_TABLE)?;
            let _ = write_txn.open_table(MELT_QUOTES_TABLE)?;
            let _ = write_txn.open_table(PENDING_PROOFS_TABLE)?;
            let _ = write_txn.open_table(SPENT_PROOFS_TABLE)?;
            let _ = write_txn.open_table(CONFIG_TABLE)?;
        }
        write_txn.commit()?;

        Ok(Self {
            db: Arc::new(Mutex::new(db)),
        })
    }
}

#[async_trait]
impl LocalStore for RedbLocalStore {
    async fn set_mint_info(&self, mint_info: &MintInfo) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(CONFIG_TABLE)?;
            table.insert("mint_info", serde_json::to_string(mint_info)?.as_str())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(CONFIG_TABLE)?;

        let mint_info = table.get("mint_info")?.ok_or(Error::UnknownMintInfo)?;

        Ok(serde_json::from_str(mint_info.value())?)
    }

    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(ACTIVE_KEYSETS_TABLE)?;
            table.insert(unit.to_string().as_str(), id.to_string().as_str())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(ACTIVE_KEYSETS_TABLE)?;

        if let Some(id) = table.get(unit.to_string().as_str())? {
            return Ok(Some(Id::from_str(id.value())?));
        }

        Ok(None)
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(ACTIVE_KEYSETS_TABLE)?;

        let mut active_keysets = HashMap::new();

        for (unit, id) in (table.iter()?).flatten() {
            let unit = CurrencyUnit::from_str(unit.value())?;
            let id = Id::from_str(id.value())?;

            active_keysets.insert(unit, id);
        }

        Ok(active_keysets)
    }

    async fn add_keyset(&self, keyset: KeySet) -> Result<(), Error> {
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

    async fn get_keyset(&self, keyset_id: &Id) -> Result<Option<KeySet>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(KEYSETS_TABLE)?;

        let keyset = table.get(keyset_id.to_string().as_str())?;

        if let Some(keyset) = keyset {
            Ok(serde_json::from_str(keyset.value())?)
        } else {
            Ok(None)
        }
    }

    async fn get_keysets(&self) -> Result<Vec<KeySet>, Error> {
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

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MINT_QUOTES_TABLE)?;
            table.insert(quote.id.as_str(), serde_json::to_string(&quote)?.as_str())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MINT_QUOTES_TABLE)?;

        let quote = table.get(quote_id)?;

        if let Some(quote) = quote {
            Ok(serde_json::from_str(quote.value())?)
        } else {
            Ok(None)
        }
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
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

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MINT_QUOTES_TABLE)?;
            table.remove(quote_id)?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MELT_QUOTES_TABLE)?;
            table.insert(quote.id.as_str(), serde_json::to_string(&quote)?.as_str())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MELT_QUOTES_TABLE)?;

        let quote = table.get(quote_id)?;

        Ok(quote.map(|q| serde_json::from_str(q.value()).unwrap()))
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
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

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MELT_QUOTES_TABLE)?;
            table.remove(quote_id)?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn add_spent_proof(&self, proof: Proof) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(SPENT_PROOFS_TABLE)?;
            table.insert(
                hash_to_curve(&proof.secret.to_bytes())?
                    .to_sec1_bytes()
                    .as_ref(),
                serde_json::to_string(&proof)?.as_str(),
            )?;
        }
        write_txn.commit()?;
        debug!("Added spend secret: {}", proof.secret.to_string());

        Ok(())
    }

    async fn get_spent_proof_by_hash(
        &self,
        secret_point: &k256::PublicKey,
    ) -> Result<Option<Proof>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(SPENT_PROOFS_TABLE)?;

        let proof = table.get(secret_point.to_sec1_bytes().as_ref())?;

        if let Some(proof) = proof {
            Ok(serde_json::from_str(proof.value())?)
        } else {
            Ok(None)
        }
    }

    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(SPENT_PROOFS_TABLE)?;

        let secret_hash = hash_to_curve(&secret.to_bytes())?;

        let proof = table.get(secret_hash.to_sec1_bytes().as_ref())?;

        debug!("Checking secret: {}", secret.to_string());

        if let Some(proof) = proof {
            Ok(serde_json::from_str(proof.value())?)
        } else {
            Ok(None)
        }
    }

    async fn add_pending_proof(&self, proof: Proof) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(PENDING_PROOFS_TABLE)?;
            table.insert(
                hash_to_curve(&proof.secret.to_bytes())?
                    .to_sec1_bytes()
                    .as_ref(),
                serde_json::to_string(&proof)?.as_str(),
            )?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_pending_proof_by_hash(
        &self,
        secret_point: &k256::PublicKey,
    ) -> Result<Option<Proof>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(PENDING_PROOFS_TABLE)?;

        let proof = table.get(secret_point.to_sec1_bytes().as_ref())?;

        if let Some(proof) = proof {
            Ok(serde_json::from_str(proof.value())?)
        } else {
            Ok(None)
        }
    }

    async fn get_pending_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(PENDING_PROOFS_TABLE)?;

        let secret_hash = hash_to_curve(&secret.to_bytes())?;

        let proof = table.get(secret_hash.to_sec1_bytes().as_ref())?;

        if let Some(proof) = proof {
            Ok(serde_json::from_str(proof.value())?)
        } else {
            Ok(None)
        }
    }

    async fn remove_pending_proof(&self, secret: &Secret) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(PENDING_PROOFS_TABLE)?;
            let secret_hash = hash_to_curve(&secret.to_bytes())?;
            table.remove(secret_hash.to_sec1_bytes().as_ref())?;
        }
        write_txn.commit()?;

        Ok(())
    }
}
