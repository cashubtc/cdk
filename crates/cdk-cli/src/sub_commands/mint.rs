use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use cdk::amount::SplitTarget;
use cdk::cdk_database::{Error, WalletDatabase};
use cdk::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod};
use cdk::url::UncheckedUrl;
use cdk::wallet::multi_mint_wallet::WalletKey;
use cdk::wallet::{MultiMintWallet, Wallet};
use cdk::Amount;
use clap::Args;
use payjoin::PjUriBuilder;
use tokio::time::sleep;

#[derive(Args, Debug)]
pub struct MintSubCommand {
    /// Mint url
    mint_url: UncheckedUrl,
    /// Amount
    amount: u64,
    /// Currency unit e.g. sat
    #[arg(short, long, default_value = "sat")]
    unit: String,
    #[arg(long, default_value = "bolt11")]
    method: String,
    /// Payjoin relay
    #[arg(short, long, default_value = "https://pj.bobspacebkk.com")]
    payjoin_relay: String,
}

pub async fn mint(
    multi_mint_wallet: &MultiMintWallet,
    seed: &[u8],
    localstore: Arc<dyn WalletDatabase<Err = Error> + Sync + Send>,
    sub_command_args: &MintSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    println!("{:?}", sub_command_args);
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;

    let method = PaymentMethod::from_str(&sub_command_args.method)?;

    let wallet = match multi_mint_wallet
        .get_wallet(&WalletKey::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
    {
        Some(wallet) => wallet.clone(),
        None => {
            let wallet = Wallet::new(&mint_url.to_string(), unit, localstore, seed, None);

            multi_mint_wallet.add_wallet(wallet.clone()).await;
            wallet
        }
    };

    let quote_id;

    match method {
        PaymentMethod::Bolt11 => {
            let quote = wallet
                .mint_quote(Amount::from(sub_command_args.amount))
                .await?;
            quote_id = quote.id.clone();
            println!("Quote: {:#?}", quote);

            println!("Please pay: {}", quote.request);

            loop {
                let status = wallet.mint_quote_state(&quote.id).await?;

                if status.state == MintQuoteState::Paid {
                    break;
                }

                sleep(Duration::from_secs(2)).await;
            }
        }
        PaymentMethod::BtcOnChain => {
            let quote = wallet
                .mint_onchain_quote(Amount::from(sub_command_args.amount))
                .await?;
            quote_id = quote.quote.clone();
            println!("Quote: {:#?}", quote);

            match quote.payjoin {
                Some(payjoin_info) => {
                    let ohttp_keys = payjoin::io::fetch_ohttp_keys(
                        sub_command_args.payjoin_relay.parse()?,
                        payjoin_info.origin.parse()?,
                    )
                    .await?;

                    let address = payjoin::bitcoin::Address::from_str(&quote.address)?;

                    let uri = PjUriBuilder::new(
                        address.assume_checked(),
                        payjoin_info.origin.parse()?,
                        Some(ohttp_keys),
                        None,
                    )
                    .amount(payjoin::bitcoin::Amount::from_sat(sub_command_args.amount))
                    .pjos(false)
                    .build();

                    println!("Please pay: ");
                    println!("{}", uri);
                }

                None => {
                    println!("please pay: {}", quote.address);
                }
            }

            loop {
                let status = wallet.mint_onchain_quote_state(&quote.quote).await?;

                if status.state == MintQuoteState::Paid {
                    break;
                }

                sleep(Duration::from_secs(2)).await;
            }
        }
    };

    let receive_amount = wallet.mint(&quote_id, SplitTarget::default(), None).await?;

    println!("Received {receive_amount} from mint {mint_url}");

    Ok(())
}
