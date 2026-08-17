//! Low-cardinality metrics and correlated traces for federated transaction latency.

use std::time::{Duration, Instant};

/// Backend or subsystem that owns one measured phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FederationLatencyBackend {
    Cashu,
    Bark,
    Bdk,
    Consensus,
    AlephBft,
}

impl FederationLatencyBackend {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Cashu => "cashu",
            Self::Bark => "bark",
            Self::Bdk => "bdk",
            Self::Consensus => "consensus",
            Self::AlephBft => "aleph_bft",
        }
    }
}

/// Direction of value flow for one measured phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FederationLatencyDirection {
    Incoming,
    Outgoing,
    Internal,
}

impl FederationLatencyDirection {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Incoming => "incoming",
            Self::Outgoing => "outgoing",
            Self::Internal => "internal",
        }
    }
}

/// Stable correlation domain used only in structured traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // One shared bounded domain spans independently feature-gated backends.
pub(crate) enum FederationLatencyCorrelation {
    Quote,
    Operation,
    Action,
    Proposal,
    Batch,
    Wallet,
    Session,
}

impl FederationLatencyCorrelation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Quote => "quote",
            Self::Operation => "operation",
            Self::Action => "action",
            Self::Proposal => "proposal",
            Self::Batch => "batch",
            Self::Wallet => "wallet",
            Self::Session => "session",
        }
    }
}

/// Fixed phase domain for payment and consensus latency attribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // One shared bounded domain spans independently feature-gated backends.
pub(crate) enum FederationLatencyPhase {
    CashuSwap,
    CashuMint,
    CashuMelt,
    MintPaymentWait,
    PaymentCoordinatorDispatch,
    PaymentObservationRound,
    BackendPaymentProbe,
    BarkProjectionReady,
    BarkSecretReplication,
    BarkInvoicePreparation,
    BarkReceiveHtlcObservation,
    BarkReceiveServerWait,
    BarkEvidenceCollection,
    BarkReceiveClaim,
    BarkReceiveAcknowledge,
    BarkSendPlanPreparation,
    BarkHtlcCosigning,
    BarkLightningInitiation,
    BarkLightningTerminalPoll,
    BarkLightningServerWait,
    BarkResultValidation,
    BdkAddressAllocation,
    OnchainWalletSync,
    BdkWalletScan,
    BdkLivenessRound,
    BdkFeeEstimation,
    BdkPsbtBuild,
    BdkProposalPreparation,
    FrostCommitmentRound,
    FrostSignatureRound,
    BdkTransactionFinalization,
    BdkBroadcastRound,
    BdkBroadcast,
    BdkConfirmationPoll,
    ConsensusSubmission,
    AlephBftNetworkPump,
    AlephBftBackupProbe,
    AlephBftBackupRecovery,
}

impl FederationLatencyPhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CashuSwap => "cashu_swap",
            Self::CashuMint => "cashu_mint",
            Self::CashuMelt => "cashu_melt",
            Self::MintPaymentWait => "mint_payment_wait",
            Self::PaymentCoordinatorDispatch => "payment_coordinator_dispatch",
            Self::PaymentObservationRound => "payment_observation_round",
            Self::BackendPaymentProbe => "backend_payment_probe",
            Self::BarkProjectionReady => "bark_projection_ready",
            Self::BarkSecretReplication => "bark_secret_replication",
            Self::BarkInvoicePreparation => "bark_invoice_preparation",
            Self::BarkReceiveHtlcObservation => "bark_receive_htlc_observation",
            Self::BarkReceiveServerWait => "bark_receive_server_wait",
            Self::BarkEvidenceCollection => "bark_evidence_collection",
            Self::BarkReceiveClaim => "bark_receive_claim",
            Self::BarkReceiveAcknowledge => "bark_receive_acknowledge",
            Self::BarkSendPlanPreparation => "bark_send_plan_preparation",
            Self::BarkHtlcCosigning => "bark_htlc_cosigning",
            Self::BarkLightningInitiation => "bark_lightning_initiation",
            Self::BarkLightningTerminalPoll => "bark_lightning_terminal_poll",
            Self::BarkLightningServerWait => "bark_lightning_server_wait",
            Self::BarkResultValidation => "bark_result_validation",
            Self::BdkAddressAllocation => "bdk_address_allocation",
            Self::OnchainWalletSync => "onchain_wallet_sync",
            Self::BdkWalletScan => "bdk_wallet_scan",
            Self::BdkLivenessRound => "bdk_liveness_round",
            Self::BdkFeeEstimation => "bdk_fee_estimation",
            Self::BdkPsbtBuild => "bdk_psbt_build",
            Self::BdkProposalPreparation => "bdk_proposal_preparation",
            Self::FrostCommitmentRound => "frost_commitment_round",
            Self::FrostSignatureRound => "frost_signature_round",
            Self::BdkTransactionFinalization => "bdk_transaction_finalization",
            Self::BdkBroadcastRound => "bdk_broadcast_round",
            Self::BdkBroadcast => "bdk_broadcast",
            Self::BdkConfirmationPoll => "bdk_confirmation_poll",
            Self::ConsensusSubmission => "consensus_submission",
            Self::AlephBftNetworkPump => "aleph_bft_network_pump",
            Self::AlephBftBackupProbe => "aleph_bft_backup_probe",
            Self::AlephBftBackupRecovery => "aleph_bft_backup_recovery",
        }
    }
}

/// Bounded terminal classification for one measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FederationLatencyOutcome {
    Success,
    Pending,
    Error,
    Timeout,
    Skipped,
    Cancelled,
}

impl FederationLatencyOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Pending => "pending",
            Self::Error => "error",
            Self::Timeout => "timeout",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) const fn from_success(success: bool) -> Self {
        if success {
            Self::Success
        } else {
            Self::Error
        }
    }
}

/// A timer that reports cancellation if control exits before an explicit finish.
#[derive(Debug)]
pub(crate) struct FederationLatencyTimer {
    correlation: FederationLatencyCorrelation,
    correlation_id: String,
    backend: FederationLatencyBackend,
    direction: FederationLatencyDirection,
    phase: FederationLatencyPhase,
    started_at: Instant,
    finished: bool,
}

impl FederationLatencyTimer {
    pub(crate) fn start(
        correlation: FederationLatencyCorrelation,
        correlation_id: impl Into<String>,
        backend: FederationLatencyBackend,
        direction: FederationLatencyDirection,
        phase: FederationLatencyPhase,
    ) -> Self {
        let correlation_id = correlation_id.into();
        tracing::trace!(
            target: "cdk_axum::federation::transaction_latency",
            event = "start",
            correlation = correlation.as_str(),
            correlation_id = %correlation_id,
            backend = backend.as_str(),
            direction = direction.as_str(),
            phase = phase.as_str(),
            "Federated transaction latency phase started"
        );
        Self {
            correlation,
            correlation_id,
            backend,
            direction,
            phase,
            started_at: Instant::now(),
            finished: false,
        }
    }

    pub(crate) fn finish(mut self, outcome: FederationLatencyOutcome) -> Duration {
        let duration = self.started_at.elapsed();
        self.finished = true;
        record_duration(
            self.correlation,
            &self.correlation_id,
            self.backend,
            self.direction,
            self.phase,
            outcome,
            duration,
        );
        duration
    }
}

impl Drop for FederationLatencyTimer {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        record_duration(
            self.correlation,
            &self.correlation_id,
            self.backend,
            self.direction,
            self.phase,
            FederationLatencyOutcome::Cancelled,
            self.started_at.elapsed(),
        );
    }
}

pub(crate) fn record_duration(
    correlation: FederationLatencyCorrelation,
    correlation_id: &str,
    backend: FederationLatencyBackend,
    direction: FederationLatencyDirection,
    phase: FederationLatencyPhase,
    outcome: FederationLatencyOutcome,
    duration: Duration,
) {
    tracing::trace!(
        target: "cdk_axum::federation::transaction_latency",
        event = "finish",
        correlation = correlation.as_str(),
        correlation_id = %correlation_id,
        backend = backend.as_str(),
        direction = direction.as_str(),
        phase = phase.as_str(),
        outcome = outcome.as_str(),
        duration_us = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX),
        duration_ms = duration.as_secs_f64() * 1_000.0,
        "Federated transaction latency phase finished"
    );
    #[cfg(feature = "prometheus")]
    cdk_prometheus::METRICS.record_federation_transaction_phase_duration(
        backend.as_str(),
        direction.as_str(),
        phase.as_str(),
        outcome.as_str(),
        duration.as_secs_f64(),
    );
}
