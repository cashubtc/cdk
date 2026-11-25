//! Consensus history retention.
//!
//! After a checkpoint promotion a member deletes journal entries, conflict
//! keys, finalized items and scheduled checkpoints below its retention floor
//! in bounded batches, each in its own transaction. A member whose catch-up
//! cursor lies below every peer's floor installs a state transfer instead of
//! replaying, then resumes ordinary catch-up.

use cdk::federation::{
    FederationCheckpointError, FederationCheckpointFrontier, FederationCheckpointKind,
    FederationConsensusRecoveryBase, FederationConsensusRuntime, FederationError,
    FederationJournalCatchUpRequest, FederationSuffixVerifier, MemberConfigPublic,
    FEDERATION_RETENTION_PRUNE_BATCH_ROWS,
};
use cdk_http_client::Transport;

use super::node_runtime::{runtime_unavailable, FederationNodeRuntime};
use super::peer_client::FederationRestPeerClient;
use crate::FederationNode;

/// Batches deleted after one promotion; later promotions continue.
const FEDERATION_RETENTION_MAX_BATCHES_PER_PROMOTION: u64 = 64;

impl FederationNodeRuntime {
    /// Delete consensus history below the retention floor.
    ///
    /// Each batch runs in its own transaction, so application and catch-up
    /// interleave with deletion. Live state pins the floor: unfinished
    /// outgoing transactions, melt sagas and Bark actions keep the journal
    /// entries they still read.
    pub async fn prune_history_below_retention_floor(&self) -> Result<u64, cdk::Error> {
        let mint = self.active_mint()?;
        let keep_journal_from = mint.federation_transaction_retention_pin().await?;
        let db = mint.localstore();
        let mut deleted = 0_u64;
        for _ in 0..FEDERATION_RETENTION_MAX_BATCHES_PER_PROMOTION {
            let mut tx = db.begin_transaction().await?;
            let batch = tx
                .prune_federation_history(FEDERATION_RETENTION_PRUNE_BATCH_ROWS, keep_journal_from)
                .await?;
            tx.commit().await?;
            deleted = deleted.saturating_add(batch);
            if batch == 0 {
                break;
            }
        }
        if deleted > 0 {
            tracing::debug!(
                deleted,
                "Federation history below the retention floor deleted"
            );
        }
        Ok(deleted)
    }

    /// Prune after a promotion; a failure is logged and retried at the next one.
    pub(crate) async fn prune_history_after_promotion(&self) {
        if let Err(err) = self.prune_history_below_retention_floor().await {
            tracing::warn!("Federation history pruning failed: {}", err);
        }
    }

    /// Adopt a state transfer that was just installed at `quorum`'s
    /// checkpoint: the consensus runtime restarts from that checkpoint.
    pub async fn adopt_installed_state_transfer(
        &self,
        quorum: &cdk::federation::FederationCheckpointQuorum,
    ) -> Result<(), cdk::Error> {
        let _application = self.components.operation.lock().await;
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .cloned()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let public_config = runtime_config.public_config();
        let verified_quorum = super::verify_checkpoint_quorum_with_metrics(
            quorum,
            public_config,
            runtime_config.member_id(),
            super::CheckpointQuorumVerificationContext::StartupCatchUp,
        )?;
        let frontier = mint
            .localstore()
            .get_federation_checkpoint_frontier()
            .await?;
        if !frontier.exactly_matches_checkpoint(verified_quorum.checkpoint()) {
            return Err(FederationCheckpointError::StateTransferInvalid {
                reason: "installed frontier does not equal the transferred checkpoint",
            }
            .into());
        }
        let checkpoint_runtime = FederationConsensusRuntime::recover_verified_suffix(
            FederationSuffixVerifier::new_for_validated_config(
                public_config,
                runtime_config.member_id(),
                verified_quorum.checkpoint().aleph_bft_session,
                FederationConsensusRecoveryBase::Checkpoint(&verified_quorum),
            )?
            .finish_for_checkpoint(&verified_quorum)?,
        );
        let consensus = self
            .consensus()
            .ok_or_else(|| runtime_unavailable("consensus"))?;
        *consensus.lock().await = checkpoint_runtime;
        Ok(())
    }
}

/// Whether `error` says a peer deleted the history this member asked for.
pub fn is_peer_history_pruned(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<FederationError>(),
            Some(FederationError::Checkpoint(
                FederationCheckpointError::PeerHistoryPruned
            ))
        ) || matches!(
            cause.downcast_ref::<cdk::Error>(),
            Some(cdk::Error::Federation(FederationError::Checkpoint(
                FederationCheckpointError::PeerHistoryPruned
            )))
        )
    })
}

/// Ask `member` whether it still holds the history up to the latest scheduled
/// checkpoint quorum this member knows.
///
/// A member that knows a later scheduled quorum but not the next one has no
/// catch-up target: peers delete the scheduled checkpoints below their floor
/// together with the history. One bounded catch-up request toward the known
/// quorum asks; a GONE answer comes back as `PeerHistoryPruned`, so the sync
/// round can install a state transfer. Any other outcome is left to the
/// ordinary recovery-checkpoint path.
pub async fn probe_peer_history_to_known_checkpoint<T>(
    state: &FederationNode,
    client: &FederationRestPeerClient<T>,
    member: &MemberConfigPublic,
) -> anyhow::Result<()>
where
    T: Transport,
{
    let Some(known) = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Scheduled)
        .await?
    else {
        return Ok(());
    };
    let consensus = state
        .federation
        .consensus()
        .ok_or_else(|| runtime_unavailable("consensus"))?;
    let (next_journal_index, next_item_index) = {
        let consensus = consensus.lock().await;
        (
            consensus.journal().next_index(),
            consensus.aleph_bft_status()?.next_item_index,
        )
    };
    let checkpoint = &known.checkpoint;
    if next_journal_index >= checkpoint.journal_next_index
        && next_item_index >= checkpoint.finalized_next_item_index
    {
        return Ok(());
    }
    let request = FederationJournalCatchUpRequest::new(next_journal_index)
        .with_target_checkpoint(checkpoint.id()?)
        .with_limit(1)
        .with_aleph_bft_next_item_index(next_item_index);
    match client
        .catch_up_to_checkpoint(member, &request, checkpoint)
        .await
    {
        Err(err) if is_peer_history_pruned(&err) => Err(err),
        _ => Ok(()),
    }
}

/// Install a peer's latest scheduled checkpoint state because the history
/// this member needs lies below every peer's retention floor.
///
/// The installer requires empty federation state. A member that already
/// holds state below every floor cannot be repaired automatically: it is
/// reported, and an operator must reset it so it can install the transfer.
pub async fn install_state_transfer_below_peer_floors<T>(
    state: &FederationNode,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
) -> bool
where
    T: Transport,
{
    let db = state.mint.localstore();
    match db.get_federation_checkpoint_frontier().await {
        Ok(frontier) if frontier == FederationCheckpointFrontier::genesis() => {}
        Ok(_) => {
            tracing::error!(
                "Federation history this member needs is below every peer's retention floor; \
                 a state transfer needs empty federation state, so this member must be reset"
            );
            return false;
        }
        Err(err) => {
            tracing::warn!(
                "Could not inspect the local frontier for state transfer: {}",
                err
            );
            return false;
        }
    }
    for peer in peers {
        let summary = match client.checkpoint_summary(peer, false, None).await {
            Ok(summary) => summary,
            Err(err) => {
                tracing::warn!(
                    member_id = %peer.member_id,
                    "Federation state transfer summary request failed: {}",
                    err
                );
                continue;
            }
        };
        let Some(advertised) = summary.latest_scheduled_quorum.as_ref() else {
            continue;
        };
        let quorum = match client
            .fetch_checkpoint(peer, advertised.checkpoint_id)
            .await
        {
            Ok(response) => match response.quorum {
                Some(quorum) if quorum.checkpoint.kind == FederationCheckpointKind::Scheduled => {
                    quorum
                }
                _ => continue,
            },
            Err(err) => {
                tracing::warn!(
                    member_id = %peer.member_id,
                    "Federation state transfer checkpoint fetch failed: {}",
                    err
                );
                continue;
            }
        };
        if let Err(err) = client
            .transfer_state_from_member(&state.mint, peer, &quorum)
            .await
        {
            tracing::warn!(
                member_id = %peer.member_id,
                "Federation state transfer failed: {}",
                err
            );
            continue;
        }
        match state
            .federation
            .adopt_installed_state_transfer(&quorum)
            .await
        {
            Ok(()) => {
                tracing::info!(
                    member_id = %peer.member_id,
                    journal_next_index = quorum.checkpoint.journal_next_index,
                    "Federation state transfer installed; resuming catch-up"
                );
                return true;
            }
            Err(err) => {
                tracing::error!(
                    "Could not adopt the installed federation state transfer: {}",
                    err
                );
                return false;
            }
        }
    }
    false
}
