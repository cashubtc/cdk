//! Fee types shared across crates

use std::collections::HashMap;

use crate::nuts::Id;
use crate::Amount;

/// Fee breakdown containing total fee and fee per keyset
#[derive(Debug, Clone, PartialEq)]
pub struct ProofsFeeBreakdown {
    /// Total fee across all keysets
    pub total: Amount,
    /// Fee collected per keyset
    pub per_keyset: HashMap<Id, Amount>,
}
