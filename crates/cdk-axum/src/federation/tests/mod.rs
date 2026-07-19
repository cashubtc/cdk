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
    FederationConfigSnapshot, FederationConsensusDefaultItem, FederationConsensusItem,
    FederationConsensusItemAdmission, FederationConsensusModuleFact, FederationConsensusRuntime,
    FederationDatabaseBackupManifest, FederationDkgActivationRequest, FederationDkgCeremonyId,
    FederationDkgCeremonyRecord, FederationDkgCeremonyState, FederationDkgCommitmentRound,
    FederationDkgMemberKeysetShare, FederationDkgSetupMethod, FederationDkgTranscript,
    FederationDkgTranscriptHash, FederationDkgTranscriptSignature, FederationError,
    FederationJournalAppendOutcome, FederationJournalDigest, FederationJournalEntry,
    FederationKeysetRotationOperation, FederationMeltQuoteMetadata, FederationMintQuoteOperation,
    FederationMintQuotePaymentObservation, FederationOperation, FederationOperationEnvelope,
    FederationOperationId, FederationOperationKind, FederationOperationStatus,
    FederationPaymentObservation, FederationPaymentObservationContext,
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
use cdk_http_client::{HttpError, RawResponse};
use futures::Stream;
use parity_scale_codec::{Decode, Encode};
use serde::de::DeserializeOwned;
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

include!("foundation.rs");
include!("peer_client.rs");
include!("catch_up.rs");
include!("recovery.rs");
include!("checkpoint.rs");
include!("finalization.rs");
include!("aleph_session.rs");
include!("aleph_network.rs");
include!("aleph_value_flow.rs");
include!("aleph_restart.rs");
include!("aleph_runtime.rs");
include!("operations.rs");
include!("operation_results.rs");
include!("status.rs");
