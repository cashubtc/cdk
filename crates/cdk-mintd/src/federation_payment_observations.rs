use anyhow::{Context, Result};
use cdk_common::federation::{
    FederationConfig, FederationPaymentObservationContext,
    FederationPaymentObservationReportStatus, FederationPaymentObservationReportSummary,
    MemberConfigPrivate,
};
use cdk_http_client::Async;

use crate::cli::{
    CLIArgs, FederationPaymentObservationContextArg, FederationPaymentObservationsArgs,
};
use crate::federation_status::{
    payment_observation_group_line, payment_observation_report_line, query_local_statuses,
    FederationLocalStatusResponse,
};

pub(crate) async fn run_payment_observations_command(
    cli_args: &CLIArgs,
    args: &FederationPaymentObservationsArgs,
) -> Result<()> {
    let (public_config_path, private_config_path) = crate::federation_config_paths(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation payment-observations",
    )?;
    let public_config: FederationConfig =
        crate::read_federation_json(&public_config_path, "public federation config")?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(&private_config_path, "private federation config")?;
    let statuses = query_local_statuses(
        Async::default(),
        &public_config,
        &private_config,
        &args.members,
    )
    .await
    .context("failed to query signed federation payment-observation status")?;

    print_payment_observations(&public_config, &private_config, args, &statuses);

    Ok(())
}

fn print_payment_observations(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    args: &FederationPaymentObservationsArgs,
    statuses: &[FederationLocalStatusResponse],
) {
    println!(
        "Federation {} payment observations signed by member {}",
        public_config.federation_id, private_config.member_id
    );
    let context = args.context.map(context_from_arg);
    for status in statuses {
        let reports = matching_reports(
            &status.payment_observation_reports,
            context,
            args.quote.as_deref(),
            args.include_resolved,
        );
        println!(
            "member {}: matching_payment_reports={}",
            status.peer_status.member_id,
            reports.len()
        );
        for report in reports {
            println!("{}", payment_observation_report_line(report));
            for group in &report.groups {
                println!("{}", payment_observation_group_line(group));
            }
        }
    }
}

fn matching_reports<'a>(
    reports: &'a [FederationPaymentObservationReportSummary],
    context: Option<FederationPaymentObservationContext>,
    quote: Option<&str>,
    include_resolved: bool,
) -> Vec<&'a FederationPaymentObservationReportSummary> {
    reports
        .iter()
        .filter(|report| report_matches(report, context, quote, include_resolved))
        .collect()
}

fn report_matches(
    report: &FederationPaymentObservationReportSummary,
    context: Option<FederationPaymentObservationContext>,
    quote: Option<&str>,
    include_resolved: bool,
) -> bool {
    match context {
        Some(context) if report.context != context => return false,
        Some(_) | None => {}
    }
    match quote {
        Some(quote) if report.quote != quote => return false,
        Some(_) | None => {}
    }

    include_resolved || is_actionable_payment_report(report)
}

fn is_actionable_payment_report(report: &FederationPaymentObservationReportSummary) -> bool {
    matches!(
        report.status,
        FederationPaymentObservationReportStatus::AwaitingQuorum
            | FederationPaymentObservationReportStatus::Conflicting
    ) || report.stale
}

fn context_from_arg(
    value: FederationPaymentObservationContextArg,
) -> FederationPaymentObservationContext {
    match value {
        FederationPaymentObservationContextArg::MintQuote => {
            FederationPaymentObservationContext::MintQuote
        }
        FederationPaymentObservationContextArg::MeltQuote => {
            FederationPaymentObservationContext::MeltQuote
        }
    }
}

#[cfg(test)]
mod tests;
