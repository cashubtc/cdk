//! Wallet example with memory store

use std::sync::Arc;

use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::nuts::{CurrencyUnit, MintQuoteState, NotificationPayload};
use cdk::wallet::{Wallet, WalletSubscription};
use cdk::Amount;
use rand::Rng;

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

        let mut subscription = wallet
            .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![quote
                .id
                .clone()]))
            .await;

        while let Some(msg) = subscription.recv().await {
            if let NotificationPayload::MintQuoteBolt11Response(response) = msg {
                if response.state == MintQuoteState::Paid {
                    break;
                }
            }
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
