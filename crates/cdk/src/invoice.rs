//! Invoice and offer decoding utilities
//!
//! Provides standalone functions to decode bolt11 invoices and bolt12 offers
//! without requiring a wallet instance or creating melt quotes.

use std::str::FromStr;

use lightning::offers::offer::Offer;
use lightning_invoice::Bolt11Invoice;

use crate::error::Error;

/// Type of Lightning payment request
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PaymentType {
    /// Bolt11 invoice
    Bolt11,
    /// Bolt12 offer
    Bolt12,
}

/// Decoded invoice or offer information
#[derive(Debug, Clone)]
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
/// * `Err(Error)` - Failed to parse as either bolt11 or bolt12
///
/// # Example
///
/// ```ignore
/// let decoded = decode_invoice("lnbc...")?;
/// match decoded.payment_type {
///     PaymentType::Bolt11 => println!("Bolt11 invoice"),
///     PaymentType::Bolt12 => println!("Bolt12 offer"),
/// }
/// ```
pub fn decode_invoice(invoice_str: &str) -> Result<DecodedInvoice, Error> {
    // Try to parse as Bolt11 first
    if let Ok(invoice) = Bolt11Invoice::from_str(invoice_str) {
        let amount_msat = invoice.amount_milli_satoshis();

        let expiry = invoice.expires_at().map(|duration| duration.as_secs());

        let description = match invoice.description() {
            lightning_invoice::Bolt11InvoiceDescriptionRef::Direct(desc) => Some(desc.to_string()),
            lightning_invoice::Bolt11InvoiceDescriptionRef::Hash(hash) => {
                Some(format!("Hash: {}", hash.0))
            }
        };

        return Ok(DecodedInvoice {
            payment_type: PaymentType::Bolt11,
            amount_msat,
            expiry,
            description,
        });
    }

    // Try to parse as Bolt12
    if let Ok(offer) = Offer::from_str(invoice_str) {
        let amount_msat = offer.amount().and_then(|amount| {
            // Bolt12 amounts can be in different currencies
            // For now, we only extract if it's in Bitcoin (millisatoshis)
            match amount {
                lightning::offers::offer::Amount::Bitcoin { amount_msats } => Some(amount_msats),
                _ => None,
            }
        });

        let expiry = offer.absolute_expiry().map(|duration| duration.as_secs());

        let description = offer.description().map(|d| d.to_string());

        return Ok(DecodedInvoice {
            payment_type: PaymentType::Bolt12,
            amount_msat,
            expiry,
            description,
        });
    }

    // If both parsing attempts failed
    Err(Error::InvalidInvoice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_bolt11() {
        // This is a sample bolt11 invoice for testing
        let bolt11 = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygshp58yjmdan79s6qqdhdzgynm4zwqd5d7xmw5fk98klysy043l2ahrqspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53pe5clth2l6j95s92zcz2v5h9z8xrzm0j2w3sye65pjzqgpl44gc8";

        let result = decode_invoice(bolt11);
        assert!(result.is_ok());

        let decoded = result.unwrap();
        assert_eq!(decoded.payment_type, PaymentType::Bolt11);
        assert_eq!(decoded.amount_msat, Some(250000000));
    }

    #[test]
    fn test_invalid_invoice() {
        let result = decode_invoice("invalid_string");
        assert!(result.is_err());
    }
}
