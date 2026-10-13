//! Mint identity, separated from the mint URL.
//!
//! A mint URL is a mutable attribute: a mint that announces a new endpoint
//! (NUT-06 `urls`) is still the same mint. Records therefore carry an internal
//! mint id in place of `mint_url`, and moving a mint rewrites only its own row.

use std::collections::{HashMap, HashSet};

use cdk_common::common::ProofInfo;
use cdk_common::mint_url::MintUrl;
use cdk_common::wallet::{MintQuote, Transaction, WalletSaga};
use cdk_common::{wallet, MintInfo};
use redb::{ReadableTable, TableDefinition, WriteTransaction};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::Error;

/// <mint id, [`StoredMint`]>
pub const MINTS_TABLE: TableDefinition<u64, &str> = TableDefinition::new("mints_by_id");

/// <mint URL, mint id>
///
/// The mints table is keyed by id, so without this every URL resolution would
/// be a full scan. A removed mint keeps its entry here: it still owns its URL,
/// so a write lands on its row instead of starting a second mint at the same
/// URL, and a move onto that URL is still refused as taken.
pub const MINT_IDS_TABLE: TableDefinition<&str, u64> = TableDefinition::new("mint_ids_by_url");

/// A mint as stored: the table key is its identity, the URL is data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMint {
    /// URL the mint is currently reached at
    pub mint_url: MintUrl,
    /// Last known mint info
    pub mint_info: Option<MintInfo>,
    /// When the mint was removed, if it was.
    ///
    /// A removed mint keeps its row and its id so the records attached to it
    /// survive; it is hidden from every read instead.
    #[serde(default)]
    pub removed_at: Option<u64>,
}

/// The fields of a [`StoredMint`] the index actually reads.
///
/// Deserializing the whole row would build the mint's entire [`MintInfo`], the
/// NUT-06 `nuts` blob included, only to drop it. Serde skips the rest
/// syntactically, which is what makes reading the mints table cheap enough to
/// do per operation.
#[derive(Debug, Deserialize)]
struct MintRef {
    mint_url: MintUrl,
    #[serde(default)]
    removed_at: Option<u64>,
}

/// A record on its way to storage, paired with the id of the mint it belongs to.
///
/// The record is serialized whole, its own `mint_url` included. That URL is
/// dead weight: `mint_id` identifies the mint, and a move rewrites only the
/// mint's own row, so a stored URL is whatever was current when the record was
/// written. [`MintIndex::decode`] puts the mint's real URL back.
#[derive(Debug, Serialize)]
struct StoredRecord<'a, T> {
    mint_id: Option<u64>,
    record: &'a T,
}

/// A record as read back, still carrying the id it was stored under.
#[derive(Debug, Deserialize)]
struct LoadedRecord<T> {
    #[serde(default)]
    mint_id: Option<u64>,
    record: T,
}

/// The mint id of a stored record, read without building the record.
///
/// Lets a scan skip a row belonging to a mint the caller did not ask for
/// before paying to deserialize it.
#[derive(Debug, Deserialize)]
pub struct MintIdProbe {
    /// Id of the mint the record belongs to, absent for a record with no mint.
    #[serde(default)]
    pub mint_id: Option<u64>,
}

/// A record that names the mint it belongs to.
///
/// Storage keeps the mint id, not the URL, so a read has to put the URL back.
/// Doing it through this trait rather than through a `serde_json::Value` field
/// swap keeps encode and decode to one serde pass each.
pub trait MintScoped {
    /// URL of the mint this record belongs to, if it has one.
    fn mint_url(&self) -> Option<&MintUrl>;
    /// Replace the mint URL with the one the mint is currently reached at.
    ///
    /// Fails when a record whose mint is not optional was stored without one,
    /// which means the row and the mints table disagree.
    fn set_mint_url(&mut self, mint_url: Option<MintUrl>) -> Result<(), Error>;
}

macro_rules! mint_scoped {
    ($type:ty) => {
        impl MintScoped for $type {
            fn mint_url(&self) -> Option<&MintUrl> {
                Some(&self.mint_url)
            }

            fn set_mint_url(&mut self, mint_url: Option<MintUrl>) -> Result<(), Error> {
                self.mint_url = mint_url.ok_or(Error::MintReference)?;
                Ok(())
            }
        }
    };
}

mint_scoped!(ProofInfo);
mint_scoped!(MintQuote);
mint_scoped!(Transaction);
mint_scoped!(WalletSaga);

impl MintScoped for wallet::MeltQuote {
    fn mint_url(&self) -> Option<&MintUrl> {
        self.mint_url.as_ref()
    }

    fn set_mint_url(&mut self, mint_url: Option<MintUrl>) -> Result<(), Error> {
        self.mint_url = mint_url;
        Ok(())
    }
}

/// Both directions of the mint id to URL mapping, for one operation.
///
/// A removed mint is in both maps: it still owns its id and its URL, so its
/// records stay readable and a write lands on its row instead of creating a
/// second mint at the same URL, matching the `ON CONFLICT(mint_url)` upsert the
/// SQL backends do. `removed` is what makes it invisible, and only
/// [`MintIndex::decode_visible`] and [`MintIndex::live_id`] act on it. Hiding a
/// mint by failing to resolve it instead would turn every write that reads a
/// record back into a hard error.
#[derive(Debug, Default)]
pub struct MintIndex {
    by_id: HashMap<u64, MintUrl>,
    by_url: HashMap<MintUrl, u64>,
    removed: HashSet<u64>,
}

impl MintIndex {
    /// Read the whole mapping from the mints table.
    ///
    /// For an operation that walks a table: its rows can belong to any mint, so
    /// every id is wanted and reading them one at a time would cost more than
    /// the scan. An operation that knows which mints it touches wants
    /// [`MintIndex::for_urls`] or [`MintIndex::read_ensuring`] instead.
    pub fn read<T>(table: &T) -> Result<Self, Error>
    where
        T: ReadableTable<u64, &'static str>,
    {
        let mut index = Self::default();

        for entry in table.iter()? {
            let (id, mint) = entry?;
            let mint: MintRef = serde_json::from_str(mint.value())?;

            index.insert(id.value(), mint);
        }

        Ok(index)
    }

    /// Resolve only the mints named by `mint_urls`, through the URL index.
    ///
    /// An unknown URL is simply absent, which [`MintIndex::id`] reports as
    /// [`Error::UnknownMint`] and [`MintIndex::live_id`] as `None`.
    pub fn for_urls<'a, I, T, U>(mints: &T, mint_ids: &U, mint_urls: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = &'a MintUrl>,
        T: ReadableTable<u64, &'static str>,
        U: ReadableTable<&'static str, u64>,
    {
        let mut index = Self::default();

        for mint_url in mint_urls {
            let Some(mint_id) = mint_ids.get(mint_url.to_string().as_str())? else {
                continue;
            };
            let mint_id = mint_id.value();

            if let Some(mint) = mints.get(mint_id)? {
                index.insert(mint_id, serde_json::from_str(mint.value())?);
            }
        }

        Ok(index)
    }

    /// Resolve the mints named by `mint_urls`, storing any that is not known yet.
    ///
    /// A wallet is built synchronously and can be handed an empty database, so
    /// it has no opportunity to register its mint before its first write; the
    /// store creates the mint on demand instead. A created mint holds only its
    /// URL, leaving metadata to [`super::WalletRedbDatabase::add_mint`], which
    /// would otherwise overwrite what is already there.
    ///
    /// New ids come from the mints table, which holds removed mints too:
    /// handing a removed mint's id out again would give the new mint every
    /// record the removed one left behind.
    pub fn read_ensuring<'a, I>(txn: &WriteTransaction, mint_urls: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = &'a MintUrl>,
    {
        let mut mints = txn.open_table(MINTS_TABLE)?;
        let mut mint_ids = txn.open_table(MINT_IDS_TABLE)?;
        let mut index = Self::default();

        for mint_url in mint_urls {
            if index.by_url.contains_key(mint_url) {
                continue;
            }

            let key = mint_url.to_string();

            if let Some(mint_id) = mint_ids.get(key.as_str())? {
                let mint_id = mint_id.value();

                if let Some(mint) = mints.get(mint_id)? {
                    index.insert(mint_id, serde_json::from_str(mint.value())?);
                    continue;
                }
            }

            let mint_id = next_mint_id(&mints)?;
            let mint = StoredMint {
                mint_url: mint_url.clone(),
                mint_info: None,
                removed_at: None,
            };

            mints.insert(mint_id, serde_json::to_string(&mint)?.as_str())?;
            mint_ids.insert(key.as_str(), mint_id)?;

            index.by_id.insert(mint_id, mint_url.clone());
            index.by_url.insert(mint_url.clone(), mint_id);
        }

        Ok(index)
    }

    fn insert(&mut self, mint_id: u64, mint: MintRef) {
        if mint.removed_at.is_some() {
            self.removed.insert(mint_id);
        }

        self.by_id.insert(mint_id, mint.mint_url.clone());
        self.by_url.insert(mint.mint_url, mint_id);
    }

    /// Id a record for this URL belongs to, a removed mint included.
    ///
    /// This is the write-side lookup. Reads want [`MintIndex::live_id`].
    pub fn id(&self, mint_url: &MintUrl) -> Result<u64, Error> {
        self.by_url
            .get(mint_url)
            .copied()
            .ok_or_else(|| Error::UnknownMint(mint_url.to_string()))
    }

    /// Id of the mint reachable at this URL, or `None` once it is removed.
    pub fn live_id(&self, mint_url: &MintUrl) -> Option<u64> {
        self.by_url
            .get(mint_url)
            .copied()
            .filter(|mint_id| !self.removed.contains(mint_id))
    }

    /// URL a mint is currently reached at, a removed mint included.
    pub fn url(&self, mint_id: u64) -> Result<&MintUrl, Error> {
        self.by_id
            .get(&mint_id)
            .ok_or_else(|| Error::UnknownMint(mint_id.to_string()))
    }

    /// Whether a record stored under this id belongs to a removed mint.
    pub fn is_removed(&self, mint_id: Option<u64>) -> bool {
        mint_id.is_some_and(|mint_id| self.removed.contains(&mint_id))
    }

    /// Serialize a record alongside the id of the mint it belongs to.
    pub fn encode<T>(&self, record: &T) -> Result<String, Error>
    where
        T: Serialize + MintScoped,
    {
        let mint_id = record
            .mint_url()
            .map(|mint_url| self.id(mint_url))
            .transpose()?;

        Ok(serde_json::to_string(&StoredRecord { mint_id, record })?)
    }

    /// Deserialize a record, or `None` if the mint it belongs to was removed.
    ///
    /// Every read a caller can observe goes through here. Separate from
    /// [`MintIndex::decode`] so a removed mint stays distinguishable from a
    /// record that cannot be read at all.
    pub fn decode_visible<T>(&self, stored: &str) -> Result<Option<T>, Error>
    where
        T: DeserializeOwned + MintScoped,
    {
        let loaded: LoadedRecord<T> = serde_json::from_str(stored)?;

        if self.is_removed(loaded.mint_id) {
            return Ok(None);
        }

        self.restore(loaded).map(Some)
    }

    /// Deserialize a record, putting the mint's current URL back in place of
    /// its id.
    ///
    /// Resolves a removed mint like any other, so a write that reads a record
    /// back to update it works on a hidden row the way the SQL backends' plain
    /// `UPDATE` does.
    pub fn decode<T>(&self, stored: &str) -> Result<T, Error>
    where
        T: DeserializeOwned + MintScoped,
    {
        self.restore(serde_json::from_str(stored)?)
    }

    fn restore<T>(&self, loaded: LoadedRecord<T>) -> Result<T, Error>
    where
        T: MintScoped,
    {
        let mut record = loaded.record;
        let mint_url = loaded
            .mint_id
            .map(|mint_id| self.url(mint_id).cloned())
            .transpose()?;

        record.set_mint_url(mint_url)?;

        Ok(record)
    }
}

/// The id to give the next mint: one past the highest ever handed out.
///
/// Reusing an id a removed mint still holds would give the new mint every
/// record the removed one left behind.
pub fn next_mint_id<T>(mints: &T) -> Result<u64, Error>
where
    T: ReadableTable<u64, &'static str>,
{
    Ok(mints.last()?.map(|(id, _)| id.value()).unwrap_or_default() + 1)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cdk_common::nuts::{CurrencyUnit, Id, SecretKey, State};
    use cdk_common::secret::Secret;
    use cdk_common::Amount;
    use redb::{Database, ReadableDatabase};

    use super::*;

    fn mint_url(host: &str) -> MintUrl {
        MintUrl::from_str(&format!("https://{host}.example.com")).expect("valid mint URL")
    }

    fn database() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = Database::create(directory.path().join("wallet.redb")).expect("database");

        (directory, database)
    }

    fn proof_info(mint_url: &MintUrl) -> ProofInfo {
        let proof = cdk_common::nuts::Proof {
            amount: Amount::from(1),
            keyset_id: Id::from_str("00916bbf7ef91a36").expect("valid keyset id"),
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        };

        ProofInfo::new(proof, mint_url.clone(), State::Unspent, CurrencyUnit::Sat)
            .expect("proof info")
    }

    /// Stamp `removed_at` on a mint the way `remove_mint` does.
    fn remove(database: &Database, mint_id: u64) {
        let write_txn = database.begin_write().expect("write transaction");
        {
            let mut mints = write_txn.open_table(MINTS_TABLE).expect("mints table");
            let stored = {
                let stored = mints
                    .get(mint_id)
                    .expect("mint lookup")
                    .expect("stored mint");
                let mut mint: StoredMint =
                    serde_json::from_str(stored.value()).expect("stored mint");
                mint.removed_at = Some(1);
                serde_json::to_string(&mint).expect("stored mint")
            };
            mints.insert(mint_id, stored.as_str()).expect("insert mint");
        }
        write_txn.commit().expect("commit");
    }

    fn indexed(database: &Database, mint_url: &MintUrl) -> Option<u64> {
        let read_txn = database.begin_read().expect("read transaction");
        let mint_ids = read_txn.open_table(MINT_IDS_TABLE).expect("mint ids table");
        let mint_id = mint_ids
            .get(mint_url.to_string().as_str())
            .expect("mint id lookup")
            .map(|mint_id| mint_id.value());

        mint_id
    }

    fn ensure(database: &Database, mint_urls: &[&MintUrl]) -> MintIndex {
        let write_txn = database.begin_write().expect("write transaction");
        let index =
            MintIndex::read_ensuring(&write_txn, mint_urls.iter().copied()).expect("ensure mints");
        write_txn.commit().expect("commit");

        index
    }

    #[test]
    fn ensuring_a_mint_writes_both_tables_and_is_idempotent() {
        let (_directory, database) = database();
        let url = mint_url("first");

        let mint_id = ensure(&database, &[&url]).id(&url).expect("mint id");

        assert_eq!(indexed(&database, &url), Some(mint_id));
        assert_eq!(
            ensure(&database, &[&url]).id(&url).expect("mint id"),
            mint_id,
            "a second write reuses the id its records point at"
        );
    }

    #[test]
    fn a_removed_mint_keeps_its_index_entry_and_its_id() {
        let (_directory, database) = database();
        let url = mint_url("first");

        let mint_id = ensure(&database, &[&url]).id(&url).expect("mint id");
        remove(&database, mint_id);

        assert_eq!(
            indexed(&database, &url),
            Some(mint_id),
            "the URL still belongs to the removed mint"
        );
        assert_eq!(
            ensure(&database, &[&url]).id(&url).expect("mint id"),
            mint_id,
            "a write lands on the removed mint rather than starting a second one"
        );
    }

    #[test]
    fn a_new_mint_never_reuses_a_removed_mint_id() {
        let (_directory, database) = database();
        let first = mint_url("first");
        let second = mint_url("second");

        let first_id = ensure(&database, &[&first]).id(&first).expect("mint id");
        remove(&database, first_id);

        let second_id = ensure(&database, &[&second]).id(&second).expect("mint id");

        assert_ne!(
            second_id, first_id,
            "reusing the id would hand the new mint the removed one's records"
        );
    }

    #[test]
    fn the_index_and_the_mints_table_agree() {
        let (_directory, database) = database();
        let first = mint_url("first");
        let second = mint_url("second");

        ensure(&database, &[&first, &second]);
        remove(&database, 1);

        let read_txn = database.begin_read().expect("read transaction");
        let mints = read_txn.open_table(MINTS_TABLE).expect("mints table");
        let mint_ids = read_txn.open_table(MINT_IDS_TABLE).expect("mint ids table");

        let mut entries = 0;
        for entry in mint_ids.iter().expect("mint id scan") {
            let (url, mint_id) = entry.expect("mint id entry");
            let stored = mints
                .get(mint_id.value())
                .expect("mint lookup")
                .expect("stored mint");
            let mint: StoredMint = serde_json::from_str(stored.value()).expect("stored mint");

            assert_eq!(mint.mint_url.to_string(), url.value());
            entries += 1;
        }

        assert_eq!(entries, mints.iter().expect("mint scan").count());
    }

    #[test]
    fn a_record_reads_back_under_the_mints_current_url() {
        let (_directory, database) = database();
        let url = mint_url("first");
        let moved = mint_url("moved");

        let index = ensure(&database, &[&url]);
        let stored = index.encode(&proof_info(&url)).expect("encode");

        let write_txn = database.begin_write().expect("write transaction");
        {
            let mut mints = write_txn.open_table(MINTS_TABLE).expect("mints table");
            let mint_id = index.id(&url).expect("mint id");
            let mint = StoredMint {
                mint_url: moved.clone(),
                mint_info: None,
                removed_at: None,
            };
            mints
                .insert(
                    mint_id,
                    serde_json::to_string(&mint).expect("mint").as_str(),
                )
                .expect("insert mint");
        }
        write_txn.commit().expect("commit");

        let read_txn = database.begin_read().expect("read transaction");
        let renamed = MintIndex::read(&read_txn.open_table(MINTS_TABLE).expect("mints table"))
            .expect("index");
        let decoded: ProofInfo = renamed.decode(&stored).expect("decode");

        assert_eq!(
            decoded.mint_url, moved,
            "the URL stored inside the record is dead weight; the id is what counts"
        );
    }

    #[test]
    fn a_removed_mints_record_is_hidden_from_reads_but_still_decodable() {
        let (_directory, database) = database();
        let url = mint_url("first");

        let index = ensure(&database, &[&url]);
        let stored = index.encode(&proof_info(&url)).expect("encode");
        remove(&database, index.id(&url).expect("mint id"));

        let read_txn = database.begin_read().expect("read transaction");
        let index = MintIndex::read(&read_txn.open_table(MINTS_TABLE).expect("mints table"))
            .expect("index");

        assert!(index
            .decode_visible::<ProofInfo>(&stored)
            .expect("decode visible")
            .is_none());
        assert_eq!(
            index.decode::<ProofInfo>(&stored).expect("decode").mint_url,
            url,
            "a write that reads a record back still resolves a hidden mint"
        );
    }

    #[test]
    fn a_probe_agrees_with_the_decoder() {
        let (_directory, database) = database();
        let url = mint_url("first");

        let index = ensure(&database, &[&url]);
        let stored = index.encode(&proof_info(&url)).expect("encode");

        let probe: MintIdProbe = serde_json::from_str(&stored).expect("probe");

        assert_eq!(probe.mint_id, Some(index.id(&url).expect("mint id")));
    }

    #[test]
    fn a_melt_quote_without_a_mint_round_trips() {
        let (_directory, database) = database();
        let url = mint_url("first");

        let index = ensure(&database, &[&url]);
        let quote = wallet::MeltQuote {
            id: "quote".to_string(),
            mint_url: None,
            unit: CurrencyUnit::Sat,
            amount: Amount::from(1),
            request: "lnbc1...".to_string(),
            fee_reserve: Amount::ZERO,
            state: cdk_common::nuts::MeltQuoteState::Unpaid,
            expiry: 0,
            payment_proof: None,
            estimated_blocks: None,
            fee_index: None,
            payment_method: cdk_common::PaymentMethod::Known(
                cdk_common::nuts::nut00::KnownMethod::Bolt11,
            ),
            used_by_operation: None,
            version: 0,
        };

        let stored = index.encode(&quote).expect("encode");
        let decoded: wallet::MeltQuote = index.decode(&stored).expect("decode");

        assert!(decoded.mint_url.is_none());
    }

    #[test]
    fn a_record_with_no_mint_id_is_refused_when_its_mint_is_required() {
        let (_directory, database) = database();
        let url = mint_url("first");

        let index = ensure(&database, &[&url]);
        let stored = format!(
            r#"{{"record":{}}}"#,
            serde_json::to_string(&proof_info(&url)).expect("record")
        );

        assert!(matches!(
            index.decode::<ProofInfo>(&stored),
            Err(Error::MintReference)
        ));
    }
}
