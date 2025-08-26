use std::io::{self, Write};

use anyhow::{anyhow, Result};
use cdk::nuts::nut18::TransportType;
use cdk::nuts::{PaymentRequest, PaymentRequestPayload, Token};
use cdk::wallet::{MultiMintWallet, SendOptions};
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

    if payment_request.transports.is_empty() {
        return Err(anyhow!("Cannot pay request without transport"));
    }
    let transports = payment_request.transports.clone();

    // We prefer nostr transport if it is available to hide ip.
    let transport = transports
        .iter()
        .find(|t| t._type == TransportType::Nostr)
        .or_else(|| {
            transports
                .iter()
                .find(|t| t._type == TransportType::HttpPost)
        });

    let prepared_send = matching_wallet
        .prepare_send(
            amount,
            SendOptions {
                include_fee: true,
                ..Default::default()
            },
        )
        .await?;

    let token = prepared_send.confirm(None).await?;

    // We need the keysets information to properly convert from token proof to proof
    let keysets_info = match matching_wallet
        .localstore
        .get_mint_keysets(token.mint_url()?)
        .await?
    {
        Some(keysets_info) => keysets_info,
        None => matching_wallet.load_mint_keysets().await?, // Hit the keysets endpoint if we don't have the keysets for this Mint
    };
    let proofs = token.proofs(&keysets_info)?;

    if let Some(transport) = transport {
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
                )
                .build(nprofile.public_key);
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
                    println!("{res:?}");
                    println!("Error posting payment");
                }
            }
        }
    } else {
        // If no transport is available, print the token
        let token = Token::new(
            matching_wallet.mint_url.clone(),
            proofs,
            None,
            matching_wallet.unit.clone(),
        );
        println!("Token: {token}");
    }

    Ok(())
}
