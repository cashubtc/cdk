//! Utils

use bitcoin::hashes::sha256::Hash as Sha256;
use bitcoin::hashes::Hash;
use rand::prelude::*;
use regex::Regex;

use crate::Amount;

/// Split amount into cashu denominations (powers of 2)
pub fn split_amount(amount: Amount) -> Vec<Amount> {
    let mut chunks = Vec::new();
    let value = amount.to_sat();
    for i in 0..64 {
        let mask = 1 << i;
        if (value & mask) != 0 {
            chunks.push(Amount::from_sat(2u64.pow(i as u32)));
        }
    }
    chunks
}

pub fn extract_url_from_error(error: &str) -> Option<String> {
    let regex = Regex::new(r"https?://[^\s]+").unwrap();
    if let Some(capture) = regex.captures(error) {
        return Some(capture[0].to_owned());
    }
    None
}

pub fn random_hash() -> Vec<u8> {
    let mut rng = rand::thread_rng();
    let mut random_bytes = [0u8; Sha256::LEN];
    rng.fill_bytes(&mut random_bytes);
    let hash = Sha256::hash(&random_bytes);
    hash.to_byte_array().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_amount() {
        assert_eq!(split_amount(Amount::from_sat(1)), vec![Amount::from_sat(1)]);
        assert_eq!(split_amount(Amount::from_sat(2)), vec![Amount::from_sat(2)]);
        assert_eq!(
            split_amount(Amount::from_sat(3)),
            vec![Amount::from_sat(1), Amount::from_sat(2)]
        );
        let amounts: Vec<Amount> = [1, 2, 8].iter().map(|a| Amount::from_sat(*a)).collect();
        assert_eq!(split_amount(Amount::from_sat(11)), amounts);
        let amounts: Vec<Amount> = [1, 2, 4, 8, 16, 32, 64, 128]
            .iter()
            .map(|a| Amount::from_sat(*a))
            .collect();
        assert_eq!(split_amount(Amount::from_sat(255)), amounts);
    }
}
