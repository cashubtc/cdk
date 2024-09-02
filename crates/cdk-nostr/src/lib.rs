//! Wallet based on Nostr [NIP-60](https://github.com/nostr-protocol/nips/pull/1369)

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::{
    collections::{HashMap, HashSet},
    fmt,
    str::FromStr,
    sync::Arc,
};

use async_trait::async_trait;
use cdk::{
    cdk_database::{self, WalletDatabase, WalletMemoryDatabase},
    mint_url::MintUrl,
    nuts::{
        CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, Proofs, PublicKey, SecretKey,
        SpendingConditions, State,
    },
    types::ProofInfo,
    wallet::{MeltQuote, MintQuote},
    Amount,
};
use itertools::Itertools;
use nostr_database::{DatabaseError, MemoryDatabase, MemoryDatabaseOptions, NostrDatabase};
use nostr_sdk::{
    client, nips::nip44, Client, Event, EventBuilder, EventId, EventSource, Filter, Kind,
    SingleLetterTag, Tag, TagKind, Timestamp,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, MutexGuard};
use url::Url;

const PROOFS_KIND: Kind = Kind::Custom(7375);
const TX_HISTORY_KIND: Kind = Kind::Custom(7376);
const WALLET_INFO_KIND: Kind = Kind::Custom(37375);
const ID_TAG: char = 'd';
const ID_LINK_TAG: char = 'a';
const EVENT_TAG: char = 'e';
const MINT_TAG: &str = "mint";
const NAME_TAG: &str = "name";
const UNIT_TAG: &str = "unit";
const DESCRIPTION_TAG: &str = "description";
const RELAY_TAG: &str = "relay";
const BALANCE_TAG: &str = "balance";
const PRIVKEY_TAG: &str = "privkey";
const COUNTER_TAG: &str = "counter";
const DIRECTION_TAG: &str = "direction";
const AMOUNT_TAG: &str = "amount";

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
    info: Arc<Mutex<(Timestamp, WalletInfo)>>,
    // In-memory cache
    wallet_db: WalletMemoryDatabase,
}

impl WalletNostrDatabase {
    /// Create a new [`WalletNostrDatabase`] with a local event database
    pub async fn local<D>(
        id: String,
        keys: nostr_sdk::Keys,
        relays: Vec<Url>,
        nostr_db: D,
    ) -> Result<Self, Error>
    where
        D: NostrDatabase + 'static,
    {
        let client = Client::builder().signer(&keys).database(nostr_db).build();
        Self::connect_client(&client, relays).await?;
        let mut self_ = Self {
            client,
            keys,
            id: id.clone(),
            info: Arc::new(Mutex::new((
                Timestamp::now(),
                WalletInfo {
                    id,
                    ..Default::default()
                },
            ))),
            wallet_db: WalletMemoryDatabase::default(),
        };
        let info = self_.refresh_info().await?;
        self_.load_db(info).await?;
        self_.sync_proofs().await?;
        Ok(self_)
    }

    /// Create a new [`WalletNostrDatabase`] from remote relays
    pub async fn remote(
        id: String,
        keys: nostr_sdk::Keys,
        relays: Vec<Url>,
    ) -> Result<Self, Error> {
        let client = Client::builder()
            .signer(&keys)
            .database(MemoryDatabase::with_opts(MemoryDatabaseOptions {
                events: true,
                max_events: None,
            }))
            .build();
        Self::connect_client(&client, relays).await?;
        let mut self_ = Self {
            client,
            keys,
            id: id.clone(),
            info: Arc::new(Mutex::new((
                Timestamp::now(),
                WalletInfo {
                    id,
                    ..Default::default()
                },
            ))),
            wallet_db: WalletMemoryDatabase::default(),
        };
        let info = self_.refresh_info().await?;
        self_.load_db(info).await?;
        self_.sync_proofs().await?;
        Ok(self_)
    }

    async fn connect_client(client: &Client, relays: Vec<Url>) -> Result<(), Error> {
        client.add_relays(relays).await?;
        client.connect().await;
        Ok(())
    }

    async fn load_db(&mut self, info: WalletInfo) -> Result<(), Error> {
        let wallet_db =
            WalletMemoryDatabase::new(vec![], vec![], vec![], info.counters, HashMap::new());
        for mint_url in info.mints.iter() {
            wallet_db.add_mint(mint_url.clone(), None).await?;
        }
        self.wallet_db = wallet_db;
        Ok(())
    }

    /// Get the latest [`WalletInfo`]
    pub async fn get_info(&self) -> WalletInfo {
        self.info.lock().await.1.clone()
    }

    /// Get [`TransactionEvent`]s
    #[tracing::instrument(skip(self))]
    pub async fn get_transactions(
        &self,
        until: Option<Timestamp>,
        limit: Option<usize>,
        sync_relays: bool,
    ) -> Result<Vec<TransactionEvent>, Error> {
        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(TX_HISTORY_KIND),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_LINK_TAG).expect("ID_LINK_TAG is not a single letter tag") => wallet_link_tag_value(&self.id, &self.keys),
            ),
            until,
            limit,
            ..Default::default()
        }];
        let events = self.get_events(filters, sync_relays).await?;
        Ok(events
            .into_iter()
            .map(|event| TransactionEvent::from_event(&event, &self.keys))
            .collect::<Result<Vec<TransactionEvent>, Error>>()?)
    }

    /// Refresh all events
    #[tracing::instrument(skip(self))]
    pub async fn refresh_events(&self) -> Result<(), Error> {
        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(PROOFS_KIND, TX_HISTORY_KIND),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_LINK_TAG).expect("ID_LINK_TAG is not a single letter tag") => wallet_link_tag_value(&self.id, &self.keys),
            ),
            ..Default::default()
        }];
        self.get_events(filters, true).await?;
        Ok(())
    }

    /// Refresh the latest [`WalletInfo`]
    #[tracing::instrument(skip(self))]
    pub async fn refresh_info(&self) -> Result<WalletInfo, Error> {
        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(WALLET_INFO_KIND),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_TAG).expect("ID_TAG is not a single letter tag") => self.id.clone(),
            ),
            ..Default::default()
        }];
        let events = self.get_events(filters, true).await?;
        let mut info = self.info.lock().await;
        match events.first() {
            Some(event) => {
                *info = (event.created_at, WalletInfo::from_event(event, &self.keys)?);
            }
            None => {
                *info = (
                    Timestamp::now(),
                    WalletInfo {
                        id: self.id.clone(),
                        ..Default::default()
                    },
                );
                self.save_info_with_lock(&mut info).await?;
            }
        }
        for url in info.1.mints.iter() {
            self.wallet_db.add_mint(url.clone(), None).await?;
        }
        tracing::debug!("Refreshed wallet info: {:?}", info);
        Ok(info.1.clone())
    }

    /// Save the latest [`WalletInfo`]
    #[tracing::instrument(skip(self))]
    pub async fn save_info(&self) -> Result<(), Error> {
        self.save_info_with_lock(&mut self.info.lock().await).await
    }

    async fn save_info_with_lock<'a>(
        &self,
        info: &mut MutexGuard<'a, (Timestamp, WalletInfo)>,
    ) -> Result<(), Error> {
        let mut timestamp = Timestamp::now();
        if timestamp <= info.0 {
            timestamp = info.0 + 1;
            tracing::debug!("Incrementing timestamp to {}", timestamp);
        }
        info.0 = timestamp;
        tracing::debug!("Saving wallet info: {:?}", info);
        let event = info.1.to_event(&self.keys, timestamp)?;
        self.save_event(event).await
    }

    /// Save a [`Transaction`]
    #[tracing::instrument(skip(self))]
    pub async fn save_transaction(&self, tx: Transaction) -> Result<EventId, Error> {
        let event = tx.to_event(&self.id, &self.keys)?;
        let id = event.id();
        self.save_event(event).await?;
        let mut info = self.info.lock().await;
        tx.update_balance(info.1.balance.get_or_insert(Amount::ZERO));
        self.save_info_with_lock(&mut info).await?;
        Ok(id)
    }

    /// Sync proofs from Nostr
    #[tracing::instrument(skip(self))]
    pub async fn sync_proofs(&self) -> Result<(), Error> {
        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(PROOFS_KIND),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_LINK_TAG).expect("ID_LINK_TAG is not a single letter tag") => wallet_link_tag_value(&self.id, &self.keys),
            ),
            ..Default::default()
        }];
        let mut events = self.get_events(filters, true).await?;
        events.sort(); // Ensure events are sorted by timestamp
        for event in events {
            let event = ProofsEvent::from_event(&event, &self.keys)?;
            self.wallet_db.add_mint(event.url.clone(), None).await?;
            self.wallet_db
                .update_proofs(
                    event
                        .added
                        .into_iter()
                        .flat_map(|p| {
                            ProofInfo::new(p, event.url.clone(), State::Unspent, CurrencyUnit::Sat)
                                .ok()
                        })
                        .collect(),
                    event.deleted,
                )
                .await?;
            self.wallet_db.reserve_proofs(event.reserved).await?;
        }
        Ok(())
    }

    /// Update the name or description of the [`WalletInfo`]
    #[tracing::instrument(skip(self))]
    pub async fn update_info(
        &self,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<(), Error> {
        let mut info = self.info.lock().await;
        if let Some(name) = name {
            info.1.name = Some(name);
        }
        if let Some(description) = description {
            info.1.description = Some(description);
        }
        self.save_info_with_lock(&mut info).await
    }

    async fn get_events(
        &self,
        filters: Vec<Filter>,
        sync_relays: bool,
    ) -> Result<Vec<Event>, Error> {
        Ok(self
            .client
            .get_events_of(
                filters,
                if sync_relays {
                    EventSource::both(None)
                } else {
                    EventSource::Database
                },
            )
            .await?)
    }

    async fn save_event(&self, event: Event) -> Result<(), Error> {
        self.client.send_event(event).await?;
        Ok(())
    }
}

#[async_trait]
impl WalletDatabase for WalletNostrDatabase {
    type Err = cdk_database::Error;

    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
        let mut info = self.info.lock().await;
        if info.1.mints.insert(mint_url.clone()) {
            self.save_info_with_lock(&mut info).await.map_err(map_err)?;
        }
        self.wallet_db.add_mint(mint_url, mint_info).await
    }

    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), Self::Err> {
        let mut info = self.info.lock().await;
        if info.1.mints.remove(&mint_url) {
            self.save_info_with_lock(&mut info).await.map_err(map_err)?;
        }
        self.wallet_db.remove_mint(mint_url).await
    }

    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Self::Err> {
        self.wallet_db.get_mint(mint_url).await
    }

    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Self::Err> {
        self.wallet_db.get_mints().await
    }

    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Self::Err> {
        let mut info = self.info.lock().await;
        let removed = info.1.mints.remove(&old_mint_url);
        if info.1.mints.insert(new_mint_url.clone()) || removed {
            self.save_info_with_lock(&mut info).await.map_err(map_err)?;
        }
        self.wallet_db
            .update_mint_url(old_mint_url, new_mint_url)
            .await
    }

    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err> {
        self.wallet_db.add_mint_keysets(mint_url, keysets).await
    }

    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
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

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Self::Err> {
        let added_proofs_by_url = added.iter().into_group_map_by(|info| info.mint_url.clone());
        for (mint_url, proofs) in added_proofs_by_url {
            let event = ProofsEvent {
                url: mint_url.clone(),
                added: proofs.iter().map(|info| info.proof.clone()).collect(),
                deleted: removed_ys.clone(),
                reserved: vec![],
            };
            self.client
                .send_event(event.to_event(&self.id, &self.keys).map_err(map_err)?)
                .await
                .map_err(|e| map_err(e.into()))?;
        }
        self.wallet_db.update_proofs(added, removed_ys).await
    }

    async fn set_pending_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Self::Err> {
        self.wallet_db.set_pending_proofs(ys).await
    }

    async fn reserve_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Self::Err> {
        let proofs = self.get_proofs(None, None, None, None).await?;
        let mint_url = proofs.into_iter().find_map(|info| {
            if info.proof.y().ok() == ys.first().cloned() {
                Some(info.mint_url)
            } else {
                None
            }
        });
        if let Some(mint_url) = mint_url {
            let event = ProofsEvent {
                url: mint_url.clone(),
                added: vec![],
                deleted: vec![],
                reserved: ys.clone(),
            };
            self.client
                .send_event(event.to_event(&self.id, &self.keys).map_err(map_err)?)
                .await
                .map_err(|e| map_err(e.into()))?;
        }
        self.wallet_db.reserve_proofs(ys).await
    }

    async fn set_unspent_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Self::Err> {
        let proof_infos = self
            .get_proofs(None, None, None, None)
            .await?
            .into_iter()
            .filter(|info| {
                if let Ok(y) = info.proof.y() {
                    ys.contains(&y)
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();
        self.update_proofs(proof_infos, vec![]).await
    }

    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Self::Err> {
        self.wallet_db
            .get_proofs(mint_url, unit, state, spending_conditions)
            .await
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err> {
        let mut info = self.info.lock().await;
        info.1
            .counters
            .entry(keyset_id.clone())
            .and_modify(|c| *c += count)
            .or_insert(count);
        self.save_info_with_lock(&mut info).await.map_err(map_err)?;
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
    pub mints: HashSet<MintUrl>,
    /// Name
    pub name: Option<String>,
    /// Currency unit
    pub unit: Option<CurrencyUnit>,
    /// Description
    pub description: Option<String>,
    /// List of relays
    pub relays: HashSet<MintUrl>,
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
                    info.mints.insert(MintUrl::from_str(
                        tag.content().ok_or(Error::EmptyTag(MINT_TAG.to_string()))?,
                    )?);
                }
                RELAY_TAG => {
                    info.relays.insert(MintUrl::from_str(
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
    pub fn to_event(&self, keys: &nostr_sdk::Keys, timestamp: Timestamp) -> Result<Event, Error> {
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
        )
        .custom_created_at(timestamp);
        Ok(event.to_event(keys)?)
    }
}

/// Proofs event
#[derive(Debug, Serialize, Deserialize)]
pub struct ProofsEvent {
    /// Mint url
    pub url: MintUrl,
    /// Added proofs
    #[serde(rename = "a")]
    pub added: Proofs,
    /// Deleted proofs
    #[serde(rename = "d")]
    pub deleted: Vec<PublicKey>,
    /// Updated proofs
    #[serde(rename = "u")]
    pub reserved: Vec<PublicKey>,
}

impl ProofsEvent {
    /// Parses a [`ProofsEvent`] from an [`Event`]
    pub fn from_event(event: &Event, keys: &nostr_sdk::Keys) -> Result<Self, Error> {
        Ok(serde_json::from_str(&nip44::decrypt(
            keys.secret_key()?,
            &keys.public_key(),
            &event.content,
        )?)?)
    }

    /// Parses a [`ProofsEvent`] from an [`Event`]
    pub fn to_event(&self, wallet_id: &str, keys: &nostr_sdk::Keys) -> Result<Event, Error> {
        let mut tags = Vec::new();
        tags.push(wallet_link_tag(wallet_id, keys)?);

        let event = EventBuilder::new(
            PROOFS_KIND,
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
}

/// Tx history
#[derive(Debug)]
pub struct Transaction {
    /// Direction (in for received, out for sent)
    pub direction: Direction,
    /// Amount
    pub amount: Amount,
    /// Event ID of proofs update
    pub event_id: Option<EventId>,
    /// Relay URL
    pub relay: Option<Url>,
}

impl Transaction {
    /// Parses a [`TxHistory`] from an [`Event`]
    pub fn from_event(event: &Event, keys: &nostr_sdk::Keys) -> Result<Self, Error> {
        let mut direction: Option<Direction> = None;
        let mut amount: Option<Amount> = None;
        let mut event_id: Option<EventId> = None;
        let mut relay: Option<Url> = None;

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
                DIRECTION_TAG => {
                    direction = Some(Direction::from_str(
                        tag.content()
                            .ok_or(Error::EmptyTag(DIRECTION_TAG.to_string()))?,
                    )?);
                }
                AMOUNT_TAG => {
                    amount = Some(Amount::from(
                        tag.content()
                            .ok_or(Error::EmptyTag(AMOUNT_TAG.to_string()))?
                            .parse::<u64>()?,
                    ));
                }
                t => {
                    if t == EVENT_TAG.to_string().as_str() {
                        let mut parts = tag.as_vec().into_iter();
                        event_id = Some(EventId::from_str(
                            parts.next().ok_or(Error::EmptyTag(EVENT_TAG.to_string()))?,
                        )?);
                        relay = match parts.next() {
                            Some(relay) => {
                                Some(Url::from_str(relay).map_err(cdk::mint_url::Error::Url)?)
                            }
                            None => None,
                        };
                    }
                }
            }
        }

        Ok(Self {
            direction: direction.ok_or(Error::MissingTag(DIRECTION_TAG.to_string()))?,
            amount: amount.ok_or(Error::MissingTag(AMOUNT_TAG.to_string()))?,
            event_id,
            relay,
        })
    }

    /// Converts a [`TxHistory`] to an [`Event`]
    pub fn to_event(&self, wallet_id: &str, keys: &nostr_sdk::Keys) -> Result<Event, Error> {
        let mut content = Vec::new();
        content.push(Tag::parse(&[DIRECTION_TAG, &self.direction.to_string()])?);
        content.push(Tag::parse(&[AMOUNT_TAG, &self.amount.to_string()])?);
        if let Some(event_id) = &self.event_id {
            match self.relay.as_ref() {
                Some(relay) => {
                    content.push(Tag::parse(&[
                        &EVENT_TAG.to_string(),
                        &event_id.to_string(),
                        &relay.to_string(),
                    ])?);
                }
                None => {
                    content.push(Tag::parse(&[
                        &EVENT_TAG.to_string(),
                        &event_id.to_string(),
                    ])?);
                }
            }
        }

        let mut tags = Vec::new();
        tags.push(wallet_link_tag(wallet_id, keys)?);

        let event = EventBuilder::new(
            TX_HISTORY_KIND,
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

    fn update_balance(&self, balance: &mut Amount) {
        match self.direction {
            Direction::Incoming => *balance += self.amount,
            Direction::Outgoing => *balance -= self.amount,
        }
    }
}

/// Direction of the transaction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Incoming (received)
    Incoming,
    /// Outgoing (sent)
    Outgoing,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Direction::Incoming => write!(f, "in"),
            Direction::Outgoing => write!(f, "out"),
        }
    }
}

impl FromStr for Direction {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "in" => Ok(Direction::Incoming),
            "out" => Ok(Direction::Outgoing),
            _ => Err(Error::TagParse("direction".to_string())),
        }
    }
}

/// Transaction event
pub struct TransactionEvent {
    /// Event ID
    pub event_id: EventId,
    /// Created at
    pub created_at: Timestamp,
    /// Transaction
    pub tx: Transaction,
}

impl TransactionEvent {
    /// Parses a [`TransactionEvent`] from an [`Event`]
    pub fn from_event(event: &Event, keys: &nostr_sdk::Keys) -> Result<Self, Error> {
        Ok(Self {
            event_id: event.id(),
            created_at: event.created_at(),
            tx: Transaction::from_event(event, keys)?,
        })
    }
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
    /// Event id error
    #[error(transparent)]
    EventId(#[from] nostr_sdk::event::id::Error),
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
    /// Tag parse error
    #[error("Tag parse error: {0}")]
    TagParse(String),
    /// Url parse error
    #[error(transparent)]
    UrlParse(#[from] cdk::mint_url::Error),
    /// Wallet database error
    #[error(transparent)]
    WalletDatabase(#[from] cdk_database::Error),
}
