use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
#[cfg(feature = "prometheus")]
use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aleph_bft::{default_delay_config, Network, NodeIndex, Recipient};
use async_trait::async_trait;
use bip39::Mnemonic;
use cdk::cdk_database::{
    MintDatabase, MintProofsDatabase, MintQuotesDatabase, MintSignaturesDatabase,
};
use cdk::cdk_payment::{
    self, CreateIncomingPaymentResponse, DynMintPayment, Event, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, SettingsResponse, WaitPaymentResponse,
};
use cdk::federation::{
    generate_trusted_dealer_configs, FederationAlephBftActivityRequest,
    FederationAlephBftBackupRequest, FederationAlephBftBackupResponse,
    FederationAlephBftFinalizedItemStatus, FederationAlephBftNetworkMessageRequest,
    FederationAlephBftNetworkMessageResponse, FederationAlephBftPendingOperationRecord,
    FederationAlephBftUnitPayload, FederationApplicationState, FederationCheckpoint,
    FederationCheckpointAuditRequest, FederationCheckpointBoundary, FederationCheckpointKind,
    FederationCheckpointPolicy, FederationCheckpointQuorum, FederationCheckpointSignature,
    FederationCheckpointSummaryRequest, FederationCheckpointSummaryResponse,
    FederationConfigSnapshot, FederationConsensusItem, FederationConsensusItemAdmission,
    FederationConsensusModuleFact, FederationConsensusRuntime, FederationDatabaseBackupManifest,
    FederationDkgActivationRequest, FederationDkgCeremonyId, FederationDkgCeremonyRecord,
    FederationDkgCeremonyState, FederationDkgCommitmentRound, FederationDkgMemberKeysetShare,
    FederationDkgSetupMethod, FederationDkgTranscript, FederationDkgTranscriptHash,
    FederationDkgTranscriptSignature, FederationError, FederationJournalAppendOutcome,
    FederationJournalDigest, FederationJournalEntry, FederationKeysetRotationOperation,
    FederationMeltQuoteMetadata, FederationMeltQuotePaymentObservation,
    FederationMintQuoteOperation, FederationMintQuotePaymentObservation, FederationOperation,
    FederationOperationEnvelope, FederationOperationId, FederationOperationKind,
    FederationOperationStatus, FederationPaymentObservation, FederationPaymentObservationContext,
    FederationPaymentObservationPolicy, FederationPaymentObservationRecord,
    FederationPaymentObservationReportAction, FederationPaymentVerificationPolicy,
    FederationRecoveryBarrier, FederationRestMessageKind, FederationSignedRequest,
    InMemoryFederationJournal, MemberConfigPrivate, MemberConfigPublic, MemberId, ThresholdParams,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk::mint::{MintBuilder, MintInput, MintMeltLimits, MintQuote, QuoteId};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nut01::{
    aggregate_blind_signature_shares, evaluate_secret_key_coefficients, BlsG1PublicKey,
    BlsSecretKey,
};
use cdk::nuts::nut22::AuthToken;
use cdk::nuts::{
    BlindedMessage, CheckStateRequest, CurrencyUnit, Id, MeltQuoteBolt11Request,
    MeltQuoteBolt11Response, MeltQuoteState, MintQuoteBolt11Request, MintQuoteBolt11Response,
    MintQuoteState, MintRequest, PaymentMethod, PublicKey, RestoreRequest, SecretKey,
};
use cdk::types::FeeReserve;
use cdk::Amount;
use cdk_common::database::mint::{CompletedOperationsDatabase, FederationApplicationDatabase};
use cdk_common::database::MintFederationJournalDatabase;
use cdk_common::mint::Operation;
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription, FakeWallet};
use cdk_http_client::{HttpError, RawResponse, Transport};
use futures::Stream;
use parity_scale_codec::{Decode, Encode};
use serde::Serialize;
use tower::ServiceExt;
#[cfg(feature = "prometheus")]
use tracing::instrument::WithSubscriber;

use crate::auth::AuthHeader;

use super::aleph_bft_latency::{
    CdkAlephBftLatencyEvent, CdkAlephBftLatencyRecorder, CdkAlephBftLatencyStage,
};
use super::aleph_bft_network::CdkAlephBftNetworkInboundHandle;
use super::aleph_bft_network::{
    run_cdk_aleph_bft_network_pump_tick, run_cdk_aleph_bft_network_pump_tick_with_retry_queue,
    run_cdk_aleph_bft_network_pump_tick_with_retry_queue_and_first_outbound,
    CdkAlephBftNetworkPumpConfig, CdkAlephBftNetworkRetryQueue,
    CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT,
};
#[cfg(feature = "prometheus")]
use super::aleph_bft_runtime::CdkAlephBftSessionIo;
use super::aleph_bft_runtime::{
    CdkAlephBftActivityHint, CdkAlephBftKeychain, CdkAlephBftNetwork, CdkAlephBftNetworkData,
};
use super::aleph_bft_session::AlephBftSessionSupervisorState;
use super::peer_client::FederationRestPeerClientError;
use super::*;

mod aleph_bft_routes;
mod dkg_driver;
mod dkg_setup_routes;
mod dkg_transcript_signature;
mod frost_signing;
mod private_operation_route;
#[cfg(feature = "federation-bdk")]
mod rbf;

#[cfg(feature = "prometheus")]
fn prometheus_metric_value(metric: &str, labels: &[(&str, &str)]) -> f64 {
    let registry = cdk_prometheus::METRICS.registry();
    let scrape = cdk_prometheus::prometheus::TextEncoder::new()
        .encode_to_string(&registry.gather())
        .expect("encode prometheus metrics");
    scrape
        .lines()
        .find_map(|line| {
            if !line.starts_with(metric) {
                return None;
            }
            for (key, value) in labels {
                let fragment = format!("{key}=\"{value}\"");
                if !line.contains(&fragment) {
                    return None;
                }
            }
            line.rsplit_once(' ')
                .and_then(|(_, value)| value.parse::<f64>().ok())
        })
        .unwrap_or(0.0)
}

#[cfg(feature = "prometheus")]
fn prometheus_metric_sum(metric: &str, labels: &[(&str, &str)]) -> f64 {
    let registry = cdk_prometheus::METRICS.registry();
    let scrape = cdk_prometheus::prometheus::TextEncoder::new()
        .encode_to_string(&registry.gather())
        .expect("encode prometheus metrics");
    scrape
        .lines()
        .filter_map(|line| {
            if !line.starts_with(metric) {
                return None;
            }
            for (key, value) in labels {
                let fragment = format!("{key}=\"{value}\"");
                if !line.contains(&fragment) {
                    return None;
                }
            }
            line.rsplit_once(' ')
                .and_then(|(_, value)| value.parse::<f64>().ok())
        })
        .sum()
}

#[cfg(feature = "prometheus")]
#[derive(Debug, Clone, Default)]
struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

#[cfg(feature = "prometheus")]
impl io::Write for CapturedLogWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("captured log lock")
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(feature = "prometheus")]
impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedLogWriter {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn federation_peer_sync_disable_value_accepts_truthy_values() {
    for value in [
        "1", "true", "TRUE", "True", "yes", "YES", "Yes", "on", "ON", "On",
    ] {
        assert!(federation_peer_sync_disabled_value(value));
    }

    for value in ["", "0", "false", "no", "off"] {
        assert!(!federation_peer_sync_disabled_value(value));
    }
}

#[tokio::test]
async fn recovery_checkpoint_demand_uses_a_bounded_local_proposal_lease() {
    let runtime = FederationNodeRuntime::disabled();
    assert!(!runtime.recovery_checkpoint_is_requested().await);
    runtime.request_recovery_checkpoint().await;
    assert!(runtime.recovery_checkpoint_is_requested().await);
    let now = std::time::Instant::now();
    assert!(
        runtime
            .components
            .checkpoint
            .reserve_recovery_proposal(now, Duration::from_secs(30))
            .await
    );
    assert!(
        !runtime
            .components
            .checkpoint
            .reserve_recovery_proposal(now + Duration::from_secs(29), Duration::from_secs(30))
            .await
    );
    assert!(
        runtime
            .components
            .checkpoint
            .reserve_recovery_proposal(now + Duration::from_secs(30), Duration::from_secs(30))
            .await
    );
}

#[tokio::test]
async fn idle_member_does_not_propose_recovery_barrier_work() {
    let (public_config, first_private, _) = transport_config();
    let state = federation_route_state_for_private(public_config, first_private).await;

    assert!(!state.federation.recovery_checkpoint_is_requested().await);
    assert!(!state
        .federation
        .propose_recovery_barrier_if_requested()
        .await
        .expect("idle recovery proposal"));
    assert!(!state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .has_pending_aleph_bft_recovery_barrier());
}

#[tokio::test]
async fn independent_members_derive_identical_recovery_checkpoint_at_terminal_boundary() {
    let (public_config, first_private, second_private) = transport_config();
    let first =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;
    let second =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    let items = (0..2)
        .map(|_| FederationConsensusItem::operation(mint_quote_envelope(&first, &QuoteId::new())))
        .collect();
    time::timeout(
        Duration::from_secs(15),
        apply_identical_finalized_unit(&first, &second, items),
    )
    .await
    .expect("stable-tip unit application timeout");

    let first_checkpoint = time::timeout(
        Duration::from_secs(15),
        recovery_checkpoint_for_state(&first, &public_config, None),
    )
    .await
    .expect("first recovery-checkpoint timeout");
    let second_checkpoint = time::timeout(
        Duration::from_secs(15),
        recovery_checkpoint_for_state(&second, &public_config, None),
    )
    .await
    .expect("second recovery-checkpoint timeout");
    assert_eq!(first_checkpoint, second_checkpoint);
    let quorum = FederationCheckpointQuorum::new(
        first_checkpoint.clone(),
        vec![
            FederationCheckpointSignature::new_signed(&first_checkpoint, &first_private)
                .expect("first recovery-checkpoint signature"),
            FederationCheckpointSignature::new_signed(&second_checkpoint, &second_private)
                .expect("second recovery-checkpoint signature"),
        ],
    );
    quorum
        .verify(&public_config)
        .expect("stable-tip recovery-checkpoint quorum");
    assert_eq!(first_checkpoint.journal_next_index, 2);
    assert_eq!(first_checkpoint.finalized_next_item_index, 2);
    assert_eq!(first_checkpoint.next_unit_finalization_index, 1);
    assert_eq!(
        first_checkpoint.materialized_state_digest,
        first
            .mint
            .federation_materialized_state_snapshot()
            .await
            .expect("first stable-tip materialized state")
            .digest()
            .expect("first stable-tip materialized digest")
    );
    assert_eq!(
        first_checkpoint.materialized_state_digest,
        second
            .mint
            .federation_materialized_state_snapshot()
            .await
            .expect("second stable-tip materialized state")
            .digest()
            .expect("second stable-tip materialized digest")
    );
}

#[tokio::test]
async fn ordered_recovery_barrier_captures_complete_unit_without_stopping_traffic() {
    let (public_config, first_private, _) = transport_config();
    let state = federation_route_state_for_private(public_config.clone(), first_private).await;
    let first_quote = QuoteId::new();
    let second_quote = QuoteId::new();
    let payload = {
        let consensus = state
            .federation
            .consensus()
            .expect("consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![
                FederationConsensusItem::operation(mint_quote_envelope(&state, &first_quote)),
                FederationConsensusItem::recovery_barrier(
                    FederationRecoveryBarrier::new(public_config.federation_id, 0, None, 0)
                        .expect("recovery barrier"),
                ),
                FederationConsensusItem::operation(mint_quote_envelope(&state, &second_quote)),
            ],
        )
        .expect("barrier unit")
    };

    let responses = state
        .federation
        .apply_finalized_unit(payload)
        .await
        .expect("finalize barrier unit");
    assert_eq!(responses.len(), 2);
    assert!(responses.iter().all(|response| matches!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied | FederationOperationStatus::Rejected
    )));
    let checkpoint = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("recovery checkpoint lookup")
        .expect("recovery checkpoint candidate");
    assert_eq!(checkpoint.journal_next_index, 2);
    assert_eq!(checkpoint.finalized_next_item_index, 3);
    assert_eq!(checkpoint.next_unit_finalization_index, 1);
    let checkpoint_id = checkpoint.id().expect("initial recovery checkpoint id");
    assert!(state
        .mint
        .localstore()
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("trusted checkpoint lookup")
        .is_none());

    let finality_only_unit = {
        let consensus = state
            .federation
            .consensus()
            .expect("consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![FederationConsensusItem::ModuleFact(
                FederationConsensusModuleFact::new(
                    "checkpoint-test",
                    "finality-only",
                    serde_json::json!({}),
                ),
            )],
        )
        .expect("finality-only unit")
    };
    state
        .federation
        .apply_finalized_unit(finality_only_unit)
        .await
        .expect("finalize non-application item");
    let redundant_barrier = {
        let consensus = state
            .federation
            .consensus()
            .expect("consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![FederationConsensusItem::recovery_barrier(
                FederationRecoveryBarrier::new(
                    public_config.federation_id,
                    0,
                    None,
                    status.next_item_index,
                )
                .expect("redundant recovery barrier"),
            )],
        )
        .expect("redundant barrier unit")
    };
    state
        .federation
        .apply_finalized_unit(redundant_barrier)
        .await
        .expect("finalize redundant barrier");
    let retained_checkpoint = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(FederationCheckpointKind::Recovery)
        .await
        .expect("retained recovery checkpoint lookup")
        .expect("retained recovery checkpoint");
    assert_eq!(
        retained_checkpoint
            .id()
            .expect("retained recovery checkpoint id"),
        checkpoint_id
    );

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("operation after recovery barrier");
    assert_eq!(consensus_operation_count(&state).await, 3);

    let repeated_barrier = {
        let consensus = state
            .federation
            .consensus()
            .expect("consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![FederationConsensusItem::recovery_barrier(
                FederationRecoveryBarrier::new(
                    public_config.federation_id,
                    0,
                    None,
                    status.next_item_index,
                )
                .expect("repeated recovery barrier"),
            )],
        )
        .expect("repeated barrier unit")
    };
    state
        .federation
        .apply_finalized_unit(repeated_barrier)
        .await
        .expect("finalize repeated barrier");
    let advanced_checkpoint = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("advanced recovery checkpoint lookup")
        .expect("advanced recovery checkpoint");
    assert_ne!(
        advanced_checkpoint
            .id()
            .expect("advanced recovery checkpoint id"),
        checkpoint_id
    );
    assert_eq!(advanced_checkpoint.journal_next_index, 3);
    assert_eq!(advanced_checkpoint.finalized_next_item_index, 7);
}

#[tokio::test]
async fn recovery_barrier_reaches_quorum_with_one_member_unavailable() {
    let (public_config, first_private, second_private, _unavailable_private) =
        transport_config_three();
    let first =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;
    let second =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    let items = vec![
        FederationConsensusItem::operation(mint_quote_envelope(&first, &QuoteId::new())),
        FederationConsensusItem::recovery_barrier(
            FederationRecoveryBarrier::new(public_config.federation_id, 0, None, 0)
                .expect("recovery barrier"),
        ),
        FederationConsensusItem::operation(mint_quote_envelope(&first, &QuoteId::new())),
    ];
    apply_identical_finalized_unit(&first, &second, items).await;

    let first_checkpoint = first
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("first checkpoint lookup")
        .expect("first checkpoint");
    let second_checkpoint = second
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("second checkpoint lookup")
        .expect("second checkpoint");
    assert_eq!(
        first_checkpoint.id().expect("first checkpoint id"),
        second_checkpoint.id().expect("second checkpoint id")
    );
    FederationCheckpointQuorum::new(
        first_checkpoint.clone(),
        vec![
            FederationCheckpointSignature::new_signed(&first_checkpoint, &first_private)
                .expect("first signature"),
            FederationCheckpointSignature::new_signed(&second_checkpoint, &second_private)
                .expect("second signature"),
        ],
    )
    .verify(&public_config)
    .expect("two-of-three recovery checkpoint quorum");
}

#[tokio::test]
async fn concurrent_recovery_requests_queue_one_effective_barrier() {
    let (public_config, first_private, _) = transport_config();
    let state = federation_route_state_for_private(public_config, first_private).await;
    state.federation.request_recovery_checkpoint().await;

    let (first, second) = tokio::join!(
        state.federation.propose_recovery_barrier_if_requested(),
        state.federation.propose_recovery_barrier_if_requested(),
    );
    assert_eq!(
        usize::from(first.expect("first proposal")) + usize::from(second.expect("second proposal")),
        1
    );
    let consensus = state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await;
    assert_eq!(
        consensus
            .aleph_bft_finalized_items()
            .expect("finalized items")
            .iter()
            .filter(|item| matches!(item.item, FederationConsensusItem::RecoveryBarrier(_)))
            .count(),
        1
    );
    drop(consensus);
    assert!(state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("recovery checkpoint lookup")
        .is_some());
}

#[tokio::test]
async fn stale_pending_barrier_does_not_block_current_recovery_request() {
    let (public_config, first_private, second_private) = transport_config();
    let state = federation_route_state_for_private(public_config.clone(), first_private).await;
    let stale = FederationRecoveryBarrier::new(public_config.federation_id, 0, None, 0)
        .expect("stale recovery barrier");
    state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .submit_aleph_bft_recovery_barrier(stale)
        .expect("queue stale recovery barrier");

    let advancing_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        second_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(mint_quote_envelope(
            &state,
            &QuoteId::new(),
        ))],
    )
    .expect("advancing unit");
    state
        .federation
        .apply_finalized_unit(advancing_unit)
        .await
        .expect("advance finalized frontier");

    state.federation.request_recovery_checkpoint().await;
    assert!(state
        .federation
        .propose_recovery_barrier_if_requested()
        .await
        .expect("current recovery proposal"));

    let finalized_barriers = state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .aleph_bft_finalized_items()
        .expect("finalized items")
        .iter()
        .filter(|item| matches!(item.item, FederationConsensusItem::RecoveryBarrier(_)))
        .count();
    assert_eq!(finalized_barriers, 2);
    assert!(state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(FederationCheckpointKind::Recovery)
        .await
        .expect("recovery checkpoint lookup")
        .is_some());
}

#[tokio::test]
async fn restart_retries_last_ordered_barrier_with_the_same_checkpoint_id() {
    let (public_config, first_private, _) = transport_config();
    let mut state = federation_route_state_for_private(public_config.clone(), first_private).await;
    state.federation.suppress_next_recovery_checkpoint_capture();
    state.federation.request_recovery_checkpoint().await;
    assert!(state
        .federation
        .propose_recovery_barrier_if_requested()
        .await
        .expect("barrier proposal"));
    assert!(state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("suppressed checkpoint lookup")
        .is_none());
    let expected_id = recovery_checkpoint_for_state(&state, &public_config, None)
        .await
        .id()
        .expect("expected recovery checkpoint id");

    let (restarted_consensus, _) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("restart consensus from durable finalized barrier");
    state.federation.member_mut_for_test().consensus =
        restarted_consensus.expect("configured member handle");

    state
        .federation
        .recover_recovery_checkpoint_candidate_at_startup()
        .await
        .expect("startup recovery checkpoint retry");
    let first_id = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("recovered checkpoint lookup")
        .expect("recovered checkpoint")
        .id()
        .expect("recovered checkpoint id");
    assert_eq!(first_id, expected_id);
    state
        .federation
        .recover_recovery_checkpoint_candidate_at_startup()
        .await
        .expect("idempotent startup retry");
    let repeated_id = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("repeated checkpoint lookup")
        .expect("repeated checkpoint")
        .id()
        .expect("repeated checkpoint id");
    assert_eq!(first_id, repeated_id);
}

#[test]
fn federation_peer_sync_config_rejects_invalid_values() {
    let valid = FederationPeerSyncConfig::default();
    let cases = [
        (
            FederationPeerSyncConfig {
                peer_sync_interval: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "peer sync interval",
            },
        ),
        (
            FederationPeerSyncConfig {
                peer_request_timeout: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "peer request timeout",
            },
        ),
        (
            FederationPeerSyncConfig {
                operation_submission_timeout: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "operation submission timeout",
            },
        ),
        (
            FederationPeerSyncConfig {
                catch_up_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "catch-up limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                reconnect_initial_backoff: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "reconnect initial backoff",
            },
        ),
        (
            FederationPeerSyncConfig {
                reconnect_max_backoff: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "reconnect max backoff",
            },
        ),
        (
            FederationPeerSyncConfig {
                reconnect_initial_backoff: Duration::from_secs(2),
                reconnect_max_backoff: Duration::from_secs(1),
                ..valid
            },
            FederationPeerSyncConfigError::ReconnectBackoffOrder,
        ),
        (
            FederationPeerSyncConfig {
                private_rest_inbound_concurrency_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "private REST inbound concurrency limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                private_rest_outbound_concurrency_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "private REST outbound concurrency limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                pending_mempool_item_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "pending mempool item limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                response_waiter_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "response waiter limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                response_cache_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "response cache limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                aleph_bft_active_unit_interval: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT active unit interval",
            },
        ),
        (
            FederationPeerSyncConfig {
                aleph_bft_network_pump_max_messages_per_tick: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT network pump max messages per tick",
            },
        ),
        (
            FederationPeerSyncConfig {
                aleph_bft_live_session_max_round: Some(0),
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT live session max round",
            },
        ),
        (
            FederationPeerSyncConfig {
                aleph_bft_live_session_max_round: Some(9),
                aleph_bft_live_session_rotation_limit: Some(0),
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT live session rotation limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                aleph_bft_live_session_rotation_limit: Some(2),
                ..valid
            },
            FederationPeerSyncConfigError::RotationLimitRequiresMaxRound,
        ),
    ];

    for (config, expected) in cases {
        assert_eq!(config.validate(), Err(expected));
    }
    assert!(valid.validate().is_ok());
}

#[test]
fn aleph_bft_live_session_options_use_configured_sync_round_cap() {
    let pump_config = CdkAlephBftNetworkPumpConfig::default();
    let default_options =
        FederationAlephBftLiveSessionOptions::from_sync_config(pump_config, Default::default());
    assert_eq!(default_options.max_round(), 3000);
    assert_eq!(default_options.automatic_rotation_limit(), None);

    let configured_options = FederationAlephBftLiveSessionOptions::from_sync_config(
        pump_config,
        FederationPeerSyncConfig {
            aleph_bft_live_session_max_round: Some(9),
            aleph_bft_live_session_rotation_limit: Some(2),
            ..Default::default()
        },
    );
    assert_eq!(configured_options.max_round(), 9);
    assert_eq!(configured_options.automatic_rotation_limit(), Some(2));

    let first_rotated_options = configured_options.after_automatic_rotation();
    assert_eq!(first_rotated_options.max_round(), 9);
    assert_eq!(first_rotated_options.automatic_rotation_limit(), Some(1));

    let second_rotated_options = first_rotated_options.after_automatic_rotation();
    assert_eq!(second_rotated_options.max_round(), 3000);
    assert_eq!(second_rotated_options.automatic_rotation_limit(), None);
}

#[tokio::test(start_paused = true)]
async fn federation_peer_sync_interval_uses_configured_delay() {
    let configured_delay = Duration::from_secs(2);
    let mut interval = federation_peer_sync_interval(FederationPeerSyncConfig {
        peer_sync_interval: configured_delay,
        ..FederationPeerSyncConfig::default()
    });
    interval.tick().await;

    let second_tick = tokio::spawn(async move {
        interval.tick().await;
    });
    tokio::task::yield_now().await;
    assert!(!second_tick.is_finished());

    tokio::time::advance(configured_delay - Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert!(!second_tick.is_finished());

    tokio::time::advance(Duration::from_millis(1)).await;
    second_tick.await.expect("second configured tick");
}

#[test]
fn federation_peer_sync_initial_delay_evenly_phases_members() {
    let interval = Duration::from_secs(5);
    let phases = (0..5)
        .map(|rank| federation_peer_sync_initial_delay(interval, rank, 5))
        .collect::<Vec<_>>();

    assert_eq!(
        phases,
        vec![
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(3),
            Duration::from_secs(4),
        ]
    );
    assert!(phases.iter().all(|phase| *phase < interval));
    assert_eq!(
        federation_peer_sync_initial_delay(interval, 0, 1),
        Duration::ZERO
    );
}

#[test]
fn federation_reconnect_backoff_doubles_until_max() {
    let config = FederationPeerSyncConfig {
        reconnect_initial_backoff: Duration::from_secs(2),
        reconnect_max_backoff: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };

    assert_eq!(
        federation_reconnect_backoff_delay(config, 0),
        Duration::from_secs(2)
    );
    assert_eq!(
        federation_reconnect_backoff_delay(config, 1),
        Duration::from_secs(4)
    );
    assert_eq!(
        federation_reconnect_backoff_delay(config, 2),
        Duration::from_secs(5)
    );
    assert_eq!(
        federation_reconnect_backoff_delay(config, u32::MAX),
        Duration::from_secs(5)
    );
}

#[derive(Debug, Default)]
struct MockFederationTransportState {
    status_response: Option<FederationPeerStatus>,
    checkpoint_summary_response: Option<(FederationCheckpointSummaryResponse, MemberConfigPrivate)>,
    submit_response: Option<FederationSubmitOperationResponse>,
    aleph_bft_network_message_response: Option<FederationAlephBftNetworkMessageResponse>,
    aleph_bft_network_message_delays: VecDeque<Duration>,
    aleph_bft_network_message_status_error: Option<(u16, String)>,
    dkg_setup_responses: VecDeque<FederationDkgSetupResponse>,
    aleph_bft_backup_responses: VecDeque<(FederationAlephBftBackupResponse, MemberConfigPrivate)>,
    catch_up_responses: VecDeque<FederationJournalCatchUpResponse>,
    status_requests: Vec<FederationSignedRequest<FederationPeerStatusRequest>>,
    checkpoint_summary_requests: Vec<FederationSignedRequest<FederationCheckpointSummaryRequest>>,
    dkg_commitment_requests: Vec<FederationSignedRequest<FederationDkgCommitmentRequest>>,
    dkg_reveal_requests: Vec<FederationSignedRequest<FederationDkgRevealRequest>>,
    dkg_secret_share_requests: Vec<FederationSignedRequest<FederationDkgSecretShareRequest>>,
    dkg_transcript_signature_requests:
        Vec<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>>,
    submitted_operations: Vec<FederationSignedRequest<FederationSubmitOperationRequest>>,
    aleph_bft_network_message_requests:
        Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>>,
    aleph_bft_backup_requests: Vec<FederationSignedRequest<FederationAlephBftBackupRequest>>,
    catch_up_requests: Vec<FederationSignedRequest<FederationJournalCatchUpRequest>>,
}

#[derive(Debug, Clone, Default)]
struct MockFederationTransport {
    state: Arc<Mutex<MockFederationTransportState>>,
    aleph_bft_network_message_changed: Arc<tokio::sync::Notify>,
}

#[derive(Debug, Clone)]
struct DelayTransport {
    delay: Duration,
    request_started: Option<Arc<tokio::sync::Notify>>,
}

impl DelayTransport {
    fn signal_request_started(&self) {
        if let Some(request_started) = &self.request_started {
            request_started.notify_one();
        }
    }
}

#[derive(Clone, Default)]
struct RouterFederationTransport {
    router: Option<Router>,
}

#[derive(Clone, Default)]
struct RouterFederationTransportRegistry {
    routers: Arc<Mutex<BTreeMap<String, Router>>>,
    shadow_routers: Arc<Mutex<BTreeMap<String, Router>>>,
    shadow_delivery_lock: Arc<tokio::sync::Mutex<()>>,
    missing_router_wait: Duration,
    faults: Arc<Mutex<RouterFederationTransportFaults>>,
    held_aleph_bft_network_message: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    aleph_bft_activity_requests:
        Arc<Mutex<Vec<FederationSignedRequest<FederationAlephBftActivityRequest>>>>,
    dkg_readiness_requests: Arc<Mutex<Vec<FederationSignedRequest<FederationDkgReadinessRequest>>>>,
    aleph_bft_network_message_requests:
        Arc<Mutex<Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>>>>,
    aleph_bft_network_message_changed: Arc<tokio::sync::Notify>,
}

#[derive(Debug, Default)]
struct RouterFederationTransportFaults {
    post_response_failures_after_delivery: BTreeMap<String, usize>,
    post_delays_before_delivery: BTreeMap<(String, MemberId), VecDeque<Duration>>,
    aleph_bft_network_message_delays: VecDeque<Duration>,
    aleph_bft_network_latency_model: Option<AlephBftNetworkLatencyModel>,
    aleph_bft_network_message_duplicate_replays: usize,
    aleph_bft_network_message_reorder_once: bool,
    aleph_bft_network_message_reorders_completed: usize,
    held_aleph_bft_data_senders: BTreeSet<MemberId>,
    blocked_federation_links: BTreeSet<(MemberId, MemberId)>,
}

#[derive(Debug, Clone, Copy)]
struct AlephBftNetworkLatencyModel {
    nominal_round_trip: Duration,
    jitter_percent: u8,
    state: u64,
}

impl AlephBftNetworkLatencyModel {
    fn next_delays(&mut self) -> (Duration, Duration) {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let nominal_micros =
            u64::try_from(self.nominal_round_trip.as_micros()).expect("test RTT fits u64");
        let jitter_micros = nominal_micros.saturating_mul(u64::from(self.jitter_percent)) / 100;
        let sample_width = jitter_micros.saturating_mul(2).saturating_add(1);
        let sampled_round_trip_micros = nominal_micros
            .saturating_sub(jitter_micros)
            .saturating_add(self.state % sample_width);
        let outbound_micros = sampled_round_trip_micros / 2;
        (
            Duration::from_micros(outbound_micros),
            Duration::from_micros(sampled_round_trip_micros - outbound_micros),
        )
    }
}

enum AlephBftNetworkMessageReorderDecision {
    None,
    Hold(tokio::sync::oneshot::Receiver<()>),
    ReleaseAfterPost(tokio::sync::oneshot::Sender<()>),
}

impl std::fmt::Debug for RouterFederationTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterFederationTransport")
            .field("configured", &self.router.is_some())
            .finish()
    }
}

impl std::fmt::Debug for RouterFederationTransportRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let router_count = self
            .routers
            .lock()
            .map(|routers| routers.len())
            .unwrap_or_default();
        let shadow_router_count = self
            .shadow_routers
            .lock()
            .map(|routers| routers.len())
            .unwrap_or_default();
        f.debug_struct("RouterFederationTransportRegistry")
            .field("router_count", &router_count)
            .field("shadow_router_count", &shadow_router_count)
            .field("missing_router_wait", &self.missing_router_wait)
            .field(
                "faults",
                &self
                    .faults
                    .lock()
                    .map(|faults| {
                        (
                            faults.post_response_failures_after_delivery.len(),
                            faults.post_delays_before_delivery.len(),
                            faults.aleph_bft_network_message_delays.len(),
                            faults.aleph_bft_network_latency_model,
                            faults.aleph_bft_network_message_duplicate_replays,
                            faults.aleph_bft_network_message_reorder_once,
                            faults.aleph_bft_network_message_reorders_completed,
                            faults.held_aleph_bft_data_senders.len(),
                            faults.blocked_federation_links.len(),
                        )
                    })
                    .unwrap_or_default(),
            )
            .field(
                "held_aleph_bft_network_message",
                &self
                    .held_aleph_bft_network_message
                    .lock()
                    .map(|held| held.is_some())
                    .unwrap_or_default(),
            )
            .field(
                "aleph_bft_activity_requests",
                &self
                    .aleph_bft_activity_requests
                    .lock()
                    .map(|requests| requests.len())
                    .unwrap_or_default(),
            )
            .field(
                "dkg_readiness_requests",
                &self
                    .dkg_readiness_requests
                    .lock()
                    .map(|requests| requests.len())
                    .unwrap_or_default(),
            )
            .field(
                "aleph_bft_network_message_requests",
                &self
                    .aleph_bft_network_message_requests
                    .lock()
                    .map(|requests| requests.len())
                    .unwrap_or_default(),
            )
            .finish()
    }
}

impl RouterFederationTransport {
    fn new(router: Router) -> Self {
        Self {
            router: Some(router),
        }
    }
}

impl RouterFederationTransportRegistry {
    fn fail_post_responses_after_delivery(&self, path_suffix: &str, count: usize) {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .post_response_failures_after_delivery
            .insert(path_suffix.to_string(), count);
    }

    fn take_post_response_failure_after_delivery(&self, path: &str) -> bool {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        let Some((_, remaining)) = faults
            .post_response_failures_after_delivery
            .iter_mut()
            .find(|(suffix, remaining)| path.ends_with(suffix.as_str()) && **remaining > 0)
        else {
            return false;
        };
        *remaining = remaining.saturating_sub(1);
        true
    }

    fn delay_next_posts_before_delivery<I>(&self, path_suffix: &str, receiver: MemberId, delays: I)
    where
        I: IntoIterator<Item = Duration>,
    {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .post_delays_before_delivery
            .entry((path_suffix.to_string(), receiver))
            .or_default()
            .extend(delays);
    }

    fn take_post_delay_before_delivery(&self, path: &str, receiver: MemberId) -> Duration {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .post_delays_before_delivery
            .iter_mut()
            .find(|((suffix, configured_receiver), delays)| {
                *configured_receiver == receiver && path.ends_with(suffix) && !delays.is_empty()
            })
            .and_then(|(_, delays)| delays.pop_front())
            .unwrap_or_default()
    }

    fn with_missing_router_wait(self, timeout: Duration) -> Self {
        Self {
            missing_router_wait: timeout,
            ..self
        }
    }

    fn with_aleph_bft_network_message_delays<I>(self, delays: I) -> Self
    where
        I: IntoIterator<Item = Duration>,
    {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_delays
            .extend(delays);
        self
    }

    fn set_aleph_bft_network_latency_model(
        &self,
        nominal_round_trip: Duration,
        jitter_percent: u8,
        seed: u64,
    ) {
        assert!(
            jitter_percent <= 100,
            "network jitter percent must not exceed 100"
        );
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_latency_model = if nominal_round_trip.is_zero() {
            None
        } else {
            Some(AlephBftNetworkLatencyModel {
                nominal_round_trip,
                jitter_percent,
                state: seed,
            })
        };
    }

    fn with_aleph_bft_network_message_duplicate_replays(self, count: usize) -> Self {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_duplicate_replays = count;
        self
    }

    fn with_aleph_bft_network_message_reorder_once(self) -> Self {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_reorder_once = true;
        self
    }

    fn aleph_bft_network_message_reorders_completed(&self) -> usize {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_reorders_completed
    }

    fn aleph_bft_network_message_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>> {
        self.aleph_bft_network_message_requests
            .lock()
            .expect("router transport network-message requests lock")
            .clone()
    }

    async fn wait_for_aleph_bft_network_message_request_matching<F>(
        &self,
        matches: F,
    ) -> FederationSignedRequest<FederationAlephBftNetworkMessageRequest>
    where
        F: Fn(&FederationSignedRequest<FederationAlephBftNetworkMessageRequest>) -> bool,
    {
        loop {
            let changed = self
                .aleph_bft_network_message_changed
                .clone()
                .notified_owned();
            if let Some(request) = self
                .aleph_bft_network_message_requests()
                .into_iter()
                .find(|request| matches(request))
            {
                return request;
            }
            changed.await;
        }
    }

    fn dkg_readiness_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgReadinessRequest>> {
        self.dkg_readiness_requests
            .lock()
            .expect("router transport DKG readiness requests lock")
            .clone()
    }

    fn insert(&self, url: MintUrl, router: Router) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.routers
            .lock()
            .expect("router registry lock")
            .insert(endpoint, router);
    }

    fn remove(&self, url: &MintUrl) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.routers
            .lock()
            .expect("router registry lock")
            .remove(&endpoint);
    }

    fn insert_shadow(&self, url: MintUrl, router: Router) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.shadow_routers
            .lock()
            .expect("shadow router registry lock")
            .insert(endpoint, router);
    }

    fn remove_shadow(&self, url: &MintUrl) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.shadow_routers
            .lock()
            .expect("shadow router registry lock")
            .remove(&endpoint);
    }

    fn set_member_outbound_partition(
        &self,
        public_config: &FederationConfig,
        sender: MemberId,
        partitioned: bool,
    ) {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        for receiver in public_config
            .members
            .keys()
            .copied()
            .filter(|receiver| *receiver != sender)
        {
            if partitioned {
                faults.blocked_federation_links.insert((sender, receiver));
            } else {
                faults.blocked_federation_links.remove(&(sender, receiver));
            }
        }
    }

    fn set_network_partition(&self, first: &[MemberId], second: &[MemberId], partitioned: bool) {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        for &sender in first {
            for &receiver in second {
                for link in [(sender, receiver), (receiver, sender)] {
                    if partitioned {
                        faults.blocked_federation_links.insert(link);
                    } else {
                        faults.blocked_federation_links.remove(&link);
                    }
                }
            }
        }
    }

    fn set_member_aleph_bft_data_hold(&self, sender: MemberId, held: bool) {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        if held {
            faults.held_aleph_bft_data_senders.insert(sender);
        } else {
            faults.held_aleph_bft_data_senders.remove(&sender);
        }
    }

    fn aleph_bft_data_is_held(&self, sender: MemberId) -> bool {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .held_aleph_bft_data_senders
            .contains(&sender)
    }

    fn federation_link_is_blocked(&self, sender: MemberId, receiver: MemberId) -> bool {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .blocked_federation_links
            .contains(&(sender, receiver))
    }

    fn pop_aleph_bft_network_message_delay(&self) -> Duration {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_delays
            .pop_front()
            .unwrap_or_default()
    }

    fn next_aleph_bft_network_round_trip_delays(&self) -> (Duration, Duration) {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_latency_model
            .as_mut()
            .map(AlephBftNetworkLatencyModel::next_delays)
            .unwrap_or_default()
    }

    fn take_aleph_bft_network_message_duplicate_replay(&self) -> bool {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        if faults.aleph_bft_network_message_duplicate_replays == 0 {
            return false;
        }
        faults.aleph_bft_network_message_duplicate_replays -= 1;
        true
    }

    fn aleph_bft_network_message_reorder_decision(&self) -> AlephBftNetworkMessageReorderDecision {
        let mut held = self
            .held_aleph_bft_network_message
            .lock()
            .expect("router transport held-message lock");
        if let Some(release) = held.take() {
            return AlephBftNetworkMessageReorderDecision::ReleaseAfterPost(release);
        }

        let mut faults = self.faults.lock().expect("router transport faults lock");
        if faults.aleph_bft_network_message_reorder_once {
            faults.aleph_bft_network_message_reorder_once = false;
            let (release, wait) = tokio::sync::oneshot::channel();
            *held = Some(release);
            return AlephBftNetworkMessageReorderDecision::Hold(wait);
        }

        AlephBftNetworkMessageReorderDecision::None
    }

    fn mark_aleph_bft_network_message_reorder_completed(&self) {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_reorders_completed += 1;
    }
}

impl Default for DelayTransport {
    fn default() -> Self {
        Self {
            delay: Duration::from_secs(60),
            request_started: None,
        }
    }
}

struct TempMintDbFile {
    path: String,
}

impl TempMintDbFile {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("{name}-{}.sqlite", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .to_string();

        Self { path }
    }

    fn path(&self) -> &str {
        &self.path
    }
}

impl Drop for TempMintDbFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(format!("{}-shm", self.path));
        let _ = std::fs::remove_file(format!("{}-wal", self.path));
    }
}

impl MockFederationTransport {
    fn with_status_response(self, response: FederationPeerStatus) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .status_response = Some(response);
        self
    }

    #[cfg(feature = "prometheus")]
    fn with_checkpoint_summary_response(
        self,
        response: FederationCheckpointSummaryResponse,
        signer: MemberConfigPrivate,
    ) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .checkpoint_summary_response = Some((response, signer));
        self
    }

    fn with_submit_response(self, response: FederationSubmitOperationResponse) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .submit_response = Some(response);
        self
    }

    fn with_aleph_bft_network_message_response(
        self,
        response: FederationAlephBftNetworkMessageResponse,
    ) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_response = Some(response);
        self
    }

    fn with_mock_aleph_bft_network_message_delays<I>(self, delays: I) -> Self
    where
        I: IntoIterator<Item = Duration>,
    {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_delays
            .extend(delays);
        self
    }

    fn with_aleph_bft_network_message_status_error(
        self,
        status: u16,
        message: impl Into<String>,
    ) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_status_error = Some((status, message.into()));
        self
    }

    fn with_dkg_setup_response(self, response: FederationDkgSetupResponse) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_setup_responses
            .push_back(response);
        self
    }

    fn with_catch_up_response(self, response: FederationJournalCatchUpResponse) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .catch_up_responses
            .push_back(response);
        self
    }

    fn with_aleph_bft_backup_response(
        self,
        response: FederationAlephBftBackupResponse,
        signer: MemberConfigPrivate,
    ) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_backup_responses
            .push_back((response, signer));
        self
    }

    fn status_requests(&self) -> Vec<FederationSignedRequest<FederationPeerStatusRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .status_requests
            .clone()
    }

    #[cfg(feature = "prometheus")]
    fn checkpoint_summary_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationCheckpointSummaryRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .checkpoint_summary_requests
            .clone()
    }

    fn dkg_commitment_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgCommitmentRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_commitment_requests
            .clone()
    }

    fn dkg_reveal_requests(&self) -> Vec<FederationSignedRequest<FederationDkgRevealRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_reveal_requests
            .clone()
    }

    fn dkg_secret_share_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgSecretShareRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_secret_share_requests
            .clone()
    }

    fn dkg_transcript_signature_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_transcript_signature_requests
            .clone()
    }

    fn submitted_operations(
        &self,
    ) -> Vec<FederationSignedRequest<FederationSubmitOperationRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .submitted_operations
            .clone()
    }

    fn aleph_bft_network_message_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_requests
            .clone()
    }

    async fn wait_for_aleph_bft_network_message_request_matching<F>(
        &self,
        matches: F,
    ) -> FederationSignedRequest<FederationAlephBftNetworkMessageRequest>
    where
        F: Fn(&FederationSignedRequest<FederationAlephBftNetworkMessageRequest>) -> bool,
    {
        loop {
            let changed = self
                .aleph_bft_network_message_changed
                .clone()
                .notified_owned();
            if let Some(request) = self
                .aleph_bft_network_message_requests()
                .into_iter()
                .find(|request| matches(request))
            {
                return request;
            }
            changed.await;
        }
    }

    fn catch_up_requests(&self) -> Vec<FederationSignedRequest<FederationJournalCatchUpRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .catch_up_requests
            .clone()
    }

    fn aleph_bft_backup_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationAlephBftBackupRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_backup_requests
            .clone()
    }
}

#[async_trait]
impl Transport for DelayTransport {
    fn with_proxy(
        &mut self,
        _proxy: url::Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Ok(Vec::new())
    }

    async fn http_get(
        &self,
        _url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        self.signal_request_started();
        time::sleep(self.delay).await;
        Err(HttpError::Other("delayed raw GET completed".to_string()))
    }

    async fn http_post<P>(
        &self,
        _url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        self.signal_request_started();
        time::sleep(self.delay).await;
        Err(HttpError::Other("delayed POST completed".to_string()))
    }

    async fn http_post_form<P>(
        &self,
        _url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        self.signal_request_started();
        time::sleep(self.delay).await;
        Err(HttpError::Other(
            "delayed raw form POST completed".to_string(),
        ))
    }
}

#[async_trait]
impl Transport for MockFederationTransport {
    fn with_proxy(
        &mut self,
        _proxy: url::Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Ok(Vec::new())
    }

    async fn http_get(
        &self,
        url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other(format!(
            "unexpected raw GET path {}",
            url.path()
        )))
    }

    async fn http_post<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        if url.path().ends_with("/federation/v1/aleph-bft/messages") {
            let request = serde_json::from_value::<
                FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
            >(serde_json::to_value(payload)?)?;
            let (response, status_error, delay) = {
                let mut state = self.state.lock().expect("mock transport lock");
                state.aleph_bft_network_message_requests.push(request);
                (
                    state.aleph_bft_network_message_response.clone(),
                    state.aleph_bft_network_message_status_error.clone(),
                    state
                        .aleph_bft_network_message_delays
                        .pop_front()
                        .unwrap_or_default(),
                )
            };
            self.aleph_bft_network_message_changed.notify_waiters();
            if !delay.is_zero() {
                time::sleep(delay).await;
            }
            if let Some((status, message)) = status_error {
                return Err(HttpError::Status { status, message });
            }
            let response = response.ok_or_else(|| {
                HttpError::Other("missing AlephBFT network-message response".to_string())
            })?;
            return json_response(response);
        }

        let mut state = self.state.lock().expect("mock transport lock");
        match url.path() {
            path if path.ends_with("/federation/v1/status") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationPeerStatusRequest>,
                >(serde_json::to_value(payload)?)?;
                state.status_requests.push(request);
                let response = state
                    .status_response
                    .clone()
                    .ok_or_else(|| HttpError::Other("missing status response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/checkpoints/summary") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationCheckpointSummaryRequest>,
                >(serde_json::to_value(payload)?)?;
                state.checkpoint_summary_requests.push(request.clone());
                let (response, signer) =
                    state.checkpoint_summary_response.clone().ok_or_else(|| {
                        HttpError::Other("missing checkpoint summary response".to_string())
                    })?;
                let response = FederationSignedRequest::new_signed(
                    request.federation_id,
                    request.sender,
                    request.sequence,
                    FederationRestMessageKind::CheckpointSummary,
                    response,
                    &signer,
                )
                .map_err(|err| HttpError::Other(err.to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/dkg/commitments") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationDkgCommitmentRequest>,
                >(serde_json::to_value(payload)?)?;
                state.dkg_commitment_requests.push(request);
                let response = state
                    .dkg_setup_responses
                    .pop_front()
                    .ok_or_else(|| HttpError::Other("missing DKG setup response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/dkg/reveals") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationDkgRevealRequest>,
                >(serde_json::to_value(payload)?)?;
                state.dkg_reveal_requests.push(request);
                let response = state
                    .dkg_setup_responses
                    .pop_front()
                    .ok_or_else(|| HttpError::Other("missing DKG setup response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/dkg/secret-shares") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationDkgSecretShareRequest>,
                >(serde_json::to_value(payload)?)?;
                state.dkg_secret_share_requests.push(request);
                let response = state
                    .dkg_setup_responses
                    .pop_front()
                    .ok_or_else(|| HttpError::Other("missing DKG setup response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/dkg/transcript-signatures") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationDkgTranscriptSignatureRequest>,
                >(serde_json::to_value(payload)?)?;
                state.dkg_transcript_signature_requests.push(request);
                let response = state
                    .dkg_setup_responses
                    .pop_front()
                    .ok_or_else(|| HttpError::Other("missing DKG setup response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/operations") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationSubmitOperationRequest>,
                >(serde_json::to_value(payload)?)?;
                state.submitted_operations.push(request);
                let response = state
                    .submit_response
                    .clone()
                    .ok_or_else(|| HttpError::Other("missing submit response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/aleph-bft/backup") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationAlephBftBackupRequest>,
                >(serde_json::to_value(payload)?)?;
                state.aleph_bft_backup_requests.push(request.clone());
                let (response, signer) =
                    state
                        .aleph_bft_backup_responses
                        .pop_front()
                        .ok_or_else(|| {
                            HttpError::Other("missing AlephBFT backup response".to_string())
                        })?;
                let response = FederationSignedRequest::new_signed(
                    request.federation_id,
                    request.sender,
                    request.sequence,
                    FederationRestMessageKind::AlephBftBackup,
                    response,
                    &signer,
                )
                .map_err(|err| HttpError::Other(err.to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/journal") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationJournalCatchUpRequest>,
                >(serde_json::to_value(payload)?)?;
                state.catch_up_requests.push(request);
                let response = state
                    .catch_up_responses
                    .pop_front()
                    .ok_or_else(|| HttpError::Other("missing catch-up response".to_string()))?;
                json_response(response)
            }
            path => Err(HttpError::Other(format!("unexpected POST path {path}"))),
        }
    }

    async fn http_post_form<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        Err(HttpError::Other(format!(
            "unexpected raw form POST path {}",
            url.path()
        )))
    }
}

#[async_trait]
impl Transport for RouterFederationTransport {
    fn with_proxy(
        &mut self,
        _proxy: url::Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Ok(Vec::new())
    }

    async fn http_get(
        &self,
        url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other(format!(
            "unexpected raw GET path {}",
            url.path()
        )))
    }

    async fn http_post<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        let router = self
            .router
            .clone()
            .ok_or_else(|| HttpError::Other("router transport not configured".to_string()))?;
        let request = axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri(url.path())
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(serde_json::to_vec(payload)?))
            .map_err(|err| HttpError::Other(err.to_string()))?;
        let response = router
            .oneshot(request)
            .await
            .map_err(|err| HttpError::Other(err.to_string()))?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .map_err(|err| HttpError::Other(err.to_string()))?;
        if !status.is_success() {
            return Err(HttpError::Status {
                status: status.as_u16(),
                message: String::from_utf8_lossy(&body).to_string(),
            });
        }
        Ok(RawResponse::new(status.as_u16(), body.to_vec()))
    }

    async fn http_post_form<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        Err(HttpError::Other(format!(
            "unexpected raw form POST path {}",
            url.path()
        )))
    }
}

#[async_trait]
impl Transport for RouterFederationTransportRegistry {
    fn with_proxy(
        &mut self,
        _proxy: url::Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Ok(Vec::new())
    }

    async fn http_get(
        &self,
        url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other(format!(
            "unexpected raw GET path {}",
            url.path()
        )))
    }

    async fn http_post<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        let endpoint = url.origin().ascii_serialization();
        let path = url.path().to_string();
        let is_aleph_bft_activity = path.ends_with("/federation/v1/aleph-bft/activity");
        let is_aleph_bft_network_message = path.ends_with("/federation/v1/aleph-bft/messages");
        let is_dkg_readiness = path.ends_with("/federation/v1/dkg/readiness");
        let deadline = tokio::time::Instant::now() + self.missing_router_wait;
        let router = loop {
            if let Some(router) = self
                .routers
                .lock()
                .expect("router registry lock")
                .get(&endpoint)
                .cloned()
            {
                break router;
            }
            if self.missing_router_wait.is_zero() || tokio::time::Instant::now() >= deadline {
                return Err(HttpError::Other(format!(
                    "router endpoint {endpoint} not configured"
                )));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        let payload_bytes = serde_json::to_vec(payload)?;
        let signed_request =
            serde_json::from_slice::<FederationSignedRequest<serde_json::Value>>(&payload_bytes)?;
        if is_aleph_bft_activity {
            let signed_request = serde_json::from_slice::<
                FederationSignedRequest<FederationAlephBftActivityRequest>,
            >(&payload_bytes)?;
            self.aleph_bft_activity_requests
                .lock()
                .expect("router transport activity requests lock")
                .push(signed_request);
        }
        if is_dkg_readiness {
            let signed_request = serde_json::from_slice::<
                FederationSignedRequest<FederationDkgReadinessRequest>,
            >(&payload_bytes)?;
            self.dkg_readiness_requests
                .lock()
                .expect("router transport DKG readiness requests lock")
                .push(signed_request);
        }
        let mut aleph_bft_message_contains_data = false;
        if is_aleph_bft_network_message {
            let signed_request = serde_json::from_slice::<
                FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
            >(&payload_bytes)?;
            aleph_bft_message_contains_data = signed_request
                .payload
                .message_bytes()
                .ok()
                .and_then(|bytes| {
                    let mut input = &bytes[..];
                    CdkAlephBftNetworkData::decode(&mut input)
                        .ok()
                        .filter(|_| input.is_empty())
                })
                .is_some_and(|message| !message.included_data().is_empty());
            self.aleph_bft_network_message_requests
                .lock()
                .expect("router transport network-message requests lock")
                .push(signed_request);
            self.aleph_bft_network_message_changed.notify_waiters();
        }
        if self.federation_link_is_blocked(signed_request.sender, signed_request.receiver) {
            return Err(HttpError::Other(format!(
                "federation link {} -> {} is partitioned",
                signed_request.sender, signed_request.receiver
            )));
        }
        let post_delay = self.take_post_delay_before_delivery(&path, signed_request.receiver);
        if !post_delay.is_zero() {
            tokio::time::sleep(post_delay).await;
        }
        if aleph_bft_message_contains_data && self.aleph_bft_data_is_held(signed_request.sender) {
            return Err(HttpError::Other(format!(
                "AlephBFT data messages from member {} are held",
                signed_request.sender
            )));
        }
        let (_, response_network_delay) = if is_aleph_bft_network_message {
            let delay = self.pop_aleph_bft_network_message_delay();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let (request_delay, response_delay) = self.next_aleph_bft_network_round_trip_delays();
            if !request_delay.is_zero() {
                tokio::time::sleep(request_delay).await;
            }
            (request_delay, response_delay)
        } else {
            (Duration::ZERO, Duration::ZERO)
        };
        let reorder_decision = if is_aleph_bft_network_message {
            self.aleph_bft_network_message_reorder_decision()
        } else {
            AlephBftNetworkMessageReorderDecision::None
        };
        let release_after_post = match reorder_decision {
            AlephBftNetworkMessageReorderDecision::Hold(wait_for_overtaking_post) => {
                let _ =
                    tokio::time::timeout(Duration::from_secs(1), wait_for_overtaking_post).await;
                None
            }
            AlephBftNetworkMessageReorderDecision::ReleaseAfterPost(release_held_post) => {
                Some(release_held_post)
            }
            AlephBftNetworkMessageReorderDecision::None => None,
        };

        let shadow_router = if is_aleph_bft_activity || is_aleph_bft_network_message {
            self.shadow_routers
                .lock()
                .expect("shadow router registry lock")
                .get(&endpoint)
                .cloned()
        } else {
            None
        };
        let _shadow_delivery_guard = match shadow_router.as_ref() {
            Some(_) => Some(self.shadow_delivery_lock.lock().await),
            None => None,
        };

        let response_body_result = match shadow_router {
            Some(shadow_router) => {
                let (primary_result, shadow_result) = tokio::join!(
                    router_transport_post_once_bytes(&router, &path, payload_bytes.clone()),
                    router_transport_post_once_bytes(&shadow_router, &path, payload_bytes.clone()),
                );
                shadow_result?;
                primary_result
            }
            None => router_transport_post_once_bytes(&router, &path, payload_bytes.clone()).await,
        };
        if let Some(release_held_post) = release_after_post {
            if release_held_post.send(()).is_ok() {
                self.mark_aleph_bft_network_message_reorder_completed();
            }
        }
        let response_body = response_body_result?;
        if !response_network_delay.is_zero() {
            tokio::time::sleep(response_network_delay).await;
        }
        if is_aleph_bft_network_message && self.take_aleph_bft_network_message_duplicate_replay() {
            let _ = router_transport_post_once_bytes(&router, &path, payload_bytes).await;
        }
        if self.take_post_response_failure_after_delivery(&path) {
            return Err(HttpError::Other(format!(
                "simulated response loss after durable POST delivery for {path}"
            )));
        }

        Ok(RawResponse::new(200, response_body))
    }

    async fn http_post_form<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        Err(HttpError::Other(format!(
            "unexpected raw form POST path {}",
            url.path()
        )))
    }
}

async fn router_transport_post_once_bytes(
    router: &Router,
    path: &str,
    payload_bytes: Vec<u8>,
) -> Result<Vec<u8>, HttpError> {
    let request = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri(path)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(payload_bytes))
        .map_err(|err| HttpError::Other(err.to_string()))?;
    let response = router
        .clone()
        .oneshot(request)
        .await
        .map_err(|err| HttpError::Other(err.to_string()))?;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|err| HttpError::Other(err.to_string()))?;
    if !status.is_success() {
        return Err(HttpError::Status {
            status: status.as_u16(),
            message: String::from_utf8_lossy(&body).to_string(),
        });
    }
    Ok(body.to_vec())
}

fn json_response<T>(value: T) -> Result<RawResponse, HttpError>
where
    T: Serialize,
{
    Ok(RawResponse::new(200, serde_json::to_vec(&value)?))
}

fn member_id(value: u16) -> MemberId {
    MemberId::new(value).expect("member id")
}

fn zero_aleph_bft_unit_cursor(config: &FederationConfig) -> BTreeMap<MemberId, u64> {
    config
        .members
        .keys()
        .copied()
        .map(|member| (member, 0))
        .collect()
}

#[test]
fn quorum_startup_requires_threshold_matching_views() {
    let (public_config, _, _) = transport_config();
    let local = FederationPeerStatus::new_with_materialized_state_digest(
        public_config.federation_id,
        member_id(1),
        0,
        None,
        FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
    );
    let matching_peer = |member| {
        let mut status = local.clone();
        status.member_id = member_id(member);
        status
    };
    let peers = vec![matching_peer(2)];

    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &peers, 2),
        Some(0)
    );
    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &peers, 3),
        None
    );
}

#[test]
fn quorum_startup_adopts_only_a_matching_later_session() {
    let (public_config, _, _) = transport_config();
    let local = FederationPeerStatus::new_with_materialized_state_digest(
        public_config.federation_id,
        member_id(1),
        7,
        Some(6),
        FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
    );
    let later_peer = |member| {
        let mut status = local.clone();
        status.member_id = member_id(member);
        status.aleph_bft_session = 2;
        status
    };
    let mut divergent = later_peer(3);
    divergent.operation_count = 8;

    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &[later_peer(2)], 2),
        Some(2)
    );
    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &[divergent], 2),
        None
    );
}

#[test]
fn quorum_startup_ignores_non_application_finality_and_local_candidate_audits() {
    let (public_config, _, _) = transport_config();
    let mut local = FederationPeerStatus::new(
        public_config.federation_id,
        member_id(1),
        7,
        Some(6),
        FederationJournalDigest::genesis(),
    )
    .with_aleph_bft_next_item_index(40);
    local.application_digest = Some(cdk::federation::FederationApplicationDigest::genesis());
    local.latest_audited_checkpoint_id =
        Some(cdk::federation::FederationCheckpointId::from_bytes([1; 32]));
    local.materialized_state_digest =
        Some(cdk::federation::FederationMaterializedStateDigest::from_bytes([2; 32]));

    let mut peer = local.clone();
    peer.member_id = member_id(2);
    peer.aleph_bft_next_item_index = Some(44);
    peer.latest_audited_checkpoint_id =
        Some(cdk::federation::FederationCheckpointId::from_bytes([3; 32]));
    peer.materialized_state_digest =
        Some(cdk::federation::FederationMaterializedStateDigest::from_bytes([4; 32]));

    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &[peer.clone()], 2),
        Some(0)
    );

    peer.application_digest = Some(cdk::federation::FederationApplicationDigest::from_bytes(
        [5; 32],
    ));
    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &[peer], 2),
        None
    );
}

#[test]
fn quorum_replay_target_uses_supported_current_session_frontier() {
    let (public_config, _) = transport_config_five_with_threshold(
        ThresholdParams::new(5, 3, 4).expect("threshold params"),
    );
    let members = public_config.members.keys().copied().collect::<Vec<_>>();
    let local_member = members[0];
    let tracked_member = members[1];
    let local_cursor = members.iter().copied().map(|member| (member, 5)).collect();
    let local = FederationPeerStatus::new(
        public_config.federation_id,
        local_member,
        7,
        Some(6),
        FederationJournalDigest::genesis(),
    )
    .with_aleph_bft_session(3)
    .with_aleph_bft_next_unit_index_by_member(local_cursor);
    let peer = |member: MemberId, tracked_next_unit_index: u64| {
        let mut status = local.clone();
        status.member_id = member;
        status
            .aleph_bft_next_unit_index_by_member
            .insert(tracked_member, tracked_next_unit_index);
        status
    };
    let mut stale_session = peer(members[4], 100);
    stale_session.aleph_bft_session = 2;
    let peer_statuses = vec![
        peer(members[1], 11),
        peer(members[2], 10),
        peer(members[3], 9),
        peer(members[4], 1),
        stale_session,
    ];

    let target = sync_loop::quorum_supported_replay_target(
        &local,
        &peer_statuses,
        3,
        usize::from(public_config.threshold_params.consensus_threshold),
    );

    assert_eq!(target.get(&tracked_member), Some(&5));
    assert_eq!(target.get(&local_member), Some(&5));
}

#[test]
fn quorum_replay_target_ignores_a_single_peers_unsupported_high_cursor() {
    let (public_config, first_private, second_private, third_private) =
        transport_config_three_with_threshold(
            ThresholdParams::new(3, 2, 2).expect("threshold params"),
        );
    let members = [&first_private, &second_private, &third_private]
        .into_iter()
        .map(|private| private.member_id)
        .collect::<Vec<_>>();
    let tracked_member = members[1];
    let mut local = FederationPeerStatus::new(
        public_config.federation_id,
        members[0],
        7,
        Some(6),
        FederationJournalDigest::genesis(),
    )
    .with_aleph_bft_session(3)
    .with_aleph_bft_next_unit_index_by_member(
        members.iter().copied().map(|member| (member, 5)).collect(),
    );
    local.application_digest = Some(cdk::federation::FederationApplicationDigest::genesis());
    let mut unsupported = local.clone();
    unsupported.member_id = members[1];
    unsupported
        .aleph_bft_next_unit_index_by_member
        .insert(tracked_member, 1_000_000);

    let target = sync_loop::quorum_supported_replay_target(&local, &[unsupported], 3, 2);

    assert_eq!(target.get(&tracked_member), Some(&5));
}

#[test]
fn quorum_startup_rejects_stale_round_evidence() {
    let observed_at = tokio::time::Instant::now();
    let max_age = std::time::Duration::from_secs(10);

    assert!(sync_loop::quorum_evidence_is_current(
        observed_at,
        observed_at + max_age,
        max_age,
    ));
    assert!(!sync_loop::quorum_evidence_is_current(
        observed_at,
        observed_at + max_age + std::time::Duration::from_millis(1),
        max_age,
    ));
}

#[test]
fn recovery_target_ignores_proof_superseded_by_scheduled_checkpoint() {
    assert!(sync_loop::recovery_checkpoint_is_superseded(1, 2));
    assert!(!sync_loop::recovery_checkpoint_is_superseded(2, 2));
    assert!(!sync_loop::recovery_checkpoint_is_superseded(3, 2));
}

#[tokio::test]
async fn checkpoint_target_ignores_newer_frontier_on_superseded_recovery_lineage() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_private = state
        .mint
        .federation_config()
        .expect("federation config")
        .private_config()
        .clone();
    let signers = [&local_private, &peer_private];

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("first scheduled operation");
    let mut first_scheduled = recovery_checkpoint_for_state(&state, &public_config, None).await;
    first_scheduled.kind = FederationCheckpointKind::Scheduled;
    first_scheduled.scheduled_checkpoint_sequence = 1;
    let first_quorum = FederationCheckpointQuorum::new(
        first_scheduled.clone(),
        signers
            .iter()
            .map(|signer| {
                FederationCheckpointSignature::new_signed_no_aux_rand(&first_scheduled, signer)
                    .expect("first scheduled signature")
            })
            .collect(),
    );

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("second scheduled operation");
    let mut second_scheduled =
        recovery_checkpoint_for_state(&state, &public_config, Some(&first_scheduled)).await;
    second_scheduled.kind = FederationCheckpointKind::Scheduled;
    second_scheduled.scheduled_checkpoint_sequence = 2;
    let second_quorum = FederationCheckpointQuorum::new(
        second_scheduled.clone(),
        signers
            .iter()
            .map(|signer| {
                FederationCheckpointSignature::new_signed_no_aux_rand(&second_scheduled, signer)
                    .expect("second scheduled signature")
            })
            .collect(),
    );

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("stale-lineage recovery operation");
    let (stale_recovery, stale_recovery_quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        Some(&first_scheduled),
        &signers,
    )
    .await;
    assert!(stale_recovery.journal_next_index > second_scheduled.journal_next_index);
    assert!(
        !checkpoint_manager::checkpoint_covers_current_recovery_need(
            &stale_recovery,
            stale_recovery.finalized_next_item_index,
            None,
            second_scheduled.scheduled_checkpoint_sequence,
            Some(
                second_scheduled
                    .id()
                    .expect("second scheduled checkpoint id")
            ),
        )
    );

    let db = state.mint.localstore();
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin checkpoint setup");
    tx.promote_verified_federation_checkpoint(
        &first_quorum
            .verify(&public_config)
            .expect("first scheduled quorum"),
    )
    .await
    .expect("promote first scheduled checkpoint");
    tx.promote_verified_federation_checkpoint(
        &second_quorum
            .verify(&public_config)
            .expect("second scheduled quorum"),
    )
    .await
    .expect("promote second scheduled checkpoint");
    tx.add_federation_checkpoint_quorum(&stale_recovery_quorum)
        .await
        .expect("store stale recovery quorum");
    tx.commit().await.expect("commit checkpoint setup");

    assert!(sync_loop::next_local_checkpoint_catch_up_target(&state)
        .await
        .expect("inspect next checkpoint target")
        .is_none());
}

#[test]
fn live_healthy_member_does_not_reenter_startup_quorum_reconciliation() {
    assert!(!sync_loop::quorum_reconciliation_is_required(
        &crate::FederationSyncStatus::Healthy,
        true,
    ));
    assert!(sync_loop::quorum_reconciliation_is_required(
        &crate::FederationSyncStatus::Healthy,
        false,
    ));
    assert!(sync_loop::quorum_reconciliation_is_required(
        &crate::FederationSyncStatus::AwaitingQuorum { operation_count: 7 },
        true,
    ));
    assert!(!sync_loop::quorum_reconciliation_is_required(
        &crate::FederationSyncStatus::Halted {
            reason: "test halt".to_string(),
        },
        false,
    ));
}

async fn response_error_detail(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body bytes");
    String::from_utf8_lossy(&bytes).to_string()
}

#[test]
fn private_federation_error_statuses_preserve_failure_class() {
    assert_eq!(
        federation_error_status(&FederationError::RuntimeUnavailable {
            component: "consensus",
        }),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        federation_error_status(&FederationError::PendingConsensusMempoolFull {
            pending: 4,
            max: 4,
        }),
        axum::http::StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        federation_error_status(&FederationError::ConflictingOperation {
            conflict_key: "quote:test".to_string(),
            existing_operation_id: "existing".to_string(),
            attempted_operation_id: "attempted".to_string(),
        }),
        axum::http::StatusCode::CONFLICT
    );
    assert_eq!(
        federation_error_status(&FederationError::AlephBftFinalizedItemRecoveryMismatch {
            reason: "stored state mismatch".to_string(),
        }),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        federation_error_status(&FederationError::InvalidMemberId(0)),
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        federation_error_status(&FederationError::JournalConsensusEvidenceMissing { index: 1 }),
        axum::http::StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn private_internal_federation_error_hides_diagnostic_detail() {
    for error in [
        FederationError::AlephBftFinalizedItemRecoveryMismatch {
            reason: "secret=internal-value".to_string(),
        },
        FederationError::BackendFailure {
            backend: "Bark",
            phase: "observe outgoing payment",
            operation_id: Some(FederationOperationId::from_bytes([42; 32])),
            reason: "internal diagnostic sentinel".to_string(),
        },
    ] {
        let response = federation_error_response(error);
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            response_error_detail(response).await,
            "federation internal error"
        );
    }
}

fn private_member(value: u16) -> MemberConfigPrivate {
    MemberConfigPrivate {
        federation_id: None,
        member_id: member_id(value),
        identity_secret_key: SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
        keyset_secret_shares: BTreeMap::new(),
        setup_transcript_hash: None,
    }
}

fn transport_config() -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
    transport_config_with_federation_api_urls(
        MintUrl::from_str("https://fed1.example.com/api").expect("federation api url"),
        MintUrl::from_str("https://fed2.example.com/api").expect("federation api url"),
    )
}

fn transport_config_with_federation_api_urls(
    first_federation_api_url: MintUrl,
    second_federation_api_url: MintUrl,
) -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
    transport_config_with_members_and_federation_api_urls(
        member_id(1),
        member_id(2),
        first_federation_api_url,
        second_federation_api_url,
    )
}

fn transport_config_with_members_and_federation_api_urls(
    first_member_id: MemberId,
    second_member_id: MemberId,
    first_federation_api_url: MintUrl,
    second_federation_api_url: MintUrl,
) -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
    let members = vec![
        TrustedDealerMember::new(
            first_member_id,
            MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
            first_federation_api_url,
            SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        ),
        TrustedDealerMember::new(
            second_member_id,
            MintUrl::from_str("https://mint2.example.com").expect("public mint url"),
            second_federation_api_url,
            SecretKey::from_slice(&[2; 32]).expect("identity secret"),
        ),
    ];
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[11; 32]),
                BlsSecretKey::from_reduced_bytes(&[12; 32]),
            ],
        )]),
    );
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::new(2, 2, 2).expect("threshold params"),
        members,
        vec![keyset],
    )
    .expect("trusted dealer bundle");
    let first_private = bundle
        .private_configs
        .get(&first_member_id)
        .expect("first private config")
        .clone();
    let second_private = bundle
        .private_configs
        .get(&second_member_id)
        .expect("second private config")
        .clone();
    (bundle.public_config, first_private, second_private)
}

fn transport_config_three() -> (
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    transport_config_three_with_threshold(ThresholdParams::new(3, 2, 2).expect("threshold params"))
}

fn transport_config_three_with_threshold(
    threshold_params: ThresholdParams,
) -> (
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    let members = vec![
        TrustedDealerMember::new(
            member_id(1),
            MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed1.example.com/api").expect("federation api url"),
            SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        ),
        TrustedDealerMember::new(
            member_id(2),
            MintUrl::from_str("https://mint2.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed2.example.com/api").expect("federation api url"),
            SecretKey::from_slice(&[2; 32]).expect("identity secret"),
        ),
        TrustedDealerMember::new(
            member_id(3),
            MintUrl::from_str("https://mint3.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed3.example.com/api").expect("federation api url"),
            SecretKey::from_slice(&[3; 32]).expect("identity secret"),
        ),
    ];
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[11; 32]),
                BlsSecretKey::from_reduced_bytes(&[12; 32]),
            ],
        )]),
    );
    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset])
        .expect("trusted dealer bundle");
    let first_private = bundle
        .private_configs
        .get(&member_id(1))
        .expect("first private config")
        .clone();
    let second_private = bundle
        .private_configs
        .get(&member_id(2))
        .expect("second private config")
        .clone();
    let third_private = bundle
        .private_configs
        .get(&member_id(3))
        .expect("third private config")
        .clone();
    (
        bundle.public_config,
        first_private,
        second_private,
        third_private,
    )
}

fn transport_config_with_member_federation_api_urls(
    threshold_params: ThresholdParams,
    federation_api_urls: Vec<MintUrl>,
) -> (FederationConfig, Vec<MemberConfigPrivate>) {
    let member_count = threshold_params.member_count;
    assert_eq!(usize::from(member_count), federation_api_urls.len());
    let members = federation_api_urls
        .into_iter()
        .enumerate()
        .map(|(index, federation_api_url)| {
            let value = u16::try_from(index + 1).expect("member index");
            TrustedDealerMember::new(
                member_id(value),
                MintUrl::from_str(&format!("https://mint{value}.example.com"))
                    .expect("public mint url"),
                federation_api_url,
                SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
            )
        })
        .collect::<Vec<_>>();
    let amount_coefficients = (0..threshold_params.signature_threshold)
        .map(|index| BlsSecretKey::from_reduced_bytes(&[31 + index as u8; 32]))
        .collect::<Vec<_>>();
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(Amount::from(1), amount_coefficients)]),
    );
    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset])
        .expect("trusted dealer bundle");
    let private_configs = (1u16..=member_count)
        .map(|value| {
            bundle
                .private_configs
                .get(&member_id(value))
                .expect("private config")
                .clone()
        })
        .collect::<Vec<_>>();

    (bundle.public_config, private_configs)
}

fn transport_config_with_member_federation_api_urls_and_keysets(
    threshold_params: ThresholdParams,
    federation_api_urls: Vec<MintUrl>,
    keysets: Vec<TrustedDealerKeysetSpec>,
) -> (FederationConfig, Vec<MemberConfigPrivate>) {
    let member_count = threshold_params.member_count;
    assert_eq!(usize::from(member_count), federation_api_urls.len());
    let members = federation_api_urls
        .into_iter()
        .enumerate()
        .map(|(index, federation_api_url)| {
            let value = u16::try_from(index + 1).expect("member index");
            TrustedDealerMember::new(
                member_id(value),
                MintUrl::from_str(&format!("https://mint{value}.example.com"))
                    .expect("public mint url"),
                federation_api_url,
                SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
            )
        })
        .collect::<Vec<_>>();
    let bundle = generate_trusted_dealer_configs(threshold_params, members, keysets)
        .expect("trusted dealer bundle");
    let private_configs = (1u16..=member_count)
        .map(|value| {
            bundle
                .private_configs
                .get(&member_id(value))
                .expect("private config")
                .clone()
        })
        .collect::<Vec<_>>();

    (bundle.public_config, private_configs)
}

fn transport_config_five_with_threshold(
    threshold_params: ThresholdParams,
) -> (FederationConfig, Vec<MemberConfigPrivate>) {
    let members = (1u16..=5)
        .map(|value| {
            TrustedDealerMember::new(
                member_id(value),
                MintUrl::from_str(&format!("https://mint{value}.example.com"))
                    .expect("public mint url"),
                MintUrl::from_str(&format!("https://fed{value}.example.com/api"))
                    .expect("federation api url"),
                SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
            )
        })
        .collect::<Vec<_>>();
    let amount_coefficients = (0..threshold_params.signature_threshold)
        .map(|index| BlsSecretKey::from_reduced_bytes(&[21 + index as u8; 32]))
        .collect::<Vec<_>>();
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(Amount::from(1), amount_coefficients)]),
    );
    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset])
        .expect("trusted dealer bundle");
    let private_configs = (1u16..=5)
        .map(|value| {
            bundle
                .private_configs
                .get(&member_id(value))
                .expect("private config")
                .clone()
        })
        .collect::<Vec<_>>();

    (bundle.public_config, private_configs)
}

#[derive(Debug)]
struct CountingFakeWallet {
    inner: FakeWallet,
    make_payment_calls: Arc<AtomicUsize>,
}

impl CountingFakeWallet {
    fn with_call_counter() -> (DynMintPayment, Arc<AtomicUsize>) {
        let make_payment_calls = Arc::new(AtomicUsize::new(0));
        let wallet = Self {
            inner: FakeWallet::new(
                FeeReserve {
                    min_fee_reserve: 1.into(),
                    percent_fee_reserve: 0.0,
                },
                HashMap::default(),
                HashSet::default(),
                0,
                CurrencyUnit::Sat,
            ),
            make_payment_calls: Arc::clone(&make_payment_calls),
        };

        (Arc::new(wallet), make_payment_calls)
    }
}

#[async_trait]
impl MintPayment for CountingFakeWallet {
    type Err = cdk_payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        self.inner.get_settings().await
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        self.inner.create_incoming_payment_request(options).await
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        self.inner.get_payment_quote(unit, options).await
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.make_payment_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.make_payment(unit, options).await
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        self.inner.wait_payment_event().await
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.inner.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        self.inner
            .check_incoming_payment_status(payment_identifier)
            .await
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.inner.check_outgoing_payment(payment_identifier).await
    }
}

async fn federation_route_state_with_local_private() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    let (public_config, local_private, peer_private) = transport_config();
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;

    (state, public_config, local_private, peer_private)
}

async fn federation_route_state_three_with_local_private() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    let (public_config, first_private, second_private, third_private) = transport_config_three();
    let state =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;

    (
        state,
        public_config,
        first_private,
        second_private,
        third_private,
    )
}

async fn federation_route_state_for_private(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
) -> MintState {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    federation_route_state_for_private_with_db(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await
}

async fn recovery_checkpoint_quorum_for_state(
    state: &MintState,
    public_config: &FederationConfig,
    scheduled_checkpoint: Option<&FederationCheckpoint>,
    signers: &[&MemberConfigPrivate],
) -> (FederationCheckpoint, FederationCheckpointQuorum) {
    let checkpoint =
        recovery_checkpoint_for_state(state, public_config, scheduled_checkpoint).await;
    let signatures = signers
        .iter()
        .map(|signer| {
            FederationCheckpointSignature::new_signed(&checkpoint, signer)
                .expect("checkpoint signature")
        })
        .collect();
    let quorum = FederationCheckpointQuorum::new(checkpoint.clone(), signatures);
    quorum
        .verify(public_config)
        .expect("recovery checkpoint quorum");
    (checkpoint, quorum)
}

async fn recovery_checkpoint_for_state(
    state: &MintState,
    public_config: &FederationConfig,
    scheduled_checkpoint: Option<&FederationCheckpoint>,
) -> FederationCheckpoint {
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("checkpoint frontier");
    let consensus = state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await;
    let status = consensus.status();
    let boundary = FederationCheckpointBoundary::from_complete_unit(
        &frontier,
        status.session,
        status.next_unit_finalization_index,
        consensus.next_unit_index_by_member().clone(),
    )
    .expect("complete checkpoint boundary");
    drop(consensus);
    let materialized_state_digest = state
        .mint
        .federation_materialized_state_snapshot()
        .await
        .expect("materialized state snapshot")
        .digest()
        .expect("materialized state digest");

    boundary
        .recovery_checkpoint(
            public_config,
            scheduled_checkpoint,
            materialized_state_digest,
        )
        .expect("recovery checkpoint")
}

fn one_operation_unit_checkpoint_pages(
    runtime: &FederationConsensusRuntime,
    checkpoint: &FederationCheckpoint,
    limit: u16,
) -> Vec<FederationJournalCatchUpResponse> {
    let checkpoint_id = checkpoint.id().expect("checkpoint page target id");
    let mut pages = Vec::new();
    let mut next = 0_usize;
    let target = usize::try_from(checkpoint.journal_next_index)
        .expect("checkpoint journal cursor fits usize");
    assert_eq!(
        checkpoint.journal_next_index, checkpoint.finalized_next_item_index,
        "test helper requires one accepted operation per finalized item"
    );
    while next < target {
        let request = FederationJournalCatchUpRequest::new(next as u64)
            .with_target_checkpoint(checkpoint_id)
            .with_limit(limit)
            .with_aleph_bft_next_item_index(next as u64);
        let response = FederationJournalCatchUpResponse::from_journal(
            checkpoint.federation_id,
            runtime.journal(),
            checkpoint.materialized_state_digest,
            &request,
            limit,
            limit,
        )
        .bound_to_checkpoint(checkpoint)
        .expect("bound one-operation-unit checkpoint page");
        let end = next.saturating_add(response.entries.len());
        pages.push(response.with_aleph_bft_finalized_items(
            runtime.finalized_items()[next..end].to_vec(),
            end == target,
        ));
        next = end;
    }
    pages
}

async fn federation_route_state_for_private_with_db(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
) -> MintState {
    let fake: DynMintPayment = Arc::new(FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 0.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    ));
    federation_route_state_for_private_with_db_payment_and_finality(
        public_config,
        local_private,
        db,
        seed,
        fake,
        true,
    )
    .await
}

async fn federation_route_state_for_private_with_db_and_aleph_bft(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
) -> MintState {
    let fake: DynMintPayment = Arc::new(FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 0.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    ));
    federation_route_state_for_private_with_db_payment_and_finality(
        public_config,
        local_private,
        db,
        seed,
        fake,
        false,
    )
    .await
}

async fn federation_route_state_for_private_with_db_and_aleph_bft_payment_processor(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
    payment_processor: DynMintPayment,
) -> MintState {
    federation_route_state_for_private_with_db_payment_and_finality(
        public_config,
        local_private,
        db,
        seed,
        payment_processor,
        false,
    )
    .await
}

async fn federation_route_state_for_private_with_db_payment_and_finality(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
    payment_processor: DynMintPayment,
    local_finality_test: bool,
) -> MintState {
    let runtime_config =
        cdk::mint::FederationRuntimeConfig::new(public_config.clone(), local_private.clone())
            .expect("runtime config");
    let mut builder = MintBuilder::new(db.clone())
        .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
        .with_federation_config(runtime_config.clone());
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            payment_processor,
        )
        .await
        .expect("payment processor");
    let mint = builder.build_with_seed(db, seed).await.expect("mint");
    let consensus = FederationConsensusRuntime::authenticated_aleph_bft(
        runtime_config.public_config(),
        runtime_config.member_id(),
        0,
    )
    .expect("AlephBFT consensus runtime");
    let mint = Arc::new(mint);
    MintState {
        mint: Arc::clone(&mint),
        cache: Arc::new(crate::cache::HttpCache::default()),
        federation: crate::federation::FederationNodeRuntime {
            transport: crate::default_federation_transport(),
            membership: crate::federation::FederationMembership::configured(
                crate::federation::FederationMemberContext::new(
                    mint,
                    Arc::new(tokio::sync::Mutex::new(consensus)),
                    Arc::new(tokio::sync::Mutex::new(
                        cdk::federation::FederationRestSequenceTracker::new(),
                    )),
                    crate::FederationPeerSyncConfig::default(),
                ),
            ),
            tasks: crate::federation::services::FederationTaskGroup::default(),
            payment_observer: crate::federation::FederationPaymentObserver::default(),
            #[cfg(feature = "federation-bdk")]
            transaction_broadcaster: crate::federation::FederationTransactionBroadcaster::default(),
            #[cfg(feature = "federation-bdk")]
            wallet_liveness: crate::federation::FederationWalletLivenessWorker::default(),
            #[cfg(feature = "federation-bdk")]
            onchain_sync_interval: None,
            components: crate::federation::runtime_components::FederationRuntimeComponents::new(
                Some(Arc::new(tokio::sync::Mutex::new(if local_finality_test {
                    crate::FederationSyncState::default()
                } else {
                    crate::FederationSyncState::for_aleph_bft()
                }))),
                Some(Arc::new(FederationDkgAutoStartConfig {
                    setup_authorization: public_config.setup_authorization,
                    ceremony_id: FederationDkgCeremonyId::from_bytes([55; 32]),
                    root_key_epoch: 1,
                    keyset_spec: cdk::federation::FederationDkgKeysetSpec::new(
                        CurrencyUnit::Sat,
                        true,
                        0,
                        None,
                        [Amount::from(1)].into_iter().collect(),
                    )
                    .expect("test DKG keyset spec"),
                    driver: FederationDkgDriverRunConfig::default(),
                })),
                Some(crate::federation_dkg_member_admission(
                    &public_config,
                    FederationDkgDriverRunConfig::default().member_request_burst,
                    FederationDkgDriverRunConfig::default().member_request_refill_interval,
                )),
                #[cfg(feature = "federation-bdk")]
                Vec::new(),
                None,
                #[cfg(feature = "federation-bark")]
                None,
            ),
            aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_halt_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            local_finality_test,
            manual_finality_test: false,
        },
    }
}

async fn federation_route_state() -> (MintState, FederationConfig, MemberConfigPrivate) {
    let (state, public_config, _, peer_private) = federation_route_state_with_local_private().await;
    (state, public_config, peer_private)
}

fn signed_operation_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    sequence: u64,
    kind: FederationRestMessageKind,
    envelope: FederationOperationEnvelope,
) -> FederationSignedRequest<FederationSubmitOperationRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        member_id(1),
        sequence,
        kind,
        FederationSubmitOperationRequest::new(envelope),
        private_config,
    )
    .expect("signed operation request")
}

fn signed_status_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
) -> FederationSignedRequest<FederationPeerStatusRequest> {
    signed_status_request_with_payload(
        public_config,
        private_config,
        receiver,
        sequence,
        FederationPeerStatusRequest::default(),
    )
}

fn signed_status_request_with_payload(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    payload: FederationPeerStatusRequest,
) -> FederationSignedRequest<FederationPeerStatusRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::Status,
        payload,
        private_config,
    )
    .expect("signed status request")
}

fn signed_catch_up_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationJournalCatchUpRequest,
) -> FederationSignedRequest<FederationJournalCatchUpRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::CatchUp,
        request,
        private_config,
    )
    .expect("signed catch-up request")
}

fn signed_catch_up_request_with_kind(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    kind: FederationRestMessageKind,
    request: FederationJournalCatchUpRequest,
) -> FederationSignedRequest<FederationJournalCatchUpRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        kind,
        request,
        private_config,
    )
    .expect("signed catch-up request")
}

fn finalized_aleph_bft_history<I>(
    public_config: &FederationConfig,
    submitting_member: MemberId,
    envelopes: I,
) -> FederationConsensusRuntime
where
    I: IntoIterator<Item = FederationOperationEnvelope>,
{
    let mut adapter = FederationConsensusRuntime::for_members(
        public_config.federation_id,
        submitting_member,
        0,
        public_config.members.keys().copied(),
    )
    .expect("test AlephBFT adapter");
    for (unit_index, envelope) in (0_u64..).zip(envelopes) {
        let unit = FederationAlephBftUnitPayload::new(
            public_config.federation_id,
            submitting_member,
            0,
            unit_index,
            vec![FederationConsensusItem::operation(envelope)],
        )
        .expect("test AlephBFT finalized unit");
        adapter
            .finalize_unit(unit)
            .expect("finalize test AlephBFT history");
    }
    adapter
}

fn test_aleph_bft_peer(
    federation_id: cdk::federation::FederationId,
    member_id: MemberId,
) -> FederationConsensusRuntime {
    FederationConsensusRuntime::new(federation_id, member_id, 0)
}

async fn backup_recovery_test_state() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPublic,
    FederationConsensusRuntime,
) {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let peer_member = public_config
        .member(member_id(2))
        .expect("peer member")
        .clone();
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    let envelope = mint_quote_envelope(&state, &QuoteId::new());
    finalize_test_operation(&mut peer_consensus, envelope).expect("finalize peer operation");

    let catch_up_request = state
        .federation
        .consensus()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let response = aleph_bft_catch_up_response_with_materialized_digest(
        &state,
        &public_config,
        &peer_consensus,
        &catch_up_request,
    )
    .await;
    state
        .apply_federation_catch_up_response(&response)
        .await
        .expect("seed exact finalized cursor");

    (
        state,
        public_config,
        local_private,
        peer_private,
        peer_member,
        peer_consensus,
    )
}

fn aleph_bft_catch_up_response(
    public_config: &FederationConfig,
    adapter: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let finalized_from = usize::try_from(request.aleph_bft_next_item_index).unwrap_or_default();
    FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(
        adapter
            .finalized_items()
            .get(finalized_from..)
            .unwrap_or_default()
            .to_vec(),
        true,
    )
}

fn test_catch_up_response(
    adapter: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let status = adapter.status();
    let finalized_from = usize::try_from(request.aleph_bft_next_item_index).unwrap_or_default();
    FederationJournalCatchUpResponse::from_journal(
        status.federation_id,
        adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(
        adapter
            .finalized_items()
            .get(finalized_from..)
            .unwrap_or_default()
            .to_vec(),
        true,
    )
}

fn checked_test_catch_up_response(
    adapter: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> Result<FederationJournalCatchUpResponse, FederationError> {
    if let Some(known_order_digest) = request.known_order_digest {
        if let Some(expected_digest) = adapter.journal().order_digest_before(request.next_index) {
            if expected_digest != known_order_digest {
                return Err(FederationError::CatchUpDigestMismatch {
                    index: request.next_index,
                    expected: expected_digest,
                    found: known_order_digest,
                });
            }
        }
    }

    Ok(test_catch_up_response(adapter, request))
}

fn dkg_reveal(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> cdk::federation::FederationDkgParticipantCommitmentReveal {
    cdk::federation::FederationDkgParticipantCommitmentReveal {
        federation_id: public_config.federation_id,
        setup_authorization: public_config.setup_authorization,
        ceremony_id: cdk::federation::FederationDkgCeremonyId::from_bytes([55; 32]),
        member_id,
        keyset_spec: cdk::federation::FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1)].into_iter().collect(),
        )
        .expect("DKG keyset spec"),
        coefficient_commitments: BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[seed; 32]).public_key_g2(),
                BlsSecretKey::from_reduced_bytes(&[seed + 10; 32]).public_key_g2(),
            ],
        )]),
    }
}

fn dkg_commitment_request(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> FederationDkgCommitmentRequest {
    let reveal = dkg_reveal(public_config, member_id, seed);
    let commitment = cdk::federation::FederationDkgParticipantCommitment::from_reveal(&reveal)
        .expect("dkg commitment");
    FederationDkgCommitmentRequest::new(commitment)
}

fn dkg_reveal_request(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> FederationDkgRevealRequest {
    FederationDkgRevealRequest::new(dkg_reveal(public_config, member_id, seed))
}

fn dkg_secret_share_request(
    public_config: &FederationConfig,
    sender: MemberId,
    receiver: MemberId,
    seed: u8,
) -> FederationDkgSecretShareRequest {
    FederationDkgSecretShareRequest::new(
        public_config.federation_id,
        public_config.setup_authorization,
        cdk::federation::FederationDkgCeremonyId::from_bytes([55; 32]),
        sender,
        receiver,
        BTreeMap::from([(
            Amount::from(1),
            evaluate_secret_key_coefficients(
                receiver.to_bls_signer_id(),
                &[
                    BlsSecretKey::from_reduced_bytes(&[seed; 32]),
                    BlsSecretKey::from_reduced_bytes(&[seed + 10; 32]),
                ],
            )
            .expect("dkg secret share"),
        )]),
    )
}

fn signed_dkg_commitment_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgCommitmentRequest,
) -> FederationSignedRequest<FederationDkgCommitmentRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgCommitment,
        request,
        private_config,
    )
    .expect("signed DKG commitment request")
}

fn signed_dkg_reveal_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgRevealRequest,
) -> FederationSignedRequest<FederationDkgRevealRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgReveal,
        request,
        private_config,
    )
    .expect("signed DKG reveal request")
}

fn signed_dkg_secret_share_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgSecretShareRequest,
) -> FederationSignedRequest<FederationDkgSecretShareRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgSecretShare,
        request,
        private_config,
    )
    .expect("signed DKG secret-share request")
}

async fn persist_complete_dkg_round(
    state: &MintState,
    public_config: &FederationConfig,
    first_private: &MemberConfigPrivate,
    second_private: &MemberConfigPrivate,
) {
    for (private_config, seed) in [(first_private, 11), (second_private, 21)] {
        let commitment =
            dkg_commitment_request(public_config, private_config.member_id, seed).commitment;
        state
            .persist_federation_dkg_commitment(&commitment, private_config.member_id, 1)
            .await
            .expect("persist DKG commitment");
        let reveal = dkg_reveal_request(public_config, private_config.member_id, seed).reveal;
        state
            .persist_federation_dkg_reveal(&reveal, private_config.member_id, 2)
            .await
            .expect("persist DKG reveal");
    }
}

fn dkg_transcript_signature_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
) -> FederationDkgTranscriptSignatureRequest {
    let transcript = public_config
        .setup_transcript
        .as_ref()
        .expect("setup transcript");
    let signature = cdk::federation::FederationDkgTranscriptSignature::sign(
        transcript.transcript_hash,
        private_config,
    )
    .expect("transcript signature");
    FederationDkgTranscriptSignatureRequest::new(
        public_config.federation_id,
        public_config.setup_authorization,
        private_config.member_id,
        transcript.transcript_hash,
        signature,
    )
}

async fn derive_and_persist_dkg_member_keyset_share(
    state: &MintState,
    public_config: &FederationConfig,
) -> FederationDkgMemberKeysetShare {
    let db = state.mint.localstore();
    let commitments = db
        .get_federation_dkg_commitments()
        .await
        .expect("DKG commitments")
        .into_iter()
        .map(|commitment| (commitment.member_id, commitment))
        .collect();
    let round = FederationDkgCommitmentRound::new(public_config, commitments)
        .expect("complete DKG commitment round");
    let reveals = db
        .get_federation_dkg_reveals()
        .await
        .expect("DKG reveals")
        .into_iter()
        .map(|reveal| (reveal.member_id, reveal))
        .collect::<BTreeMap<_, _>>();
    round
        .validate_reveals(public_config, &reveals)
        .expect("complete DKG reveals");
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let contributions = db
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("DKG secret-share contributions")
        .into_iter()
        .filter(|contribution| contribution.receiver == local_member)
        .map(|contribution| (contribution.sender, contribution))
        .collect::<BTreeMap<_, _>>();
    let keyset_share = round
        .derive_member_keyset_share(public_config, &reveals, local_member, &contributions)
        .expect("driver-derived member keyset share");
    let mut tx = db.begin_transaction().await.expect("begin keyset share");
    tx.add_federation_dkg_member_keyset_share(&keyset_share)
        .await
        .expect("persist driver-derived keyset share");
    tx.commit()
        .await
        .expect("commit driver-derived keyset share");
    keyset_share
}

async fn durable_dkg_transcript_fixture() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    FederationDkgTranscriptHash,
) {
    let (trusted_public_config, local_private, peer_private) = transport_config();
    let mut setup_public_config = trusted_public_config.clone();
    setup_public_config.keysets.clear();
    setup_public_config.setup_transcript = None;
    setup_public_config.validate().expect("setup public config");

    let mut setup_local_private = local_private.clone();
    setup_local_private.keyset_secret_shares.clear();
    setup_local_private.setup_transcript_hash = None;
    setup_local_private
        .validate_against_public(&setup_public_config)
        .expect("setup local private config");
    let mut setup_peer_private = peer_private.clone();
    setup_peer_private.keyset_secret_shares.clear();
    setup_peer_private.setup_transcript_hash = None;
    setup_peer_private
        .validate_against_public(&setup_public_config)
        .expect("setup peer private config");

    let state = federation_route_state_for_private(
        setup_public_config.clone(),
        setup_local_private.clone(),
    )
    .await;
    let local_member = setup_local_private.member_id;
    persist_complete_dkg_round(
        &state,
        &setup_public_config,
        &setup_local_private,
        &setup_peer_private,
    )
    .await;
    let local_contribution =
        dkg_secret_share_request(&setup_public_config, local_member, local_member, 11);
    state
        .persist_federation_dkg_secret_share_contribution(&local_contribution, local_member, 3)
        .await
        .expect("persist local secret-share contribution");
    let peer_contribution = dkg_secret_share_request(
        &setup_public_config,
        setup_peer_private.member_id,
        local_member,
        21,
    );
    state
        .persist_federation_dkg_secret_share_contribution(
            &peer_contribution,
            setup_peer_private.member_id,
            3,
        )
        .await
        .expect("persist peer secret-share contribution");
    let keyset_share =
        derive_and_persist_dkg_member_keyset_share(&state, &setup_public_config).await;
    let mut final_public_config = FederationConfig::new(
        setup_public_config.setup_authorization,
        setup_public_config.threshold_params,
        setup_public_config.members.clone(),
        vec![keyset_share.keyset.clone()],
    )
    .expect("final public config");
    final_public_config.payment_observation_policy =
        setup_public_config.payment_observation_policy.clone();
    let public_ceremonies = vec![keyset_share.public_ceremony];
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
        &public_ceremonies,
    )
    .expect("durable transcript hash");

    (
        state,
        setup_public_config,
        setup_local_private,
        setup_peer_private,
        transcript_hash,
    )
}

fn signed_dkg_transcript_signature_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgTranscriptSignatureRequest,
) -> FederationSignedRequest<FederationDkgTranscriptSignatureRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgTranscriptSignature,
        request,
        private_config,
    )
    .expect("signed DKG transcript signature request")
}

async fn consensus_operation_count(state: &MintState) -> u64 {
    state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .peer_status()
        .operation_count
}

fn reset_federation_runtime_journal(state: &mut MintState) {
    let runtime_config = state.mint.federation_config().expect("federation config");
    state.federation.member_mut_for_test().consensus =
        Arc::new(tokio::sync::Mutex::new(FederationConsensusRuntime::new(
            runtime_config.federation_id(),
            runtime_config.member_id(),
            0,
        )));
}

fn activate_local_test_finality(state: &mut MintState) {
    state.federation.local_finality_test = true;
}

async fn catch_up_response_with_materialized_digest(
    state: &MintState,
    consensus: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let mut response = test_catch_up_response(consensus, request);
    response.materialized_state_digest =
        materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized catch-up state digest");
    response
}

async fn aleph_bft_peer_status_with_materialized_digest(
    state: &MintState,
    public_config: &FederationConfig,
    member_id: MemberId,
    consensus: &FederationConsensusRuntime,
) -> FederationPeerStatus {
    let mut status = consensus.peer_status_for(public_config.federation_id, member_id);
    for configured_member in public_config.members.keys() {
        status
            .aleph_bft_next_unit_index_by_member
            .entry(*configured_member)
            .or_insert(0);
    }
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("peer checkpoint frontier");
    status.application_digest = Some(frontier.application_digest);
    status.latest_audited_checkpoint_id = frontier.latest_audited_checkpoint_id;
    status.materialized_state_digest = Some(
        materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized AlephBFT peer status digest"),
    );
    status
}

async fn aleph_bft_catch_up_response_with_materialized_digest(
    state: &MintState,
    public_config: &FederationConfig,
    consensus: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let mut response = aleph_bft_catch_up_response(public_config, consensus, request);
    response.materialized_state_digest =
        materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized AlephBFT catch-up state digest");
    response
}

fn finalize_test_operation(
    consensus: &mut FederationConsensusRuntime,
    envelope: FederationOperationEnvelope,
) -> Result<FederationSubmitOperationResponse, FederationError> {
    let status = consensus.status();
    let unit_index = consensus
        .finalized_items()
        .last()
        .map_or(0, |item| item.unit_index.saturating_add(1));
    let unit = FederationAlephBftUnitPayload::new(
        status.federation_id,
        status.local_member,
        status.session,
        unit_index,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("test AlephBFT finalized unit");
    let mut responses = consensus.finalize_unit(unit)?;
    Ok(responses
        .pop()
        .expect("one finalized response for one test operation"))
}

fn finalize_test_entry(
    consensus: &mut FederationConsensusRuntime,
    envelope: FederationOperationEnvelope,
) -> FederationJournalEntry {
    finalize_test_operation(consensus, envelope)
        .expect("finalize test journal entry")
        .outcome
        .entry()
        .clone()
}

async fn persist_test_journal_entry(state: &MintState, entry: &FederationJournalEntry) {
    let runtime_config = state.mint.federation_config().expect("federation config");
    let db = state.mint.localstore();
    let retained_items = db
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("load test AlephBFT finalized items");
    let mut adapter = FederationConsensusRuntime::recover_finalized_items_for_test(
        runtime_config.public_config(),
        runtime_config.member_id(),
        0,
        retained_items.clone(),
    )
    .expect("recover test AlephBFT history");
    let unit = FederationAlephBftUnitPayload::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        0,
        retained_items.len() as u64,
        vec![FederationConsensusItem::operation(entry.envelope.clone())],
    )
    .expect("test AlephBFT finalized unit");
    adapter
        .finalize_unit(unit)
        .expect("finalize test journal entry");
    let finalized_entry = adapter
        .journal()
        .entries()
        .last()
        .expect("finalized test journal entry");
    assert_eq!(finalized_entry.operation_id, entry.operation_id);
    let new_items = &adapter.finalized_items()[retained_items.len()..];
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin journal transaction");
    tx.add_federation_aleph_bft_finalized_items(new_items)
        .await
        .expect("persist test AlephBFT finalized item");
    tx.add_federation_journal_entry(finalized_entry)
        .await
        .expect("persist journal entry");
    tx.commit().await.expect("commit journal entry");
}

async fn persist_finalized_aleph_bft_history(
    state: &MintState,
    adapter: &FederationConsensusRuntime,
) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin finalized AlephBFT history transaction");
    tx.add_federation_aleph_bft_finalized_items(adapter.finalized_items())
        .await
        .expect("persist finalized AlephBFT history");
    for entry in adapter.journal().entries() {
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist finalized AlephBFT journal entry");
    }
    tx.commit()
        .await
        .expect("commit finalized AlephBFT history");
}

async fn add_blinded_message_conflict(
    state: &MintState,
    quote_id: Option<&QuoteId>,
    output: &BlindedMessage,
) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin conflict transaction");
    tx.add_blinded_messages(
        quote_id,
        std::slice::from_ref(output),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .expect("persist conflicting blinded message");
    tx.commit().await.expect("commit conflict transaction");
}

async fn remove_blinded_message_conflict(state: &MintState, output: &BlindedMessage) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin conflict cleanup transaction");
    tx.delete_blinded_messages(&[output.blinded_secret])
        .await
        .expect("remove conflicting blinded message");
    tx.commit().await.expect("commit conflict cleanup");
}

fn payment_observation_envelope(federation_id: FederationId) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id,
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote: "quote-paid".to_string(),
            observer: MemberId::new(1).expect("member id"),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(21),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some("fake-wallet-paid".to_string()),
        }),
    )
    .authorize(&private_member(1))
    .expect("authorize payment observation")
}

fn oversized_payment_observation_envelope(
    federation_id: FederationId,
) -> FederationOperationEnvelope {
    let mut envelope = payment_observation_envelope(federation_id);
    let FederationOperation::MintQuotePayment(observation) = &mut envelope.operation else {
        panic!("mint quote payment observation");
    };
    observation.payment_proof =
        Some("x".repeat(cdk::federation::FEDERATION_MAX_OPERATION_ENVELOPE_BYTES));
    envelope
        .authorize(&private_member(1))
        .expect("reauthorize oversized payment observation")
}

fn mint_quote_envelope(state: &MintState, quote_id: &QuoteId) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.mint_quote_operation(
        quote_id.clone(),
        cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(1),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }),
        cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: quote_id.to_string(),
            request: format!("invoice-{quote_id}"),
            amount: Some(Amount::from(1)),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state: MintQuoteState::Unpaid,
            expiry: Some(10_000),
            pubkey: None,
        }),
    )
}

fn invalid_mint_quote_transcript_envelope(
    state: &MintState,
    quote_id: &QuoteId,
) -> FederationOperationEnvelope {
    let mut envelope = mint_quote_envelope(state, quote_id);
    let FederationOperation::MintQuote(operation) = &mut envelope.operation else {
        panic!("expected mint quote operation");
    };
    if let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response {
        response.quote = QuoteId::new().to_string();
    }
    envelope
        .authorize(
            state
                .mint
                .federation_config()
                .expect("federation config")
                .private_config(),
        )
        .expect("authorize invalid mint quote transcript")
}

fn non_v3_output_for_state(state: &MintState, label: &[u8]) -> BlindedMessage {
    let config = state.mint.federation_config().expect("federation config");
    let keyset = &config.public_config().keysets[0];
    BlindedMessage::new(
        Amount::from(1),
        Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        ),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(label)),
    )
}

fn invalid_mint_operation_keyset_envelope(state: &MintState) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.operation_envelope(FederationOperation::Mint(
        cdk::federation::FederationMintOperation {
            request: cdk::federation::FederationMintRequest::Single(MintRequest {
                quote: QuoteId::new().to_string(),
                outputs: vec![non_v3_output_for_state(state, b"invalid route mint output")],
                signature: None,
            }),
        },
    ))
}

fn paid_observation_envelope(state: &MintState, quote_id: &QuoteId) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.mint_quote_payment_observation(
        quote_id.clone(),
        PaymentMethod::Known(KnownMethod::Bolt11),
        MintQuoteState::Paid,
        Amount::from(1),
        Amount::ZERO,
        Some(format!("paid-{quote_id}")),
    )
}

fn mint_operation_envelope(
    state: &MintState,
    quote_id: &QuoteId,
    output_label: &[u8],
) -> (FederationOperationEnvelope, BlindedMessage) {
    let config = state.mint.federation_config().expect("federation config");
    let keyset = &config.public_config().keysets[0];
    let output = BlindedMessage::new(
        Amount::from(1),
        keyset.id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(output_label)),
    );
    let envelope = config.operation_envelope(FederationOperation::Mint(
        cdk::federation::FederationMintOperation {
            request: cdk::federation::FederationMintRequest::Single(MintRequest {
                quote: quote_id.to_string(),
                outputs: vec![output.clone()],
                signature: None,
            }),
        },
    ));

    (envelope, output)
}

fn mint_payment_observation_envelope_for_member(
    state: &MintState,
    quote_id: &QuoteId,
    observer: u16,
    amount_paid: u64,
    payment_proof: &str,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    FederationOperationEnvelope::new(
        config.federation_id(),
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote: quote_id.to_string(),
            observer: member_id(observer),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(amount_paid),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some(payment_proof.to_string()),
        }),
    )
    .authorize(&private_member(observer))
    .expect("authorize member payment observation")
}

fn mint_output(state: &MintState, quote_id: &QuoteId) -> BlindedMessage {
    let config = state.mint.federation_config().expect("federation config");
    BlindedMessage::new(
        Amount::from(1),
        config.public_config().keysets[0].id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(
            format!("federation replay output {quote_id}").as_bytes(),
        )),
    )
}

fn aggregate_secret_for_amount(amount: u64) -> BlsSecretKey {
    match amount {
        1 => BlsSecretKey::from_reduced_bytes(&[11; 32]),
        _ => panic!("missing aggregate secret fixture for amount {amount}"),
    }
}

fn proof_for_amount(state: &MintState, amount: u64, label: &str) -> cdk::nuts::Proof {
    proof_for_amount_with_aggregate_secret(
        state,
        amount,
        label,
        &aggregate_secret_for_amount(amount),
    )
}

fn proof_for_amount_with_aggregate_secret(
    state: &MintState,
    amount: u64,
    label: &str,
    aggregate_secret: &BlsSecretKey,
) -> cdk::nuts::Proof {
    let config = state.mint.federation_config().expect("federation config");
    let keyset = &config.public_config().keysets[0];
    let secret = cdk::secret::Secret::new(format!("federation replay proof {label}"));
    let signature = BlsG1PublicKey::hash_to_curve(secret.as_bytes()).mul(aggregate_secret);

    cdk::nuts::Proof::new(
        Amount::from(amount),
        keyset.id,
        secret,
        PublicKey::from(signature),
    )
}

async fn proof_from_accepted_output(
    states: &[MintState],
    signature_threshold: usize,
    operation_id: FederationOperationId,
    output_index: u32,
    output: &BlindedMessage,
    secret: cdk::secret::Secret,
) -> cdk::nuts::Proof {
    let mut shares = Vec::with_capacity(signature_threshold);
    for state in states.iter().take(signature_threshold) {
        let records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, output_index)
            .await
            .expect("accepted output signature share lookup");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].blinded_secret, output.blinded_secret);
        assert!(records[0].accepted_operation_index.is_some());
        shares.push(records[0].share);
    }
    let signature = aggregate_blind_signature_shares(signature_threshold, &shares)
        .expect("accepted output aggregate signature");

    cdk::nuts::Proof::new(
        output.amount,
        output.keyset_id,
        secret,
        PublicKey::from(signature),
    )
}

fn swap_output(state: &MintState, label: &str) -> BlindedMessage {
    let config = state.mint.federation_config().expect("federation config");
    BlindedMessage::new(
        Amount::from(1),
        config.public_config().keysets[0].id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(
            format!("federation replay swap output {label}").as_bytes(),
        )),
    )
}

fn test_melt_quote_operation(
    config: &cdk::mint::FederationRuntimeConfig,
    quote: QuoteId,
    request: cdk::MeltQuoteRequest,
) -> FederationOperationEnvelope {
    let cdk::MeltQuoteRequest::Bolt11(bolt11) = &request else {
        panic!("federation route tests only construct bolt11 melt quotes");
    };
    let response = cdk::MeltQuoteCreateResponse::Bolt11(MeltQuoteBolt11Response {
        quote: quote.clone(),
        amount: Amount::from(
            bolt11
                .request
                .amount_milli_satoshis()
                .unwrap_or(1_000)
                .div_ceil(1_000),
        ),
        fee_reserve: Amount::from(1),
        state: MeltQuoteState::Unpaid,
        expiry: 4_000_000_000,
        payment_preimage: None,
        change: None,
        request: Some(bolt11.request.to_string()),
        unit: Some(bolt11.unit.clone()),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
    });
    config.melt_quote_operation(
        quote,
        request,
        response,
        FederationMeltQuoteMetadata::default(),
    )
}

fn test_melt_quote_operation_with_lookup_id(
    config: &cdk::mint::FederationRuntimeConfig,
    quote: QuoteId,
    request: cdk::MeltQuoteRequest,
    request_lookup_id: PaymentIdentifier,
) -> FederationOperationEnvelope {
    let envelope = test_melt_quote_operation(config, quote, request);
    let FederationOperation::MeltQuote(mut operation) = envelope.operation else {
        unreachable!("test melt quote helper always creates a melt quote operation");
    };
    operation.metadata.request_lookup_id = Some(request_lookup_id);
    config.operation_envelope(FederationOperation::MeltQuote(operation))
}

async fn melt_quote(state: &MintState, amount_msat: u64) -> QuoteId {
    melt_quote_with_description(state, amount_msat, "".to_string()).await
}

async fn melt_quote_with_description(
    state: &MintState,
    amount_msat: u64,
    description: String,
) -> QuoteId {
    let quote_id = QuoteId::new();
    let request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: create_fake_invoice(amount_msat, description),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let config = state.mint.federation_config().expect("federation config");
    let envelope = test_melt_quote_operation(config, quote_id.clone(), request);
    state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit melt quote through federation consensus");
    quote_id
}

fn melt_payment_observation_envelope(
    state: &MintState,
    quote_id: &QuoteId,
    state_observed: MeltQuoteState,
    payment_proof: Option<String>,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.operation_envelope(FederationOperation::MeltQuotePayment(
        FederationMeltQuotePaymentObservation {
            quote: quote_id.to_string(),
            observer: config.member_id(),
            method: PaymentMethod::BOLT11,
            state: state_observed,
            amount: Amount::from(1),
            fee_reserve: Amount::from(1),
            total_spent: match state_observed {
                MeltQuoteState::Paid => Some(Amount::from(2)),
                _ => None,
            },
            payment_proof,
            payment_id: None,
        },
    ))
}

async fn submit_melt_payment_observation_from_backend(
    state: &MintState,
    quote_id: &QuoteId,
) -> FederationPaymentObservationRecord {
    let observer = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let observed = state
                .federation
                .payment_observer
                .observe_melt_quote(state, quote_id)
                .await
                .expect("probe and submit melt payment observation");
            if observed {
                return state
                    .mint
                    .localstore()
                    .get_federation_payment_observations(
                        FederationPaymentObservationContext::MeltQuote,
                        &quote_id.to_string(),
                    )
                    .await
                    .expect("melt payment observation lookup")
                    .into_iter()
                    .find(|record| record.observation.observer() == observer)
                    .expect("observer payment record");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("melt payment observation timeout")
}

fn matching_melt_payment_observation_envelope(
    state: &MintState,
    evidence: &FederationPaymentObservation,
) -> FederationOperationEnvelope {
    let FederationPaymentObservation::MeltQuote(evidence) = evidence else {
        panic!("expected melt payment evidence");
    };
    let config = state.mint.federation_config().expect("federation config");
    config.operation_envelope(FederationOperation::MeltQuotePayment(
        FederationMeltQuotePaymentObservation {
            observer: config.member_id(),
            ..evidence.clone()
        },
    ))
}

fn test_operation_envelope(
    federation_id: FederationId,
    label: &str,
) -> FederationOperationEnvelope {
    let hash = BlsG1PublicKey::hash_to_curve(label.as_bytes()).to_bytes();
    let mut quote_bytes = [0u8; 16];
    quote_bytes.copy_from_slice(&hash[..16]);
    let quote = QuoteId::from(uuid::Uuid::from_bytes(quote_bytes)).to_string();
    FederationOperationEnvelope::new(
        federation_id,
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote,
            observer: member_id(1),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(1),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some(label.to_string()),
        }),
    )
    .authorize(&private_member(1))
    .expect("authorize test operation")
}

fn test_operation_envelope_for_state(
    state: &MintState,
    label: &str,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    test_operation_envelope_for_config(config.public_config(), label)
}

fn test_operation_envelope_for_config(
    config: &FederationConfig,
    label: &str,
) -> FederationOperationEnvelope {
    let hash = BlsG1PublicKey::hash_to_curve(label.as_bytes()).to_bytes();
    let mut quote_bytes = [0u8; 16];
    quote_bytes.copy_from_slice(&hash[..16]);
    let quote_id = QuoteId::from(uuid::Uuid::from_bytes(quote_bytes));
    let quote = quote_id.to_string();
    FederationOperationEnvelope::new(
        config.federation_id,
        FederationOperation::MintQuote(FederationMintQuoteOperation {
            quote: quote.clone(),
            request_lookup_id: PaymentIdentifier::QuoteId(quote_id),
            request: cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(1),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }),
            response: cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                quote,
                request: format!("invoice-{label}"),
                amount: Some(Amount::from(1)),
                unit: Some(CurrencyUnit::Sat),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                amount_paid: Amount::ZERO,
                amount_issued: Amount::ZERO,
                updated_at: 0,
                state: MintQuoteState::Unpaid,
                expiry: Some(10_000),
                pubkey: None,
            }),
        }),
    )
    .authorize(&private_member(1))
    .expect("authorize valid test operation")
}

fn restore_request_for_config(config: &FederationConfig, label: &str) -> RestoreRequest {
    RestoreRequest {
        outputs: vec![cdk::nuts::BlindedMessage::new(
            Amount::ZERO,
            config.keysets[0].id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(label.as_bytes())),
        )],
    }
}

mod aleph_network;
mod aleph_restart;
mod aleph_runtime;
mod aleph_session;
mod aleph_value_flow;
mod catch_up;
mod checkpoint;
mod finalization;
#[cfg(feature = "iroh")]
mod iroh_transport;
mod operation_results;
mod operations;
mod peer_client;
mod recovery;
mod status;

async fn queue_local_aleph_bft_unit(
    state: &MintState,
    envelope: FederationOperationEnvelope,
) -> FederationAlephBftUnitPayload {
    let consensus = state.federation.consensus().expect("consensus");
    let mut consensus = consensus.lock().await;
    consensus
        .submit_aleph_bft_envelope(envelope)
        .expect("queue AlephBFT envelope");
    consensus
        .next_aleph_bft_unit_payload()
        .expect("drain AlephBFT unit")
        .expect("queued unit")
}

async fn wait_for_local_aleph_bft_unit(state: &MintState) -> FederationAlephBftUnitPayload {
    for _ in 0..50 {
        let maybe_unit = {
            let consensus = state.federation.consensus().expect("consensus");
            let mut consensus = consensus.lock().await;
            consensus
                .next_aleph_bft_unit_payload()
                .expect("drain AlephBFT unit")
        };
        if let Some(unit) = maybe_unit {
            return unit;
        }
        tokio::task::yield_now().await;
    }

    panic!("AlephBFT unit was not queued");
}

fn assert_pending_finalization_error(
    err: &cdk::Error,
    operation_id: FederationOperationId,
    timeout_ms: u64,
) {
    assert!(
        !err.is_definitive_failure(),
        "pending finalization must remain retryable: {err}"
    );
    match err {
        cdk::Error::Federation(FederationError::OperationPendingFinalization {
            operation_id: found_operation_id,
            timeout_ms: found_timeout_ms,
        }) => {
            assert_eq!(found_operation_id, &operation_id);
            assert_eq!(*found_timeout_ms, timeout_ms);
        }
        _ => panic!("unexpected error: {err}"),
    }
}

async fn submit_latency_probe_mint_quote(state: &MintState) -> FederationOperationId {
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = state.clone();
    let response = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    })
    .await
    .expect("submit task")
    .expect("live AlephBFT operation response");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );

    operation_id
}

async fn submit_duplicate_latency_probe_mint_quote(state: &MintState) -> FederationOperationId {
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let first_state = state.clone();
    let first_envelope = envelope.clone();
    let first = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_to_consensus(first_envelope)
            .await
    });
    let second_state = state.clone();
    let second = tokio::spawn(async move {
        second_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });

    let (first_response, second_response) = tokio::join!(first, second);
    let first_response = first_response
        .expect("first submit task")
        .expect("first live AlephBFT operation response");
    let second_response = second_response
        .expect("second submit task")
        .expect("second live AlephBFT operation response");

    for response in [first_response, second_response] {
        assert_eq!(response.operation_id, operation_id);
        assert_eq!(
            response.outcome.entry().status(),
            FederationOperationStatus::Applied
        );
    }

    operation_id
}

async fn wait_for_latency_stage(
    recorder: &CdkAlephBftLatencyRecorder,
    operation_id: FederationOperationId,
    stage: CdkAlephBftLatencyStage,
) {
    time::timeout(
        Duration::from_secs(5),
        recorder.wait_for_stage(operation_id, stage),
    )
    .await
    .expect("AlephBFT latency stage was not recorded before timeout")
}

fn assert_latency_stage_order(
    events: &[CdkAlephBftLatencyEvent],
    expected: &[CdkAlephBftLatencyStage],
) {
    let mut cursor = 0usize;
    for event in events {
        if event.stage == expected[cursor] {
            cursor += 1;
            if cursor == expected.len() {
                return;
            }
        }
    }

    panic!(
        "AlephBFT latency stages were not observed in order; expected {:?}, got {}",
        expected,
        format_latency_events(events)
    );
}

fn latency_stage_delta(
    events: &[CdkAlephBftLatencyEvent],
    start: CdkAlephBftLatencyStage,
    end: CdkAlephBftLatencyStage,
) -> Option<Duration> {
    let start_elapsed = events
        .iter()
        .find(|event| event.stage == start)
        .map(|event| event.elapsed)?;
    let end_elapsed = events
        .iter()
        .find(|event| event.stage == end)
        .map(|event| event.elapsed)?;

    end_elapsed.checked_sub(start_elapsed)
}

fn latency_stage_count(
    events: &[CdkAlephBftLatencyEvent],
    stage: CdkAlephBftLatencyStage,
) -> usize {
    events.iter().filter(|event| event.stage == stage).count()
}

fn format_latency_events(events: &[CdkAlephBftLatencyEvent]) -> String {
    events
        .iter()
        .map(|event| format!("{:?}@{}ms", event.stage, event.elapsed.as_millis()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn fast_aleph_bft_live_session_options(
    pump_config: CdkAlephBftNetworkPumpConfig,
    max_round: u16,
) -> FederationAlephBftLiveSessionOptions {
    let mut delay_config = default_delay_config();
    delay_config.tick_interval = Duration::from_millis(5);
    delay_config.unit_rebroadcast_interval_min = Duration::from_millis(20);
    delay_config.unit_rebroadcast_interval_max = Duration::from_millis(40);
    delay_config.unit_creation_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.coord_request_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.parent_request_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.newest_request_delay = Arc::new(|_| Duration::from_millis(10));

    FederationAlephBftLiveSessionOptions::new(pump_config)
        .with_max_round(max_round)
        .with_delay_config(delay_config)
}

fn fast_aleph_bft_live_session_options_with_idle_tail(
    pump_config: CdkAlephBftNetworkPumpConfig,
    max_round: u16,
) -> FederationAlephBftLiveSessionOptions {
    let mut delay_config = default_delay_config();
    delay_config.tick_interval = Duration::from_millis(5);
    delay_config.unit_rebroadcast_interval_min = Duration::from_millis(20);
    delay_config.unit_rebroadcast_interval_max = Duration::from_millis(40);
    delay_config.unit_creation_delay = Arc::new(|round| {
        if round > 16 {
            Duration::from_millis(250)
        } else {
            Duration::from_millis(10)
        }
    });
    delay_config.coord_request_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.parent_request_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.newest_request_delay = Arc::new(|_| Duration::from_millis(10));

    FederationAlephBftLiveSessionOptions::new(pump_config)
        .with_max_round(max_round)
        .with_delay_config(delay_config)
}

async fn wait_for_aleph_bft_session(state: &MintState, session_id: u64) {
    time::timeout(Duration::from_secs(10), async {
        loop {
            let status = state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status");
            if status.session == session_id {
                return;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT session did not advance before timeout")
}

async fn operation_applied_within(
    state: &MintState,
    operation_id: FederationOperationId,
    timeout: Duration,
) -> bool {
    operation_terminal_within(state, operation_id, timeout).await
        == Some(FederationOperationStatus::Applied)
}

async fn operation_terminal_within(
    state: &MintState,
    operation_id: FederationOperationId,
    timeout: Duration,
) -> Option<FederationOperationStatus> {
    if let Some(entry) = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(operation_id)
        .await
        .expect("operation status lookup")
    {
        if matches!(
            entry.status(),
            FederationOperationStatus::Applied | FederationOperationStatus::Rejected
        ) {
            return Some(entry.status());
        }
    }

    let response = time::timeout(
        timeout,
        state.wait_for_federation_operation_acceptance(operation_id),
    )
    .await
    .ok()
    .flatten()?;
    let status = response.outcome.entry().status();
    matches!(
        status,
        FederationOperationStatus::Applied | FederationOperationStatus::Rejected
    )
    .then_some(status)
}

async fn wait_for_aleph_bft_pending_operations(
    state: &MintState,
    session_id: u64,
) -> Vec<FederationAlephBftPendingOperationRecord> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let pending = state
                .mint
                .localstore()
                .get_federation_aleph_bft_pending_operations(session_id)
                .await
                .expect("AlephBFT pending operation lookup");
            if !pending.is_empty() {
                return pending;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT pending operations were not persisted")
}

async fn wait_for_aleph_bft_network_message_request(
    transport: &RouterFederationTransportRegistry,
    sender: MemberId,
    receiver: MemberId,
) -> FederationSignedRequest<FederationAlephBftNetworkMessageRequest> {
    time::timeout(
        Duration::from_secs(5),
        transport.wait_for_aleph_bft_network_message_request_matching(|request| {
            request.sender == sender && request.receiver == receiver
        }),
    )
    .await
    .expect("AlephBFT network message request was not captured")
}

async fn wait_for_aleph_bft_unit_with_operation(
    transport: &RouterFederationTransportRegistry,
    sender: MemberId,
    operation_id: FederationOperationId,
) -> Option<u64> {
    let request = time::timeout(
        Duration::from_secs(10),
        transport.wait_for_aleph_bft_network_message_request_matching(|request| {
            request.sender == sender
                && aleph_bft_network_request_contains_operation(request, operation_id)
        }),
    )
    .await
    .ok()?;
    aleph_bft_network_request_operation_unit_index(&request, operation_id)
}

fn aleph_bft_network_request_contains_operation(
    request: &FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
    operation_id: FederationOperationId,
) -> bool {
    aleph_bft_network_request_operation_unit_index(request, operation_id).is_some()
}

fn aleph_bft_network_request_operation_unit_index(
    request: &FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
    operation_id: FederationOperationId,
) -> Option<u64> {
    let bytes = request.payload.message_bytes().ok()?;
    let mut input = &bytes[..];
    let message = CdkAlephBftNetworkData::decode(&mut input).ok()?;
    if !input.is_empty() {
        return None;
    }
    message.included_data().iter().find_map(|data| {
        let payload = data.payload();
        payload
            .operation_ids
            .contains(&operation_id)
            .then_some(payload.unit_index)
    })
}

async fn wait_for_aleph_bft_backup_bytes(state: &MintState, session_id: u64) -> Vec<u8> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let backup_bytes = state
                .mint
                .localstore()
                .get_federation_aleph_bft_backup_bytes(session_id)
                .await
                .expect("AlephBFT backup byte lookup")
                .unwrap_or_default();
            if !backup_bytes.is_empty() {
                return backup_bytes;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT backup bytes were not persisted")
}

async fn seed_aleph_bft_backup(state: &MintState, session_id: u64, backup_bytes: &[u8]) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin AlephBFT backup seed transaction");
    tx.append_federation_aleph_bft_backup_bytes(session_id, backup_bytes)
        .await
        .expect("seed AlephBFT backup bytes");
    tx.set_federation_aleph_bft_active_session(session_id)
        .await
        .expect("seed AlephBFT active session");
    tx.commit()
        .await
        .expect("commit AlephBFT backup seed transaction");
}

async fn wait_for_aleph_bft_backup_bytes_longer_than(
    state: &MintState,
    session_id: u64,
    previous_len: usize,
) -> Vec<u8> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let backup_bytes = state
                .mint
                .localstore()
                .get_federation_aleph_bft_backup_bytes(session_id)
                .await
                .expect("AlephBFT backup byte lookup")
                .unwrap_or_default();
            if backup_bytes.len() > previous_len {
                return backup_bytes;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT backup bytes did not grow")
}

#[cfg(feature = "prometheus")]
fn synthetic_conflicting_mint_quote_envelope(
    base: &FederationOperationEnvelope,
    private_config: &MemberConfigPrivate,
    operation_index: u64,
) -> FederationOperationEnvelope {
    let mut envelope = base.clone();
    let FederationOperation::MintQuote(operation) = &mut envelope.operation else {
        panic!("synthetic history base must be a mint quote");
    };
    let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response else {
        panic!("synthetic history base must use a bolt11 mint quote");
    };
    response
        .request
        .push_str(&format!("-synthetic-conflict-{operation_index}"));
    envelope
        .authorize(private_config)
        .expect("authorize synthetic conflicting operation")
}

#[cfg(feature = "prometheus")]
async fn reset_synthetic_history_consensus(
    state: &mut MintState,
    public_config: &FederationConfig,
    base_entry: &FederationJournalEntry,
) {
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("synthetic history checkpoint frontier");
    let (status, next_unit_index_by_member) = {
        let consensus = state
            .federation
            .consensus()
            .expect("synthetic history consensus")
            .lock()
            .await;
        (
            consensus.status(),
            consensus.next_unit_index_by_member().clone(),
        )
    };
    let boundary = FederationCheckpointBoundary::from_complete_unit(
        &frontier,
        status.session,
        status.next_unit_finalization_index,
        next_unit_index_by_member,
    )
    .expect("synthetic history complete-unit boundary");
    let checkpoint = boundary
        .recovery_checkpoint(
            public_config,
            None,
            FederationMaterializedStateDigest::genesis(),
        )
        .expect("synthetic history transient checkpoint");
    let mut consensus = FederationConsensusRuntime::checkpoint_base_for_test(
        public_config,
        status.local_member,
        &checkpoint,
    )
    .expect("reset synthetic history consensus");
    consensus
        .register_historical_journal_entry(base_entry.clone())
        .expect("register synthetic history conflict anchor");
    state.federation.member_mut_for_test().consensus = Arc::new(tokio::sync::Mutex::new(consensus));
}

#[cfg(feature = "prometheus")]
async fn append_synthetic_history_conflicts(
    state: &mut MintState,
    public_config: &FederationConfig,
    local_private: &MemberConfigPrivate,
    base_envelope: &FederationOperationEnvelope,
    base_entry: &FederationJournalEntry,
    mut next_operation_index: u64,
    operation_count: u64,
) {
    while next_operation_index < operation_count {
        let batch_len = usize::try_from(
            operation_count
                .saturating_sub(next_operation_index)
                .min(cdk::federation::FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS as u64),
        )
        .expect("synthetic batch length fits in usize");
        let items = (0..batch_len)
            .map(|offset| {
                let operation_index = next_operation_index.saturating_add(
                    u64::try_from(offset).expect("synthetic batch offset fits in u64"),
                );
                FederationConsensusItem::operation(synthetic_conflicting_mint_quote_envelope(
                    base_envelope,
                    local_private,
                    operation_index,
                ))
            })
            .collect::<Vec<_>>();
        let (finalized_items, entries) = {
            let mut consensus = state
                .federation
                .consensus()
                .expect("synthetic history consensus")
                .lock()
                .await;
            let status = consensus.status();
            let payload = FederationAlephBftUnitPayload::new(
                status.federation_id,
                status.local_member,
                status.session,
                consensus.next_unit_index(),
                items,
            )
            .expect("synthetic history finalized payload");
            let finalized_start = consensus.finalized_items().len();
            let responses = consensus
                .finalize_aleph_bft_unit(payload)
                .expect("finalize synthetic history batch");
            let finalized_items = consensus.finalized_items()[finalized_start..].to_vec();
            let entries = responses
                .into_iter()
                .map(|response| match response.outcome {
                    FederationJournalAppendOutcome::Appended(entry) => entry,
                    FederationJournalAppendOutcome::Duplicate(_) => {
                        panic!("synthetic history operation must be unique")
                    }
                })
                .collect::<Vec<_>>();
            (finalized_items, entries)
        };
        assert_eq!(finalized_items.len(), batch_len);
        assert_eq!(entries.len(), batch_len);
        assert!(entries.iter().all(|entry| {
            matches!(
                entry.application,
                FederationApplicationState::ConsensusRejected {}
            )
        }));
        state
            .federation
            .persist_finalization(&finalized_items, &entries)
            .await
            .expect("persist synthetic history batch");
        next_operation_index = next_operation_index
            .saturating_add(u64::try_from(batch_len).expect("synthetic batch length fits in u64"));
        reset_synthetic_history_consensus(state, public_config, base_entry).await;
    }
}

#[cfg(feature = "prometheus")]
async fn build_synthetic_checkpoint_history(
    state: &mut MintState,
    public_config: &FederationConfig,
    local_private: &MemberConfigPrivate,
    peer_private: &MemberConfigPrivate,
    operation_count: u64,
) -> FederationCheckpoint {
    assert!(operation_count > 0, "synthetic history must be non-empty");
    let quote_id = QuoteId::new();
    let base_envelope = mint_quote_envelope(state, &quote_id);
    state
        .submit_federation_envelope(base_envelope.clone())
        .await
        .expect("apply synthetic history conflict anchor");
    let base_entry = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(
            base_envelope.operation_id().expect("base operation id"),
        )
        .await
        .expect("load synthetic history conflict anchor")
        .expect("synthetic history conflict anchor exists");

    append_synthetic_history_conflicts(
        state,
        public_config,
        local_private,
        &base_envelope,
        &base_entry,
        1,
        operation_count,
    )
    .await;

    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("completed synthetic history frontier");
    assert_eq!(frontier.journal_next_index, operation_count);
    assert_eq!(frontier.application_next_index, operation_count);
    assert_eq!(frontier.finalized_next_item_index, operation_count);

    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        state,
        public_config,
        None,
        &[local_private, peer_private],
    )
    .await;
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin synthetic checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(public_config)
            .expect("verify synthetic checkpoint quorum"),
    )
    .await
    .expect("promote synthetic checkpoint");
    tx.commit()
        .await
        .expect("commit synthetic checkpoint promotion");
    checkpoint
}

#[cfg(feature = "prometheus")]
fn checkpoint_work_delta(member: &str, work: &str, before: f64) -> f64 {
    prometheus_metric_value(
        "cdk_federation_checkpoint_work_total",
        &[("member", member), ("work", work)],
    ) - before
}

async fn seed_synthetic_active_session_backup(
    state: &MintState,
    session: u64,
    chunk_count: usize,
    bytes_per_chunk: usize,
) {
    let chunks = (0..chunk_count)
        .map(|index| {
            vec![u8::try_from(index.saturating_add(1)).unwrap_or(u8::MAX); bytes_per_chunk]
        })
        .collect::<Vec<_>>();
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin synthetic backup transaction");
    tx.replace_federation_aleph_bft_backup_chunks(session, &chunks)
        .await
        .expect("persist synthetic backup chunks");
    tx.set_federation_aleph_bft_active_session(session)
        .await
        .expect("persist synthetic active session");
    tx.commit().await.expect("commit synthetic backup");
}

async fn wait_for_applied_operation(state: &MintState, operation_id: FederationOperationId) {
    assert!(
        operation_applied_within(state, operation_id, Duration::from_secs(15)).await,
        "operation applied before timeout"
    );
}

fn journal_convergence_projection(
    entries: &[FederationJournalEntry],
) -> Vec<(
    u64,
    FederationOperationId,
    FederationJournalDigest,
    FederationJournalDigest,
    FederationOperationStatus,
)> {
    entries
        .iter()
        .map(|entry| {
            (
                entry.index,
                entry.operation_id,
                entry.previous_order_digest,
                entry.order_digest,
                entry.status(),
            )
        })
        .collect()
}

async fn wait_for_terminal_operation(
    state: &MintState,
    operation_id: FederationOperationId,
) -> FederationOperationStatus {
    operation_terminal_within(state, operation_id, Duration::from_secs(30))
        .await
        .expect("operation reached terminal status before timeout")
}

async fn replay_aleph_bft_runtime_after_test_process_restart(state: &mut MintState) {
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("replay AlephBFT runtime after test process restart");
    state.federation.member_mut_for_test().consensus =
        replayed_consensus.expect("configured member handle");
    state.federation.member_mut_for_test().sequences =
        replayed_sequences.expect("configured member handle");
}

async fn submit_live_operation_allowing_pending_timeout(
    state: &MintState,
    envelope: FederationOperationEnvelope,
) -> FederationOperationId {
    let operation_id = envelope.operation_id().expect("live operation id");
    match state
        .submit_federation_envelope_to_consensus(envelope)
        .await
    {
        Ok(response) => assert_eq!(response.operation_id, operation_id),
        Err(err) => assert_pending_finalization_error(&err, operation_id, 10_000),
    }

    operation_id
}

async fn wait_for_applied_operation_with_context(
    state: &MintState,
    operation_id: FederationOperationId,
    context: &str,
) {
    if operation_applied_within(state, operation_id, Duration::from_secs(15)).await {
        return;
    }

    let member_id = state
        .mint
        .federation_config()
        .expect("contextual apply wait federation config")
        .member_id();
    let status = state.federation_aleph_bft_status().await;
    let journal_operation_ids = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("contextual apply wait journal")
        .into_iter()
        .map(|entry| entry.operation_id)
        .collect::<Vec<_>>();
    let pending_operation_ids = state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(0)
        .await
        .expect("contextual apply wait pending operations")
        .into_iter()
        .map(|operation| operation.operation_id)
        .collect::<Vec<_>>();
    panic!(
        "{context}: operation {operation_id} did not apply on member {member_id} before timeout; status={status:?}; journal_operation_ids={journal_operation_ids:?}; pending_operation_ids={pending_operation_ids:?}"
    );
}

async fn sync_and_certify_partitioned_member(
    session: &MultiMemberAlephBftLiveSession,
    partitioned_index: usize,
    recovery_live_options: FederationAlephBftLiveSessionOptions,
) {
    let state = &session.states[partitioned_index];
    let public_config = &session.public_config;
    let clients = session
        .private_configs
        .iter()
        .cloned()
        .map(|private_config| {
            FederationRestPeerClient::new_with_config(
                session.transport.clone(),
                public_config,
                private_config,
                session.sync_config,
            )
            .expect("partition recovery peer-sync client")
        })
        .collect::<Vec<_>>();
    let peer_sets = session
        .private_configs
        .iter()
        .map(|private_config| {
            public_config
                .members
                .values()
                .filter(|member| member.member_id != private_config.member_id)
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let required_remote_quorum =
        usize::from(public_config.threshold_params.consensus_threshold) - 1;
    let catch_up_deadline = time::Instant::now() + Duration::from_secs(90);
    let mut maximum_synced = 0;
    let mut restarted_with_fast_test_options = false;
    loop {
        for index in 0..session.states.len() {
            let synced = run_federation_peer_sync_round(
                &session.states[index],
                &clients[index],
                &peer_sets[index],
            )
            .await;
            if index == partitioned_index {
                maximum_synced = maximum_synced.max(synced);
            }
        }
        if !restarted_with_fast_test_options
            && state.federation.live_session_active().await
            && matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::CatchingUp { .. })
            )
        {
            let local_status = local_peer_status(state)
                .await
                .expect("partition recovery local status");
            let mut peer_statuses = Vec::new();
            for peer in &peer_sets[partitioned_index] {
                peer_statuses.push(
                    clients[partitioned_index]
                        .peer_status(peer)
                        .await
                        .expect("partition recovery peer status"),
                );
            }
            let target_session = sync_loop::quorum_compatible_session(
                &local_status,
                &peer_statuses,
                usize::from(public_config.threshold_params.consensus_threshold),
            )
            .expect("partition recovery quorum-compatible session");
            let replay_target = sync_loop::quorum_supported_replay_target(
                &local_status,
                &peer_statuses,
                target_session,
                usize::from(public_config.threshold_params.consensus_threshold),
            );
            stop_current_federation_aleph_bft_session_for_catch_up(state).await;
            start_federation_aleph_bft_live_session_with_options_and_transport(
                state,
                session.sync_config,
                recovery_live_options
                    .clone()
                    .with_replay_target_next_unit_index_by_member(replay_target),
                session.transport.clone(),
            )
            .await
            .expect("restart partition recovery with fast local test options")
            .expect("partition recovery fast live session");
            restarted_with_fast_test_options = true;
        }
        match state.ensure_federation_ready_for_authoritative_read().await {
            Ok(()) => {
                assert!(
                    maximum_synced >= required_remote_quorum,
                    "catch-up certification must reach a remote BFT quorum"
                );
                return;
            }
            Err(_) if time::Instant::now() < catch_up_deadline => {
                time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => {
                let local_status = local_peer_status(state)
                    .await
                    .expect("partition recovery timeout status");
                let trusted_checkpoint = state
                    .mint
                    .localstore()
                    .get_trusted_federation_checkpoint_quorum()
                    .await
                    .expect("partition recovery timeout checkpoint")
                    .map(|quorum| quorum.checkpoint.id().expect("trusted checkpoint id"));
                panic!(
                    "partitioned member did not become ready after ordered recovery; maximum synced peers: {maximum_synced}; required remote quorum: {required_remote_quorum}; readiness error: {err}; local_status={local_status:?}; live_session_active={}; replay_ready={}; trusted_checkpoint={trusted_checkpoint:?}",
                    state.federation.live_session_active().await,
                    state.federation.live_session_replay_ready().await,
                )
            }
        }
    }
}

const LATENCY_EXPECTED_STAGES: &[CdkAlephBftLatencyStage] = &[
    CdkAlephBftLatencyStage::SubmitQueued,
    CdkAlephBftLatencyStage::DataProviderYielded,
    CdkAlephBftLatencyStage::NetworkMessageDelivered,
    CdkAlephBftLatencyStage::FinalizedDataReceived,
    CdkAlephBftLatencyStage::ApplyLoopReceived,
    CdkAlephBftLatencyStage::ApplyCompleted,
    CdkAlephBftLatencyStage::OperationNotified,
    CdkAlephBftLatencyStage::WaiterReturned,
];

struct TwoMemberAlephBftLiveSession {
    first_state: MintState,
    second_state: MintState,
    _first_handle: Arc<AlephBftSessionSupervisor>,
    _second_handle: Arc<AlephBftSessionSupervisor>,
}

struct MultiMemberAlephBftLiveSession {
    states: Vec<MintState>,
    dbs: Vec<Arc<cdk_sqlite::MintSqliteDatabase>>,
    seeds: Vec<Vec<u8>>,
    transport: RouterFederationTransportRegistry,
    federation_api_urls: Vec<MintUrl>,
    public_config: FederationConfig,
    private_configs: Vec<MemberConfigPrivate>,
    sync_config: FederationPeerSyncConfig,
    _handles: Vec<Arc<AlephBftSessionSupervisor>>,
}

async fn start_two_member_aleph_bft_live_session(
    live_options: FederationAlephBftLiveSessionOptions,
) -> Result<TwoMemberAlephBftLiveSession, cdk::Error> {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };

    let first_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &first_state,
        sync_config,
        live_options.clone(),
        transport.clone(),
    )
    .await?
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let second_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &second_state,
        sync_config,
        live_options,
        transport.clone(),
    )
    .await?
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    Ok(TwoMemberAlephBftLiveSession {
        first_state,
        second_state,
        _first_handle: first_handle,
        _second_handle: second_handle,
    })
}

async fn start_five_member_aleph_bft_live_session(
    live_options: FederationAlephBftLiveSessionOptions,
) -> Result<MultiMemberAlephBftLiveSession, cdk::Error> {
    start_multi_member_aleph_bft_live_session(5, 3, live_options).await
}

async fn start_multi_member_aleph_bft_live_session(
    member_count: u16,
    signature_threshold: u16,
    live_options: FederationAlephBftLiveSessionOptions,
) -> Result<MultiMemberAlephBftLiveSession, cdk::Error> {
    let (session, _) = start_multi_member_aleph_bft_live_session_with_payment_policy(
        member_count,
        signature_threshold,
        live_options,
        FederationPaymentObservationPolicy::development_single_observation_for_testing(),
    )
    .await?;

    Ok(session)
}

async fn start_multi_member_aleph_bft_live_session_with_payment_policy(
    member_count: u16,
    signature_threshold: u16,
    live_options: FederationAlephBftLiveSessionOptions,
    payment_observation_policy: FederationPaymentObservationPolicy,
) -> Result<(MultiMemberAlephBftLiveSession, Vec<Arc<AtomicUsize>>), cdk::Error> {
    let federation_api_urls = (1..=member_count)
        .map(|value| {
            MintUrl::from_str(&format!("http://fed{value}.test")).expect("federation api url")
        })
        .collect::<Vec<_>>();
    let (mut public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(member_count, signature_threshold)
            .expect("threshold params"),
        federation_api_urls.clone(),
    );
    public_config.payment_observation_policy = payment_observation_policy;
    public_config.validate()?;
    let mut states = Vec::new();
    let mut dbs = Vec::new();
    let mut seeds = Vec::new();
    let mut make_payment_calls = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let (payment_processor, member_make_payment_calls) =
            CountingFakeWallet::with_call_counter();
        states.push(
            federation_route_state_for_private_with_db_and_aleph_bft_payment_processor(
                public_config.clone(),
                private_config,
                Arc::clone(&db),
                &seed,
                payment_processor,
            )
            .await,
        );
        dbs.push(db);
        seeds.push(seed.to_vec());
        make_payment_calls.push(member_make_payment_calls);
    }
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    };
    let mut handles = Vec::new();
    for (index, state) in states.iter_mut().enumerate() {
        let handle = start_federation_aleph_bft_live_session_with_options_and_transport(
            state,
            sync_config,
            live_options.clone(),
            transport.clone(),
        )
        .await?
        .expect("live session handle");
        handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(state.clone()),
        );
    }

    Ok((
        MultiMemberAlephBftLiveSession {
            states,
            dbs,
            seeds,
            transport,
            federation_api_urls,
            public_config,
            private_configs,
            sync_config,
            _handles: handles,
        },
        make_payment_calls,
    ))
}

async fn assert_multi_member_journal_and_state_converged(
    session: &MultiMemberAlephBftLiveSession,
    expected_operation_ids: &[FederationOperationId],
) {
    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("expected converged journal");
    assert_eq!(
        expected_entries
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>(),
        expected_operation_ids
    );
    let expected_projection = journal_convergence_projection(&expected_entries);
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("expected converged materialized-state digest");
    for state in &session.states {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("converged journal");
        assert_eq!(
            journal_convergence_projection(&entries),
            expected_projection
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("converged materialized-state digest"),
            expected_digest
        );
    }
}

async fn run_five_member_restart_case(restarted_indexes: &[usize]) {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let live_options = fast_aleph_bft_live_session_options(pump_config, 500);
    let mut session = start_five_member_aleph_bft_live_session(live_options.clone())
        .await
        .expect("five-member restart session");

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&session.states[0], &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    session.states[0]
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("operation before restart");
    for state in &session.states {
        wait_for_applied_operation(state, first_operation_id).await;
    }
    for &index in restarted_indexes {
        wait_for_aleph_bft_backup_bytes(&session.states[index], 0).await;
        *session.states[index]
            .federation
            .aleph_bft_network_inbound
            .write()
            .await = None;
        *session.states[index]
            .federation
            .aleph_bft_live_session
            .write()
            .await = None;
        session
            .transport
            .remove(&session.federation_api_urls[index]);
    }
    for &index in restarted_indexes.iter().rev() {
        drop(session._handles.remove(index));
    }

    for &index in restarted_indexes {
        let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
            session.public_config.clone(),
            session.private_configs[index].clone(),
            Arc::clone(&session.dbs[index]),
            &session.seeds[index],
        )
        .await;
        replay_aleph_bft_runtime_after_test_process_restart(&mut state).await;
        let handle = start_federation_aleph_bft_live_session_with_options_and_transport(
            &state,
            session.sync_config,
            live_options.clone(),
            session.transport.clone(),
        )
        .await
        .expect("restart live member")
        .expect("restarted live member handle");
        session.transport.insert(
            session.federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(state.clone()),
        );
        session.states[index] = state;
        session._handles.push(handle);
    }

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&session.states[0], &second_quote_id);
    let second_operation_id = second_envelope
        .operation_id()
        .expect("post-restart operation id");
    session.states[0]
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("operation after restart");
    for state in &session.states {
        wait_for_applied_operation(state, second_operation_id).await;
    }
    assert_multi_member_journal_and_state_converged(
        &session,
        &[first_operation_id, second_operation_id],
    )
    .await;
}

struct ApplyRetryBudgetOverrideGuard;

impl ApplyRetryBudgetOverrideGuard {
    fn set(millis: u64) -> Self {
        crate::FEDERATION_APPLY_RETRY_BUDGET_OVERRIDE_MILLIS
            .store(millis, std::sync::atomic::Ordering::Relaxed);
        Self
    }
}

impl Drop for ApplyRetryBudgetOverrideGuard {
    fn drop(&mut self) {
        crate::FEDERATION_APPLY_RETRY_BUDGET_OVERRIDE_MILLIS
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Test-scale inbound channel budget used by the backpressure regression so
/// the flood stays small and deterministic.
const FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY_TEST: u64 = 128;

async fn apply_identical_finalized_unit(
    first: &MintState,
    second: &MintState,
    items: Vec<FederationConsensusItem>,
) {
    let payload = {
        let consensus = first
            .federation
            .consensus()
            .expect("first agreement consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            items,
        )
        .expect("agreement payload")
    };

    first
        .federation
        .apply_finalized_unit(payload.clone())
        .await
        .expect("apply first agreement unit");
    second
        .federation
        .apply_finalized_unit(payload)
        .await
        .expect("apply second agreement unit");
}

#[tokio::test]
async fn service_startup_failure_cancels_workers_already_started() {
    let (state, _, _) = federation_route_state().await;
    let tasks = state.federation.tasks.clone();
    let stop = tasks.stop_token();
    let invalid_sync = FederationPeerSyncConfig {
        catch_up_limit: 0,
        ..Default::default()
    };
    let services = super::services::FederationServices::new(state, Some(invalid_sync), None);
    assert!(services.start().await.is_err());
    assert!(stop.is_cancelled());
}
