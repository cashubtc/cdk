use cdk_common::amount::SplitTarget;
use cdk_common::wallet::{
    OperationData, SwapOperationData, SwapSagaState, WalletSaga, WalletSagaState,
};

use crate::nuts::{Id, P2PKWitness, PreMintSecrets, Witness};
use crate::wallet::test_utils::{create_test_db, create_test_wallet};

#[tokio::test]
async fn random_nutroot_outputs_survive_saga_storage() {
    let db = create_test_db().await;
    let wallet = create_test_wallet(db.clone()).await;
    let id = Id::from_bytes(&[vec![2], vec![42; 32]].concat()).unwrap();
    let mut premints =
        PreMintSecrets::random(id, 7.into(), &SplitTarget::None, &(0, vec![1, 2, 4]).into())
            .unwrap();
    premints.secrets.reverse();
    let outputs = premints.blinded_messages();
    let saga = WalletSaga::new(
        uuid::Uuid::new_v4(),
        WalletSagaState::Swap(SwapSagaState::SwapRequested),
        7.into(),
        wallet.mint_url.clone(),
        wallet.unit.clone(),
        OperationData::Swap(SwapOperationData {
            input_amount: 7.into(),
            output_amount: 7.into(),
            counter_start: Some(0),
            counter_end: Some(0),
            blinded_messages: Some(outputs.clone()),
            premint_secrets: Some(premints.clone()),
        }),
    );
    // Exercise serialized storage and a new wallet instance, not in-memory state.
    let saga_id = saga.id;
    db.add_saga(saga).await.unwrap();
    let restarted = create_test_wallet(db).await;
    let recovered = restarted
        .recover_premint_secrets(&saga_id, &outputs, 0, 0)
        .await
        .unwrap();
    assert_eq!(recovered, premints);
    for premint in recovered.secrets {
        premint
            .spend_info
            .unwrap()
            .key_path_key(&premint.secret.to_string(), None)
            .unwrap();
    }
    let mut signed = outputs.clone();
    for output in &mut signed {
        output.witness = Some(Witness::P2PKWitness(P2PKWitness {
            signatures: vec!["00".repeat(64)],
        }));
    }
    assert_eq!(
        restarted
            .recover_premint_secrets(&saga_id, &signed, 0, 0)
            .await
            .unwrap(),
        premints
    );
    let mut wrong = outputs.clone();
    wrong.reverse();
    assert!(restarted
        .recover_premint_secrets(&saga_id, &wrong, 0, 0)
        .await
        .is_err());
}

#[tokio::test]
async fn swap_replay_falls_back_when_a_v3_input_cannot_be_signed() {
    use std::str::FromStr;
    use std::sync::Arc;

    use cdk_common::wallet::ProofInfo;

    use super::RecoveryHelpers;
    use crate::nuts::{CurrencyUnit, Proof, PublicKey, State};
    use crate::secret::Secret;
    use crate::wallet::test_utils::{
        create_test_wallet_with_mock, test_keyset_id, MockMintConnector,
    };

    let db = create_test_db().await;
    let wallet = create_test_wallet_with_mock(db.clone(), Arc::new(MockMintConnector::new())).await;
    let premints = PreMintSecrets::random(
        test_keyset_id(),
        8.into(),
        &SplitTarget::None,
        &(0, vec![8]).into(),
    )
    .unwrap();
    let outputs = premints.blinded_messages();
    let saga = WalletSaga::new(
        uuid::Uuid::new_v4(),
        WalletSagaState::Swap(SwapSagaState::SwapRequested),
        8.into(),
        wallet.mint_url.clone(),
        wallet.unit.clone(),
        OperationData::Swap(SwapOperationData {
            input_amount: 8.into(),
            output_amount: 8.into(),
            counter_start: Some(0),
            counter_end: Some(0),
            blinded_messages: Some(outputs.clone()),
            premint_secrets: Some(premints),
        }),
    );
    let saga_id = saga.id;
    db.add_saga(saga).await.unwrap();
    // The NUT-10 swap vector's input: a v3 proof whose key this wallet does not hold.
    let proof = Proof {
        amount: 8.into(),
        keyset_id: Id::from_str("022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701").unwrap(),
        secret: Secret::from_str("0240c186f3e19f6f8cd00db81c5266d5c74ddca48ff848efd0e3e125b2846f30d9").unwrap(),
        c: PublicKey::from_hex("a33dd4de26ea950060ae5c45bdc408d72aaf51ba9f4ccdeee4a040df067ca87c799a406fec88c2a328b75b290e27f7b6").unwrap(),
        witness: None,
        dleq: None,
        p2pk_e: None,
        spend_info: None,
    };
    let input = ProofInfo::new(
        proof,
        wallet.mint_url.clone(),
        State::Reserved,
        CurrencyUnit::Sat,
    )
    .unwrap();
    let replayed = wallet
        .try_replay_swap_request(&saga_id, "swap", Some(&outputs), Some(0), Some(0), &[input])
        .await;
    assert!(matches!(replayed, Ok(None)), "{replayed:?}");
}
