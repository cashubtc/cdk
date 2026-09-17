//! Paid quotes already contain enough information to recover their change.
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
    create_test_db, create_test_wallet_with_mock, test_melt_quote, test_mint_url, test_proof_info,
    MockMintConnector,
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
    async fn new() -> Self {
        let db = create_test_db().await;
        let client = Arc::new(MockMintConnector::new());
        client.enable_mint_signing();
        let wallet = create_test_wallet_with_mock(db.clone(), client.clone()).await;
        let keyset = client.keysets.lock().unwrap()[0].clone();
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
        quote.fee_reserve = Amount::from(1);
        quote.used_by_operation = Some(id.to_string());
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
        let signing_keys = self.client.mint_signing_keys.lock().unwrap();
        let signing_keys = signing_keys.as_ref().unwrap();
        let signatures = outputs
            .iter()
            .zip(amounts)
            .map(|(message, amount)| {
                let amount = Amount::from(*amount);
                let key = &signing_keys[&amount];
                BlindSignature::new(
                    amount,
                    crate::dhke::sign_message(key, &message.blinded_secret).unwrap(),
                    message.keyset_id,
                    &message.blinded_secret,
                    key,
                )
                .unwrap()
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
async fn test_paid_quote_change_recovers_without_restore_and_keeps_metadata() {
    let f = Fixture::new().await;
    let mut signatures = f.response(&[16, 8]).signatures;
    // DLEQ remains optional; even without it, these must be spendable proofs.
    for signature in &mut signatures {
        signature.dleq = None;
    }
    f.status(Some(signatures));
    f.client._set_restore_response(Err(crate::Error::Timeout));
    let result = f.wallet.resume_melt_saga(&f.saga).await.unwrap().unwrap();
    assert_eq!(result.fee_paid(), Amount::ZERO);
    assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::from(24));
    assert!(
        f.client.restore_response.lock().unwrap().is_some(),
        "quote change must not make a restore request"
    );
    let txs = f
        .wallet
        .localstore
        .list_transactions(None, None, None)
        .await
        .unwrap();
    assert_eq!(
        txs[0].metadata.get("purpose").map(String::as_str),
        Some("recovery test")
    );
    let outputs = f.wallet.get_unspent_proofs().await.unwrap();
    for proof in &outputs {
        crate::dhke::verify_message(
            &f.client.mint_signing_keys.lock().unwrap().as_ref().unwrap()[&proof.amount],
            proof.c,
            &proof.secret.to_bytes(),
        )
        .unwrap();
    }
    let infos = f
        .wallet
        .localstore
        .get_proofs_by_ys(outputs.iter().map(|proof| proof.y().unwrap()).collect())
        .await
        .unwrap();
    assert!(infos.iter().all(|proof| proof.derivation_index.is_some()));
}

#[tokio::test]
async fn test_paid_quote_change_rejects_truncated_change_before_writes_and_can_retry() {
    for use_quote_change in [true, false] {
        for amounts in [vec![16], vec![], vec![32]] {
            let f = Fixture::new().await;
            let inputs = f
                .wallet
                .localstore
                .get_reserved_proofs(&f.saga.id)
                .await
                .unwrap();
            let result = if use_quote_change {
                f.status(Some(f.response(&amounts).signatures));
                f.client._set_restore_response(Ok(f.response(&amounts)));
                let result = f.wallet.resume_melt_saga(&f.saga).await;
                assert_eq!(
                    f.client.restore_response.lock().unwrap().is_some(),
                    amounts == vec![32],
                    "only insufficient change should try restore"
                );
                result
            } else {
                f.restore(&amounts).await
            };
            assert!(result.is_err(), "accepted change {amounts:?}");
            assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
            assert_eq!(
                f.wallet.localstore.get_saga(&f.saga.id).await.unwrap(),
                Some(f.saga.clone())
            );
            assert_eq!(
                f.wallet
                    .localstore
                    .get_reserved_proofs(&f.saga.id)
                    .await
                    .unwrap(),
                inputs
            );
            assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::ZERO);
            assert_eq!(
                f.wallet
                    .localstore
                    .get_melt_quote(&f.quote.id)
                    .await
                    .unwrap(),
                Some(f.quote.clone())
            );
            let transactions = f
                .wallet
                .localstore
                .list_transactions(None, None, None)
                .await
                .unwrap();
            assert_eq!(transactions.len(), 1);
            assert_eq!(transactions[0].status, TransactionStatus::Pending);

            // Retrying the same persisted operation must recover all change.
            f.status(Some(f.response(&[16, 8]).signatures));
            let recovered = f.wallet.resume_melt_saga(&f.saga).await.unwrap().unwrap();
            assert_eq!(recovered.fee_paid(), Amount::ZERO);
            assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::from(24));
            assert!(f
                .wallet
                .localstore
                .get_saga(&f.saga.id)
                .await
                .unwrap()
                .is_none());
            let transactions = f
                .wallet
                .localstore
                .list_transactions(None, None, None)
                .await
                .unwrap();
            assert_eq!(transactions.len(), 1);
            assert_eq!(transactions[0].status, TransactionStatus::Completed);
            assert_eq!(transactions[0].fee, Amount::ZERO);
            assert_eq!(
                transactions[0].metadata.get("purpose").map(String::as_str),
                Some("recovery test")
            );
        }
    }
}

#[tokio::test]
async fn test_paid_quote_incomplete_change_uses_restore_through_public_recovery() {
    for amounts in [vec![16], vec![]] {
        for finalize_pending in [true, false] {
            let f = Fixture::new().await;
            f.status(Some(f.response(&amounts).signatures));
            f.client._set_restore_response(Ok(f.response(&[16, 8])));

            if finalize_pending {
                let recovered = f.wallet.finalize_pending_melts().await.unwrap();
                assert_eq!(recovered.len(), 1);
                assert_eq!(recovered[0].state(), MeltQuoteState::Paid);
                assert_eq!(recovered[0].fee_paid(), Amount::ZERO);
            } else {
                let report = f.wallet.recover_incomplete_sagas().await.unwrap();
                assert_eq!(report.recovered, 1);
                assert_eq!(report.failed, 0);
                assert_eq!(report.compensated, 0);
            }
            assert!(f.client.restore_response.lock().unwrap().is_none());
            assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::from(24));
            assert!(f
                .wallet
                .localstore
                .get_saga(&f.saga.id)
                .await
                .unwrap()
                .is_none());
            let quote = f
                .wallet
                .localstore
                .get_melt_quote(&f.quote.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(quote.state, MeltQuoteState::Paid);
            assert!(quote.used_by_operation.is_none());
            let transactions = f
                .wallet
                .localstore
                .list_transactions(None, None, None)
                .await
                .unwrap();
            assert_eq!(transactions.len(), 1);
            assert_eq!(transactions[0].status, TransactionStatus::Completed);
            assert_eq!(transactions[0].fee, Amount::ZERO);
            assert_eq!(
                transactions[0].metadata.get("purpose").map(String::as_str),
                Some("recovery test")
            );
        }
    }
}

#[tokio::test]
async fn test_paid_quote_incomplete_change_keeps_saga_when_restore_fails_and_can_retry() {
    for error in [
        crate::Error::Timeout,
        crate::Error::HttpError(Some(404), "Not Found".to_owned()),
    ] {
        let f = Fixture::new().await;
        let inputs = f
            .wallet
            .localstore
            .get_reserved_proofs(&f.saga.id)
            .await
            .unwrap();
        f.status(Some(f.response(&[16]).signatures));
        f.client._set_restore_response(Err(error));
        let report = f.wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.recovered, 0);
        assert_eq!(report.compensated, 0);
        assert!(f.client.restore_response.lock().unwrap().is_none());
        assert_eq!(
            f.wallet.localstore.get_saga(&f.saga.id).await.unwrap(),
            Some(f.saga.clone())
        );
        assert_eq!(
            f.wallet
                .localstore
                .get_reserved_proofs(&f.saga.id)
                .await
                .unwrap(),
            inputs
        );
        assert_eq!(
            f.wallet
                .localstore
                .get_melt_quote(&f.quote.id)
                .await
                .unwrap(),
            Some(f.quote.clone())
        );
        assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::ZERO);
        let transactions = f
            .wallet
            .localstore
            .list_transactions(None, None, None)
            .await
            .unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].status, TransactionStatus::Pending);

        // The quote can remain truncated: a later restore must still finish it.
        f.status(Some(f.response(&[16]).signatures));
        f.client._set_restore_response(Ok(f.response(&[16, 8])));
        let recovered = f.wallet.finalize_pending_melts().await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].fee_paid(), Amount::ZERO);
        assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::from(24));
        assert!(f
            .wallet
            .localstore
            .get_saga(&f.saga.id)
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn test_paid_quote_incomplete_change_rejects_duplicate_restore() {
    let f = Fixture::new().await;
    f.status(Some(f.response(&[16]).signatures));
    let response = f.response(&[8]);
    // Three copies would total 24 sats, but storage can only retain one proof.
    f.client._set_restore_response(Ok(RestoreResponse {
        outputs: vec![response.outputs[0].clone(); 3],
        signatures: vec![response.signatures[0].clone(); 3],
    }));
    assert!(matches!(
        f.wallet.resume_melt_saga(&f.saga).await,
        Err(crate::Error::InvalidMintResponse(_))
    ));
    assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::ZERO);
    assert_eq!(
        f.wallet.localstore.get_saga(&f.saga.id).await.unwrap(),
        Some(f.saga.clone())
    );
    assert!(f
        .wallet
        .localstore
        .get_reserved_proofs(&f.saga.id)
        .await
        .unwrap()
        .iter()
        .all(|proof| proof.state == State::Pending));
    assert_eq!(
        f.wallet
            .localstore
            .get_melt_quote(&f.quote.id)
            .await
            .unwrap(),
        Some(f.quote.clone())
    );
    let transactions = f
        .wallet
        .localstore
        .list_transactions(None, None, None)
        .await
        .unwrap();
    assert_eq!(transactions.len(), 1);
    assert_eq!(transactions[0].status, TransactionStatus::Pending);
}

#[tokio::test]
async fn test_paid_quote_change_allows_full_fee_reserve() {
    for use_quote_change in [true, false] {
        let f = Fixture::new().await;
        let amounts = [16, 4, 2, 1];
        let recovered = if use_quote_change {
            f.status(Some(f.response(&amounts).signatures));
            f.wallet.resume_melt_saga(&f.saga).await
        } else {
            f.restore(&amounts).await
        }
        .unwrap()
        .unwrap();
        assert_eq!(recovered.fee_paid(), Amount::from(1));
        assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::from(23));
        assert!(f
            .wallet
            .localstore
            .get_saga(&f.saga.id)
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn test_paid_quote_change_rejects_malformed_signatures() {
    for case in [
        "too_many",
        "wrong_keyset",
        "unsupported_amount",
        "invalid_dleq",
    ] {
        let f = Fixture::new().await;
        let mut signatures = f.response(&[16, 8]).signatures;
        match case {
            "too_many" => signatures.resize(f.secrets.len() + 1, signatures[0].clone()),
            "wrong_keyset" => signatures[0].keyset_id = "00ffffffffffffff".parse().unwrap(),
            "unsupported_amount" => signatures[0].amount = Amount::from(3),
            "invalid_dleq" => {
                signatures[0].dleq = Some(crate::nuts::nut12::BlindSignatureDleq {
                    e: crate::nuts::SecretKey::from_slice(&[1; 32]).unwrap(),
                    s: crate::nuts::SecretKey::from_slice(&[2; 32]).unwrap(),
                });
            }

            _ => unreachable!(),
        }
        f.status(Some(signatures));
        // An invalid quote response must not silently fall back to a valid restore.
        f.client._set_restore_response(Ok(f.response(&[16, 8])));
        assert!(
            f.wallet.resume_melt_saga(&f.saga).await.is_err(),
            "accepted {case}"
        );
        assert!(f.client.restore_response.lock().unwrap().is_some());
        assert!(f
            .wallet
            .localstore
            .get_saga(&f.saga.id)
            .await
            .unwrap()
            .is_some());
        assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::ZERO);
        assert!(f
            .wallet
            .localstore
            .get_reserved_proofs(&f.saga.id)
            .await
            .unwrap()
            .iter()
            .all(|proof| proof.state == State::Pending));
    }
}

#[tokio::test]
async fn test_paid_quote_change_omitted_uses_restore() {
    let f = Fixture::new().await;
    let result = f.restore(&[16, 8]).await.unwrap().unwrap();
    assert_eq!(result.fee_paid(), Amount::ZERO);
    assert_eq!(f.wallet.total_balance().await.unwrap(), Amount::from(24));
    assert!(f.client.restore_response.lock().unwrap().is_none());
}
