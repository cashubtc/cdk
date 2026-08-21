//! Adapt draft Cashu Payjoin parameters to backend metadata and BIP77 endpoints.

use crate::bip77::{
    build_receiver_endpoint, parse_receiver_endpoint, payjoin_expires_at_is_expired,
    PayjoinReceiverError,
};
use crate::nuts::nut31::PayjoinV2;

/// Extra JSON key used for Payjoin v2 parameters.
pub const ONCHAIN_PAYJOIN_EXTRA_KEY: &str = "payjoin";
/// Internal extra JSON key used to persist destination Payjoin v2 parameters
/// for onchain melt recovery.
pub const ONCHAIN_PAYJOIN_DESTINATION_EXTRA_KEY: &str = "payjoin_destination";

/// Read Cashu Payjoin v2 parameters from an `extra_json` object.
pub fn payjoin_v2_from_extra_json(extra_json: Option<&serde_json::Value>) -> Option<PayjoinV2> {
    extra_json
        .and_then(|extra| extra.get(ONCHAIN_PAYJOIN_EXTRA_KEY))
        .cloned()
        .and_then(|payjoin| serde_json::from_value(payjoin).ok())
}

/// Parse draft Cashu Payjoin parameters from a BIP77 mailbox endpoint.
pub fn payjoin_v2_from_bip77_endpoint(endpoint: &str) -> Result<PayjoinV2, PayjoinReceiverError> {
    parse_receiver_endpoint(endpoint).map(Into::into)
}

/// Build a BIP77 mailbox endpoint from draft Cashu Payjoin parameters.
pub fn payjoin_v2_to_bip77_endpoint(payjoin: &PayjoinV2) -> Result<String, PayjoinReceiverError> {
    build_receiver_endpoint(&payjoin.clone().into())
}

/// Returns true when the Payjoin parameters are expired at `now`.
pub fn payjoin_v2_is_expired_at(payjoin: &PayjoinV2, now: u64) -> bool {
    payjoin_expires_at_is_expired(payjoin.expires_at, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OHTTP_KEYS: &str = "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ";
    const RECEIVER_KEY: &str = "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG";

    #[test]
    fn reads_payjoin_from_extra_json() {
        let payjoin = PayjoinV2::new(
            "https://payjoin.example/pj".to_string(),
            OHTTP_KEYS,
            RECEIVER_KEY,
            1_720_547_781,
        )
        .expect("valid Payjoin keys");
        let extra = serde_json::json!({ ONCHAIN_PAYJOIN_EXTRA_KEY: payjoin.clone() });

        assert_eq!(payjoin_v2_from_extra_json(Some(&extra)).unwrap(), payjoin);
        assert_eq!(payjoin_v2_from_extra_json(None), None);
    }
}
