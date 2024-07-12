use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use cdk::amount::SplitTarget;
use cdk::cdk_database::{Error, WalletDatabase};
use cdk::nuts::{CurrencyUnit, SecretKey, Token};
use cdk::util::unix_time;
use cdk::wallet::Wallet;
use cdk::{Amount, UncheckedUrl};
use clap::Args;
use nostr_sdk::nips::nip04;
use nostr_sdk::{Filter, Keys, Kind, Timestamp};

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
    #[arg(long)]
    since: Option<u64>,
    /// Preimage
    #[arg(short, long,  action = clap::ArgAction::Append)]
    preimage: Vec<String>,
}

pub async fn receive(
    wallets: HashMap<UncheckedUrl, Wallet>,
    seed: &[u8],
    localstore: Arc<dyn WalletDatabase<Err = Error> + Sync + Send>,
    sub_command_args: &ReceiveSubCommand,
) -> Result<()> {
    let mut signing_keys = Vec::new();

    if !sub_command_args.signing_key.is_empty() {
        let mut s_keys: Vec<SecretKey> = sub_command_args
            .signing_key
            .iter()
            .map(|s| SecretKey::from_str(s).unwrap())
            .collect();
        signing_keys.append(&mut s_keys);
    }

    let amount = match &sub_command_args.token {
        Some(token_str) => {
            receive_token(
                token_str,
                wallets,
                seed,
                &localstore,
                &signing_keys,
                &sub_command_args.preimage,
            )
            .await?
        }
        None => {
            //wallet.add_p2pk_signing_key(nostr_signing_key).await;
            let nostr_key = match sub_command_args.nostr_key.as_ref() {
                Some(nostr_key) => {
                    let secret_key = SecretKey::from_str(nostr_key)?;
                    Some(secret_key)
                }
                None => None,
            };

            let nostr_key =
                nostr_key.ok_or(anyhow!("Nostr key required if token is not provided"))?;

            signing_keys.push(nostr_key.clone());

            let relays = sub_command_args.relay.clone();
            let since = localstore
                .get_nostr_last_checked(&nostr_key.public_key())
                .await?;

            let tokens = nostr_receive(relays, nostr_key.clone(), since).await?;

            let mut total_amount = Amount::ZERO;
            for token_str in &tokens {
                match receive_token(
                    token_str,
                    wallets.clone(),
                    seed,
                    &localstore,
                    &signing_keys,
                    &sub_command_args.preimage,
                )
                .await
                {
                    Ok(amount) => {
                        total_amount += amount;
                    }
                    Err(err) => {
                        println!("{}", err);
                    }
                }
            }

            localstore
                .add_nostr_last_checked(nostr_key.public_key(), unix_time() as u32)
                .await?;
            total_amount
        }
    };

    println!("Received: {}", amount);

    Ok(())
}

async fn receive_token(
    token_str: &str,
    wallets: HashMap<UncheckedUrl, Wallet>,
    seed: &[u8],
    localstore: &Arc<dyn WalletDatabase<Err = Error> + Sync + Send>,
    signing_keys: &[SecretKey],
    preimage: &[String],
) -> Result<Amount> {
    let token = Token::from_str(token_str)?;
    let mint_url = token.proofs().iter().next().unwrap().0.clone();

    let wallet = match wallets.get(&mint_url) {
        Some(wallet) => wallet.clone(),
        None => Wallet::new(
            &mint_url.to_string(),
            CurrencyUnit::Sat,
            Arc::clone(localstore),
            seed,
            None,
        ),
    };

    let amount = wallet
        .receive(token_str, SplitTarget::default(), signing_keys, preimage)
        .await?;
    Ok(amount)
}

/// Receive tokens sent to nostr pubkey via dm
async fn nostr_receive(
    relays: Vec<String>,
    nostr_signing_key: SecretKey,
    since: Option<u32>,
) -> Result<HashSet<String>> {
    let verifying_key = nostr_signing_key.public_key();

    let x_only_pubkey = verifying_key.x_only_public_key();

    let nostr_pubkey = nostr_sdk::PublicKey::from_hex(x_only_pubkey.to_string())?;

    let since = since.map(|s| Timestamp::from(s as u64));

    let filter = match since {
        Some(since) => Filter::new()
            .pubkey(nostr_pubkey)
            .kind(Kind::EncryptedDirectMessage)
            .since(since),
        None => Filter::new()
            .pubkey(nostr_pubkey)
            .kind(Kind::EncryptedDirectMessage),
    };

    let client = nostr_sdk::Client::default();

    client.add_relays(relays).await?;

    client.connect().await;

    let events = client.get_events_of(vec![filter], None).await?;

    let mut tokens: HashSet<String> = HashSet::new();

    let keys = Keys::from_str(&(nostr_signing_key).to_secret_hex())?;

    for event in events {
        if event.kind() == Kind::EncryptedDirectMessage {
            if let Ok(msg) = nip04::decrypt(keys.secret_key()?, event.author_ref(), event.content())
            {
                if let Some(token) = cdk::wallet::util::token_from_text(&msg) {
                    tokens.insert(token.to_string());
                }
            } else {
                tracing::error!("Impossible to decrypt direct message");
            }
        }
    }

    Ok(tokens)
}
