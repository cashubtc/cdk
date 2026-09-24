use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{anyhow, Result};
use cdk::nuts::{CurrencyUnit, SecretKey, Token};
use cdk::util::unix_time;
use cdk::wallet::{ReceiveOptions, WalletRepository};
use cdk::Amount;
use clap::Args;
use nostr::prelude::{
    nip04, Filter, Keys, Kind, PublicKey, SecretKey as NostrSecretKey, Timestamp,
};
use nostr_sdk::prelude::{Client, RelayCapabilities, SignerAuthenticator};

use crate::nostr_storage;
use crate::terminal::escape_control;
use crate::utils::get_or_create_wallet;

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
    /// Unix time to query nostr from
    #[arg(long)]
    since: Option<u64>,
    /// Preimage
    #[arg(short, long,  action = clap::ArgAction::Append)]
    preimage: Vec<String>,
    /// Allow receiving from untrusted mints (mints not already in the wallet)
    #[arg(long, default_value = "false")]
    allow_untrusted: bool,
}

pub async fn receive(
    wallet_repository: &WalletRepository,
    sub_command_args: &ReceiveSubCommand,
    work_dir: &Path,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mut signing_keys = Vec::new();

    if !sub_command_args.signing_key.is_empty() {
        let mut s_keys: Vec<SecretKey> = sub_command_args
            .signing_key
            .iter()
            .map(|s| {
                if s.starts_with("nsec") {
                    let nostr_key = NostrSecretKey::from_str(s).expect("Invalid secret key");

                    SecretKey::from_str(&nostr_key.to_secret_hex())
                } else {
                    SecretKey::from_str(s)
                }
            })
            .collect::<Result<Vec<SecretKey>, _>>()?;
        signing_keys.append(&mut s_keys);
    }

    let amount = match &sub_command_args.token {
        Some(token_str) => {
            receive_token(
                wallet_repository,
                token_str,
                &signing_keys,
                &sub_command_args.preimage,
                sub_command_args.allow_untrusted,
                unit,
            )
            .await?
        }
        None => {
            //wallet.add_p2pk_signing_key(nostr_signing_key).await;
            let nostr_key = match sub_command_args.nostr_key.as_ref() {
                Some(nostr_key) => {
                    let secret_key = NostrSecretKey::from_str(nostr_key)?;
                    let secret_key = SecretKey::from_str(&secret_key.to_secret_hex())?;
                    Some(secret_key)
                }
                None => None,
            };

            let nostr_key =
                nostr_key.ok_or(anyhow!("Nostr key required if token is not provided"))?;

            signing_keys.push(nostr_key.clone());

            let relays = sub_command_args.relay.clone();
            let since =
                nostr_storage::get_nostr_last_checked(work_dir, &nostr_key.public_key()).await?;

            let tokens = nostr_receive(relays, nostr_key.clone(), since).await?;

            // Store the current time as last checked
            nostr_storage::store_nostr_last_checked(
                work_dir,
                &nostr_key.public_key(),
                unix_time() as u32,
            )
            .await?;

            let mut total_amount = Amount::ZERO;
            for token_str in &tokens {
                match receive_token(
                    wallet_repository,
                    token_str,
                    &signing_keys,
                    &sub_command_args.preimage,
                    sub_command_args.allow_untrusted,
                    unit,
                )
                .await
                {
                    Ok(amount) => {
                        total_amount += amount;
                    }
                    Err(err) => {
                        // Errors may embed mint-controlled data such as error
                        // bodies or token mint URLs; escape before printing.
                        println!("{}", escape_control(&format!("{err:#}")));
                    }
                }
            }

            total_amount
        }
    };

    println!("Received: {amount}");

    Ok(())
}

async fn receive_token(
    wallet_repository: &WalletRepository,
    token_str: &str,
    signing_keys: &[SecretKey],
    preimage: &[String],
    allow_untrusted: bool,
    unit: &CurrencyUnit,
) -> Result<Amount> {
    let token: Token = Token::from_str(token_str)?;

    let mint_url = token.mint_url()?;

    // Check if the mint is already trusted
    #[cfg(feature = "federation")]
    let is_trusted = wallet_repository.has_mint(&mint_url).await
        || crate::utils::is_configured_federation_mint_url(&mint_url)?;
    #[cfg(not(feature = "federation"))]
    let is_trusted = wallet_repository.has_mint(&mint_url).await;

    // If mint is not trusted and we don't allow untrusted, error out
    if !is_trusted && !allow_untrusted {
        #[cfg(feature = "federation")]
        return Err(anyhow!(
            "Mint {} is not trusted or imported as a federation member. Run `cdk-cli federation add --mint-url {}` for federated tokens, or use --allow-untrusted to receive from untrusted standalone mints.",
            escape_control(&mint_url.to_string()),
            escape_control(&mint_url.to_string())
        ));

        #[cfg(not(feature = "federation"))]
        // The mint URL comes from the received token and may be
        // attacker-controlled; escape it before it reaches the terminal.
        return Err(anyhow!(
            "Mint {} is not trusted. Use --allow-untrusted to receive from untrusted mints.",
            escape_control(&mint_url.to_string())
        ));
    }

    // Get or create wallet for the token's mint
    let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

    // Create receive options
    let receive_options = ReceiveOptions {
        p2pk_signing_keys: signing_keys.to_vec(),
        preimages: preimage.to_vec(),
        ..Default::default()
    };

    let received = wallet.receive(token_str, receive_options).await?;
    Ok(received)
}

/// Receive tokens sent to nostr pubkey via dm
async fn nostr_receive(
    relays: Vec<String>,
    nostr_signing_key: SecretKey,
    since: Option<u32>,
) -> Result<HashSet<String>> {
    let verifying_key = nostr_signing_key.public_key();

    let x_only_pubkey = verifying_key.try_x_only_public_key()?;

    let nostr_pubkey = PublicKey::from_hex(&x_only_pubkey.to_string())?;

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

    let keys = Keys::from_str(&(nostr_signing_key).to_secret_hex())?;
    let client = Client::builder()
        .authenticator(SignerAuthenticator::new(keys.clone()))
        .build();

    for relay in &relays {
        client
            .add_relay(relay)
            .capabilities(RelayCapabilities::READ)
            .await?;
    }

    client.connect().await;

    let events = client
        .fetch_events(filter)
        .timeout(Duration::from_secs(30))
        .await?;

    let mut tokens: HashSet<String> = HashSet::new();

    for event in events {
        if event.kind == Kind::EncryptedDirectMessage {
            if let Ok(msg) = nip04::decrypt(keys.secret_key(), &event.pubkey, event.content) {
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

#[cfg(test)]
mod tests {
    #[cfg(feature = "federation")]
    use std::collections::BTreeMap;
    use std::str::FromStr;
    use std::sync::Arc;

    use cdk::cdk_database::{self, WalletDatabase};
    use cdk::mint_url::MintUrl;
    #[cfg(feature = "federation")]
    use cdk::nuts::nut01::BlsSecretKey;
    use cdk::nuts::{Id, Proof};
    use cdk::secret::Secret;
    use cdk::wallet::{WalletRepository, WalletRepositoryBuilder};
    use cdk::Amount;
    #[cfg(feature = "federation")]
    use cdk_common::federation::{
        generate_trusted_dealer_configs, FederationConfig, MemberId, ThresholdParams,
        TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    #[cfg(feature = "federation")]
    use cdk_common::SecretKey;

    use super::*;
    #[cfg(feature = "federation")]
    use crate::sub_commands::federation::{
        FederationWalletRegistry, StoredFederationConnectorPolicy,
    };

    async fn test_repository() -> WalletRepository {
        let localstore: Arc<dyn WalletDatabase<cdk_database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("wallet memory db"),
        );
        WalletRepositoryBuilder::new()
            .localstore(localstore)
            .seed([0u8; 64])
            .build()
            .await
            .expect("wallet repository")
    }

    #[cfg(feature = "federation")]
    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("valid member id")
    }

    #[cfg(feature = "federation")]
    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("valid secp256k1 secret")
    }

    #[cfg(feature = "federation")]
    fn member(member_id: MemberId) -> TrustedDealerMember {
        TrustedDealerMember::new(
            member_id,
            MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
                .expect("public mint url"),
            MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get()))
                .expect("federation api url"),
            identity_secret(member_id),
        )
    }

    #[cfg(feature = "federation")]
    fn bls_key(byte: u8) -> BlsSecretKey {
        BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    #[cfg(feature = "federation")]
    fn valid_public_config() -> FederationConfig {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([
                    (Amount::from(1), vec![bls_key(1), bls_key(2)]),
                    (Amount::from(2), vec![bls_key(3), bls_key(4)]),
                ]),
            )],
        )
        .expect("trusted dealer configs")
        .public_config
    }

    fn token_for_mint(mint_url: MintUrl) -> String {
        let proof = Proof::new(
            Amount::from(1),
            Id::from_str("009a1f293253e41e").expect("keyset id"),
            Secret::generate(),
            "02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"
                .parse()
                .expect("proof signature"),
        );
        Token::new(mint_url, vec![proof], None, CurrencyUnit::Sat).to_string()
    }

    #[tokio::test]
    async fn receive_token_rejects_untrusted_mint_with_actionable_error() {
        let repository = test_repository().await;
        let token = token_for_mint(
            MintUrl::from_str("https://unimported-fed.example.com").expect("mint url"),
        );

        let err = receive_token(&repository, &token, &[], &[], false, &CurrencyUnit::Sat)
            .await
            .expect_err("untrusted federation token rejected");

        let err = err.to_string();
        #[cfg(feature = "federation")]
        assert!(err.contains("not trusted or imported as a federation member"));
        #[cfg(not(feature = "federation"))]
        assert!(err.contains("is not trusted"));
        #[cfg(feature = "federation")]
        assert!(err.contains("cdk-cli federation add --mint-url"));
        assert!(err.contains("--allow-untrusted"));
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn receive_token_resolves_imported_member_url_as_federation_metadata() {
        let repository = test_repository().await;
        let public_config = valid_public_config();
        let local_mint_url = public_config
            .members
            .get(&member_id(1))
            .expect("member one")
            .public_mint_url
            .clone();
        let member_two_url = public_config
            .members
            .get(&member_id(2))
            .expect("member two")
            .public_mint_url
            .clone();
        let mut registry = FederationWalletRegistry::default();
        registry
            .upsert(
                public_config,
                local_mint_url,
                StoredFederationConnectorPolicy::default(),
            )
            .expect("upsert federation registry");
        crate::utils::set_federation_registry(registry).expect("set federation registry");

        let token = token_for_mint(member_two_url);
        let err = receive_token(&repository, &token, &[], &[], false, &CurrencyUnit::Usd)
            .await
            .expect_err("unsupported federation unit rejected after member-url resolution");

        let err = err.to_string();
        assert!(!err.contains("not trusted or imported as a federation member"));
        assert!(err.contains("does not advertise an active"));
        assert!(err.contains("keyset"));
    }
}
