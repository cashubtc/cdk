use std::sync::Arc;

use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::error::Error;
use cdk::nuts::{CurrencyUnit, MintQuoteState, NotificationPayload, SecretKey, SpendingConditions};
use cdk::wallet::types::SendKind;
use cdk::wallet::{Wallet, WalletSubscription};
use cdk::Amount;
use rand::Rng;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let localstore = WalletMemoryDatabase::default();
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    let mint_url = "https://testnut.cashu.space";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None).unwrap();

    let quote = wallet.mint_quote(amount, None).await.unwrap();

    println!("Minting nuts ...");

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

    let _receive_amount = wallet
        .mint(&quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let secret = SecretKey::generate();

    let spending_conditions = SpendingConditions::new_p2pk(secret.public_key(), None);

    let bal = wallet.total_balance().await.unwrap();

    println!("{}", bal);

    let token = wallet
        .send(
            amount,
            None,
            Some(spending_conditions),
            &SplitTarget::default(),
            &SendKind::default(),
            false,
        )
        .await
        .unwrap();

    println!("Created token locked to pubkey: {}", secret.public_key());
    println!("{}", token);

    let amount = wallet
        .receive(&token.to_string(), SplitTarget::default(), &[secret], &[])
        .await
        .unwrap();

    println!("Redeemed locked token worth: {}", u64::from(amount));

    Ok(())
}
