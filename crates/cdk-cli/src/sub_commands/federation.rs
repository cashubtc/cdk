use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, MintInfo};
use cdk::wallet::{
    FederatedMintConnector, FederatedMintConnectorConfig, MintConnector, WalletConfig,
    WalletRepository,
};
use cdk_common::federation::{FederationConfig, FederationId, FederationInvite, MemberId};
use cdk_http_client::HttpClient;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::utils::set_federation_registry;

const FEDERATION_REGISTRY_VERSION: u16 = 1;
const FEDERATION_REGISTRY_FILE: &str = "federations.json";

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
    /// first member's public mint URL.
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
    fn connector_policy(&self) -> Result<StoredFederationConnectorPolicy> {
        let quote_coordinator_member_id = self
            .quote_coordinator_member_id
            .map(MemberId::new)
            .transpose()?;
        Ok(StoredFederationConnectorPolicy {
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
}

/// Persistent public federation wallet registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWalletRegistry {
    /// Sidecar schema version.
    pub version: u16,
    /// Imported federation wallet configs.
    pub wallets: Vec<StoredFederationWallet>,
}

impl Default for FederationWalletRegistry {
    fn default() -> Self {
        Self {
            version: FEDERATION_REGISTRY_VERSION,
            wallets: Vec::new(),
        }
    }
}

impl FederationWalletRegistry {
    /// Load the public federation wallet registry from a CLI work directory.
    pub fn load(work_dir: &Path) -> Result<Self> {
        let path = registry_path(work_dir);
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)?;
        let registry: Self = serde_json::from_str(&contents)?;
        registry.validate()?;
        Ok(registry)
    }

    /// Persist the public federation wallet registry in a CLI work directory.
    pub fn save(&self, work_dir: &Path) -> Result<()> {
        self.validate()?;
        let contents = serde_json::to_string_pretty(self)?;
        fs::write(registry_path(work_dir), contents)?;
        Ok(())
    }

    /// Validate the registry and all imported federation configs.
    pub fn validate(&self) -> Result<()> {
        if self.version != FEDERATION_REGISTRY_VERSION {
            bail!("Unsupported federation registry version {}", self.version);
        }

        for wallet in &self.wallets {
            wallet.validate()?;
        }

        Ok(())
    }

    /// Add or replace an imported public federation config.
    pub fn upsert(
        &mut self,
        public_config: FederationConfig,
        local_mint_url: MintUrl,
        connector_policy: StoredFederationConnectorPolicy,
    ) -> Result<StoredFederationWallet> {
        let wallet = StoredFederationWallet {
            local_mint_url,
            public_config,
            connector_policy,
        };
        wallet.validate()?;

        if let Some(existing) = self
            .wallets
            .iter_mut()
            .find(|existing| existing.local_mint_url == wallet.local_mint_url)
        {
            *existing = wallet.clone();
        } else {
            self.wallets.push(wallet.clone());
        }

        Ok(wallet)
    }

    /// Find an imported federation wallet by its local mint URL mapping.
    pub fn find_by_mint_url(&self, mint_url: &MintUrl) -> Option<&StoredFederationWallet> {
        self.wallets
            .iter()
            .find(|wallet| wallet.matches_mint_url(mint_url))
    }
}

/// Public metadata needed by the CLI to reopen a federation wallet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredFederationWallet {
    /// Mint URL used by existing CLI wallet and token flows.
    pub local_mint_url: MintUrl,
    /// Public federation config; never contains member private configs or secret shares.
    pub public_config: FederationConfig,
    /// Wallet-side member request policy.
    pub connector_policy: StoredFederationConnectorPolicy,
}

impl StoredFederationWallet {
    /// Validate this imported federation wallet config.
    pub fn validate(&self) -> Result<()> {
        self.public_config.validate_wallet_runtime_metadata()?;
        self.public_config
            .threshold_params
            .validate_bft_safety()
            .context("wallet refuses an unsafe federation threshold profile")?;
        self.connector_policy.connector_config().validate()?;
        if let Some(quote_coordinator_member_id) = self.connector_policy.quote_coordinator_member_id
        {
            if !self
                .public_config
                .members
                .contains_key(&quote_coordinator_member_id)
            {
                bail!(
                    "Federation {} does not contain quote coordinator member {}",
                    self.public_config.federation_id,
                    quote_coordinator_member_id
                );
            }
        }

        if self.active_units().is_empty() {
            bail!(
                "Federation {} does not advertise an active keyset",
                self.public_config.federation_id
            );
        }

        Ok(())
    }

    /// Build the wallet config that injects a federated mint connector.
    pub fn wallet_config(&self) -> Result<WalletConfig> {
        Ok(WalletConfig::new().with_mint_connector(self.mint_connector()?))
    }

    /// Build the federated mint connector for this imported federation.
    pub fn mint_connector(&self) -> Result<Arc<dyn MintConnector + Send + Sync>> {
        let connector = FederatedMintConnector::from_validated_public_config_urls_with_config(
            self.public_config.clone(),
            self.connector_policy.connector_config(),
        )?;
        Ok(Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>)
    }

    /// Return all active units advertised by the imported federation.
    pub fn active_units(&self) -> Vec<CurrencyUnit> {
        self.public_config
            .keysets
            .iter()
            .filter(|keyset| keyset.active)
            .map(|keyset| keyset.unit.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Return whether the imported federation advertises an active keyset for a unit.
    pub fn supports_unit(&self, unit: &CurrencyUnit) -> bool {
        self.public_config
            .keysets
            .iter()
            .any(|keyset| keyset.active && &keyset.unit == unit)
    }

    /// Return whether a mint URL identifies this imported federation wallet.
    pub fn matches_mint_url(&self, mint_url: &MintUrl) -> bool {
        &self.local_mint_url == mint_url
            || self
                .public_config
                .members
                .values()
                .any(|member| &member.public_mint_url == mint_url)
    }

    /// Return the federation ID for display.
    pub fn federation_id(&self) -> FederationId {
        self.public_config.federation_id
    }

    /// Return the public config digest for display.
    pub fn config_digest(&self) -> Result<String> {
        self.validate()?;
        Ok(self
            .public_config
            .config_digest_for_validated_config()?
            .to_string())
    }
}

/// Persisted wallet-side federation request policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredFederationConnectorPolicy {
    /// Preferred member for the first quote-creation attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_coordinator_member_id: Option<MemberId>,
    /// Timeout in seconds for sequential first-healthy-member calls.
    pub member_fallback_timeout_secs: u64,
    /// Per-member timeout in seconds for fanout requests.
    pub member_request_timeout_secs: u64,
    /// Additional retry attempts for retryable member fanout failures.
    pub member_request_max_retries: u16,
    /// Per-member timeout in seconds for quote-status polling calls.
    pub quote_status_member_timeout_secs: u64,
    /// Maximum number of concurrent member calls.
    pub fanout_concurrency: usize,
    /// Attempts to wait for a newly created mint quote to become threshold-visible.
    pub quote_visibility_attempts: u16,
    /// Delay in milliseconds between mint quote threshold-visibility attempts.
    pub quote_visibility_retry_delay_ms: u64,
}

impl Default for StoredFederationConnectorPolicy {
    fn default() -> Self {
        Self::from_connector_config(FederatedMintConnectorConfig::default())
    }
}

impl StoredFederationConnectorPolicy {
    /// Convert a runtime connector policy into the persisted CLI form.
    pub fn from_connector_config(config: FederatedMintConnectorConfig) -> Self {
        Self {
            quote_coordinator_member_id: config.quote_coordinator_member_id,
            member_fallback_timeout_secs: config.member_fallback_timeout.as_secs(),
            member_request_timeout_secs: config.member_request_timeout.as_secs(),
            member_request_max_retries: config.member_request_max_retries,
            quote_status_member_timeout_secs: config.quote_status_member_timeout.as_secs(),
            fanout_concurrency: config.fanout_concurrency,
            quote_visibility_attempts: config.quote_visibility_attempts,
            quote_visibility_retry_delay_ms: millis_u64(config.quote_visibility_retry_delay),
        }
    }

    /// Convert the persisted CLI policy into the runtime connector policy.
    pub fn connector_config(&self) -> FederatedMintConnectorConfig {
        FederatedMintConnectorConfig {
            quote_coordinator_member_id: self.quote_coordinator_member_id,
            member_fallback_timeout: Duration::from_secs(self.member_fallback_timeout_secs),
            member_request_timeout: Duration::from_secs(self.member_request_timeout_secs),
            member_request_max_retries: self.member_request_max_retries,
            quote_status_member_timeout: Duration::from_secs(self.quote_status_member_timeout_secs),
            fanout_concurrency: self.fanout_concurrency,
            quote_visibility_attempts: self.quote_visibility_attempts,
            quote_visibility_retry_delay: Duration::from_millis(
                self.quote_visibility_retry_delay_ms,
            ),
        }
    }
}

/// Run a federation-aware wallet command.
pub async fn federation(
    work_dir: &Path,
    _wallet_repository: &WalletRepository,
    command: &FederationCommand,
) -> Result<()> {
    match &command.command {
        FederationSubCommand::Add(args) => add(work_dir, args).await,
        FederationSubCommand::List => list(work_dir),
    }
}

async fn add(work_dir: &Path, args: &FederationAddSubCommand) -> Result<()> {
    let mut registry = FederationWalletRegistry::load(work_dir)?;
    let public_config = load_imported_public_config(args).await?;
    let local_mint_url = match &args.local_mint_url {
        Some(local_mint_url) => local_mint_url.clone(),
        None => match &args.mint_url {
            Some(mint_url) => mint_url.clone(),
            None => default_member_mint_url(&public_config)?,
        },
    };
    let wallet = registry.upsert(public_config, local_mint_url, args.connector_policy()?)?;
    registry.save(work_dir)?;
    set_federation_registry(registry)?;

    println!("Imported federation {}", wallet.federation_id());
    println!("  Local mint URL: {}", wallet.local_mint_url);
    println!(
        "  Members: {} (signature threshold {}, consensus threshold {})",
        wallet.public_config.threshold_params.member_count,
        wallet.public_config.threshold_params.signature_threshold,
        wallet.public_config.threshold_params.consensus_threshold
    );
    if let Some(quote_coordinator_member_id) = wallet.connector_policy.quote_coordinator_member_id {
        println!(
            "  Preferred quote coordinator: {}",
            quote_coordinator_member_id
        );
    }
    println!("  Active units: {}", display_units(&wallet.active_units()));
    println!("  Config digest: {}", wallet.config_digest()?);

    Ok(())
}

fn list(work_dir: &Path) -> Result<()> {
    let registry = FederationWalletRegistry::load(work_dir)?;

    if registry.wallets.is_empty() {
        println!("No federations imported");
        return Ok(());
    }

    for wallet in registry.wallets {
        println!("Federation {}", wallet.federation_id());
        println!("  Local mint URL: {}", wallet.local_mint_url);
        println!(
            "  Members: {} (signature threshold {}, consensus threshold {})",
            wallet.public_config.threshold_params.member_count,
            wallet.public_config.threshold_params.signature_threshold,
            wallet.public_config.threshold_params.consensus_threshold
        );
        if let Some(quote_coordinator_member_id) =
            wallet.connector_policy.quote_coordinator_member_id
        {
            println!(
                "  Preferred quote coordinator: {}",
                quote_coordinator_member_id
            );
        }
        println!("  Active units: {}", display_units(&wallet.active_units()));
        println!("  Config digest: {}", wallet.config_digest()?);
    }

    Ok(())
}

fn load_public_config(path: &Path) -> Result<FederationConfig> {
    let contents = fs::read_to_string(path)?;
    let config: FederationConfig = serde_json::from_str(&contents)?;
    config.validate()?;
    Ok(config)
}

async fn load_imported_public_config(args: &FederationAddSubCommand) -> Result<FederationConfig> {
    match (&args.public_config, &args.invite_code, &args.mint_url) {
        (Some(path), None, None) => load_public_config(path),
        (None, Some(invite_code), None) => {
            let invite = FederationInvite::from_str(invite_code)?;
            invite.config.validate()?;
            Ok(invite.config)
        }
        (None, None, Some(mint_url)) => discover_public_config(mint_url).await,
        _ => bail!("Provide exactly one of --mint-url, --public-config, or --invite-code"),
    }
}

async fn discover_public_config(mint_url: &MintUrl) -> Result<FederationConfig> {
    let client = HttpClient::builder().no_redirects().build()?;
    let config_url = mint_url.join_paths(&["federation", "v1", "config"])?;
    let public_config: FederationConfig = client.fetch(config_url.as_str()).await?;
    let info_url = mint_url.join_paths(&["v1", "info"])?;
    let mint_info: MintInfo = client.fetch(info_url.as_str()).await?;
    validate_discovered_public_config(mint_url, public_config, &mint_info)
}

fn validate_discovered_public_config(
    mint_url: &MintUrl,
    public_config: FederationConfig,
    mint_info: &MintInfo,
) -> Result<FederationConfig> {
    public_config.validate()?;
    if !public_config.keysets.iter().any(|keyset| keyset.active) {
        bail!("Federation public config does not contain an active keyset");
    }

    let mut matching_members = public_config
        .members
        .iter()
        .filter(|(_, member)| &member.public_mint_url == mint_url);
    let Some((member_id, _)) = matching_members.next() else {
        bail!("Federation public config does not contain discovery mint URL {mint_url}");
    };
    if matching_members.next().is_some() {
        bail!("Federation public config contains discovery mint URL {mint_url} more than once");
    }

    let federation_info = mint_info.federation.as_ref().ok_or_else(|| {
        anyhow!("Discovery member mint info does not advertise federation metadata")
    })?;
    let config_digest = public_config.config_digest_for_validated_config()?;
    public_config.validate_member_mint_info_with_config_digest(
        *member_id,
        federation_info,
        config_digest,
    )?;
    Ok(public_config)
}

fn default_member_mint_url(config: &FederationConfig) -> Result<MintUrl> {
    config
        .members
        .values()
        .next()
        .map(|member| member.public_mint_url.clone())
        .ok_or_else(|| anyhow!("Federation config does not contain any members"))
}

fn registry_path(work_dir: &Path) -> PathBuf {
    work_dir.join(FEDERATION_REGISTRY_FILE)
}

fn display_units(units: &[CurrencyUnit]) -> String {
    units
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use cdk::nuts::nut01::BlsSecretKey;
    use cdk::nuts::nut02::Id;
    use cdk::Amount;
    use cdk_common::federation::{
        generate_trusted_dealer_configs, FederationConfigDigest, MemberId, ThresholdParams,
        TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    use cdk_common::SecretKey;

    use super::*;

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("valid member id")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("valid secp256k1 secret")
    }

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

    fn bls_key(byte: u8) -> BlsSecretKey {
        BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    fn valid_public_config() -> FederationConfig {
        public_config_with_thresholds(ThresholdParams::with_bft_consensus(3, 2).expect("params"))
    }

    fn public_config_with_thresholds(params: ThresholdParams) -> FederationConfig {
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

    fn make_first_keyset_non_v3(public_config: &mut FederationConfig) {
        let keyset = public_config
            .keysets
            .first_mut()
            .expect("valid config has a keyset");
        keyset.id = Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        );
    }

    fn temp_work_dir() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "cdk-cli-federation-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temp dir");
        path
    }

    fn add_args_with_import_source(
        public_config: Option<PathBuf>,
        invite_code: Option<String>,
    ) -> FederationAddSubCommand {
        FederationAddSubCommand {
            public_config,
            invite_code,
            mint_url: None,
            local_mint_url: None,
            quote_coordinator_member_id: None,
            member_fallback_timeout_secs: 10,
            member_request_timeout_secs: 10,
            member_request_max_retries: 0,
            quote_status_member_timeout_secs: 10,
            fanout_concurrency: usize::MAX,
            quote_visibility_attempts: 100,
            quote_visibility_retry_delay_ms: 50,
        }
    }

    #[test]
    fn registry_stores_only_public_federation_metadata() {
        let work_dir = temp_work_dir();
        let public_config = valid_public_config();
        let local_mint_url = default_member_mint_url(&public_config).expect("default member url");

        let mut registry = FederationWalletRegistry::default();
        registry
            .upsert(
                public_config,
                local_mint_url,
                StoredFederationConnectorPolicy::default(),
            )
            .expect("upsert");
        registry.save(&work_dir).expect("save");

        let contents = fs::read_to_string(registry_path(&work_dir)).expect("registry file");
        assert!(contents.contains("\"public_config\""));
        assert!(!contents.contains("private_configs"));
        assert!(!contents.contains("identity_secret_key"));
        assert!(!contents.contains("secret_shares"));

        let loaded = FederationWalletRegistry::load(&work_dir).expect("load");
        assert_eq!(loaded.wallets.len(), 1);

        fs::remove_dir_all(work_dir).expect("remove temp dir");
    }

    #[test]
    fn registry_load_rejects_malformed_json() {
        let work_dir = temp_work_dir();
        fs::write(registry_path(&work_dir), "{not-json").expect("write malformed registry");

        let err =
            FederationWalletRegistry::load(&work_dir).expect_err("malformed registry rejected");

        assert!(!err.to_string().is_empty());
        fs::remove_dir_all(work_dir).expect("remove temp dir");
    }

    #[test]
    fn registry_load_rejects_missing_current_fields() {
        let work_dir = temp_work_dir();
        let public_config = valid_public_config();
        let local_mint_url = default_member_mint_url(&public_config).expect("default member url");
        let wallet = StoredFederationWallet {
            local_mint_url,
            public_config,
            connector_policy: StoredFederationConnectorPolicy::default(),
        };
        let registry = FederationWalletRegistry {
            version: FEDERATION_REGISTRY_VERSION,
            wallets: vec![wallet],
        };

        for field in ["version", "connector_policy"] {
            let mut value = serde_json::to_value(&registry).expect("registry json");
            if field == "version" {
                value
                    .as_object_mut()
                    .expect("registry object")
                    .remove(field);
            } else {
                value["wallets"][0]
                    .as_object_mut()
                    .expect("wallet object")
                    .remove(field);
            }
            fs::write(
                registry_path(&work_dir),
                serde_json::to_vec(&value).expect("registry bytes"),
            )
            .expect("write incomplete registry");
            FederationWalletRegistry::load(&work_dir)
                .expect_err("incomplete current registry rejected");
        }

        fs::remove_dir_all(work_dir).expect("remove temp dir");
    }

    #[test]
    fn public_config_loader_rejects_malformed_json() {
        let work_dir = temp_work_dir();
        let public_config_path = work_dir.join("federation-public.json");
        fs::write(&public_config_path, "{not-json").expect("write malformed config");

        let err = load_public_config(&public_config_path).expect_err("malformed config rejected");

        assert!(!err.to_string().is_empty());
        fs::remove_dir_all(work_dir).expect("remove temp dir");
    }

    #[tokio::test]
    async fn imported_public_config_loader_accepts_invite_code() {
        let public_config = valid_public_config();
        let invite_code = FederationInvite::new(public_config.clone())
            .expect("invite")
            .to_invite_code()
            .expect("invite code");
        let args = add_args_with_import_source(None, Some(invite_code));

        let loaded = load_imported_public_config(&args)
            .await
            .expect("load invite config");

        assert_eq!(loaded, public_config);
    }

    #[tokio::test]
    async fn imported_public_config_loader_rejects_bad_invite_code() {
        let args = add_args_with_import_source(None, Some("cashuAunsupported".to_string()));

        let err = load_imported_public_config(&args)
            .await
            .expect_err("bad invite rejected");

        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn discovered_public_config_accepts_matching_source_member() {
        let public_config = valid_public_config();
        let source_url = public_config
            .member(member_id(2))
            .expect("source member")
            .public_mint_url
            .clone();
        let mint_info = MintInfo::new().federation(
            public_config
                .mint_info_for_member(member_id(2))
                .expect("member federation info"),
        );

        let validated =
            validate_discovered_public_config(&source_url, public_config.clone(), &mint_info)
                .expect("matching discovery metadata");

        assert_eq!(validated, public_config);
    }

    #[test]
    fn discovered_public_config_rejects_unlisted_source_url() {
        let public_config = valid_public_config();
        let source_url = MintUrl::from_str("https://unknown.example.com").expect("source URL");
        let mint_info = MintInfo::new().federation(
            public_config
                .mint_info_for_member(member_id(1))
                .expect("member federation info"),
        );

        let err = validate_discovered_public_config(&source_url, public_config, &mint_info)
            .expect_err("unlisted source rejected");

        assert!(err
            .to_string()
            .contains("does not contain discovery mint URL"));
    }

    #[test]
    fn discovered_public_config_rejects_mismatched_mint_info_digest() {
        let public_config = valid_public_config();
        let source_url = public_config
            .member(member_id(1))
            .expect("source member")
            .public_mint_url
            .clone();
        let mut federation_info = public_config
            .mint_info_for_member(member_id(1))
            .expect("member federation info");
        federation_info.config_digest = FederationConfigDigest::from_bytes([42; 32]).to_string();
        let mint_info = MintInfo::new().federation(federation_info);

        let err = validate_discovered_public_config(&source_url, public_config, &mint_info)
            .expect_err("mismatched digest rejected");

        assert!(err.to_string().contains("config digest"));
    }

    #[test]
    fn registry_accepts_only_current_v1_shape() {
        let registry = FederationWalletRegistry::default();
        assert_eq!(registry.version, 1);
        registry.validate().expect("current v1 registry");

        for version in [0, 2, u16::MAX] {
            let registry = FederationWalletRegistry {
                version,
                wallets: Vec::new(),
            };
            let err = registry
                .validate()
                .expect_err("non-v1 registry version rejected");
            assert!(err
                .to_string()
                .contains("Unsupported federation registry version"));
        }
    }

    #[test]
    fn registry_rejects_removed_primary_member_field() {
        let mut value = serde_json::to_value(FederationWalletRegistry {
            version: FEDERATION_REGISTRY_VERSION,
            wallets: vec![StoredFederationWallet {
                local_mint_url: MintUrl::from_str("https://wallet.example.com")
                    .expect("local mint url"),
                public_config: valid_public_config(),
                connector_policy: StoredFederationConnectorPolicy::default(),
            }],
        })
        .expect("registry json");
        value["wallets"][0]["connector_policy"]["primary_member_id"] = serde_json::json!(1);

        serde_json::from_value::<FederationWalletRegistry>(value)
            .expect_err("removed primary_member_id must not be accepted as current v1");
    }

    #[test]
    fn registry_rejects_inactive_federation_wallet() {
        let mut public_config = valid_public_config();
        for keyset in &mut public_config.keysets {
            keyset.active = false;
        }
        let local_mint_url = default_member_mint_url(&public_config).expect("default member url");
        let mut registry = FederationWalletRegistry::default();

        let err = registry
            .upsert(
                public_config,
                local_mint_url,
                StoredFederationConnectorPolicy::default(),
            )
            .expect_err("inactive federation should be rejected");

        assert!(err
            .to_string()
            .contains("does not advertise an active keyset"));
    }

    #[test]
    fn registry_rejects_non_v3_federation_keyset_before_connector() {
        let mut public_config = valid_public_config();
        make_first_keyset_non_v3(&mut public_config);
        let local_mint_url = default_member_mint_url(&public_config).expect("default member url");
        let mut registry = FederationWalletRegistry::default();

        let err = registry
            .upsert(
                public_config,
                local_mint_url,
                StoredFederationConnectorPolicy::default(),
            )
            .expect_err("non-v3 federation keyset rejected");

        assert!(err.to_string().contains("must use Cashu keyset v3"));
    }

    #[test]
    fn registry_rejects_stale_public_config_protocol_version() {
        let mut public_config = valid_public_config();
        public_config.wallet_protocol_version += 1;
        let local_mint_url = default_member_mint_url(&public_config).expect("default member url");
        let mut registry = FederationWalletRegistry::default();

        let err = registry
            .upsert(
                public_config,
                local_mint_url,
                StoredFederationConnectorPolicy::default(),
            )
            .expect_err("stale public config version rejected");

        assert!(err.to_string().contains("protocol version mismatch"));
    }

    #[test]
    fn registry_rejects_mismatched_roster_metadata() {
        let mut public_config = valid_public_config();
        public_config.threshold_params =
            ThresholdParams::with_bft_consensus(4, 3).expect("mismatched params");
        let local_mint_url = default_member_mint_url(&public_config).expect("default member url");
        let mut registry = FederationWalletRegistry::default();

        let err = registry
            .upsert(
                public_config,
                local_mint_url,
                StoredFederationConnectorPolicy::default(),
            )
            .expect_err("mismatched roster metadata rejected");

        assert!(err.to_string().contains("member count mismatch"));
    }

    #[test]
    fn registry_rejects_unsafe_production_threshold_profile() {
        let public_config = public_config_with_thresholds(
            ThresholdParams::new(3, 2, 2).expect("structurally valid thresholds"),
        );
        let local_mint_url = default_member_mint_url(&public_config).expect("default member url");
        let mut registry = FederationWalletRegistry::default();

        let err = registry
            .upsert(
                public_config,
                local_mint_url,
                StoredFederationConnectorPolicy::default(),
            )
            .expect_err("unsafe threshold profile rejected");

        assert!(err
            .to_string()
            .contains("unsafe federation threshold profile"));
    }

    #[test]
    fn stored_wallet_builds_federated_connector_config() {
        let public_config = valid_public_config();
        let local_mint_url = default_member_mint_url(&public_config).expect("default member url");
        let wallet = StoredFederationWallet {
            local_mint_url,
            public_config,
            connector_policy: StoredFederationConnectorPolicy {
                quote_coordinator_member_id: Some(member_id(2)),
                ..StoredFederationConnectorPolicy::default()
            },
        };

        wallet.wallet_config().expect("wallet config");
        assert!(wallet.supports_unit(&CurrencyUnit::Sat));
        assert_eq!(
            wallet
                .connector_policy
                .connector_config()
                .quote_coordinator_member_id,
            Some(member_id(2))
        );
    }

    #[test]
    fn registry_rejects_unknown_quote_coordinator_preference() {
        let public_config = valid_public_config();
        let local_mint_url = default_member_mint_url(&public_config).expect("default member url");
        let mut registry = FederationWalletRegistry::default();

        let err = registry
            .upsert(
                public_config,
                local_mint_url,
                StoredFederationConnectorPolicy {
                    quote_coordinator_member_id: Some(member_id(99)),
                    ..StoredFederationConnectorPolicy::default()
                },
            )
            .expect_err("unknown quote coordinator rejected");

        assert!(err
            .to_string()
            .contains("does not contain quote coordinator member 99"));
    }

    #[test]
    fn registry_resolves_member_public_mint_url() {
        let public_config = valid_public_config();
        let member_two_url = public_config
            .members
            .get(&member_id(2))
            .expect("member two")
            .public_mint_url
            .clone();
        let local_mint_url = default_member_mint_url(&public_config).expect("default member url");

        let mut registry = FederationWalletRegistry::default();
        registry
            .upsert(
                public_config,
                local_mint_url,
                StoredFederationConnectorPolicy::default(),
            )
            .expect("upsert");

        let resolved = registry
            .find_by_mint_url(&member_two_url)
            .expect("member URL resolves to imported federation");
        assert!(resolved.matches_mint_url(&member_two_url));
    }
}
