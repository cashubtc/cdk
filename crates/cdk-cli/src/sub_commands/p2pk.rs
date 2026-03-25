use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::nuts::{CurrencyUnit, PublicKey, SecretKey};
use cdk::wallet::WalletRepository;
use clap::{Args, Subcommand};

use crate::utils::get_or_create_wallet;

#[derive(Args)]
pub struct P2pkSubCommand {
    /// Mint URL (required to obtain a wallet context)
    #[arg(short, long)]
    mint_url: String,
    #[command(subcommand)]
    command: P2pkCommands,
}

#[derive(Subcommand)]
pub enum P2pkCommands {
    /// Generate a new P2PK signing key and store it in the wallet
    Generate,
    /// Store an existing P2PK signing key
    Store {
        /// Secret key in hex format
        secret_key: String,
    },
    /// List all stored P2PK signing keys (shows public keys)
    List,
    /// Remove a stored P2PK signing key by its public key
    Remove {
        /// Public key in hex format
        pubkey: String,
    },
}

pub async fn p2pk(
    wallet_repository: &WalletRepository,
    sub_command_args: &P2pkSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mint_url = cdk::mint_url::MintUrl::from_str(&sub_command_args.mint_url)?;
    let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

    match &sub_command_args.command {
        P2pkCommands::Generate => {
            let pubkey = wallet.generate_p2pk_key().await?;
            println!("Generated P2PK key:");
            println!("  Public key: {}", pubkey.to_hex());
        }
        P2pkCommands::Store { secret_key } => {
            let sk =
                SecretKey::from_hex(secret_key).map_err(|e| anyhow!("Invalid secret key: {e}"))?;
            let pubkey = wallet.store_p2pk_key(sk).await?;
            println!("Stored P2PK key:");
            println!("  Public key: {}", pubkey.to_hex());
        }
        P2pkCommands::List => {
            let keys = wallet.get_p2pk_signing_keys().await?;
            if keys.is_empty() {
                println!("No P2PK signing keys stored.");
            } else {
                println!("Stored P2PK signing keys ({}):", keys.len());
                for xonly in keys.keys() {
                    println!("  {xonly}");
                }
            }
        }
        P2pkCommands::Remove { pubkey } => {
            let pk = PublicKey::from_hex(pubkey).map_err(|e| anyhow!("Invalid public key: {e}"))?;
            wallet.remove_p2pk_key(&pk).await?;
            println!("Removed P2PK key: {pubkey}");
        }
    }

    Ok(())
}
