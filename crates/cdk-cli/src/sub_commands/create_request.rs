use std::sync::Arc;

use anyhow::{anyhow, Result};
use cdk::cdk_database::{self, WalletDatabase};
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, PublicKey, SupportedMethod};
use cdk::wallet::payment_request::{
    CreatePaymentRequest, PaymentRequestLock, PaymentRequestMintPolicy,
    PaymentRequestReceiverState, PaymentRequestTransport,
};
use cdk::wallet::WalletManager;
use clap::Args;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(super) struct StoredNostrWaitInfo {
    pub(super) secret_key_hex: String,
    pub(super) relays: Vec<String>,
    pub(super) pubkey_hex: String,
    #[serde(default)]
    pub(super) mints: Vec<MintUrl>,
    #[serde(default)]
    pub(super) mint_preferred: Option<bool>,
}

impl StoredNostrWaitInfo {
    #[cfg(test)]
    pub(super) fn accepts_mint(&self, mint_url: &MintUrl) -> bool {
        self.mints.is_empty() || self.mint_preferred == Some(true) || self.mints.contains(mint_url)
    }

    pub(super) fn into_receiver_state(self) -> PaymentRequestReceiverState {
        PaymentRequestReceiverState {
            secret_key_hex: self.secret_key_hex,
            relays: self.relays,
            public_key_hex: self.pubkey_hex,
            mints: self.mints,
            mint_preferred: self.mint_preferred,
        }
    }
}

impl From<PaymentRequestReceiverState> for StoredNostrWaitInfo {
    fn from(info: PaymentRequestReceiverState) -> Self {
        Self {
            secret_key_hex: info.secret_key_hex,
            relays: info.relays,
            pubkey_hex: info.public_key_hex,
            mints: info.mints,
            mint_preferred: info.mint_preferred,
        }
    }
}

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
    /// Mint URLs the receiver trusts. Can be specified multiple times.
    #[arg(long, action = clap::ArgAction::Append)]
    mints: Option<Vec<String>>,
    /// Prefer the listed mints while allowing payment from other mints
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
    wallet_manager: &WalletManager,
    localstore: &Arc<dyn WalletDatabase<cdk_database::Error> + Send + Sync>,
    sub_command_args: &CreateRequestSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let public_keys = sub_command_args
        .pubkey
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|key| key.parse::<PublicKey>().map_err(|error| anyhow!(error)))
        .collect::<Result<Vec<_>>>()?;
    let lock = match (&sub_command_args.hash, &sub_command_args.preimage) {
        (Some(hash), None) => Some(PaymentRequestLock::HtlcHash {
            hash: hash.clone(),
            public_keys,
            signatures_required: sub_command_args.num_sigs,
        }),
        (None, Some(preimage)) => Some(PaymentRequestLock::HtlcPreimage {
            preimage: preimage.clone(),
            public_keys,
            signatures_required: sub_command_args.num_sigs,
        }),
        (None, None) if !public_keys.is_empty() => Some(PaymentRequestLock::P2pk {
            public_keys,
            signatures_required: sub_command_args.num_sigs,
        }),
        (None, None) => None,
        (Some(_), Some(_)) => return Err(anyhow!("hash and preimage are mutually exclusive")),
    };
    let transport = match sub_command_args.transport.to_ascii_lowercase().as_str() {
        "nostr" => PaymentRequestTransport::Nostr(
            sub_command_args
                .nostr_relay
                .clone()
                .ok_or_else(|| anyhow!("Nostr transport requires at least one relay"))?,
        ),
        "http" => PaymentRequestTransport::Http(
            sub_command_args
                .http_url
                .as_deref()
                .ok_or_else(|| anyhow!("HTTP transport requires --http-url"))?
                .parse()?,
        ),
        "none" => PaymentRequestTransport::OutOfBand,
        transport => {
            return Err(anyhow!(
                "unsupported payment request transport `{transport}`"
            ))
        }
    };
    let mints = sub_command_args
        .mints
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|mint| mint.parse::<MintUrl>().map_err(|error| anyhow!(error)))
        .collect::<Result<Vec<_>>>()?;
    let mint_policy = if mints.is_empty() {
        PaymentRequestMintPolicy::Any
    } else if sub_command_args.mint_preferred {
        PaymentRequestMintPolicy::Preferred(mints)
    } else {
        PaymentRequestMintPolicy::Strict(mints)
    };
    let request = CreatePaymentRequest {
        amount: sub_command_args.amount.map(Into::into),
        unit: unit.clone(),
        description: sub_command_args.description.clone(),
        lock,
        transport,
        mint_policy,
        supported_methods: sub_command_args.supported_methods.clone(),
    };

    let created = wallet_manager.create_payment_request(request).await?;
    let req = created.payment_request;

    // Print the request to stdout
    if sub_command_args.bech32 {
        println!("{}", req.to_bech32_string()?);
    } else {
        println!("{}", req);
    }

    // If we set up Nostr transport, optionally wait for payment and receive it
    if let Some(receiver) = created.receiver {
        let state = receiver.state();
        let key = state.public_key_hex.clone();

        let serializable_info = StoredNostrWaitInfo::from(state);
        let val = serde_json::to_vec(&serializable_info)?;
        localstore
            .kv_write("cdk_cli", "pending_nostr_requests", &key, &val)
            .await?;

        println!("Listening for payment via Nostr...");
        let amount = receiver.receive().await?;
        localstore
            .kv_remove("cdk_cli", "pending_nostr_requests", &key)
            .await?;
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
    use std::str::FromStr;

    use super::*;

    #[test]
    fn stored_nostr_wait_info_enforces_strict_mints() {
        let listed_mint = MintUrl::from_str("https://listed.example.com").expect("valid mint");
        let unlisted_mint = MintUrl::from_str("https://unlisted.example.com").expect("valid mint");
        let info = stored_info(vec![listed_mint.clone()], None);

        assert!(info.accepts_mint(&listed_mint));
        assert!(!info.accepts_mint(&unlisted_mint));
    }

    #[test]
    fn stored_nostr_wait_info_allows_preferred_or_empty_mints() {
        let listed_mint = MintUrl::from_str("https://listed.example.com").expect("valid mint");
        let unlisted_mint = MintUrl::from_str("https://unlisted.example.com").expect("valid mint");

        assert!(stored_info(vec![listed_mint], Some(true)).accepts_mint(&unlisted_mint));
        assert!(stored_info(vec![], None).accepts_mint(&unlisted_mint));
    }

    #[test]
    fn old_stored_nostr_wait_info_deserializes_with_empty_policy() {
        let json = r#"{
            "secret_key_hex":"secret",
            "relays":["wss://relay.example.com"],
            "pubkey_hex":"pubkey"
        }"#;

        let info: StoredNostrWaitInfo = serde_json::from_str(json).expect("old record");

        assert!(info.mints.is_empty());
        assert!(info.mint_preferred.is_none());
    }

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

    fn stored_info(mints: Vec<MintUrl>, mint_preferred: Option<bool>) -> StoredNostrWaitInfo {
        StoredNostrWaitInfo {
            secret_key_hex: "secret".to_string(),
            relays: vec![],
            pubkey_hex: "pubkey".to_string(),
            mints,
            mint_preferred,
        }
    }
}
