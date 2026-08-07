//! NUT-08: Lightning fee return
//!
//! <https://github.com/cashubtc/nuts/blob/main/08.md>

use super::nut05::{MeltQuoteCustomResponse, MeltRequest};
use super::nut23::MeltQuoteBolt11Response;
use super::nut25::MeltQuoteBolt12Response;
use crate::Amount;

impl<Q> MeltRequest<Q> {
    /// Total output [`Amount`]
    pub fn output_amount(&self) -> Option<Amount> {
        self.outputs()
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

impl<Q> MeltQuoteBolt12Response<Q> {
    /// Total change [`Amount`]
    pub fn change_amount(&self) -> Option<Amount> {
        self.change
            .as_ref()
            .and_then(|o| Amount::try_sum(o.iter().map(|proof| proof.amount)).ok())
    }
}

impl<Q> MeltQuoteCustomResponse<Q> {
    /// Total change [`Amount`]
    pub fn change_amount(&self) -> Option<Amount> {
        self.change
            .as_ref()
            .and_then(|o| Amount::try_sum(o.iter().map(|proof| proof.amount)).ok())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::nuts::{BlindSignature, Id, MeltQuoteState, PaymentMethod, PublicKey};
    use crate::CurrencyUnit;

    fn blind_signature(amount: u64) -> BlindSignature {
        BlindSignature {
            amount: Amount::from(amount),
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
            c: PublicKey::from_hex(
                "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
            )
            .unwrap(),
            dleq: None,
            metadata: None,
        }
    }

    #[test]
    fn bolt11_change_amount_sums_change_outputs() {
        let response = MeltQuoteBolt11Response {
            quote: "quote-id",
            amount: Amount::from(10),
            fee_reserve: Amount::from(1),
            state: MeltQuoteState::Paid,
            expiry: 123,
            payment_preimage: None,
            change: Some(vec![blind_signature(2), blind_signature(3)]),
            request: Some("invoice".to_string()),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::BOLT11,
        };

        assert_eq!(response.change_amount(), Some(Amount::from(5)));
    }

    #[test]
    fn bolt12_change_amount_sums_change_outputs() {
        let response = MeltQuoteBolt12Response {
            quote: "quote-id",
            amount: Amount::from(10),
            fee_reserve: Amount::from(1),
            state: MeltQuoteState::Paid,
            expiry: 123,
            payment_preimage: None,
            change: Some(vec![blind_signature(3), blind_signature(4)]),
            request: Some("offer".to_string()),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::BOLT12,
        };

        assert_eq!(response.change_amount(), Some(Amount::from(7)));
    }

    #[test]
    fn custom_change_amount_sums_change_outputs() {
        let response = MeltQuoteCustomResponse {
            quote: "quote-id",
            amount: Amount::from(10),
            fee_reserve: Some(Amount::from(1)),
            state: MeltQuoteState::Paid,
            expiry: 123,
            payment_preimage: None,
            change: Some(vec![blind_signature(4), blind_signature(6)]),
            request: None,
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Custom("custom".to_string()),
            extra: serde_json::Value::Null,
        };

        assert_eq!(response.change_amount(), Some(Amount::from(10)));
    }
}
