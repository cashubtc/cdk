//! Thin persistence boundary for accepted on-chain intents and transactions.

use std::collections::BTreeMap;
use std::str::FromStr;

use cdk_common::bitcoin::address::NetworkUnchecked;
use cdk_common::bitcoin::Address;
use cdk_common::database;
use cdk_common::database::mint::FederationJournalEntryKey;
use cdk_common::error::Error;
use cdk_common::federation::{
    add_pending_federation_onchain_intent_in_tx, authorize_federation_transaction_broadcast_in_tx,
    load_federation_accepted_transaction_in_tx, load_federation_chain_utxos_in_tx,
    load_federation_onchain_intent_in_tx, load_federation_utxo_reservation_in_tx,
    mark_federation_transaction_signed_in_tx, read_federation_accepted_transaction,
    read_federation_chain_utxos, read_federation_state, read_federation_transaction_lineage,
    record_federation_transaction_broadcast_observation_in_tx,
    reserve_federation_chain_utxos_in_tx, FederationAcceptedTransaction,
    FederationAcceptedTransactionState, FederationChainObservationState, FederationError,
    FederationJournalEntry, FederationOnchainPaymentIntent, FederationOnchainPaymentIntentRecord,
    FederationOperation, FederationOperationId, FederationOperationStatus,
    FederationSignedTransaction, FederationTransactionBroadcastIntent,
    FederationTransactionBroadcastObservation, FederationTransactionProposal,
    FederationTransactionProposalId, FederationTransactionScope, FederationTransactionState,
    FederationTransactionStateOutcome, LoadedFederationTransactionLineage, MemberId,
};
use cdk_common::mint::{MeltPaymentRequest, MeltQuote};

use super::super::Mint;
use super::application::finish_federation_apply_transaction;
use super::FederationRuntimeConfig;

impl Mint {
    /// Load the complete validated outgoing transaction projection.
    pub async fn federation_transaction_state(&self) -> Result<FederationTransactionState, Error> {
        read_federation_state(&self.localstore()).await
    }

    /// Earliest journal entry an unfinished outgoing transaction still reads.
    ///
    /// Signing and broadcast read a transaction's accepted proposal entry
    /// until the transaction is confirmed, abandoned or replaced, so history
    /// deletion must keep it.
    pub async fn federation_transaction_retention_pin(&self) -> Result<Option<u64>, Error> {
        Ok(self
            .federation_transaction_state()
            .await?
            .accepted_transactions()
            .values()
            .filter(|accepted| {
                !matches!(
                    accepted.state,
                    FederationAcceptedTransactionState::Confirmed
                        | FederationAcceptedTransactionState::Abandoned
                        | FederationAcceptedTransactionState::Replaced { .. }
                )
            })
            .map(|accepted| accepted.accepted_operation_index)
            .min())
    }

    pub(super) async fn apply_federation_transaction_proposal(
        &self,
        entry: &FederationJournalEntry,
        proposal: &FederationTransactionProposal,
    ) -> Result<FederationTransactionStateOutcome, Error> {
        proposal.validate()?;
        let runtime = self.federation_config().ok_or(Error::Internal)?;
        let proposal_id = proposal.proposal_id()?;
        let outpoints = proposal
            .selected_inputs
            .iter()
            .map(|input| input.outpoint)
            .collect::<Vec<_>>();
        let mut transaction = self.localstore().begin_transaction().await?;
        let outcome = async {
            let mut lineage =
                LoadedFederationTransactionLineage::load(&mut transaction, proposal.batch_id)
                    .await?;
            let chain_state =
                load_federation_chain_utxos_in_tx(&mut transaction, &outpoints).await?;

            let outcome = match lineage.accepted_transaction(proposal_id) {
                Some(existing) => {
                    if existing.proposal != *proposal {
                        return Err(invalid(
                            "proposal replay differs from the accepted transaction",
                        ));
                    }
                    ensure_existing_reservations(&lineage, &chain_state, existing)?;
                    return Ok(FederationTransactionStateOutcome::Existing);
                }
                None => match proposal.replacement_of {
                    Some(original_id) => {
                        let original = match lineage.accepted_transaction(original_id).cloned() {
                            Some(original) => original,
                            None => load_federation_accepted_transaction_in_tx(
                                &mut transaction,
                                original_id,
                            )
                            .await?
                            .ok_or_else(|| invalid("replacement predecessor is not accepted"))?,
                        };
                        let policy_audit = runtime.validate_onchain_transaction_replacement(
                            &chain_state,
                            lineage.intents(),
                            &original,
                            proposal,
                        )?;
                        let outcome = lineage.accept_replacement(
                            proposal.clone(),
                            policy_audit,
                            entry.operation_id,
                            entry.index,
                        )?;
                        lineage.save(&mut transaction).await?;
                        outcome
                    }
                    None => {
                        if !lineage.accepted_transactions().is_empty() {
                            return Err(invalid(
                                "on-chain batch is already owned by another accepted transaction",
                            ));
                        }
                        for outpoint in &outpoints {
                            if load_federation_utxo_reservation_in_tx(&mut transaction, *outpoint)
                                .await?
                                .is_some_and(|owner| owner != proposal_id)
                            {
                                return Err(invalid(
                                    "wallet UTXO is already reserved by another accepted transaction",
                                ));
                            }
                        }
                        let mut target_intents = BTreeMap::new();
                        for intent_id in &proposal.intent_ids {
                            if let Some(record) =
                                load_federation_onchain_intent_in_tx(&mut transaction, *intent_id)
                                    .await?
                            {
                                target_intents.insert(*intent_id, record);
                            }
                        }
                        let policy_audit = runtime.validate_onchain_transaction_proposal(
                            &chain_state,
                            &target_intents,
                            proposal,
                        )?;
                        lineage.insert_pending_intents(target_intents)?;
                        let outcome = lineage.accept_transaction(
                            proposal.clone(),
                            policy_audit,
                            entry.operation_id,
                            entry.index,
                        )?;
                        lineage.save(&mut transaction).await?;
                        reserve_federation_chain_utxos_in_tx(&mut transaction, &outpoints).await?;
                        outcome
                    }
                },
            };

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
        let accepted = read_federation_accepted_transaction(&self.localstore(), proposal_id)
            .await?
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
            .get_federation_journal_entry(
                FederationJournalEntryKey::Operation(accepted.accepted_operation_id),
                None,
            )
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
        let lineage =
            read_federation_transaction_lineage(&self.localstore(), accepted.proposal.batch_id)
                .await?;
        let outpoints = accepted
            .proposal
            .selected_inputs
            .iter()
            .map(|input| input.outpoint)
            .collect::<Vec<_>>();
        let chain_state = read_federation_chain_utxos(&self.localstore(), &outpoints).await?;
        ensure_existing_reservations(&lineage, &chain_state, &accepted)?;
        let recomputed = runtime.revalidate_accepted_onchain_transaction_proposal(
            &chain_state,
            lineage.intents(),
            &accepted.proposal,
            accepted
                .proposal
                .replacement_of
                .and_then(|original_id| lineage.accepted_transaction(original_id)),
        )?;
        if recomputed != accepted.policy_audit {
            return Err(invalid(
                "accepted transaction policy audit differs from local recomputation",
            ));
        }
        Ok(accepted)
    }

    /// Validate a signed transaction against the exact local accepted proposal.
    pub async fn validate_federation_signed_transaction_submission(
        &self,
        signed: &FederationSignedTransaction,
    ) -> Result<(), Error> {
        let accepted = read_federation_accepted_transaction(&self.localstore(), signed.proposal_id)
            .await?
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
            let accepted =
                load_federation_accepted_transaction_in_tx(&mut transaction, signed.proposal_id)
                    .await?
                    .ok_or_else(|| invalid("cannot apply signing for an unknown transaction"))?;
            signed.verify_signatures(&accepted.proposal)?;
            mark_federation_transaction_signed_in_tx(
                &mut transaction,
                signed.proposal_id,
                signed.clone(),
            )
            .await
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
        let accepted = read_federation_accepted_transaction(&self.localstore(), intent.proposal_id)
            .await?
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
        let accepted =
            read_federation_accepted_transaction(&self.localstore(), observation.proposal_id)
                .await?
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
        let outcome = authorize_federation_transaction_broadcast_in_tx(
            &mut transaction,
            *intent,
            entry.operation_id,
            entry.index,
        )
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
        let outcome = record_federation_transaction_broadcast_observation_in_tx(
            &mut transaction,
            observation.clone(),
            entry.operation_id,
            entry.index,
        )
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
    add_pending_federation_onchain_intent_in_tx(
        transaction,
        FederationOnchainPaymentIntentRecord::pending(intent)?,
    )
    .await?;
    Ok(())
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

fn invalid(reason: &'static str) -> Error {
    FederationError::TransactionProposalInvalid { reason }.into()
}
