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
    ];

    let matches = matching_reports(&reports, None, None, false);

    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0].quote, "quote-1");
    assert_eq!(matches[1].quote, "quote-2");
    assert_eq!(matches[2].quote, "quote-4");
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
