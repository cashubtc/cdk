use cdk_common::terminal::escape_control;
use clap::{Args, Subcommand};

use crate::quote::resolve_melt_quote_request::Resolution;
use crate::quote::{CompensateMeltQuote, FinalizeMeltQuote, ResolveMeltQuoteRequest};
use crate::InterceptedQuoteServiceClient;

/// Apply an operator-verified outcome to an inspected melt operation.
#[derive(Debug, Args)]
pub struct ResolveMeltQuoteCommand {
    /// Quote ID to resolve.
    quote_id: String,
    /// Operation ID from list-melt-quotes.
    #[arg(long)]
    operation_id: String,
    /// Recovery stage from list-melt-quotes.
    #[arg(long, value_parser = ["setup_complete", "payment_attempted", "payment_pending", "payment_failed", "finalizing"])]
    expected_saga_state: String,
    /// Reason and external evidence for the decision, retained in the audit record.
    #[arg(long)]
    reason: String,
    #[command(subcommand)]
    action: ResolutionAction,
}

#[derive(Debug, Subcommand)]
enum ResolutionAction {
    /// Record a verified successful payment, spend inputs, and return change.
    Finalize {
        /// Actual total spent including payment fees, in the quote's unit.
        #[arg(long)]
        total_spent: u64,
        /// Quote currency unit, e.g. sat or msat.
        #[arg(long)]
        unit: String,
        /// Actual backend payment identifier.
        #[arg(long)]
        payment_lookup_id: String,
        /// Identifier kind, e.g. payment_hash, payment_id, or quote_id.
        #[arg(long)]
        payment_lookup_id_kind: String,
        /// Payment proof, when available (e.g. preimage or onchain outpoint).
        #[arg(long)]
        payment_proof: Option<String>,
    },
    /// Release inputs after verifying payment cannot complete; does not cancel it.
    Compensate {
        /// Assert payment failed and all backend attempts/retries are stopped.
        #[arg(long, required = true)]
        payment_failure_confirmed: bool,
    },
}

/// Resolve a melt through its normal finalization or compensation path.
pub async fn resolve_melt_quote(
    client: &mut InterceptedQuoteServiceClient,
    command: &ResolveMeltQuoteCommand,
) -> Result<(), tonic::Status> {
    let resolution = match &command.action {
        ResolutionAction::Finalize {
            total_spent,
            unit,
            payment_lookup_id,
            payment_lookup_id_kind,
            payment_proof,
        } => Resolution::Finalize(FinalizeMeltQuote {
            total_spent: *total_spent,
            unit: unit.clone(),
            payment_lookup_id: payment_lookup_id.clone(),
            payment_lookup_id_kind: payment_lookup_id_kind.clone(),
            payment_proof: payment_proof.clone(),
        }),
        ResolutionAction::Compensate {
            payment_failure_confirmed,
        } => Resolution::Compensate(CompensateMeltQuote {
            payment_failure_confirmed: *payment_failure_confirmed,
        }),
    };
    let response = client
        .resolve_melt_quote(ResolveMeltQuoteRequest {
            quote_id: command.quote_id.clone(),
            operation_id: command.operation_id.clone(),
            expected_saga_state: command.expected_saga_state.clone(),
            reason: command.reason.clone(),
            resolution: Some(resolution),
        })
        .await?
        .into_inner();
    println!(
        "quote_id: {}, operation_id: {}, state: {}, cleanup: complete",
        escape_control(&response.quote_id),
        escape_control(&response.operation_id),
        response
            .state()
            .as_str_name()
            .trim_start_matches("MELT_QUOTE_STATE_")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        command: ResolveMeltQuoteCommand,
    }

    #[test]
    fn compensation_requires_explicit_failure_confirmation() {
        let args = [
            "resolve-melt-quote",
            "quote",
            "--operation-id",
            "operation",
            "--expected-saga-state",
            "payment_pending",
            "--reason",
            "verified failure",
            "compensate",
        ];
        assert!(TestCli::try_parse_from(args).is_err());
        let cli = TestCli::try_parse_from(args.into_iter().chain(["--payment-failure-confirmed"]))
            .unwrap();
        assert!(matches!(
            cli.command.action,
            ResolutionAction::Compensate {
                payment_failure_confirmed: true
            }
        ));
    }
}
