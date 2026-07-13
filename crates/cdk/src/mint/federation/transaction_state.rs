//! Thin persistence boundary for accepted on-chain intents and transactions.

use std::str::FromStr;

use cdk_common::bitcoin::address::NetworkUnchecked;
use cdk_common::bitcoin::Address;
use cdk_common::database;
use cdk_common::error::Error;
use cdk_common::federation::{
    FederationAcceptedTransaction, FederationAcceptedTransactionState,
    FederationChainObservationState, FederationError, FederationJournalEntry,
    FederationOnchainPaymentIntent, FederationOnchainPaymentIntentRecord, FederationOperation,
    FederationOperationStatus, FederationTransactionProposal, FederationTransactionProposalId,
    FederationTransactionScope, FederationTransactionState, FederationTransactionStateOutcome,
};
use cdk_common::mint::{MeltPaymentRequest, MeltQuote};

use super::super::Mint;
use super::chain_observation::{
    CHAIN_OBSERVATION_PRIMARY_NAMESPACE, CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
    CHAIN_OBSERVATION_STATE_KEY,
};
use super::{finish_federation_apply_transaction, FederationRuntimeConfig};

pub(super) const TRANSACTION_STATE_PRIMARY_NAMESPACE: &str = "federation";
pub(super) const TRANSACTION_STATE_SECONDARY_NAMESPACE: &str = "transaction";
pub(super) const TRANSACTION_STATE_KEY: &str = "state_v1";

impl Mint {
    /// Load the complete validated outgoing transaction projection.
    pub async fn federation_transaction_state(&self) -> Result<FederationTransactionState, Error> {
        match self
            .localstore()
            .kv_read(
                TRANSACTION_STATE_PRIMARY_NAMESPACE,
                TRANSACTION_STATE_SECONDARY_NAMESPACE,
                TRANSACTION_STATE_KEY,
            )
            .await?
        {
            Some(bytes) => Ok(FederationTransactionState::from_bytes(&bytes)?),
            None => Ok(FederationTransactionState::new()),
        }
    }

    pub(super) async fn apply_federation_transaction_proposal(
        &self,
        entry: &FederationJournalEntry,
        proposal: &FederationTransactionProposal,
    ) -> Result<FederationTransactionStateOutcome, Error> {
        proposal.validate()?;
        let runtime = self.federation_config().ok_or(Error::Internal)?;
        let proposal_id = proposal.proposal_id()?;
        let mut transaction = self.localstore().begin_transaction().await?;
        let outcome = async {
            let mut state = load_transaction_state(&mut transaction).await?;
            let mut chain_state = load_chain_state(&mut transaction).await?;

            let outcome = match state.accepted_transaction(proposal_id) {
                Some(existing) => {
                    if existing.proposal != *proposal {
                        return Err(invalid(
                            "proposal replay differs from the accepted transaction",
                        ));
                    }
                    ensure_existing_reservations(&state, &chain_state, existing)?;
                    FederationTransactionStateOutcome::Existing
                }
                None => {
                    let policy_audit = runtime.validate_onchain_transaction_proposal(
                        &chain_state,
                        state.intents(),
                        proposal,
                    )?;
                    let outcome = state.accept_transaction(
                        proposal.clone(),
                        policy_audit,
                        entry.operation_id,
                        entry.index,
                    )?;
                    let outpoints = proposal
                        .selected_inputs
                        .iter()
                        .map(|input| input.outpoint)
                        .collect::<Vec<_>>();
                    chain_state.reserve_utxos(&outpoints)?;
                    outcome
                }
            };

            transaction
                .kv_write(
                    TRANSACTION_STATE_PRIMARY_NAMESPACE,
                    TRANSACTION_STATE_SECONDARY_NAMESPACE,
                    TRANSACTION_STATE_KEY,
                    &state.to_bytes()?,
                )
                .await?;
            transaction
                .kv_write(
                    CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
                    CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
                    CHAIN_OBSERVATION_STATE_KEY,
                    &chain_state.to_bytes()?,
                )
                .await?;
            Ok(outcome)
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }

    /// Authorize FROST signing only for an exact locally applied transaction.
    pub async fn federation_transaction_signing_authorization(
        &self,
        proposal_id: FederationTransactionProposalId,
    ) -> Result<FederationAcceptedTransaction, Error> {
        let state = self.federation_transaction_state().await?;
        let accepted = state
            .accepted_transaction(proposal_id)
            .ok_or_else(|| invalid("transaction is not accepted for signing"))?;
        if accepted.state != FederationAcceptedTransactionState::Accepted {
            return Err(invalid("transaction is not active for signing"));
        }
        let runtime = self
            .effective_federation_config()
            .await?
            .ok_or_else(|| invalid("transaction signing has no active federation config"))?;
        let application = runtime.active_cdk_onchain_application()?;
        accepted
            .proposal
            .validate_scope(FederationTransactionScope {
                root_key_epoch: application.root_key_epoch,
                application_id: application.application_id,
                application_key_epoch: application.application_key_epoch,
                network: application.network,
                policy_version: application.fee_policy_version,
            })?;
        let entry = self
            .localstore()
            .get_federation_journal_entry_by_operation_id(accepted.accepted_operation_id)
            .await?
            .ok_or_else(|| invalid("accepted transaction has no local journal entry"))?;
        if entry.status() != FederationOperationStatus::Applied {
            return Err(invalid(
                "transaction signing is forbidden before local accepted-state apply",
            ));
        }
        let FederationOperation::TransactionProposal(applied_proposal) = &entry.envelope.operation
        else {
            return Err(invalid(
                "accepted transaction journal entry has the wrong operation kind",
            ));
        };
        if applied_proposal.proposal_id()? != proposal_id || applied_proposal != &accepted.proposal
        {
            return Err(invalid(
                "accepted transaction journal payload differs from durable state",
            ));
        }
        let chain_state = self.federation_chain_observation_state().await?;
        ensure_existing_reservations(&state, &chain_state, accepted)?;
        let recomputed = runtime.revalidate_accepted_onchain_transaction_proposal(
            &chain_state,
            state.intents(),
            &accepted.proposal,
        )?;
        if recomputed != accepted.policy_audit {
            return Err(invalid(
                "accepted transaction policy audit differs from local recomputation",
            ));
        }
        Ok(accepted.clone())
    }
}

pub(super) async fn persist_pending_onchain_intent(
    transaction: &mut database::mint::DynMintTransaction,
    runtime: &FederationRuntimeConfig,
    quote: &MeltQuote,
    accepted_time: u64,
) -> Result<(), Error> {
    let MeltPaymentRequest::Onchain { address } = &quote.request else {
        return Ok(());
    };
    if runtime.public_config().frost_wallet.is_none() {
        return Ok(());
    }
    if quote.state != cdk_common::MeltQuoteState::Pending {
        return Err(invalid(
            "FROST on-chain melt did not remain pending for transaction batching",
        ));
    }
    let application = runtime.active_cdk_onchain_application()?;
    let unchecked = Address::<NetworkUnchecked>::from_str(address)
        .map_err(|_| invalid("on-chain melt destination address is invalid"))?;
    let destination = unchecked
        .require_network(application.network)
        .map_err(|_| invalid("on-chain melt destination uses the wrong Bitcoin network"))?;
    let fee_tier = quote
        .selected_fee_index
        .ok_or_else(|| invalid("on-chain melt has no selected fee tier"))?;
    let intent = FederationOnchainPaymentIntent::new(
        quote.id.to_string(),
        destination.script_pubkey(),
        quote.amount().to_sat()?,
        quote.fee_reserve().to_sat()?,
        fee_tier,
        accepted_time,
        FederationTransactionScope {
            root_key_epoch: application.root_key_epoch,
            application_id: application.application_id,
            application_key_epoch: application.application_key_epoch,
            network: application.network,
            policy_version: application.fee_policy_version,
        },
    )?;
    let mut state = load_transaction_state(transaction).await?;
    state.add_pending_intent(FederationOnchainPaymentIntentRecord::pending(intent)?)?;
    transaction
        .kv_write(
            TRANSACTION_STATE_PRIMARY_NAMESPACE,
            TRANSACTION_STATE_SECONDARY_NAMESPACE,
            TRANSACTION_STATE_KEY,
            &state.to_bytes()?,
        )
        .await?;
    Ok(())
}

async fn load_transaction_state(
    transaction: &mut database::mint::DynMintTransaction,
) -> Result<FederationTransactionState, Error> {
    match transaction
        .kv_read(
            TRANSACTION_STATE_PRIMARY_NAMESPACE,
            TRANSACTION_STATE_SECONDARY_NAMESPACE,
            TRANSACTION_STATE_KEY,
        )
        .await?
    {
        Some(bytes) => Ok(FederationTransactionState::from_bytes(&bytes)?),
        None => Ok(FederationTransactionState::new()),
    }
}

async fn load_chain_state(
    transaction: &mut database::mint::DynMintTransaction,
) -> Result<FederationChainObservationState, Error> {
    match transaction
        .kv_read(
            CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
            CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
            CHAIN_OBSERVATION_STATE_KEY,
        )
        .await?
    {
        Some(bytes) => Ok(FederationChainObservationState::from_bytes(&bytes)?),
        None => Ok(FederationChainObservationState::new()),
    }
}

fn ensure_existing_reservations(
    state: &FederationTransactionState,
    chain_state: &FederationChainObservationState,
    accepted: &FederationAcceptedTransaction,
) -> Result<(), Error> {
    let proposal_id = accepted.policy_audit.proposal_id;
    for input in &accepted.proposal.selected_inputs {
        let utxo = chain_state
            .utxo(input.outpoint)
            .ok_or_else(|| invalid("accepted transaction reservation has no wallet UTXO"))?;
        if state.reservation(input.outpoint) != Some(proposal_id)
            || utxo.state != cdk_common::federation::FederationWalletUtxoState::Reserved
        {
            return Err(invalid(
                "accepted transaction wallet reservation is not materialized",
            ));
        }
    }
    Ok(())
}

fn invalid(reason: impl Into<String>) -> Error {
    FederationError::TransactionProposalInvalid {
        reason: reason.into(),
    }
    .into()
}
