use std::{ops::Deref, str::FromStr, sync::Arc, time::Duration};

use bitcoin::key::Parity;
use cdk::{
    amount::{Amount, SplitTarget},
    mint_url::MintUrl,
    nuts::{CurrencyUnit, Proof, SpendingConditions},
    wallet::{multi_mint_wallet::WalletKey, MultiMintWallet, SendKind},
};
use nostr_sdk::{
    Alphabet, Client, Event, EventBuilder, EventId, Filter, Keys, Kind, PublicKey, SecretKey,
    SingleLetterTag, Tag, TagKind, TagStandard, Timestamp, UncheckedUrl,
};
use tokio::sync::Mutex;

const NUT_ZAP_KIND: Kind = Kind::Custom(9321);
const PROOF_TAG: &str = "proof";
const UNIT_TAG: &str = "unit";

#[derive(Clone)]
pub struct NutZapper {
    client: Client,
    keys: Keys,
    wallet: MultiMintWallet,

    last_timestamp: Arc<Mutex<Timestamp>>,
}

impl NutZapper {
    pub fn new(
        client: Client,
        key: SecretKey,
        wallet: MultiMintWallet,
        start_timestamp: Option<Timestamp>,
    ) -> Self {
        Self {
            client,
            keys: Keys::new(key),
            wallet,
            last_timestamp: Arc::new(Mutex::new(start_timestamp.unwrap_or(Timestamp::now()))),
        }
    }

    pub async fn claim_zap(&self, event: NutZapEvent) -> Result<Amount, Error> {
        let wallet = self
            .wallet
            .get_wallet(&WalletKey::new(event.mint_url, event.unit))
            .await
            .ok_or(Error::MissingWallet)?;
        let amount = wallet
            .receive_proofs(
                event.proofs,
                SplitTarget::None,
                &[cdk::nuts::SecretKey::from(*self.keys.secret_key().deref())],
                &[],
            )
            .await?;
        Ok(amount)
    }

    pub async fn get_zap_events(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Vec<NutZapEvent>, Error> {
        let mut last_timestamp = self.last_timestamp.lock().await;
        let events = self
            .client
            .get_events_of(
                vec![Filter::new()
                    .kind(NUT_ZAP_KIND)
                    .since(*last_timestamp)
                    .custom_tag(
                        SingleLetterTag::lowercase(Alphabet::P),
                        vec![self.keys.public_key()],
                    )
                    .custom_tag(
                        SingleLetterTag::lowercase(Alphabet::U),
                        self.wallet
                            .get_wallets()
                            .await
                            .iter()
                            .map(|w| w.mint_url.clone().to_string())
                            .collect::<Vec<_>>(),
                    )],
                nostr_sdk::EventSource::relays(timeout),
            )
            .await?;
        let max_timestamp = events.iter().map(|e| e.created_at).max();
        if let Some(max_timestamp) = max_timestamp {
            *last_timestamp = max_timestamp;
        }
        Ok(events
            .into_iter()
            .filter_map(|event| match NutZapEvent::try_from(event) {
                Ok(event) => Some(event),
                Err(err) => {
                    tracing::error!("Failed to parse event: {}", err);
                    None
                }
            })
            .collect())
    }

    pub async fn zap_from_mint(
        &self,
        pubkey: PublicKey,
        mint_url: MintUrl,
        amount: Amount,
        unit: CurrencyUnit,
    ) -> Result<(), Error> {
        let wallet = self
            .wallet
            .get_wallet(&WalletKey::new(mint_url.clone(), unit))
            .await
            .ok_or(Error::MissingWallet)?;
        let token = wallet
            .send(
                amount,
                None,
                Some(SpendingConditions::new_p2pk(
                    cdk::nuts::PublicKey::from(pubkey.public_key(Parity::Even)),
                    None,
                )),
                &SplitTarget::None,
                &SendKind::OnlineExact,
                false,
            )
            .await?;
        let proofs = token
            .proofs()
            .get(&mint_url)
            .ok_or(Error::MissingProofs)?
            .clone();
        let event = NutZapEvent {
            id: EventId::from_byte_array([0; 32]),
            sender_pubkey: self.keys.public_key(),
            receiver_pubkey: pubkey,
            content: "".to_string(),
            mint_url,
            unit,
            proofs,
            zapped_event_id: None,
        };
        self.client.send_event_builder(event.try_into()?).await?;
        Ok(())
    }
}

pub struct NutZapEvent {
    pub id: EventId,
    pub sender_pubkey: PublicKey,
    pub receiver_pubkey: PublicKey,
    pub content: String,
    pub mint_url: MintUrl,
    pub unit: CurrencyUnit,
    pub proofs: Vec<Proof>,
    pub zapped_event_id: Option<(EventId, Option<UncheckedUrl>)>,
}

impl TryInto<EventBuilder> for NutZapEvent {
    type Error = Error;

    fn try_into(self) -> Result<EventBuilder, Self::Error> {
        let mut tags = Vec::new();
        tags.push(Tag::from_standardized(TagStandard::public_key(
            self.receiver_pubkey,
        )));
        tags.push(Tag::custom(
            TagKind::SingleLetter(SingleLetterTag::lowercase(Alphabet::U)),
            vec![self.mint_url.to_string()],
        ));
        tags.push(Tag::custom(
            TagKind::custom(UNIT_TAG),
            vec![self.unit.to_string()],
        ));
        for proof in self.proofs {
            tags.push(Tag::custom(
                TagKind::custom(PROOF_TAG),
                vec![serde_json::to_string(&proof)?],
            ));
        }
        if let Some((zapped_event_id, _relay_hint)) = self.zapped_event_id {
            tags.push(Tag::from_standardized(TagStandard::event(zapped_event_id)));
        }
        Ok(EventBuilder::new(NUT_ZAP_KIND, self.content, tags))
    }
}

impl TryFrom<Event> for NutZapEvent {
    type Error = Error;

    fn try_from(event: Event) -> Result<Self, Self::Error> {
        let id = event.id;
        let sender_pubkey = event.pubkey;
        let content = event.content;

        let mut receiver_pubkey: Option<PublicKey> = None;
        let mut mint_url: Option<MintUrl> = None;
        let mut unit: Option<CurrencyUnit> = None;
        let mut proofs: Vec<Proof> = Vec::new();
        let mut zapped_event_id: Option<(EventId, Option<UncheckedUrl>)> = None;

        for tag in event.tags {
            match tag.as_standardized() {
                Some(tag) => match tag {
                    TagStandard::Event {
                        event_id,
                        relay_url,
                        ..
                    } => {
                        zapped_event_id = Some((event_id.clone(), relay_url.clone()));
                    }
                    TagStandard::PublicKey {
                        public_key,
                        uppercase,
                        ..
                    } if !uppercase => {
                        receiver_pubkey = Some(public_key.clone());
                    }
                    _ => {}
                },
                None => match tag.kind() {
                    TagKind::SingleLetter(letter) => match letter.character {
                        Alphabet::U if letter.is_lowercase() => {
                            mint_url = Some(MintUrl::from_str(
                                tag.content().ok_or(Error::InvalidTag("u"))?,
                            )?);
                        }
                        _ => {}
                    },
                    nostr_sdk::TagKind::Custom(custom) => match custom.to_string().as_str() {
                        PROOF_TAG => {
                            proofs.push(
                                serde_json::from_str(
                                    tag.content().ok_or(Error::InvalidTag("proof"))?,
                                )
                                .map_err(|_| Error::InvalidTag("proof"))?,
                            );
                        }
                        UNIT_TAG => {
                            unit = Some(
                                CurrencyUnit::from_str(
                                    tag.content().ok_or(Error::InvalidTag("unit"))?,
                                )
                                .map_err(|_| Error::InvalidTag("unit"))?,
                            );
                        }
                        _ => {}
                    },
                    _ => {}
                },
            }
        }

        if proofs.is_empty() {
            return Err(Error::MissingProofs);
        }
        Ok(Self {
            id,
            sender_pubkey,
            receiver_pubkey: receiver_pubkey.ok_or(Error::MissingPubkey)?,
            content,
            mint_url: mint_url.ok_or(Error::MissingMintUrl)?,
            unit: unit.unwrap_or(CurrencyUnit::Sat),
            proofs,
            zapped_event_id,
        })
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Client(#[from] nostr_sdk::client::Error),
    #[error("Invalid tag: {0}")]
    InvalidTag(&'static str),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Key(#[from] nostr_sdk::key::Error),
    #[error(transparent)]
    MintUrl(#[from] cdk::mint_url::Error),
    #[error("Missing mint url")]
    MissingMintUrl,
    #[error("Missing proofs")]
    MissingProofs,
    #[error("Missing pubkey")]
    MissingPubkey,
    #[error("Missing wallet")]
    MissingWallet,
    #[error(transparent)]
    Wallet(#[from] cdk::error::Error),
}