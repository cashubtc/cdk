#![allow(missing_docs)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cdk::amount::SplitTarget;
use cdk::error::Error;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::advanced::{MintBatchClaimRequest, MintBatchRefreshRequest};
use cdk::wallet::mint::{MintRequest, MintState};
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

    let mint_url = "https://testnut.cashudevkit.org";
    // let mint_url = "http://127.0.0.1:8085";
    let unit = CurrencyUnit::Sat;

    let wallet = Wallet::open(cdk::wallet::WalletOpenRequest::new(
        cdk::wallet::WalletIdentity::new(mint_url.parse()?, unit),
        localstore.clone(),
        seed,
    ))?;

    let amount1 = Amount::from(10);
    let amount2 = Amount::from(20);
    let amount3 = Amount::from(30);

    println!("Creating 3 mint quotes...");
    let session1 = wallet.request_mint(MintRequest::bolt11(amount1)).await?;
    println!(
        "Quote 1: {} - {}",
        session1.id(),
        session1.initial_state().payment_request
    );

    let session2 = wallet.request_mint(MintRequest::bolt11(amount2)).await?;
    println!(
        "Quote 2: {} - {}",
        session2.id(),
        session2.initial_state().payment_request
    );

    let session3 = wallet.request_mint(MintRequest::bolt11(amount3)).await?;
    println!(
        "Quote 3: {} - {}",
        session3.id(),
        session3.initial_state().payment_request
    );

    let quote_ids = vec![
        session1.id().clone(),
        session2.id().clone(),
        session3.id().clone(),
    ];

    println!("\nWaiting for all batch quotes to be PAID...");
    let deadline = Instant::now() + Duration::from_secs(15);

    loop {
        let statuses = wallet
            .advanced()
            .refresh_mint_batch(MintBatchRefreshRequest {
                quote_ids: quote_ids.clone(),
            })
            .await?;
        for state in &statuses {
            println!("  Quote {}: {}", state.id, state.state);
        }

        if statuses.iter().all(|state| state.state == MintState::Paid) {
            break;
        }

        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }

        sleep(Duration::from_millis(500)).await;
    }

    let receipt = wallet
        .advanced()
        .claim_mint_batch(MintBatchClaimRequest {
            quote_ids,
            amount_split_target: SplitTarget::default(),
            conditions: None,
            external_keys: HashMap::new(),
        })
        .await?;

    println!(
        "\nBatch mint complete: minted {} sats in {} proofs",
        receipt.amount,
        receipt.proofs.len()
    );

    Ok(())
}
