use std::str::FromStr;

use anyhow::Error;
use bitcoin::secp256k1::schnorr::Signature;
use cdk::wallet::WalletRepository;
use cdk_common::{
    mint_url::MintUrl, nutxx::MintQuoteByPubkeyRequest, CurrencyUnit, PaymentMethod, PublicKey,
};
use clap::Args;

use crate::utils::get_or_create_wallet;

#[derive(Args)]
pub struct ListMintQuotesSubCommand {
    /// Payment Method
    #[arg(long)]
    payment_method: PaymentMethod,

    /// Mint URL
    #[arg(long)]
    mint_url: String,

    /// Pubkeys
    #[arg(long)]
    pubkeys: Option<Vec<String>>,

    /// Signatures
    #[arg(long)]
    signatures: Option<Vec<String>>,
}

pub async fn quotes(
    wallet_repository: &WalletRepository,
    sub_command_args: &ListMintQuotesSubCommand,
    unit: &CurrencyUnit,
) -> Result<(), Error> {
    return get_quotes(wallet_repository, sub_command_args, unit).await;
}

pub async fn get_quotes(
    wallet_repository: &WalletRepository,
    sub_command_args: &ListMintQuotesSubCommand,
    unit: &CurrencyUnit,
) -> Result<(), Error> {
    let mint_url = MintUrl::from_str(&sub_command_args.mint_url)?;
    let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

    let pubkeys: Vec<PublicKey> = match &sub_command_args.pubkeys {
        Some(pubkeys) => pubkeys
            .iter()
            .map(|pk| {
                PublicKey::from_hex(pk)
                    .unwrap_or_else(|_| panic!("Unable to parse the pubkey {}.", pk))
            })
            .collect(),
        None => wallet
            .get_public_keys()
            .await?
            .iter()
            .map(|pk| pk.pubkey)
            .collect(),
    };

    if pubkeys.is_empty() {
        return Err(anyhow::anyhow!("No pubkey has found. Use --pubkeys.",));
    }

    let signatures: Vec<Signature> =
        match &sub_command_args.signatures {
            Some(signatures) => signatures
                .iter()
                .map(|sign| {
                    Signature::from_str(sign.as_str())
                        .unwrap_or_else(|_| panic!("Unable to parse signature {}.", sign))
                })
                .collect(),
            None => {
                let mut signatures = Vec::new();
                for pk in &pubkeys {
                    signatures.push(wallet.sign_msg(pk, &pk.to_bytes()).await?.unwrap_or_else(
                        || panic!("No signature valid for pubkey {}. Use --signatures", pk),
                    ));
                }
                signatures
            }
        };

    let client = wallet.mint_connector();

    let request = MintQuoteByPubkeyRequest {
        pubkeys: pubkeys.iter().map(|pk| pk.to_hex()).collect(),
        pubkeys_signatures: signatures.iter().map(|sig| sig.to_string()).collect(),
    };

    let mint_quotes = client
        .post_mint_quote_by_pubkey(sub_command_args.payment_method.clone(), request)
        .await?;

    println!("{}", serde_json::to_string(&mint_quotes)?);

    Ok(())
}
