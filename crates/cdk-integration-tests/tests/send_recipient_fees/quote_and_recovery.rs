use anyhow::Result;
use cdk::amount::SplitTarget;
use cdk::nuts::{CurrencyUnit, Token};
use cdk::wallet::{ReceiveOptions, SendKind, SendOptions};
use cdk::Amount;
use cdk_common::wallet::KeysetLoadPolicy;
use cdk_integration_tests::init_pure_tests::{
    create_mint_with_fee, create_test_wallet_for_mint, fund_wallet,
};

#[tokio::test]
async fn rotation_after_prepare_preserves_quote_and_reservations() -> Result<()> {
    for (new_fee, refresh) in [(0, true), (0, false), (2000, true), (2000, false)] {
        let mint = create_mint_with_fee(1000).await?;
        let sender = create_test_wallet_for_mint(mint.clone()).await?;
        fund_wallet(
            sender.clone(),
            100,
            Some(SplitTarget::Values([64, 32, 4].map(Amount::from).to_vec())),
        )
        .await?;
        let prepared = sender
            .prepare_send(
                21.into(),
                SendOptions {
                    include_fee: true,
                    ..Default::default()
                },
            )
            .await?;
        mint.rotate_keyset(
            CurrencyUnit::Sat,
            cdk_integration_tests::standard_keyset_amounts(32),
            new_fee,
            true,
            None,
        )
        .await?;
        if refresh {
            sender.keysets(KeysetLoadPolicy::Refresh).await?;
        }
        assert!(matches!(
            prepared.confirm(None).await,
            Err(cdk::Error::InactiveKeyset)
        ));
        assert_eq!(sender.total_balance().await?, 100.into());
        assert!(sender.get_reserved_proofs().await?.is_empty());
        sender.recover_incomplete_sagas().await?;
        sender.recover_incomplete_sagas().await?;
        assert!(sender.get_pending_sends().await?.is_empty());
        sender.keysets(KeysetLoadPolicy::Refresh).await?;
        let prepared = sender
            .prepare_send(
                21.into(),
                SendOptions {
                    include_fee: true,
                    ..Default::default()
                },
            )
            .await?;
        let quoted_fee = prepared.send_fee();
        let quoted_swap_fee = prepared.swap_fee();
        let recipient = create_test_wallet_for_mint(mint).await?;
        let token = prepared.confirm(None).await?;
        let credit = recipient
            .receive(&token.to_string(), ReceiveOptions::default())
            .await?;
        let actual_fee = token.value()? - credit;
        assert_eq!(
            credit,
            21.into(),
            "OnlineExact preserves the exact net amount"
        );
        assert_eq!(
            Amount::from(100),
            sender.total_balance().await? + credit + quoted_fee + quoted_swap_fee
        );
        assert_eq!(
            quoted_fee, actual_fee,
            "quoted redemption fee must match final token"
        );
    }
    Ok(())
}

#[tokio::test]
async fn exact_net_shortcut_respects_max_proofs() -> Result<()> {
    for kind in [SendKind::OnlineExact, SendKind::OfflineExact] {
        let mint = create_mint_with_fee(1000).await?;
        let sender = create_test_wallet_for_mint(mint).await?;
        fund_wallet(
            sender.clone(),
            24,
            Some(SplitTarget::Values([16, 4, 4].map(Amount::from).to_vec())),
        )
        .await?;
        let result = sender
            .prepare_send(
                21.into(),
                SendOptions {
                    include_fee: true,
                    max_proofs: Some(2),
                    send_kind: kind,
                    ..Default::default()
                },
            )
            .await;
        let token = match result {
            Ok(prepared) => prepared.confirm(None).await?,
            Err(error) => {
                assert!(matches!(error, cdk::Error::InsufficientFunds));
                assert_eq!(sender.total_balance().await?, 24.into());
                assert!(sender.get_reserved_proofs().await?.is_empty());
                let prepared = sender
                    .prepare_send(
                        21.into(),
                        SendOptions {
                            include_fee: true,
                            max_proofs: Some(3),
                            send_kind: kind,
                            ..Default::default()
                        },
                    )
                    .await?;
                let token = prepared.confirm(None).await?;
                assert_eq!(token.value()?, 24.into());
                assert_eq!(sender.total_balance().await?, Amount::ZERO);
                continue;
            }
        };
        let count = match &token {
            Token::TokenV3(token) => token
                .token
                .iter()
                .map(|group| group.proofs.len())
                .sum::<usize>(),
            Token::TokenV4(token) => token
                .token
                .iter()
                .map(|group| group.proofs.len())
                .sum::<usize>(),
        };
        eprintln!("max_proofs: emitted={count}, maximum=2");
        assert!(count <= 2, "max_proofs violated");
    }
    Ok(())
}

#[tokio::test]
async fn tolerance_allows_surplus_without_misreporting_fees() -> Result<()> {
    for kind in [
        SendKind::OnlineTolerance(1.into()),
        SendKind::OfflineTolerance(1.into()),
    ] {
        let mint = create_mint_with_fee(1000).await?;
        mint.rotate_keyset(CurrencyUnit::Sat, vec![1, 4, 16], 1000, true, None)
            .await?;
        let sender = create_test_wallet_for_mint(mint.clone()).await?;
        let recipient = create_test_wallet_for_mint(mint).await?;
        fund_wallet(
            sender.clone(),
            20,
            Some(SplitTarget::Values([16, 4].map(Amount::from).to_vec())),
        )
        .await?;
        for send_kind in [
            SendKind::OnlineExact,
            SendKind::OfflineExact,
            SendKind::OnlineTolerance(Amount::ZERO),
        ] {
            assert!(matches!(
                sender
                    .prepare_send(
                        2.into(),
                        SendOptions {
                            include_fee: true,
                            send_kind,
                            ..Default::default()
                        }
                    )
                    .await,
                Err(cdk::Error::InsufficientFunds)
            ));
            assert_eq!(sender.total_balance().await?, 20.into());
            assert!(sender.get_reserved_proofs().await?.is_empty());
        }
        let prepared = sender
            .prepare_send(
                2.into(),
                SendOptions {
                    include_fee: true,
                    send_kind: kind,
                    ..Default::default()
                },
            )
            .await?;
        let quoted_amount = prepared.amount();
        let fee = prepared.send_fee();
        let swap_fee = prepared.swap_fee();
        let token = prepared.confirm(None).await?;
        let credit = recipient
            .receive(&token.to_string(), ReceiveOptions::default())
            .await?;
        assert!(credit >= 2.into() && credit <= 4.into());
        assert_eq!(credit, quoted_amount);
        assert_eq!(fee, token.value()? - credit);
        assert_eq!(
            Amount::from(20),
            sender.total_balance().await? + credit + fee + swap_fee
        );
    }
    Ok(())
}

#[tokio::test]
async fn rollback_recovery_checks_emitted_token() -> Result<()> {
    use bip39::Mnemonic;
    use cdk::cdk_database::WalletDatabase;
    use cdk::wallet::WalletBuilder;
    use cdk_common::wallet::{SendSagaState, WalletSagaState};
    use cdk_integration_tests::init_pure_tests::DirectMintConnection;
    use std::sync::Arc;

    let mint = create_mint_with_fee(1000).await?;
    for revoke in [false, true] {
        let path =
            std::env::temp_dir().join(format!("send-rollback-{}.sqlite", uuid::Uuid::new_v4()));
        let seed = Mnemonic::generate(12)?.to_seed_normalized("");
        let open = || async {
            let db = Arc::new(cdk_sqlite::WalletSqliteDatabase::new(path.to_str().unwrap()).await?);
            let sender = WalletBuilder::new()
                .mint_url("https://aaa".parse()?)
                .unit(CurrencyUnit::Sat)
                .localstore(db.clone())
                .seed(seed)
                .client(DirectMintConnection::new(mint.clone()))
                .build()?;
            Ok::<_, anyhow::Error>((db, sender))
        };
        let (db, sender) = open().await?;
        fund_wallet(
            sender.clone(),
            100,
            Some(SplitTarget::Values([64, 32, 4].map(Amount::from).to_vec())),
        )
        .await?;
        let prepared = sender
            .prepare_send(
                21.into(),
                SendOptions {
                    include_fee: true,
                    ..Default::default()
                },
            )
            .await?;
        assert!(!prepared.proofs_to_swap().is_empty());
        let operation_id = prepared.operation_id();
        let original_fee = prepared.fee();
        let redemption_fee = prepared.send_fee();
        let token = prepared.confirm(None).await?;
        // State at a crash immediately after revoke locks the saga, before its swap.
        let mut saga = db.get_saga(&operation_id).await?.expect("pending send");
        saga.update_state(WalletSagaState::Send(SendSagaState::RollingBack));
        assert!(db.update_saga(saga).await?);
        drop(sender);
        drop(db);
        let (db, sender) = open().await?;
        sender.recover_incomplete_sagas().await?;
        sender.recover_incomplete_sagas().await?;
        let retained = db.get_saga(&operation_id).await?;
        assert!(matches!(
            retained.as_ref().map(|saga| saga.state),
            Some(WalletSagaState::Send(SendSagaState::TokenCreated))
        ));
        assert!(!sender.check_send_status(operation_id).await?);
        if revoke {
            assert_eq!(
                sender.revoke_send(operation_id).await?,
                token.value()? - redemption_fee
            );
            assert_eq!(
                sender.total_balance().await?,
                Amount::from(100) - original_fee
            );
            let recipient = create_test_wallet_for_mint(mint.clone()).await?;
            assert!(recipient
                .receive(&token.to_string(), ReceiveOptions::default())
                .await
                .is_err());
        } else {
            let recipient = create_test_wallet_for_mint(mint.clone()).await?;
            let credit = recipient
                .receive(&token.to_string(), ReceiveOptions::default())
                .await?;
            assert!(credit >= 21.into());
            assert!(sender.check_send_status(operation_id).await?);
        }
        assert!(
            retained.is_some(),
            "recovery must retain a still spendable token"
        );
        sender.recover_incomplete_sagas().await?;
        assert!(db.get_saga(&operation_id).await?.is_none());
        drop(sender);
        drop(db);
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[tokio::test]
async fn inactive_keyset_does_not_retry_an_approved_fee_plan() -> Result<()> {
    let mint = create_mint_with_fee(1000).await?;
    let sender = create_test_wallet_for_mint(mint.clone()).await?;
    fund_wallet(
        sender.clone(),
        100,
        Some(SplitTarget::Values([64, 32, 4].map(Amount::from).to_vec())),
    )
    .await?;
    let prepared = sender
        .prepare_send(
            21.into(),
            SendOptions {
                include_fee: true,
                ..Default::default()
            },
        )
        .await?;
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        2000,
        true,
        None,
    )
    .await?;
    // Retain cached metadata to exercise the supported inactive-keyset retry.
    assert!(matches!(
        prepared.confirm(None).await,
        Err(cdk::Error::InactiveKeyset)
    ));
    sender.recover_incomplete_sagas().await?;
    sender.recover_incomplete_sagas().await?;
    let balance = sender.total_balance().await?;
    assert!(sender.get_reserved_proofs().await?.is_empty());
    assert_eq!(
        balance,
        Amount::from(100),
        "an invalidated quote must fail before spending inputs"
    );
    Ok(())
}

#[tokio::test]
async fn request_accepts_net_surplus_and_quotes_actual_debit() -> Result<()> {
    use std::sync::Arc;

    use bip39::Mnemonic;
    use cdk::nuts::{PaymentRequest, SupportedMethod, Transport, TransportType};
    use cdk::wallet::{PayRequestOptions, WalletBuilder};
    use cdk_integration_tests::init_pure_tests::DirectMintConnection;

    for (requested, balance, denominations, initial) in [
        (2, 20, vec![1, 4, 16], vec![16, 4]),
        (
            21,
            24,
            cdk_integration_tests::standard_keyset_amounts(32),
            vec![16, 8],
        ),
    ] {
        for fixed in [false, true] {
            let mint = create_mint_with_fee(1000).await?;
            mint.rotate_keyset(CurrencyUnit::Sat, denominations.clone(), 1000, true, None)
                .await?;
            let payloads = Arc::new(tokio::sync::Mutex::new(Vec::new()));
            let sender = WalletBuilder::new()
                .mint_url("https://aaa".parse()?)
                .unit(CurrencyUnit::Sat)
                .localstore(Arc::new(cdk_sqlite::wallet::memory::empty().await?))
                .seed(Mnemonic::generate(12)?.to_seed_normalized(""))
                .client(
                    DirectMintConnection::new(mint.clone())
                        .with_payment_request_payloads(payloads.clone()),
                )
                .build()?;
            fund_wallet(
                sender.clone(),
                balance,
                Some(SplitTarget::Values(
                    initial.iter().copied().map(Amount::from).collect(),
                )),
            )
            .await?;
            assert!(matches!(
                sender
                    .prepare_send(
                        requested.into(),
                        SendOptions {
                            include_fee: true,
                            ..Default::default()
                        }
                    )
                    .await,
                Err(cdk::Error::InsufficientFunds)
            ));
            assert!(sender.get_reserved_proofs().await?.is_empty());
            let request = PaymentRequest {
                payment_id: None,
                amount: fixed.then_some(requested.into()),
                unit: Some(CurrencyUnit::Sat),
                single_use: None,
                mints: vec![],
                mint_preferred: None,
                supported_methods: vec![SupportedMethod::new("bolt11")],
                description: None,
                transports: vec![Transport {
                    _type: TransportType::HttpPost,
                    target: "https://receiver.example.com".to_owned(),
                    tags: vec![],
                }],
                nut10: None,
            };
            let prepared = sender
                .prepare_pay_request(request, Some(requested.into()))
                .await?;
            let minimum = prepared.payment_amount();
            let fee = prepared.send_fee();
            let swap_fee = prepared.swap_fee();
            let debit = prepared.total_amount();
            assert_eq!(swap_fee, Amount::ZERO);
            assert!(
                prepared
                    .check_limits(PayRequestOptions {
                        max_total_amount: Some(minimum + fee + swap_fee),
                        ..Default::default()
                    })
                    .is_err(),
                "the total limit includes surplus"
            );
            prepared.confirm().await?;
            let payload = payloads.lock().await[0].clone();
            let recipient = create_test_wallet_for_mint(mint).await?;
            let token = Token::new(payload.mint, payload.proofs, None, payload.unit);
            let credit = recipient
                .receive(&token.to_string(), ReceiveOptions::default())
                .await?;
            assert!(
                credit > minimum,
                "exercise the protocol's permitted surplus"
            );
            assert_eq!(fee, token.value()? - credit);
            assert_eq!(debit, credit + fee + swap_fee);
            assert_eq!(sender.total_balance().await?, Amount::from(balance) - debit);
        }
    }
    Ok(())
}

#[tokio::test]
async fn failed_final_validation_releases_nested_swap_outputs() -> Result<()> {
    let mint = create_mint_with_fee(1000).await?;
    let sender = create_test_wallet_for_mint(mint).await?;
    fund_wallet(
        sender.clone(),
        100,
        Some(SplitTarget::Values([64, 32, 4].map(Amount::from).to_vec())),
    )
    .await?;
    let prepared = sender
        .prepare_send(
            21.into(),
            SendOptions {
                include_fee: true,
                ..Default::default()
            },
        )
        .await?;
    assert!(!prepared.proofs_to_swap().is_empty());
    let operation_id = prepared.operation_id();
    let mut options = prepared.options().clone();
    // Simulate reconstructed caller state that fails the final token check.
    options.max_proofs = Some(0);
    let result = sender
        .confirm_send(
            operation_id,
            prepared.amount(),
            options,
            prepared.proofs_to_swap().clone(),
            prepared.proofs_to_send().clone(),
            prepared.swap_fee(),
            prepared.send_fee(),
            None,
        )
        .await;
    assert!(matches!(result, Err(cdk::Error::IncorrectQuoteAmount)));
    assert_eq!(
        sender.total_balance().await?,
        Amount::from(100) - prepared.swap_fee()
    );
    assert!(sender.get_reserved_proofs().await?.is_empty());
    assert!(sender.get_pending_spent_proofs().await?.is_empty());
    sender.recover_incomplete_sagas().await?;
    sender.recover_incomplete_sagas().await?;
    assert!(sender.get_pending_sends().await?.is_empty());
    assert!(sender.check_send_status(operation_id).await.is_err());
    Ok(())
}

#[tokio::test]
async fn lost_nested_swap_response_recovers_without_releasing_spent_inputs() -> Result<()> {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use bip39::Mnemonic;
    use cdk::wallet::WalletBuilder;
    use cdk_integration_tests::init_pure_tests::DirectMintConnection;

    for refund in [false, true] {
        let mint = create_mint_with_fee(1000).await?;
        let path = std::env::temp_dir().join(format!(
            "send-lost-response-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let seed = Mnemonic::generate(12)?.to_seed_normalized("");
        let lose_response = Arc::new(AtomicBool::new(!refund));
        let open = || async {
            WalletBuilder::new()
                .mint_url("https://aaa".parse()?)
                .unit(CurrencyUnit::Sat)
                .localstore(Arc::new(
                    cdk_sqlite::WalletSqliteDatabase::new(path.to_str().unwrap()).await?,
                ))
                .seed(seed)
                .client(
                    DirectMintConnection::new(mint.clone())
                        .with_lost_swap_response(lose_response.clone()),
                )
                .build()
                .map_err(anyhow::Error::from)
        };
        let sender = open().await?;
        fund_wallet(
            sender.clone(),
            100,
            Some(SplitTarget::Values([64, 32, 4].map(Amount::from).to_vec())),
        )
        .await?;
        let prepared = sender
            .prepare_send(
                21.into(),
                SendOptions {
                    include_fee: true,
                    ..Default::default()
                },
            )
            .await?;
        let swap_fee = prepared.swap_fee();
        let total_fee = prepared.fee();
        let operation_id = prepared.operation_id();
        let original_swap_proofs = prepared.proofs_to_swap().clone();
        let original_send_proofs = prepared.proofs_to_send().clone();
        assert!(!prepared.proofs_to_swap().is_empty());
        let token = if refund {
            let token = prepared.confirm(None).await?;
            lose_response.store(true, Ordering::SeqCst);
            assert!(matches!(
                sender.revoke_send(operation_id).await,
                Err(cdk::Error::Timeout)
            ));
            Some(token)
        } else {
            assert!(matches!(
                prepared.confirm(None).await,
                Err(cdk::Error::Timeout)
            ));
            assert!(matches!(
                sender
                    .cancel_send(operation_id, original_swap_proofs, original_send_proofs)
                    .await,
                Err(cdk::Error::ConcurrentUpdate)
            ));
            None
        };
        drop(sender);
        let sender = open().await?;
        sender.recover_incomplete_sagas().await?;
        sender.recover_incomplete_sagas().await?;
        assert_eq!(
            sender.total_balance().await?,
            Amount::from(100) - if refund { total_fee } else { swap_fee }
        );
        assert!(sender.get_reserved_proofs().await?.is_empty());
        assert!(sender.get_pending_sends().await?.is_empty());
        if let Some(token) = token {
            let recipient = create_test_wallet_for_mint(mint.clone()).await?;
            assert!(recipient
                .receive(&token.to_string(), ReceiveOptions::default())
                .await
                .is_err());
        }
        drop(sender);
        std::fs::remove_file(path)?;
    }
    Ok(())
}
