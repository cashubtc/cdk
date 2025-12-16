//! NUT-XX: Batch Minting
//!
//! Batch minting support for Cashu

use serde::{Deserialize, Serialize};

use super::nut00::{BlindedMessage, PaymentMethod};
use super::nut23::MintQuoteBolt11Response;
use super::nut25::MintQuoteBolt12Response;
use super::MintQuoteState;
use crate::Amount;

/// Batch Mint Request [NUT-XX]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BatchMintRequest {
    /// Quote IDs
    pub quote: Vec<String>,
    /// Expected amount to mint per quote, in the same order as `quote`
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quote_amounts: Option<Vec<Amount>>,
    /// Blinded messages
    pub outputs: Vec<BlindedMessage>,
    /// Signatures for NUT-20 locked quotes (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<Vec<Option<String>>>,
}

impl BatchMintRequest {
    /// Total amount of outputs
    pub fn total_amount(&self) -> Result<Amount, crate::nuts::nut04::Error> {
        Amount::try_sum(self.outputs.iter().map(|msg| msg.amount))
            .map_err(|_| crate::nuts::nut04::Error::AmountOverflow)
    }

    /// Total amount the client expects to mint per quote, if provided
    pub fn total_quote_amounts(&self) -> Option<Result<Amount, crate::nuts::nut04::Error>> {
        self.quote_amounts.as_ref().map(|amounts| {
            Amount::try_sum(amounts.iter().cloned())
                .map_err(|_| crate::nuts::nut04::Error::AmountOverflow)
        })
    }
}

/// Batch Quote Status Request [NUT-XX]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BatchQuoteStatusRequest {
    /// Quote IDs
    pub quote: Vec<String>,
}

/// Batch minting settings (NUT-XX)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BatchMintSettings {
    /// Maximum quotes allowed in a batch request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_batch_size: Option<u16>,
    /// Supported payment methods for batch minting
    #[serde(default)]
    pub methods: Vec<PaymentMethod>,
}

impl Default for BatchMintSettings {
    fn default() -> Self {
        Self {
            max_batch_size: Some(100),
            methods: Vec::new(),
        }
    }
}

impl BatchMintSettings {
    /// Returns true when no batch capabilities should be advertised
    pub fn is_empty(&self) -> bool {
        self.methods.is_empty()
    }
}

/// Bolt12 batch status payload extends the standard Bolt12 quote with a derived state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + for<'a> Deserialize<'a>")]
pub struct MintQuoteBolt12BatchStatusResponse<Q> {
    /// Underlying Bolt12 quote payload
    #[serde(flatten)]
    pub quote: MintQuoteBolt12Response<Q>,
    /// Derived quote state (UNPAID, PAID, ISSUED)
    pub state: MintQuoteState,
}

impl<Q> MintQuoteBolt12BatchStatusResponse<Q> {
    fn from_quote(quote: MintQuoteBolt12Response<Q>) -> Self {
        let state = derive_quote_state(quote.amount_paid, quote.amount_issued);
        Self { quote, state }
    }

    /// Current quote state
    pub fn state(&self) -> MintQuoteState {
        self.state
    }
}

impl<Q> From<MintQuoteBolt12Response<Q>> for MintQuoteBolt12BatchStatusResponse<Q> {
    fn from(value: MintQuoteBolt12Response<Q>) -> Self {
        Self::from_quote(value)
    }
}

/// Batch quote status entry supporting both Bolt11 and Bolt12 payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum BatchQuoteStatusItem {
    /// Bolt11 quote payload
    Bolt11(MintQuoteBolt11Response<String>),
    /// Bolt12 quote payload
    Bolt12(MintQuoteBolt12BatchStatusResponse<String>),
}

impl BatchQuoteStatusItem {
    /// Quote state (UNPAID, PAID, ISSUED)
    pub fn state(&self) -> MintQuoteState {
        match self {
            Self::Bolt11(response) => response.state,
            Self::Bolt12(response) => response.state(),
        }
    }
}

impl From<MintQuoteBolt11Response<String>> for BatchQuoteStatusItem {
    fn from(value: MintQuoteBolt11Response<String>) -> Self {
        Self::Bolt11(value)
    }
}

impl From<MintQuoteBolt12Response<String>> for BatchQuoteStatusItem {
    fn from(value: MintQuoteBolt12Response<String>) -> Self {
        let batch_response = MintQuoteBolt12BatchStatusResponse::from(value);
        Self::Bolt12(batch_response)
    }
}

impl From<MintQuoteBolt12BatchStatusResponse<String>> for BatchQuoteStatusItem {
    fn from(value: MintQuoteBolt12BatchStatusResponse<String>) -> Self {
        Self::Bolt12(value)
    }
}

/// Batch Quote Status Response [NUT-XX]
/// Returns a Vec that should be serialized as a JSON array
#[derive(Debug, Clone)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema), schema(transparent))]
pub struct BatchQuoteStatusResponse(
    /// Vector of quote status responses as JSON
    pub Vec<BatchQuoteStatusItem>,
);

impl Serialize for BatchQuoteStatusResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BatchQuoteStatusResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Vec::<BatchQuoteStatusItem>::deserialize(deserializer).map(BatchQuoteStatusResponse)
    }
}

fn derive_quote_state(amount_paid: Amount, amount_issued: Amount) -> MintQuoteState {
    if amount_paid == Amount::ZERO && amount_issued == Amount::ZERO {
        return MintQuoteState::Unpaid;
    }

    match amount_paid.cmp(&amount_issued) {
        std::cmp::Ordering::Less => {
            tracing::error!("Bolt12 quote has issued more than paid");
            MintQuoteState::Issued
        }
        std::cmp::Ordering::Equal => MintQuoteState::Issued,
        std::cmp::Ordering::Greater => MintQuoteState::Paid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CurrencyUnit, PublicKey};
    use std::str::FromStr;

    #[test]
    fn batch_quote_status_item_returns_bolt11_state() {
        let response = MintQuoteBolt11Response {
            quote: "quote-1".to_string(),
            request: "bolt11".to_string(),
            amount: Some(Amount::from(100u64)),
            unit: Some(CurrencyUnit::Sat),
            state: MintQuoteState::Paid,
            expiry: Some(42),
            pubkey: None,
        };

        let item: BatchQuoteStatusItem = response.clone().into();
        assert_eq!(item.state(), MintQuoteState::Paid);

        match item {
            BatchQuoteStatusItem::Bolt11(inner) => assert_eq!(inner.quote, response.quote),
            _ => panic!("Expected bolt11 variant"),
        }
    }

    #[test]
    fn batch_quote_status_item_derives_bolt12_state() {
        let pubkey = PublicKey::from_str(
            "0279BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798",
        )
        .expect("valid pubkey");

        let response = MintQuoteBolt12Response {
            quote: "quote-2".to_string(),
            request: "bolt12".to_string(),
            amount: Some(Amount::from(200u64)),
            unit: CurrencyUnit::Sat,
            expiry: Some(100),
            pubkey,
            amount_paid: Amount::from(200u64),
            amount_issued: Amount::from(50u64),
        };

        let item: BatchQuoteStatusItem = response.into();
        assert_eq!(item.state(), MintQuoteState::Paid);

        match item {
            BatchQuoteStatusItem::Bolt12(inner) => {
                assert_eq!(inner.quote.quote, "quote-2");
                assert_eq!(inner.state(), MintQuoteState::Paid);
            }
            _ => panic!("Expected bolt12 variant"),
        }
    }
}
