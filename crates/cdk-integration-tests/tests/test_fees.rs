use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cashu::Bolt11Invoice;
use cdk::amount::Amount;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::advanced::{
    FeeEstimateRequest, PaymentFunding, PaymentPrepareOptions, ProofQuery, WalletBuilder,
};
use cdk::wallet::mint::MintRequest;
use cdk::wallet::payment::{PaymentQuoteRequest, PaymentTarget};
use cdk::wallet::receive::ReceiveRequest;
use cdk::wallet::send::{SendMode, SendRequest};
use cdk::wallet::Wallet;
use cdk_integration_tests::init_regtest::get_temp_dir;
use cdk_integration_tests::{create_invoice_for_env, get_mint_url_from_env, pay_if_regtest};
use cdk_sqlite::wallet::memory;
use tracing_subscriber::EnvFilter;

async fn funded_wallet() -> Wallet {
    let session_seed = Mnemonic::generate(12)
        .expect("mnemonic")
        .to_seed_normalized("");
    let wallet = WalletBuilder::new()
        .with_mint_url(get_mint_url_from_env().parse().expect("mint URL"))
        .with_unit(CurrencyUnit::Sat)
        .with_store(Arc::new(memory::empty().await.expect("database")))
        .with_seed(session_seed)
        .build()
        .expect("wallet");
    let session = wallet
        .request_mint(MintRequest::bolt11(100.into()))
        .await
        .expect("mint session");
    let invoice =
        Bolt11Invoice::from_str(&session.initial_state().payment_request).expect("BOLT11 invoice");
    pay_if_regtest(&get_temp_dir(), &invoice)
        .await
        .expect("pay invoice");
    session
        .wait(Duration::from_secs(60))
        .await
        .expect("claim mint quote");
    wallet
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn send_fee_is_visible_before_confirmation() {
    let default_filter = "debug";
    let sqlx_filter = "sqlx=warn,hyper_util=warn,reqwest=warn,rustls=warn";
    let env_filter = EnvFilter::new(format!("{default_filter},{sqlx_filter}"));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
    let wallet = funded_wallet().await;

    let mut request = SendRequest::new(4.into());
    request.mode = SendMode::OfflineExact;
    request.include_fee = false;
    let plan = wallet.plan_send(request).await.expect("send plan");

    assert_eq!(plan.fee(), 1.into());

    let token = plan.execute().await.expect("send receipt").token;
    let received = wallet
        .receive(ReceiveRequest::new(token.to_string()))
        .await
        .expect("receive token");

    assert_eq!(received.amount, 3.into());
    assert_eq!(
        wallet.balance().await.expect("balance").available,
        99.into()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn explicit_payment_funding_reports_the_same_input_fee() {
    let wallet = funded_wallet().await;
    let invoice_amount = 9;
    let invoice = create_invoice_for_env(Some(invoice_amount))
        .await
        .expect("invoice");
    let session = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice)))
        .await
        .expect("payment quote")
        .into_single()
        .expect("single payment quote");

    let proofs = wallet
        .advanced()
        .proofs(ProofQuery::default())
        .await
        .expect("proofs")
        .into_iter()
        .map(|record| record.proof)
        .collect::<Vec<_>>();
    let fee = wallet
        .advanced()
        .estimate_fee(FeeEstimateRequest::Proofs(proofs.clone()))
        .await
        .expect("fee estimate")
        .total;

    let receipt = session
        .prepare_with(PaymentPrepareOptions {
            funding: PaymentFunding::Proofs(proofs),
        })
        .await
        .expect("payment plan")
        .execute()
        .await
        .expect("payment receipt");

    assert_eq!(
        wallet.balance().await.expect("balance").available,
        Amount::from(100 - invoice_amount - u64::from(fee) - u64::from(receipt.fee_paid))
    );
}
