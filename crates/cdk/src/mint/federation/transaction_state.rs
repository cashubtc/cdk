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
    FederationOperationId, FederationOperationStatus, FederationSignedTransaction,
    FederationTransactionBroadcastIntent, FederationTransactionBroadcastObservation,
    FederationTransactionProposal, FederationTransactionProposalId, FederationTransactionScope,
    FederationTransactionState, FederationTransactionStateOutcome, MemberId,
};
use cdk_common::mint::{MeltPaymentRequest, MeltQuote};

use super::super::Mint;
use super::application::finish_federation_apply_transaction;
use super::chain_observation::{
    CHAIN_OBSERVATION_PRIMARY_NAMESPACE, CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
    CHAIN_OBSERVATION_STATE_KEY,
};
use super::FederationRuntimeConfig;

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
                None => match proposal.replacement_of {
                    Some(original_id) => {
                        let original = state
                            .accepted_transaction(original_id)
                            .cloned()
                            .ok_or_else(|| invalid("replacement predecessor is not accepted"))?;
                        let policy_audit = runtime.validate_onchain_transaction_replacement(
                            &chain_state,
                            state.intents(),
                            &original,
                            proposal,
                        )?;
                        state.accept_replacement(
                            proposal.clone(),
                            policy_audit,
                            entry.operation_id,
                            entry.index,
                        )?
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
                },
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
        let application = runtime.cdk_onchain_application()?;
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
            accepted
                .proposal
                .replacement_of
                .and_then(|original_id| state.accepted_transaction(original_id)),
        )?;
        if recomputed != accepted.policy_audit {
            return Err(invalid(
                "accepted transaction policy audit differs from local recomputation",
            ));
        }
        Ok(accepted.clone())
    }

    /// Validate a signed transaction against the exact local accepted proposal.
    pub async fn validate_federation_signed_transaction_submission(
        &self,
        signed: &FederationSignedTransaction,
    ) -> Result<(), Error> {
        let state = self.federation_transaction_state().await?;
        let accepted = state
            .accepted_transaction(signed.proposal_id)
            .ok_or_else(|| invalid("signed transaction has no accepted proposal"))?;
        if signed.policy_audit != accepted.policy_audit {
            return Err(invalid(
                "signed transaction policy audit differs from accepted state",
            ));
        }
        signed.verify_signatures(&accepted.proposal)?;
        let runtime = self
            .effective_federation_config()
            .await?
            .ok_or_else(|| invalid("signed transaction has no active federation config"))?;
        let wallet = runtime
            .public_config()
            .frost_wallet
            .as_ref()
            .ok_or_else(|| invalid("signed transaction has no FROST wallet config"))?;
        let application = wallet
            .cdk_onchain_application
            .as_ref()
            .ok_or_else(|| invalid("signed transaction has no CDK on-chain application"))?;
        if application.root_key_epoch != accepted.proposal.scope.root_key_epoch
            || application.application_id != accepted.proposal.scope.application_id
            || application.application_key_epoch != accepted.proposal.scope.application_key_epoch
        {
            return Err(invalid("signed transaction application epoch is unknown"));
        }
        let schedule = cdk_common::federation::FederationWalletLivenessSchedule::new(
            signed.proposal_id.to_bytes(),
            runtime.public_config().members.keys().copied(),
            application.threshold,
        )?;
        for audit in &signed.input_signatures {
            let expected = schedule.signing_plan(audit.attempt)?;
            let expected_signers = expected
                .signer_set
                .iter()
                .map(|member| member.get())
                .collect::<Vec<_>>();
            if audit.signer_set != expected_signers
                || audit.signer_set.iter().any(|member| {
                    MemberId::new(*member).map_or(true, |member| {
                        runtime.public_config().member(member).is_none()
                            || !application.verification_shares.contains_key(&member)
                    })
                })
            {
                return Err(invalid(
                    "signed transaction signer audit is below threshold or unregistered",
                ));
            }
        }
        if let Some(existing) = &accepted.signed {
            if existing != signed {
                return Err(invalid(
                    "signed transaction differs from the durable accepted record",
                ));
            }
        } else if accepted.state != FederationAcceptedTransactionState::Accepted {
            return Err(invalid("transaction is not eligible for signed submission"));
        }
        Ok(())
    }

    pub(super) async fn apply_federation_signed_transaction(
        &self,
        entry: &FederationJournalEntry,
        signed: &FederationSignedTransaction,
    ) -> Result<FederationTransactionStateOutcome, Error> {
        self.validate_federation_signed_transaction_submission(signed)
            .await?;
        let mut transaction = self.localstore().begin_transaction().await?;
        let outcome = async {
            let mut state = load_transaction_state(&mut transaction).await?;
            let accepted = state
                .accepted_transaction(signed.proposal_id)
                .ok_or_else(|| invalid("cannot apply signing for an unknown transaction"))?
                .clone();
            signed.verify_signatures(&accepted.proposal)?;
            let outcome = state.mark_transaction_signed(signed.proposal_id, signed.clone())?;
            transaction
                .kv_write(
                    TRANSACTION_STATE_PRIMARY_NAMESPACE,
                    TRANSACTION_STATE_SECONDARY_NAMESPACE,
                    TRANSACTION_STATE_KEY,
                    &state.to_bytes()?,
                )
                .await?;
            Ok(outcome)
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }

    /// Validate that broadcast intent names the exact durable signed transaction.
    pub async fn validate_federation_transaction_broadcast_intent_submission(
        &self,
        intent: &FederationTransactionBroadcastIntent,
    ) -> Result<(), Error> {
        intent.settlement_policy.validate()?;
        let state = self.federation_transaction_state().await?;
        let accepted = state
            .accepted_transaction(intent.proposal_id)
            .ok_or_else(|| invalid("broadcast intent has no accepted transaction"))?;
        let signed = accepted
            .signed
            .as_ref()
            .ok_or_else(|| invalid("broadcast intent precedes signed persistence"))?;
        if signed.txid != intent.txid {
            return Err(invalid(
                "broadcast intent txid differs from signed transaction",
            ));
        }
        let runtime = self
            .effective_federation_config()
            .await?
            .ok_or_else(|| invalid("broadcast intent has no federation config"))?;
        if intent.settlement_policy
            != runtime.onchain_transaction_settlement_policy(accepted.proposal.scope)?
        {
            return Err(invalid(
                "broadcast intent settlement policy differs from federation config",
            ));
        }
        if accepted.broadcast.is_none()
            && accepted.state != FederationAcceptedTransactionState::Signed
        {
            return Err(invalid("transaction is not eligible for broadcast intent"));
        }
        Ok(())
    }

    /// Validate an authenticated member result against durable broadcast intent.
    pub async fn validate_federation_transaction_broadcast_observation_submission(
        &self,
        observation: &FederationTransactionBroadcastObservation,
    ) -> Result<(), Error> {
        observation.validate()?;
        let runtime = self
            .effective_federation_config()
            .await?
            .ok_or_else(|| invalid("broadcast observation has no federation config"))?;
        if runtime
            .public_config()
            .member(observation.observer)
            .is_none()
        {
            return Err(invalid("broadcast observer is not a federation member"));
        }
        let state = self.federation_transaction_state().await?;
        let accepted = state
            .accepted_transaction(observation.proposal_id)
            .ok_or_else(|| invalid("broadcast observation has no accepted transaction"))?;
        let signed = accepted
            .signed
            .as_ref()
            .ok_or_else(|| invalid("broadcast observation precedes signed persistence"))?;
        if signed.txid != observation.txid || accepted.broadcast.is_none() {
            return Err(invalid("broadcast observation differs from durable intent"));
        }
        if let Some(existing) = accepted.broadcast.as_ref().and_then(|broadcast| {
            broadcast.observations.iter().find(|record| {
                record.observation.observer == observation.observer
                    && record.observation.attempt == observation.attempt
            })
        }) {
            if existing.observation != *observation {
                return Err(invalid(
                    "broadcast attempt replay carries a different result",
                ));
            }
        }
        Ok(())
    }

    pub(super) async fn apply_federation_transaction_broadcast_intent(
        &self,
        entry: &FederationJournalEntry,
        intent: &FederationTransactionBroadcastIntent,
    ) -> Result<FederationTransactionStateOutcome, Error> {
        self.validate_federation_transaction_broadcast_intent_submission(intent)
            .await?;
        let mut transaction = self.localstore().begin_transaction().await?;
        let outcome = async {
            let mut state = load_transaction_state(&mut transaction).await?;
            let outcome =
                state.authorize_transaction_broadcast(*intent, entry.operation_id, entry.index)?;
            transaction
                .kv_write(
                    TRANSACTION_STATE_PRIMARY_NAMESPACE,
                    TRANSACTION_STATE_SECONDARY_NAMESPACE,
                    TRANSACTION_STATE_KEY,
                    &state.to_bytes()?,
                )
                .await?;
            Ok(outcome)
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }

    pub(super) async fn apply_federation_transaction_broadcast_observation(
        &self,
        entry: &FederationJournalEntry,
        observation: &FederationTransactionBroadcastObservation,
    ) -> Result<FederationTransactionStateOutcome, Error> {
        self.validate_federation_transaction_broadcast_observation_submission(observation)
            .await?;
        let mut transaction = self.localstore().begin_transaction().await?;
        let outcome = async {
            let mut state = load_transaction_state(&mut transaction).await?;
            let outcome = state.record_transaction_broadcast_observation(
                observation.clone(),
                entry.operation_id,
                entry.index,
            )?;
            transaction
                .kv_write(
                    TRANSACTION_STATE_PRIMARY_NAMESPACE,
                    TRANSACTION_STATE_SECONDARY_NAMESPACE,
                    TRANSACTION_STATE_KEY,
                    &state.to_bytes()?,
                )
                .await?;
            Ok(outcome)
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }
}

pub(super) async fn persist_pending_onchain_intent(
    transaction: &mut database::mint::DynMintTransaction,
    runtime: &FederationRuntimeConfig,
    quote: &MeltQuote,
    melt_operation_id: FederationOperationId,
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
    let application = runtime.cdk_onchain_application()?;
    let unchecked = Address::<NetworkUnchecked>::from_str(address)
        .map_err(|_| invalid("on-chain melt destination address is invalid"))?;
    let destination = unchecked
        .require_network(application.network)
        .map_err(|_| invalid("on-chain melt destination uses the wrong Bitcoin network"))?;
    let fee_tier = quote
        .selected_fee_index
        .ok_or_else(|| invalid("on-chain melt has no selected fee tier"))?;
    let fee_target_blocks = quote
        .estimated_blocks
        .ok_or_else(|| invalid("on-chain melt has no selected confirmation target"))?;
    let intent = FederationOnchainPaymentIntent::new(
        quote.id.to_string(),
        melt_operation_id,
        destination.script_pubkey(),
        quote.amount().to_sat()?,
        quote.fee_reserve().to_sat()?,
        fee_tier,
        fee_target_blocks,
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
    let reservation_owner = state.lineage_reservation_owner(accepted.policy_audit.proposal_id)?;
    for input in &accepted.proposal.selected_inputs {
        let utxo = chain_state
            .utxo(input.outpoint)
            .ok_or_else(|| invalid("accepted transaction reservation has no wallet UTXO"))?;
        if state.reservation(input.outpoint) != Some(reservation_owner)
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
