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

#[tokio::test]
async fn nutroot_disclosure_opens_only_the_exercised_leaf_commitment() {
    use cdk_common::amount::SplitTarget;
    use cdk_common::nuts::nut10::nutroot::{self, Condition, Leaf, NutrootOption};
    use cdk_common::nuts::{PreMintSecrets, SecretKey};
    for disclosure in [false, true] {
        let mint = create_test_mint_with_version(None).await.unwrap();
        let id = mint.get_active_keysets()[&CurrencyUnit::Sat];
        let key = SecretKey::generate();
        let leaf = Leaf::new(
            1,
            vec![*key.public_key().as_secp256k1().unwrap()],
            Condition::Threshold,
            disclosure,
        )
        .unwrap();
        let lock = NutrootOption {
            key: nutroot::nums_point(),
            leaves: Some(vec![cdk_common::util::hex::encode(leaf.to_bytes())]),
            blind_keys: None,
        };
        let premints = PreMintSecrets::with_nutroot(
            id,
            1.into(),
            &SplitTarget::None,
            &lock,
            &(0, vec![1]).into(),
        )
        .unwrap();
        let signatures = mint
            .signatory
            .blind_sign(premints.blinded_messages())
            .await
            .unwrap();
        let keys = mint.keyset_pubkeys(&id).unwrap().keysets[0].keys.clone();
        let mut proofs = cdk_common::dhke::construct_proofs(
            signatures,
            premints.rs(),
            premints.secrets(),
            &keys,
        )
        .unwrap();
        let outputs =
            PreMintSecrets::random(id, 1.into(), &SplitTarget::None, &(0, vec![1]).into()).unwrap();
        let transaction = Transaction::new(&proofs, &[], &outputs.blinded_messages(), &[]).unwrap();
        let info = premints.secrets[0].spend_info.as_ref().unwrap();
        let tree = info.parsed_tree().unwrap().unwrap();
        let mut witness =
            NutrootWitness::script_path(&tree, 0, info.internal_key.unwrap()).unwrap();
        let digest = transaction.input_digest(0).unwrap();
        witness.signatures =
            NutrootWitness::key_path(key.as_secp256k1().unwrap(), digest).signatures;
        let raw = format!(" {} ", serde_json::to_string_pretty(&witness).unwrap());
        proofs[0].witness = Some(Witness::NutrootWitness(raw.clone()));
        let y = proofs[0].y().unwrap();
        let record = nutroot::SpendRecord::new(&proofs[0], &transaction, 0, 0).unwrap();
        let mut pending = (y, cdk_common::State::Pending).into();
        record.apply_to(&mut pending);
        assert!(
            pending.commitment.is_none()
                && pending.witness.is_none()
                && pending.input_digest.is_none()
        );
        mint.process_swap_request(SwapRequest::new(proofs, outputs.blinded_messages()))
            .await
            .unwrap();
        let state = mint
            .check_state(&cdk_common::CheckStateRequest { ys: vec![y] })
            .await
            .unwrap()
            .states
            .remove(0);
        assert_eq!(
            state.commitment,
            Some(cdk_common::util::hex::encode(nutroot::spend_commitment(
                &y, digest, &raw
            )))
        );
        assert_eq!(state.input_digest.is_some(), disclosure);
        assert_eq!(
            state.witness,
            disclosure.then(|| Witness::NutrootWitness(raw))
        );
        let roundtrip: cdk_common::ProofState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(roundtrip, state);
        mint.stop().await.unwrap();
    }
}
