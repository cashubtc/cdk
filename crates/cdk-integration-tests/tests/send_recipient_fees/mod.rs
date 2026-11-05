use anyhow::Result;
use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{ReceiveOptions, SendKind, SendOptions};
use cdk::Amount;
use cdk_integration_tests::init_pure_tests::{
    create_mint_with_fee, create_test_wallet_for_mint, fund_wallet, DirectMintConnection,
};

mod quote_and_recovery;

#[tokio::test]
async fn payment_requests_cover_recipient_fees_and_method_fees() -> Result<()> {
    use std::sync::Arc;

    use bip39::Mnemonic;
    use cdk::nuts::{PaymentRequest, SupportedMethod, Token, Transport, TransportType};
    use cdk::wallet::WalletBuilder;

    for fee_ppk in [0, 100, 1000] {
        for fixed in [false, true] {
            for method_fee in [0, 3] {
                let mint = create_mint_with_fee(fee_ppk).await?;
                let recipient = create_test_wallet_for_mint(mint.clone()).await?;
                let payloads = Arc::new(tokio::sync::Mutex::new(Vec::new()));
                let sender = WalletBuilder::new()
                    .mint_url("https://aaa".parse()?)
                    .unit(CurrencyUnit::Sat)
                    .localstore(Arc::new(cdk_sqlite::wallet::memory::empty().await?))
                    .seed(Mnemonic::generate(12)?.to_seed_normalized(""))
                    .client(
                        DirectMintConnection::new(mint)
                            .with_payment_request_payloads(payloads.clone()),
                    )
                    .build()?;
                fund_wallet(sender.clone(), 100, None).await?;
                let request = PaymentRequest {
                    payment_id: Some("recipient-fee-regression".to_owned()),
                    amount: fixed.then_some(21.into()),
                    unit: Some(CurrencyUnit::Sat),
                    single_use: Some(true),
                    mints: vec![],
                    mint_preferred: None,
                    supported_methods: vec![SupportedMethod::with_fee("bolt11", method_fee)],
                    description: None,
                    transports: vec![Transport {
                        _type: TransportType::HttpPost,
                        target: "https://receiver.example.com".to_owned(),
                        tags: vec![],
                    }],
                    nut10: None,
                };
                let prepared = sender.prepare_pay_request(request, Some(21.into())).await?;
                let total = prepared.total_amount();
                let payment_amount = prepared.payment_amount();
                let send_fee = prepared.send_fee();
                let swap_fee = prepared.swap_fee();
                assert_eq!(prepared.method_fee(), method_fee.into());
                assert!(total >= payment_amount + send_fee + swap_fee);
                prepared.confirm().await?;
                let deliveries = payloads.lock().await;
                assert_eq!(deliveries.len(), 1);
                let payload = &deliveries[0];
                assert_eq!(payload.id.as_deref(), Some("recipient-fee-regression"));
                let gross = payload.proofs.total_amount()?;
                let actual_fee = sender.get_proofs_fee(&payload.proofs).await?.total;
                assert!(gross - actual_fee >= payment_amount);
                assert_eq!(send_fee, actual_fee);
                let token = Token::new(
                    payload.mint.clone(),
                    payload.proofs.clone(),
                    None,
                    payload.unit.clone(),
                );
                let credit = recipient
                    .receive(&token.to_string(), ReceiveOptions::default())
                    .await?;
                assert!(credit >= Amount::from(21 + method_fee));
                assert_eq!(credit, gross - actual_fee);
                assert_eq!(total, credit + send_fee + swap_fee);
                assert_eq!(sender.total_balance().await?, Amount::from(100) - total);
                assert_eq!(
                    Amount::from(100),
                    sender.total_balance().await? + credit + send_fee + swap_fee
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn recipient_fee_send_covers_redemption() -> Result<()> {
    for fee in [0, 100, 1000] {
        futures::future::try_join_all(
            [1, 7, 8, 15, 16, 21, 31, 32, 63, 64].map(|amount| check_send(fee, amount)),
        )
        .await?;
    }
    Ok(())
}

async fn check_send(fee_ppk: u64, amount: u64) -> Result<()> {
    let mint = create_mint_with_fee(fee_ppk).await?;
    let sender = create_test_wallet_for_mint(mint.clone()).await?;
    let recipient = create_test_wallet_for_mint(mint).await?;
    // Larger initial proofs ensure the boundary matrix also exercises swaps,
    // rather than satisfying every request from an existing exact-net token.
    fund_wallet(
        sender.clone(),
        100,
        Some(SplitTarget::Values([64, 32, 4].map(Amount::from).to_vec())),
    )
    .await?;

    let requested = Amount::from(amount);
    let prepared = sender
        .prepare_send(
            requested,
            SendOptions {
                include_fee: true,
                ..Default::default()
            },
        )
        .await?;
    let quoted_fee = prepared.fee();
    let swap_fee = prepared.swap_fee();
    let send_fee = prepared.send_fee();
    if fee_ppk == 1000 && amount == 21 {
        assert!(!prepared.proofs_to_swap().is_empty());
    }
    let token = prepared.confirm(None).await?;
    let gross = token.value()?;
    let credit = recipient
        .receive(&token.to_string(), ReceiveOptions::default())
        .await?;
    let change = sender.total_balance().await?;
    eprintln!("requested={requested}, gross={gross}, credit={credit}, quoted_fee={quoted_fee}, swap_fee={swap_fee}, change={change}");
    assert!(
        credit >= requested,
        "recipient was underpaid: {credit} < {requested}"
    );
    assert_eq!(Amount::from(100), change + gross + swap_fee);
    assert_eq!(Amount::from(100), change + requested + quoted_fee);
    assert_eq!(gross - credit, send_fee);
    assert_eq!(recipient.total_balance().await?, credit);
    Ok(())
}

#[tokio::test]
async fn recipient_fee_direct_send_needs_no_swap() -> Result<()> {
    for send_kind in [SendKind::OnlineExact, SendKind::OfflineExact] {
        let mint = create_mint_with_fee(1000).await?;
        let sender = create_test_wallet_for_mint(mint.clone()).await?;
        let recipient = create_test_wallet_for_mint(mint).await?;
        fund_wallet(
            sender.clone(),
            24,
            Some(SplitTarget::Values([16, 4, 4].map(Amount::from).to_vec())),
        )
        .await?;
        let prepared = sender
            .prepare_send(
                21.into(),
                SendOptions {
                    include_fee: true,
                    send_kind,
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(prepared.swap_fee(), Amount::ZERO);
        assert_eq!(prepared.send_fee(), Amount::from(3));
        let token = prepared.confirm(None).await?;
        assert_eq!(
            recipient
                .receive(&token.to_string(), ReceiveOptions::default())
                .await?,
            21.into()
        );
        assert_eq!(sender.total_balance().await?, Amount::ZERO);
    }
    Ok(())
}

#[tokio::test]
async fn recipient_fee_send_uses_actual_keyset_fees() -> Result<()> {
    for (old_fee, new_fee) in [(1000, 2000), (2000, 1000)] {
        let mint = create_mint_with_fee(old_fee).await?;
        let sender = create_test_wallet_for_mint(mint.clone()).await?;
        fund_wallet(sender.clone(), 100, None).await?;
        mint.rotate_keyset(
            CurrencyUnit::Sat,
            cdk_integration_tests::standard_keyset_amounts(32),
            new_fee,
            true,
            None,
        )
        .await?;
        let recipient = create_test_wallet_for_mint(mint).await?;
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
        let send_fee = prepared.send_fee();
        let token = prepared.confirm(None).await?;
        let credit = recipient
            .receive(&token.to_string(), ReceiveOptions::default())
            .await?;
        assert!(credit >= 21.into());
        assert_eq!(token.value()? - credit, send_fee);
        assert_eq!(
            Amount::from(100),
            sender.total_balance().await? + token.value()? + swap_fee
        );
    }
    Ok(())
}

#[tokio::test]
async fn recipient_fee_reservations_and_recovery_preserve_value() -> Result<()> {
    use std::sync::Arc;

    use bip39::Mnemonic;
    use cdk::wallet::WalletBuilder;

    let mint = create_mint_with_fee(1000).await?;
    let path = std::env::temp_dir().join(format!("recipient-fees-{}.sqlite", uuid::Uuid::new_v4()));
    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let open = || async {
        WalletBuilder::new()
            .mint_url("https://aaa".parse()?)
            .unit(CurrencyUnit::Sat)
            .localstore(Arc::new(
                cdk_sqlite::WalletSqliteDatabase::new(path.to_str().unwrap()).await?,
            ))
            .seed(seed)
            .client(DirectMintConnection::new(mint.clone()))
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
    let options = SendOptions {
        include_fee: true,
        ..Default::default()
    };
    let prepared = sender.prepare_send(21.into(), options.clone()).await?;
    assert!(!sender.get_reserved_proofs().await?.is_empty());
    assert!(sender
        .prepare_send(100.into(), SendOptions::default())
        .await
        .is_err());
    prepared.cancel().await?;
    assert!(sender.get_reserved_proofs().await?.is_empty());
    assert_eq!(sender.total_balance().await?, 100.into());

    let prepared = sender.prepare_send(21.into(), options.clone()).await?;
    drop(prepared);
    drop(sender);
    let sender = open().await?;
    sender.recover_incomplete_sagas().await?;
    sender.recover_incomplete_sagas().await?;
    assert!(sender.get_reserved_proofs().await?.is_empty());
    assert_eq!(sender.total_balance().await?, 100.into());

    let prepared = sender.prepare_send(21.into(), options).await?;
    let operation_id = prepared.operation_id();
    let fee = prepared.fee();
    assert!(!prepared.proofs_to_swap().is_empty());
    let token = prepared.confirm(None).await?;
    drop(sender);
    let sender = open().await?;
    sender.recover_incomplete_sagas().await?;
    assert!(!sender.check_send_status(operation_id).await?);
    let recipient = create_test_wallet_for_mint(mint.clone()).await?;
    let credit = recipient
        .receive(&token.to_string(), ReceiveOptions::default())
        .await?;
    assert!(credit >= 21.into());
    assert!(sender.check_send_status(operation_id).await?);
    sender.recover_incomplete_sagas().await?;
    sender.recover_incomplete_sagas().await?;
    assert_eq!(
        sender.total_balance().await?,
        Amount::from(100) - Amount::from(21) - fee
    );
    assert!(recipient
        .receive(&token.to_string(), ReceiveOptions::default())
        .await
        .is_err());
    drop(sender);
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn ordinary_fee_bearing_send_max_preserves_value() -> Result<()> {
    for fee_ppk in [0, 1000] {
        let mint = create_mint_with_fee(fee_ppk).await?;
        let sender = create_test_wallet_for_mint(mint.clone()).await?;
        let recipient = create_test_wallet_for_mint(mint).await?;
        fund_wallet(sender.clone(), 100, None).await?;
        let original = sender.get_unspent_proofs().await?;
        let redemption_fee = sender.get_proofs_fee(&original).await?.total;
        let prepared = sender
            .prepare_send(sender.total_balance().await?, SendOptions::default())
            .await?;
        assert_eq!(prepared.fee(), Amount::ZERO);
        assert!(prepared.proofs_to_swap().is_empty());
        let token = prepared.confirm(None).await?;
        assert_eq!(sender.total_balance().await?, Amount::ZERO);
        let credit = recipient
            .receive(&token.to_string(), ReceiveOptions::default())
            .await?;
        assert_eq!(Amount::from(100), credit + redemption_fee);
    }
    Ok(())
}

#[tokio::test]
async fn locked_recipient_fee_send_preserves_net_value() -> Result<()> {
    use cdk::nuts::{SecretKey, SpendingConditions};

    for use_p2bk in [false, true] {
        let mint = create_mint_with_fee(1000).await?;
        let sender = create_test_wallet_for_mint(mint.clone()).await?;
        let recipient = create_test_wallet_for_mint(mint).await?;
        fund_wallet(
            sender.clone(),
            64,
            Some(SplitTarget::Values(vec![64.into()])),
        )
        .await?;
        let key = SecretKey::generate();
        let prepared = sender
            .prepare_send(
                21.into(),
                SendOptions {
                    include_fee: true,
                    conditions: Some(SpendingConditions::new_p2pk(key.public_key(), None)),
                    use_p2bk,
                    ..Default::default()
                },
            )
            .await?;
        let swap_fee = prepared.swap_fee();
        let send_fee = prepared.send_fee();
        assert!(!prepared.proofs_to_swap().is_empty());
        let token = prepared.confirm(None).await?;
        let credit = recipient
            .receive(
                &token.to_string(),
                ReceiveOptions {
                    p2pk_signing_keys: vec![key],
                    ..Default::default()
                },
            )
            .await?;
        assert!(credit >= 21.into());
        assert_eq!(token.value()? - credit, send_fee);
        assert_eq!(
            Amount::from(64),
            sender.total_balance().await? + credit + send_fee + swap_fee
        );
    }
    Ok(())
}
