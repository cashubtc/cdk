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

    let offer = Offer::from_str(invoice_str).map_err(|_| Error::InvalidInvoice)?;

    let amount_msat = offer.amount().and_then(|amount| {
        // Bolt12 amounts can be in different currencies. For now, only extract Bitcoin amounts.
        match amount {
            lightning::offers::offer::Amount::Bitcoin { amount_msats } => Some(amount_msats),
            _ => None,
        }
    });

    let expiry = offer.absolute_expiry().map(|duration| duration.as_secs());

    let description = offer.description().map(|d| d.to_string());

    Ok(DecodedInvoice {
        payment_type: PaymentType::Bolt12,
        amount_msat,
        expiry,
        description,
    })
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
    fn test_decode_bolt11() {
        // This is a valid bolt11 invoice for 100 sats
        let bolt11 = "lnbc1u1p53kkd9pp5ve8pd9zr60yjyvs6tn77mndavzrl5lwd2gx5hk934f6q8jwguzgsdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5482y73fxmlvg4t66nupdaph93h7dcmfsg2ud72wajf0cpk3a96rq9qxpqysgqujexd0l89u5dutn8hxnsec0c7jrt8wz0z67rut0eah0g7p6zhycn2vff0ts5vwn2h93kx8zzqy3tzu4gfhkya2zpdmqelg0ceqnjztcqma65pr";

        let result = decode_invoice(bolt11);
        assert!(result.is_ok());

        let decoded = result.unwrap();
        assert_eq!(decoded.payment_type, PaymentType::Bolt11);
        assert_eq!(decoded.amount_msat, Some(100000));
    }

    #[test]
    fn test_invalid_invoice() {
        let result = decode_invoice("invalid_string");
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_bolt12_offer() {
        let decoded = decode_invoice(&test_bolt12_offer()).expect("offer should decode");

        assert_eq!(decoded.payment_type, PaymentType::Bolt12);
        assert_eq!(decoded.amount_msat, Some(123_000));
        assert_eq!(decoded.expiry, Some(1_700_000_000));
        assert_eq!(decoded.description, Some("coffee".to_string()));
    }
}
