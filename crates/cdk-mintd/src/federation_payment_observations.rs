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
        || report.quarantined
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
mod tests {
    use cdk_common::federation::FederationPaymentObservationReportAction;

    use super::*;

    fn report(
        context: FederationPaymentObservationContext,
        quote: &str,
        status: FederationPaymentObservationReportStatus,
    ) -> FederationPaymentObservationReportSummary {
        FederationPaymentObservationReportSummary {
            context,
            quote: quote.to_string(),
            required: 2,
            total_observations: 1,
            groups: Vec::new(),
            conflicting_observers: Vec::new(),
            competing_terminal_state_observers: Vec::new(),
            status,
            quote_age_secs: None,
            stale: false,
            recommended_action: FederationPaymentObservationReportAction::None,
            quarantined: false,
            quarantine_operation_ids: Vec::new(),
            quarantine_operation_indices: Vec::new(),
            quarantine_reasons: Vec::new(),
        }
    }

    #[test]
    fn payment_observations_default_to_actionable_reports() {
        let mut stale_quorum = report(
            FederationPaymentObservationContext::MintQuote,
            "quote-4",
            FederationPaymentObservationReportStatus::QuorumReached,
        );
        stale_quorum.stale = true;
        let mut quarantined_quorum = report(
            FederationPaymentObservationContext::MintQuote,
            "quote-5",
            FederationPaymentObservationReportStatus::QuorumReached,
        );
        quarantined_quorum.quarantined = true;
        let reports = vec![
            report(
                FederationPaymentObservationContext::MintQuote,
                "quote-1",
                FederationPaymentObservationReportStatus::AwaitingQuorum,
            ),
            report(
                FederationPaymentObservationContext::MintQuote,
                "quote-2",
                FederationPaymentObservationReportStatus::Conflicting,
            ),
            report(
                FederationPaymentObservationContext::MintQuote,
                "quote-3",
                FederationPaymentObservationReportStatus::QuorumReached,
            ),
            stale_quorum,
            quarantined_quorum,
        ];

        let matches = matching_reports(&reports, None, None, false);

        assert_eq!(matches.len(), 4);
        assert_eq!(matches[0].quote, "quote-1");
        assert_eq!(matches[1].quote, "quote-2");
        assert_eq!(matches[2].quote, "quote-4");
        assert_eq!(matches[3].quote, "quote-5");
    }

    #[test]
    fn payment_observations_filter_by_context_and_quote() {
        let reports = vec![
            report(
                FederationPaymentObservationContext::MintQuote,
                "quote-1",
                FederationPaymentObservationReportStatus::AwaitingQuorum,
            ),
            report(
                FederationPaymentObservationContext::MeltQuote,
                "quote-1",
                FederationPaymentObservationReportStatus::AwaitingQuorum,
            ),
            report(
                FederationPaymentObservationContext::MeltQuote,
                "quote-2",
                FederationPaymentObservationReportStatus::Conflicting,
            ),
        ];

        let matches = matching_reports(
            &reports,
            Some(FederationPaymentObservationContext::MeltQuote),
            Some("quote-1"),
            false,
        );

        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].context,
            FederationPaymentObservationContext::MeltQuote
        );
        assert_eq!(matches[0].quote, "quote-1");
    }

    #[test]
    fn payment_observations_can_include_resolved_reports() {
        let reports = vec![report(
            FederationPaymentObservationContext::MintQuote,
            "quote-1",
            FederationPaymentObservationReportStatus::QuorumReached,
        )];

        assert!(matching_reports(&reports, None, None, false).is_empty());
        assert_eq!(matching_reports(&reports, None, None, true).len(), 1);
    }
}
