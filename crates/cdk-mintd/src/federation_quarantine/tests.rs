use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cdk_common::federation::{
    generate_trusted_dealer_configs, FederationOperation, FederationOperationEnvelope,
    FederationSignedRequest, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    InMemoryFederationJournal, MemberConfigPrivate, MemberId, ThresholdParams,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::{Amount, CurrencyUnit, SecretKey};
use cdk_http_client::{HttpError, RawResponse, Transport};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use super::*;
use crate::cli::{
    FederationPaymentObservationContextArg, FederationPaymentObservationQuarantineReasonArg,
    FederationQuarantinePaymentObservationArgs,
};

#[derive(Debug, Clone, Default)]
struct MockSubmitTransport {
    inner: Arc<Mutex<MockSubmitTransportInner>>,
}

#[derive(Debug, Default)]
struct MockSubmitTransportInner {
    journals: BTreeMap<MemberId, InMemoryFederationJournal>,
    requests: Vec<FederationSignedRequest<FederationSubmitOperationRequest>>,
    urls: Vec<Url>,
}

#[async_trait]
impl Transport for MockSubmitTransport {
    fn with_proxy(
        &mut self,
        _proxy: Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Ok(Vec::new())
    }

    async fn http_get<R>(
        &self,
        _url: Url,
        _auth: Option<cdk_common::nuts::nut22::AuthToken>,
    ) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        Err(HttpError::Other("unexpected GET".to_string()))
    }

    async fn http_get_raw(
        &self,
        _url: Url,
        _auth: Option<cdk_common::nuts::nut22::AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other("unexpected raw GET".to_string()))
    }

    async fn http_post<P, R>(
        &self,
        url: Url,
        _auth_token: Option<cdk_common::nuts::nut22::AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        let request_json =
            serde_json::to_value(payload).map_err(|err| HttpError::Other(err.to_string()))?;
        let request: FederationSignedRequest<FederationSubmitOperationRequest> =
            serde_json::from_value(request_json)
                .map_err(|err| HttpError::Other(err.to_string()))?;
        let response = {
            let mut inner = self.inner.lock().expect("mock transport lock");
            inner.urls.push(url);
            inner.requests.push(request.clone());
            let journal = inner.journals.entry(request.receiver).or_default();
            let outcome = journal
                .append(request.payload.envelope.clone())
                .map_err(|err| HttpError::Other(err.to_string()))?;
            FederationSubmitOperationResponse::from_outcome(outcome)
        };
        let response_json =
            serde_json::to_value(response).map_err(|err| HttpError::Other(err.to_string()))?;
        serde_json::from_value(response_json).map_err(|err| HttpError::Other(err.to_string()))
    }

    async fn http_post_form_raw<P>(
        &self,
        _url: Url,
        _auth_token: Option<cdk_common::nuts::nut22::AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        Err(HttpError::Other("unexpected raw form POST".to_string()))
    }
}

impl MockSubmitTransport {
    fn requests(&self) -> Vec<FederationSignedRequest<FederationSubmitOperationRequest>> {
        self.inner
            .lock()
            .expect("mock transport lock")
            .requests
            .clone()
    }

    fn urls(&self) -> Vec<Url> {
        self.inner.lock().expect("mock transport lock").urls.clone()
    }
}

fn quarantine_fixture() -> (FederationConfig, MemberConfigPrivate) {
    let members = (1..=3)
        .map(|id| {
            let member_id = MemberId::new(id).expect("member id");
            let url =
                MintUrl::from_str(&format!("http://127.0.0.1:808{}", id + 4)).expect("member url");
            TrustedDealerMember::new(member_id, url.clone(), url, SecretKey::generate())
        })
        .collect::<Vec<_>>();
    let unit = CurrencyUnit::Sat;
    let mut amount_coefficients = BTreeMap::new();
    amount_coefficients.insert(
        Amount::from(1),
        vec![
            SecretKey::generate_bls().as_bls().expect("bls key").clone(),
            SecretKey::generate_bls().as_bls().expect("bls key").clone(),
        ],
    );
    let keyset_spec = TrustedDealerKeysetSpec::new(unit, true, 0, None, amount_coefficients);
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(3, 2).expect("threshold"),
        members,
        vec![keyset_spec],
    )
    .expect("trusted dealer bundle");
    let private = bundle
        .private_configs
        .get(&MemberId::new(1).expect("member id"))
        .expect("private config")
        .clone();
    (bundle.public_config, private)
}

fn args() -> FederationQuarantinePaymentObservationArgs {
    FederationQuarantinePaymentObservationArgs {
        public_config: None,
        private_config: None,
        context: FederationPaymentObservationContextArg::MintQuote,
        quote: "quote-1".to_string(),
        reason: FederationPaymentObservationQuarantineReasonArg::ConflictingEvidence,
        evidence_operation_ids: vec![
            "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        ],
        members: vec![2, 3],
    }
}

#[tokio::test]
async fn quarantine_submit_signs_operation_requests_for_selected_members() {
    let (public_config, private_config) = quarantine_fixture();
    let transport = MockSubmitTransport::default();

    let responses = submit_payment_observation_quarantine(
        transport.clone(),
        &public_config,
        &private_config,
        &args(),
    )
    .await
    .expect("quarantine submit");

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].member_id, MemberId::new(2).expect("member id"));
    assert_eq!(responses[1].member_id, MemberId::new(3).expect("member id"));
    assert_eq!(
        responses[0].response.operation_id,
        responses[1].response.operation_id
    );

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].sender, private_config.member_id);
    assert_eq!(requests[0].receiver, MemberId::new(2).expect("member id"));
    assert_eq!(requests[1].receiver, MemberId::new(3).expect("member id"));
    assert!(requests[1].sequence > requests[0].sequence);
    requests[0]
        .verify(&public_config, MemberId::new(2).expect("member id"))
        .expect("first request verifies");
    requests[1]
        .verify(&public_config, MemberId::new(3).expect("member id"))
        .expect("second request verifies");

    let FederationOperation::PaymentObservationQuarantine(quarantine) =
        &requests[0].payload.envelope.operation
    else {
        panic!("expected quarantine operation");
    };
    assert_eq!(
        quarantine.context,
        FederationPaymentObservationContext::MintQuote
    );
    assert_eq!(quarantine.quote, "quote-1");
    assert_eq!(
        quarantine.reason,
        FederationPaymentObservationQuarantineReason::ConflictingEvidence
    );
    assert_eq!(quarantine.evidence_operation_ids.len(), 1);

    let urls = transport.urls();
    assert_eq!(
        urls[0].as_str(),
        "http://127.0.0.1:8086/federation/v1/operations"
    );
    assert_eq!(
        urls[1].as_str(),
        "http://127.0.0.1:8087/federation/v1/operations"
    );
}

#[tokio::test]
async fn quarantine_submit_defaults_to_all_members() {
    let (public_config, private_config) = quarantine_fixture();
    let mut args = args();
    args.members.clear();
    let transport = MockSubmitTransport::default();

    let responses =
        submit_payment_observation_quarantine(transport, &public_config, &private_config, &args)
            .await
            .expect("quarantine submit");

    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0].member_id, MemberId::new(1).expect("member id"));
    assert_eq!(responses[1].member_id, MemberId::new(2).expect("member id"));
    assert_eq!(responses[2].member_id, MemberId::new(3).expect("member id"));
}

#[test]
fn quarantine_response_line_redacts_operation_payload() {
    let (public_config, private_config) = quarantine_fixture();
    let runtime_config =
        FederationRuntimeConfig::new(public_config, private_config).expect("runtime config");
    let envelope: FederationOperationEnvelope = runtime_config.payment_observation_quarantine(
        FederationPaymentObservationContext::MintQuote,
        "quote-secret",
        FederationPaymentObservationQuarantineReason::OperatorHalt,
        Vec::new(),
    );
    let mut journal = InMemoryFederationJournal::new();
    let outcome = journal.append(envelope).expect("append quarantine");
    let response = FederationQuarantineSubmitResponse {
        member_id: MemberId::new(1).expect("member id"),
        response: FederationSubmitOperationResponse::from_outcome(outcome),
    };

    let line = quarantine_submit_response_line(&response);

    assert!(line.contains("member 1:"));
    assert!(line.contains("operation_id="));
    assert!(line.contains("outcome=appended"));
    assert!(!line.contains("quote-secret"));
}
