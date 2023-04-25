use base64::{engine::general_purpose, Engine as _};
use bitcoin::Amount;
use rand::prelude::*;

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

pub fn generate_secret() -> String {
    let mut rng = rand::thread_rng();
    let mut secret = [0u8; 32];
    rng.fill_bytes(&mut secret);
    general_purpose::STANDARD.encode(secret)
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
        let amounts: Vec<Amount> = vec![1, 2, 8].iter().map(|a| Amount::from_sat(*a)).collect();
        assert_eq!(split_amount(Amount::from_sat(11)), amounts);
        let amounts: Vec<Amount> = vec![1, 2, 4, 8, 16, 32, 64, 128]
            .iter()
            .map(|a| Amount::from_sat(*a))
            .collect();
        assert_eq!(split_amount(Amount::from_sat(255)), amounts);
    }
}
