//! v3 (nutroot) inputs on the mint: the NUT-00 signing rule, NUT-07 state
//! and v3 issuance

use cdk_common::dhke::{blind_secret, construct_proofs};
use cdk_common::nut00::transaction::MintQuoteState as QuoteInputState;
use cdk_common::nuts::nut02::KeySetVersion;
use cdk_common::nuts::nut10::nutroot::{sign_input_key_path, LeafType, NutrootWitness};
use cdk_common::nuts::{
    BlindedMessage, CheckStateRequest, CurrencyUnit, PreMintSecrets, Proofs, ProofsMethods,
    SwapRequest, Witness,
};
use cdk_common::secret::Secret;
use cdk_common::{
    Amount, Error, MintQuoteBolt11Request, MintQuoteBolt11Response, MintRequest, ProofDleq,
    SecretKey, State,
};

use std::str::FromStr;

use crate::mint::MintInput;
use crate::test_helpers::mint::{create_test_mint, mint_v3_test_proofs, sign_v3_inputs, V3Keys};
use crate::Mint;

async fn v3_mint() -> Mint {
    let mint = create_test_mint().await.expect("test mint");
    mint.rotate_keyset_by_version(
        CurrencyUnit::Sat,
        (0..32).map(|i| 2u64.pow(i)).collect(),
        0,
        KeySetVersion::Version02,
        None,
    )
    .await
    .expect("rotate to v3");
    mint
}

/// One fresh v3 output for `amount`, with its key
fn fresh_output(mint: &Mint, amount: Amount) -> (BlindedMessage, V3Keys) {
    let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
    let (secret, key) = Secret::generate_v3();
    let (blinded, _) = blind_secret(&secret, None, keyset_id.get_version()).unwrap();
    (
        BlindedMessage::new(amount, keyset_id, blinded),
        V3Keys::from([(key.public_key(), key)]),
    )
}

#[tokio::test]
async fn signed_v3_swap_succeeds_and_reports_its_witness() {
    let mint = v3_mint().await;
    let (inputs, keys) = mint_v3_test_proofs(&mint, Amount::from(8)).await.unwrap();
    let (output, _) = fresh_output(&mint, Amount::from(8));

    let mut request = SwapRequest::new(inputs.clone(), vec![output]);
    let transaction = request.transaction().unwrap();
    sign_v3_inputs(&transaction, request.inputs_mut(), &keys).unwrap();
    let sent_witness = request.inputs()[0].witness.clone();
    mint.process_swap_request(request).await.unwrap();

    // NUT-07: a spent v3 proof carries its witness, as received, and the
    // input digest it signed.
    let ys = inputs.ys().unwrap();
    let states = mint
        .check_state(&CheckStateRequest { ys: ys.clone() })
        .await
        .unwrap()
        .states;
    assert_eq!(states[0].state, State::Spent);
    assert_eq!(states[0].witness, sent_witness);
    assert!(matches!(states[0].witness, Some(Witness::Nutroot(_))));
    assert_eq!(
        states[0].input_digest,
        Some(transaction.input_digest(0).unwrap())
    );

    // The JSON response round-trips the v3 witness and digest.
    let json = serde_json::to_string(&states[0]).unwrap();
    assert!(json.contains("input_digest"));
    let parsed: cdk_common::ProofState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, states[0]);
}

#[tokio::test]
async fn unsigned_or_missigned_v3_inputs_are_rejected() {
    let mint = v3_mint().await;
    let (inputs, keys) = mint_v3_test_proofs(&mint, Amount::from(8)).await.unwrap();
    let (output, _) = fresh_output(&mint, Amount::from(8));

    // No witness.
    let result = mint
        .process_swap_request(SwapRequest::new(inputs.clone(), vec![output.clone()]))
        .await;
    assert!(
        matches!(result, Err(Error::SignatureMissingOrInvalid)),
        "{result:?}"
    );

    // Signed by another key.
    let mut request = SwapRequest::new(inputs.clone(), vec![output.clone()]);
    let transaction = request.transaction().unwrap();
    let other: V3Keys = keys
        .keys()
        .map(|point| (*point, SecretKey::generate()))
        .collect();
    sign_v3_inputs(&transaction, request.inputs_mut(), &other).unwrap();
    let result = mint.process_swap_request(request).await;
    assert!(
        matches!(result, Err(Error::SignatureMissingOrInvalid)),
        "{result:?}"
    );

    // Signed for a different transaction: the witness binds the outputs.
    let (other_output, _) = fresh_output(&mint, Amount::from(8));
    let mut signed_elsewhere = SwapRequest::new(inputs.clone(), vec![other_output]);
    let transaction = signed_elsewhere.transaction().unwrap();
    sign_v3_inputs(&transaction, signed_elsewhere.inputs_mut(), &keys).unwrap();
    let request = SwapRequest::new(signed_elsewhere.inputs().clone(), vec![output.clone()]);
    let result = mint.process_swap_request(request).await;
    assert!(
        matches!(result, Err(Error::SignatureMissingOrInvalid)),
        "{result:?}"
    );

    // A DLEQ proof on a v3 input is malformed.
    let mut request = SwapRequest::new(inputs.clone(), vec![output.clone()]);
    let transaction = request.transaction().unwrap();
    sign_v3_inputs(&transaction, request.inputs_mut(), &keys).unwrap();
    request.inputs_mut()[0].dleq = Some(ProofDleq::new(
        SecretKey::generate(),
        SecretKey::generate(),
        SecretKey::generate().into(),
    ));
    let result = mint.process_swap_request(request).await;
    assert!(matches!(result, Err(Error::NUT12(_))), "{result:?}");

    // None of these spent the proofs.
    let states = mint
        .check_state(&CheckStateRequest {
            ys: inputs.ys().unwrap(),
        })
        .await
        .unwrap()
        .states;
    assert!(states.iter().all(|state| state.state == State::Unspent));
    assert!(states.iter().all(|state| state.witness.is_none()));
}

#[tokio::test]
async fn a_witness_cannot_be_replayed_with_extra_outputs() {
    let mint = v3_mint().await;
    let (inputs, keys) = mint_v3_test_proofs(&mint, Amount::from(8)).await.unwrap();
    let (half_a, _) = fresh_output(&mint, Amount::from(4));
    let (half_b, _) = fresh_output(&mint, Amount::from(4));

    let mut request = SwapRequest::new(inputs, vec![half_a.clone(), half_b]);
    let transaction = request.transaction().unwrap();
    sign_v3_inputs(&transaction, request.inputs_mut(), &keys).unwrap();

    // Swapping one output for another of the same amount changes the
    // transcript, so the witness no longer verifies.
    let (thief, _) = fresh_output(&mint, Amount::from(4));
    let tampered = SwapRequest::new(request.inputs().clone(), vec![half_a, thief]);
    let result = mint.process_swap_request(tampered).await;
    assert!(
        matches!(result, Err(Error::SignatureMissingOrInvalid)),
        "{result:?}"
    );
    mint.process_swap_request(request).await.unwrap();
}

#[tokio::test]
async fn v3_minting_needs_a_locked_quote_and_a_valid_transcript_signature() {
    let mint = v3_mint().await;
    let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
    let paid_quote = |pubkey| {
        let mint = &mint;
        async move {
            let quote: MintQuoteBolt11Response<_> = mint
                .get_mint_quote(
                    MintQuoteBolt11Request {
                        amount: Amount::from(8),
                        unit: CurrencyUnit::Sat,
                        description: None,
                        pubkey,
                    }
                    .into(),
                )
                .await
                .unwrap()
                .into();
            let id = cdk_common::QuoteId::from_str(&quote.quote).unwrap();
            loop {
                let check: MintQuoteBolt11Response<_> = mint
                    .check_mint_quotes(std::slice::from_ref(&id), None)
                    .await
                    .unwrap()
                    .first()
                    .unwrap()
                    .clone()
                    .into();
                if check.state == cdk_common::MintQuoteState::Paid {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            id
        }
    };
    let outputs = || {
        let (secret, _) = Secret::generate_v3();
        PreMintSecrets::from_secrets(keyset_id, vec![Amount::from(8)], vec![secret]).unwrap()
    };

    // Unlocked quote: 20009.
    let unlocked = paid_quote(None).await;
    let request = MintRequest {
        quote: unlocked,
        outputs: outputs().blinded_messages(),
        signature: None,
    };
    let result = mint.process_mint_request(MintInput::Single(request)).await;
    assert!(matches!(result, Err(Error::PubkeyRequired)), "{result:?}");

    // Locked quote, signature over the wrong `issued`: 20008.
    let lock = SecretKey::generate();
    let locked = paid_quote(Some(lock.public_key())).await;
    let premint = outputs();
    let mut request = MintRequest {
        quote: locked.clone(),
        outputs: premint.blinded_messages(),
        signature: None,
    };
    let wrong = request
        .transaction(&QuoteInputState {
            lock_key: lock.public_key(),
            issued: Amount::from(1),
        })
        .unwrap();
    request.signature = Some(sign_input_key_path(&wrong, 0, &lock).unwrap().to_string());
    let result = mint
        .process_mint_request(MintInput::Single(request.clone()))
        .await;
    assert!(
        matches!(result, Err(Error::SignatureMissingOrInvalid)),
        "{result:?}"
    );

    // The legacy NUT-20 signature does not satisfy a v3 mint either.
    let mut legacy = request.clone();
    legacy.signature = None;
    legacy.sign(&lock).unwrap();
    let result = mint.process_mint_request(MintInput::Single(legacy)).await;
    assert!(
        matches!(result, Err(Error::SignatureMissingOrInvalid)),
        "{result:?}"
    );

    // The right transcript, as a bare signature: accepted.
    let right = request
        .transaction(&QuoteInputState {
            lock_key: lock.public_key(),
            issued: Amount::ZERO,
        })
        .unwrap();
    let witness = sign_input_key_path(&right, 0, &lock).unwrap();
    request.signature = Some(cdk_common::util::hex::encode(
        witness.signatures[0].serialize(),
    ));
    let response = mint
        .process_mint_request(MintInput::Single(request))
        .await
        .unwrap();
    let keys = mint
        .keyset_pubkeys(&keyset_id)
        .unwrap()
        .keysets
        .first()
        .unwrap()
        .keys
        .clone();
    construct_proofs(response.signatures, premint.rs(), premint.secrets(), &keys).unwrap();
}

#[tokio::test]
async fn non_point_v3_secret_is_rejected() {
    let mint = v3_mint().await;
    let (mut inputs, _) = mint_v3_test_proofs(&mint, Amount::from(8)).await.unwrap();
    inputs[0].secret = Secret::generate();
    let (output, _) = fresh_output(&mint, Amount::from(8));
    let result = mint
        .process_swap_request(SwapRequest::new(inputs, vec![output]))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn mint_info_lists_core_leaves_with_v3_keysets() {
    let mint = create_test_mint().await.unwrap();
    assert!(mint.mint_info().await.unwrap().nuts.nut10.leaves.is_empty());
    let mint = v3_mint().await;
    let info = mint.mint_info().await.unwrap();
    for leaf in LeafType::CORE {
        assert!(info.nuts.nut10.supports_leaf(&leaf.signal()));
    }
    assert!(info.pubkey.is_some());
}

#[tokio::test]
async fn key_path_witness_is_kept_byte_for_byte() {
    // A witness has no canonical encoding; the mint stores and echoes the
    // exact string it was sent.
    let mint = v3_mint().await;
    let (inputs, keys) = mint_v3_test_proofs(&mint, Amount::from(8)).await.unwrap();
    let (output, _) = fresh_output(&mint, Amount::from(8));
    let mut request = SwapRequest::new(inputs.clone(), vec![output]);
    let transaction = request.transaction().unwrap();
    sign_v3_inputs(&transaction, request.inputs_mut(), &keys).unwrap();
    let signature = request.inputs()[0]
        .witness
        .as_ref()
        .and_then(Witness::as_nutroot)
        .unwrap()
        .parse()
        .unwrap()
        .signatures[0];
    let spaced = format!(
        "{{ \"signatures\" : [ \"{}\" ] }}",
        cdk_common::util::hex::encode(signature.serialize())
    );
    let raw: cdk_common::nuts::nut10::nutroot::RawNutrootWitness =
        serde_json::from_value(serde_json::json!(spaced)).unwrap();
    request.inputs_mut()[0].witness = Some(Witness::Nutroot(raw));
    let _: Proofs = request.inputs().clone();
    let _ = NutrootWitness::key_path(signature);
    mint.process_swap_request(request).await.unwrap();
    let states = mint
        .check_state(&CheckStateRequest {
            ys: inputs.ys().unwrap(),
        })
        .await
        .unwrap()
        .states;
    let Some(Witness::Nutroot(stored)) = &states[0].witness else {
        panic!("nutroot witness");
    };
    assert_eq!(stored.as_str(), spaced);
}
