//! Wallet example with memory store

use std::sync::Arc;
use std::time::Duration;

use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::nuts::{CurrencyUnit, MintQuoteState};
use cdk::wallet::Wallet;
use cdk::Amount;
use rand::Rng;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    let mint_url = "https://testnut.cashu.space";
    let unit = CurrencyUnit::Sat;

    let localstore = WalletMemoryDatabase::default();

    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None).unwrap();

    for amount in [64] {
        let amount = Amount::from(amount);
        let quote = wallet.mint_quote(amount, None).await.unwrap();

        println!("Pay request: {}", quote.request);

        loop {
            let status = wallet.mint_quote_state(&quote.id).await.unwrap();

            if status.state == MintQuoteState::Paid {
                break;
            }

            println!("Quote state: {}", status.state);

            sleep(Duration::from_secs(5)).await;
        }

        let receive_amount = wallet
            .mint(&quote.id, SplitTarget::default(), None)
            .await
            .unwrap();

        println!("Minted {}", receive_amount);
    }

    let proofs = wallet.get_unspent_proofs().await.unwrap();

    let selected = wallet
        .select_proofs_to_send(Amount::from(64), proofs, false)
        .await
        .unwrap();

    for (i, proof) in selected.iter().enumerate() {
        println!("{}: {}", i, proof.amount);
    }
}
