//! End-to-end authorization with actual BLS signatures and mint storage.
use cdk_common::nuts::nut10::nutroot::{Transaction, Witness as NutrootWitness};
use cdk_common::nuts::{CurrencyUnit, SwapRequest, Witness};

use crate::test_helpers::mint::{
    create_test_blinded_messages, create_test_mint_with_version, mint_test_proofs,
};

#[tokio::test]
async fn nutroot_mint_and_swap_require_transaction_authorization() {
    let mint = create_test_mint_with_version(None).await.unwrap();
    let proofs = mint_test_proofs(&mint, 8.into()).await.unwrap();
    let (outputs, _) = create_test_blinded_messages(&mint, 8.into()).await.unwrap();
    let mut request = SwapRequest::new(proofs, outputs);
    assert!(mint.process_swap_request(request.clone()).await.is_err());

    let transaction = Transaction::new(request.inputs(), &[], request.outputs(), &[]).unwrap();
    for (index, proof) in request.inputs_mut().iter_mut().enumerate() {
        let key = proof
            .spend_info
            .as_ref()
            .unwrap()
            .key_path_key(&proof.secret.to_string(), None)
            .unwrap();
        proof.witness = Some(Witness::NutrootWitness(
            serde_json::to_string_pretty(&NutrootWitness::key_path(
                &key,
                transaction.input_digest(index).unwrap(),
            ))
            .unwrap(),
        ));
        proof.spend_info = None;
    }
    let mut redirected = request.clone();
    let (other_outputs, _) = create_test_blinded_messages(&mint, 8.into()).await.unwrap();
    *redirected.outputs_mut() = other_outputs;
    assert!(mint.process_swap_request(redirected).await.is_err());

    let ys = cdk_common::nuts::ProofsMethods::ys(request.inputs()).unwrap();
    let expected_witnesses: Vec<_> = request
        .inputs()
        .iter()
        .map(|proof| proof.witness.clone())
        .collect();
    let response = mint.process_swap_request(request).await.unwrap();
    let stored = mint.localstore.get_proofs_by_ys(&ys).await.unwrap();
    for (proof, witness) in stored.iter().zip(expected_witnesses) {
        assert_eq!(proof.as_ref().unwrap().witness, witness);
    }
    let states = mint
        .check_state(&cdk_common::nuts::CheckStateRequest { ys })
        .await
        .unwrap();
    assert!(states
        .states
        .iter()
        .all(|state| state.state == cdk_common::nuts::State::Spent && state.witness.is_none()));
    assert!(!response.signatures.is_empty());
    assert_eq!(mint.get_active_keysets().len(), 1);
    assert_eq!(
        mint.get_active_keysets()[&CurrencyUnit::Sat].get_version(),
        cdk_common::nuts::KeySetVersion::Version02
    );
    mint.stop().await.unwrap();
}
