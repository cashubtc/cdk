//! Wallet based on Nostr [NIP-60](https://github.com/nostr-protocol/nips/pull/1369)

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::Arc,
};

use async_trait::async_trait;
use cdk::{
    cdk_database::{self, WalletDatabase},
    nuts::{
        CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, Proof, Proofs, PublicKey, SecretKey,
        SpendingConditions, State,
    },
    types::ProofInfo,
    wallet::{MeltQuote, MintQuote},
    Amount, UncheckedUrl,
};
use itertools::Itertools;
use nostr_database::{DatabaseError, NostrDatabase};
use nostr_sdk::{
    client, nips::nip44, Client, Event, EventBuilder, EventId, Filter, Kind, SingleLetterTag, Tag,
    TagKind, Timestamp,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, MutexGuard};
use url::Url;

const TX_KIND: Kind = Kind::Custom(7375);
const WALLET_INFO_KIND: Kind = Kind::Custom(37375);
const ID_TAG: char = 'd';
const ID_LINK_TAG: char = 'a';
const MINT_TAG: &str = "mint";
const NAME_TAG: &str = "name";
const UNIT_TAG: &str = "unit";
const DESCRIPTION_TAG: &str = "description";
const RELAY_TAG: &str = "relay";
const BALANCE_TAG: &str = "balance";
const PRIVKEY_TAG: &str = "privkey";
const COUNTER_TAG: &str = "counter";

macro_rules! filter_value {
    ($($value:expr),*) => {
        Some(vec![$($value),*].into_iter().collect())
    };
    ($($key:expr => $value:expr),*,) => {
        vec![$(($key, vec![$value].into_iter().collect())),*].into_iter().collect()
    };
}

/// Wallet on Nostr
#[derive(Clone, Debug)]
pub struct WalletNostrDatabase {
    client: Client,
    keys: nostr_sdk::Keys,
    id: String,
    info: Arc<Mutex<WalletInfo>>,

    // Local disk storage
    nostr_db: Arc<Option<Box<dyn NostrDatabase<Err = DatabaseError>>>>,
    wallet_db: Arc<Box<dyn WalletDatabase<Err = cdk_database::Error> + Sync + Send>>,
}

impl WalletNostrDatabase {
    /// Create a new [`WalletNostrDatabase`] with a local event database
    pub async fn local<E, W>(
        id: String,
        keys: nostr_sdk::Keys,
        relays: Vec<Url>,
        nostr_db: E,
        wallet_db: W,
    ) -> Result<Self, Error>
    where
        E: NostrDatabase<Err = DatabaseError> + 'static,
        W: WalletDatabase<Err = cdk_database::Error> + Sync + Send + 'static,
    {
        let client = Self::connect_client(&keys, relays).await?;
        let self_ = Self {
            client,
            keys,
            id: id.clone(),
            info: Arc::new(Mutex::new(WalletInfo {
                id,
                ..Default::default()
            })),
            nostr_db: Arc::new(Some(Box::new(nostr_db))),
            wallet_db: Arc::new(Box::new(wallet_db)),
        };
        self_.refresh_info().await?;
        Ok(self_)
    }

    /// Create a new [`WalletNostrDatabase`] from remote relays
    pub async fn remote<W>(
        id: String,
        keys: nostr_sdk::Keys,
        relays: Vec<Url>,
        wallet_db: W,
    ) -> Result<Self, Error>
    where
        W: WalletDatabase<Err = cdk_database::Error> + Sync + Send + 'static,
    {
        let client = Self::connect_client(&keys, relays).await?;
        let self_ = Self {
            client,
            keys,
            id: id.clone(),
            info: Arc::new(Mutex::new(WalletInfo {
                id,
                ..Default::default()
            })),
            nostr_db: Arc::new(None),
            wallet_db: Arc::new(Box::new(wallet_db)),
        };
        self_.refresh_info().await?;
        Ok(self_)
    }

    async fn connect_client(keys: &nostr_sdk::Keys, relays: Vec<Url>) -> Result<Client, Error> {
        let client = Client::new(keys);
        client.add_relays(relays).await?;
        client.connect().await;
        Ok(client)
    }

    /// Get the latest [`WalletInfo`]
    pub async fn get_info(&self) -> WalletInfo {
        self.info.lock().await.clone()
    }

    /// Get tx infos
    pub async fn get_txs(
        &self,
        until: Option<Timestamp>,
        limit: Option<usize>,
    ) -> Result<Vec<TxInfo>, Error> {
        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(TX_KIND),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_LINK_TAG).expect("ID_LINK_TAG is not a single letter tag") => wallet_link_tag_value(&self.id, &self.keys),
            ),
            until,
            limit,
            ..Default::default()
        }];
        let events = self.get_events(filters).await?;
        Ok(events
            .into_iter()
            .map(|event| TxInfo::from_event(&event, &self.keys))
            .collect::<Result<Vec<TxInfo>, Error>>()?)
    }

    /// Refresh the latest [`WalletInfo`]
    pub async fn refresh_info(&self) -> Result<WalletInfo, Error> {
        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(WALLET_INFO_KIND),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_TAG).expect("ID_TAG is not a single letter tag") => self.id.clone(),
            ),
            ..Default::default()
        }];
        let events = self.get_events(filters).await?;
        let mut info = self.info.lock().await;
        match events.first() {
            Some(event) => {
                *info = WalletInfo::from_event(event, &self.keys)?;
            }
            None => {
                *info = WalletInfo {
                    id: self.id.clone(),
                    ..Default::default()
                };
                self.save_info_with_lock(&info).await?;
            }
        }
        Ok(info.clone())
    }

    /// Save the latest [`WalletInfo`]
    pub async fn save_info(&self) -> Result<(), Error> {
        self.save_info_with_lock(&self.info.lock().await).await
    }

    async fn save_info_with_lock<'a>(
        &self,
        info: &MutexGuard<'a, WalletInfo>,
    ) -> Result<(), Error> {
        self.save_event(info.to_event(&self.keys)?).await
    }

    /// Save a [`TxInfo`]
    pub async fn save_tx(&self, tx_info: TxInfo) -> Result<EventId, Error> {
        let event = tx_info.to_event(&self.id, &self.keys)?;
        let id = event.id();
        self.save_event(event).await?;
        let mut info = self.info.lock().await;
        tx_info.update_balance(info.balance.get_or_insert(Amount::ZERO));
        self.save_info_with_lock(&info).await?;
        Ok(id)
    }

    /// Update the name or description of the [`WalletInfo`]
    pub async fn update_info(
        &self,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<(), Error> {
        let mut info = self.info.lock().await;
        if let Some(name) = name {
            info.name = Some(name);
        }
        if let Some(description) = description {
            info.description = Some(description);
        }
        self.save_info_with_lock(&info).await
    }

    async fn get_events(&self, filters: Vec<Filter>) -> Result<Vec<Event>, Error> {
        if let Some(db) = self.nostr_db.as_ref() {
            return Ok(db.query(filters, nostr_database::Order::Desc).await?);
        }
        Ok(self.client.get_events_of(filters, None).await?)
    }

    async fn save_event(&self, event: Event) -> Result<(), Error> {
        if let Some(db) = self.nostr_db.as_ref() {
            db.save_event(&event).await?;
        }
        self.client.send_event(event).await?;
        Ok(())
    }
}

#[async_trait]
impl WalletDatabase for WalletNostrDatabase {
    type Err = cdk_database::Error;

    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
        let mut info = self.info.lock().await;
        info.mints.insert(mint_url.clone());
        self.save_info_with_lock(&info).await.map_err(map_err)?;
        self.wallet_db.add_mint(mint_url, mint_info).await
    }

    async fn remove_mint(&self, mint_url: UncheckedUrl) -> Result<(), Self::Err> {
        let mut info = self.info.lock().await;
        info.mints.remove(&mint_url);
        self.save_info_with_lock(&info).await.map_err(map_err)?;
        self.wallet_db.remove_mint(mint_url).await
    }

    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Self::Err> {
        self.wallet_db.get_mint(mint_url).await
    }

    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Self::Err> {
        self.wallet_db.get_mints().await
    }

    async fn update_mint_url(
        &self,
        old_mint_url: UncheckedUrl,
        new_mint_url: UncheckedUrl,
    ) -> Result<(), Self::Err> {
        let mut info = self.info.lock().await;
        info.mints.remove(&old_mint_url);
        info.mints.insert(new_mint_url.clone());
        self.save_info_with_lock(&info).await.map_err(map_err)?;
        self.wallet_db
            .update_mint_url(old_mint_url, new_mint_url)
            .await
    }

    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err> {
        self.wallet_db.add_mint_keysets(mint_url, keysets).await
    }

    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err> {
        self.wallet_db.get_mint_keysets(mint_url).await
    }

    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err> {
        self.wallet_db.get_keyset_by_id(keyset_id).await
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        self.wallet_db.add_mint_quote(quote).await
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        self.wallet_db.get_mint_quote(quote_id).await
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        self.wallet_db.get_mint_quotes().await
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        self.wallet_db.remove_mint_quote(quote_id).await
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        self.wallet_db.add_melt_quote(quote).await
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
        self.wallet_db.get_melt_quote(quote_id).await
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        self.wallet_db.remove_melt_quote(quote_id).await
    }

    async fn add_keys(&self, keys: Keys) -> Result<(), Self::Err> {
        self.wallet_db.add_keys(keys).await
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err> {
        self.wallet_db.get_keys(id).await
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), Self::Err> {
        self.wallet_db.remove_keys(id).await
    }

    async fn add_proofs(&self, proof_info: Vec<ProofInfo>) -> Result<(), Self::Err> {
        self.wallet_db.add_proofs(proof_info).await
    }

    async fn get_proofs(
        &self,
        mint_url: Option<UncheckedUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Self::Err> {
        self.wallet_db
            .get_proofs(mint_url, unit, state, spending_conditions)
            .await
    }

    async fn remove_proofs(&self, proofs: &Proofs) -> Result<(), Self::Err> {
        self.wallet_db.remove_proofs(proofs).await
    }

    async fn set_proof_state(&self, y: PublicKey, state: State) -> Result<(), Self::Err> {
        self.wallet_db.set_proof_state(y, state).await
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err> {
        self.wallet_db
            .increment_keyset_counter(keyset_id, count)
            .await
    }

    async fn get_keyset_counter(&self, id: &Id) -> Result<Option<u32>, Self::Err> {
        self.wallet_db.get_keyset_counter(id).await
    }

    async fn get_nostr_last_checked(
        &self,
        verifying_key: &PublicKey,
    ) -> Result<Option<u32>, Self::Err> {
        self.wallet_db.get_nostr_last_checked(verifying_key).await
    }

    async fn add_nostr_last_checked(
        &self,
        verifying_key: PublicKey,
        last_checked: u32,
    ) -> Result<(), Self::Err> {
        self.wallet_db
            .add_nostr_last_checked(verifying_key, last_checked)
            .await
    }
}

fn map_err(e: Error) -> cdk_database::Error {
    cdk_database::Error::Database(Box::new(e))
}

/// Wallet info
#[derive(Clone, Debug)]
pub struct WalletInfo {
    /// Wallet id
    pub id: String,
    /// Saved balance
    pub balance: Option<Amount>,
    /// List of mints
    pub mints: HashSet<UncheckedUrl>,
    /// Name
    pub name: Option<String>,
    /// Currency unit
    pub unit: Option<CurrencyUnit>,
    /// Description
    pub description: Option<String>,
    /// List of relays
    pub relays: HashSet<UncheckedUrl>,
    /// NIP-61 private key
    pub p2pk_priv_key: Option<SecretKey>,
    /// Key index counter
    pub counters: HashMap<Id, u32>,
}

impl Default for WalletInfo {
    fn default() -> Self {
        WalletInfo {
            id: String::new(),
            balance: None,
            mints: HashSet::new(),
            name: None,
            unit: Some(CurrencyUnit::Sat),
            description: None,
            relays: HashSet::new(),
            p2pk_priv_key: None,
            counters: HashMap::new(),
        }
    }
}

impl WalletInfo {
    /// Parses a [`WalletInfo`] from an [`Event`]
    pub fn from_event(event: &Event, keys: &nostr_sdk::Keys) -> Result<Self, Error> {
        let id_tag = event.tags().iter().find(|tag| {
            tag.kind()
                == TagKind::SingleLetter(
                    SingleLetterTag::from_char(ID_TAG).expect("ID_TAG is not a single letter tag"),
                )
        });
        let id = id_tag
            .ok_or(Error::TagNotFound(ID_TAG.to_string()))?
            .content()
            .ok_or(Error::EmptyTag(ID_TAG.to_string()))?;
        let mut info = WalletInfo {
            id: id.to_string(),
            ..Default::default()
        };
        let content: Vec<Tag> = serde_json::from_str(&nip44::decrypt(
            keys.secret_key()?,
            &keys.public_key(),
            &event.content,
        )?)?;
        let mut tags = Vec::new();
        tags.extend(event.tags().to_vec());
        tags.extend(content);
        for tag in tags {
            match tag.kind().to_string().as_str() {
                MINT_TAG => {
                    info.mints.insert(UncheckedUrl::from_str(
                        tag.content().ok_or(Error::EmptyTag(MINT_TAG.to_string()))?,
                    )?);
                }
                RELAY_TAG => {
                    info.relays.insert(UncheckedUrl::from_str(
                        tag.content()
                            .ok_or(Error::EmptyTag(RELAY_TAG.to_string()))?,
                    )?);
                }
                UNIT_TAG => {
                    info.unit = Some(CurrencyUnit::from_str(
                        tag.content().ok_or(Error::EmptyTag(UNIT_TAG.to_string()))?,
                    )?);
                }
                NAME_TAG => {
                    info.name = Some(
                        tag.content()
                            .ok_or(Error::EmptyTag(NAME_TAG.to_string()))?
                            .to_string(),
                    );
                }
                DESCRIPTION_TAG => {
                    info.description = Some(
                        tag.content()
                            .ok_or(Error::EmptyTag(DESCRIPTION_TAG.to_string()))?
                            .to_string(),
                    );
                }
                BALANCE_TAG => {
                    let balance = tag
                        .content()
                        .ok_or(Error::EmptyTag(BALANCE_TAG.to_string()))?;
                    info.balance = Some(Amount::from(balance.parse::<u64>()?));
                }
                PRIVKEY_TAG => {
                    let priv_key = tag
                        .content()
                        .ok_or(Error::EmptyTag(PRIVKEY_TAG.to_string()))?;
                    info.p2pk_priv_key = Some(SecretKey::from_str(priv_key)?);
                }
                COUNTER_TAG => {
                    let id = Id::from_str(
                        tag.content()
                            .ok_or(Error::EmptyTag(COUNTER_TAG.to_string()))?,
                    )?;
                    let counter = tag
                        .as_vec()
                        .last()
                        .ok_or(Error::EmptyTag(COUNTER_TAG.to_string()))?
                        .parse::<u32>()?;
                    info.counters.insert(id, counter);
                }
                _ => {}
            }
        }
        Ok(info)
    }

    /// Converts a [`WalletInfo`] to an [`Event`]
    pub fn to_event(&self, keys: &nostr_sdk::Keys) -> Result<Event, Error> {
        let mut content = Vec::new();
        let tags = vec![Tag::parse(&[&ID_TAG.to_string(), &self.id])?];
        if let Some(balance) = &self.balance {
            if let Some(unit) = &self.unit {
                content.push(Tag::parse(&[
                    BALANCE_TAG,
                    &balance.to_string(),
                    &unit.to_string(),
                ])?);
                content.push(Tag::parse(&[UNIT_TAG, &unit.to_string()])?);
            } else {
                content.push(Tag::parse(&[BALANCE_TAG, &balance.to_string()])?);
            }
        }
        for mint in &self.mints {
            content.push(Tag::parse(&[MINT_TAG, &mint.to_string()])?);
        }
        if let Some(name) = &self.name {
            content.push(Tag::parse(&[NAME_TAG, name])?);
        }
        if let Some(description) = &self.description {
            content.push(Tag::parse(&[DESCRIPTION_TAG, description])?);
        }
        for relay in &self.relays {
            content.push(Tag::parse(&[RELAY_TAG, &relay.to_string()])?);
        }
        for (id, counter) in &self.counters {
            content.push(Tag::parse(&[
                COUNTER_TAG,
                &id.to_string(),
                &counter.to_string(),
            ])?);
        }
        let event = EventBuilder::new(
            WALLET_INFO_KIND,
            nip44::encrypt(
                keys.secret_key()?,
                &keys.public_key(),
                serde_json::to_string(&content)?,
                nip44::Version::V2,
            )?,
            tags,
        );
        Ok(event.to_event(keys)?)
    }
}

/// Tx info
#[derive(Debug, Serialize, Deserialize)]
pub struct TxInfo {
    /// Inputs
    pub inputs: Vec<TxInfoProofs>,
    /// Outputs
    pub outputs: Vec<TxInfoProofs>,
}

impl TxInfo {
    /// Create a new [`TxInfo`] from inputs and outputs of [`ProofInfo`]
    pub fn new(inputs: Vec<ProofInfo>, outputs: Vec<ProofInfo>) -> Self {
        let inputs = inputs
            .into_iter()
            .chunk_by(|proof| proof.mint_url.clone())
            .into_iter()
            .map(|(mint_url, chunk)| TxInfoProofs {
                mint_url,
                proofs: chunk.into_iter().map(|proof| proof.proof).collect(),
            })
            .collect();
        let outputs = outputs
            .into_iter()
            .chunk_by(|proof| proof.mint_url.clone())
            .into_iter()
            .map(|(mint_url, chunk)| TxInfoProofs {
                mint_url,
                proofs: chunk.into_iter().map(|proof| proof.proof).collect(),
            })
            .collect();
        Self { inputs, outputs }
    }

    /// Parses a [`TxInfo`] from an [`Event`]
    pub fn from_event(event: &Event, keys: &nostr_sdk::Keys) -> Result<Self, Error> {
        Ok(serde_json::from_str(&nip44::decrypt(
            keys.secret_key()?,
            &keys.public_key(),
            &event.content,
        )?)?)
    }

    /// Converts a [`TxInfo`] to an [`Event`]
    pub fn to_event(&self, wallet_id: &str, keys: &nostr_sdk::Keys) -> Result<Event, Error> {
        let mut tags = Vec::new();
        tags.push(wallet_link_tag(wallet_id, keys)?);

        let event = EventBuilder::new(
            TX_KIND,
            nip44::encrypt(
                keys.secret_key()?,
                &keys.public_key(),
                serde_json::to_string(&self)?,
                nip44::Version::V2,
            )?,
            tags,
        );
        Ok(event.to_event(keys)?)
    }

    /// Update the provided balance [`Amount`]
    pub fn update_balance(&self, balance: &mut Amount) {
        for input in &self.inputs {
            for proof in &input.proofs {
                *balance -= proof.amount;
            }
        }
        for output in &self.outputs {
            for proof in &output.proofs {
                *balance += proof.amount;
            }
        }
    }
}

/// Tx info proofs
#[derive(Debug, Serialize, Deserialize)]
pub struct TxInfoProofs {
    /// Mint url
    pub mint_url: UncheckedUrl,
    /// Proofs
    pub proofs: Vec<Proof>,
}

fn wallet_link_tag(wallet_id: &str, keys: &nostr_sdk::Keys) -> Result<Tag, Error> {
    Ok(Tag::parse(&[
        &ID_LINK_TAG.to_string(),
        &wallet_link_tag_value(wallet_id, keys),
    ])?)
}

fn wallet_link_tag_value(wallet_id: &str, keys: &nostr_sdk::Keys) -> String {
    format!("{}:{}:{}", WALLET_INFO_KIND, keys.public_key(), wallet_id)
}

/// [`WalletNostrDatabase`]` error
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Client error
    #[error(transparent)]
    Client(#[from] client::Error),
    /// Empty tag error
    #[error("Empty tag: {0}")]
    EmptyTag(String),
    /// Encrypt error
    #[error(transparent)]
    Encrypt(#[from] nip44::Error),
    /// Event builder error
    #[error(transparent)]
    EventBuilder(#[from] nostr_sdk::event::builder::Error),
    /// Json error
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Key error
    #[error(transparent)]
    Key(#[from] nostr_sdk::key::Error),
    /// Missing tag error
    #[error("Missing tag: {0}")]
    MissingTag(String),
    /// [`NostrDatabase`] error
    #[error(transparent)]
    NostrDatabase(#[from] DatabaseError),
    /// NUT-00 error
    #[error(transparent)]
    Nut00(#[from] cdk::nuts::nut00::Error),
    /// NUT-01 error
    #[error(transparent)]
    Nut01(#[from] cdk::nuts::nut01::Error),
    /// NUT-02 error
    #[error(transparent)]
    Nut02(#[from] cdk::nuts::nut02::Error),
    /// Parse int error
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    /// Tag error
    #[error(transparent)]
    Tag(#[from] nostr_sdk::event::tag::Error),
    /// Tag not found error
    #[error("Tag not found: {0}")]
    TagNotFound(String),
    /// Url parse error
    #[error(transparent)]
    UrlParse(#[from] cdk::url::Error),
    /// Wallet database error
    #[error(transparent)]
    WalletDatabase(#[from] cdk_database::Error),
}
