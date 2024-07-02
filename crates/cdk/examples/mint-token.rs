use std::sync::Arc;
use std::time::Duration;

use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::error::Error;
use cdk::nuts::{CurrencyUnit, MintQuoteState};
use cdk::wallet::Wallet;
use cdk::Amount;
use rand::Rng;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let localstore = WalletMemoryDatabase::default();
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    let mint_url = "https://testnut.cashu.space";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed);

    let quote = wallet.mint_quote(amount).await.unwrap();

    println!("Quote: {:#?}", quote);

    loop {
        let status = wallet.mint_quote_state(&quote.id).await.unwrap();

        println!("Quote status: {}", status.state);

        if status.state == MintQuoteState::Paid {
            break;
        }

        sleep(Duration::from_secs(5)).await;
    }

    let receive_amount = wallet
        .mint(&quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    println!("Received {receive_amount} from mint {mint_url}");

    let token = wallet
        .send(amount, None, None, &SplitTarget::default())
        .await
        .unwrap();

    println!("Token:");
    println!("{}", token);

    Ok(())
}
