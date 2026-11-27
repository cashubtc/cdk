//! In-memory indexed view and frontier-validated cache over the accepted Bark
//! projection.
//!
//! Federation drivers consult the accepted Bark projection on every quote
//! creation, payment advancement, custody attempt, and maintenance step.
//! Reloading the full SQL snapshot and linearly scanning `Vec` tables on every
//! step scales poorly with active quotes and VTXOs. [`IndexedBarkProjection`]
//! builds O(1) lookup indices once per accepted Bark frontier and
//! [`BarkProjectionCache`] shares that immutable snapshot across concurrent
//! readers via `Arc<IndexedBarkProjection>`, invalidating automatically
//! whenever the database's Bark projection frontier changes.

use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::federation::FEDERATION_MAX_DERIVATION_INDEX;
use cdk_federation_bark::{
    FederationBarkActionId, FederationBarkActionRecord, FederationBarkDigest,
    FederationBarkEvidencePhase, FederationBarkObservationRecord, FederationBarkObservationStatus,
    FederationBarkOperation, FederationBarkProjectionSnapshot, FederationBarkSemanticPhase,
    FederationBarkTransitionRecord, FederationBarkVtxoId, FederationBarkVtxoRecord,
    FederationBarkVtxoState,
};
use tokio::sync::RwLock;

/// Indexed, immutable view over one authoritative [`FederationBarkProjectionSnapshot`].
#[derive(Debug, Clone)]
pub struct IndexedBarkProjection {
    snapshot: Arc<FederationBarkProjectionSnapshot>,
    frontier: Option<u64>,
    actions_by_id: HashMap<FederationBarkActionId, usize>,
    actions_by_quote: HashMap<String, Vec<usize>>,
    actions_by_payment_hash: HashMap<FederationBarkDigest, Vec<usize>>,
    vtxos_by_id: HashMap<FederationBarkVtxoId, usize>,
    spendable_vtxos: Vec<usize>,
    transitions_by_action: HashMap<FederationBarkActionId, Vec<usize>>,
    observations_by_action: HashMap<FederationBarkActionId, Vec<usize>>,
    max_locator_index: Option<u32>,
}

impl IndexedBarkProjection {
    /// Build all O(1) indices for one authoritative Bark projection snapshot.
    #[must_use]
    pub fn from_snapshot(snapshot: FederationBarkProjectionSnapshot) -> Self {
        let frontier = snapshot.frontier();

        let mut actions_by_id = HashMap::with_capacity(snapshot.actions.len());
        let mut actions_by_quote: HashMap<String, Vec<usize>> =
            HashMap::with_capacity(snapshot.actions.len());
        let mut actions_by_payment_hash: HashMap<FederationBarkDigest, Vec<usize>> =
            HashMap::with_capacity(snapshot.actions.len());
        for (index, action) in snapshot.actions.iter().enumerate() {
            actions_by_id.insert(action.action_id, index);
            if let Some(quote) = &action.quote {
                actions_by_quote
                    .entry(quote.clone())
                    .or_default()
                    .push(index);
            }
            if let Some(payment_hash) = action.payment_hash {
                actions_by_payment_hash
                    .entry(payment_hash)
                    .or_default()
                    .push(index);
            }
        }

        let mut vtxos_by_id = HashMap::with_capacity(snapshot.vtxos.len());
        let mut spendable_vtxos = Vec::new();
        for (index, vtxo) in snapshot.vtxos.iter().enumerate() {
            vtxos_by_id.insert(vtxo.vtxo_id, index);
            if vtxo.state == FederationBarkVtxoState::Spendable {
                spendable_vtxos.push(index);
            }
        }

        let mut transitions_by_action: HashMap<FederationBarkActionId, Vec<usize>> = HashMap::new();
        for (index, transition) in snapshot.transitions.iter().enumerate() {
            transitions_by_action
                .entry(transition.action_id)
                .or_default()
                .push(index);
        }

        let mut observations_by_action: HashMap<FederationBarkActionId, Vec<usize>> =
            HashMap::new();
        for (index, observation) in snapshot.observations.iter().enumerate() {
            observations_by_action
                .entry(observation.operation.action_id())
                .or_default()
                .push(index);
        }

        let max_locator_index = snapshot
            .derivation_allocations
            .iter()
            .map(|allocation| allocation.locator.index)
            .max();

        Self {
            snapshot: Arc::new(snapshot),
            frontier,
            actions_by_id,
            actions_by_quote,
            actions_by_payment_hash,
            vtxos_by_id,
            spendable_vtxos,
            transitions_by_action,
            observations_by_action,
            max_locator_index,
        }
    }

    /// Underlying authoritative snapshot.
    #[must_use]
    pub fn snapshot(&self) -> &FederationBarkProjectionSnapshot {
        &self.snapshot
    }

    /// Cheap shared reference to the underlying snapshot.
    #[must_use]
    pub fn snapshot_arc(&self) -> Arc<FederationBarkProjectionSnapshot> {
        Arc::clone(&self.snapshot)
    }

    /// Highest accepted operation index represented by this projection.
    #[must_use]
    pub const fn frontier(&self) -> Option<u64> {
        self.frontier
    }

    /// Look up one action by its exact action ID in O(1).
    #[must_use]
    pub fn action(&self, action_id: FederationBarkActionId) -> Option<&FederationBarkActionRecord> {
        self.actions_by_id
            .get(&action_id)
            .and_then(|index| self.snapshot.actions.get(*index))
    }

    /// Iterate over every action bound to `quote` in O(k).
    pub fn actions_for_quote(
        &self,
        quote: &str,
    ) -> impl Iterator<Item = &FederationBarkActionRecord> {
        self.actions_by_quote
            .get(quote)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(|index| self.snapshot.actions.get(*index))
    }

    /// Return the first action bound to `quote` in O(1).
    #[must_use]
    pub fn action_for_quote(&self, quote: &str) -> Option<&FederationBarkActionRecord> {
        self.actions_for_quote(quote).next()
    }

    /// Iterate over every action bound to `payment_hash` in O(k).
    pub fn actions_for_payment_hash(
        &self,
        payment_hash: FederationBarkDigest,
    ) -> impl Iterator<Item = &FederationBarkActionRecord> {
        self.actions_by_payment_hash
            .get(&payment_hash)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(|index| self.snapshot.actions.get(*index))
    }

    /// Look up one VTXO by its exact VTXO ID in O(1).
    #[must_use]
    pub fn vtxo(&self, vtxo_id: FederationBarkVtxoId) -> Option<&FederationBarkVtxoRecord> {
        self.vtxos_by_id
            .get(&vtxo_id)
            .and_then(|index| self.snapshot.vtxos.get(*index))
    }

    /// Iterate over every spendable VTXO in canonical `vtxo_id` order.
    pub fn spendable_vtxos(&self) -> impl Iterator<Item = &FederationBarkVtxoRecord> {
        self.spendable_vtxos
            .iter()
            .filter_map(|index| self.snapshot.vtxos.get(*index))
    }

    /// Iterate over every recorded transition for `action_id` in O(k).
    pub fn transitions_for_action(
        &self,
        action_id: FederationBarkActionId,
    ) -> impl Iterator<Item = &FederationBarkTransitionRecord> {
        self.transitions_by_action
            .get(&action_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(|index| self.snapshot.transitions.get(*index))
    }

    /// Return the first transition for `action_id` in `phase`.
    #[must_use]
    pub fn first_transition_for_phase(
        &self,
        action_id: FederationBarkActionId,
        phase: FederationBarkSemanticPhase,
    ) -> Option<&FederationBarkTransitionRecord> {
        self.transitions_for_action(action_id)
            .find(|record| record.phase == phase)
    }

    /// Return the highest-index transition for `action_id` in `phase`.
    #[must_use]
    pub fn latest_transition_for_phase(
        &self,
        action_id: FederationBarkActionId,
        phase: FederationBarkSemanticPhase,
    ) -> Option<&FederationBarkTransitionRecord> {
        self.transitions_for_action(action_id)
            .filter(|record| record.phase == phase)
            .max_by_key(|record| record.operation_index)
    }

    /// Iterate over every recorded observation/certificate for `action_id` in O(k).
    pub fn observations_for_action(
        &self,
        action_id: FederationBarkActionId,
    ) -> impl Iterator<Item = &FederationBarkObservationRecord> {
        self.observations_by_action
            .get(&action_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(|index| self.snapshot.observations.get(*index))
    }

    /// Return whether an `Accepted` member observation exists for `action_id`.
    #[must_use]
    pub fn has_accepted_receive_observation(&self, action_id: FederationBarkActionId) -> bool {
        self.observations_for_action(action_id).any(|record| {
            matches!(
                &record.operation,
                FederationBarkOperation::MemberObservation(observation)
                    if observation.action_id == action_id
                        && observation.status == FederationBarkObservationStatus::Accepted
            )
        })
    }

    /// Return whether an accepted evidence certificate exists for `(action_id, phase)`.
    #[must_use]
    pub fn has_evidence_certificate(
        &self,
        action_id: FederationBarkActionId,
        phase: FederationBarkEvidencePhase,
    ) -> bool {
        self.observations_for_action(action_id).any(|record| {
            matches!(
                &record.operation,
                FederationBarkOperation::EvidenceCertificate(certificate)
                    if certificate.binding.action_id == action_id
                        && certificate.binding.phase == phase
            )
        })
    }

    /// Highest allocated derivation locator index in the projection.
    #[must_use]
    pub const fn max_locator_index(&self) -> Option<u32> {
        self.max_locator_index
    }

    /// Next unallocated derivation locator index, or `None` if exhausted.
    #[must_use]
    pub fn next_locator_index(&self) -> Option<u32> {
        let next = match self.max_locator_index {
            Some(index) => index.checked_add(1)?,
            None => 0,
        };
        (next <= FEDERATION_MAX_DERIVATION_INDEX).then_some(next)
    }
}

/// Frontier-validated in-memory cache of [`IndexedBarkProjection`].
///
/// Readers supply the database's current Bark projection frontier (obtained via
/// a cheap indexed `MAX` query). A cached entry is returned only when its
/// frontier matches exactly, preventing stale reads across consensus advances.
#[derive(Debug, Default)]
pub(crate) struct BarkProjectionCache {
    cached: RwLock<Option<Arc<IndexedBarkProjection>>>,
}

impl BarkProjectionCache {
    /// Return the cached indexed projection if its frontier matches `frontier`.
    pub(crate) async fn get_if_frontier(
        &self,
        frontier: Option<u64>,
    ) -> Option<Arc<IndexedBarkProjection>> {
        let guard = self.cached.read().await;
        guard
            .as_ref()
            .filter(|projection| projection.frontier() == frontier)
            .cloned()
    }

    /// Store a freshly loaded projection unless a strictly newer frontier is
    /// already cached.
    pub(crate) async fn store(
        &self,
        projection: Arc<IndexedBarkProjection>,
    ) -> Arc<IndexedBarkProjection> {
        let mut guard = self.cached.write().await;
        let should_replace = match guard.as_ref() {
            None => true,
            Some(existing) => match (existing.frontier(), projection.frontier()) {
                (Some(existing_frontier), Some(new_frontier)) => new_frontier >= existing_frontier,
                (None, Some(_)) | (None, None) => true,
                (Some(_), None) => false,
            },
        };
        if should_replace {
            *guard = Some(Arc::clone(&projection));
            projection
        } else {
            Arc::clone(
                guard
                    .as_ref()
                    .expect("non-replaced cache entry must be present"),
            )
        }
    }

    /// Drop any cached projection so the next read reloads from storage.
    pub(crate) async fn invalidate(&self) {
        *self.cached.write().await = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::federation::{FederationOperationId, MemberId};
    use cdk_common::Amount;
    use cdk_federation_bark::{
        FederationBarkActionId, FederationBarkActionRecord, FederationBarkArtifact,
        FederationBarkArtifactKind, FederationBarkDerivationAllocationRecord, FederationBarkDigest,
        FederationBarkMemberObservation, FederationBarkObservationRecord,
        FederationBarkObservationStatus, FederationBarkOperation, FederationBarkProjectionSnapshot,
        FederationBarkReceiveIntent, FederationBarkReceiveProof, FederationBarkSemanticPhase,
        FederationBarkTransitionRecord, FederationBarkUnit, FederationBarkVtxoId,
        FederationBarkVtxoLocator, FederationBarkVtxoRecord, FederationBarkVtxoState,
        FEDERATION_BARK_OPERATION_VERSION,
    };

    use super::{BarkProjectionCache, IndexedBarkProjection};

    fn digest(seed: u8) -> FederationBarkDigest {
        FederationBarkDigest::from_bytes([seed; 32]).expect("digest")
    }

    fn action_id(seed: u8) -> FederationBarkActionId {
        FederationBarkActionId::from_bytes([seed; 32]).expect("action id")
    }

    fn vtxo_id(seed: u8) -> FederationBarkVtxoId {
        FederationBarkVtxoId::from_bytes([seed; 36]).expect("vtxo id")
    }

    fn sample_receive_action(
        seed: u8,
        quote: &str,
        operation_index: u64,
    ) -> FederationBarkActionRecord {
        let id = action_id(seed);
        let payment_hash = digest(seed.saturating_add(10));
        let intent = FederationBarkReceiveIntent {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: id,
            quote: quote.to_string(),
            actor: MemberId::new(1).expect("member"),
            cashu_amount: Amount::from(1_000),
            gross_amount: Amount::from(1_000),
            receive_fee: Amount::ZERO,
            minimum_net_amount: Amount::from(1_000),
            unit: FederationBarkUnit::Sat,
            description_hash: digest(seed.saturating_add(20)),
            description: None,
            payment_hash,
            preimage_commitment: digest(seed.saturating_add(30)),
            secret_acknowledgements: Vec::new(),
            destination: FederationBarkVtxoLocator::vtxo_custody(u32::from(seed)),
            mailbox_scope: digest(seed.saturating_add(40)),
            mailbox_id: [2; 33],
            ark_server: digest(seed.saturating_add(50)),
            network: bitcoin::Network::Regtest,
            minimum_cltv_delta: 40,
            fee_policy_version: 1,
            anti_dos_policy_version: 1,
            receive_proof: FederationBarkReceiveProof::NoProof,
            expires_at: 10_000,
        };
        FederationBarkActionRecord {
            action_id: id,
            quote: Some(quote.to_string()),
            payment_hash: Some(payment_hash),
            action_kind: None,
            phase: FederationBarkSemanticPhase::ReceiveIntent,
            signing_retry_phase: None,
            actor: MemberId::new(1).expect("member"),
            attempt: 1,
            origin_operation_id: FederationOperationId::from_bytes([seed; 32]),
            origin_operation_index: operation_index,
            latest_operation_id: FederationOperationId::from_bytes([seed; 32]),
            latest_operation_index: operation_index,
            plan_operation_id: None,
            predecessor_operation_id: None,
            operation_commitment: digest(seed.saturating_add(60)),
            operation: FederationBarkOperation::ReceiveIntent(intent),
            quote_settled_operation_index: None,
        }
    }

    fn sample_vtxo(
        seed: u8,
        state: FederationBarkVtxoState,
        operation_index: u64,
    ) -> FederationBarkVtxoRecord {
        FederationBarkVtxoRecord {
            vtxo_id: vtxo_id(seed),
            locator: FederationBarkVtxoLocator::vtxo_custody(u32::from(seed)),
            amount: Amount::from(500),
            expiry_height: 100_000,
            state,
            artifact: FederationBarkArtifact {
                kind: FederationBarkArtifactKind::Vtxo,
                hash: digest(seed),
                bytes: vec![seed; 4],
            },
            reservation_action_id: None,
            origin_operation_id: FederationOperationId::from_bytes([seed; 32]),
            origin_operation_index: operation_index,
            latest_operation_id: FederationOperationId::from_bytes([seed; 32]),
            latest_operation_index: operation_index,
        }
    }

    #[test]
    fn indexed_projection_matches_linear_queries_across_all_indices() {
        let action_a = sample_receive_action(1, "quote-a", 1);
        let action_b = sample_receive_action(2, "quote-b", 2);
        let spendable = sample_vtxo(11, FederationBarkVtxoState::Spendable, 3);
        let reserved = sample_vtxo(12, FederationBarkVtxoState::Reserved, 4);
        let transition = FederationBarkTransitionRecord {
            operation_id: FederationOperationId::from_bytes([21; 32]),
            operation_index: 5,
            action_id: action_a.action_id,
            phase: FederationBarkSemanticPhase::ReceivePrepared,
            predecessor_operation_id: Some(action_a.origin_operation_id),
            attempt: 1,
            operation: action_a.operation.clone(),
        };
        let observation = FederationBarkObservationRecord {
            operation_id: FederationOperationId::from_bytes([22; 32]),
            operation_index: 6,
            operation: FederationBarkOperation::MemberObservation(
                FederationBarkMemberObservation {
                    version: FEDERATION_BARK_OPERATION_VERSION,
                    action_id: action_a.action_id,
                    quote: Some("quote-a".to_string()),
                    observer: MemberId::new(1).expect("member"),
                    observation_epoch: 1,
                    source_scope: digest(51),
                    payment_hash: action_a.payment_hash,
                    status: FederationBarkObservationStatus::Accepted,
                    amount: Amount::from(1_000),
                    fee: Amount::ZERO,
                    result_hash: digest(99),
                },
            ),
        };
        let allocation = FederationBarkDerivationAllocationRecord {
            locator: FederationBarkVtxoLocator::vtxo_custody(7),
            action_id: action_b.action_id,
            amount: Amount::from(1_000),
            artifact_hash: None,
            vtxo_ids: Vec::new(),
            materialized_amount: Amount::ZERO,
            operation_id: action_b.origin_operation_id,
            operation_index: 2,
        };

        let snapshot = FederationBarkProjectionSnapshot {
            actions: vec![action_a.clone(), action_b.clone()],
            vtxos: vec![spendable.clone(), reserved.clone()],
            derivation_allocations: vec![allocation],
            reservations: Vec::new(),
            transitions: vec![transition],
            observations: vec![observation],
        };
        let indexed = IndexedBarkProjection::from_snapshot(snapshot);

        assert_eq!(indexed.frontier(), Some(6));
        assert_eq!(
            indexed.action(action_a.action_id).map(|a| a.action_id),
            Some(action_a.action_id)
        );
        assert_eq!(
            indexed.action_for_quote("quote-b").map(|a| a.action_id),
            Some(action_b.action_id)
        );
        assert_eq!(
            indexed
                .actions_for_payment_hash(action_a.payment_hash.expect("payment hash"))
                .map(|a| a.action_id)
                .collect::<Vec<_>>(),
            vec![action_a.action_id]
        );
        assert_eq!(
            indexed.vtxo(spendable.vtxo_id).map(|v| v.vtxo_id),
            Some(spendable.vtxo_id)
        );
        assert_eq!(
            indexed
                .spendable_vtxos()
                .map(|v| v.vtxo_id)
                .collect::<Vec<_>>(),
            vec![spendable.vtxo_id]
        );
        assert!(indexed
            .first_transition_for_phase(
                action_a.action_id,
                FederationBarkSemanticPhase::ReceivePrepared
            )
            .is_some());
        assert!(indexed.has_accepted_receive_observation(action_a.action_id));
        assert!(!indexed.has_accepted_receive_observation(action_b.action_id));
        assert_eq!(indexed.max_locator_index(), Some(7));
        assert_eq!(indexed.next_locator_index(), Some(8));
    }

    #[tokio::test]
    async fn projection_cache_hits_on_matching_frontier_and_never_regresses() {
        let cache = BarkProjectionCache::default();
        assert!(cache.get_if_frontier(None).await.is_none());

        let empty = Arc::new(IndexedBarkProjection::from_snapshot(
            FederationBarkProjectionSnapshot::default(),
        ));
        cache.store(Arc::clone(&empty)).await;
        assert!(cache.get_if_frontier(None).await.is_some());
        assert!(cache.get_if_frontier(Some(1)).await.is_none());

        let newer = Arc::new(IndexedBarkProjection::from_snapshot(
            FederationBarkProjectionSnapshot {
                actions: vec![sample_receive_action(1, "quote-1", 5)],
                ..FederationBarkProjectionSnapshot::default()
            },
        ));
        cache.store(Arc::clone(&newer)).await;
        assert!(cache.get_if_frontier(Some(5)).await.is_some());
        assert!(cache.get_if_frontier(Some(6)).await.is_none());

        // Storing an older frontier must not overwrite the newer cached entry.
        let older = Arc::new(IndexedBarkProjection::from_snapshot(
            FederationBarkProjectionSnapshot {
                actions: vec![sample_receive_action(2, "quote-2", 3)],
                ..FederationBarkProjectionSnapshot::default()
            },
        ));
        let retained = cache.store(older).await;
        assert_eq!(retained.frontier(), Some(5));
        assert!(cache.get_if_frontier(Some(5)).await.is_some());

        // Storing an empty snapshot must also not regress a non-empty cached frontier.
        let retained_empty = cache.store(Arc::clone(&empty)).await;
        assert_eq!(retained_empty.frontier(), Some(5));
        assert!(cache.get_if_frontier(Some(5)).await.is_some());

        cache.invalidate().await;
        assert!(cache.get_if_frontier(Some(5)).await.is_none());
    }
}
