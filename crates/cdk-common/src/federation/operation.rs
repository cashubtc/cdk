//! Canonical federation operation envelopes.
//!
//! Federation consensus orders these envelopes, not ad hoc HTTP handler
//! side effects. Keeping the operation payloads separate from the Cashu REST
//! API lets standalone mints keep their existing protocol surface while
//! federation members agree on the exact mint, swap, melt, restore, and
//! payment-observation requests before signing or mutating spend state.

use core::fmt;
use core::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{FederationError, FederationId, Result};
use crate::{
    Amount, BatchMintRequest, MeltQuoteRequest, MeltQuoteState, MeltRequest, MintQuoteRequest,
    MintQuoteState, MintRequest, PaymentMethod, RestoreRequest, SwapRequest,
};

const FEDERATION_OPERATION_HASH_VERSION: &[u8] = b"cdk-federation-operation-v1";

/// Version of the federation operation envelope format.
pub const FEDERATION_OPERATION_VERSION: u16 = 1;

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
    /// Recreate signatures for previously signed blinded outputs.
    Restore,
}

/// Semantic key used to reject mix-and-match duplicates before signing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum FederationOperationConflictKey {
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
    /// A mint quote may only have one observation for the same state.
    MintQuotePayment {
        /// Quote ID being observed.
        quote: String,
        /// Observed mint quote state.
        state: MintQuoteState,
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
    /// A melt quote may only have one observation for the same state.
    MeltQuotePayment {
        /// Quote ID being observed.
        quote: String,
        /// Observed melt quote state.
        state: MeltQuoteState,
    },
}

/// Proposed mint quote creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationMintQuoteOperation {
    /// Quote ID assigned by the proposing member.
    pub quote: String,
    /// Wallet's quote request.
    pub request: MintQuoteRequest,
}

/// Observed incoming mint quote payment state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationMintQuotePaymentObservation {
    /// Quote ID being observed.
    pub quote: String,
    /// Payment method for the quote.
    pub method: PaymentMethod,
    /// Observed quote state.
    pub state: MintQuoteState,
    /// Total amount observed as paid.
    pub amount_paid: Amount,
    /// Amount already issued for the quote.
    pub amount_issued: Amount,
    /// Optional method-specific proof or settlement reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_proof: Option<String>,
}

/// Request to sign blinded outputs for a paid mint quote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationMintOperation {
    /// Original wallet mint request.
    pub request: FederationMintRequest,
}

/// Wallet mint request variants represented in federation consensus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
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
pub struct FederationMeltQuoteOperation {
    /// Quote ID assigned by the proposing member.
    pub quote: String,
    /// Wallet's quote request.
    pub request: MeltQuoteRequest,
}

/// Observed outgoing melt payment state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationMeltQuotePaymentObservation {
    /// Quote ID being observed.
    pub quote: String,
    /// Payment method for the quote.
    pub method: PaymentMethod,
    /// Observed quote state.
    pub state: MeltQuoteState,
    /// Amount intended for the external payment.
    pub amount: Amount,
    /// Fee reserve associated with the quote.
    pub fee_reserve: Amount,
    /// Optional method-specific proof, preimage, outpoint, or settlement reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_proof: Option<String>,
    /// Optional failure reason when the observation records a failed payment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

/// Request to spend proofs against a melt quote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationMeltOperation {
    /// Original wallet melt request.
    pub request: MeltRequest<String>,
}

/// Request to spend proofs and sign replacement outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationSwapOperation {
    /// Original wallet swap request.
    pub request: SwapRequest,
}

/// Request to restore signatures for blinded outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationRestoreOperation {
    /// Original wallet restore request.
    pub request: RestoreRequest,
}

/// Consensus operation payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum FederationOperation {
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
    /// Recreate signatures for previously signed blinded outputs.
    Restore(FederationRestoreOperation),
}

impl FederationOperation {
    /// Return the operation kind.
    #[must_use]
    pub fn kind(&self) -> FederationOperationKind {
        match self {
            Self::MintQuote(_) => FederationOperationKind::MintQuote,
            Self::MintQuotePayment(_) => FederationOperationKind::MintQuotePayment,
            Self::Mint(_) => FederationOperationKind::Mint,
            Self::MeltQuote(_) => FederationOperationKind::MeltQuote,
            Self::MeltQuotePayment(_) => FederationOperationKind::MeltQuotePayment,
            Self::Melt(_) => FederationOperationKind::Melt,
            Self::Swap(_) => FederationOperationKind::Swap,
            Self::Restore(_) => FederationOperationKind::Restore,
        }
    }

    /// Return the conflict key that must be unique for this operation.
    #[must_use]
    pub fn conflict_key(&self) -> Option<FederationOperationConflictKey> {
        self.conflict_keys().into_iter().next()
    }

    /// Return all conflict keys that must be unique for this operation.
    #[must_use]
    pub fn conflict_keys(&self) -> Vec<FederationOperationConflictKey> {
        match self {
            Self::MintQuote(operation) => vec![FederationOperationConflictKey::MintQuote {
                quote: operation.quote.clone(),
            }],
            Self::MintQuotePayment(observation) => {
                vec![FederationOperationConflictKey::MintQuotePayment {
                    quote: observation.quote.clone(),
                    state: observation.state,
                }]
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
                }]
            }
            Self::Melt(operation) => vec![FederationOperationConflictKey::Melt {
                quote: operation.request.quote().clone(),
            }],
            Self::Swap(_) | Self::Restore(_) => Vec::new(),
        }
    }

    /// Return the quote ID carried by quote-scoped operations.
    #[must_use]
    pub fn quote(&self) -> Option<&str> {
        match self {
            Self::MintQuote(operation) => Some(operation.quote.as_str()),
            Self::MintQuotePayment(observation) => Some(observation.quote.as_str()),
            Self::Mint(operation) => operation.request.quote_ids().into_iter().next(),
            Self::MeltQuote(operation) => Some(operation.quote.as_str()),
            Self::MeltQuotePayment(observation) => Some(observation.quote.as_str()),
            Self::Melt(operation) => Some(operation.request.quote().as_str()),
            Self::Swap(_) | Self::Restore(_) => None,
        }
    }
}

/// Versioned envelope that binds an operation to one federation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationOperationEnvelope {
    /// Operation envelope version.
    pub version: u16,
    /// Federation whose members must order this operation.
    pub federation_id: FederationId,
    /// Operation payload.
    pub operation: FederationOperation,
}

impl FederationOperationEnvelope {
    /// Create a new operation envelope for the current operation format.
    #[must_use]
    pub fn new(federation_id: FederationId, operation: FederationOperation) -> Self {
        Self {
            version: FEDERATION_OPERATION_VERSION,
            federation_id,
            operation,
        }
    }

    /// Return canonical JSON bytes with all object keys sorted.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self)
    }

    /// Compute the domain-separated operation ID.
    pub fn operation_id(&self) -> Result<FederationOperationId> {
        let canonical = self.canonical_bytes()?;
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, FEDERATION_OPERATION_HASH_VERSION);
        append_hash_bytes(&mut transcript, &canonical);
        Ok(FederationOperationId::from_bytes(
            Sha256Hash::hash(&transcript).to_byte_array(),
        ))
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
    use std::str::FromStr;

    use serde_json::json;

    use super::*;
    use crate::{
        BatchMintRequest, BlindedMessage, CurrencyUnit, Id, MeltQuoteCustomRequest,
        MintQuoteBolt11Request, Proof, PublicKey,
    };

    fn federation_id() -> FederationId {
        FederationId::from_bytes([7; 32])
    }

    fn keyset_id() -> Id {
        Id::from_str("00bfa73302d12ffd").expect("keyset id")
    }

    fn public_key(hex: &str) -> PublicKey {
        PublicKey::from_hex(hex).expect("public key")
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

    fn mint_operation(quote: &str, outputs: Vec<BlindedMessage>) -> FederationOperation {
        FederationOperation::Mint(FederationMintOperation {
            request: FederationMintRequest::Single(MintRequest {
                quote: quote.to_string(),
                outputs,
                signature: None,
            }),
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
            first.operation.conflict_key(),
            duplicate.operation.conflict_key()
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
    fn canonical_hash_ignores_custom_json_field_order() {
        let request_one = MintQuoteRequest::Custom {
            method: PaymentMethod::Custom("custom".to_string()),
            request: crate::MintQuoteCustomRequest {
                amount: Amount::from(21),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
                extra: serde_json::from_str(r#"{"b":2,"a":1}"#).expect("extra json"),
            },
        };
        let request_two = MintQuoteRequest::Custom {
            method: PaymentMethod::Custom("custom".to_string()),
            request: crate::MintQuoteCustomRequest {
                amount: Amount::from(21),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
                extra: serde_json::from_str(r#"{"a":1,"b":2}"#).expect("extra json"),
            },
        };
        let first = envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: "quote-custom".to_string(),
                request: request_one,
            },
        ));
        let second = envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: "quote-custom".to_string(),
                request: request_two,
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
    fn payment_observations_allow_state_progression() {
        let pending = envelope(FederationOperation::MeltQuotePayment(
            FederationMeltQuotePaymentObservation {
                quote: "melt-quote".to_string(),
                method: PaymentMethod::Custom("custom".to_string()),
                state: MeltQuoteState::Pending,
                amount: Amount::from(10),
                fee_reserve: Amount::from(1),
                payment_proof: None,
                failure_reason: None,
            },
        ));
        let paid = envelope(FederationOperation::MeltQuotePayment(
            FederationMeltQuotePaymentObservation {
                quote: "melt-quote".to_string(),
                method: PaymentMethod::Custom("custom".to_string()),
                state: MeltQuoteState::Paid,
                amount: Amount::from(10),
                fee_reserve: Amount::from(1),
                payment_proof: Some("preimage".to_string()),
                failure_reason: None,
            },
        ));
        let conflicting_paid = envelope(FederationOperation::MeltQuotePayment(
            FederationMeltQuotePaymentObservation {
                quote: "melt-quote".to_string(),
                method: PaymentMethod::Custom("custom".to_string()),
                state: MeltQuoteState::Paid,
                amount: Amount::from(10),
                fee_reserve: Amount::from(1),
                payment_proof: Some("different-preimage".to_string()),
                failure_reason: None,
            },
        ));

        assert!(!pending.conflicts_with(&paid).expect("conflict check"));
        assert!(paid
            .conflicts_with(&conflicting_paid)
            .expect("conflict check"));
    }

    #[test]
    fn operation_kind_and_keys_cover_core_flows() {
        let mint_quote = FederationOperation::MintQuote(FederationMintQuoteOperation {
            quote: "mint-quote".to_string(),
            request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(10),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }),
        });
        let mint_payment =
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "mint-quote".to_string(),
                method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(10),
                amount_issued: Amount::ZERO,
                payment_proof: Some("settled".to_string()),
            });
        let mint = mint_operation(
            "mint-quote",
            vec![output(
                10,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            )],
        );
        let melt_quote = FederationOperation::MeltQuote(FederationMeltQuoteOperation {
            quote: "melt-quote".to_string(),
            request: MeltQuoteRequest::Custom(MeltQuoteCustomRequest {
                method: "custom".to_string(),
                request: "payreq".to_string(),
                unit: CurrencyUnit::Sat,
                extra: json!({"route": "fake"}),
            }),
        });
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
        let restore = FederationOperation::Restore(FederationRestoreOperation {
            request: RestoreRequest {
                outputs: vec![output(
                    10,
                    "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39",
                )],
            },
        });

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
            (restore, FederationOperationKind::Restore, None),
        ];

        for (operation, expected_kind, expected_quote) in operations {
            assert_eq!(operation.kind(), expected_kind);
            assert_eq!(operation.quote(), expected_quote);
            envelope(operation).operation_id().expect("operation id");
        }
    }
}
