//! Thin proposer and FROST coordinator failover for the joint on-chain wallet.
//!
//! This module owns only member-local scheduling. Accepted intents, UTXOs,
//! transaction policy, reservations, signing authorization, and finality remain
//! authoritative in the existing consensus projections.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cdk::federation::{
    FederationAcceptedTransaction, FederationAcceptedTransactionState,
    FederationAddressAllocationScope, FederationAddressKeychain, FederationOnchainBatchId,
    FederationOnchainPaymentBatch, FederationOnchainPaymentIntentState,
    FederationTransactionChangeOutput, FederationTransactionInput, FederationTransactionProposal,
    FederationTransactionProposalId, FederationTransactionRecipientOutput,
    FederationTransactionScope, FederationWalletLivenessActivity, FederationWalletLivenessSchedule,
    FederationWalletLivenessStatus, FederationWalletUtxoState,
    FEDERATION_MAX_ONCHAIN_BATCH_INTENTS, FEDERATION_MAX_ONCHAIN_PROPOSAL_INPUTS,
};
use cdk_common::bitcoin::{Amount, FeeRate};
use cdk_http_client::Transport;

use crate::MintState;

use super::frost_signing::FederationFrostSigningCoordinator;
use super::peer_client::FederationRestPeerClient;

const WALLET_LIVENESS_ROUND_INTERVAL: Duration = Duration::from_secs(5);
const PROPOSER_LEASE: Duration = Duration::from_secs(15);
const SIGNING_ATTEMPT_LEASE: Duration =
    Duration::from_millis(cdk::federation::FEDERATION_WALLET_SIGNING_ATTEMPT_LEASE_MILLIS);
const SIGNING_AGGREGATOR_GRACE: Duration = Duration::from_secs(10);
const RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy)]
pub(crate) struct FederationWalletLivenessConfig {
    round_interval: Duration,
    proposer_lease: Duration,
    signing_attempt_lease: Duration,
    signing_aggregator_grace: Duration,
    retry_delay: Duration,
    max_retry_delay: Duration,
}

impl Default for FederationWalletLivenessConfig {
    fn default() -> Self {
        Self {
            round_interval: WALLET_LIVENESS_ROUND_INTERVAL,
            proposer_lease: PROPOSER_LEASE,
            signing_attempt_lease: SIGNING_ATTEMPT_LEASE,
            signing_aggregator_grace: SIGNING_AGGREGATOR_GRACE,
            retry_delay: RETRY_DELAY,
            max_retry_delay: MAX_RETRY_DELAY,
        }
    }
}

impl FederationWalletLivenessConfig {
    fn validate(self) -> Result<Self, cdk::Error> {
        if self.round_interval.is_zero()
            || self.proposer_lease.is_zero()
            || self.signing_attempt_lease.is_zero()
            || self.signing_attempt_lease != SIGNING_ATTEMPT_LEASE
            || self.retry_delay.is_zero()
            || self.max_retry_delay < self.retry_delay
            || self.signing_aggregator_grace >= self.signing_attempt_lease
            || self.max_retry_delay > self.signing_attempt_lease
        {
            return Err(cdk::Error::Custom(
                "federation wallet liveness timing configuration is invalid".to_string(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LivenessWorkId {
    Proposal(FederationOnchainBatchId),
    Replacement(FederationTransactionProposalId),
    Signing(FederationTransactionProposalId),
}

#[derive(Debug, Clone, Copy)]
struct RetryState {
    failures: u32,
    not_before: Instant,
}

#[derive(Debug)]
struct FederationWalletLivenessInner {
    config: FederationWalletLivenessConfig,
    retries: tokio::sync::Mutex<BTreeMap<LivenessWorkId, RetryState>>,
    status: tokio::sync::RwLock<FederationWalletLivenessStatus>,
    round: tokio::sync::Mutex<()>,
}

/// Replaceable member-local scheduler; all safety decisions remain in consensus.
#[derive(Debug, Clone)]
pub(crate) struct FederationWalletLivenessWorker {
    inner: Arc<FederationWalletLivenessInner>,
}

impl Default for FederationWalletLivenessWorker {
    fn default() -> Self {
        Self::new(FederationWalletLivenessConfig::default())
            .expect("default federation wallet liveness configuration")
    }
}

impl FederationWalletLivenessWorker {
    fn new(config: FederationWalletLivenessConfig) -> Result<Self, cdk::Error> {
        Ok(Self {
            inner: Arc::new(FederationWalletLivenessInner {
                config: config.validate()?,
                retries: tokio::sync::Mutex::new(BTreeMap::new()),
                status: tokio::sync::RwLock::new(FederationWalletLivenessStatus::default()),
                round: tokio::sync::Mutex::new(()),
            }),
        })
    }

    pub(crate) async fn status(&self) -> FederationWalletLivenessStatus {
        self.inner.status.read().await.clone()
    }

    async fn run_round<T>(
        &self,
        state: &MintState,
        client: &FederationRestPeerClient<T>,
        now_epoch_millis: u128,
    ) -> Result<usize, cdk::Error>
    where
        T: Transport,
    {
        let Ok(_round) = self.inner.round.try_lock() else {
            return Ok(0);
        };
        let Some(runtime) = state.mint.federation_config() else {
            return Ok(0);
        };
        if runtime.public_config().frost_wallet.is_none() {
            return Ok(0);
        }
        if state.federation.onchain_wallets().is_empty()
            || state.federation.frost_signing().is_none()
        {
            return Ok(0);
        }
        state
            .ensure_federation_ready_for_authoritative_read()
            .await?;

        let transaction_state = state.mint.federation_transaction_state().await?;
        let chain_state = state.mint.federation_chain_observation_state().await?;
        let application = runtime.cdk_onchain_application()?;
        let scope = FederationTransactionScope {
            root_key_epoch: application.root_key_epoch,
            application_id: application.application_id,
            application_key_epoch: application.application_key_epoch,
            network: application.network,
            policy_version: application.fee_policy_version,
        };
        let pending = transaction_state
            .intents()
            .values()
            .filter(|record| {
                record.state == FederationOnchainPaymentIntentState::Pending
                    && record.intent.scope == scope
            })
            .map(|record| record.intent.clone())
            .collect::<Vec<_>>();
        let awaiting = transaction_state
            .accepted_transactions()
            .values()
            .filter(|accepted| {
                accepted.state == FederationAcceptedTransactionState::Accepted
                    && accepted.signed.is_none()
            })
            .count();
        {
            let mut status = self.inner.status.write().await;
            status.pending_batches = u64::from(!pending.is_empty());
            status.awaiting_signatures = awaiting.try_into().unwrap_or(u64::MAX);
            status.activity = observed_activity(!pending.is_empty(), awaiting);
        }

        let mut advanced = 0_usize;
        let mut active_work = BTreeSet::new();
        if !pending.is_empty() {
            let cutoff = pending
                .iter()
                .map(|intent| intent.creation_time)
                .max()
                .ok_or_else(|| cdk::Error::Custom("pending batch has no cutoff".to_string()))?;
            let batch = FederationOnchainPaymentBatch::select(
                pending,
                cutoff,
                FEDERATION_MAX_ONCHAIN_BATCH_INTENTS,
            )?;
            active_work.insert(LivenessWorkId::Proposal(batch.batch_id));
            let attempt = lease_attempt(now_epoch_millis, self.inner.config.proposer_lease);
            let schedule = FederationWalletLivenessSchedule::new(
                batch.batch_id.to_bytes(),
                runtime.public_config().members.keys().copied(),
                application.threshold,
            )?;
            if schedule.proposer(attempt)? == runtime.member_id()
                && self
                    .retry_is_due(LivenessWorkId::Proposal(batch.batch_id))
                    .await
            {
                self.set_activity(FederationWalletLivenessActivity::Proposing)
                    .await;
                self.increment_proposal_attempts().await;
                match self
                    .propose_transaction(state, runtime, &chain_state, &transaction_state, &batch)
                    .await
                {
                    Ok(()) => {
                        self.clear_retry(LivenessWorkId::Proposal(batch.batch_id))
                            .await;
                        advanced = advanced.saturating_add(1);
                    }
                    Err(error) => {
                        self.defer(LivenessWorkId::Proposal(batch.batch_id)).await;
                        self.set_activity(FederationWalletLivenessActivity::Backoff)
                            .await;
                        tracing::warn!(
                            "Federation wallet proposer deferred a fail-closed attempt: {}",
                            cdk::federation::redact_federation_operator_message(&error.to_string())
                        );
                    }
                }
            }
        }

        for accepted in transaction_state.accepted_transactions().values() {
            let original_id = accepted.proposal.proposal_id()?;
            if transaction_state.lineage_head(accepted.proposal.batch_id) != Some(original_id)
                || !replacement_is_due(accepted)?
            {
                continue;
            }
            let work = LivenessWorkId::Replacement(original_id);
            active_work.insert(work);
            let policy = cdk::mint::FederationOnchainTransactionPolicy::for_version(
                accepted.proposal.scope.policy_version,
            )?;
            let attempt = lease_attempt(now_epoch_millis, self.inner.config.proposer_lease);
            let schedule = FederationWalletLivenessSchedule::new(
                original_id.to_bytes(),
                runtime.public_config().members.keys().copied(),
                application.threshold,
            )?;
            if schedule.proposer(attempt)? != runtime.member_id() || !self.retry_is_due(work).await
            {
                continue;
            }
            self.set_activity(FederationWalletLivenessActivity::Proposing)
                .await;
            self.increment_proposal_attempts().await;
            match self
                .propose_replacement(
                    state,
                    runtime,
                    &chain_state,
                    &transaction_state,
                    accepted,
                    policy,
                )
                .await
            {
                Ok(()) => {
                    self.clear_retry(work).await;
                    advanced = advanced.saturating_add(1);
                }
                Err(error) => {
                    self.defer(work).await;
                    self.set_activity(FederationWalletLivenessActivity::Backoff)
                        .await;
                    tracing::warn!(
                        proposal_id = %original_id,
                        "Federation wallet replacement deferred safely: {}",
                        cdk::federation::redact_federation_operator_message(&error.to_string())
                    );
                }
            }
        }

        for accepted in transaction_state.accepted_transactions().values() {
            if accepted.state != FederationAcceptedTransactionState::Accepted
                || accepted.signed.is_some()
            {
                continue;
            }
            let proposal_id = accepted.proposal.proposal_id()?;
            active_work.insert(LivenessWorkId::Signing(proposal_id));
            if application.root_key_epoch != accepted.proposal.scope.root_key_epoch
                || application.application_id != accepted.proposal.scope.application_id
                || application.application_key_epoch
                    != accepted.proposal.scope.application_key_epoch
            {
                continue;
            }
            let attempt = cdk::federation::federation_wallet_signing_attempt_at(now_epoch_millis);
            let schedule = FederationWalletLivenessSchedule::new(
                proposal_id.to_bytes(),
                runtime.public_config().members.keys().copied(),
                application.threshold,
            )?;
            let plan = schedule.signing_plan(attempt)?;
            let elapsed = lease_elapsed(now_epoch_millis, self.inner.config.signing_attempt_lease);
            if runtime.member_id() != plan.coordinator
                && elapsed < self.inner.config.signing_aggregator_grace
            {
                continue;
            }
            let work = LivenessWorkId::Signing(proposal_id);
            if !self.retry_is_due(work).await {
                continue;
            }
            self.set_activity(FederationWalletLivenessActivity::Signing)
                .await;
            self.increment_signing_attempts().await;
            match self
                .sign_transaction(state, client, accepted, attempt, plan.signer_set)
                .await
            {
                Ok(()) => {
                    self.clear_retry(work).await;
                    advanced = advanced.saturating_add(1);
                }
                Err(error) => {
                    self.defer(work).await;
                    let mut status = self.inner.status.write().await;
                    status.signer_failures = status.signer_failures.saturating_add(1);
                    status.activity = FederationWalletLivenessActivity::Backoff;
                    drop(status);
                    tracing::warn!(
                        proposal_id = %proposal_id,
                        attempt,
                        "Federation FROST coordinator deferred an unavailable attempt: {}",
                        cdk::federation::redact_federation_operator_message(&error.to_string())
                    );
                }
            }
        }
        self.inner
            .retries
            .lock()
            .await
            .retain(|work, _| active_work.contains(work));
        Ok(advanced)
    }

    async fn propose_transaction(
        &self,
        state: &MintState,
        runtime: &cdk::mint::FederationRuntimeConfig,
        chain_state: &cdk::federation::FederationChainObservationState,
        transaction_state: &cdk::federation::FederationTransactionState,
        batch: &FederationOnchainPaymentBatch,
    ) -> Result<(), cdk::Error> {
        let wallet = wallet_for_scope(state, batch.scope).ok_or_else(|| {
            cdk::Error::Custom("proposal scope has no local watch-only wallet".to_string())
        })?;
        wallet.ensure_ready(0).await.map_err(|error| {
            cdk::Error::Custom(format!("federated BDK wallet is not ready: {error}"))
        })?;
        let mut spendable = chain_state
            .utxos()
            .filter(|utxo| {
                utxo.state == FederationWalletUtxoState::Spendable
                    && !utxo.liability_alarm
                    && utxo.allocation.application_id == batch.scope.application_id
                    && utxo.allocation.application_key_epoch == batch.scope.application_key_epoch
            })
            .take(FEDERATION_MAX_ONCHAIN_PROPOSAL_INPUTS)
            .collect::<Vec<_>>();
        spendable.sort_by_key(|utxo| utxo.outpoint);
        if spendable.is_empty() {
            return Err(cdk::Error::Custom(
                "no consensus-spendable federation wallet UTXO is available".to_string(),
            ));
        }

        let change_key = change_allocation_key(batch.batch_id);
        state
            .allocate_federation_onchain_address(
                &change_key,
                batch.scope.application_id,
                batch.scope.application_key_epoch,
                FederationAddressKeychain::Internal,
            )
            .await?;
        let allocation_scope = FederationAddressAllocationScope {
            application_id: batch.scope.application_id,
            application_key_epoch: batch.scope.application_key_epoch,
        };
        let allocation = state
            .mint
            .federation_address_allocation(allocation_scope, &change_key)
            .await?
            .ok_or_else(|| {
                cdk::Error::Custom(
                    "accepted transaction change allocation is unavailable".to_string(),
                )
            })?;
        let change_script = wallet
            .peek_federated_script(
                cdk_bdk::FederatedWalletKeychain::Internal,
                allocation.derivation_index,
            )
            .await;
        let recipients = batch
            .intents
            .iter()
            .map(|intent| (intent.destination_script.clone(), intent.amount_sat))
            .collect::<Vec<_>>();
        let outpoints = spendable
            .iter()
            .map(|utxo| utxo.outpoint)
            .collect::<Vec<_>>();
        let policy =
            cdk::mint::FederationOnchainTransactionPolicy::for_version(batch.scope.policy_version)?;
        let target_blocks = batch
            .intents
            .iter()
            .map(|intent| intent.fee_target_blocks)
            .min()
            .ok_or_else(|| cdk::Error::Custom("transaction batch has no fee target".to_string()))?;
        let target_blocks = u16::try_from(target_blocks)
            .map_err(|_| cdk::Error::Custom("fee target exceeds u16".to_string()))?;
        let fee_rate_sat_per_vb =
            bounded_fee_rate(policy, wallet.fetch_fee_rate_sat_per_vb(target_blocks)).await;
        let fee_rate = FeeRate::from_sat_per_vb(fee_rate_sat_per_vb)
            .ok_or_else(|| cdk::Error::Custom("invalid first-release fee rate".to_string()))?;
        let psbt = wallet
            .engine()
            .build_federated_psbt(&recipients, fee_rate, change_script.clone(), &outpoints)
            .await
            .map_err(|error| cdk::Error::Custom(error.to_string()))?;
        let by_outpoint = spendable
            .iter()
            .map(|utxo| (utxo.outpoint, *utxo))
            .collect::<BTreeMap<_, _>>();
        let selected_inputs = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|input| {
                let utxo = by_outpoint.get(&input.previous_output).ok_or_else(|| {
                    cdk::Error::Custom(
                        "BDK selected an input outside accepted consensus state".to_string(),
                    )
                })?;
                Ok(FederationTransactionInput {
                    outpoint: utxo.outpoint,
                    previous_output: utxo.txout.clone(),
                    allocation: utxo.allocation.clone(),
                })
            })
            .collect::<Result<Vec<_>, cdk::Error>>()?;
        let input_sum = selected_inputs.iter().try_fold(0_u64, |sum, input| {
            sum.checked_add(input.previous_output.value.to_sat())
                .ok_or_else(|| cdk::Error::Custom("transaction input sum overflow".to_string()))
        })?;
        let output_sum = psbt
            .unsigned_tx
            .output
            .iter()
            .try_fold(0_u64, |sum, output| {
                sum.checked_add(output.value.to_sat()).ok_or_else(|| {
                    cdk::Error::Custom("transaction output sum overflow".to_string())
                })
            })?;
        let fee_sat = input_sum.checked_sub(output_sum).ok_or_else(|| {
            cdk::Error::Custom("transaction outputs exceed selected inputs".to_string())
        })?;
        let contributions = fee_contributions(batch, fee_sat)?;
        let mut assigned_outputs = BTreeSet::new();
        let mut recipient_outputs = Vec::with_capacity(batch.intents.len());
        for (intent, contribution) in batch.intents.iter().zip(contributions) {
            let intent_id = intent.intent_id()?;
            let (index, output) = psbt
                .unsigned_tx
                .output
                .iter()
                .enumerate()
                .find(|(index, output)| {
                    !assigned_outputs.contains(index)
                        && output.script_pubkey == intent.destination_script
                        && output.value == Amount::from_sat(intent.amount_sat)
                })
                .ok_or_else(|| {
                    cdk::Error::Custom(
                        "BDK transaction is missing an authorized recipient".to_string(),
                    )
                })?;
            assigned_outputs.insert(index);
            recipient_outputs.push(FederationTransactionRecipientOutput {
                intent_id,
                output_index: u32::try_from(index).map_err(|_| {
                    cdk::Error::Custom("recipient output index exceeds u32".to_string())
                })?,
                output: output.clone(),
                fee_contribution_sat: contribution,
            });
        }
        let remaining = psbt
            .unsigned_tx
            .output
            .iter()
            .enumerate()
            .filter(|(index, _)| !assigned_outputs.contains(index))
            .collect::<Vec<_>>();
        let change_output = match remaining.as_slice() {
            [] => None,
            [(index, output)] if output.script_pubkey == change_script => {
                Some(FederationTransactionChangeOutput {
                    derivation_index: allocation.derivation_index,
                    output_index: u32::try_from(*index).map_err(|_| {
                        cdk::Error::Custom("change output index exceeds u32".to_string())
                    })?,
                    output: (*output).clone(),
                })
            }
            _ => {
                return Err(cdk::Error::Custom(
                    "BDK transaction has an unallocated output".to_string(),
                ));
            }
        };
        let proposal = FederationTransactionProposal::new(
            batch,
            runtime.member_id(),
            psbt.unsigned_tx,
            selected_inputs,
            recipient_outputs,
            change_output,
            fee_sat,
            None,
        )?;
        runtime.validate_onchain_transaction_proposal(
            chain_state,
            transaction_state.intents(),
            &proposal,
        )?;
        let envelope = runtime.transaction_proposal_operation(proposal)?;
        state
            .submit_federation_envelope(envelope)
            .await?
            .ensure_accepted()?;
        Ok(())
    }

    async fn sign_transaction<T>(
        &self,
        state: &MintState,
        client: &FederationRestPeerClient<T>,
        accepted: &FederationAcceptedTransaction,
        attempt: u32,
        signer_set: Vec<cdk::federation::MemberId>,
    ) -> anyhow::Result<()>
    where
        T: Transport,
    {
        let runtime = state
            .mint
            .federation_config()
            .ok_or_else(|| anyhow::anyhow!("federation configuration is unavailable"))?;
        let wallet_config = runtime
            .public_config()
            .frost_wallet
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("FROST wallet configuration is unavailable"))?;
        let application = runtime.cdk_onchain_application()?;
        if application.root_key_epoch != accepted.proposal.scope.root_key_epoch
            || application.application_id != accepted.proposal.scope.application_id
            || application.application_key_epoch != accepted.proposal.scope.application_key_epoch
        {
            anyhow::bail!("accepted FROST application is unavailable");
        }
        let root = wallet_config
            .root(application.root_key_epoch)
            .ok_or_else(|| anyhow::anyhow!("accepted FROST root epoch is unavailable"))?;
        let public = application.derived_public_package(runtime.public_config(), root)?;
        let wallet = wallet_for_scope(state, accepted.proposal.scope)
            .ok_or_else(|| anyhow::anyhow!("accepted transaction has no local BDK wallet"))?;
        let signer_set = signer_set
            .into_iter()
            .map(|member| cdk_frost::MemberId::new(member.get()))
            .collect::<Result<Vec<_>, _>>()?;
        FederationFrostSigningCoordinator::new(client, public)
            .sign_accepted_transaction(
                state,
                wallet,
                accepted.proposal.proposal_id()?,
                attempt,
                signer_set,
            )
            .await?;
        Ok(())
    }

    async fn propose_replacement(
        &self,
        state: &MintState,
        runtime: &cdk::mint::FederationRuntimeConfig,
        chain_state: &cdk::federation::FederationChainObservationState,
        transaction_state: &cdk::federation::FederationTransactionState,
        original: &FederationAcceptedTransaction,
        policy: cdk::mint::FederationOnchainTransactionPolicy,
    ) -> Result<(), cdk::Error> {
        let original_id = original.proposal.proposal_id()?;
        let wallet = wallet_for_scope(state, original.proposal.scope).ok_or_else(|| {
            cdk::Error::Custom("replacement scope has no local watch-only wallet".to_string())
        })?;
        wallet.ensure_ready(0).await.map_err(|error| {
            cdk::Error::Custom(format!("federated BDK wallet is not ready: {error}"))
        })?;
        let intents = original
            .proposal
            .intent_ids
            .iter()
            .map(|intent_id| {
                transaction_state
                    .intents()
                    .get(intent_id)
                    .map(|record| record.intent.clone())
                    .ok_or_else(|| {
                        cdk::Error::Custom(
                            "replacement lineage references a missing intent".to_string(),
                        )
                    })
            })
            .collect::<Result<Vec<_>, cdk::Error>>()?;
        let batch =
            FederationOnchainPaymentBatch::new(intents, original.proposal.eligibility_cutoff)?;
        if batch.batch_id != original.proposal.batch_id {
            return Err(cdk::Error::Custom(
                "replacement batch differs from its accepted lineage".to_string(),
            ));
        }
        let target_blocks = batch
            .intents
            .iter()
            .map(|intent| intent.fee_target_blocks)
            .min()
            .ok_or_else(|| cdk::Error::Custom("replacement has no fee target".to_string()))?;
        let target_blocks = u16::try_from(target_blocks)
            .map_err(|_| cdk::Error::Custom("fee target exceeds u16".to_string()))?;
        let estimated_sat_per_vb =
            bounded_fee_rate(policy, wallet.fetch_fee_rate_sat_per_vb(target_blocks)).await;
        let maximum_fee = batch.intents.iter().try_fold(0_u64, |sum, intent| {
            sum.checked_add(intent.maximum_fee_sat)
                .ok_or_else(|| cdk::Error::Custom("replacement fee cap overflow".to_string()))
        })?;
        let fee_sat = replacement_fee_sat(
            policy,
            estimated_sat_per_vb,
            original.proposal.fee_sat,
            original.proposal.estimated_signed_weight(),
            maximum_fee,
        )?;
        let extra_fee = fee_sat
            .checked_sub(original.proposal.fee_sat)
            .ok_or_else(|| cdk::Error::Custom("replacement fee did not increase".to_string()))?;
        let original_change = original.proposal.change_output.as_ref().ok_or_else(|| {
            cdk::Error::Custom(
                "first-release RBF requires an accepted change output; CPFP is deferred"
                    .to_string(),
            )
        })?;
        let mut unsigned_transaction = original.proposal.unsigned_transaction.clone();
        let change_index = usize::try_from(original_change.output_index)
            .map_err(|_| cdk::Error::Custom("replacement change index overflow".to_string()))?;
        let change = unsigned_transaction
            .output
            .get_mut(change_index)
            .ok_or_else(|| {
                cdk::Error::Custom("replacement change output is missing".to_string())
            })?;
        change.value = change
            .value
            .checked_sub(Amount::from_sat(extra_fee))
            .ok_or_else(|| {
                cdk::Error::Custom("replacement would consume all change".to_string())
            })?;
        if change.value
            < cdk_common::bitcoin::TxOut::minimal_non_dust(change.script_pubkey.clone()).value
        {
            return Err(cdk::Error::Custom(
                "accepted fee reserves leave no non-dust replacement change".to_string(),
            ));
        }
        let contributions = fee_contributions(&batch, fee_sat)?;
        let mut recipient_outputs = original.proposal.recipient_outputs.clone();
        for (recipient, contribution) in recipient_outputs.iter_mut().zip(contributions) {
            recipient.fee_contribution_sat = contribution;
        }
        let mut change_output = original_change.clone();
        change_output.output = change.clone();
        let replacement = FederationTransactionProposal::new(
            &batch,
            runtime.member_id(),
            unsigned_transaction,
            original.proposal.selected_inputs.clone(),
            recipient_outputs,
            Some(change_output),
            fee_sat,
            Some(original_id),
        )?;
        runtime.validate_onchain_transaction_replacement(
            chain_state,
            transaction_state.intents(),
            original,
            &replacement,
        )?;
        let envelope = runtime.transaction_proposal_operation(replacement)?;
        state
            .submit_federation_envelope(envelope)
            .await?
            .ensure_accepted()?;
        Ok(())
    }

    async fn retry_is_due(&self, work: LivenessWorkId) -> bool {
        self.inner
            .retries
            .lock()
            .await
            .get(&work)
            .is_none_or(|retry| Instant::now() >= retry.not_before)
    }

    async fn defer(&self, work: LivenessWorkId) {
        let mut retries = self.inner.retries.lock().await;
        let failures = retries
            .get(&work)
            .map_or(1, |retry| retry.failures.saturating_add(1));
        let exponent = failures.saturating_sub(1).min(31);
        let multiplier = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
        let delay = self
            .inner
            .config
            .retry_delay
            .saturating_mul(multiplier)
            .min(self.inner.config.max_retry_delay);
        retries.insert(
            work,
            RetryState {
                failures,
                not_before: Instant::now() + delay,
            },
        );
    }

    async fn clear_retry(&self, work: LivenessWorkId) {
        self.inner.retries.lock().await.remove(&work);
    }

    async fn set_activity(&self, activity: FederationWalletLivenessActivity) {
        self.inner.status.write().await.activity = activity;
    }

    async fn increment_proposal_attempts(&self) {
        let mut status = self.inner.status.write().await;
        status.proposal_attempts = status.proposal_attempts.saturating_add(1);
    }

    async fn increment_signing_attempts(&self) {
        let mut status = self.inner.status.write().await;
        status.signing_attempts = status.signing_attempts.saturating_add(1);
    }

    pub(crate) fn spawn(self, state: MintState) {
        let Some(runtime) = state.mint.federation_config() else {
            return;
        };
        if runtime.public_config().frost_wallet.is_none()
            || state.federation.onchain_wallets().is_empty()
            || state.federation.frost_signing().is_none()
        {
            return;
        }
        let public_config = runtime.public_config().clone();
        let private_config = runtime.private_config().clone();
        let sync_config = state.federation.peer_sync_config.unwrap_or_default();
        tokio::spawn(async move {
            let client = match FederationRestPeerClient::new_with_validated_config(
                state.federation.transport.clone(),
                &public_config,
                private_config,
                sync_config,
            ) {
                Ok(client) => client,
                Err(error) => {
                    tracing::error!(
                        "Could not start federation wallet liveness worker: {}",
                        cdk::federation::redact_federation_operator_message(&error.to_string())
                    );
                    return;
                }
            };
            let mut interval = tokio::time::interval(self.inner.config.round_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                match self.run_round(&state, &client, epoch_millis()).await {
                    Ok(advanced) if advanced > 0 => tracing::debug!(
                        advanced,
                        "Federation wallet liveness worker advanced durable state"
                    ),
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        "Federation wallet liveness round failed closed: {}",
                        cdk::federation::redact_federation_operator_message(&error.to_string())
                    ),
                }
            }
        });
    }
}

fn wallet_for_scope(
    state: &MintState,
    scope: FederationTransactionScope,
) -> Option<&cdk_bdk::FederatedBdkWallet> {
    state.federation.onchain_wallets().iter().find(|wallet| {
        let wallet_scope = wallet.scope();
        wallet_scope.root_key_epoch() == scope.root_key_epoch
            && wallet_scope.application_id() == scope.application_id.to_bytes()
            && wallet_scope.application_key_epoch() == scope.application_key_epoch
    })
}

fn fee_contributions(
    batch: &FederationOnchainPaymentBatch,
    fee_sat: u64,
) -> Result<Vec<u64>, cdk::Error> {
    let mut remaining = fee_sat;
    let contributions = batch
        .intents
        .iter()
        .map(|intent| {
            let contribution = remaining.min(intent.maximum_fee_sat);
            remaining -= contribution;
            contribution
        })
        .collect::<Vec<_>>();
    if remaining != 0 {
        return Err(cdk::Error::Custom(
            "transaction fee exceeds the accepted intent reserves".to_string(),
        ));
    }
    Ok(contributions)
}

fn replacement_is_due(accepted: &FederationAcceptedTransaction) -> Result<bool, cdk::Error> {
    if !matches!(
        accepted.state,
        FederationAcceptedTransactionState::BroadcastPending
            | FederationAcceptedTransactionState::Broadcast
    ) || accepted.signed.is_none()
    {
        return Ok(false);
    }
    let policy = cdk::mint::FederationOnchainTransactionPolicy::for_version(
        accepted.proposal.scope.policy_version,
    )?;
    let Some(broadcast) = accepted.broadcast.as_ref() else {
        return Ok(false);
    };
    Ok(broadcast.observations.iter().any(|record| {
        record.observation.result == cdk::federation::FederationTransactionBroadcastResult::Rejected
            || record.observation.attempt >= policy.replacement_after_broadcast_attempts
    }))
}

fn replacement_fee_sat(
    policy: cdk::mint::FederationOnchainTransactionPolicy,
    estimated_sat_per_vb: u64,
    original_fee_sat: u64,
    weight: u64,
    maximum_intent_fee_sat: u64,
) -> Result<u64, cdk::Error> {
    let estimated_fee = estimated_sat_per_vb
        .checked_mul(weight)
        .and_then(|scaled| scaled.checked_add(3))
        .map(|scaled| scaled / 4)
        .ok_or_else(|| cdk::Error::Custom("replacement fee estimate overflow".to_string()))?;
    let minimum_fee = original_fee_sat
        .checked_add(policy.minimum_replacement_fee_delta(weight)?)
        .ok_or_else(|| cdk::Error::Custom("replacement fee overflow".to_string()))?;
    let replacement_fee = estimated_fee.max(minimum_fee);
    if replacement_fee > maximum_intent_fee_sat || replacement_fee > policy.maximum_absolute_fee_sat
    {
        return Err(cdk::Error::Custom(
            "accepted fee reserves cannot fund a valid replacement".to_string(),
        ));
    }
    Ok(replacement_fee)
}

async fn bounded_fee_rate<E, F>(
    policy: cdk::mint::FederationOnchainTransactionPolicy,
    estimate: F,
) -> u64
where
    F: std::future::Future<Output = Result<f64, E>>,
{
    let estimate = tokio::time::timeout(
        Duration::from_millis(policy.fee_estimation_timeout_millis),
        estimate,
    )
    .await
    .ok()
    .and_then(Result::ok);
    bounded_fee_rate_value(policy, estimate)
}

fn bounded_fee_rate_value(
    policy: cdk::mint::FederationOnchainTransactionPolicy,
    estimate_sat_per_vb: Option<f64>,
) -> u64 {
    let minimum = policy.minimum_fee_rate_sat_per_kwu.saturating_add(249) / 250;
    let maximum = policy.maximum_fee_rate_sat_per_kwu / 250;
    let estimated = estimate_sat_per_vb
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.ceil() as u64)
        .unwrap_or(policy.fallback_fee_rate_sat_per_vb);
    estimated.clamp(minimum.max(1), maximum.max(minimum).max(1))
}

fn change_allocation_key(batch_id: FederationOnchainBatchId) -> String {
    format!("wallet-change-{batch_id}")
}

fn lease_attempt(now_epoch_millis: u128, lease: Duration) -> u32 {
    let lease_millis = lease.as_millis().max(1);
    let attempt = now_epoch_millis / lease_millis + 1;
    u32::try_from(attempt).unwrap_or(u32::MAX)
}

fn lease_elapsed(now_epoch_millis: u128, lease: Duration) -> Duration {
    let lease_millis = lease.as_millis().max(1);
    let elapsed_millis = now_epoch_millis % lease_millis;
    Duration::from_millis(u64::try_from(elapsed_millis).unwrap_or(u64::MAX))
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn observed_activity(
    has_pending_batch: bool,
    awaiting_signatures: usize,
) -> FederationWalletLivenessActivity {
    if has_pending_batch || awaiting_signatures > 0 {
        FederationWalletLivenessActivity::WaitingForLease
    } else {
        FederationWalletLivenessActivity::Idle
    }
}

#[cfg(test)]
mod tests {
    use cdk::federation::{
        FederationFrostApplicationId, FederationOnchainPaymentIntent, FederationOperationId,
    };
    use cdk_common::bitcoin::{Network, ScriptBuf};

    use super::*;

    #[test]
    fn leases_are_restart_stable_and_advance_at_bounded_boundaries() {
        let lease = Duration::from_secs(10);
        assert_eq!(lease_attempt(100_000, lease), 11);
        assert_eq!(lease_attempt(109_999, lease), 11);
        assert_eq!(lease_attempt(110_000, lease), 12);
        assert_eq!(
            lease_elapsed(119_999, lease),
            lease - Duration::from_millis(1)
        );
    }

    #[test]
    fn retry_backoff_is_bounded_by_the_signing_lease() {
        assert!(FederationWalletLivenessConfig::default().validate().is_ok());
        assert!(FederationWalletLivenessConfig {
            max_retry_delay: SIGNING_ATTEMPT_LEASE + Duration::from_secs(1),
            ..FederationWalletLivenessConfig::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn fee_allocation_is_deterministic_and_never_exceeds_intent_caps() {
        let application_id =
            FederationFrostApplicationId::from_bytes([7; 32]).expect("application id");
        let scope = FederationTransactionScope {
            root_key_epoch: 1,
            application_id,
            application_key_epoch: 1,
            network: Network::Regtest,
            policy_version: 1,
        };
        let intents = [
            ("first", 4_u64, 1_u64),
            ("second", 6_u64, 2_u64),
            ("third", 10_u64, 3_u64),
        ]
        .into_iter()
        .map(|(quote, maximum_fee_sat, creation_time)| {
            FederationOnchainPaymentIntent::new(
                quote.to_string(),
                FederationOperationId::from_bytes([creation_time as u8; 32]),
                ScriptBuf::from_bytes(vec![0x51]),
                1_000,
                maximum_fee_sat,
                0,
                1,
                creation_time,
                scope,
            )
            .expect("intent")
        })
        .collect::<Vec<_>>();
        let batch = FederationOnchainPaymentBatch::new(intents, 3).expect("batch");

        assert_eq!(fee_contributions(&batch, 9).expect("fees"), vec![4, 5, 0]);
        assert!(fee_contributions(&batch, 21).is_err());
        let rebuilt = FederationOnchainPaymentBatch::new(batch.intents.clone(), 3)
            .expect("rebuilt batch after proposer loss");
        assert_eq!(rebuilt.batch_id, batch.batch_id);
        assert_eq!(
            change_allocation_key(rebuilt.batch_id),
            change_allocation_key(batch.batch_id)
        );

        let policy = cdk::mint::FederationOnchainTransactionPolicy::for_version(1)
            .expect("replacement policy");
        assert_eq!(
            replacement_fee_sat(policy, 2, 500, 500, 1_000).expect("funded replacement"),
            625
        );
        assert!(replacement_fee_sat(policy, 2, 500, 500, 624).is_err());
    }

    #[test]
    fn offline_proposer_failover_rebuilds_the_same_consensus_batch() {
        let application_id =
            FederationFrostApplicationId::from_bytes([11; 32]).expect("application id");
        let scope = FederationTransactionScope {
            root_key_epoch: 1,
            application_id,
            application_key_epoch: 1,
            network: Network::Regtest,
            policy_version: 1,
        };
        let intent = FederationOnchainPaymentIntent::new(
            "offline-proposer".to_string(),
            FederationOperationId::from_bytes([12; 32]),
            ScriptBuf::from_bytes(vec![0x51]),
            1_000,
            10,
            0,
            1,
            7,
            scope,
        )
        .expect("intent");
        let before_build =
            FederationOnchainPaymentBatch::new(vec![intent.clone()], 7).expect("initial batch");
        let after_lost_build =
            FederationOnchainPaymentBatch::new(vec![intent], 7).expect("failover batch");
        let members = (1..=3)
            .map(|member| cdk::federation::MemberId::new(member).expect("member"))
            .collect::<Vec<_>>();
        let schedule =
            FederationWalletLivenessSchedule::new(before_build.batch_id.to_bytes(), members, 2)
                .expect("schedule");

        assert_ne!(
            schedule.proposer(1).expect("initial proposer"),
            schedule.proposer(2).expect("failover proposer")
        );
        assert_eq!(before_build.batch_id, after_lost_build.batch_id);
        assert_eq!(
            change_allocation_key(before_build.batch_id),
            change_allocation_key(after_lost_build.batch_id)
        );
    }

    #[test]
    fn long_idle_period_reactivates_only_when_payment_work_appears() {
        for _ in 0..100_000 {
            assert_eq!(
                observed_activity(false, 0),
                FederationWalletLivenessActivity::Idle
            );
        }
        assert_eq!(
            observed_activity(true, 0),
            FederationWalletLivenessActivity::WaitingForLease
        );
        assert_eq!(
            observed_activity(false, 1),
            FederationWalletLivenessActivity::WaitingForLease
        );
    }

    #[tokio::test]
    async fn fee_source_success_timeout_disagreement_fallback_and_extremes_are_bounded() {
        let policy =
            cdk::mint::FederationOnchainTransactionPolicy::for_version(1).expect("fee policy");
        assert_eq!(bounded_fee_rate_value(policy, Some(5.1)), 6);
        assert_eq!(bounded_fee_rate_value(policy, None), 2);
        assert_eq!(bounded_fee_rate_value(policy, Some(f64::NAN)), 2);
        assert_eq!(bounded_fee_rate_value(policy, Some(1_000_000.0)), 400);
        assert_eq!(bounded_fee_rate_value(policy, Some(7.0)), 7);
        assert_eq!(bounded_fee_rate_value(policy, Some(9.0)), 9);

        let mut timeout_policy = policy;
        timeout_policy.fee_estimation_timeout_millis = 1;
        let timed_out = bounded_fee_rate::<(), _>(timeout_policy, std::future::pending()).await;
        assert_eq!(timed_out, timeout_policy.fallback_fee_rate_sat_per_vb);
    }
}
