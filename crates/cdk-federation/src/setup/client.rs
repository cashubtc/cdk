//! Authenticated client for the setup coordinator API: the operator side of
//! the setup ceremony.

use core::future::Future;
use std::time::Duration;

use async_trait::async_trait;
use cdk_common::federation::{
    FederationSetupApproval, FederationSetupCorrelationNonce, FederationSetupEnrollment,
    FederationSetupEnrollmentAcknowledgement, FederationSetupInvitation, FederationSetupManifest,
    FederationSetupProposal, FederationSetupResponseRouteKind, FederationSetupSignedResponse,
    FederationSetupStatus, FederationSetupUrlPolicy, FEDERATION_MAX_SETUP_ARTIFACT_BYTES,
    FEDERATION_MAX_SETUP_ENROLLMENT_BYTES, FEDERATION_MAX_SETUP_STATUS_BYTES,
    FEDERATION_SETUP_CORRELATION_HEADER,
};
use cdk_http_client::{HttpClient, HttpError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;
use uuid::Uuid;

use super::error::{
    FederationSetupHttpError, FederationSetupHttpErrorBody, FederationSetupHttpErrorCode,
};
use super::{
    FEDERATION_SETUP_APPROVALS_PATH, FEDERATION_SETUP_APPROVAL_REQUEST_BYTES,
    FEDERATION_SETUP_ENROLLMENTS_PATH, FEDERATION_SETUP_MANIFEST_PATH,
    FEDERATION_SETUP_PROPOSAL_PATH, FEDERATION_SETUP_STATUS_PATH,
};

#[cfg(feature = "iroh")]
fn setup_iroh_headers(
    authorization: &str,
    nonce: FederationSetupCorrelationNonce,
    json: bool,
) -> Result<Vec<(http::HeaderName, http::HeaderValue)>, HttpError> {
    let authorization = http::HeaderValue::from_str(authorization)
        .map_err(|_| HttpError::Other("invalid setup authorization header".to_string()))?;
    let nonce = http::HeaderValue::from_str(&nonce.to_string())
        .map_err(|_| HttpError::Other("invalid setup correlation header".to_string()))?;
    let mut headers = vec![
        (http::header::AUTHORIZATION, authorization),
        (
            http::HeaderName::from_static(FEDERATION_SETUP_CORRELATION_HEADER),
            nonce,
        ),
    ];
    if json {
        headers.push((
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        ));
    }
    Ok(headers)
}

/// Authenticated, no-redirect client for the setup-only coordinator API.
#[derive(Debug, Clone)]
pub struct FederationSetupHttpClient {
    client: HttpClient,
    #[cfg(feature = "iroh")]
    iroh_client: Option<cdk_iroh::IrohClient>,
    request_deadline: Duration,
    retry_attempts: usize,
}

impl Default for FederationSetupHttpClient {
    fn default() -> Self {
        Self {
            client: HttpClient::builder()
                .no_redirects()
                .build()
                .expect("default no-redirect setup client"),
            #[cfg(feature = "iroh")]
            iroh_client: None,
            request_deadline: Duration::from_secs(7),
            retry_attempts: 3,
        }
    }
}

impl FederationSetupHttpClient {
    /// Create an authenticated setup client backed by the mint-owned Iroh node.
    #[cfg(feature = "iroh")]
    #[must_use]
    pub fn with_iroh_node(node: cdk_iroh::IrohNode) -> Self {
        Self {
            iroh_client: Some(cdk_iroh::IrohClient::new(node)),
            ..Self::default()
        }
    }

    /// Bound each attempt of a request to `deadline` (default seven seconds).
    #[must_use]
    pub fn with_request_deadline(mut self, deadline: Duration) -> Self {
        self.request_deadline = deadline;
        self
    }

    /// Attempt each request at most `attempts` times (default three).
    #[must_use]
    pub fn with_retry_attempts(mut self, attempts: usize) -> Self {
        self.retry_attempts = attempts;
        self
    }

    /// Enroll an invitation's exact signed member record.
    pub async fn enroll(
        &self,
        invitation: &FederationSetupInvitation,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<FederationSetupEnrollment, FederationSetupHttpError> {
        let nonce = random_correlation_nonce()?;
        let response: FederationSetupSignedResponse<FederationSetupEnrollmentAcknowledgement> =
            self.post(
                invitation,
                FEDERATION_SETUP_ENROLLMENTS_PATH,
                nonce,
                enrollment,
                FEDERATION_MAX_SETUP_ENROLLMENT_BYTES,
            )
            .await?;
        response
            .validate(
                FederationSetupResponseRouteKind::EnrollmentAck,
                nonce,
                invitation.coordinator_identity,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
        response
            .body
            .validate(enrollment, nonce, invitation.coordinator_identity)
            .map_err(FederationSetupHttpError::from_federation)?;
        Ok(enrollment.clone())
    }

    /// Fetch and authenticate the immutable setup proposal.
    pub async fn proposal(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> Result<FederationSetupProposal, FederationSetupHttpError> {
        let nonce = random_correlation_nonce()?;
        let response: FederationSetupSignedResponse<FederationSetupProposal> = self
            .get(
                invitation,
                FEDERATION_SETUP_PROPOSAL_PATH,
                nonce,
                FEDERATION_MAX_SETUP_ARTIFACT_BYTES,
            )
            .await?;
        response
            .validate(
                FederationSetupResponseRouteKind::Proposal,
                nonce,
                invitation.coordinator_identity,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
        response
            .body
            .validate(url_policy_for(invitation))
            .map_err(FederationSetupHttpError::from_federation)?;
        if response.body.body.setup_id != invitation.setup_id
            || response.body.body.coordinator_identity != invitation.coordinator_identity
            || !response
                .body
                .body
                .enrollments
                .contains_key(&invitation.member_id)
        {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup proposal does not match the invitation",
            ));
        }
        Ok(response.body)
    }

    /// Submit and authenticate acknowledgement of one durable approval.
    pub async fn approve(
        &self,
        invitation: &FederationSetupInvitation,
        approval: &FederationSetupApproval,
    ) -> Result<FederationSetupApproval, FederationSetupHttpError> {
        let nonce = random_correlation_nonce()?;
        let response: FederationSetupSignedResponse<FederationSetupApproval> = self
            .post(
                invitation,
                FEDERATION_SETUP_APPROVALS_PATH,
                nonce,
                approval,
                FEDERATION_SETUP_APPROVAL_REQUEST_BYTES,
            )
            .await?;
        response
            .validate(
                FederationSetupResponseRouteKind::ApprovalAck,
                nonce,
                invitation.coordinator_identity,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
        if response.body != *approval {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup approval acknowledgement changed the approval",
            ));
        }
        Ok(response.body)
    }

    /// Fetch and authenticate the complete certified manifest.
    pub async fn manifest(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> Result<FederationSetupManifest, FederationSetupHttpError> {
        let nonce = random_correlation_nonce()?;
        let response: FederationSetupSignedResponse<FederationSetupManifest> = self
            .get(
                invitation,
                FEDERATION_SETUP_MANIFEST_PATH,
                nonce,
                FEDERATION_MAX_SETUP_ARTIFACT_BYTES,
            )
            .await?;
        response
            .validate(
                FederationSetupResponseRouteKind::Manifest,
                nonce,
                invitation.coordinator_identity,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
        response
            .body
            .validate(url_policy_for(invitation))
            .map_err(FederationSetupHttpError::from_federation)?;
        if response.body.proposal.body.setup_id != invitation.setup_id
            || response.body.proposal.body.coordinator_identity != invitation.coordinator_identity
        {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup manifest does not match the invitation",
            ));
        }
        Ok(response.body)
    }

    /// Fetch authenticated aggregate setup progress.
    pub async fn status(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> Result<FederationSetupStatus, FederationSetupHttpError> {
        let nonce = random_correlation_nonce()?;
        let response: FederationSetupSignedResponse<FederationSetupStatus> = self
            .get(
                invitation,
                FEDERATION_SETUP_STATUS_PATH,
                nonce,
                FEDERATION_MAX_SETUP_STATUS_BYTES,
            )
            .await?;
        response
            .validate(
                FederationSetupResponseRouteKind::Status,
                nonce,
                invitation.coordinator_identity,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
        response
            .body
            .validate()
            .map_err(FederationSetupHttpError::from_federation)?;
        if response.body.setup_id != invitation.setup_id {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup status does not match the invitation",
            ));
        }
        Ok(response.body)
    }

    async fn get<T>(
        &self,
        invitation: &FederationSetupInvitation,
        path: &str,
        nonce: FederationSetupCorrelationNonce,
        max_response_bytes: usize,
    ) -> Result<T, FederationSetupHttpError>
    where
        T: DeserializeOwned,
    {
        let url = setup_url(invitation, path)?;
        let authorization = format!(
            "Bearer {}",
            invitation
                .to_invitation_code()
                .map_err(FederationSetupHttpError::from_federation)?
        );
        self.retry_request(|| self.get_once(url.clone(), &authorization, nonce, max_response_bytes))
            .await
    }

    async fn post<P, T>(
        &self,
        invitation: &FederationSetupInvitation,
        path: &str,
        nonce: FederationSetupCorrelationNonce,
        payload: &P,
        max_response_bytes: usize,
    ) -> Result<T, FederationSetupHttpError>
    where
        P: Serialize + Sync,
        T: DeserializeOwned,
    {
        let url = setup_url(invitation, path)?;
        let authorization = format!(
            "Bearer {}",
            invitation
                .to_invitation_code()
                .map_err(FederationSetupHttpError::from_federation)?
        );
        let body = serde_json::to_vec(payload).map_err(|_| {
            FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::InvalidRequest,
                "setup request serialization failed",
            )
        })?;
        self.retry_request(|| {
            self.post_once(
                url.clone(),
                &authorization,
                nonce,
                body.clone(),
                max_response_bytes,
            )
        })
        .await
    }

    async fn get_once(
        &self,
        url: Url,
        authorization: &str,
        nonce: FederationSetupCorrelationNonce,
        max_response_bytes: usize,
    ) -> Result<cdk_http_client::RawResponse, HttpError> {
        #[cfg(feature = "iroh")]
        if url.scheme() == "iroh" {
            let client = self.iroh_client.as_ref().ok_or_else(|| {
                HttpError::Other("setup Iroh endpoint is not initialized".to_string())
            })?;
            return client
                .request_raw(
                    http::Method::GET,
                    url,
                    &setup_iroh_headers(authorization, nonce, false)?,
                    bytes::Bytes::new(),
                    max_response_bytes,
                )
                .await
                .map_err(HttpError::from);
        }
        self.client
            .get(url.as_str())
            .header("authorization", authorization)
            .header(FEDERATION_SETUP_CORRELATION_HEADER, nonce.to_string())
            .send_bounded(max_response_bytes)
            .await
    }

    async fn post_once(
        &self,
        url: Url,
        authorization: &str,
        nonce: FederationSetupCorrelationNonce,
        body: Vec<u8>,
        max_response_bytes: usize,
    ) -> Result<cdk_http_client::RawResponse, HttpError> {
        #[cfg(feature = "iroh")]
        if url.scheme() == "iroh" {
            let client = self.iroh_client.as_ref().ok_or_else(|| {
                HttpError::Other("setup Iroh endpoint is not initialized".to_string())
            })?;
            return client
                .request_raw(
                    http::Method::POST,
                    url,
                    &setup_iroh_headers(authorization, nonce, true)?,
                    bytes::Bytes::from(body),
                    max_response_bytes,
                )
                .await
                .map_err(HttpError::from);
        }
        let payload: serde_json::Value = serde_json::from_slice(&body)?;
        self.client
            .post(url.as_str())
            .header("authorization", authorization)
            .header(FEDERATION_SETUP_CORRELATION_HEADER, nonce.to_string())
            .json(&payload)
            .send_bounded(max_response_bytes)
            .await
    }

    async fn retry_request<T, F, Fut>(&self, mut request: F) -> Result<T, FederationSetupHttpError>
    where
        T: DeserializeOwned,
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<cdk_http_client::RawResponse, HttpError>>,
    {
        let mut delay = Duration::from_millis(100);
        for attempt in 0..self.retry_attempts {
            let result = tokio::time::timeout(self.request_deadline, request()).await;
            match result {
                Ok(Ok(response)) if response.is_success() => {
                    return response.json().await.map_err(map_transport_error);
                }
                Ok(Ok(response)) => {
                    let status = response.status();
                    let body = response
                        .json::<FederationSetupHttpErrorBody>()
                        .await
                        .map_err(map_transport_error)?;
                    let error = FederationSetupHttpError::new(body.code, body.code.safe_message());
                    if status < 500 || attempt + 1 == self.retry_attempts {
                        return Err(error);
                    }
                }
                Ok(Err(error)) => {
                    if attempt + 1 == self.retry_attempts {
                        return Err(map_transport_error(error));
                    }
                }
                Err(_) => {
                    if attempt + 1 == self.retry_attempts {
                        return Err(FederationSetupHttpError::timeout());
                    }
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(1));
        }
        Err(FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::Unavailable,
            "setup coordinator is unavailable",
        ))
    }
}

fn setup_url(
    invitation: &FederationSetupInvitation,
    path: &str,
) -> Result<Url, FederationSetupHttpError> {
    let mut url = Url::parse(&invitation.coordinator_url.to_string()).map_err(|_| {
        FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::InvalidInvitation,
            "setup coordinator URL is invalid",
        )
    })?;
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn random_correlation_nonce() -> Result<FederationSetupCorrelationNonce, FederationSetupHttpError> {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut bytes = [0; 32];
    bytes[..16].copy_from_slice(first.as_bytes());
    bytes[16..].copy_from_slice(second.as_bytes());
    FederationSetupCorrelationNonce::from_bytes(bytes)
        .map_err(FederationSetupHttpError::from_federation)
}

fn url_policy_for(invitation: &FederationSetupInvitation) -> FederationSetupUrlPolicy {
    match invitation
        .coordinator_url
        .to_string()
        .starts_with("http://")
    {
        true => FederationSetupUrlPolicy::AllowInsecureLocalhost,
        false => FederationSetupUrlPolicy::RequireSecureTransport,
    }
}

fn map_transport_error(error: HttpError) -> FederationSetupHttpError {
    match error {
        HttpError::Timeout => FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::Timeout,
            "setup request deadline elapsed",
        ),
        HttpError::ResponseTooLarge { .. } => FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::InvalidRequest,
            "setup response exceeds its configured bound",
        ),
        _ => FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::Unavailable,
            "setup coordinator is unavailable",
        ),
    }
}

/// The coordinator a joining or approving operator talks to.
#[async_trait]
pub trait FederationSetupCoordinator: Send + Sync {
    /// Enroll the invitation's signed member record.
    async fn enroll(
        &self,
        invitation: &FederationSetupInvitation,
        enrollment: &FederationSetupEnrollment,
    ) -> anyhow::Result<FederationSetupEnrollment>;

    /// Fetch the authenticated setup proposal.
    async fn proposal(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> anyhow::Result<FederationSetupProposal>;

    /// Submit one approval and authenticate its acknowledgement.
    async fn approve(
        &self,
        invitation: &FederationSetupInvitation,
        approval: &FederationSetupApproval,
    ) -> anyhow::Result<FederationSetupApproval>;
}

#[async_trait]
impl FederationSetupCoordinator for FederationSetupHttpClient {
    async fn enroll(
        &self,
        invitation: &FederationSetupInvitation,
        enrollment: &FederationSetupEnrollment,
    ) -> anyhow::Result<FederationSetupEnrollment> {
        Self::enroll(self, invitation, enrollment)
            .await
            .map_err(|error| {
                anyhow::Error::new(error).context("setup enrollment HTTP request failed")
            })
    }

    async fn proposal(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> anyhow::Result<FederationSetupProposal> {
        Self::proposal(self, invitation).await.map_err(|error| {
            anyhow::Error::new(error).context("setup proposal HTTP request failed")
        })
    }

    async fn approve(
        &self,
        invitation: &FederationSetupInvitation,
        approval: &FederationSetupApproval,
    ) -> anyhow::Result<FederationSetupApproval> {
        Self::approve(self, invitation, approval)
            .await
            .map_err(|error| {
                anyhow::Error::new(error).context("setup approval HTTP request failed")
            })
    }
}
