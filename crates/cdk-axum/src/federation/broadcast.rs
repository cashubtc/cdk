//! Thin failover loop for exact-byte federated Bitcoin broadcast.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cdk::federation::{
    redact_federation_operator_message, FederationAcceptedTransaction,
    FederationAcceptedTransactionState, FederationTransactionBroadcastIntent,
    FederationTransactionBroadcastObservation, FederationTransactionBroadcastResult,
    FederationTransactionProposalId,
};

use crate::MintState;

const BROADCAST_ROUND_INTERVAL: Duration = Duration::from_secs(5);
const BROADCAST_RETRY_DELAY: Duration = Duration::from_secs(5);
const BROADCAST_SUCCESS_RECHECK_DELAY: Duration = Duration::from_secs(60);
const BROADCAST_REJECTION_RECHECK_DELAY: Duration = Duration::from_secs(5 * 60);
const MAX_BROADCAST_DETAIL_BYTES: usize = 512;

#[derive(Debug, Clone, Copy)]
struct LocalRetryState {
    not_before: Instant,
}

/// Member-local liveness scheduler; transaction authority remains in consensus state.
#[derive(Debug, Clone, Default)]
pub(crate) struct FederationTransactionBroadcaster {
    retries: Arc<tokio::sync::Mutex<BTreeMap<FederationTransactionProposalId, LocalRetryState>>>,
}

impl FederationTransactionBroadcaster {
    pub(crate) async fn run_round(&self, state: &MintState) -> Result<usize, cdk::Error> {
        let Some(runtime) = state.mint.federation_config() else {
            return Ok(0);
        };
        let observer = runtime.member_id();
        let transaction_state = state.mint.federation_transaction_state().await?;
        let mut submitted = 0usize;

        for accepted in transaction_state.accepted_transactions().values() {
            let Some(signed) = &accepted.signed else {
                continue;
            };
            let Some(wallet) = wallet_for_transaction(state, accepted) else {
                continue;
            };
            if accepted.broadcast.is_none() {
                if accepted.state != FederationAcceptedTransactionState::Signed {
                    continue;
                }
                let envelope = runtime.transaction_broadcast_intent_operation(
                    FederationTransactionBroadcastIntent {
                        proposal_id: signed.proposal_id,
                        txid: signed.txid,
                    },
                );
                state.submit_federation_envelope(envelope).await?;
                submitted = submitted.saturating_add(1);
                continue;
            }
            let broadcast = accepted
                .broadcast
                .as_ref()
                .expect("checked broadcast state");
            if broadcast.observations.iter().any(|record| {
                matches!(
                    record.observation.result,
                    FederationTransactionBroadcastResult::Confirmed { .. }
                )
            }) {
                continue;
            }
            let attempt = broadcast
                .observations
                .iter()
                .filter(|record| record.observation.observer == observer)
                .map(|record| record.observation.attempt)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| cdk::Error::Custom("broadcast attempt overflow".to_string()))?;

            if let Some(confirmation) = wallet.transaction_confirmation(signed.txid).await {
                let observation = FederationTransactionBroadcastObservation {
                    proposal_id: signed.proposal_id,
                    txid: signed.txid,
                    observer,
                    attempt,
                    result: FederationTransactionBroadcastResult::Confirmed {
                        block_hash: confirmation.block_hash,
                        block_height: confirmation.block_height,
                    },
                    detail: None,
                };
                submit_observation(state, runtime, observation).await?;
                submitted = submitted.saturating_add(1);
                continue;
            }
            if !matches!(
                accepted.state,
                FederationAcceptedTransactionState::BroadcastPending
                    | FederationAcceptedTransactionState::Broadcast
            ) {
                continue;
            }
            if wallet.ensure_ready(0).await.is_err() || !self.retry_is_due(signed.proposal_id).await
            {
                continue;
            }

            let transaction = signed.transaction()?;
            if transaction.compute_txid() != signed.txid {
                return Err(cdk::Error::Custom(
                    "durable signed transaction bytes changed before broadcast".to_string(),
                ));
            }
            let (result, detail, delay) =
                classify_broadcast_attempt(wallet.engine().broadcast_classified(transaction).await);
            let observation = FederationTransactionBroadcastObservation {
                proposal_id: signed.proposal_id,
                txid: signed.txid,
                observer,
                attempt,
                result,
                detail,
            };
            submit_observation(state, runtime, observation).await?;
            self.defer(signed.proposal_id, delay).await;
            submitted = submitted.saturating_add(1);
        }
        Ok(submitted)
    }

    async fn retry_is_due(&self, proposal_id: FederationTransactionProposalId) -> bool {
        self.retries
            .lock()
            .await
            .get(&proposal_id)
            .is_none_or(|retry| Instant::now() >= retry.not_before)
    }

    async fn defer(&self, proposal_id: FederationTransactionProposalId, delay: Duration) {
        self.retries.lock().await.insert(
            proposal_id,
            LocalRetryState {
                not_before: Instant::now() + delay,
            },
        );
    }

    pub(crate) fn spawn(self, state: MintState) {
        if state.mint.federation_config().is_none() || state.federation.onchain_wallets.is_empty() {
            return;
        }
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(BROADCAST_ROUND_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                match self.run_round(&state).await {
                    Ok(submitted) if submitted > 0 => tracing::debug!(
                        submitted,
                        "Federation transaction broadcaster advanced durable state"
                    ),
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        "Federation transaction broadcaster failed; exact-byte retry remains pending: {}",
                        error
                    ),
                }
            }
        });
    }
}

fn wallet_for_transaction<'a>(
    state: &'a MintState,
    accepted: &FederationAcceptedTransaction,
) -> Option<&'a cdk_bdk::FederatedBdkWallet> {
    state.federation.onchain_wallets.iter().find(|wallet| {
        let scope = wallet.scope();
        scope.root_key_epoch() == accepted.proposal.scope.root_key_epoch
            && scope.application_id() == accepted.proposal.scope.application_id.to_bytes()
            && scope.application_key_epoch() == accepted.proposal.scope.application_key_epoch
    })
}

async fn submit_observation(
    state: &MintState,
    runtime: &cdk::mint::FederationRuntimeConfig,
    observation: FederationTransactionBroadcastObservation,
) -> Result<(), cdk::Error> {
    let envelope = runtime.transaction_broadcast_observation_operation(observation)?;
    state.submit_federation_envelope(envelope).await?;
    Ok(())
}

fn redacted_detail(message: &str) -> String {
    let mut detail = redact_federation_operator_message(message);
    if detail.len() > MAX_BROADCAST_DETAIL_BYTES {
        let mut end = MAX_BROADCAST_DETAIL_BYTES;
        while !detail.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        detail.truncate(end);
    }
    if redact_federation_operator_message(&detail) == detail
        && !detail.chars().any(char::is_control)
    {
        detail
    } else {
        "backend detail redacted".to_string()
    }
}

fn classify_broadcast_attempt(
    result: Result<cdk_bdk::BroadcastOutcome, cdk_bdk::BroadcastFailure>,
) -> (
    FederationTransactionBroadcastResult,
    Option<String>,
    Duration,
) {
    match result {
        Ok(cdk_bdk::BroadcastOutcome::Accepted) => (
            FederationTransactionBroadcastResult::Accepted,
            None,
            BROADCAST_SUCCESS_RECHECK_DELAY,
        ),
        Ok(cdk_bdk::BroadcastOutcome::AlreadyKnown) => (
            FederationTransactionBroadcastResult::AlreadyKnown,
            None,
            BROADCAST_SUCCESS_RECHECK_DELAY,
        ),
        Err(failure) => {
            let (result, delay) = match failure.kind {
                cdk_bdk::BroadcastErrorKind::Rejected => (
                    FederationTransactionBroadcastResult::Rejected,
                    BROADCAST_REJECTION_RECHECK_DELAY,
                ),
                cdk_bdk::BroadcastErrorKind::Transient => (
                    FederationTransactionBroadcastResult::Transient,
                    BROADCAST_RETRY_DELAY,
                ),
                cdk_bdk::BroadcastErrorKind::Unknown => (
                    FederationTransactionBroadcastResult::Unknown,
                    BROADCAST_RETRY_DELAY,
                ),
            };
            (result, Some(redacted_detail(&failure.message)), delay)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_failure_detail_is_redacted_and_bounded() {
        let detail = redacted_detail(&format!(
            "secret=wallet-token endpoint=http://user:pass@example.test/{}",
            "x".repeat(1_000)
        ));

        assert!(detail.len() <= MAX_BROADCAST_DETAIL_BYTES);
        assert!(!detail.contains("wallet-token"));
        assert!(!detail.contains("user:pass"));
        assert_eq!(redact_federation_operator_message(&detail), detail);
    }

    #[test]
    fn every_existing_bdk_broadcast_class_maps_to_federation_state() {
        for (backend, expected, expected_delay) in [
            (
                Ok(cdk_bdk::BroadcastOutcome::Accepted),
                FederationTransactionBroadcastResult::Accepted,
                BROADCAST_SUCCESS_RECHECK_DELAY,
            ),
            (
                Ok(cdk_bdk::BroadcastOutcome::AlreadyKnown),
                FederationTransactionBroadcastResult::AlreadyKnown,
                BROADCAST_SUCCESS_RECHECK_DELAY,
            ),
        ] {
            let (result, detail, delay) = classify_broadcast_attempt(backend);
            assert_eq!(result, expected);
            assert_eq!(detail, None);
            assert_eq!(delay, expected_delay);
        }
        for (kind, expected, expected_delay) in [
            (
                cdk_bdk::BroadcastErrorKind::Rejected,
                FederationTransactionBroadcastResult::Rejected,
                BROADCAST_REJECTION_RECHECK_DELAY,
            ),
            (
                cdk_bdk::BroadcastErrorKind::Transient,
                FederationTransactionBroadcastResult::Transient,
                BROADCAST_RETRY_DELAY,
            ),
            (
                cdk_bdk::BroadcastErrorKind::Unknown,
                FederationTransactionBroadcastResult::Unknown,
                BROADCAST_RETRY_DELAY,
            ),
        ] {
            let (result, detail, delay) =
                classify_broadcast_attempt(Err(cdk_bdk::BroadcastFailure {
                    kind,
                    message: "backend response".to_string(),
                }));
            assert_eq!(result, expected);
            assert_eq!(detail.as_deref(), Some("backend response"));
            assert_eq!(delay, expected_delay);
        }

        let (ambiguous, _, _) = classify_broadcast_attempt(Err(cdk_bdk::BroadcastFailure {
            kind: cdk_bdk::BroadcastErrorKind::Unknown,
            message: "response lost".to_string(),
        }));
        let (recovered, _, _) =
            classify_broadcast_attempt(Ok(cdk_bdk::BroadcastOutcome::AlreadyKnown));
        assert_eq!(ambiguous, FederationTransactionBroadcastResult::Unknown);
        assert_eq!(
            recovered,
            FederationTransactionBroadcastResult::AlreadyKnown,
            "a lost accepted response recovers through exact-byte already-known retry"
        );
    }
}
