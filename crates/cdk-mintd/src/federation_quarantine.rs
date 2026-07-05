use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use cdk::mint::FederationRuntimeConfig;
use cdk_common::federation::{
    FederationConfig, FederationJournalAppendOutcome, FederationOperationId,
    FederationPaymentObservationContext, FederationPaymentObservationQuarantineReason,
    FederationRestMessageKind, FederationSignedRequest, FederationSubmitOperationRequest,
    FederationSubmitOperationResponse, MemberConfigPrivate, MemberConfigPublic, MemberId,
};
use cdk_http_client::{Async, Transport};

use crate::cli::{
    CLIArgs, FederationPaymentObservationContextArg,
    FederationPaymentObservationQuarantineReasonArg, FederationQuarantinePaymentObservationArgs,
};

pub(crate) async fn run_quarantine_payment_observation_command(
    cli_args: &CLIArgs,
    args: &FederationQuarantinePaymentObservationArgs,
) -> Result<()> {
    let (public_config_path, private_config_path) = crate::federation_config_paths(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation quarantine-payment-observation",
    )?;
    let public_config: FederationConfig =
        crate::read_federation_json(&public_config_path, "public federation config")?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(&private_config_path, "private federation config")?;
    let responses = submit_payment_observation_quarantine(
        Async::default(),
        &public_config,
        &private_config,
        args,
    )
    .await?;

    print_quarantine_submit_responses(&public_config, &private_config, &responses);

    Ok(())
}

async fn submit_payment_observation_quarantine<T>(
    transport: T,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    args: &FederationQuarantinePaymentObservationArgs,
) -> Result<Vec<FederationQuarantineSubmitResponse>>
where
    T: Transport,
{
    let runtime_config =
        FederationRuntimeConfig::new(public_config.clone(), private_config.clone())
            .context("private federation config does not match public federation config")?;
    let evidence_operation_ids = parse_operation_ids(&args.evidence_operation_ids)?;
    let envelope = runtime_config.payment_observation_quarantine(
        FederationPaymentObservationContext::from(args.context),
        &args.quote,
        FederationPaymentObservationQuarantineReason::from(args.reason),
        evidence_operation_ids,
    );
    let request = FederationSubmitOperationRequest::new(envelope);
    request.ensure_admissible_size()?;

    let operation_id = request.operation_id()?;
    let targets = federation_targets(public_config, &args.members)?;
    let mut sequence = initial_quarantine_sequence();
    let mut responses = Vec::with_capacity(targets.len());

    for member in targets {
        sequence = sequence.saturating_add(1);
        let response = submit_quarantine_operation(
            &transport,
            public_config,
            private_config,
            &member,
            sequence,
            &request,
        )
        .await?;
        if response.operation_id != operation_id {
            bail!(
                "federation member {} returned operation id {} for submitted operation {}",
                member.member_id,
                response.operation_id,
                operation_id
            );
        }
        responses.push(FederationQuarantineSubmitResponse {
            member_id: member.member_id,
            response,
        });
    }

    Ok(responses)
}

async fn submit_quarantine_operation<T>(
    transport: &T,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    member: &MemberConfigPublic,
    sequence: u64,
    request: &FederationSubmitOperationRequest,
) -> Result<FederationSubmitOperationResponse>
where
    T: Transport,
{
    let signed_request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        member.member_id,
        sequence,
        FederationRestMessageKind::SubmitOperation,
        request.clone(),
        private_config,
    )?;
    let url = member
        .federation_api_url
        .join_paths(&["federation", "v1", "operations"])?;
    Ok(transport.http_post(url, None, &signed_request).await?)
}

fn parse_operation_ids(values: &[String]) -> Result<Vec<FederationOperationId>> {
    values
        .iter()
        .map(|value| {
            FederationOperationId::from_str(value)
                .with_context(|| format!("invalid evidence operation id {value}"))
        })
        .collect()
}

fn federation_targets(
    public_config: &FederationConfig,
    selected_members: &[u16],
) -> Result<Vec<MemberConfigPublic>> {
    if selected_members.is_empty() {
        return Ok(public_config.members.values().cloned().collect());
    }

    selected_members
        .iter()
        .map(|member_id| {
            let member_id = MemberId::new(*member_id)?;
            public_config.member(member_id).cloned().ok_or_else(|| {
                anyhow::anyhow!("federation public config has no member {}", member_id)
            })
        })
        .collect()
}

fn print_quarantine_submit_responses(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    responses: &[FederationQuarantineSubmitResponse],
) {
    println!(
        "Federation {} payment observation quarantine signed by member {}",
        public_config.federation_id, private_config.member_id
    );
    for response in responses {
        println!("{}", quarantine_submit_response_line(response));
    }
}

fn quarantine_submit_response_line(response: &FederationQuarantineSubmitResponse) -> String {
    let (outcome, index) = match &response.response.outcome {
        FederationJournalAppendOutcome::Appended(entry) => ("appended", entry.index),
        FederationJournalAppendOutcome::Duplicate(entry) => ("duplicate", entry.index),
    };
    format!(
        "member {}: operation_id={} outcome={} index={}",
        response.member_id, response.response.operation_id, outcome, index
    )
}

fn initial_quarantine_sequence() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().saturating_mul(1_000))
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
struct FederationQuarantineSubmitResponse {
    member_id: MemberId,
    response: FederationSubmitOperationResponse,
}

impl From<FederationPaymentObservationContextArg> for FederationPaymentObservationContext {
    fn from(value: FederationPaymentObservationContextArg) -> Self {
        match value {
            FederationPaymentObservationContextArg::MintQuote => Self::MintQuote,
            FederationPaymentObservationContextArg::MeltQuote => Self::MeltQuote,
        }
    }
}

impl From<FederationPaymentObservationQuarantineReasonArg>
    for FederationPaymentObservationQuarantineReason
{
    fn from(value: FederationPaymentObservationQuarantineReasonArg) -> Self {
        match value {
            FederationPaymentObservationQuarantineReasonArg::ConflictingEvidence => {
                Self::ConflictingEvidence
            }
            FederationPaymentObservationQuarantineReasonArg::CompetingTerminalStates => {
                Self::CompetingTerminalStates
            }
            FederationPaymentObservationQuarantineReasonArg::OperatorHalt => Self::OperatorHalt,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use cdk_common::federation::{
        generate_trusted_dealer_configs, FederationOperation, FederationOperationEnvelope,
        InMemoryFederationJournal, ThresholdParams, TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    use cdk_common::mint_url::MintUrl;
    use cdk_common::{Amount, CurrencyUnit, SecretKey};
    use cdk_http_client::HttpError;
    use serde::de::DeserializeOwned;
    use serde::Serialize;
    use url::Url;

    use super::*;

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
                let url = MintUrl::from_str(&format!("http://127.0.0.1:808{}", id + 4))
                    .expect("member url");
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

        let responses = submit_payment_observation_quarantine(
            transport,
            &public_config,
            &private_config,
            &args,
        )
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
}
