//! Calculate fees
//!
//! <https://github.com/cashubtc/nuts/blob/main/02.md>

use std::collections::HashMap;

use tracing::instrument;

use crate::nuts::Id;
use crate::{Amount, Error};

/// Fee breakdown containing total fee and fee per keyset
#[derive(Debug, Clone)]
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
    let mut fee_per_keyset_raw: HashMap<Id, u64> = HashMap::new();

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
    let mut per_keyset = HashMap::new();
    let mut distributed_fee: u64 = 0;

    if sum_fee > 0 {
        let keyset_ids: Vec<_> = fee_per_keyset_raw.keys().copied().collect();

        for (i, keyset_id) in keyset_ids.iter().enumerate() {
            let raw_fee = fee_per_keyset_raw[keyset_id];

            // Calculate proportional fee, rounding down
            let keyset_fee = if i == keyset_ids.len() - 1 {
                // Last keyset gets the remainder to ensure total matches
                total_fee.saturating_sub(distributed_fee)
            } else {
                (raw_fee * total_fee) / sum_fee
            };

            distributed_fee = distributed_fee.saturating_add(keyset_fee);
            per_keyset.insert(*keyset_id, keyset_fee.into());
        }
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
}
