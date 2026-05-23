//! Payjoin for onchain payment method

use serde::{Deserialize, Serialize};

/// Supported Payjoin version for the onchain payment method.
pub const PAYJOIN_V2_VERSION: u64 = 2;

/// BIP77/v2 Payjoin parameters for an onchain payment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct PayjoinV2 {
    /// BIP77 mailbox endpoint, equivalent to the `pj` value.
    pub endpoint: String,
    /// OHTTP relay URL.
    pub ohttp_relay: String,
    /// Encoded OHTTP key material needed by the sender.
    pub ohttp_keys: String,
    /// Encoded receiver session key.
    pub receiver_key: String,
    /// Unix timestamp until the Payjoin parameters are valid.
    pub expires_at: Option<u64>,
    /// Whether fallback address payment is allowed.
    pub required: bool,
}

/// Wallet request for Payjoin-capable onchain mint instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct OnchainPayjoinRequest {
    /// Payjoin version requested by the wallet.
    pub version: u64,
    /// Whether fallback address payment is allowed.
    #[serde(default)]
    pub required: bool,
}

impl OnchainPayjoinRequest {
    /// Returns true when the requested Payjoin version is supported.
    pub fn is_supported(&self) -> bool {
        self.version == PAYJOIN_V2_VERSION
    }
}

/// Structured Payjoin instructions for an onchain payment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct OnchainPayjoin {
    /// Payjoin version.
    pub version: u64,
    /// Version-specific Payjoin parameters.
    pub params: PayjoinV2,
}

impl OnchainPayjoin {
    /// Returns true when the Payjoin version is supported.
    pub fn is_supported(&self) -> bool {
        self.version == PAYJOIN_V2_VERSION
    }

    /// Returns true when fallback address payment is not allowed.
    pub fn is_required(&self) -> bool {
        self.params.required
    }
}
