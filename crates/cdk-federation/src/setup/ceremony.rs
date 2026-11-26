//! Each operator's local side of the setup ceremony.
//!
//! The creator runs [`create_setup`], issues invitations and proposes; every
//! member joins with [`join_setup`], reviews and approves the exact proposal.
//! Once the coordinator certifies the manifest, [`load_certified_dkg_handoff`]
//! yields the inputs of the member's DKG run and [`activate_setup`] publishes
//! the final config pair. Protected state lives in files the local setup config
//! names and in the member database the caller opens.

use core::fmt;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use cdk_common::database::mint::Database;
use cdk_common::database::{DynMintDatabase, Error as DatabaseError, MintKeysDatabase};
use cdk_common::federation::{
    FederationCheckpointPolicy, FederationConfig, FederationConfigDigest,
    FederationDkgCeremonyRecord, FederationDkgCeremonyState, FederationDkgFinalizedMemberConfig,
    FederationDkgKeysetSpec, FederationPaymentObservationPolicy, FederationSetupActivation,
    FederationSetupBitcoinNetwork, FederationSetupCoordinatorPhase,
    FederationSetupCoordinatorPublicKey, FederationSetupEnrollment, FederationSetupFrostPolicy,
    FederationSetupId, FederationSetupInvitation, FederationSetupInvitationCapability,
    FederationSetupInvitationId, FederationSetupInvitationRecord, FederationSetupInvitationState,
    FederationSetupLocalRole, FederationSetupManifest, FederationSetupParticipantPhase,
    FederationSetupProposal, FederationSetupProposalBody, FederationSetupProposalHash,
    FederationSetupProposalReview, FederationSetupProtocolVersions, FederationSetupSession,
    FederationSetupStatus, FederationSetupUrlPolicy, MemberConfigPrivate, MemberId,
    ThresholdParams, FEDERATION_SETUP_MAX_INVITATION_LIFETIME_SECS,
    FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::SecretKey;
use serde::{Deserialize, Serialize};

use super::files::{
    read_federation_json, read_private_json_file, read_private_text_file, write_private_json_file,
    write_private_text_file,
};
use super::finalize::{
    attach_manifest_frost_applications, write_final_config_file, FederationDkgCompletionRecords,
    FederationSetupDkgProgress, FINAL_PRIVATE_CONFIG_FILE, FINAL_PUBLIC_CONFIG_FILE,
};
use super::{
    FederationSetupCoordinator, FederationSetupCoordinatorService,
    FederationSetupCoordinatorServiceConfig, FederationSetupHttpClient,
};
use crate::FederationDkgAutoStartConfig;

const LOCAL_SETUP_CONFIG_VERSION: u16 = 3;
const LOCAL_SETUP_SECRETS_FILE: &str = "federation-setup-secrets.json";
const LOCAL_SETUP_MANIFEST_FILE: &str = "federation-setup-manifest.json";
const LOCAL_SETUP_INVITATION_DIRECTORY: &str = "invitations";
const LOCAL_SETUP_ACTIVATION_DIRECTORY: &str = "activated";

static FINAL_CONFIG_EXPORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Build the setup-only mint a certified member runs its DKG with.
///
/// `keystore` must be an isolated, empty key store: the setup-only signatory's
/// keys are never used and must not reach the member database.
pub async fn build_certified_dkg_mint(
    database: DynMintDatabase,
    #[cfg(feature = "federation-bark")] bark_store: cdk_federation_bark::DynFederationBarkStore,
    keystore: Arc<dyn MintKeysDatabase<Err = DatabaseError> + Send + Sync>,
    handoff: &FederationSetupDkgHandoff,
) -> Result<cdk::mint::Mint> {
    let runtime_config = cdk::mint::FederationRuntimeConfig::new(
        handoff.public_config.clone(),
        handoff.private_config.clone(),
    )
    .context("invalid certified DKG runtime config")?;
    let [keyset_spec] = handoff.manifest.keyset_specs() else {
        bail!("setup-only DKG requires exactly one manifest keyset policy");
    };
    #[allow(unused_mut)]
    let mut builder = cdk::mint::MintBuilder::new(database).with_federation_config(runtime_config);
    #[cfg(feature = "federation-bark")]
    builder.set_federation_bark_store(bark_store);
    builder
        .configure_unit(
            keyset_spec.unit.clone(),
            cdk::mint::UnitConfig {
                amounts: keyset_spec.amounts.iter().copied().map(u64::from).collect(),
                input_fee_ppk: keyset_spec.input_fee_ppk,
            },
        )
        .context("failed to configure isolated setup-only signatory")?;
    builder
        .build_with_seed(keystore, &handoff.runtime_seed)
        .await
        .context("failed to build setup-only DKG member")
}

/// Record the manifest's DKG ceremony as active and move the local setup to
/// `DkgRunning`; resuming the same ceremony is accepted.
pub async fn register_manifest_dkg_ceremony(
    database: &DynMintDatabase,
    handoff: &FederationSetupDkgHandoff,
    auto_start: &FederationDkgAutoStartConfig,
) -> Result<()> {
    let desired = FederationDkgCeremonyRecord::active(
        handoff.public_config.federation_id,
        handoff.public_config.setup_authorization,
        auto_start.ceremony_id(),
        handoff.private_config.member_id,
        auto_start.keyset_spec().clone(),
        None,
    );
    let records = database
        .get_federation_dkg_ceremonies()
        .await
        .context("failed to read certified DKG lifecycle")?;
    if let Some(existing) = records
        .iter()
        .find(|record| record.ceremony_id == desired.ceremony_id)
    {
        let expected = match existing.state {
            FederationDkgCeremonyState::Active => desired,
            FederationDkgCeremonyState::Complete => desired.completed()?,
            FederationDkgCeremonyState::Aborted => {
                bail!(
                    "certified DKG ceremony {} was durably aborted and cannot be resumed",
                    existing.ceremony_id
                )
            }
        };
        if existing != &expected {
            bail!(
                "certified DKG lifecycle does not match the local manifest, identity, or keyset policy"
            );
        }
        return ensure_setup_dkg_running(database, handoff.manifest.proposal.body.setup_id).await;
    }
    if !records.is_empty() {
        bail!(
            "local DKG lifecycle belongs to a different certified manifest; start the new setup with independent state"
        );
    }
    let session = database
        .get_federation_setup_session(handoff.manifest.proposal.body.setup_id)
        .await
        .context("failed to read certified setup lifecycle")?
        .ok_or_else(|| anyhow!("certified setup lifecycle is missing"))?;
    if session.participant_phase != FederationSetupParticipantPhase::Certified {
        bail!("new certified DKG can start only from the certified setup phase");
    }
    let running = session
        .with_participant_phase(
            FederationSetupParticipantPhase::DkgRunning,
            current_unix_time()?,
        )
        .context("failed to start local DKG lifecycle")?;
    let mut transaction = database
        .begin_transaction()
        .await
        .context("failed to begin certified DKG lifecycle transaction")?;
    transaction
        .add_federation_dkg_ceremony(&desired)
        .await
        .context("failed to register certified DKG lifecycle")?;
    transaction
        .transition_federation_setup_session(&session, &running)
        .await
        .context("failed to persist local DKG lifecycle")?;
    transaction
        .commit()
        .await
        .context("failed to commit certified DKG lifecycle")
}

async fn ensure_setup_dkg_running(
    database: &DynMintDatabase,
    setup_id: FederationSetupId,
) -> Result<()> {
    let session = database
        .get_federation_setup_session(setup_id)
        .await
        .context("failed to read resumed setup lifecycle")?
        .ok_or_else(|| anyhow!("resumed setup lifecycle is missing"))?;
    match session.participant_phase {
        FederationSetupParticipantPhase::Certified => {
            let running = session
                .with_participant_phase(
                    FederationSetupParticipantPhase::DkgRunning,
                    current_unix_time()?,
                )
                .context("failed to resume local DKG lifecycle")?;
            let mut transaction = database
                .begin_transaction()
                .await
                .context("failed to begin resumed DKG lifecycle transaction")?;
            transaction
                .transition_federation_setup_session(&session, &running)
                .await
                .context("failed to persist resumed DKG lifecycle")?;
            transaction
                .commit()
                .await
                .context("failed to commit resumed DKG lifecycle")
        }
        FederationSetupParticipantPhase::DkgRunning
        | FederationSetupParticipantPhase::ReadyToActivate
        | FederationSetupParticipantPhase::Activated => Ok(()),
        _ => bail!("persisted DKG lifecycle conflicts with the local setup phase"),
    }
}

/// Local setup progress. A member fetches authenticated progress from the
/// coordinator and stores the certified manifest once it exists; the creator
/// reads its own coordinator state.
pub async fn setup_status(
    database: &DynMintDatabase,
    config_path: &Path,
    client: &FederationSetupHttpClient,
) -> Result<FederationSetupLocalStatus> {
    let now_unix = current_unix_time()?;
    let (config, secrets) = load_local_state(config_path, now_unix)?;
    let manifest = database
        .get_federation_setup_manifest(config.setup_id)
        .await
        .context("failed to load the local certified setup manifest")?;
    let bootstrap_status = if let Some(manifest) = manifest.as_ref() {
        persist_local_manifest(database.as_ref(), &config, &secrets, manifest, now_unix).await?;
        None
    } else {
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
                let service = FederationSetupCoordinatorService::new(
                    database.clone(),
                    config.setup_id,
                    draft.threshold_params.member_count,
                    coordinator_secret_key,
                    config.url_policy,
                    FederationSetupCoordinatorServiceConfig::default(),
                )
                .context("failed to initialize local setup status reader")?;
                Some(
                    service
                        .local_status()
                        .await
                        .context("failed to read local setup status")?,
                )
            }
            FederationSetupLocalRole::Member { .. } => {
                let invitation = secrets
                    .invitation
                    .as_ref()
                    .ok_or_else(|| anyhow!("protected joining invitation is missing"))?;
                let status = client
                    .status(invitation)
                    .await
                    .context("failed to fetch authenticated setup status")?;
                if status.manifest_hash.is_some() {
                    let manifest = client
                        .manifest(invitation)
                        .await
                        .context("failed to fetch authenticated certified manifest")?;
                    persist_local_manifest(
                        database.as_ref(),
                        &config,
                        &secrets,
                        &manifest,
                        now_unix,
                    )
                    .await?;
                }
                Some(status)
            }
        }
    };
    build_setup_command_status(database, &config, bootstrap_status, now_unix).await
}

/// Stable, secret-free view of one operator's setup progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FederationSetupLocalStatus {
    /// Format version of this view.
    pub version: u16,
    /// The setup.
    pub setup_id: FederationSetupId,
    /// This operator's member slot.
    pub member_id: MemberId,
    /// Label of this operator's lifecycle phase.
    pub participant_phase: String,
    /// Label of the coordinator lifecycle phase, on the creator.
    pub coordinator_phase: Option<String>,
    /// Number of members.
    pub member_count: u16,
    /// Members that enrolled.
    pub enrolled_members: Vec<MemberId>,
    /// Members that have not enrolled.
    pub missing_enrollments: Vec<MemberId>,
    /// Members that approved the proposal.
    pub approved_members: Vec<MemberId>,
    /// Members that have not approved.
    pub missing_approvals: Vec<MemberId>,
    /// Hash of the published proposal.
    pub proposal_hash: Option<String>,
    /// Hash of the certified manifest.
    pub manifest_hash: Option<String>,
    /// DKG progress once the manifest is certified.
    pub dkg: Option<FederationSetupDkgProgress>,
    /// The local activation, once activated.
    pub activation: Option<FederationSetupActivation>,
}

async fn build_setup_command_status(
    database: &DynMintDatabase,
    config: &FederationSetupLocalConfig,
    bootstrap_status: Option<FederationSetupStatus>,
    now_unix: u64,
) -> Result<FederationSetupLocalStatus> {
    let manifest = database
        .get_federation_setup_manifest(config.setup_id)
        .await
        .context("failed to read certified manifest for local status")?;
    let (member_count, enrolled_members, approved_members, proposal_hash, manifest_hash) =
        match (manifest.as_ref(), bootstrap_status) {
            (Some(manifest), _) => (
                manifest.proposal.body.threshold_params.member_count,
                manifest.proposal.body.enrollments.keys().copied().collect(),
                manifest.approvals.keys().copied().collect(),
                Some(manifest.proposal.proposal_hash.to_string()),
                Some(manifest.manifest_hash.to_string()),
            ),
            (None, Some(status)) => (
                status.member_count,
                status.enrolled_members,
                status.approved_members,
                status.proposal_hash.map(|hash| hash.to_string()),
                status.manifest_hash.map(|hash| hash.to_string()),
            ),
            (None, None) => bail!("setup status has neither bootstrap status nor a manifest"),
        };
    let dkg = match manifest.as_ref() {
        Some(_) => {
            let handoff = load_certified_dkg_handoff_from_state(config, current_unix_time()?)?;
            let records = FederationDkgCompletionRecords::read(database).await?;
            let (mut progress, finalized) =
                records.finalization_progress(&handoff.public_config, &handoff.private_config);
            if let Some(mut finalized) = finalized {
                attach_manifest_frost_applications(
                    &mut finalized,
                    records.frost_roots()?,
                    &handoff.manifest,
                )?;
                validate_manifest_finalized_config(&handoff, &finalized)?;
                progress.final_config_digest = Some(
                    finalized
                        .public_config
                        .config_digest_for_validated_config()?
                        .to_string(),
                );
            }
            Some(progress)
        }
        None => None,
    };
    let mut session = require_local_session(database.as_ref(), config).await?;
    if dkg
        .as_ref()
        .is_some_and(|progress| progress.finalized_ready)
        && session.participant_phase == FederationSetupParticipantPhase::DkgRunning
    {
        let ready = session
            .with_participant_phase(FederationSetupParticipantPhase::ReadyToActivate, now_unix)
            .context("failed to advance completed DKG lifecycle")?;
        let mut transaction = Database::begin_transaction(database.as_ref())
            .await
            .context("failed to begin DKG-ready setup transaction")?;
        transaction
            .transition_federation_setup_session(&session, &ready)
            .await
            .context("failed to persist DKG-ready setup phase")?;
        transaction
            .commit()
            .await
            .context("failed to commit DKG-ready setup phase")?;
        session = ready;
    }
    let activation = database
        .get_federation_setup_activations(config.setup_id)
        .await
        .context("failed to read local setup activation")?
        .into_iter()
        .find(|activation| activation.member_id == config.local_role.member_id());
    let all_members = (1..=member_count)
        .map(|member| MemberId::new(member).context("invalid setup status member ID"))
        .collect::<Result<Vec<_>>>()?;
    let missing_enrollments = all_members
        .iter()
        .filter(|member| !enrolled_members.contains(member))
        .copied()
        .collect();
    let missing_approvals = all_members
        .iter()
        .filter(|member| !approved_members.contains(member))
        .copied()
        .collect();
    Ok(FederationSetupLocalStatus {
        version: 1,
        setup_id: config.setup_id,
        member_id: config.local_role.member_id(),
        participant_phase: participant_phase_label(session.participant_phase).to_string(),
        coordinator_phase: session
            .coordinator_phase
            .map(coordinator_phase_label)
            .map(str::to_string),
        member_count,
        enrolled_members,
        missing_enrollments,
        approved_members,
        missing_approvals,
        proposal_hash,
        manifest_hash,
        dkg,
        activation,
    })
}

/// A member's completed activation and its published final config pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationSetupActivationResult {
    /// The setup.
    pub setup_id: FederationSetupId,
    /// This member.
    pub member_id: MemberId,
    /// The federation the final config defines.
    pub federation_id: cdk_common::federation::FederationId,
    /// Digest of the final config.
    pub final_config_digest: FederationConfigDigest,
    /// Path of the final public config.
    pub public_config_path: PathBuf,
    /// Path of the final private config.
    pub private_config_path: PathBuf,
}

/// Test fault in the final config export.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalConfigExportFault {
    /// No fault.
    None,
    /// Fail writing the public config.
    #[cfg(any(test, feature = "test-utils"))]
    FailPublicWrite,
}

/// Test fault in the activation transaction.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupActivationFault {
    /// No fault.
    None,
    /// Roll back instead of committing the activation.
    #[cfg(any(test, feature = "test-utils"))]
    BeforeDatabaseCommit,
}

/// Publish the final config pair and record this member's activation.
///
/// Requires a completed DKG with every final-config confirmation. Repeating an
/// activation returns the recorded one.
pub async fn activate_setup(
    database: &DynMintDatabase,
    config_path: &Path,
    now_unix: u64,
) -> Result<FederationSetupActivationResult> {
    activate_setup_with_faults(
        database,
        config_path,
        now_unix,
        FinalConfigExportFault::None,
        SetupActivationFault::None,
    )
    .await
}

/// [`activate_setup`] with injected faults.
#[doc(hidden)]
pub async fn activate_setup_with_faults(
    database: &DynMintDatabase,
    config_path: &Path,
    now_unix: u64,
    export_fault: FinalConfigExportFault,
    activation_fault: SetupActivationFault,
) -> Result<FederationSetupActivationResult> {
    let config = read_local_config(config_path)?;
    let handoff = load_certified_dkg_handoff_from_state(&config, now_unix)?;
    let records = FederationDkgCompletionRecords::read(database).await?;
    let (_, finalized) =
        records.finalization_progress(&handoff.public_config, &handoff.private_config);
    let mut finalized = finalized.ok_or_else(|| anyhow!(
        "federation setup cannot activate before DKG and all final-config confirmations are complete"
    ))?;
    attach_manifest_frost_applications(&mut finalized, records.frost_roots()?, &handoff.manifest)
        .context("failed to attach the manifest-authorized FROST applications")?;
    validate_manifest_finalized_config(&handoff, &finalized)?;
    let final_config_digest = finalized
        .public_config
        .config_digest_for_validated_config()
        .context("failed to digest final federation config")?;
    let (public_config_path, private_config_path) =
        publish_final_config_pair(config.activation_dir(), &finalized, export_fault).await?;

    let existing = database
        .get_federation_setup_activations(config.setup_id)
        .await
        .context("failed to read existing local activation")?
        .into_iter()
        .find(|activation| activation.member_id == config.local_role.member_id());
    if let Some(existing) = existing {
        validate_existing_activation(&existing, &handoff, final_config_digest)?;
        let session = require_local_session(database.as_ref(), &config).await?;
        if session.participant_phase != FederationSetupParticipantPhase::Activated {
            bail!("durable setup activation conflicts with the local lifecycle");
        }
        return Ok(FederationSetupActivationResult {
            setup_id: existing.setup_id,
            member_id: existing.member_id,
            federation_id: existing.federation_id,
            final_config_digest: existing.final_config_digest,
            public_config_path,
            private_config_path,
        });
    }

    let mut session = require_local_session(database.as_ref(), &config).await?;
    if session.participant_phase == FederationSetupParticipantPhase::DkgRunning {
        let ready = session
            .with_participant_phase(FederationSetupParticipantPhase::ReadyToActivate, now_unix)
            .context("failed to advance final setup lifecycle")?;
        let mut transaction = Database::begin_transaction(database.as_ref())
            .await
            .context("failed to begin ready-to-activate transaction")?;
        transaction
            .transition_federation_setup_session(&session, &ready)
            .await
            .context("failed to persist ready-to-activate phase")?;
        transaction
            .commit()
            .await
            .context("failed to commit ready-to-activate phase")?;
        session = ready;
    }
    if session.participant_phase != FederationSetupParticipantPhase::ReadyToActivate {
        bail!("local setup lifecycle is not ready for explicit activation");
    }
    let activation = FederationSetupActivation {
        version: cdk_common::federation::FEDERATION_SETUP_PROTOCOL_VERSION,
        setup_id: config.setup_id,
        member_id: config.local_role.member_id(),
        manifest_hash: handoff.manifest.manifest_hash,
        setup_authorization: handoff.public_config.setup_authorization,
        federation_id: finalized.public_config.federation_id,
        final_config_digest,
        activated_at_unix: now_unix,
    };
    let activated = session
        .with_participant_phase(FederationSetupParticipantPhase::Activated, now_unix)
        .context("failed to construct local activation lifecycle")?;
    let mut transaction = Database::begin_transaction(database.as_ref())
        .await
        .context("failed to begin local activation transaction")?;
    transaction
        .activate_federation_setup(&session, &activated, &activation)
        .await
        .context("failed to persist local setup activation")?;
    #[cfg(any(test, feature = "test-utils"))]
    if activation_fault == SetupActivationFault::BeforeDatabaseCommit {
        transaction
            .rollback()
            .await
            .context("failed to roll back interrupted activation")?;
        bail!("simulated interruption before activation commit");
    }
    #[cfg(not(any(test, feature = "test-utils")))]
    let _ = activation_fault;
    transaction
        .commit()
        .await
        .context("failed to commit local setup activation")?;
    Ok(FederationSetupActivationResult {
        setup_id: activation.setup_id,
        member_id: activation.member_id,
        federation_id: activation.federation_id,
        final_config_digest: activation.final_config_digest,
        public_config_path,
        private_config_path,
    })
}

fn validate_manifest_finalized_config(
    handoff: &FederationSetupDkgHandoff,
    finalized: &FederationDkgFinalizedMemberConfig,
) -> Result<()> {
    finalized.public_config.validate()?;
    finalized
        .private_config
        .validate_against_public(&finalized.public_config)?;
    if finalized.private_config.member_id != handoff.private_config.member_id
        || finalized.public_config.setup_authorization != handoff.public_config.setup_authorization
    {
        bail!("final federation config does not match the certified local setup identity");
    }
    let mut setup = finalized.public_config.clone();
    setup.keysets.clear();
    setup.setup_transcript = None;
    setup.frost_wallet = None;
    if setup != handoff.public_config {
        bail!("final federation config does not extend the certified manifest config");
    }
    let transcript = finalized
        .public_config
        .setup_transcript
        .as_ref()
        .ok_or_else(|| anyhow!("final federation config is missing its DKG transcript"))?;
    let expected_ceremony = handoff
        .manifest
        .ceremony_id()
        .context("invalid manifest-derived ceremony ID")?;
    if !transcript
        .payload
        .ceremonies
        .iter()
        .any(|ceremony| ceremony.ceremony_id() == expected_ceremony)
    {
        bail!("final federation transcript omits the manifest-derived ceremony");
    }
    Ok(())
}

fn validate_existing_activation(
    activation: &FederationSetupActivation,
    handoff: &FederationSetupDkgHandoff,
    final_config_digest: FederationConfigDigest,
) -> Result<()> {
    activation.validate()?;
    if activation.setup_id != handoff.manifest.proposal.body.setup_id
        || activation.member_id != handoff.private_config.member_id
        || activation.manifest_hash != handoff.manifest.manifest_hash
        || activation.setup_authorization != handoff.public_config.setup_authorization
        || activation.federation_id != handoff.public_config.federation_id
        || activation.final_config_digest != final_config_digest
    {
        bail!("durable local activation does not match the exact finalized setup config");
    }
    Ok(())
}

/// Publish the final public and private config into `activation_dir` at once:
/// written to a staging directory, checked, then renamed. An existing
/// directory must hold exactly this config pair.
pub async fn publish_final_config_pair(
    activation_dir: &Path,
    finalized: &FederationDkgFinalizedMemberConfig,
    fault: FinalConfigExportFault,
) -> Result<(PathBuf, PathBuf)> {
    #[cfg(not(any(test, feature = "test-utils")))]
    let _ = fault;
    if activation_dir.exists() {
        validate_activation_directory(activation_dir)?;
        validate_published_final_config_pair(activation_dir, finalized)?;
        return Ok((
            activation_dir.join(FINAL_PUBLIC_CONFIG_FILE),
            activation_dir.join(FINAL_PRIVATE_CONFIG_FILE),
        ));
    }
    let parent = activation_dir
        .parent()
        .ok_or_else(|| anyhow!("activated config directory has no parent"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;
    let directory_name = activation_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("activated");
    let staging_dir = parent.join(format!(
        ".{directory_name}.{}.{}.tmp",
        std::process::id(),
        FINAL_CONFIG_EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&staging_dir)
        .with_context(|| format!("failed to create {}", staging_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(&staging_dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to protect {}", staging_dir.display()))?;
    }
    let public_path = staging_dir.join(FINAL_PUBLIC_CONFIG_FILE);
    let private_path = staging_dir.join(FINAL_PRIVATE_CONFIG_FILE);
    let public_config = finalized.public_config.clone();
    let private_config = finalized.private_config.clone();
    let public_task = tokio::task::spawn_blocking(move || {
        #[cfg(any(test, feature = "test-utils"))]
        if fault == FinalConfigExportFault::FailPublicWrite {
            bail!("simulated final public-config export failure");
        }
        write_final_config_file(&public_path, &public_config, false, false)
    });
    let private_task = tokio::task::spawn_blocking(move || {
        write_final_config_file(&private_path, &private_config, false, true)
    });
    let export_result = tokio::try_join!(public_task, private_task);
    match export_result {
        Ok((Ok(()), Ok(()))) => {}
        Ok((public, private)) => {
            let _ = std::fs::remove_dir_all(&staging_dir);
            public.context("failed to export final public config")?;
            private.context("failed to export final private config")?;
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return Err(error).context("final config export task failed");
        }
    }
    if let Err(error) = validate_published_final_config_pair(&staging_dir, finalized) {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(error).context("staged final federation config validation failed");
    }
    sync_directory(&staging_dir)?;
    match std::fs::rename(&staging_dir, activation_dir) {
        Ok(()) => sync_directory(parent)?,
        Err(error) if activation_dir.exists() => {
            let _ = std::fs::remove_dir_all(&staging_dir);
            validate_activation_directory(activation_dir)?;
            validate_published_final_config_pair(activation_dir, finalized).with_context(|| {
                format!("concurrent final config publication failed after {error}")
            })?;
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return Err(error).with_context(|| {
                format!("failed to atomically publish {}", activation_dir.display())
            });
        }
    }
    validate_activation_directory(activation_dir)?;
    validate_published_final_config_pair(activation_dir, finalized)?;
    Ok((
        activation_dir.join(FINAL_PUBLIC_CONFIG_FILE),
        activation_dir.join(FINAL_PRIVATE_CONFIG_FILE),
    ))
}

fn validate_published_final_config_pair(
    directory: &Path,
    finalized: &FederationDkgFinalizedMemberConfig,
) -> Result<()> {
    let public_path = directory.join(FINAL_PUBLIC_CONFIG_FILE);
    let private_path = directory.join(FINAL_PRIVATE_CONFIG_FILE);
    let public_metadata = std::fs::symlink_metadata(&public_path)
        .with_context(|| format!("failed to inspect {}", public_path.display()))?;
    if public_metadata.file_type().is_symlink() || !public_metadata.file_type().is_file() {
        bail!("activated public federation config must be a regular non-symlink file");
    }
    let public: FederationConfig =
        read_federation_json(&public_path, "activated public federation config")?;
    let private: MemberConfigPrivate =
        read_private_json_file(&private_path, "activated private federation config")?;
    public.validate()?;
    private.validate_against_public(&public)?;
    if public != finalized.public_config || private != finalized.private_config {
        bail!("activated federation config files do not match the exact finalized DKG output");
    }
    Ok(())
}

fn sync_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(directory)
            .with_context(|| format!("failed to open {} for sync", directory.display()))?
            .sync_all()
            .with_context(|| format!("failed to sync {}", directory.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Ok(())
    }
}

fn validate_activation_directory(directory: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(directory)
        .with_context(|| format!("failed to inspect {}", directory.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!("activated federation config path must be a regular non-symlink directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("activated federation config directory allows group or other access");
        }
    }
    Ok(())
}

async fn persist_local_manifest<DB>(
    database: &DB,
    config: &FederationSetupLocalConfig,
    secrets: &FederationSetupLocalSecrets,
    manifest: &FederationSetupManifest,
    now_unix: u64,
) -> Result<()>
where
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
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
        let session = require_local_session(database, config).await?;
        if session.participant_phase == FederationSetupParticipantPhase::Approved {
            let certified = session
                .with_participant_phase(FederationSetupParticipantPhase::Certified, now_unix)
                .context("manifest certification is not legal in the current setup phase")?;
            let mut transaction = Database::begin_transaction(database)
                .await
                .context("failed to begin local manifest replay transaction")?;
            transaction
                .store_federation_setup_manifest(&session, &certified, manifest)
                .await
                .context("failed to certify the local manifest replay")?;
            transaction
                .commit()
                .await
                .context("failed to commit the local manifest replay")?;
        } else if !matches!(
            session.participant_phase,
            FederationSetupParticipantPhase::Certified
                | FederationSetupParticipantPhase::DkgRunning
                | FederationSetupParticipantPhase::ReadyToActivate
                | FederationSetupParticipantPhase::Activated
        ) {
            bail!("certified manifest conflicts with the local setup lifecycle");
        }
        validate_and_persist_manifest_file(config, secrets, manifest)?;
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
        .context("failed to commit certified setup manifest")?;
    validate_and_persist_manifest_file(config, secrets, manifest)
}

fn validate_and_persist_manifest_file(
    config: &FederationSetupLocalConfig,
    secrets: &FederationSetupLocalSecrets,
    manifest: &FederationSetupManifest,
) -> Result<()> {
    manifest
        .validate(config.url_policy)
        .context("invalid certified setup manifest")?;
    validate_proposal_for_local_state(&manifest.proposal, config, secrets)?;
    if config.manifest_path.exists() {
        let existing: FederationSetupManifest =
            read_private_json_file(&config.manifest_path, "certified setup manifest")?;
        if existing != *manifest {
            bail!("protected setup manifest file conflicts with durable setup state");
        }
        return Ok(());
    }
    write_private_json_file(&config.manifest_path, manifest, false)
        .context("failed to persist protected certified setup manifest")
}

const fn coordinator_phase_label(phase: FederationSetupCoordinatorPhase) -> &'static str {
    use FederationSetupCoordinatorPhase;

    match phase {
        FederationSetupCoordinatorPhase::CollectingEnrollments => "collecting_enrollments",
        FederationSetupCoordinatorPhase::ProposalPublished => "proposal_published",
        FederationSetupCoordinatorPhase::Certified => "certified",
        FederationSetupCoordinatorPhase::Closed => "closed",
        FederationSetupCoordinatorPhase::Aborted => "aborted",
    }
}

const fn participant_phase_label(phase: FederationSetupParticipantPhase) -> &'static str {
    match phase {
        FederationSetupParticipantPhase::IdentityCreated => "identity_created",
        FederationSetupParticipantPhase::Enrolled => "enrolled",
        FederationSetupParticipantPhase::ProposalReviewed => "proposal_reviewed",
        FederationSetupParticipantPhase::Approved => "approved",
        FederationSetupParticipantPhase::Certified => "certified",
        FederationSetupParticipantPhase::DkgRunning => "dkg_running",
        FederationSetupParticipantPhase::ReadyToActivate => "ready_to_activate",
        FederationSetupParticipantPhase::Activated => "activated",
        FederationSetupParticipantPhase::Aborted => "aborted",
    }
}

/// Everything the creator decides when creating a setup.
#[derive(Debug, Clone)]
pub struct FederationSetupCreateSpec {
    /// Member count and thresholds.
    pub threshold_params: ThresholdParams,
    /// The creator's wallet-facing mint URL.
    pub public_mint_url: MintUrl,
    /// The creator's federation API URL.
    pub federation_api_url: MintUrl,
    /// URL of the setup coordinator.
    pub coordinator_url: MintUrl,
    /// Bitcoin network of the federation.
    pub network: FederationSetupBitcoinNetwork,
    /// Payment observation quorum.
    pub payment_observation_policy: FederationPaymentObservationPolicy,
    /// Checkpoint intervals.
    pub checkpoint_policy: FederationCheckpointPolicy,
    /// BLS keysets the DKG creates.
    pub keysets: Vec<FederationDkgKeysetSpec>,
    /// FROST root and applications.
    pub frost: FederationSetupFrostPolicy,
    /// Lifetime of the issued invitations.
    pub invitation_lifetime_secs: u64,
    /// Whether plain-HTTP localhost URLs are allowed.
    pub url_policy: FederationSetupUrlPolicy,
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

/// A created setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationSetupCreateResult {
    /// The protected local setup config.
    pub config_path: PathBuf,
    /// The new setup.
    pub setup_id: FederationSetupId,
    /// The protected invitation file of every other member.
    pub invitation_paths: BTreeMap<MemberId, PathBuf>,
}

/// One operator's protected local setup config: its role and the paths of its
/// secrets, manifest, database and activation directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupLocalConfig {
    version: u16,
    setup_id: FederationSetupId,
    local_role: FederationSetupLocalRole,
    database_path: PathBuf,
    secrets_path: PathBuf,
    manifest_path: PathBuf,
    activation_dir: PathBuf,
    url_policy: FederationSetupUrlPolicy,
    coordinator: Option<FederationSetupCoordinatorDraft>,
}

impl FederationSetupLocalConfig {
    /// The member database holding the setup lifecycle.
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// The setup.
    pub fn setup_id(&self) -> FederationSetupId {
        self.setup_id
    }

    /// This operator's role.
    pub fn local_role(&self) -> FederationSetupLocalRole {
        self.local_role
    }

    /// The protected certified manifest, present once certified.
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// Directory the final config pair is published into.
    pub fn activation_dir(&self) -> &Path {
        &self.activation_dir
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
    runtime_seed: Vec<u8>,
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
            .field("runtime_seed", &"[REDACTED]")
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
            || self.runtime_seed.len() != 64
            || self.runtime_seed.iter().all(|byte| *byte == 0)
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

#[allow(clippy::too_many_arguments)]
/// Create a setup as its coordinator: the creator's identity, the protected
/// local config and an invitation for every other member.
pub async fn create_setup<DB>(
    db: &DB,
    config_path: &Path,
    state_directory: &Path,
    database_path: &Path,
    spec: &FederationSetupCreateSpec,
    now_unix: u64,
) -> Result<FederationSetupCreateResult>
where
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
{
    spec.validate()?;
    std::fs::create_dir_all(state_directory)
        .context("failed to create federation setup state directory")?;
    let secrets_path = state_directory.join(LOCAL_SETUP_SECRETS_FILE);
    let manifest_path = state_directory.join(LOCAL_SETUP_MANIFEST_FILE);
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
                runtime_seed: random_runtime_seed(),
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
        manifest_path,
        activation_dir: state_directory.join(LOCAL_SETUP_ACTIVATION_DIRECTORY),
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
/// Join a setup with a protected invitation: create this member's identity and
/// enroll it with the coordinator.
pub async fn join_setup<DB, C>(
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
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
    C: FederationSetupCoordinator + ?Sized,
{
    invitation
        .validate(now_unix, url_policy)
        .context("invalid federation setup invitation")?;
    std::fs::create_dir_all(state_directory)
        .context("failed to create federation setup state directory")?;
    let secrets_path = state_directory.join(LOCAL_SETUP_SECRETS_FILE);
    let manifest_path = state_directory.join(LOCAL_SETUP_MANIFEST_FILE);
    let local_role = FederationSetupLocalRole::Member {
        member_id: invitation.member_id,
    };
    let config = FederationSetupLocalConfig {
        version: LOCAL_SETUP_CONFIG_VERSION,
        setup_id: invitation.setup_id,
        local_role,
        database_path: database_path.to_path_buf(),
        secrets_path: secrets_path.clone(),
        manifest_path,
        activation_dir: state_directory.join(LOCAL_SETUP_ACTIVATION_DIRECTORY),
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
                runtime_seed: random_runtime_seed(),
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

/// Freeze the complete enrollment and policy set into one signed proposal.
pub async fn propose_setup<DB>(
    db: &DB,
    config_path: &Path,
    now_unix: u64,
) -> Result<FederationSetupProposal>
where
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
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
        .with_coordinator_phase(FederationSetupCoordinatorPhase::ProposalPublished, now_unix)
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

/// Review the coordinator's own proposal against the local setup state.
pub async fn review_local_proposal<DB>(
    db: &DB,
    config_path: &Path,
    proposal: FederationSetupProposal,
    now_unix: u64,
) -> Result<FederationSetupProposalReview>
where
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
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

/// Fetch the proposal from the coordinator and review it against the local setup state.
pub async fn fetch_and_review_proposal<DB, C>(
    db: &DB,
    coordinator: &C,
    config_path: &Path,
    now_unix: u64,
) -> Result<FederationSetupProposalReview>
where
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
    C: FederationSetupCoordinator + ?Sized,
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

/// Approve exactly the reviewed proposal with `proposal_hash`.
pub async fn approve_setup<DB, C>(
    db: &DB,
    coordinator: &C,
    config_path: &Path,
    exact_proposal_hash: FederationSetupProposalHash,
    now_unix: u64,
) -> Result<cdk_common::federation::FederationSetupApproval>
where
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
    C: FederationSetupCoordinator + ?Sized,
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

/// Issue a member's invitation, or with `replace` expire and reissue an unused one.
pub async fn issue_invitation<DB>(
    db: &DB,
    config_path: &Path,
    member_id: MemberId,
    replace: bool,
    output_path: &Path,
    now_unix: u64,
) -> Result<FederationSetupInvitation>
where
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
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
                active.map(|invitation| invitation.expires_at_unix),
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

/// Read and validate a protected local setup config.
pub fn read_local_config(config_path: &Path) -> Result<FederationSetupLocalConfig> {
    let config = read_private_json_file(config_path, "federation setup config")?;
    FederationSetupLocalConfig::validate(&config)?;
    Ok(config)
}

/// The creator's coordinator service for its local setup.
pub fn coordinator_service(
    database: DynMintDatabase,
    config_path: &Path,
) -> Result<FederationSetupCoordinatorService> {
    let (config, secrets) = load_local_state(config_path, current_unix_time()?)?;
    let draft = config
        .coordinator
        .as_ref()
        .ok_or_else(|| anyhow!("only the setup creator can run the coordinator service"))?;
    let coordinator_secret_key = secrets
        .coordinator_secret_key
        .clone()
        .ok_or_else(|| anyhow!("protected setup coordinator identity is missing"))?;
    FederationSetupCoordinatorService::new(
        database,
        config.setup_id,
        draft.threshold_params.member_count,
        coordinator_secret_key,
        config.url_policy,
        FederationSetupCoordinatorServiceConfig::default(),
    )
    .context("failed to initialize setup coordinator service")
}

/// Derive the certified DKG ceremony from the manifest and register it.
pub async fn start_certified_dkg(
    database: &DynMintDatabase,
    handoff: &FederationSetupDkgHandoff,
    driver: crate::FederationDkgDriverRunConfig,
) -> Result<FederationDkgAutoStartConfig> {
    let auto_start =
        FederationDkgAutoStartConfig::from_manifest(&handoff.manifest, handoff.url_policy, driver)
            .context("certified manifest cannot start DKG")?;
    register_manifest_dkg_ceremony(database, handoff, &auto_start).await?;
    Ok(auto_start)
}

/// Validated local input for one certified, independently started DKG member.
#[derive(Debug, Clone)]
pub struct FederationSetupDkgHandoff {
    /// The certified manifest.
    pub manifest: FederationSetupManifest,
    /// The setup-only public config the manifest defines.
    pub public_config: FederationConfig,
    /// This member's private config, without key shares yet.
    pub private_config: MemberConfigPrivate,
    /// Seed of the setup-only signatory.
    pub runtime_seed: Vec<u8>,
    /// URL policy of the setup.
    pub url_policy: FederationSetupUrlPolicy,
}

/// Load and fully validate the locally persisted unanimous setup certificate.
pub fn load_certified_dkg_handoff(config_path: &Path) -> Result<FederationSetupDkgHandoff> {
    let config = read_local_config(config_path)?;
    load_certified_dkg_handoff_from_state(&config, current_unix_time()?)
}

fn load_certified_dkg_handoff_from_state(
    config: &FederationSetupLocalConfig,
    now_unix: u64,
) -> Result<FederationSetupDkgHandoff> {
    let secrets: FederationSetupLocalSecrets =
        read_private_json_file(&config.secrets_path, "federation setup secrets")?;
    secrets.validate(config, now_unix)?;
    let manifest: FederationSetupManifest =
        read_private_json_file(config.manifest_path(), "certified setup manifest")
            .context("run `cdk-mintd federation setup status` after certification before DKG")?;
    manifest
        .validate(config.url_policy)
        .context("invalid certified setup manifest")?;
    validate_proposal_for_local_state(&manifest.proposal, config, &secrets)?;
    match manifest.keyset_specs() {
        [_] => {}
        keysets => bail!(
            "initial DKG handoff requires exactly one BLS keyset specification, found {}",
            keysets.len()
        ),
    }
    let public_config = manifest
        .setup_public_config()
        .context("failed to derive setup-only federation config from manifest")?;
    let private_config = MemberConfigPrivate {
        federation_id: Some(public_config.federation_id),
        member_id: secrets.member_id,
        identity_secret_key: secrets.identity_secret_key.clone(),
        keyset_secret_shares: BTreeMap::new(),
        setup_transcript_hash: None,
    };
    private_config
        .validate_against_public(&public_config)
        .context("protected setup identity does not match the certified manifest")?;
    Ok(FederationSetupDkgHandoff {
        manifest,
        public_config,
        private_config,
        runtime_seed: secrets.runtime_seed,
        url_policy: config.url_policy,
    })
}

async fn persist_creator_state<'a, DB>(
    db: &DB,
    config: &FederationSetupLocalConfig,
    secrets: &FederationSetupLocalSecrets,
    invitations: impl Iterator<Item = &'a FederationSetupInvitation>,
    now_unix: u64,
) -> Result<()>
where
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
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

async fn persist_pending_join<DB>(
    db: &DB,
    config: &FederationSetupLocalConfig,
    secrets: &FederationSetupLocalSecrets,
    now_unix: u64,
) -> Result<()>
where
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
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

async fn mark_local_enrollment_accepted<DB>(
    db: &DB,
    config: &FederationSetupLocalConfig,
    secrets: &FederationSetupLocalSecrets,
    now_unix: u64,
) -> Result<()>
where
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
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
    DB: Database<DatabaseError> + Send + Sync + ?Sized + 'static,
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
            let invitation = new_invitation(
                setup_id,
                member_id,
                draft,
                coordinator_identity,
                now_unix,
                None,
            )?;
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
    replaced_expiry: Option<u64>,
) -> Result<FederationSetupInvitation> {
    let requested_expiry = now_unix
        .checked_add(draft.invitation_lifetime_secs)
        .ok_or_else(|| anyhow!("setup invitation expiry overflow"))?;
    let expires_at_unix = match replaced_expiry {
        Some(replaced_expiry) => requested_expiry.max(
            replaced_expiry
                .checked_add(1)
                .ok_or_else(|| anyhow!("replacement invitation expiry overflow"))?,
        ),
        None => requested_expiry,
    };
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
        && existing.manifest_path == expected.manifest_path
        && existing.activation_dir == expected.activation_dir
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
        FederationSetupUrlPolicy::RequireSecureTransport
            if !matches!(parsed.scheme(), "https" | "iroh") =>
        {
            bail!("production setup URLs must use HTTPS or Iroh")
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
        _ if !matches!(parsed.scheme(), "https" | "iroh") => {
            bail!("unsupported setup URL scheme")
        }
        _ => {}
    }
    Ok(())
}

fn url_policy_for_url(url: &MintUrl) -> FederationSetupUrlPolicy {
    match url.to_string().starts_with("http://") {
        true => FederationSetupUrlPolicy::AllowInsecureLocalhost,
        false => FederationSetupUrlPolicy::RequireSecureTransport,
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

fn random_runtime_seed() -> Vec<u8> {
    let mut seed = Vec::with_capacity(64);
    seed.extend_from_slice(&random_bytes());
    seed.extend_from_slice(&random_bytes());
    seed
}

/// Seconds since the Unix epoch.
pub fn current_unix_time() -> Result<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system time predates the Unix epoch")
        .map(|duration| duration.as_secs())
}

#[cfg(test)]
mod tests;
