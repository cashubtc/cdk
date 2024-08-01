//! Wallet based on Nostr
use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::Arc,
};

use async_trait::async_trait;
use itertools::Itertools;
use nostr_database::{DatabaseError, NostrDatabase};
use nostr_sdk::{
    client, nips::nip44, Client, Event, EventBuilder, Filter, Kind, SingleLetterTag, Tag, TagKind,
    Timestamp,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, MutexGuard};
use url::Url;

use crate::{
    nuts::{
        CurrencyUnit, Id, KeySetInfo, Keys, MeltQuoteState, MintInfo, MintQuoteState, Proof,
        Proofs, PublicKey, SecretKey, SpendingConditions, State,
    },
    types::ProofInfo,
    wallet::{MeltQuote, MintQuote},
    Amount, UncheckedUrl,
};

use super::{WalletDatabase, WalletMemoryDatabase};

const TX_KIND: Kind = Kind::Custom(7375);
const QUOTE_KIND: Kind = Kind::Custom(7376);
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
const QUOTE_TYPE_TAG: &str = "quote_type";
const QUOTE_ID_TAG: &str = "quote_id";
const AMOUNT_TAG: &str = "amount";
const REQUEST_TAG: &str = "request";
const STATE_TAG: &str = "state";
const FEE_RESERVE: &str = "fee_reserve";
const PREIMAGE_TAG: &str = "preimage";
const EXPIRATION_TAG: &str = "expiration";

const MINT_QUOTE_TYPE: &str = "mint";
const MELT_QUOTE_TYPE: &str = "melt";

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
    db: Arc<Option<Box<dyn NostrDatabase<Err = DatabaseError>>>>,

    // Inner in-memory db
    inner: WalletMemoryDatabase,
}

impl WalletNostrDatabase {
    /// Create a new [`WalletNostrDatabase`] with a local event database
    pub async fn local<D>(
        id: String,
        keys: nostr_sdk::Keys,
        relays: Vec<Url>,
        db: D,
    ) -> Result<Self, Error>
    where
        D: NostrDatabase<Err = DatabaseError> + 'static,
    {
        let client = Self::connect_client(&keys, relays).await?;
        let mut self_ = Self {
            client,
            keys,
            id: id.clone(),
            info: Arc::new(Mutex::new(WalletInfo {
                id,
                ..Default::default()
            })),
            db: Arc::new(Some(Box::new(db))),
            inner: WalletMemoryDatabase::default(),
        };
        self_.load().await?;
        Ok(self_)
    }

    /// Create a new [`WalletNostrDatabase`] from remote relays
    pub async fn remote<D>(
        id: String,
        keys: nostr_sdk::Keys,
        relays: Vec<Url>,
    ) -> Result<Self, Error> {
        let client = Self::connect_client(&keys, relays).await?;
        let mut self_ = Self {
            client,
            keys,
            id: id.clone(),
            info: Arc::new(Mutex::new(WalletInfo {
                id,
                ..Default::default()
            })),
            db: Arc::new(None),
            inner: WalletMemoryDatabase::default(),
        };
        self_.load().await?;
        Ok(self_)
    }

    async fn connect_client(keys: &nostr_sdk::Keys, relays: Vec<Url>) -> Result<Client, Error> {
        let client = Client::new(keys);
        client.add_relays(relays).await?;
        client.connect().await;
        Ok(client)
    }

    async fn load(&mut self) -> Result<(), Error> {
        self.refresh_info().await?;
        let quote_events = self
            .get_events(vec![Filter {
                authors: filter_value!(self.keys.public_key()),
                kinds: filter_value!(QUOTE_KIND),
                generic_tags: filter_value!(
                    SingleLetterTag::from_char(ID_LINK_TAG).expect("ID_LINK_TAG is not a single letter tag") => self.id.clone(),
                ),
                ..Default::default()
            }])
            .await?;
        let mint_quotes = quote_events
            .iter()
            .map(|e| MintQuote::from_event(e, &self.keys))
            .collect::<Result<Vec<Option<MintQuote>>, Error>>()?
            .into_iter()
            .flatten()
            .collect();
        let melt_quotes = quote_events
            .iter()
            .map(|e| MeltQuote::from_event(e, &self.keys))
            .collect::<Result<Vec<Option<MeltQuote>>, Error>>()?
            .into_iter()
            .flatten()
            .collect();
        self.inner = WalletMemoryDatabase::new(
            mint_quotes,
            melt_quotes,
            vec![],
            self.info.lock().await.counters.clone(),
            HashMap::new(),
        );
        Ok(())
    }

    /// Get the latest [`WalletInfo`]
    pub async fn get_info(&self) -> WalletInfo {
        self.info.lock().await.clone()
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
        match events.last() {
            Some(event) => {
                *info = WalletInfo::from_event(event, &self.keys)?;
            }
            None => {
                *info = WalletInfo {
                    id: self.id.clone(),
                    ..Default::default()
                };
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

    async fn get_events(&self, filters: Vec<Filter>) -> Result<Vec<Event>, Error> {
        if let Some(db) = self.db.as_ref() {
            return Ok(db.query(filters, nostr_database::Order::Asc).await?);
        }
        Ok(self.client.get_events_of(filters, None).await?)
    }

    async fn save_event(&self, event: Event) -> Result<(), Error> {
        if let Some(db) = self.db.as_ref() {
            db.save_event(&event).await?;
        }
        self.client.send_event(event).await?;
        Ok(())
    }
}

#[async_trait]
impl WalletDatabase for WalletNostrDatabase {
    type Err = super::Error;

    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
        let mut info = self.info.lock().await;
        info.mints.insert(mint_url.clone());
        self.save_info_with_lock(&info).await.map_err(map_err)?;
        self.inner.add_mint(mint_url, mint_info).await
    }

    async fn remove_mint(&self, mint_url: UncheckedUrl) -> Result<(), Self::Err> {
        let mut info = self.info.lock().await;
        info.mints.remove(&mint_url);
        self.save_info_with_lock(&info).await.map_err(map_err)?;
        self.inner.remove_mint(mint_url).await
    }

    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Self::Err> {
        self.inner.get_mint(mint_url).await
    }

    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Self::Err> {
        self.inner.get_mints().await
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
        self.inner.update_mint_url(old_mint_url, new_mint_url).await
    }

    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err> {
        self.inner.add_mint_keysets(mint_url, keysets).await
    }

    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err> {
        self.inner.get_mint_keysets(mint_url).await
    }

    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err> {
        self.inner.get_keyset_by_id(keyset_id).await
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        self.save_event(quote.to_event(&self.id, &self.keys).map_err(map_err)?)
            .await
            .map_err(map_err)?;
        self.inner.add_mint_quote(quote).await
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        self.inner.get_mint_quote(quote_id).await
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        self.inner.get_mint_quotes().await
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        self.inner.remove_mint_quote(quote_id).await
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        self.save_event(quote.to_event(&self.id, &self.keys).map_err(map_err)?)
            .await
            .map_err(map_err)?;
        self.inner.add_melt_quote(quote).await
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
        self.inner.get_melt_quote(quote_id).await
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        self.inner.remove_melt_quote(quote_id).await
    }

    async fn add_keys(&self, keys: Keys) -> Result<(), Self::Err> {
        self.inner.add_keys(keys).await
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err> {
        self.inner.get_keys(id).await
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), Self::Err> {
        self.inner.remove_keys(id).await
    }

    async fn add_proofs(&self, proof_info: Vec<ProofInfo>) -> Result<(), Self::Err> {
        self.inner.add_proofs(proof_info).await
    }

    async fn get_proofs(
        &self,
        mint_url: Option<UncheckedUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Self::Err> {
        self.inner
            .get_proofs(mint_url, unit, state, spending_conditions)
            .await
    }

    async fn remove_proofs(&self, proofs: &Proofs) -> Result<(), Self::Err> {
        self.inner.remove_proofs(proofs).await
    }

    async fn set_proof_state(&self, y: PublicKey, state: State) -> Result<(), Self::Err> {
        self.inner.set_proof_state(y, state).await
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err> {
        self.inner.increment_keyset_counter(keyset_id, count).await
    }

    async fn get_keyset_counter(&self, id: &Id) -> Result<Option<u32>, Self::Err> {
        self.inner.get_keyset_counter(id).await
    }

    async fn get_nostr_last_checked(
        &self,
        verifying_key: &PublicKey,
    ) -> Result<Option<u32>, Self::Err> {
        self.inner.get_nostr_last_checked(verifying_key).await
    }

    async fn add_nostr_last_checked(
        &self,
        verifying_key: PublicKey,
        last_checked: u32,
    ) -> Result<(), Self::Err> {
        self.inner
            .add_nostr_last_checked(verifying_key, last_checked)
            .await
    }
}

fn map_err(e: Error) -> super::Error {
    super::Error::Database(Box::new(e))
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

impl MintQuote {
    /// Parses a [`MintQuote`] from an [`Event`]
    pub fn from_event(event: &Event, keys: &nostr_sdk::Keys) -> Result<Option<MintQuote>, Error> {
        if event.verify().is_err() {
            return Ok(None);
        }
        let mut id: Option<String> = None;
        let mut mint_url: Option<UncheckedUrl> = None;
        let mut amount: Option<Amount> = None;
        let mut unit: Option<CurrencyUnit> = None;
        let mut request: Option<String> = None;
        let mut state: Option<MintQuoteState> = None;
        let mut expiry: Option<u64> = None;

        let mut tags = event.tags().to_vec();
        let content: Vec<Tag> = serde_json::from_str(&nip44::decrypt(
            keys.secret_key()?,
            &keys.public_key(),
            &event.content,
        )?)?;
        tags.extend(content);
        for tag in tags {
            match tag.kind().to_string().as_str() {
                QUOTE_TYPE_TAG => {
                    if tag
                        .content()
                        .ok_or(Error::EmptyTag(QUOTE_TYPE_TAG.to_string()))?
                        != MINT_QUOTE_TYPE
                    {
                        return Ok(None);
                    }
                }
                QUOTE_ID_TAG => {
                    id = Some(
                        tag.content()
                            .ok_or(Error::EmptyTag(QUOTE_ID_TAG.to_string()))?
                            .to_string(),
                    );
                }
                MINT_TAG => {
                    mint_url = Some(UncheckedUrl::from_str(
                        tag.content().ok_or(Error::EmptyTag(MINT_TAG.to_string()))?,
                    )?);
                }
                AMOUNT_TAG => {
                    amount = Some(Amount::from(
                        tag.content()
                            .ok_or(Error::EmptyTag(AMOUNT_TAG.to_string()))?
                            .parse::<u64>()?,
                    ));
                }
                UNIT_TAG => {
                    unit = Some(CurrencyUnit::from_str(
                        tag.content().ok_or(Error::EmptyTag(UNIT_TAG.to_string()))?,
                    )?);
                }
                REQUEST_TAG => {
                    request = Some(
                        tag.content()
                            .ok_or(Error::EmptyTag(REQUEST_TAG.to_string()))?
                            .to_string(),
                    );
                }
                STATE_TAG => {
                    state = Some(MintQuoteState::from_str(
                        tag.content()
                            .ok_or(Error::EmptyTag(STATE_TAG.to_string()))?,
                    )?);
                }
                EXPIRATION_TAG => {
                    expiry = Some(
                        tag.content()
                            .ok_or(Error::EmptyTag(EXPIRATION_TAG.to_string()))?
                            .parse::<u64>()?,
                    );
                }
                _ => {}
            }
        }
        if let Some(expiry) = expiry {
            if expiry < Timestamp::now().as_u64() {
                return Ok(None);
            }
        }

        let quote = MintQuote {
            id: id.ok_or(Error::MissingTag(QUOTE_ID_TAG.to_string()))?,
            mint_url: mint_url.ok_or(Error::MissingTag(MINT_TAG.to_string()))?,
            amount: amount.ok_or(Error::MissingTag(AMOUNT_TAG.to_string()))?,
            unit: unit.ok_or(Error::MissingTag(UNIT_TAG.to_string()))?,
            request: request.ok_or(Error::MissingTag(REQUEST_TAG.to_string()))?,
            state: state.ok_or(Error::MissingTag(STATE_TAG.to_string()))?,
            expiry: expiry.ok_or(Error::MissingTag(EXPIRATION_TAG.to_string()))?,
        };
        Ok(Some(quote))
    }

    /// Converts a [`MintQuote`] to an [`Event`]
    pub fn to_event(&self, wallet_id: &str, keys: &nostr_sdk::Keys) -> Result<Event, Error> {
        let mut content = Vec::new();
        let mut tags = Vec::new();
        content.push(Tag::parse(&[QUOTE_TYPE_TAG, MINT_QUOTE_TYPE])?);
        content.push(Tag::parse(&[QUOTE_ID_TAG, &self.id])?);
        content.push(Tag::parse(&[MINT_TAG, &self.mint_url.to_string()])?);
        content.push(Tag::parse(&[AMOUNT_TAG, &self.amount.to_string()])?);
        content.push(Tag::parse(&[UNIT_TAG, &self.unit.to_string()])?);
        content.push(Tag::parse(&[REQUEST_TAG, &self.request])?);
        content.push(Tag::parse(&[STATE_TAG, &self.state.to_string()])?);
        tags.push(wallet_link_tag(wallet_id, keys)?);
        tags.push(Tag::parse(&[
            &EXPIRATION_TAG.to_string(),
            &self.expiry.to_string(),
        ])?);
        let event = EventBuilder::new(
            QUOTE_KIND,
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

impl MeltQuote {
    /// Converts a [`MeltQuote`] to an [`Event`]
    pub fn from_event(event: &Event, keys: &nostr_sdk::Keys) -> Result<Option<MeltQuote>, Error> {
        if event.verify().is_err() {
            return Ok(None);
        }
        let mut id: Option<String> = None;
        let mut amount: Option<Amount> = None;
        let mut unit: Option<CurrencyUnit> = None;
        let mut request: Option<String> = None;
        let mut fee_reserve: Option<Amount> = None;
        let mut preimage: Option<String> = None;
        let mut expiry: Option<u64> = None;

        let mut tags = event.tags().to_vec();
        let content: Vec<Tag> = serde_json::from_str(&nip44::decrypt(
            keys.secret_key()?,
            &keys.public_key(),
            &event.content,
        )?)?;
        tags.extend(content);
        for tag in tags {
            match tag.kind().to_string().as_str() {
                QUOTE_TYPE_TAG => {
                    if tag
                        .content()
                        .ok_or(Error::EmptyTag(QUOTE_TYPE_TAG.to_string()))?
                        != MELT_QUOTE_TYPE
                    {
                        return Ok(None);
                    }
                }
                QUOTE_ID_TAG => {
                    id = Some(
                        tag.content()
                            .ok_or(Error::EmptyTag(QUOTE_ID_TAG.to_string()))?
                            .to_string(),
                    );
                }
                AMOUNT_TAG => {
                    amount = Some(Amount::from(
                        tag.content()
                            .ok_or(Error::EmptyTag(AMOUNT_TAG.to_string()))?
                            .parse::<u64>()?,
                    ));
                }
                UNIT_TAG => {
                    unit = Some(CurrencyUnit::from_str(
                        tag.content().ok_or(Error::EmptyTag(UNIT_TAG.to_string()))?,
                    )?);
                }
                REQUEST_TAG => {
                    request = Some(
                        tag.content()
                            .ok_or(Error::EmptyTag(REQUEST_TAG.to_string()))?
                            .to_string(),
                    );
                }
                FEE_RESERVE => {
                    fee_reserve = Some(Amount::from(
                        tag.content()
                            .ok_or(Error::EmptyTag(FEE_RESERVE.to_string()))?
                            .parse::<u64>()?,
                    ));
                }
                PREIMAGE_TAG => {
                    preimage = Some(
                        tag.content()
                            .ok_or(Error::EmptyTag(PREIMAGE_TAG.to_string()))?
                            .to_string(),
                    );
                }
                EXPIRATION_TAG => {
                    expiry = Some(
                        tag.content()
                            .ok_or(Error::EmptyTag(EXPIRATION_TAG.to_string()))?
                            .parse::<u64>()?,
                    );
                }
                _ => {}
            }
        }
        if let Some(expiry) = expiry {
            if expiry < Timestamp::now().as_u64() {
                return Ok(None);
            }
        }

        let quote = MeltQuote {
            id: id.ok_or(Error::MissingTag(QUOTE_ID_TAG.to_string()))?,
            amount: amount.ok_or(Error::MissingTag(AMOUNT_TAG.to_string()))?,
            unit: unit.ok_or(Error::MissingTag(UNIT_TAG.to_string()))?,
            request: request.ok_or(Error::MissingTag(REQUEST_TAG.to_string()))?,
            fee_reserve: fee_reserve.ok_or(Error::MissingTag(FEE_RESERVE.to_string()))?,
            state: MeltQuoteState::Pending,
            expiry: expiry.ok_or(Error::MissingTag(EXPIRATION_TAG.to_string()))?,
            payment_preimage: preimage,
        };
        Ok(Some(quote))
    }

    /// Converts a [`MeltQuote`] to an [`Event`]
    pub fn to_event(&self, wallet_id: &str, keys: &nostr_sdk::Keys) -> Result<Event, Error> {
        let mut content = Vec::new();
        let mut tags = Vec::new();
        content.push(Tag::parse(&[QUOTE_TYPE_TAG, MELT_QUOTE_TYPE])?);
        content.push(Tag::parse(&[QUOTE_ID_TAG, &self.id])?);
        content.push(Tag::parse(&[AMOUNT_TAG, &self.amount.to_string()])?);
        content.push(Tag::parse(&[UNIT_TAG, &self.unit.to_string()])?);
        content.push(Tag::parse(&[REQUEST_TAG, &self.request])?);
        tags.push(wallet_link_tag(wallet_id, keys)?);
        tags.push(Tag::parse(&[
            &EXPIRATION_TAG.to_string(),
            &self.expiry.to_string(),
        ])?);
        let event = EventBuilder::new(
            QUOTE_KIND,
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
    inputs: Vec<TxInfoProofs>,
    outputs: Vec<TxInfoProofs>,
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
}

#[derive(Debug, Serialize, Deserialize)]
struct TxInfoProofs {
    mint_url: UncheckedUrl,
    proofs: Vec<Proof>,
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
    Nut00(#[from] crate::nuts::nut00::Error),
    /// NUT-01 error
    #[error(transparent)]
    Nut01(#[from] crate::nuts::nut01::Error),
    /// NUT-02 error
    #[error(transparent)]
    Nut02(#[from] crate::nuts::nut02::Error),
    /// NUT-04 error
    #[error(transparent)]
    Nut04(#[from] crate::nuts::nut04::Error),
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
    UrlParse(#[from] crate::url::Error),
    /// Wallet database error
    #[error(transparent)]
    WalletDatabase(#[from] super::Error),
}
