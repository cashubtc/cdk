//! Deterministic, backend-neutral Bark projection records.

use serde::{Deserialize, Serialize};

use super::{
    FederationBarkActionId, FederationBarkActionKind, FederationBarkArtifact, FederationBarkDigest,
    FederationBarkOperation, FederationBarkSemanticPhase, FederationBarkVtxoId,
    FederationBarkVtxoLocator, FederationBarkVtxoState, FederationJournalEntry,
    FederationMaterializedStateRow, FederationOperationId, MemberId, Result,
};
use crate::Amount;

fn amount_is_zero(amount: &Amount) -> bool {
    *amount == Amount::ZERO
}

/// Current action head reconstructed from accepted Bark operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkActionRecord {
    /// Stable action lineage.
    pub action_id: FederationBarkActionId,
    /// Bound Cashu quote, when present.
    pub quote: Option<String>,
    /// Bound Lightning payment hash, when present.
    pub payment_hash: Option<FederationBarkDigest>,
    /// Side-effecting action category once planned.
    pub action_kind: Option<FederationBarkActionKind>,
    /// Current semantic phase.
    pub phase: FederationBarkSemanticPhase,
    /// Semantic phase whose custody work is being retried.
    ///
    /// This is set only while `phase` is `SigningAttemptAdvanced`, so an
    /// attempt change cannot erase the action's position in the state machine.
    pub signing_retry_phase: Option<FederationBarkSemanticPhase>,
    /// Current deterministic actor.
    pub actor: MemberId,
    /// Current custody attempt, or zero before a plan.
    pub attempt: u32,
    /// First accepted operation in this lineage.
    pub origin_operation_id: FederationOperationId,
    /// Consensus index of the origin operation.
    pub origin_operation_index: u64,
    /// Current action-head operation.
    pub latest_operation_id: FederationOperationId,
    /// Consensus index of the action head.
    pub latest_operation_index: u64,
    /// Current accepted plan, when one exists.
    pub plan_operation_id: Option<FederationOperationId>,
    /// Exact predecessor named by the current head, when applicable.
    pub predecessor_operation_id: Option<FederationOperationId>,
    /// Commitment to the current canonical Bark operation.
    pub operation_commitment: FederationBarkDigest,
    /// Complete current operation for independent replay.
    pub operation: FederationBarkOperation,
}

impl FederationBarkActionRecord {
    /// Build the first action head for a newly accepted lineage.
    pub fn new(
        entry: &FederationJournalEntry,
        operation: &FederationBarkOperation,
    ) -> Result<Self> {
        let (action_kind, attempt, plan_operation_id, predecessor_operation_id) =
            operation_head_fields(entry.operation_id, operation);
        Ok(Self {
            action_id: operation.action_id(),
            quote: operation.quote().map(ToOwned::to_owned),
            payment_hash: operation.payment_hash(),
            action_kind,
            phase: operation.phase(),
            signing_retry_phase: None,
            actor: operation.claimed_author().ok_or_else(|| {
                super::FederationError::BarkOperationInvalid {
                    reason: "Bark action head requires a claimed author".to_string(),
                }
            })?,
            attempt,
            origin_operation_id: entry.operation_id,
            origin_operation_index: entry.index,
            latest_operation_id: entry.operation_id,
            latest_operation_index: entry.index,
            plan_operation_id,
            predecessor_operation_id,
            operation_commitment: operation.commitment()?,
            operation: operation.clone(),
        })
    }

    /// Advance this action head after stateful predecessor checks have passed.
    pub fn advanced(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationBarkOperation,
    ) -> Result<Self> {
        let (action_kind, attempt, plan_operation_id, predecessor_operation_id) =
            operation_head_fields(entry.operation_id, operation);
        let phase = operation.phase();
        let signing_retry_phase = match phase {
            FederationBarkSemanticPhase::SigningAttemptAdvanced => match self.phase {
                FederationBarkSemanticPhase::SigningAttemptAdvanced => self.signing_retry_phase,
                _ => Some(self.phase),
            },
            _ => None,
        };
        Ok(Self {
            action_id: self.action_id,
            quote: operation
                .quote()
                .map(ToOwned::to_owned)
                .or_else(|| self.quote.clone()),
            payment_hash: operation.payment_hash().or(self.payment_hash),
            action_kind: action_kind.or(self.action_kind),
            phase,
            signing_retry_phase,
            actor: match operation.claimed_author() {
                Some(actor) => actor,
                None if matches!(
                    operation,
                    FederationBarkOperation::ActionTransition(transition)
                        if transition.evidence_certificate.is_some()
                ) || matches!(
                    operation,
                    FederationBarkOperation::ActionPlan(plan)
                        if plan.certified_receive_htlcs.is_some()
                ) =>
                {
                    self.actor
                }
                None => {
                    return Err(super::FederationError::BarkOperationInvalid {
                        reason: "Bark action transition has no authority".to_string(),
                    });
                }
            },
            attempt,
            origin_operation_id: self.origin_operation_id,
            origin_operation_index: self.origin_operation_index,
            latest_operation_id: entry.operation_id,
            latest_operation_index: entry.index,
            plan_operation_id: plan_operation_id.or(self.plan_operation_id),
            predecessor_operation_id,
            operation_commitment: operation.commitment()?,
            operation: operation.clone(),
        })
    }

    /// Stable materialized-state row.
    pub fn materialized_row(&self) -> Result<FederationMaterializedStateRow> {
        Ok(FederationMaterializedStateRow::new(
            "federation_bark_actions",
            self.action_id.to_string(),
        )
        .with_field("payload_json", serde_json::to_string(self)?))
    }
}

/// Consensus-owned derivation locator allocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkDerivationAllocationRecord {
    /// Unique locator.
    pub locator: FederationBarkVtxoLocator,
    /// Owning action.
    pub action_id: FederationBarkActionId,
    /// Allocated value.
    pub amount: Amount,
    /// Expected VTXO template or policy commitment.
    pub artifact_hash: Option<FederationBarkDigest>,
    /// VTXO eventually materialized at the locator.
    ///
    /// Retained for compatibility with the original one-VTXO projection.
    /// New code uses `vtxo_ids` because one Ark package may split one logical
    /// output allocation into several VTXOs controlled by the same locator.
    pub vtxo_id: Option<FederationBarkVtxoId>,
    /// Every VTXO materialized under this locator, sorted by identifier.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vtxo_ids: Vec<FederationBarkVtxoId>,
    /// Aggregate value materialized under this allocation.
    #[serde(default, skip_serializing_if = "amount_is_zero")]
    pub materialized_amount: Amount,
    /// Operation that allocated or last materialized this locator.
    pub operation_id: FederationOperationId,
    /// Consensus index of that operation.
    pub operation_index: u64,
}

impl FederationBarkDerivationAllocationRecord {
    /// Stable materialized-state row.
    pub fn materialized_row(&self) -> Result<FederationMaterializedStateRow> {
        Ok(FederationMaterializedStateRow::new(
            "federation_bark_derivation_allocations",
            self.locator.to_string(),
        )
        .with_field("payload_json", serde_json::to_string(self)?))
    }
}

/// Append-only ownership record for one planned VTXO reservation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkReservationRecord {
    /// Reserved VTXO.
    pub vtxo_id: FederationBarkVtxoId,
    /// Owning action.
    pub action_id: FederationBarkActionId,
    /// Accepted plan operation.
    pub operation_id: FederationOperationId,
    /// Consensus index of the plan.
    pub operation_index: u64,
    /// Exact reserved amount.
    pub amount: Amount,
}

impl FederationBarkReservationRecord {
    /// Stable materialized-state row.
    pub fn materialized_row(&self) -> Result<FederationMaterializedStateRow> {
        Ok(FederationMaterializedStateRow::new(
            "federation_bark_reservations",
            self.vtxo_id.to_string(),
        )
        .with_field("payload_json", serde_json::to_string(self)?))
    }
}

/// Current authoritative state of one canonical VTXO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkVtxoRecord {
    /// Canonical VTXO identifier.
    pub vtxo_id: FederationBarkVtxoId,
    /// Consensus-owned locator.
    pub locator: FederationBarkVtxoLocator,
    /// Exact value.
    pub amount: Amount,
    /// Absolute block height at which the VTXO expires.
    pub expiry_height: u32,
    /// Current lifecycle state.
    pub state: FederationBarkVtxoState,
    /// Immutable canonical VTXO bytes and hash.
    pub artifact: FederationBarkArtifact,
    /// Current action reservation owner, when applicable.
    pub reservation_action_id: Option<FederationBarkActionId>,
    /// Operation that first materialized the VTXO.
    pub origin_operation_id: FederationOperationId,
    /// Consensus index that first materialized the VTXO.
    pub origin_operation_index: u64,
    /// Operation that last changed this row.
    pub latest_operation_id: FederationOperationId,
    /// Consensus index of the latest change.
    pub latest_operation_index: u64,
}

impl FederationBarkVtxoRecord {
    /// Stable materialized-state row.
    pub fn materialized_row(&self) -> Result<FederationMaterializedStateRow> {
        Ok(
            FederationMaterializedStateRow::new("federation_bark_vtxos", self.vtxo_id.to_string())
                .with_field("payload_json", serde_json::to_string(self)?),
        )
    }
}

/// Append-only accepted Bark action transition or preparation record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkTransitionRecord {
    /// Accepted operation.
    pub operation_id: FederationOperationId,
    /// Consensus index.
    pub operation_index: u64,
    /// Action lineage.
    pub action_id: FederationBarkActionId,
    /// Accepted phase.
    pub phase: FederationBarkSemanticPhase,
    /// Exact predecessor, when the operation names one.
    pub predecessor_operation_id: Option<FederationOperationId>,
    /// Custody attempt, or zero before a plan.
    pub attempt: u32,
    /// Complete accepted operation.
    pub operation: FederationBarkOperation,
}

impl FederationBarkTransitionRecord {
    /// Build an append-only transition record.
    #[must_use]
    pub fn new(entry: &FederationJournalEntry, operation: &FederationBarkOperation) -> Self {
        let (_, attempt, _, predecessor_operation_id) =
            operation_head_fields(entry.operation_id, operation);
        Self {
            operation_id: entry.operation_id,
            operation_index: entry.index,
            action_id: operation.action_id(),
            phase: operation.phase(),
            predecessor_operation_id,
            attempt,
            operation: operation.clone(),
        }
    }

    /// Stable materialized-state row.
    pub fn materialized_row(&self) -> Result<FederationMaterializedStateRow> {
        Ok(FederationMaterializedStateRow::new(
            "federation_bark_transitions",
            self.operation_id.to_string(),
        )
        .with_field("payload_json", serde_json::to_string(self)?))
    }
}

/// Append-only accepted member observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkObservationRecord {
    /// Accepted operation.
    pub operation_id: FederationOperationId,
    /// Consensus index.
    pub operation_index: u64,
    /// Complete observation operation.
    pub operation: FederationBarkOperation,
}

impl FederationBarkObservationRecord {
    /// Stable materialized-state row.
    pub fn materialized_row(&self) -> Result<FederationMaterializedStateRow> {
        Ok(FederationMaterializedStateRow::new(
            "federation_bark_observations",
            self.operation_id.to_string(),
        )
        .with_field("payload_json", serde_json::to_string(self)?))
    }
}

/// Complete backend-neutral Bark projection, sorted by SQL primary key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkProjectionSnapshot {
    /// Current action heads.
    pub actions: Vec<FederationBarkActionRecord>,
    /// Current VTXO rows.
    pub vtxos: Vec<FederationBarkVtxoRecord>,
    /// Locator allocations.
    pub derivation_allocations: Vec<FederationBarkDerivationAllocationRecord>,
    /// Append-only reservations.
    pub reservations: Vec<FederationBarkReservationRecord>,
    /// Append-only action transitions.
    pub transitions: Vec<FederationBarkTransitionRecord>,
    /// Append-only member observations.
    pub observations: Vec<FederationBarkObservationRecord>,
}

impl FederationBarkProjectionSnapshot {
    /// Highest accepted operation index represented by any materialized Bark row.
    #[must_use]
    pub fn frontier(&self) -> Option<u64> {
        self.actions
            .iter()
            .map(|record| record.latest_operation_index)
            .chain(
                self.vtxos
                    .iter()
                    .map(|record| record.latest_operation_index),
            )
            .chain(
                self.derivation_allocations
                    .iter()
                    .map(|record| record.operation_index),
            )
            .chain(
                self.reservations
                    .iter()
                    .map(|record| record.operation_index),
            )
            .chain(self.transitions.iter().map(|record| record.operation_index))
            .chain(
                self.observations
                    .iter()
                    .map(|record| record.operation_index),
            )
            .max()
    }

    /// Canonical backend-parity bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Return every projection row in deterministic table/key order.
    pub fn materialized_rows(&self) -> Result<Vec<FederationMaterializedStateRow>> {
        let mut rows = Vec::with_capacity(
            self.actions.len()
                + self.vtxos.len()
                + self.derivation_allocations.len()
                + self.reservations.len()
                + self.transitions.len()
                + self.observations.len(),
        );
        for record in &self.actions {
            rows.push(record.materialized_row()?);
        }
        for record in &self.vtxos {
            rows.push(record.materialized_row()?);
        }
        for record in &self.derivation_allocations {
            rows.push(record.materialized_row()?);
        }
        for record in &self.reservations {
            rows.push(record.materialized_row()?);
        }
        for record in &self.transitions {
            rows.push(record.materialized_row()?);
        }
        for record in &self.observations {
            rows.push(record.materialized_row()?);
        }
        Ok(rows)
    }
}

fn operation_head_fields(
    operation_id: FederationOperationId,
    operation: &FederationBarkOperation,
) -> (
    Option<FederationBarkActionKind>,
    u32,
    Option<FederationOperationId>,
    Option<FederationOperationId>,
) {
    match operation {
        FederationBarkOperation::ReceiveIntent(_) => (None, 0, None, None),
        FederationBarkOperation::ReceivePrepared(prepared) => {
            (None, 0, None, Some(prepared.intent_operation_id))
        }
        FederationBarkOperation::ActionPlan(plan) => (
            Some(plan.kind),
            plan.attempt,
            Some(operation_id),
            Some(plan.predecessor_operation_id),
        ),
        FederationBarkOperation::ActionTransition(transition) => (
            None,
            transition.attempt,
            Some(transition.plan_operation_id),
            Some(transition.predecessor_operation_id),
        ),
        FederationBarkOperation::MemberObservation(_)
        | FederationBarkOperation::EvidenceCertificate(_) => (None, 0, None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::{FederationBarkObservationRecord, FederationBarkProjectionSnapshot};
    use crate::federation::{
        FederationBarkActionId, FederationBarkDigest, FederationBarkMemberObservation,
        FederationBarkObservationStatus, FederationBarkOperation, FederationOperationId, MemberId,
        FEDERATION_BARK_OPERATION_VERSION,
    };
    use crate::Amount;

    #[test]
    fn projection_frontier_covers_observation_only_progress() {
        let mut snapshot = FederationBarkProjectionSnapshot::default();
        assert_eq!(snapshot.frontier(), None);
        snapshot.observations.push(FederationBarkObservationRecord {
            operation_id: FederationOperationId::from_bytes([9; 32]),
            operation_index: 17,
            operation: FederationBarkOperation::MemberObservation(
                FederationBarkMemberObservation {
                    version: FEDERATION_BARK_OPERATION_VERSION,
                    action_id: FederationBarkActionId::from_bytes([8; 32]).expect("action ID"),
                    quote: None,
                    observer: MemberId::new(1).expect("member ID"),
                    observation_epoch: 1,
                    source_scope: FederationBarkDigest::from_bytes([7; 32]).expect("source scope"),
                    payment_hash: None,
                    status: FederationBarkObservationStatus::Pending,
                    amount: Amount::ZERO,
                    fee: Amount::ZERO,
                    result_hash: FederationBarkDigest::from_bytes([6; 32]).expect("result hash"),
                },
            ),
        });
        assert_eq!(snapshot.frontier(), Some(17));
    }
}
