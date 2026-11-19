//! Spent tracking for one secret signed on both curves
//!
//! `Y` depends on the keyset's curve, so the mint tracks spent state per
//! curve. These tests pin that behaviour: a secret signed under a secp256k1
//! keyset and a BLS12-381 (v3) keyset can be spent once on each, and never
//! twice on the same one.

use cdk_common::dhke::{blind_secret, construct_proofs};
use cdk_common::nuts::nut02::KeySetVersion;
use cdk_common::nuts::{BlindedMessage, CurrencyUnit, Proof, Proofs, ProofsMethods, SwapRequest};
use cdk_common::secret::Secret;
use cdk_common::{Amount, Error};

use crate::test_helpers::mint::{
    create_test_mint, mint_test_proofs, mint_v3_test_proofs, sign_v3_inputs, V3Keys,
};
use crate::Mint;

/// Sign any v3 inputs, then swap them
async fn swap(
    mint: &Mint,
    inputs: Proofs,
    outputs: Vec<BlindedMessage>,
    keys: &V3Keys,
) -> Result<cdk_common::SwapResponse, Error> {
    let mut request = SwapRequest::new(inputs, outputs);
    let transaction = request.transaction().expect("transaction");
    sign_v3_inputs(&transaction, request.inputs_mut(), keys).expect("sign");
    mint.process_swap_request(request).await
}

/// Swap `inputs` into one output for `secret` on the active sat keyset.
async fn sign_secret(mint: &Mint, inputs: Proofs, secret: &Secret, v3_keys: &V3Keys) -> Proof {
    let keyset_id = *mint
        .get_active_keysets()
        .get(&CurrencyUnit::Sat)
        .expect("active sat keyset");
    let keys = mint
        .keyset_pubkeys(&keyset_id)
        .expect("keyset keys")
        .keysets
        .first()
        .expect("one keyset")
        .keys
        .clone();

    let amount = inputs.total_amount().expect("input total");
    let (blinded_secret, r) =
        blind_secret(secret, None, keyset_id.get_version()).expect("blind secret");
    let output = BlindedMessage::new(amount, keyset_id, blinded_secret);

    let response = swap(mint, inputs, vec![output], v3_keys)
        .await
        .expect("swap into the chosen secret");

    construct_proofs(response.signatures, vec![r], vec![secret.clone()], &keys)
        .expect("construct proof")
        .pop()
        .expect("one proof")
}

/// Swap `inputs` into a fresh random output.
async fn spend(mint: &Mint, inputs: Proofs, keys: &V3Keys) -> Result<(), Error> {
    let keyset_id = *mint
        .get_active_keysets()
        .get(&CurrencyUnit::Sat)
        .expect("active sat keyset");
    let (fresh, _) = Secret::generate_for_version(keyset_id.get_version());
    let amount = inputs.total_amount().expect("input total");
    let (blinded_secret, _) =
        blind_secret(&fresh, None, keyset_id.get_version()).expect("blind secret");
    let output = BlindedMessage::new(amount, keyset_id, blinded_secret);

    swap(mint, inputs, vec![output], keys).await.map(|_| ())
}

#[tokio::test]
async fn one_secret_is_spendable_once_per_curve() {
    let mint = create_test_mint().await.expect("test mint");
    // A v3 secret must be a point; its hex string is also a valid pre-v3 secret.
    let (secret, secret_key) = Secret::generate_v3();
    let mut keys = V3Keys::from([(secret_key.public_key(), secret_key)]);

    // Sign the secret under the default secp256k1 keyset.
    let legacy_inputs = mint_test_proofs(&mint, Amount::from(1))
        .await
        .expect("legacy inputs");
    let legacy = sign_secret(&mint, legacy_inputs, &secret, &keys).await;
    assert_ne!(legacy.keyset_id.get_version(), KeySetVersion::Version02);

    // Rotate to a v3 keyset and sign the same secret there.
    mint.rotate_keyset_by_version(
        CurrencyUnit::Sat,
        (0..32).map(|i| 2u64.pow(i)).collect(),
        0,
        KeySetVersion::Version02,
        None,
    )
    .await
    .expect("rotate to v3");
    let (bls_inputs, input_keys) = mint_v3_test_proofs(&mint, Amount::from(1))
        .await
        .expect("v3 inputs");
    keys.extend(input_keys);
    let bls = sign_secret(&mint, bls_inputs, &secret, &keys).await;
    assert_eq!(bls.keyset_id.get_version(), KeySetVersion::Version02);

    assert_eq!(legacy.secret, bls.secret);
    assert_ne!(legacy.y().expect("legacy Y"), bls.y().expect("v3 Y"));

    // The two proofs are distinct inputs, so one swap may spend both.
    spend(&mint, vec![legacy.clone(), bls.clone()], &keys)
        .await
        .expect("both curves spend once");

    // Neither can be spent again.
    for proof in [legacy, bls] {
        let result = spend(&mint, vec![proof], &keys).await;
        assert!(
            matches!(result, Err(Error::TokenAlreadySpent)),
            "expected TokenAlreadySpent, got: {result:?}"
        );
    }
}
