use std::sync::Arc;

use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::error::Error;
use cdk::nuts::{CurrencyUnit, MintQuoteState, NotificationPayload};
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

    println!("Quote: {:#?}", quote);

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

    println!("Received {receive_amount} from mint {mint_url}");

    let token = wallet
        .send(
            amount,
            None,
            None,
            &SplitTarget::default(),
            &SendKind::OnlineExact,
            false,
        )
        .await
        .unwrap();

    println!("Token:");
    println!("{}", token);

    Ok(())
}
