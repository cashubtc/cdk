//! Federation mint state integration.

use std::sync::Arc;

use anyhow::Result;

#[cfg(feature = "federation-bdk")]
use cdk::federation::FederationOnchainWalletStatus;
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use cdk::federation::{
    FederationAddressAllocationScope, FederationAddressKeychain, FederationChainObservation,
    FederationFrostApplicationId,
};
use cdk::federation::{
    FederationAlephBftUnitPayload, FederationCheckpointQuorum, FederationConsensusRuntimeStatus,
    FederationDkgActivationRequest, FederationDkgFinalizedMemberConfig,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgSecretShareRequest, FederationDkgTranscriptSignatureRequest, FederationError,
    FederationInvalidOperationAlertSummary, FederationJournalAppendOutcome,
    FederationJournalCatchUpResponse, FederationJournalEntry, FederationOperationEnvelope,
    FederationOperationId, FederationOperationKind, FederationOperationStatus,
    FederationRestMessageKind, FederationSubmitOperationResponse, MemberId,
};

use super::state::*;
use crate::{federation, MintState};

impl MintState {
    /// Validate one bounded local descriptor-wallet observation against
    /// accepted allocation state, then submit it through normal federation
    /// consensus. Raw member-local scans never mutate canonical UTXOs directly.
    #[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
    pub async fn submit_federation_chain_observation(
        &self,
        observation: FederationChainObservation,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        if let FederationChainObservation::Deposit(deposit) = &observation {
            let allocation = self
                .mint
                .federation_address_allocation(
                    deposit.allocation.scope(),
                    &deposit.allocation.quote,
                )
                .await?
                .ok_or_else(|| {
                    cdk::Error::Custom(
                        "federation chain observation has no accepted address allocation"
                            .to_string(),
                    )
                })?;
            deposit.allocation.validate_allocation(&allocation)?;
            let runtime = self.mint.federation_config().ok_or(cdk::Error::Internal)?;
            let wallet_config = runtime
                .public_config()
                .frost_wallet
                .as_ref()
                .ok_or(cdk::Error::Internal)?;
            let expected_script = wallet_config
                .cdk_onchain_application
                .as_ref()
                .filter(|application| {
                    application.application_id == deposit.allocation.application_id
                        && application.application_key_epoch
                            == deposit.allocation.application_key_epoch
                })
                .map(|_| {
                    wallet_config.derive_cdk_onchain_script(
                        runtime.public_config(),
                        deposit.allocation.application_id,
                        deposit.allocation.application_key_epoch,
                        deposit.allocation.keychain,
                        deposit.allocation.derivation_index,
                    )
                })
                .transpose()?;
            #[cfg(feature = "federation-bark")]
            let expected_script = match expected_script {
                Some(script) => Some(script),
                None => cdk_federation_bark::federation_bark_recovery_application(
                    runtime.public_config(),
                )?
                .filter(|application| {
                    application.application_id() == deposit.allocation.application_id
                        && application.application_key_epoch()
                            == deposit.allocation.application_key_epoch
                })
                .map(|application| {
                    application.derive_script(
                        runtime.public_config(),
                        &wallet_config.root,
                        deposit.allocation.keychain == FederationAddressKeychain::Internal,
                        deposit.allocation.derivation_index,
                    )
                })
                .transpose()?,
            };
            let expected_script = expected_script.ok_or_else(|| {
                cdk::Error::Federation(FederationError::ChainObservationInvalid {
                    reason: "deposit scope is not a configured descriptor application".to_string(),
                })
            })?;
            if deposit.txout.script_pubkey != expected_script {
                return Err(cdk::Error::Federation(
                    FederationError::ChainObservationInvalid {
                        reason: "deposit script does not match accepted allocation".to_string(),
                    },
                ));
            }
            #[cfg(feature = "federation-bdk")]
            let local_wallet = self.federation.onchain_wallets().iter().find_map(|wallet| {
                let scope = wallet.scope();
                (scope.application_id() == deposit.allocation.application_id.to_bytes()
                    && scope.application_key_epoch() == deposit.allocation.application_key_epoch)
                    .then(|| wallet.clone())
            });
            #[cfg(not(feature = "federation-bdk"))]
            let local_wallet = None::<cdk_bitcoin_wallet::FederatedDescriptorWallet>;
            #[cfg(feature = "federation-bark")]
            let local_wallet = local_wallet.or_else(|| {
                self.federation.bark_runtime().and_then(|runtime| {
                    let wallet = runtime.recovery_wallet();
                    let scope = wallet.scope();
                    (scope.application_id() == deposit.allocation.application_id.to_bytes()
                        && scope.application_key_epoch()
                            == deposit.allocation.application_key_epoch)
                        .then(|| wallet.as_ref().clone())
                })
            });
            let local_wallet = local_wallet.ok_or_else(|| {
                cdk::Error::Custom(
                    "federation chain observation has no local watch-only wallet".to_string(),
                )
            })?;
            let required_height = deposit.confirmation.map_or(0, |confirmation| {
                confirmation
                    .block_height
                    .saturating_add(confirmation.confirmation_depth.saturating_sub(1))
            });
            local_wallet
                .ensure_ready(required_height)
                .await
                .map_err(|_| {
                    cdk::Error::Custom(
                        "local federation chain view is not ready for observation".to_string(),
                    )
                })?;
        }
        let envelope = self
            .mint
            .federation_config()
            .ok_or(cdk::Error::Internal)?
            .chain_observation_operation(observation)?;
        self.submit_federation_envelope(envelope).await
    }

    /// Allocate or recover one consensus-controlled address. No BDK reveal is
    /// attempted until the operation has a terminal consensus result and the
    /// accepted mapping is readable from materialized state.
    #[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
    pub async fn allocate_federation_onchain_address(
        &self,
        quote: &str,
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
        keychain: FederationAddressKeychain,
    ) -> Result<String, cdk::Error> {
        let timer = crate::federation::latency::FederationLatencyTimer::start(
            crate::federation::latency::FederationLatencyCorrelation::Quote,
            quote.to_owned(),
            crate::federation::latency::FederationLatencyBackend::Bdk,
            crate::federation::latency::FederationLatencyDirection::Internal,
            crate::federation::latency::FederationLatencyPhase::BdkAddressAllocation,
        );
        let result = self
            .allocate_federation_onchain_address_inner(
                quote,
                application_id,
                application_key_epoch,
                keychain,
            )
            .await;
        timer.finish(
            crate::federation::latency::FederationLatencyOutcome::from_success(result.is_ok()),
        );
        result
    }

    #[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
    pub(crate) async fn allocate_federation_onchain_address_inner(
        &self,
        quote: &str,
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
        keychain: FederationAddressKeychain,
    ) -> Result<String, cdk::Error> {
        if let Some(address) = self
            .reveal_accepted_federation_onchain_address(
                quote,
                application_id,
                application_key_epoch,
                keychain,
            )
            .await?
        {
            return Ok(address);
        }
        let envelope = self
            .mint
            .federation_config()
            .ok_or(cdk::Error::Internal)?
            .address_allocation_operation(quote, application_id, application_key_epoch, keychain)?;
        let response = self.submit_federation_envelope(envelope).await?;
        if let Some(address) = self
            .reveal_accepted_federation_onchain_address(
                quote,
                application_id,
                application_key_epoch,
                keychain,
            )
            .await?
        {
            return Ok(address);
        }
        response.ensure_accepted()?;
        Err(cdk::Error::Custom(
            "accepted federation address allocation is not materialized".to_string(),
        ))
    }

    #[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
    pub(crate) async fn reveal_accepted_federation_onchain_address(
        &self,
        quote: &str,
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
        keychain: FederationAddressKeychain,
    ) -> Result<Option<String>, cdk::Error> {
        let scope = FederationAddressAllocationScope {
            application_id,
            application_key_epoch,
        };
        let allocation = match self
            .mint
            .federation_address_allocation(scope, quote)
            .await?
        {
            Some(allocation) if allocation.keychain == keychain => allocation,
            Some(_) => {
                return Err(cdk::Error::Federation(
                    FederationError::AddressAllocationConflict {
                        quote: quote.to_string(),
                    },
                ));
            }
            None => return Ok(None),
        };
        #[cfg(feature = "federation-bdk")]
        if let Some(wallet) = self.federation.onchain_wallets().iter().find(|wallet| {
            let wallet_scope = wallet.scope();
            wallet_scope.application_id() == application_id.to_bytes()
                && wallet_scope.application_key_epoch() == application_key_epoch
        }) {
            let wallet_keychain = match keychain {
                FederationAddressKeychain::External => cdk_bdk::FederatedWalletKeychain::External,
                FederationAddressKeychain::Internal => cdk_bdk::FederatedWalletKeychain::Internal,
            };
            return wallet
                .reveal_address(wallet_keychain, allocation.derivation_index)
                .await
                .map(Some)
                .map_err(|_| {
                    cdk::Error::Custom(
                        "accepted federation address allocation could not be revealed locally"
                            .to_string(),
                    )
                });
        }
        #[cfg(feature = "federation-bark")]
        if let Some(runtime) = self.federation.bark_runtime() {
            let wallet = runtime.recovery_wallet();
            let wallet_scope = wallet.scope();
            if wallet_scope.application_id() == application_id.to_bytes()
                && wallet_scope.application_key_epoch() == application_key_epoch
            {
                let wallet_keychain = match keychain {
                    FederationAddressKeychain::External => {
                        cdk_bark::FederatedWalletKeychain::External
                    }
                    FederationAddressKeychain::Internal => {
                        cdk_bark::FederatedWalletKeychain::Internal
                    }
                };
                return wallet
                    .reveal_address(wallet_keychain, allocation.derivation_index)
                    .await
                    .map(Some)
                    .map_err(|_| {
                        cdk::Error::Custom(
                            "accepted Bark recovery allocation could not be revealed locally"
                                .to_string(),
                        )
                    });
            }
        }
        Err(cdk::Error::Custom(
            "accepted federation address allocation has no local descriptor wallet".to_string(),
        ))
    }

    #[cfg(feature = "federation-bdk")]
    pub(crate) async fn federation_onchain_wallet_statuses(
        &self,
    ) -> Vec<FederationOnchainWalletStatus> {
        let mut statuses = Vec::with_capacity(self.federation.onchain_wallets().len());
        for wallet in self.federation.onchain_wallets() {
            let status = wallet.sync_status().await;
            let observation_source = wallet.observation_source();
            let correlated_source_warning = matches!(
                observation_source.scope,
                cdk_bdk::FederatedChainSourceScope::Shared
            );
            statuses.push(FederationOnchainWalletStatus {
                source: match status.source {
                    cdk_bdk::ChainSourceKind::Esplora => "esplora",
                    cdk_bdk::ChainSourceKind::BitcoinRpc => "bitcoin_rpc",
                    cdk_bdk::ChainSourceKind::Electrum => "electrum",
                }
                .to_string(),
                source_scope: if correlated_source_warning {
                    "shared"
                } else {
                    "independent"
                }
                .to_string(),
                source_id: observation_source.source_id.clone(),
                correlated_source_warning,
                health: match status.health {
                    cdk_bdk::FederatedWalletSyncHealth::NeverSynced => "never_synced",
                    cdk_bdk::FederatedWalletSyncHealth::Healthy => "healthy",
                    cdk_bdk::FederatedWalletSyncHealth::Degraded => "degraded",
                }
                .to_string(),
                local_tip_height: status.local_tip_height,
                local_tip_hash: status.local_tip_hash,
                last_success_unix: status.last_success_unix,
                consecutive_failures: status.consecutive_failures,
            });
        }
        statuses
    }

    pub(crate) async fn federation_pending_mempool_len(&self) -> usize {
        self.federation.pending_mempool_len().await
    }

    pub(crate) async fn federation_aleph_bft_status(
        &self,
    ) -> Option<FederationConsensusRuntimeStatus> {
        self.federation.aleph_bft_status().await
    }

    pub(crate) async fn federation_aleph_bft_live_session_active(&self) -> bool {
        self.federation.live_session_active().await
    }

    pub(crate) async fn federation_aleph_bft_network_inbound_active(&self) -> bool {
        self.federation.network_inbound_active().await
    }

    pub(crate) async fn mark_aleph_bft_peer_activity(
        &self,
        session_id: u64,
        operation_id: FederationOperationId,
    ) -> Result<(), cdk::Error> {
        self.federation
            .mark_peer_activity(session_id, operation_id)
            .await
    }

    pub(crate) async fn federation_invalid_operation_alert_summaries(
        &self,
    ) -> Vec<FederationInvalidOperationAlertSummary> {
        match &self.federation.invalid_operation_alerts {
            Some(alerts) => alerts.lock().await.summaries(std::time::Instant::now()),
            None => Vec::new(),
        }
    }

    pub(crate) async fn record_federation_invalid_operation_alert(
        &self,
        source: FederationInvalidOperationSource,
    ) {
        let Some(alerts) = &self.federation.invalid_operation_alerts else {
            return;
        };
        let Some(event) = alerts
            .lock()
            .await
            .record(source, std::time::Instant::now())
        else {
            return;
        };
        tracing::warn!(
            source = event.source.as_str(),
            count = event.count,
            threshold = event.threshold,
            window_secs = event.window_secs,
            "Federation invalid operation alert threshold reached"
        );
    }

    pub(crate) fn submit_federation_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> futures::future::BoxFuture<'_, Result<FederationSubmitOperationResponse, cdk::Error>> {
        Box::pin(self.submit_federation_envelope_inner(envelope))
    }

    pub(crate) async fn submit_federation_envelope_inner(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        self.validate_federation_envelope_admission(&envelope)
            .await?;
        let operation_id = envelope.operation_id()?;
        self.ensure_federation_ready_for_public_submit().await?;

        let coordinate_wallet_publication =
            federation::wallet_operation_publisher::coordinates(envelope.operation.kind()) && {
                #[cfg(test)]
                {
                    !(self.federation.local_finality_test || self.federation.manual_finality_test)
                }
                #[cfg(not(test))]
                {
                    true
                }
            };
        if coordinate_wallet_publication {
            return Box::pin(
                self.submit_coordinated_wallet_federation_envelope(envelope, operation_id),
            )
            .await;
        }

        let response = self
            .submit_federation_envelope_to_consensus(envelope)
            .await?;
        validate_federation_submit_response(&response, operation_id)?;
        Ok(response)
    }

    #[cfg(feature = "federation-bark")]
    pub(crate) async fn submit_federation_ordered_successor_group(
        &self,
        envelopes: Vec<FederationOperationEnvelope>,
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        let Some(first) = envelopes.first() else {
            return Err(FederationError::AlephBftOrderedOperationGroupInvalid {
                reason: "ordered successor group cannot be empty".to_string(),
            }
            .into());
        };
        self.validate_federation_envelope_admission(first).await?;
        for successor in envelopes.iter().skip(1) {
            self.federation
                .validate_envelope_static_admission(successor)
                .await?;
        }
        let operation_ids = envelopes
            .iter()
            .map(FederationOperationEnvelope::operation_id)
            .collect::<Result<Vec<_>, _>>()?;
        self.ensure_federation_ready_for_public_submit().await?;
        let responses = self.federation.submit_envelope_group(envelopes).await?;
        for (response, operation_id) in responses.iter().zip(operation_ids) {
            validate_federation_submit_response(response, operation_id)?;
        }
        Ok(responses)
    }

    pub(crate) async fn submit_coordinated_wallet_federation_envelope(
        &self,
        envelope: FederationOperationEnvelope,
        operation_id: FederationOperationId,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        let config = self.mint.federation_config().ok_or(cdk::Error::Internal)?;
        let rank = federation::wallet_operation_publisher::publisher_rank(
            config.public_config().members.keys().copied(),
            operation_id,
            config.member_id(),
        )
        .ok_or(cdk::Error::Internal)?;
        if let Some(delay) = federation::wallet_operation_publisher::prior_publisher_wait(rank) {
            if let Some(response) =
                federation::wallet_operation_publisher::wait_for_prior_publisher(
                    &self.federation,
                    operation_id,
                    delay,
                )
                .await?
            {
                validate_federation_submit_response(&response, operation_id)?;
                return Ok(*response);
            }
        }

        let response = self
            .submit_federation_envelope_to_consensus(envelope)
            .await?;
        validate_federation_submit_response(&response, operation_id)?;
        Ok(response)
    }

    pub(crate) async fn submit_federation_envelope_to_consensus(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        self.validate_federation_envelope_admission(&envelope)
            .await?;
        let operation_id = envelope.operation_id()?;
        let response = self.federation.submit_envelope(envelope).await?;
        validate_federation_submit_response(&response, operation_id)?;
        Ok(response)
    }

    pub(crate) async fn validate_federation_envelope_admission(
        &self,
        envelope: &FederationOperationEnvelope,
    ) -> Result<(), cdk::Error> {
        self.federation.validate_envelope_admission(envelope).await
    }

    pub(crate) async fn submit_federation_payment_observation_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        match envelope.operation.kind() {
            FederationOperationKind::MintQuotePayment
            | FederationOperationKind::MeltQuotePayment => {}
            other => {
                return Err(cdk::Error::Custom(format!(
                    "federation payment observation expected payment operation, got {other}"
                )));
            }
        }
        let operation_id = envelope.operation_id()?;
        let response = self
            .submit_federation_envelope_to_consensus(envelope)
            .await?;
        validate_federation_submit_response(&response, operation_id)?;
        Ok(response)
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_federation_operation_acceptance(
        &self,
        operation_id: FederationOperationId,
    ) -> Option<FederationSubmitOperationResponse> {
        self.wait_for_federation_operation_acceptance_with_limit(operation_id, usize::MAX)
            .await
            .ok()
            .flatten()
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_federation_operation_acceptance_with_limit(
        &self,
        operation_id: FederationOperationId,
        waiter_limit: usize,
    ) -> Result<Option<FederationSubmitOperationResponse>, cdk::Error> {
        self.federation
            .wait_for_operation_result_with_limit(operation_id, waiter_limit)
            .await
    }

    #[cfg(test)]
    pub(crate) fn federation_pending_response_waiter_count(&self) -> usize {
        self.federation.pending_response_waiter_count()
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_federation_pending_response_waiter_count(&self, expected: usize) {
        self.federation
            .wait_for_pending_response_waiter_count(expected)
            .await;
    }

    #[cfg(test)]
    pub(crate) fn federation_response_cache_entry_count(&self) -> usize {
        self.federation.response_cache_entry_count()
    }

    pub(crate) fn federation_operation_notification_stats(
        &self,
    ) -> FederationOperationNotificationStats {
        self.federation.operation_notification_stats()
    }

    #[cfg(test)]
    pub(crate) async fn notify_federation_operation(
        &self,
        response: FederationSubmitOperationResponse,
    ) {
        self.federation.notify_operation(response);
    }

    #[cfg(test)]
    pub(crate) async fn federation_submit_responses_with_current_status(
        &self,
        responses: &[FederationSubmitOperationResponse],
    ) -> Vec<FederationSubmitOperationResponse> {
        self.federation
            .responses_with_current_status(responses)
            .await
    }

    pub(crate) async fn apply_federation_aleph_bft_finalized_unit(
        &self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        self.federation.apply_finalized_unit(payload).await
    }

    #[cfg(test)]
    pub(crate) async fn apply_federation_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        self.federation.apply_test_catch_up_response(response).await
    }

    pub(crate) async fn apply_federation_checkpoint_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
        quorum: &FederationCheckpointQuorum,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        self.federation
            .apply_checkpoint_catch_up_response(response, quorum)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn persist_federation_dkg_sequence(
        &self,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation.persist_dkg_sequence(sender, sequence).await
    }

    pub(crate) async fn persist_federation_dkg_commitment(
        &self,
        commitment: &FederationDkgParticipantCommitment,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_dkg_commitment(commitment, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_dkg_reveal(
        &self,
        reveal: &FederationDkgParticipantCommitmentReveal,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_dkg_reveal(reveal, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_frost_dkg_round1(
        &self,
        request: &cdk::federation::FederationFrostDkgRound1Request,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_frost_dkg_round1(request, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_frost_dkg_round2(
        &self,
        record: &cdk::federation::FederationFrostDkgRound2Record,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_frost_dkg_round2(record, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_frost_dkg_confirmation(
        &self,
        request: &cdk::federation::FederationFrostDkgConfirmationRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_frost_dkg_confirmation(request, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_dkg_secret_share_contribution(
        &self,
        contribution: &FederationDkgSecretShareRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_dkg_secret_share_contribution(contribution, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_dkg_transcript_signature(
        &self,
        signature: &FederationDkgTranscriptSignatureRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_dkg_transcript_signature(signature, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_dkg_activation(
        &self,
        activation: &FederationDkgActivationRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_dkg_activation(activation, sender, sequence)
            .await
    }

    pub(crate) fn ready_federation_dkg_finalized_member_config(
        &self,
    ) -> Option<Arc<FederationDkgFinalizedMemberConfig>> {
        self.federation.ready_dkg_finalized_member_config()
    }

    pub(crate) async fn remember_federation_dkg_finalized_member_config(
        &self,
        finalized: Arc<FederationDkgFinalizedMemberConfig>,
    ) -> Result<(), cdk::Error> {
        self.federation
            .remember_dkg_finalized_member_config(finalized)
            .await
    }

    pub(crate) async fn record_federation_sequence(
        &self,
        kind: FederationRestMessageKind,
        sender: MemberId,
        sequence: u64,
    ) {
        self.federation
            .record_sequence(kind, sender, sequence)
            .await;
    }

    pub(crate) async fn restore_federation_startup_readiness(&self) {
        self.federation.restore_startup_readiness().await;
    }

    pub(crate) async fn federation_sync_status(&self) -> Option<FederationSyncStatus> {
        self.federation.sync_status().await
    }

    #[cfg(test)]
    pub(crate) async fn federation_sync_transition_stats(
        &self,
    ) -> Option<FederationSyncTransitionStats> {
        let sync = self.federation.sync_handle()?;
        Some(sync.lock().await.transition_stats())
    }

    pub(crate) async fn mark_federation_lagging(
        &self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) {
        self.federation
            .mark_lagging(peer, local_operation_count, peer_operation_count)
            .await;
    }

    pub(crate) async fn mark_federation_awaiting_quorum(&self, operation_count: u64) {
        self.federation.mark_awaiting_quorum(operation_count).await;
    }

    pub(crate) async fn mark_federation_catching_up(&self, peer: MemberId) {
        self.federation.mark_catching_up(peer).await;
    }

    pub(crate) async fn mark_federation_healthy(&self) {
        self.federation.mark_healthy().await;
    }

    pub(crate) async fn mark_federation_halted<S>(&self, reason: S)
    where
        S: Into<String>,
    {
        self.federation.mark_halted(reason.into()).await;
    }

    pub(crate) async fn ensure_federation_ready_for_public_submit(&self) -> Result<(), cdk::Error> {
        self.ensure_federation_ready_for_authoritative_read().await
    }

    pub(crate) async fn ensure_federation_ready_for_authoritative_read(
        &self,
    ) -> Result<(), cdk::Error> {
        self.federation.ensure_ready_for_authoritative_read().await
    }

    pub(crate) async fn audit_trusted_federation_checkpoint(
        &self,
    ) -> Result<cdk::federation::FederationCheckpointAuditResponse, cdk::Error> {
        let audit = self
            .federation
            .audit_trusted_checkpoint_materialized_state()
            .await;
        if matches!(
            &audit,
            Err(cdk::Error::Federation(
                cdk::federation::FederationError::MaterializedStateAuditMismatch { .. }
            ))
        ) {
            federation::halt_current_federation_aleph_bft_session(
                self,
                "failed an operator-requested trusted-checkpoint materialized-state audit",
            )
            .await;
        }
        audit
    }
}

pub(crate) fn accepted_federation_journal_entries(
    entries: &[FederationJournalEntry],
) -> Vec<FederationJournalEntry> {
    entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.status(),
                FederationOperationStatus::Accepted | FederationOperationStatus::Applying
            )
        })
        .cloned()
        .collect()
}

#[cfg(all(feature = "federation", test))]
pub(crate) fn should_overlay_local_status_after_aleph_bft_recovery(
    status: FederationOperationStatus,
) -> bool {
    matches!(
        status,
        FederationOperationStatus::Applied | FederationOperationStatus::Rejected
    )
}

#[cfg(test)]
pub(crate) fn stale_local_aleph_bft_entries_to_retry(
    local_entries: &[FederationJournalEntry],
    recovered_entries: &[FederationJournalEntry],
) -> Vec<FederationJournalEntry> {
    local_entries
        .iter()
        .zip(recovered_entries.iter())
        .filter_map(|(local_entry, recovered_entry)| {
            let finalized_entry_is_accepted = matches!(
                recovered_entry.status(),
                FederationOperationStatus::Accepted | FederationOperationStatus::Applying
            );
            let local_entry_needs_retry = matches!(
                local_entry.status(),
                FederationOperationStatus::Accepted
                    | FederationOperationStatus::Applying
                    | FederationOperationStatus::Failed
            );
            (finalized_entry_is_accepted && local_entry_needs_retry)
                .then(|| recovered_entry.clone())
        })
        .collect()
}

pub(crate) fn is_retryable_federation_apply_error(err: &cdk::Error) -> bool {
    match err {
        cdk::Error::ConcurrentUpdate => true,
        cdk::Error::Database(err) => is_retryable_federation_database_error(err),
        _ => false,
    }
}

pub(crate) fn is_retryable_federation_database_error(err: &cdk::cdk_database::Error) -> bool {
    matches!(
        err,
        cdk::cdk_database::Error::ConcurrentUpdate | cdk::cdk_database::Error::Locked
    )
}

pub(crate) fn federation_submit_response_with_entry(
    response: &FederationSubmitOperationResponse,
    current_entry: FederationJournalEntry,
) -> FederationSubmitOperationResponse {
    let mut response = response.clone();
    match &mut response.outcome {
        FederationJournalAppendOutcome::Appended(entry)
        | FederationJournalAppendOutcome::Duplicate(entry) => {
            *entry = current_entry;
        }
    }
    response
}

pub(crate) async fn federation_invalid_operation_alert_middleware(
    axum::extract::State(state): axum::extract::State<MintState>,
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let monitor = federation_invalid_operation_alert_path(req.method(), req.uri().path());
    let source = monitor.then(|| FederationInvalidOperationSource::from_request(&req));
    let response = next.run(req).await;
    if monitor && federation_invalid_operation_alert_status(response.status()) {
        if let Some(source) = source {
            state
                .record_federation_invalid_operation_alert(source)
                .await;
        }
    }

    response
}

pub(crate) fn federation_invalid_operation_alert_path(
    method: &axum::http::Method,
    path: &str,
) -> bool {
    if *method != axum::http::Method::POST {
        return false;
    }

    path == "/v1/swap"
        || path == "/v1/restore"
        || path == "/v1/checkstate"
        || path == "/v1/auth/blind/mint"
        || path == "/federation/v1/operations"
        || path.starts_with("/v1/mint/")
        || path.starts_with("/v1/melt/")
}

pub(crate) fn federation_invalid_operation_alert_status(status: axum::http::StatusCode) -> bool {
    status.is_client_error()
        && status != axum::http::StatusCode::NOT_FOUND
        && status != axum::http::StatusCode::METHOD_NOT_ALLOWED
}
