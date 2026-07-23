//! Bounded, redacted operator status for the federated Bark backend.

use serde::{Deserialize, Serialize};

use super::{
    FederationBarkActionId, FederationBarkActionKind, FederationBarkDigest,
    FederationBarkSemanticPhase, FederationBarkVtxoState, FederationError, MemberId, Result,
    FEDERATION_REST_API_VERSION,
};

/// Status schema version for the private federated Bark operator view.
pub const FEDERATION_BARK_OPERATOR_STATUS_VERSION: u16 = 1;

/// Replay-only request to reconcile accepted Bark state into the local wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkReconcileRequest {
    /// Private REST payload version.
    pub version: u16,
}

impl Default for FederationBarkReconcileRequest {
    fn default() -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
        }
    }
}

impl FederationBarkReconcileRequest {
    /// Reject requests from incompatible private API versions.
    pub fn validate(self) -> Result<()> {
        validate_rest_version(self.version)
    }
}

/// Redacted result of replay-only Bark reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkReconcileResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Accepted Bark projection frontier.
    pub accepted_frontier: u64,
    /// Local Bark frontier before reconciliation.
    pub local_frontier_before: u64,
    /// Local Bark frontier after reconciliation.
    pub local_frontier_after: u64,
    /// Stable post-reconciliation readiness name.
    pub readiness: String,
    /// Bounded post-reconciliation degraded reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
}

/// Guarded request to retry exactly one accepted Bark action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkRetryRequest {
    /// Private REST payload version.
    pub version: u16,
    /// Exact accepted action to retry.
    pub action_id: FederationBarkActionId,
    /// Explicit acknowledgement that retry may repeat idempotent Ark effects.
    pub acknowledge_side_effects: bool,
}

impl FederationBarkRetryRequest {
    /// Construct a guarded request for one exact accepted action.
    #[must_use]
    pub const fn new(action_id: FederationBarkActionId, acknowledge_side_effects: bool) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            action_id,
            acknowledge_side_effects,
        }
    }

    /// Reject incompatible or unacknowledged side-effect requests.
    pub fn validate(self) -> Result<()> {
        validate_rest_version(self.version)?;
        if !self.acknowledge_side_effects {
            return Err(FederationError::BarkOperationInvalid {
                reason: "Bark action retry requires explicit side-effect acknowledgement"
                    .to_string(),
            });
        }
        Ok(())
    }
}

/// Redacted result of retrying exactly one accepted Bark action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkRetryResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Exact accepted action targeted by the request.
    pub action_id: FederationBarkActionId,
    /// Action phase before the retry.
    pub phase_before: FederationBarkSemanticPhase,
    /// Action phase after the retry.
    pub phase_after: FederationBarkSemanticPhase,
    /// Number of consensus-visible operations submitted by this bounded step.
    pub advanced: u64,
}

fn validate_rest_version(version: u16) -> Result<()> {
    if version != FEDERATION_REST_API_VERSION {
        return Err(FederationError::RestVersionMismatch {
            expected: FEDERATION_REST_API_VERSION,
            found: version,
        });
    }
    Ok(())
}

/// Count and value grouped by one accepted VTXO state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkVtxoStatusCount {
    /// Stable accepted VTXO state.
    pub state: FederationBarkVtxoState,
    /// Number of VTXOs in this state.
    pub count: u64,
    /// Saturating aggregate value in satoshis.
    pub value_sat: u64,
}

/// One active accepted Bark action without invoices, preimages, or artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkActionStatus {
    /// Stable accepted action identity.
    pub action_id: FederationBarkActionId,
    /// Side-effect category after a plan is accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<FederationBarkActionKind>,
    /// Current accepted semantic phase.
    pub phase: FederationBarkSemanticPhase,
    /// Phase being retried after signer rotation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_retry_phase: Option<FederationBarkSemanticPhase>,
    /// Accepted actor for the current action head.
    pub actor: MemberId,
    /// Whether this status-producing member is the accepted actor.
    pub local_actor: bool,
    /// Current accepted custody attempt, or zero before planning.
    pub attempt: u32,
    /// Accepted operations elapsed since this action began.
    pub age_operations: u64,
    /// Current value tied to this action in nonterminal VTXO states.
    pub value_sat: u64,
}

/// Nonterminal participant protocol records, without IDs or cryptographic material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkProtocolStatus {
    /// Ordinary FROST nonce/session records awaiting a terminal transition.
    pub ordinary_frost_nonterminal: u64,
    /// Virtual MuSig2 records awaiting a terminal transition.
    pub virtual_musig2_nonterminal: u64,
    /// Threshold-DH records awaiting a terminal transition.
    pub threshold_dh_nonterminal: u64,
    /// Independently verified aggregate results retained in bounded memory.
    pub aggregate_results_cached: u64,
}

/// Protected receive-secret coverage without secret identifiers or material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkReceiveSecretCoverage {
    /// Accepted receive actions represented in the projection.
    pub accepted_actions: u64,
    /// Actions whose accepted acknowledgement set names this member.
    pub acknowledged_by_local_member: u64,
    /// Accepted receive records still retained in the encrypted local store.
    pub locally_retained: u64,
    /// Locally acknowledged, nonterminal actions missing their protected record.
    pub missing_acknowledged_nonterminal: u64,
    /// Smallest accepted acknowledgement set across receive actions.
    pub minimum_acknowledgements: u64,
    /// Largest accepted acknowledgement set across receive actions.
    pub maximum_acknowledgements: u64,
}

/// Redacted member-local Bark diagnosis derived from accepted public state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkOperatorStatus {
    /// Status schema version.
    pub version: u16,
    /// Stable readiness name (`ready`, `behind_consensus`, or `degraded`).
    pub readiness: String,
    /// Bounded degraded reason, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    /// Highest accepted Bark operation represented by the SQL projection.
    pub accepted_frontier: u64,
    /// Highest accepted Bark operation replayed into local Bark state.
    pub local_frontier: u64,
    /// Accepted operations not yet replayed locally.
    pub projection_lag_operations: u64,
    /// Reusable root epoch serving Bark custody.
    pub root_key_epoch: u64,
    /// Bark application key epoch.
    pub application_key_epoch: u64,
    /// Bitcoin network fixed by accepted configuration.
    pub network: String,
    /// Public commitment to the accepted Ark server identity.
    pub ark_server_scope: FederationBarkDigest,
    /// Ark source classification. V1 uses one shared protocol server.
    pub ark_source_scope: String,
    /// Whether Ark observations are correlated across federation members.
    pub ark_source_correlated_warning: bool,
    /// Chain source declaration. V1 reports `undeclared` rather than endpoints.
    pub chain_source_scope: String,
    /// Whether chain-source independence cannot be established from configuration.
    pub chain_source_correlated_warning: bool,
    /// Latest member-local Bark chain height, when the source is readable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_tip_height: Option<u32>,
    /// Accepted refresh margin applied to the member-local chain tip.
    pub refresh_margin_blocks: u32,
    /// Fixed-state accepted VTXO counts and values.
    pub vtxos: Vec<FederationBarkVtxoStatusCount>,
    /// VTXOs at or inside the accepted refresh safety margin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiring_vtxo_count: Option<u64>,
    /// Value at or inside the accepted refresh safety margin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiring_value_sat: Option<u64>,
    /// Earliest nonterminal accepted VTXO expiry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub earliest_expiry_height: Option<u32>,
    /// Actions whose Ark outcome is ambiguous after the point of no return.
    pub ambiguous_action_count: u64,
    /// Value currently tied to ambiguous actions.
    pub ambiguous_value_sat: u64,
    /// Actions requiring revocation, refresh, or emergency-exit recovery.
    pub recovery_required_action_count: u64,
    /// Value currently tied to recovery-required actions.
    pub recovery_required_value_sat: u64,
    /// Actions currently on an accepted higher signer attempt.
    pub signer_retry_action_count: u64,
    /// Active action summaries sorted by action identity.
    pub actions: Vec<FederationBarkActionStatus>,
    /// Participant protocol ledger counts without nonces, shares, or session IDs.
    pub protocols: FederationBarkProtocolStatus,
    /// Protected-secret coverage without preimages or storage paths.
    pub receive_secrets: FederationBarkReceiveSecretCoverage,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn retry_requires_explicit_side_effect_acknowledgement() {
        let action_id = FederationBarkActionId::from_bytes([7; 32]).expect("action id");
        assert!(FederationBarkRetryRequest::new(action_id, false)
            .validate()
            .is_err());
        assert!(FederationBarkRetryRequest::new(action_id, true)
            .validate()
            .is_ok());
    }

    #[test]
    fn semantic_phase_metric_labels_are_fixed_and_unique() {
        let labels = FederationBarkSemanticPhase::ALL
            .into_iter()
            .map(FederationBarkSemanticPhase::as_str)
            .collect::<BTreeSet<_>>();

        assert_eq!(labels.len(), FederationBarkSemanticPhase::ALL.len());
    }

    #[test]
    fn operator_status_wire_is_bounded_and_redacted() {
        let action_id = FederationBarkActionId::from_bytes([8; 32]).expect("action id");
        let member = MemberId::new(2).expect("member");
        let status = FederationBarkOperatorStatus {
            version: FEDERATION_BARK_OPERATOR_STATUS_VERSION,
            readiness: "degraded".to_string(),
            degraded_reason: Some("recovery_required".to_string()),
            accepted_frontier: 11,
            local_frontier: 10,
            projection_lag_operations: 1,
            root_key_epoch: 2,
            application_key_epoch: 3,
            network: "regtest".to_string(),
            ark_server_scope: FederationBarkDigest::from_bytes([9; 32]).expect("scope"),
            ark_source_scope: "shared_protocol_server".to_string(),
            ark_source_correlated_warning: true,
            chain_source_scope: "undeclared".to_string(),
            chain_source_correlated_warning: true,
            chain_tip_height: Some(100),
            refresh_margin_blocks: 12,
            vtxos: vec![FederationBarkVtxoStatusCount {
                state: FederationBarkVtxoState::Reserved,
                count: 1,
                value_sat: 21_000,
            }],
            expiring_vtxo_count: Some(1),
            expiring_value_sat: Some(21_000),
            earliest_expiry_height: Some(105),
            ambiguous_action_count: 1,
            ambiguous_value_sat: 21_000,
            recovery_required_action_count: 1,
            recovery_required_value_sat: 21_000,
            signer_retry_action_count: 0,
            actions: vec![FederationBarkActionStatus {
                action_id,
                kind: Some(FederationBarkActionKind::LightningSend),
                phase: FederationBarkSemanticPhase::SendHtlcsCosigned,
                signing_retry_phase: None,
                actor: member,
                local_actor: true,
                attempt: 1,
                age_operations: 4,
                value_sat: 21_000,
            }],
            protocols: FederationBarkProtocolStatus {
                ordinary_frost_nonterminal: 1,
                virtual_musig2_nonterminal: 2,
                threshold_dh_nonterminal: 0,
                aggregate_results_cached: 3,
            },
            receive_secrets: FederationBarkReceiveSecretCoverage::default(),
        };
        let json = serde_json::to_string(&status).expect("operator status json");
        let decoded: FederationBarkOperatorStatus =
            serde_json::from_str(&json).expect("operator status round trip");
        assert_eq!(decoded, status);
        for forbidden in [
            "invoice",
            "preimage",
            "credential",
            "private_path",
            "nonce_material",
            "secret_share",
        ] {
            assert!(!json.contains(forbidden));
        }
    }
}
