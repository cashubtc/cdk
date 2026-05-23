//! Payjoin for onchain payment method

use serde::{Deserialize, Serialize};

/// Payjoin v2 parameters for an onchain payment.
///
/// Cashu uses Unix timestamp; BIP77 URI fragments use encoded `EX1`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PayjoinV2 {
    /// BIP77 mailbox endpoint URL without receiver fragment parameters.
    ///
    /// When assembled into a `pj` URI parameter, the endpoint value must be
    /// encoded according to BIP77.
    pub endpoint: String,
    /// Encoded OHTTP key material needed by the sender, without the `OH1` prefix.
    pub ohttp_keys: String,
    /// Encoded receiver session key, without the `RK1` prefix.
    pub receiver_key: String,
    /// Unix timestamp until the Payjoin parameters are valid.
    pub expires_at: u64,
}
