//! Create and pay NUT-18 payment requests with the application wallet API.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example payment_request --features="wallet nostr"
//! ```

use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::{CurrencyUnit, PaymentRequest, SecretKey};
use cdk::wallet::mint::MintRequest;
use cdk::wallet::payment_request::{
    CreatePaymentRequest, PaymentRequestLock, PaymentRequestTransport, RequestPayment,
};
use cdk::wallet::{WalletIdentity, WalletManagerBuilder};
use cdk::{Amount, Error};
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let manager = WalletManagerBuilder::new()
        .with_store(Arc::new(memory::empty().await?))
        .with_seed(random::<[u8; 64]>())
        .build()
        .await?;
    let wallet = manager
        .open_wallet(WalletIdentity::new(
            "https://testnut.cashudevkit.org".parse()?,
            CurrencyUnit::Sat,
        ))
        .await?;

    let mint = wallet
        .request_mint(MintRequest::bolt11(Amount::from(100)))
        .await?;
    println!(
        "Fund the example with: {}",
        mint.initial_state().payment_request
    );
    mint.wait(Duration::from_secs(300)).await?;

    let mut nostr_request = CreatePaymentRequest::new(CurrencyUnit::Sat);
    nostr_request.amount = Some(Amount::from(10));
    nostr_request.description = Some("Coffee payment".to_owned());
    nostr_request.transport = PaymentRequestTransport::Nostr(vec![
        "wss://relay.damus.io".to_owned(),
        "wss://nos.lol".to_owned(),
    ]);
    let created = manager.create_payment_request(nostr_request).await?;
    println!("Nostr request: {}", created.payment_request);
    if let Some(receiver) = created.receiver {
        let state = receiver.state();
        println!(
            "Listen on {} as {}",
            state.relays.join(", "),
            state.public_key_hex
        );
        // Persist `state` if listening must survive a process restart. To wait now:
        // let received = receiver.receive().await?;
    }

    let mut http_request = CreatePaymentRequest::new(CurrencyUnit::Sat);
    http_request.amount = Some(Amount::from(21));
    http_request.description = Some("Tip jar".to_owned());
    http_request.transport =
        PaymentRequestTransport::Http("https://example.com/cashu/callback".parse()?);
    let created = manager.create_payment_request(http_request).await?;
    println!("HTTP request: {}", created.payment_request);

    let secret = SecretKey::generate();
    let mut locked_request = CreatePaymentRequest::new(CurrencyUnit::Sat);
    locked_request.amount = Some(Amount::from(50));
    locked_request.lock = Some(PaymentRequestLock::P2pk {
        public_keys: vec![secret.public_key()],
        signatures_required: 1,
    });
    let created = manager.create_payment_request(locked_request).await?;
    println!("P2PK request: {}", created.payment_request);

    // A payer decodes the request, reviews the exact debit, then confirms or cancels it.
    let encoded = created.payment_request.to_string();
    let decoded: PaymentRequest = encoded.parse()?;
    let plan = manager
        .plan_request_payment(RequestPayment::new(decoded))
        .await?;
    println!(
        "Pay {} with {} in fees ({} total)",
        plan.requested_amount(),
        plan.input_fee(),
        plan.total_amount()
    );
    match plan.execute().await {
        Ok(receipt) => println!("Delivered operation {}", receipt.operation_id),
        Err(Error::PaymentRequestDeliveryFailed {
            operation_id,
            source,
        }) => {
            eprintln!("Token creation succeeded but delivery failed: {source}");
            wallet.reclaim_send(operation_id.into()).await?;
        }
        Err(error) => return Err(error.into()),
    }

    Ok(())
}
