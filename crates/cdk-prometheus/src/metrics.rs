use std::sync::Arc;
use std::time::Instant;

use prometheus::{HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Registry};

/// Global metrics instance
pub static METRICS: std::sync::LazyLock<CdkMetrics> = std::sync::LazyLock::new(CdkMetrics::default);

/// Fixed units of checkpoint and restart work used for bounded instrumentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FederationCheckpointWork {
    /// Journal rows loaded while reconstructing federation state.
    JournalRowsLoaded,
    /// Finalized items loaded while reconstructing federation state.
    FinalizedItemsLoaded,
    /// Checkpoint identity signatures verified.
    SignaturesVerified,
    /// Historical journal entries replayed during pre-checkpoint startup.
    StartupEntriesReplayed,
    /// Journal rows loaded from the suffix after a trusted checkpoint.
    StartupTailJournalRowsLoaded,
    /// Finalized items loaded from the suffix after a trusted checkpoint.
    StartupTailFinalizedItemsLoaded,
    /// AlephBFT live-session backup bytes loaded.
    BackupBytesLoaded,
    /// AlephBFT live-session backup chunks loaded.
    BackupChunksLoaded,
    /// Scheduled checkpoint candidates durably created.
    ScheduledCheckpointsCreated,
    /// Recovery checkpoint candidates durably created.
    RecoveryCheckpointsCreated,
    /// Stored checkpoint quorums transferred over private federation transport.
    QuorumsTransferred,
    /// Verified checkpoint frontiers durably promoted.
    CheckpointsPromoted,
    /// Checkpoint-bounded catch-up batches durably applied.
    CatchUpBatchesApplied,
    /// Checkpoint catch-up pages fetched from peers.
    CatchUpPagesFetched,
    /// Canonical transport bytes fetched in checkpoint catch-up pages.
    CatchUpCanonicalBytesFetched,
    /// Verified checkpoint catch-up pages durably applied.
    CatchUpPagesApplied,
    /// Verified checkpoint catch-up page bytes durably staged before application.
    CatchUpStagedBytes,
    /// Verified checkpoint catch-up page bytes durably applied.
    CatchUpAppliedBytes,
    /// Checkpoint catch-up requests retried with another source or round.
    CatchUpRetries,
}

/// Fixed federation synchronization state entries used for transition counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FederationSyncTransition {
    /// The local member observed a peer frontier ahead of its own.
    Lagging,
    /// The local member started applying checkpoint-bounded peer state.
    CatchingUp,
}

impl FederationSyncTransition {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Lagging => "lagging",
            Self::CatchingUp => "catching_up",
        }
    }
}

impl FederationCheckpointWork {
    const fn as_str(self) -> &'static str {
        match self {
            Self::JournalRowsLoaded => "journal_rows_loaded",
            Self::FinalizedItemsLoaded => "finalized_items_loaded",
            Self::SignaturesVerified => "signatures_verified",
            Self::StartupEntriesReplayed => "startup_entries_replayed",
            Self::StartupTailJournalRowsLoaded => "startup_tail_journal_rows_loaded",
            Self::StartupTailFinalizedItemsLoaded => "startup_tail_finalized_items_loaded",
            Self::BackupBytesLoaded => "backup_bytes_loaded",
            Self::BackupChunksLoaded => "backup_chunks_loaded",
            Self::ScheduledCheckpointsCreated => "scheduled_checkpoints_created",
            Self::RecoveryCheckpointsCreated => "recovery_checkpoints_created",
            Self::QuorumsTransferred => "quorums_transferred",
            Self::CheckpointsPromoted => "checkpoints_promoted",
            Self::CatchUpBatchesApplied => "catch_up_batches_applied",
            Self::CatchUpPagesFetched => "catch_up_pages_fetched",
            Self::CatchUpCanonicalBytesFetched => "catch_up_canonical_bytes_fetched",
            Self::CatchUpPagesApplied => "catch_up_pages_applied",
            Self::CatchUpStagedBytes => "catch_up_staged_bytes",
            Self::CatchUpAppliedBytes => "catch_up_applied_bytes",
            Self::CatchUpRetries => "catch_up_retries",
        }
    }
}

/// Fixed checkpoint operations whose latency is measured without dynamic labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FederationCheckpointOperation {
    /// Full authoritative materialized-state digest creation.
    MaterializedStateDigest,
    /// Router startup through initial federation readiness evaluation.
    StartupReadiness,
    /// Validation and reconstruction of a suffix after a trusted checkpoint.
    StartupTail,
    /// Scheduled checkpoint creation, including its full-state audit.
    ScheduledCheckpointCreation,
    /// Recovery checkpoint creation, including its full-state audit.
    RecoveryCheckpointCreation,
    /// Transfer of one already assembled checkpoint quorum.
    QuorumTransfer,
    /// Durable promotion of one verified checkpoint frontier.
    CheckpointPromotion,
    /// Verification of a checkpoint signature set.
    SignatureVerification,
    /// Validation and application of one checkpoint-bounded catch-up batch.
    CatchUpBatch,
}

impl FederationCheckpointOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::MaterializedStateDigest => "materialized_state_digest",
            Self::StartupReadiness => "startup_readiness",
            Self::StartupTail => "startup_tail",
            Self::ScheduledCheckpointCreation => "scheduled_checkpoint_creation",
            Self::RecoveryCheckpointCreation => "recovery_checkpoint_creation",
            Self::QuorumTransfer => "quorum_transfer",
            Self::CheckpointPromotion => "checkpoint_promotion",
            Self::SignatureVerification => "signature_verification",
            Self::CatchUpBatch => "catch_up_batch",
        }
    }
}

/// Fixed units of process-local federation performance work.
///
/// These values intentionally contain no member, peer, checkpoint, signature,
/// quote, or operation identifiers. A federation deployment identifies the
/// member from the Prometheus scrape target instead of adding another label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FederationPerformanceWork {
    /// Full public-config validation invocations.
    ConfigValidationAttempts,
    /// Failed full public-config validation invocations.
    ConfigValidationFailures,
    /// Federated keyset validation invocations.
    KeysetValidationAttempts,
    /// Failed federated keyset validation invocations.
    KeysetValidationFailures,
    /// Aggregate-key interpolation invocations, one per amount.
    AggregateKeyInterpolationAttempts,
    /// Failed aggregate-key interpolation invocations.
    AggregateKeyInterpolationFailures,
    /// Setup-transcript lineage validation invocations.
    SetupLineageValidationAttempts,
    /// Failed setup-transcript lineage validation invocations.
    SetupLineageValidationFailures,
    /// Scheduled-checkpoint capture invocations.
    ScheduledCaptureAttempts,
    /// Failed scheduled-checkpoint capture invocations.
    ScheduledCaptureFailures,
    /// Captures skipped because a newer local candidate is already pending.
    ScheduledCaptureSkippedPendingCandidate,
    /// Captures skipped because there is no complete finalized boundary.
    ScheduledCaptureSkippedNoBoundary,
    /// Captures skipped because the local frontier trails the trusted checkpoint.
    ScheduledCaptureSkippedFrontierBehind,
    /// Captures skipped because the configured checkpoint interval is not eligible.
    ScheduledCaptureSkippedIneligible,
    /// Captures suppressed by a test-only fault injection.
    ScheduledCaptureSuppressed,
    /// Scheduled-checkpoint candidates durably created.
    ScheduledCaptureCreated,
    /// Finalized suffix rows loaded by scheduled-checkpoint capture.
    ScheduledCaptureSuffixRowsLoaded,
    /// Canonical suffix bytes computed by scheduled-checkpoint capture.
    ScheduledCaptureCanonicalBytesProcessed,
    /// Loaded suffix rows discarded when a scheduled capture is ineligible.
    ScheduledCaptureIneligibleRowsDiscarded,
    /// Computed canonical bytes discarded when a scheduled capture is ineligible.
    ScheduledCaptureIneligibleCanonicalBytesDiscarded,
    /// Rows loaded into a materialized-state snapshot.
    MaterializedStateRowsLoaded,
    /// Checkpoint candidate persistence invocations.
    CheckpointPersistenceAttempts,
    /// Failed checkpoint candidate persistence invocations.
    CheckpointPersistenceFailures,
    /// Successful checkpoint candidate persistence invocations.
    CheckpointPersistenceSuccesses,
    /// Upstream AlephBFT finalized units that contain application data.
    AlephBftFinalizedUnitsWithData,
    /// Upstream AlephBFT finalized units that contain no application data.
    AlephBftFinalizedUnitsWithoutData,
    /// Quorum verifications while inspecting checkpoint capture state.
    QuorumVerificationCaptureAttempts,
    /// Failed quorum verifications while inspecting checkpoint capture state.
    QuorumVerificationCaptureFailures,
    /// Quorum verifications while forming a quorum from signatures.
    QuorumVerificationFormationAttempts,
    /// Failed quorum verifications while forming a quorum from signatures.
    QuorumVerificationFormationFailures,
    /// Quorum verifications while persisting or promoting a checkpoint.
    QuorumVerificationPromotionAttempts,
    /// Failed quorum verifications while persisting or promoting a checkpoint.
    QuorumVerificationPromotionFailures,
    /// Quorum verifications before publishing a checkpoint to peers.
    QuorumVerificationPublishAttempts,
    /// Failed quorum verifications before publishing a checkpoint to peers.
    QuorumVerificationPublishFailures,
    /// Quorum verifications while validating a checkpoint fetched from a peer.
    QuorumVerificationPeerFetchAttempts,
    /// Failed quorum verifications while validating a checkpoint fetched from a peer.
    QuorumVerificationPeerFetchFailures,
    /// Quorum verifications during startup or checkpoint catch-up.
    QuorumVerificationStartupCatchUpAttempts,
    /// Failed quorum verifications during startup or checkpoint catch-up.
    QuorumVerificationStartupCatchUpFailures,
    /// Quorum verifications during an operator-requested checkpoint audit.
    QuorumVerificationAuditAttempts,
    /// Failed quorum verifications during an operator-requested checkpoint audit.
    QuorumVerificationAuditFailures,
    /// Swap proofs passed to aggregate-BLS verification by the request builder.
    SwapBuilderProofsVerified,
    /// Swap proofs passed to aggregate-BLS verification during operation admission.
    SwapAdmissionProofsVerified,
    /// Swap proofs passed to aggregate-BLS verification during authoritative apply.
    SwapApplyProofsVerified,
    /// Melt proofs passed to aggregate-BLS verification by the request builder.
    MeltBuilderProofsVerified,
    /// Melt proofs passed to aggregate-BLS verification during operation admission.
    MeltAdmissionProofsVerified,
    /// Melt proofs passed to aggregate-BLS verification during authoritative apply.
    MeltApplyProofsVerified,
    /// Journal entries processed by authoritative application.
    JournalEntriesProcessed,
    /// Blinded outputs for which this member attempted to create a signature share.
    MemberBlindedShareOutputs,
}

impl FederationPerformanceWork {
    /// Complete fixed label domain for schema and cardinality checks.
    pub const ALL: [Self; 48] = [
        Self::ConfigValidationAttempts,
        Self::ConfigValidationFailures,
        Self::KeysetValidationAttempts,
        Self::KeysetValidationFailures,
        Self::AggregateKeyInterpolationAttempts,
        Self::AggregateKeyInterpolationFailures,
        Self::SetupLineageValidationAttempts,
        Self::SetupLineageValidationFailures,
        Self::ScheduledCaptureAttempts,
        Self::ScheduledCaptureFailures,
        Self::ScheduledCaptureSkippedPendingCandidate,
        Self::ScheduledCaptureSkippedNoBoundary,
        Self::ScheduledCaptureSkippedFrontierBehind,
        Self::ScheduledCaptureSkippedIneligible,
        Self::ScheduledCaptureSuppressed,
        Self::ScheduledCaptureCreated,
        Self::ScheduledCaptureSuffixRowsLoaded,
        Self::ScheduledCaptureCanonicalBytesProcessed,
        Self::ScheduledCaptureIneligibleRowsDiscarded,
        Self::ScheduledCaptureIneligibleCanonicalBytesDiscarded,
        Self::MaterializedStateRowsLoaded,
        Self::CheckpointPersistenceAttempts,
        Self::CheckpointPersistenceFailures,
        Self::CheckpointPersistenceSuccesses,
        Self::AlephBftFinalizedUnitsWithData,
        Self::AlephBftFinalizedUnitsWithoutData,
        Self::QuorumVerificationCaptureAttempts,
        Self::QuorumVerificationCaptureFailures,
        Self::QuorumVerificationFormationAttempts,
        Self::QuorumVerificationFormationFailures,
        Self::QuorumVerificationPromotionAttempts,
        Self::QuorumVerificationPromotionFailures,
        Self::QuorumVerificationPublishAttempts,
        Self::QuorumVerificationPublishFailures,
        Self::QuorumVerificationPeerFetchAttempts,
        Self::QuorumVerificationPeerFetchFailures,
        Self::QuorumVerificationStartupCatchUpAttempts,
        Self::QuorumVerificationStartupCatchUpFailures,
        Self::QuorumVerificationAuditAttempts,
        Self::QuorumVerificationAuditFailures,
        Self::SwapBuilderProofsVerified,
        Self::SwapAdmissionProofsVerified,
        Self::SwapApplyProofsVerified,
        Self::MeltBuilderProofsVerified,
        Self::MeltAdmissionProofsVerified,
        Self::MeltApplyProofsVerified,
        Self::JournalEntriesProcessed,
        Self::MemberBlindedShareOutputs,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::ConfigValidationAttempts => "config_validation_attempts",
            Self::ConfigValidationFailures => "config_validation_failures",
            Self::KeysetValidationAttempts => "keyset_validation_attempts",
            Self::KeysetValidationFailures => "keyset_validation_failures",
            Self::AggregateKeyInterpolationAttempts => "aggregate_key_interpolation_attempts",
            Self::AggregateKeyInterpolationFailures => "aggregate_key_interpolation_failures",
            Self::SetupLineageValidationAttempts => "setup_lineage_validation_attempts",
            Self::SetupLineageValidationFailures => "setup_lineage_validation_failures",
            Self::ScheduledCaptureAttempts => "scheduled_capture_attempts",
            Self::ScheduledCaptureFailures => "scheduled_capture_failures",
            Self::ScheduledCaptureSkippedPendingCandidate => {
                "scheduled_capture_skipped_pending_candidate"
            }
            Self::ScheduledCaptureSkippedNoBoundary => "scheduled_capture_skipped_no_boundary",
            Self::ScheduledCaptureSkippedFrontierBehind => {
                "scheduled_capture_skipped_frontier_behind"
            }
            Self::ScheduledCaptureSkippedIneligible => "scheduled_capture_skipped_ineligible",
            Self::ScheduledCaptureSuppressed => "scheduled_capture_suppressed",
            Self::ScheduledCaptureCreated => "scheduled_capture_created",
            Self::ScheduledCaptureSuffixRowsLoaded => "scheduled_capture_suffix_rows_loaded",
            Self::ScheduledCaptureCanonicalBytesProcessed => {
                "scheduled_capture_canonical_bytes_processed"
            }
            Self::ScheduledCaptureIneligibleRowsDiscarded => {
                "scheduled_capture_ineligible_rows_discarded"
            }
            Self::ScheduledCaptureIneligibleCanonicalBytesDiscarded => {
                "scheduled_capture_ineligible_canonical_bytes_discarded"
            }
            Self::MaterializedStateRowsLoaded => "materialized_state_rows_loaded",
            Self::CheckpointPersistenceAttempts => "checkpoint_persistence_attempts",
            Self::CheckpointPersistenceFailures => "checkpoint_persistence_failures",
            Self::CheckpointPersistenceSuccesses => "checkpoint_persistence_successes",
            Self::AlephBftFinalizedUnitsWithData => "aleph_bft_finalized_units_with_data",
            Self::AlephBftFinalizedUnitsWithoutData => "aleph_bft_finalized_units_without_data",
            Self::QuorumVerificationCaptureAttempts => "quorum_verification_capture_attempts",
            Self::QuorumVerificationCaptureFailures => "quorum_verification_capture_failures",
            Self::QuorumVerificationFormationAttempts => "quorum_verification_formation_attempts",
            Self::QuorumVerificationFormationFailures => "quorum_verification_formation_failures",
            Self::QuorumVerificationPromotionAttempts => "quorum_verification_promotion_attempts",
            Self::QuorumVerificationPromotionFailures => "quorum_verification_promotion_failures",
            Self::QuorumVerificationPublishAttempts => "quorum_verification_publish_attempts",
            Self::QuorumVerificationPublishFailures => "quorum_verification_publish_failures",
            Self::QuorumVerificationPeerFetchAttempts => "quorum_verification_peer_fetch_attempts",
            Self::QuorumVerificationPeerFetchFailures => "quorum_verification_peer_fetch_failures",
            Self::QuorumVerificationStartupCatchUpAttempts => {
                "quorum_verification_startup_catch_up_attempts"
            }
            Self::QuorumVerificationStartupCatchUpFailures => {
                "quorum_verification_startup_catch_up_failures"
            }
            Self::QuorumVerificationAuditAttempts => "quorum_verification_audit_attempts",
            Self::QuorumVerificationAuditFailures => "quorum_verification_audit_failures",
            Self::SwapBuilderProofsVerified => "swap_builder_proofs_verified",
            Self::SwapAdmissionProofsVerified => "swap_admission_proofs_verified",
            Self::SwapApplyProofsVerified => "swap_apply_proofs_verified",
            Self::MeltBuilderProofsVerified => "melt_builder_proofs_verified",
            Self::MeltAdmissionProofsVerified => "melt_admission_proofs_verified",
            Self::MeltApplyProofsVerified => "melt_apply_proofs_verified",
            Self::JournalEntriesProcessed => "journal_entries_processed",
            Self::MemberBlindedShareOutputs => "member_blinded_share_outputs",
        }
    }
}

/// Fixed process-local federation operations whose duration is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FederationPerformanceOperation {
    /// Full public federation-config validation.
    FullConfigValidation,
    /// Validation of one federated keyset.
    KeysetValidation,
    /// Interpolation of one aggregate key from member shares.
    AggregateKeyInterpolation,
    /// Validation of setup-transcript lineage.
    SetupLineageValidation,
    /// One complete scheduled-checkpoint capture attempt.
    ScheduledCheckpointCapture,
    /// Loading the finalized suffix for scheduled-checkpoint capture.
    ScheduledCheckpointSuffixLoad,
    /// Computing canonical bytes for a scheduled-checkpoint suffix.
    ScheduledCheckpointCanonicalBytes,
    /// Loading a complete materialized-state snapshot.
    MaterializedStateLoad,
    /// Digesting a loaded materialized-state snapshot.
    MaterializedStateDigest,
    /// Persisting one checkpoint candidate, audit, and local signature.
    CheckpointPersistence,
    /// Quorum verification while inspecting checkpoint capture state.
    QuorumVerificationCapture,
    /// Quorum verification while forming a quorum from signatures.
    QuorumVerificationFormation,
    /// Quorum verification while persisting or promoting a checkpoint.
    QuorumVerificationPromotion,
    /// Quorum verification before publishing a checkpoint to peers.
    QuorumVerificationPublish,
    /// Quorum verification while validating a checkpoint fetched from a peer.
    QuorumVerificationPeerFetch,
    /// Quorum verification during startup or checkpoint catch-up.
    QuorumVerificationStartupCatchUp,
    /// Quorum verification during an operator-requested checkpoint audit.
    QuorumVerificationAudit,
    /// Verification of one incoming checkpoint signature during quorum formation.
    CheckpointSignatureVerificationFormation,
    /// Self-verification of one local signature before publishing it.
    CheckpointSignatureVerificationPublish,
    /// Verification of one checkpoint signature fetched from a peer.
    CheckpointSignatureVerificationPeerFetch,
    /// Waiting to acquire the outer operation-application lock.
    OperationApplicationLockWait,
    /// Holding the outer operation-application lock.
    OperationApplicationLockHold,
    /// Applying accepted federation journal entries to mint state.
    JournalApplication,
    /// Capturing a recovery checkpoint while application is serialized.
    RecoveryCheckpointCapture,
    /// Aggregate-BLS verification of swap proofs by the request builder.
    SwapProofVerificationBuilder,
    /// Aggregate-BLS verification of swap proofs during operation admission.
    SwapProofVerificationAdmission,
    /// Aggregate-BLS verification of swap proofs during authoritative apply.
    SwapProofVerificationApply,
    /// Aggregate-BLS verification of melt proofs by the request builder.
    MeltProofVerificationBuilder,
    /// Aggregate-BLS verification of melt proofs during operation admission.
    MeltProofVerificationAdmission,
    /// Aggregate-BLS verification of melt proofs during authoritative apply.
    MeltProofVerificationApply,
    /// Creation of one member BLS blind-signature share.
    MemberBlindedShareSigning,
    /// Pairing-based self-verification of one member BLS blind-signature share.
    MemberBlindedShareSelfVerification,
}

impl FederationPerformanceOperation {
    /// Complete fixed label domain for schema and cardinality checks.
    pub const ALL: [Self; 32] = [
        Self::FullConfigValidation,
        Self::KeysetValidation,
        Self::AggregateKeyInterpolation,
        Self::SetupLineageValidation,
        Self::ScheduledCheckpointCapture,
        Self::ScheduledCheckpointSuffixLoad,
        Self::ScheduledCheckpointCanonicalBytes,
        Self::MaterializedStateLoad,
        Self::MaterializedStateDigest,
        Self::CheckpointPersistence,
        Self::QuorumVerificationCapture,
        Self::QuorumVerificationFormation,
        Self::QuorumVerificationPromotion,
        Self::QuorumVerificationPublish,
        Self::QuorumVerificationPeerFetch,
        Self::QuorumVerificationStartupCatchUp,
        Self::QuorumVerificationAudit,
        Self::CheckpointSignatureVerificationFormation,
        Self::CheckpointSignatureVerificationPublish,
        Self::CheckpointSignatureVerificationPeerFetch,
        Self::OperationApplicationLockWait,
        Self::OperationApplicationLockHold,
        Self::JournalApplication,
        Self::RecoveryCheckpointCapture,
        Self::SwapProofVerificationBuilder,
        Self::SwapProofVerificationAdmission,
        Self::SwapProofVerificationApply,
        Self::MeltProofVerificationBuilder,
        Self::MeltProofVerificationAdmission,
        Self::MeltProofVerificationApply,
        Self::MemberBlindedShareSigning,
        Self::MemberBlindedShareSelfVerification,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::FullConfigValidation => "full_config_validation",
            Self::KeysetValidation => "keyset_validation",
            Self::AggregateKeyInterpolation => "aggregate_key_interpolation",
            Self::SetupLineageValidation => "setup_lineage_validation",
            Self::ScheduledCheckpointCapture => "scheduled_checkpoint_capture",
            Self::ScheduledCheckpointSuffixLoad => "scheduled_checkpoint_suffix_load",
            Self::ScheduledCheckpointCanonicalBytes => "scheduled_checkpoint_canonical_bytes",
            Self::MaterializedStateLoad => "materialized_state_load",
            Self::MaterializedStateDigest => "materialized_state_digest",
            Self::CheckpointPersistence => "checkpoint_persistence",
            Self::QuorumVerificationCapture => "quorum_verification_capture",
            Self::QuorumVerificationFormation => "quorum_verification_formation",
            Self::QuorumVerificationPromotion => "quorum_verification_promotion",
            Self::QuorumVerificationPublish => "quorum_verification_publish",
            Self::QuorumVerificationPeerFetch => "quorum_verification_peer_fetch",
            Self::QuorumVerificationStartupCatchUp => "quorum_verification_startup_catch_up",
            Self::QuorumVerificationAudit => "quorum_verification_audit",
            Self::CheckpointSignatureVerificationFormation => {
                "checkpoint_signature_verification_formation"
            }
            Self::CheckpointSignatureVerificationPublish => {
                "checkpoint_signature_verification_publish"
            }
            Self::CheckpointSignatureVerificationPeerFetch => {
                "checkpoint_signature_verification_peer_fetch"
            }
            Self::OperationApplicationLockWait => "operation_application_lock_wait",
            Self::OperationApplicationLockHold => "operation_application_lock_hold",
            Self::JournalApplication => "journal_application",
            Self::RecoveryCheckpointCapture => "recovery_checkpoint_capture",
            Self::SwapProofVerificationBuilder => "swap_proof_verification_builder",
            Self::SwapProofVerificationAdmission => "swap_proof_verification_admission",
            Self::SwapProofVerificationApply => "swap_proof_verification_apply",
            Self::MeltProofVerificationBuilder => "melt_proof_verification_builder",
            Self::MeltProofVerificationAdmission => "melt_proof_verification_admission",
            Self::MeltProofVerificationApply => "melt_proof_verification_apply",
            Self::MemberBlindedShareSigning => "member_blinded_share_signing",
            Self::MemberBlindedShareSelfVerification => "member_blinded_share_self_verification",
        }
    }
}

/// Fixed outcome label for a process-local federation performance operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FederationPerformanceOutcome {
    /// The measured operation completed successfully, including intentional skips.
    Success,
    /// The measured operation returned an error.
    Failure,
}

impl FederationPerformanceOutcome {
    /// Complete fixed label domain for schema and cardinality checks.
    pub const ALL: [Self; 2] = [Self::Success, Self::Failure];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }

    /// Convert a result predicate into the corresponding fixed outcome.
    #[must_use]
    pub const fn from_success(success: bool) -> Self {
        if success {
            Self::Success
        } else {
            Self::Failure
        }
    }
}

/// RAII guard for recording mint operation metrics.
///
/// The guard increments the in-flight gauge when it is created, records the
/// operation count and duration when [`Self::record`] is called, and always
/// decrements the in-flight gauge when it is dropped.
#[derive(Debug)]
pub struct MintMetricGuard {
    operation: &'static str,
    start_time: Instant,
}

impl MintMetricGuard {
    /// Start tracking a mint operation.
    #[must_use]
    pub fn new(operation: &'static str) -> Self {
        METRICS.inc_in_flight_requests(operation);

        Self {
            operation,
            start_time: Instant::now(),
        }
    }

    /// Record the operation result and duration.
    pub fn record(self, success: bool) {
        METRICS.record_mint_operation(self.operation, success);
        METRICS.record_mint_operation_histogram(
            self.operation,
            success,
            self.start_time.elapsed().as_secs_f64(),
        );

        if !success {
            METRICS.record_error();
        }
    }
}

impl Drop for MintMetricGuard {
    fn drop(&mut self) {
        METRICS.dec_in_flight_requests(self.operation);
    }
}

/// Custom metrics for CDK applications
#[derive(Clone, Debug)]
pub struct CdkMetrics {
    registry: Arc<Registry>,

    // HTTP metrics
    http_requests_total: IntCounterVec,
    http_request_duration: HistogramVec,

    // Authentication metrics
    auth_attempts_total: IntCounter,
    auth_successes_total: IntCounter,

    // Payment metrics
    payments_total: IntCounterVec,
    payment_amount: HistogramVec,
    payment_fees: HistogramVec,

    // Database metrics
    db_operations_total: IntCounter,
    db_operation_duration: HistogramVec,
    db_connections_active: IntGauge,

    // Error metrics
    errors_total: IntCounter,

    // Mint metrics
    mint_operations_total: IntCounterVec,
    mint_in_flight_requests: IntGaugeVec,
    mint_operation_duration: HistogramVec,

    // Federation metrics
    federation_consensus_operation_count: IntGaugeVec,
    federation_sync_ready: IntGaugeVec,
    federation_sync_state: IntGaugeVec,
    federation_sync_transitions_total: IntCounterVec,
    federation_checkpoint_lag_operations: IntGaugeVec,
    federation_payment_observation_reports: IntGaugeVec,
    federation_peer_connected: IntGaugeVec,
    federation_peer_operation_count: IntGaugeVec,
    federation_pending_mempool_items: IntGaugeVec,
    federation_wallet_utxos: IntGaugeVec,
    federation_wallet_value_sat: IntGaugeVec,
    federation_wallet_operation_phase: IntGaugeVec,
    federation_wallet_revealed_count: IntGaugeVec,
    federation_wallet_pending_batches: IntGaugeVec,
    federation_wallet_signing_attempts: IntGaugeVec,
    federation_wallet_signing_failures: IntGaugeVec,
    federation_wallet_broadcast_health: IntGaugeVec,
    federation_wallet_broadcast_failures: IntGaugeVec,
    federation_bark_readiness: IntGaugeVec,
    federation_bark_projection_lag_operations: IntGaugeVec,
    federation_bark_vtxos: IntGaugeVec,
    federation_bark_value_sat: IntGaugeVec,
    federation_bark_action_count: IntGaugeVec,
    federation_bark_action_max_age_operations: IntGaugeVec,
    federation_bark_exposure_count: IntGaugeVec,
    federation_bark_exposure_value_sat: IntGaugeVec,
    federation_bark_protocol_records: IntGaugeVec,
    federation_bark_receive_secret_coverage: IntGaugeVec,
    federation_accepted_operations_total: IntCounterVec,
    federation_rejected_operations_total: IntCounterVec,
    federation_signing_share_latency: HistogramVec,
    federation_aggregation_failures_total: IntCounterVec,
    federation_state_digest_mismatches_total: IntCounterVec,
    federation_checkpoint_work_total: IntCounterVec,
    federation_checkpoint_catch_up_peak_in_flight_bytes: IntGaugeVec,
    federation_checkpoint_duration: HistogramVec,
    federation_performance_work_total: IntCounterVec,
    federation_performance_operation_duration: HistogramVec,
    federation_transaction_phase_duration: HistogramVec,
}

#[derive(Debug)]
struct FederationMetricCollectors {
    consensus_operation_count: IntGaugeVec,
    sync_ready: IntGaugeVec,
    sync_state: IntGaugeVec,
    sync_transitions_total: IntCounterVec,
    checkpoint_lag_operations: IntGaugeVec,
    payment_observation_reports: IntGaugeVec,
    peer_connected: IntGaugeVec,
    peer_operation_count: IntGaugeVec,
    pending_mempool_items: IntGaugeVec,
    wallet_utxos: IntGaugeVec,
    wallet_value_sat: IntGaugeVec,
    wallet_operation_phase: IntGaugeVec,
    wallet_revealed_count: IntGaugeVec,
    wallet_pending_batches: IntGaugeVec,
    wallet_signing_attempts: IntGaugeVec,
    wallet_signing_failures: IntGaugeVec,
    wallet_broadcast_health: IntGaugeVec,
    wallet_broadcast_failures: IntGaugeVec,
    bark_readiness: IntGaugeVec,
    bark_projection_lag_operations: IntGaugeVec,
    bark_vtxos: IntGaugeVec,
    bark_value_sat: IntGaugeVec,
    bark_action_count: IntGaugeVec,
    bark_action_max_age_operations: IntGaugeVec,
    bark_exposure_count: IntGaugeVec,
    bark_exposure_value_sat: IntGaugeVec,
    bark_protocol_records: IntGaugeVec,
    bark_receive_secret_coverage: IntGaugeVec,
    accepted_operations_total: IntCounterVec,
    rejected_operations_total: IntCounterVec,
    signing_share_latency: HistogramVec,
    aggregation_failures_total: IntCounterVec,
    state_digest_mismatches_total: IntCounterVec,
    checkpoint_work_total: IntCounterVec,
    checkpoint_catch_up_peak_in_flight_bytes: IntGaugeVec,
    checkpoint_duration: HistogramVec,
    performance_work_total: IntCounterVec,
    performance_operation_duration: HistogramVec,
    transaction_phase_duration: HistogramVec,
}

impl CdkMetrics {
    /// Create a new instance with default metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    pub fn new() -> crate::Result<Self> {
        let registry = Arc::new(Registry::new());

        // Create and register HTTP metrics
        let (http_requests_total, http_request_duration) = Self::create_http_metrics(&registry)?;

        // Create and register authentication metrics
        let (auth_attempts_total, auth_successes_total) = Self::create_auth_metrics(&registry)?;

        // Create and register payment metrics
        let (payments_total, payment_amount, payment_fees) =
            Self::create_payment_metrics(&registry)?;

        // Create and register database metrics
        let (db_operations_total, db_operation_duration, db_connections_active) =
            Self::create_db_metrics(&registry)?;

        // Create and register error metrics
        let errors_total = Self::create_error_metrics(&registry)?;

        // Create and register mint metrics
        let (mint_operations_total, mint_operation_duration, mint_in_flight_requests) =
            Self::create_mint_metrics(&registry)?;

        // Create and register federation metrics
        let federation_metrics = Self::create_federation_metrics(&registry)?;

        Ok(Self {
            registry,
            http_requests_total,
            http_request_duration,
            auth_attempts_total,
            auth_successes_total,
            payments_total,
            payment_amount,
            payment_fees,
            db_operations_total,
            db_operation_duration,
            db_connections_active,
            errors_total,
            mint_operations_total,
            mint_in_flight_requests,
            mint_operation_duration,
            federation_consensus_operation_count: federation_metrics.consensus_operation_count,
            federation_sync_ready: federation_metrics.sync_ready,
            federation_sync_state: federation_metrics.sync_state,
            federation_sync_transitions_total: federation_metrics.sync_transitions_total,
            federation_checkpoint_lag_operations: federation_metrics.checkpoint_lag_operations,
            federation_payment_observation_reports: federation_metrics.payment_observation_reports,
            federation_peer_connected: federation_metrics.peer_connected,
            federation_peer_operation_count: federation_metrics.peer_operation_count,
            federation_pending_mempool_items: federation_metrics.pending_mempool_items,
            federation_wallet_utxos: federation_metrics.wallet_utxos,
            federation_wallet_value_sat: federation_metrics.wallet_value_sat,
            federation_wallet_operation_phase: federation_metrics.wallet_operation_phase,
            federation_wallet_revealed_count: federation_metrics.wallet_revealed_count,
            federation_wallet_pending_batches: federation_metrics.wallet_pending_batches,
            federation_wallet_signing_attempts: federation_metrics.wallet_signing_attempts,
            federation_wallet_signing_failures: federation_metrics.wallet_signing_failures,
            federation_wallet_broadcast_health: federation_metrics.wallet_broadcast_health,
            federation_wallet_broadcast_failures: federation_metrics.wallet_broadcast_failures,
            federation_bark_readiness: federation_metrics.bark_readiness,
            federation_bark_projection_lag_operations: federation_metrics
                .bark_projection_lag_operations,
            federation_bark_vtxos: federation_metrics.bark_vtxos,
            federation_bark_value_sat: federation_metrics.bark_value_sat,
            federation_bark_action_count: federation_metrics.bark_action_count,
            federation_bark_action_max_age_operations: federation_metrics
                .bark_action_max_age_operations,
            federation_bark_exposure_count: federation_metrics.bark_exposure_count,
            federation_bark_exposure_value_sat: federation_metrics.bark_exposure_value_sat,
            federation_bark_protocol_records: federation_metrics.bark_protocol_records,
            federation_bark_receive_secret_coverage: federation_metrics
                .bark_receive_secret_coverage,
            federation_accepted_operations_total: federation_metrics.accepted_operations_total,
            federation_rejected_operations_total: federation_metrics.rejected_operations_total,
            federation_signing_share_latency: federation_metrics.signing_share_latency,
            federation_aggregation_failures_total: federation_metrics.aggregation_failures_total,
            federation_state_digest_mismatches_total: federation_metrics
                .state_digest_mismatches_total,
            federation_checkpoint_work_total: federation_metrics.checkpoint_work_total,
            federation_checkpoint_catch_up_peak_in_flight_bytes: federation_metrics
                .checkpoint_catch_up_peak_in_flight_bytes,
            federation_checkpoint_duration: federation_metrics.checkpoint_duration,
            federation_performance_work_total: federation_metrics.performance_work_total,
            federation_performance_operation_duration: federation_metrics
                .performance_operation_duration,
            federation_transaction_phase_duration: federation_metrics.transaction_phase_duration,
        })
    }

    /// Create and register HTTP metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_http_metrics(registry: &Registry) -> crate::Result<(IntCounterVec, HistogramVec)> {
        let http_requests_total = IntCounterVec::new(
            prometheus::Opts::new("cdk_http_requests_total", "Total number of HTTP requests"),
            &["endpoint", "status"],
        )?;
        registry.register(Box::new(http_requests_total.clone()))?;

        let http_request_duration = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_http_request_duration_seconds",
                "HTTP request duration in seconds",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
            &["endpoint"],
        )?;
        registry.register(Box::new(http_request_duration.clone()))?;

        Ok((http_requests_total, http_request_duration))
    }

    /// Create and register authentication metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_auth_metrics(registry: &Registry) -> crate::Result<(IntCounter, IntCounter)> {
        let auth_attempts_total =
            IntCounter::new("cdk_auth_attempts_total", "Total authentication attempts")?;
        registry.register(Box::new(auth_attempts_total.clone()))?;

        let auth_successes_total = IntCounter::new(
            "cdk_auth_successes_total",
            "Total successful authentications",
        )?;
        registry.register(Box::new(auth_successes_total.clone()))?;

        Ok((auth_attempts_total, auth_successes_total))
    }

    /// Create and register payment metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_payment_metrics(
        registry: &Registry,
    ) -> crate::Result<(IntCounterVec, HistogramVec, HistogramVec)> {
        let wallet_operations_total =
            IntCounter::new("cdk_wallet_operations_total", "Total wallet operations")?;
        registry.register(Box::new(wallet_operations_total))?;

        let payments_total = IntCounterVec::new(
            prometheus::Opts::new("cdk_payments_total", "Total confirmed payments"),
            &["method"],
        )?;
        registry.register(Box::new(payments_total.clone()))?;

        let payment_amount = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_payment_amount_sats",
                "Confirmed payment amounts in satoshis",
            )
            .buckets(vec![
                1.0,
                10.0,
                100.0,
                1000.0,
                10_000.0,
                100_000.0,
                1_000_000.0,
            ]),
            &["method"],
        )?;
        registry.register(Box::new(payment_amount.clone()))?;

        let payment_fees = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_payment_fees_sats",
                "Confirmed payment fees in satoshis",
            )
            .buckets(vec![0.0, 1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0]),
            &["method"],
        )?;
        registry.register(Box::new(payment_fees.clone()))?;

        Ok((payments_total, payment_amount, payment_fees))
    }

    /// Create and register database metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_db_metrics(
        registry: &Registry,
    ) -> crate::Result<(IntCounter, HistogramVec, IntGauge)> {
        let db_operations_total =
            IntCounter::new("cdk_db_operations_total", "Total database operations")?;
        registry.register(Box::new(db_operations_total.clone()))?;
        let db_operation_duration = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_db_operation_duration_seconds",
                "Database operation duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]),
            &["operation"],
        )?;
        registry.register(Box::new(db_operation_duration.clone()))?;

        let db_connections_active = IntGauge::new(
            "cdk_db_connections_active",
            "Number of active database connections",
        )?;
        registry.register(Box::new(db_connections_active.clone()))?;

        Ok((
            db_operations_total,
            db_operation_duration,
            db_connections_active,
        ))
    }

    /// Create and register error metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_error_metrics(registry: &Registry) -> crate::Result<IntCounter> {
        let errors_total = IntCounter::new("cdk_errors_total", "Total errors")?;
        registry.register(Box::new(errors_total.clone()))?;

        Ok(errors_total)
    }

    /// Create and register mint metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_mint_metrics(
        registry: &Registry,
    ) -> crate::Result<(IntCounterVec, HistogramVec, IntGaugeVec)> {
        let mint_operations_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_mint_operations_total",
                "Total number of mint operations",
            ),
            &["operation", "status"],
        )?;
        registry.register(Box::new(mint_operations_total.clone()))?;

        let mint_operation_duration = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_mint_operation_duration_seconds",
                "Duration of mint operations in seconds",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
            &["operation", "status"],
        )?;
        registry.register(Box::new(mint_operation_duration.clone()))?;

        let mint_in_flight_requests = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_mint_in_flight_requests",
                "Number of in-flight mint requests",
            ),
            &["operation"],
        )?;
        registry.register(Box::new(mint_in_flight_requests.clone()))?;

        Ok((
            mint_operations_total,
            mint_operation_duration,
            mint_in_flight_requests,
        ))
    }

    /// Create and register federation metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_federation_metrics(registry: &Registry) -> crate::Result<FederationMetricCollectors> {
        let federation_consensus_operation_count = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_consensus_operation_count",
                "Number of consensus operations accepted by a federation member",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_consensus_operation_count.clone()))?;

        let federation_sync_ready = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_sync_ready",
                "Whether a federation member reports it is ready to serve federation requests",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_sync_ready.clone()))?;

        let federation_sync_state = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_sync_state",
                "Current sync state for a federation member, exported as one-hot gauges",
            ),
            &["member", "state"],
        )?;
        registry.register(Box::new(federation_sync_state.clone()))?;

        let federation_sync_transitions_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_sync_transitions_total",
                "Cumulative entries into lagging or catch-up federation sync states",
            ),
            &["member", "transition"],
        )?;
        registry.register(Box::new(federation_sync_transitions_total.clone()))?;

        let federation_checkpoint_lag_operations = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_checkpoint_lag_operations",
                "Accepted operations after the locally trusted federation checkpoint",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_checkpoint_lag_operations.clone()))?;

        let federation_payment_observation_reports = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_payment_observation_reports",
                "Number of redacted payment observation reports grouped by status and recommended action",
            ),
            &["member", "status", "action"],
        )?;
        registry.register(Box::new(federation_payment_observation_reports.clone()))?;

        let federation_peer_connected = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_peer_connected",
                "Whether the latest private REST sync attempt to a federation peer succeeded",
            ),
            &["member", "peer"],
        )?;
        registry.register(Box::new(federation_peer_connected.clone()))?;

        let federation_peer_operation_count = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_peer_operation_count",
                "Latest consensus operation count reported by a federation peer",
            ),
            &["member", "peer"],
        )?;
        registry.register(Box::new(federation_peer_operation_count.clone()))?;

        let federation_pending_mempool_items = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_pending_mempool_items",
                "Number of locally queued or dispatched federation consensus items awaiting finalization",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_pending_mempool_items.clone()))?;

        let federation_wallet_utxos = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_wallet_utxos",
                "Federated wallet UTXO count grouped by fixed lifecycle state",
            ),
            &["member", "state"],
        )?;
        registry.register(Box::new(federation_wallet_utxos.clone()))?;

        let federation_wallet_value_sat = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_wallet_value_sat",
                "Federated wallet value in satoshis grouped by fixed UTXO lifecycle state",
            ),
            &["member", "state"],
        )?;
        registry.register(Box::new(federation_wallet_value_sat.clone()))?;

        let federation_wallet_operation_phase = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_wallet_operation_phase",
                "Federated wallet records grouped by fixed operation phase",
            ),
            &["member", "phase"],
        )?;
        registry.register(Box::new(federation_wallet_operation_phase.clone()))?;

        let federation_wallet_revealed_count = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_wallet_revealed_count",
                "Consensus-revealed descriptor child count grouped by fixed keychain",
            ),
            &["member", "keychain"],
        )?;
        registry.register(Box::new(federation_wallet_revealed_count.clone()))?;

        let federation_wallet_pending_batches = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_wallet_pending_batches",
                "Pending deterministic federated wallet batches",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_wallet_pending_batches.clone()))?;

        let federation_wallet_signing_attempts = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_wallet_signing_attempts",
                "Local FROST coordination attempts for federated wallet batches",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_wallet_signing_attempts.clone()))?;

        let federation_wallet_signing_failures = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_wallet_signing_failures",
                "Saturating local FROST signer or transport failures",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_wallet_signing_failures.clone()))?;

        let federation_wallet_broadcast_health = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_wallet_broadcast_health",
                "Federated wallet broadcast health exported as fixed one-hot gauges",
            ),
            &["member", "health"],
        )?;
        registry.register(Box::new(federation_wallet_broadcast_health.clone()))?;

        let federation_wallet_broadcast_failures = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_wallet_broadcast_failures",
                "Non-success federated wallet broadcast observations",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_wallet_broadcast_failures.clone()))?;

        let federation_bark_readiness = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_bark_readiness",
                "Federated Bark readiness exported as fixed one-hot gauges",
            ),
            &["member", "state"],
        )?;
        registry.register(Box::new(federation_bark_readiness.clone()))?;
        let federation_bark_projection_lag_operations = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_bark_projection_lag_operations",
                "Accepted Bark operations not replayed into member-local Bark state",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_bark_projection_lag_operations.clone()))?;
        let federation_bark_vtxos = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_bark_vtxos",
                "Accepted federated Bark VTXO count grouped by fixed state",
            ),
            &["member", "state"],
        )?;
        registry.register(Box::new(federation_bark_vtxos.clone()))?;
        let federation_bark_value_sat = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_bark_value_sat",
                "Accepted federated Bark value grouped by fixed VTXO state",
            ),
            &["member", "state"],
        )?;
        registry.register(Box::new(federation_bark_value_sat.clone()))?;
        let federation_bark_action_count = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_bark_action_count",
                "Active federated Bark actions grouped by fixed semantic phase",
            ),
            &["member", "phase"],
        )?;
        registry.register(Box::new(federation_bark_action_count.clone()))?;
        let federation_bark_action_max_age_operations = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_bark_action_max_age_operations",
                "Oldest active federated Bark action age grouped by fixed semantic phase",
            ),
            &["member", "phase"],
        )?;
        registry.register(Box::new(federation_bark_action_max_age_operations.clone()))?;
        let federation_bark_exposure_count = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_bark_exposure_count",
                "Federated Bark VTXO or action count grouped by fixed risk class",
            ),
            &["member", "risk"],
        )?;
        registry.register(Box::new(federation_bark_exposure_count.clone()))?;
        let federation_bark_exposure_value_sat = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_bark_exposure_value_sat",
                "Federated Bark value grouped by fixed risk class",
            ),
            &["member", "risk"],
        )?;
        registry.register(Box::new(federation_bark_exposure_value_sat.clone()))?;
        let federation_bark_protocol_records = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_bark_protocol_records",
                "Redacted federated Bark participant records grouped by fixed protocol",
            ),
            &["member", "protocol"],
        )?;
        registry.register(Box::new(federation_bark_protocol_records.clone()))?;
        let federation_bark_receive_secret_coverage = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_bark_receive_secret_coverage",
                "Redacted Bark receive-secret records grouped by fixed coverage class",
            ),
            &["member", "coverage"],
        )?;
        registry.register(Box::new(federation_bark_receive_secret_coverage.clone()))?;

        let federation_accepted_operations_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_accepted_operations_total",
                "Total number of federation operations accepted into local consensus",
            ),
            &["member", "operation"],
        )?;
        registry.register(Box::new(federation_accepted_operations_total.clone()))?;

        let federation_rejected_operations_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_rejected_operations_total",
                "Total number of federation operations rejected before local consensus acceptance",
            ),
            &["member", "operation", "reason"],
        )?;
        registry.register(Box::new(federation_rejected_operations_total.clone()))?;

        let federation_signing_share_latency = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_federation_signing_share_latency_seconds",
                "Latency for producing local federation BLS signing shares in seconds",
            )
            .buckets(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
            ]),
            &["member", "operation"],
        )?;
        registry.register(Box::new(federation_signing_share_latency.clone()))?;

        let federation_aggregation_failures_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_aggregation_failures_total",
                "Total number of wallet-side federation signature aggregation failures",
            ),
            &["member", "operation"],
        )?;
        registry.register(Box::new(federation_aggregation_failures_total.clone()))?;

        let federation_state_digest_mismatches_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_state_digest_mismatches_total",
                "Total number of detected materialized-state digest mismatches with federation peers",
            ),
            &["member", "peer"],
        )?;
        registry.register(Box::new(federation_state_digest_mismatches_total.clone()))?;

        let federation_checkpoint_work_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_checkpoint_work_total",
                "Cumulative checkpoint and restart work grouped by a fixed unit",
            ),
            &["member", "work"],
        )?;
        registry.register(Box::new(federation_checkpoint_work_total.clone()))?;

        let federation_checkpoint_catch_up_peak_in_flight_bytes = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_checkpoint_catch_up_peak_in_flight_bytes",
                "Largest encoded checkpoint catch-up page observed by the latest completed batch",
            ),
            &["member"],
        )?;
        registry.register(Box::new(
            federation_checkpoint_catch_up_peak_in_flight_bytes.clone(),
        ))?;

        let federation_checkpoint_duration = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_federation_checkpoint_duration_seconds",
                "Checkpoint and restart operation duration grouped by a fixed operation",
            )
            .buckets(vec![
                0.000_1, 0.000_5, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 15.0, 60.0,
            ]),
            &["member", "operation"],
        )?;
        registry.register(Box::new(federation_checkpoint_duration.clone()))?;

        let federation_performance_work_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_performance_work_total",
                "Cumulative process-local federation performance work grouped by a fixed unit",
            ),
            &["work"],
        )?;
        registry.register(Box::new(federation_performance_work_total.clone()))?;

        let federation_performance_operation_duration = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_federation_performance_operation_duration_seconds",
                "Process-local federation performance operation duration grouped by fixed operation and outcome",
            )
            .buckets(vec![
                0.000_01, 0.000_05, 0.000_1, 0.000_5, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5,
                1.0, 5.0, 15.0, 60.0,
            ]),
            &["operation", "outcome"],
        )?;
        registry.register(Box::new(federation_performance_operation_duration.clone()))?;

        let federation_transaction_phase_duration = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_federation_transaction_phase_duration_seconds",
                "Federated transaction phase duration grouped by fixed backend, direction, phase, and outcome",
            )
            .buckets(vec![
                0.000_01, 0.000_05, 0.000_1, 0.000_5, 0.001, 0.005, 0.01, 0.05, 0.1, 0.25,
                0.5, 1.0, 2.5, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0,
            ]),
            &["backend", "direction", "phase", "outcome"],
        )?;
        registry.register(Box::new(federation_transaction_phase_duration.clone()))?;

        Ok(FederationMetricCollectors {
            consensus_operation_count: federation_consensus_operation_count,
            sync_ready: federation_sync_ready,
            sync_state: federation_sync_state,
            sync_transitions_total: federation_sync_transitions_total,
            checkpoint_lag_operations: federation_checkpoint_lag_operations,
            payment_observation_reports: federation_payment_observation_reports,
            peer_connected: federation_peer_connected,
            peer_operation_count: federation_peer_operation_count,
            pending_mempool_items: federation_pending_mempool_items,
            wallet_utxos: federation_wallet_utxos,
            wallet_value_sat: federation_wallet_value_sat,
            wallet_operation_phase: federation_wallet_operation_phase,
            wallet_revealed_count: federation_wallet_revealed_count,
            wallet_pending_batches: federation_wallet_pending_batches,
            wallet_signing_attempts: federation_wallet_signing_attempts,
            wallet_signing_failures: federation_wallet_signing_failures,
            wallet_broadcast_health: federation_wallet_broadcast_health,
            wallet_broadcast_failures: federation_wallet_broadcast_failures,
            bark_readiness: federation_bark_readiness,
            bark_projection_lag_operations: federation_bark_projection_lag_operations,
            bark_vtxos: federation_bark_vtxos,
            bark_value_sat: federation_bark_value_sat,
            bark_action_count: federation_bark_action_count,
            bark_action_max_age_operations: federation_bark_action_max_age_operations,
            bark_exposure_count: federation_bark_exposure_count,
            bark_exposure_value_sat: federation_bark_exposure_value_sat,
            bark_protocol_records: federation_bark_protocol_records,
            bark_receive_secret_coverage: federation_bark_receive_secret_coverage,
            accepted_operations_total: federation_accepted_operations_total,
            rejected_operations_total: federation_rejected_operations_total,
            signing_share_latency: federation_signing_share_latency,
            aggregation_failures_total: federation_aggregation_failures_total,
            state_digest_mismatches_total: federation_state_digest_mismatches_total,
            checkpoint_work_total: federation_checkpoint_work_total,
            checkpoint_catch_up_peak_in_flight_bytes:
                federation_checkpoint_catch_up_peak_in_flight_bytes,
            checkpoint_duration: federation_checkpoint_duration,
            performance_work_total: federation_performance_work_total,
            performance_operation_duration: federation_performance_operation_duration,
            transaction_phase_duration: federation_transaction_phase_duration,
        })
    }

    /// Get the metrics registry
    #[must_use]
    pub fn registry(&self) -> Arc<Registry> {
        Arc::<Registry>::clone(&self.registry)
    }

    // HTTP metrics methods
    /// Record an HTTP request
    pub fn record_http_request(&self, endpoint: &str, status: &str) {
        self.http_requests_total
            .with_label_values(&[endpoint, status])
            .inc();
    }

    /// Record HTTP request duration
    pub fn record_http_request_duration(&self, duration_seconds: f64, endpoint: &str) {
        self.http_request_duration
            .with_label_values(&[endpoint])
            .observe(duration_seconds);
    }

    // Authentication metrics methods
    /// Record an authentication attempt
    pub fn record_auth_attempt(&self) {
        self.auth_attempts_total.inc();
    }

    /// Record a successful authentication
    pub fn record_auth_success(&self) {
        self.auth_successes_total.inc();
    }

    // Payment metrics methods
    /// Record a confirmed payment with known amount and fee in sats.
    pub fn record_payment(&self, method: &str, amount: f64, fee: f64) {
        self.record_payment_total(method);
        self.record_payment_amount(method, amount);
        self.record_payment_fee(method, fee);
    }

    /// Record a confirmed payment.
    pub fn record_payment_total(&self, method: &str) {
        self.payments_total.with_label_values(&[method]).inc();
    }

    /// Record a confirmed payment amount in sats.
    pub fn record_payment_amount(&self, method: &str, amount: f64) {
        self.payment_amount
            .with_label_values(&[method])
            .observe(amount);
    }

    /// Record a confirmed payment fee in sats.
    pub fn record_payment_fee(&self, method: &str, fee: f64) {
        self.payment_fees.with_label_values(&[method]).observe(fee);
    }

    // Database metrics methods
    /// Record a database operation
    pub fn record_db_operation(&self, duration_seconds: f64, op: &str) {
        self.db_operations_total.inc();
        self.db_operation_duration
            .with_label_values(&[op])
            .observe(duration_seconds);
    }

    /// Set the number of active database connections
    pub fn set_db_connections_active(&self, count: i64) {
        self.db_connections_active.set(count);
    }

    /// Increment the number of active database connections.
    pub fn increment_db_connections_active(&self) {
        self.db_connections_active.inc();
    }

    /// Decrement the number of active database connections.
    pub fn decrement_db_connections_active(&self) {
        self.db_connections_active.dec();
    }

    // Error metrics methods
    /// Record an error
    pub fn record_error(&self) {
        self.errors_total.inc();
    }

    // Mint metrics methods
    /// Record a mint operation
    pub fn record_mint_operation(&self, operation: &str, success: bool) {
        let status = if success { "success" } else { "error" };
        self.mint_operations_total
            .with_label_values(&[operation, status])
            .inc();
    }

    /// Record a mint operation with duration
    pub fn record_mint_operation_histogram(
        &self,
        operation: &str,
        success: bool,
        duration_seconds: f64,
    ) {
        let status = if success { "success" } else { "error" };
        self.mint_operation_duration
            .with_label_values(&[operation, status])
            .observe(duration_seconds);
    }

    /// Increment in-flight mint requests
    pub fn inc_in_flight_requests(&self, operation: &str) {
        self.mint_in_flight_requests
            .with_label_values(&[operation])
            .inc();
    }

    /// Decrement in-flight mint requests
    pub fn dec_in_flight_requests(&self, operation: &str) {
        self.mint_in_flight_requests
            .with_label_values(&[operation])
            .dec();
    }

    /// Set the observed federation consensus operation count for a member.
    pub fn set_federation_consensus_operation_count(&self, member: &str, count: i64) {
        self.federation_consensus_operation_count
            .with_label_values(&[member])
            .set(count);
    }

    /// Set whether a federation member reports that it is ready.
    pub fn set_federation_sync_ready(&self, member: &str, ready: bool) {
        self.federation_sync_ready
            .with_label_values(&[member])
            .set(if ready { 1 } else { 0 });
    }

    /// Set a one-hot federation sync-state gauge.
    pub fn set_federation_sync_state(&self, member: &str, state: &str, active: bool) {
        self.federation_sync_state
            .with_label_values(&[member, state])
            .set(if active { 1 } else { 0 });
    }

    /// Record entry into a fixed federation synchronization state.
    pub fn record_federation_sync_transition(
        &self,
        member: &str,
        transition: FederationSyncTransition,
    ) {
        self.federation_sync_transitions_total
            .with_label_values(&[member, transition.as_str()])
            .inc();
    }

    /// Set the operation count after the locally trusted checkpoint.
    pub fn set_federation_checkpoint_lag_operations(&self, member: &str, count: i64) {
        self.federation_checkpoint_lag_operations
            .with_label_values(&[member])
            .set(count);
    }

    /// Set the count of redacted payment observation reports for a member.
    pub fn set_federation_payment_observation_report_count(
        &self,
        member: &str,
        status: &str,
        action: &str,
        count: i64,
    ) {
        self.federation_payment_observation_reports
            .with_label_values(&[member, status, action])
            .set(count);
    }

    /// Set whether the latest private REST sync attempt to a federation peer succeeded.
    pub fn set_federation_peer_connected(&self, member: &str, peer: &str, connected: bool) {
        self.federation_peer_connected
            .with_label_values(&[member, peer])
            .set(if connected { 1 } else { 0 });
    }

    /// Set the latest operation count reported by a federation peer.
    pub fn set_federation_peer_operation_count(&self, member: &str, peer: &str, count: i64) {
        self.federation_peer_operation_count
            .with_label_values(&[member, peer])
            .set(count);
    }

    /// Set the number of federation consensus items awaiting finalization.
    pub fn set_federation_pending_mempool_items(&self, member: &str, count: i64) {
        self.federation_pending_mempool_items
            .with_label_values(&[member])
            .set(count);
    }

    /// Set a fixed-state federated wallet UTXO count and value.
    pub fn set_federation_wallet_utxos(
        &self,
        member: &str,
        state: &str,
        count: i64,
        value_sat: i64,
    ) {
        self.federation_wallet_utxos
            .with_label_values(&[member, state])
            .set(count);
        self.federation_wallet_value_sat
            .with_label_values(&[member, state])
            .set(value_sat);
    }

    /// Set a fixed federated wallet operation-phase count.
    pub fn set_federation_wallet_operation_phase(&self, member: &str, phase: &str, count: i64) {
        self.federation_wallet_operation_phase
            .with_label_values(&[member, phase])
            .set(count);
    }

    /// Set the consensus-revealed descriptor child count for a fixed keychain.
    pub fn set_federation_wallet_revealed_count(&self, member: &str, keychain: &str, count: i64) {
        self.federation_wallet_revealed_count
            .with_label_values(&[member, keychain])
            .set(count);
    }

    /// Set bounded local federated wallet liveness counters.
    pub fn set_federation_wallet_liveness(
        &self,
        member: &str,
        pending_batches: i64,
        signing_attempts: i64,
        signing_failures: i64,
    ) {
        self.federation_wallet_pending_batches
            .with_label_values(&[member])
            .set(pending_batches);
        self.federation_wallet_signing_attempts
            .with_label_values(&[member])
            .set(signing_attempts);
        self.federation_wallet_signing_failures
            .with_label_values(&[member])
            .set(signing_failures);
    }

    /// Set one fixed one-hot broadcast-health gauge.
    pub fn set_federation_wallet_broadcast_health(&self, member: &str, health: &str, active: bool) {
        self.federation_wallet_broadcast_health
            .with_label_values(&[member, health])
            .set(if active { 1 } else { 0 });
    }

    /// Set the number of non-success federated wallet broadcast observations.
    pub fn set_federation_wallet_broadcast_failures(&self, member: &str, count: i64) {
        self.federation_wallet_broadcast_failures
            .with_label_values(&[member])
            .set(count);
    }

    /// Set one fixed one-hot federated Bark readiness gauge.
    pub fn set_federation_bark_readiness(&self, member: &str, state: &str, active: bool) {
        self.federation_bark_readiness
            .with_label_values(&[member, state])
            .set(if active { 1 } else { 0 });
    }

    /// Set accepted operations not yet replayed into local Bark state.
    pub fn set_federation_bark_projection_lag(&self, member: &str, count: i64) {
        self.federation_bark_projection_lag_operations
            .with_label_values(&[member])
            .set(count);
    }

    /// Set a fixed-state federated Bark VTXO count and value.
    pub fn set_federation_bark_vtxos(&self, member: &str, state: &str, count: i64, value_sat: i64) {
        self.federation_bark_vtxos
            .with_label_values(&[member, state])
            .set(count);
        self.federation_bark_value_sat
            .with_label_values(&[member, state])
            .set(value_sat);
    }

    /// Set active Bark action count and maximum age for one fixed phase.
    pub fn set_federation_bark_action_phase(
        &self,
        member: &str,
        phase: &str,
        count: i64,
        max_age_operations: i64,
    ) {
        self.federation_bark_action_count
            .with_label_values(&[member, phase])
            .set(count);
        self.federation_bark_action_max_age_operations
            .with_label_values(&[member, phase])
            .set(max_age_operations);
    }

    /// Set Bark count and value for one fixed risk class.
    pub fn set_federation_bark_exposure(
        &self,
        member: &str,
        risk: &str,
        count: i64,
        value_sat: i64,
    ) {
        self.federation_bark_exposure_count
            .with_label_values(&[member, risk])
            .set(count);
        self.federation_bark_exposure_value_sat
            .with_label_values(&[member, risk])
            .set(value_sat);
    }

    /// Set one redacted participant-protocol record count.
    pub fn set_federation_bark_protocol_records(&self, member: &str, protocol: &str, count: i64) {
        self.federation_bark_protocol_records
            .with_label_values(&[member, protocol])
            .set(count);
    }

    /// Set one redacted receive-secret coverage count.
    pub fn set_federation_bark_receive_secret_coverage(
        &self,
        member: &str,
        coverage: &str,
        count: i64,
    ) {
        self.federation_bark_receive_secret_coverage
            .with_label_values(&[member, coverage])
            .set(count);
    }

    /// Record a federation operation accepted into local consensus.
    pub fn record_federation_accepted_operation(&self, member: &str, operation: &str) {
        self.federation_accepted_operations_total
            .with_label_values(&[member, operation])
            .inc();
    }

    /// Record a federation operation rejected before local consensus acceptance.
    pub fn record_federation_rejected_operation(
        &self,
        member: &str,
        operation: &str,
        reason: &str,
    ) {
        self.federation_rejected_operations_total
            .with_label_values(&[member, operation, reason])
            .inc();
    }

    /// Record local federation signing-share production latency.
    pub fn record_federation_signing_share_latency(
        &self,
        member: &str,
        operation: &str,
        duration_seconds: f64,
    ) {
        self.federation_signing_share_latency
            .with_label_values(&[member, operation])
            .observe(duration_seconds);
    }

    /// Record a wallet-side federation signature aggregation failure.
    pub fn record_federation_aggregation_failure(&self, member: &str, operation: &str) {
        self.federation_aggregation_failures_total
            .with_label_values(&[member, operation])
            .inc();
    }

    /// Record a materialized-state digest mismatch with a federation peer.
    pub fn record_federation_state_digest_mismatch(&self, member: &str, peer: &str) {
        self.federation_state_digest_mismatches_total
            .with_label_values(&[member, peer])
            .inc();
    }

    /// Add a bounded unit of checkpoint or restart work.
    pub fn record_federation_checkpoint_work(
        &self,
        member: &str,
        work: FederationCheckpointWork,
        count: u64,
    ) {
        self.federation_checkpoint_work_total
            .with_label_values(&[member, work.as_str()])
            .inc_by(count);
    }

    /// Set the largest encoded page observed by the latest checkpoint catch-up batch.
    pub fn set_federation_checkpoint_catch_up_peak_in_flight_bytes(
        &self,
        member: &str,
        bytes: i64,
    ) {
        self.federation_checkpoint_catch_up_peak_in_flight_bytes
            .with_label_values(&[member])
            .set(bytes);
    }

    /// Record the duration of one fixed checkpoint or restart operation.
    pub fn record_federation_checkpoint_duration(
        &self,
        member: &str,
        operation: FederationCheckpointOperation,
        duration_seconds: f64,
    ) {
        self.federation_checkpoint_duration
            .with_label_values(&[member, operation.as_str()])
            .observe(duration_seconds);
    }

    /// Add a bounded unit of process-local federation performance work.
    pub fn record_federation_performance_work(&self, work: FederationPerformanceWork, count: u64) {
        self.federation_performance_work_total
            .with_label_values(&[work.as_str()])
            .inc_by(count);
    }

    /// Record one process-local fixed federation operation and its outcome.
    pub fn record_federation_performance_operation(
        &self,
        operation: FederationPerformanceOperation,
        outcome: FederationPerformanceOutcome,
        duration_seconds: f64,
    ) {
        self.federation_performance_operation_duration
            .with_label_values(&[operation.as_str(), outcome.as_str()])
            .observe(duration_seconds);
    }

    /// Record one fixed federated transaction phase duration.
    ///
    /// Callers must use bounded, compile-time label domains. Correlation IDs,
    /// quote IDs, payment hashes, transaction IDs, and peer IDs belong in
    /// structured traces rather than Prometheus labels.
    pub fn record_federation_transaction_phase_duration(
        &self,
        backend: &str,
        direction: &str,
        phase: &str,
        outcome: &str,
        duration_seconds: f64,
    ) {
        self.federation_transaction_phase_duration
            .with_label_values(&[backend, direction, phase, outcome])
            .observe(duration_seconds);
    }
}

impl Default for CdkMetrics {
    fn default() -> Self {
        Self::new().expect("Failed to create default CdkMetrics")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration;

    use prometheus::core::Collector;

    use super::{
        CdkMetrics, FederationCheckpointOperation, FederationCheckpointWork,
        FederationPerformanceOperation, FederationPerformanceOutcome, FederationPerformanceWork,
        FederationSyncTransition, MintMetricGuard, METRICS,
    };

    static METRICS_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn metrics_lock() -> MutexGuard<'static, ()> {
        METRICS_TEST_LOCK
            .lock()
            .expect("metrics test lock should not be poisoned")
    }

    #[test]
    fn mint_metric_guard_records_success_and_balances_in_flight() {
        let _lock = metrics_lock();
        let operation = "test_guard_success";
        let in_flight = METRICS
            .mint_in_flight_requests
            .with_label_values(&[operation]);
        let success_count = METRICS
            .mint_operations_total
            .with_label_values(&[operation, "success"]);
        let error_count = METRICS
            .mint_operations_total
            .with_label_values(&[operation, "error"]);
        let duration = METRICS
            .mint_operation_duration
            .with_label_values(&[operation, "success"]);

        let in_flight_before = in_flight.get();
        let success_count_before = success_count.get();
        let error_count_before = error_count.get();
        let duration_count_before = duration.get_sample_count();
        let errors_before = METRICS.errors_total.get();

        let guard = MintMetricGuard::new(operation);
        assert_eq!(in_flight.get(), in_flight_before + 1);

        std::thread::sleep(Duration::from_millis(1));
        guard.record(true);

        assert_eq!(in_flight.get(), in_flight_before);
        assert_eq!(success_count.get(), success_count_before + 1);
        assert_eq!(error_count.get(), error_count_before);
        assert_eq!(duration.get_sample_count(), duration_count_before + 1);
        assert_eq!(METRICS.errors_total.get(), errors_before);
    }

    #[test]
    fn mint_metric_guard_records_error_and_global_error_count() {
        let _lock = metrics_lock();
        let operation = "test_guard_error";
        let in_flight = METRICS
            .mint_in_flight_requests
            .with_label_values(&[operation]);
        let error_count = METRICS
            .mint_operations_total
            .with_label_values(&[operation, "error"]);
        let duration = METRICS
            .mint_operation_duration
            .with_label_values(&[operation, "error"]);

        let in_flight_before = in_flight.get();
        let error_count_before = error_count.get();
        let duration_count_before = duration.get_sample_count();
        let errors_before = METRICS.errors_total.get();

        let guard = MintMetricGuard::new(operation);
        assert_eq!(in_flight.get(), in_flight_before + 1);

        guard.record(false);

        assert_eq!(in_flight.get(), in_flight_before);
        assert_eq!(error_count.get(), error_count_before + 1);
        assert_eq!(duration.get_sample_count(), duration_count_before + 1);
        assert_eq!(METRICS.errors_total.get(), errors_before + 1);
    }

    #[test]
    fn mint_metric_guard_drop_without_record_only_balances_in_flight() {
        let _lock = metrics_lock();
        let operation = "test_guard_drop_without_record";
        let in_flight = METRICS
            .mint_in_flight_requests
            .with_label_values(&[operation]);
        let success_count = METRICS
            .mint_operations_total
            .with_label_values(&[operation, "success"]);
        let error_count = METRICS
            .mint_operations_total
            .with_label_values(&[operation, "error"]);
        let success_duration = METRICS
            .mint_operation_duration
            .with_label_values(&[operation, "success"]);
        let error_duration = METRICS
            .mint_operation_duration
            .with_label_values(&[operation, "error"]);

        let in_flight_before = in_flight.get();
        let success_count_before = success_count.get();
        let error_count_before = error_count.get();
        let success_duration_before = success_duration.get_sample_count();
        let error_duration_before = error_duration.get_sample_count();
        let errors_before = METRICS.errors_total.get();

        {
            let _guard = MintMetricGuard::new(operation);
            assert_eq!(in_flight.get(), in_flight_before + 1);
        }

        assert_eq!(in_flight.get(), in_flight_before);
        assert_eq!(success_count.get(), success_count_before);
        assert_eq!(error_count.get(), error_count_before);
        assert_eq!(success_duration.get_sample_count(), success_duration_before);
        assert_eq!(error_duration.get_sample_count(), error_duration_before);
        assert_eq!(METRICS.errors_total.get(), errors_before);
    }

    #[test]
    fn payment_metrics_are_labeled_by_method() {
        let _lock = metrics_lock();
        let method = "test_payment_method";
        let payments = METRICS.payments_total.with_label_values(&[method]);
        let amount = METRICS.payment_amount.with_label_values(&[method]);
        let fee = METRICS.payment_fees.with_label_values(&[method]);

        let payments_before = payments.get();
        let amount_count_before = amount.get_sample_count();
        let fee_count_before = fee.get_sample_count();

        METRICS.record_payment(method, 21.0, 1.0);

        assert_eq!(payments.get(), payments_before + 1);
        assert_eq!(amount.get_sample_count(), amount_count_before + 1);
        assert_eq!(fee.get_sample_count(), fee_count_before + 1);
    }

    #[test]
    fn federation_member_status_metrics_are_labeled_by_member() {
        let metrics = CdkMetrics::new().expect("metrics should be created");
        let member = "2";
        let peer = "3";

        metrics.set_federation_consensus_operation_count(member, 7);
        metrics.set_federation_sync_ready(member, false);
        metrics.set_federation_sync_state(member, "healthy", false);
        metrics.set_federation_sync_state(member, "lagging", true);
        metrics.set_federation_checkpoint_lag_operations(member, 5);
        metrics.set_federation_peer_connected(member, peer, true);
        metrics.set_federation_peer_operation_count(member, peer, 9);
        metrics.set_federation_pending_mempool_items(member, 2);
        metrics.record_federation_state_digest_mismatch(member, peer);

        assert_eq!(
            metrics
                .federation_consensus_operation_count
                .with_label_values(&[member])
                .get(),
            7
        );
        assert_eq!(
            metrics
                .federation_sync_ready
                .with_label_values(&[member])
                .get(),
            0
        );
        assert_eq!(
            metrics
                .federation_sync_state
                .with_label_values(&[member, "healthy"])
                .get(),
            0
        );
        assert_eq!(
            metrics
                .federation_sync_state
                .with_label_values(&[member, "lagging"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .federation_checkpoint_lag_operations
                .with_label_values(&[member])
                .get(),
            5
        );
        assert_eq!(
            metrics
                .federation_peer_connected
                .with_label_values(&[member, peer])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .federation_peer_operation_count
                .with_label_values(&[member, peer])
                .get(),
            9
        );
        assert_eq!(
            metrics
                .federation_pending_mempool_items
                .with_label_values(&[member])
                .get(),
            2
        );
        assert_eq!(
            metrics
                .federation_state_digest_mismatches_total
                .with_label_values(&[member, peer])
                .get(),
            1
        );
    }

    #[test]
    fn federation_checkpoint_metrics_use_fixed_work_and_operation_labels() {
        let metrics = CdkMetrics::new().expect("metrics should be created");
        let member = "2";

        metrics.record_federation_checkpoint_work(
            member,
            FederationCheckpointWork::StartupTailJournalRowsLoaded,
            7,
        );
        metrics.record_federation_checkpoint_duration(
            member,
            FederationCheckpointOperation::StartupTail,
            0.25,
        );
        metrics.set_federation_checkpoint_catch_up_peak_in_flight_bytes(member, 4096);

        assert_eq!(
            metrics
                .federation_checkpoint_work_total
                .with_label_values(&[member, "startup_tail_journal_rows_loaded"])
                .get(),
            7
        );
        let duration = metrics
            .federation_checkpoint_duration
            .with_label_values(&[member, "startup_tail"]);
        assert_eq!(duration.get_sample_count(), 1);
        assert_eq!(duration.get_sample_sum(), 0.25);
        assert_eq!(
            metrics
                .federation_checkpoint_catch_up_peak_in_flight_bytes
                .with_label_values(&[member])
                .get(),
            4096
        );
    }

    #[test]
    fn federation_performance_metrics_have_fixed_process_local_cardinality() {
        let metrics = CdkMetrics::new().expect("metrics should be created");

        for work in FederationPerformanceWork::ALL {
            metrics.record_federation_performance_work(work, 1);
        }
        for operation in FederationPerformanceOperation::ALL {
            for outcome in FederationPerformanceOutcome::ALL {
                metrics.record_federation_performance_operation(operation, outcome, 0.001);
            }
        }

        let work_families = metrics.federation_performance_work_total.collect();
        assert_eq!(work_families.len(), 1);
        assert_eq!(
            work_families[0].get_metric().len(),
            FederationPerformanceWork::ALL.len()
        );
        for metric in work_families[0].get_metric() {
            let labels = metric
                .get_label()
                .iter()
                .map(|label| label.name())
                .collect::<Vec<_>>();
            assert_eq!(labels, vec!["work"]);
        }

        let operation_families = metrics.federation_performance_operation_duration.collect();
        assert_eq!(operation_families.len(), 1);
        assert_eq!(
            operation_families[0].get_metric().len(),
            FederationPerformanceOperation::ALL.len() * FederationPerformanceOutcome::ALL.len()
        );
        for metric in operation_families[0].get_metric() {
            let labels = metric
                .get_label()
                .iter()
                .map(|label| label.name())
                .collect::<Vec<_>>();
            assert_eq!(labels, vec!["operation", "outcome"]);
        }
    }

    #[test]
    fn federation_transaction_phase_metric_uses_only_bounded_labels() {
        let metrics = CdkMetrics::new().expect("metrics should be created");

        metrics.record_federation_transaction_phase_duration(
            "bark",
            "outgoing",
            "bark_lightning_initiation",
            "success",
            0.125,
        );

        let families = metrics.federation_transaction_phase_duration.collect();
        assert_eq!(families.len(), 1);
        assert_eq!(families[0].get_metric().len(), 1);
        let metric = &families[0].get_metric()[0];
        let labels = metric
            .get_label()
            .iter()
            .map(|label| label.name())
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["backend", "direction", "outcome", "phase"]);
        assert_eq!(metric.get_histogram().get_sample_count(), 1);
        assert_eq!(metric.get_histogram().get_sample_sum(), 0.125);
    }

    #[test]
    fn federation_performance_enum_labels_are_unique() {
        let work_labels = FederationPerformanceWork::ALL
            .into_iter()
            .map(FederationPerformanceWork::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(work_labels.len(), FederationPerformanceWork::ALL.len());

        let operation_labels = FederationPerformanceOperation::ALL
            .into_iter()
            .map(FederationPerformanceOperation::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            operation_labels.len(),
            FederationPerformanceOperation::ALL.len()
        );
    }

    #[test]
    fn federation_sync_transition_metrics_use_fixed_labels() {
        let metrics = CdkMetrics::new().expect("metrics should be created");
        let member = "2";

        metrics.record_federation_sync_transition(member, FederationSyncTransition::Lagging);
        metrics.record_federation_sync_transition(member, FederationSyncTransition::CatchingUp);

        assert_eq!(
            metrics
                .federation_sync_transitions_total
                .with_label_values(&[member, "lagging"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .federation_sync_transitions_total
                .with_label_values(&[member, "catching_up"])
                .get(),
            1
        );
    }

    #[test]
    fn federation_payment_report_metrics_are_labeled_by_status_and_action() {
        let metrics = CdkMetrics::new().expect("metrics should be created");

        metrics.set_federation_payment_observation_report_count("2", "conflicting", "inspect", 3);

        assert_eq!(
            metrics
                .federation_payment_observation_reports
                .with_label_values(&["2", "conflicting", "inspect"])
                .get(),
            3
        );
    }

    #[test]
    fn federation_wallet_metrics_use_only_fixed_redacted_labels() {
        let metrics = CdkMetrics::new().expect("metrics should be created");
        metrics.set_federation_wallet_utxos("2", "reserved", 3, 21_000);
        metrics.set_federation_wallet_operation_phase("2", "signing", 2);
        metrics.set_federation_wallet_revealed_count("2", "external", 9);
        metrics.set_federation_wallet_revealed_count("2", "internal", 4);
        metrics.set_federation_wallet_liveness("2", 1, 5, 1);
        for health in [
            "idle",
            "awaiting_authorization",
            "broadcasting",
            "confirming",
            "degraded",
        ] {
            metrics.set_federation_wallet_broadcast_health("2", health, health == "degraded");
        }
        metrics.set_federation_wallet_broadcast_failures("2", 1);

        let scrape = prometheus::TextEncoder::new()
            .encode_to_string(&metrics.registry().gather())
            .expect("encode metrics");
        assert!(scrape.contains("state=\"reserved\""));
        assert!(scrape.contains("phase=\"signing\""));
        assert!(scrape.contains("keychain=\"external\""));
        assert!(scrape.contains("health=\"degraded\""));
        for secret in [
            "quote-operator-secret",
            "bc1p-sensitive-address",
            "02000000-sensitive-transaction",
            "wallet-mnemonic-secret",
            "wallet-password-secret",
            "/sensitive/local/path",
        ] {
            assert!(!scrape.contains(secret));
        }
        assert_eq!(
            metrics.federation_wallet_utxos.collect()[0]
                .get_metric()
                .len(),
            1,
            "only the one fixed state explicitly recorded by this fixture exists"
        );
        assert_eq!(
            metrics.federation_wallet_broadcast_health.collect()[0]
                .get_metric()
                .len(),
            5
        );
    }

    #[test]
    fn federation_bark_metrics_use_only_fixed_redacted_labels() {
        let metrics = CdkMetrics::new().expect("metrics should be created");
        for readiness in ["ready", "behind_consensus", "degraded"] {
            metrics.set_federation_bark_readiness("2", readiness, readiness == "degraded");
        }
        metrics.set_federation_bark_projection_lag("2", 3);
        metrics.set_federation_bark_vtxos("2", "reserved", 2, 42_000);
        metrics.set_federation_bark_action_phase("2", "send_htlcs_cosigned", 1, 17);
        metrics.set_federation_bark_exposure("2", "ambiguous", 1, 21_000);
        metrics.set_federation_bark_protocol_records("2", "virtual_musig2", 2);
        metrics.set_federation_bark_receive_secret_coverage("2", "locally_retained", 3);

        let scrape = prometheus::TextEncoder::new()
            .encode_to_string(&metrics.registry().gather())
            .expect("encode metrics");
        for expected in [
            "state=\"degraded\"",
            "state=\"reserved\"",
            "phase=\"send_htlcs_cosigned\"",
            "risk=\"ambiguous\"",
            "protocol=\"virtual_musig2\"",
            "coverage=\"locally_retained\"",
        ] {
            assert!(scrape.contains(expected));
        }
        for secret in [
            "lnbc-secret-invoice",
            "receive-preimage-secret",
            "frost-nonce-secret",
            "ark-credential-secret",
            "/sensitive/bark/path",
        ] {
            assert!(!scrape.contains(secret));
        }
    }

    #[test]
    fn federation_operation_metrics_are_low_cardinality() {
        let metrics = CdkMetrics::new().expect("metrics should be created");

        metrics.record_federation_accepted_operation("2", "mint");
        metrics.record_federation_rejected_operation("2", "swap", "conflict");
        metrics.record_federation_signing_share_latency("2", "mint", 0.01);
        metrics.record_federation_aggregation_failure("wallet", "restore");

        assert_eq!(
            metrics
                .federation_accepted_operations_total
                .with_label_values(&["2", "mint"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .federation_rejected_operations_total
                .with_label_values(&["2", "swap", "conflict"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .federation_signing_share_latency
                .with_label_values(&["2", "mint"])
                .get_sample_count(),
            1
        );
        assert_eq!(
            metrics
                .federation_aggregation_failures_total
                .with_label_values(&["wallet", "restore"])
                .get(),
            1
        );

        let scrape = prometheus::TextEncoder::new()
            .encode_to_string(&metrics.registry().gather())
            .expect("encode metrics");
        assert!(!scrape.contains("quote="));
        assert!(!scrape.contains("invoice="));
        assert!(!scrape.contains("proof="));
        assert!(!scrape.contains("secret="));
        assert!(!scrape.contains("token="));
        assert!(!scrape.contains("ip="));
    }
}
