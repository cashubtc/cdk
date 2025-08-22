//! Mint types

use bitcoin::bip32::DerivationPath;
use cashu::util::unix_time;
use cashu::{
    Bolt11Invoice, MeltOptions, MeltQuoteBolt11Response, MintQuoteBolt11Response,
    MintQuoteBolt12Response, PaymentMethod,
};
use lightning::offers::offer::Offer;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use crate::nuts::{MeltQuoteState, MintQuoteState};
use crate::payment::PaymentIdentifier;
use crate::{Amount, CurrencyUnit, Id, KeySetInfo, PublicKey};

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuote {
    /// Quote id
    pub id: Uuid,
    /// Amount of quote
    pub amount: Option<Amount>,
    /// Unit of quote
    pub unit: CurrencyUnit,
    /// Quote payment request e.g. bolt11
    pub request: String,
    /// Expiration time of quote
    pub expiry: u64,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: PaymentIdentifier,
    /// Pubkey
    pub pubkey: Option<PublicKey>,
    /// Unix time quote was created
    #[serde(default)]
    pub created_time: u64,
    /// Amount paid
    #[serde(default)]
    amount_paid: Amount,
    /// Amount issued
    #[serde(default)]
    amount_issued: Amount,
    /// Payment of payment(s) that filled quote
    #[serde(default)]
    pub payments: Vec<IncomingPayment>,
    /// Payment Method
    #[serde(default)]
    pub payment_method: PaymentMethod,
    /// Payment of payment(s) that filled quote
    #[serde(default)]
    pub issuance: Vec<Issuance>,
}

impl MintQuote {
    /// Create new [`MintQuote`]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Option<Uuid>,
        request: String,
        unit: CurrencyUnit,
        amount: Option<Amount>,
        expiry: u64,
        request_lookup_id: PaymentIdentifier,
        pubkey: Option<PublicKey>,
        amount_paid: Amount,
        amount_issued: Amount,
        payment_method: PaymentMethod,
        created_time: u64,
        payments: Vec<IncomingPayment>,
        issuance: Vec<Issuance>,
    ) -> Self {
        let id = id.unwrap_or(Uuid::new_v4());

        Self {
            id,
            amount,
            unit,
            request,
            expiry,
            request_lookup_id,
            pubkey,
            created_time,
            amount_paid,
            amount_issued,
            payment_method,
            payments,
            issuance,
        }
    }

    /// Increment the amount paid on the mint quote by a given amount
    #[instrument(skip(self))]
    pub fn increment_amount_paid(
        &mut self,
        additional_amount: Amount,
    ) -> Result<Amount, crate::Error> {
        self.amount_paid = self
            .amount_paid
            .checked_add(additional_amount)
            .ok_or(crate::Error::AmountOverflow)?;
        Ok(self.amount_paid)
    }

    /// Amount paid
    #[instrument(skip(self))]
    pub fn amount_paid(&self) -> Amount {
        self.amount_paid
    }

    /// Increment the amount issued on the mint quote by a given amount
    #[instrument(skip(self))]
    pub fn increment_amount_issued(
        &mut self,
        additional_amount: Amount,
    ) -> Result<Amount, crate::Error> {
        self.amount_issued = self
            .amount_issued
            .checked_add(additional_amount)
            .ok_or(crate::Error::AmountOverflow)?;
        Ok(self.amount_issued)
    }

    /// Amount issued
    #[instrument(skip(self))]
    pub fn amount_issued(&self) -> Amount {
        self.amount_issued
    }

    /// Get state of mint quote
    #[instrument(skip(self))]
    pub fn state(&self) -> MintQuoteState {
        self.compute_quote_state()
    }

    /// Existing payment ids of a mint quote
    pub fn payment_ids(&self) -> Vec<&String> {
        self.payments.iter().map(|a| &a.payment_id).collect()
    }

    /// Add a payment ID to the list of payment IDs
    ///
    /// Returns an error if the payment ID is already in the list
    #[instrument(skip(self))]
    pub fn add_payment(
        &mut self,
        amount: Amount,
        payment_id: String,
        time: u64,
    ) -> Result<(), crate::Error> {
        let payment_ids = self.payment_ids();
        if payment_ids.contains(&&payment_id) {
            return Err(crate::Error::DuplicatePaymentId);
        }

        let payment = IncomingPayment::new(amount, payment_id, time);

        self.payments.push(payment);
        Ok(())
    }

    /// Compute quote state
    #[instrument(skip(self))]
    fn compute_quote_state(&self) -> MintQuoteState {
        if self.amount_paid == Amount::ZERO && self.amount_issued == Amount::ZERO {
            return MintQuoteState::Unpaid;
        }

        match self.amount_paid.cmp(&self.amount_issued) {
            std::cmp::Ordering::Less => {
                // self.amount_paid is less than other (amount issued)
                // Handle case where paid amount is insufficient
                tracing::error!("We should not have issued more then has been paid");
                MintQuoteState::Issued
            }
            std::cmp::Ordering::Equal => {
                // We do this extra check for backwards compatibility for quotes where amount paid/issed was not tracked
                // self.amount_paid equals other (amount issued)
                // Handle case where paid amount exactly matches
                MintQuoteState::Issued
            }
            std::cmp::Ordering::Greater => {
                // self.amount_paid is greater than other (amount issued)
                // Handle case where paid amount exceeds required amount
                MintQuoteState::Paid
            }
        }
    }
}

/// Mint Payments
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingPayment {
    /// Amount
    pub amount: Amount,
    /// Pyament unix time
    pub time: u64,
    /// Payment id
    pub payment_id: String,
}

impl IncomingPayment {
    /// New [`IncomingPayment`]
    pub fn new(amount: Amount, payment_id: String, time: u64) -> Self {
        Self {
            payment_id,
            time,
            amount,
        }
    }
}

/// Informattion about issued quote
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issuance {
    /// Amount
    pub amount: Amount,
    /// Time
    pub time: u64,
}

impl Issuance {
    /// Create new [`Issuance`]
    pub fn new(amount: Amount, time: u64) -> Self {
        Self { amount, time }
    }
}

/// Melt Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuote {
    /// Quote id
    pub id: Uuid,
    /// Quote unit
    pub unit: CurrencyUnit,
    /// Quote amount
    pub amount: Amount,
    /// Quote Payment request e.g. bolt11
    pub request: MeltPaymentRequest,
    /// Quote fee reserve
    pub fee_reserve: Amount,
    /// Quote state
    pub state: MeltQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Payment preimage
    pub payment_preimage: Option<String>,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: Option<PaymentIdentifier>,
    /// Payment options
    ///
    /// Used for amountless invoices and MPP payments
    pub options: Option<MeltOptions>,
    /// Unix time quote was created
    #[serde(default)]
    pub created_time: u64,
    /// Unix time quote was paid
    pub paid_time: Option<u64>,
    /// Payment method
    #[serde(default)]
    pub payment_method: PaymentMethod,
}

impl MeltQuote {
    /// Create new [`MeltQuote`]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request: MeltPaymentRequest,
        unit: CurrencyUnit,
        amount: Amount,
        fee_reserve: Amount,
        expiry: u64,
        request_lookup_id: Option<PaymentIdentifier>,
        options: Option<MeltOptions>,
        payment_method: PaymentMethod,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            id,
            amount,
            unit,
            request,
            fee_reserve,
            state: MeltQuoteState::Unpaid,
            expiry,
            payment_preimage: None,
            request_lookup_id,
            options,
            created_time: unix_time(),
            paid_time: None,
            payment_method,
        }
    }
}

/// Mint Keyset Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeySetInfo {
    /// Keyset [`Id`]
    pub id: Id,
    /// Keyset [`CurrencyUnit`]
    pub unit: CurrencyUnit,
    /// Keyset active or inactive
    /// Mint will only issue new signatures on active keysets
    pub active: bool,
    /// Starting unix time Keyset is valid from
    pub valid_from: u64,
    /// [`DerivationPath`] keyset
    pub derivation_path: DerivationPath,
    /// DerivationPath index of Keyset
    pub derivation_path_index: Option<u32>,
    /// Max order of keyset
    pub max_order: u8,
    /// Input Fee ppk
    #[serde(default = "default_fee")]
    pub input_fee_ppk: u64,
    /// Final expiry
    pub final_expiry: Option<u64>,
}

/// Default fee
pub fn default_fee() -> u64 {
    0
}

impl From<MintKeySetInfo> for KeySetInfo {
    fn from(keyset_info: MintKeySetInfo) -> Self {
        Self {
            id: keyset_info.id,
            unit: keyset_info.unit,
            active: keyset_info.active,
            input_fee_ppk: keyset_info.input_fee_ppk,
            final_expiry: keyset_info.final_expiry,
        }
    }
}

impl From<MintQuote> for MintQuoteBolt11Response<Uuid> {
    fn from(mint_quote: crate::mint::MintQuote) -> MintQuoteBolt11Response<Uuid> {
        MintQuoteBolt11Response {
            quote: mint_quote.id,
            state: mint_quote.state(),
            request: mint_quote.request,
            expiry: Some(mint_quote.expiry),
            pubkey: mint_quote.pubkey,
            amount: mint_quote.amount,
            unit: Some(mint_quote.unit.clone()),
        }
    }
}

impl From<MintQuote> for MintQuoteBolt11Response<String> {
    fn from(quote: MintQuote) -> Self {
        let quote: MintQuoteBolt11Response<Uuid> = quote.into();

        quote.into()
    }
}

impl TryFrom<crate::mint::MintQuote> for MintQuoteBolt12Response<Uuid> {
    type Error = crate::Error;

    fn try_from(mint_quote: crate::mint::MintQuote) -> Result<Self, Self::Error> {
        Ok(MintQuoteBolt12Response {
            quote: mint_quote.id,
            request: mint_quote.request,
            expiry: Some(mint_quote.expiry),
            amount_paid: mint_quote.amount_paid,
            amount_issued: mint_quote.amount_issued,
            pubkey: mint_quote.pubkey.ok_or(crate::Error::PubkeyRequired)?,
            amount: mint_quote.amount,
            unit: mint_quote.unit,
        })
    }
}

impl TryFrom<MintQuote> for MintQuoteBolt12Response<String> {
    type Error = crate::Error;

    fn try_from(quote: MintQuote) -> Result<Self, Self::Error> {
        let quote: MintQuoteBolt12Response<Uuid> = quote.try_into()?;

        Ok(quote.into())
    }
}

impl From<&MeltQuote> for MeltQuoteBolt11Response<Uuid> {
    fn from(melt_quote: &MeltQuote) -> MeltQuoteBolt11Response<Uuid> {
        MeltQuoteBolt11Response {
            quote: melt_quote.id,
            payment_preimage: None,
            change: None,
            state: melt_quote.state,
            paid: Some(melt_quote.state == MeltQuoteState::Paid),
            expiry: melt_quote.expiry,
            amount: melt_quote.amount,
            fee_reserve: melt_quote.fee_reserve,
            request: None,
            unit: Some(melt_quote.unit.clone()),
        }
    }
}

impl From<MeltQuote> for MeltQuoteBolt11Response<Uuid> {
    fn from(melt_quote: MeltQuote) -> MeltQuoteBolt11Response<Uuid> {
        let paid = melt_quote.state == MeltQuoteState::Paid;
        MeltQuoteBolt11Response {
            quote: melt_quote.id,
            amount: melt_quote.amount,
            fee_reserve: melt_quote.fee_reserve,
            paid: Some(paid),
            state: melt_quote.state,
            expiry: melt_quote.expiry,
            payment_preimage: melt_quote.payment_preimage,
            change: None,
            request: Some(melt_quote.request.to_string()),
            unit: Some(melt_quote.unit.clone()),
        }
    }
}

/// Payment request
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeltPaymentRequest {
    /// Bolt11 Payment
    Bolt11 {
        /// Bolt11 invoice
        bolt11: Bolt11Invoice,
    },
    /// Bolt12 Payment
    Bolt12 {
        /// Offer
        #[serde(with = "offer_serde")]
        offer: Box<Offer>,
    },
}

impl std::fmt::Display for MeltPaymentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeltPaymentRequest::Bolt11 { bolt11 } => write!(f, "{bolt11}"),
            MeltPaymentRequest::Bolt12 { offer } => write!(f, "{offer}"),
        }
    }
}

mod offer_serde {
    use std::str::FromStr;

    use serde::{self, Deserialize, Deserializer, Serializer};

    use super::Offer;

    pub fn serialize<S>(offer: &Offer, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = offer.to_string();
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Box<Offer>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Box::new(Offer::from_str(&s).map_err(|_| {
            serde::de::Error::custom("Invalid Bolt12 Offer")
        })?))
    }
}
