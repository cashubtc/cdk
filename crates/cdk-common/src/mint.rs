//! Mint types

use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use bitcoin::bip32::DerivationPath;
use cashu::nuts::nut30::MeltQuoteOnchainFeeOption;
use cashu::quote_id::QuoteId;
use cashu::util::unix_time;
use cashu::{
    Bolt11Invoice, MeltOptions, MeltQuoteBolt11Response, MeltQuoteCustomResponse,
    MeltQuoteOnchainResponse, MintQuoteBolt11Response, MintQuoteBolt12Response,
    MintQuoteCustomResponse, MintQuoteOnchainResponse, PaymentMethod, Proofs, State,
};
use lightning::offers::offer::Offer;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use crate::common::IssuerVersion;
use crate::mint_quote::MintQuoteResponse;
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
    /// Batch mint
    BatchMint,
}

/// A collection of proofs that share a common state.
///
/// This type enforces the invariant that all proofs in the collection have the same state.
/// The mint never needs to operate on a set of proofs with different states - proofs are
/// always processed together as a unit (e.g., during swap, melt, or mint operations).
///
/// # Database Layer Responsibility
///
/// This design shifts the responsibility of ensuring state consistency to the database layer.
/// When the database retrieves proofs via [`get_proofs`](crate::database::mint::ProofsTransaction::get_proofs),
/// it must verify that all requested proofs share the same state and return an error if they don't.
/// This prevents invalid proof sets from propagating through the system.
///
/// # State Transitions
///
/// State transitions are validated using [`check_state_transition`](crate::state::check_state_transition)
/// before updating. The database layer then persists the new state for all proofs in a single transaction
/// via [`update_proofs_state`](crate::database::mint::ProofsTransaction::update_proofs_state).
///
/// # Example
///
/// ```ignore
/// // Database layer ensures all proofs have the same state
/// let mut proofs = tx.get_proofs(&ys).await?;
///
/// // Validate the state transition
/// check_state_transition(proofs.state, State::Spent)?;
///
/// // Persist the state change
/// tx.update_proofs_state(&mut proofs, State::Spent).await?;
/// ```
#[derive(Debug)]
pub struct ProofsWithState {
    proofs: Proofs,
    /// The current state of the proofs
    pub state: State,
}

impl Deref for ProofsWithState {
    type Target = Proofs;

    fn deref(&self) -> &Self::Target {
        &self.proofs
    }
}

impl ProofsWithState {
    /// Creates a new `ProofsWithState` with the given proofs and their shared state.
    ///
    /// # Note
    ///
    /// This constructor assumes all proofs share the given state. It is typically
    /// called by the database layer after verifying state consistency.
    pub fn new(proofs: Proofs, current_state: State) -> Self {
        Self {
            proofs,
            state: current_state,
        }
    }
}

impl fmt::Display for OperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperationKind::Swap => write!(f, "swap"),
            OperationKind::Mint => write!(f, "mint"),
            OperationKind::Melt => write!(f, "melt"),
            OperationKind::BatchMint => write!(f, "batch_mint"),
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
            "batch_mint" => Ok(OperationKind::BatchMint),
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
    /// TX1 committed (proofs Spent, quote Paid) - change signing + cleanup pending
    Finalizing,
}

impl fmt::Display for MeltSagaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeltSagaState::SetupComplete => write!(f, "setup_complete"),
            MeltSagaState::PaymentAttempted => write!(f, "payment_attempted"),
            MeltSagaState::Finalizing => write!(f, "finalizing"),
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
            "finalizing" => Ok(MeltSagaState::Finalizing),
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
            OperationKind::Mint | OperationKind::BatchMint => {
                Err(Error::Custom("Mint saga not implemented yet".to_string()))
            }
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
                MeltSagaState::Finalizing => "finalizing",
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
    /// Exact payment result for resuming melt finalization after TX1 commits.
    pub finalization_data: Option<MeltFinalizationData>,
    /// Unix timestamp when saga was created
    pub created_at: u64,
    /// Unix timestamp when saga was last updated
    pub updated_at: u64,
}

/// Persisted payment result for resuming melt finalization after a crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeltFinalizationData {
    /// Total amount actually spent on the payment.
    pub total_spent: Amount<CurrencyUnit>,
    /// Backend payment lookup identifier.
    pub payment_lookup_id: PaymentIdentifier,
    /// Optional payment proof / preimage.
    pub payment_proof: Option<String>,
}

impl Serialize for MeltFinalizationData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct MeltFinalizationDataSer<'a> {
            total_spent: Amount,
            unit: &'a CurrencyUnit,
            payment_lookup_id: &'a PaymentIdentifier,
            payment_proof: &'a Option<String>,
        }

        MeltFinalizationDataSer {
            total_spent: self.total_spent.clone().into(),
            unit: self.total_spent.unit(),
            payment_lookup_id: &self.payment_lookup_id,
            payment_proof: &self.payment_proof,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MeltFinalizationData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct MeltFinalizationDataDe {
            total_spent: Amount,
            unit: CurrencyUnit,
            payment_lookup_id: PaymentIdentifier,
            payment_proof: Option<String>,
        }

        let data = MeltFinalizationDataDe::deserialize(deserializer)?;

        Ok(Self {
            total_spent: data.total_spent.with_unit(data.unit),
            payment_lookup_id: data.payment_lookup_id,
            payment_proof: data.payment_proof,
        })
    }
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
            finalization_data: None,
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
            finalization_data: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Update melt saga state
    pub fn update_melt_state(&mut self, new_state: MeltSagaState) {
        self.state = SagaStateEnum::Melt(new_state);
        self.updated_at = unix_time();
    }

    /// Store exact payment data needed to resume melt finalization after TX1.
    pub fn set_melt_finalization_data(&mut self, finalization_data: MeltFinalizationData) {
        self.finalization_data = Some(finalization_data);
        self.updated_at = unix_time();
    }
}

/// Operation
#[derive(Debug)]
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
            id: Uuid::now_v7(),
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

    /// Batch mint
    pub fn new_batch_mint(total_issued: Amount, payment_method: PaymentMethod) -> Self {
        Self {
            id: Uuid::now_v7(),
            kind: OperationKind::BatchMint,
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
            id: Uuid::now_v7(),
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
            id: Uuid::now_v7(),
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
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MintQuote {
    /// Quote id
    pub id: QuoteId,
    /// Amount of quote
    pub amount: Option<Amount<CurrencyUnit>>,
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
    pub created_time: u64,
    /// Amount paid (typed for type safety)
    amount_paid: Amount<CurrencyUnit>,
    /// Amount issued (typed for type safety)
    amount_issued: Amount<CurrencyUnit>,
    /// Payment of payment(s) that filled quote
    pub payments: Vec<IncomingPayment>,
    /// Payment Method
    pub payment_method: PaymentMethod,
    /// Payment of payment(s) that filled quote
    pub issuance: Vec<Issuance>,
    /// Extra payment-method-specific fields
    pub extra_json: Option<serde_json::Value>,
    /// Accumulated changes since this quote was loaded or created.
    ///
    /// This field is not serialized and is used internally to track modifications
    /// that need to be persisted. Use [`Self::take_changes`] to extract pending
    /// changes for persistence.
    changes: Option<MintQuoteChange>,
}

impl MintQuote {
    /// Create new [`MintQuote`]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Option<QuoteId>,
        request: String,
        unit: CurrencyUnit,
        amount: Option<Amount<CurrencyUnit>>,
        expiry: u64,
        request_lookup_id: PaymentIdentifier,
        pubkey: Option<PublicKey>,
        amount_paid: Amount<CurrencyUnit>,
        amount_issued: Amount<CurrencyUnit>,
        payment_method: PaymentMethod,
        created_time: u64,
        payments: Vec<IncomingPayment>,
        issuance: Vec<Issuance>,
        extra_json: Option<serde_json::Value>,
    ) -> Self {
        let id = id.unwrap_or_default();

        Self {
            id,
            amount,
            unit: unit.clone(),
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
            extra_json,
            changes: None,
        }
    }

    /// Increment the amount paid on the mint quote by a given amount
    #[instrument(skip(self))]
    pub fn increment_amount_paid(
        &mut self,
        additional_amount: Amount<CurrencyUnit>,
    ) -> Result<Amount, crate::Error> {
        self.amount_paid = self
            .amount_paid
            .checked_add(&additional_amount)
            .map_err(|_| crate::Error::AmountOverflow)?;
        Ok(Amount::from(self.amount_paid.value()))
    }

    /// Amount paid
    #[instrument(skip(self))]
    pub fn amount_paid(&self) -> Amount<CurrencyUnit> {
        self.amount_paid.clone()
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
    pub fn add_issuance(
        &mut self,
        additional_amount: Amount<CurrencyUnit>,
    ) -> Result<Amount<CurrencyUnit>, crate::Error> {
        let new_amount_issued = self
            .amount_issued
            .checked_add(&additional_amount)
            .map_err(|_| crate::Error::AmountOverflow)?;

        // Can't issue more than what's been paid
        if new_amount_issued > self.amount_paid {
            return Err(crate::Error::OverIssue);
        }

        self.changes
            .get_or_insert_default()
            .issuances
            .get_or_insert_default()
            .push(additional_amount.into());

        self.amount_issued = new_amount_issued;

        Ok(self.amount_issued.clone())
    }

    /// Amount issued
    #[instrument(skip(self))]
    pub fn amount_issued(&self) -> Amount<CurrencyUnit> {
        self.amount_issued.clone()
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
    pub fn amount_mintable(&self) -> Amount<CurrencyUnit> {
        self.amount_paid
            .checked_sub(&self.amount_issued)
            .unwrap_or_else(|_| Amount::new(0, self.unit.clone()))
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
        amount: Amount<CurrencyUnit>,
        payment_id: String,
        time: Option<u64>,
    ) -> Result<(), crate::Error> {
        let time = time.unwrap_or_else(unix_time);

        let payment_ids = self.payment_ids();
        if payment_ids.contains(&&payment_id) {
            return Err(crate::Error::DuplicatePaymentId);
        }

        self.amount_paid = self
            .amount_paid
            .checked_add(&amount)
            .map_err(|_| crate::Error::AmountOverflow)?;

        let payment = IncomingPayment::new(amount, payment_id, time);

        self.payments.push(payment.clone());

        self.changes
            .get_or_insert_default()
            .payments
            .get_or_insert_default()
            .push(payment);

        Ok(())
    }

    /// Compute quote state
    #[instrument(skip(self))]
    fn compute_quote_state(&self) -> MintQuoteState {
        let zero_amount = Amount::new(0, self.unit.clone());

        if self.amount_paid == zero_amount && self.amount_issued == zero_amount {
            return MintQuoteState::Unpaid;
        }

        match self.amount_paid.value().cmp(&self.amount_issued.value()) {
            std::cmp::Ordering::Less => {
                tracing::error!("We should not have issued more then has been paid");
                MintQuoteState::Issued
            }
            std::cmp::Ordering::Equal => MintQuoteState::Issued,
            std::cmp::Ordering::Greater => MintQuoteState::Paid,
        }
    }
}

/// Mint Payments
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct IncomingPayment {
    /// Amount
    pub amount: Amount<CurrencyUnit>,
    /// Pyament unix time
    pub time: u64,
    /// Payment id
    pub payment_id: String,
}

impl IncomingPayment {
    /// New [`IncomingPayment`]
    pub fn new(amount: Amount<CurrencyUnit>, payment_id: String, time: u64) -> Self {
        Self {
            payment_id,
            time,
            amount,
        }
    }
}

/// Information about issued quote
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Issuance {
    /// Amount
    pub amount: Amount<CurrencyUnit>,
    /// Time
    pub time: u64,
}

impl Issuance {
    /// Create new [`Issuance`]
    pub fn new(amount: Amount<CurrencyUnit>, time: u64) -> Self {
        Self { amount, time }
    }
}

/// Melt Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MeltQuote {
    /// Quote id
    pub id: QuoteId,
    /// Quote unit
    pub unit: CurrencyUnit,
    /// Quote Payment request e.g. bolt11
    pub request: MeltPaymentRequest,
    /// Quote amount (typed for type safety)
    amount: Amount<CurrencyUnit>,
    /// Quote fee reserve (typed for type safety)
    fee_reserve: Amount<CurrencyUnit>,
    /// Quote state
    pub state: MeltQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Payment proof (e.g. Lightning preimage or onchain outpoint)
    pub payment_proof: Option<String>,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: Option<PaymentIdentifier>,
    /// Payment options
    ///
    /// Used for amountless invoices and MPP payments
    pub options: Option<MeltOptions>,
    /// Unix time quote was created
    pub created_time: u64,
    /// Unix time quote was paid
    pub paid_time: Option<u64>,
    /// Payment method
    pub payment_method: PaymentMethod,
    /// Extra payment-method-specific response fields
    pub extra_json: Option<serde_json::Value>,
    /// Estimated confirmation target in blocks for onchain quotes
    pub estimated_blocks: Option<u32>,
    /// Onchain fee options fixed for the lifetime of the quote.
    ///
    /// Intentionally private: callers read via [`MeltQuote::fee_options`].
    /// This makes the "fixed for the lifetime of the quote" NUT invariant
    /// enforceable at the type level — external code cannot replace or push
    /// into the vec after construction. Mutations that do happen (via
    /// [`MeltQuote::select_onchain_fee_option`]) only touch
    /// `fee_reserve`/`estimated_blocks`/`selected_fee_index`, never
    /// this list.
    fee_options: Vec<MeltQuoteOnchainFeeOption>,
    /// Selected fee option index once an onchain quote is executed
    pub selected_fee_index: Option<u32>,
}

impl MeltQuote {
    /// Create new [`MeltQuote`]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Option<QuoteId>,
        request: MeltPaymentRequest,
        unit: CurrencyUnit,
        amount: Amount<CurrencyUnit>,
        fee_reserve: Amount<CurrencyUnit>,
        expiry: u64,
        request_lookup_id: Option<PaymentIdentifier>,
        options: Option<MeltOptions>,
        payment_method: PaymentMethod,
        extra_json: Option<serde_json::Value>,
        estimated_blocks: Option<u32>,
    ) -> Self {
        let id = id.unwrap_or_default();

        let fee_options = estimated_blocks
            .map(|estimated_blocks| {
                vec![MeltQuoteOnchainFeeOption {
                    fee_index: 0,
                    fee_reserve: fee_reserve.clone().into(),
                    estimated_blocks,
                }]
            })
            .unwrap_or_default();

        Self {
            id,
            unit: unit.clone(),
            request,
            amount,
            fee_reserve,
            state: MeltQuoteState::Unpaid,
            expiry,
            payment_proof: None,
            request_lookup_id,
            options,
            created_time: unix_time(),
            paid_time: None,
            payment_method,
            extra_json,
            estimated_blocks,
            fee_options,
            selected_fee_index: None,
        }
    }

    /// Create a new onchain [`MeltQuote`] with explicit `fee_options`.
    ///
    /// Preserves backend-provided `fee_index` values and validates that the
    /// quote contains at least one option (`OnchainFeeOptionsEmpty`).
    ///
    /// `fee_reserve` is initialized to the lowest-fee option so the quote has
    /// a definite reserve before the wallet selects a tier. Once the wallet
    /// calls [`MeltQuote::select_onchain_fee_option`] the reserve is updated
    /// to match the selected option. `fee_options` itself is never mutated
    /// after this call; that invariant is enforced by making the field
    /// private.
    #[allow(clippy::too_many_arguments)]
    pub fn new_onchain(
        id: Option<QuoteId>,
        request: MeltPaymentRequest,
        unit: CurrencyUnit,
        amount: Amount<CurrencyUnit>,
        expiry: u64,
        request_lookup_id: Option<PaymentIdentifier>,
        extra_json: Option<serde_json::Value>,
        fee_options: Vec<MeltQuoteOnchainFeeOption>,
    ) -> Result<Self, crate::Error> {
        if fee_options.is_empty() {
            return Err(crate::Error::OnchainFeeOptionsEmpty);
        }

        validate_onchain_fee_options(&fee_options)?;

        let id = id.unwrap_or_default();

        // Pick the lowest-reserve option as the initial reserve. The `ok_or` is
        // unreachable — we checked for empty above — but we use it instead of
        // `expect` to avoid a needless panic path.
        let initial = fee_options
            .iter()
            .min_by_key(|option| u64::from(option.fee_reserve))
            .copied()
            .ok_or(crate::Error::OnchainFeeOptionsEmpty)?;

        let fee_reserve = initial.fee_reserve.with_unit(unit.clone());
        let estimated_blocks = Some(initial.estimated_blocks);

        Ok(Self {
            id,
            unit: unit.clone(),
            request,
            amount,
            fee_reserve,
            state: MeltQuoteState::Unpaid,
            expiry,
            payment_proof: None,
            request_lookup_id,
            options: None,
            created_time: unix_time(),
            paid_time: None,
            payment_method: PaymentMethod::Known(cashu::nuts::nut00::KnownMethod::Onchain),
            extra_json,
            estimated_blocks,
            fee_options,
            selected_fee_index: None,
        })
    }

    /// Onchain fee options for this quote.
    ///
    /// For non-onchain quotes this returns an empty slice. For onchain quotes
    /// this is guaranteed non-empty (enforced at construction in
    /// [`MeltQuote::new_onchain`] and on reload in [`MeltQuote::from_db`]).
    #[inline]
    pub fn fee_options(&self) -> &[MeltQuoteOnchainFeeOption] {
        &self.fee_options
    }

    /// Quote amount
    #[inline]
    pub fn amount(&self) -> Amount<CurrencyUnit> {
        self.amount.clone()
    }

    /// Fee reserve
    #[inline]
    pub fn fee_reserve(&self) -> Amount<CurrencyUnit> {
        self.fee_reserve.clone()
    }

    /// Select an onchain fee option by its `fee_index`.
    pub fn select_onchain_fee_option(&mut self, fee_index: u32) -> Result<(), crate::Error> {
        let option = self
            .fee_options
            .iter()
            .find(|option| option.fee_index == fee_index)
            .copied()
            .ok_or(crate::Error::OnchainFeeIndexNotFound { index: fee_index })?;

        if self
            .selected_fee_index
            .is_some_and(|selected| selected != fee_index)
        {
            return Err(crate::Error::InvalidPaymentRequest);
        }

        self.fee_reserve = option.fee_reserve.with_unit(self.unit.clone());
        self.estimated_blocks = Some(option.estimated_blocks);
        self.selected_fee_index = Some(fee_index);

        Ok(())
    }

    /// Convert into `MeltQuoteResponse`, overriding `change` on the inner
    /// response with the provided signatures.
    ///
    /// Dispatches to the per-variant `From<MeltQuote>` conversions so that
    /// field mapping stays centralized. Note that `MeltQuoteBolt12Response`
    /// is a type alias for `MeltQuoteBolt11Response`, so both Bolt11 and
    /// Bolt12 go through the same conversion.
    pub fn into_response(
        self,
        change: Option<Vec<cashu::nuts::BlindSignature>>,
    ) -> crate::MeltQuoteResponse<QuoteId> {
        match self.payment_method {
            PaymentMethod::Known(cashu::nuts::nut00::KnownMethod::Bolt11) => {
                let mut response: MeltQuoteBolt11Response<QuoteId> = self.into();
                response.change = change;
                crate::MeltQuoteResponse::Bolt11(response)
            }
            PaymentMethod::Known(cashu::nuts::nut00::KnownMethod::Bolt12) => {
                let mut response: MeltQuoteBolt11Response<QuoteId> = self.into();
                response.change = change;
                crate::MeltQuoteResponse::Bolt12(response)
            }
            PaymentMethod::Known(cashu::nuts::nut00::KnownMethod::Onchain) => {
                let mut response: MeltQuoteOnchainResponse<QuoteId> = self.into();
                response.change = change;
                crate::MeltQuoteResponse::Onchain(response)
            }
            _ => {
                let method = self.payment_method.clone();
                let mut response: MeltQuoteCustomResponse<QuoteId> = self.into();
                response.change = change;
                crate::MeltQuoteResponse::Custom((method, response))
            }
        }
    }

    /// Total amount needed (amount + fee_reserve)
    pub fn total_needed(&self) -> Result<Amount, crate::Error> {
        let total = self
            .amount
            .checked_add(&self.fee_reserve)
            .map_err(|_| crate::Error::AmountOverflow)?;
        Ok(Amount::from(total.value()))
    }

    /// Create MeltQuote from database fields (for deserialization)
    #[allow(clippy::too_many_arguments)]
    pub fn from_db(
        id: QuoteId,
        unit: CurrencyUnit,
        request: MeltPaymentRequest,
        amount: u64,
        fee_reserve: u64,
        state: MeltQuoteState,
        expiry: u64,
        payment_proof: Option<String>,
        request_lookup_id: Option<PaymentIdentifier>,
        options: Option<MeltOptions>,
        created_time: u64,
        paid_time: Option<u64>,
        payment_method: PaymentMethod,
        extra_json: Option<serde_json::Value>,
        estimated_blocks: Option<u32>,
        fee_options: Vec<MeltQuoteOnchainFeeOption>,
        selected_fee_index: Option<u32>,
    ) -> Result<Self, crate::Error> {
        // For onchain quotes, re-validate the persisted `fee_options` so a
        // corrupted or hand-edited row cannot silently be served as a valid
        // quote. Non-onchain quotes legitimately carry an empty vec and are
        // skipped.
        if payment_method == PaymentMethod::Known(cashu::nuts::nut00::KnownMethod::Onchain) {
            validate_onchain_fee_options(&fee_options)?;
        }

        Ok(Self {
            id,
            unit: unit.clone(),
            request,
            amount: Amount::new(amount, unit.clone()),
            fee_reserve: Amount::new(fee_reserve, unit),
            state,
            expiry,
            payment_proof,
            request_lookup_id,
            options,
            created_time,
            paid_time,
            payment_method,
            extra_json,
            estimated_blocks,
            fee_options,
            selected_fee_index,
        })
    }
}

/// Validate the NUT `fee_options` rules for an onchain melt quote.
///
/// Per spec, for every onchain melt quote the mint MUST return at least one
/// `fee_options` item.
///
/// Returns:
/// - [`Error::OnchainFeeOptionsEmpty`]
///   when the slice is empty.
pub fn validate_onchain_fee_options(
    fee_options: &[MeltQuoteOnchainFeeOption],
) -> Result<(), crate::Error> {
    if fee_options.is_empty() {
        return Err(crate::Error::OnchainFeeOptionsEmpty);
    }

    Ok(())
}

impl From<MeltQuote> for MeltQuoteOnchainResponse<QuoteId> {
    fn from(quote: MeltQuote) -> Self {
        Self {
            quote: quote.id.clone(),
            amount: quote.amount().into(),
            unit: quote.unit.clone(),
            state: quote.state,
            expiry: quote.expiry,
            request: quote.request.to_string(),
            fee_options: quote.fee_options().to_vec(),
            selected_fee_index: quote.selected_fee_index,
            outpoint: quote.payment_proof.clone(),
            change: None,
        }
    }
}

impl TryFrom<MintQuote> for MintQuoteOnchainResponse<QuoteId> {
    type Error = crate::error::Error;
    fn try_from(quote: MintQuote) -> Result<Self, Self::Error> {
        Ok(Self {
            quote: quote.id.clone(),
            request: quote.request.clone(),
            unit: quote.unit.clone(),
            expiry: (quote.expiry != 0).then_some(quote.expiry),
            pubkey: quote.pubkey.ok_or(crate::error::Error::MissingPubkey)?,
            amount_paid: quote.amount_paid().into(),
            amount_issued: quote.amount_issued().into(),
        })
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
    /// Issuer Version
    pub issuer_version: Option<IssuerVersion>,
}

impl MintKeySetInfo {
    /// Returns true if `final_expiry` is set and strictly in the past.
    pub fn is_expired(&self) -> bool {
        self.final_expiry.is_some_and(|expiry| expiry < unix_time())
    }
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
    fn from(mint_quote: MintQuote) -> MintQuoteBolt11Response<QuoteId> {
        MintQuoteBolt11Response {
            quote: mint_quote.id.clone(),
            state: mint_quote.state(),
            request: mint_quote.request,
            expiry: Some(mint_quote.expiry),
            pubkey: mint_quote.pubkey,
            amount: mint_quote.amount.map(Into::into),
            unit: Some(mint_quote.unit),
        }
    }
}

impl From<MintQuote> for MintQuoteBolt11Response<String> {
    fn from(quote: MintQuote) -> Self {
        let quote: MintQuoteBolt11Response<QuoteId> = quote.into();
        quote.into()
    }
}

impl TryFrom<MintQuote> for MintQuoteBolt12Response<QuoteId> {
    type Error = Error;

    fn try_from(mint_quote: MintQuote) -> Result<Self, Self::Error> {
        Ok(MintQuoteBolt12Response {
            quote: mint_quote.id.clone(),
            request: mint_quote.request,
            expiry: Some(mint_quote.expiry),
            amount_paid: mint_quote.amount_paid.into(),
            amount_issued: mint_quote.amount_issued.into(),
            pubkey: mint_quote.pubkey.ok_or(Error::PubkeyRequired)?,
            amount: mint_quote.amount.map(Into::into),
            unit: mint_quote.unit,
        })
    }
}

impl TryFrom<MintQuote> for MintQuoteBolt12Response<String> {
    type Error = Error;

    fn try_from(quote: MintQuote) -> Result<Self, Self::Error> {
        let quote: MintQuoteBolt12Response<QuoteId> = quote.try_into()?;
        Ok(quote.into())
    }
}

impl TryFrom<MintQuote> for MintQuoteCustomResponse<QuoteId> {
    type Error = Error;

    fn try_from(quote: MintQuote) -> Result<Self, Self::Error> {
        let amount_paid = quote.amount_paid().into();
        let amount_issued = quote.amount_issued().into();

        Ok(MintQuoteCustomResponse {
            quote: quote.id,
            request: quote.request,
            unit: Some(quote.unit),
            expiry: Some(quote.expiry),
            pubkey: quote.pubkey,
            amount: quote.amount.map(Into::into),
            amount_paid,
            amount_issued,
            extra: quote.extra_json.unwrap_or_default(),
        })
    }
}

impl TryFrom<MintQuote> for MintQuoteCustomResponse<String> {
    type Error = Error;

    fn try_from(quote: MintQuote) -> Result<Self, Self::Error> {
        let quote: MintQuoteCustomResponse<QuoteId> = quote.try_into()?;
        Ok(quote.into())
    }
}

impl From<MeltQuote> for crate::nuts::MeltQuoteCustomResponse<QuoteId> {
    fn from(melt_quote: MeltQuote) -> Self {
        let request = match melt_quote.request {
            MeltPaymentRequest::Custom { request, .. } => Some(request),
            _ => None,
        };

        Self {
            quote: melt_quote.id,
            amount: melt_quote.amount.into(),
            fee_reserve: Some(melt_quote.fee_reserve.into()),
            state: melt_quote.state,
            expiry: melt_quote.expiry,
            payment_preimage: melt_quote.payment_proof,
            change: None,
            request,
            unit: Some(melt_quote.unit),
            extra: melt_quote.extra_json.unwrap_or_default(),
        }
    }
}
impl TryFrom<MintQuote> for MintQuoteResponse<QuoteId> {
    type Error = Error;

    fn try_from(quote: MintQuote) -> Result<Self, Self::Error> {
        if quote.payment_method.is_bolt11() {
            Ok(Self::Bolt11(crate::nuts::nut23::MintQuoteBolt11Response {
                quote: quote.id.clone(),
                request: quote.request.clone(),
                state: quote.state(),
                expiry: Some(quote.expiry),
                amount: quote.amount.as_ref().map(|a| a.clone().into()),
                unit: Some(quote.unit.clone()),
                pubkey: quote.pubkey,
            }))
        } else if quote.payment_method.is_bolt12() {
            Ok(Self::Bolt12(crate::nuts::nut25::MintQuoteBolt12Response {
                quote: quote.id.clone(),
                request: quote.request.clone(),
                amount: quote.amount.as_ref().map(|a| a.clone().into()),
                unit: quote.unit.clone(),
                expiry: Some(quote.expiry),
                pubkey: quote.pubkey.ok_or(Error::PubkeyRequired)?,
                amount_paid: quote.amount_paid().into(),
                amount_issued: quote.amount_issued().into(),
            }))
        } else if quote.payment_method.is_onchain() {
            let onchain_response = MintQuoteOnchainResponse::try_from(quote)?;
            Ok(MintQuoteResponse::Onchain(onchain_response))
        } else {
            let method = quote.payment_method.clone();
            Ok(MintQuoteResponse::Custom {
                method,
                response: crate::nuts::nut04::MintQuoteCustomResponse {
                    quote: quote.id.clone(),
                    request: quote.request.clone(),
                    expiry: Some(quote.expiry),
                    amount: quote.amount.as_ref().map(|a| a.clone().into()),
                    amount_paid: quote.amount_paid().into(),
                    amount_issued: quote.amount_issued().into(),
                    unit: Some(quote.unit.clone()),
                    pubkey: quote.pubkey,
                    extra: serde_json::Value::Null,
                },
            })
        }
    }
}

impl From<MintQuoteResponse<QuoteId>> for MintQuoteResponse<String> {
    fn from(response: MintQuoteResponse<QuoteId>) -> Self {
        match response {
            MintQuoteResponse::Bolt11(response) => MintQuoteResponse::Bolt11(response.into()),
            MintQuoteResponse::Bolt12(response) => MintQuoteResponse::Bolt12(response.into()),
            MintQuoteResponse::Onchain(response) => MintQuoteResponse::Onchain(response.into()),
            MintQuoteResponse::Custom { method, response } => MintQuoteResponse::Custom {
                method,
                response: response.into(),
            },
        }
    }
}

impl From<MintQuoteResponse<QuoteId>> for MintQuoteBolt11Response<String> {
    fn from(response: MintQuoteResponse<QuoteId>) -> Self {
        match response {
            MintQuoteResponse::Bolt11(bolt11_response) => MintQuoteBolt11Response {
                quote: bolt11_response.quote.to_string(),
                state: bolt11_response.state,
                request: bolt11_response.request,
                expiry: bolt11_response.expiry,
                pubkey: bolt11_response.pubkey,
                amount: bolt11_response.amount,
                unit: bolt11_response.unit,
            },
            _ => panic!("Expected Bolt11 response"),
        }
    }
}

impl TryFrom<MintQuoteResponse<QuoteId>> for MintQuoteBolt11Response<QuoteId> {
    type Error = Error;

    fn try_from(response: MintQuoteResponse<QuoteId>) -> Result<Self, Self::Error> {
        match response {
            MintQuoteResponse::Bolt11(r) => Ok(r),
            _ => Err(Error::InvalidPaymentMethod),
        }
    }
}

impl TryFrom<MintQuoteResponse<QuoteId>> for MintQuoteBolt12Response<QuoteId> {
    type Error = Error;

    fn try_from(response: MintQuoteResponse<QuoteId>) -> Result<Self, Self::Error> {
        match response {
            MintQuoteResponse::Bolt12(r) => Ok(r),
            _ => Err(Error::InvalidPaymentMethod),
        }
    }
}

impl TryFrom<MintQuoteResponse<QuoteId>> for MintQuoteOnchainResponse<QuoteId> {
    type Error = Error;

    fn try_from(response: MintQuoteResponse<QuoteId>) -> Result<Self, Self::Error> {
        match response {
            MintQuoteResponse::Onchain(r) => Ok(r),
            _ => Err(Error::InvalidPaymentMethod),
        }
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
            amount: melt_quote.amount().into(),
            fee_reserve: melt_quote.fee_reserve().into(),
            request: None,
            unit: Some(melt_quote.unit.clone()),
        }
    }
}

impl From<MeltQuote> for MeltQuoteBolt11Response<QuoteId> {
    fn from(melt_quote: MeltQuote) -> MeltQuoteBolt11Response<QuoteId> {
        MeltQuoteBolt11Response {
            quote: melt_quote.id.clone(),
            amount: melt_quote.amount().into(),
            fee_reserve: melt_quote.fee_reserve().into(),
            state: melt_quote.state,
            expiry: melt_quote.expiry,
            payment_preimage: melt_quote.payment_proof,
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
    /// Custom payment method
    Custom {
        /// Payment method name
        method: String,
        /// Payment request string
        request: String,
    },
    /// Onchain Payment
    Onchain {
        /// Onchain address
        address: String,
    },
}

impl std::fmt::Display for MeltPaymentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeltPaymentRequest::Bolt11 { bolt11 } => write!(f, "{bolt11}"),
            MeltPaymentRequest::Bolt12 { offer } => write!(f, "{offer}"),
            MeltPaymentRequest::Custom { request, .. } => write!(f, "{request}"),
            MeltPaymentRequest::Onchain { address } => write!(f, "{address}"),
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cashu::Bolt11Invoice;

    use super::*;

    #[test]
    fn test_operation_new_mint_uses_uuid_v7() {
        let operation = Operation::new_mint(Amount::from(100), PaymentMethod::BOLT11);

        assert_eq!(operation.id.get_version(), Some(uuid::Version::SortRand));
    }

    #[test]
    fn test_melt_quote_to_custom_response_with_custom_request() {
        let melt_quote = MeltQuote::new(
            Some(QuoteId::new()),
            MeltPaymentRequest::Custom {
                method: "custom".to_string(),
                request: "custom_request_string".to_string(),
            },
            CurrencyUnit::Sat,
            Amount::new(100, CurrencyUnit::Sat),
            Amount::new(2, CurrencyUnit::Sat),
            unix_time() + 3600,
            None,
            None,
            PaymentMethod::Custom("custom".to_string()),
            Some(serde_json::json!({"extra_field": "value"})),
            None,
        );

        let response: crate::nuts::MeltQuoteCustomResponse<QuoteId> = melt_quote.clone().into();

        assert_eq!(response.quote, melt_quote.id);
        assert_eq!(response.amount, 100.into());
        assert_eq!(response.fee_reserve, Some(2.into()));
        assert_eq!(response.state, melt_quote.state);
        assert_eq!(response.expiry, melt_quote.expiry);
        assert_eq!(response.payment_preimage, melt_quote.payment_proof);
        assert_eq!(response.change, None);
        assert_eq!(response.request, Some("custom_request_string".to_string()));
        assert_eq!(response.unit, Some(CurrencyUnit::Sat));
        assert_eq!(response.extra, serde_json::json!({"extra_field": "value"}));
    }

    #[test]
    fn test_melt_quote_to_custom_response_with_bolt11_request() {
        let bolt11_str = "lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq";
        let bolt11 = Bolt11Invoice::from_str(bolt11_str).unwrap();

        let melt_quote = MeltQuote::new(
            Some(QuoteId::new()),
            MeltPaymentRequest::Bolt11 { bolt11 },
            CurrencyUnit::Sat,
            Amount::new(100, CurrencyUnit::Sat),
            Amount::new(2, CurrencyUnit::Sat),
            unix_time() + 3600,
            None,
            None,
            PaymentMethod::BOLT11,
            None,
            None,
        );

        let response: crate::nuts::MeltQuoteCustomResponse<QuoteId> = melt_quote.clone().into();

        assert_eq!(response.quote, melt_quote.id);
        assert_eq!(response.request, None);
    }

    #[test]
    fn test_melt_quote_to_custom_response_with_bolt12_request() {
        use bitcoin::secp256k1::{PublicKey as Secp256k1PublicKey, Secp256k1, SecretKey};
        use lightning::offers::offer::OfferBuilder;
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0xcd; 32]).unwrap();
        let pubkey = Secp256k1PublicKey::from_secret_key(&secp, &secret_key);
        let offer = OfferBuilder::new(pubkey).build().unwrap();

        let melt_quote = MeltQuote::new(
            Some(QuoteId::new()),
            MeltPaymentRequest::Bolt12 {
                offer: Box::new(offer),
            },
            CurrencyUnit::Sat,
            Amount::new(100, CurrencyUnit::Sat),
            Amount::new(2, CurrencyUnit::Sat),
            unix_time() + 3600,
            None,
            None,
            PaymentMethod::BOLT12,
            None,
            None,
        );

        let response: crate::nuts::MeltQuoteCustomResponse<QuoteId> = melt_quote.clone().into();

        assert_eq!(response.quote, melt_quote.id);
        assert_eq!(response.request, None);
    }

    fn dummy_mint_keyset_info(final_expiry: Option<u64>) -> MintKeySetInfo {
        use std::str::FromStr;
        MintKeySetInfo {
            id: Id::from_str("009a1f293253e41e").unwrap(),
            unit: CurrencyUnit::Sat,
            active: true,
            valid_from: 0,
            derivation_path: "m/0'/0'/0'".parse().unwrap(),
            derivation_path_index: Some(0),
            amounts: vec![1, 2, 4, 8, 16, 32, 64, 128, 256, 512],
            input_fee_ppk: 0,
            final_expiry,
            issuer_version: None,
        }
    }

    #[test]
    fn test_is_expired_none() {
        let info = dummy_mint_keyset_info(None);
        assert!(!info.is_expired());
    }

    #[test]
    fn test_is_expired_far_future() {
        let info = dummy_mint_keyset_info(Some(unix_time() + 1_000_000));
        assert!(!info.is_expired());
    }

    #[test]
    fn test_is_expired_exactly_now_is_not_expired() {
        // strict less-than: expiry == now is not yet expired
        let info = dummy_mint_keyset_info(Some(unix_time()));
        assert!(!info.is_expired());
    }

    #[test]
    fn test_is_expired_one_second_ago() {
        let info = dummy_mint_keyset_info(Some(unix_time() - 1));
        assert!(info.is_expired());
    }

    #[test]
    fn test_is_expired_zero() {
        let info = dummy_mint_keyset_info(Some(0));
        assert!(info.is_expired());
    }

    #[test]
    fn test_melt_quote_into_response_onchain() {
        let address = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
        let mut melt_quote = MeltQuote::new(
            Some(QuoteId::new()),
            MeltPaymentRequest::Onchain {
                address: address.to_string(),
            },
            CurrencyUnit::Sat,
            Amount::new(5_000, CurrencyUnit::Sat),
            Amount::new(250, CurrencyUnit::Sat),
            unix_time() + 3600,
            None,
            None,
            PaymentMethod::Known(cashu::nuts::nut00::KnownMethod::Onchain),
            None,
            Some(6),
        );

        // Simulate the terminal paid path: payment_proof becomes the broadcast outpoint.
        melt_quote.payment_proof =
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:1".to_string());
        melt_quote.state = MeltQuoteState::Paid;

        let expected_id = melt_quote.id.clone();
        let expected_amount: Amount = melt_quote.amount().into();
        let expected_fee_options = melt_quote.fee_options().to_vec();
        let expected_expiry = melt_quote.expiry;
        let expected_state = melt_quote.state;
        let expected_outpoint = melt_quote.payment_proof.clone();

        let response = melt_quote.into_response(None);
        match response {
            crate::MeltQuoteResponse::Onchain(r) => {
                assert_eq!(r.quote, expected_id);
                assert_eq!(r.request, address);
                assert_eq!(r.amount, expected_amount);
                assert_eq!(r.unit, CurrencyUnit::Sat);
                assert_eq!(r.fee_options, expected_fee_options);
                assert_eq!(r.selected_fee_index, None);
                assert_eq!(r.state, expected_state);
                assert_eq!(r.expiry, expected_expiry);
                assert_eq!(r.outpoint, expected_outpoint);
                assert_eq!(r.change, None);
            }
            _ => panic!("expected MeltQuoteResponse::Onchain variant"),
        }
    }

    #[test]
    fn test_mint_quote_onchain_response_converts_zero_expiry_to_none() {
        let pubkey = PublicKey::from_hex(
            "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
        )
        .unwrap();
        let quote_id = QuoteId::new();
        let mint_quote = MintQuote::new(
            Some(quote_id.clone()),
            "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
            CurrencyUnit::Sat,
            None,
            0,
            PaymentIdentifier::QuoteId(quote_id.clone()),
            Some(pubkey),
            Amount::new(10_000, CurrencyUnit::Sat),
            Amount::new(1_000, CurrencyUnit::Sat),
            PaymentMethod::Known(cashu::nuts::nut00::KnownMethod::Onchain),
            unix_time(),
            vec![],
            vec![],
            None,
        );

        let response = MintQuoteOnchainResponse::try_from(mint_quote).unwrap();

        assert_eq!(response.quote, quote_id);
        assert_eq!(response.expiry, None);
        assert_eq!(response.pubkey, pubkey);
        assert_eq!(response.amount_paid, Amount::from(10_000));
        assert_eq!(response.amount_issued, Amount::from(1_000));
    }

    #[test]
    fn test_melt_quote_into_response_onchain_includes_change() {
        let address = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
        let melt_quote = MeltQuote::new(
            Some(QuoteId::new()),
            MeltPaymentRequest::Onchain {
                address: address.to_string(),
            },
            CurrencyUnit::Sat,
            Amount::new(1_000, CurrencyUnit::Sat),
            Amount::new(10, CurrencyUnit::Sat),
            unix_time() + 3600,
            None,
            None,
            PaymentMethod::Known(cashu::nuts::nut00::KnownMethod::Onchain),
            None,
            Some(3),
        );

        let response = melt_quote.into_response(Some(vec![]));
        match response {
            crate::MeltQuoteResponse::Onchain(r) => assert_eq!(r.change, Some(vec![])),
            _ => panic!("expected MeltQuoteResponse::Onchain variant"),
        }
    }

    #[test]
    fn validate_onchain_fee_options_rejects_empty() {
        let err = validate_onchain_fee_options(&[]).expect_err("empty must be rejected");
        assert!(matches!(err, crate::Error::OnchainFeeOptionsEmpty));
    }

    #[test]
    fn validate_onchain_fee_options_allows_duplicate_fee_index() {
        let options = [
            MeltQuoteOnchainFeeOption {
                fee_index: 10,
                fee_reserve: Amount::from(10),
                estimated_blocks: 3,
            },
            MeltQuoteOnchainFeeOption {
                fee_index: 10,
                fee_reserve: Amount::from(20),
                estimated_blocks: 6,
            },
        ];
        validate_onchain_fee_options(&options).expect("duplicate fee_index must be allowed");
    }

    #[test]
    fn validate_onchain_fee_options_allows_duplicate_estimated_blocks() {
        // With selection by fee_index, duplicate estimated_blocks values are
        // permitted (although unusual).
        let options = [
            MeltQuoteOnchainFeeOption {
                fee_index: 20,
                fee_reserve: Amount::from(10),
                estimated_blocks: 3,
            },
            MeltQuoteOnchainFeeOption {
                fee_index: 1,
                fee_reserve: Amount::from(20),
                estimated_blocks: 3,
            },
        ];
        validate_onchain_fee_options(&options).expect("duplicate blocks must be allowed");
    }

    #[test]
    fn validate_onchain_fee_options_allows_duplicate_fee_reserve() {
        // With selection by fee_index, duplicate fee_reserve values are
        // permitted (although unusual).
        let options = [
            MeltQuoteOnchainFeeOption {
                fee_index: 0,
                fee_reserve: Amount::from(42),
                estimated_blocks: 1,
            },
            MeltQuoteOnchainFeeOption {
                fee_index: 1,
                fee_reserve: Amount::from(42),
                estimated_blocks: 6,
            },
        ];
        validate_onchain_fee_options(&options).expect("duplicate fee must be allowed");
    }

    #[test]
    fn validate_onchain_fee_options_accepts_well_formed() {
        let options = [
            MeltQuoteOnchainFeeOption {
                fee_index: 0,
                fee_reserve: Amount::from(500),
                estimated_blocks: 1,
            },
            MeltQuoteOnchainFeeOption {
                fee_index: 1,
                fee_reserve: Amount::from(200),
                estimated_blocks: 6,
            },
            MeltQuoteOnchainFeeOption {
                fee_index: 2,
                fee_reserve: Amount::from(50),
                estimated_blocks: 144,
            },
        ];
        validate_onchain_fee_options(&options).expect("well-formed must validate");
    }

    #[test]
    fn new_onchain_rejects_empty_fee_options() {
        let err = MeltQuote::new_onchain(
            None,
            MeltPaymentRequest::Onchain {
                address: "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            },
            CurrencyUnit::Sat,
            Amount::new(1_000, CurrencyUnit::Sat),
            unix_time() + 3600,
            None,
            None,
            vec![],
        )
        .expect_err("empty fee_options must be rejected");
        assert!(matches!(err, crate::Error::OnchainFeeOptionsEmpty));
    }

    #[test]
    fn new_onchain_initializes_reserve_to_cheapest_tier() {
        // Submit options in an unsorted order to ensure cheapest-by-fee_reserve
        // is what wins (not first-in-list).
        let options = vec![
            MeltQuoteOnchainFeeOption {
                fee_index: 10,
                fee_reserve: Amount::from(500),
                estimated_blocks: 1,
            },
            MeltQuoteOnchainFeeOption {
                fee_index: 30,
                fee_reserve: Amount::from(50),
                estimated_blocks: 144,
            },
            MeltQuoteOnchainFeeOption {
                fee_index: 20,
                fee_reserve: Amount::from(200),
                estimated_blocks: 6,
            },
        ];
        let quote = MeltQuote::new_onchain(
            None,
            MeltPaymentRequest::Onchain {
                address: "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            },
            CurrencyUnit::Sat,
            Amount::new(10_000, CurrencyUnit::Sat),
            unix_time() + 3600,
            None,
            None,
            options.clone(),
        )
        .expect("well-formed quote must construct");

        assert_eq!(quote.fee_reserve().value(), 50);
        assert_eq!(quote.estimated_blocks, Some(144));
        assert_eq!(quote.selected_fee_index, None);
        let returned: Vec<u32> = quote.fee_options().iter().map(|o| o.fee_index).collect();
        assert_eq!(returned, vec![10, 30, 20]);
    }

    #[test]
    fn new_onchain_preserves_duplicate_backend_fee_index() {
        let options = vec![
            MeltQuoteOnchainFeeOption {
                fee_index: 7,
                fee_reserve: Amount::from(500),
                estimated_blocks: 1,
            },
            MeltQuoteOnchainFeeOption {
                fee_index: 7,
                fee_reserve: Amount::from(200),
                estimated_blocks: 6,
            },
        ];
        let quote = MeltQuote::new_onchain(
            None,
            MeltPaymentRequest::Onchain {
                address: "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            },
            CurrencyUnit::Sat,
            Amount::new(10_000, CurrencyUnit::Sat),
            unix_time() + 3600,
            None,
            None,
            options,
        )
        .expect("duplicate backend fee_index must be preserved");

        let returned: Vec<u32> = quote.fee_options().iter().map(|o| o.fee_index).collect();
        assert_eq!(returned, vec![7, 7]);
    }

    #[test]
    fn select_onchain_fee_option_leaves_fee_options_untouched() {
        let options = vec![
            MeltQuoteOnchainFeeOption {
                fee_index: 1,
                fee_reserve: Amount::from(500),
                estimated_blocks: 1,
            },
            MeltQuoteOnchainFeeOption {
                fee_index: 2,
                fee_reserve: Amount::from(200),
                estimated_blocks: 6,
            },
        ];
        let mut quote = MeltQuote::new_onchain(
            None,
            MeltPaymentRequest::Onchain {
                address: "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            },
            CurrencyUnit::Sat,
            Amount::new(10_000, CurrencyUnit::Sat),
            unix_time() + 3600,
            None,
            None,
            options.clone(),
        )
        .unwrap();

        let before = quote.fee_options().to_vec();
        quote
            .select_onchain_fee_option(1)
            .expect("selecting a known fee_index must succeed");

        assert_eq!(
            quote.fee_options(),
            before.as_slice(),
            "fee_options is fixed for the lifetime of the quote and must not \
             mutate on selection"
        );
        assert_eq!(quote.selected_fee_index, Some(1));
        assert_eq!(quote.estimated_blocks, Some(1));
        assert_eq!(quote.fee_reserve().value(), 500);
    }

    #[test]
    fn select_onchain_fee_option_unknown_index_rejected() {
        let options = vec![MeltQuoteOnchainFeeOption {
            fee_index: 0,
            fee_reserve: Amount::from(500),
            estimated_blocks: 1,
        }];
        let mut quote = MeltQuote::new_onchain(
            None,
            MeltPaymentRequest::Onchain {
                address: "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            },
            CurrencyUnit::Sat,
            Amount::new(10_000, CurrencyUnit::Sat),
            unix_time() + 3600,
            None,
            None,
            options,
        )
        .unwrap();

        match quote
            .select_onchain_fee_option(7)
            .expect_err("unknown fee_index must be rejected")
        {
            crate::Error::OnchainFeeIndexNotFound { index: 7 } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn from_db_preserves_duplicate_onchain_fee_options() {
        let options = vec![
            MeltQuoteOnchainFeeOption {
                fee_index: 0,
                fee_reserve: Amount::from(100),
                estimated_blocks: 6,
            },
            MeltQuoteOnchainFeeOption {
                fee_index: 0,
                fee_reserve: Amount::from(200),
                estimated_blocks: 6,
            },
        ];
        let quote = MeltQuote::from_db(
            QuoteId::new(),
            CurrencyUnit::Sat,
            MeltPaymentRequest::Onchain {
                address: "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            },
            10_000,
            100,
            MeltQuoteState::Unpaid,
            unix_time() + 3600,
            None,
            None,
            None,
            unix_time(),
            None,
            PaymentMethod::Known(cashu::nuts::nut00::KnownMethod::Onchain),
            None,
            None,
            options,
            None,
        )
        .expect("duplicate onchain fee_options on reload must be preserved");

        let returned: Vec<u32> = quote.fee_options().iter().map(|o| o.fee_index).collect();
        assert_eq!(returned, vec![0, 0]);
    }

    #[test]
    fn from_db_rejects_empty_onchain_fee_options() {
        let err = MeltQuote::from_db(
            QuoteId::new(),
            CurrencyUnit::Sat,
            MeltPaymentRequest::Onchain {
                address: "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            },
            10_000,
            100,
            MeltQuoteState::Unpaid,
            unix_time() + 3600,
            None,
            None,
            None,
            unix_time(),
            None,
            PaymentMethod::Known(cashu::nuts::nut00::KnownMethod::Onchain),
            None,
            Some(6),
            Vec::new(),
            None,
        )
        .expect_err("empty onchain fee_options on reload must be rejected");
        assert!(matches!(err, crate::Error::OnchainFeeOptionsEmpty));
    }
}
