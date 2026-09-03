use super::*;
use crate::nuts::{MeltQuoteState, PublicKey, State};

fn element(seed: &str) -> FilterElement {
    FilterElement::new(FilterKind::ProofState, seed.as_bytes(), b"SPENT")
}

fn elements(count: usize) -> Vec<FilterElement> {
    (0..count).map(|i| element(&format!("e{i}"))).collect()
}

fn hex_of(element: &FilterElement) -> String {
    crate::util::hex::encode(*element.as_bytes())
}

const Y: &str = "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2";
const QUOTE_ID: &str = "018f3c1a-7b2e-7c4d-9a10-5f6e7d8c9b0a";

#[test]
fn proof_state_element_matches_vector() {
    let y = PublicKey::from_hex(Y).expect("valid key");

    assert_eq!(
        hex_of(&FilterElement::proof_state(&y, State::Spent).expect("supported")),
        "19f292034075cf6c45502a15b9f1279fae06cbc82ed35ec2f30af29bd081dd5b"
    );
    assert_eq!(
        hex_of(&FilterElement::proof_state(&y, State::Pending).expect("supported")),
        "c756532bf551d835700576dade33c652c5c97b5c82368f306f9a8ede2713751b"
    );
    assert_eq!(
        hex_of(&FilterElement::proof_state(&y, State::Unspent).expect("supported")),
        "4d7871735fa4b0370f5dc9495191e339fcd454597df5740d9d08d854bfa6ade2"
    );
}

#[test]
fn quote_elements_match_vectors() {
    assert_eq!(
        hex_of(&FilterElement::mint_quote(QUOTE_ID)),
        "1f440ebb2a45b4a05a95256bcbe52c3cc8ee88902d204fcd0ab27e616252193d"
    );
    assert_eq!(
        hex_of(&FilterElement::melt_quote(QUOTE_ID, MeltQuoteState::Paid).expect("supported")),
        "09da08ed7532872ec723f43e7cc1803e326c15639f758fde2ea09e746038333a"
    );
    assert_eq!(
        hex_of(&FilterElement::melt_quote(QUOTE_ID, MeltQuoteState::Unpaid).expect("supported")),
        "a27154c8c2858afd2722a92f868af74ed0bfdc24e423f477533f164172444be0"
    );
}

#[test]
fn failed_melt_is_published_as_unpaid() {
    assert_eq!(
        FilterElement::melt_quote(QUOTE_ID, MeltQuoteState::Failed).expect("supported"),
        FilterElement::melt_quote(QUOTE_ID, MeltQuoteState::Unpaid).expect("supported")
    );
}

#[test]
fn quote_id_is_hashed_verbatim() {
    let upper = FilterElement::mint_quote(&QUOTE_ID.to_uppercase());
    let stripped = FilterElement::mint_quote(&QUOTE_ID.replace('-', ""));

    assert_ne!(FilterElement::mint_quote(QUOTE_ID), upper);
    assert_ne!(FilterElement::mint_quote(QUOTE_ID), stripped);
}

#[test]
fn wallet_local_states_have_no_element() {
    let y = PublicKey::from_hex(Y).expect("valid key");

    assert_eq!(
        FilterElement::proof_state(&y, State::Reserved),
        Err(Error::UnsupportedState("RESERVED".to_string()))
    );
    assert!(FilterElement::proof_state(&y, State::PendingSpent).is_err());
    assert!(FilterElement::melt_quote(QUOTE_ID, MeltQuoteState::Unknown).is_err());
}

#[test]
fn kinds_round_trip_through_strings() {
    for kind in [
        FilterKind::ProofState,
        FilterKind::MintQuote,
        FilterKind::MeltQuote,
    ] {
        assert_eq!(kind.to_string().parse::<FilterKind>().expect("known"), kind);
    }
    assert!("swap".parse::<FilterKind>().is_err());
}

#[test]
fn empty_filter_encodes_to_nothing_and_matches_nothing() {
    let data = encode(&[], DEFAULT_P).expect("valid p");
    assert!(data.is_empty());

    let decoded = DecodedFilter::decode(&data, DEFAULT_P).expect("valid");
    assert_eq!(decoded.n(), 0);
    assert!(!decoded.contains(&element("anything")));
}

#[test]
fn every_encoded_element_matches() {
    for count in [1, 2, 3, 17, 500] {
        let set = elements(count);
        let data = encode(&set, DEFAULT_P).expect("valid p");
        let decoded = DecodedFilter::decode(&data, DEFAULT_P).expect("valid");

        assert_eq!(
            decoded.n(),
            count as u64,
            "n recovered for {count} elements"
        );
        for e in &set {
            assert!(
                decoded.contains(e),
                "no false negatives for {count} elements"
            );
        }
    }
}

#[test]
fn n_is_recovered_across_every_padding_length() {
    let mut seen_remainders = std::collections::HashSet::new();

    for count in 1..64usize {
        let set = elements(count);
        let data = encode(&set, MIN_P).expect("valid p");
        let decoded = DecodedFilter::decode(&data, MIN_P).expect("valid");

        assert_eq!(decoded.n(), count as u64);
        seen_remainders.insert(data.len() % 8);
    }

    assert!(
        seen_remainders.len() > 1,
        "expected varied padding across sizes"
    );
}

#[test]
fn duplicate_elements_are_removed_before_counting() {
    let e = element("repeated");
    let data = encode(&[e, e, e], DEFAULT_P).expect("valid p");
    let decoded = DecodedFilter::decode(&data, DEFAULT_P).expect("valid");

    assert_eq!(decoded.n(), 1);
    assert!(decoded.contains(&e));
}

#[test]
fn colliding_positions_survive_a_round_trip() {
    let a = FilterElement::from_bytes(
        crate::util::hex::decode(
            "84f070e7c9e6a5d301dfc1c8077e7a2785f857df2121cc2d599feb26b0d45717",
        )
        .expect("hex")
        .try_into()
        .expect("32 bytes"),
    );
    let b = FilterElement::from_bytes(
        crate::util::hex::decode(
            "8456eb85759144d1d18b4e345261e63f3a178977c92075523da7fd9997bae497",
        )
        .expect("hex")
        .try_into()
        .expect("32 bytes"),
    );
    assert_ne!(a, b);

    let data = encode(&[a, b], MIN_P).expect("valid p");
    let decoded = DecodedFilter::decode(&data, MIN_P).expect("valid");

    assert_eq!(decoded.n(), 2, "a zero delta still counts as a value");
    assert!(decoded.contains(&a));
    assert!(decoded.contains(&b));
}

#[test]
fn position_depends_on_the_filter_it_is_tested_against() {
    let target = element("target");

    let small = encode(&[target], DEFAULT_P).expect("valid p");
    let large = encode(&[vec![target], elements(400)].concat(), DEFAULT_P).expect("valid p");

    let small = DecodedFilter::decode(&small, DEFAULT_P).expect("valid");
    let large = DecodedFilter::decode(&large, DEFAULT_P).expect("valid");

    assert_ne!(small.n(), large.n());
    assert!(small.contains(&target));
    assert!(large.contains(&target));
}

#[test]
fn non_members_rarely_match() {
    let set = elements(200);
    let data = encode(&set, DEFAULT_P).expect("valid p");
    let decoded = DecodedFilter::decode(&data, DEFAULT_P).expect("valid");

    let false_positives = (0..2000)
        .map(|i| element(&format!("outsider{i}")))
        .filter(|e| decoded.contains(e))
        .count();

    assert_eq!(false_positives, 0, "2^-28 should not fire in 2000 tests");
}

#[test]
fn out_of_range_parameters_are_rejected() {
    for p in [0u8, 6, 64, 255] {
        assert_eq!(encode(&[], p), Err(Error::InvalidParameter(p)));
        assert_eq!(
            DecodedFilter::decode(&[], p),
            Err(Error::InvalidParameter(p))
        );
    }

    assert!(encode(&[], MIN_P).is_ok());
    assert!(encode(&[], MAX_P).is_ok());
}

#[test]
fn truncated_code_is_rejected() {
    let data = encode(&elements(4), DEFAULT_P).expect("valid p");
    let truncated = &data[..data.len() - 1];

    let decoded = DecodedFilter::decode(truncated, DEFAULT_P);
    assert!(decoded.is_err() || decoded.expect("checked").n() < 4);
}

#[test]
fn runaway_unary_prefix_is_rejected() {
    let data = vec![0xff; 64];
    assert_eq!(
        DecodedFilter::decode(&data, MIN_P),
        Err(Error::UnexpectedEnd)
    );
}

#[test]
fn filter_decodes_from_hex() {
    let set = elements(5);
    let data = encode(&set, DEFAULT_P).expect("valid p");

    let filter = Filter {
        start: 1701704757,
        end: 1701708357,
        data: crate::util::hex::encode(data),
    };

    let decoded = filter.decode(DEFAULT_P).expect("valid");
    assert_eq!(decoded.n(), 5);
    assert!(decoded.contains(&set[0]));
}

#[test]
fn settings_hide_when_unsupported() {
    assert!(Settings::default().is_empty());
    assert!(!Settings::new(vec![FilterKind::ProofState]).is_empty());

    let settings = Settings::new(vec![FilterKind::ProofState, FilterKind::MeltQuote]);
    let json = serde_json::to_string(&settings).expect("serializes");
    assert_eq!(
        json,
        r#"{"supported":true,"kinds":["proof_state","melt_quote"]}"#
    );
}
