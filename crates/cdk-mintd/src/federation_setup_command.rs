use core::fmt;
use std::collections::BTreeMap;
#[cfg(feature = "sqlite")]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use cdk_common::database::mint::Database;
use cdk_common::database::Error as DatabaseError;
#[cfg(feature = "sqlite")]
use cdk_common::database::MintFederationSetupDatabase;
#[cfg(feature = "sqlite")]
use cdk_common::federation::FederationSetupFrostApplication;
use cdk_common::federation::{
    FederationCheckpointPolicy, FederationDkgKeysetSpec, FederationPaymentObservationPolicy,
    FederationSetupBitcoinNetwork, FederationSetupCoordinatorPublicKey, FederationSetupEnrollment,
    FederationSetupFrostPolicy, FederationSetupId, FederationSetupInvitation,
    FederationSetupInvitationCapability, FederationSetupInvitationId,
    FederationSetupInvitationRecord, FederationSetupInvitationState, FederationSetupLocalRole,
    FederationSetupManifest, FederationSetupParticipantPhase, FederationSetupProposal,
    FederationSetupProposalBody, FederationSetupProposalHash, FederationSetupProposalReview,
    FederationSetupProtocolVersions, FederationSetupSession, FederationSetupUrlPolicy, MemberId,
    ThresholdParams, FEDERATION_SETUP_MAX_INVITATION_LIFETIME_SECS,
    FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::SecretKey;
#[cfg(feature = "sqlite")]
use cdk_common::{Amount, CurrencyUnit};
use serde::{Deserialize, Serialize};

use crate::cli;
use crate::federation_files::{
    read_private_json_file, read_private_text_file, write_private_json_file,
    write_private_text_file,
};

const LOCAL_SETUP_CONFIG_VERSION: u16 = 1;
const LOCAL_SETUP_SECRETS_FILE: &str = "federation-setup-secrets.json";
const LOCAL_SETUP_INVITATION_DIRECTORY: &str = "invitations";

#[cfg(feature = "sqlite")]
pub(crate) async fn run_setup_command(args: &cli::FederationSetupArgs) -> Result<()> {
    match &args.command {
        cli::FederationSetupCommand::Create(args) => {
            let thresholds = ThresholdParams::with_bft_consensus(args.members, args.threshold)
                .context("invalid federation threshold profile")?;
            let payment_threshold = args
                .payment_observation_threshold
                .unwrap_or(thresholds.consensus_threshold);
            let network = setup_network(args.network);
            let spec = FederationSetupCreateSpec {
                threshold_params: thresholds,
                public_mint_url: parse_mint_url(&args.public_mint_url, "public mint URL")?,
                federation_api_url: parse_mint_url(&args.federation_api_url, "federation API URL")?,
                coordinator_url: parse_mint_url(&args.coordinator_url, "coordinator URL")?,
                network,
                payment_observation_policy: FederationPaymentObservationPolicy::member_quorum(
                    payment_threshold,
                ),
                checkpoint_policy: FederationCheckpointPolicy::default(),
                keysets: vec![FederationDkgKeysetSpec::new(
                    CurrencyUnit::from_str(&args.unit).context("invalid setup currency unit")?,
                    true,
                    args.input_fee_ppk,
                    args.final_expiry,
                    args.amounts
                        .iter()
                        .copied()
                        .map(Amount::from)
                        .collect::<BTreeSet<_>>(),
                )
                .context("invalid setup keyset")?],
                frost: FederationSetupFrostPolicy::new(
                    args.frost_root_key_epoch,
                    vec![FederationSetupFrostApplication::CdkOnchain {
                        application_key_epoch: args.frost_application_key_epoch,
                        confirmation_policy_version: 1,
                        minimum_confirmations: args.frost_minimum_confirmations,
                        fee_policy_version: args.frost_fee_policy_version,
                    }],
                    network,
                )
                .context("invalid setup FROST policy")?,
                invitation_lifetime_secs: args.invitation_lifetime_seconds,
                url_policy: setup_url_policy(args.allow_insecure_localhost),
            };
            let db = cdk_sqlite::MintSqliteDatabase::new(args.database.clone())
                .await
                .context("failed to open federation setup database")?;
            let created = create_setup(
                &db,
                &args.config,
                &args.state_dir,
                &args.database,
                &spec,
                current_unix_time()?,
            )
            .await?;
            println!("setup_id: {}", created.setup_id);
            println!("config: {}", created.config_path.display());
            for (member_id, path) in created.invitation_paths {
                println!("member_{}_invitation: {}", member_id.get(), path.display());
            }
        }
        cli::FederationSetupCommand::Invite(args) => {
            let config = read_local_config(&args.config)?;
            let db = cdk_sqlite::MintSqliteDatabase::new(config.database_path().to_path_buf())
                .await
                .context("failed to open federation setup database")?;
            let invitation = issue_invitation(
                &db,
                &args.config,
                MemberId::new(args.member).context("invalid setup member ID")?,
                args.replace,
                &args.output,
                current_unix_time()?,
            )
            .await?;
            println!("setup_id: {}", invitation.setup_id);
            println!("member_id: {}", invitation.member_id.get());
            println!("invitation_file: {}", args.output.display());
        }
        cli::FederationSetupCommand::Join(args) => {
            let invitation =
                read_private_text_file(&args.invitation_file, "federation setup invitation")?
                    .trim()
                    .parse::<FederationSetupInvitation>()
                    .context("invalid federation setup invitation")?;
            let db = cdk_sqlite::MintSqliteDatabase::new(args.database.clone())
                .await
                .context("failed to open federation setup database")?;
            let enrollment = join_setup(
                &db,
                &HttpSetupCoordinator::default(),
                &args.config,
                &args.state_dir,
                &args.database,
                invitation,
                parse_mint_url(&args.public_mint_url, "public mint URL")?,
                parse_mint_url(&args.federation_api_url, "federation API URL")?,
                setup_url_policy(args.allow_insecure_localhost),
                current_unix_time()?,
            )
            .await?;
            println!("setup_id: {}", enrollment.setup_id);
            println!("member_id: {}", enrollment.member_id.get());
        }
        cli::FederationSetupCommand::Propose(args) => {
            let config = read_local_config(&args.config)?;
            let db = cdk_sqlite::MintSqliteDatabase::new(config.database_path().to_path_buf())
                .await
                .context("failed to open federation setup database")?;
            let proposal = propose_setup(&db, &args.config, current_unix_time()?).await?;
            println!("proposal_hash: {}", proposal.proposal_hash);
        }
        cli::FederationSetupCommand::Review(args) => {
            let config = read_local_config(&args.config)?;
            let db = cdk_sqlite::MintSqliteDatabase::new(config.database_path().to_path_buf())
                .await
                .context("failed to open federation setup database")?;
            let review = match config.local_role {
                FederationSetupLocalRole::Coordinator { .. } => {
                    let proposal = db
                        .get_federation_setup_proposal(config.setup_id())
                        .await
                        .context("failed to load the local setup proposal")?
                        .ok_or_else(|| anyhow!("no local setup proposal is available to review"))?;
                    review_local_proposal(&db, &args.config, proposal, current_unix_time()?).await?
                }
                FederationSetupLocalRole::Member { .. } => {
                    fetch_and_review_proposal(
                        &db,
                        &HttpSetupCoordinator::default(),
                        &args.config,
                        current_unix_time()?,
                    )
                    .await?
                }
            };
            match args.json {
                true => println!("{}", review.to_pretty_json()?),
                false => println!("{review}"),
            }
        }
        cli::FederationSetupCommand::Approve(args) => {
            let config = read_local_config(&args.config)?;
            let db = cdk_sqlite::MintSqliteDatabase::new(config.database_path().to_path_buf())
                .await
                .context("failed to open federation setup database")?;
            let hash = FederationSetupProposalHash::from_str(&args.proposal_hash)
                .context("invalid exact proposal hash")?;
            let approval = approve_setup(
                &db,
                &HttpSetupCoordinator::default(),
                &args.config,
                hash,
                current_unix_time()?,
            )
            .await?;
            println!("approved_proposal_hash: {}", approval.proposal_hash);
        }
        cli::FederationSetupCommand::Serve(args) => {
            serve_setup_coordinator(&args.config, args.listen).await?;
        }
        cli::FederationSetupCommand::Status(args) => {
            let status = setup_status(&args.config).await?;
            match args.json {
                true => println!("{}", serde_json::to_string_pretty(&status)?),
                false => {
                    println!("setup_id: {}", status.setup_id);
                    println!(
                        "phase: {}",
                        coordinator_phase_label(status.coordinator_phase)
                    );
                    println!(
                        "enrollments: {}/{}",
                        status.enrolled_members.len(),
                        status.member_count
                    );
                    println!(
                        "approvals: {}/{}",
                        status.approved_members.len(),
                        status.member_count
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(not(feature = "sqlite"))]
pub(crate) async fn run_setup_command(_args: &cli::FederationSetupArgs) -> Result<()> {
    bail!("federation setup commands currently require the `sqlite` feature")
}

#[cfg(feature = "sqlite")]
#[derive(Debug, Default)]
struct HttpSetupCoordinator(cdk_axum::FederationSetupHttpClient);

#[cfg(feature = "sqlite")]
#[async_trait]
impl FederationSetupCoordinator for HttpSetupCoordinator {
    async fn enroll(
        &self,
        invitation: &FederationSetupInvitation,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<FederationSetupEnrollment> {
        self.0
            .enroll(invitation, enrollment)
            .await
            .context("setup enrollment HTTP request failed")
    }

    async fn proposal(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> Result<FederationSetupProposal> {
        self.0
            .proposal(invitation)
            .await
            .context("setup proposal HTTP request failed")
    }

    async fn approve(
        &self,
        invitation: &FederationSetupInvitation,
        approval: &cdk_common::federation::FederationSetupApproval,
    ) -> Result<cdk_common::federation::FederationSetupApproval> {
        self.0
            .approve(invitation, approval)
            .await
            .context("setup approval HTTP request failed")
    }
}

#[cfg(feature = "sqlite")]
async fn serve_setup_coordinator(config_path: &Path, listen: std::net::SocketAddr) -> Result<()> {
    let now_unix = current_unix_time()?;
    let (config, secrets) = load_local_state(config_path, now_unix)?;
    let draft = config
        .coordinator
        .as_ref()
        .ok_or_else(|| anyhow!("only the setup creator can run the coordinator service"))?;
    let coordinator_secret_key = secrets
        .coordinator_secret_key
        .clone()
        .ok_or_else(|| anyhow!("protected setup coordinator identity is missing"))?;
    let database: cdk_common::database::DynMintDatabase = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(config.database_path().to_path_buf())
            .await
            .context("failed to open federation setup database")?,
    );
    let service = Arc::new(
        cdk_axum::FederationSetupCoordinatorService::new(
            database,
            config.setup_id,
            draft.threshold_params.member_count,
            coordinator_secret_key,
            config.url_policy,
            cdk_axum::FederationSetupCoordinatorServiceConfig::default(),
        )
        .context("failed to initialize setup coordinator service")?,
    );
    let router = cdk_axum::create_federation_setup_router(service.clone());
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .context("failed to bind setup-only coordinator listener")?;
    println!("setup_id: {}", config.setup_id);
    println!("setup_only_listen: {listen}");
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!("setup coordinator shutdown signal failed: {error}");
        }
    });
    let result = server.await.context("setup-only coordinator server failed");
    service.shutdown().await;
    result
}

#[cfg(feature = "sqlite")]
async fn setup_status(config_path: &Path) -> Result<cdk_common::federation::FederationSetupStatus> {
    let now_unix = current_unix_time()?;
    let (config, secrets) = load_local_state(config_path, now_unix)?;
    let database = cdk_sqlite::MintSqliteDatabase::new(config.database_path().to_path_buf())
        .await
        .context("failed to open federation setup database")?;
    match config.local_role {
        FederationSetupLocalRole::Coordinator { .. } => {
            let draft = config
                .coordinator
                .as_ref()
                .ok_or_else(|| anyhow!("local coordinator policy is missing"))?;
            let coordinator_secret_key = secrets
                .coordinator_secret_key
                .clone()
                .ok_or_else(|| anyhow!("protected setup coordinator identity is missing"))?;
            let database: cdk_common::database::DynMintDatabase = Arc::new(database);
            let service = cdk_axum::FederationSetupCoordinatorService::new(
                database,
                config.setup_id,
                draft.threshold_params.member_count,
                coordinator_secret_key,
                config.url_policy,
                cdk_axum::FederationSetupCoordinatorServiceConfig::default(),
            )
            .context("failed to initialize local setup status reader")?;
            service
                .local_status()
                .await
                .context("failed to read local setup status")
        }
        FederationSetupLocalRole::Member { .. } => {
            let invitation = secrets
                .invitation
                .as_ref()
                .ok_or_else(|| anyhow!("protected joining invitation is missing"))?;
            let client = cdk_axum::FederationSetupHttpClient::default();
            let status = client
                .status(invitation)
                .await
                .context("failed to fetch authenticated setup status")?;
            if status.manifest_hash.is_some() {
                let manifest = client
                    .manifest(invitation)
                    .await
                    .context("failed to fetch authenticated certified manifest")?;
                persist_local_manifest(&database, &config, &secrets, &manifest, now_unix).await?;
            }
            Ok(status)
        }
    }
}

#[cfg(feature = "sqlite")]
async fn persist_local_manifest<DB>(
    database: &DB,
    config: &FederationSetupLocalConfig,
    secrets: &FederationSetupLocalSecrets,
    manifest: &FederationSetupManifest,
    now_unix: u64,
) -> Result<()>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
{
    manifest
        .validate(config.url_policy)
        .context("invalid certified setup manifest")?;
    validate_proposal_for_local_state(&manifest.proposal, config, secrets)?;
    if let Some(existing) = database
        .get_federation_setup_manifest(config.setup_id)
        .await
        .context("failed to load local certified setup manifest")?
    {
        if existing != *manifest {
            bail!("local setup is already certified by a different manifest");
        }
        return Ok(());
    }
    let session = require_local_session(database, config).await?;
    if session.participant_phase != FederationSetupParticipantPhase::Approved {
        bail!("certified manifest cannot be persisted before local approval");
    }
    let certified = session
        .with_participant_phase(FederationSetupParticipantPhase::Certified, now_unix)
        .context("manifest certification is not legal in the current setup phase")?;
    let mut transaction = Database::begin_transaction(database)
        .await
        .context("failed to begin setup manifest transaction")?;
    transaction
        .store_federation_setup_manifest(&session, &certified, manifest)
        .await
        .context("failed to persist certified setup manifest")?;
    transaction
        .commit()
        .await
        .context("failed to commit certified setup manifest")
}

#[cfg(feature = "sqlite")]
fn setup_network(network: cli::FederationSetupNetworkArg) -> FederationSetupBitcoinNetwork {
    match network {
        cli::FederationSetupNetworkArg::Bitcoin => FederationSetupBitcoinNetwork::Bitcoin,
        cli::FederationSetupNetworkArg::Testnet => FederationSetupBitcoinNetwork::Testnet,
        cli::FederationSetupNetworkArg::Signet => FederationSetupBitcoinNetwork::Signet,
        cli::FederationSetupNetworkArg::Regtest => FederationSetupBitcoinNetwork::Regtest,
    }
}

#[cfg(feature = "sqlite")]
fn setup_url_policy(allow_insecure_localhost: bool) -> FederationSetupUrlPolicy {
    match allow_insecure_localhost {
        true => FederationSetupUrlPolicy::AllowInsecureLocalhost,
        false => FederationSetupUrlPolicy::RequireHttps,
    }
}

#[cfg(feature = "sqlite")]
const fn coordinator_phase_label(
    phase: cdk_common::federation::FederationSetupCoordinatorPhase,
) -> &'static str {
    use cdk_common::federation::FederationSetupCoordinatorPhase;

    match phase {
        FederationSetupCoordinatorPhase::CollectingEnrollments => "collecting_enrollments",
        FederationSetupCoordinatorPhase::ProposalPublished => "proposal_published",
        FederationSetupCoordinatorPhase::Certified => "certified",
        FederationSetupCoordinatorPhase::Closed => "closed",
        FederationSetupCoordinatorPhase::Aborted => "aborted",
    }
}

#[cfg(feature = "sqlite")]
fn parse_mint_url(value: &str, label: &str) -> Result<MintUrl> {
    MintUrl::from_str(value).with_context(|| format!("invalid {label}"))
}

#[derive(Debug, Clone)]
pub(crate) struct FederationSetupCreateSpec {
    pub(crate) threshold_params: ThresholdParams,
    pub(crate) public_mint_url: MintUrl,
    pub(crate) federation_api_url: MintUrl,
    pub(crate) coordinator_url: MintUrl,
    pub(crate) network: FederationSetupBitcoinNetwork,
    pub(crate) payment_observation_policy: FederationPaymentObservationPolicy,
    pub(crate) checkpoint_policy: FederationCheckpointPolicy,
    pub(crate) keysets: Vec<FederationDkgKeysetSpec>,
    pub(crate) frost: FederationSetupFrostPolicy,
    pub(crate) invitation_lifetime_secs: u64,
    pub(crate) url_policy: FederationSetupUrlPolicy,
}

impl FederationSetupCreateSpec {
    fn validate(&self) -> Result<()> {
        self.threshold_params
            .validate_bft_safety()
            .context("unsafe federation setup threshold profile")?;
        if !(FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS
            ..=FEDERATION_SETUP_MAX_INVITATION_LIFETIME_SECS)
            .contains(&self.invitation_lifetime_secs)
        {
            bail!("federation setup invitation lifetime is outside the supported bounds");
        }
        self.payment_observation_policy
            .validate(self.threshold_params)
            .context("invalid payment-observation policy")?;
        self.checkpoint_policy
            .validate()
            .context("invalid checkpoint policy")?;
        if self.keysets.is_empty() {
            bail!("federation setup requires at least one BLS keyset");
        }
        for keyset in &self.keysets {
            keyset.validate().context("invalid setup keyset policy")?;
        }
        self.frost
            .validate(self.network)
            .context("invalid setup FROST policy")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FederationSetupCreateResult {
    pub(crate) config_path: PathBuf,
    pub(crate) setup_id: FederationSetupId,
    pub(crate) invitation_paths: BTreeMap<MemberId, PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FederationSetupLocalConfig {
    version: u16,
    setup_id: FederationSetupId,
    local_role: FederationSetupLocalRole,
    database_path: PathBuf,
    secrets_path: PathBuf,
    url_policy: FederationSetupUrlPolicy,
    coordinator: Option<FederationSetupCoordinatorDraft>,
}

impl FederationSetupLocalConfig {
    pub(crate) fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub(crate) fn setup_id(&self) -> FederationSetupId {
        self.setup_id
    }

    fn validate(&self) -> Result<()> {
        if self.version != LOCAL_SETUP_CONFIG_VERSION {
            bail!("unsupported local federation setup config version");
        }
        let session = FederationSetupSession::new(self.setup_id, self.local_role, 0)
            .context("invalid local federation setup config")?;
        let is_coordinator = matches!(
            session.local_role,
            FederationSetupLocalRole::Coordinator { .. }
        );
        if is_coordinator != self.coordinator.is_some() {
            bail!("local setup coordinator policy does not match the configured role");
        }
        if let Some(coordinator) = &self.coordinator {
            coordinator.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FederationSetupCoordinatorDraft {
    coordinator_url: MintUrl,
    threshold_params: ThresholdParams,
    required_versions: FederationSetupProtocolVersions,
    network: FederationSetupBitcoinNetwork,
    payment_observation_policy: FederationPaymentObservationPolicy,
    checkpoint_policy: FederationCheckpointPolicy,
    keysets: Vec<FederationDkgKeysetSpec>,
    frost: FederationSetupFrostPolicy,
    invitation_lifetime_secs: u64,
}

impl FederationSetupCoordinatorDraft {
    fn from_spec(spec: &FederationSetupCreateSpec) -> Self {
        Self {
            coordinator_url: spec.coordinator_url.clone(),
            threshold_params: spec.threshold_params,
            required_versions: FederationSetupProtocolVersions::default(),
            network: spec.network,
            payment_observation_policy: spec.payment_observation_policy.clone(),
            checkpoint_policy: spec.checkpoint_policy,
            keysets: spec.keysets.clone(),
            frost: spec.frost.clone(),
            invitation_lifetime_secs: spec.invitation_lifetime_secs,
        }
    }

    fn validate(&self) -> Result<()> {
        self.required_versions
            .validate()
            .context("invalid setup protocol tuple")?;
        self.threshold_params
            .validate_bft_safety()
            .context("unsafe federation setup threshold profile")?;
        self.payment_observation_policy
            .validate(self.threshold_params)
            .context("invalid payment-observation policy")?;
        self.checkpoint_policy
            .validate()
            .context("invalid checkpoint policy")?;
        if !(FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS
            ..=FEDERATION_SETUP_MAX_INVITATION_LIFETIME_SECS)
            .contains(&self.invitation_lifetime_secs)
        {
            bail!("federation setup invitation lifetime is outside the supported bounds");
        }
        for keyset in &self.keysets {
            keyset.validate().context("invalid setup keyset policy")?;
        }
        self.frost
            .validate(self.network)
            .context("invalid setup FROST policy")
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FederationSetupLocalSecrets {
    version: u16,
    setup_id: FederationSetupId,
    member_id: MemberId,
    identity_secret_key: SecretKey,
    coordinator_secret_key: Option<SecretKey>,
    invitation: Option<FederationSetupInvitation>,
    enrollment: FederationSetupEnrollment,
}

impl fmt::Debug for FederationSetupLocalSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederationSetupLocalSecrets")
            .field("version", &self.version)
            .field("setup_id", &self.setup_id)
            .field("member_id", &self.member_id)
            .field("identity_secret_key", &"[REDACTED]")
            .field(
                "coordinator_secret_key",
                &self.coordinator_secret_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "invitation",
                &self.invitation.as_ref().map(|_| "[REDACTED]"),
            )
            .field("enrollment", &self.enrollment)
            .finish()
    }
}

impl FederationSetupLocalSecrets {
    fn validate(&self, config: &FederationSetupLocalConfig, now_unix: u64) -> Result<()> {
        if self.version != LOCAL_SETUP_CONFIG_VERSION
            || self.setup_id != config.setup_id
            || self.member_id != config.local_role.member_id()
            || self.identity_secret_key.public_key() != self.enrollment.identity_public_key
            || self.enrollment.setup_id != self.setup_id
            || self.enrollment.member_id != self.member_id
        {
            bail!("protected local setup identity does not match its config or enrollment");
        }
        self.enrollment
            .validate(config.url_policy)
            .context("invalid protected local setup enrollment")?;
        match (config.local_role, self.coordinator_secret_key.as_ref()) {
            (FederationSetupLocalRole::Coordinator { .. }, Some(coordinator_secret)) => {
                let configured = config
                    .coordinator
                    .as_ref()
                    .ok_or_else(|| anyhow!("coordinator policy is missing"))?;
                validate_setup_url(&configured.coordinator_url, config.url_policy)
                    .context("invalid coordinator URL")?;
                let _identity =
                    FederationSetupCoordinatorPublicKey::new(coordinator_secret.public_key())
                        .context("invalid protected coordinator identity")?;
                if self.invitation.is_some() {
                    bail!("protected coordinator setup secrets have an invalid shape");
                }
            }
            (FederationSetupLocalRole::Member { .. }, None) => {
                let invitation = self
                    .invitation
                    .as_ref()
                    .ok_or_else(|| anyhow!("joining member invitation is missing"))?;
                invitation
                    .validate(
                        now_unix.min(invitation.expires_at_unix.saturating_sub(1)),
                        config.url_policy,
                    )
                    .context("protected joining invitation is invalid")?;
                if invitation.setup_id != self.setup_id
                    || invitation.member_id != self.member_id
                    || invitation.invitation_id != self.enrollment.invitation_id
                {
                    bail!("protected joining invitation does not match its enrollment");
                }
            }
            _ => bail!("protected local setup secrets do not match the configured role"),
        }
        Ok(())
    }

    fn coordinator_identity(&self) -> Result<FederationSetupCoordinatorPublicKey> {
        let secret = self
            .coordinator_secret_key
            .as_ref()
            .ok_or_else(|| anyhow!("local setup member is not the coordinator"))?;
        FederationSetupCoordinatorPublicKey::new(secret.public_key())
            .context("invalid protected coordinator identity")
    }
}

#[allow(dead_code)] // Used by the M3 fake gate and the bounded M4 client.
#[async_trait]
pub(crate) trait FederationSetupCoordinator: Send + Sync {
    async fn enroll(
        &self,
        invitation: &FederationSetupInvitation,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<FederationSetupEnrollment>;

    async fn proposal(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> Result<FederationSetupProposal>;

    async fn approve(
        &self,
        invitation: &FederationSetupInvitation,
        approval: &cdk_common::federation::FederationSetupApproval,
    ) -> Result<cdk_common::federation::FederationSetupApproval>;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_setup<DB>(
    db: &DB,
    config_path: &Path,
    state_directory: &Path,
    database_path: &Path,
    spec: &FederationSetupCreateSpec,
    now_unix: u64,
) -> Result<FederationSetupCreateResult>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
{
    spec.validate()?;
    std::fs::create_dir_all(state_directory)
        .context("failed to create federation setup state directory")?;
    let secrets_path = state_directory.join(LOCAL_SETUP_SECRETS_FILE);
    let invitation_directory = state_directory.join(LOCAL_SETUP_INVITATION_DIRECTORY);
    std::fs::create_dir_all(&invitation_directory)
        .context("failed to create protected invitation output directory")?;

    let draft = FederationSetupCoordinatorDraft::from_spec(spec);
    let creator_member_id = MemberId::new(1).context("invalid creator member ID")?;
    let secrets = match secrets_path.exists() {
        true => read_private_json_file::<FederationSetupLocalSecrets>(
            &secrets_path,
            "federation setup secrets",
        )?,
        false => {
            let setup_id = random_setup_id();
            let identity_secret_key = SecretKey::generate();
            let coordinator_secret_key = SecretKey::generate();
            let enrollment = FederationSetupEnrollment::sign(
                setup_id,
                random_invitation_id(),
                creator_member_id,
                spec.public_mint_url.clone(),
                spec.federation_api_url.clone(),
                random_bytes(),
                FederationSetupProtocolVersions::default(),
                &identity_secret_key,
                spec.url_policy,
            )
            .context("failed to create the creator enrollment")?;
            let secrets = FederationSetupLocalSecrets {
                version: LOCAL_SETUP_CONFIG_VERSION,
                setup_id,
                member_id: creator_member_id,
                identity_secret_key,
                coordinator_secret_key: Some(coordinator_secret_key),
                invitation: None,
                enrollment,
            };
            write_private_json_file(&secrets_path, &secrets, false)
                .context("failed to persist protected creator identity")?;
            secrets
        }
    };
    let config = FederationSetupLocalConfig {
        version: LOCAL_SETUP_CONFIG_VERSION,
        setup_id: secrets.setup_id,
        local_role: FederationSetupLocalRole::Coordinator {
            member_id: creator_member_id,
        },
        database_path: database_path.to_path_buf(),
        secrets_path: secrets_path.clone(),
        url_policy: spec.url_policy,
        coordinator: Some(draft.clone()),
    };
    if config_path.exists() {
        let existing: FederationSetupLocalConfig =
            read_private_json_file(config_path, "federation setup config")?;
        if !local_config_matches(&existing, &config) {
            bail!("existing local federation setup config conflicts with create arguments");
        }
    } else {
        write_private_json_file(config_path, &config, false)
            .context("failed to persist local federation setup config")?;
    }
    config.validate()?;
    secrets.validate(&config, now_unix)?;
    let coordinator_identity = secrets.coordinator_identity()?;

    let mut invitations = BTreeMap::new();
    let mut invitation_paths = BTreeMap::new();
    for member in 2..=spec.threshold_params.member_count {
        let member_id = MemberId::new(member).context("invalid invited member ID")?;
        let invitation_path = invitation_directory.join(format!("member-{member}.invite"));
        let invitation = load_or_create_invitation(
            &invitation_path,
            secrets.setup_id,
            member_id,
            &draft,
            coordinator_identity,
            now_unix,
        )?;
        invitation_paths.insert(member_id, invitation_path);
        invitations.insert(member_id, invitation);
    }
    persist_creator_state(db, &config, &secrets, invitations.values(), now_unix).await?;

    Ok(FederationSetupCreateResult {
        config_path: config_path.to_path_buf(),
        setup_id: secrets.setup_id,
        invitation_paths,
    })
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)] // Wired to the public CLI when the bounded M4 client lands.
pub(crate) async fn join_setup<DB, C>(
    db: &DB,
    coordinator: &C,
    config_path: &Path,
    state_directory: &Path,
    database_path: &Path,
    invitation: FederationSetupInvitation,
    public_mint_url: MintUrl,
    federation_api_url: MintUrl,
    url_policy: FederationSetupUrlPolicy,
    now_unix: u64,
) -> Result<FederationSetupEnrollment>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
    C: FederationSetupCoordinator,
{
    invitation
        .validate(now_unix, url_policy)
        .context("invalid federation setup invitation")?;
    std::fs::create_dir_all(state_directory)
        .context("failed to create federation setup state directory")?;
    let secrets_path = state_directory.join(LOCAL_SETUP_SECRETS_FILE);
    let local_role = FederationSetupLocalRole::Member {
        member_id: invitation.member_id,
    };
    let config = FederationSetupLocalConfig {
        version: LOCAL_SETUP_CONFIG_VERSION,
        setup_id: invitation.setup_id,
        local_role,
        database_path: database_path.to_path_buf(),
        secrets_path: secrets_path.clone(),
        url_policy,
        coordinator: None,
    };
    if config_path.exists() {
        let existing: FederationSetupLocalConfig =
            read_private_json_file(config_path, "federation setup config")?;
        if !local_config_matches(&existing, &config) {
            bail!("existing local federation setup config conflicts with join arguments");
        }
    }
    let secrets = match secrets_path.exists() {
        true => read_private_json_file::<FederationSetupLocalSecrets>(
            &secrets_path,
            "federation setup secrets",
        )?,
        false => {
            let identity_secret_key = SecretKey::generate();
            let enrollment = FederationSetupEnrollment::sign(
                invitation.setup_id,
                invitation.invitation_id,
                invitation.member_id,
                public_mint_url.clone(),
                federation_api_url.clone(),
                random_bytes(),
                FederationSetupProtocolVersions::default(),
                &identity_secret_key,
                url_policy,
            )
            .context("failed to create joining member enrollment")?;
            let secrets = FederationSetupLocalSecrets {
                version: LOCAL_SETUP_CONFIG_VERSION,
                setup_id: invitation.setup_id,
                member_id: invitation.member_id,
                identity_secret_key,
                coordinator_secret_key: None,
                invitation: Some(invitation.clone()),
                enrollment,
            };
            write_private_json_file(&secrets_path, &secrets, false)
                .context("failed to persist protected joining identity")?;
            secrets
        }
    };
    if !config_path.exists() {
        write_private_json_file(config_path, &config, false)
            .context("failed to persist local federation setup config")?;
    }
    config.validate()?;
    secrets.validate(&config, now_unix)?;
    if secrets.invitation.as_ref() != Some(&invitation)
        || secrets.enrollment.public_mint_url.to_string() != public_mint_url.to_string()
        || secrets.enrollment.federation_api_url.to_string() != federation_api_url.to_string()
    {
        bail!("join retry does not match the protected identity and enrollment");
    }

    persist_pending_join(db, &config, &secrets, now_unix).await?;
    let accepted = coordinator
        .enroll(&invitation, &secrets.enrollment)
        .await
        .context("coordinator enrollment failed; retry reuses the protected enrollment")?;
    if accepted != secrets.enrollment {
        bail!("coordinator acknowledged a different federation setup enrollment");
    }
    mark_local_enrollment_accepted(db, &config, &secrets, now_unix).await?;
    Ok(secrets.enrollment)
}

pub(crate) async fn propose_setup<DB>(
    db: &DB,
    config_path: &Path,
    now_unix: u64,
) -> Result<FederationSetupProposal>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
{
    let (config, secrets) = load_local_state(config_path, now_unix)?;
    let draft = config
        .coordinator
        .as_ref()
        .ok_or_else(|| anyhow!("only the setup coordinator can publish a proposal"))?;
    if let Some(proposal) = db
        .get_federation_setup_proposal(config.setup_id)
        .await
        .context("failed to load an existing setup proposal")?
    {
        validate_proposal_for_local_state(&proposal, &config, &secrets)?;
        return Ok(proposal);
    }
    let enrollments = db
        .get_federation_setup_enrollments(config.setup_id)
        .await
        .context("failed to load federation setup enrollments")?;
    if enrollments.len() != usize::from(draft.threshold_params.member_count) {
        bail!(
            "cannot publish proposal: accepted {} of {} enrollments",
            enrollments.len(),
            draft.threshold_params.member_count
        );
    }
    let enrollments = enrollments
        .into_iter()
        .map(|enrollment| (enrollment.member_id, enrollment))
        .collect();
    let body = FederationSetupProposalBody::new(
        config.setup_id,
        secrets.coordinator_identity()?,
        enrollments,
        draft.threshold_params,
        draft.required_versions,
        draft.network,
        draft.payment_observation_policy.clone(),
        draft.checkpoint_policy,
        draft.keysets.clone(),
        draft.frost.clone(),
        config.url_policy,
    )
    .context("invalid complete federation setup proposal")?;
    let coordinator_secret = secrets
        .coordinator_secret_key
        .as_ref()
        .ok_or_else(|| anyhow!("protected coordinator identity is missing"))?;
    let proposal = FederationSetupProposal::sign(body, coordinator_secret, config.url_policy)
        .context("failed to sign federation setup proposal")?;
    let session = require_local_session(db, &config).await?;
    let published = session
        .with_coordinator_phase(
            cdk_common::federation::FederationSetupCoordinatorPhase::ProposalPublished,
            now_unix,
        )
        .context("proposal publication is not legal in the current setup phase")?;
    let mut tx = Database::begin_transaction(db)
        .await
        .context("failed to begin proposal publication transaction")?;
    tx.publish_federation_setup_proposal(&session, &published, &proposal)
        .await
        .context("failed to persist federation setup proposal")?;
    tx.commit()
        .await
        .context("failed to commit federation setup proposal")?;
    Ok(proposal)
}

pub(crate) async fn review_local_proposal<DB>(
    db: &DB,
    config_path: &Path,
    proposal: FederationSetupProposal,
    now_unix: u64,
) -> Result<FederationSetupProposalReview>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
{
    let (config, secrets) = load_local_state(config_path, now_unix)?;
    validate_proposal_for_local_state(&proposal, &config, &secrets)?;
    let session = require_local_session(db, &config).await?;
    match session.participant_phase {
        FederationSetupParticipantPhase::Enrolled => {
            let reviewed = session
                .with_participant_phase(FederationSetupParticipantPhase::ProposalReviewed, now_unix)
                .context("proposal review is not legal in the current setup phase")?;
            let mut tx = Database::begin_transaction(db)
                .await
                .context("failed to begin proposal review transaction")?;
            tx.review_federation_setup_proposal(&session, &reviewed, &proposal)
                .await
                .context("failed to persist reviewed setup proposal")?;
            tx.commit()
                .await
                .context("failed to commit reviewed setup proposal")?;
        }
        FederationSetupParticipantPhase::ProposalReviewed
        | FederationSetupParticipantPhase::Approved
        | FederationSetupParticipantPhase::Certified
        | FederationSetupParticipantPhase::DkgRunning
        | FederationSetupParticipantPhase::ReadyToActivate
        | FederationSetupParticipantPhase::Activated => {
            let existing = db
                .get_federation_setup_proposal(config.setup_id)
                .await
                .context("failed to reload reviewed proposal")?;
            if existing.as_ref() != Some(&proposal) {
                bail!("local setup is already bound to a different reviewed proposal");
            }
        }
        _ => bail!("proposal cannot be reviewed in the current local setup phase"),
    }
    FederationSetupProposalReview::from_proposal(&proposal, config.url_policy)
        .context("failed to render federation setup proposal review")
}

#[allow(dead_code)] // Wired to the public CLI when the bounded M4 client lands.
pub(crate) async fn fetch_and_review_proposal<DB, C>(
    db: &DB,
    coordinator: &C,
    config_path: &Path,
    now_unix: u64,
) -> Result<FederationSetupProposalReview>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
    C: FederationSetupCoordinator,
{
    let (_, secrets) = load_local_state(config_path, now_unix)?;
    let invitation = secrets
        .invitation
        .as_ref()
        .ok_or_else(|| anyhow!("joining member invitation is missing"))?;
    let proposal = coordinator
        .proposal(invitation)
        .await
        .context("failed to fetch the coordinator proposal")?;
    review_local_proposal(db, config_path, proposal, now_unix).await
}

pub(crate) async fn approve_setup<DB, C>(
    db: &DB,
    coordinator: &C,
    config_path: &Path,
    exact_proposal_hash: FederationSetupProposalHash,
    now_unix: u64,
) -> Result<cdk_common::federation::FederationSetupApproval>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
    C: FederationSetupCoordinator,
{
    let (config, secrets) = load_local_state(config_path, now_unix)?;
    let proposal = db
        .get_federation_setup_proposal(config.setup_id)
        .await
        .context("failed to load reviewed setup proposal")?
        .ok_or_else(|| anyhow!("no reviewed federation setup proposal is available"))?;
    validate_proposal_for_local_state(&proposal, &config, &secrets)?;
    if proposal.proposal_hash != exact_proposal_hash {
        bail!("exact proposal hash does not match the reviewed federation setup proposal");
    }
    let member_id = config.local_role.member_id();
    let approval = match db
        .get_federation_setup_approval_fence(config.setup_id, member_id)
        .await
        .context("failed to load the local setup approval fence")?
    {
        Some(approval) => {
            approval
                .validate(&proposal)
                .context("persisted setup approval fence is invalid")?;
            approval
        }
        None => {
            let approval = cdk_common::federation::FederationSetupApproval::sign(
                &proposal,
                member_id,
                &secrets.identity_secret_key,
                config.url_policy,
            )
            .context("failed to sign federation setup approval")?;
            let session = require_local_session(db, &config).await?;
            let approved = session
                .with_participant_phase(FederationSetupParticipantPhase::Approved, now_unix)
                .context("approval is not legal in the current setup phase")?;
            let mut tx = Database::begin_transaction(db)
                .await
                .context("failed to begin setup approval transaction")?;
            tx.fence_federation_setup_approval(&session, &approved, &proposal, &approval)
                .await
                .context("failed to persist setup approval fence")?;
            tx.commit()
                .await
                .context("failed to commit setup approval fence")?;
            approval
        }
    };
    let Some(invitation) = secrets.invitation.as_ref() else {
        let mut tx = Database::begin_transaction(db)
            .await
            .context("failed to begin local coordinator approval transaction")?;
        tx.add_federation_setup_approval(&approval)
            .await
            .context("failed to persist local coordinator approval")?;
        tx.commit()
            .await
            .context("failed to commit local coordinator approval")?;
        return Ok(approval);
    };
    let acknowledged = coordinator
        .approve(invitation, &approval)
        .await
        .context("coordinator approval submission failed; retry reuses the durable approval")?;
    if acknowledged != approval {
        bail!("coordinator acknowledged a different federation setup approval");
    }
    Ok(approval)
}

pub(crate) async fn issue_invitation<DB>(
    db: &DB,
    config_path: &Path,
    member_id: MemberId,
    replace: bool,
    output_path: &Path,
    now_unix: u64,
) -> Result<FederationSetupInvitation>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
{
    let (config, secrets) = load_local_state(config_path, now_unix)?;
    let draft = config
        .coordinator
        .as_ref()
        .ok_or_else(|| anyhow!("only the setup coordinator can issue invitations"))?;
    if member_id.get() <= 1 || member_id.get() > draft.threshold_params.member_count {
        bail!("invitation member is outside the unclaimed setup slots");
    }
    let invitations = db
        .get_federation_setup_invitations(config.setup_id)
        .await
        .context("failed to load setup invitations")?;
    let existing_for_member = invitations
        .iter()
        .filter(|invitation| invitation.member_id == member_id)
        .collect::<Vec<_>>();
    if existing_for_member
        .iter()
        .any(|invitation| invitation.state == FederationSetupInvitationState::Consumed)
    {
        bail!("an accepted enrollment prevents invitation replacement for this member");
    }
    let active = existing_for_member
        .iter()
        .find(|invitation| invitation.state == FederationSetupInvitationState::Available)
        .copied();
    if active.is_some() && !replace {
        bail!("member already has an available invitation; pass --replace explicitly");
    }
    let invitation = match output_path.exists() {
        true => {
            let encoded = read_private_text_file(output_path, "setup invitation")?;
            FederationSetupInvitation::from_str(encoded.trim())
                .context("protected setup invitation output is invalid")?
        }
        false => {
            let invitation = new_invitation(
                config.setup_id,
                member_id,
                draft,
                secrets.coordinator_identity()?,
                now_unix,
            )?;
            write_private_text_file(output_path, &format!("{invitation}\n"), false)
                .context("failed to write protected setup invitation")?;
            invitation
        }
    };
    invitation
        .validate(now_unix, config.url_policy)
        .context("replacement invitation is invalid")?;
    if invitation.setup_id != config.setup_id || invitation.member_id != member_id {
        bail!("protected invitation output does not match the requested setup slot");
    }
    let replacement = FederationSetupInvitationRecord::from_invitation(&invitation, now_unix)
        .context("invalid durable replacement invitation")?;
    if db
        .get_federation_setup_invitation(config.setup_id, invitation.invitation_id)
        .await
        .context("failed to check setup invitation retry")?
        .as_ref()
        == Some(&replacement)
    {
        return Ok(invitation);
    }
    let mut tx = Database::begin_transaction(db)
        .await
        .context("failed to begin invitation transaction")?;
    match active {
        Some(expected) => tx
            .replace_federation_setup_invitation(expected, &replacement)
            .await
            .context("failed to replace setup invitation")?,
        None => tx
            .add_federation_setup_invitation(&replacement)
            .await
            .context("failed to persist setup invitation")?,
    }
    tx.commit()
        .await
        .context("failed to commit setup invitation")?;
    Ok(invitation)
}

pub(crate) fn read_local_config(config_path: &Path) -> Result<FederationSetupLocalConfig> {
    let config = read_private_json_file(config_path, "federation setup config")?;
    FederationSetupLocalConfig::validate(&config)?;
    Ok(config)
}

async fn persist_creator_state<'a, DB>(
    db: &DB,
    config: &FederationSetupLocalConfig,
    secrets: &FederationSetupLocalSecrets,
    invitations: impl Iterator<Item = &'a FederationSetupInvitation>,
    now_unix: u64,
) -> Result<()>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
{
    let existing_session = db
        .get_federation_setup_session(config.setup_id)
        .await
        .context("failed to load creator setup session")?;
    let initial = match &existing_session {
        Some(session) => session.clone(),
        None => FederationSetupSession::new(config.setup_id, config.local_role, now_unix)
            .context("failed to construct creator setup session")?,
    };
    let mut invitation_records = Vec::new();
    for invitation in invitations {
        let validation_time = invitation_validation_time(invitation, now_unix);
        let record = FederationSetupInvitationRecord::from_invitation(invitation, validation_time)
            .context("invalid durable invitation")?;
        match db
            .get_federation_setup_invitation(config.setup_id, record.invitation_id)
            .await
            .context("failed to load setup invitation")?
        {
            Some(existing) if invitation_record_matches_immutable(&existing, &record) => {}
            Some(_) => bail!("existing setup invitation conflicts with protected output"),
            None => invitation_records.push(record),
        }
    }
    let mut tx = Database::begin_transaction(db)
        .await
        .context("failed to begin creator setup transaction")?;
    if existing_session.is_none() {
        tx.add_federation_setup_session(&initial)
            .await
            .context("failed to persist creator setup session")?;
    }
    match existing_session.as_ref().map_or(
        FederationSetupParticipantPhase::IdentityCreated,
        |session| session.participant_phase,
    ) {
        FederationSetupParticipantPhase::IdentityCreated => {
            let enrolled = initial
                .with_participant_phase(FederationSetupParticipantPhase::Enrolled, now_unix)
                .context("failed to construct creator enrollment transition")?;
            tx.record_federation_setup_enrollment(&initial, &enrolled, &secrets.enrollment)
                .await
                .context("failed to persist creator enrollment")?;
        }
        _ => {
            tx.add_federation_setup_enrollment(&secrets.enrollment)
                .await
                .context("creator enrollment retry conflicted")?;
        }
    }
    for record in &invitation_records {
        tx.add_federation_setup_invitation(record)
            .await
            .context("failed to persist setup invitation")?;
    }
    tx.commit()
        .await
        .context("failed to commit creator setup state")
}

#[allow(dead_code)] // Reachable through the M3 join application boundary.
async fn persist_pending_join<DB>(
    db: &DB,
    config: &FederationSetupLocalConfig,
    secrets: &FederationSetupLocalSecrets,
    now_unix: u64,
) -> Result<()>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
{
    let session = db
        .get_federation_setup_session(config.setup_id)
        .await
        .context("failed to load joining setup session")?;
    let mut tx = Database::begin_transaction(db)
        .await
        .context("failed to begin joining setup transaction")?;
    if session.is_none() {
        let initial = FederationSetupSession::new(config.setup_id, config.local_role, now_unix)
            .context("failed to construct joining setup session")?;
        tx.add_federation_setup_session(&initial)
            .await
            .context("failed to persist joining setup session")?;
    }
    tx.add_federation_setup_enrollment(&secrets.enrollment)
        .await
        .context("failed to persist exact pending enrollment")?;
    tx.commit()
        .await
        .context("failed to commit pending joining enrollment")
}

#[allow(dead_code)] // Reachable through the M3 join application boundary.
async fn mark_local_enrollment_accepted<DB>(
    db: &DB,
    config: &FederationSetupLocalConfig,
    secrets: &FederationSetupLocalSecrets,
    now_unix: u64,
) -> Result<()>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
{
    let session = require_local_session(db, config).await?;
    if session.participant_phase != FederationSetupParticipantPhase::IdentityCreated {
        return Ok(());
    }
    let enrolled = session
        .with_participant_phase(FederationSetupParticipantPhase::Enrolled, now_unix)
        .context("failed to construct accepted enrollment transition")?;
    let mut tx = Database::begin_transaction(db)
        .await
        .context("failed to begin accepted enrollment transaction")?;
    tx.record_federation_setup_enrollment(&session, &enrolled, &secrets.enrollment)
        .await
        .context("failed to persist accepted enrollment transition")?;
    tx.commit()
        .await
        .context("failed to commit accepted enrollment transition")
}

async fn require_local_session<DB>(
    db: &DB,
    config: &FederationSetupLocalConfig,
) -> Result<FederationSetupSession>
where
    DB: Database<DatabaseError> + Send + Sync + 'static,
{
    let session = db
        .get_federation_setup_session(config.setup_id)
        .await
        .context("failed to load local federation setup session")?
        .ok_or_else(|| anyhow!("local federation setup session is missing"))?;
    session
        .validate()
        .context("local federation setup session is invalid")?;
    if session.local_role != config.local_role {
        bail!("local federation setup session role conflicts with its config");
    }
    Ok(session)
}

fn load_local_state(
    config_path: &Path,
    now_unix: u64,
) -> Result<(FederationSetupLocalConfig, FederationSetupLocalSecrets)> {
    let config = read_local_config(config_path)?;
    let secrets: FederationSetupLocalSecrets =
        read_private_json_file(&config.secrets_path, "federation setup secrets")?;
    secrets.validate(&config, now_unix)?;
    Ok((config, secrets))
}

fn validate_proposal_for_local_state(
    proposal: &FederationSetupProposal,
    config: &FederationSetupLocalConfig,
    secrets: &FederationSetupLocalSecrets,
) -> Result<()> {
    proposal
        .validate(config.url_policy)
        .context("invalid federation setup proposal")?;
    if proposal.body.setup_id != config.setup_id
        || !proposal
            .body
            .enrollments
            .get(&config.local_role.member_id())
            .is_some_and(|enrollment| enrollment == &secrets.enrollment)
    {
        bail!("federation setup proposal does not contain the exact local enrollment");
    }
    let expected_coordinator = match &secrets.invitation {
        Some(invitation) => invitation.coordinator_identity,
        None => secrets.coordinator_identity()?,
    };
    if proposal.body.coordinator_identity != expected_coordinator {
        bail!("federation setup proposal was signed by another coordinator identity");
    }
    Ok(())
}

fn load_or_create_invitation(
    path: &Path,
    setup_id: FederationSetupId,
    member_id: MemberId,
    draft: &FederationSetupCoordinatorDraft,
    coordinator_identity: FederationSetupCoordinatorPublicKey,
    now_unix: u64,
) -> Result<FederationSetupInvitation> {
    let invitation = match path.exists() {
        true => {
            let encoded = read_private_text_file(path, "setup invitation")?;
            FederationSetupInvitation::from_str(encoded.trim())
                .context("protected setup invitation is invalid")?
        }
        false => {
            let invitation =
                new_invitation(setup_id, member_id, draft, coordinator_identity, now_unix)?;
            write_private_text_file(path, &format!("{invitation}\n"), false)
                .context("failed to persist protected setup invitation")?;
            invitation
        }
    };
    invitation
        .validate(
            invitation_validation_time(&invitation, now_unix),
            url_policy_for_url(&draft.coordinator_url),
        )
        .context("protected setup invitation failed validation")?;
    if invitation.setup_id != setup_id
        || invitation.member_id != member_id
        || invitation.coordinator_url != draft.coordinator_url
        || invitation.coordinator_identity != coordinator_identity
    {
        bail!("protected setup invitation does not match the creator setup");
    }
    Ok(invitation)
}

fn new_invitation(
    setup_id: FederationSetupId,
    member_id: MemberId,
    draft: &FederationSetupCoordinatorDraft,
    coordinator_identity: FederationSetupCoordinatorPublicKey,
    now_unix: u64,
) -> Result<FederationSetupInvitation> {
    let expires_at_unix = now_unix
        .checked_add(draft.invitation_lifetime_secs)
        .ok_or_else(|| anyhow!("setup invitation expiry overflow"))?;
    FederationSetupInvitation::new(
        setup_id,
        random_invitation_id(),
        member_id,
        draft.coordinator_url.clone(),
        coordinator_identity,
        expires_at_unix,
        FederationSetupInvitationCapability::from_bytes(random_bytes())
            .context("failed to generate invitation capability")?,
        now_unix,
        url_policy_for_url(&draft.coordinator_url),
    )
    .context("failed to create federation setup invitation")
}

fn local_config_matches(
    existing: &FederationSetupLocalConfig,
    expected: &FederationSetupLocalConfig,
) -> bool {
    existing.version == expected.version
        && existing.setup_id == expected.setup_id
        && existing.local_role == expected.local_role
        && existing.database_path == expected.database_path
        && existing.secrets_path == expected.secrets_path
        && existing.url_policy == expected.url_policy
        && existing.coordinator == expected.coordinator
}

fn invitation_record_matches_immutable(
    existing: &FederationSetupInvitationRecord,
    expected: &FederationSetupInvitationRecord,
) -> bool {
    existing.version == expected.version
        && existing.setup_id == expected.setup_id
        && existing.invitation_id == expected.invitation_id
        && existing.member_id == expected.member_id
        && existing.expires_at_unix == expected.expires_at_unix
        && existing.capability_hash == expected.capability_hash
}

fn validate_setup_url(url: &MintUrl, policy: FederationSetupUrlPolicy) -> Result<()> {
    let parsed = url::Url::parse(&url.to_string()).context("invalid setup URL")?;
    match policy {
        FederationSetupUrlPolicy::RequireHttps if parsed.scheme() != "https" => {
            bail!("production setup URLs must use HTTPS")
        }
        FederationSetupUrlPolicy::AllowInsecureLocalhost
            if parsed.scheme() == "http"
                && !parsed.host().is_some_and(|host| match host {
                    url::Host::Ipv4(address) => address.is_loopback(),
                    url::Host::Ipv6(address) => address.is_loopback(),
                    url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
                }) =>
        {
            bail!("insecure setup URLs are allowed only for localhost")
        }
        FederationSetupUrlPolicy::AllowInsecureLocalhost if parsed.scheme() == "http" => {}
        _ if parsed.scheme() != "https" => bail!("unsupported setup URL scheme"),
        _ => {}
    }
    Ok(())
}

fn url_policy_for_url(url: &MintUrl) -> FederationSetupUrlPolicy {
    match url.to_string().starts_with("http://") {
        true => FederationSetupUrlPolicy::AllowInsecureLocalhost,
        false => FederationSetupUrlPolicy::RequireHttps,
    }
}

fn invitation_validation_time(invitation: &FederationSetupInvitation, now_unix: u64) -> u64 {
    now_unix.min(invitation.expires_at_unix.saturating_sub(1))
}

fn random_setup_id() -> FederationSetupId {
    FederationSetupId::from_bytes(random_bytes())
}

fn random_invitation_id() -> FederationSetupInvitationId {
    FederationSetupInvitationId::from_bytes(random_bytes())
}

fn random_bytes() -> [u8; 32] {
    SecretKey::generate().to_secret_bytes()
}

pub(crate) fn current_unix_time() -> Result<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system time predates the Unix epoch")
        .map(|duration| duration.as_secs())
}

#[cfg(all(test, feature = "sqlite"))]
mod tests;
