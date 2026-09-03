use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, SupportedMethod};
use cdk::wallet::{payment_request as pr, NostrWaitInfo, WalletRepository};
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
    /// Time at which this request started accepting payments.
    ///
    /// Old records omit this field and are queried from the Unix epoch so a
    /// still-pending payment is not silently missed.
    #[serde(default)]
    pub(super) created_at: Option<u64>,
    /// Inclusive upper timestamp for the next backward history page.
    #[serde(default)]
    pub(super) history_until: Option<u64>,
    /// Event IDs already processed at the inclusive history boundary.
    #[serde(default)]
    pub(super) history_boundary_event_ids: Vec<String>,
}

impl StoredNostrWaitInfo {
    pub(super) fn accepts_mint(&self, mint_url: &MintUrl) -> bool {
        self.mints.is_empty() || self.mint_preferred == Some(true) || self.mints.contains(mint_url)
    }

    pub(super) fn nostr_query_since(&self, now: u64) -> nostr_sdk::Timestamp {
        // NIP-59 gift wraps deliberately backdate their timestamps by up to
        // two days. Include that overlap so an immediate payment is visible.
        const GIFT_WRAP_BACKDATE_SECS: u64 = 2 * 24 * 60 * 60;

        nostr_sdk::Timestamp::from(
            self.created_at
                .unwrap_or_default()
                .min(now)
                .saturating_sub(GIFT_WRAP_BACKDATE_SECS),
        )
    }
}

impl From<NostrWaitInfo> for StoredNostrWaitInfo {
    fn from(info: NostrWaitInfo) -> Self {
        Self {
            secret_key_hex: info.keys.secret_key().to_secret_hex(),
            relays: info.relays,
            pubkey_hex: info.pubkey.to_hex(),
            mints: info.mints,
            mint_preferred: info.mint_preferred,
            created_at: Some(nostr_sdk::Timestamp::now().as_secs()),
            history_until: None,
            history_boundary_event_ids: Vec::new(),
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
        let key = info.pubkey.to_string();

        if let Some(wallet) = wallet_repository.get_wallets().await.first() {
            let serializable_info = StoredNostrWaitInfo::from(info.clone());
            let val = serde_json::to_vec(&serializable_info)?;
            wallet
                .localstore
                .kv_write("cdk_cli", "pending_nostr_requests", &key, &val)
                .await?;
        }

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
        assert!(info.created_at.is_none());
        assert!(info.history_until.is_none());
        assert_eq!(info.nostr_query_since(123), nostr_sdk::Timestamp::from(0));
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
            created_at: Some(100),
            history_until: None,
            history_boundary_event_ids: Vec::new(),
        }
    }

    #[test]
    fn stored_request_queries_from_creation_without_seven_day_cutoff() {
        let mut info = stored_info(vec![], None);
        info.created_at = Some(1_000_000);

        assert_eq!(
            info.nostr_query_since(2_000_000),
            nostr_sdk::Timestamp::from(827_200)
        );
        assert_eq!(
            info.nostr_query_since(900_000),
            nostr_sdk::Timestamp::from(727_200)
        );
    }
}
