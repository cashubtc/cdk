//! Private federation member REST routes.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use cdk::federation::MemberConfigPublic;
use cdk::federation::{
    FederationError, FederationId, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationJournalEntry, FederationPeerStatus,
    FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    InMemoryFederationConsensus, FEDERATION_REST_API_VERSION,
};
use cdk_http_client::{Async, Transport};
use tracing::instrument;

use crate::MintState;

type ConsensusHandle = std::sync::Arc<tokio::sync::Mutex<InMemoryFederationConsensus>>;

pub(crate) fn create_federation_router() -> Router<MintState> {
    Router::new()
        .route("/status", get(get_status))
        .route("/operations", post(post_operation))
        .route("/journal", post(post_journal))
}

#[instrument(skip_all)]
async fn get_status(
    State(state): State<MintState>,
) -> Result<Json<FederationPeerStatus>, Response> {
    let consensus = federation_consensus(&state)?;
    let consensus = consensus.lock().await;
    Ok(Json(consensus.peer_status()))
}

#[instrument(skip_all)]
async fn post_operation(
    State(state): State<MintState>,
    Json(payload): Json<FederationSubmitOperationRequest>,
) -> Result<Json<FederationSubmitOperationResponse>, Response> {
    let consensus = federation_consensus(&state)?;
    let mut consensus = consensus.lock().await;
    let response = consensus
        .submit_operation(payload)
        .map_err(federation_error_response)?;
    tracing::debug!("Accepted federation operation {}", response.operation_id);
    Ok(Json(response))
}

#[instrument(skip_all)]
async fn post_journal(
    State(state): State<MintState>,
    Json(payload): Json<FederationJournalCatchUpRequest>,
) -> Result<Json<FederationJournalCatchUpResponse>, Response> {
    let consensus = federation_consensus(&state)?;
    tracing::debug!(
        next_index = payload.next_index,
        limit = ?payload.limit,
        "Received federation journal catch-up request"
    );
    let consensus = consensus.lock().await;
    let response = consensus
        .catch_up_checked(&payload)
        .map_err(federation_error_response)?;
    Ok(Json(response))
}

fn federation_consensus(state: &MintState) -> Result<ConsensusHandle, Response> {
    state
        .federation_consensus
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
        | FederationError::CatchUpOperationIdMismatch { .. } => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_REQUEST,
    };
    tracing::warn!("Federation request failed: {}", err);
    (status, err.to_string()).into_response()
}

#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationRestPeerClient<T = Async> {
    transport: T,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Default for FederationRestPeerClient<Async> {
    fn default() -> Self {
        Self::new(Async::default())
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl<T> FederationRestPeerClient<T>
where
    T: Transport,
{
    pub(crate) fn new(transport: T) -> Self {
        Self { transport }
    }

    pub(crate) async fn peer_status(
        &self,
        expected_federation_id: FederationId,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<FederationPeerStatus> {
        let status = self
            .transport
            .http_get(federation_endpoint(member, "status")?, None)
            .await?;
        validate_peer_status(expected_federation_id, member, &status)?;
        Ok(status)
    }

    pub(crate) async fn submit_operation(
        &self,
        member: &MemberConfigPublic,
        request: &FederationSubmitOperationRequest,
    ) -> anyhow::Result<FederationSubmitOperationResponse> {
        let expected_operation_id = request.operation_id()?;
        let response: FederationSubmitOperationResponse = self
            .transport
            .http_post(federation_endpoint(member, "operations")?, None, request)
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
        let response: FederationJournalCatchUpResponse = self
            .transport
            .http_post(federation_endpoint(member, "journal")?, None, request)
            .await?;
        if response.version != FEDERATION_REST_API_VERSION {
            anyhow::bail!(
                "federation peer {} returned unsupported REST version {}",
                member.member_id,
                response.version
            );
        }
        Ok(response)
    }

    pub(crate) async fn catch_up_local_from_member(
        &self,
        consensus: ConsensusHandle,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<Vec<FederationJournalEntry>> {
        let request = {
            let consensus = consensus.lock().await;
            consensus.catch_up_request()
        };
        let response = self.catch_up(member, &request).await?;
        let mut consensus = consensus.lock().await;
        Ok(consensus.apply_catch_up_response(&response)?)
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
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use cdk::federation::{
        FederationMintQuotePaymentObservation, FederationOperation, FederationOperationEnvelope,
        FederationOperationKind, MemberId,
    };
    use cdk::mint_url::MintUrl;
    use cdk::nuts::nut00::KnownMethod;
    use cdk::nuts::nut22::AuthToken;
    use cdk::nuts::{MintQuoteState, PaymentMethod, PublicKey, RestoreRequest, SecretKey};
    use cdk::Amount;
    use cdk_http_client::HttpError;
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    use super::*;

    #[derive(Debug, Default)]
    struct MockFederationTransportState {
        status_response: Option<FederationPeerStatus>,
        submit_response: Option<FederationSubmitOperationResponse>,
        catch_up_response: Option<FederationJournalCatchUpResponse>,
        submitted_operations: Vec<FederationSubmitOperationRequest>,
        catch_up_requests: Vec<FederationJournalCatchUpRequest>,
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

        fn submitted_operations(&self) -> Vec<FederationSubmitOperationRequest> {
            self.state
                .lock()
                .expect("mock transport lock")
                .submitted_operations
                .clone()
        }

        fn catch_up_requests(&self) -> Vec<FederationJournalCatchUpRequest> {
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
            if !url.path().ends_with("/federation/v1/status") {
                return Err(HttpError::Other(format!(
                    "unexpected GET path {}",
                    url.path()
                )));
            }
            let response = self
                .state
                .lock()
                .expect("mock transport lock")
                .status_response
                .clone()
                .ok_or_else(|| HttpError::Other("missing status response".to_string()))?;
            json_response(response)
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
                path if path.ends_with("/federation/v1/operations") => {
                    let request = serde_json::from_value::<FederationSubmitOperationRequest>(
                        serde_json::to_value(payload)?,
                    )?;
                    state.submitted_operations.push(request);
                    let response = state
                        .submit_response
                        .clone()
                        .ok_or_else(|| HttpError::Other("missing submit response".to_string()))?;
                    json_response(response)
                }
                path if path.ends_with("/federation/v1/journal") => {
                    let request = serde_json::from_value::<FederationJournalCatchUpRequest>(
                        serde_json::to_value(payload)?,
                    )?;
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

    fn federation_id(byte: u8) -> FederationId {
        FederationId::from_bytes([byte; 32])
    }

    fn member_id(value: u16) -> MemberId {
        MemberId::new(value).expect("member id")
    }

    fn member_config(value: u16) -> MemberConfigPublic {
        let member_id = member_id(value);
        MemberConfigPublic {
            member_id,
            public_mint_url: MintUrl::from_str(&format!("https://mint{value}.example.com"))
                .expect("public mint url"),
            federation_api_url: MintUrl::from_str(&format!("https://fed{value}.example.com/api"))
                .expect("federation api url"),
            identity_public_key: SecretKey::from_slice(&[value as u8; 32])
                .expect("identity secret")
                .public_key(),
        }
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
        let federation_id = federation_id(31);
        let member = member_config(2);
        let status = FederationPeerStatus::new(
            federation_id,
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status.clone());
        let client = FederationRestPeerClient::new(transport);

        let response = client
            .peer_status(federation_id, &member)
            .await
            .expect("peer status");

        assert_eq!(response, status);
    }

    #[tokio::test]
    async fn rest_peer_client_rejects_status_wrong_federation() {
        let federation_id = federation_id(31);
        let member = member_config(2);
        let status = FederationPeerStatus::new(
            super::FederationId::from_bytes([32; 32]),
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let client = FederationRestPeerClient::new(transport);

        assert!(client.peer_status(federation_id, &member).await.is_err());
    }

    #[tokio::test]
    async fn rest_peer_client_submits_payment_observation_operations() {
        let federation_id = federation_id(31);
        let member = member_config(2);
        let envelope = payment_observation_envelope(federation_id);
        let request = FederationSubmitOperationRequest::new(envelope.clone());
        let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, member.member_id);
        let submit_response = peer_consensus
            .submit_operation(FederationSubmitOperationRequest::new(envelope))
            .expect("peer submit response");
        let transport =
            MockFederationTransport::default().with_submit_response(submit_response.clone());
        let client = FederationRestPeerClient::new(transport.clone());

        let response = client
            .submit_operation(&member, &request)
            .await
            .expect("submit operation");

        assert_eq!(response.operation_id, submit_response.operation_id);
        let submitted = transport.submitted_operations();
        assert_eq!(submitted.len(), 1);
        assert_eq!(
            submitted[0].operation_id().expect("operation id"),
            response.operation_id
        );
        assert_eq!(
            submitted[0].envelope.operation.kind(),
            FederationOperationKind::MintQuotePayment
        );
    }

    #[tokio::test]
    async fn rest_peer_client_catches_up_local_consensus_from_member() {
        let federation_id = federation_id(31);
        let local_member = member_id(1);
        let peer_member = member_config(2);
        let first = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut local_consensus = InMemoryFederationConsensus::new(federation_id, local_member);
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        local_consensus
            .submit_envelope(first.clone())
            .expect("local first operation");
        peer_consensus
            .submit_envelope(first)
            .expect("peer first operation");
        peer_consensus
            .submit_envelope(second)
            .expect("peer second operation");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.catch_up_request())
            .expect("peer catch-up response");
        let consensus = Arc::new(tokio::sync::Mutex::new(local_consensus));
        let transport =
            MockFederationTransport::default().with_catch_up_response(catch_up_response);
        let client = FederationRestPeerClient::new(transport.clone());

        let applied = client
            .catch_up_local_from_member(consensus.clone(), &peer_member)
            .await
            .expect("catch up");

        assert_eq!(applied.len(), 1);
        assert_eq!(
            consensus.lock().await.peer_status().latest_order_digest,
            peer_consensus.peer_status().latest_order_digest
        );
        let requests = transport.catch_up_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].next_index, 1);
    }
}
