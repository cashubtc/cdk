use std::str::FromStr;

use anyhow::{bail, Result};
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use cdk::nuts::nut01::PublicKey;
use cdk::nuts::nut11::{Conditions, SigFlag, SpendingConditions};
use cdk::nuts::nut18::{Nut10SecretRequest, TransportType};
use cdk::nuts::{CurrencyUnit, PaymentRequest, PaymentRequestPayload, Token, Transport};
use cdk::wallet::{MultiMintWallet, ReceiveOptions};
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
    /// P2PK: Public key(s) for which the token can be spent with valid signature(s)
    /// Can be specified multiple times for multiple pubkeys
    #[arg(long, action = clap::ArgAction::Append)]
    pubkey: Option<Vec<String>>,
    /// Number of required signatures (for multiple pubkeys)
    /// Defaults to 1 if not specified
    #[arg(long, default_value = "1")]
    num_sigs: u64,
    /// HTLC: Hash for hash time locked contract
    #[arg(long, conflicts_with = "preimage")]
    hash: Option<String>,
    /// HTLC: Preimage of the hash (to be used instead of hash)
    #[arg(long, conflicts_with = "hash")]
    preimage: Option<String>,
    /// Transport type to use (nostr, http, or none)
    /// - nostr: Use Nostr transport and listen for payment
    /// - http: Use HTTP transport but only print the request
    /// - none: Don't use any transport, just print the request
    #[arg(long, default_value = "nostr")]
    transport: String,
    /// URL for HTTP transport (only used when transport=http)
    #[arg(long)]
    http_url: Option<String>,
    /// Nostr relays to use (only used when transport=nostr)
    /// Can be specified multiple times for multiple relays
    /// If not provided, defaults to standard relays
    #[arg(long, action = clap::ArgAction::Append)]
    nostr_relay: Option<Vec<String>>,
}

pub async fn create_request(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &CreateRequestSubCommand,
) -> Result<()> {
    // Get available mints from the wallet
    let mints: Vec<cdk::mint_url::MintUrl> = multi_mint_wallet
        .get_balances(&CurrencyUnit::Sat)
        .await?
        .keys()
        .cloned()
        .collect();

    // Process transport based on command line args
    let transport_type = sub_command_args.transport.to_lowercase();
    let transports = match transport_type.as_str() {
        "nostr" => {
            let keys = Keys::generate();

            // Use custom relays if provided, otherwise use defaults
            let relays = if let Some(custom_relays) = &sub_command_args.nostr_relay {
                if !custom_relays.is_empty() {
                    println!("Using custom Nostr relays: {custom_relays:?}");
                    custom_relays.clone()
                } else {
                    // Empty vector provided, fall back to defaults
                    vec![
                        "wss://relay.nos.social".to_string(),
                        "wss://relay.damus.io".to_string(),
                    ]
                }
            } else {
                // No relays provided, use defaults
                vec![
                    "wss://relay.nos.social".to_string(),
                    "wss://relay.damus.io".to_string(),
                ]
            };

            let nprofile = Nip19Profile::new(keys.public_key, relays.clone())?;

            let nostr_transport = Transport {
                _type: TransportType::Nostr,
                target: nprofile.to_bech32()?,
                tags: Some(vec![vec!["n".to_string(), "17".to_string()]]),
            };

            // We'll need the Nostr keys and relays later for listening
            let transport_info = Some((keys, relays, nprofile.public_key));

            (vec![nostr_transport], transport_info)
        }
        "http" => {
            if let Some(url) = &sub_command_args.http_url {
                let http_transport = Transport {
                    _type: TransportType::HttpPost,
                    target: url.clone(),
                    tags: None,
                };

                (vec![http_transport], None)
            } else {
                println!(
                    "Warning: HTTP transport selected but no URL provided, skipping transport"
                );
                (vec![], None)
            }
        }
        "none" => (vec![], None),
        _ => {
            println!("Warning: Unknown transport type '{transport_type}', defaulting to none");
            (vec![], None)
        }
    };

    // Create spending conditions based on provided arguments
    // Handle the following cases:
    // 1. Only P2PK condition
    // 2. Only HTLC condition with hash
    // 3. Only HTLC condition with preimage
    // 4. Both P2PK and HTLC conditions

    let spending_conditions = if let Some(pubkey_strings) = &sub_command_args.pubkey {
        // Parse all pubkeys
        let mut parsed_pubkeys = Vec::new();
        for pubkey_str in pubkey_strings {
            match PublicKey::from_str(pubkey_str) {
                Ok(pubkey) => parsed_pubkeys.push(pubkey),
                Err(err) => {
                    println!("Error parsing pubkey {pubkey_str}: {err}");
                    // Continue with other pubkeys
                }
            }
        }

        if parsed_pubkeys.is_empty() {
            println!("No valid pubkeys provided");
            None
        } else {
            // We have pubkeys for P2PK condition
            let num_sigs = sub_command_args.num_sigs.min(parsed_pubkeys.len() as u64);

            // Check if we also have an HTLC condition
            if let Some(hash_str) = &sub_command_args.hash {
                // Create conditions with the pubkeys
                let conditions = Conditions {
                    locktime: None,
                    pubkeys: Some(parsed_pubkeys),
                    refund_keys: None,
                    num_sigs: Some(num_sigs),
                    sig_flag: SigFlag::SigInputs,
                    num_sigs_refund: None,
                };

                // Try to parse the hash
                match Sha256Hash::from_str(hash_str) {
                    Ok(hash) => {
                        // Create HTLC condition with P2PK in the conditions
                        Some(SpendingConditions::HTLCConditions {
                            data: hash,
                            conditions: Some(conditions),
                        })
                    }
                    Err(err) => {
                        println!("Error parsing hash: {err}");
                        // Fallback to just P2PK with multiple pubkeys
                        bail!("Error parsing hash");
                    }
                }
            } else if let Some(preimage) = &sub_command_args.preimage {
                // Create conditions with the pubkeys
                let conditions = Conditions {
                    locktime: None,
                    pubkeys: Some(parsed_pubkeys),
                    refund_keys: None,
                    num_sigs: Some(num_sigs),
                    sig_flag: SigFlag::SigInputs,
                    num_sigs_refund: None,
                };

                // Create HTLC conditions with the hash and pubkeys in conditions
                Some(SpendingConditions::new_htlc(
                    preimage.to_string(),
                    Some(conditions),
                )?)
            } else {
                // Only P2PK condition with multiple pubkeys
                Some(SpendingConditions::new_p2pk(
                    *parsed_pubkeys.first().unwrap(),
                    Some(Conditions {
                        locktime: None,
                        pubkeys: Some(parsed_pubkeys[1..].to_vec()),
                        refund_keys: None,
                        num_sigs: Some(num_sigs),
                        sig_flag: SigFlag::SigInputs,
                        num_sigs_refund: None,
                    }),
                ))
            }
        }
    } else if let Some(hash_str) = &sub_command_args.hash {
        // Only HTLC condition with provided hash
        match Sha256Hash::from_str(hash_str) {
            Ok(hash) => Some(SpendingConditions::HTLCConditions {
                data: hash,
                conditions: None,
            }),
            Err(err) => {
                println!("Error parsing hash: {err}");
                None
            }
        }
    } else if let Some(preimage) = &sub_command_args.preimage {
        // Only HTLC condition with provided preimage
        // For HTLC, create the hash from the preimage and use it directly
        Some(SpendingConditions::new_htlc(preimage.to_string(), None)?)
    } else {
        None
    };

    // Convert SpendingConditions to Nut10SecretRequest
    let nut10 = spending_conditions.map(Nut10SecretRequest::from);

    // Extract the transports option from our match result
    let (transports, nostr_info) = transports;

    let req = PaymentRequest {
        payment_id: None,
        amount: sub_command_args.amount.map(|a| a.into()),
        unit: Some(CurrencyUnit::from_str(&sub_command_args.unit)?),
        single_use: Some(true),
        mints: Some(mints),
        description: sub_command_args.description.clone(),
        transports,
        nut10,
    };

    // Always print the request
    println!("{req}");

    // Only listen for Nostr payment if Nostr transport was selected
    if let Some((keys, relays, pubkey)) = nostr_info {
        println!("Listening for payment via Nostr...");

        let client = NostrClient::new(keys);
        let filter = Filter::new().pubkey(pubkey);

        for relay in relays {
            client.add_read_relay(relay).await?;
        }

        client.connect().await;
        client.subscribe(filter, None).await?;

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
                    let token =
                        Token::new(payload.mint, payload.proofs, payload.memo, payload.unit);

                    let amount = multi_mint_wallet
                        .receive(&token.to_string(), ReceiveOptions::default())
                        .await?;

                    println!("Received {amount}");
                    exit = true;
                }
                Ok(exit) // Set to true to exit from the loop
            })
            .await?;
    }

    Ok(())
}
