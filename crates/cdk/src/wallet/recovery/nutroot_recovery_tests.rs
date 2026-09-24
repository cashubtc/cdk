use cdk_common::amount::SplitTarget;
use cdk_common::wallet::{
    OperationData, SwapOperationData, SwapSagaState, WalletSaga, WalletSagaState,
};

use crate::nuts::{Id, PreMintSecrets};
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
    let mut wrong = outputs.clone();
    wrong.reverse();
    assert!(restarted
        .recover_premint_secrets(&saga_id, &wrong, 0, 0)
        .await
        .is_err());
}
