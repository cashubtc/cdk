//! Payjoin for onchain payment method
//!
//! Wire fields follow the unmerged [Payjoin proposal](https://github.com/cashubtc/nuts/pull/376),
//! currently numbered NUT-XX.

use serde::{Deserialize, Serialize};

use crate::bip77::PayjoinReceiver;
pub use crate::bip77::{
    PayjoinKeyError as PayjoinV2KeyError, PayjoinOhttpKeys, PayjoinReceiverKey,
};

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
    pub ohttp_keys: PayjoinOhttpKeys,
    /// Encoded receiver session key, without the `RK1` prefix.
    pub receiver_key: PayjoinReceiverKey,
    /// Unix timestamp until the Payjoin parameters are valid.
    pub expires_at: u64,
}

impl PayjoinV2 {
    /// Construct Payjoin v2 parameters from encoded key strings.
    pub fn new<O, R>(
        endpoint: String,
        ohttp_keys: O,
        receiver_key: R,
        expires_at: u64,
    ) -> Result<Self, PayjoinV2KeyError>
    where
        O: AsRef<str>,
        R: AsRef<str>,
    {
        Ok(Self {
            endpoint,
            ohttp_keys: ohttp_keys.as_ref().parse()?,
            receiver_key: receiver_key.as_ref().parse()?,
            expires_at,
        })
    }
}

impl From<PayjoinReceiver> for PayjoinV2 {
    fn from(value: PayjoinReceiver) -> Self {
        Self {
            endpoint: value.endpoint,
            ohttp_keys: value.ohttp_keys,
            receiver_key: value.receiver_key,
            expires_at: value.expires_at,
        }
    }
}

impl From<PayjoinV2> for PayjoinReceiver {
    fn from(value: PayjoinV2) -> Self {
        Self {
            endpoint: value.endpoint,
            ohttp_keys: value.ohttp_keys,
            receiver_key: value.receiver_key,
            expires_at: value.expires_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PayjoinOhttpKeys, PayjoinReceiverKey, PayjoinV2, PayjoinV2KeyError};

    const OHTTP_KEYS: &str = "QYPM59NK2LXXS4890SUAXXYT25Z2VAPHP0X7YEYCJXGWAG6UG9ZU6NQ";
    const RECEIVER_KEY: &str = "Q0DJS3VVDXWQQTLQ8022QGXSX7ML9PHZ6EDSF6AKEWQG758JPS2EV";

    #[test]
    fn payjoin_draft_wire_example_roundtrips() {
        // Cashu NUTs PR #376, mint quote example.
        let value = serde_json::json!({
            "endpoint": "HTTPS://PAYJOIN.EXAMPLE/TXJCGKTKXLUUZ",
            "ohttp_keys": OHTTP_KEYS,
            "receiver_key": RECEIVER_KEY,
            "expires_at": 1701704757,
        });
        let payjoin: PayjoinV2 = serde_json::from_value(value.clone()).unwrap();
        let receiver = crate::bip77::PayjoinReceiver::from(payjoin.clone());
        assert_eq!(PayjoinV2::from(receiver), payjoin);
        assert_eq!(serde_json::to_value(payjoin).unwrap(), value);
    }

    #[test]
    fn rejects_nonzero_key_padding() {
        let ohttp = format!("{}P", &OHTTP_KEYS[..OHTTP_KEYS.len() - 1]);
        let receiver = format!("{}D", &RECEIVER_KEY[..RECEIVER_KEY.len() - 1]);
        assert!(matches!(
            ohttp.parse::<PayjoinOhttpKeys>(),
            Err(PayjoinV2KeyError::InvalidPadding { .. })
        ));
        assert!(matches!(
            receiver.parse::<PayjoinReceiverKey>(),
            Err(PayjoinV2KeyError::InvalidPadding { .. })
        ));
        assert!(serde_json::from_value::<PayjoinV2>(serde_json::json!({
            "endpoint": "HTTPS://PAYJOIN.EXAMPLE/TXJCGKTKXLUUZ",
            "ohttp_keys": ohttp,
            "receiver_key": RECEIVER_KEY,
            "expires_at": 1701704757,
        }))
        .is_err());
    }

    #[test]
    fn rejects_extra_receiver_key_character() {
        for suffix in ["Q", "P"] {
            let key = format!("{RECEIVER_KEY}{suffix}");
            assert!(matches!(
                key.parse::<PayjoinReceiverKey>(),
                Err(PayjoinV2KeyError::InvalidPadding { .. })
            ));
            assert!(serde_json::from_value::<PayjoinReceiverKey>(serde_json::json!(key)).is_err());
        }
    }
}
