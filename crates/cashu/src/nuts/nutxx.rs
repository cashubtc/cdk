//! NUT-XX: Mint Quote Lookup by Public Key
//!
//! <https://github.com/cashubtc/nuts/blob/get-quotes-by-pubkeys/xx.md>

use serde::{Deserialize, Serialize};

/// Mint quote by pubkey request [NUT-XX]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintQuoteByPubkeyRequest {
    /// Pubkeys
    pub pubkeys: Vec<String>,
    /// Signatures
    pub pubkeys_signatures: Vec<String>,
}
