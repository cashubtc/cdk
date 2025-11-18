use anyhow::Result;
use cdk::wallet::{payment_request as pr, MultiMintWallet};
use clap::Args;

#[derive(Args)]
pub struct CreateRequestSubCommand {
    #[arg(short, long)]
    amount: Option<u64>,
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
    // Gather parameters for library call
    let params = pr::CreateRequestParams {
        amount: sub_command_args.amount,
        unit: multi_mint_wallet.unit().to_string(),
        description: sub_command_args.description.clone(),
        pubkeys: sub_command_args.pubkey.clone(),
        num_sigs: sub_command_args.num_sigs,
        hash: sub_command_args.hash.clone(),
        preimage: sub_command_args.preimage.clone(),
        transport: sub_command_args.transport.to_lowercase(),
        http_url: sub_command_args.http_url.clone(),
        nostr_relays: sub_command_args.nostr_relay.clone(),
    };

    let (req, nostr_wait) = multi_mint_wallet.create_request(params).await?;

    // Print the request to stdout
    println!("{}", req);

    // If we set up Nostr transport, optionally wait for payment and receive it
    if let Some(info) = nostr_wait {
        println!("Listening for payment via Nostr...");
        let amount = multi_mint_wallet.wait_for_nostr_payment(info).await?;
        println!("Received {}", amount);
    }

    Ok(())
}
