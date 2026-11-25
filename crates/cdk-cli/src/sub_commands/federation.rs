use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use cdk::mint_url::MintUrl;
use cdk::wallet::{
    FederationConnectorPolicy, FederationImportOptions, FederationRegistry, FederationSource,
    RegisteredFederation, WalletRepository,
};
use cdk_common::federation::{FederationConfig, MemberId};
use clap::{Args, Subcommand};

/// Federation-aware wallet commands.
#[derive(Args)]
pub struct FederationCommand {
    /// Federation command to run.
    #[command(subcommand)]
    pub command: FederationSubCommand,
}

/// Federation-aware wallet subcommands.
#[derive(Subcommand)]
pub enum FederationSubCommand {
    /// Import a public federation config for wallet operations.
    Add(FederationAddSubCommand),
    /// List imported federation wallet configs.
    List,
}

/// Import a public federation config for wallet operations.
#[derive(Args)]
pub struct FederationAddSubCommand {
    /// Path to federation-public.json.
    #[arg(
        long,
        conflicts_with_all = ["invite_code", "mint_url"],
        required_unless_present_any = ["invite_code", "mint_url"]
    )]
    public_config: Option<PathBuf>,
    /// Federation invite code containing a public federation config.
    #[arg(
        long,
        conflicts_with_all = ["public_config", "mint_url"],
        required_unless_present_any = ["public_config", "mint_url"]
    )]
    invite_code: Option<String>,
    /// Public URL of any federation member to discover and import.
    #[arg(
        long,
        conflicts_with_all = ["public_config", "invite_code"],
        required_unless_present_any = ["public_config", "invite_code"]
    )]
    mint_url: Option<MintUrl>,
    /// Local wallet mint URL to map this federation to.
    ///
    /// Defaults to the discovery member for `--mint-url`, or otherwise to the
    /// first member's public mint URL. Re-importing a federation keeps the
    /// local mint URL it was first imported with.
    #[arg(long)]
    local_mint_url: Option<MintUrl>,
    /// Preferred federation member for the first quote-creation attempt.
    #[arg(long)]
    quote_coordinator_member_id: Option<u16>,
    /// Timeout in seconds for sequential first-healthy-member calls.
    #[arg(long, default_value_t = 10)]
    member_fallback_timeout_secs: u64,
    /// Per-member timeout in seconds for fanout requests.
    #[arg(long, default_value_t = 10)]
    member_request_timeout_secs: u64,
    /// Additional retry attempts for retryable member fanout failures.
    #[arg(long, default_value_t = 0)]
    member_request_max_retries: u16,
    /// Per-member timeout in seconds for quote-status polling calls.
    #[arg(long, default_value_t = 10)]
    quote_status_member_timeout_secs: u64,
    /// Maximum number of concurrent member calls.
    #[arg(long, default_value_t = cdk_common::federation::FEDERATION_MAX_DKG_MEMBERS)]
    fanout_concurrency: usize,
    /// Attempts to wait for a newly created mint quote to become threshold-visible.
    #[arg(long, default_value_t = 100)]
    quote_visibility_attempts: u16,
    /// Delay in milliseconds between mint quote threshold-visibility attempts.
    #[arg(long, default_value_t = 50)]
    quote_visibility_retry_delay_ms: u64,
}

impl FederationAddSubCommand {
    fn connector_policy(&self) -> Result<FederationConnectorPolicy> {
        let quote_coordinator_member_id = self
            .quote_coordinator_member_id
            .map(MemberId::new)
            .transpose()?;
        Ok(FederationConnectorPolicy {
            quote_coordinator_member_id,
            member_fallback_timeout_secs: self.member_fallback_timeout_secs,
            member_request_timeout_secs: self.member_request_timeout_secs,
            member_request_max_retries: self.member_request_max_retries,
            quote_status_member_timeout_secs: self.quote_status_member_timeout_secs,
            fanout_concurrency: self.fanout_concurrency,
            quote_visibility_attempts: self.quote_visibility_attempts,
            quote_visibility_retry_delay_ms: self.quote_visibility_retry_delay_ms,
        })
    }

    fn source(&self) -> Result<FederationSource> {
        match (&self.public_config, &self.invite_code, &self.mint_url) {
            (Some(path), None, None) => {
                let contents = fs::read_to_string(path)
                    .with_context(|| format!("could not read {}", path.display()))?;
                let public_config: FederationConfig = serde_json::from_str(&contents)
                    .with_context(|| format!("could not parse {}", path.display()))?;
                Ok(FederationSource::PublicConfig(Box::new(public_config)))
            }
            (None, Some(invite_code), None) => Ok(FederationSource::Invite(invite_code.clone())),
            (None, None, Some(mint_url)) => Ok(FederationSource::MemberUrl(mint_url.clone())),
            _ => bail!("Provide exactly one of --mint-url, --public-config, or --invite-code"),
        }
    }
}

/// Run a federation-aware wallet command.
pub async fn federation(
    registry: &FederationRegistry,
    _wallet_repository: &WalletRepository,
    command: &FederationCommand,
) -> Result<()> {
    match &command.command {
        FederationSubCommand::Add(args) => add(registry, args).await,
        FederationSubCommand::List => list(registry).await,
    }
}

async fn add(registry: &FederationRegistry, args: &FederationAddSubCommand) -> Result<()> {
    let options = FederationImportOptions {
        local_mint_url: args.local_mint_url.clone(),
        connector_policy: Some(args.connector_policy()?),
    };
    let federation = registry.import(args.source()?, options).await?;

    println!("Imported federation {}", federation.federation_id());
    print_federation(&federation)
}

async fn list(registry: &FederationRegistry) -> Result<()> {
    let federations = registry.list().await?;

    if federations.is_empty() {
        println!("No federations imported");
        return Ok(());
    }

    for federation in federations {
        println!("Federation {}", federation.federation_id());
        print_federation(&federation)?;
    }

    Ok(())
}

fn print_federation(federation: &RegisteredFederation) -> Result<()> {
    let threshold_params = federation.public_config().threshold_params;
    println!("  Local mint URL: {}", federation.local_mint_url());
    println!(
        "  Members: {} (signature threshold {}, consensus threshold {})",
        threshold_params.member_count,
        threshold_params.signature_threshold,
        threshold_params.consensus_threshold
    );
    if let Some(quote_coordinator_member_id) =
        federation.connector_policy().quote_coordinator_member_id
    {
        println!(
            "  Preferred quote coordinator: {}",
            quote_coordinator_member_id
        );
    }
    println!(
        "  Active units: {}",
        federation
            .active_units()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  Config digest: {}", federation.config_digest()?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk::cdk_database::{self, WalletDatabase};
    use cdk_common::federation::FederationInvite;

    use super::*;
    use crate::test_support::federation_public_config;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cdk-cli-federation-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    fn add_args(
        public_config: Option<PathBuf>,
        invite_code: Option<String>,
    ) -> FederationAddSubCommand {
        FederationAddSubCommand {
            public_config,
            invite_code,
            mint_url: None,
            local_mint_url: None,
            quote_coordinator_member_id: Some(2),
            member_fallback_timeout_secs: 10,
            member_request_timeout_secs: 30,
            member_request_max_retries: 0,
            quote_status_member_timeout_secs: 10,
            fanout_concurrency: usize::MAX,
            quote_visibility_attempts: 100,
            quote_visibility_retry_delay_ms: 50,
        }
    }

    async fn registry() -> FederationRegistry {
        let localstore: Arc<dyn WalletDatabase<cdk_database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("wallet memory db"),
        );
        FederationRegistry::new(localstore)
    }

    #[tokio::test]
    async fn add_imports_a_public_config_file_with_the_command_policy() {
        let public_config = federation_public_config();
        let path = temp_path("federation-public.json");
        fs::write(
            &path,
            serde_json::to_vec(&public_config).expect("config json"),
        )
        .expect("write config");
        let registry = registry().await;

        add(&registry, &add_args(Some(path.clone()), None))
            .await
            .expect("add");

        let federations = registry.list().await.expect("list");
        assert_eq!(federations.len(), 1);
        assert_eq!(federations[0].public_config(), &public_config);
        let policy = federations[0].connector_policy();
        assert_eq!(policy.member_request_timeout_secs, 30);
        assert_eq!(
            policy.quote_coordinator_member_id,
            Some(MemberId::new(2).expect("member id"))
        );
        fs::remove_file(path).expect("remove config");
    }

    #[tokio::test]
    async fn add_imports_an_invite_code() {
        let public_config = federation_public_config();
        let invite_code = FederationInvite::new(public_config.clone())
            .expect("invite")
            .to_invite_code()
            .expect("invite code");
        let registry = registry().await;

        add(&registry, &add_args(None, Some(invite_code)))
            .await
            .expect("add");

        assert_eq!(
            registry.list().await.expect("list")[0].public_config(),
            &public_config
        );
    }

    #[tokio::test]
    async fn add_rejects_bad_sources() {
        let registry = registry().await;
        add(
            &registry,
            &add_args(None, Some("cashuAunsupported".to_string())),
        )
        .await
        .expect_err("bad invite rejected");

        let path = temp_path("malformed.json");
        fs::write(&path, "{not-json").expect("write malformed config");
        add(&registry, &add_args(Some(path.clone()), None))
            .await
            .expect_err("malformed config rejected");
        fs::remove_file(path).expect("remove config");

        let mut args = add_args(None, None);
        args.quote_coordinator_member_id = Some(0);
        args.source().expect_err("no source");
        args.connector_policy()
            .expect_err("member id zero rejected");

        assert!(registry.list().await.expect("list").is_empty());
    }
}
