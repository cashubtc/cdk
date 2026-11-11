//! Prepared output denominations and their quoted redemption fee.

use cdk_common::amount::FeeAndAmounts;
use cdk_common::Id;

use super::fees::split_exact_with_fee;
use super::SendOptions;
use crate::fees::calculate_fee;
use crate::{Amount, Error};

/// Internal output plan carried by binding adapters through confirmation.
///
/// Obtain this from a prepared send; its denominations and fee must travel
/// together when reconstructing that send for confirmation.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
pub struct PreparedSendPlan {
    pub(super) amounts: Vec<Amount>,
    pub(super) fee: Amount,
}

impl PreparedSendPlan {
    pub(super) fn new(
        amount: Amount,
        options: &SendOptions,
        keyset_id: Id,
        fees: &FeeAndAmounts,
        budget: Amount,
    ) -> Result<Self, Error> {
        let (amounts, fee) = if options.include_fee && options.send_kind.is_exact() {
            split_exact_with_fee(amount, fees, budget, options.max_proofs)?
        } else if options.include_fee {
            let amounts = amount.split_with_fee(fees)?;
            let fee = calculate_fee(
                &[(keyset_id, amounts.len() as u64)].into_iter().collect(),
                &[(keyset_id, fees.fee())].into_iter().collect(),
            )?;
            (amounts, fee.total)
        } else {
            (amount.split(fees)?, Amount::ZERO)
        };
        Ok(Self { amounts, fee })
    }

    /// Quoted fee for redeeming the outgoing proofs.
    pub fn fee(&self) -> Amount {
        self.fee
    }
}
