//! Canonical federation operation envelopes.
//!
//! Federation consensus orders these envelopes, not ad hoc HTTP handler
//! side effects. Keeping the operation payloads separate from the Cashu REST
//! API lets standalone mints keep their existing protocol surface while
//! federation members agree on the exact mint, swap, and melt
//! payment-observation requests before signing or mutating spend state.

use core::fmt;
use core::str::FromStr;
use std::collections::BTreeSet;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::journal::FederationJournalEntry;
use super::{
    FederatedKeyset, FederationAddressAllocationOperation, FederationChainObservation,
    FederationConfig, FederationError, FederationId, FederationPaymentObservationContext,
    FederationSignedTransaction, FederationTransactionBroadcastIntent,
    FederationTransactionBroadcastObservation, FederationTransactionProposal, MemberConfigPrivate,
    MemberId, Result,
};
use crate::mint::{MeltPaymentRequest, MeltQuote};
use crate::payment::PaymentIdentifier;
use crate::{
    Amount, BatchMintRequest, BlindedMessage, Id, MeltQuoteCreateResponse, MeltQuoteRequest,
    MeltQuoteResponse, MeltQuoteState, MeltRequest, MintQuoteRequest, MintQuoteResponse,
    MintQuoteState, MintRequest, PaymentMethod, QuoteId, SwapRequest,
};

const FEDERATION_OPERATION_HASH_VERSION: &[u8] = b"cdk-federation-operation-v1";
const FEDERATION_OPERATION_AUTHORIZATION_VERSION: &[u8] =
    b"cdk-federation-operation-authorization-v1";

/// Version of the federation operation envelope format.
pub const FEDERATION_OPERATION_VERSION: u16 = 4;

/// Maximum canonical operation envelope size admitted into federation consensus.
///
/// This is intentionally enforced on the canonical bytes used for operation
/// IDs so every member evaluates the same payload size independent of local
/// JSON formatting.
pub const FEDERATION_MAX_OPERATION_ENVELOPE_BYTES: usize = 1024 * 1024;

/// Stable identifier for a canonical federation operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationOperationId([u8; 32]);

impl FederationOperationId {
    /// Create an operation ID from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse an operation ID from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidOperationIdSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse an operation ID from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this operation ID as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this operation ID as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationOperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationOperationId {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationOperationId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationOperationId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// High-level kind of a federation operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationOperationKind {
    /// Allocate one descriptor address in accepted consensus order.
    AddressAllocation,
    /// Submit bounded member-local Bitcoin chain evidence.
    ChainObservation,
    /// Create and persist a mint quote.
    MintQuote,
    /// Observe a mint quote payment state.
    MintQuotePayment,
    /// Spend a paid mint quote and sign blinded outputs.
    Mint,
    /// Create and persist a melt quote.
    MeltQuote,
    /// Observe an outgoing melt payment state.
    MeltQuotePayment,
    /// Spend proofs against a melt quote and optionally sign change outputs.
    Melt,
    /// Spend proofs and sign replacement outputs.
    Swap,
    /// Propose a new federated v3/BLS aggregate keyset.
    KeysetRotation,
    /// Propose and reserve one independently validated on-chain transaction.
    TransactionProposal,
    /// Persist one exact threshold-signed on-chain transaction.
    TransactionSigned,
    /// Persist authorization to broadcast one exact signed transaction.
    TransactionBroadcastIntent,
    /// Persist one authenticated member broadcast result.
    TransactionBroadcastObservation,
}

impl fmt::Display for FederationOperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddressAllocation => f.write_str("address_allocation"),
            Self::ChainObservation => f.write_str("chain_observation"),
            Self::MintQuote => f.write_str("mint_quote"),
            Self::MintQuotePayment => f.write_str("mint_quote_payment"),
            Self::Mint => f.write_str("mint"),
            Self::MeltQuote => f.write_str("melt_quote"),
            Self::MeltQuotePayment => f.write_str("melt_quote_payment"),
            Self::Melt => f.write_str("melt"),
            Self::Swap => f.write_str("swap"),
            Self::KeysetRotation => f.write_str("keyset_rotation"),
            Self::TransactionProposal => f.write_str("transaction_proposal"),
            Self::TransactionSigned => f.write_str("transaction_signed"),
            Self::TransactionBroadcastIntent => f.write_str("transaction_broadcast_intent"),
            Self::TransactionBroadcastObservation => {
                f.write_str("transaction_broadcast_observation")
            }
        }
    }
}

/// Semantic key used to reject mix-and-match duplicates before signing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum FederationOperationConflictKey {
    /// One application-epoch quote key may only map to one address.
    AddressAllocation {
        /// Registered CDK on-chain application.
        application_id: super::FederationFrostApplicationId,
        /// Exact descriptor application epoch.
        application_key_epoch: u64,
        /// Quote/allocation key.
        quote: String,
    },
    /// One member may submit one canonical view per outpoint and observed depth.
    DepositObservation {
        /// Observed transaction output.
        outpoint: bitcoin::OutPoint,
        /// Zero before confirmation, otherwise the observer's claimed depth.
        confirmation_depth: u32,
        /// Reporting federation member.
        observer: MemberId,
    },
    /// One member may report one conservative reorg event per outpoint.
    DepositReorgObservation {
        /// Disconnected transaction output.
        outpoint: bitcoin::OutPoint,
        /// Reporting federation member.
        observer: MemberId,
    },
    /// One mint quote ID may only map to one quote request.
    MintQuote {
        /// Quote ID assigned by the federation.
        quote: String,
    },
    /// One mint quote ID may only sign one exact output set.
    Mint {
        /// Quote ID being spent.
        quote: String,
    },
    /// A mint quote may only have one observation for the same state per observer.
    MintQuotePayment {
        /// Quote ID being observed.
        quote: String,
        /// Observed mint quote state.
        state: MintQuoteState,
        /// Member that observed the payment state.
        observer: MemberId,
    },
    /// A cumulative on-chain payment revision is unique by its output commitment.
    MintQuotePaymentRevision {
        /// Quote ID being observed.
        quote: String,
        /// Member that observed the cumulative payment.
        observer: MemberId,
        /// Commitment to the exact accepted output set.
        payment_id: [u8; 32],
    },
    /// One melt quote ID may only map to one quote request.
    MeltQuote {
        /// Quote ID assigned by the federation.
        quote: String,
    },
    /// One melt quote ID may only execute one exact melt request.
    Melt {
        /// Quote ID being spent.
        quote: String,
    },
    /// A melt quote may only have one observation for the same state per observer.
    MeltQuotePayment {
        /// Quote ID being observed.
        quote: String,
        /// Observed melt quote state.
        state: MeltQuoteState,
        /// Member that observed the payment state.
        observer: MemberId,
    },
    /// One aggregate keyset ID may only map to one exact rotation payload.
    KeysetRotation {
        /// Proposed aggregate keyset ID.
        keyset_id: Id,
    },
    /// One accepted on-chain payment intent may be assigned only once.
    OnchainPaymentIntent {
        /// Melt quote authorizing the intent.
        quote: String,
    },
    /// One canonical intent may be assigned to only one accepted transaction.
    OnchainPaymentIntentAssignment {
        /// Canonical intent identifier.
        intent_id: super::FederationOnchainPaymentIntentId,
    },
    /// One deterministic payment batch may accept only one transaction lineage.
    OnchainBatch {
        /// Canonical batch identifier.
        batch_id: super::FederationOnchainBatchId,
    },
    /// One wallet UTXO may be reserved by only one accepted transaction.
    OnchainUtxo {
        /// Exact wallet outpoint.
        outpoint: bitcoin::OutPoint,
    },
    /// Original and replacement proposals share one serialized lineage.
    OnchainReplacementLineage {
        /// Batch-bound replacement lineage.
        lineage: super::FederationOnchainBatchId,
    },
}

/// Proposed mint quote creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationMintQuoteOperation {
    /// Quote ID assigned by the proposing member.
    pub quote: String,
    /// Stable backend identifier used by every member to observe payment.
    pub request_lookup_id: PaymentIdentifier,
    /// Wallet's quote request.
    pub request: MintQuoteRequest,
    /// Quote response produced by the proposing member, including the payment request.
    pub response: MintQuoteResponse<String>,
}

/// Observed incoming mint quote payment state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationMintQuotePaymentObservation {
    /// Quote ID being observed.
    pub quote: String,
    /// Federation member that observed this payment state.
    pub observer: MemberId,
    /// Payment method for the quote.
    pub method: PaymentMethod,
    /// Observed quote state.
    pub state: MintQuoteState,
    /// Total amount observed as paid.
    pub amount_paid: Amount,
    /// Amount already issued for the quote.
    pub amount_issued: Amount,
    /// Optional stable backend payment identifier observed by this member.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_id: Option<PaymentIdentifier>,
    /// Optional method-specific proof or settlement reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_proof: Option<String>,
}

impl FederationMintQuotePaymentObservation {
    /// Return true when two observations describe the same payment evidence.
    #[must_use]
    pub fn matches_payment_evidence(&self, other: &Self) -> bool {
        self.quote == other.quote
            && self.method == other.method
            && self.state == other.state
            && self.amount_paid == other.amount_paid
            && self.amount_issued == other.amount_issued
            && self.payment_id == other.payment_id
            && self.payment_proof == other.payment_proof
    }

    /// Count matching and conflicting observations from distinct members.
    #[must_use]
    pub fn quorum<'a, I>(
        &self,
        observations: I,
        required: u16,
    ) -> FederationPaymentObservationQuorum
    where
        I: IntoIterator<Item = &'a Self>,
    {
        let mut matching = BTreeSet::from([self.observer]);
        let mut conflicting = BTreeSet::new();

        for observation in observations {
            if observation.quote != self.quote || observation.state != self.state {
                continue;
            }

            if self.matches_payment_evidence(observation) {
                matching.insert(observation.observer);
            } else {
                conflicting.insert(observation.observer);
            }
        }

        FederationPaymentObservationQuorum::new(
            FederationPaymentObservationContext::MintQuote,
            required,
            matching,
            conflicting,
        )
    }
}

/// Request to sign blinded outputs for a paid mint quote.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationMintOperation {
    /// Original wallet mint request.
    pub request: FederationMintRequest,
}

/// Wallet mint request variants represented in federation consensus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FederationMintRequest {
    /// Legacy NUT-04 single-quote mint request.
    Single(MintRequest<String>),
    /// NUT-29 batch mint request where multiple quotes share one output set.
    Batch(BatchMintRequest<String>),
}

impl FederationMintRequest {
    /// Return every quote ID touched by this mint request.
    #[must_use]
    pub fn quote_ids(&self) -> Vec<&str> {
        match self {
            Self::Single(request) => vec![request.quote.as_str()],
            Self::Batch(request) => request.quotes.iter().map(String::as_str).collect(),
        }
    }
}

/// Proposed melt quote creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationMeltQuoteOperation {
    /// Quote ID assigned by the proposing member.
    pub quote: String,
    /// Wallet's quote request.
    pub request: MeltQuoteRequest,
    /// Canonical public quote response accepted by the federation.
    pub response: MeltQuoteCreateResponse<String>,
    /// Internal backend metadata omitted from the public response.
    pub metadata: FederationMeltQuoteMetadata,
}

/// Backend metadata required to execute an accepted federated melt quote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationMeltQuoteMetadata {
    /// Stable payment-processor lookup identifier.
    pub request_lookup_id: Option<PaymentIdentifier>,
    /// Payment-method-specific response fields.
    pub extra_json: Option<serde_json::Value>,
    /// Backend-estimated confirmation target.
    pub estimated_blocks: Option<u32>,
}

impl FederationMeltQuoteMetadata {
    /// Capture internal metadata from a prepared quote.
    #[must_use]
    pub fn from_melt_quote(quote: &MeltQuote) -> Self {
        Self {
            request_lookup_id: quote.request_lookup_id.clone(),
            extra_json: quote.extra_json.clone(),
            estimated_blocks: quote.estimated_blocks,
        }
    }
}

impl FederationMeltQuoteOperation {
    /// Validate and materialize the accepted quote using deterministic time.
    pub fn into_melt_quote(
        &self,
        quote_id: QuoteId,
        created_time: u64,
    ) -> std::result::Result<MeltQuote, crate::Error> {
        let response: MeltQuoteResponse<String> = self.response.clone().into();
        let (expected_request, unit) = match &self.request {
            MeltQuoteRequest::Bolt11(request) => {
                (request.request.to_string(), request.unit.clone())
            }
            MeltQuoteRequest::Bolt12(request) => (request.request.clone(), request.unit.clone()),
            MeltQuoteRequest::Onchain(request) => (request.request.clone(), request.unit.clone()),
            MeltQuoteRequest::Custom(request) => (request.request.clone(), request.unit.clone()),
        };
        if self.quote != quote_id.to_string()
            || response.quote() != &self.quote
            || response.method() != self.request.method()
            || response.state() != MeltQuoteState::Unpaid
            || response.payment_proof().is_some()
            || response.change().is_some()
            || response.request() != Some(expected_request.as_str())
            || response.unit() != Some(unit.clone())
        {
            return Err(crate::Error::InvalidPaymentRequest);
        }

        let (payment_request, fee_reserve, options, fee_options) =
            match (&self.request, &self.response) {
                (MeltQuoteRequest::Bolt11(request), MeltQuoteCreateResponse::Bolt11(_)) => (
                    MeltPaymentRequest::Bolt11 {
                        bolt11: request.request.clone(),
                    },
                    response.fee_reserve(),
                    request.options,
                    Vec::new(),
                ),
                (MeltQuoteRequest::Bolt12(request), MeltQuoteCreateResponse::Bolt12(_)) => {
                    let offer = request
                        .request
                        .parse()
                        .map_err(|_| crate::Error::Bolt12parse)?;
                    (
                        MeltPaymentRequest::Bolt12 {
                            offer: Box::new(offer),
                        },
                        response.fee_reserve(),
                        request.options,
                        Vec::new(),
                    )
                }
                (MeltQuoteRequest::Onchain(request), MeltQuoteCreateResponse::Onchain(onchain)) => {
                    if onchain.selected_fee_index.is_some()
                        || onchain.amount != request.amount
                        || self.metadata.request_lookup_id
                            != Some(PaymentIdentifier::QuoteId(quote_id.clone()))
                    {
                        return Err(crate::Error::InvalidPaymentRequest);
                    }
                    let initial = onchain
                        .fee_options
                        .iter()
                        .min_by_key(|option| u64::from(option.fee_reserve))
                        .ok_or(crate::Error::OnchainFeeOptionsEmpty)?;
                    if self.metadata.estimated_blocks != Some(initial.estimated_blocks) {
                        return Err(crate::Error::InvalidPaymentRequest);
                    }
                    (
                        MeltPaymentRequest::Onchain {
                            address: request.request.clone(),
                        },
                        initial.fee_reserve,
                        None,
                        onchain.fee_options.clone(),
                    )
                }
                (
                    MeltQuoteRequest::Custom(request),
                    MeltQuoteCreateResponse::Custom((_, custom)),
                ) => {
                    if custom.extra
                        != self
                            .metadata
                            .extra_json
                            .clone()
                            .unwrap_or(serde_json::Value::Null)
                    {
                        return Err(crate::Error::InvalidPaymentRequest);
                    }
                    (
                        MeltPaymentRequest::Custom {
                            method: request.method.clone(),
                            request: request.request.clone(),
                        },
                        response.fee_reserve(),
                        None,
                        Vec::new(),
                    )
                }
                _ => return Err(crate::Error::InvalidPaymentMethod),
            };

        MeltQuote::from_db(
            quote_id,
            unit,
            payment_request,
            response.amount().into(),
            fee_reserve.into(),
            MeltQuoteState::Unpaid,
            response.expiry(),
            None,
            self.metadata.request_lookup_id.clone(),
            options,
            created_time,
            None,
            self.request.method(),
            self.metadata.extra_json.clone(),
            self.metadata.estimated_blocks,
            fee_options,
            None,
        )
    }
}

#[cfg(test)]
pub(crate) fn test_melt_quote_operation(
    quote: &str,
    request: MeltQuoteRequest,
) -> FederationMeltQuoteOperation {
    let response = match &request {
        MeltQuoteRequest::Bolt11(request) => {
            MeltQuoteCreateResponse::Bolt11(crate::MeltQuoteBolt11Response {
                quote: quote.to_string(),
                amount: Amount::from(
                    request
                        .request
                        .amount_milli_satoshis()
                        .unwrap_or(1_000)
                        .div_ceil(1_000),
                ),
                fee_reserve: Amount::from(1),
                state: MeltQuoteState::Unpaid,
                expiry: 4_000_000_000,
                payment_preimage: None,
                change: None,
                request: Some(request.request.to_string()),
                unit: Some(request.unit.clone()),
                method: PaymentMethod::BOLT11,
            })
        }
        MeltQuoteRequest::Custom(request) => MeltQuoteCreateResponse::Custom((
            PaymentMethod::from(request.method.as_str()),
            crate::MeltQuoteCustomResponse {
                quote: quote.to_string(),
                method: PaymentMethod::from(request.method.as_str()),
                amount: Amount::from(1),
                fee_reserve: Some(Amount::from(1)),
                state: MeltQuoteState::Unpaid,
                expiry: 4_000_000_000,
                payment_preimage: None,
                change: None,
                request: Some(request.request.clone()),
                unit: Some(request.unit.clone()),
                extra: serde_json::Value::Null,
            },
        )),
        _ => panic!("unsupported common federation melt quote fixture"),
    };
    FederationMeltQuoteOperation {
        quote: quote.to_string(),
        request,
        response,
        metadata: FederationMeltQuoteMetadata::default(),
    }
}

/// Observed outgoing melt payment state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationMeltQuotePaymentObservation {
    /// Quote ID being observed.
    pub quote: String,
    /// Federation member that observed this payment state.
    pub observer: MemberId,
    /// Payment method for the quote.
    pub method: PaymentMethod,
    /// Observed quote state.
    pub state: MeltQuoteState,
    /// Amount intended for the external payment.
    pub amount: Amount,
    /// Fee reserve associated with the quote.
    pub fee_reserve: Amount,
    /// Optional stable backend payment identifier observed by this member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_id: Option<PaymentIdentifier>,
    /// Optional method-specific proof, preimage, outpoint, or settlement reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_proof: Option<String>,
}

impl FederationMeltQuotePaymentObservation {
    /// Return true when two observations describe the same payment evidence.
    #[must_use]
    pub fn matches_payment_evidence(&self, other: &Self) -> bool {
        self.quote == other.quote
            && self.method == other.method
            && self.state == other.state
            && self.amount == other.amount
            && self.fee_reserve == other.fee_reserve
            && self.payment_id == other.payment_id
            && self.payment_proof == other.payment_proof
    }

    /// Count matching and conflicting observations from distinct members.
    #[must_use]
    pub fn quorum<'a, I>(
        &self,
        observations: I,
        required: u16,
    ) -> FederationPaymentObservationQuorum
    where
        I: IntoIterator<Item = &'a Self>,
    {
        let mut matching = BTreeSet::from([self.observer]);
        let mut conflicting = BTreeSet::new();

        for observation in observations {
            if observation.quote != self.quote || observation.state != self.state {
                continue;
            }

            if self.matches_payment_evidence(observation) {
                matching.insert(observation.observer);
            } else {
                conflicting.insert(observation.observer);
            }
        }

        FederationPaymentObservationQuorum::new(
            FederationPaymentObservationContext::MeltQuote,
            required,
            matching,
            conflicting,
        )
    }
}

/// Payment observation extracted from an accepted federation operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FederationPaymentObservation {
    /// Incoming mint quote payment observation.
    MintQuote(FederationMintQuotePaymentObservation),
    /// Outgoing melt quote payment observation.
    MeltQuote(FederationMeltQuotePaymentObservation),
}

impl FederationPaymentObservation {
    /// Extract a durable payment observation from a consensus operation.
    #[must_use]
    pub fn from_operation(operation: &FederationOperation) -> Option<Self> {
        match operation {
            FederationOperation::MintQuotePayment(observation) => {
                Some(Self::MintQuote(observation.clone()))
            }
            FederationOperation::MeltQuotePayment(observation) => {
                Some(Self::MeltQuote(observation.clone()))
            }
            FederationOperation::AddressAllocation(_)
            | FederationOperation::ChainObservation(_)
            | FederationOperation::MintQuote(_)
            | FederationOperation::Mint(_)
            | FederationOperation::MeltQuote(_)
            | FederationOperation::Melt(_)
            | FederationOperation::Swap(_)
            | FederationOperation::KeysetRotation(_)
            | FederationOperation::TransactionProposal(_)
            | FederationOperation::TransactionSigned(_)
            | FederationOperation::TransactionBroadcastIntent(_)
            | FederationOperation::TransactionBroadcastObservation(_) => None,
        }
    }

    /// Return the observation context.
    #[must_use]
    pub fn context(&self) -> FederationPaymentObservationContext {
        match self {
            Self::MintQuote(_) => FederationPaymentObservationContext::MintQuote,
            Self::MeltQuote(_) => FederationPaymentObservationContext::MeltQuote,
        }
    }

    /// Return the quote ID observed.
    #[must_use]
    pub fn quote(&self) -> &str {
        match self {
            Self::MintQuote(observation) => observation.quote.as_str(),
            Self::MeltQuote(observation) => observation.quote.as_str(),
        }
    }

    /// Return the observing member.
    #[must_use]
    pub fn observer(&self) -> MemberId {
        match self {
            Self::MintQuote(observation) => observation.observer,
            Self::MeltQuote(observation) => observation.observer,
        }
    }

    /// Return the observed state as a stable label.
    #[must_use]
    pub fn state_label(&self) -> String {
        match self {
            Self::MintQuote(observation) => observation.state.to_string(),
            Self::MeltQuote(observation) => observation.state.to_string(),
        }
    }

    /// Return true when this observation describes a terminal quote state.
    #[must_use]
    pub fn is_terminal_state(&self) -> bool {
        match self {
            Self::MintQuote(observation) => {
                matches!(
                    observation.state,
                    MintQuoteState::Paid | MintQuoteState::Issued
                )
            }
            Self::MeltQuote(observation) => matches!(
                observation.state,
                MeltQuoteState::Paid | MeltQuoteState::Failed | MeltQuoteState::Unpaid
            ),
        }
    }

    /// Return true when two observations describe the same payment evidence.
    #[must_use]
    pub fn matches_payment_evidence(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::MintQuote(left), Self::MintQuote(right)) => left.matches_payment_evidence(right),
            (Self::MeltQuote(left), Self::MeltQuote(right)) => left.matches_payment_evidence(right),
            (Self::MintQuote(_), Self::MeltQuote(_)) | (Self::MeltQuote(_), Self::MintQuote(_)) => {
                false
            }
        }
    }

    /// Count matching and conflicting observations from distinct members.
    #[must_use]
    pub fn quorum<'a, I>(
        &self,
        observations: I,
        required: u16,
    ) -> FederationPaymentObservationQuorum
    where
        I: IntoIterator<Item = &'a Self>,
    {
        match self {
            Self::MintQuote(candidate) => candidate.quorum(
                observations
                    .into_iter()
                    .filter_map(|observation| match observation {
                        Self::MintQuote(observation) => Some(observation),
                        Self::MeltQuote(_) => None,
                    }),
                required,
            ),
            Self::MeltQuote(candidate) => candidate.quorum(
                observations
                    .into_iter()
                    .filter_map(|observation| match observation {
                        Self::MintQuote(_) => None,
                        Self::MeltQuote(observation) => Some(observation),
                    }),
                required,
            ),
        }
    }
}

/// Durable payment observation plus the consensus entry that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPaymentObservationRecord {
    /// Stable operation ID for the consensus observation.
    pub operation_id: FederationOperationId,
    /// Consensus order index of the observation.
    pub operation_index: u64,
    /// Typed payment observation evidence.
    pub observation: FederationPaymentObservation,
}

impl FederationPaymentObservationRecord {
    /// Extract a durable observation record from a federation journal entry.
    #[must_use]
    pub fn from_journal_entry(entry: &FederationJournalEntry) -> Option<Self> {
        FederationPaymentObservation::from_operation(&entry.envelope.operation).map(|observation| {
            Self {
                operation_id: entry.operation_id,
                operation_index: entry.index,
                observation,
            }
        })
    }
}

/// Operator-facing status for stored payment observation evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationPaymentObservationReportStatus {
    /// No stored observations were found for the requested quote and context.
    Empty,
    /// Observations exist, but no matching group has reached the required quorum.
    AwaitingQuorum,
    /// At least one matching evidence group reached quorum and no conflicts were found.
    QuorumReached,
    /// Conflicting evidence or competing terminal states were found.
    Conflicting,
}

impl fmt::Display for FederationPaymentObservationReportStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("empty"),
            Self::AwaitingQuorum => f.write_str("awaiting_quorum"),
            Self::QuorumReached => f.write_str("quorum_reached"),
            Self::Conflicting => f.write_str("conflicting"),
        }
    }
}

/// Operator action suggested by a payment observation report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationPaymentObservationReportAction {
    /// No action is currently suggested.
    #[default]
    None,
    /// The quote should be inspected by an operator or follow-up task.
    Inspect,
}

impl fmt::Display for FederationPaymentObservationReportAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Inspect => f.write_str("inspect"),
        }
    }
}

/// One group of payment observations that all describe the same evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPaymentObservationEvidenceGroup {
    /// Representative observation for this evidence group.
    pub representative: FederationPaymentObservation,
    /// Members whose observations match the representative evidence.
    pub observers: Vec<MemberId>,
    /// Operation IDs that carried matching observations.
    pub operation_ids: Vec<FederationOperationId>,
    /// Consensus order indices that carried matching observations.
    pub operation_indices: Vec<u64>,
    /// Members that reported a different evidence payload for the same quote and state.
    pub conflicting_observers: Vec<MemberId>,
    /// Whether this matching evidence group satisfies the configured quorum.
    pub quorum_reached: bool,
}

/// Redacted summary for one group of matching payment observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPaymentObservationEvidenceGroupSummary {
    /// Observed quote state for this evidence group.
    pub state: String,
    /// Members whose observations match this evidence group.
    pub observers: Vec<MemberId>,
    /// Number of distinct matching observers.
    pub observer_count: u16,
    /// Operation IDs that carried matching observations.
    pub operation_ids: Vec<FederationOperationId>,
    /// Consensus order indices that carried matching observations.
    pub operation_indices: Vec<u64>,
    /// Members that reported a different evidence payload for the same quote and state.
    pub conflicting_observers: Vec<MemberId>,
    /// Whether this matching evidence group satisfies the configured quorum.
    pub quorum_reached: bool,
    /// Whether this group represents a terminal quote state for conflict reporting.
    pub terminal_state: bool,
}

/// Redacted payment observation report summary for operator status surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPaymentObservationReportSummary {
    /// Observation context being summarized.
    pub context: FederationPaymentObservationContext,
    /// Quote ID being summarized.
    pub quote: String,
    /// Required matching observer count for quorum.
    pub required: u16,
    /// Number of matching records included in this report.
    pub total_observations: usize,
    /// Evidence groups found among stored observations without backend proof material.
    pub groups: Vec<FederationPaymentObservationEvidenceGroupSummary>,
    /// Distinct members whose observations conflict with another evidence group.
    pub conflicting_observers: Vec<MemberId>,
    /// Distinct members participating in competing terminal-state observations.
    pub competing_terminal_state_observers: Vec<MemberId>,
    /// High-level report status for operator and admin surfaces.
    pub status: FederationPaymentObservationReportStatus,
    /// Age of the quote in seconds when it can be resolved from local quote storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_age_secs: Option<u64>,
    /// Whether this unresolved report has exceeded the stale observation threshold.
    pub stale: bool,
    /// Suggested operator action for this report.
    pub recommended_action: FederationPaymentObservationReportAction,
}

/// Summary of stored payment observations for one quote and context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPaymentObservationReport {
    /// Observation context being summarized.
    pub context: FederationPaymentObservationContext,
    /// Quote ID being summarized.
    pub quote: String,
    /// Required matching observer count for quorum.
    pub required: u16,
    /// Number of matching records included in this report.
    pub total_observations: usize,
    /// Evidence groups found among stored observations.
    pub groups: Vec<FederationPaymentObservationEvidenceGroup>,
    /// Distinct members whose observations conflict with another evidence group.
    pub conflicting_observers: Vec<MemberId>,
    /// Distinct members participating in competing terminal-state observations.
    pub competing_terminal_state_observers: Vec<MemberId>,
    /// High-level report status for operator and admin surfaces.
    pub status: FederationPaymentObservationReportStatus,
}

impl FederationPaymentObservationReport {
    /// Build a report from stored observations for one quote and context.
    #[must_use]
    pub fn from_records(
        context: FederationPaymentObservationContext,
        quote: &str,
        required: u16,
        records: &[FederationPaymentObservationRecord],
    ) -> Self {
        let matching_records = records
            .iter()
            .filter(|record| {
                record.observation.context() == context && record.observation.quote() == quote
            })
            .collect::<Vec<_>>();
        let observations = matching_records
            .iter()
            .map(|record| &record.observation)
            .collect::<Vec<_>>();
        let mut groups = Vec::<FederationPaymentObservationEvidenceGroup>::new();

        for record in matching_records.iter().copied() {
            match groups.iter_mut().find(|group| {
                group
                    .representative
                    .matches_payment_evidence(&record.observation)
            }) {
                Some(group) => {
                    group.observers.push(record.observation.observer());
                    group.operation_ids.push(record.operation_id);
                    group.operation_indices.push(record.operation_index);
                }
                None => groups.push(FederationPaymentObservationEvidenceGroup {
                    representative: record.observation.clone(),
                    observers: vec![record.observation.observer()],
                    operation_ids: vec![record.operation_id],
                    operation_indices: vec![record.operation_index],
                    conflicting_observers: Vec::new(),
                    quorum_reached: false,
                }),
            }
        }

        for group in &mut groups {
            let quorum = group
                .representative
                .quorum(observations.iter().copied(), required);
            let quorum_reached = quorum.is_reached();
            group.conflicting_observers = quorum.conflicting_observers;
            group.quorum_reached = quorum_reached;
            group.observers.sort_unstable();
            group.observers.dedup();
        }

        let mut conflicting_observers = groups
            .iter()
            .flat_map(|group| group.conflicting_observers.iter().copied())
            .collect::<BTreeSet<_>>();
        let competing_terminal_state_observers = competing_terminal_state_observers(&groups);
        conflicting_observers.extend(competing_terminal_state_observers.iter().copied());
        let conflicting_observers = conflicting_observers.into_iter().collect::<Vec<_>>();
        let status = if groups.is_empty() {
            FederationPaymentObservationReportStatus::Empty
        } else if !conflicting_observers.is_empty() {
            FederationPaymentObservationReportStatus::Conflicting
        } else if groups.iter().any(|group| group.quorum_reached) {
            FederationPaymentObservationReportStatus::QuorumReached
        } else {
            FederationPaymentObservationReportStatus::AwaitingQuorum
        };

        Self {
            context,
            quote: quote.to_string(),
            required,
            total_observations: matching_records.len(),
            groups,
            conflicting_observers,
            competing_terminal_state_observers,
            status,
        }
    }

    /// Return true when the report contains any conflicting evidence.
    #[must_use]
    pub fn has_conflicts(&self) -> bool {
        self.status == FederationPaymentObservationReportStatus::Conflicting
    }

    /// Return true when no unique evidence group has reached quorum.
    #[must_use]
    pub fn is_unresolved(&self) -> bool {
        !self.groups.iter().any(|group| group.quorum_reached) || self.has_competing_quorums()
    }

    /// Return true when incompatible evidence groups both satisfy quorum.
    ///
    /// This is the minimal conflicting-threshold safety policy: minority
    /// disagreement is reported but cannot block a valid quorum, while two
    /// incompatible quorums leave the quote unresolved.
    #[must_use]
    pub fn has_competing_quorums(&self) -> bool {
        self.groups
            .iter()
            .filter(|group| group.quorum_reached)
            .take(2)
            .count()
            > 1
    }

    /// Return a redacted summary that does not expose backend proof material.
    #[must_use]
    pub fn summary(&self) -> FederationPaymentObservationReportSummary {
        self.summary_with_quote_age(None, 0)
    }

    /// Return a redacted summary with quote-age based staleness classification.
    #[must_use]
    pub fn summary_with_quote_age(
        &self,
        quote_age_secs: Option<u64>,
        stale_after_secs: u64,
    ) -> FederationPaymentObservationReportSummary {
        let stale = quote_age_secs.is_some_and(|age| {
            stale_after_secs > 0 && self.is_unresolved() && age >= stale_after_secs
        });
        let recommended_action = if self.has_conflicts() || stale {
            FederationPaymentObservationReportAction::Inspect
        } else {
            FederationPaymentObservationReportAction::None
        };

        FederationPaymentObservationReportSummary {
            context: self.context,
            quote: self.quote.clone(),
            required: self.required,
            total_observations: self.total_observations,
            groups: self
                .groups
                .iter()
                .map(|group| FederationPaymentObservationEvidenceGroupSummary {
                    state: group.representative.state_label(),
                    observers: group.observers.clone(),
                    observer_count: group.observers.len().try_into().unwrap_or(u16::MAX),
                    operation_ids: group.operation_ids.clone(),
                    operation_indices: group.operation_indices.clone(),
                    conflicting_observers: group.conflicting_observers.clone(),
                    quorum_reached: group.quorum_reached,
                    terminal_state: group.representative.is_terminal_state(),
                })
                .collect(),
            conflicting_observers: self.conflicting_observers.clone(),
            competing_terminal_state_observers: self.competing_terminal_state_observers.clone(),
            status: self.status,
            quote_age_secs,
            stale,
            recommended_action,
        }
    }
}

/// Result of evaluating matching payment observations against a quorum policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationPaymentObservationQuorum {
    /// Observation context being evaluated.
    pub context: FederationPaymentObservationContext,
    /// Number of matching observers required.
    pub required: u16,
    /// Members whose observations match the candidate evidence.
    pub matching_observers: Vec<MemberId>,
    /// Members whose observations conflict with the candidate evidence.
    pub conflicting_observers: Vec<MemberId>,
}

impl FederationPaymentObservationQuorum {
    fn new(
        context: FederationPaymentObservationContext,
        required: u16,
        matching_observers: BTreeSet<MemberId>,
        conflicting_observers: BTreeSet<MemberId>,
    ) -> Self {
        Self {
            context,
            required,
            matching_observers: matching_observers.into_iter().collect(),
            conflicting_observers: conflicting_observers.into_iter().collect(),
        }
    }

    /// Number of distinct members with matching observations.
    #[must_use]
    pub fn matching_count(&self) -> u16 {
        self.matching_observers.len().try_into().unwrap_or(u16::MAX)
    }

    /// Number of distinct members with conflicting observations.
    #[must_use]
    pub fn conflicting_count(&self) -> u16 {
        self.conflicting_observers
            .len()
            .try_into()
            .unwrap_or(u16::MAX)
    }

    /// Return true once the matching observer set reaches the configured quorum.
    #[must_use]
    pub fn is_reached(&self) -> bool {
        self.matching_count() >= self.required
    }

    /// Return true when at least one member submitted conflicting evidence.
    #[must_use]
    pub fn has_conflicts(&self) -> bool {
        !self.conflicting_observers.is_empty()
    }
}

fn competing_terminal_state_observers(
    groups: &[FederationPaymentObservationEvidenceGroup],
) -> Vec<MemberId> {
    let terminal_groups = groups
        .iter()
        .filter(|group| group.representative.is_terminal_state())
        .collect::<Vec<_>>();

    if terminal_groups.len() <= 1 {
        return Vec::new();
    }

    terminal_groups
        .into_iter()
        .flat_map(|group| group.observers.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Request to spend proofs against a melt quote.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationMeltOperation {
    /// Original wallet melt request.
    pub request: MeltRequest<String>,
}

/// Request to spend proofs and sign replacement outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSwapOperation {
    /// Original wallet swap request.
    pub request: SwapRequest,
}

/// Request to add a new federated aggregate keyset through consensus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationKeysetRotationOperation {
    /// Proposed public aggregate keyset and every member's public BLS shares.
    pub keyset: FederatedKeyset,
}

impl FederationKeysetRotationOperation {
    /// Validate this proposed keyset against the current federation roster.
    pub fn validate_against_config(&self, config: &FederationConfig) -> Result<()> {
        self.keyset
            .validate(config.threshold_params, &config.members)?;
        if !self.keyset.active {
            return Err(FederationError::InactiveFederatedKeyset {
                keyset_id: self.keyset.id,
            });
        }

        Ok(())
    }
}

/// Operation-local value accounting summary computed before journal acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationOperationBalanceAudit {
    /// Operation kind that produced this audit row.
    pub operation_kind: FederationOperationKind,
    /// Spendable value consumed by this operation, or incoming value observed for a quote.
    pub assets: Amount,
    /// Value signed or already issued by this operation-local scope.
    pub liabilities: Amount,
    /// External payment amount observed for melt quotes.
    pub external_payment: Amount,
    /// Fee reserve observed for melt quotes.
    pub fee_reserve: Amount,
    /// Non-negative remainder after subtracting liabilities from assets when locally checkable.
    pub remainder: Amount,
}

impl FederationOperationBalanceAudit {
    fn unchecked(operation_kind: FederationOperationKind) -> Self {
        Self {
            operation_kind,
            assets: Amount::ZERO,
            liabilities: Amount::ZERO,
            external_payment: Amount::ZERO,
            fee_reserve: Amount::ZERO,
            remainder: Amount::ZERO,
        }
    }

    fn checked(
        operation_kind: FederationOperationKind,
        assets: Amount,
        liabilities: Amount,
    ) -> Result<Self> {
        let remainder =
            assets
                .checked_sub(liabilities)
                .ok_or(FederationError::NegativeBalanceAudit {
                    operation_kind,
                    assets,
                    liabilities,
                })?;

        Ok(Self {
            operation_kind,
            assets,
            liabilities,
            external_payment: Amount::ZERO,
            fee_reserve: Amount::ZERO,
            remainder,
        })
    }

    fn external(
        operation_kind: FederationOperationKind,
        external_payment: Amount,
        fee_reserve: Amount,
    ) -> Self {
        Self {
            operation_kind,
            assets: Amount::ZERO,
            liabilities: Amount::ZERO,
            external_payment,
            fee_reserve,
            remainder: Amount::ZERO,
        }
    }
}

/// Consensus operation payload.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FederationOperation {
    /// Allocate one deterministic descriptor address after consensus.
    AddressAllocation(FederationAddressAllocationOperation),
    /// Submit one bounded local-chain observation for consensus materialization.
    ChainObservation(FederationChainObservation),
    /// Create and persist a mint quote.
    MintQuote(FederationMintQuoteOperation),
    /// Observe a mint quote payment state.
    MintQuotePayment(FederationMintQuotePaymentObservation),
    /// Spend a paid mint quote and sign blinded outputs.
    Mint(FederationMintOperation),
    /// Create and persist a melt quote.
    MeltQuote(FederationMeltQuoteOperation),
    /// Observe an outgoing melt payment state.
    MeltQuotePayment(FederationMeltQuotePaymentObservation),
    /// Spend proofs against a melt quote and optionally sign change outputs.
    Melt(FederationMeltOperation),
    /// Spend proofs and sign replacement outputs.
    Swap(FederationSwapOperation),
    /// Add a new federated aggregate keyset after consensus.
    KeysetRotation(FederationKeysetRotationOperation),
    /// Accept an exact unsigned on-chain transaction after consensus.
    TransactionProposal(FederationTransactionProposal),
    /// Accept the exact threshold-signed transaction after consensus.
    TransactionSigned(FederationSignedTransaction),
    /// Authorize network submission only after exact signed-byte persistence.
    TransactionBroadcastIntent(FederationTransactionBroadcastIntent),
    /// Record one authenticated local-backend broadcast result.
    TransactionBroadcastObservation(FederationTransactionBroadcastObservation),
}

impl FederationOperation {
    /// Return the operation kind.
    #[must_use]
    pub fn kind(&self) -> FederationOperationKind {
        match self {
            Self::AddressAllocation(_) => FederationOperationKind::AddressAllocation,
            Self::ChainObservation(_) => FederationOperationKind::ChainObservation,
            Self::MintQuote(_) => FederationOperationKind::MintQuote,
            Self::MintQuotePayment(_) => FederationOperationKind::MintQuotePayment,
            Self::Mint(_) => FederationOperationKind::Mint,
            Self::MeltQuote(_) => FederationOperationKind::MeltQuote,
            Self::MeltQuotePayment(_) => FederationOperationKind::MeltQuotePayment,
            Self::Melt(_) => FederationOperationKind::Melt,
            Self::Swap(_) => FederationOperationKind::Swap,
            Self::KeysetRotation(_) => FederationOperationKind::KeysetRotation,
            Self::TransactionProposal(_) => FederationOperationKind::TransactionProposal,
            Self::TransactionSigned(_) => FederationOperationKind::TransactionSigned,
            Self::TransactionBroadcastIntent(_) => {
                FederationOperationKind::TransactionBroadcastIntent
            }
            Self::TransactionBroadcastObservation(_) => {
                FederationOperationKind::TransactionBroadcastObservation
            }
        }
    }

    /// Return all conflict keys that must be unique for this operation.
    #[must_use]
    pub fn conflict_keys(&self) -> Vec<FederationOperationConflictKey> {
        match self {
            Self::AddressAllocation(operation) => {
                vec![FederationOperationConflictKey::AddressAllocation {
                    application_id: operation.application_id,
                    application_key_epoch: operation.application_key_epoch,
                    quote: operation.quote.clone(),
                }]
            }
            Self::ChainObservation(FederationChainObservation::Deposit(observation)) => {
                vec![FederationOperationConflictKey::DepositObservation {
                    outpoint: observation.outpoint,
                    confirmation_depth: observation
                        .confirmation
                        .map_or(0, |confirmation| confirmation.confirmation_depth),
                    observer: observation.observer,
                }]
            }
            Self::ChainObservation(FederationChainObservation::Reorg(observation)) => {
                vec![FederationOperationConflictKey::DepositReorgObservation {
                    outpoint: observation.outpoint,
                    observer: observation.observer,
                }]
            }
            Self::MintQuote(operation) => vec![FederationOperationConflictKey::MintQuote {
                quote: operation.quote.clone(),
            }],
            Self::MintQuotePayment(observation) => {
                match (&observation.method, &observation.payment_id) {
                    (method, Some(PaymentIdentifier::PaymentId(payment_id)))
                        if method.is_onchain() =>
                    {
                        vec![FederationOperationConflictKey::MintQuotePaymentRevision {
                            quote: observation.quote.clone(),
                            observer: observation.observer,
                            payment_id: *payment_id,
                        }]
                    }
                    _ => vec![FederationOperationConflictKey::MintQuotePayment {
                        quote: observation.quote.clone(),
                        state: observation.state,
                        observer: observation.observer,
                    }],
                }
            }
            Self::Mint(operation) => operation
                .request
                .quote_ids()
                .into_iter()
                .map(|quote| FederationOperationConflictKey::Mint {
                    quote: quote.to_string(),
                })
                .collect(),
            Self::MeltQuote(operation) => vec![FederationOperationConflictKey::MeltQuote {
                quote: operation.quote.clone(),
            }],
            Self::MeltQuotePayment(observation) => {
                vec![FederationOperationConflictKey::MeltQuotePayment {
                    quote: observation.quote.clone(),
                    state: observation.state,
                    observer: observation.observer,
                }]
            }
            Self::Melt(operation) => vec![FederationOperationConflictKey::Melt {
                quote: operation.request.quote().clone(),
            }],
            Self::KeysetRotation(operation) => {
                vec![FederationOperationConflictKey::KeysetRotation {
                    keyset_id: operation.keyset.id,
                }]
            }
            Self::TransactionProposal(proposal) => {
                if proposal.replacement_of.is_some() {
                    return Vec::new();
                }
                let mut keys = Vec::with_capacity(
                    proposal.intent_ids.len() + proposal.selected_inputs.len() + 2,
                );
                keys.push(FederationOperationConflictKey::OnchainBatch {
                    batch_id: proposal.batch_id,
                });
                keys.push(FederationOperationConflictKey::OnchainReplacementLineage {
                    lineage: proposal.replacement_lineage,
                });
                keys.extend(proposal.intent_ids.iter().copied().map(|intent_id| {
                    FederationOperationConflictKey::OnchainPaymentIntentAssignment { intent_id }
                }));
                keys.extend(proposal.selected_inputs.iter().map(|input| {
                    FederationOperationConflictKey::OnchainUtxo {
                        outpoint: input.outpoint,
                    }
                }));
                keys
            }
            Self::Swap(_)
            | Self::TransactionSigned(_)
            | Self::TransactionBroadcastIntent(_)
            | Self::TransactionBroadcastObservation(_) => Vec::new(),
        }
    }

    /// Return the quote ID carried by quote-scoped operations.
    #[must_use]
    pub fn quote(&self) -> Option<&str> {
        match self {
            Self::AddressAllocation(operation) => Some(operation.quote.as_str()),
            Self::ChainObservation(FederationChainObservation::Deposit(observation)) => {
                Some(observation.allocation.quote.as_str())
            }
            Self::ChainObservation(FederationChainObservation::Reorg(_)) => None,
            Self::MintQuote(operation) => Some(operation.quote.as_str()),
            Self::MintQuotePayment(observation) => Some(observation.quote.as_str()),
            Self::Mint(operation) => operation.request.quote_ids().into_iter().next(),
            Self::MeltQuote(operation) => Some(operation.quote.as_str()),
            Self::MeltQuotePayment(observation) => Some(observation.quote.as_str()),
            Self::Melt(operation) => Some(operation.request.quote().as_str()),
            Self::Swap(_)
            | Self::KeysetRotation(_)
            | Self::TransactionProposal(_)
            | Self::TransactionSigned(_)
            | Self::TransactionBroadcastIntent(_)
            | Self::TransactionBroadcastObservation(_) => None,
        }
    }

    /// Compute the operation-local balance audit for this consensus payload.
    ///
    /// This audit intentionally only checks value scopes available in the
    /// canonical operation payload. Quote-level payment quorum checks remain in
    /// the mint apply layer, where duplicate observer evidence can be
    /// de-duplicated by quote and policy.
    pub fn balance_audit(&self) -> Result<FederationOperationBalanceAudit> {
        let operation_kind = self.kind();
        match self {
            Self::AddressAllocation(_)
            | Self::ChainObservation(_)
            | Self::MintQuote(_)
            | Self::KeysetRotation(_)
            | Self::TransactionProposal(_)
            | Self::TransactionSigned(_)
            | Self::TransactionBroadcastIntent(_)
            | Self::TransactionBroadcastObservation(_) => {
                Ok(FederationOperationBalanceAudit::unchecked(operation_kind))
            }
            Self::MintQuotePayment(observation) => FederationOperationBalanceAudit::checked(
                operation_kind,
                observation.amount_paid,
                observation.amount_issued,
            ),
            Self::Mint(operation) => Ok(FederationOperationBalanceAudit {
                operation_kind,
                assets: Amount::ZERO,
                liabilities: operation.request.output_amount()?,
                external_payment: Amount::ZERO,
                fee_reserve: Amount::ZERO,
                remainder: Amount::ZERO,
            }),
            Self::MeltQuote(_) => Ok(FederationOperationBalanceAudit::unchecked(operation_kind)),
            Self::MeltQuotePayment(observation) => Ok(FederationOperationBalanceAudit::external(
                operation_kind,
                observation.amount,
                observation.fee_reserve,
            )),
            Self::Melt(operation) => FederationOperationBalanceAudit::checked(
                operation_kind,
                operation
                    .request
                    .inputs_amount()
                    .map_err(|_| FederationError::BalanceAuditAmountOverflow { operation_kind })?,
                output_amount(operation.request.outputs(), operation_kind)?,
            ),
            Self::Swap(operation) => {
                FederationOperationBalanceAudit::checked(
                    operation_kind,
                    operation.request.input_amount().map_err(|_| {
                        FederationError::BalanceAuditAmountOverflow { operation_kind }
                    })?,
                    operation.request.output_amount().map_err(|_| {
                        FederationError::BalanceAuditAmountOverflow { operation_kind }
                    })?,
                )
            }
        }
    }
}

impl FederationMintRequest {
    fn output_amount(&self) -> Result<Amount> {
        match self {
            Self::Single(request) => {
                outputs_amount(&request.outputs, FederationOperationKind::Mint)
            }
            Self::Batch(request) => outputs_amount(&request.outputs, FederationOperationKind::Mint),
        }
    }
}

fn output_amount(
    outputs: &Option<Vec<BlindedMessage>>,
    operation_kind: FederationOperationKind,
) -> Result<Amount> {
    match outputs {
        Some(outputs) => outputs_amount(outputs, operation_kind),
        None => Ok(Amount::ZERO),
    }
}

fn outputs_amount(
    outputs: &[BlindedMessage],
    operation_kind: FederationOperationKind,
) -> Result<Amount> {
    Amount::try_sum(outputs.iter().map(|output| output.amount))
        .map_err(|_| FederationError::BalanceAuditAmountOverflow { operation_kind })
}

/// Schnorr signature authenticating the federation member that authored an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederationOperationSignature([u8; 64]);

impl FederationOperationSignature {
    /// Create an operation signature from raw Schnorr bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 64] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidOperationSignatureSize {
                    expected: 64,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse an operation signature from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this signature as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 64] {
        self.0
    }

    /// Return this signature as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn as_schnorr(self) -> Result<Signature> {
        Signature::from_slice(&self.0).map_err(|_| FederationError::InvalidOperationSignature)
    }
}

impl fmt::Display for FederationOperationSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationOperationSignature {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationOperationSignature {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationOperationSignature {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Durable, receiver-independent member authorization for an operation envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationOperationAuthorization {
    /// Federation member that authored the operation.
    pub author: MemberId,
    /// Signature over the author and canonical unsigned operation envelope.
    pub signature: FederationOperationSignature,
}

#[derive(Serialize)]
struct FederationUnsignedOperationEnvelope<'a> {
    version: u16,
    federation_id: FederationId,
    operation: &'a FederationOperation,
}

/// Versioned envelope that binds an operation to one federation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationOperationEnvelope {
    /// Operation envelope version.
    pub version: u16,
    /// Federation whose members must order this operation.
    pub federation_id: FederationId,
    /// Operation payload.
    pub operation: FederationOperation,
    /// Receiver-independent member authorization required at consensus admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<FederationOperationAuthorization>,
}

impl FederationOperationEnvelope {
    /// Create a new operation envelope for the current operation format.
    #[must_use]
    pub fn new(federation_id: FederationId, operation: FederationOperation) -> Self {
        Self {
            version: FEDERATION_OPERATION_VERSION,
            federation_id,
            operation,
            authorization: None,
        }
    }

    /// Sign this operation with the authoring federation member's identity key.
    pub fn authorize(mut self, private_config: &MemberConfigPrivate) -> Result<Self> {
        if let Some(federation_id) = private_config.federation_id {
            if federation_id != self.federation_id {
                return Err(FederationError::FederationIdMismatch {
                    expected: self.federation_id,
                    found: federation_id,
                });
            }
        }
        let author = private_config.member_id;
        let transcript = self.authorization_transcript(author)?;
        let signature = private_config
            .identity_secret_key
            .sign_no_aux_rand(&transcript)?;
        self.authorization = Some(FederationOperationAuthorization {
            author,
            signature: FederationOperationSignature(signature.serialize()),
        });
        Ok(self)
    }

    /// Verify the durable member authorization and operation-specific author binding.
    pub fn verify_authorization(&self, public_config: &FederationConfig) -> Result<MemberId> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: public_config.federation_id,
                found: self.federation_id,
            });
        }
        let authorization = self
            .authorization
            .ok_or(FederationError::OperationAuthorizationMissing)?;
        let member =
            public_config
                .member(authorization.author)
                .ok_or(FederationError::UnknownMember {
                    member_id: authorization.author,
                })?;
        let transcript = self.authorization_transcript(authorization.author)?;
        let signature = authorization.signature.as_schnorr()?;
        member
            .identity_public_key
            .verify(&transcript, &signature)
            .map_err(|_| FederationError::OperationSignatureVerificationFailed {
                member_id: authorization.author,
            })?;

        let claimed_observer = match &self.operation {
            FederationOperation::ChainObservation(observation) => Some(observation.observer()),
            FederationOperation::MintQuotePayment(observation) => Some(observation.observer),
            FederationOperation::MeltQuotePayment(observation) => Some(observation.observer),
            FederationOperation::TransactionProposal(proposal) => Some(proposal.proposer),
            FederationOperation::TransactionBroadcastObservation(observation) => {
                Some(observation.observer)
            }
            FederationOperation::AddressAllocation(_)
            | FederationOperation::MintQuote(_)
            | FederationOperation::Mint(_)
            | FederationOperation::MeltQuote(_)
            | FederationOperation::Melt(_)
            | FederationOperation::Swap(_)
            | FederationOperation::KeysetRotation(_)
            | FederationOperation::TransactionSigned(_)
            | FederationOperation::TransactionBroadcastIntent(_) => None,
        };
        if let Some(claimed_observer) = claimed_observer {
            if claimed_observer != authorization.author {
                return Err(FederationError::OperationAuthorMismatch {
                    author: authorization.author,
                    claimed_member: claimed_observer,
                });
            }
        }

        Ok(authorization.author)
    }

    /// Decode a JSON operation envelope and reject unsupported versions.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self> {
        let envelope: Self = serde_json::from_slice(bytes)?;
        envelope.ensure_supported_version()?;
        Ok(envelope)
    }

    /// Reject this operation if its envelope version is unsupported.
    pub fn ensure_supported_version(&self) -> Result<()> {
        if self.version != FEDERATION_OPERATION_VERSION {
            return Err(FederationError::OperationVersionMismatch {
                expected: FEDERATION_OPERATION_VERSION,
                found: self.version,
            });
        }

        Ok(())
    }

    /// Return canonical JSON bytes with all object keys sorted.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self)
    }

    /// Return the canonical operation envelope size in bytes.
    pub fn canonical_size(&self) -> Result<usize> {
        Ok(self.canonical_bytes()?.len())
    }

    /// Reject this operation if its canonical envelope exceeds `max_bytes`.
    pub fn ensure_within_size_limit(&self, max_bytes: usize) -> Result<usize> {
        self.ensure_supported_version()?;
        let actual = self.canonical_size()?;
        if actual > max_bytes {
            return Err(FederationError::OperationEnvelopeTooLarge {
                actual,
                max: max_bytes,
            });
        }

        Ok(actual)
    }

    /// Reject this operation if its canonical envelope exceeds the default consensus limit.
    pub fn ensure_admissible_size(&self) -> Result<usize> {
        self.ensure_within_size_limit(FEDERATION_MAX_OPERATION_ENVELOPE_BYTES)
    }

    /// Compute the domain-separated operation ID.
    pub fn operation_id(&self) -> Result<FederationOperationId> {
        self.ensure_supported_version()?;
        let canonical = self.unsigned_canonical_bytes()?;
        Ok(operation_id_from_canonical(&canonical))
    }

    /// Return true when another envelope has the same conflict key but different contents.
    pub fn conflicts_with(&self, other: &Self) -> Result<bool> {
        let left_keys = self.operation.conflict_keys();
        let right_keys = other.operation.conflict_keys();

        for left in &left_keys {
            if right_keys.iter().any(|right| right == left) {
                return Ok(self.operation_id()? != other.operation_id()?);
            }
        }

        Ok(false)
    }

    fn unsigned_canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(&FederationUnsignedOperationEnvelope {
            version: self.version,
            federation_id: self.federation_id,
            operation: &self.operation,
        })
    }

    fn authorization_transcript(&self, author: MemberId) -> Result<Vec<u8>> {
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, FEDERATION_OPERATION_AUTHORIZATION_VERSION);
        append_hash_bytes(&mut transcript, &author.get().to_be_bytes());
        append_hash_bytes(&mut transcript, &self.unsigned_canonical_bytes()?);
        Ok(transcript)
    }
}

fn operation_id_from_canonical(canonical: &[u8]) -> FederationOperationId {
    let mut transcript = Vec::new();
    append_hash_bytes(&mut transcript, FEDERATION_OPERATION_HASH_VERSION);
    append_hash_bytes(&mut transcript, canonical);
    FederationOperationId::from_bytes(Sha256Hash::hash(&transcript).to_byte_array())
}

fn canonical_json_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}

fn append_hash_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u32).to_be_bytes());
    transcript.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, str::FromStr};

    use serde_json::json;

    use super::*;
    use crate::federation::{
        generate_trusted_dealer_configs, InMemoryFederationJournal, ThresholdParams,
        TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    use crate::nuts::nut01::BlsSecretKey;
    use crate::{
        mint_url::MintUrl, BatchMintRequest, BlindedMessage, Conditions, CurrencyUnit, HTLCWitness,
        Id, MeltQuoteCustomRequest, MintQuoteBolt11Request, MintQuoteBolt11Response,
        MintQuoteCustomResponse, MintQuoteResponse, P2PKWitness, Proof, PublicKey, SecretKey,
        SigFlag, SpendingConditions, Witness,
    };

    fn federation_id() -> FederationId {
        FederationId::from_bytes([7; 32])
    }

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn keyset_id() -> Id {
        Id::from_str("00bfa73302d12ffd").expect("keyset id")
    }

    fn public_key(hex: &str) -> PublicKey {
        PublicKey::from_hex(hex).expect("public key")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("valid secp256k1 secret")
    }

    fn trusted_dealer_member(member_id: MemberId) -> TrustedDealerMember {
        TrustedDealerMember::new(
            member_id,
            MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
                .expect("public mint url"),
            MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get()))
                .expect("federation api url"),
            identity_secret(member_id),
        )
    }

    fn bls_key(byte: u8) -> BlsSecretKey {
        BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    #[test]
    fn federated_melt_quote_conversion_handles_bolt12_and_onchain() {
        use bitcoin::secp256k1::{PublicKey as Secp256k1PublicKey, Secp256k1, SecretKey};
        use lightning::offers::offer::OfferBuilder;

        use crate::nuts::nut30::MeltQuoteOnchainFeeOption;
        use crate::{MeltQuoteBolt12Request, MeltQuoteOnchainRequest, MeltQuoteOnchainResponse};

        let quote_id = QuoteId::new();
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0xcd; 32]).expect("secret key");
        let pubkey = Secp256k1PublicKey::from_secret_key(&secp, &secret_key);
        let offer = OfferBuilder::new(pubkey).build().expect("offer");
        let bolt12_request = MeltQuoteRequest::Bolt12(MeltQuoteBolt12Request {
            request: offer.to_string(),
            unit: CurrencyUnit::Sat,
            options: None,
        });
        let bolt12 = FederationMeltQuoteOperation {
            quote: quote_id.to_string(),
            request: bolt12_request,
            response: MeltQuoteCreateResponse::Bolt12(crate::MeltQuoteBolt12Response {
                quote: quote_id.to_string(),
                amount: Amount::from(2),
                fee_reserve: Amount::from(1),
                state: MeltQuoteState::Unpaid,
                expiry: 4_000_000_000,
                payment_preimage: None,
                change: None,
                request: Some(offer.to_string()),
                unit: Some(CurrencyUnit::Sat),
                method: PaymentMethod::BOLT12,
            }),
            metadata: FederationMeltQuoteMetadata::default(),
        };
        let bolt12_quote = bolt12
            .into_melt_quote(quote_id.clone(), 1)
            .expect("bolt12 quote");
        assert_eq!(bolt12_quote.payment_method, PaymentMethod::BOLT12);

        let onchain_quote_id = QuoteId::new();
        let onchain_request = MeltQuoteRequest::Onchain(MeltQuoteOnchainRequest {
            request: "bcrt1qexample".to_string(),
            amount: Amount::from(100),
            unit: CurrencyUnit::Sat,
        });
        let fee_options = vec![MeltQuoteOnchainFeeOption {
            fee_index: 7,
            fee_reserve: Amount::from(3),
            estimated_blocks: 6,
        }];
        let onchain = FederationMeltQuoteOperation {
            quote: onchain_quote_id.to_string(),
            request: onchain_request,
            response: MeltQuoteCreateResponse::Onchain(MeltQuoteOnchainResponse {
                quote: onchain_quote_id.to_string(),
                amount: Amount::from(100),
                unit: CurrencyUnit::Sat,
                method: PaymentMethod::Custom("onchain".to_string()),
                state: MeltQuoteState::Unpaid,
                expiry: 4_000_000_000,
                request: "bcrt1qexample".to_string(),
                fee_options: fee_options.clone(),
                selected_fee_index: None,
                outpoint: None,
                change: None,
            }),
            metadata: FederationMeltQuoteMetadata {
                request_lookup_id: Some(PaymentIdentifier::QuoteId(onchain_quote_id.clone())),
                extra_json: None,
                estimated_blocks: Some(6),
            },
        };
        let onchain_quote = onchain
            .into_melt_quote(onchain_quote_id, 2)
            .expect("onchain quote");
        assert_eq!(
            onchain_quote.payment_method,
            PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Onchain)
        );
        assert_eq!(onchain_quote.fee_options(), fee_options);
    }

    fn rotation_keyset_spec(seed: u8) -> TrustedDealerKeysetSpec {
        TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            BTreeMap::from([(Amount::from(1), vec![bls_key(seed), bls_key(seed + 10)])]),
        )
    }

    fn federation_config_with_keyset(seed: u8) -> FederationConfig {
        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("threshold params"),
            vec![
                trusted_dealer_member(member_id(1)),
                trusted_dealer_member(member_id(2)),
                trusted_dealer_member(member_id(3)),
            ],
            vec![rotation_keyset_spec(seed)],
        )
        .expect("trusted dealer config")
        .public_config
    }

    fn keyset_rotation(seed: u8) -> FederationKeysetRotationOperation {
        let keyset = federation_config_with_keyset(seed)
            .keysets
            .into_iter()
            .next()
            .expect("generated keyset");
        FederationKeysetRotationOperation { keyset }
    }

    fn keyset_rotation_operation(seed: u8) -> FederationOperation {
        FederationOperation::KeysetRotation(keyset_rotation(seed))
    }

    fn signed_transaction_operation(seed: u8) -> FederationOperation {
        let proposal_id = super::super::FederationTransactionProposalId::from_bytes([11; 32]);
        let txid = bitcoin::Txid::from_str(&"22".repeat(32)).expect("txid");
        FederationOperation::TransactionSigned(FederationSignedTransaction {
            version: 1,
            proposal_id,
            txid,
            transaction_bytes: vec![seed],
            policy_audit: super::super::FederationTransactionPolicyAudit {
                proposal_id,
                txid,
                fee_sat: 1,
                fee_rate_sat_per_kwu: 1,
                weight: 1,
                fee_contributions: BTreeMap::new(),
                taproot_sighashes: Vec::new(),
            },
            input_signatures: Vec::new(),
        })
    }

    fn output(amount: u64, hex: &str) -> BlindedMessage {
        BlindedMessage::new(Amount::from(amount), keyset_id(), public_key(hex))
    }

    fn proof(amount: u64, secret: &str) -> Proof {
        Proof::new(
            Amount::from(amount),
            keyset_id(),
            crate::secret::Secret::new(secret),
            public_key("02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd"),
        )
    }

    fn p2pk_witness(marker: &str) -> Witness {
        Witness::P2PKWitness(P2PKWitness {
            signatures: vec![marker.repeat(64)],
        })
    }

    fn htlc_witness(preimage_byte: u8, marker: &str) -> Witness {
        Witness::HTLCWitness(HTLCWitness {
            preimage: crate::util::hex::encode([preimage_byte; 32]),
            signatures: Some(vec![marker.repeat(64)]),
        })
    }

    fn p2pk_locked_proof(amount: u64, sig_flag: SigFlag) -> (Proof, SecretKey) {
        let secret_key = SecretKey::generate();
        let secret: crate::secret::Secret = SpendingConditions::new_p2pk(
            secret_key.public_key(),
            Some(Conditions {
                sig_flag,
                ..Default::default()
            }),
        )
        .try_into()
        .expect("p2pk locked secret");

        (
            Proof::new(
                Amount::from(amount),
                keyset_id(),
                secret,
                public_key("02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd"),
            ),
            secret_key,
        )
    }

    fn htlc_locked_proof(amount: u64, preimage_byte: u8, sig_flag: SigFlag) -> Proof {
        let preimage = crate::util::hex::encode([preimage_byte; 32]);
        let secret: crate::secret::Secret = SpendingConditions::new_htlc(
            preimage,
            Some(Conditions {
                sig_flag,
                ..Default::default()
            }),
        )
        .expect("htlc locked condition")
        .try_into()
        .expect("htlc locked secret");

        Proof::new(
            Amount::from(amount),
            keyset_id(),
            secret,
            public_key("02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd"),
        )
    }

    fn mint_operation(quote: &str, outputs: Vec<BlindedMessage>) -> FederationOperation {
        FederationOperation::Mint(FederationMintOperation {
            request: FederationMintRequest::Single(MintRequest {
                quote: quote.to_string(),
                outputs,
                signature: None,
            }),
        })
    }

    fn sign_mint_request_for_test(request: &mut MintRequest<String>, secret_key: &SecretKey) {
        let signature = secret_key
            .sign_no_aux_rand(&request.msg_to_sign())
            .expect("mint request signature");
        request.signature = Some(signature.to_string());
    }

    fn bolt11_quote_response(quote: &str, amount: u64) -> MintQuoteResponse<String> {
        MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: quote.to_string(),
            request: format!("invoice-{quote}"),
            amount: Some(Amount::from(amount)),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::BOLT11,
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state: MintQuoteState::Unpaid,
            expiry: None,
            pubkey: None,
        })
    }

    fn batch_mint_operation(
        quotes: Vec<&str>,
        outputs: Vec<BlindedMessage>,
    ) -> FederationOperation {
        FederationOperation::Mint(FederationMintOperation {
            request: FederationMintRequest::Batch(BatchMintRequest {
                quotes: quotes.into_iter().map(str::to_string).collect(),
                quote_amounts: None,
                outputs,
                signatures: None,
            }),
        })
    }

    fn envelope(operation: FederationOperation) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(federation_id(), operation)
    }

    fn payment_record(
        operation_index: u64,
        operation: FederationOperation,
    ) -> FederationPaymentObservationRecord {
        let entry = journal_entry(operation_index, operation);
        FederationPaymentObservationRecord::from_journal_entry(&entry)
            .expect("payment observation record")
    }

    fn journal_entry(
        operation_index: u64,
        operation: FederationOperation,
    ) -> FederationJournalEntry {
        let mut journal = InMemoryFederationJournal::new();
        let response = journal
            .append(envelope(operation))
            .expect("append operation");
        let mut entry = response.entry().clone();
        entry.index = operation_index;
        entry
    }

    fn oversized_mint_quote_envelope() -> FederationOperationEnvelope {
        let envelope = envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: "quote-oversized".to_string(),
                request_lookup_id: PaymentIdentifier::CustomId("quote-oversized".to_string()),
                request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                    amount: Amount::from(1),
                    unit: CurrencyUnit::Sat,
                    description: Some("x".repeat(FEDERATION_MAX_OPERATION_ENVELOPE_BYTES)),
                    pubkey: None,
                }),
                response: bolt11_quote_response("quote-oversized", 1),
            },
        ));
        assert!(
            envelope.canonical_size().expect("canonical size")
                > FEDERATION_MAX_OPERATION_ENVELOPE_BYTES
        );
        envelope
    }

    #[test]
    fn operation_id_serializes_as_hex() {
        let envelope = envelope(mint_operation(
            "quote-1",
            vec![output(
                1,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        ));
        let operation_id = envelope.operation_id().expect("operation id");
        let json = serde_json::to_string(&operation_id).expect("operation id json");

        assert_eq!(json.len(), 66);
        assert_eq!(
            serde_json::from_str::<FederationOperationId>(&json).expect("operation id from json"),
            operation_id
        );
        assert_eq!(
            FederationOperationId::from_slice(&operation_id.to_bytes())
                .expect("operation id from bytes"),
            operation_id
        );
    }

    #[test]
    fn canonical_operation_encoding_matches_v4_golden_vector() {
        let envelope = envelope(mint_operation(
            "quote-golden",
            vec![output(
                1,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        ));
        let encoded = envelope.canonical_bytes().expect("canonical bytes");
        let encoded = std::str::from_utf8(&encoded).expect("canonical utf8");

        assert_eq!(
            encoded,
            concat!(
                r#"{"federation_id":"0707070707070707070707070707070707070707070707070707070707070707","#,
                r#""operation":{"data":{"request":{"data":{"outputs":[{"B_":"0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41","#,
                r#""amount":1,"id":"00bfa73302d12ffd"}],"quote":"quote-golden"},"type":"single"}},"type":"mint"},"version":4}"#
            )
        );
        assert_eq!(
            envelope.operation_id().expect("operation id").to_hex(),
            "01d678fdcaea34b609ebd79940f71c02100e94f731b0ccb174f63e2e5abdc20c"
        );
    }

    #[test]
    fn operation_id_tracks_nut20_mint_outputs() {
        let signing_key = identity_secret(member_id(9));
        let first_outputs = vec![output(
            1,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        )];
        let second_outputs = vec![output(
            1,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        )];
        let mut first_request = MintRequest {
            quote: "quote-nut20".to_string(),
            outputs: first_outputs,
            signature: None,
        };
        let mut second_request = MintRequest {
            quote: "quote-nut20".to_string(),
            outputs: second_outputs,
            signature: None,
        };
        sign_mint_request_for_test(&mut first_request, &signing_key);
        sign_mint_request_for_test(&mut second_request, &signing_key);
        first_request
            .verify_signature(signing_key.public_key())
            .expect("first NUT-20 signature verifies");
        second_request
            .verify_signature(signing_key.public_key())
            .expect("second NUT-20 signature verifies");

        let first = envelope(FederationOperation::Mint(FederationMintOperation {
            request: FederationMintRequest::Single(first_request),
        }));
        let second = envelope(FederationOperation::Mint(FederationMintOperation {
            request: FederationMintRequest::Single(second_request),
        }));
        assert_ne!(
            first.operation_id().expect("first operation id"),
            second.operation_id().expect("second operation id")
        );
        assert_ne!(
            first.canonical_bytes().expect("first canonical bytes"),
            second.canonical_bytes().expect("second canonical bytes")
        );
    }

    #[test]
    fn operation_envelope_rejects_unsupported_version_before_admission() {
        let envelope = envelope(mint_operation(
            "quote-version",
            vec![output(
                1,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        ));

        for found in [0, 1, u16::MAX] {
            let mut unsupported = envelope.clone();
            unsupported.version = found;
            let err = unsupported
                .ensure_admissible_size()
                .expect_err("unsupported version should fail admission");
            assert!(matches!(
                err,
                FederationError::OperationVersionMismatch {
                    expected: FEDERATION_OPERATION_VERSION,
                    found: actual
                } if actual == found
            ));

            let err = unsupported
                .operation_id()
                .expect_err("unsupported version should not hash");
            assert!(matches!(
                err,
                FederationError::OperationVersionMismatch {
                    expected: FEDERATION_OPERATION_VERSION,
                    found: actual
                } if actual == found
            ));
        }
    }

    #[test]
    fn operation_envelope_decode_rejects_unknown_versions_and_malformed_payloads() {
        let envelope = envelope(mint_operation(
            "quote-decode",
            vec![output(
                1,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        ));
        let encoded = serde_json::to_vec(&envelope).expect("operation json");
        FederationOperationEnvelope::from_json_slice(&encoded).expect("valid operation envelope");

        for found in [0, 1, u16::MAX] {
            let mut unsupported = envelope.clone();
            unsupported.version = found;
            let unsupported = serde_json::to_vec(&unsupported).expect("unsupported operation json");
            let err = FederationOperationEnvelope::from_json_slice(&unsupported)
                .expect_err("unsupported version should fail decode");
            assert!(matches!(
                err,
                FederationError::OperationVersionMismatch {
                    expected: FEDERATION_OPERATION_VERSION,
                    found: actual
                } if actual == found
            ));
        }

        for malformed in [
            br#"{"#.as_slice(),
            br#"{"federation_id":"0707070707070707070707070707070707070707070707070707070707070707","version":1}"#.as_slice(),
            br#"{"federation_id":"0707070707070707070707070707070707070707070707070707070707070707","operation":{"type":"unknown","data":{}},"version":1}"#.as_slice(),
        ] {
            assert!(
                FederationOperationEnvelope::from_json_slice(malformed).is_err(),
                "malformed payload should fail decode"
            );
        }
    }

    #[test]
    fn operation_envelope_rejects_removed_and_unknown_v1_fields() {
        let envelope = envelope(FederationOperation::MeltQuotePayment(
            FederationMeltQuotePaymentObservation {
                quote: "quote-strict-v1".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("preimage".to_string()),
            },
        ));

        for (field, removed_value) in [
            ("failure_reason", json!("route failed")),
            ("observed_time", json!(42)),
            ("unknown_v1_field", json!(true)),
        ] {
            let mut value = serde_json::to_value(&envelope).expect("operation json");
            value["operation"]["data"][field] = removed_value;
            let encoded = serde_json::to_vec(&value).expect("operation bytes");
            FederationOperationEnvelope::from_json_slice(&encoded)
                .expect_err("removed operation fields must not survive the v1 cut-through");
        }

        let mut value = serde_json::to_value(&envelope).expect("operation json");
        value["unknown_envelope_field"] = json!(true);
        let encoded = serde_json::to_vec(&value).expect("operation bytes");
        FederationOperationEnvelope::from_json_slice(&encoded)
            .expect_err("unknown envelope fields must fail closed");
    }

    #[test]
    fn operation_envelope_rejects_removed_restore_variant() {
        let envelope = envelope(mint_operation(
            "quote-removed-restore",
            vec![output(
                1,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        ));
        let mut value = serde_json::to_value(&envelope).expect("operation json");
        value["operation"] = json!({
            "type": "restore",
            "data": { "request": { "outputs": [] } }
        });
        let encoded = serde_json::to_vec(&value).expect("operation bytes");

        FederationOperationEnvelope::from_json_slice(&encoded)
            .expect_err("removed restore operation must fail closed");
    }

    #[test]
    fn envelope_size_limit_uses_canonical_bytes() {
        let oversized = oversized_mint_quote_envelope();
        let err = oversized
            .ensure_admissible_size()
            .expect_err("oversized operation should fail");

        assert!(matches!(
            err,
            FederationError::OperationEnvelopeTooLarge { .. }
        ));
    }

    #[test]
    fn mint_outputs_conflict_by_quote_id() {
        let first = envelope(mint_operation(
            "quote-1",
            vec![output(
                1,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        ));
        let duplicate = envelope(mint_operation(
            "quote-1",
            vec![output(
                1,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        ));
        let different_outputs = envelope(mint_operation(
            "quote-1",
            vec![output(
                1,
                "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
            )],
        ));

        assert_eq!(
            first.operation.conflict_keys(),
            duplicate.operation.conflict_keys()
        );
        assert!(!first.conflicts_with(&duplicate).expect("conflict check"));
        assert!(first
            .conflicts_with(&different_outputs)
            .expect("conflict check"));
    }

    #[test]
    fn batch_mint_conflicts_by_each_quote_id() {
        let first = envelope(batch_mint_operation(
            vec!["quote-1", "quote-2"],
            vec![output(
                1,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        ));
        let duplicate = envelope(batch_mint_operation(
            vec!["quote-1", "quote-2"],
            vec![output(
                1,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        ));
        let overlapping_conflict = envelope(batch_mint_operation(
            vec!["quote-2", "quote-3"],
            vec![output(
                1,
                "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
            )],
        ));

        assert_eq!(first.operation.conflict_keys().len(), 2);
        assert!(!first.conflicts_with(&duplicate).expect("conflict check"));
        assert!(first
            .conflicts_with(&overlapping_conflict)
            .expect("conflict check"));
    }

    #[test]
    fn keyset_rotation_validates_v3_bls_keyset() {
        let config = federation_config_with_keyset(1);
        let rotation = keyset_rotation(7);

        rotation
            .validate_against_config(&config)
            .expect("rotation keyset validates against roster");
    }

    #[test]
    fn keyset_rotation_rejects_non_v3_keyset() {
        let config = federation_config_with_keyset(1);
        let mut rotation = keyset_rotation(7);
        rotation.keyset.id = keyset_id();

        let err = rotation
            .validate_against_config(&config)
            .expect_err("non-v3 keyset should be rejected");

        assert!(matches!(
            err,
            FederationError::InvalidFederatedKeysetVersion { .. }
        ));
    }

    #[test]
    fn keyset_rotation_rejects_inactive_keyset() {
        let config = federation_config_with_keyset(1);
        let mut rotation = keyset_rotation(7);
        rotation.keyset.active = false;

        let err = rotation
            .validate_against_config(&config)
            .expect_err("inactive keyset should be rejected");

        assert!(matches!(
            err,
            FederationError::InactiveFederatedKeyset { .. }
        ));
    }

    #[test]
    fn keyset_rotation_conflicts_by_keyset_id() {
        let first = envelope(keyset_rotation_operation(7));
        let duplicate = envelope(keyset_rotation_operation(7));
        let mut changed = keyset_rotation(7);
        changed.keyset.active = !changed.keyset.active;
        let changed = envelope(FederationOperation::KeysetRotation(changed));

        assert_eq!(
            first.operation.conflict_keys(),
            duplicate.operation.conflict_keys()
        );
        assert!(!first.conflicts_with(&duplicate).expect("conflict check"));
        assert!(first.conflicts_with(&changed).expect("conflict check"));
    }

    #[test]
    fn signed_transactions_defer_conflicts_until_signature_validation() {
        let first = envelope(signed_transaction_operation(1));
        let duplicate = envelope(signed_transaction_operation(1));
        let different = envelope(signed_transaction_operation(2));

        assert!(first.operation.conflict_keys().is_empty());
        assert_eq!(
            first.operation.kind(),
            FederationOperationKind::TransactionSigned
        );
        assert!(!first.conflicts_with(&duplicate).expect("conflict check"));
        assert!(!first.conflicts_with(&different).expect("conflict check"));
    }

    #[test]
    fn keyset_rotation_operation_id_tracks_payload() {
        let first = envelope(keyset_rotation_operation(7));
        let duplicate = envelope(keyset_rotation_operation(7));
        let second = envelope(keyset_rotation_operation(8));

        assert_eq!(
            first.operation_id().expect("operation id"),
            duplicate.operation_id().expect("operation id")
        );
        assert_ne!(
            first.operation_id().expect("operation id"),
            second.operation_id().expect("operation id")
        );
    }

    #[test]
    fn canonical_hash_ignores_custom_json_field_order() {
        let request_one = MintQuoteRequest::Custom {
            method: PaymentMethod::Custom("custom".to_string()),
            request: crate::MintQuoteCustomRequest {
                amount: Some(Amount::from(21)),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
                extra: serde_json::from_str(r#"{"b":2,"a":1}"#).expect("extra json"),
            },
        };
        let request_two = MintQuoteRequest::Custom {
            method: PaymentMethod::Custom("custom".to_string()),
            request: crate::MintQuoteCustomRequest {
                amount: Some(Amount::from(21)),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
                extra: serde_json::from_str(r#"{"a":1,"b":2}"#).expect("extra json"),
            },
        };
        let first = envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: "quote-custom".to_string(),
                request_lookup_id: PaymentIdentifier::CustomId("quote-custom".to_string()),
                request: request_one,
                response: MintQuoteResponse::Custom {
                    method: PaymentMethod::Custom("custom".to_string()),
                    response: MintQuoteCustomResponse {
                        quote: "quote-custom".to_string(),
                        request: "custom-payment-request".to_string(),
                        method: PaymentMethod::Custom("custom".to_string()),
                        amount: Some(Amount::from(21)),
                        amount_paid: Amount::ZERO,
                        amount_issued: Amount::ZERO,
                        updated_at: 0,
                        unit: Some(CurrencyUnit::Sat),
                        expiry: None,
                        pubkey: None,
                        extra: serde_json::from_str(r#"{"d":4,"c":3}"#).expect("extra json"),
                    },
                },
            },
        ));
        let second = envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: "quote-custom".to_string(),
                request_lookup_id: PaymentIdentifier::CustomId("quote-custom".to_string()),
                request: request_two,
                response: MintQuoteResponse::Custom {
                    method: PaymentMethod::Custom("custom".to_string()),
                    response: MintQuoteCustomResponse {
                        quote: "quote-custom".to_string(),
                        request: "custom-payment-request".to_string(),
                        method: PaymentMethod::Custom("custom".to_string()),
                        amount: Some(Amount::from(21)),
                        amount_paid: Amount::ZERO,
                        amount_issued: Amount::ZERO,
                        updated_at: 0,
                        unit: Some(CurrencyUnit::Sat),
                        expiry: None,
                        pubkey: None,
                        extra: serde_json::from_str(r#"{"c":3,"d":4}"#).expect("extra json"),
                    },
                },
            },
        ));

        assert_eq!(
            first.canonical_bytes().expect("canonical bytes"),
            second.canonical_bytes().expect("canonical bytes")
        );
        assert_eq!(
            first.operation_id().expect("operation id"),
            second.operation_id().expect("operation id")
        );
    }

    #[test]
    fn operation_id_tracks_input_witness_mutation() {
        let mut first_proof = proof(10, "witness-bound-secret");
        first_proof.witness = Some(p2pk_witness("11"));
        let mut second_proof = first_proof.clone();
        second_proof.witness = Some(p2pk_witness("22"));

        let outputs = vec![output(
            10,
            "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39",
        )];
        let first = envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![first_proof], outputs.clone()),
        }));
        let second = envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![second_proof], outputs),
        }));

        assert_ne!(
            first.canonical_bytes().expect("canonical bytes"),
            second.canonical_bytes().expect("canonical bytes")
        );
        assert_ne!(
            first.operation_id().expect("operation id"),
            second.operation_id().expect("operation id")
        );
    }

    #[test]
    fn operation_id_tracks_p2pk_locked_witness_mutation() {
        let (mut first_proof, _) = p2pk_locked_proof(10, SigFlag::SigInputs);
        first_proof.witness = Some(p2pk_witness("33"));
        let mut second_proof = first_proof.clone();
        second_proof.witness = Some(p2pk_witness("44"));

        let outputs = vec![output(
            10,
            "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39",
        )];
        let first = envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![first_proof], outputs.clone()),
        }));
        let second = envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![second_proof], outputs),
        }));

        assert_ne!(
            first.canonical_bytes().expect("canonical bytes"),
            second.canonical_bytes().expect("canonical bytes")
        );
        assert_ne!(
            first.operation_id().expect("operation id"),
            second.operation_id().expect("operation id")
        );
    }

    #[test]
    fn operation_id_tracks_htlc_locked_witness_mutation() {
        let mut first_proof = htlc_locked_proof(10, 0x42, SigFlag::SigInputs);
        first_proof.witness = Some(htlc_witness(0x42, "55"));
        let mut second_proof = first_proof.clone();
        second_proof.witness = Some(htlc_witness(0x43, "66"));

        let outputs = vec![output(
            10,
            "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39",
        )];
        let first = envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![first_proof], outputs.clone()),
        }));
        let second = envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![second_proof], outputs),
        }));

        assert_ne!(
            first.canonical_bytes().expect("canonical bytes"),
            second.canonical_bytes().expect("canonical bytes")
        );
        assert_ne!(
            first.operation_id().expect("operation id"),
            second.operation_id().expect("operation id")
        );
    }

    #[test]
    fn sig_all_signature_and_operation_id_track_exact_swap_outputs() {
        let (proof, signing_key) = p2pk_locked_proof(10, SigFlag::SigAll);
        let first_outputs = vec![output(
            10,
            "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39",
        )];
        let second_outputs = vec![output(
            10,
            "022d000b4bab1f64ba07c940fdc43ecfe45a94cc574023c3578f17ea635addb437",
        )];
        let mut first_request = SwapRequest::new(vec![proof.clone()], first_outputs);
        let mut second_request = SwapRequest::new(vec![proof], second_outputs);

        first_request
            .sign_sig_all(signing_key.clone())
            .expect("first sig_all signature");
        second_request
            .sign_sig_all(signing_key)
            .expect("second sig_all signature");

        assert_ne!(
            first_request.inputs()[0]
                .witness
                .as_ref()
                .and_then(Witness::signatures)
                .expect("first sig_all witness"),
            second_request.inputs()[0]
                .witness
                .as_ref()
                .and_then(Witness::signatures)
                .expect("second sig_all witness")
        );

        let first = envelope(FederationOperation::Swap(FederationSwapOperation {
            request: first_request,
        }));
        let second = envelope(FederationOperation::Swap(FederationSwapOperation {
            request: second_request,
        }));

        assert_ne!(
            first.canonical_bytes().expect("canonical bytes"),
            second.canonical_bytes().expect("canonical bytes")
        );
        assert_ne!(
            first.operation_id().expect("operation id"),
            second.operation_id().expect("operation id")
        );
    }

    #[test]
    fn payment_observations_allow_state_progression() {
        let pending = envelope(FederationOperation::MeltQuotePayment(
            FederationMeltQuotePaymentObservation {
                quote: "melt-quote".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Custom("custom".to_string()),
                state: MeltQuoteState::Pending,
                amount: Amount::from(10),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
            },
        ));
        let paid = envelope(FederationOperation::MeltQuotePayment(
            FederationMeltQuotePaymentObservation {
                quote: "melt-quote".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Custom("custom".to_string()),
                state: MeltQuoteState::Paid,
                amount: Amount::from(10),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("preimage".to_string()),
            },
        ));
        let conflicting_paid = envelope(FederationOperation::MeltQuotePayment(
            FederationMeltQuotePaymentObservation {
                quote: "melt-quote".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Custom("custom".to_string()),
                state: MeltQuoteState::Paid,
                amount: Amount::from(10),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("different-preimage".to_string()),
            },
        ));

        assert!(!pending.conflicts_with(&paid).expect("conflict check"));
        assert!(paid
            .conflicts_with(&conflicting_paid)
            .expect("conflict check"));
    }

    #[test]
    fn payment_observations_are_member_scoped_for_quorum() {
        let first = FederationMintQuotePaymentObservation {
            quote: "mint-quote".to_string(),
            observer: member_id(1),
            method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(10),
            amount_issued: Amount::ZERO,
            payment_id: Some(PaymentIdentifier::CustomId("payment-a".to_string())),
            payment_proof: Some("settled".to_string()),
        };
        let matching_second = FederationMintQuotePaymentObservation {
            observer: member_id(2),
            ..first.clone()
        };
        let conflicting_third = FederationMintQuotePaymentObservation {
            observer: member_id(3),
            amount_paid: Amount::from(11),
            payment_proof: Some("different-settlement".to_string()),
            ..first.clone()
        };
        let conflicting_first = FederationMintQuotePaymentObservation {
            amount_paid: Amount::from(12),
            payment_proof: Some("same-member-different-settlement".to_string()),
            ..first.clone()
        };
        let different_payment_id = FederationMintQuotePaymentObservation {
            observer: member_id(4),
            payment_id: Some(PaymentIdentifier::CustomId("payment-b".to_string())),
            ..first.clone()
        };

        let first_envelope = envelope(FederationOperation::MintQuotePayment(first.clone()));
        let matching_second_envelope = envelope(FederationOperation::MintQuotePayment(
            matching_second.clone(),
        ));
        let conflicting_third_envelope = envelope(FederationOperation::MintQuotePayment(
            conflicting_third.clone(),
        ));
        let conflicting_first_envelope = envelope(FederationOperation::MintQuotePayment(
            conflicting_first.clone(),
        ));

        assert!(!first_envelope
            .conflicts_with(&matching_second_envelope)
            .expect("different observers can report matching evidence"));
        assert!(!first_envelope
            .conflicts_with(&conflicting_third_envelope)
            .expect("different observers are accumulated, not journal-conflicted"));
        assert!(first_envelope
            .conflicts_with(&conflicting_first_envelope)
            .expect("same observer conflicting evidence is rejected"));

        let single = first.quorum(std::iter::empty(), 2);
        assert!(!single.is_reached());
        assert_eq!(single.matching_count(), 1);
        assert!(!single.has_conflicts());

        assert!(first.matches_payment_evidence(&matching_second));
        assert!(!first.matches_payment_evidence(&different_payment_id));

        let quorum = first.quorum(
            [&matching_second, &conflicting_third, &different_payment_id],
            2,
        );
        assert!(quorum.is_reached());
        assert!(quorum.has_conflicts());
        assert_eq!(quorum.matching_observers, vec![member_id(1), member_id(2)]);
        assert_eq!(
            quorum.conflicting_observers,
            vec![member_id(3), member_id(4)]
        );
    }

    #[test]
    fn payment_observation_operation_ids_track_exact_evidence() {
        let first_mint = FederationMintQuotePaymentObservation {
            quote: "mint-quote".to_string(),
            observer: member_id(1),
            method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(10),
            amount_issued: Amount::ZERO,
            payment_id: Some(PaymentIdentifier::CustomId("payment-a".to_string())),
            payment_proof: Some("settled".to_string()),
        };
        let duplicate_mint = first_mint.clone();
        let conflicting_mint = FederationMintQuotePaymentObservation {
            payment_proof: Some("different-settlement".to_string()),
            ..first_mint.clone()
        };

        let first_mint_envelope = envelope(FederationOperation::MintQuotePayment(first_mint));
        let duplicate_mint_envelope =
            envelope(FederationOperation::MintQuotePayment(duplicate_mint));
        let conflicting_mint_envelope =
            envelope(FederationOperation::MintQuotePayment(conflicting_mint));

        assert_eq!(
            first_mint_envelope
                .canonical_bytes()
                .expect("first mint canonical bytes"),
            duplicate_mint_envelope
                .canonical_bytes()
                .expect("duplicate mint canonical bytes")
        );
        assert_eq!(
            first_mint_envelope.operation_id().expect("first mint id"),
            duplicate_mint_envelope
                .operation_id()
                .expect("duplicate mint id")
        );
        assert!(!first_mint_envelope
            .conflicts_with(&duplicate_mint_envelope)
            .expect("duplicate mint conflict check"));
        assert_ne!(
            first_mint_envelope.operation_id().expect("first mint id"),
            conflicting_mint_envelope
                .operation_id()
                .expect("conflicting mint id")
        );
        assert!(first_mint_envelope
            .conflicts_with(&conflicting_mint_envelope)
            .expect("conflicting mint conflict check"));

        let mut journal = InMemoryFederationJournal::new();
        let first_entry = journal
            .append(first_mint_envelope)
            .expect("append first mint observation")
            .entry()
            .clone();
        let duplicate_entry = journal
            .append(duplicate_mint_envelope)
            .expect("append duplicate mint observation")
            .entry()
            .clone();

        assert_eq!(first_entry.operation_id, duplicate_entry.operation_id);
        assert_eq!(journal.entries().len(), 1);

        let first_melt = FederationMeltQuotePaymentObservation {
            quote: "melt-quote".to_string(),
            observer: member_id(1),
            method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
            state: MeltQuoteState::Paid,
            amount: Amount::from(10),
            fee_reserve: Amount::from(1),
            payment_id: Some(PaymentIdentifier::CustomId("payment-b".to_string())),
            payment_proof: Some("preimage".to_string()),
        };
        let duplicate_melt = first_melt.clone();
        let conflicting_melt = FederationMeltQuotePaymentObservation {
            payment_proof: Some("different-preimage".to_string()),
            ..first_melt.clone()
        };

        let first_melt_envelope = envelope(FederationOperation::MeltQuotePayment(first_melt));
        let duplicate_melt_envelope =
            envelope(FederationOperation::MeltQuotePayment(duplicate_melt));
        let conflicting_melt_envelope =
            envelope(FederationOperation::MeltQuotePayment(conflicting_melt));

        assert_eq!(
            first_melt_envelope
                .canonical_bytes()
                .expect("first melt canonical bytes"),
            duplicate_melt_envelope
                .canonical_bytes()
                .expect("duplicate melt canonical bytes")
        );
        assert_eq!(
            first_melt_envelope.operation_id().expect("first melt id"),
            duplicate_melt_envelope
                .operation_id()
                .expect("duplicate melt id")
        );
        assert!(!first_melt_envelope
            .conflicts_with(&duplicate_melt_envelope)
            .expect("duplicate melt conflict check"));
        assert_ne!(
            first_melt_envelope.operation_id().expect("first melt id"),
            conflicting_melt_envelope
                .operation_id()
                .expect("conflicting melt id")
        );
        assert!(first_melt_envelope
            .conflicts_with(&conflicting_melt_envelope)
            .expect("conflicting melt conflict check"));
    }

    #[test]
    fn payment_observation_report_flags_conflicts_and_unresolved_evidence() {
        let first = payment_record(
            0,
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "mint-quote".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(10),
                amount_issued: Amount::ZERO,
                payment_id: None,
                payment_proof: Some("settled".to_string()),
            }),
        );
        let matching_second = payment_record(
            1,
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "mint-quote".to_string(),
                observer: member_id(2),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(10),
                amount_issued: Amount::ZERO,
                payment_id: None,
                payment_proof: Some("settled".to_string()),
            }),
        );
        let conflicting_third = payment_record(
            2,
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "mint-quote".to_string(),
                observer: member_id(3),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(11),
                amount_issued: Amount::ZERO,
                payment_id: None,
                payment_proof: Some("different-settlement".to_string()),
            }),
        );

        let report = FederationPaymentObservationReport::from_records(
            FederationPaymentObservationContext::MintQuote,
            "mint-quote",
            2,
            &[
                first.clone(),
                matching_second.clone(),
                conflicting_third.clone(),
            ],
        );
        assert_eq!(
            report.status,
            FederationPaymentObservationReportStatus::Conflicting
        );
        assert!(report.has_conflicts());
        assert_eq!(report.total_observations, 3);
        assert_eq!(report.groups.len(), 2);
        assert_eq!(report.groups[0].observers, vec![member_id(1), member_id(2)]);
        assert!(report.groups[0].quorum_reached);
        assert!(!report.has_competing_quorums());
        assert!(!report.is_unresolved());
        assert_eq!(report.groups[0].conflicting_observers, vec![member_id(3)]);
        assert_eq!(
            report.conflicting_observers,
            vec![member_id(1), member_id(2), member_id(3)]
        );

        let unresolved = FederationPaymentObservationReport::from_records(
            FederationPaymentObservationContext::MintQuote,
            "mint-quote",
            2,
            std::slice::from_ref(&first),
        );
        assert_eq!(
            unresolved.status,
            FederationPaymentObservationReportStatus::AwaitingQuorum
        );
        assert!(unresolved.is_unresolved());

        let quorum = FederationPaymentObservationReport::from_records(
            FederationPaymentObservationContext::MintQuote,
            "mint-quote",
            2,
            std::slice::from_ref(&matching_second),
        );
        assert_eq!(
            quorum.status,
            FederationPaymentObservationReportStatus::AwaitingQuorum
        );

        let summary = report.summary();
        assert_eq!(
            summary.status,
            FederationPaymentObservationReportStatus::Conflicting
        );
        assert_eq!(summary.groups[0].observer_count, 2);
        assert_eq!(summary.groups[0].state, "PAID");
        assert_eq!(summary.groups[0].operation_indices, vec![0, 1]);
        assert!(!summary.stale);
        assert_eq!(
            summary.recommended_action,
            FederationPaymentObservationReportAction::Inspect
        );
        let summary_json = serde_json::to_string(&summary).expect("summary json");
        assert!(!summary_json.contains("settled"));
        assert!(!summary_json.contains("different-settlement"));

        let conflicting_fourth = payment_record(
            3,
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                observer: member_id(4),
                ..match &conflicting_third.observation {
                    FederationPaymentObservation::MintQuote(observation) => observation.clone(),
                    FederationPaymentObservation::MeltQuote(_) => {
                        panic!("expected mint observation")
                    }
                }
            }),
        );
        let competing_quorums = FederationPaymentObservationReport::from_records(
            FederationPaymentObservationContext::MintQuote,
            "mint-quote",
            2,
            &[
                first.clone(),
                matching_second,
                conflicting_third,
                conflicting_fourth,
            ],
        );
        assert!(competing_quorums.has_competing_quorums());
        assert!(competing_quorums.is_unresolved());

        let stale_unresolved = unresolved.summary_with_quote_age(Some(3_601), 3_600);
        assert!(stale_unresolved.stale);
        assert_eq!(stale_unresolved.quote_age_secs, Some(3_601));
        assert_eq!(
            stale_unresolved.recommended_action,
            FederationPaymentObservationReportAction::Inspect
        );

        let fresh_unresolved = unresolved.summary_with_quote_age(Some(30), 3_600);
        assert!(!fresh_unresolved.stale);
        assert_eq!(
            fresh_unresolved.recommended_action,
            FederationPaymentObservationReportAction::None
        );
    }

    #[test]
    fn payment_observation_report_allows_mint_unpaid_to_paid_progression() {
        let unpaid = payment_record(
            0,
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "mint-quote".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MintQuoteState::Unpaid,
                amount_paid: Amount::ZERO,
                amount_issued: Amount::ZERO,
                payment_id: None,
                payment_proof: None,
            }),
        );
        let paid = payment_record(
            1,
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "mint-quote".to_string(),
                observer: member_id(2),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(10),
                amount_issued: Amount::ZERO,
                payment_id: None,
                payment_proof: Some("settled".to_string()),
            }),
        );

        let report = FederationPaymentObservationReport::from_records(
            FederationPaymentObservationContext::MintQuote,
            "mint-quote",
            2,
            &[unpaid, paid],
        );
        assert_eq!(
            report.status,
            FederationPaymentObservationReportStatus::AwaitingQuorum
        );
        assert!(report.conflicting_observers.is_empty());
        assert!(report.competing_terminal_state_observers.is_empty());
    }

    #[test]
    fn payment_observation_report_flags_competing_terminal_states() {
        let pending = payment_record(
            0,
            FederationOperation::MeltQuotePayment(FederationMeltQuotePaymentObservation {
                quote: "melt-quote".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Custom("custom".to_string()),
                state: MeltQuoteState::Pending,
                amount: Amount::from(10),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
            }),
        );
        let paid = payment_record(
            1,
            FederationOperation::MeltQuotePayment(FederationMeltQuotePaymentObservation {
                quote: "melt-quote".to_string(),
                observer: member_id(2),
                method: PaymentMethod::Custom("custom".to_string()),
                state: MeltQuoteState::Paid,
                amount: Amount::from(10),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("preimage".to_string()),
            }),
        );
        let failed = payment_record(
            2,
            FederationOperation::MeltQuotePayment(FederationMeltQuotePaymentObservation {
                quote: "melt-quote".to_string(),
                observer: member_id(3),
                method: PaymentMethod::Custom("custom".to_string()),
                state: MeltQuoteState::Failed,
                amount: Amount::from(10),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
            }),
        );

        let progression = FederationPaymentObservationReport::from_records(
            FederationPaymentObservationContext::MeltQuote,
            "melt-quote",
            1,
            &[pending.clone(), paid.clone()],
        );
        assert_eq!(
            progression.status,
            FederationPaymentObservationReportStatus::QuorumReached
        );
        assert!(progression.competing_terminal_state_observers.is_empty());

        let conflict = FederationPaymentObservationReport::from_records(
            FederationPaymentObservationContext::MeltQuote,
            "melt-quote",
            1,
            &[pending, paid, failed],
        );
        assert_eq!(
            conflict.status,
            FederationPaymentObservationReportStatus::Conflicting
        );
        assert_eq!(
            conflict.competing_terminal_state_observers,
            vec![member_id(2), member_id(3)]
        );
        assert_eq!(
            conflict.conflicting_observers,
            vec![member_id(2), member_id(3)]
        );
    }

    #[test]
    fn operation_kind_and_keys_cover_core_flows() {
        let mint_quote = FederationOperation::MintQuote(FederationMintQuoteOperation {
            quote: "mint-quote".to_string(),
            request_lookup_id: PaymentIdentifier::CustomId("mint-quote".to_string()),
            request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(10),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }),
            response: bolt11_quote_response("mint-quote", 10),
        });
        let mint_payment =
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "mint-quote".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(10),
                amount_issued: Amount::ZERO,
                payment_id: None,
                payment_proof: Some("settled".to_string()),
            });
        let mint = mint_operation(
            "mint-quote",
            vec![output(
                10,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        );
        let request = MeltQuoteRequest::Custom(MeltQuoteCustomRequest {
            method: "custom".to_string(),
            request: "payreq".to_string(),
            unit: CurrencyUnit::Sat,
            amount: None,
            extra: json!({"route": "fake"}),
        });
        let melt_quote =
            FederationOperation::MeltQuote(test_melt_quote_operation("melt-quote", request));
        let melt = FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(
                "melt-quote".to_string(),
                vec![proof(11, "secret-1")],
                Some(vec![output(
                    1,
                    "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
                )]),
            ),
        });
        let swap = FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(
                vec![proof(10, "secret-2")],
                vec![output(
                    10,
                    "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39",
                )],
            ),
        });
        let rotation = keyset_rotation_operation(7);

        let operations = [
            (
                mint_quote,
                FederationOperationKind::MintQuote,
                Some("mint-quote"),
            ),
            (
                mint_payment,
                FederationOperationKind::MintQuotePayment,
                Some("mint-quote"),
            ),
            (mint, FederationOperationKind::Mint, Some("mint-quote")),
            (
                melt_quote,
                FederationOperationKind::MeltQuote,
                Some("melt-quote"),
            ),
            (melt, FederationOperationKind::Melt, Some("melt-quote")),
            (swap, FederationOperationKind::Swap, None),
            (rotation, FederationOperationKind::KeysetRotation, None),
        ];

        for (operation, expected_kind, expected_quote) in operations {
            assert_eq!(operation.kind(), expected_kind);
            assert_eq!(operation.quote(), expected_quote);
            envelope(operation).operation_id().expect("operation id");
        }
    }

    #[test]
    fn operation_balance_audit_summarizes_core_value_flows() {
        let mint_payment =
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "mint-quote".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(10),
                amount_issued: Amount::from(4),
                payment_id: None,
                payment_proof: Some("settled".to_string()),
            });
        let audit = mint_payment.balance_audit().expect("mint payment audit");
        assert_eq!(
            audit.operation_kind,
            FederationOperationKind::MintQuotePayment
        );
        assert_eq!(audit.assets, Amount::from(10));
        assert_eq!(audit.liabilities, Amount::from(4));
        assert_eq!(audit.remainder, Amount::from(6));

        let swap = FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(
                vec![proof(8, "swap-input")],
                vec![output(
                    5,
                    "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
                )],
            ),
        });
        let audit = swap.balance_audit().expect("swap audit");
        assert_eq!(audit.operation_kind, FederationOperationKind::Swap);
        assert_eq!(audit.assets, Amount::from(8));
        assert_eq!(audit.liabilities, Amount::from(5));
        assert_eq!(audit.remainder, Amount::from(3));

        let melt = FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(
                "melt-quote".to_string(),
                vec![proof(11, "melt-input")],
                Some(vec![output(
                    2,
                    "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
                )]),
            ),
        });
        let audit = melt.balance_audit().expect("melt audit");
        assert_eq!(audit.operation_kind, FederationOperationKind::Melt);
        assert_eq!(audit.assets, Amount::from(11));
        assert_eq!(audit.liabilities, Amount::from(2));
        assert_eq!(audit.remainder, Amount::from(9));

        let melt_payment =
            FederationOperation::MeltQuotePayment(FederationMeltQuotePaymentObservation {
                quote: "melt-quote".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MeltQuoteState::Paid,
                amount: Amount::from(7),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("preimage".to_string()),
            });
        let audit = melt_payment.balance_audit().expect("melt payment audit");
        assert_eq!(
            audit.operation_kind,
            FederationOperationKind::MeltQuotePayment
        );
        assert_eq!(audit.external_payment, Amount::from(7));
        assert_eq!(audit.fee_reserve, Amount::from(1));
    }

    #[test]
    fn operation_balance_audit_rejects_negative_payment_observation() {
        let operation =
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "mint-quote".to_string(),
                observer: member_id(1),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(4),
                amount_issued: Amount::from(5),
                payment_id: None,
                payment_proof: Some("settled".to_string()),
            });

        assert!(matches!(
            operation.balance_audit(),
            Err(FederationError::NegativeBalanceAudit {
                operation_kind: FederationOperationKind::MintQuotePayment,
                assets,
                liabilities,
            }) if assets == Amount::from(4) && liabilities == Amount::from(5)
        ));
    }

    #[test]
    fn journal_append_rejects_negative_swap_balance_audit() {
        let mut journal = InMemoryFederationJournal::new();
        let operation = FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(
                vec![proof(3, "swap-input")],
                vec![output(
                    5,
                    "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
                )],
            ),
        });

        assert!(matches!(
            journal.append(envelope(operation)),
            Err(FederationError::NegativeBalanceAudit {
                operation_kind: FederationOperationKind::Swap,
                assets,
                liabilities,
            }) if assets == Amount::from(3) && liabilities == Amount::from(5)
        ));
        assert_eq!(journal.next_index(), 0);
    }

    #[test]
    fn journal_append_rejects_negative_melt_change_balance_audit() {
        let mut journal = InMemoryFederationJournal::new();
        let operation = FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(
                "melt-quote".to_string(),
                vec![proof(3, "melt-input")],
                Some(vec![output(
                    4,
                    "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
                )]),
            ),
        });

        assert!(matches!(
            journal.append(envelope(operation)),
            Err(FederationError::NegativeBalanceAudit {
                operation_kind: FederationOperationKind::Melt,
                assets,
                liabilities,
            }) if assets == Amount::from(3) && liabilities == Amount::from(4)
        ));
        assert_eq!(journal.next_index(), 0);
    }
}
