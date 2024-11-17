use std::io::{self, Write};

use anyhow::{anyhow, Result};
use cdk::amount::SplitTarget;
use cdk::nuts::nut18::TransportType;
use cdk::nuts::{PaymentRequest, PaymentRequestPayload};
use cdk::wallet::{MultiMintWallet, SendKind};
use clap::Args;
use nostr_sdk::nips::nip19::Nip19Profile;
use nostr_sdk::{Client as NostrClient, EventBuilder, FromBech32, Keys};
use reqwest::Client;

#[derive(Args)]
pub struct PayRequestSubCommand {
    payment_request: PaymentRequest,
}

pub async fn pay_request(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &PayRequestSubCommand,
) -> Result<()> {
    let payment_request = &sub_command_args.payment_request;

    let unit = &payment_request.unit;

    let amount = match payment_request.amount {
        Some(amount) => amount,
        None => {
            println!("Enter the amount you would like to pay");

            let mut user_input = String::new();
            let stdin = io::stdin();
            io::stdout().flush().unwrap();
            stdin.read_line(&mut user_input)?;

            let amount: u64 = user_input.trim().parse()?;

            amount.into()
        }
    };

    let request_mints = &payment_request.mints;

    let wallet_mints = multi_mint_wallet.get_wallets().await;

    // Wallets where unit, balance and mint match request
    let mut matching_wallets = vec![];

    for wallet in wallet_mints.iter() {
        let balance = wallet.total_balance().await?;

        if let Some(request_mints) = request_mints {
            if !request_mints.contains(&wallet.mint_url) {
                continue;
            }
        }

        if let Some(unit) = unit {
            if &wallet.unit != unit {
                continue;
            }
        }

        if balance >= amount {
            matching_wallets.push(wallet);
        }
    }

    let matching_wallet = matching_wallets.first().unwrap();

    // We prefer nostr transport if it is available to hide ip.
    let transport = payment_request
        .transports
        .iter()
        .find(|t| t._type == TransportType::Nostr)
        .or_else(|| {
            payment_request
                .transports
                .iter()
                .find(|t| t._type == TransportType::HttpPost)
        })
        .ok_or(anyhow!("No supported transport method found"))?;

    let proofs = matching_wallet
        .send(
            amount,
            None,
            None,
            &SplitTarget::default(),
            &SendKind::default(),
            true,
        )
        .await?
        .proofs();

    let payload = PaymentRequestPayload {
        id: payment_request.payment_id.clone(),
        memo: None,
        mint: matching_wallet.mint_url.clone(),
        unit: matching_wallet.unit.clone(),
        proofs,
    };

    match transport._type {
        TransportType::Nostr => {
            let keys = Keys::generate();
            let client = NostrClient::new(keys);
            let nprofile = Nip19Profile::from_bech32(&transport.target)?;

            println!("{:?}", nprofile.relays);

            let rumor = EventBuilder::new(
                nostr_sdk::Kind::from_u16(14),
                serde_json::to_string(&payload)?,
                [],
            );

            let relays = nprofile.relays;

            for relay in relays.iter() {
                client.add_write_relay(relay).await?;
            }

            client.connect().await;

            let gift_wrap = client
                .gift_wrap_to(relays, &nprofile.public_key, rumor, None)
                .await?;

            println!(
                "Published event {} succufully to {}",
                gift_wrap.val,
                gift_wrap
                    .success
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            if !gift_wrap.failed.is_empty() {
                println!(
                    "Could not publish to {:?}",
                    gift_wrap
                        .failed
                        .keys()
                        .map(|relay| relay.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }

        TransportType::HttpPost => {
            let client = Client::new();

            let res = client
                .post(transport.target.clone())
                .json(&payload)
                .send()
                .await?;

            let status = res.status();
            if status.is_success() {
                println!("Successfully posted payment");
            } else {
                println!("{:?}", res);
                println!("Error posting payment");
            }
        }
    }

    Ok(())
}
