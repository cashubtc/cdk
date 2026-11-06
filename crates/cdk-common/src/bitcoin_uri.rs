//! Shared Bitcoin payment URI amount and address utilities.

use bitcoin::{Amount, Denomination};
use thiserror::Error;

/// Error for converting BIP21 BTC amount strings to satoshis.
#[derive(Debug, Error)]
#[error("invalid BIP21 amount '{amount}': {source}")]
pub struct Bip21AmountError {
    /// Original BIP21 amount string.
    amount: String,
    /// Underlying amount parsing error.
    source: bitcoin::amount::ParseAmountError,
}

/// Format a satoshi amount as a BIP21 BTC decimal string.
pub fn format_bip21_amount_from_sats(amount_sat: u64) -> String {
    Amount::from_sat(amount_sat).to_string_in(Denomination::Bitcoin)
}

/// Parse a BIP21 BTC decimal amount string into satoshis.
pub fn parse_bip21_amount_to_sats(amount: &str) -> Result<u64, Bip21AmountError> {
    Amount::from_str_in(amount, Denomination::Bitcoin)
        .map(Amount::to_sat)
        .map_err(|source| Bip21AmountError {
            amount: amount.to_string(),
            source,
        })
}

/// Normalize the case of a Bech32 address, preserving Base58 addresses.
pub fn normalize_bip21_address(address: &str) -> String {
    let lowercase = address.to_ascii_lowercase();
    if is_bech32_address(&lowercase) {
        lowercase
    } else {
        address.to_string()
    }
}

/// Uppercase a Bech32 address for QR encoding, preserving Base58 addresses.
pub fn uppercase_bip21_address(address: &str) -> String {
    let lowercase = address.to_ascii_lowercase();
    if is_bech32_address(&lowercase) {
        address.to_ascii_uppercase()
    } else {
        address.to_string()
    }
}

fn is_bech32_address(address: &str) -> bool {
    address.starts_with("bc1") || address.starts_with("tb1") || address.starts_with("bcrt1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bip21_amount_from_sats() {
        assert_eq!(format_bip21_amount_from_sats(0), "0");
        assert_eq!(format_bip21_amount_from_sats(1), "0.00000001");
        assert_eq!(format_bip21_amount_from_sats(100_000_000), "1");
        assert_eq!(format_bip21_amount_from_sats(123_456_780), "1.2345678");
    }

    #[test]
    fn parses_bip21_amount_to_sats() {
        assert_eq!(parse_bip21_amount_to_sats("0").unwrap(), 0);
        assert_eq!(parse_bip21_amount_to_sats("0.00000001").unwrap(), 1);
        assert_eq!(parse_bip21_amount_to_sats("1").unwrap(), 100_000_000);
        assert_eq!(
            parse_bip21_amount_to_sats("1.2345678").unwrap(),
            123_456_780
        );
        assert!(
            parse_bip21_amount_to_sats("1.000000001").is_err(),
            "sub-satoshi precision must be rejected"
        );
    }
}
