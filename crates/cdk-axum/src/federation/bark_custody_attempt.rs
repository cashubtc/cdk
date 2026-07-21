//! Shared accepted-attempt lookup for Bark custody orchestration.

use cdk::federation::{
    FederationBarkActionRecord, FederationBarkOperation, FederationBarkProjectionSnapshot,
    FederationBarkSemanticPhase, FederationJournalEntry, FederationOperation,
};
use cdk::mint::Mint;

/// Resolve the consensus entry authorizing the action's current custody
/// attempt. The action plan authorizes attempt one; a later attempt is
/// authorized only by its accepted `SigningAttemptAdvanced` transition.
pub(super) async fn accepted_bark_custody_authorization(
    mint: &Mint,
    projection: &FederationBarkProjectionSnapshot,
    action: &FederationBarkActionRecord,
    plan_entry: &FederationJournalEntry,
) -> Result<FederationJournalEntry, cdk::Error> {
    let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
        &plan_entry.envelope.operation
    else {
        return Err(cdk::Error::Internal);
    };
    if action.action_id != plan.action_id
        || action.plan_operation_id != Some(plan_entry.operation_id)
        || action.attempt < plan.attempt
    {
        return Err(cdk::Error::Internal);
    }
    if action.attempt == plan.attempt {
        return Ok(plan_entry.clone());
    }
    let operation_id = projection
        .transitions
        .iter()
        .filter_map(|record| match &record.operation {
            FederationBarkOperation::ActionTransition(transition)
                if record.action_id == action.action_id
                    && transition.phase == FederationBarkSemanticPhase::SigningAttemptAdvanced
                    && transition.plan_operation_id == plan_entry.operation_id
                    && transition.attempt == action.attempt =>
            {
                Some((record.operation_index, record.operation_id))
            }
            _ => None,
        })
        .max_by_key(|(operation_index, _)| *operation_index)
        .map(|(_, operation_id)| operation_id)
        .ok_or(cdk::Error::Internal)?;
    mint.localstore()
        .get_federation_journal_entry_by_operation_id(operation_id)
        .await?
        .ok_or(cdk::Error::Internal)
}
