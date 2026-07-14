use std::collections::BTreeMap;

use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::transaction::Version;
use bitcoin::{
    Amount as BitcoinAmount, BlockHash, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn,
    TxOut, Txid, Witness,
};
use cdk_common::federation::{
    FederationAcceptedTransaction, FederationAcceptedTransactionState, FederationAddressAllocation,
    FederationAddressKeychain, FederationAllocatedAddressRef, FederationChainObservationState,
    FederationChainSourceDescriptor, FederationChainSourceScope, FederationDepositConfirmation,
    FederationDepositObservation, FederationFrostInputSignatureAudit,
    FederationOnchainPaymentBatch, FederationOnchainPaymentIntent,
    FederationOnchainPaymentIntentId, FederationOnchainPaymentIntentRecord,
    FederationOnchainPaymentIntentState, FederationOperationId, FederationSignedTransaction,
    FederationTaprootSighashType, FederationTransactionBroadcastIntent,
    FederationTransactionChangeOutput, FederationTransactionInput,
    FederationTransactionPolicyAudit, FederationTransactionProposal,
    FederationTransactionRecipientOutput, FederationTransactionScope,
    FederationTransactionSettlementPolicy, FederationTransactionState, FederationWalletUtxoState,
};

use super::address_allocation::runtime_with_address_application;
use super::*;

#[derive(Debug, Clone, Copy)]
struct ProposalOptions {
    fee_sat: u64,
    version: Version,
    lock_time: LockTime,
    sequence: Sequence,
    change_script_index: u32,
    declared_change_index: u32,
}

impl Default for ProposalOptions {
    fn default() -> Self {
        Self {
            fee_sat: 500,
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            change_script_index: 8,
            declared_change_index: 8,
        }
    }
}

fn transaction_scope(runtime: &FederationRuntimeConfig) -> FederationTransactionScope {
    let application = runtime
        .active_cdk_onchain_application()
        .expect("active application");
    FederationTransactionScope {
        root_key_epoch: application.root_key_epoch,
        application_id: application.application_id,
        application_key_epoch: application.application_key_epoch,
        network: application.network,
        policy_version: application.fee_policy_version,
    }
}

fn wallet_script(
    runtime: &FederationRuntimeConfig,
    keychain: FederationAddressKeychain,
    index: u32,
) -> ScriptBuf {
    let scope = transaction_scope(runtime);
    runtime
        .public_config()
        .frost_wallet
        .as_ref()
        .expect("wallet config")
        .derive_cdk_onchain_script(
            runtime.public_config(),
            scope.application_id,
            scope.application_key_epoch,
            keychain,
            index,
        )
        .expect("wallet script")
}

fn spendable_input(
    runtime: &FederationRuntimeConfig,
    state: &mut FederationChainObservationState,
    marker: u8,
    value_sat: u64,
) -> FederationTransactionInput {
    let scope = transaction_scope(runtime);
    let outpoint = OutPoint::new(Txid::from_byte_array([marker; 32]), u32::from(marker));
    let script = wallet_script(
        runtime,
        FederationAddressKeychain::External,
        u32::from(marker),
    );
    let allocation = FederationAddressAllocation {
        quote: format!("deposit-{marker}"),
        keychain: FederationAddressKeychain::External,
        derivation_index: u32::from(marker),
        operation_id: FederationOperationId::from_bytes([marker; 32]),
        operation_index: u64::from(marker),
    };
    let allocation_ref = FederationAllocatedAddressRef {
        application_id: scope.application_id,
        application_key_epoch: scope.application_key_epoch,
        quote: allocation.quote.clone(),
        keychain: allocation.keychain,
        derivation_index: allocation.derivation_index,
    };
    let txout = TxOut {
        value: BitcoinAmount::from_sat(value_sat),
        script_pubkey: script.clone(),
    };
    let observation = FederationDepositObservation {
        allocation: allocation_ref.clone(),
        outpoint,
        txout: txout.clone(),
        confirmation: Some(FederationDepositConfirmation {
            block_hash: BlockHash::from_byte_array([marker.saturating_add(40); 32]),
            block_height: 100 + u32::from(marker),
            confirmation_depth: 2,
        }),
        observer: runtime.member_id(),
        source: FederationChainSourceDescriptor::new(
            FederationChainSourceScope::Independent,
            format!("policy-source-{marker}"),
        )
        .expect("source"),
    };
    state
        .apply_deposit(
            &observation,
            &allocation,
            script.as_script(),
            1,
            1,
            FederationOperationId::from_bytes([marker.saturating_add(80); 32]),
            100 + u64::from(marker),
        )
        .expect("spendable deposit");
    assert_eq!(
        state.utxo(outpoint).expect("accepted UTXO").state,
        FederationWalletUtxoState::Spendable
    );
    FederationTransactionInput {
        outpoint,
        previous_output: txout,
        allocation: allocation_ref,
    }
}

fn unaccepted_input(
    runtime: &FederationRuntimeConfig,
    marker: u8,
    value_sat: u64,
) -> FederationTransactionInput {
    let scope = transaction_scope(runtime);
    let outpoint = OutPoint::new(Txid::from_byte_array([marker; 32]), u32::from(marker));
    FederationTransactionInput {
        outpoint,
        previous_output: TxOut {
            value: BitcoinAmount::from_sat(value_sat),
            script_pubkey: wallet_script(
                runtime,
                FederationAddressKeychain::External,
                u32::from(marker),
            ),
        },
        allocation: FederationAllocatedAddressRef {
            application_id: scope.application_id,
            application_key_epoch: scope.application_key_epoch,
            quote: format!("unaccepted-{marker}"),
            keychain: FederationAddressKeychain::External,
            derivation_index: u32::from(marker),
        },
    }
}

fn intents(
    runtime: &FederationRuntimeConfig,
) -> (
    FederationOnchainPaymentBatch,
    BTreeMap<FederationOnchainPaymentIntentId, FederationOnchainPaymentIntentRecord>,
) {
    let scope = transaction_scope(runtime);
    let batch = FederationOnchainPaymentBatch::new(
        vec![
            FederationOnchainPaymentIntent::new(
                "melt-quote-b".to_string(),
                FederationOperationId::from_bytes([21; 32]),
                wallet_script(runtime, FederationAddressKeychain::External, 50),
                15_000,
                1_000,
                1,
                6,
                20,
                scope,
            )
            .expect("intent b"),
            FederationOnchainPaymentIntent::new(
                "melt-quote-a".to_string(),
                FederationOperationId::from_bytes([22; 32]),
                wallet_script(runtime, FederationAddressKeychain::External, 51),
                10_000,
                1_000,
                0,
                1,
                10,
                scope,
            )
            .expect("intent a"),
        ],
        20,
    )
    .expect("batch");
    let records = batch
        .intents
        .iter()
        .cloned()
        .map(|intent| {
            let record =
                FederationOnchainPaymentIntentRecord::pending(intent).expect("pending intent");
            (record.intent_id, record)
        })
        .collect();
    (batch, records)
}

fn build_proposal(
    runtime: &FederationRuntimeConfig,
    batch: &FederationOnchainPaymentBatch,
    selected_inputs: Vec<FederationTransactionInput>,
    options: ProposalOptions,
) -> FederationTransactionProposal {
    build_proposal_with_recipient_override(runtime, batch, selected_inputs, options, None)
}

fn build_proposal_with_recipient_override(
    runtime: &FederationRuntimeConfig,
    batch: &FederationOnchainPaymentBatch,
    selected_inputs: Vec<FederationTransactionInput>,
    options: ProposalOptions,
    first_recipient_override: Option<(ScriptBuf, u64)>,
) -> FederationTransactionProposal {
    let input_sum = selected_inputs
        .iter()
        .map(|input| input.previous_output.value.to_sat())
        .sum::<u64>();
    let mut outputs = batch
        .intents
        .iter()
        .enumerate()
        .map(|(index, intent)| TxOut {
            value: BitcoinAmount::from_sat(
                first_recipient_override
                    .as_ref()
                    .filter(|_| index == 0)
                    .map_or(intent.amount_sat, |(_, amount_sat)| *amount_sat),
            ),
            script_pubkey: first_recipient_override
                .as_ref()
                .filter(|_| index == 0)
                .map_or_else(
                    || intent.destination_script.clone(),
                    |(script, _)| script.clone(),
                ),
        })
        .collect::<Vec<_>>();
    let recipient_sum = outputs
        .iter()
        .map(|output| output.value.to_sat())
        .sum::<u64>();
    let change = TxOut {
        value: BitcoinAmount::from_sat(input_sum - recipient_sum - options.fee_sat),
        script_pubkey: wallet_script(
            runtime,
            FederationAddressKeychain::Internal,
            options.change_script_index,
        ),
    };
    outputs.push(change.clone());
    let transaction = Transaction {
        version: options.version,
        lock_time: options.lock_time,
        input: selected_inputs
            .iter()
            .map(|input| TxIn {
                previous_output: input.outpoint,
                script_sig: ScriptBuf::new(),
                sequence: options.sequence,
                witness: Witness::new(),
            })
            .collect(),
        output: outputs.clone(),
    };
    let intent_count = batch.intent_ids.len() as u64;
    let base_fee = options.fee_sat / intent_count;
    let remainder = options.fee_sat % intent_count;
    let recipients = batch
        .intent_ids
        .iter()
        .copied()
        .enumerate()
        .map(|(index, intent_id)| FederationTransactionRecipientOutput {
            intent_id,
            output_index: index as u32,
            output: outputs[index].clone(),
            fee_contribution_sat: base_fee + u64::from(index == 0) * remainder,
        })
        .collect();
    FederationTransactionProposal::new(
        batch,
        runtime.member_id(),
        transaction,
        selected_inputs,
        recipients,
        Some(FederationTransactionChangeOutput {
            derivation_index: options.declared_change_index,
            output_index: batch.intent_ids.len() as u32,
            output: change,
        }),
        options.fee_sat,
        None,
    )
    .expect("structurally valid proposal")
}

fn state_with_lifecycle(
    state: &FederationChainObservationState,
    outpoint: OutPoint,
    lifecycle: &str,
) -> FederationChainObservationState {
    let mut value = serde_json::to_value(state).expect("state JSON");
    value["utxos"][outpoint.to_string()]["state"] = serde_json::json!(lifecycle);
    serde_json::from_value(value).expect("mutated state")
}

fn replacement_proposal(
    runtime: &FederationRuntimeConfig,
    batch: &FederationOnchainPaymentBatch,
    original: &FederationTransactionProposal,
    additional_fee_sat: u64,
) -> FederationTransactionProposal {
    let mut transaction = original.unsigned_transaction.clone();
    let original_change = original.change_output.as_ref().expect("original change");
    let change_index = original_change.output_index as usize;
    transaction.output[change_index].value = transaction.output[change_index]
        .value
        .checked_sub(BitcoinAmount::from_sat(additional_fee_sat))
        .expect("replacement change");
    let fee_sat = original.fee_sat + additional_fee_sat;
    let mut recipients = original.recipient_outputs.clone();
    let base_fee = fee_sat / recipients.len() as u64;
    let remainder = fee_sat % recipients.len() as u64;
    for (index, recipient) in recipients.iter_mut().enumerate() {
        recipient.fee_contribution_sat = base_fee + u64::from(index == 0) * remainder;
    }
    let mut change = original_change.clone();
    change.output = transaction.output[change_index].clone();
    FederationTransactionProposal::new(
        batch,
        runtime.member_id(),
        transaction,
        original.selected_inputs.clone(),
        recipients,
        Some(change),
        fee_sat,
        Some(original.proposal_id().expect("original id")),
    )
    .expect("replacement proposal")
}

fn structurally_signed(
    proposal: FederationTransactionProposal,
    policy_audit: FederationTransactionPolicyAudit,
) -> FederationAcceptedTransaction {
    let mut accepted = FederationAcceptedTransaction {
        proposal,
        policy_audit,
        accepted_operation_id: FederationOperationId::from_bytes([70; 32]),
        accepted_operation_index: 70,
        state: FederationAcceptedTransactionState::Accepted,
        signed: None,
        broadcast: None,
    };
    let mut transaction = accepted.proposal.unsigned_transaction.clone();
    let input_signatures = transaction
        .input
        .iter_mut()
        .enumerate()
        .map(|(input_index, input)| {
            let signature = vec![u8::try_from(input_index + 31).expect("signature byte"); 64];
            input.witness.push(&signature);
            FederationFrostInputSignatureAudit {
                input_index: input_index as u32,
                session_id: [u8::try_from(input_index + 41).expect("session byte"); 32],
                attempt: 1,
                signer_set: vec![1, 2],
                package_hash: [u8::try_from(input_index + 51).expect("package byte"); 32],
                signature,
            }
        })
        .collect();
    accepted.signed = Some(
        FederationSignedTransaction::new(&accepted, &transaction, input_signatures)
            .expect("structurally signed original"),
    );
    accepted.state = FederationAcceptedTransactionState::Broadcast;
    accepted
}

#[test]
fn accepted_consensus_state_validates_without_a_local_bdk_cache_and_allows_coin_selection() {
    let (runtime, _) = runtime_with_address_application();
    let mut state = FederationChainObservationState::new();
    let first = spendable_input(&runtime, &mut state, 1, 35_000);
    let second = spendable_input(&runtime, &mut state, 2, 20_000);
    let third = spendable_input(&runtime, &mut state, 3, 40_000);
    let (batch, records) = intents(&runtime);

    let compact = build_proposal(
        &runtime,
        &batch,
        vec![first.clone()],
        ProposalOptions::default(),
    );
    let alternate = build_proposal(
        &runtime,
        &batch,
        vec![second, third],
        ProposalOptions::default(),
    );
    let compact_audit = runtime
        .validate_onchain_transaction_proposal(&state, &records, &compact)
        .expect("consensus state is sufficient without local BDK data");
    let alternate_audit = runtime
        .validate_onchain_transaction_proposal(&state, &records, &alternate)
        .expect("different BDK coin selection is independently valid");
    assert_ne!(compact_audit.txid, alternate_audit.txid);
    assert_eq!(compact_audit.taproot_sighashes.len(), 1);
    assert_eq!(alternate_audit.taproot_sighashes.len(), 2);
    assert_eq!(compact_audit.fee_sat, 500);
    assert_eq!(compact_audit.fee_contributions.values().sum::<u64>(), 500);

    let single_batch = FederationOnchainPaymentBatch::new(
        vec![batch.intents[0].clone()],
        batch.eligibility_cutoff,
    )
    .expect("single-intent batch");
    let single_records = single_batch
        .intents
        .iter()
        .cloned()
        .map(|intent| {
            let record = FederationOnchainPaymentIntentRecord::pending(intent)
                .expect("single pending intent");
            (record.intent_id, record)
        })
        .collect();
    let single = build_proposal(
        &runtime,
        &single_batch,
        vec![first],
        ProposalOptions::default(),
    );
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &single_records, &single)
        .is_ok());
}

#[test]
fn retiring_epoch_sweep_requires_one_active_destination_and_rejects_mixed_inputs() {
    let (mut runtime, application_id) = runtime_with_address_application();
    let source_scope = transaction_scope(&runtime);
    let mut chain_state = FederationChainObservationState::new();
    let input = spendable_input(&runtime, &mut chain_state, 9, 40_000);
    let wallet = runtime
        .public_config
        .frost_wallet
        .as_mut()
        .expect("wallet config");
    wallet.cdk_onchain_applications[0].state =
        cdk_common::federation::FederationFrostEpochState::Retiring;
    let mut destination_application = wallet.cdk_onchain_applications[0].clone();
    destination_application.application_key_epoch = 4;
    destination_application.state = cdk_common::federation::FederationFrostEpochState::Active;
    wallet
        .cdk_onchain_applications
        .push(destination_application.clone());
    wallet
        .active_epochs
        .application_key_epochs
        .insert(application_id, 4);
    let destination_scope = FederationTransactionScope {
        root_key_epoch: destination_application.root_key_epoch,
        application_id,
        application_key_epoch: 4,
        network: destination_application.network,
        policy_version: destination_application.fee_policy_version,
    };
    let destination_script = wallet
        .derive_cdk_onchain_script_for_known_epoch(
            application_id,
            4,
            FederationAddressKeychain::Internal,
            9,
        )
        .expect("destination script");
    let output = TxOut {
        value: BitcoinAmount::from_sat(39_500),
        script_pubkey: destination_script,
    };
    let proposal = FederationTransactionProposal::new_epoch_sweep(
        source_scope,
        destination_scope,
        FederationAllocatedAddressRef {
            application_id,
            application_key_epoch: 4,
            quote: "accepted-sweep-destination".to_string(),
            keychain: FederationAddressKeychain::Internal,
            derivation_index: 9,
        },
        77,
        runtime.member_id(),
        Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: input.outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![output.clone()],
        },
        vec![input],
        FederationTransactionChangeOutput {
            derivation_index: 9,
            output_index: 0,
            output,
        },
        500,
    )
    .expect("sweep proposal");
    runtime
        .validate_onchain_transaction_proposal(&chain_state, &BTreeMap::new(), &proposal)
        .expect("retiring-to-active sweep policy");

    let mut mixed = proposal.clone();
    mixed.selected_inputs[0].allocation.application_key_epoch = 4;
    assert!(runtime
        .validate_onchain_transaction_proposal(&chain_state, &BTreeMap::new(), &mixed)
        .is_err());
    let mut inactive_destination = runtime.clone();
    inactive_destination
        .public_config
        .frost_wallet
        .as_mut()
        .expect("wallet")
        .cdk_onchain_applications[1]
        .state = cdk_common::federation::FederationFrostEpochState::Proposed;
    assert!(inactive_destination
        .validate_onchain_transaction_proposal(&chain_state, &BTreeMap::new(), &proposal)
        .is_err());
}

#[test]
fn unavailable_or_non_spendable_utxos_and_non_pending_intents_fail_closed() {
    let (runtime, _) = runtime_with_address_application();
    let mut state = FederationChainObservationState::new();
    let accepted = spendable_input(&runtime, &mut state, 4, 35_000);
    let (batch, records) = intents(&runtime);
    let proposal = build_proposal(
        &runtime,
        &batch,
        vec![accepted.clone()],
        ProposalOptions::default(),
    );

    let unknown = build_proposal(
        &runtime,
        &batch,
        vec![unaccepted_input(&runtime, 9, 35_000)],
        ProposalOptions::default(),
    );
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &records, &unknown)
        .is_err());
    for lifecycle in ["reserved", "spent", "reorged", "quarantined"] {
        let unavailable = state_with_lifecycle(&state, accepted.outpoint, lifecycle);
        assert!(runtime
            .validate_onchain_transaction_proposal(&unavailable, &records, &proposal)
            .is_err());
    }
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &BTreeMap::new(), &proposal)
        .is_err());
    for lifecycle in [
        FederationOnchainPaymentIntentState::Accepted {
            proposal_id: proposal.proposal_id().expect("proposal id"),
        },
        FederationOnchainPaymentIntentState::Completed {
            txid: proposal.unsigned_transaction.compute_txid(),
        },
        FederationOnchainPaymentIntentState::Failed,
    ] {
        let mut unavailable = records.clone();
        unavailable
            .get_mut(&proposal.intent_ids[0])
            .expect("intent")
            .state = lifecycle;
        assert!(runtime
            .validate_onchain_transaction_proposal(&state, &unavailable, &proposal)
            .is_err());
    }
}

#[test]
fn destination_change_fee_and_transaction_policy_are_independently_enforced() {
    let (runtime, _) = runtime_with_address_application();
    let mut state = FederationChainObservationState::new();
    let input = spendable_input(&runtime, &mut state, 5, 40_000);
    let (batch, records) = intents(&runtime);

    let wrong_change = build_proposal(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions {
            declared_change_index: 9,
            ..ProposalOptions::default()
        },
    );
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &records, &wrong_change)
        .is_err());

    let excessive_fee = build_proposal(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions {
            fee_sat: 2_500,
            ..ProposalOptions::default()
        },
    );
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &records, &excessive_fee)
        .is_err());

    let below_minimum_fee_rate = build_proposal(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions {
            fee_sat: 1,
            ..ProposalOptions::default()
        },
    );
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &records, &below_minimum_fee_rate)
        .is_err());

    for options in [
        ProposalOptions {
            version: Version::ONE,
            ..ProposalOptions::default()
        },
        ProposalOptions {
            lock_time: LockTime::from_height(1).expect("height"),
            ..ProposalOptions::default()
        },
        ProposalOptions {
            sequence: Sequence::MAX,
            ..ProposalOptions::default()
        },
    ] {
        let invalid = build_proposal(&runtime, &batch, vec![input.clone()], options);
        assert!(runtime
            .validate_onchain_transaction_proposal(&state, &records, &invalid)
            .is_err());
    }

    let mut wrong_sighash = build_proposal(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions::default(),
    );
    wrong_sighash.sighash_type = FederationTaprootSighashType::AllAnyoneCanPay;
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &records, &wrong_sighash)
        .is_err());

    let mut wrong_network = build_proposal(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions::default(),
    );
    wrong_network.scope.network = Network::Bitcoin;
    for selected in &mut wrong_network.selected_inputs {
        selected.allocation.application_key_epoch = wrong_network.scope.application_key_epoch;
    }
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &records, &wrong_network)
        .is_err());

    let mut wrong_epoch = build_proposal(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions::default(),
    );
    wrong_epoch.scope.root_key_epoch += 1;
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &records, &wrong_epoch)
        .is_err());

    let wrong_destination = build_proposal_with_recipient_override(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions::default(),
        Some((
            wallet_script(&runtime, FederationAddressKeychain::External, 99),
            batch.intents[0].amount_sat,
        )),
    );
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &records, &wrong_destination)
        .is_err());

    let wrong_amount = build_proposal_with_recipient_override(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions::default(),
        Some((
            batch.intents[0].destination_script.clone(),
            batch.intents[0].amount_sat + 1,
        )),
    );
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &records, &wrong_amount)
        .is_err());

    let scope = transaction_scope(&runtime);
    let dust_batch = FederationOnchainPaymentBatch::new(
        vec![FederationOnchainPaymentIntent::new(
            "dust-intent".to_string(),
            FederationOperationId::from_bytes([23; 32]),
            wallet_script(&runtime, FederationAddressKeychain::External, 100),
            1,
            1_000,
            0,
            1,
            1,
            scope,
        )
        .expect("dust intent")],
        1,
    )
    .expect("dust batch");
    let dust_records = dust_batch
        .intents
        .iter()
        .cloned()
        .map(|intent| {
            let record =
                FederationOnchainPaymentIntentRecord::pending(intent).expect("dust record");
            (record.intent_id, record)
        })
        .collect();
    let dust = build_proposal(
        &runtime,
        &dust_batch,
        vec![input],
        ProposalOptions::default(),
    );
    assert!(runtime
        .validate_onchain_transaction_proposal(&state, &dust_records, &dust)
        .is_err());
}

#[test]
fn rbf_replacements_preserve_recipients_inputs_lineage_and_fee_caps() {
    let (runtime, _) = runtime_with_address_application();
    let mut chain_state = FederationChainObservationState::new();
    let input = spendable_input(&runtime, &mut chain_state, 17, 40_000);
    let (batch, pending_records) = intents(&runtime);
    let original = build_proposal(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions::default(),
    );
    let original_audit = runtime
        .validate_onchain_transaction_proposal(&chain_state, &pending_records, &original)
        .expect("initial transaction policy");
    let accepted = structurally_signed(original.clone(), original_audit);
    let original_id = original.proposal_id().expect("original id");
    let mut accepted_records = pending_records;
    for record in accepted_records.values_mut() {
        record.state = FederationOnchainPaymentIntentState::Accepted {
            proposal_id: original_id,
        };
    }
    let reserved_chain = state_with_lifecycle(&chain_state, input.outpoint, "reserved");

    let valid = replacement_proposal(&runtime, &batch, &original, 500);
    let valid_audit = runtime
        .validate_onchain_transaction_replacement(
            &reserved_chain,
            &accepted_records,
            &accepted,
            &valid,
        )
        .expect("valid bounded RBF replacement");
    assert_eq!(valid_audit.fee_sat, 1_000);
    assert_eq!(valid_audit.fee_contributions.values().sum::<u64>(), 1_000);
    assert!(valid
        .recipient_outputs
        .iter()
        .zip(&original.recipient_outputs)
        .all(|(replacement, original)| replacement.output == original.output));

    let insufficient_delta = replacement_proposal(&runtime, &batch, &original, 1);
    assert!(runtime
        .validate_onchain_transaction_replacement(
            &reserved_chain,
            &accepted_records,
            &accepted,
            &insufficient_delta,
        )
        .is_err());

    let mut recipient_transaction = valid.unsigned_transaction.clone();
    recipient_transaction.output[0].value = recipient_transaction.output[0]
        .value
        .checked_sub(BitcoinAmount::from_sat(1))
        .expect("mutated recipient");
    let mut recipient_outputs = valid.recipient_outputs.clone();
    recipient_outputs[0].output = recipient_transaction.output[0].clone();
    recipient_outputs[0].fee_contribution_sat += 1;
    let recipient_mutation = FederationTransactionProposal::new(
        &batch,
        runtime.member_id(),
        recipient_transaction,
        valid.selected_inputs.clone(),
        recipient_outputs,
        valid.change_output.clone(),
        valid.fee_sat + 1,
        Some(original_id),
    )
    .expect("structural recipient mutation");
    assert!(runtime
        .validate_onchain_transaction_replacement(
            &reserved_chain,
            &accepted_records,
            &accepted,
            &recipient_mutation,
        )
        .is_err());

    let mut changed_input = valid.selected_inputs[0].clone();
    changed_input.outpoint = OutPoint::new(Txid::from_byte_array([99; 32]), 0);
    let mut input_transaction = valid.unsigned_transaction.clone();
    input_transaction.input[0].previous_output = changed_input.outpoint;
    let input_mutation = FederationTransactionProposal::new(
        &batch,
        runtime.member_id(),
        input_transaction,
        vec![changed_input],
        valid.recipient_outputs.clone(),
        valid.change_output.clone(),
        valid.fee_sat,
        Some(original_id),
    )
    .expect("structural input mutation");
    assert!(runtime
        .validate_onchain_transaction_replacement(
            &reserved_chain,
            &accepted_records,
            &accepted,
            &input_mutation,
        )
        .is_err());

    let changed_change = build_proposal(
        &runtime,
        &batch,
        vec![input],
        ProposalOptions {
            fee_sat: 1_000,
            change_script_index: 9,
            declared_change_index: 9,
            ..ProposalOptions::default()
        },
    );
    let change_mutation = FederationTransactionProposal::new(
        &batch,
        runtime.member_id(),
        changed_change.unsigned_transaction,
        changed_change.selected_inputs,
        changed_change.recipient_outputs,
        changed_change.change_output,
        changed_change.fee_sat,
        Some(original_id),
    )
    .expect("structural change mutation");
    assert!(runtime
        .validate_onchain_transaction_replacement(
            &reserved_chain,
            &accepted_records,
            &accepted,
            &change_mutation,
        )
        .is_err());
}

#[tokio::test]
async fn accepted_proposal_atomically_reserves_replays_restarts_and_gates_signing() {
    use super::super::chain_observation::{
        CHAIN_OBSERVATION_PRIMARY_NAMESPACE, CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
        CHAIN_OBSERVATION_STATE_KEY,
    };
    use super::super::transaction_state::{
        TRANSACTION_STATE_KEY, TRANSACTION_STATE_PRIMARY_NAMESPACE,
        TRANSACTION_STATE_SECONDARY_NAMESPACE,
    };

    let (runtime, _) = runtime_with_address_application();
    let (mint, localstore) = federated_test_mint(runtime.clone()).await;
    let mut chain_state = FederationChainObservationState::new();
    let input = spendable_input(&runtime, &mut chain_state, 21, 40_000);
    let (batch, records) = intents(&runtime);
    let proposal = build_proposal(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions::default(),
    );
    let proposal_id = proposal.proposal_id().expect("proposal id");
    let mut transaction_state = FederationTransactionState::new();
    for record in records.values().cloned() {
        transaction_state
            .add_pending_intent(record)
            .expect("persist pending intent");
    }
    let mut setup = localstore.begin_transaction().await.expect("begin setup");
    setup
        .kv_write(
            CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
            CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
            CHAIN_OBSERVATION_STATE_KEY,
            &chain_state.to_bytes().expect("chain state bytes"),
        )
        .await
        .expect("write chain state");
    setup
        .kv_write(
            TRANSACTION_STATE_PRIMARY_NAMESPACE,
            TRANSACTION_STATE_SECONDARY_NAMESPACE,
            TRANSACTION_STATE_KEY,
            &transaction_state
                .to_bytes()
                .expect("transaction state bytes"),
        )
        .await
        .expect("write transaction state");
    setup.commit().await.expect("commit setup state");

    let invalid = build_proposal(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions {
            declared_change_index: 9,
            ..ProposalOptions::default()
        },
    );
    let invalid_id = invalid.proposal_id().expect("invalid policy proposal id");
    let invalid_entry = accepted_entry_without_journal(
        200,
        runtime
            .transaction_proposal_operation(invalid)
            .expect("invalid policy envelope"),
    );
    persist_journal_entry(&localstore, &invalid_entry).await;
    assert!(mint
        .apply_accepted_federation_operation(&invalid_entry)
        .await
        .is_err());
    assert!(mint
        .federation_transaction_signing_authorization(invalid_id)
        .await
        .is_err());
    assert_eq!(
        mint.federation_transaction_state().await.expect("state"),
        transaction_state
    );
    assert_eq!(
        mint.federation_chain_observation_state()
            .await
            .expect("chain state"),
        chain_state
    );

    let entry = accepted_entry_without_journal(
        201,
        runtime
            .transaction_proposal_operation(proposal.clone())
            .expect("proposal envelope"),
    );
    persist_journal_entry(&localstore, &entry).await;
    assert!(mint
        .federation_transaction_signing_authorization(proposal_id)
        .await
        .is_err());
    let (first_apply, second_apply) = tokio::join!(
        mint.apply_accepted_federation_operation(&entry),
        mint.apply_accepted_federation_operation(&entry),
    );
    let first_apply = first_apply.expect("first concurrent proposal apply");
    let second_apply = second_apply.expect("second concurrent proposal apply");
    assert!(first_apply || second_apply);
    assert!(!mint
        .apply_accepted_federation_operation(&entry)
        .await
        .expect("replay proposal"));

    let accepted = mint
        .federation_transaction_signing_authorization(proposal_id)
        .await
        .expect("signing authorization after apply");
    assert_eq!(accepted.proposal, proposal);
    let reserved_chain = mint
        .federation_chain_observation_state()
        .await
        .expect("reserved chain state");
    assert_eq!(
        reserved_chain
            .utxo(input.outpoint)
            .expect("reserved UTXO")
            .state,
        FederationWalletUtxoState::Reserved
    );

    let (catch_up, catch_up_store) = federated_test_mint(runtime.clone()).await;
    let mut catch_up_setup = catch_up_store
        .begin_transaction()
        .await
        .expect("begin catch-up setup");
    catch_up_setup
        .kv_write(
            CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
            CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
            CHAIN_OBSERVATION_STATE_KEY,
            &chain_state.to_bytes().expect("catch-up chain bytes"),
        )
        .await
        .expect("write catch-up chain state");
    catch_up_setup
        .kv_write(
            TRANSACTION_STATE_PRIMARY_NAMESPACE,
            TRANSACTION_STATE_SECONDARY_NAMESPACE,
            TRANSACTION_STATE_KEY,
            &transaction_state
                .to_bytes()
                .expect("catch-up transaction bytes"),
        )
        .await
        .expect("write catch-up transaction state");
    catch_up_setup
        .commit()
        .await
        .expect("commit catch-up setup");
    persist_journal_entry(&catch_up_store, &entry).await;
    assert_eq!(
        catch_up
            .apply_federation_journal_entries(std::slice::from_ref(&entry))
            .await
            .expect("catch-up apply"),
        vec![entry.operation_id]
    );
    assert_eq!(
        catch_up
            .federation_transaction_state()
            .await
            .expect("catch-up transaction state"),
        mint.federation_transaction_state()
            .await
            .expect("source transaction state")
    );
    assert_eq!(
        catch_up
            .federation_chain_observation_state()
            .await
            .expect("catch-up chain state"),
        reserved_chain
    );

    let readiness_gated =
        restart_federated_test_mint(runtime_config(), Arc::clone(&localstore)).await;
    assert!(readiness_gated
        .federation_transaction_signing_authorization(proposal_id)
        .await
        .is_err());

    let restarted = restart_federated_test_mint(runtime, Arc::clone(&localstore)).await;
    assert_eq!(
        restarted
            .federation_transaction_signing_authorization(proposal_id)
            .await
            .expect("restart signing authorization"),
        accepted
    );

    let durable = restarted
        .federation_transaction_state()
        .await
        .expect("durable transaction state");
    let mut tampered_audit = serde_json::to_value(&durable).expect("transaction state audit JSON");
    let accepted_transactions = tampered_audit["accepted_transactions"]
        .as_object_mut()
        .expect("accepted transaction object");
    let accepted_value = accepted_transactions
        .values_mut()
        .next()
        .expect("accepted transaction value");
    let sighash_byte = accepted_value["policy_audit"]["taproot_sighashes"][0][0]
        .as_u64()
        .expect("sighash byte");
    accepted_value["policy_audit"]["taproot_sighashes"][0][0] = serde_json::json!(sighash_byte ^ 1);
    let mut audit_tamper_transaction = localstore
        .begin_transaction()
        .await
        .expect("begin audit tamper");
    audit_tamper_transaction
        .kv_write(
            TRANSACTION_STATE_PRIMARY_NAMESPACE,
            TRANSACTION_STATE_SECONDARY_NAMESPACE,
            TRANSACTION_STATE_KEY,
            &serde_json::to_vec(&tampered_audit).expect("tampered audit bytes"),
        )
        .await
        .expect("write tampered audit state");
    audit_tamper_transaction
        .commit()
        .await
        .expect("commit tampered audit state");
    assert!(restarted.federation_transaction_state().await.is_ok());
    assert!(restarted
        .federation_transaction_signing_authorization(proposal_id)
        .await
        .is_err());

    let mut restore_transaction = localstore.begin_transaction().await.expect("begin restore");
    restore_transaction
        .kv_write(
            TRANSACTION_STATE_PRIMARY_NAMESPACE,
            TRANSACTION_STATE_SECONDARY_NAMESPACE,
            TRANSACTION_STATE_KEY,
            &durable.to_bytes().expect("durable transaction bytes"),
        )
        .await
        .expect("restore transaction state");
    restore_transaction
        .commit()
        .await
        .expect("commit restored state");

    let mut tampered = serde_json::to_value(durable).expect("transaction state JSON");
    tampered["utxo_reservations"] = serde_json::json!({});
    let mut tamper_transaction = localstore.begin_transaction().await.expect("begin tamper");
    tamper_transaction
        .kv_write(
            TRANSACTION_STATE_PRIMARY_NAMESPACE,
            TRANSACTION_STATE_SECONDARY_NAMESPACE,
            TRANSACTION_STATE_KEY,
            &serde_json::to_vec(&tampered).expect("tampered bytes"),
        )
        .await
        .expect("write tampered state");
    tamper_transaction
        .commit()
        .await
        .expect("commit tampered state");
    assert!(restarted.federation_transaction_state().await.is_err());
}

#[tokio::test]
async fn accepted_replacement_uses_the_existing_operation_and_fresh_signing_path() {
    use super::super::chain_observation::{
        CHAIN_OBSERVATION_PRIMARY_NAMESPACE, CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
        CHAIN_OBSERVATION_STATE_KEY,
    };
    use super::super::transaction_state::{
        TRANSACTION_STATE_KEY, TRANSACTION_STATE_PRIMARY_NAMESPACE,
        TRANSACTION_STATE_SECONDARY_NAMESPACE,
    };

    let (runtime, _) = runtime_with_address_application();
    let (mint, localstore) = federated_test_mint(runtime.clone()).await;
    let mut chain_state = FederationChainObservationState::new();
    let input = spendable_input(&runtime, &mut chain_state, 18, 40_000);
    let (batch, records) = intents(&runtime);
    let original = build_proposal(
        &runtime,
        &batch,
        vec![input.clone()],
        ProposalOptions::default(),
    );
    let original_id = original.proposal_id().expect("original id");
    let original_audit = runtime
        .validate_onchain_transaction_proposal(&chain_state, &records, &original)
        .expect("original policy audit");
    let original_accepted = structurally_signed(original.clone(), original_audit.clone());
    let original_signed = original_accepted.signed.expect("signed original");
    let original_txid = original_signed.txid;
    let mut transaction_state = FederationTransactionState::new();
    for record in records.values().cloned() {
        transaction_state
            .add_pending_intent(record)
            .expect("persist pending intent");
    }
    transaction_state
        .accept_transaction(
            original.clone(),
            original_audit,
            FederationOperationId::from_bytes([70; 32]),
            70,
        )
        .expect("accept original");
    transaction_state
        .mark_transaction_signed(original_id, original_signed)
        .expect("persist signed original");
    transaction_state
        .authorize_transaction_broadcast(
            FederationTransactionBroadcastIntent {
                proposal_id: original_id,
                txid: original_txid,
                settlement_policy: FederationTransactionSettlementPolicy::new(1, 1)
                    .expect("settlement policy"),
            },
            FederationOperationId::from_bytes([71; 32]),
            71,
        )
        .expect("authorize original broadcast");
    chain_state
        .reserve_utxos(&[input.outpoint])
        .expect("reserve original input");

    let mut setup = localstore
        .begin_transaction()
        .await
        .expect("begin replacement setup");
    setup
        .kv_write(
            CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
            CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
            CHAIN_OBSERVATION_STATE_KEY,
            &chain_state.to_bytes().expect("chain state bytes"),
        )
        .await
        .expect("write reserved chain state");
    setup
        .kv_write(
            TRANSACTION_STATE_PRIMARY_NAMESPACE,
            TRANSACTION_STATE_SECONDARY_NAMESPACE,
            TRANSACTION_STATE_KEY,
            &transaction_state
                .to_bytes()
                .expect("transaction state bytes"),
        )
        .await
        .expect("write original transaction state");
    setup.commit().await.expect("commit replacement setup");

    let replacement = replacement_proposal(&runtime, &batch, &original, 500);
    let replacement_id = replacement.proposal_id().expect("replacement id");
    let entry = accepted_entry_without_journal(
        72,
        runtime
            .transaction_proposal_operation(replacement.clone())
            .expect("replacement operation"),
    );
    persist_journal_entry(&localstore, &entry).await;
    assert!(mint
        .apply_accepted_federation_operation(&entry)
        .await
        .expect("apply replacement"));

    let durable = mint
        .federation_transaction_state()
        .await
        .expect("durable replacement state");
    assert_eq!(durable.lineage_head(batch.batch_id), Some(replacement_id));
    assert_eq!(
        durable
            .accepted_transaction(original_id)
            .expect("replaced original")
            .state,
        FederationAcceptedTransactionState::Replaced { by: replacement_id }
    );
    assert_eq!(
        durable
            .accepted_transaction(replacement_id)
            .expect("accepted replacement")
            .state,
        FederationAcceptedTransactionState::Accepted
    );
    assert_eq!(durable.reservation(input.outpoint), Some(original_id));
    assert!(mint
        .federation_transaction_signing_authorization(original_id)
        .await
        .is_err());
    let signing = mint
        .federation_transaction_signing_authorization(replacement_id)
        .await
        .expect("replacement signing authorization");
    assert_eq!(signing.proposal, replacement);
    assert_ne!(signing.policy_audit.proposal_id, original_id);
    assert_ne!(signing.policy_audit.txid, original_txid);
}

#[test]
fn aleph_bft_journal_rejects_same_batch_and_shared_utxo_competitors() {
    let (runtime, _) = runtime_with_address_application();
    let mut chain_state = FederationChainObservationState::new();
    let input = spendable_input(&runtime, &mut chain_state, 22, 40_000);
    let alternate_input = spendable_input(&runtime, &mut chain_state, 23, 40_000);
    let (first_batch, _) = intents(&runtime);
    let first = build_proposal(
        &runtime,
        &first_batch,
        vec![input.clone()],
        ProposalOptions::default(),
    );
    let second_intents = first_batch
        .intents
        .iter()
        .enumerate()
        .map(|(index, original)| {
            FederationOnchainPaymentIntent::new(
                format!("competing-{}", original.quote),
                FederationOperationId::from_bytes(
                    [u8::try_from(index + 24).expect("operation marker"); 32],
                ),
                original.destination_script.clone(),
                original.amount_sat,
                original.maximum_fee_sat,
                original.fee_tier,
                original.fee_target_blocks,
                original.creation_time + index as u64,
                original.scope,
            )
            .expect("competing intent")
        })
        .collect::<Vec<_>>();
    let second_batch =
        FederationOnchainPaymentBatch::new(second_intents, 21).expect("competing payment batch");
    let second = build_proposal(
        &runtime,
        &second_batch,
        vec![input],
        ProposalOptions::default(),
    );
    let mut journal = InMemoryFederationJournal::new();
    let first_envelope = runtime
        .transaction_proposal_operation(first)
        .expect("first envelope");
    runtime
        .validate_federated_operation_submission(&first_envelope.operation)
        .expect("proposal admission");
    first_envelope
        .verify_authorization(runtime.public_config())
        .expect("proposal authorization");
    journal
        .append(first_envelope)
        .expect("accept first proposal");
    let same_batch = build_proposal(
        &runtime,
        &first_batch,
        vec![alternate_input],
        ProposalOptions::default(),
    );
    assert!(matches!(
        journal.append(
            runtime
                .transaction_proposal_operation(same_batch)
                .expect("same-batch envelope")
        ),
        Err(FederationError::ConflictingOperation { .. })
    ));
    assert!(matches!(
        journal.append(
            runtime
                .transaction_proposal_operation(second)
                .expect("second envelope")
        ),
        Err(FederationError::ConflictingOperation { .. })
    ));
}
