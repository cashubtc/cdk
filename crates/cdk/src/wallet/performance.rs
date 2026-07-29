use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WalletPerformanceStage {
    #[cfg(feature = "federation")]
    MemberShareBatchVerification,
    #[cfg(feature = "federation")]
    MemberShareScalarFallback,
    #[cfg(feature = "federation")]
    ShareAggregation,
    FinalBlindSignatureVerification,
    FinalProofVerification,
}

impl WalletPerformanceStage {
    #[cfg(all(test, feature = "federation"))]
    const ALL: [Self; 5] = [
        Self::MemberShareBatchVerification,
        Self::MemberShareScalarFallback,
        Self::ShareAggregation,
        Self::FinalBlindSignatureVerification,
        Self::FinalProofVerification,
    ];

    #[cfg(all(test, not(feature = "federation")))]
    const ALL: [Self; 2] = [
        Self::FinalBlindSignatureVerification,
        Self::FinalProofVerification,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            #[cfg(feature = "federation")]
            Self::MemberShareBatchVerification => "member_share_batch_verification",
            #[cfg(feature = "federation")]
            Self::MemberShareScalarFallback => "member_share_scalar_fallback",
            #[cfg(feature = "federation")]
            Self::ShareAggregation => "share_aggregation",
            Self::FinalBlindSignatureVerification => "final_blind_signature_verification",
            Self::FinalProofVerification => "final_proof_verification",
        }
    }
}

pub(crate) fn record_wallet_performance_stage(
    stage: WalletPerformanceStage,
    started_at: Instant,
    work_items: usize,
    success: bool,
) {
    tracing::debug!(
        target: "cdk::wallet::federation_performance",
        stage = stage.as_str(),
        work_items,
        duration_seconds = started_at.elapsed().as_secs_f64(),
        success,
        "Federation wallet performance stage completed"
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::WalletPerformanceStage;

    #[test]
    fn wallet_performance_stages_have_unique_fixed_labels() {
        let labels = WalletPerformanceStage::ALL
            .into_iter()
            .map(WalletPerformanceStage::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(labels.len(), WalletPerformanceStage::ALL.len());
    }
}
