use std::cmp::Ordering;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cashu::dhke::hash_to_curve;
use cashu::nuts::{AuthProof, BlindSignature, Id, PublicKey, State};
use cdk_common::database::{self, MintAuthDatabase};
use cdk_common::mint::MintKeySetInfo;
use redb::{Database, ReadableTable, TableDefinition};

use crate::error::Error;

const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
const ACTIVE_KEYSET_TABLE: TableDefinition<&str, &str> = TableDefinition::new("active_keyset");
const KEYSETS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("keysets");
const PROOFS_TABLE: TableDefinition<[u8; 33], &str> = TableDefinition::new("proofs");
const PROOFS_STATE_TABLE: TableDefinition<[u8; 33], &str> = TableDefinition::new("proofs_state");
// Key is hex blinded_message B_ value is blinded_signature
const BLINDED_SIGNATURES: TableDefinition<[u8; 33], &str> =
    TableDefinition::new("blinded_signatures");

/// Mint Redbdatabase
#[derive(Debug, Clone)]
pub struct MintRedbAuthDatabase {
    db: Arc<Database>,
}

const DATABASE_VERSION: u32 = 0;

impl MintRedbAuthDatabase {
    /// Create new [`MintRedbDatabase`]
    pub fn new(path: &Path) -> Result<Self, Error> {
        {
            // Check database version

            let db = Arc::new(Database::create(path)?);

            // Check database version
            let read_txn = db.begin_read()?;
            let table = read_txn.open_table(CONFIG_TABLE);

            let db_version = match table {
                Ok(table) => table.get("db_version")?.map(|v| v.value().to_owned()),
                Err(_) => None,
            };
            match db_version {
                Some(db_version) => {
                    let current_file_version = u32::from_str(&db_version)?;
                    match current_file_version.cmp(&DATABASE_VERSION) {
                        Ordering::Less => {
                            tracing::info!(
                                "Database needs to be upgraded at {} current is {}",
                                current_file_version,
                                DATABASE_VERSION
                            );
                        }
                        Ordering::Equal => {
                            tracing::info!("Database is at current version {}", DATABASE_VERSION);
                        }
                        Ordering::Greater => {
                            tracing::warn!(
                                "Database upgrade did not complete at {} current is {}",
                                current_file_version,
                                DATABASE_VERSION
                            );
                            return Err(Error::UnknownDatabaseVersion);
                        }
                    }
                }
                None => {
                    let write_txn = db.begin_write()?;
                    {
                        let mut table = write_txn.open_table(CONFIG_TABLE)?;
                        // Open all tables to init a new db
                        let _ = write_txn.open_table(ACTIVE_KEYSET_TABLE)?;
                        let _ = write_txn.open_table(KEYSETS_TABLE)?;
                        let _ = write_txn.open_table(PROOFS_TABLE)?;
                        let _ = write_txn.open_table(PROOFS_STATE_TABLE)?;
                        let _ = write_txn.open_table(BLINDED_SIGNATURES)?;

                        table.insert("db_version", DATABASE_VERSION.to_string().as_str())?;
                    }

                    write_txn.commit()?;
                }
            }
            drop(db);
        }

        let db = Database::create(path)?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl MintAuthDatabase for MintRedbAuthDatabase {
    type Err = database::Error;

    async fn set_active_keyset(&self, id: Id) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(ACTIVE_KEYSET_TABLE)
                .map_err(Error::from)?;
            table
                .insert("active_keyset_id", id.to_string().as_str())
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_active_keyset_id(&self) -> Result<Option<Id>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(ACTIVE_KEYSET_TABLE)
            .map_err(Error::from)?;

        if let Some(id) = table.get("active_keyset_id").map_err(Error::from)? {
            return Ok(Some(Id::from_str(id.value()).map_err(Error::from)?));
        }

        Ok(None)
    }

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

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
        let read_txn = self.db.begin_read().map_err(Error::from)?;
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
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

        let mut keysets = Vec::new();

        for (_id, keyset) in (table.iter().map_err(Error::from)?).flatten() {
            let keyset = serde_json::from_str(keyset.value()).map_err(Error::from)?;

            keysets.push(keyset)
        }

        Ok(keysets)
    }

    async fn add_proof(&self, proof: AuthProof) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;
            let y: PublicKey = hash_to_curve(&proof.secret.to_bytes()).map_err(Error::from)?;
            let y = y.to_bytes();
            if table.get(y).map_err(Error::from)?.is_none() {
                table
                    .insert(
                        y,
                        serde_json::to_string(&proof).map_err(Error::from)?.as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn update_proof_state(
        &self,
        y: &PublicKey,
        proof_state: State,
    ) -> Result<Option<State>, Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        let state_str = serde_json::to_string(&proof_state).map_err(Error::from)?;

        let current_state;

        {
            let mut table = write_txn
                .open_table(PROOFS_STATE_TABLE)
                .map_err(Error::from)?;

            {
                match table.get(y.to_bytes()).map_err(Error::from)? {
                    Some(state) => {
                        current_state =
                            Some(serde_json::from_str(state.value()).map_err(Error::from)?)
                    }
                    None => current_state = None,
                }
            }

            if current_state != Some(State::Spent) {
                table
                    .insert(y.to_bytes(), state_str.as_str())
                    .map_err(Error::from)?;
            }
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(current_state)
    }

    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(PROOFS_STATE_TABLE)
            .map_err(Error::from)?;

        let mut states = Vec::with_capacity(ys.len());

        for y in ys {
            match table.get(y.to_bytes()).map_err(Error::from)? {
                Some(state) => states.push(Some(
                    serde_json::from_str(state.value()).map_err(Error::from)?,
                )),
                None => states.push(None),
            }
        }

        Ok(states)
    }

    async fn add_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
    ) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(BLINDED_SIGNATURES)
                .map_err(Error::from)?;

            for (blinded_message, blind_signature) in blinded_messages.iter().zip(blind_signatures)
            {
                let blind_sig = serde_json::to_string(&blind_signature).map_err(Error::from)?;
                table
                    .insert(blinded_message.to_bytes(), blind_sig.as_str())
                    .map_err(Error::from)?;
            }
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
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