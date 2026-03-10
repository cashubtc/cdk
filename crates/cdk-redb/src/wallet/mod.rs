//! Redb Wallet

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::{validate_kvstore_params, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::nut00::KnownMethod;
use cdk_common::util::unix_time;
use cdk_common::wallet::{
    self, MintQuote, ProofInfo, Transaction, TransactionDirection, TransactionId,
};
use cdk_common::{
    database, Amount, CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, PaymentMethod,
    PublicKey, SpendingConditions, State,
};
use redb::{Database, MultimapTableDefinition, ReadableDatabase, ReadableTable, TableDefinition};
use tracing::instrument;

use super::error::Error;
use crate::migrations::migrate_00_to_01;
use crate::wallet::migrations::{migrate_01_to_02, migrate_02_to_03, migrate_03_to_04};

mod migrations;

// <Mint_url, Info>
const MINTS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mints_table");
// <Mint_Url, Keyset_id>
const MINT_KEYSETS_TABLE: MultimapTableDefinition<&str, &[u8]> =
    MultimapTableDefinition::new("mint_keysets");
// <Keyset_id, KeysetInfo>
const KEYSETS_TABLE: TableDefinition<&[u8], &str> = TableDefinition::new("keysets");
// <Quote_id, quote>
const MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");
// <Quote_id, quote>
const MELT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("melt_quotes");
const MINT_KEYS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_keys");
// <Y, Proof Info>
const PROOFS_TABLE: TableDefinition<&[u8], &str> = TableDefinition::new("proofs");
const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
const KEYSET_COUNTER: TableDefinition<&str, u32> = TableDefinition::new("keyset_counter");
// <Transaction_id, Transaction>
const TRANSACTIONS_TABLE: TableDefinition<&[u8], &str> = TableDefinition::new("transactions");
// <Saga_id, WalletSaga>
const SAGAS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("wallet_sagas");

const KEYSET_U32_MAPPING: TableDefinition<u32, &str> = TableDefinition::new("keyset_u32_mapping");
// <(primary_namespace, secondary_namespace, key), value>
const KV_STORE_TABLE: TableDefinition<(&str, &str, &str), &[u8]> = TableDefinition::new("kv_store");

const DATABASE_VERSION: u32 = 4;

/// Wallet Redb Database
#[derive(Debug, Clone)]
pub struct WalletRedbDatabase {
    db: Arc<Database>,
}

impl WalletRedbDatabase {
    /// Create new [`WalletRedbDatabase`]
    pub fn new(path: &Path) -> Result<Self, Error> {
        {
            // Check if parent directory exists before attempting to create database
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("Parent directory does not exist: {parent:?}"),
                    )));
                }
            }

            let db = Arc::new(Database::create(path)?);

            let db_version: Option<String>;
            {
                // Check database version
                let read_txn = db.begin_read()?;
                let table = read_txn.open_table(CONFIG_TABLE);

                db_version = match table {
                    Ok(table) => table.get("db_version")?.map(|v| v.value().to_string()),
                    Err(_) => None,
                };
            }

            match db_version {
                Some(db_version) => {
                    let mut current_file_version = u32::from_str(&db_version)?;
                    tracing::info!("Current file version {}", current_file_version);

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
                        let mut table = write_txn.open_table(CONFIG_TABLE)?;
                        // Open all tables to init a new db
                        let _ = write_txn.open_table(MINTS_TABLE)?;
                        let _ = write_txn.open_multimap_table(MINT_KEYSETS_TABLE)?;
                        let _ = write_txn.open_table(KEYSETS_TABLE)?;
                        let _ = write_txn.open_table(MINT_QUOTES_TABLE)?;
                        let _ = write_txn.open_table(MELT_QUOTES_TABLE)?;
                        let _ = write_txn.open_table(MINT_KEYS_TABLE)?;
                        let _ = write_txn.open_table(PROOFS_TABLE)?;
                        let _ = write_txn.open_table(KEYSET_COUNTER)?;
                        let _ = write_txn.open_table(TRANSACTIONS_TABLE)?;
                        let _ = write_txn.open_table(KEYSET_U32_MAPPING)?;
                        let _ = write_txn.open_table(KV_STORE_TABLE)?;
                        table.insert("db_version", DATABASE_VERSION.to_string().as_str())?;
                    }

                    write_txn.commit()?;
                }
            }
            drop(db);
        }

        // Check parent directory again for final database creation
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Parent directory does not exist: {parent:?}"),
                )));
            }
        }

        let db = Database::create(path)?;

        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl WalletDatabase<database::Error> for WalletRedbDatabase {
    #[instrument(skip(self))]
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn.open_table(MINTS_TABLE).map_err(Error::from)?;

        if let Some(mint_info) = table
            .get(mint_url.to_string().as_str())
            .map_err(Error::from)?
        {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip(self))]
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(MINTS_TABLE).map_err(Error::from)?;
        let mints = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .filter_map(|(mint, mint_info)| {
                MintUrl::from_str(mint.value())
                    .ok()
                    .map(|url| (url, serde_json::from_str(mint_info.value()).ok()))
            })
            .collect();

        Ok(mints)
    }

    #[instrument(skip(self))]
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn
            .open_multimap_table(MINT_KEYSETS_TABLE)
            .map_err(Error::from)?;

        let keyset_ids = table
            .get(mint_url.to_string().as_str())
            .map_err(Error::from)?
            .flatten()
            .map(|k| Id::from_bytes(k.value()))
            .collect::<Result<Vec<_>, _>>()?;

        let mut keysets = vec![];

        let keysets_t = read_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

        for keyset_id in keyset_ids {
            if let Some(keyset) = keysets_t
                .get(keyset_id.to_bytes().as_slice())
                .map_err(Error::from)?
            {
                let keyset = serde_json::from_str(keyset.value()).map_err(Error::from)?;

                keysets.push(keyset);
            }
        }

        match keysets.is_empty() {
            true => Ok(None),
            false => Ok(Some(keysets)),
        }
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keyset_by_id(
        &self,
        keyset_id: &Id,
    ) -> Result<Option<KeySetInfo>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

        match table
            .get(keyset_id.to_bytes().as_slice())
            .map_err(Error::from)?
        {
            Some(keyset) => {
                let keyset: KeySetInfo =
                    serde_json::from_str(keyset.value()).map_err(Error::from)?;

                Ok(Some(keyset))
            }
            None => Ok(None),
        }
    }

    #[instrument(skip_all)]
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        if let Some(mint_info) = table.get(quote_id).map_err(Error::from)? {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip_all)]
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        Ok(table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .flat_map(|(_id, quote)| serde_json::from_str(quote.value()))
            .collect())
    }

    async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        Ok(table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .flat_map(|(_id, quote)| serde_json::from_str::<MintQuote>(quote.value()).ok())
            .filter(|quote| {
                quote.amount_issued == Amount::ZERO
                    || quote.payment_method == PaymentMethod::Known(KnownMethod::Bolt12)
            })
            .collect())
    }

    #[instrument(skip_all)]
    async fn get_melt_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<wallet::MeltQuote>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MELT_QUOTES_TABLE)
            .map_err(Error::from)?;

        if let Some(mint_info) = table.get(quote_id).map_err(Error::from)? {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip_all)]
    async fn get_melt_quotes(&self) -> Result<Vec<wallet::MeltQuote>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MELT_QUOTES_TABLE)
            .map_err(Error::from)?;

        Ok(table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .flat_map(|(_id, quote)| serde_json::from_str(quote.value()))
            .collect())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keys(&self, keyset_id: &Id) -> Result<Option<Keys>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

        if let Some(mint_info) = table
            .get(keyset_id.to_string().as_str())
            .map_err(Error::from)?
        {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip_all)]
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;

        let table = read_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

        let proofs: Vec<ProofInfo> = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .filter_map(|(_k, v)| {
                let mut proof = None;

                if let Ok(proof_info) = serde_json::from_str::<ProofInfo>(v.value()) {
                    if proof_info.matches_conditions(&mint_url, &unit, &state, &spending_conditions)
                    {
                        proof = Some(proof_info)
                    }
                }

                proof
            })
            .collect();

        Ok(proofs)
    }

    #[instrument(skip(self, ys))]
    async fn get_proofs_by_ys(
        &self,
        ys: Vec<PublicKey>,
    ) -> Result<Vec<ProofInfo>, database::Error> {
        if ys.is_empty() {
            return Ok(Vec::new());
        }

        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

        let mut proofs = Vec::new();

        for y in ys {
            if let Some(proof) = table.get(y.to_bytes().as_slice()).map_err(Error::from)? {
                let proof_info =
                    serde_json::from_str::<ProofInfo>(proof.value()).map_err(Error::from)?;
                proofs.push(proof_info);
            }
        }

        Ok(proofs)
    }

    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
    ) -> Result<u64, database::Error> {
        // For redb, we still need to fetch all proofs and sum them
        // since redb doesn't have SQL aggregation
        let proofs = self.get_proofs(mint_url, unit, state, None).await?;
        Ok(proofs.iter().map(|p| u64::from(p.proof.amount)).sum())
    }

    #[instrument(skip(self))]
    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(TRANSACTIONS_TABLE)
            .map_err(Error::from)?;

        if let Some(transaction) = table.get(transaction_id.as_slice()).map_err(Error::from)? {
            return Ok(serde_json::from_str(transaction.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip(self))]
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;

        let table = read_txn
            .open_table(TRANSACTIONS_TABLE)
            .map_err(Error::from)?;

        let transactions: Vec<Transaction> = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .filter_map(|(_k, v)| {
                let mut transaction = None;

                if let Ok(tx) = serde_json::from_str::<Transaction>(v.value()) {
                    if tx.matches_conditions(&mint_url, &direction, &unit) {
                        transaction = Some(tx)
                    }
                }

                transaction
            })
            .collect();

        Ok(transactions)
    }

    #[instrument(skip(self, added, removed_ys))]
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

            for proof_info in added.iter() {
                table
                    .insert(
                        proof_info.y.to_bytes().as_slice(),
                        serde_json::to_string(&proof_info)
                            .map_err(Error::from)?
                            .as_str(),
                    )
                    .map_err(Error::from)?;
            }

            for y in removed_ys.iter() {
                table.remove(y.to_bytes().as_slice()).map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: State,
    ) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

            for y in ys {
                let y_slice = y.to_bytes();
                let proof = table
                    .get(y_slice.as_slice())
                    .map_err(Error::from)?
                    .ok_or(Error::UnknownY)?;

                let mut proof_info =
                    serde_json::from_str::<ProofInfo>(proof.value()).map_err(Error::from)?;
                drop(proof);

                proof_info.state = state;

                table
                    .insert(
                        y_slice.as_slice(),
                        serde_json::to_string(&proof_info)
                            .map_err(Error::from)?
                            .as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), database::Error> {
        let id = transaction.id();
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn
                .open_table(TRANSACTIONS_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    id.as_slice(),
                    serde_json::to_string(&transaction)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        // Update proofs table
        {
            let read_table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;
            let proofs: Vec<ProofInfo> = read_table
                .iter()
                .map_err(Error::from)?
                .flatten()
                .filter_map(|(_k, v)| {
                    let proof_info = serde_json::from_str::<ProofInfo>(v.value()).ok()?;
                    if proof_info.mint_url == old_mint_url {
                        Some(proof_info)
                    } else {
                        None
                    }
                })
                .collect();
            drop(read_table);

            if !proofs.is_empty() {
                let mut write_table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;
                for mut proof_info in proofs {
                    proof_info.mint_url = new_mint_url.clone();
                    write_table
                        .insert(
                            proof_info.y.to_bytes().as_slice(),
                            serde_json::to_string(&proof_info)
                                .map_err(Error::from)?
                                .as_str(),
                        )
                        .map_err(Error::from)?;
                }
            }
        }

        // Update mint quotes
        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;

            let unix_time = unix_time();

            let quotes: Vec<MintQuote> = table
                .iter()
                .map_err(Error::from)?
                .flatten()
                .filter_map(|(_, quote)| {
                    let mut q: MintQuote = serde_json::from_str(quote.value()).ok()?;
                    if q.mint_url == old_mint_url && q.expiry >= unix_time {
                        q.mint_url = new_mint_url.clone();
                        Some(q)
                    } else {
                        None
                    }
                })
                .collect();

            for quote in quotes {
                table
                    .insert(
                        quote.id.as_str(),
                        serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }

        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn increment_keyset_counter(
        &self,
        keyset_id: &Id,
        count: u32,
    ) -> Result<u32, database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        let new_counter = {
            let mut table = write_txn.open_table(KEYSET_COUNTER).map_err(Error::from)?;
            let current_counter = table
                .get(keyset_id.to_string().as_str())
                .map_err(Error::from)?
                .map(|x| x.value())
                .unwrap_or_default();

            let new_counter = current_counter + count;

            table
                .insert(keyset_id.to_string().as_str(), new_counter)
                .map_err(Error::from)?;

            new_counter
        };
        write_txn.commit().map_err(Error::from)?;
        Ok(new_counter)
    }

    #[instrument(skip(self))]
    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn.open_table(MINTS_TABLE).map_err(Error::from)?;
            table
                .insert(
                    mint_url.to_string().as_str(),
                    serde_json::to_string(&mint_info)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn.open_table(MINTS_TABLE).map_err(Error::from)?;
            table
                .remove(mint_url.to_string().as_str())
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn
                .open_multimap_table(MINT_KEYSETS_TABLE)
                .map_err(Error::from)?;
            let mut keysets_table = write_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;
            let mut u32_table = write_txn
                .open_table(KEYSET_U32_MAPPING)
                .map_err(Error::from)?;

            let mut existing_u32 = false;

            for keyset in keysets {
                // Check if keyset already exists
                let existing_keyset = {
                    let existing_keyset = keysets_table
                        .get(keyset.id.to_bytes().as_slice())
                        .map_err(Error::from)?;

                    existing_keyset.map(|r| r.value().to_string())
                };

                let existing = u32_table
                    .insert(u32::from(keyset.id), keyset.id.to_string().as_str())
                    .map_err(Error::from)?;

                match existing {
                    None => existing_u32 = false,
                    Some(id) => {
                        let id = Id::from_str(id.value())?;

                        if id == keyset.id {
                            existing_u32 = false;
                        } else {
                            existing_u32 = true;
                            break;
                        }
                    }
                }

                let keyset = if let Some(existing_keyset) = existing_keyset {
                    let mut existing_keyset: KeySetInfo = serde_json::from_str(&existing_keyset)?;

                    existing_keyset.active = keyset.active;
                    existing_keyset.input_fee_ppk = keyset.input_fee_ppk;

                    existing_keyset
                } else {
                    table
                        .insert(
                            mint_url.to_string().as_str(),
                            keyset.id.to_bytes().as_slice(),
                        )
                        .map_err(Error::from)?;

                    keyset
                };

                keysets_table
                    .insert(
                        keyset.id.to_bytes().as_slice(),
                        serde_json::to_string(&keyset)
                            .map_err(Error::from)?
                            .as_str(),
                    )
                    .map_err(Error::from)?;
            }

            if existing_u32 {
                tracing::warn!("Keyset already exists for keyset id");
                return Err(database::Error::Duplicate);
            }
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;

            // Check for existing quote and version match
            let existing_quote_json = table
                .get(quote.id.as_str())
                .map_err(Error::from)?
                .map(|v| v.value().to_string());

            let mut quote_to_save = quote.clone();

            if let Some(json) = existing_quote_json {
                let existing_quote: MintQuote = serde_json::from_str(&json).map_err(Error::from)?;

                if existing_quote.version != quote.version {
                    return Err(database::Error::ConcurrentUpdate);
                }

                // Increment version for update
                quote_to_save.version = quote.version.wrapping_add(1);
            }

            table
                .insert(
                    quote_to_save.id.as_str(),
                    serde_json::to_string(&quote_to_save)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table.remove(quote_id).map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;

            // Check for existing quote and version match
            let existing_quote_json = table
                .get(quote.id.as_str())
                .map_err(Error::from)?
                .map(|v| v.value().to_string());

            let mut quote_to_save = quote.clone();

            if let Some(json) = existing_quote_json {
                let existing_quote: wallet::MeltQuote =
                    serde_json::from_str(&json).map_err(Error::from)?;

                if existing_quote.version != quote.version {
                    return Err(database::Error::ConcurrentUpdate);
                }

                // Increment version for update
                quote_to_save.version = quote.version.wrapping_add(1);
            }

            table
                .insert(
                    quote_to_save.id.as_str(),
                    serde_json::to_string(&quote_to_save)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table.remove(quote_id).map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_keys(&self, keyset: KeySet) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        keyset.verify_id()?;

        {
            let mut table = write_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

            let existing_keys = table
                .insert(
                    keyset.id.to_string().as_str(),
                    serde_json::to_string(&keyset.keys)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?
                .is_some();

            let mut table = write_txn
                .open_table(KEYSET_U32_MAPPING)
                .map_err(Error::from)?;

            let existing = table
                .insert(u32::from(keyset.id), keyset.id.to_string().as_str())
                .map_err(Error::from)?;

            let existing_u32 = match existing {
                None => false,
                Some(id) => {
                    let id = Id::from_str(id.value())?;
                    id != keyset.id
                }
            };

            if existing_keys || existing_u32 {
                tracing::warn!("Keys already exist for keyset id");
                return Err(database::Error::Duplicate);
            }
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn remove_keys(&self, keyset_id: &Id) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

            table
                .remove(keyset_id.to_string().as_str())
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn
                .open_table(TRANSACTIONS_TABLE)
                .map_err(Error::from)?;
            table
                .remove(transaction_id.as_slice())
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn add_saga(&self, saga: wallet::WalletSaga) -> Result<(), database::Error> {
        let saga_json = serde_json::to_string(&saga).map_err(Error::from)?;
        let id_str = saga.id.to_string();

        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn.open_table(SAGAS_TABLE).map_err(Error::from)?;
            table
                .insert(id_str.as_str(), saga_json.as_str())
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_saga(
        &self,
        id: &uuid::Uuid,
    ) -> Result<Option<wallet::WalletSaga>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(SAGAS_TABLE).map_err(Error::from)?;
        let id_str = id.to_string();

        let result = table
            .get(id_str.as_str())
            .map_err(Error::from)?
            .map(|saga| serde_json::from_str(saga.value()).map_err(Error::from))
            .transpose()?;

        Ok(result)
    }

    #[instrument(skip(self))]
    async fn update_saga(&self, saga: wallet::WalletSaga) -> Result<bool, database::Error> {
        let id_str = saga.id.to_string();

        // The saga.version has already been incremented by the caller, so we check
        // for (saga.version - 1) as the expected version in the database.
        let expected_version = saga.version.saturating_sub(1);

        let write_txn = self.db.begin_write().map_err(Error::from)?;
        let updated = {
            let mut table = write_txn.open_table(SAGAS_TABLE).map_err(Error::from)?;

            // Read existing saga to check version (optimistic locking)
            let existing_saga_json = table
                .get(id_str.as_str())
                .map_err(Error::from)?
                .map(|v| v.value().to_string());

            match existing_saga_json {
                Some(json) => {
                    let existing_saga: wallet::WalletSaga =
                        serde_json::from_str(&json).map_err(Error::from)?;

                    // Check if version matches expected version
                    if existing_saga.version != expected_version {
                        // Version mismatch - another instance modified it
                        false
                    } else {
                        // Version matches - safe to update
                        let saga_json = serde_json::to_string(&saga).map_err(Error::from)?;
                        table
                            .insert(id_str.as_str(), saga_json.as_str())
                            .map_err(Error::from)?;
                        true
                    }
                }
                None => {
                    // Saga doesn't exist - can't update
                    false
                }
            }
        };
        write_txn.commit().map_err(Error::from)?;
        Ok(updated)
    }

    #[instrument(skip(self))]
    async fn delete_saga(&self, id: &uuid::Uuid) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        let id_str = id.to_string();
        {
            let mut table = write_txn.open_table(SAGAS_TABLE).map_err(Error::from)?;
            table.remove(id_str.as_str()).map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_incomplete_sagas(&self) -> Result<Vec<wallet::WalletSaga>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(SAGAS_TABLE).map_err(Error::from)?;

        let mut sagas: Vec<wallet::WalletSaga> = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .filter_map(|(_, saga_json)| {
                serde_json::from_str::<wallet::WalletSaga>(saga_json.value()).ok()
            })
            .collect();

        // Sort by created_at ascending (oldest first)
        sagas.sort_by_key(|saga| saga.created_at);

        Ok(sagas)
    }

    #[instrument(skip(self))]
    async fn reserve_proofs(
        &self,
        ys: Vec<PublicKey>,
        operation_id: &uuid::Uuid,
    ) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

            for y in ys {
                let y_bytes = y.to_bytes();

                // Read the proof and convert to string immediately
                let proof_json_str = {
                    let proof_json_opt = table.get(y_bytes.as_slice()).map_err(Error::from)?;
                    proof_json_opt.map(|proof_json| proof_json.value().to_string())
                };

                let Some(proof_json_str) = proof_json_str else {
                    return Err(database::Error::ProofNotUnspent);
                };

                let mut proof: ProofInfo =
                    serde_json::from_str(&proof_json_str).map_err(Error::from)?;

                if proof.state != State::Unspent {
                    return Err(database::Error::ProofNotUnspent);
                }

                proof.state = State::Reserved;
                proof.used_by_operation = Some(*operation_id);

                let updated_json = serde_json::to_string(&proof).map_err(Error::from)?;
                table
                    .insert(y_bytes.as_slice(), updated_json.as_str())
                    .map_err(Error::from)?;
            }
        }

        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn release_proofs(&self, operation_id: &uuid::Uuid) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

            // Collect all proofs first to avoid borrowing issues
            let all_proofs: Vec<(Vec<u8>, ProofInfo)> = table
                .iter()
                .map_err(Error::from)?
                .flatten()
                .filter_map(|(y, proof_json)| {
                    let proof: ProofInfo = serde_json::from_str(proof_json.value()).ok()?;
                    Some((y.value().to_vec(), proof))
                })
                .collect();

            // Now update proofs that match the operation_id
            for (y_bytes, mut proof) in all_proofs {
                if proof.used_by_operation == Some(*operation_id) {
                    proof.state = State::Unspent;
                    proof.used_by_operation = None;

                    let updated_json = serde_json::to_string(&proof).map_err(Error::from)?;
                    table
                        .insert(y_bytes.as_slice(), updated_json.as_str())
                        .map_err(Error::from)?;
                }
            }
        }

        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_reserved_proofs(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<ProofInfo>, database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

        let proofs: Vec<ProofInfo> = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .filter_map(|(_, proof_json)| {
                serde_json::from_str::<ProofInfo>(proof_json.value()).ok()
            })
            .filter(|proof| proof.used_by_operation == Some(*operation_id))
            .collect();

        Ok(proofs)
    }

    #[instrument(skip(self))]
    async fn reserve_melt_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        let operation_id_str = operation_id.to_string();

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;

            // Read existing quote
            let quote_json = table
                .get(quote_id)
                .map_err(Error::from)?
                .map(|v| v.value().to_string());

            match quote_json {
                Some(json) => {
                    let mut quote: wallet::MeltQuote =
                        serde_json::from_str(&json).map_err(Error::from)?;

                    // Check if already reserved by another operation
                    if quote.used_by_operation.is_some() {
                        return Err(database::Error::QuoteAlreadyInUse);
                    }

                    // Reserve the quote
                    quote.used_by_operation = Some(operation_id_str);
                    let updated_json = serde_json::to_string(&quote).map_err(Error::from)?;
                    table
                        .insert(quote_id, updated_json.as_str())
                        .map_err(Error::from)?;
                }
                None => {
                    return Err(database::Error::UnknownQuote);
                }
            }
        }

        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn release_melt_quote(&self, operation_id: &uuid::Uuid) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        let operation_id_str = operation_id.to_string();

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;

            // Collect all quotes first to avoid borrowing issues
            let all_quotes: Vec<(String, wallet::MeltQuote)> = table
                .iter()
                .map_err(Error::from)?
                .flatten()
                .filter_map(|(id, quote_json)| {
                    let quote: wallet::MeltQuote = serde_json::from_str(quote_json.value()).ok()?;
                    Some((id.value().to_string(), quote))
                })
                .collect();

            // Update quotes that match the operation_id
            for (quote_id, mut quote) in all_quotes {
                if quote.used_by_operation.as_deref() == Some(&operation_id_str) {
                    quote.used_by_operation = None;
                    let updated_json = serde_json::to_string(&quote).map_err(Error::from)?;
                    table
                        .insert(quote_id.as_str(), updated_json.as_str())
                        .map_err(Error::from)?;
                }
            }
        }

        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn reserve_mint_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        let operation_id_str = operation_id.to_string();

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;

            // Read existing quote
            let quote_json = table
                .get(quote_id)
                .map_err(Error::from)?
                .map(|v| v.value().to_string());

            match quote_json {
                Some(json) => {
                    let mut quote: MintQuote = serde_json::from_str(&json).map_err(Error::from)?;

                    // Check if already reserved by another operation
                    if quote.used_by_operation.is_some() {
                        return Err(database::Error::QuoteAlreadyInUse);
                    }

                    // Reserve the quote
                    quote.used_by_operation = Some(operation_id_str);
                    let updated_json = serde_json::to_string(&quote).map_err(Error::from)?;
                    table
                        .insert(quote_id, updated_json.as_str())
                        .map_err(Error::from)?;
                }
                None => {
                    return Err(database::Error::UnknownQuote);
                }
            }
        }

        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn release_mint_quote(&self, operation_id: &uuid::Uuid) -> Result<(), database::Error> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        let operation_id_str = operation_id.to_string();

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;

            // Collect all quotes first to avoid borrowing issues
            let all_quotes: Vec<(String, MintQuote)> = table
                .iter()
                .map_err(Error::from)?
                .flatten()
                .filter_map(|(id, quote_json)| {
                    let quote: MintQuote = serde_json::from_str(quote_json.value()).ok()?;
                    Some((id.value().to_string(), quote))
                })
                .collect();

            // Update quotes that match the operation_id
            for (quote_id, mut quote) in all_quotes {
                if quote.used_by_operation.as_deref() == Some(&operation_id_str) {
                    quote.used_by_operation = None;
                    let updated_json = serde_json::to_string(&quote).map_err(Error::from)?;
                    table
                        .insert(quote_id.as_str(), updated_json.as_str())
                        .map_err(Error::from)?;
                }
            }
        }

        write_txn.commit().map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self, value))]
    async fn kv_write(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), database::Error> {
        // Validate parameters according to KV store requirements
        validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;

        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn.open_table(KV_STORE_TABLE).map_err(Error::from)?;
            table
                .insert((primary_namespace, secondary_namespace, key), value)
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, database::Error> {
        // Validate parameters according to KV store requirements
        validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;

        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(KV_STORE_TABLE).map_err(Error::from)?;

        let result = table
            .get((primary_namespace, secondary_namespace, key))
            .map_err(Error::from)?
            .map(|v| v.value().to_vec());

        Ok(result)
    }

    #[instrument(skip(self))]
    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, database::Error> {
        // Validate parameters according to KV store requirements
        validate_kvstore_params(primary_namespace, secondary_namespace, None)?;

        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(KV_STORE_TABLE).map_err(Error::from)?;

        let start = (primary_namespace, secondary_namespace, "");
        let iter = table.range(start..).map_err(Error::from)?;

        let mut keys = Vec::new();

        for item in iter {
            let (key, _) = item.map_err(Error::from)?;
            let (p, s, k) = key.value();
            if p == primary_namespace && s == secondary_namespace {
                keys.push(k.to_string());
            } else {
                break;
            }
        }

        Ok(keys)
    }

    #[instrument(skip(self))]
    async fn kv_remove(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), database::Error> {
        // Validate parameters according to KV store requirements
        validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;

        let write_txn = self.db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn.open_table(KV_STORE_TABLE).map_err(Error::from)?;
            table
                .remove((primary_namespace, secondary_namespace, key))
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use cdk_common::wallet_db_test;

    use super::WalletRedbDatabase;

    async fn provide_db(test_id: String) -> WalletRedbDatabase {
        let path = PathBuf::from(format!("/tmp/cdk-test-{}.redb", test_id));
        WalletRedbDatabase::new(&path).expect("database")
    }

    wallet_db_test!(provide_db);
}
