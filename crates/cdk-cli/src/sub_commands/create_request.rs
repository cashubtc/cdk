use anyhow::Result;
use cdk::nuts::nut18::TransportType;
use cdk::nuts::{CurrencyUnit, PaymentRequest, PaymentRequestPayload, Token, Transport};
use cdk::wallet::MultiMintWallet;
use clap::Args;
use nostr_sdk::nips::nip19::Nip19Profile;
use nostr_sdk::prelude::*;
use nostr_sdk::{Client as NostrClient, Filter, Keys, ToBech32};

#[derive(Args)]
pub struct CreateRequestSubCommand {
    #[arg(short, long)]
    amount: Option<u64>,
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
    /// Quote description
    description: Option<String>,
}

pub async fn create_request(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &CreateRequestSubCommand,
) -> Result<()> {
    let keys = Keys::generate();
    let relays = vec!["wss://relay.nos.social", "wss://relay.damus.io"];

    let nprofile = Nip19Profile::new(keys.public_key, relays.clone())?;

    let nostr_transport = Transport {
        _type: TransportType::Nostr,
        target: nprofile.to_bech32()?,
        tags: Some(vec![vec!["n".to_string(), "17".to_string()]]),
    };

    let mints: Vec<cdk::mint_url::MintUrl> = multi_mint_wallet
        .get_balances(&CurrencyUnit::Sat)
        .await?
        .keys()
        .cloned()
        .collect();

    let req = PaymentRequest {
        payment_id: None,
        amount: sub_command_args.amount.map(|a| a.into()),
        unit: None,
        single_use: Some(true),
        mints: Some(mints),
        description: sub_command_args.description.clone(),
        transports: vec![nostr_transport],
    };

    println!("{}", req);

    let client = NostrClient::new(keys);

    let filter = Filter::new().pubkey(nprofile.public_key);

    for relay in relays {
        client.add_read_relay(relay).await?;
    }

    client.connect().await;

    client.subscribe(vec![filter], None).await?;

    // Handle subscription notifications with `handle_notifications` method
    client
        .handle_notifications(|notification| async {
            let mut exit = false;
            if let RelayPoolNotification::Event {
                subscription_id: _,
                event,
                ..
            } = notification
            {
                let unwrapped = client.unwrap_gift_wrap(&event).await?;

                let rumor = unwrapped.rumor;

                let payload: PaymentRequestPayload = serde_json::from_str(&rumor.content)?;

                let token = Token::new(payload.mint, payload.proofs, payload.memo, payload.unit);

                let amount = multi_mint_wallet
                    .receive(&token.to_string(), &[], &[])
                    .await?;

                println!("Received {}", amount);
                exit = true;
            }
            Ok(exit) // Set to true to exit from the loop
        })
        .await?;

    Ok(())
}
