use anyhow::Result;
use cdk::nuts::{CurrencyUnit, SupportedMethod};
use cdk::wallet::{payment_request as pr, WalletRepository};
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
    /// Mint URLs the receiver trusts. Defaults to configured mints for the unit.
    /// Can be specified multiple times.
    #[arg(long, action = clap::ArgAction::Append)]
    mints: Option<Vec<String>>,
    /// Prefer the listed mints while allowing payment from other configured mints
    #[arg(long)]
    mint_preferred: bool,
    /// Accepted payment method and optional fee as METHOD or METHOD:FEE; repeatable
    #[arg(
        long = "supported-method",
        action = clap::ArgAction::Append,
        value_parser = parse_supported_method
    )]
    supported_methods: Vec<SupportedMethod>,
    /// Use bech32 encoding (CREQ-B)
    #[arg(short, long)]
    bech32: bool,
}

pub async fn create_request(
    wallet_repository: &WalletRepository,
    sub_command_args: &CreateRequestSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    // Gather parameters for library call
    let params = pr::CreateRequestParams {
        amount: sub_command_args.amount,
        unit: unit.to_string(),
        description: sub_command_args.description.clone(),
        pubkeys: sub_command_args.pubkey.clone(),
        num_sigs: sub_command_args.num_sigs,
        hash: sub_command_args.hash.clone(),
        preimage: sub_command_args.preimage.clone(),
        transport: sub_command_args.transport.to_lowercase(),
        http_url: sub_command_args.http_url.clone(),
        nostr_relays: sub_command_args.nostr_relay.clone(),
        mints: sub_command_args.mints.clone(),
        mint_preferred: sub_command_args.mint_preferred.then_some(true),
        supported_methods: sub_command_args.supported_methods.clone(),
    };

    let (req, nostr_wait) = wallet_repository.create_request(params).await?;

    // Print the request to stdout
    if sub_command_args.bech32 {
        println!("{}", req.to_bech32_string()?);
    } else {
        println!("{}", req);
    }

    // If we set up Nostr transport, optionally wait for payment and receive it
    if let Some(info) = nostr_wait {
        println!("Listening for payment via Nostr...");
        let amount = wallet_repository.wait_for_nostr_payment(info).await?;
        println!("Received {}", amount);
    }

    Ok(())
}

fn parse_supported_method(value: &str) -> Result<SupportedMethod, String> {
    let (method, fee) = match value.rsplit_once(':') {
        Some((method, fee)) => {
            let fee = fee
                .parse::<u64>()
                .map_err(|err| format!("invalid method fee `{fee}`: {err}"))?;
            (method, Some(fee))
        }
        None => (value, None),
    };

    if method.is_empty() {
        return Err("payment method cannot be empty".to_string());
    }

    Ok(match fee {
        Some(fee) => SupportedMethod::with_fee(method, fee),
        None => SupportedMethod::new(method),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_method_cli_value_accepts_optional_fee() {
        assert_eq!(
            parse_supported_method("bolt11").expect("method"),
            SupportedMethod::new("bolt11")
        );
        assert_eq!(
            parse_supported_method("onchain:50").expect("method with fee"),
            SupportedMethod::with_fee("onchain", 50)
        );
        assert!(parse_supported_method("bolt12:not-a-fee").is_err());
    }
}
