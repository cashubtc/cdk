//! Mint event types
use std::fmt::Debug;
use std::hash::Hash;
use std::ops::Deref;

use serde::de::DeserializeOwned;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::nut17::{deserialize_payload_for_kind, Kind, NotificationId};
use crate::pub_sub::Event;
use crate::{
    MeltQuoteBolt11Response, MeltQuoteBolt12Response, MeltQuoteOnchainResponse,
    MintQuoteBolt11Response, MintQuoteBolt12Response, MintQuoteOnchainResponse,
    NotificationPayload, ProofState,
};

/// Simple wrapper over `NotificationPayload<QuoteId>` which is a foreign type
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MintEvent<T>(NotificationPayload<T>)
where
    T: Clone + Eq + PartialEq;

/// Kind-tagged wire format for [`MintEvent`].
///
/// NUT-17 payloads are not self-describing: quote responses for different
/// payment methods share field names, so a payload can only be decoded when the
/// subscription kind that produced it is known. Anything that puts an event on a
/// wire (the cross-instance notification bus) therefore sends the kind next to
/// the payload, and decodes with it.
#[derive(Deserialize)]
struct TaggedEvent {
    kind: Kind,
    payload: serde_json::Value,
}

impl<T> Serialize for MintEvent<T>
where
    T: Clone + Eq + PartialEq + Serialize + DeserializeOwned,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut event = serializer.serialize_struct("MintEvent", 2)?;
        event.serialize_field("kind", &self.0.kind())?;
        event.serialize_field("payload", &self.0)?;
        event.end()
    }
}

impl<'de, T> Deserialize<'de> for MintEvent<T>
where
    T: Clone + Eq + PartialEq + Serialize + DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let tagged = TaggedEvent::deserialize(deserializer)?;
        deserialize_payload_for_kind::<T, D::Error>(&tagged.kind, tagged.payload).map(Self)
    }
}

impl<T> From<MintEvent<T>> for NotificationPayload<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MintEvent<T>) -> Self {
        value.0
    }
}

impl<T> Deref for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    type Target = NotificationPayload<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> From<ProofState> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: ProofState) -> Self {
        Self(NotificationPayload::ProofState(value))
    }
}

impl<T> MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    /// New instance
    pub fn new(t: NotificationPayload<T>) -> Self {
        Self(t)
    }

    /// Get inner
    pub fn inner(&self) -> &NotificationPayload<T> {
        &self.0
    }

    /// Into inner
    pub fn into_inner(self) -> NotificationPayload<T> {
        self.0
    }
}

impl<T> From<NotificationPayload<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: NotificationPayload<T>) -> Self {
        Self(value)
    }
}

impl<T> From<MintQuoteBolt11Response<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MintQuoteBolt11Response<T>) -> Self {
        Self(NotificationPayload::MintQuoteBolt11Response(value))
    }
}

impl<T> From<MeltQuoteBolt11Response<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MeltQuoteBolt11Response<T>) -> Self {
        Self(NotificationPayload::MeltQuoteBolt11Response(value))
    }
}

impl<T> From<MintQuoteBolt12Response<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MintQuoteBolt12Response<T>) -> Self {
        Self(NotificationPayload::MintQuoteBolt12Response(value))
    }
}

impl<T> From<MeltQuoteBolt12Response<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MeltQuoteBolt12Response<T>) -> Self {
        Self(NotificationPayload::MeltQuoteBolt12Response(value))
    }
}

impl<T> From<MintQuoteOnchainResponse<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MintQuoteOnchainResponse<T>) -> Self {
        Self(NotificationPayload::MintQuoteOnchainResponse(value))
    }
}

impl<T> From<MeltQuoteOnchainResponse<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MeltQuoteOnchainResponse<T>) -> Self {
        Self(NotificationPayload::MeltQuoteOnchainResponse(value))
    }
}

impl<T> Event for MintEvent<T>
where
    T: Clone + Serialize + DeserializeOwned + Debug + Ord + Hash + Send + Sync + Eq + PartialEq,
{
    type Topic = NotificationId<T>;

    fn get_topics(&self) -> Vec<Self::Topic> {
        match &self.0 {
            NotificationPayload::MeltQuoteBolt11Response(r) => {
                vec![NotificationId::MeltQuoteBolt11(r.quote.to_owned())]
            }
            NotificationPayload::MintQuoteBolt11Response(r) => {
                vec![NotificationId::MintQuoteBolt11(r.quote.to_owned())]
            }
            NotificationPayload::MintQuoteBolt12Response(r) => {
                vec![NotificationId::MintQuoteBolt12(r.quote.to_owned())]
            }
            NotificationPayload::MeltQuoteBolt12Response(r) => {
                vec![NotificationId::MeltQuoteBolt12(r.quote.to_owned())]
            }
            NotificationPayload::MeltQuoteOnchainResponse(r) => {
                vec![NotificationId::MeltQuoteOnchain(r.quote.to_owned())]
            }
            NotificationPayload::MintQuoteOnchainResponse(r) => {
                vec![NotificationId::MintQuoteOnchain(r.quote.to_owned())]
            }
            NotificationPayload::CustomMintQuoteResponse(method, r) => {
                vec![NotificationId::MintQuoteCustom(
                    method.clone(),
                    r.quote.to_owned(),
                )]
            }
            NotificationPayload::CustomMeltQuoteResponse(method, r) => {
                vec![NotificationId::MeltQuoteCustom(
                    method.clone(),
                    r.quote.to_owned(),
                )]
            }
            NotificationPayload::ProofState(p) => vec![NotificationId::ProofState(p.y.to_owned())],
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::nut00::{CurrencyUnit, KnownMethod, PaymentMethod};
    use crate::nut05::QuoteState;
    use crate::nut07::State;
    use crate::nut30::MeltQuoteOnchainFeeOption;
    use crate::nuts::{MeltQuoteState, MintQuoteState};
    use crate::{Amount, MeltQuoteCustomResponse, MintQuoteCustomResponse, PublicKey};

    fn pubkey() -> PublicKey {
        PublicKey::from_hex("03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac")
            .expect("valid pubkey")
    }

    /// One event per [`NotificationPayload`] variant, as the mint publishes
    /// them.
    fn sample_events() -> Vec<MintEvent<String>> {
        vec![
            MintEvent::new(NotificationPayload::ProofState(ProofState {
                y: pubkey(),
                state: State::Spent,
                witness: None,
            })),
            MintEvent::new(NotificationPayload::MintQuoteBolt11Response(
                MintQuoteBolt11Response {
                    quote: "mint-bolt11".to_string(),
                    request: "lnbc...".to_string(),
                    amount: Some(Amount::from(100_000)),
                    unit: Some(CurrencyUnit::Sat),
                    method: PaymentMethod::BOLT11,
                    amount_paid: Amount::from(0),
                    amount_issued: Amount::from(0),
                    updated_at: 0,
                    state: MintQuoteState::Unpaid,
                    expiry: Some(1701704757),
                    pubkey: Some(pubkey()),
                },
            )),
            MintEvent::new(NotificationPayload::MeltQuoteBolt11Response(
                MeltQuoteBolt11Response {
                    quote: "melt-bolt11".to_string(),
                    amount: Amount::from(100_000),
                    fee_reserve: Amount::from(10),
                    state: MeltQuoteState::Pending,
                    expiry: 1701704757,
                    payment_preimage: None,
                    change: None,
                    request: Some("lnbc...".to_string()),
                    unit: Some(CurrencyUnit::Sat),
                    method: PaymentMethod::BOLT11,
                },
            )),
            MintEvent::new(NotificationPayload::MintQuoteBolt12Response(
                MintQuoteBolt12Response {
                    quote: "mint-bolt12".to_string(),
                    request: "lno1...".to_string(),
                    amount: Some(Amount::from(100_000)),
                    unit: CurrencyUnit::Sat,
                    method: PaymentMethod::Known(KnownMethod::Bolt12),
                    expiry: Some(1701704757),
                    pubkey: pubkey(),
                    amount_paid: Amount::from(0),
                    amount_issued: Amount::from(0),
                    updated_at: 0,
                },
            )),
            MintEvent::new(NotificationPayload::MeltQuoteBolt12Response(
                MeltQuoteBolt12Response {
                    quote: "melt-bolt12".to_string(),
                    amount: Amount::from(100_000),
                    fee_reserve: Amount::from(10),
                    state: MeltQuoteState::Pending,
                    expiry: 1701704757,
                    payment_preimage: None,
                    change: None,
                    request: Some("lno1...".to_string()),
                    unit: Some(CurrencyUnit::Sat),
                    method: PaymentMethod::Known(KnownMethod::Bolt12),
                },
            )),
            MintEvent::new(NotificationPayload::MintQuoteOnchainResponse(
                MintQuoteOnchainResponse {
                    quote: "mint-onchain".to_string(),
                    request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
                    unit: CurrencyUnit::Sat,
                    method: PaymentMethod::Known(KnownMethod::Onchain),
                    expiry: Some(1701704757),
                    pubkey: pubkey(),
                    amount_paid: Amount::from(100_000),
                    amount_issued: Amount::from(0),
                    updated_at: 0,
                },
            )),
            MintEvent::new(NotificationPayload::MeltQuoteOnchainResponse(
                MeltQuoteOnchainResponse {
                    quote: "melt-onchain".to_string(),
                    amount: Amount::from(100_000),
                    unit: CurrencyUnit::Sat,
                    method: PaymentMethod::Known(KnownMethod::Onchain),
                    state: MeltQuoteState::Pending,
                    expiry: 1701704757,
                    request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
                    fee_options: vec![MeltQuoteOnchainFeeOption {
                        fee_index: 0,
                        fee_reserve: Amount::from(5_000),
                        estimated_blocks: 1,
                    }],
                    selected_fee_index: Some(0),
                    outpoint: Some("3b7f3b85:2".to_string()),
                    change: None,
                },
            )),
            MintEvent::new(NotificationPayload::CustomMintQuoteResponse(
                "paypal".to_string(),
                MintQuoteCustomResponse {
                    quote: "mint-custom".to_string(),
                    request: "pay://abc".to_string(),
                    method: PaymentMethod::Custom("paypal".to_string()),
                    amount: Some(Amount::from(10)),
                    amount_paid: Amount::from(0),
                    amount_issued: Amount::from(0),
                    updated_at: 0,
                    unit: Some(CurrencyUnit::Sat),
                    expiry: Some(1701704757),
                    pubkey: None,
                    extra: json!({}),
                },
            )),
            MintEvent::new(NotificationPayload::CustomMeltQuoteResponse(
                "paypal".to_string(),
                MeltQuoteCustomResponse {
                    quote: "melt-custom".to_string(),
                    method: PaymentMethod::Custom("paypal".to_string()),
                    amount: Amount::from(10),
                    fee_reserve: Some(Amount::from(1)),
                    state: QuoteState::Pending,
                    expiry: 1701704757,
                    payment_preimage: None,
                    change: None,
                    request: Some("pay://abc".to_string()),
                    unit: Some(CurrencyUnit::Sat),
                    extra: json!({}),
                },
            )),
        ]
    }

    /// Every event the mint publishes survives the JSON round-trip the
    /// cross-instance bus performs. Without the kind tag the payloads are
    /// undecodable, and every forwarded event would be dropped as malformed.
    #[test]
    fn every_mint_event_variant_roundtrips_as_json() {
        for event in sample_events() {
            let encoded = serde_json::to_string(&event).expect("serialize");
            let decoded: MintEvent<String> = serde_json::from_str(&encoded)
                .unwrap_or_else(|err| panic!("deserialize {encoded}: {err}"));

            assert_eq!(decoded, event);
            assert_eq!(decoded.get_topics(), event.get_topics());
        }
    }

    #[test]
    fn wire_format_carries_the_subscription_kind() {
        let event: MintEvent<String> =
            MintEvent::new(NotificationPayload::ProofState(ProofState {
                y: pubkey(),
                state: State::Spent,
                witness: None,
            }));

        let encoded: serde_json::Value = serde_json::to_value(&event).expect("serialize");

        assert_eq!(encoded["kind"], json!("proof_state"));
        assert_eq!(encoded["payload"]["state"], json!("SPENT"));
    }

    #[test]
    fn payload_that_does_not_match_its_kind_is_rejected() {
        let mismatched = json!({
            "kind": "bolt11_melt_quote",
            "payload": {"Y": pubkey().to_hex(), "state": "SPENT"},
        });

        assert!(serde_json::from_value::<MintEvent<String>>(mismatched).is_err());
    }
}
