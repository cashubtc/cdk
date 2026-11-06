//! Payjoin quote parameters.

use crate::error::FfiError;

/// Payjoin v2 receiver parameters.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PayjoinV2 {
    /// Directory mailbox endpoint.
    pub endpoint: String,
    /// Encoded OHTTP keys.
    pub ohttp_keys: String,
    /// Encoded receiver public key.
    pub receiver_key: String,
    /// Expiration in Unix seconds.
    pub expires_at: u64,
}

impl TryFrom<PayjoinV2> for cdk_common::nuts::nut31::PayjoinV2 {
    type Error = FfiError;

    fn try_from(value: PayjoinV2) -> Result<Self, Self::Error> {
        Self::new(
            value.endpoint,
            value.ohttp_keys,
            value.receiver_key,
            value.expires_at,
        )
        .map_err(|error| FfiError::Internal {
            error_message: error.to_string(),
        })
    }
}

impl From<cdk_common::nuts::nut31::PayjoinV2> for PayjoinV2 {
    fn from(value: cdk_common::nuts::nut31::PayjoinV2) -> Self {
        Self {
            endpoint: value.endpoint,
            ohttp_keys: value.ohttp_keys.to_string(),
            receiver_key: value.receiver_key.to_string(),
            expires_at: value.expires_at,
        }
    }
}
