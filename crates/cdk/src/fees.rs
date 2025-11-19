//! Calculate fees
//!
//! <https://github.com/cashubtc/nuts/blob/main/02.md>

use std::collections::{BTreeMap, HashMap};

use tracing::instrument;

use crate::nuts::Id;
use crate::{Amount, Error};

/// Fee breakdown containing total fee and fee per keyset
#[derive(Debug, Clone, PartialEq)]
pub struct ProofsFeeBreakdown {
    /// Total fee across all keysets
    pub total: Amount,
    /// Fee collected per keyset
    pub per_keyset: HashMap<Id, Amount>,
}

/// Fee required for proof set
#[instrument(skip_all)]
pub fn calculate_fee(
    proofs_count: &HashMap<Id, u64>,
    keyset_fee: &HashMap<Id, u64>,
) -> Result<ProofsFeeBreakdown, Error> {
    let mut sum_fee: u64 = 0;
    let mut fee_per_keyset_raw: BTreeMap<Id, u64> = BTreeMap::new();

    for (keyset_id, proof_count) in proofs_count {
        let keyset_fee_ppk = keyset_fee
            .get(keyset_id)
            .ok_or(Error::KeysetUnknown(*keyset_id))?;

        let proofs_fee = keyset_fee_ppk * proof_count;

        sum_fee = sum_fee
            .checked_add(proofs_fee)
            .ok_or(Error::AmountOverflow)?;

        fee_per_keyset_raw.insert(*keyset_id, proofs_fee);
    }

    let total_fee = (sum_fee.checked_add(999).ok_or(Error::AmountOverflow)?) / 1000;

    // Calculate fee per keyset proportionally based on the total
    // BTreeMap ensures deterministic iteration order (sorted by keyset ID)
    let mut per_keyset = HashMap::new();
    let mut distributed_fee: u64 = 0;
    let keyset_count = fee_per_keyset_raw.len();

    for (i, (keyset_id, raw_fee)) in fee_per_keyset_raw.iter().enumerate() {
        if sum_fee == 0 {
            continue;
        }

        // Calculate proportional fee, rounding down
        let keyset_fee = if i == keyset_count - 1 {
            // Last keyset gets the remainder to ensure total matches
            total_fee.saturating_sub(distributed_fee)
        } else {
            (raw_fee * total_fee) / sum_fee
        };

        distributed_fee = distributed_fee.saturating_add(keyset_fee);
        per_keyset.insert(*keyset_id, keyset_fee.into());
    }

    Ok(ProofsFeeBreakdown {
        total: total_fee.into(),
        per_keyset,
    })
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

        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        assert_eq!(breakdown.total, 1.into());
        assert_eq!(breakdown.per_keyset[&keyset_id], 1.into());

        proofs_count.insert(keyset_id, 500);

        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        assert_eq!(breakdown.total, 1.into());
        assert_eq!(breakdown.per_keyset[&keyset_id], 1.into());

        proofs_count.insert(keyset_id, 1000);

        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        assert_eq!(breakdown.total, 2.into());
        assert_eq!(breakdown.per_keyset[&keyset_id], 2.into());

        proofs_count.insert(keyset_id, 2000);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(breakdown.total, 4.into());
        assert_eq!(breakdown.per_keyset[&keyset_id], 4.into());

        proofs_count.insert(keyset_id, 3500);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(breakdown.total, 7.into());
        assert_eq!(breakdown.per_keyset[&keyset_id], 7.into());

        proofs_count.insert(keyset_id, 3501);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(breakdown.total, 8.into());
        assert_eq!(breakdown.per_keyset[&keyset_id], 8.into());
    }

    #[test]
    fn test_fee_calculation_with_ppk_200() {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

        let fee_ppk = 200;

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, fee_ppk);

        let mut proofs_count = HashMap::new();

        proofs_count.insert(keyset_id, 1);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(breakdown.total, 1.into(), "1 proof: ceil(200/1000) = 1 sat");

        proofs_count.insert(keyset_id, 3);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(
            breakdown.total,
            1.into(),
            "3 proofs: ceil(600/1000) = 1 sat"
        );

        proofs_count.insert(keyset_id, 5);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(
            breakdown.total,
            1.into(),
            "5 proofs: ceil(1000/1000) = 1 sat"
        );

        proofs_count.insert(keyset_id, 6);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(
            breakdown.total,
            2.into(),
            "6 proofs: ceil(1200/1000) = 2 sats"
        );
    }

    #[test]
    fn test_fee_calculation_with_ppk_1000() {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

        let fee_ppk = 1000;

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, fee_ppk);

        let mut proofs_count = HashMap::new();

        proofs_count.insert(keyset_id, 1);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(breakdown.total, 1.into(), "1 proof at 1000 ppk = 1 sat");

        proofs_count.insert(keyset_id, 2);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(breakdown.total, 2.into(), "2 proofs at 1000 ppk = 2 sats");

        proofs_count.insert(keyset_id, 10);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(
            breakdown.total,
            10.into(),
            "10 proofs at 1000 ppk = 10 sats"
        );
    }

    #[test]
    fn test_fee_calculation_zero_fee() {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

        let fee_ppk = 0;

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, fee_ppk);

        let mut proofs_count = HashMap::new();

        proofs_count.insert(keyset_id, 100);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(
            breakdown.total,
            0.into(),
            "0 ppk means no fee: ceil(0/1000) = 0"
        );
    }

    #[test]
    fn test_fee_calculation_with_ppk_100() {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

        let fee_ppk = 100;

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, fee_ppk);

        let mut proofs_count = HashMap::new();

        proofs_count.insert(keyset_id, 1);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(breakdown.total, 1.into(), "1 proof: ceil(100/1000) = 1 sat");

        proofs_count.insert(keyset_id, 10);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(
            breakdown.total,
            1.into(),
            "10 proofs: ceil(1000/1000) = 1 sat"
        );

        proofs_count.insert(keyset_id, 11);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(
            breakdown.total,
            2.into(),
            "11 proofs: ceil(1100/1000) = 2 sats"
        );

        proofs_count.insert(keyset_id, 91);
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(
            breakdown.total,
            10.into(),
            "91 proofs: ceil(9100/1000) = 10 sats"
        );
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

        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        assert_eq!(
            breakdown.total,
            2.into(),
            "3*200 + 2*500 = 1600, ceil(1600/1000) = 2"
        );
    }

    #[test]
    fn test_per_keyset_fee_sums_to_total() {
        let keyset_id_1 = Id::from_str("001711afb1de20cb").unwrap();
        let keyset_id_2 = Id::from_str("001711afb1de20cc").unwrap();
        let keyset_id_3 = Id::from_str("001711afb1de20cd").unwrap();

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id_1, 100);
        keyset_fees.insert(keyset_id_2, 100);
        keyset_fees.insert(keyset_id_3, 100);

        let mut proofs_count = HashMap::new();
        proofs_count.insert(keyset_id_1, 1);
        proofs_count.insert(keyset_id_2, 1);
        proofs_count.insert(keyset_id_3, 1);

        // 3 proofs * 100 ppk = 300 ppk, ceil(300/1000) = 1 sat total
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        assert_eq!(breakdown.total, 1.into());

        // Sum of per_keyset fees must equal total
        let per_keyset_sum: u64 = breakdown.per_keyset.values().map(|a| u64::from(*a)).sum();
        assert_eq!(per_keyset_sum, u64::from(breakdown.total));
    }

    #[test]
    fn test_per_keyset_fee_remainder_goes_to_last_sorted_keyset() {
        // Use keyset IDs where sorting order is predictable
        let keyset_id_1 = Id::from_str("00aaaaaaaaaaaaa1").unwrap();
        let keyset_id_2 = Id::from_str("00aaaaaaaaaaaaa2").unwrap();
        let keyset_id_3 = Id::from_str("00aaaaaaaaaaaaa3").unwrap();

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id_1, 100);
        keyset_fees.insert(keyset_id_2, 100);
        keyset_fees.insert(keyset_id_3, 100);

        let mut proofs_count = HashMap::new();
        proofs_count.insert(keyset_id_1, 1);
        proofs_count.insert(keyset_id_2, 1);
        proofs_count.insert(keyset_id_3, 1);

        // 3 * 100 = 300 ppk, ceil(300/1000) = 1 sat total
        // Each keyset contributed 100/300 = 1/3 of raw fee
        // Proportional: (100 * 1) / 300 = 0 for first two (integer division)
        // Last keyset (keyset_id_3) gets remainder: 1 - 0 - 0 = 1
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        assert_eq!(breakdown.total, 1.into());
        assert_eq!(breakdown.per_keyset[&keyset_id_1], 0.into());
        assert_eq!(breakdown.per_keyset[&keyset_id_2], 0.into());
        assert_eq!(breakdown.per_keyset[&keyset_id_3], 1.into());
    }

    #[test]
    fn test_per_keyset_fee_distribution_is_deterministic() {
        let keyset_id_1 = Id::from_str("001711afb1de20cb").unwrap();
        let keyset_id_2 = Id::from_str("001711afb1de20cc").unwrap();

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id_1, 333);
        keyset_fees.insert(keyset_id_2, 333);

        let mut proofs_count = HashMap::new();
        proofs_count.insert(keyset_id_1, 1);
        proofs_count.insert(keyset_id_2, 1);

        // Run multiple times to verify determinism
        let breakdown1 = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        let breakdown2 = calculate_fee(&proofs_count, &keyset_fees).unwrap();
        let breakdown3 = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        // All runs should produce identical per-keyset results
        assert_eq!(
            breakdown1.per_keyset[&keyset_id_1],
            breakdown2.per_keyset[&keyset_id_1]
        );
        assert_eq!(
            breakdown1.per_keyset[&keyset_id_2],
            breakdown2.per_keyset[&keyset_id_2]
        );
        assert_eq!(
            breakdown2.per_keyset[&keyset_id_1],
            breakdown3.per_keyset[&keyset_id_1]
        );
        assert_eq!(
            breakdown2.per_keyset[&keyset_id_2],
            breakdown3.per_keyset[&keyset_id_2]
        );
    }

    #[test]
    fn test_per_keyset_fee_proportional_distribution() {
        let keyset_id_1 = Id::from_str("001711afb1de20cb").unwrap();
        let keyset_id_2 = Id::from_str("001711afb1de20cc").unwrap();

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id_1, 1000); // 1 sat per proof
        keyset_fees.insert(keyset_id_2, 1000);

        let mut proofs_count = HashMap::new();
        proofs_count.insert(keyset_id_1, 3); // 3000 ppk = 3 sat raw
        proofs_count.insert(keyset_id_2, 7); // 7000 ppk = 7 sat raw

        // Total: 10000 ppk = 10 sat
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        assert_eq!(breakdown.total, 10.into());
        // keyset_id_1: (3000 * 10) / 10000 = 3
        // keyset_id_2: 10 - 3 = 7 (gets remainder, but happens to be exact)
        assert_eq!(breakdown.per_keyset[&keyset_id_1], 3.into());
        assert_eq!(breakdown.per_keyset[&keyset_id_2], 7.into());
    }

    #[test]
    fn test_per_keyset_fee_with_uneven_distribution() {
        let keyset_id_1 = Id::from_str("00aaaaaaaaaaaaa1").unwrap();
        let keyset_id_2 = Id::from_str("00aaaaaaaaaaaaa2").unwrap();

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id_1, 100);
        keyset_fees.insert(keyset_id_2, 100);

        let mut proofs_count = HashMap::new();
        proofs_count.insert(keyset_id_1, 5); // 500 ppk
        proofs_count.insert(keyset_id_2, 6); // 600 ppk

        // Total: 1100 ppk, ceil(1100/1000) = 2 sat
        // keyset_id_1: (500 * 2) / 1100 = 0 (integer division)
        // keyset_id_2: 2 - 0 = 2 (gets remainder)
        let breakdown = calculate_fee(&proofs_count, &keyset_fees).unwrap();

        assert_eq!(breakdown.total, 2.into());

        // Verify sum equals total
        let per_keyset_sum: u64 = breakdown.per_keyset.values().map(|a| u64::from(*a)).sum();
        assert_eq!(per_keyset_sum, 2);
    }
}
