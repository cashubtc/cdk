use std::collections::BTreeMap;

use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::transaction::Version;
use bitcoin::{
    Amount as BitcoinAmount, BlockHash, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn,
    TxOut, Txid, Witness,
};
use cdk_common::federation::{
    FederationAddressAllocation, FederationAddressKeychain, FederationAllocatedAddressRef,
    FederationChainObservationState, FederationChainSourceDescriptor, FederationChainSourceScope,
    FederationDepositConfirmation, FederationDepositObservation, FederationOnchainPaymentBatch,
    FederationOnchainPaymentIntent, FederationOnchainPaymentIntentId,
    FederationOnchainPaymentIntentRecord, FederationOnchainPaymentIntentState,
    FederationOperationId, FederationTaprootSighashType, FederationTransactionChangeOutput,
    FederationTransactionInput, FederationTransactionProposal,
    FederationTransactionRecipientOutput, FederationTransactionScope, FederationWalletUtxoState,
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
                wallet_script(runtime, FederationAddressKeychain::External, 50),
                15_000,
                1_000,
                1,
                20,
                scope,
            )
            .expect("intent b"),
            FederationOnchainPaymentIntent::new(
                "melt-quote-a".to_string(),
                wallet_script(runtime, FederationAddressKeychain::External, 51),
                10_000,
                1_000,
                0,
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
            wallet_script(&runtime, FederationAddressKeychain::External, 100),
            1,
            1_000,
            0,
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
