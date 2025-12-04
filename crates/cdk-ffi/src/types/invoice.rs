//! Invoice decoding FFI types and functions

use serde::{Deserialize, Serialize};

use crate::error::FfiError;

/// Type of Lightning payment request
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum)]
pub enum PaymentType {
    /// Bolt11 invoice
    Bolt11,
    /// Bolt12 offer
    Bolt12,
}

impl From<cdk::invoice::PaymentType> for PaymentType {
    fn from(payment_type: cdk::invoice::PaymentType) -> Self {
        match payment_type {
            cdk::invoice::PaymentType::Bolt11 => Self::Bolt11,
            cdk::invoice::PaymentType::Bolt12 => Self::Bolt12,
        }
    }
}

impl From<PaymentType> for cdk::invoice::PaymentType {
    fn from(payment_type: PaymentType) -> Self {
        match payment_type {
            PaymentType::Bolt11 => Self::Bolt11,
            PaymentType::Bolt12 => Self::Bolt12,
        }
    }
}

/// Decoded invoice or offer information
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct DecodedInvoice {
    /// Type of payment request (Bolt11 or Bolt12)
    pub payment_type: PaymentType,
    /// Amount in millisatoshis, if specified
    pub amount_msat: Option<u64>,
    /// Expiry timestamp (Unix timestamp), if specified
    pub expiry: Option<u64>,
    /// Description or offer description, if specified
    pub description: Option<String>,
}

impl From<cdk::invoice::DecodedInvoice> for DecodedInvoice {
    fn from(decoded: cdk::invoice::DecodedInvoice) -> Self {
        Self {
            payment_type: decoded.payment_type.into(),
            amount_msat: decoded.amount_msat,
            expiry: decoded.expiry,
            description: decoded.description,
        }
    }
}

impl From<DecodedInvoice> for cdk::invoice::DecodedInvoice {
    fn from(decoded: DecodedInvoice) -> Self {
        Self {
            payment_type: decoded.payment_type.into(),
            amount_msat: decoded.amount_msat,
            expiry: decoded.expiry,
            description: decoded.description,
        }
    }
}

/// Decode a bolt11 invoice or bolt12 offer from a string
///
/// This function attempts to parse the input as a bolt11 invoice first,
/// then as a bolt12 offer if bolt11 parsing fails.
///
/// # Arguments
///
/// * `invoice_str` - The invoice or offer string to decode
///
/// # Returns
///
/// * `Ok(DecodedInvoice)` - Successfully decoded invoice/offer information
/// * `Err(FfiError)` - Failed to parse as either bolt11 or bolt12
///
/// # Example
///
/// ```kotlin
/// val decoded = decodeInvoice("lnbc...")
/// when (decoded.paymentType) {
///     PaymentType.BOLT11 -> println("Bolt11 invoice")
///     PaymentType.BOLT12 -> println("Bolt12 offer")
/// }
/// ```
#[uniffi::export]
pub fn decode_invoice(invoice_str: String) -> Result<DecodedInvoice, FfiError> {
    let decoded = cdk::invoice::decode_invoice(&invoice_str)?;
    Ok(decoded.into())
}
