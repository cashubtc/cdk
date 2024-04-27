//! NUT-08: Lightning fee return
//!
//! <https://github.com/cashubtc/nuts/blob/main/08.md>

use super::nut05::{MeltBolt11Request, MeltBolt11Response};
use crate::Amount;

impl MeltBolt11Request {
    pub fn output_amount(&self) -> Option<Amount> {
        self.outputs
            .as_ref()
            .map(|o| o.iter().map(|proof| proof.amount).sum())
    }
}

impl MeltBolt11Response {
    pub fn change_amount(&self) -> Option<Amount> {
        self.change
            .as_ref()
            .map(|c| c.iter().map(|b| b.amount).sum())
    }
}
