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
    FederationAlephBftBackupRequest, FederationAlephBftBackupResponse, FederationAlephBftError,
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

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
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

mod state_transfer;

mod status;

mod fixture_aleph;
mod fixture_envelopes;
mod fixture_live_session;
mod fixture_requests;
mod fixture_state;
#[cfg(feature = "prometheus")]
mod fixture_telemetry;
mod fixture_transport;

use self::fixture_aleph::*;
use self::fixture_envelopes::*;
use self::fixture_live_session::*;
use self::fixture_requests::*;
use self::fixture_state::*;
#[cfg(feature = "prometheus")]
use self::fixture_telemetry::*;
use self::fixture_transport::*;

mod error_status;
mod recovery_barrier;
mod service_startup;
mod startup_quorum;
mod sync_config;
