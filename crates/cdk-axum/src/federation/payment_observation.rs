use std::collections::BTreeSet;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use cdk::federation::{
    FederationPaymentObservationQuarantineRecord, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, FederationPaymentObservationReportSummary,
};

use crate::MintState;

pub(super) async fn payment_observation_report_summaries(
    state: &MintState,
) -> Result<Vec<FederationPaymentObservationReportSummary>, cdk::Error> {
    let Some(runtime_config) = state.mint.federation_config() else {
        return Ok(Vec::new());
    };
    let db = state.mint.localstore();
    let entries = db.get_federation_journal_entries().await?;
    let mut report_keys = BTreeSet::new();

    for entry in &entries {
        if let Some(record) = FederationPaymentObservationRecord::from_journal_entry(entry) {
            report_keys.insert((
                record.observation.context(),
                record.observation.quote().to_string(),
            ));
        }
        if let Some(record) =
            FederationPaymentObservationQuarantineRecord::from_journal_entry(entry)
        {
            report_keys.insert((record.quarantine.context, record.quarantine.quote.clone()));
        }
    }
    let quarantines = entries
        .iter()
        .filter_map(FederationPaymentObservationQuarantineRecord::from_journal_entry)
        .collect::<Vec<_>>();

    let mut reports = Vec::with_capacity(report_keys.len());
    let stale_observation_secs = runtime_config
        .public_config()
        .payment_observation_policy
        .stale_observation_secs;
    let now = current_unix_time();
    for (context, quote) in report_keys {
        let required = runtime_config
            .public_config()
            .payment_verification_policy(context)
            .required_matching_observations();
        let observations = db
            .get_federation_payment_observations(context, &quote)
            .await?;
        let report = FederationPaymentObservationReport::from_records_with_quarantines(
            context,
            &quote,
            required,
            &observations,
            &quarantines,
        );
        let quote_age_secs =
            payment_observation_quote_age_secs(state, context, &quote, now).await?;
        reports.push(report.summary_with_quote_age(quote_age_secs, stale_observation_secs));
    }

    Ok(reports)
}

async fn payment_observation_quote_age_secs(
    state: &MintState,
    context: cdk::federation::FederationPaymentObservationContext,
    quote: &str,
    now: u64,
) -> Result<Option<u64>, cdk::Error> {
    let Ok(quote_id) = cdk::mint::QuoteId::from_str(quote) else {
        return Ok(None);
    };
    let db = state.mint.localstore();
    match context {
        cdk::federation::FederationPaymentObservationContext::MintQuote => Ok(db
            .get_mint_quote(&quote_id)
            .await?
            .map(|quote| now.saturating_sub(quote.created_time))),
        cdk::federation::FederationPaymentObservationContext::MeltQuote => Ok(db
            .get_melt_quote(&quote_id)
            .await?
            .map(|quote| now.saturating_sub(quote.created_time))),
    }
}

fn current_unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
