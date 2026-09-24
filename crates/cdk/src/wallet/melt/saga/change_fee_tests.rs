//! A paid melt must retain recovery until change fits its original fee budget.
use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::wallet::{
    MeltOperationData, MeltQuote, MeltSagaState, OperationData, TransactionStatus, WalletSaga,
    WalletSagaState,
};

use crate::nuts::{
    BlindSignature, CurrencyUnit, KeySetInfo, MeltQuoteBolt11Response, MeltQuoteState,
    PaymentMethod, PreMintSecrets, RestoreResponse, State,
};
use crate::wallet::test_utils::{
    create_test_db, create_test_wallet_with_mock, test_keyset, test_melt_quote, test_mint_url,
    test_proof_info, MockMintConnector,
};
use crate::{Amount, Wallet};

struct Fixture {
    wallet: Wallet,
    client: Arc<MockMintConnector>,
    saga: WalletSaga,
    quote: MeltQuote,
    secrets: PreMintSecrets,
}

impl Fixture {
    async fn new(reserve: u64) -> Self {
        let db = create_test_db().await;
        let client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), client.clone()).await;
        let mut keyset = test_keyset();
        keyset.input_fee_ppk = 0;
        db.add_mint(test_mint_url(), None).await.unwrap();
        db.add_mint_keysets(
            test_mint_url(),
            vec![KeySetInfo {
                id: keyset.id,
                unit: keyset.unit.clone(),
                active: true,
                input_fee_ppk: 0,
                final_expiry: None,
            }],
        )
        .await
        .unwrap();
        db.add_keys(keyset.clone()).await.unwrap();
        let id = uuid::Uuid::new_v4();
        let mut quote = test_melt_quote();
        quote.id = id.to_string();
        quote.amount = Amount::from(8);
        quote.fee_reserve = Amount::from(reserve);
        db.add_melt_quote(quote.clone()).await.unwrap();
        let mut input = test_proof_info(keyset.id, 32, test_mint_url());
        input.state = State::Pending;
        input.used_by_operation = Some(id);
        db.update_proofs(vec![input.clone()], vec![]).await.unwrap();
        let secrets =
            PreMintSecrets::from_seed_blank(keyset.id, 0, &wallet.seed, Amount::from(24)).unwrap();
        let saga = WalletSaga::new(
            id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(8),
            test_mint_url(),
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                premint_secrets: None,
                quote_id: quote.id.clone(),
                amount: quote.amount,
                fee_reserve: quote.fee_reserve,
                counter_start: Some(0),
                counter_end: Some(secrets.len() as u32),
                change_amount: Some(Amount::from(24)),
                metadata: HashMap::from([("purpose".to_owned(), "recovery test".to_owned())]),
                final_proof_ys: Some(vec![input.y]),
                change_blinded_messages: Some(secrets.blinded_messages()),
            }),
        );
        db.add_saga(saga.clone()).await.unwrap();
        Self {
            wallet,
            client,
            saga,
            quote,
            secrets,
        }
    }

    fn response(&self, amounts: &[u64]) -> RestoreResponse {
        let outputs = self
            .secrets
            .blinded_messages()
            .into_iter()
            .take(amounts.len())
            .collect::<Vec<_>>();
        let signatures = outputs
            .iter()
            .zip(amounts)
            .map(|(message, amount)| BlindSignature {
                amount: Amount::from(*amount),
                keyset_id: message.keyset_id,
                c: message.blinded_secret,
                dleq: None,
            })
            .collect();
        RestoreResponse {
            outputs,
            signatures,
        }
    }

    fn status(&self, signatures: Option<Vec<BlindSignature>>) {
        self.client
            .set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
                quote: self.quote.id.clone(),
                state: MeltQuoteState::Paid,
                expiry: 9999999999,
                fee_reserve: self.quote.fee_reserve,
                amount: self.quote.amount,
                request: None,
                payment_preimage: Some("preimage".to_owned()),
                change: signatures,
                unit: Some(CurrencyUnit::Sat),
                method: PaymentMethod::BOLT11,
            }));
    }

    async fn restore(
        &self,
        amounts: &[u64],
    ) -> Result<Option<crate::types::FinalizedMelt>, crate::Error> {
        self.status(None);
        self.client
            ._set_restore_response(Ok(self.response(amounts)));
        self.wallet.resume_melt_saga(&self.saga).await
    }
}

#[tokio::test]
async fn test_paid_change_bound_rejects_missing_or_excessive_change_before_writes() {
    for amounts in [vec![], vec![16], vec![32]] {
        let f = Fixture::new(1).await;
        let before = f
            .wallet
            .localstore
            .get_reserved_proofs(&f.saga.id)
            .await
            .unwrap();
        assert!(
            f.restore(&amounts).await.is_err(),
            "response must fit the original 1-sat fee reserve: {amounts:?}"
        );
        assert!(f
            .wallet
            .localstore
            .get_saga(&f.saga.id)
            .await
            .unwrap()
            .is_some());
        assert_eq!(
            f.wallet
                .localstore
                .get_reserved_proofs(&f.saga.id)
                .await
                .unwrap(),
            before
        );
        assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::ZERO);
        assert!(f
            .wallet
            .localstore
            .list_transactions(None, None, None)
            .await
            .unwrap()
            .iter()
            .all(|tx| tx.status != TransactionStatus::Completed));

        let result = f.restore(&[16, 8]).await.unwrap().unwrap();
        assert_eq!(result.fee_paid(), Amount::ZERO);
        assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::from(24));
        assert!(f
            .wallet
            .localstore
            .get_saga(&f.saga.id)
            .await
            .unwrap()
            .is_none());
        let txs = f
            .wallet
            .localstore
            .list_transactions(None, None, None)
            .await
            .unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(
            txs[0].metadata.get("purpose").map(String::as_str),
            Some("recovery test")
        );
    }
}

#[tokio::test]
async fn test_paid_change_bound_allows_full_fee_reserve() {
    let f = Fixture::new(1).await;
    let result = f.restore(&[16, 4, 2, 1]).await.unwrap().unwrap();
    assert_eq!(result.fee_paid(), Amount::from(1));
    assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::from(23));
    assert!(f
        .wallet
        .localstore
        .get_saga(&f.saga.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_paid_change_bound_allows_no_change_when_reserve_is_all_consumed() {
    let f = Fixture::new(24).await;
    let result = f.restore(&[]).await.unwrap().unwrap();
    assert_eq!(result.fee_paid(), Amount::from(24));
    assert!(result.change().is_none());
    assert!(f
        .wallet
        .localstore
        .get_saga(&f.saga.id)
        .await
        .unwrap()
        .is_none());
}
