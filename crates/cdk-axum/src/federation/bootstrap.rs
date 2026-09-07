//! Federation bootstrap integration.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::Result;

use cdk::federation::{
    FederationAlephBftFinalizedItem, FederationAlephBftFinalizedItemStatus, FederationCheckpoint,
    FederationCheckpointFrontier, FederationConfig, FederationConfigSnapshot,
    FederationConsensusItem, FederationConsensusRecoveryBase, FederationConsensusRuntime,
    FederationError, FederationFrostDkgContext, FederationJournalEntry, FederationOperationStatus,
    FederationRestSequenceTracker, FederationSuffixVerifier, MemberId,
    VerifiedFederationCheckpointQuorum,
};
use cdk::mint::Mint;
#[cfg(feature = "federation-bdk")]
use cdk_bdk::{
    FederatedBdkLocalConfig, FederatedBdkWallet, FederatedWalletConfig, FederatedWalletScope,
};

use super::mint_state::*;
use crate::federation;
#[cfg(test)]
use crate::{default_federation_transport, FederationConsensusHandle, FederationSequenceHandle};
use crate::{FederationPeerSyncConfig, FederationTransport};

pub(crate) fn checkpoint_is_exact_startup_frontier(
    checkpoint: &FederationCheckpoint,
    frontier: &FederationCheckpointFrontier,
) -> std::result::Result<bool, cdk::Error> {
    if frontier.materialized_state_audit_mismatch {
        return Err(FederationError::CheckpointInvalid {
            reason: "materialized-state audit mismatch is latched".to_string(),
        }
        .into());
    }
    frontier.ensure_covers_checkpoint_position(checkpoint)?;
    frontier.ensure_covers_checkpoint(checkpoint)?;
    Ok(frontier.is_at_checkpoint_position(checkpoint))
}

pub(crate) async fn historical_entries_referenced_by_finalized_items(
    mint: &Mint,
    finalized_items: &[FederationAlephBftFinalizedItem],
    checkpoint_journal_next_index: u64,
) -> std::result::Result<Vec<FederationJournalEntry>, cdk::Error> {
    let db = mint.localstore();
    let mut historical_entries = BTreeMap::new();
    for finalized_item in finalized_items {
        if finalized_item.status == FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation
        {
            continue;
        }
        let FederationConsensusItem::Operation(envelope) = &finalized_item.item else {
            continue;
        };
        let operation_id = envelope.operation_id()?;
        if let Some(entry) = db
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await?
            .filter(|entry| entry.index < checkpoint_journal_next_index)
        {
            historical_entries.insert(entry.index, entry);
        }
        for conflict_key in envelope.operation.conflict_keys() {
            if let Some(entry) = db
                .get_federation_journal_entry_by_conflict_key(&conflict_key)
                .await?
                .filter(|entry| entry.index < checkpoint_journal_next_index)
            {
                historical_entries.insert(entry.index, entry);
            }
        }
    }
    Ok(historical_entries.into_values().collect())
}

pub(crate) struct FederationCheckpointStartupVerifier<'a> {
    pub(crate) mint: &'a Mint,
    pub(crate) config: &'a FederationConfig,
    pub(crate) member_id: MemberId,
    pub(crate) active_session: u64,
    pub(crate) checkpoint: &'a VerifiedFederationCheckpointQuorum,
}

impl FederationCheckpointStartupVerifier<'_> {
    pub(crate) async fn verify(
        &self,
        frontier: &FederationCheckpointFrontier,
        journal_entries: &[FederationJournalEntry],
        finalized_items: &[FederationAlephBftFinalizedItem],
    ) -> std::result::Result<cdk::federation::VerifiedFederationSuffix, cdk::Error> {
        let checkpoint_payload = self.checkpoint.checkpoint();
        let historical_entries = historical_entries_referenced_by_finalized_items(
            self.mint,
            finalized_items,
            checkpoint_payload.journal_next_index,
        )
        .await?;
        let mut verifier = FederationSuffixVerifier::new_for_validated_config(
            self.config,
            self.member_id,
            self.active_session,
            FederationConsensusRecoveryBase::Checkpoint(self.checkpoint),
        )?;
        verifier.register_historical_entries(historical_entries)?;
        verifier.push_finalized_items(finalized_items)?;
        verifier.push_journal_entries(journal_entries)?;
        Ok(verifier.finish_for_durable_frontier(frontier)?)
    }

    pub(crate) async fn repair_application(
        &self,
        initial_frontier: &FederationCheckpointFrontier,
        initial_journal_entries: &[FederationJournalEntry],
        finalized_items: &[FederationAlephBftFinalizedItem],
        terminal_prefix_len: usize,
        consensus: &mut FederationConsensusRuntime,
    ) -> std::result::Result<usize, cdk::Error> {
        let checkpoint_payload = self.checkpoint.checkpoint();
        let entries_to_replay = &initial_journal_entries[terminal_prefix_len..];
        if !entries_to_replay.is_empty() {
            tracing::info!(
                trusted_checkpoint_journal_index = checkpoint_payload.journal_next_index,
                application_next_index = initial_frontier.application_next_index,
                journal_next_index = initial_frontier.journal_next_index,
                replay_entries = entries_to_replay.len(),
                "Repairing terminal application after trusted checkpoint startup"
            );
        } else {
            return Ok(0);
        }
        for entry in entries_to_replay {
            federation::apply_federation_journal_entry_for_replay(self.mint, entry).await?;
        }

        let db = self.mint.localstore();
        let repaired_frontier = db.get_federation_checkpoint_frontier().await?;
        if !repaired_frontier.ordered_history_matches(initial_frontier) {
            return Err(FederationError::CheckpointStartupSuffixMismatch {
                reason: "durable ordered frontier changed during startup application replay"
                    .to_string(),
            }
            .into());
        }
        let repaired_journal_entries = db
            .get_federation_journal_entries_from(checkpoint_payload.journal_next_index)
            .await?;
        let repaired_suffix = self
            .verify(
                &repaired_frontier,
                &repaired_journal_entries,
                finalized_items,
            )
            .await?;
        let repaired_terminal_prefix_len =
            usize::try_from(repaired_suffix.terminal_application_count()).map_err(|_| {
                FederationError::CheckpointStartupSuffixMismatch {
                    reason: "terminal application suffix length exceeds local address space"
                        .to_string(),
                }
            })?;
        if !repaired_frontier.is_fully_terminal()
            || repaired_terminal_prefix_len != repaired_journal_entries.len()
        {
            return Err(
                FederationError::CheckpointStartupApplicationReplayIncomplete {
                    expected: repaired_frontier.journal_next_index,
                    found: repaired_frontier.application_next_index,
                }
                .into(),
            );
        }
        *consensus = FederationConsensusRuntime::recover_verified_suffix(repaired_suffix);
        Ok(entries_to_replay.len())
    }
}

pub(crate) struct FederationRuntimeState {
    pub(crate) member: Option<federation::FederationMemberContext>,
    #[cfg(feature = "federation-bdk")]
    pub(crate) onchain_wallets: Vec<FederatedBdkWallet>,
    #[cfg(feature = "federation-bdk")]
    pub(crate) onchain_sync_interval: Option<std::time::Duration>,
    pub(crate) frost_signing: Option<federation::frost_signing::FederationFrostSigningRuntime>,
    #[cfg(feature = "federation-bark")]
    pub(crate) bark: Option<Arc<cdk_bark::FederatedBarkRuntime>>,
    #[cfg(feature = "federation-bdk")]
    pub(crate) bdk_degraded_reason: Option<String>,
}

pub(crate) async fn create_federation_runtime_state_with_wallet(
    mint: &Arc<Mint>,
    _peer_sync_config: FederationPeerSyncConfig,
    _federation_transport: FederationTransport,
    #[cfg(feature = "federation-bdk")] local_wallet_config: Option<FederatedBdkLocalConfig>,
    #[cfg(feature = "federation-bark")] local_bark_config: Option<
        cdk_bark::FederatedBarkLocalConfig,
    >,
) -> Result<FederationRuntimeState> {
    let Some(config) = mint.federation_config() else {
        #[cfg(feature = "federation-bdk")]
        if local_wallet_config.is_some() {
            return Err(anyhow::anyhow!(
                "federated BDK configuration requires an active federation mint"
            ));
        }
        #[cfg(feature = "federation-bark")]
        if local_bark_config.is_some() {
            return Err(anyhow::anyhow!(
                "federated Bark configuration requires an active federation mint"
            ));
        }
        return Ok(FederationRuntimeState {
            member: None,
            #[cfg(feature = "federation-bdk")]
            onchain_wallets: Vec::new(),
            #[cfg(feature = "federation-bdk")]
            onchain_sync_interval: None,
            frost_signing: None,
            #[cfg(feature = "federation-bark")]
            bark: None,
            #[cfg(feature = "federation-bdk")]
            bdk_degraded_reason: None,
        });
    };
    let db = mint.localstore();
    validate_and_materialize_frost_wallet_config(mint).await?;
    #[cfg(feature = "federation-bdk")]
    let frost_storage_root = local_wallet_config
        .as_ref()
        .map(|local| local.storage_root.clone());
    #[cfg(not(feature = "federation-bdk"))]
    let frost_storage_root: Option<std::path::PathBuf> = None;
    #[cfg(feature = "federation-bark")]
    let frost_storage_root = frost_storage_root.or_else(|| {
        local_bark_config
            .as_ref()
            .map(|local| local.data_directory.join("frost-signing"))
    });
    let frost_signing = match frost_storage_root {
        Some(storage_root) => {
            federation::frost_signing::FederationFrostSigningRuntime::open(mint, &storage_root)
                .await?
        }
        None => None,
    };
    #[cfg(feature = "federation-bdk")]
    let onchain_sync_interval = local_wallet_config
        .as_ref()
        .map(|config| config.sync_interval);
    #[cfg(feature = "federation-bdk")]
    let onchain_wallets = initialize_federation_onchain_wallets(config, local_wallet_config)?;
    #[cfg(feature = "federation-bark")]
    let bark = match local_bark_config {
        Some(local) => {
            let local_signing = frost_signing.clone().ok_or_else(|| {
                anyhow::anyhow!("federated Bark requires a local FROST signing runtime")
            })?;
            let wallet = config
                .public_config()
                .frost_wallet
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("federated Bark requires a FROST wallet"))?;
            let application = cdk::federation::federation_bark_application(config.public_config())?
                .ok_or_else(|| anyhow::anyhow!("federated Bark application is missing"))?;
            let custody_public =
                application.derived_public_package(config.public_config(), &wallet.root)?;
            let recovery_application =
                cdk::federation::federation_bark_recovery_application(config.public_config())?
                    .ok_or_else(|| {
                        anyhow::anyhow!("federated Bark recovery application is missing")
                    })?;
            let recovery_public = recovery_application
                .derived_public_package(config.public_config(), &wallet.root)?;
            let peer_client =
                federation::peer_client::FederationRestPeerClient::new_with_validated_config(
                    _federation_transport.clone(),
                    config.public_config(),
                    config.private_config().clone(),
                    _peer_sync_config,
                )?;
            let custody_client = Arc::new(
                federation::bark_custody::FederatedBarkCustodyPeerClient::new(
                    peer_client,
                    config.public_config().clone(),
                    custody_public,
                    recovery_public,
                    config.member_id(),
                    Arc::clone(mint),
                    local_signing,
                ),
            );
            Some(
                cdk_bark::FederatedBarkRuntime::open(
                    true,
                    config.public_config(),
                    config.member_id(),
                    local,
                    custody_client,
                )
                .await?,
            )
        }
        None => None,
    };
    let snapshot = FederationConfigSnapshot::new(config.public_config().clone())?;
    let mut tx = db.begin_transaction().await?;
    tx.set_federation_config(&snapshot).await?;
    tx.commit().await?;

    let dkg_sequences = db.get_federation_dkg_member_sequences().await?;
    let active_session = db
        .get_federation_aleph_bft_active_session()
        .await?
        .unwrap_or(0);
    let frontier = db.get_federation_checkpoint_frontier().await?;
    let trusted_quorum = db.get_trusted_federation_checkpoint_quorum().await?;
    let checkpoint_consensus = match (frontier.trusted_checkpoint_id, trusted_quorum) {
        (None, None) => None,
        (Some(_), None) => {
            return Err(FederationError::CheckpointInvalid {
                reason: "trusted checkpoint metadata references a missing quorum".to_string(),
            }
            .into());
        }
        (None, Some(_)) => {
            return Err(FederationError::CheckpointInvalid {
                reason: "trusted checkpoint quorum is not bound to the durable frontier"
                    .to_string(),
            }
            .into());
        }
        (Some(trusted_checkpoint_id), Some(quorum)) => {
            let verified_quorum = federation::verify_checkpoint_quorum_with_metrics(
                &quorum,
                config.public_config(),
                config.member_id(),
                federation::CheckpointQuorumVerificationContext::StartupCatchUp,
            )?;
            let checkpoint = verified_quorum.checkpoint();
            if checkpoint.id()? != trusted_checkpoint_id {
                return Err(FederationError::CheckpointInvalid {
                    reason: "trusted checkpoint ID does not match its stored quorum".to_string(),
                }
                .into());
            }
            if active_session < checkpoint.aleph_bft_session {
                return Err(FederationError::CheckpointInvalid {
                    reason: "active AlephBFT session is behind the trusted checkpoint".to_string(),
                }
                .into());
            }
            let exact_checkpoint = checkpoint_is_exact_startup_frontier(checkpoint, &frontier)?;
            #[cfg(feature = "prometheus")]
            let tail_started_at = std::time::Instant::now();
            let (journal_entries, finalized_items) = if exact_checkpoint {
                (Vec::new(), Vec::new())
            } else {
                let journal_entries = db
                    .get_federation_journal_entries_from(checkpoint.journal_next_index)
                    .await?;
                let finalized_items = db
                    .get_federation_aleph_bft_finalized_items_from(
                        checkpoint.finalized_next_item_index,
                    )
                    .await?;
                #[cfg(feature = "prometheus")]
                {
                    let member = config.member_id().to_string();
                    let journal_rows = u64::try_from(journal_entries.len()).unwrap_or(u64::MAX);
                    let finalized_rows = u64::try_from(finalized_items.len()).unwrap_or(u64::MAX);
                    cdk_prometheus::METRICS.record_federation_checkpoint_work(
                        &member,
                        cdk_prometheus::FederationCheckpointWork::JournalRowsLoaded,
                        journal_rows,
                    );
                    cdk_prometheus::METRICS.record_federation_checkpoint_work(
                        &member,
                        cdk_prometheus::FederationCheckpointWork::FinalizedItemsLoaded,
                        finalized_rows,
                    );
                    cdk_prometheus::METRICS.record_federation_checkpoint_work(
                        &member,
                        cdk_prometheus::FederationCheckpointWork::StartupTailJournalRowsLoaded,
                        journal_rows,
                    );
                    cdk_prometheus::METRICS.record_federation_checkpoint_work(
                        &member,
                        cdk_prometheus::FederationCheckpointWork::StartupTailFinalizedItemsLoaded,
                        finalized_rows,
                    );
                }
                (journal_entries, finalized_items)
            };
            let startup_verifier = FederationCheckpointStartupVerifier {
                mint,
                config: config.public_config(),
                member_id: config.member_id(),
                active_session,
                checkpoint: &verified_quorum,
            };
            let verified_suffix = startup_verifier
                .verify(&frontier, &journal_entries, &finalized_items)
                .await?;
            let terminal_prefix_len = usize::try_from(verified_suffix.terminal_application_count())
                .map_err(|_| FederationError::CheckpointStartupSuffixMismatch {
                    reason: "terminal application suffix length exceeds local address space"
                        .to_string(),
                })?;
            let mut consensus =
                FederationConsensusRuntime::recover_verified_suffix(verified_suffix);
            if !exact_checkpoint {
                let replayed_entries = startup_verifier
                    .repair_application(
                        &frontier,
                        &journal_entries,
                        &finalized_items,
                        terminal_prefix_len,
                        &mut consensus,
                    )
                    .await?;
                #[cfg(feature = "prometheus")]
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &config.member_id().to_string(),
                    cdk_prometheus::FederationCheckpointWork::StartupEntriesReplayed,
                    u64::try_from(replayed_entries).unwrap_or(u64::MAX),
                );
                #[cfg(not(feature = "prometheus"))]
                let _ = replayed_entries;
                #[cfg(feature = "prometheus")]
                cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                    &config.member_id().to_string(),
                    cdk_prometheus::FederationCheckpointOperation::StartupTail,
                    tail_started_at.elapsed().as_secs_f64(),
                );
            }
            Some(consensus)
        }
    };
    let used_checkpoint = checkpoint_consensus.is_some();
    let mut consensus = match checkpoint_consensus {
        Some(consensus) => consensus,
        None => {
            let persisted_entries = db.get_federation_journal_entries().await?;
            let finalized_items = db.get_federation_aleph_bft_finalized_items().await?;
            #[cfg(feature = "prometheus")]
            {
                let member = config.member_id().to_string();
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &member,
                    cdk_prometheus::FederationCheckpointWork::JournalRowsLoaded,
                    u64::try_from(persisted_entries.len()).unwrap_or(u64::MAX),
                );
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &member,
                    cdk_prometheus::FederationCheckpointWork::FinalizedItemsLoaded,
                    u64::try_from(finalized_items.len()).unwrap_or(u64::MAX),
                );
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &member,
                    cdk_prometheus::FederationCheckpointWork::StartupEntriesReplayed,
                    u64::try_from(persisted_entries.len()).unwrap_or(u64::MAX),
                );
            }
            let recovered_session =
                latest_aleph_bft_finalized_session(&finalized_items).max(active_session);
            let mut verifier = FederationSuffixVerifier::new_for_validated_config(
                config.public_config(),
                config.member_id(),
                recovered_session,
                FederationConsensusRecoveryBase::Genesis,
            )?;
            verifier.push_finalized_items(&finalized_items)?;
            verifier.push_journal_entries(&persisted_entries)?;
            FederationConsensusRuntime::recover_verified_suffix(
                verifier.finish_for_durable_frontier(&frontier)?,
            )
        }
    };
    let session_id = consensus.aleph_bft_status()?.session;
    let mut tx = db.begin_transaction().await?;
    tx.set_federation_aleph_bft_active_session(session_id)
        .await?;
    if used_checkpoint {
        tx.prune_federation_aleph_bft_backups_before(session_id)
            .await?;
    }
    tx.commit().await?;
    let pending_operations = db
        .get_federation_aleph_bft_pending_operations(session_id)
        .await?;
    let pending_by_id = pending_operations
        .iter()
        .cloned()
        .map(|pending| (pending.operation_id, pending))
        .collect::<BTreeMap<_, _>>();
    let mut recovered_groups = BTreeSet::new();
    for pending in pending_operations {
        let operation_id = pending.envelope.operation_id()?;
        if operation_id != pending.operation_id {
            return Err(cdk::Error::Custom(format!(
                "AlephBFT pending operation ID mismatch: stored {}, envelope {}",
                pending.operation_id, operation_id
            ))
            .into());
        }
        let Some(group) = pending.ordered_group.clone() else {
            if consensus
                .journal()
                .entry_by_operation_id(pending.operation_id)
                .is_some()
                || db
                    .get_federation_journal_entry_by_operation_id(pending.operation_id)
                    .await?
                    .is_some()
            {
                continue;
            }
            consensus.submit_aleph_bft_envelope(pending.envelope)?;
            continue;
        };
        if group.operation_ids.is_empty()
            || group
                .operation_ids
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
                != group.operation_ids.len()
        {
            return Err(FederationError::AlephBftOrderedOperationGroupInvalid {
                reason: "persisted ordered group is empty or contains duplicate IDs".to_string(),
            }
            .into());
        }
        let group_key = group.operation_ids[0];
        if !recovered_groups.insert(group_key) {
            continue;
        }
        let mut envelopes = Vec::with_capacity(group.operation_ids.len());
        let mut finalized = Vec::with_capacity(group.operation_ids.len());
        for group_operation_id in &group.operation_ids {
            let member = pending_by_id.get(group_operation_id).ok_or_else(|| {
                FederationError::AlephBftOrderedOperationGroupInvalid {
                    reason: "persisted ordered group is incomplete".to_string(),
                }
            })?;
            if member.session_id != session_id || member.ordered_group.as_ref() != Some(&group) {
                return Err(FederationError::AlephBftOrderedOperationGroupInvalid {
                    reason: "persisted ordered group metadata is inconsistent".to_string(),
                }
                .into());
            }
            let computed = member.envelope.operation_id()?;
            if computed != *group_operation_id || computed != member.operation_id {
                return Err(FederationError::AlephBftOrderedOperationGroupInvalid {
                    reason: "persisted ordered group operation ID is inconsistent".to_string(),
                }
                .into());
            }
            finalized.push(
                consensus
                    .journal()
                    .entry_by_operation_id(*group_operation_id)
                    .is_some()
                    || db
                        .get_federation_journal_entry_by_operation_id(*group_operation_id)
                        .await?
                        .is_some(),
            );
            envelopes.push(member.envelope.clone());
        }
        if finalized.iter().any(|is_finalized| *is_finalized) {
            if !finalized.iter().all(|is_finalized| *is_finalized) {
                return Err(FederationError::AlephBftOrderedOperationGroupInvalid {
                    reason: "persisted ordered group is only partially finalized".to_string(),
                }
                .into());
            }
            continue;
        }
        consensus.submit_aleph_bft_envelope_group(envelopes)?;
    }
    let entries = consensus.journal().entries().to_vec();
    if !used_checkpoint {
        let entries_to_apply = accepted_federation_journal_entries(&entries);
        let applied = mint
            .apply_federation_journal_entries(&entries_to_apply)
            .await?;
        for operation_id in applied {
            consensus.update_status(operation_id, FederationOperationStatus::Applied)?;
        }
    }
    #[cfg(feature = "federation-bdk")]
    let startup_bdk_degraded_reason = match reconcile_federation_onchain_wallets(
        mint,
        &onchain_wallets,
    )
    .await
    {
        Ok(()) => None,
        Err(err) => {
            let reason = format!("{err:#}");
            tracing::error!(
                reason = %reason,
                "federation local BDK reconciliation failed during startup; member starts BDK-degraded and local signing is blocked"
            );
            Some(reason)
        }
    };
    let sequences = FederationRestSequenceTracker::from_dkg_sequences(dkg_sequences);

    #[cfg(feature = "federation-bark")]
    if let Some(runtime) = bark.as_ref() {
        let persisted_entries = db.get_federation_journal_entries().await?;
        if let Err(err) = reconcile_federation_bark_runtime(mint, runtime, &persisted_entries).await
        {
            tracing::error!(
                reason = %format!("{err:#}"),
                "federation local Bark reconciliation failed during startup; member starts Bark-degraded"
            );
        }
    }

    Ok(FederationRuntimeState {
        member: Some(federation::FederationMemberContext::new(
            Arc::clone(mint),
            Arc::new(tokio::sync::Mutex::new(consensus)),
            Arc::new(tokio::sync::Mutex::new(sequences)),
            _peer_sync_config,
        )),
        #[cfg(feature = "federation-bdk")]
        onchain_wallets,
        #[cfg(feature = "federation-bdk")]
        onchain_sync_interval,
        frost_signing,
        #[cfg(feature = "federation-bark")]
        bark,
        #[cfg(feature = "federation-bdk")]
        bdk_degraded_reason: startup_bdk_degraded_reason,
    })
}

#[cfg(feature = "federation-bark")]
pub(crate) async fn reconcile_federation_bark_runtime(
    mint: &Mint,
    runtime: &Arc<cdk_bark::FederatedBarkRuntime>,
    entries: &[FederationJournalEntry],
) -> Result<()> {
    cdk_bark::FederatedBarkRecoveryDriver::new(runtime.clone())
        .reconcile(&AxumFederatedBarkReplaySource { mint }, entries)
        .await
        .map_err(|_| anyhow::anyhow!("accepted Bark state could not be replayed"))
}

#[cfg(feature = "federation-bark")]
pub(crate) struct AxumFederatedBarkReplaySource<'a> {
    pub(crate) mint: &'a Mint,
}

#[cfg(feature = "federation-bark")]
#[async_trait::async_trait]
impl cdk_bark::FederatedBarkReplaySource for AxumFederatedBarkReplaySource<'_> {
    async fn replay_projection(
        &self,
    ) -> std::result::Result<cdk::federation::FederationBarkProjectionSnapshot, cdk_bark::Error>
    {
        self.mint
            .federation_bark_projection_snapshot()
            .await
            .map_err(|_| cdk_bark::Error::ReceiveProjectionFailed)
    }

    async fn replay_entry(
        &self,
        operation_id: cdk::federation::FederationOperationId,
    ) -> std::result::Result<FederationJournalEntry, cdk_bark::Error> {
        self.mint
            .localstore()
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await
            .map_err(|_| cdk_bark::Error::MaintenanceReconciliationFailed)?
            .ok_or(cdk_bark::Error::MaintenanceReconciliationFailed)
    }

    async fn replay_receive_intent(
        &self,
        operation_id: cdk::federation::FederationOperationId,
    ) -> std::result::Result<cdk::federation::FederationBarkReceiveIntent, cdk_bark::Error> {
        self.mint
            .federation_bark_receive_intent(operation_id)
            .await
            .map_err(|_| cdk_bark::Error::ReceiveProjectionFailed)?
            .ok_or(cdk_bark::Error::ReceiveProjectionFailed)
    }

    async fn replay_melt_quote(
        &self,
        quote: &str,
    ) -> std::result::Result<cdk_common::mint::MeltQuote, cdk_bark::Error> {
        let quote = quote
            .parse::<cdk::mint::QuoteId>()
            .map_err(|_| cdk_bark::Error::SendReconciliationFailed)?;
        self.mint
            .localstore()
            .get_melt_quote(&quote)
            .await
            .map_err(|_| cdk_bark::Error::SendReconciliationFailed)?
            .ok_or(cdk_bark::Error::SendReconciliationFailed)
    }
}

#[cfg(feature = "federation-bark")]
pub(crate) async fn federation_bark_ready_projection(
    mint: &Mint,
    runtime: &Arc<cdk_bark::FederatedBarkRuntime>,
) -> Result<cdk::federation::FederationBarkProjectionSnapshot> {
    let mut projection = mint.federation_bark_projection_snapshot().await?;
    let required_frontier = projection.frontier().unwrap_or(0);
    if runtime.ensure_ready(required_frontier).await.is_err() {
        let entries = mint.localstore().get_federation_journal_entries().await?;
        reconcile_federation_bark_runtime(mint, runtime, &entries).await?;
        projection = mint.federation_bark_projection_snapshot().await?;
    }
    runtime
        .ensure_ready(projection.frontier().unwrap_or(0))
        .await
        .map_err(|_| anyhow::anyhow!("federation Bark runtime is behind or degraded"))?;
    Ok(projection)
}

#[cfg(feature = "federation-bdk")]
pub(crate) async fn reconcile_federation_onchain_wallets(
    mint: &Mint,
    wallets: &[FederatedBdkWallet],
) -> Result<()> {
    let address_state = mint.federation_address_allocation_state().await?;
    let transaction_state = mint.federation_transaction_state().await?;
    for wallet in wallets {
        let wallet_scope = wallet.scope();
        let application_id = cdk::federation::FederationFrostApplicationId::from_bytes(
            wallet_scope.application_id(),
        )?;
        let scope = cdk::federation::FederationAddressAllocationScope {
            application_id,
            application_key_epoch: wallet_scope.application_key_epoch(),
        };
        if let Some(materialized) = address_state.wallet(scope) {
            wallet
                .reconcile_revealed_counters(
                    materialized.external_next_index,
                    materialized.internal_next_index,
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "failed to reconcile accepted federated address allocation into local BDK state"
                    )
                })?;
        }
        for accepted in transaction_state
            .accepted_transactions()
            .values()
            .filter(|accepted| {
                accepted.proposal.scope.root_key_epoch == wallet_scope.root_key_epoch()
                    && accepted.proposal.scope.application_id.to_bytes()
                        == wallet_scope.application_id()
                    && accepted.proposal.scope.application_key_epoch
                        == wallet_scope.application_key_epoch()
            })
        {
            let Some(signed) = &accepted.signed else {
                continue;
            };
            signed.verify_signatures(&accepted.proposal)?;
            wallet
                .engine()
                .apply_accepted_transaction(
                    signed.transaction()?,
                    accepted.accepted_operation_index,
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "failed to reconcile an accepted federated transaction into local BDK state"
                    )
                })?;
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "federation"))]
pub(crate) async fn create_federation_runtime_state(
    mint: &Arc<Mint>,
) -> Result<(
    Option<FederationConsensusHandle>,
    Option<FederationSequenceHandle>,
)> {
    let state = create_federation_runtime_state_with_wallet(
        mint,
        FederationPeerSyncConfig::default(),
        default_federation_transport(),
        #[cfg(feature = "federation-bdk")]
        None,
        #[cfg(feature = "federation-bark")]
        None,
    )
    .await?;
    Ok(match state.member {
        Some(member) => (Some(member.consensus), Some(member.sequences)),
        None => (None, None),
    })
}

#[cfg(feature = "federation-bdk")]
pub(crate) fn initialize_federation_onchain_wallets(
    runtime: &cdk::mint::FederationRuntimeConfig,
    local_config: Option<FederatedBdkLocalConfig>,
) -> Result<Vec<FederatedBdkWallet>> {
    let public = runtime.public_config();
    let Some(wallet_config) = &public.frost_wallet else {
        if local_config.is_some() {
            return Err(anyhow::anyhow!(
                "federated BDK configuration requires accepted FROST wallet config"
            ));
        }
        return Ok(Vec::new());
    };
    initialize_accepted_federation_onchain_wallets(
        public.federation_id,
        wallet_config,
        local_config,
    )
}

#[cfg(feature = "federation-bdk")]
pub(crate) fn initialize_accepted_federation_onchain_wallets(
    federation_id: cdk::federation::FederationId,
    wallet_config: &cdk::federation::FederationFrostWalletConfig,
    local_config: Option<FederatedBdkLocalConfig>,
) -> Result<Vec<FederatedBdkWallet>> {
    let Some(application) = wallet_config.cdk_onchain_application.as_ref() else {
        if local_config.is_some() {
            return Err(anyhow::anyhow!(
                "member-local BDK configuration has no accepted CDK on-chain application"
            ));
        }
        return Ok(Vec::new());
    };
    let local_config = local_config.ok_or_else(|| {
        anyhow::anyhow!(
            "active federated on-chain application requires member-local BDK configuration"
        )
    })?;

    [application]
        .into_iter()
        .map(|application| {
            let scope = FederatedWalletScope::new(
                federation_id.to_bytes(),
                application.application_id.to_bytes(),
                application.root_key_epoch,
                application.application_key_epoch,
                application.descriptor_commitment,
            )?;
            let config = FederatedWalletConfig::new(
                scope,
                &application.external_descriptor,
                &application.internal_descriptor,
                application.network,
            )?;
            let wallet = FederatedBdkWallet::open_with_observation_source(
                config,
                local_config.chain_source.clone(),
                &local_config.storage_root,
                local_config.observation_source.clone(),
            )?;
            if matches!(
                wallet.observation_source().scope,
                cdk_bdk::FederatedChainSourceScope::Shared
            ) {
                tracing::warn!(
                    source = ?wallet.engine().chain_source_kind(),
                    source_id = %wallet.observation_source().source_id,
                    "Federated BDK chain source is intentionally shared; member quorum does not provide independent upstream observations"
                );
            }
            Ok(wallet)
        })
        .collect()
}

pub(crate) async fn validate_and_materialize_frost_wallet_config(mint: &Arc<Mint>) -> Result<()> {
    let runtime = mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("federation mint is missing its runtime config".to_string())
    })?;
    let public = runtime.public_config();
    let Some(wallet) = &public.frost_wallet else {
        return Ok(());
    };
    wallet.validate(public)?;

    let db = mint.localstore();
    let mut tx = db.begin_transaction().await?;
    tx.add_federation_frost_root_public_config(&wallet.root)
        .await?;
    if let Some(application) = wallet.cdk_onchain_application.as_ref() {
        tx.add_federation_frost_application_config(application)
            .await?;
    }
    tx.commit().await?;

    let stored_roots = db.get_federation_frost_root_public_configs().await?;
    let stored_applications = db.get_federation_frost_application_configs().await?;
    let expected_applications = wallet
        .cdk_onchain_application
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    if stored_roots.as_slice() != std::slice::from_ref(&wallet.root)
        || stored_applications != expected_applications
    {
        return Err(cdk::Error::Custom(
            "persisted federation FROST wallet configuration mismatch".to_string(),
        )
        .into());
    }

    let private_roots = db.get_federation_frost_root_private_configs().await?;
    let [local] = private_roots.as_slice() else {
        return Err(cdk::Error::Custom(
            "missing or unknown participant-local FROST root configuration".to_string(),
        )
        .into());
    };
    let root = &wallet.root;
    let context = FederationFrostDkgContext::new(public, root.ceremony_id, root.root_key_epoch)?;
    let storage_key = crate::federation::frost_storage_key(runtime.private_config(), &context)?;
    local.open(public, root, runtime.member_id(), &storage_key)?;
    Ok(())
}

pub(crate) fn latest_aleph_bft_finalized_session(
    finalized_items: &[FederationAlephBftFinalizedItem],
) -> u64 {
    finalized_items
        .iter()
        .map(|item| item.session)
        .max()
        .unwrap_or(0)
}
