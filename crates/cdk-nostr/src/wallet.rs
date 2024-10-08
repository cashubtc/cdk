//! Wallet based on Nostr [NIP-60](https://github.com/nostr-protocol/nips/pull/1369)

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::{
    collections::{HashMap, HashSet},
    fmt,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use bitcoin::hashes::sha256::Hash as PaymentHash;
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
use hkdf::Hkdf;
use itertools::Itertools;
use nostr_database::{DatabaseError, MemoryDatabase, MemoryDatabaseOptions, NostrDatabase};
use nostr_sdk::{
    client,
    nips::nip44,
    secp256k1::{ecdh::shared_secret_point, Parity},
    Client, Event, EventBuilder, EventSource, Filter, Kind, RelaySendOptions, SingleLetterTag, Tag,
    TagKind, Timestamp,
};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::{Mutex, MutexGuard};
use url::Url;

pub use nostr_sdk::EventId;

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
const PRICE_TAG: &str = "price";
const PROOFS_TAG: &str = "proofs";
const PAYMENT_HASH_TAG: &str = "payment_hash";

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
    last_timestamp: Arc<Mutex<Timestamp>>,
    // In-memory cache
    wallet_db: WalletMemoryDatabase,
}

impl WalletNostrDatabase {
    /// Create a new [`WalletNostrDatabase`] with a local event database
    pub async fn local<D>(
        id: String,
        key: nostr_sdk::SecretKey,
        relays: Vec<Url>,
        nostr_db: D,
    ) -> Result<Self, Error>
    where
        D: NostrDatabase + 'static,
    {
        let keys = nostr_sdk::Keys::new(key);
        let client = Client::builder().signer(&keys).database(nostr_db).build();
        for relay in relays {
            client.add_relay(relay).await?;
        }
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
            last_timestamp: Arc::new(Mutex::new(Timestamp::now())),
            wallet_db: WalletMemoryDatabase::default(),
        };
        let info = self_.refresh_info(false).await?;
        self_.load_db(info).await?;
        self_.sync_proofs(false).await?;
        Ok(self_)
    }

    /// Create a new [`WalletNostrDatabase`] from remote relays
    pub async fn remote(
        id: String,
        key: nostr_sdk::SecretKey,
        relays: Vec<Url>,
    ) -> Result<Self, Error> {
        let keys = nostr_sdk::Keys::new(key);
        let client = Client::builder()
            .signer(&keys)
            .database(MemoryDatabase::with_opts(MemoryDatabaseOptions {
                events: true,
                max_events: None,
            }))
            .build();
        for relay in relays {
            client.add_relay(relay).await?;
        }
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
            last_timestamp: Arc::new(Mutex::new(Timestamp::now())),
            wallet_db: WalletMemoryDatabase::default(),
        };
        let info = self_.refresh_info(true).await?;
        self_.load_db(info).await?;
        self_.sync_proofs(true).await?;
        Ok(self_)
    }

    #[cfg(test)]
    fn test() -> Self {
        let keys = nostr_sdk::Keys::generate();
        let client = Client::builder()
            .signer(&keys)
            .database(MemoryDatabase::with_opts(MemoryDatabaseOptions {
                events: true,
                max_events: None,
            }))
            .build();
        let id = "test".to_string();
        let info = Arc::new(Mutex::new((
            Timestamp::now(),
            WalletInfo {
                id: id.clone(),
                ..Default::default()
            },
        )));
        let wallet_db = WalletMemoryDatabase::default();
        Self {
            client,
            keys,
            id,
            info,
            last_timestamp: Arc::new(Mutex::new(Timestamp::now())),
            wallet_db,
        }
    }

    async fn ensure_relays_connected(&self) {
        let relays = self.client.relays().await;
        for relay in relays.values() {
            if !relay.is_connected().await {
                self.client
                    .connect_with_timeout(Duration::from_secs(5))
                    .await;
            }
            break;
        }
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

    /// Get the [`Client`] for this wallet
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Delete a [`TransactionEvent`] by its [`EventId`].
    ///
    /// *Important*: This will remove the transaction from the wallet's history, but will *not* affect the proofs or balance!
    pub async fn delete_transaction(&self, event_id: EventId) -> Result<(), Error> {
        self.ensure_relays_connected().await;
        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(TX_HISTORY_KIND),
            ids: filter_value!(event_id),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_LINK_TAG).expect("ID_LINK_TAG is not a single letter tag") => wallet_link_tag_value(&self.id, &self.keys),
            ),
            ..Default::default()
        }];
        let events = self.get_events(filters, false).await?;
        if let Some(event) = events.first() {
            self.delete_event(event.id).await?;
            Ok(())
        } else {
            Err(Error::EventNotFound(event_id))
        }
    }

    /// Get the latest [`WalletInfo`]
    pub async fn get_info(&self) -> WalletInfo {
        self.info.lock().await.1.clone()
    }

    /// Get all of the wallet's [`ProofsEvent`]s
    pub async fn get_proofs_events(&self) -> Result<Vec<ProofsEvent>, Error> {
        self.ensure_relays_connected().await;
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
        Ok(events
            .into_iter()
            .map(|event| ProofsEvent::from_event(&event, &self.keys))
            .collect::<Result<Vec<ProofsEvent>, Error>>()?)
    }

    /// Get a transaction by its [`EventId`]
    #[tracing::instrument(skip(self))]
    pub async fn get_transaction(&self, event_id: EventId) -> Result<TransactionEvent, Error> {
        self.ensure_relays_connected().await;
        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(TX_HISTORY_KIND),
            ids: filter_value!(event_id),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_LINK_TAG).expect("ID_LINK_TAG is not a single letter tag") => wallet_link_tag_value(&self.id, &self.keys),
            ),
            ..Default::default()
        }];
        let events = self.get_events(filters, false).await?;
        if let Some(event) = events.first() {
            Ok(TransactionEvent::from_event(&event, &self.keys)?)
        } else {
            Err(Error::EventNotFound(event_id))
        }
    }

    /// Get [`TransactionEvent`]s
    #[tracing::instrument(skip(self))]
    pub async fn get_transactions(
        &self,
        until: Option<Timestamp>,
        limit: Option<usize>,
        sync_relays: bool,
    ) -> Result<Vec<TransactionEvent>, Error> {
        if sync_relays {
            self.ensure_relays_connected().await;
        }

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
        let events = self.get_events(filters, sync_relays).await?; // Events are reverse timestamp sorted

        Ok(events
            .into_iter()
            .map(|event| TransactionEvent::from_event(&event, &self.keys))
            .collect::<Result<Vec<TransactionEvent>, Error>>()?)
    }

    /// Refresh all events
    #[tracing::instrument(skip(self))]
    pub async fn refresh_events(&self) -> Result<(), Error> {
        self.ensure_relays_connected().await;
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
    pub async fn refresh_info(&self, sync_relays: bool) -> Result<WalletInfo, Error> {
        if sync_relays {
            self.ensure_relays_connected().await;
        }
        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(WALLET_INFO_KIND),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_TAG).expect("ID_TAG is not a single letter tag") => self.id.clone(),
            ),
            ..Default::default()
        }];
        let events = self.get_events(filters, sync_relays).await?;
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
        let mut last_timestamp = self.last_timestamp.lock().await;
        let event = tx.to_event(&self.id, &self.keys, *last_timestamp)?;
        let id = event.id;
        *last_timestamp = event.created_at;
        self.save_event(event).await?;
        drop(last_timestamp);

        let mut info = self.info.lock().await;
        let mut wallet_balance = Amount::try_sum(
            self.get_proofs(
                None,
                Some(CurrencyUnit::Sat),
                Some(vec![State::Unspent]),
                None,
            )
            .await?
            .into_iter()
            .map(|info| info.proof.amount),
        )?;
        tx.update_balance(&mut wallet_balance);
        info.1.balance = Some(wallet_balance);
        self.save_info_with_lock(&mut info).await?;
        Ok(id)
    }

    /// Sync proofs from database (and optionally relays) to wallet database.
    #[tracing::instrument(skip(self))]
    pub async fn sync_proofs(&self, sync_relays: bool) -> Result<(), Error> {
        if sync_relays {
            self.ensure_relays_connected().await;
        }

        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(PROOFS_KIND),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_LINK_TAG).expect("ID_LINK_TAG is not a single letter tag") => wallet_link_tag_value(&self.id, &self.keys),
            ),
            ..Default::default()
        }];
        let mut events = self.get_events(filters, sync_relays).await?;
        events.sort(); // Ensure events are sorted by timestamp

        if sync_relays {
            if let Err(e) = self.save_events(events.clone()).await {
                tracing::warn!("Failed to sync proofs to relays: {}", e);
            }
        }

        for event in events {
            let event = ProofsEvent::from_event(&event, &self.keys)?;
            if self.wallet_db.get_mint(event.url.clone()).await?.is_none() {
                self.wallet_db.add_mint(event.url.clone(), None).await?;
            }
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

    /// Sync transactions with relays.
    #[tracing::instrument(skip(self))]
    pub async fn sync_transactions(&self) -> Result<(), Error> {
        self.ensure_relays_connected().await;
        let filters = vec![Filter {
            authors: filter_value!(self.keys.public_key()),
            kinds: filter_value!(TX_HISTORY_KIND),
            generic_tags: filter_value!(
                SingleLetterTag::from_char(ID_LINK_TAG).expect("ID_LINK_TAG is not a single letter tag") => wallet_link_tag_value(&self.id, &self.keys),
            ),
            ..Default::default()
        }];
        let events = self.get_events(filters, true).await?;
        if let Err(e) = self.save_events(events).await {
            tracing::warn!("Failed to sync transactions to relays: {}", e);
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

    async fn delete_event(&self, event_id: EventId) -> Result<(), Error> {
        #[cfg(test)]
        if self.client.relays().await.is_empty() {
            self.client
                .database()
                .delete(Filter::new().id(event_id))
                .await?;
            return Ok(());
        }

        self.client.delete_event(event_id).await?;
        Ok(())
    }

    async fn get_events(
        &self,
        filters: Vec<Filter>,
        sync_relays: bool,
    ) -> Result<Vec<Event>, Error> {
        #[cfg(test)]
        if self.client.relays().await.is_empty() {
            return Ok(self.client.database().query(filters).await?);
        }

        if sync_relays {
            self.ensure_relays_connected().await;
        }

        let events = self
            .client
            .get_events_of(
                filters,
                if sync_relays {
                    EventSource::both(None)
                } else {
                    EventSource::Database
                },
            )
            .await?;
        for event in events.iter() {
            if let Err(e) = self.client.database().save_event(event).await {
                tracing::warn!("Failed to save event to database: {}", e);
            }
        }
        Ok(events)
    }

    async fn save_event(&self, event: Event) -> Result<(), Error> {
        #[cfg(test)]
        if self.client.relays().await.is_empty() {
            self.client.database().save_event(&event).await?;
            return Ok(());
        }

        self.ensure_relays_connected().await;
        self.client.send_event(event).await?;
        Ok(())
    }

    async fn save_events(&self, events: Vec<Event>) -> Result<(), Error> {
        if events.is_empty() {
            return Ok(());
        }

        #[cfg(test)]
        if self.client.relays().await.is_empty() {
            for event in events {
                self.client.database().save_event(&event).await?;
            }
            return Ok(());
        }

        self.ensure_relays_connected().await;
        self.client
            .batch_event(events, RelaySendOptions::default())
            .await?;
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
        let active_proofs = self
            .get_proofs(None, Some(CurrencyUnit::Sat), None, None)
            .await?;
        let added_proofs_by_url = added.iter().into_group_map_by(|info| info.mint_url.clone());
        let removed_proofs_by_url = active_proofs
            .iter()
            .filter_map(|info| match info.proof.y() {
                Ok(y) if removed_ys.contains(&y) => Some(info),
                _ => None,
            })
            .into_group_map_by(|info| info.mint_url.clone());
        let mint_urls = added_proofs_by_url
            .keys()
            .chain(removed_proofs_by_url.keys())
            .collect::<HashSet<_>>();
        for mint_url in mint_urls {
            let added_proofs = added_proofs_by_url
                .get(mint_url)
                .cloned()
                .unwrap_or_default();
            let removed_proofs = removed_proofs_by_url
                .get(mint_url)
                .cloned()
                .unwrap_or_default();
            let event = ProofsEvent {
                url: mint_url.clone(),
                added: added_proofs.iter().map(|info| info.proof.clone()).collect(),
                deleted: removed_proofs.iter().map(|info| info.y).collect(),
                reserved: vec![],
            };
            let mut last_timestamp = self.last_timestamp.lock().await;
            let event = event
                .to_event(&self.id, &self.keys, *last_timestamp)
                .map_err(map_err)?;
            *last_timestamp = event.created_at;
            drop(last_timestamp);
            self.save_event(event)
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
            let mut last_timestamp = self.last_timestamp.lock().await;
            let event = event
                .to_event(&self.id, &self.keys, *last_timestamp)
                .map_err(map_err)?;
            *last_timestamp = event.created_at;
            drop(last_timestamp);
            self.save_event(event)
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
        let id_tag = event.tags.iter().find(|tag| {
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
            keys.secret_key(),
            &keys.public_key(),
            &event.content,
        )?)?;
        let mut tags = Vec::new();
        tags.extend(event.tags.to_vec());
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
                        .as_slice()
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
                keys.secret_key(),
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
            keys.secret_key(),
            &keys.public_key(),
            &event.content,
        )?)?)
    }

    /// Parses a [`ProofsEvent`] from an [`Event`]
    pub fn to_event(
        &self,
        wallet_id: &str,
        keys: &nostr_sdk::Keys,
        last_timestamp: Timestamp,
    ) -> Result<Event, Error> {
        let mut tags = Vec::new();
        tags.push(wallet_link_tag(wallet_id, keys)?);

        let mut created_at = Timestamp::now();
        if created_at <= last_timestamp {
            created_at = last_timestamp + 1;
        }
        let event = EventBuilder::new(
            PROOFS_KIND,
            nip44::encrypt(
                keys.secret_key(),
                &keys.public_key(),
                serde_json::to_string(&self)?,
                nip44::Version::V2,
            )?,
            tags,
        )
        .custom_created_at(created_at);
        Ok(event.to_event(keys)?)
    }
}

/// Tx history
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    /// Direction (in for received, out for sent)
    pub direction: Direction,
    /// Amount
    pub amount: Amount,
    /// Event ID of proofs update
    pub event_id: Option<EventId>,
    /// Relay URL
    pub relay: Option<Url>,
    /// Price
    pub price: Option<String>,
    /// List of proofs IDs (Ys)
    pub proofs: Vec<PublicKey>,
    /// Payment hash for Lightning Network transactions
    pub payment_hash: Option<PaymentHash>,
}

impl Transaction {
    /// Parses a [`TxHistory`] from an [`Event`]
    pub fn from_event(event: &Event, keys: &nostr_sdk::Keys) -> Result<Self, Error> {
        let mut direction: Option<Direction> = None;
        let mut amount: Option<Amount> = None;
        let mut event_id: Option<EventId> = None;
        let mut relay: Option<Url> = None;
        let mut price: Option<String> = None;
        let mut proofs: Vec<PublicKey> = Vec::new();
        let mut payment_hash: Option<PaymentHash> = None;

        let content: Vec<Tag> = serde_json::from_str(&nip44::decrypt(
            keys.secret_key(),
            &keys.public_key(),
            &event.content,
        )?)?;
        let mut tags = Vec::new();
        tags.extend(event.tags.to_vec());
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
                PRICE_TAG => {
                    price = Some(
                        tag.content()
                            .ok_or(Error::EmptyTag(PRICE_TAG.to_string()))?
                            .to_string(),
                    );
                }
                PROOFS_TAG => {
                    proofs = tag
                        .as_slice()
                        .iter()
                        .skip(1)
                        .map(|y| PublicKey::from_str(y))
                        .collect::<Result<Vec<PublicKey>, _>>()?;
                }
                PAYMENT_HASH_TAG => {
                    payment_hash = Some(PaymentHash::from_str(
                        tag.content()
                            .ok_or(Error::EmptyTag(PAYMENT_HASH_TAG.to_string()))?,
                    )?);
                }
                t => {
                    if t == EVENT_TAG.to_string().as_str() {
                        let mut parts = tag.as_slice().into_iter();
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
            price,
            proofs,
            payment_hash,
        })
    }

    /// Converts a [`TxHistory`] to an [`Event`]
    pub fn to_event(
        &self,
        wallet_id: &str,
        keys: &nostr_sdk::Keys,
        last_timestamp: Timestamp,
    ) -> Result<Event, Error> {
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
        if let Some(price) = &self.price {
            content.push(Tag::parse(&[PRICE_TAG, price])?);
        }
        if !self.proofs.is_empty() {
            let mut proof_vec = vec![PROOFS_TAG.to_string()];
            proof_vec.extend(self.proofs.iter().map(|y| y.to_string()));
            content.push(Tag::parse(&proof_vec)?);
        }
        if let Some(payment_hash) = &self.payment_hash {
            content.push(Tag::parse(&[PAYMENT_HASH_TAG, &payment_hash.to_string()])?);
        }

        let mut tags = Vec::new();
        tags.push(wallet_link_tag(wallet_id, keys)?);

        let mut created_at = Timestamp::now();
        if created_at <= last_timestamp {
            created_at = last_timestamp + 1;
        }
        let event = EventBuilder::new(
            TX_HISTORY_KIND,
            nip44::encrypt(
                keys.secret_key(),
                &keys.public_key(),
                serde_json::to_string(&content)?,
                nip44::Version::V2,
            )?,
            tags,
        )
        .custom_created_at(created_at);
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
#[derive(Clone, Debug)]
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
            event_id: event.id,
            created_at: event.created_at,
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

/// Derive a wallet secret key from a secret key, public key, and password.
///
/// If the wallet is shared, provide the public key of the other party. If the wallet is not shared, provide the public key of the wallet.
/// The password is optional and can be used to derive a wallet with a different key.
pub fn derive_wallet_secret(
    secret_key: nostr_sdk::SecretKey,
    public_key: nostr_sdk::PublicKey,
    password: Option<String>,
) -> Result<nostr_sdk::SecretKey, Error> {
    let mut ssp = shared_secret_point(&public_key.public_key(Parity::Even), &secret_key)
        .as_slice()
        .to_owned();
    ssp.resize(32, 0); // toss the Y part
    let shared_point: [u8; 32] = ssp.try_into().expect("shared_point is not 32 bytes");
    let (shared_key, _hkdf) =
        Hkdf::<Sha256>::extract(password.as_deref().map(|s| s.as_bytes()), &shared_point);
    Ok(nostr_sdk::SecretKey::from_slice(&shared_key)?)
}

/// [`WalletNostrDatabase`]` error
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Amount error
    #[error(transparent)]
    Amount(#[from] cdk::amount::Error),
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
    /// Event not found error
    #[error("Event not found: {0}")]
    EventNotFound(EventId),
    /// Payment hash error
    #[error(transparent)]
    Hex(#[from] bitcoin::hex::HexToArrayError),
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cdk::{
        amount::Amount,
        cdk_database::WalletDatabase,
        mint_url::MintUrl,
        nuts::{CurrencyUnit, Id, Proof, PublicKey, State},
        secret::Secret,
        types::ProofInfo,
    };

    use crate::wallet::{Direction, Error};

    use super::{Transaction, WalletNostrDatabase};

    #[tokio::test]
    async fn save_and_delete_transaction() {
        let db = WalletNostrDatabase::test();
        let tx = Transaction {
            direction: Direction::Incoming,
            amount: Amount::from(100),
            event_id: None,
            relay: None,
            price: None,
            proofs: Vec::new(),
            payment_hash: None,
        };

        let event_id = db.save_transaction(tx.clone()).await.unwrap();
        let db_tx = db.get_transaction(event_id).await.unwrap();
        assert_eq!(tx, db_tx.tx);

        db.delete_transaction(event_id).await.unwrap();
        let err = db.get_transaction(event_id).await.unwrap_err();
        match err {
            Error::EventNotFound(id) => {
                assert_eq!(id, event_id);
            }
            _ => panic!("Unexpected error: {:?}", err),
        }
    }

    #[tokio::test]
    async fn save_and_delete_proof_events() {
        let db = WalletNostrDatabase::test();
        let id = Id::from_str("00123456789abcde").unwrap();
        let pk = PublicKey::from_hex(
            "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        )
        .unwrap();
        let proof = ProofInfo::new(
            Proof::new(Amount::from(1), id, Secret::generate(), pk),
            MintUrl::from_str("https://example.com").unwrap(),
            State::Spent,
            CurrencyUnit::Sat,
        )
        .unwrap();
        db.update_proofs(vec![proof.clone()], vec![]).await.unwrap();

        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert_eq!(proof, proofs[0]);

        db.update_proofs(vec![], vec![proof.y]).await.unwrap();
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(proofs.is_empty());
    }
}
