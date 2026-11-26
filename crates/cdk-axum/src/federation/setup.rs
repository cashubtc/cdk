//! HTTP routes of the setup-only coordinator.
//!
//! The coordinator itself, its typed client and its errors live in
//! `cdk_federation::setup`; this module maps requests and errors to HTTP.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::error_handling::HandleErrorLayer;
use axum::extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts, Json, State};
use axum::http::{header, request::Parts, HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use cdk_common::federation::{
    FederationSetupApproval, FederationSetupEnrollment, FEDERATION_MAX_SETUP_ARTIFACT_BYTES,
    FEDERATION_MAX_SETUP_ENROLLMENT_BYTES, FEDERATION_MAX_SETUP_STATUS_BYTES,
    FEDERATION_SETUP_CORRELATION_HEADER,
};
use cdk_federation::setup::{
    FederationSetupCoordinatorService, FederationSetupHttpError, FederationSetupHttpErrorCode,
    FederationSetupRequestAuthorization, FederationSetupRequestSource,
    FEDERATION_SETUP_APPROVALS_PATH, FEDERATION_SETUP_APPROVAL_REQUEST_BYTES,
    FEDERATION_SETUP_ENROLLMENTS_PATH, FEDERATION_SETUP_ERROR_RESPONSE_BYTES,
    FEDERATION_SETUP_MANIFEST_PATH, FEDERATION_SETUP_PROPOSAL_PATH, FEDERATION_SETUP_STATUS_PATH,
};
use serde::Serialize;
use tower::{BoxError, ServiceBuilder};

/// A setup error as an HTTP response.
#[derive(Debug)]
struct SetupErrorResponse(FederationSetupHttpError);

impl From<FederationSetupHttpError> for SetupErrorResponse {
    fn from(error: FederationSetupHttpError) -> Self {
        Self(error)
    }
}

impl IntoResponse for SetupErrorResponse {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        no_store((status, Json(self.0.body())).into_response())
    }
}

/// The connection a setup request arrived on.
struct SetupRequestSource(FederationSetupRequestSource);

impl<S> FromRequestParts<S> for SetupRequestSource
where
    S: Send + Sync,
{
    type Rejection = SetupErrorResponse;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(ConnectInfo(source)) = parts.extensions.get::<ConnectInfo<SocketAddr>>() {
            return Ok(Self(FederationSetupRequestSource::Ip(source.ip())));
        }
        #[cfg(feature = "iroh")]
        if let Some(connection) = parts.extensions.get::<cdk_iroh::IrohConnectionInfo>() {
            return Ok(Self(FederationSetupRequestSource::Iroh(
                connection.remote_endpoint_id,
            )));
        }
        Err(FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::InvalidRequest,
            "setup connection identity is unavailable",
        )
        .into())
    }
}

/// Create the isolated router containing only the five setup protocol routes.
pub fn create_federation_setup_router(service: Arc<FederationSetupCoordinatorService>) -> Router {
    let request_deadline = service.request_deadline();
    Router::new()
        .route(
            FEDERATION_SETUP_ENROLLMENTS_PATH,
            post(post_enrollment).layer(DefaultBodyLimit::max(
                FEDERATION_MAX_SETUP_ENROLLMENT_BYTES + FEDERATION_SETUP_ERROR_RESPONSE_BYTES,
            )),
        )
        .route(FEDERATION_SETUP_PROPOSAL_PATH, get(get_proposal))
        .route(
            FEDERATION_SETUP_APPROVALS_PATH,
            post(post_approval).layer(DefaultBodyLimit::max(
                FEDERATION_SETUP_APPROVAL_REQUEST_BYTES,
            )),
        )
        .route(FEDERATION_SETUP_MANIFEST_PATH, get(get_manifest))
        .route(FEDERATION_SETUP_STATUS_PATH, get(get_status))
        .fallback(setup_not_found)
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_setup_timeout))
                .timeout(request_deadline),
        )
        .layer(axum::middleware::from_fn(add_no_store))
        .with_state(service)
}

async fn add_no_store(request: Request<Body>, next: Next) -> Response {
    no_store(next.run(request).await)
}

async fn handle_setup_timeout(error: BoxError) -> SetupErrorResponse {
    match error.is::<tower::timeout::error::Elapsed>() {
        true => FederationSetupHttpError::timeout().into(),
        false => FederationSetupHttpError::storage().into(),
    }
}

async fn post_enrollment(
    State(service): State<Arc<FederationSetupCoordinatorService>>,
    SetupRequestSource(source): SetupRequestSource,
    headers: HeaderMap,
    Json(enrollment): Json<FederationSetupEnrollment>,
) -> Result<Response, SetupErrorResponse> {
    let response = service
        .enroll_request(source, request_authorization(&headers)?, enrollment)
        .await?;
    bounded_json_response(response, FEDERATION_MAX_SETUP_ENROLLMENT_BYTES)
}

async fn get_proposal(
    State(service): State<Arc<FederationSetupCoordinatorService>>,
    SetupRequestSource(source): SetupRequestSource,
    headers: HeaderMap,
) -> Result<Response, SetupErrorResponse> {
    let response = service
        .proposal_request(source, request_authorization(&headers)?)
        .await?;
    bounded_json_response(response, FEDERATION_MAX_SETUP_ARTIFACT_BYTES)
}

async fn post_approval(
    State(service): State<Arc<FederationSetupCoordinatorService>>,
    SetupRequestSource(source): SetupRequestSource,
    headers: HeaderMap,
    Json(approval): Json<FederationSetupApproval>,
) -> Result<Response, SetupErrorResponse> {
    let response = service
        .approval_request(source, request_authorization(&headers)?, approval)
        .await?;
    bounded_json_response(response, FEDERATION_SETUP_APPROVAL_REQUEST_BYTES)
}

async fn get_manifest(
    State(service): State<Arc<FederationSetupCoordinatorService>>,
    SetupRequestSource(source): SetupRequestSource,
    headers: HeaderMap,
) -> Result<Response, SetupErrorResponse> {
    let response = service
        .manifest_request(source, request_authorization(&headers)?)
        .await?;
    bounded_json_response(response, FEDERATION_MAX_SETUP_ARTIFACT_BYTES)
}

async fn get_status(
    State(service): State<Arc<FederationSetupCoordinatorService>>,
    SetupRequestSource(source): SetupRequestSource,
    headers: HeaderMap,
) -> Result<Response, SetupErrorResponse> {
    let response = service
        .status_request(source, request_authorization(&headers)?)
        .await?;
    bounded_json_response(response, FEDERATION_MAX_SETUP_STATUS_BYTES)
}

async fn setup_not_found() -> SetupErrorResponse {
    FederationSetupHttpError::new(
        FederationSetupHttpErrorCode::InvalidRequest,
        "unknown setup-only route",
    )
    .into()
}

fn request_authorization(
    headers: &HeaderMap,
) -> Result<FederationSetupRequestAuthorization, FederationSetupHttpError> {
    FederationSetupRequestAuthorization::from_headers(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        headers
            .get(FEDERATION_SETUP_CORRELATION_HEADER)
            .and_then(|value| value.to_str().ok()),
    )
}

fn bounded_json_response<T>(value: T, max: usize) -> Result<Response, SetupErrorResponse>
where
    T: Serialize,
{
    let body = serde_json::to_vec(&value).map_err(|_| FederationSetupHttpError::storage())?;
    if body.len() > max {
        return Err(FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::StorageFailure,
            "setup response exceeds its configured bound",
        )
        .into());
    }
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(no_store(response))
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::str::FromStr;

    use cdk_common::database::MintFederationSetupDatabase;
    use cdk_common::federation::{
        FederationCheckpointPolicy, FederationDkgKeysetSpec, FederationPaymentObservationPolicy,
        FederationSetupBitcoinNetwork, FederationSetupFrostApplication, FederationSetupFrostPolicy,
        FederationSetupInvitationCapability, FederationSetupLocalRole,
        FederationSetupParticipantPhase, FederationSetupProposalBody,
        FederationSetupProtocolVersions, MemberId, ThresholdParams,
        FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
    };
    use cdk_common::mint_url::MintUrl;
    use cdk_common::{Amount, CurrencyUnit};
    use cdk_sqlite::MintSqliteDatabase;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;

    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use cdk_common::database::mint::{Database, DynMintDatabase};
    use cdk_common::federation::{
        FederationSetupCoordinatorPhase, FederationSetupCoordinatorPublicKey,
        FederationSetupCorrelationNonce, FederationSetupId, FederationSetupInvitation,
        FederationSetupInvitationId, FederationSetupInvitationRecord, FederationSetupProposal,
        FederationSetupSession, FederationSetupUrlPolicy,
    };
    use cdk_common::SecretKey;
    use cdk_federation::setup::{
        FederationSetupCoordinatorServiceConfig, FederationSetupHttpClient,
        FederationSetupOperationFaults,
    };
    use url::Url;

    use super::*;

    /// Injected operation delay and storage failure for the coordinator.
    #[derive(Default)]
    struct TestSetupOperationFaults {
        delay: tokio::sync::Mutex<Option<Duration>>,
        fail_storage: AtomicBool,
    }

    impl TestSetupOperationFaults {
        async fn set_operation_delay(&self, delay: Option<Duration>) {
            *self.delay.lock().await = delay;
        }

        fn set_storage_failure(&self, fail: bool) {
            self.fail_storage.store(fail, Ordering::Release);
        }
    }

    #[async_trait::async_trait]
    impl FederationSetupOperationFaults for TestSetupOperationFaults {
        async fn before_operation(&self) -> Result<(), FederationSetupHttpError> {
            if let Some(delay) = *self.delay.lock().await {
                tokio::time::sleep(delay).await;
            }
            if self.fail_storage.load(Ordering::Acquire) {
                return Err(FederationSetupHttpError::storage());
            }
            Ok(())
        }
    }

    struct SetupHttpFixture {
        database: Arc<MintSqliteDatabase>,
        service: Arc<FederationSetupCoordinatorService>,
        faults: Arc<TestSetupOperationFaults>,
        invitations: BTreeMap<MemberId, FederationSetupInvitation>,
        enrollments: BTreeMap<MemberId, FederationSetupEnrollment>,
        member_secrets: BTreeMap<MemberId, SecretKey>,
        coordinator_secret: SecretKey,
        url_policy: FederationSetupUrlPolicy,
        shutdown: oneshot::Sender<()>,
        server: tokio::task::JoinHandle<()>,
    }

    impl SetupHttpFixture {
        async fn stop(self) {
            let _ = self.shutdown.send(());
            self.server.await.expect("setup server task");
            self.service.shutdown().await;
        }

        fn invitation(&self, member: u16) -> &FederationSetupInvitation {
            self.invitations
                .get(&member_id(member))
                .expect("member invitation")
        }

        fn enrollment(&self, member: u16) -> &FederationSetupEnrollment {
            self.enrollments
                .get(&member_id(member))
                .expect("member enrollment")
        }
    }

    #[tokio::test]
    async fn real_http_setup_flow_is_authenticated_idempotent_and_certifies() {
        let fixture = setup_http_fixture(FederationSetupCoordinatorServiceConfig::default()).await;
        let client = FederationSetupHttpClient::default();

        let not_ready = client
            .proposal(fixture.invitation(2))
            .await
            .expect_err("proposal must not exist before publication");
        assert_eq!(not_ready.code, FederationSetupHttpErrorCode::Replay);

        let (member_two, member_three) = tokio::join!(
            client.enroll(fixture.invitation(2), fixture.enrollment(2)),
            client.enroll(fixture.invitation(3), fixture.enrollment(3)),
        );
        assert_eq!(
            member_two.expect("member two enrollment"),
            *fixture.enrollment(2)
        );
        assert_eq!(
            member_three.expect("member three enrollment"),
            *fixture.enrollment(3)
        );
        assert_eq!(
            client
                .enroll(fixture.invitation(2), fixture.enrollment(2))
                .await
                .expect("exact enrollment replay"),
            *fixture.enrollment(2)
        );

        let mut invalid_enrollment = fixture.enrollment(2).clone();
        invalid_enrollment.signature = fixture.enrollment(3).signature;
        let invalid_signature = client
            .enroll(fixture.invitation(2), &invalid_enrollment)
            .await
            .expect_err("invalid enrollment signature");
        assert_eq!(
            invalid_signature.code,
            FederationSetupHttpErrorCode::InvalidSignature
        );

        let proposal = publish_proposal(&fixture).await;
        assert_eq!(
            client
                .proposal(fixture.invitation(2))
                .await
                .expect("authenticated proposal"),
            proposal
        );

        let mut equivocating_body = proposal.body.clone();
        equivocating_body
            .checkpoint_policy
            .checkpoint_interval_journal_operations += 1;
        let equivocating_proposal = FederationSetupProposal::sign(
            equivocating_body,
            &fixture.coordinator_secret,
            fixture.url_policy,
        )
        .expect("alternate proposal");
        let mut transaction = Database::begin_transaction(&*fixture.database)
            .await
            .expect("equivocation transaction");
        assert!(transaction
            .add_federation_setup_record(FederationSetupRecord::Proposal(&equivocating_proposal))
            .await
            .is_err());
        transaction.rollback().await.expect("rollback equivocation");

        let mut wrong_identity = fixture.invitation(2).clone();
        wrong_identity.coordinator_identity =
            FederationSetupCoordinatorPublicKey::new(SecretKey::generate().public_key())
                .expect("alternate identity");
        let wrong_identity_error = client
            .proposal(&wrong_identity)
            .await
            .expect_err("wrong setup identity");
        assert_eq!(
            wrong_identity_error.code,
            FederationSetupHttpErrorCode::InvalidInvitation
        );

        let creator_approval = FederationSetupApproval::sign(
            &proposal,
            member_id(1),
            fixture.member_secrets.get(&member_id(1)).expect("secret"),
            fixture.url_policy,
        )
        .expect("creator approval");
        let mut transaction = Database::begin_transaction(&*fixture.database)
            .await
            .expect("creator approval transaction");
        transaction
            .add_federation_setup_record(FederationSetupRecord::Approval(&creator_approval))
            .await
            .expect("creator approval");
        transaction.commit().await.expect("commit creator approval");

        let member_two_approval = FederationSetupApproval::sign(
            &proposal,
            member_id(2),
            fixture.member_secrets.get(&member_id(2)).expect("secret"),
            fixture.url_policy,
        )
        .expect("member two approval");
        client
            .approve(fixture.invitation(2), &member_two_approval)
            .await
            .expect("member two approval accepted");
        let alternate_member_two_approval = FederationSetupApproval::sign(
            &proposal,
            member_id(2),
            fixture.member_secrets.get(&member_id(2)).expect("secret"),
            fixture.url_policy,
        )
        .expect("alternate valid approval signature");
        if alternate_member_two_approval != member_two_approval {
            let conflict = client
                .approve(fixture.invitation(2), &alternate_member_two_approval)
                .await
                .expect_err("approval exact-byte conflict");
            assert_eq!(conflict.code, FederationSetupHttpErrorCode::Conflict);
        }

        let member_three_approval = FederationSetupApproval::sign(
            &proposal,
            member_id(3),
            fixture.member_secrets.get(&member_id(3)).expect("secret"),
            fixture.url_policy,
        )
        .expect("member three approval");
        client
            .approve(fixture.invitation(3), &member_three_approval)
            .await
            .expect("member three approval accepted");
        let manifest = client
            .manifest(fixture.invitation(2))
            .await
            .expect("certified manifest");
        assert_eq!(manifest.approvals.len(), 3);
        let status = client
            .status(fixture.invitation(2))
            .await
            .expect("certified status");
        assert_eq!(
            status.coordinator_phase,
            FederationSetupCoordinatorPhase::Certified
        );
        assert_eq!(status.enrolled_members.len(), 3);
        assert_eq!(status.approved_members.len(), 3);
        assert_eq!(status.manifest_hash, Some(manifest.manifest_hash));

        fixture.stop().await;
    }

    #[cfg(feature = "iroh")]
    #[tokio::test]
    async fn setup_client_preserves_authenticated_headers_over_iroh() {
        let config = cdk_iroh::IrohConfig::static_only().with_bind_addr(([127, 0, 0, 1], 0).into());
        let fixture = setup_http_fixture(FederationSetupCoordinatorServiceConfig::default()).await;
        let server_node = cdk_iroh::IrohNode::ephemeral(config.clone())
            .await
            .expect("setup Iroh server node");
        let client_node =
            cdk_iroh::IrohNode::ephemeral(config.with_ticket(server_node.endpoint_ticket()))
                .await
                .expect("setup Iroh client node");
        let server = cdk_iroh::IrohServer::start(
            server_node.clone(),
            create_federation_setup_router(fixture.service.clone()),
        );
        let mut invitation = fixture.invitation(2).clone();
        invitation.coordinator_url = mint_url(&format!("iroh://{}", server_node.endpoint_id()));
        let client = FederationSetupHttpClient::with_iroh_node(client_node);

        let accepted = client
            .enroll(&invitation, fixture.enrollment(2))
            .await
            .expect("authenticated setup enrollment over Iroh");
        assert_eq!(accepted, *fixture.enrollment(2));

        server.shutdown().await.expect("setup Iroh shutdown");
        fixture.stop().await;
    }

    #[tokio::test]
    async fn real_http_limits_expiry_storage_failure_and_no_store_errors() {
        let fixture = setup_http_fixture(FederationSetupCoordinatorServiceConfig::default()).await;
        let client = FederationSetupHttpClient::default();
        let mut expired = fixture.invitation(2).clone();
        expired.expires_at_unix = current_unix_time().expect("time");
        let expired_error = client
            .enroll(&expired, fixture.enrollment(2))
            .await
            .expect_err("expired invitation");
        assert_eq!(expired_error.code, FederationSetupHttpErrorCode::Expired);

        fixture.faults.set_storage_failure(true);
        let storage_error = client
            .enroll(fixture.invitation(2), fixture.enrollment(2))
            .await
            .expect_err("injected storage failure");
        assert_eq!(
            storage_error.code,
            FederationSetupHttpErrorCode::StorageFailure
        );
        fixture.faults.set_storage_failure(false);

        let nonce = FederationSetupCorrelationNonce::from_bytes([4; 32]).expect("nonce");
        let response = reqwest::Client::new()
            .post(setup_url(fixture.invitation(2), FEDERATION_SETUP_ENROLLMENTS_PATH).expect("URL"))
            .header(
                header::AUTHORIZATION,
                format!(
                    "Bearer {}",
                    fixture
                        .invitation(2)
                        .to_invitation_code()
                        .expect("invitation")
                ),
            )
            .header(FEDERATION_SETUP_CORRELATION_HEADER, nonce.to_string())
            .header(header::CONTENT_TYPE, "application/json")
            .body(vec![b' '; FEDERATION_MAX_SETUP_ENROLLMENT_BYTES + 8 * 1024])
            .send()
            .await
            .expect("oversized request response");
        assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .expect("cache control"),
            "no-store"
        );

        let response = reqwest::Client::new()
            .get(setup_url(fixture.invitation(2), FEDERATION_SETUP_STATUS_PATH).expect("URL"))
            .header(FEDERATION_SETUP_CORRELATION_HEADER, nonce.to_string())
            .send()
            .await
            .expect("invalid invitation response");
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .expect("cache control"),
            "no-store"
        );

        fixture.stop().await;
    }

    #[tokio::test]
    async fn timeout_retains_admission_until_owned_work_finishes_and_shutdown_waits() {
        let config = FederationSetupCoordinatorServiceConfig {
            request_deadline: Duration::from_millis(25),
            per_source_concurrency: 1,
            ..FederationSetupCoordinatorServiceConfig::default()
        };
        let fixture = setup_http_fixture(config).await;
        let coordinator_url = Url::parse(&fixture.invitation(2).coordinator_url.to_string())
            .expect("coordinator URL");
        let coordinator_address = format!(
            "{}:{}",
            coordinator_url.host_str().expect("coordinator host"),
            coordinator_url.port().expect("coordinator port")
        );
        let mut slow_client = tokio::net::TcpStream::connect(coordinator_address)
            .await
            .expect("slow client connection");
        let slow_request = format!(
            "POST {FEDERATION_SETUP_ENROLLMENTS_PATH} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\n{FEDERATION_SETUP_CORRELATION_HEADER}: {}\r\nContent-Type: application/json\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n{{",
            fixture
                .invitation(2)
                .to_invitation_code()
                .expect("invitation"),
            FederationSetupCorrelationNonce::from_bytes([3; 32]).expect("nonce")
        );
        slow_client
            .write_all(slow_request.as_bytes())
            .await
            .expect("partial slow request");
        let mut slow_response = vec![0; FEDERATION_SETUP_ERROR_RESPONSE_BYTES];
        let response_bytes =
            tokio::time::timeout(Duration::from_secs(1), slow_client.read(&mut slow_response))
                .await
                .expect("slow request was bounded")
                .expect("slow response read");
        let slow_response = String::from_utf8_lossy(&slow_response[..response_bytes]);
        assert!(slow_response.starts_with("HTTP/1.1 504"));
        assert!(slow_response
            .to_ascii_lowercase()
            .contains("cache-control: no-store"));

        fixture
            .faults
            .set_operation_delay(Some(Duration::from_millis(150)))
            .await;
        let client = FederationSetupHttpClient::default()
            .with_request_deadline(Duration::from_secs(1))
            .with_retry_attempts(1);
        let timeout = client
            .enroll(fixture.invitation(2), fixture.enrollment(2))
            .await
            .expect_err("slow owned operation times out");
        assert_eq!(timeout.code, FederationSetupHttpErrorCode::Timeout);
        let saturated = client
            .enroll(fixture.invitation(2), fixture.enrollment(2))
            .await
            .expect_err("timed-out operation retains admission");
        assert_eq!(saturated.code, FederationSetupHttpErrorCode::RateLimited);

        let started = tokio::time::Instant::now();
        fixture.service.shutdown().await;
        assert!(started.elapsed() >= Duration::from_millis(75));
        assert_eq!(fixture.service.active_tasks(), 0);
        let unavailable = client
            .enroll(fixture.invitation(2), fixture.enrollment(2))
            .await
            .expect_err("shutdown coordinator rejects new work");
        assert_eq!(unavailable.code, FederationSetupHttpErrorCode::Unavailable);

        let _ = fixture.shutdown.send(());
        fixture.server.await.expect("setup server task");
    }

    #[tokio::test]
    async fn concurrent_same_invitation_has_one_owner_and_exact_retry_succeeds() {
        let config = FederationSetupCoordinatorServiceConfig {
            per_invitation_concurrency: 1,
            ..FederationSetupCoordinatorServiceConfig::default()
        };
        let fixture = setup_http_fixture(config).await;
        fixture
            .faults
            .set_operation_delay(Some(Duration::from_millis(100)))
            .await;
        let client = FederationSetupHttpClient::default().with_retry_attempts(1);
        let (first, second) = tokio::join!(
            client.enroll(fixture.invitation(2), fixture.enrollment(2)),
            client.enroll(fixture.invitation(2), fixture.enrollment(2)),
        );
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        let error = first
            .err()
            .or_else(|| second.err())
            .expect("one saturation");
        assert_eq!(error.code, FederationSetupHttpErrorCode::RateLimited);
        fixture.faults.set_operation_delay(None).await;
        client
            .enroll(fixture.invitation(2), fixture.enrollment(2))
            .await
            .expect("exact retry after owner completes");
        fixture.stop().await;
    }

    #[tokio::test]
    async fn setup_client_never_follows_redirects_with_invitation_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("redirect listener");
        let address = listener.local_addr().expect("redirect address");
        let redirected_requests = Arc::new(AtomicUsize::new(0));
        let captured = redirected_requests.clone();
        let router = Router::new()
            .route(
                FEDERATION_SETUP_ENROLLMENTS_PATH,
                post(|| async { axum::response::Redirect::temporary("/captured") }),
            )
            .route(
                "/captured",
                post(move || {
                    let captured = captured.clone();
                    async move {
                        captured.fetch_add(1, Ordering::AcqRel);
                        StatusCode::OK
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("redirect server");
        });
        let fixture = setup_http_fixture(FederationSetupCoordinatorServiceConfig::default()).await;
        let mut invitation = fixture.invitation(2).clone();
        invitation.coordinator_url = mint_url(&format!("http://{address}"));
        let client = FederationSetupHttpClient::default().with_retry_attempts(1);
        client
            .enroll(&invitation, fixture.enrollment(2))
            .await
            .expect_err("redirect is rejected");
        assert_eq!(redirected_requests.load(Ordering::Acquire), 0);
        server.abort();
        let _ = server.await;
        fixture.stop().await;
    }

    async fn setup_http_fixture(
        config: FederationSetupCoordinatorServiceConfig,
    ) -> SetupHttpFixture {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("localhost setup listener");
        let address = listener.local_addr().expect("listener address");
        let coordinator_url = mint_url(&format!("http://{address}"));
        let now = current_unix_time().expect("time");
        let setup_id = FederationSetupId::from_bytes([42; 32]);
        let coordinator_secret = secret_key(60_000);
        let coordinator_identity =
            FederationSetupCoordinatorPublicKey::new(coordinator_secret.public_key())
                .expect("coordinator identity");
        let database = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("database"));
        let url_policy = FederationSetupUrlPolicy::AllowInsecureLocalhost;
        let member_secrets = (1..=3)
            .map(|member| (member_id(member), secret_key(1_000 + member)))
            .collect::<BTreeMap<_, _>>();
        let enrollments = (1..=3)
            .map(|member| {
                let member_id = member_id(member);
                let invitation_id = invitation_id(member);
                let enrollment = FederationSetupEnrollment::sign(
                    setup_id,
                    invitation_id,
                    member_id,
                    mint_url(&format!("http://127.0.0.1:{}", 30_000 + member)),
                    mint_url(&format!("http://127.0.0.1:{}", 31_000 + member)),
                    [u8::try_from(member).expect("member byte"); 32],
                    FederationSetupProtocolVersions::default(),
                    member_secrets.get(&member_id).expect("member secret"),
                    url_policy,
                )
                .expect("enrollment");
                (member_id, enrollment)
            })
            .collect::<BTreeMap<_, _>>();
        let invitations = (2..=3)
            .map(|member| {
                let member_id = member_id(member);
                let invitation = FederationSetupInvitation::new(
                    setup_id,
                    invitation_id(member),
                    member_id,
                    coordinator_url.clone(),
                    coordinator_identity,
                    now + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS + 60,
                    FederationSetupInvitationCapability::from_bytes(
                        [0xa0 + u8::try_from(member).expect("member byte"); 32],
                    )
                    .expect("capability"),
                    now,
                    url_policy,
                )
                .expect("invitation");
                (member_id, invitation)
            })
            .collect::<BTreeMap<_, _>>();
        let initial = FederationSetupSession::new(
            setup_id,
            FederationSetupLocalRole::Coordinator {
                member_id: member_id(1),
            },
            now,
        )
        .expect("session");
        let enrolled = initial
            .with_participant_phase(FederationSetupParticipantPhase::Enrolled, now)
            .expect("creator enrolled phase");
        let mut transaction = Database::begin_transaction(&*database)
            .await
            .expect("fixture transaction");
        transaction
            .add_federation_setup_session(&enrolled)
            .await
            .expect("session");
        transaction
            .add_federation_setup_record(FederationSetupRecord::Enrollment(
                enrollments.get(&member_id(1)).expect("creator"),
            ))
            .await
            .expect("creator enrollment");
        for invitation in invitations.values() {
            transaction
                .add_federation_setup_invitation(
                    &FederationSetupInvitationRecord::from_invitation(invitation, now)
                        .expect("invitation record"),
                )
                .await
                .expect("invitation");
        }
        transaction.commit().await.expect("commit fixture");
        let dynamic_database: DynMintDatabase = database.clone();
        let faults = Arc::new(TestSetupOperationFaults::default());
        let service = Arc::new(
            FederationSetupCoordinatorService::new(
                dynamic_database,
                setup_id,
                3,
                coordinator_secret.clone(),
                url_policy,
                config,
            )
            .expect("setup service")
            .with_faults(faults.clone()),
        );
        let router = create_federation_setup_router(service.clone());
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = shutdown_receiver.await;
            })
            .await
            .expect("setup server");
        });
        SetupHttpFixture {
            database,
            service,
            faults,
            invitations,
            enrollments,
            member_secrets,
            coordinator_secret,
            url_policy,
            shutdown,
            server,
        }
    }

    async fn publish_proposal(fixture: &SetupHttpFixture) -> FederationSetupProposal {
        let enrollments = fixture
            .database
            .get_federation_setup_enrollments(fixture.service.setup_id())
            .await
            .expect("enrollments")
            .into_iter()
            .map(|enrollment| (enrollment.member_id, enrollment))
            .collect();
        let network = FederationSetupBitcoinNetwork::Regtest;
        let proposal = FederationSetupProposal::sign(
            FederationSetupProposalBody::new(
                fixture.service.setup_id(),
                fixture.service.coordinator_identity(),
                enrollments,
                ThresholdParams::new(3, 2, 3).expect("thresholds"),
                FederationSetupProtocolVersions::default(),
                network,
                FederationPaymentObservationPolicy::member_quorum(3),
                FederationCheckpointPolicy::default(),
                vec![FederationDkgKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    [Amount::from(1), Amount::from(2)]
                        .into_iter()
                        .collect::<BTreeSet<_>>(),
                )
                .expect("keyset")],
                FederationSetupFrostPolicy::new(
                    1,
                    vec![FederationSetupFrostApplication::CdkOnchain {
                        application_key_epoch: 1,
                        confirmation_policy_version: 1,
                        minimum_confirmations: 1,
                        fee_policy_version: 1,
                    }],
                    network,
                )
                .expect("FROST policy"),
                fixture.url_policy,
            )
            .expect("proposal body"),
            &fixture.coordinator_secret,
            fixture.url_policy,
        )
        .expect("proposal");
        let session = fixture.service.session().await.expect("session");
        let published = session
            .with_coordinator_phase(
                FederationSetupCoordinatorPhase::ProposalPublished,
                current_unix_time().expect("time"),
            )
            .expect("published phase");
        let mut transaction = Database::begin_transaction(&*fixture.database)
            .await
            .expect("proposal transaction");
        transaction
            .advance_federation_setup_session(
                &session,
                &published,
                FederationSetupStep::PublishProposal(&proposal),
            )
            .await
            .expect("publish proposal");
        transaction.commit().await.expect("commit proposal");
        proposal
    }

    fn current_unix_time() -> Result<u64, FederationSetupHttpError> {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .map_err(|_| FederationSetupHttpError::storage())
    }

    fn setup_url(invitation: &FederationSetupInvitation, path: &str) -> Option<Url> {
        let mut url = Url::parse(&invitation.coordinator_url.to_string()).ok()?;
        url.set_path(path);
        Some(url)
    }

    fn member_id(value: u16) -> MemberId {
        MemberId::new(value).expect("member ID")
    }

    fn invitation_id(member: u16) -> FederationSetupInvitationId {
        FederationSetupInvitationId::from_bytes(
            [42_u8.wrapping_add(u8::try_from(member).expect("member byte")); 32],
        )
    }

    fn mint_url(value: &str) -> MintUrl {
        MintUrl::from_str(value).expect("mint URL")
    }

    fn secret_key(value: u16) -> SecretKey {
        let mut bytes = [0; 32];
        bytes[30..].copy_from_slice(&value.to_be_bytes());
        SecretKey::from_slice(&bytes).expect("secret key")
    }
}
