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
    let mut sum_fee = 0;

    for (keyset_id, proof_count) in proofs_count {
        let keyset_fee_ppk = keyset_fee
            .get(keyset_id)
            .ok_or(Error::KeysetUnknown(*keyset_id))?;

        let proofs_fee = keyset_fee_ppk * proof_count;

        sum_fee += proofs_fee;
    }

    let fee = (sum_fee + 999) / 1000;

    Ok(fee.into())
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_calc_fee() -> anyhow::Result<()> {
        let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

        let fee = 2;

        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(keyset_id, fee);

        let mut proofs_count = HashMap::new();

        proofs_count.insert(keyset_id, 1);

        let sum_fee = calculate_fee(&proofs_count, &keyset_fees)?;

        assert_eq!(sum_fee, 1.into());

        proofs_count.insert(keyset_id, 500);

        let sum_fee = calculate_fee(&proofs_count, &keyset_fees)?;

        assert_eq!(sum_fee, 1.into());

        proofs_count.insert(keyset_id, 1000);

        let sum_fee = calculate_fee(&proofs_count, &keyset_fees)?;

        assert_eq!(sum_fee, 2.into());

        proofs_count.insert(keyset_id, 2000);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees)?;
        assert_eq!(sum_fee, 4.into());

        proofs_count.insert(keyset_id, 3500);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees)?;
        assert_eq!(sum_fee, 7.into());

        proofs_count.insert(keyset_id, 3501);
        let sum_fee = calculate_fee(&proofs_count, &keyset_fees)?;
        assert_eq!(sum_fee, 8.into());

        Ok(())
    }
}
