//! SQLite Storage for CDK

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::common::QuoteTTL;
use cdk_common::database::{
    self, MintDatabase, MintKeysDatabase, MintProofsDatabase, MintQuotesDatabase,
    MintSignaturesDatabase,
};
use cdk_common::dhke::hash_to_curve;
use cdk_common::mint::{self, MintKeySetInfo, MintQuote};
use cdk_common::nut00::ProofsMethods;
use cdk_common::state::check_state_transition;
use cdk_common::util::unix_time;
use cdk_common::{
    BlindSignature, CurrencyUnit, Id, MeltQuoteState, MintInfo, MintQuoteState, Proof, Proofs,
    PublicKey, State,
};
use migrations::{migrate_01_to_02, migrate_04_to_05};
use redb::{Database, MultimapTableDefinition, ReadableTable, TableDefinition};
use uuid::Uuid;

use super::error::Error;
use crate::migrations::migrate_00_to_01;
use crate::mint::migrations::{migrate_02_to_03, migrate_03_to_04};

#[cfg(feature = "auth")]
mod auth;
mod migrations;

#[cfg(feature = "auth")]
pub use auth::MintRedbAuthDatabase;

const ACTIVE_KEYSETS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("active_keysets");
const KEYSETS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("keysets");
const MINT_QUOTES_TABLE: TableDefinition<[u8; 16], &str> = TableDefinition::new("mint_quotes");
const MELT_QUOTES_TABLE: TableDefinition<[u8; 16], &str> = TableDefinition::new("melt_quotes");
const PROOFS_TABLE: TableDefinition<[u8; 33], &str> = TableDefinition::new("proofs");
const PROOFS_STATE_TABLE: TableDefinition<[u8; 33], &str> = TableDefinition::new("proofs_state");
const PROOF_CREATED_TIME: TableDefinition<[u8; 33], u64> =
    TableDefinition::new("proof_created_time");
const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
// Key is hex blinded_message B_ value is blinded_signature
const BLINDED_SIGNATURES: TableDefinition<[u8; 33], &str> =
    TableDefinition::new("blinded_signatures");
const BLIND_SIGNATURE_CREATED_TIME: TableDefinition<[u8; 33], u64> =
    TableDefinition::new("blind_signature_created_time");
const QUOTE_PROOFS_TABLE: MultimapTableDefinition<[u8; 16], [u8; 33]> =
    MultimapTableDefinition::new("quote_proofs");
const QUOTE_SIGNATURES_TABLE: MultimapTableDefinition<[u8; 16], [u8; 33]> =
    MultimapTableDefinition::new("quote_signatures");

const DATABASE_VERSION: u32 = 5;

/// Mint Redbdatabase
#[derive(Debug, Clone)]
pub struct MintRedbDatabase {
    db: Arc<Database>,
}

impl MintRedbDatabase {
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
                    let mut current_file_version = u32::from_str(&db_version)?;
                    match current_file_version.cmp(&DATABASE_VERSION) {
                        Ordering::Less => {
                            tracing::info!(
                                "Database needs to be upgraded at {} current is {}",
                                current_file_version,
                                DATABASE_VERSION
                            );
                            if current_file_version == 0 {
                                current_file_version = migrate_00_to_01(Arc::clone(&db))?;
                            }

                            if current_file_version == 1 {
                                current_file_version = migrate_01_to_02(Arc::clone(&db))?;
                            }

                            if current_file_version == 2 {
                                current_file_version = migrate_02_to_03(Arc::clone(&db))?;
                            }

                            if current_file_version == 3 {
                                current_file_version = migrate_03_to_04(Arc::clone(&db))?;
                            }

                            if current_file_version == 4 {
                                current_file_version = migrate_04_to_05(Arc::clone(&db))?;
                            }

                            if current_file_version != DATABASE_VERSION {
                                tracing::warn!(
                                    "Database upgrade did not complete at {} current is {}",
                                    current_file_version,
                                    DATABASE_VERSION
                                );
                                return Err(Error::UnknownDatabaseVersion);
                            }

                            let write_txn = db.begin_write()?;
                            {
                                let mut table = write_txn.open_table(CONFIG_TABLE)?;

                                table
                                    .insert("db_version", DATABASE_VERSION.to_string().as_str())?;
                            }

                            write_txn.commit()?;
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
                        // Open all tables to init a new db
                        let mut table = write_txn.open_table(CONFIG_TABLE)?;
                        let _ = write_txn.open_table(ACTIVE_KEYSETS_TABLE)?;
                        let _ = write_txn.open_table(KEYSETS_TABLE)?;
                        let _ = write_txn.open_table(MINT_QUOTES_TABLE)?;
                        let _ = write_txn.open_table(MELT_QUOTES_TABLE)?;
                        let _ = write_txn.open_table(PROOFS_TABLE)?;
                        let _ = write_txn.open_table(PROOFS_STATE_TABLE)?;
                        let _ = write_txn.open_table(PROOF_CREATED_TIME)?;
                        let _ = write_txn.open_table(BLINDED_SIGNATURES)?;
                        let _ = write_txn.open_table(BLIND_SIGNATURE_CREATED_TIME)?;
                        let _ = write_txn.open_multimap_table(QUOTE_PROOFS_TABLE)?;
                        let _ = write_txn.open_multimap_table(QUOTE_SIGNATURES_TABLE)?;

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
impl MintKeysDatabase for MintRedbDatabase {
    type Err = database::Error;

    async fn set_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

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
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(ACTIVE_KEYSETS_TABLE)
            .map_err(Error::from)?;

        if let Some(id) = table.get(unit.to_string().as_str()).map_err(Error::from)? {
            return Ok(Some(Id::from_str(id.value()).map_err(Error::from)?));
        }

        Ok(None)
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(ACTIVE_KEYSETS_TABLE)
            .map_err(Error::from)?;

        let mut active_keysets = HashMap::new();

        for (unit, id) in (table.iter().map_err(Error::from)?).flatten() {
            let unit = CurrencyUnit::from_str(unit.value())?;
            let id = Id::from_str(id.value()).map_err(Error::from)?;

            active_keysets.insert(unit, id);
        }

        Ok(active_keysets)
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
}

#[async_trait]
impl MintQuotesDatabase for MintRedbDatabase {
    type Err = database::Error;

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    quote.id.as_bytes(),
                    serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &Uuid) -> Result<Option<MintQuote>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        match table.get(quote_id.as_bytes()).map_err(Error::from)? {
            Some(quote) => Ok(serde_json::from_str(quote.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn update_mint_quote_state(
        &self,
        quote_id: &Uuid,
        state: MintQuoteState,
    ) -> Result<MintQuoteState, Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        let current_state;
        {
            let mut mint_quote: MintQuote;
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            {
                let quote_guard = table
                    .get(quote_id.as_bytes())
                    .map_err(Error::from)?
                    .ok_or(Error::UnknownQuote)?;

                let quote = quote_guard.value();

                mint_quote = serde_json::from_str(quote).map_err(Error::from)?;
            }

            current_state = mint_quote.state;
            mint_quote.state = state;

            {
                table
                    .insert(
                        quote_id.as_bytes(),
                        serde_json::to_string(&mint_quote)
                            .map_err(Error::from)?
                            .as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(current_state)
    }
    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let quotes = self.get_mint_quotes().await?;

        let quote = quotes
            .into_iter()
            .filter(|q| q.request.eq(request))
            .collect::<Vec<MintQuote>>()
            .first()
            .cloned();

        Ok(quote)
    }

    async fn get_mint_quote_by_request_lookup_id(
        &self,
        request_lookup_id: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let quotes = self.get_mint_quotes().await?;

        let quote = quotes
            .into_iter()
            .filter(|q| q.request_lookup_id.eq(request_lookup_id))
            .collect::<Vec<MintQuote>>()
            .first()
            .cloned();

        Ok(quote)
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
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

    async fn get_mint_quotes_with_state(
        &self,
        state: MintQuoteState,
    ) -> Result<Vec<MintQuote>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        let mut quotes = Vec::new();

        for (_id, quote) in (table.iter().map_err(Error::from)?).flatten() {
            let quote: MintQuote = serde_json::from_str(quote.value()).map_err(Error::from)?;

            if quote.state == state {
                quotes.push(quote)
            }
        }

        Ok(quotes)
    }

    async fn remove_mint_quote(&self, quote_id: &Uuid) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table.remove(quote_id.as_bytes()).map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_melt_quote(&self, quote: mint::MeltQuote) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    quote.id.as_bytes(),
                    serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &Uuid) -> Result<Option<mint::MeltQuote>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MELT_QUOTES_TABLE)
            .map_err(Error::from)?;

        let quote = table.get(quote_id.as_bytes()).map_err(Error::from)?;

        Ok(quote.map(|q| serde_json::from_str(q.value()).unwrap()))
    }

    async fn update_melt_quote_state(
        &self,
        quote_id: &Uuid,
        state: MeltQuoteState,
    ) -> Result<(MeltQuoteState, mint::MeltQuote), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        let current_state;
        let mut melt_quote: mint::MeltQuote;

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            {
                let quote_guard = table
                    .get(quote_id.as_bytes())
                    .map_err(Error::from)?
                    .ok_or(Error::UnknownQuote)?;

                let quote = quote_guard.value();

                melt_quote = serde_json::from_str(quote).map_err(Error::from)?;
            }

            current_state = melt_quote.state;
            melt_quote.state = state;

            {
                table
                    .insert(
                        quote_id.as_bytes(),
                        serde_json::to_string(&melt_quote)
                            .map_err(Error::from)?
                            .as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok((current_state, melt_quote))
    }

    async fn get_melt_quotes(&self) -> Result<Vec<mint::MeltQuote>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
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

    async fn remove_melt_quote(&self, quote_id: &Uuid) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table.remove(quote_id.as_bytes()).map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }
}

#[async_trait]
impl MintProofsDatabase for MintRedbDatabase {
    type Err = database::Error;

    async fn add_proofs(&self, proofs: Proofs, quote_id: Option<Uuid>) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;
            let mut time_table = write_txn
                .open_table(PROOF_CREATED_TIME)
                .map_err(Error::from)?;
            let mut quote_proofs_table = write_txn
                .open_multimap_table(QUOTE_PROOFS_TABLE)
                .map_err(Error::from)?;

            // Get current timestamp in seconds
            let current_time = unix_time();

            for proof in proofs {
                let y: PublicKey = hash_to_curve(&proof.secret.to_bytes()).map_err(Error::from)?;
                let y = y.to_bytes();
                if table.get(y).map_err(Error::from)?.is_none() {
                    table
                        .insert(
                            y,
                            serde_json::to_string(&proof).map_err(Error::from)?.as_str(),
                        )
                        .map_err(Error::from)?;

                    // Store creation time
                    time_table.insert(y, current_time).map_err(Error::from)?;
                }

                if let Some(quote_id) = &quote_id {
                    quote_proofs_table
                        .insert(quote_id.as_bytes(), y)
                        .map_err(Error::from)?;
                }
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn remove_proofs(
        &self,
        ys: &[PublicKey],
        quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        let mut states: HashSet<State> = HashSet::new();

        {
            let mut proof_state_table = write_txn
                .open_table(PROOFS_STATE_TABLE)
                .map_err(Error::from)?;
            for y in ys {
                let state = proof_state_table
                    .remove(&y.to_bytes())
                    .map_err(Error::from)?;

                if let Some(state) = state {
                    let state: State = serde_json::from_str(state.value()).map_err(Error::from)?;

                    states.insert(state);
                }
            }
        }

        if states.contains(&State::Spent) {
            tracing::warn!("Db attempted to remove spent proof");
            write_txn.abort().map_err(Error::from)?;
            return Err(Self::Err::AttemptRemoveSpentProof);
        }

        {
            let mut proofs_table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;
            let mut time_table = write_txn
                .open_table(PROOF_CREATED_TIME)
                .map_err(Error::from)?;

            for y in ys {
                proofs_table.remove(&y.to_bytes()).map_err(Error::from)?;
                time_table.remove(&y.to_bytes()).map_err(Error::from)?;
            }
        }

        if let Some(quote_id) = quote_id {
            let mut quote_proofs_table = write_txn
                .open_multimap_table(QUOTE_PROOFS_TABLE)
                .map_err(Error::from)?;

            quote_proofs_table
                .remove_all(quote_id.as_bytes())
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

        let mut proofs = Vec::with_capacity(ys.len());

        for y in ys {
            match table.get(y.to_bytes()).map_err(Error::from)? {
                Some(proof) => proofs.push(Some(
                    serde_json::from_str(proof.value()).map_err(Error::from)?,
                )),
                None => proofs.push(None),
            }
        }

        Ok(proofs)
    }

    async fn get_proof_ys_by_quote_id(&self, quote_id: &Uuid) -> Result<Vec<PublicKey>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_multimap_table(QUOTE_PROOFS_TABLE)
            .map_err(Error::from)?;

        let ys = table.get(quote_id.as_bytes()).map_err(Error::from)?;

        let proof_ys = ys.fold(Vec::new(), |mut acc, y| {
            if let Ok(y) = y {
                if let Ok(pubkey) = PublicKey::from_slice(&y.value()) {
                    acc.push(pubkey);
                }
            }
            acc
        });

        Ok(proof_ys)
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

    async fn get_proofs_by_keyset_id(
        &self,
        keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

        let proofs_for_id = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .map(|(_, p)| serde_json::from_str::<Proof>(p.value()))
            .collect::<Result<Proofs, _>>()?
            .into_iter()
            .filter(|p| &p.keyset_id == keyset_id)
            .collect::<Proofs>();

        let proof_ys = proofs_for_id.ys()?;

        assert_eq!(proofs_for_id.len(), proof_ys.len());

        let states = self.get_proofs_states(&proof_ys).await?;

        Ok((proofs_for_id, states))
    }

    async fn update_proofs_states(
        &self,
        ys: &[PublicKey],
        proofs_state: State,
    ) -> Result<Vec<Option<State>>, Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        let mut states = Vec::with_capacity(ys.len());
        {
            let table = write_txn
                .open_table(PROOFS_STATE_TABLE)
                .map_err(Error::from)?;
            {
                // First collect current states
                for y in ys {
                    let current_state = match table.get(y.to_bytes()).map_err(Error::from)? {
                        Some(state) => {
                            let current_state =
                                serde_json::from_str(state.value()).map_err(Error::from)?;
                            check_state_transition(current_state, proofs_state)?;
                            Some(current_state)
                        }
                        None => None,
                    };

                    states.push(current_state);
                }
            }
        }

        {
            let mut table = write_txn
                .open_table(PROOFS_STATE_TABLE)
                .map_err(Error::from)?;
            {
                // If no proofs are spent, proceed with update
                let state_str = serde_json::to_string(&proofs_state).map_err(Error::from)?;
                for y in ys {
                    table
                        .insert(y.to_bytes(), state_str.as_str())
                        .map_err(Error::from)?;
                }
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(states)
    }
}

#[async_trait]
impl MintSignaturesDatabase for MintRedbDatabase {
    type Err = database::Error;

    async fn add_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
        quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(BLINDED_SIGNATURES)
                .map_err(Error::from)?;
            let mut time_table = write_txn
                .open_table(BLIND_SIGNATURE_CREATED_TIME)
                .map_err(Error::from)?;
            let mut quote_sigs_table = write_txn
                .open_multimap_table(QUOTE_SIGNATURES_TABLE)
                .map_err(Error::from)?;

            // Get current timestamp in seconds
            let current_time = unix_time();

            for (blinded_message, blind_signature) in blinded_messages.iter().zip(blind_signatures)
            {
                let blind_sig = serde_json::to_string(&blind_signature).map_err(Error::from)?;
                table
                    .insert(blinded_message.to_bytes(), blind_sig.as_str())
                    .map_err(Error::from)?;

                // Store creation time
                time_table
                    .insert(blinded_message.to_bytes(), current_time)
                    .map_err(Error::from)?;

                if let Some(quote_id) = &quote_id {
                    quote_sigs_table
                        .insert(quote_id.as_bytes(), blinded_message.to_bytes())
                        .map_err(Error::from)?;
                }
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

    async fn get_blind_signatures_for_keyset(
        &self,
        keyset_id: &Id,
    ) -> Result<Vec<BlindSignature>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(BLINDED_SIGNATURES)
            .map_err(Error::from)?;

        Ok(table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .filter_map(|(_m, s)| {
                match serde_json::from_str::<BlindSignature>(s.value()).ok() {
                    Some(signature) if &signature.keyset_id == keyset_id => Some(signature), // Filter by keyset_id
                    _ => None, // Exclude non-matching entries
                }
            })
            .collect())
    }

    /// Get [`BlindSignature`]s for quote
    async fn get_blind_signatures_for_quote(
        &self,
        quote_id: &Uuid,
    ) -> Result<Vec<BlindSignature>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let quote_proofs_table = read_txn
            .open_multimap_table(QUOTE_SIGNATURES_TABLE)
            .map_err(Error::from)?;

        let ys = quote_proofs_table.get(quote_id.as_bytes()).unwrap();

        let ys: Vec<[u8; 33]> = ys.into_iter().flatten().map(|v| v.value()).collect();

        let mut signatures = Vec::new();

        let signatures_table = read_txn
            .open_table(BLINDED_SIGNATURES)
            .map_err(Error::from)?;

        for y in ys {
            if let Some(sig) = signatures_table.get(y).map_err(Error::from)? {
                let sig = serde_json::from_str(sig.value())?;
                signatures.push(sig);
            }
        }

        Ok(signatures)
    }
}

#[async_trait]
impl MintDatabase<database::Error> for MintRedbDatabase {
    async fn set_mint_info(&self, mint_info: MintInfo) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(CONFIG_TABLE).map_err(Error::from)?;
            table
                .insert("mint_info", serde_json::to_string(&mint_info)?.as_str())
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }
    async fn get_mint_info(&self) -> Result<MintInfo, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(CONFIG_TABLE).map_err(Error::from)?;

        if let Some(mint_info) = table.get("mint_info").map_err(Error::from)? {
            let mint_info = serde_json::from_str(mint_info.value())?;

            return Ok(mint_info);
        }

        Err(Error::UnknownMintInfo.into())
    }

    async fn set_quote_ttl(&self, quote_ttl: QuoteTTL) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(CONFIG_TABLE).map_err(Error::from)?;
            table
                .insert("quote_ttl", serde_json::to_string(&quote_ttl)?.as_str())
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }
    async fn get_quote_ttl(&self) -> Result<QuoteTTL, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(CONFIG_TABLE).map_err(Error::from)?;

        if let Some(quote_ttl) = table.get("quote_ttl").map_err(Error::from)? {
            let quote_ttl = serde_json::from_str(quote_ttl.value())?;

            return Ok(quote_ttl);
        }

        Err(Error::UnknownQuoteTTL.into())
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::secret::Secret;
    use cdk_common::{mint_db_test, Amount, SecretKey};
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn test_remove_spent_proofs() {
        let tmp_dir = tempdir().unwrap();

        let db = MintRedbDatabase::new(&tmp_dir.path().join("mint.redb")).unwrap();
        // Create some test proofs
        let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();

        let proofs = vec![
            Proof {
                amount: Amount::from(100),
                keyset_id,
                secret: Secret::generate(),
                c: SecretKey::generate().public_key(),
                witness: None,
                dleq: None,
            },
            Proof {
                amount: Amount::from(200),
                keyset_id,
                secret: Secret::generate(),
                c: SecretKey::generate().public_key(),
                witness: None,
                dleq: None,
            },
        ];

        // Add proofs to database
        db.add_proofs(proofs.clone(), None).await.unwrap();

        // Mark one proof as spent
        db.update_proofs_states(&[proofs[0].y().unwrap()], State::Spent)
            .await
            .unwrap();

        db.update_proofs_states(&[proofs[1].y().unwrap()], State::Unspent)
            .await
            .unwrap();

        // Try to remove both proofs - should fail because one is spent
        let result = db
            .remove_proofs(&[proofs[0].y().unwrap(), proofs[1].y().unwrap()], None)
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            database::Error::AttemptRemoveSpentProof
        ));

        // Verify both proofs still exist
        let states = db
            .get_proofs_states(&[proofs[0].y().unwrap(), proofs[1].y().unwrap()])
            .await
            .unwrap();

        assert_eq!(states.len(), 2);
        assert_eq!(states[0], Some(State::Spent));
        assert_eq!(states[1], Some(State::Unspent));
    }

    #[tokio::test]
    async fn test_update_spent_proofs() {
        let tmp_dir = tempdir().unwrap();

        let db = MintRedbDatabase::new(&tmp_dir.path().join("mint.redb")).unwrap();
        // Create some test proofs
        let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();

        let proofs = vec![
            Proof {
                amount: Amount::from(100),
                keyset_id,
                secret: Secret::generate(),
                c: SecretKey::generate().public_key(),
                witness: None,
                dleq: None,
            },
            Proof {
                amount: Amount::from(200),
                keyset_id,
                secret: Secret::generate(),
                c: SecretKey::generate().public_key(),
                witness: None,
                dleq: None,
            },
        ];

        // Add proofs to database
        db.add_proofs(proofs.clone(), None).await.unwrap();

        // Mark one proof as spent
        db.update_proofs_states(&[proofs[0].y().unwrap()], State::Spent)
            .await
            .unwrap();

        db.update_proofs_states(&[proofs[1].y().unwrap()], State::Unspent)
            .await
            .unwrap();

        // Mark one proof as spent
        let result = db
            .update_proofs_states(
                &[proofs[0].y().unwrap(), proofs[1].y().unwrap()],
                State::Unspent,
            )
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            database::Error::AttemptUpdateSpentProof
        ));

        // Verify both proofs still exist
        let states = db
            .get_proofs_states(&[proofs[0].y().unwrap(), proofs[1].y().unwrap()])
            .await
            .unwrap();

        assert_eq!(states.len(), 2);
        assert_eq!(states[0], Some(State::Spent));
        assert_eq!(states[1], Some(State::Unspent));
    }

    async fn provide_db() -> MintRedbDatabase {
        let tmp_dir = tempdir().unwrap();

        MintRedbDatabase::new(&tmp_dir.path().join("mint.redb")).unwrap()
    }

    mint_db_test!(provide_db);
}
