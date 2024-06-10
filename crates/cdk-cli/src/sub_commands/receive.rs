use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::amount::SplitTarget;
use cdk::nuts::SecretKey;
use cdk::wallet::Wallet;
use clap::Args;

#[derive(Args)]
pub struct ReceiveSubCommand {
    /// Cashu Token
    token: Option<String>,
    /// Signing Key
    #[arg(short, long, action = clap::ArgAction::Append)]
    signing_key: Vec<String>,
    /// Nostr key
    #[arg(short, long)]
    nostr_key: Option<String>,
    /// Nostr relay
    #[arg(short, long, action = clap::ArgAction::Append)]
    relay: Vec<String>,
    /// Unix time to to query nostr from
    #[arg(short, long)]
    since: Option<u64>,
    /// Preimage
    #[arg(short, long,  action = clap::ArgAction::Append)]
    preimage: Vec<String>,
}

pub async fn receive(wallet: Wallet, sub_command_args: &ReceiveSubCommand) -> Result<()> {
    let nostr_key = match sub_command_args.nostr_key.as_ref() {
        Some(nostr_key) => {
            let secret_key = SecretKey::from_str(nostr_key)?;
            wallet.add_p2pk_signing_key(secret_key.clone()).await;
            Some(secret_key)
        }
        None => None,
    };

    if !sub_command_args.signing_key.is_empty() {
        let signing_keys: Vec<SecretKey> = sub_command_args
            .signing_key
            .iter()
            .map(|s| SecretKey::from_str(s).unwrap())
            .collect();

        for signing_key in signing_keys {
            wallet.add_p2pk_signing_key(signing_key).await;
        }
    }

    let preimage = match sub_command_args.preimage.is_empty() {
        true => None,
        false => Some(sub_command_args.preimage.clone()),
    };

    let amount = match nostr_key {
        Some(nostr_key) => {
            assert!(!sub_command_args.relay.is_empty());
            wallet
                .add_nostr_relays(sub_command_args.relay.clone())
                .await?;
            wallet
                .nostr_receive(nostr_key, sub_command_args.since, SplitTarget::default())
                .await?
        }
        None => {
            wallet
                .receive(
                    sub_command_args
                        .token
                        .as_ref()
                        .ok_or(anyhow!("Token Required"))?,
                    &SplitTarget::default(),
                    preimage,
                )
                .await?
        }
    };

    println!("Received: {}", amount);

    Ok(())
}
