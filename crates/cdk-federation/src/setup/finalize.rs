//! The final member config derived from durable DKG records.
//!
//! [`FederationDkgCompletionRecords`] reads every DKG record a member holds and
//! selects the current ceremony; [`finalized_member_config`] derives the final
//! public and private config from them, and the `attach_*` functions add the
//! FROST applications a certified manifest authorizes.

#[cfg(feature = "federation-bdk")]
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use cdk_common::bitcoin;
use cdk_common::database::DynMintDatabase;
#[cfg(feature = "federation-bdk")]
use cdk_common::federation::FederationFrostApplicationConfig;
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use cdk_common::federation::FederationFrostWalletConfig;
use cdk_common::federation::{
    redact_federation_operator_message, FederationConfig, FederationDkgActivationRequest,
    FederationDkgCeremonyRecord, FederationDkgCeremonyState, FederationDkgDriverPhase,
    FederationDkgDriverRecords, FederationDkgFinalizedMemberConfig, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgParticipantState, FederationDkgSecretShareRequest, FederationDkgSetupMethod,
    FederationDkgTranscriptSignatureRequest, FederationFrostRootPublicConfig,
    FederationOnchainConfirmationPolicy, FederationSetupFrostApplication, FederationSetupManifest,
    MemberConfigPrivate, MemberId,
};
#[cfg(feature = "federation-bark")]
use cdk_federation_bark::{
    federation_bark_application, federation_bark_frost_applications,
    federation_bark_recovery_application, federation_bark_recovery_setup_config,
    federation_bark_setup_config, FederationBarkFrostApplicationConfig, FederationBarkPolicyConfig,
    FederationBarkRecoveryApplicationConfig, FederationBarkRecoverySetupConfig,
};
use serde::Serialize;

/// File name of the final public federation config.
pub const FINAL_PUBLIC_CONFIG_FILE: &str = "federation-public.json";
/// File name of the final private member config.
pub const FINAL_PRIVATE_CONFIG_FILE: &str = "federation-private.json";

/// The initial Bark custody application to attach to a final config.
#[cfg(feature = "federation-bark")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationFrostBarkBootstrap {
    /// Bitcoin network of the Bark wallet.
    pub network: bitcoin::Network,
    /// The Ark server the federation uses.
    pub ark_server_public_key: bitcoin::secp256k1::PublicKey,
    /// FROST application key epoch.
    pub application_key_epoch: u64,
    /// Public Bark policy.
    pub policy: FederationBarkPolicyConfig,
    /// Confirmations the recovery wallet requires.
    pub recovery_minimum_confirmations: u32,
    /// Fee policy version of the recovery wallet.
    pub recovery_fee_policy_version: u16,
}

/// The initial CDK on-chain application to attach to a final config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationFrostCdkBootstrap {
    /// Bitcoin network of the on-chain wallet.
    pub network: bitcoin::Network,
    /// Confirmations a deposit requires.
    pub minimum_confirmations: u32,
    /// Transaction fee policy version.
    pub fee_policy_version: u16,
    /// FROST application key epoch.
    pub application_key_epoch: u64,
}

/// Check a CDK on-chain bootstrap's consensus parameters.
pub fn validate_frost_cdk_bootstrap(bootstrap: &FederationFrostCdkBootstrap) -> Result<()> {
    FederationOnchainConfirmationPolicy::new(bootstrap.minimum_confirmations)?;
    if bootstrap.fee_policy_version == 0 || bootstrap.application_key_epoch == 0 {
        bail!("FROST CDK fee-policy and application epochs must be non-zero");
    }
    Ok(())
}

/// Attach the FROST applications a certified manifest authorizes.
pub fn attach_manifest_frost_applications(
    _finalized: &mut FederationDkgFinalizedMemberConfig,
    _frost_roots: &[FederationFrostRootPublicConfig],
    manifest: &FederationSetupManifest,
) -> Result<()> {
    manifest.validate(cdk_common::federation::FederationSetupUrlPolicy::AllowInsecureLocalhost)?;
    let mut cdk_application = None;
    #[cfg(feature = "federation-bark")]
    let mut bark_application = None;
    #[cfg(feature = "federation-bark")]
    let mut bark_recovery_application = None;
    for application in &manifest.frost_policy().applications {
        match application {
            FederationSetupFrostApplication::CdkOnchain {
                application_key_epoch,
                confirmation_policy_version,
                minimum_confirmations,
                fee_policy_version,
            } => {
                if *confirmation_policy_version != 1 {
                    bail!("unsupported manifest CDK confirmation policy version");
                }
                cdk_application = Some(FederationFrostCdkBootstrap {
                    network: manifest.proposal.body.network.bitcoin_network(),
                    minimum_confirmations: *minimum_confirmations,
                    fee_policy_version: *fee_policy_version,
                    application_key_epoch: *application_key_epoch,
                });
            }
            #[cfg(feature = "federation-bark")]
            FederationSetupFrostApplication::Extension { .. } => {
                if let Some(setup) = federation_bark_setup_config(application)? {
                    bark_application = Some(FederationFrostBarkBootstrap {
                        network: setup.network,
                        ark_server_public_key: setup.ark_server_public_key,
                        application_key_epoch: setup.application_key_epoch,
                        policy: setup.policy,
                        recovery_minimum_confirmations: 1,
                        recovery_fee_policy_version: 1,
                    });
                    continue;
                }
                if let Some(setup) = federation_bark_recovery_setup_config(application)? {
                    bark_recovery_application = Some(setup);
                }
            }
            #[cfg(not(feature = "federation-bark"))]
            FederationSetupFrostApplication::Extension { .. } => {}
        }
    }
    #[cfg(feature = "federation-bdk")]
    if let Some(cdk_application) = cdk_application {
        attach_initial_frost_cdk_wallet(_finalized, _frost_roots, cdk_application)?;
    }
    #[cfg(not(feature = "federation-bdk"))]
    if cdk_application.is_some() {
        bail!("manifest enables the CDK FROST application without `federation-bdk`");
    }
    #[cfg(feature = "federation-bark")]
    if let Some(bark_application) = bark_application {
        let recovery = bark_recovery_application.ok_or_else(|| {
            anyhow::anyhow!("manifest Bark custody application requires Bark recovery application")
        })?;
        attach_initial_frost_bark_application(
            _finalized,
            _frost_roots,
            bark_application,
            recovery,
        )?;
    } else if bark_recovery_application.is_some() {
        bail!("manifest Bark recovery application requires Bark custody application");
    }
    Ok(())
}

#[cfg(feature = "federation-bdk")]
/// Attach the initial CDK on-chain wallet to a final config.
pub fn attach_initial_frost_cdk_wallet(
    finalized: &mut FederationDkgFinalizedMemberConfig,
    frost_roots: &[FederationFrostRootPublicConfig],
    bootstrap: FederationFrostCdkBootstrap,
) -> Result<()> {
    if finalized.public_config.frost_wallet.is_some() {
        bail!("initial FROST CDK bootstrap requires a config without an existing wallet");
    }
    let [root] = frost_roots else {
        bail!("initial FROST CDK bootstrap requires exactly one root from the completed dual DKG");
    };
    root.validate(&finalized.public_config)?;
    let application = FederationFrostApplicationConfig::new_cdk_onchain(
        &finalized.public_config,
        root,
        bootstrap.application_key_epoch,
        bootstrap.network,
        FederationOnchainConfirmationPolicy::new(bootstrap.minimum_confirmations)?,
        bootstrap.fee_policy_version,
    )?;
    finalized.public_config.frost_wallet = Some(FederationFrostWalletConfig::new(
        &finalized.public_config,
        root.clone(),
        Some(application),
        BTreeMap::new(),
    )?);
    finalized.public_config.validate()?;
    finalized
        .private_config
        .validate_against_public(&finalized.public_config)?;
    Ok(())
}

#[cfg(feature = "federation-bark")]
/// Attach the initial Bark custody and recovery applications to a final config.
pub fn attach_initial_frost_bark_application(
    finalized: &mut FederationDkgFinalizedMemberConfig,
    frost_roots: &[FederationFrostRootPublicConfig],
    bootstrap: FederationFrostBarkBootstrap,
    recovery_setup: FederationBarkRecoverySetupConfig,
) -> Result<()> {
    let root = match finalized.public_config.frost_wallet.as_ref() {
        Some(wallet) => wallet.root.clone(),
        None => {
            let [root] = frost_roots else {
                bail!("initial FROST Bark bootstrap requires exactly one completed root");
            };
            root.validate(&finalized.public_config)?;
            root.clone()
        }
    };
    if federation_bark_application(&finalized.public_config)?.is_some() {
        bail!("initial Bark bootstrap cannot replace an existing Bark application");
    }
    if federation_bark_recovery_application(&finalized.public_config)?.is_some() {
        bail!("initial Bark bootstrap cannot replace an existing Bark recovery application");
    }
    let cdk_onchain = finalized
        .public_config
        .frost_wallet
        .as_ref()
        .and_then(|wallet| wallet.cdk_onchain_application.clone());
    if cdk_onchain
        .as_ref()
        .is_some_and(|application| application.network != bootstrap.network)
    {
        bail!("Bark and CDK on-chain wallet networks must match");
    }
    let bark = FederationBarkFrostApplicationConfig::new(
        &finalized.public_config,
        &root,
        bootstrap.application_key_epoch,
        bootstrap.network,
        bootstrap.ark_server_public_key,
        bootstrap.policy,
    )?;
    if recovery_setup.network != bootstrap.network {
        bail!("Bark custody and recovery wallet networks must match");
    }
    let recovery = FederationBarkRecoveryApplicationConfig::new(
        &finalized.public_config,
        &root,
        recovery_setup,
    )?;
    let applications =
        federation_bark_frost_applications(&finalized.public_config, &root, bark, recovery)?;
    let upgraded = FederationFrostWalletConfig::new(
        &finalized.public_config,
        root,
        cdk_onchain,
        applications,
    )?;
    finalized.public_config.frost_wallet = Some(upgraded);
    finalized.public_config.validate()?;
    finalized
        .private_config
        .validate_against_public(&finalized.public_config)?;
    Ok(())
}

/// Derive the final member config from DKG records, or check an already final one.
pub fn finalized_member_config(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    keyset_shares: &[FederationDkgMemberKeysetShare],
    transcript_signatures: &[FederationDkgTranscriptSignatureRequest],
    activations: &[FederationDkgActivationRequest],
) -> Result<FederationDkgFinalizedMemberConfig> {
    let (setup_public_config, finalized) =
        if !public_config.keysets.is_empty() || public_config.setup_transcript.is_some() {
            public_config.validate()?;
            private_config.validate_against_public(public_config)?;
            let mut setup_public_config = public_config.clone();
            setup_public_config.keysets.clear();
            setup_public_config.setup_transcript = None;
            (
                setup_public_config,
                FederationDkgFinalizedMemberConfig {
                    public_config: public_config.clone(),
                    private_config: private_config.clone(),
                },
            )
        } else {
            (
                public_config.clone(),
                FederationDkgFinalizedMemberConfig::from_records(
                    public_config,
                    private_config,
                    keyset_shares,
                    transcript_signatures,
                )?,
            )
        };
    if finalized
        .public_config
        .setup_transcript
        .as_ref()
        .is_some_and(|transcript| {
            transcript.payload.setup_method == FederationDkgSetupMethod::PedersenDkg
        })
    {
        finalized.validate_activation_records(&setup_public_config, activations)?;
    }
    Ok(finalized)
}

/// Write the final public and private config files into `output_dir`.
pub fn write_finalized_member_config(
    output_dir: &Path,
    finalized: &FederationDkgFinalizedMemberConfig,
    force: bool,
) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let public_path = output_dir.join(FINAL_PUBLIC_CONFIG_FILE);
    let private_path = output_dir.join(FINAL_PRIVATE_CONFIG_FILE);
    if !force {
        for path in [&public_path, &private_path] {
            if path.exists() {
                bail!(
                    "{} already exists; pass --force to replace finalized DKG config material",
                    path.display()
                );
            }
        }
    }

    write_final_config_file(&private_path, &finalized.private_config, force, true)?;
    write_final_config_file(&public_path, &finalized.public_config, force, false)?;
    Ok((public_path, private_path))
}

/// Write one final config file; private files are readable only by the owner.
pub fn write_final_config_file<T>(path: &Path, value: &T, force: bool, private: bool) -> Result<()>
where
    T: serde::Serialize + ?Sized,
{
    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(if private { 0o600 } else { 0o644 });
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, value)
        .with_context(|| format!("failed to serialize {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to protect {}", path.display()))?;
    }

    Ok(())
}

impl FederationDkgCompletionRecords {
    /// Read every DKG record the member database holds.
    pub async fn read(db: &DynMintDatabase) -> Result<Self> {
        Ok(Self {
            participant_states: db
                .get_federation_dkg_participant_states()
                .await
                .context("failed to read persisted local DKG participant states")?,
            commitments: db
                .get_federation_dkg_commitments()
                .await
                .context("failed to read persisted DKG commitment records")?,
            reveals: db
                .get_federation_dkg_reveals()
                .await
                .context("failed to read persisted DKG reveal records")?,
            secret_share_contributions: db
                .get_federation_dkg_secret_share_contributions()
                .await
                .context("failed to read persisted DKG secret-share contribution records")?,
            keyset_shares: db
                .get_federation_dkg_member_keyset_shares()
                .await
                .context("failed to read persisted DKG keyset-share records")?,
            transcript_signatures: db
                .get_federation_dkg_transcript_signatures()
                .await
                .context("failed to read persisted DKG transcript-signature records")?,
            activations: db
                .get_federation_dkg_activations()
                .await
                .context("failed to read persisted DKG activation records")?,
            ceremonies: db
                .get_federation_dkg_ceremonies()
                .await
                .context("failed to read persisted DKG ceremony lifecycle records")?,
            frost_roots: db
                .get_federation_frost_root_public_configs()
                .await
                .context("failed to read persisted FROST root public configs")?,
        })
    }
}

/// Every DKG record one member holds, across ceremonies.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FederationDkgCompletionRecords {
    /// Local participant states.
    pub participant_states: Vec<FederationDkgParticipantState>,
    /// Commitments.
    pub commitments: Vec<FederationDkgParticipantCommitment>,
    /// Commitment reveals.
    pub reveals: Vec<FederationDkgParticipantCommitmentReveal>,
    /// Secret-share contributions.
    pub secret_share_contributions: Vec<FederationDkgSecretShareRequest>,
    /// Member keyset shares.
    pub keyset_shares: Vec<FederationDkgMemberKeysetShare>,
    /// Transcript signatures.
    pub transcript_signatures: Vec<FederationDkgTranscriptSignatureRequest>,
    /// Final-config confirmations.
    pub activations: Vec<FederationDkgActivationRequest>,
    /// Ceremony lifecycle records.
    pub ceremonies: Vec<FederationDkgCeremonyRecord>,
    /// FROST root public configs.
    pub frost_roots: Vec<FederationFrostRootPublicConfig>,
}

impl FederationDkgCompletionRecords {
    /// The records of the one current, unreplaced and unaborted ceremony.
    pub fn records_for_current_ceremony(&self) -> Result<Self> {
        if self.ceremonies.is_empty() {
            return Ok(self.clone());
        }
        let current = self
            .ceremonies
            .iter()
            .filter(|record| record.replacement_ceremony_id.is_none())
            .filter(|record| record.state != FederationDkgCeremonyState::Aborted)
            .collect::<Vec<_>>();
        if current.len() > 1 {
            bail!("multiple unreplaced DKG ceremony lifecycle records are durable");
        }
        let mut selected = self.clone();
        let Some(ceremony_id) = current.first().map(|record| record.ceremony_id) else {
            selected.participant_states.clear();
            selected.commitments.clear();
            selected.reveals.clear();
            selected.secret_share_contributions.clear();
            selected.keyset_shares.clear();
            selected.transcript_signatures.clear();
            selected.activations.clear();
            return Ok(selected);
        };
        selected
            .participant_states
            .retain(|state| state.ceremony_id() == ceremony_id);
        selected
            .commitments
            .retain(|commitment| commitment.ceremony_id == ceremony_id);
        selected
            .reveals
            .retain(|reveal| reveal.ceremony_id == ceremony_id);
        selected
            .secret_share_contributions
            .retain(|contribution| contribution.ceremony_id == ceremony_id);
        selected
            .keyset_shares
            .retain(|share| share.public_ceremony.ceremony_id() == ceremony_id);
        selected
            .activations
            .retain(|activation| activation.ceremony_id == ceremony_id);
        selected
            .frost_roots
            .retain(|root| root.ceremony_id == ceremony_id);
        Ok(selected)
    }

    /// FROST roots of the current ceremony.
    pub fn frost_roots(&self) -> Result<&[FederationFrostRootPublicConfig]> {
        if self.ceremonies.is_empty() {
            return Ok(&self.frost_roots);
        }
        let current = self
            .ceremonies
            .iter()
            .filter(|record| record.replacement_ceremony_id.is_none())
            .filter(|record| record.state != FederationDkgCeremonyState::Aborted)
            .collect::<Vec<_>>();
        let [current] = current.as_slice() else {
            bail!("exactly one current DKG ceremony is required for finalization");
        };
        if self
            .frost_roots
            .iter()
            .any(|root| root.ceremony_id != current.ceremony_id)
        {
            bail!("persisted FROST roots include a different DKG ceremony");
        }
        Ok(&self.frost_roots)
    }

    /// Progress towards the final config, and the config once it can be derived.
    pub fn finalization_progress(
        &self,
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
    ) -> (
        FederationSetupDkgProgress,
        Option<FederationDkgFinalizedMemberConfig>,
    ) {
        let summary = FederationDkgRecordSummary::from_records(self, public_config, private_config);
        let current = self.records_for_current_ceremony();
        let confirmed_members = current
            .as_ref()
            .map(|records| {
                records
                    .activations
                    .iter()
                    .map(|activation| activation.signer)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default();
        let confirmation_digests = current
            .as_ref()
            .map(|records| {
                records
                    .activations
                    .iter()
                    .map(|activation| activation.final_config_digest.to_string())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let dkg_confirmation_digest = match confirmation_digests.len() {
            1 => confirmation_digests.iter().next().cloned(),
            _ => None,
        };
        let (finalized, transcript_hash, error) = match current.and_then(|records| {
            let finalized = finalized_member_config(
                public_config,
                private_config,
                &records.keyset_shares,
                &records.transcript_signatures,
                &records.activations,
            )?;
            let transcript_hash = finalized
                .public_config
                .setup_transcript
                .as_ref()
                .map(|transcript| transcript.transcript_hash.to_string());
            Ok((finalized, transcript_hash))
        }) {
            Ok((finalized, transcript_hash)) => (Some(finalized), transcript_hash, None),
            Err(err) => (
                None,
                None,
                Some(redact_federation_operator_message(&err.to_string())),
            ),
        };
        let error = if confirmation_digests.len() > 1 {
            Some("conflicting durable DKG final-config confirmation digests".to_string())
        } else {
            error
        };
        let phase = summary.driver_phase.map(dkg_driver_phase_label).unwrap_or(
            if summary.driver_error.is_some() {
                "invalid"
            } else {
                "not_started"
            },
        );
        let progress = FederationSetupDkgProgress {
            phase: phase.to_string(),
            participant_states: summary.participant_states,
            commitments: summary.commitments,
            reveals: summary.reveals,
            secret_share_contributions: summary.secret_share_contributions,
            keyset_shares: summary.keyset_shares,
            transcript_signatures: summary.transcript_signatures,
            final_config_confirmations: summary.activations,
            frost_root_public_configs: summary.frost_root_public_configs,
            confirmed_members,
            finalized_ready: finalized.is_some(),
            dkg_confirmation_digest,
            final_config_digest: None,
            transcript_hash,
            error: summary.driver_error.or(error),
        };
        (progress, finalized)
    }
}

/// Stable, secret-free local setup view derived from durable DKG records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FederationSetupDkgProgress {
    /// Local driver phase label.
    pub phase: String,
    /// Number of local participant states.
    pub participant_states: usize,
    /// Number of commitments.
    pub commitments: usize,
    /// Number of reveals.
    pub reveals: usize,
    /// Number of secret-share contributions.
    pub secret_share_contributions: usize,
    /// Number of member keyset shares.
    pub keyset_shares: usize,
    /// Number of transcript signatures.
    pub transcript_signatures: usize,
    /// Number of final-config confirmations.
    pub final_config_confirmations: usize,
    /// Number of FROST root public configs.
    pub frost_root_public_configs: usize,
    /// Members that confirmed the final config.
    pub confirmed_members: Vec<MemberId>,
    /// Whether the final config can be derived.
    pub finalized_ready: bool,
    /// The one digest every confirmation names.
    pub dkg_confirmation_digest: Option<String>,
    /// Digest of the derived final config.
    pub final_config_digest: Option<String>,
    /// Hash of the DKG transcript.
    pub transcript_hash: Option<String>,
    /// Redacted reason finalization is not ready.
    pub error: Option<String>,
}

/// Counts and members of the DKG records, for operator reports.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FederationDkgRecordSummary {
    /// One line per ceremony lifecycle record.
    pub ceremony_lifecycle: Vec<String>,
    /// Number of local participant states.
    pub participant_states: usize,
    /// Number of commitments.
    pub commitments: usize,
    /// Number of reveals.
    pub reveals: usize,
    /// Number of secret-share contributions.
    pub secret_share_contributions: usize,
    /// Number of member keyset shares.
    pub keyset_shares: usize,
    /// Number of transcript signatures.
    pub transcript_signatures: usize,
    /// Number of final-config confirmations.
    pub activations: usize,
    /// Number of FROST root public configs.
    pub frost_root_public_configs: usize,
    /// Members with a local participant state.
    pub participant_state_members: BTreeSet<MemberId>,
    /// Ceremonies with a local participant state.
    pub participant_state_ceremonies: BTreeSet<String>,
    /// Members that committed.
    pub commitment_members: BTreeSet<MemberId>,
    /// Members that revealed.
    pub reveal_members: BTreeSet<MemberId>,
    /// Members that sent secret shares.
    pub secret_share_senders: BTreeSet<MemberId>,
    /// Members that received secret shares.
    pub secret_share_receivers: BTreeSet<MemberId>,
    /// The local driver phase, when it can be planned.
    pub driver_phase: Option<FederationDkgDriverPhase>,
    /// Why the local driver cannot be planned.
    pub driver_error: Option<String>,
}

impl FederationDkgRecordSummary {
    /// Counts of every record.
    pub fn from_record_counts(records: &FederationDkgCompletionRecords) -> Self {
        Self {
            ceremony_lifecycle: records
                .ceremonies
                .iter()
                .map(|record| {
                    let successor = record
                        .replacement_ceremony_id
                        .map(|ceremony_id| ceremony_id.to_string())
                        .unwrap_or_else(|| "none".to_string());
                    format!(
                        "{}:{}:replacement={successor}",
                        record.ceremony_id,
                        record.state.as_str()
                    )
                })
                .collect(),
            participant_states: records.participant_states.len(),
            commitments: records.commitments.len(),
            reveals: records.reveals.len(),
            secret_share_contributions: records.secret_share_contributions.len(),
            keyset_shares: records.keyset_shares.len(),
            transcript_signatures: records.transcript_signatures.len(),
            activations: records.activations.len(),
            frost_root_public_configs: records.frost_roots.len(),
            participant_state_members: records
                .participant_states
                .iter()
                .map(FederationDkgParticipantState::member_id)
                .collect(),
            participant_state_ceremonies: records
                .participant_states
                .iter()
                .map(|state| state.ceremony_id().to_string())
                .collect(),
            commitment_members: records
                .commitments
                .iter()
                .map(|commitment| commitment.member_id)
                .collect(),
            reveal_members: records
                .reveals
                .iter()
                .map(|reveal| reveal.member_id)
                .collect(),
            secret_share_senders: records
                .secret_share_contributions
                .iter()
                .map(|contribution| contribution.sender)
                .collect(),
            secret_share_receivers: records
                .secret_share_contributions
                .iter()
                .map(|contribution| contribution.receiver)
                .collect(),
            driver_phase: None,
            driver_error: None,
        }
    }

    /// Counts of every record and the local driver phase of the current ceremony.
    pub fn from_records(
        records: &FederationDkgCompletionRecords,
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
    ) -> Self {
        let selected = match records.records_for_current_ceremony() {
            Ok(selected) => selected,
            Err(err) => {
                return Self {
                    driver_error: Some(redact_federation_operator_message(&err.to_string())),
                    ..Self::from_record_counts(records)
                };
            }
        };
        let local_participants = selected
            .participant_states
            .iter()
            .filter(|state| state.member_id() == private_config.member_id)
            .collect::<Vec<_>>();
        let (driver_phase, driver_error) = match local_participants.as_slice() {
            [] => (None, None),
            [participant] => {
                let driver_records = FederationDkgDriverRecords {
                    commitments: selected.commitments.clone(),
                    reveals: selected.reveals.clone(),
                    secret_share_contributions: selected.secret_share_contributions.clone(),
                    keyset_shares: selected.keyset_shares.clone(),
                    transcript_signatures: selected.transcript_signatures.clone(),
                    activations: selected.activations.clone(),
                };
                match driver_records.plan(public_config, private_config, participant) {
                    Ok(plan) => (Some(plan.phase), None),
                    Err(err) => (
                        None,
                        Some(redact_federation_operator_message(&err.to_string())),
                    ),
                }
            }
            _ => (
                None,
                Some("multiple local participant states require ceremony selection".to_string()),
            ),
        };

        Self {
            driver_phase,
            driver_error,
            ..Self::from_record_counts(records)
        }
    }
}

/// Stable label of a DKG driver phase.
pub fn dkg_driver_phase_label(phase: FederationDkgDriverPhase) -> &'static str {
    match phase {
        FederationDkgDriverPhase::Commitment => "commitment",
        FederationDkgDriverPhase::Reveal => "reveal",
        FederationDkgDriverPhase::SecretShares => "share_delivery",
        FederationDkgDriverPhase::LocalKeysetShare => "local_keyset_share",
        FederationDkgDriverPhase::TranscriptSignature => "transcript_signature",
        FederationDkgDriverPhase::Activation => "activation",
        FederationDkgDriverPhase::FrostRound1 => "frost_round_1",
        FederationDkgDriverPhase::FrostRound2 => "frost_round_2",
        FederationDkgDriverPhase::FrostConfirmation => "frost_confirmation",
        FederationDkgDriverPhase::Complete => "complete",
    }
}
