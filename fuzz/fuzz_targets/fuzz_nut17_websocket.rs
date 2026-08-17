#![no_main]

//! Fuzz NUT-17 raw WebSocket parsing and kind-directed payload decoding.

use cashu::nuts::nut17::ws::RawWsMessageOrResponse;
use cashu::nuts::nut17::{deserialize_payload_for_kind, Kind, NotificationPayload};
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Value};

const PUBLIC_KEY: &str =
    "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac";

#[derive(Debug)]
struct Input {
    raw_message: String,
    quote: String,
    amount: u16,
    variant: u8,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        Ok(Self {
            raw_message: String::arbitrary(u)?,
            quote: String::arbitrary(u)?.chars().take(64).collect(),
            amount: u.arbitrary()?,
            variant: u.arbitrary()?,
        })
    }
}

fn canonical_payload(input: &Input) -> (Kind, Value) {
    let amount = u64::from(input.amount);
    match input.variant % 9 {
        0 => (
            Kind::ProofState,
            json!({"Y": PUBLIC_KEY, "state": "UNSPENT", "witness": null}),
        ),
        1 => (
            Kind::Bolt11MintQuote,
            json!({
                "quote": input.quote,
                "request": "lnbc1fuzz",
                "amount": amount,
                "unit": "sat",
                "method": "bolt11",
                "amount_paid": 0,
                "amount_issued": 0,
                "updated_at": 0,
                "state": "UNPAID",
                "expiry": null,
                "pubkey": PUBLIC_KEY
            }),
        ),
        2 => (
            Kind::Bolt11MeltQuote,
            json!({
                "quote": input.quote,
                "amount": amount,
                "fee_reserve": amount % 100,
                "state": "PENDING",
                "expiry": 1,
                "payment_preimage": null,
                "change": null,
                "request": "lnbc1fuzz",
                "unit": "sat",
                "method": "bolt11"
            }),
        ),
        3 => (
            Kind::Bolt12MintQuote,
            json!({
                "quote": input.quote,
                "request": "lno1fuzz",
                "amount": amount,
                "unit": "sat",
                "method": "bolt12",
                "expiry": null,
                "pubkey": PUBLIC_KEY,
                "amount_paid": 0,
                "amount_issued": 0,
                "updated_at": 0
            }),
        ),
        4 => (
            Kind::Bolt12MeltQuote,
            json!({
                "quote": input.quote,
                "amount": amount,
                "fee_reserve": amount % 100,
                "state": "PENDING",
                "expiry": 1,
                "payment_preimage": null,
                "change": null,
                "request": "lno1fuzz",
                "unit": "sat",
                "method": "bolt12"
            }),
        ),
        5 => (
            Kind::OnchainMintQuote,
            json!({
                "quote": input.quote,
                "request": "bcrt1qfuzz",
                "unit": "sat",
                "method": "onchain",
                "expiry": null,
                "pubkey": PUBLIC_KEY,
                "amount_paid": amount,
                "amount_issued": 0,
                "updated_at": 0
            }),
        ),
        6 => (
            Kind::OnchainMeltQuote,
            json!({
                "quote": input.quote,
                "amount": amount,
                "unit": "sat",
                "method": "onchain",
                "state": "PENDING",
                "expiry": 1,
                "request": "bcrt1qfuzz",
                "fee_options": [{
                    "fee_index": 0,
                    "fee_reserve": amount % 100,
                    "estimated_blocks": 1
                }],
                "selected_fee_index": 0,
                "outpoint": null,
                "change": null
            }),
        ),
        7 => (
            Kind::Custom("fuzzpay_mint_quote".to_string()),
            json!({
                "quote": input.quote,
                "request": "fuzzpay:request",
                "method": "fuzzpay",
                "amount": amount,
                "amount_paid": 0,
                "amount_issued": 0,
                "updated_at": 0,
                "unit": "sat",
                "expiry": null,
                "pubkey": null
            }),
        ),
        _ => (
            Kind::Custom("fuzzpay_melt_quote".to_string()),
            json!({
                "quote": input.quote,
                "method": "fuzzpay",
                "amount": amount,
                "fee_reserve": amount % 100,
                "state": "PENDING",
                "expiry": 1,
                "payment_preimage": null,
                "change": null,
                "request": "fuzzpay:request",
                "unit": "sat"
            }),
        ),
    }
}

fn variant_matches(kind: &Kind, payload: &NotificationPayload<String>) -> bool {
    match (kind, payload) {
        (Kind::ProofState, NotificationPayload::ProofState(_))
        | (Kind::Bolt11MintQuote, NotificationPayload::MintQuoteBolt11Response(_))
        | (Kind::Bolt11MeltQuote, NotificationPayload::MeltQuoteBolt11Response(_))
        | (Kind::Bolt12MintQuote, NotificationPayload::MintQuoteBolt12Response(_))
        | (Kind::Bolt12MeltQuote, NotificationPayload::MeltQuoteBolt12Response(_))
        | (Kind::OnchainMintQuote, NotificationPayload::MintQuoteOnchainResponse(_))
        | (Kind::OnchainMeltQuote, NotificationPayload::MeltQuoteOnchainResponse(_)) => true,
        (Kind::Custom(kind), NotificationPayload::CustomMintQuoteResponse(method, _)) => {
            kind == &format!("{method}_mint_quote")
        }
        (Kind::Custom(kind), NotificationPayload::CustomMeltQuoteResponse(method, _)) => {
            kind == &format!("{method}_melt_quote")
        }
        _ => false,
    }
}

fuzz_target!(|input: Input| {
    if let Ok(parsed) =
        serde_json::from_str::<RawWsMessageOrResponse<String>>(&input.raw_message)
    {
        let value = serde_json::to_value(&parsed).expect("parsed message must serialize");
        let reparsed: RawWsMessageOrResponse<String> =
            serde_json::from_value(value.clone()).expect("serialized message must reparse");
        assert_eq!(
            serde_json::to_value(reparsed).expect("reparsed message must serialize"),
            value,
            "raw WebSocket round-trip must preserve its JSON shape"
        );
    }

    let (kind, value) = canonical_payload(&input);
    let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(&kind, value.clone())
        .expect("canonical payload must decode for its subscription kind");
    assert!(
        variant_matches(&kind, &decoded),
        "kind-directed decoder selected the wrong payload variant"
    );

    let encoded = serde_json::to_value(&decoded).expect("decoded payload must serialize");
    let decoded_again =
        deserialize_payload_for_kind::<String, serde_json::Error>(&kind, encoded)
            .expect("serialized payload must decode for the same kind");
    assert_eq!(decoded_again, decoded, "payload round-trip mismatch");

    if !matches!(kind, Kind::ProofState) {
        let mut mismatched = value;
        if let Some(object) = mismatched.as_object_mut() {
            object.insert("method".to_string(), json!("definitely-wrong"));
            assert!(
                deserialize_payload_for_kind::<String, serde_json::Error>(&kind, mismatched)
                    .is_err(),
                "kind-directed decoder must reject a mismatched payment method"
            );
        }
    }
});
