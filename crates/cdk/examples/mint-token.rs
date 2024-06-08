use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::error::Error;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet;
use cdk::{Amount, UncheckedUrl};
use rand::Rng;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let localstore = WalletMemoryDatabase::default();
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    let mint_url = UncheckedUrl::from_str("https://testnut.cashu.space").unwrap();
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    let wallet = Wallet::new(Arc::new(localstore), &seed, vec![]);

    let quote = wallet
        .mint_quote(mint_url.clone(), amount, unit.clone())
        .await
        .unwrap();

    println!("Quote: {:#?}", quote);

    loop {
        let status = wallet
            .mint_quote_status(mint_url.clone(), &quote.id)
            .await
            .unwrap();

        println!("Quote status: {}", status.paid);

        if status.paid {
            break;
        }

        sleep(Duration::from_secs(5)).await;
    }

    let receive_amount = wallet
        .mint(mint_url.clone(), &quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    println!("Received {receive_amount} from mint {mint_url}");

    let token = wallet
        .send(&mint_url, unit, None, amount, &SplitTarget::None, None)
        .await
        .unwrap();

    println!("Token:");
    println!("{}", token);

    Ok(())
}
