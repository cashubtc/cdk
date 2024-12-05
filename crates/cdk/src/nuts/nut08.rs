//! NUT-08: Lightning fee return
//!
//! <https://github.com/cashubtc/nuts/blob/main/08.md>

use super::nut05::{MeltBolt11Request, MeltQuoteBolt11Response};
use crate::Amount;

impl<Q> MeltBolt11Request<Q> {
    /// Total output [`Amount`]
    pub fn output_amount(&self) -> Option<Amount> {
        self.outputs
            .as_ref()
            .and_then(|o| Amount::try_sum(o.iter().map(|proof| proof.amount)).ok())
    }
}

impl<Q> MeltQuoteBolt11Response<Q> {
    /// Total change [`Amount`]
    pub fn change_amount(&self) -> Option<Amount> {
        self.change
            .as_ref()
            .and_then(|o| Amount::try_sum(o.iter().map(|proof| proof.amount)).ok())
    }
}
