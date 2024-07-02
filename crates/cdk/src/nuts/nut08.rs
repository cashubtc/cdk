//! NUT-08: Lightning fee return
//!
//! <https://github.com/cashubtc/nuts/blob/main/08.md>

use super::nut05::{MeltBolt11Request, MeltQuoteBolt11Response};
use crate::Amount;

impl MeltBolt11Request {
    /// Total output [`Amount`]
    pub fn output_amount(&self) -> Option<Amount> {
        self.outputs
            .as_ref()
            .map(|o| o.iter().map(|proof| proof.amount).sum())
    }
}

impl MeltQuoteBolt11Response {
    /// Total change [`Amount`]
    pub fn change_amount(&self) -> Option<Amount> {
        self.change
            .as_ref()
            .map(|c| c.iter().map(|b| b.amount).sum())
    }
}
