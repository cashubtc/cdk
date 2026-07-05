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
mod tests;
