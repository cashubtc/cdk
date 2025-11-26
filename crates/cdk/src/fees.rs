//! Calculate fees
//!
//! <https://github.com/cashubtc/nuts/blob/main/02.md>

use std::collections::HashMap;

use tracing::instrument;

use crate::nuts::Id;
use crate::{Amount, Error};

/// Fee required for proof set
#[instrument(skip_all)]
pub fn calculate_fee(
    proofs_count: &HashMap<Id, u64>,
    keyset_fee: &HashMap<Id, u64>,
) -> Result<Amount, Error> {
    let mut sum_fee: u64 = 0;

    for (keyset_id, proof_count) in proofs_count {
        let keyset_fee_ppk = keyset_fee
            .get(keyset_id)
            .ok_or(Error::KeysetUnknown(*keyset_id))?;

        let proofs_fee = keyset_fee_ppk * proof_count;

        sum_fee = sum_fee
            .checked_add(proofs_fee)
            .ok_or(Error::AmountOverflow)?;
    }

    let fee = (sum_fee.checked_add(999).ok_or(Error::AmountOverflow)?) / 1000;

    Ok(fee.into())
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_calc_fee() {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

        let fee = 2;

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, fee);

        let mut proofs_count = HashMap::new();

        proofs_count.insert(keyset_id, 1);

        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        assert_eq!(sum_fee, 1.into());

        proofs_count.insert(keyset_id, 500);

        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        assert_eq!(sum_fee, 1.into());

        proofs_count.insert(keyset_id, 1000);

        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        assert_eq!(sum_fee, 2.into());

        proofs_count.insert(keyset_id, 2000);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 4.into());

        proofs_count.insert(keyset_id, 3500);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 7.into());

        proofs_count.insert(keyset_id, 3501);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 8.into());
    }

    #[test]
    fn test_fee_calculation_with_ppk_200() {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

        let fee_ppk = 200;

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, fee_ppk);

        let mut proofs_count = HashMap::new();

        proofs_count.insert(keyset_id, 1);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 1.into(), "1 proof: ceil(200/1000) = 1 sat");

        proofs_count.insert(keyset_id, 3);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 1.into(), "3 proofs: ceil(600/1000) = 1 sat");

        proofs_count.insert(keyset_id, 5);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 1.into(), "5 proofs: ceil(1000/1000) = 1 sat");

        proofs_count.insert(keyset_id, 6);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 2.into(), "6 proofs: ceil(1200/1000) = 2 sats");
    }

    #[test]
    fn test_fee_calculation_with_ppk_1000() {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

        let fee_ppk = 1000;

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, fee_ppk);

        let mut proofs_count = HashMap::new();

        proofs_count.insert(keyset_id, 1);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 1.into(), "1 proof at 1000 ppk = 1 sat");

        proofs_count.insert(keyset_id, 2);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 2.into(), "2 proofs at 1000 ppk = 2 sats");

        proofs_count.insert(keyset_id, 10);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 10.into(), "10 proofs at 1000 ppk = 10 sats");
    }

    #[test]
    fn test_fee_calculation_zero_fee() {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

        let fee_ppk = 0;

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, fee_ppk);

        let mut proofs_count = HashMap::new();

        proofs_count.insert(keyset_id, 100);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 0.into(), "0 ppk means no fee: ceil(0/1000) = 0");
    }

    #[test]
    fn test_fee_calculation_with_ppk_100() {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

        let fee_ppk = 100;

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, fee_ppk);

        let mut proofs_count = HashMap::new();

        proofs_count.insert(keyset_id, 1);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 1.into(), "1 proof: ceil(100/1000) = 1 sat");

        proofs_count.insert(keyset_id, 10);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 1.into(), "10 proofs: ceil(1000/1000) = 1 sat");

        proofs_count.insert(keyset_id, 11);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 2.into(), "11 proofs: ceil(1100/1000) = 2 sats");

        proofs_count.insert(keyset_id, 91);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(sum_fee, 10.into(), "91 proofs: ceil(9100/1000) = 10 sats");
    }

    #[test]
    fn test_fee_calculation_unknown_keyset() {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();
        let unknown_keyset_id = Id::from_str("001711afb1de20cc").unwrap();

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, 100);

        let mut proofs_count = HashMap::new();
        proofs_count.insert(unknown_keyset_id, 1);

        let result = calculate_fee(&proofs_count, &keyset_fees);
        assert!(result.is_err(), "Unknown keyset should return error");
    }

    #[test]
    fn test_fee_calculation_multiple_keysets() {
        let keyset_id_1 = Id::from_str("001711afb1de20cb").unwrap();
        let keyset_id_2 = Id::from_str("001711afb1de20cc").unwrap();

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id_1, 200);
        keyset_fees.insert(keyset_id_2, 500);

        let mut proofs_count = HashMap::new();
        proofs_count.insert(keyset_id_1, 3);
        proofs_count.insert(keyset_id_2, 2);

        let sum_fee = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(
            sum_fee,
            2.into(),
            "3*200 + 2*500 = 1600, ceil(1600/1000) = 2"
        );
    }
}
