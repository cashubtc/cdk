#![allow(missing_docs)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use cdk::amount::SplitTarget;
use cdk::error::Error;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod};
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;
use tokio::time::sleep;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn,hyper_util=warn,reqwest=warn,rustls=warn";

    let env_filter = EnvFilter::new(format!("{},{}", default_filter, sqlx_filter));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let localstore = Arc::new(memory::empty().await?);

    let seed = random::<[u8; 64]>();

    let mint_url = "https://fake.thesimplekid.dev";
    // let mint_url = "http://127.0.0.1:8085";
    let unit = CurrencyUnit::Sat;

    let wallet = Wallet::new(mint_url, unit, localstore.clone(), seed, None)?;

    let amount1 = Amount::from(10);
    let amount2 = Amount::from(20);
    let amount3 = Amount::from(30);

    println!("Creating 3 mint quotes...");
    let quote1 = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount1), None, None)
        .await?;
    println!("Quote 1: {} - {}", quote1.id, quote1.request);

    let quote2 = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount2), None, None)
        .await?;
    println!("Quote 2: {} - {}", quote2.id, quote2.request);

    let quote3 = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount3), None, None)
        .await?;
    println!("Quote 3: {} - {}", quote3.id, quote3.request);

    let quote_ids = [quote1.id.as_str(), quote2.id.as_str(), quote3.id.as_str()];

    println!("\nWaiting for all batch quotes to be PAID...");
    let deadline = Instant::now() + Duration::from_secs(15);

    loop {
        let statuses = wallet.batch_check_mint_quote_status(&quote_ids).await?;
        for q in &statuses {
            println!("  Quote {}: {}", q.id, q.state);
        }

        if statuses
            .iter()
            .all(|q| matches!(q.state, MintQuoteState::Paid))
        {
            break;
        }

        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }

        sleep(Duration::from_millis(500)).await;
    }

    let proofs = wallet
        .batch_mint(&quote_ids, SplitTarget::default(), None, None)
        .await?;

    println!(
        "\nBatch mint complete: minted {} sats in {} proofs",
        proofs.total_amount()?,
        proofs.len()
    );

    Ok(())
}
