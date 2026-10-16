//! SQLite regressions for quote status persistence and recovery.

use std::sync::Arc;

use cdk_common::database::WalletDatabase;
use cdk_common::wallet::{
    IssueSagaState, MintOperationData, OperationData, WalletSaga, WalletSagaState,
};

use super::*;
use crate::wallet::test_utils::{
    create_test_db, create_test_wallet_with_mock, test_mint_quote, test_mint_url, MockMintConnector,
};

fn paid_response(quote: &MintQuote) -> MintQuoteResponse<String> {
    MintQuoteResponse::Bolt11(cdk_common::MintQuoteBolt11Response {
        quote: quote.id.clone(),
        request: quote.request.clone(),
        amount: quote.amount,
        unit: Some(quote.unit.clone()),
        method: quote.payment_method.clone(),
        amount_paid: Amount::from(1_000),
        amount_issued: Amount::ZERO,
        updated_at: 1,
        state: MintQuoteState::Paid,
        expiry: Some(quote.expiry),
        pubkey: None,
    })
}

#[tokio::test]
async fn quote_status_preserves_recovery_ownership() {
    // An orphan and a compensated saga release ownership; a saga whose
    // recovery data is unavailable must stay reserved for a later retry.
    for state in [
        None,
        Some(IssueSagaState::SecretsPrepared),
        Some(IssueSagaState::MintRequested),
    ] {
        let db = create_test_db().await;
        let client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), client.clone()).await;
        let quote = test_mint_quote(test_mint_url());
        db.add_mint_quote(quote.clone()).await.unwrap();
        let operation_id = uuid::Uuid::new_v4();
        db.reserve_mint_quote(&quote.id, &operation_id)
            .await
            .unwrap();
        let pending = state == Some(IssueSagaState::MintRequested);
        if let Some(state) = state {
            db.add_saga(WalletSaga::new(
                operation_id,
                WalletSagaState::Issue(state),
                Amount::from(1_000),
                quote.mint_url.clone(),
                quote.unit.clone(),
                OperationData::Mint(MintOperationData::new_single(
                    quote.id.clone(),
                    Amount::from(1_000),
                    None,
                    None,
                    None,
                )),
            ))
            .await
            .unwrap();
        }
        client.set_mint_quote_status_response(&quote.id, paid_response(&quote));

        for expected_version in 1..=2 {
            let returned = wallet.check_mint_quote_status(&quote.id).await.unwrap();
            let stored = db.get_mint_quote(&quote.id).await.unwrap().unwrap();
            assert_eq!(returned, stored);
            assert_eq!(returned.version, expected_version);
            assert_eq!(returned.amount_paid, Amount::from(1_000));
            assert_eq!(returned.amount_issued, Amount::ZERO);
            assert_eq!(
                returned.used_by_operation,
                pending.then(|| operation_id.to_string())
            );
            assert_eq!(db.get_saga(&operation_id).await.unwrap().is_some(), pending);
        }
        assert!(db
            .get_proofs(None, None, None, None)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn quote_status_returns_state_after_successful_recovery() {
    use crate::amount::FeeAndAmounts;
    use crate::nuts::PreMintSecrets;

    let db = create_test_db().await;
    let client = Arc::new(MockMintConnector::new());
    client.enable_mint_signing();
    let keyset_id = client.keysets.lock().unwrap()[0].id;
    let wallet = create_test_wallet_with_mock(db.clone(), client.clone()).await;
    let mut quote = test_mint_quote(test_mint_url());
    quote.amount = Some(Amount::from(1));
    quote.amount_paid = Amount::from(1);
    quote.update_state_from_amounts();
    db.add_mint_quote(quote.clone()).await.unwrap();
    let operation_id = uuid::Uuid::new_v4();
    db.reserve_mint_quote(&quote.id, &operation_id)
        .await
        .unwrap();
    let secrets = PreMintSecrets::from_seed(
        keyset_id,
        0,
        &wallet.seed,
        Amount::from(1),
        &SplitTarget::None,
        &FeeAndAmounts::from((0, vec![1])),
    )
    .unwrap();
    db.add_saga(WalletSaga::new(
        operation_id,
        WalletSagaState::Issue(IssueSagaState::MintRequested),
        Amount::from(1),
        quote.mint_url.clone(),
        quote.unit.clone(),
        OperationData::Mint(MintOperationData::new_single(
            quote.id.clone(),
            Amount::from(1),
            Some(0),
            Some(1),
            Some(secrets.blinded_messages()),
        )),
    ))
    .await
    .unwrap();
    let mut response = paid_response(&quote);
    if let MintQuoteResponse::Bolt11(response) = &mut response {
        response.amount_paid = Amount::from(1);
        response.amount_issued = Amount::from(1);
        response.state = MintQuoteState::Issued;
    }
    client.set_mint_quote_status_response(&quote.id, response);

    let returned = wallet.check_mint_quote_status(&quote.id).await.unwrap();
    let stored = db.get_mint_quote(&quote.id).await.unwrap().unwrap();
    assert_eq!(returned, stored);
    assert_eq!(returned.version, 2); // Recovery write, then status write.
    assert_eq!(returned.amount_paid, Amount::from(1));
    assert_eq!(returned.amount_issued, Amount::from(1));
    assert_eq!(returned.state, MintQuoteState::Issued);
    assert!(db.get_saga(&operation_id).await.unwrap().is_none());
    assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(1));

    // The next check cleans up any reservation left by completed recovery.
    let returned = wallet.check_mint_quote_status(&quote.id).await.unwrap();
    assert_eq!(
        returned,
        db.get_mint_quote(&quote.id).await.unwrap().unwrap()
    );
    assert_eq!(returned.used_by_operation, None);
    assert_eq!(returned.amount_issued, Amount::from(1));
    assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(1));
}

#[tokio::test]
async fn quote_status_propagates_reload_failures() {
    // Inject failures only after the optimistic UPDATE has succeeded. This
    // exercises the public APIs against real SQLite writes, not a mock that
    // silently retains the input version.
    for missing in [true, false] {
        for api in ["single", "batch", "fetch", "all"] {
            let path = std::env::temp_dir()
                .join(format!("cdk-quote-status-{}.sqlite", uuid::Uuid::new_v4()));
            let db = Arc::new(cdk_sqlite::WalletSqliteDatabase::new(&path).await.unwrap());
            let client = Arc::new(MockMintConnector::new());
            let wallet = create_test_wallet_with_mock(db.clone(), client.clone()).await;
            let quote = test_mint_quote(test_mint_url());
            db.add_mint_quote(quote.clone()).await.unwrap();
            client.set_mint_quote_status_response(&quote.id, paid_response(&quote));
            client
                .push_post_batch_check_mint_quote_status_response(Ok(vec![paid_response(&quote)]));
            let connection = rusqlite::Connection::open(&path).unwrap();
            let action = match missing {
                true => "DELETE FROM mint_quote WHERE id = NEW.id;",
                false => "UPDATE mint_quote SET state = 'invalid-state' WHERE id = NEW.id;",
            };
            connection
                .execute_batch(&format!(
                    "CREATE TRIGGER fail_reload AFTER UPDATE OF version ON mint_quote
                     BEGIN {action} END;"
                ))
                .unwrap();

            let result = match api {
                "single" => wallet.check_mint_quote_status(&quote.id).await,
                "batch" => wallet
                    .batch_check_mint_quote_status(&[&quote.id])
                    .await
                    .map(|mut quotes| quotes.remove(0)),
                "fetch" => wallet.fetch_mint_quote(&quote.id, None).await,
                _ => {
                    // Keep the existing best-effort contract: failed quotes
                    // are omitted, never returned with a stale snapshot.
                    assert!(wallet.check_all_mint_quotes().await.unwrap().is_empty());
                    Err(Error::UnknownQuote)
                }
            };
            if api != "all" {
                match missing {
                    true => assert!(matches!(result, Err(Error::UnknownQuote)), "{result:?}"),
                    false => assert!(matches!(result, Err(Error::Database(_))), "{result:?}"),
                }
            }
            // Prove that failure was injected after the write, not on the
            // initial read or status request.
            let versions: Vec<u32> = connection
                .prepare("SELECT version FROM mint_quote")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(versions, if missing { vec![] } else { vec![1] });
            drop(wallet);
            drop(db);
            drop(connection);
            std::fs::remove_file(path).unwrap();
        }
    }
}
