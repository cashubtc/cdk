//! DLEQ proofs on v3 inputs
//!
//! NUT-00: v3 proofs never carry a DLEQ proof, and a receiver treats one that
//! does as malformed. The mint refuses such an input before touching its state.

use cdk_common::dhke::blind_secret;
use cdk_common::nuts::nut02::KeySetVersion;
use cdk_common::nuts::{
    nut12, BlindedMessage, BlindingFactor, CurrencyUnit, ProofDleq, Proofs, ProofsMethods,
    SwapRequest,
};
use cdk_common::secret::Secret;
use cdk_common::{Amount, Error, SecretKey};

use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};
use crate::Mint;

/// A swap of `inputs` into a fresh random output on the active sat keyset.
fn swap_request(mint: &Mint, inputs: Proofs) -> SwapRequest {
    let keyset_id = *mint
        .get_active_keysets()
        .get(&CurrencyUnit::Sat)
        .expect("active sat keyset");
    let (fresh, _) = Secret::generate_for_version(keyset_id.get_version());
    let amount = inputs.total_amount().expect("input total");
    let (blinded_secret, _) =
        blind_secret(&fresh, None, keyset_id.get_version()).expect("blind secret");
    let output = BlindedMessage::new(amount, keyset_id, blinded_secret);

    SwapRequest::new(inputs, vec![output])
}

#[tokio::test]
async fn v3_input_with_dleq_is_rejected() {
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

    let proofs = mint_test_proofs(&mint, Amount::from(2))
        .await
        .expect("v3 inputs");
    assert!(proofs
        .iter()
        .all(|proof| proof.keyset_id.get_version() == KeySetVersion::Version02));

    // `SwapRequest::new` drops DLEQs, but a client can still send one on the
    // wire, so attach it to the built request.
    let mut with_dleq = swap_request(&mint, proofs.clone());
    with_dleq.inputs_mut()[0].dleq = Some(ProofDleq::new(
        SecretKey::generate(),
        SecretKey::generate(),
        BlindingFactor::generate(),
    ));
    let result = mint.process_swap_request(with_dleq).await;
    assert!(
        matches!(result, Err(Error::NUT12(nut12::Error::UnexpectedDleqProof))),
        "expected UnexpectedDleqProof, got: {result:?}"
    );

    // The rejection left the inputs unspent.
    mint.process_swap_request(swap_request(&mint, proofs))
        .await
        .expect("the same inputs without a DLEQ spend");
}
