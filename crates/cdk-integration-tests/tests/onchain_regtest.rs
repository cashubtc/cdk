//! Onchain Regtest Integration Tests
//!
//! This file contains tests for NUT-26 onchain payments against a regtest environment.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::{CurrencyUnit, NotificationPayload, PaymentMethod, PreMintSecrets};
use cdk::wallet::advanced::{
    HttpClient, MetadataSource, MintConnector, MintMetadataRequest, MintSessionFilter,
    SubscriptionRequest,
};
use cdk::wallet::mint::{MintRequest as WalletMintRequest, MintSession, MintState};
use cdk::wallet::operation::SyncPolicy;
use cdk::wallet::payment::{
    PaymentConfirmation, PaymentPlan, PaymentQuoteRequest, PaymentSession, PaymentTarget,
};
use cdk::wallet::{RestoreRequest, Wallet};
use cdk_integration_tests::get_mint_url_from_env;
use cdk_integration_tests::init_regtest::init_bitcoin_client;
use cdk_integration_tests::ln_regtest::bitcoin_client::BitcoinClient;
use cdk_sqlite::wallet::memory;
use futures::StreamExt;
use tokio::time::timeout;

fn onchain_method() -> PaymentMethod {
    PaymentMethod::from_str("onchain").expect("onchain payment method")
}

async fn request_onchain_mint(wallet: &Wallet, amount: u64) -> MintSession {
    wallet
        .request_mint(WalletMintRequest::new(
            onchain_method(),
            Some(Amount::from(amount)),
        ))
        .await
        .expect("onchain mint session")
}

fn pay_and_mine(bitcoin_client: &BitcoinClient, session: &MintSession, amount: u64) -> String {
    bitcoin_client
        .send_to_address(&session.initial_state().payment_request, amount)
        .expect("send bitcoin to mint");
    let mine_addr = bitcoin_client.get_new_address().expect("mining address");
    bitcoin_client
        .generate_blocks(&mine_addr, 1)
        .expect("mine confirmation block");
    mine_addr
}

async fn fund_onchain(wallet: &Wallet, bitcoin_client: &BitcoinClient, amount: u64) {
    let session = request_onchain_mint(wallet, amount).await;
    pay_and_mine(bitcoin_client, &session, amount);
    let receipt = session
        .wait(Duration::from_secs(60))
        .await
        .expect("claim onchain mint");
    assert_eq!(receipt.amount, Amount::from(amount));
}

async fn wait_until_paid(session: &MintSession) -> cdk::wallet::mint::MintSessionState {
    timeout(Duration::from_secs(30), async {
        loop {
            let state = session.refresh().await.expect("refresh mint session");
            if matches!(state.state, MintState::Paid | MintState::Issued) {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("onchain payment timeout")
}

async fn onchain_payment_options(
    wallet: &Wallet,
    address: impl Into<String>,
    amount: u64,
) -> Vec<PaymentSession> {
    wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::Onchain {
            address: address.into(),
            amount: Amount::from(amount),
            max_fee: None,
        }))
        .await
        .expect("onchain payment options")
        .into_sessions()
}

async fn confirm_with_mining(
    plan: &PaymentPlan,
    bitcoin_client: &BitcoinClient,
    mine_addr: &str,
) -> cdk::wallet::payment::PaymentReceipt {
    timeout(Duration::from_secs(60), async {
        let confirm = plan.execute();
        tokio::pin!(confirm);
        loop {
            tokio::select! {
                result = &mut confirm => return result.expect("confirm onchain payment"),
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    bitcoin_client
                        .generate_blocks(mine_addr, 1)
                        .expect("mine payment confirmation");
                }
            }
        }
    })
    .await
    .expect("onchain payment confirmation timeout")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    let mint_amount = 10_000;
    let session = request_onchain_mint(&wallet, mint_amount).await;

    assert!(session.initial_state().payment_request.starts_with("bcrt1"));
    let mut subscription = wallet
        .advanced()
        .subscribe(SubscriptionRequest::OnchainMintQuotes(vec![session
            .id()
            .to_string()]))
        .await
        .expect("subscription");
    pay_and_mine(&bitcoin_client, &session, mint_amount);

    let paid_amount = timeout(Duration::from_secs(30), async {
        while let Some(message) = subscription.recv().await {
            if let NotificationPayload::MintQuoteOnchainResponse(response) = message.into_inner() {
                assert_eq!(response.quote, session.id().as_str());
                if response.amount_paid == Amount::from(mint_amount) {
                    return response.amount_paid;
                }
            }
        }
        panic!("notification stream ended")
    })
    .await
    .expect("paid notification");
    assert_eq!(paid_amount, Amount::from(mint_amount));

    let receipt = session.claim().await.expect("mint claim");
    assert_eq!(receipt.amount, Amount::from(mint_amount));
    assert_eq!(wallet.balance().await.unwrap().available, receipt.amount);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_melt() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    fund_onchain(&wallet, &bitcoin_client, 50_000).await;

    let destination = bitcoin_client.get_new_address().unwrap();
    let options = onchain_payment_options(&wallet, destination, 20_000).await;
    assert!(!options.is_empty());
    let plan = options[0].prepare().await.expect("payment plan");
    let mine_addr = bitcoin_client.get_new_address().unwrap();
    let receipt = confirm_with_mining(&plan, &bitcoin_client, &mine_addr).await;
    assert_eq!(receipt.amount, Amount::from(20_000));
    assert!(wallet.balance().await.unwrap().available < Amount::from(30_000));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_melt_selects_standard_fee_option() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    fund_onchain(&wallet, &bitcoin_client, 50_000).await;

    let destination = bitcoin_client.get_new_address().unwrap();
    let options = onchain_payment_options(&wallet, destination, 20_000).await;
    let standard = options.get(1).expect("standard onchain fee option").clone();
    let preview = standard.quote();
    assert!(preview.estimated_blocks.is_some());
    let plan = standard.prepare().await.expect("standard payment plan");
    assert!(plan.maximum_fee() >= preview.fee_reserve);

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    confirm_with_mining(&plan, &bitcoin_client, &mine_addr).await;
    assert!(wallet.balance().await.unwrap().available < Amount::from(30_000));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_restore() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("wallet");
    fund_onchain(&wallet, &bitcoin_client, 20_000).await;

    let restored_wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("restored wallet");
    assert_eq!(
        restored_wallet.balance().await.unwrap().available,
        Amount::ZERO
    );
    let restored = restored_wallet
        .restore_from_seed(RestoreRequest::default())
        .await
        .unwrap();
    assert_eq!(restored.unspent, Amount::from(20_000));
    assert_eq!(
        restored_wallet.balance().await.unwrap().available,
        Amount::from(20_000)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint_multiple_payments() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    let session = request_onchain_mint(&wallet, 30_000).await;
    let mine_addr = pay_and_mine(&bitcoin_client, &session, 10_000);
    bitcoin_client
        .send_to_address(&session.initial_state().payment_request, 20_000)
        .unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    let mut receipts = Box::pin(session.receipts(Default::default()));
    let mut issued = Amount::ZERO;
    while issued < Amount::from(30_000) {
        issued += receipts
            .next()
            .await
            .expect("receipt event")
            .expect("receipt")
            .amount;
    }
    assert_eq!(issued, Amount::from(30_000));
    assert_eq!(wallet.balance().await.unwrap().available, issued);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_melt_prefer_async() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    fund_onchain(&wallet, &bitcoin_client, 50_000).await;

    let destination = bitcoin_client.get_new_address().unwrap();
    let session = onchain_payment_options(&wallet, destination, 20_000)
        .await
        .into_iter()
        .next()
        .expect("payment option");
    let plan = session.prepare().await.unwrap();
    match plan.submit().await.unwrap() {
        PaymentConfirmation::Pending(pending) => {
            let wait = pending.wait();
            tokio::pin!(wait);
            let receipt = timeout(Duration::from_secs(60), async {
                loop {
                    tokio::select! {
                        result = &mut wait => break result.expect("finalized payment"),
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {
                            let mine_addr = bitcoin_client.get_new_address().unwrap();
                            bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                        }
                    }
                }
            })
            .await
            .expect("async payment timeout");
            assert_eq!(receipt.amount, Amount::from(20_000));
        }
        PaymentConfirmation::Completed(_) => panic!("expected pending onchain payment"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint_underpaid() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    let session = request_onchain_mint(&wallet, 20_000).await;
    pay_and_mine(&bitcoin_client, &session, 15_000);
    let state = wait_until_paid(&session).await;
    assert!(state.amount_paid >= Amount::from(15_000));
    let receipt = session.claim().await.unwrap();
    assert_eq!(receipt.amount, Amount::from(15_000));
    assert_eq!(wallet.balance().await.unwrap().available, receipt.amount);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint_unique_addresses() {
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    let first = request_onchain_mint(&wallet, 10_000).await;
    let second = request_onchain_mint(&wallet, 10_000).await;
    assert_ne!(
        first.initial_state().payment_request,
        second.initial_state().payment_request
    );
    assert!(first.initial_state().payment_request.starts_with("bcrt1"));
    assert!(second.initial_state().payment_request.starts_with("bcrt1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_concurrent_mint_quotes() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    let (first, second, third) = tokio::join!(
        request_onchain_mint(&wallet, 10_000),
        request_onchain_mint(&wallet, 20_000),
        request_onchain_mint(&wallet, 30_000),
    );
    bitcoin_client
        .send_to_address(&first.initial_state().payment_request, 10_000)
        .unwrap();
    bitcoin_client
        .send_to_address(&second.initial_state().payment_request, 20_000)
        .unwrap();
    bitcoin_client
        .send_to_address(&third.initial_state().payment_request, 30_000)
        .unwrap();
    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    let (first, second, third) = tokio::try_join!(
        first.wait(Duration::from_secs(30)),
        second.wait(Duration::from_secs(30)),
        third.wait(Duration::from_secs(30)),
    )
    .expect("concurrent claims");
    assert_eq!(first.amount, Amount::from(10_000));
    assert_eq!(second.amount, Amount::from(20_000));
    assert_eq!(third.amount, Amount::from(30_000));
    assert_eq!(
        wallet.balance().await.unwrap().available,
        Amount::from(60_000)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_concurrent_melt_quotes() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    fund_onchain(&wallet, &bitcoin_client, 200_000).await;

    let first = onchain_payment_options(&wallet, bitcoin_client.get_new_address().unwrap(), 20_000)
        .await
        .remove(0);
    let second =
        onchain_payment_options(&wallet, bitcoin_client.get_new_address().unwrap(), 30_000)
            .await
            .remove(0);
    let third = onchain_payment_options(&wallet, bitcoin_client.get_new_address().unwrap(), 40_000)
        .await
        .remove(0);

    let (first, second, third) =
        tokio::try_join!(first.prepare(), second.prepare(), third.prepare(),)
            .expect("concurrent payment plans");
    let first_confirm = first.execute();
    let second_confirm = second.execute();
    let third_confirm = third.execute();
    tokio::pin!(first_confirm, second_confirm, third_confirm);
    let mut complete = [false; 3];
    timeout(Duration::from_secs(120), async {
        while complete.iter().any(|done| !done) {
            tokio::select! {
                result = &mut first_confirm, if !complete[0] => {
                    result.expect("first payment");
                    complete[0] = true;
                }
                result = &mut second_confirm, if !complete[1] => {
                    result.expect("second payment");
                    complete[1] = true;
                }
                result = &mut third_confirm, if !complete[2] => {
                    result.expect("third payment");
                    complete[2] = true;
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    let mine_addr = bitcoin_client.get_new_address().unwrap();
                    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                }
            }
        }
    })
    .await
    .expect("concurrent payments timeout");
    assert!(wallet.balance().await.unwrap().available < Amount::from(110_000));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_unissued_quotes_onchain() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let localstore = Arc::new(memory::empty().await.unwrap());
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        localstore.clone(),
        seed,
        None,
    )
    .expect("wallet");
    let session = request_onchain_mint(&wallet, 10_000).await;
    let unissued = wallet
        .advanced()
        .mint_sessions(MintSessionFilter::Unissued)
        .await
        .unwrap();
    assert!(unissued.iter().any(|known| known.id() == session.id()));

    pay_and_mine(&bitcoin_client, &session, 10_000);
    wait_until_paid(&session).await;
    assert_eq!(wallet.balance().await.unwrap().available, Amount::ZERO);
    let report = wallet.synchronize(SyncPolicy::Online).await.unwrap();
    assert_eq!(report.claimed_amount, Amount::from(10_000));
    assert_eq!(report.balance.available, Amount::from(10_000));
    assert_eq!(
        wallet
            .synchronize(SyncPolicy::Online)
            .await
            .unwrap()
            .claimed_amount,
        Amount::ZERO
    );

    let restarted = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        localstore,
        seed,
        None,
    )
    .expect("restarted wallet");
    assert_eq!(
        restarted.balance().await.unwrap().available,
        Amount::from(10_000)
    );
    assert_eq!(
        restarted
            .synchronize(SyncPolicy::Online)
            .await
            .unwrap()
            .claimed_amount,
        Amount::ZERO
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_check_all_mint_quotes_onchain() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    let session = request_onchain_mint(&wallet, 10_000).await;
    let mut subscription = wallet
        .advanced()
        .subscribe(SubscriptionRequest::OnchainMintQuotes(vec![session
            .id()
            .to_string()]))
        .await
        .expect("subscription");
    pay_and_mine(&bitcoin_client, &session, 10_000);

    timeout(Duration::from_secs(30), async {
        while let Some(message) = subscription.recv().await {
            if let NotificationPayload::MintQuoteOnchainResponse(response) = message.into_inner() {
                if response.amount_paid == Amount::from(10_000) {
                    return;
                }
            }
        }
    })
    .await
    .expect("paid notification");
    let report = wallet.synchronize(SyncPolicy::Online).await.unwrap();
    assert_eq!(report.claimed_amount, Amount::from(10_000));
    assert_eq!(report.balance.available, Amount::from(10_000));
    assert_eq!(
        wallet
            .synchronize(SyncPolicy::Online)
            .await
            .unwrap()
            .claimed_amount,
        Amount::ZERO
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_quote_amount_issued_tracking() {
    let bitcoin_client = init_bitcoin_client().expect("bitcoin client");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    let session = request_onchain_mint(&wallet, 10_000).await;
    assert_eq!(session.initial_state().amount_paid, Amount::ZERO);
    assert_eq!(session.initial_state().amount_claimed, Amount::ZERO);
    pay_and_mine(&bitcoin_client, &session, 10_000);
    let paid = wait_until_paid(&session).await;
    assert_eq!(paid.amount_paid, Amount::from(10_000));
    assert_eq!(paid.amount_claimed, Amount::ZERO);
    session.claim().await.unwrap();
    let issued = session.refresh().await.unwrap();
    assert_eq!(issued.amount_paid, Amount::from(10_000));
    assert_eq!(issued.amount_claimed, Amount::from(10_000));
    assert_eq!(issued.state, MintState::Issued);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_attempt_to_mint_unpaid() {
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("wallet");
    let session = request_onchain_mint(&wallet, 10_000).await;
    let active_keyset = wallet
        .advanced()
        .mint_metadata(MintMetadataRequest {
            source: MetadataSource::Refresh,
        })
        .await
        .unwrap()
        .active_keyset()
        .expect("active keyset")
        .id;
    let fee_and_amounts = (0, (0..32).map(|power| 2u64.pow(power)).collect::<Vec<_>>()).into();
    let premint = PreMintSecrets::random(
        active_keyset,
        Amount::from(10_000),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();
    let request = cdk::nuts::MintRequest {
        quote: session.id().to_string(),
        outputs: premint.blinded_messages(),
        signature: None,
    };
    let response = HttpClient::new(get_mint_url_from_env().parse().unwrap(), None)
        .post_mint(&onchain_method(), request)
        .await;
    assert!(matches!(response, Err(cdk::Error::UnpaidQuote)));
}
