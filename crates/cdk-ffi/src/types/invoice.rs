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

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use bitcoin::secp256k1::{Keypair, PublicKey, Secp256k1, SecretKey};
    use lightning::offers::offer::OfferBuilder;

    use super::*;

    fn test_bolt12_offer() -> String {
        let secp_ctx = Secp256k1::new();
        let secret_key =
            SecretKey::from_slice(&[42; 32]).expect("static secret key should be valid");
        let keys = Keypair::from_secret_key(&secp_ctx, &secret_key);
        let pubkey = PublicKey::from(keys);

        OfferBuilder::new(pubkey)
            .description("coffee".to_string())
            .amount_msats(123_000)
            .absolute_expiry(Duration::from_secs(1_700_000_000))
            .build()
            .expect("offer should build")
            .to_string()
    }

    #[test]
    fn test_decode_bolt12_offer() {
        let decoded = decode_invoice(test_bolt12_offer()).expect("offer should decode");

        assert_eq!(decoded.payment_type, PaymentType::Bolt12);
        assert_eq!(decoded.amount_msat, Some(123_000));
        assert_eq!(decoded.expiry, Some(1_700_000_000));
        assert_eq!(decoded.description, Some("coffee".to_string()));
    }
}
