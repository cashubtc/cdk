//! Private federation member REST routes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use cdk::federation::{
    FederationConfig, FederationError, FederationId, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationJournalEntry, FederationPeerStatus,
    FederationPeerStatusRequest, FederationRestMessageKind, FederationRestSequenceTracker,
    FederationSignedRequest, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    InMemoryFederationConsensus, MemberConfigPrivate, MemberConfigPublic,
    FEDERATION_REST_API_VERSION,
};
use cdk_http_client::{Async, Transport};
use serde::Serialize;
use tracing::instrument;

use crate::MintState;

type ConsensusHandle = std::sync::Arc<tokio::sync::Mutex<InMemoryFederationConsensus>>;
type SequenceHandle = std::sync::Arc<tokio::sync::Mutex<FederationRestSequenceTracker>>;

pub(crate) fn create_federation_router() -> Router<MintState> {
    Router::new()
        .route("/status", post(post_status))
        .route("/operations", post(post_operation))
        .route("/journal", post(post_journal))
}

#[instrument(skip_all)]
async fn post_status(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationPeerStatusRequest>>,
) -> Result<Json<FederationPeerStatus>, Response> {
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::Status).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let consensus = federation_consensus(&state)?;
    let consensus = consensus.lock().await;
    Ok(Json(consensus.peer_status()))
}

#[instrument(skip_all)]
async fn post_operation(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationSubmitOperationRequest>>,
) -> Result<Json<FederationSubmitOperationResponse>, Response> {
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::SubmitOperation)
            .await?;
    let response = state
        .submit_federation_envelope_with_sequence(
            payload.payload.envelope,
            Some((verified.sender, verified.sequence)),
        )
        .await
        .map_err(cdk_error_response)?;
    tracing::debug!("Accepted federation operation {}", response.operation_id);
    Ok(Json(response))
}

#[instrument(skip_all)]
async fn post_journal(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationJournalCatchUpRequest>>,
) -> Result<Json<FederationJournalCatchUpResponse>, Response> {
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::CatchUp).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let consensus = federation_consensus(&state)?;
    tracing::debug!(
        next_index = payload.payload.next_index,
        limit = ?payload.payload.limit,
        "Received federation journal catch-up request"
    );
    let consensus = consensus.lock().await;
    let response = consensus
        .catch_up_checked(&payload.payload)
        .map_err(federation_error_response)?;
    Ok(Json(response))
}

async fn verify_federation_request<T>(
    state: &MintState,
    request: &FederationSignedRequest<T>,
    expected_kind: FederationRestMessageKind,
) -> Result<VerifiedFederationRequest, Response>
where
    T: Serialize,
{
    if request.kind != expected_kind {
        return Err(federation_error_response(
            FederationError::RestMessageKindMismatch {
                expected: expected_kind,
                found: request.kind,
            },
        ));
    }

    let runtime_config = federation_runtime_config(state)?;
    let sequences = federation_sequences(state)?;
    let sequences = sequences.lock().await;
    sequences
        .verify_next(
            request,
            runtime_config.public_config(),
            runtime_config.member_id(),
        )
        .map_err(federation_error_response)?;

    Ok(VerifiedFederationRequest {
        sender: request.sender,
        sequence: request.sequence,
    })
}

#[derive(Debug, Clone, Copy)]
struct VerifiedFederationRequest {
    sender: cdk::federation::MemberId,
    sequence: u64,
}

fn federation_runtime_config(
    state: &MintState,
) -> Result<&cdk::mint::FederationRuntimeConfig, Response> {
    state
        .mint
        .federation_config()
        .ok_or_else(federation_not_configured)
}

fn federation_consensus(state: &MintState) -> Result<ConsensusHandle, Response> {
    state
        .federation_consensus
        .clone()
        .ok_or_else(federation_not_configured)
}

fn federation_sequences(state: &MintState) -> Result<SequenceHandle, Response> {
    state
        .federation_sequences
        .clone()
        .ok_or_else(federation_not_configured)
}

fn federation_not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        "mint is not configured as a federation member",
    )
        .into_response()
}

fn federation_error_response(err: FederationError) -> Response {
    let status = match err {
        FederationError::ConflictingOperation { .. } => StatusCode::CONFLICT,
        FederationError::OperationFederationMismatch { .. }
        | FederationError::CatchUpFederationMismatch { .. }
        | FederationError::CatchUpIndexMismatch { .. }
        | FederationError::CatchUpDigestMismatch { .. }
        | FederationError::CatchUpOperationIdMismatch { .. }
        | FederationError::JournalConflictKeysMismatch { .. } => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_REQUEST,
    };
    tracing::warn!("Federation request failed: {}", err);
    (status, err.to_string()).into_response()
}

fn cdk_error_response(err: cdk::Error) -> Response {
    match err {
        cdk::Error::Federation(err) => federation_error_response(err),
        err => {
            tracing::error!("Federation storage mutation failed: {}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationRestPeerClient<T = Async> {
    transport: T,
    federation_id: FederationId,
    private_config: MemberConfigPrivate,
    next_sequence: Arc<AtomicU64>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl<T> FederationRestPeerClient<T>
where
    T: Transport,
{
    pub(crate) fn new(
        transport: T,
        public_config: &FederationConfig,
        private_config: MemberConfigPrivate,
    ) -> anyhow::Result<Self> {
        private_config.validate_against_public(public_config)?;
        Ok(Self {
            transport,
            federation_id: public_config.federation_id,
            private_config,
            next_sequence: Arc::new(AtomicU64::new(0)),
        })
    }

    pub(crate) async fn peer_status(
        &self,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<FederationPeerStatus> {
        let request = self.signed_request(
            member,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
        )?;
        let status = self
            .transport
            .http_post(federation_endpoint(member, "status")?, None, &request)
            .await?;
        validate_peer_status(self.federation_id, member, &status)?;
        Ok(status)
    }

    pub(crate) async fn submit_operation(
        &self,
        member: &MemberConfigPublic,
        request: &FederationSubmitOperationRequest,
    ) -> anyhow::Result<FederationSubmitOperationResponse> {
        let expected_operation_id = request.operation_id()?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::SubmitOperation,
            request.clone(),
        )?;
        let response: FederationSubmitOperationResponse = self
            .transport
            .http_post(federation_endpoint(member, "operations")?, None, &request)
            .await?;

        if response.operation_id != expected_operation_id {
            anyhow::bail!(
                "federation peer {} returned operation id {} for submitted operation {}",
                member.member_id,
                response.operation_id,
                expected_operation_id
            );
        }

        Ok(response)
    }

    pub(crate) async fn catch_up(
        &self,
        member: &MemberConfigPublic,
        request: &FederationJournalCatchUpRequest,
    ) -> anyhow::Result<FederationJournalCatchUpResponse> {
        let request =
            self.signed_request(member, FederationRestMessageKind::CatchUp, request.clone())?;
        let response: FederationJournalCatchUpResponse = self
            .transport
            .http_post(federation_endpoint(member, "journal")?, None, &request)
            .await?;
        if response.version != FEDERATION_REST_API_VERSION {
            anyhow::bail!(
                "federation peer {} returned unsupported REST version {}",
                member.member_id,
                response.version
            );
        }
        if response.federation_id != self.federation_id {
            anyhow::bail!(
                "federation peer {} returned catch-up for federation {}, expected {}",
                member.member_id,
                response.federation_id,
                self.federation_id
            );
        }
        Ok(response)
    }

    pub(crate) async fn catch_up_state_from_member(
        &self,
        state: &MintState,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<Vec<FederationJournalEntry>> {
        let consensus = state
            .federation_consensus
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("mint is not configured as a federation member"))?;
        let request = {
            let consensus = consensus.lock().await;
            consensus.catch_up_request()
        };
        let response = self.catch_up(member, &request).await?;
        Ok(state.apply_federation_catch_up_response(&response).await?)
    }

    fn signed_request<P>(
        &self,
        member: &MemberConfigPublic,
        kind: FederationRestMessageKind,
        payload: P,
    ) -> anyhow::Result<FederationSignedRequest<P>>
    where
        P: Serialize,
    {
        Ok(FederationSignedRequest::new_signed(
            self.federation_id,
            member.member_id,
            self.next_sequence.fetch_add(1, Ordering::Relaxed) + 1,
            kind,
            payload,
            &self.private_config,
        )?)
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn federation_endpoint(member: &MemberConfigPublic, endpoint: &str) -> anyhow::Result<url::Url> {
    Ok(member
        .federation_api_url
        .join_paths(&["federation", "v1", endpoint])?)
}

#[cfg_attr(not(test), allow(dead_code))]
fn validate_peer_status(
    expected_federation_id: FederationId,
    member: &MemberConfigPublic,
    status: &FederationPeerStatus,
) -> anyhow::Result<()> {
    if status.version != FEDERATION_REST_API_VERSION {
        anyhow::bail!(
            "federation peer {} returned unsupported REST version {}",
            member.member_id,
            status.version
        );
    }
    if status.member_id != member.member_id {
        anyhow::bail!(
            "federation peer status member mismatch: expected {}, found {}",
            member.member_id,
            status.member_id
        );
    }
    if status.federation_id != expected_federation_id {
        anyhow::bail!(
            "federation peer status federation mismatch: expected {}, found {}",
            expected_federation_id,
            status.federation_id
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use bip39::Mnemonic;
    use cdk::federation::{
        generate_trusted_dealer_configs, FederationMintQuotePaymentObservation,
        FederationOperation, FederationOperationEnvelope, FederationOperationKind,
        FederationOperationStatus, MemberId, ThresholdParams, TrustedDealerKeysetSpec,
        TrustedDealerMember,
    };
    use cdk::mint::{MintBuilder, MintInput, MintMeltLimits, QuoteId};
    use cdk::mint_url::MintUrl;
    use cdk::nuts::nut00::KnownMethod;
    use cdk::nuts::nut01::{BlsG1PublicKey, BlsSecretKey};
    use cdk::nuts::nut22::AuthToken;
    use cdk::nuts::{
        BlindedMessage, CurrencyUnit, MintQuoteBolt11Request, MintQuoteBolt11Response,
        MintQuoteState, MintRequest, PaymentMethod, PublicKey, RestoreRequest, SecretKey,
    };
    use cdk::types::FeeReserve;
    use cdk::Amount;
    use cdk_fake_wallet::FakeWallet;
    use cdk_http_client::HttpError;
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    use super::*;

    #[derive(Debug, Default)]
    struct MockFederationTransportState {
        status_response: Option<FederationPeerStatus>,
        submit_response: Option<FederationSubmitOperationResponse>,
        catch_up_response: Option<FederationJournalCatchUpResponse>,
        status_requests: Vec<FederationSignedRequest<FederationPeerStatusRequest>>,
        submitted_operations: Vec<FederationSignedRequest<FederationSubmitOperationRequest>>,
        catch_up_requests: Vec<FederationSignedRequest<FederationJournalCatchUpRequest>>,
    }

    #[derive(Debug, Clone, Default)]
    struct MockFederationTransport {
        state: Arc<Mutex<MockFederationTransportState>>,
    }

    impl MockFederationTransport {
        fn with_status_response(self, response: FederationPeerStatus) -> Self {
            self.state
                .lock()
                .expect("mock transport lock")
                .status_response = Some(response);
            self
        }

        fn with_submit_response(self, response: FederationSubmitOperationResponse) -> Self {
            self.state
                .lock()
                .expect("mock transport lock")
                .submit_response = Some(response);
            self
        }

        fn with_catch_up_response(self, response: FederationJournalCatchUpResponse) -> Self {
            self.state
                .lock()
                .expect("mock transport lock")
                .catch_up_response = Some(response);
            self
        }

        fn status_requests(&self) -> Vec<FederationSignedRequest<FederationPeerStatusRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .status_requests
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

        fn catch_up_requests(
            &self,
        ) -> Vec<FederationSignedRequest<FederationJournalCatchUpRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .catch_up_requests
                .clone()
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

        async fn http_get<R>(&self, url: url::Url, _auth: Option<AuthToken>) -> Result<R, HttpError>
        where
            R: DeserializeOwned,
        {
            Err(HttpError::Other(format!(
                "unexpected GET path {}",
                url.path()
            )))
        }

        async fn http_post<P, R>(
            &self,
            url: url::Url,
            _auth_token: Option<AuthToken>,
            payload: &P,
        ) -> Result<R, HttpError>
        where
            P: Serialize + Send + Sync,
            R: DeserializeOwned,
        {
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
                path if path.ends_with("/federation/v1/journal") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationJournalCatchUpRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.catch_up_requests.push(request);
                    let response = state
                        .catch_up_response
                        .clone()
                        .ok_or_else(|| HttpError::Other("missing catch-up response".to_string()))?;
                    json_response(response)
                }
                path => Err(HttpError::Other(format!("unexpected POST path {path}"))),
            }
        }
    }

    fn json_response<T, R>(value: T) -> Result<R, HttpError>
    where
        T: Serialize,
        R: DeserializeOwned,
    {
        Ok(serde_json::from_value(serde_json::to_value(value)?)?)
    }

    fn member_id(value: u16) -> MemberId {
        MemberId::new(value).expect("member id")
    }

    fn private_member(value: u16) -> MemberConfigPrivate {
        MemberConfigPrivate {
            member_id: member_id(value),
            identity_secret_key: SecretKey::from_slice(&[value as u8; 32])
                .expect("identity secret"),
            keyset_secret_shares: BTreeMap::new(),
        }
    }

    fn transport_config() -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
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
            .get(&member_id(1))
            .expect("first private config")
            .clone();
        let second_private = bundle
            .private_configs
            .get(&member_id(2))
            .expect("second private config")
            .clone();
        (bundle.public_config, first_private, second_private)
    }

    async fn federation_route_state_with_local_private() -> (
        MintState,
        FederationConfig,
        MemberConfigPrivate,
        MemberConfigPrivate,
    ) {
        let (public_config, local_private, peer_private) = transport_config();
        let runtime_config =
            cdk::mint::FederationRuntimeConfig::new(public_config.clone(), local_private.clone())
                .expect("runtime config");
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let mut builder = MintBuilder::new(db.clone())
            .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
            .with_federation_config(runtime_config.clone());
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(fake),
            )
            .await
            .expect("payment processor");
        let mint = builder
            .build_with_seed(db, &mnemonic.to_seed_normalized(""))
            .await
            .expect("mint");
        let state = MintState {
            mint: Arc::new(mint),
            cache: Arc::new(crate::cache::HttpCache::default()),
            federation_consensus: Some(Arc::new(tokio::sync::Mutex::new(
                InMemoryFederationConsensus::new(
                    runtime_config.federation_id(),
                    runtime_config.member_id(),
                ),
            ))),
            federation_sequences: Some(Arc::new(tokio::sync::Mutex::new(
                FederationRestSequenceTracker::new(),
            ))),
        };

        (state, public_config, local_private, peer_private)
    }

    async fn federation_route_state() -> (MintState, FederationConfig, MemberConfigPrivate) {
        let (state, public_config, _, peer_private) =
            federation_route_state_with_local_private().await;
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

    async fn consensus_operation_count(state: &MintState) -> u64 {
        state
            .federation_consensus
            .as_ref()
            .expect("consensus")
            .lock()
            .await
            .peer_status()
            .operation_count
    }

    fn payment_observation_envelope(federation_id: FederationId) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "quote-paid".to_string(),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(21),
                amount_issued: Amount::ZERO,
                payment_proof: Some("fake-wallet-paid".to_string()),
            }),
        )
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
                state: MintQuoteState::Unpaid,
                expiry: Some(10_000),
                pubkey: None,
            }),
        )
    }

    fn paid_observation_envelope(
        state: &MintState,
        quote_id: &QuoteId,
    ) -> FederationOperationEnvelope {
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

    fn restore_envelope(
        federation_id: FederationId,
        label: &'static str,
    ) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            FederationOperation::Restore(cdk::federation::FederationRestoreOperation {
                request: RestoreRequest {
                    outputs: vec![cdk::nuts::BlindedMessage::new(
                        Amount::from(1),
                        cdk::nuts::Id::from_str("00bfa73302d12ffd").expect("keyset id"),
                        PublicKey::from_hex(label).expect("public key"),
                    )],
                },
            }),
        )
    }

    #[tokio::test]
    async fn rest_peer_client_validates_peer_status() {
        let (public_config, local_private, _) = transport_config();
        let federation_id = public_config.federation_id;
        let member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new(
            federation_id,
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status.clone());
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let response = client.peer_status(member).await.expect("peer status");

        assert_eq!(response, status);
        let requests = transport.status_requests();
        assert_eq!(requests.len(), 1);
        requests[0]
            .verify(&public_config, member.member_id)
            .expect("signed status request");
        assert_eq!(requests[0].sender, member_id(1));
        assert_eq!(requests[0].kind, FederationRestMessageKind::Status);
    }

    #[tokio::test]
    async fn rest_peer_client_rejects_status_wrong_federation() {
        let (public_config, local_private, _) = transport_config();
        let member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new(
            super::FederationId::from_bytes([32; 32]),
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        assert!(client.peer_status(member).await.is_err());
    }

    #[tokio::test]
    async fn rest_peer_client_submits_payment_observation_operations() {
        let (public_config, local_private, _) = transport_config();
        let federation_id = public_config.federation_id;
        let member = public_config.member(member_id(2)).expect("peer member");
        let envelope = payment_observation_envelope(federation_id);
        let request = FederationSubmitOperationRequest::new(envelope.clone());
        let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, member.member_id);
        let submit_response = peer_consensus
            .submit_operation(FederationSubmitOperationRequest::new(envelope))
            .expect("peer submit response");
        let transport =
            MockFederationTransport::default().with_submit_response(submit_response.clone());
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let response = client
            .submit_operation(member, &request)
            .await
            .expect("submit operation");

        assert_eq!(response.operation_id, submit_response.operation_id);
        let submitted = transport.submitted_operations();
        assert_eq!(submitted.len(), 1);
        submitted[0]
            .verify(&public_config, member.member_id)
            .expect("signed submit request");
        assert_eq!(
            submitted[0].payload.operation_id().expect("operation id"),
            response.operation_id
        );
        assert_eq!(
            submitted[0].payload.envelope.operation.kind(),
            FederationOperationKind::MintQuotePayment
        );
    }

    #[tokio::test]
    async fn rest_peer_client_catches_up_local_consensus_from_member() {
        let (state, public_config, local_private, _) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let first = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        state
            .submit_federation_envelope(first.clone())
            .await
            .expect("local first operation");
        peer_consensus
            .submit_envelope(first)
            .expect("peer first operation");
        peer_consensus
            .submit_envelope(second)
            .expect("peer second operation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");
        let transport =
            MockFederationTransport::default().with_catch_up_response(catch_up_response);
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let applied = client
            .catch_up_state_from_member(&state, peer_member)
            .await
            .expect("catch up");

        assert_eq!(applied.len(), 1);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .peer_status()
                .latest_order_digest,
            peer_consensus.peer_status().latest_order_digest
        );
        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("persisted journal");
        assert_eq!(persisted.len(), 2);
        assert_eq!(persisted[1].operation_id, applied[0].operation_id);
        let requests = transport.catch_up_requests();
        assert_eq!(requests.len(), 1);
        requests[0]
            .verify(&public_config, peer_member.member_id)
            .expect("signed catch-up request");
        assert_eq!(requests[0].payload.next_index, 1);
    }

    #[tokio::test]
    async fn catch_up_applies_mint_quote_operation_to_local_store() {
        let (state, public_config, _) = federation_route_state().await;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let quote_id = QuoteId::new();
        let envelope = mint_quote_envelope(&state, &quote_id);
        let operation_id = envelope.operation_id().expect("operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted quote");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.request, format!("invoice-{quote_id}"));

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn federation_runtime_state_replays_persisted_journal_and_sequences() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let peer_member = peer_private.member_id;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let response = state
            .submit_federation_envelope(envelope)
            .await
            .expect("persist operation");
        state
            .persist_federation_sequence(peer_member, 42)
            .await
            .expect("persist sequence");

        let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("runtime state");
        let consensus = consensus.expect("consensus");
        let sequences = sequences.expect("sequences");
        let consensus = consensus.lock().await;
        let sequences = sequences.lock().await;

        assert_eq!(consensus.peer_status().operation_count, 1);
        assert_eq!(
            consensus
                .journal()
                .entries()
                .first()
                .expect("journal entry")
                .operation_id,
            response.operation_id
        );
        assert_eq!(sequences.sequences().get(&peer_member), Some(&42));
    }

    #[tokio::test]
    async fn submit_mint_quote_operation_applies_quote_to_local_store() {
        let (state, _, _) = federation_route_state().await;
        let quote_id = QuoteId::new();
        let envelope = mint_quote_envelope(&state, &quote_id);
        let operation_id = envelope.operation_id().expect("operation id");

        state
            .submit_federation_envelope(envelope)
            .await
            .expect("submit mint quote");

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.request, format!("invoice-{quote_id}"));
        assert_eq!(quote.amount, Some(Amount::new(1, CurrencyUnit::Sat)));
        assert_eq!(quote.state(), MintQuoteState::Unpaid);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            state
                .federation_consensus
                .as_ref()
                .expect("consensus")
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn submit_mint_operation_applies_issuance_and_persists_share() {
        let (state, _, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let quote_id = QuoteId::new();
        state
            .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
            .await
            .expect("submit mint quote");
        state
            .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
            .await
            .expect("submit paid observation");

        let output = mint_output(&state, &quote_id);
        let input = MintInput::Single(MintRequest {
            quote: quote_id.clone(),
            outputs: vec![output.clone()],
            signature: None,
        });
        let envelope = config.mint_operation(input.clone()).expect("mint envelope");
        let operation_id = envelope.operation_id().expect("operation id");
        state
            .submit_federation_envelope(envelope)
            .await
            .expect("submit mint");

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.amount_paid(), Amount::new(1, CurrencyUnit::Sat));
        assert_eq!(quote.amount_issued(), Amount::new(1, CurrencyUnit::Sat));
        assert_eq!(quote.state(), MintQuoteState::Issued);

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup");
        let persisted_signature = signatures[0].clone().expect("signature persisted");
        let expected_response = config
            .mint_share_response(operation_id, &input)
            .expect("expected share response");
        assert_eq!(persisted_signature, expected_response.signatures[0]);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(
            persisted
                .iter()
                .find(|entry| entry.operation_id == operation_id)
                .expect("mint entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn private_operation_route_accepts_signed_peer_request() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            envelope.clone(),
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect("operation response");

        assert_eq!(
            response.0.operation_id,
            envelope.operation_id().expect("operation id")
        );
        assert_eq!(consensus_operation_count(&state).await, 1);
        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("persisted journal");
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].operation_id, response.0.operation_id);
        assert_eq!(sequences.get(&peer_private.member_id), Some(&1));
    }

    #[tokio::test]
    async fn private_operation_route_rejects_tampered_payload_before_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let first = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            public_config.federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            first,
        );
        request.payload = FederationSubmitOperationRequest::new(second);

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("tampered request must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
    }

    #[tokio::test]
    async fn private_operation_route_rejects_unknown_sender_before_mutation() {
        let (state, public_config, _) = federation_route_state().await;
        let unknown_private = private_member(3);
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let request = signed_operation_request(
            &public_config,
            &unknown_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            envelope,
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("unknown sender must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
    }

    #[tokio::test]
    async fn private_operation_route_rejects_kind_mismatch_before_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::Status,
            envelope,
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("wrong message kind must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
    }

    #[tokio::test]
    async fn private_operation_route_rejects_stale_sequence_before_second_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let first = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            public_config.federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let first_request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            first,
        );
        let second_request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            second,
        );

        let _ = post_operation(State(state.clone()), Json(first_request))
            .await
            .expect("first operation accepted");
        let response = post_operation(State(state.clone()), Json(second_request))
            .await
            .expect_err("stale sequence must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 1);
    }
}
