//! Mint types

use std::fmt;
use std::str::FromStr;

use bitcoin::bip32::DerivationPath;
use cashu::quote_id::QuoteId;
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
use crate::{Amount, CurrencyUnit, Error, Id, KeySetInfo, PublicKey};

/// Operation kind for saga persistence
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationKind {
    /// Swap operation
    Swap,
    /// Mint operation
    Mint,
    /// Melt operation
    Melt,
}

impl fmt::Display for OperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperationKind::Swap => write!(f, "swap"),
            OperationKind::Mint => write!(f, "mint"),
            OperationKind::Melt => write!(f, "melt"),
        }
    }
}

impl FromStr for OperationKind {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.to_lowercase();
        match value.as_str() {
            "swap" => Ok(OperationKind::Swap),
            "mint" => Ok(OperationKind::Mint),
            "melt" => Ok(OperationKind::Melt),
            _ => Err(Error::Custom(format!("Invalid operation kind: {value}"))),
        }
    }
}

/// States specific to swap saga
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapSagaState {
    /// Swap setup complete (proofs added, blinded messages added)
    SetupComplete,
    /// Outputs signed (signatures generated but not persisted)
    Signed,
}

impl fmt::Display for SwapSagaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SwapSagaState::SetupComplete => write!(f, "setup_complete"),
            SwapSagaState::Signed => write!(f, "signed"),
        }
    }
}

impl FromStr for SwapSagaState {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.to_lowercase();
        match value.as_str() {
            "setup_complete" => Ok(SwapSagaState::SetupComplete),
            "signed" => Ok(SwapSagaState::Signed),
            _ => Err(Error::Custom(format!("Invalid swap saga state: {value}"))),
        }
    }
}

/// States specific to melt saga
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeltSagaState {
    /// Setup complete (proofs reserved, quote verified)
    SetupComplete,
    /// Payment attempted to Lightning network (may or may not have succeeded)
    PaymentAttempted,
}

impl fmt::Display for MeltSagaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeltSagaState::SetupComplete => write!(f, "setup_complete"),
            MeltSagaState::PaymentAttempted => write!(f, "payment_attempted"),
        }
    }
}

impl FromStr for MeltSagaState {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.to_lowercase();
        match value.as_str() {
            "setup_complete" => Ok(MeltSagaState::SetupComplete),
            "payment_attempted" => Ok(MeltSagaState::PaymentAttempted),
            _ => Err(Error::Custom(format!("Invalid melt saga state: {}", value))),
        }
    }
}

/// Saga state for different operation types
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SagaStateEnum {
    /// Swap saga states
    Swap(SwapSagaState),
    /// Melt saga states
    Melt(MeltSagaState),
    // Future: Mint saga states
    // Mint(MintSagaState),
}

impl SagaStateEnum {
    /// Create from string given operation kind
    pub fn new(operation_kind: OperationKind, s: &str) -> Result<Self, Error> {
        match operation_kind {
            OperationKind::Swap => Ok(SagaStateEnum::Swap(SwapSagaState::from_str(s)?)),
            OperationKind::Melt => Ok(SagaStateEnum::Melt(MeltSagaState::from_str(s)?)),
            OperationKind::Mint => Err(Error::Custom("Mint saga not implemented yet".to_string())),
        }
    }

    /// Get string representation of the state
    pub fn state(&self) -> &str {
        match self {
            SagaStateEnum::Swap(state) => match state {
                SwapSagaState::SetupComplete => "setup_complete",
                SwapSagaState::Signed => "signed",
            },
            SagaStateEnum::Melt(state) => match state {
                MeltSagaState::SetupComplete => "setup_complete",
                MeltSagaState::PaymentAttempted => "payment_attempted",
            },
        }
    }
}

/// Persisted saga for recovery
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saga {
    /// Operation ID (correlation key)
    pub operation_id: Uuid,
    /// Operation kind (swap, mint, melt)
    pub operation_kind: OperationKind,
    /// Current saga state (operation-specific)
    pub state: SagaStateEnum,
    /// Quote ID for melt operations (used for payment status lookup during recovery)
    /// None for swap operations
    pub quote_id: Option<String>,
    /// Unix timestamp when saga was created
    pub created_at: u64,
    /// Unix timestamp when saga was last updated
    pub updated_at: u64,
}

impl Saga {
    /// Create new swap saga
    pub fn new_swap(operation_id: Uuid, state: SwapSagaState) -> Self {
        let now = unix_time();
        Self {
            operation_id,
            operation_kind: OperationKind::Swap,
            state: SagaStateEnum::Swap(state),
            quote_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Update swap saga state
    pub fn update_swap_state(&mut self, new_state: SwapSagaState) {
        self.state = SagaStateEnum::Swap(new_state);
        self.updated_at = unix_time();
    }

    /// Create new melt saga
    pub fn new_melt(operation_id: Uuid, state: MeltSagaState, quote_id: String) -> Self {
        let now = unix_time();
        Self {
            operation_id,
            operation_kind: OperationKind::Melt,
            state: SagaStateEnum::Melt(state),
            quote_id: Some(quote_id),
            created_at: now,
            updated_at: now,
        }
    }

    /// Update melt saga state
    pub fn update_melt_state(&mut self, new_state: MeltSagaState) {
        self.state = SagaStateEnum::Melt(new_state);
        self.updated_at = unix_time();
    }
}

/// Operation
pub struct Operation {
    id: Uuid,
    kind: OperationKind,
    total_issued: Amount,
    total_redeemed: Amount,
    fee_collected: Amount,
    complete_at: Option<u64>,
    /// Payment amount (only for melt operations)
    payment_amount: Option<Amount>,
    /// Payment fee (only for melt operations)
    payment_fee: Option<Amount>,
    /// Payment method (only for mint/melt operations)
    payment_method: Option<PaymentMethod>,
}

impl Operation {
    /// New
    pub fn new(
        id: Uuid,
        kind: OperationKind,
        total_issued: Amount,
        total_redeemed: Amount,
        fee_collected: Amount,
        complete_at: Option<u64>,
        payment_method: Option<PaymentMethod>,
    ) -> Self {
        Self {
            id,
            kind,
            total_issued,
            total_redeemed,
            fee_collected,
            complete_at,
            payment_amount: None,
            payment_fee: None,
            payment_method,
        }
    }

    /// Mint
    pub fn new_mint(total_issued: Amount, payment_method: PaymentMethod) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: OperationKind::Mint,
            total_issued,
            total_redeemed: Amount::ZERO,
            fee_collected: Amount::ZERO,
            complete_at: None,
            payment_amount: None,
            payment_fee: None,
            payment_method: Some(payment_method),
        }
    }
    /// Melt
    ///
    /// In the context of a melt total_issued refrests to the change
    pub fn new_melt(
        total_redeemed: Amount,
        fee_collected: Amount,
        payment_method: PaymentMethod,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: OperationKind::Melt,
            total_issued: Amount::ZERO,
            total_redeemed,
            fee_collected,
            complete_at: None,
            payment_amount: None,
            payment_fee: None,
            payment_method: Some(payment_method),
        }
    }

    /// Swap
    pub fn new_swap(total_issued: Amount, total_redeemed: Amount, fee_collected: Amount) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: OperationKind::Swap,
            total_issued,
            total_redeemed,
            fee_collected,
            complete_at: None,
            payment_amount: None,
            payment_fee: None,
            payment_method: None,
        }
    }

    /// Operation id
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Operation kind
    pub fn kind(&self) -> OperationKind {
        self.kind
    }

    /// Total issued
    pub fn total_issued(&self) -> Amount {
        self.total_issued
    }

    /// Total redeemed
    pub fn total_redeemed(&self) -> Amount {
        self.total_redeemed
    }

    /// Fee collected
    pub fn fee_collected(&self) -> Amount {
        self.fee_collected
    }

    /// Completed time
    pub fn completed_at(&self) -> &Option<u64> {
        &self.complete_at
    }

    /// Add change
    pub fn add_change(&mut self, change: Amount) {
        self.total_issued = change;
    }

    /// Payment amount (only for melt operations)
    pub fn payment_amount(&self) -> Option<Amount> {
        self.payment_amount
    }

    /// Payment fee (only for melt operations)
    pub fn payment_fee(&self) -> Option<Amount> {
        self.payment_fee
    }

    /// Set payment details for melt operations
    pub fn set_payment_details(&mut self, payment_amount: Amount, payment_fee: Amount) {
        self.payment_amount = Some(payment_amount);
        self.payment_fee = Some(payment_fee);
    }

    /// Payment method (only for mint/melt operations)
    pub fn payment_method(&self) -> Option<PaymentMethod> {
        self.payment_method.clone()
    }
}

/// Tracks pending changes made to a [`MintQuote`] that need to be persisted.
///
/// This struct implements a change-tracking pattern that separates domain logic from
/// persistence concerns. When modifications are made to a `MintQuote` via methods like
/// [`MintQuote::add_payment`] or [`MintQuote::add_issuance`], the changes are recorded
/// here rather than being immediately persisted. The database layer can then call
/// [`MintQuote::take_changes`] to retrieve and persist only the modifications.
///
/// This approach allows business rule validation to happen in the domain model while
/// keeping the database layer focused purely on persistence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct MintQuoteChange {
    /// New payments added since the quote was loaded or last persisted.
    pub payments: Option<Vec<IncomingPayment>>,
    /// New issuance amounts recorded since the quote was loaded or last persisted.
    pub issuances: Option<Vec<Amount>>,
}

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuote {
    /// Quote id
    pub id: QuoteId,
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
    /// Accumulated changes since this quote was loaded or created.
    ///
    /// This field is not serialized and is used internally to track modifications
    /// that need to be persisted. Use [`Self::take_changes`] to extract pending
    /// changes for persistence.
    #[serde(skip)]
    changes: Option<MintQuoteChange>,
}

impl MintQuote {
    /// Create new [`MintQuote`]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Option<QuoteId>,
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
        let id = id.unwrap_or_else(QuoteId::new_uuid);

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
            changes: None,
        }
    }

    /// Amount paid
    #[instrument(skip(self))]
    pub fn amount_paid(&self) -> Amount {
        self.amount_paid
    }

    /// Records tokens being issued against this mint quote.
    ///
    /// This method validates that the issuance doesn't exceed the amount paid, updates
    /// the quote's internal state, and records the change for later persistence. The
    /// `amount_issued` counter is incremented and the issuance is added to the change
    /// tracker for the database layer to persist.
    ///
    /// # Arguments
    ///
    /// * `additional_amount` - The amount of tokens being issued.
    ///
    /// # Returns
    ///
    /// Returns the new total `amount_issued` after this issuance is recorded.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::OverIssue`] if the new issued amount would exceed the
    /// amount paid (cannot issue more tokens than have been paid for).
    ///
    /// Returns [`crate::Error::AmountOverflow`] if adding the issuance amount would
    /// cause an arithmetic overflow.
    #[instrument(skip(self))]
    pub fn add_issuance(&mut self, additional_amount: Amount) -> Result<Amount, crate::Error> {
        let new_amount_issued = self
            .amount_issued
            .checked_add(additional_amount)
            .ok_or(crate::Error::AmountOverflow)?;

        // Can't issue more than what's been paid
        if new_amount_issued > self.amount_paid {
            return Err(crate::Error::OverIssue);
        }

        self.changes
            .get_or_insert_default()
            .issuances
            .get_or_insert_default()
            .push(additional_amount);

        self.amount_issued = new_amount_issued;

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

    /// Amount mintable
    /// Returns the amount that is still available for minting.
    ///
    /// The value is computed as the difference between the total amount that
    /// has been paid for this issuance (`self.amount_paid`) and the amount
    /// that has already been issued (`self.amount_issued`). In other words,
    pub fn amount_mintable(&self) -> Amount {
        self.amount_paid - self.amount_issued
    }

    /// Extracts and returns all pending changes, leaving the internal change tracker empty.
    ///
    /// This method is typically called by the database layer after loading or modifying a quote. It
    /// returns any accumulated changes (new payments, issuances) that need to be persisted, and
    /// clears the internal change buffer so that subsequent calls return `None` until new
    /// modifications are made.
    ///
    /// Returns `None` if no changes have been made since the last call to this method or since the
    /// quote was created/loaded.
    pub fn take_changes(&mut self) -> Option<MintQuoteChange> {
        self.changes.take()
    }

    /// Records a new payment received for this mint quote.
    ///
    /// This method validates the payment, updates the quote's internal state, and records the
    /// change for later persistence. The `amount_paid` counter is incremented and the payment is
    /// added to the change tracker for the database layer to persist.
    ///
    /// # Arguments
    ///
    /// * `amount` - The amount of the payment in the quote's currency unit. * `payment_id` - A
    /// unique identifier for this payment (e.g., lightning payment hash). * `time` - Optional Unix
    /// timestamp of when the payment was received. If `None`, the current time is used.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::DuplicatePaymentId`] if a payment with the same ID has already been
    /// recorded for this quote.
    ///
    /// Returns [`crate::Error::AmountOverflow`] if adding the payment amount would cause an
    /// arithmetic overflow.
    #[instrument(skip(self))]
    pub fn add_payment(
        &mut self,
        amount: Amount,
        payment_id: String,
        time: Option<u64>,
    ) -> Result<(), crate::Error> {
        let time = time.unwrap_or_else(unix_time);

        let payment_ids = self.payment_ids();
        if payment_ids.contains(&&payment_id) {
            return Err(crate::Error::DuplicatePaymentId);
        }

        let payment = IncomingPayment::new(amount, payment_id, time);

        self.payments.push(payment.clone());

        self.changes
            .get_or_insert_default()
            .payments
            .get_or_insert_default()
            .push(payment);

        self.amount_paid = self
            .amount_paid
            .checked_add(amount)
            .ok_or(crate::Error::AmountOverflow)?;

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
    pub id: QuoteId,
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
            id: QuoteId::UUID(id),
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
    /// Supported amounts
    pub amounts: Vec<u64>,
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

impl From<MintQuote> for MintQuoteBolt11Response<QuoteId> {
    fn from(mint_quote: crate::mint::MintQuote) -> MintQuoteBolt11Response<QuoteId> {
        MintQuoteBolt11Response {
            quote: mint_quote.id.clone(),
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
        let quote: MintQuoteBolt11Response<QuoteId> = quote.into();

        quote.into()
    }
}

impl TryFrom<crate::mint::MintQuote> for MintQuoteBolt12Response<QuoteId> {
    type Error = crate::Error;

    fn try_from(mint_quote: crate::mint::MintQuote) -> Result<Self, Self::Error> {
        Ok(MintQuoteBolt12Response {
            quote: mint_quote.id.clone(),
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
        let quote: MintQuoteBolt12Response<QuoteId> = quote.try_into()?;

        Ok(quote.into())
    }
}

impl From<&MeltQuote> for MeltQuoteBolt11Response<QuoteId> {
    fn from(melt_quote: &MeltQuote) -> MeltQuoteBolt11Response<QuoteId> {
        MeltQuoteBolt11Response {
            quote: melt_quote.id.clone(),
            payment_preimage: None,
            change: None,
            state: melt_quote.state,
            expiry: melt_quote.expiry,
            amount: melt_quote.amount,
            fee_reserve: melt_quote.fee_reserve,
            request: None,
            unit: Some(melt_quote.unit.clone()),
        }
    }
}

impl From<MeltQuote> for MeltQuoteBolt11Response<QuoteId> {
    fn from(melt_quote: MeltQuote) -> MeltQuoteBolt11Response<QuoteId> {
        MeltQuoteBolt11Response {
            quote: melt_quote.id.clone(),
            amount: melt_quote.amount,
            fee_reserve: melt_quote.fee_reserve,
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
