use bitcoin::hashes::Hash;
use bitcoin::{Amount as BitcoinAmount, BlockHash, OutPoint, TxOut, Txid};
use cdk_common::federation::{
    FederationAcceptedTransactionState, FederationAddressAllocationOperation,
    FederationAddressAllocationScope, FederationAddressAllocationState, FederationAddressKeychain,
    FederationAllocatedAddressRef, FederationChainObservation, FederationChainObservationState,
    FederationChainSourceDescriptor, FederationChainSourceScope, FederationDepositConfirmation,
    FederationDepositObservation, FederationOnchainPaymentIntent,
    FederationOnchainPaymentIntentRecord, FederationOperationId, FederationStateTable,
    FederationTransactionBroadcastIntent, FederationTransactionBroadcastObservation,
    FederationTransactionBroadcastResult, FederationTransactionScope, FederationTransactionState,
};

use super::address_allocation::runtime_with_address_application;
use super::transaction_signing::{
    persist_reserved_inputs, persist_transaction_state, signed_fixture_for_intent,
};
use super::*;

#[tokio::test]
async fn mint_incremental_onchain_apply_matches_full_history_with_many_rows() {
    let (mut runtime, application_id) = runtime_with_address_application();
    runtime.public_config_mut().payment_observation_policy =
        FederationPaymentObservationPolicy::member_quorum(1);
    let (mint, localstore) = federated_test_mint(runtime.clone()).await;
    let mut journal = InMemoryFederationJournal::new();

    let scope = FederationAddressAllocationScope {
        application_id,
        application_key_epoch: 3,
    };
    let wallet = runtime
        .public_config()
        .frost_wallet
        .as_ref()
        .expect("wallet config")
        .clone();

    let mut ref_allocs = FederationAddressAllocationState::new();
    let mut ref_chain = FederationChainObservationState::new();

    // Apply 80 AddressAllocation + 80 ChainObservation (Deposit) operations through Mint
    for idx in 0..80_u32 {
        let quote = format!("perf-quote-{idx}");
        let keychain = if idx % 4 == 0 {
            FederationAddressKeychain::Internal
        } else {
            FederationAddressKeychain::External
        };
        let alloc_op =
            FederationAddressAllocationOperation::new(quote.clone(), application_id, 3, keychain)
                .expect("allocation op");
        let alloc_entry = appended_entry(
            &mut journal,
            runtime
                .address_allocation_operation(&quote, application_id, 3, keychain)
                .expect("allocation envelope"),
        );
        persist_journal_entry(&localstore, &alloc_entry).await;
        mint.apply_accepted_federation_operation(&alloc_entry)
            .await
            .expect("apply allocation");
        let expected_alloc = ref_allocs
            .apply(&alloc_op, alloc_entry.operation_id, alloc_entry.index)
            .expect("ref apply allocation");

        let looked_up = mint
            .federation_address_allocation(scope, &quote)
            .await
            .expect("lookup allocation")
            .expect("allocation exists");
        assert_eq!(&looked_up, expected_alloc.allocation());

        let expected_script = wallet
            .derive_cdk_onchain_script(
                runtime.public_config(),
                application_id,
                3,
                looked_up.keychain,
                looked_up.derivation_index,
            )
            .expect("derive script");
        let mut txid_bytes = [0u8; 32];
        txid_bytes[..4].copy_from_slice(&idx.to_be_bytes());
        txid_bytes[31] = 0xAB;
        let deposit = FederationDepositObservation {
            allocation: FederationAllocatedAddressRef {
                application_id,
                application_key_epoch: 3,
                quote: quote.clone(),
                keychain: looked_up.keychain,
                derivation_index: looked_up.derivation_index,
            },
            outpoint: OutPoint::new(Txid::from_byte_array(txid_bytes), 0),
            txout: TxOut {
                value: BitcoinAmount::from_sat(25_000 + u64::from(idx)),
                script_pubkey: expected_script.clone(),
            },
            confirmation: Some(FederationDepositConfirmation {
                block_hash: BlockHash::from_byte_array([71; 32]),
                block_height: 200,
                confirmation_depth: 2,
            }),
            observation_epoch: 0,
            observer: runtime.member_id(),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Independent,
                "perf-source",
            )
            .expect("source"),
        };
        let chain_entry = appended_entry(
            &mut journal,
            runtime
                .chain_observation_operation(FederationChainObservation::Deposit(deposit.clone()))
                .expect("chain envelope"),
        );
        persist_journal_entry(&localstore, &chain_entry).await;
        mint.apply_accepted_federation_operation(&chain_entry)
            .await
            .expect("apply chain observation");
        ref_chain
            .apply_deposit(
                &deposit,
                &looked_up,
                expected_script.as_script(),
                1,
                2,
                chain_entry.operation_id,
                chain_entry.index,
            )
            .expect("ref apply deposit");
    }

    let durable_allocs = mint
        .federation_address_allocation_state()
        .await
        .expect("durable alloc state");
    assert_eq!(durable_allocs, ref_allocs);
    assert_eq!(
        durable_allocs.materialized_rows().expect("durable rows"),
        ref_allocs.materialized_rows().expect("ref rows")
    );

    let durable_chain = mint
        .federation_chain_observation_state()
        .await
        .expect("durable chain state");
    assert_eq!(durable_chain, ref_chain);
    assert_eq!(
        durable_chain.materialized_rows().expect("durable rows"),
        ref_chain.materialized_rows().expect("ref rows")
    );

    // Exercise TransactionSigned -> BroadcastIntent -> BroadcastObservation with many historical intents
    let tx_scope = FederationTransactionScope {
        root_key_epoch: 1,
        application_id,
        application_key_epoch: 3,
        network: bitcoin::Network::Regtest,
        policy_version: 1,
    };
    let mut tx_state = FederationTransactionState::new();
    for idx in 1..=60_u8 {
        let extra_intent = FederationOnchainPaymentIntent::new(
            format!("extra-melt-{idx}"),
            FederationOperationId::from_bytes([idx; 32]),
            bitcoin::ScriptBuf::from_bytes(vec![0x51]),
            10_000,
            1_000,
            0,
            1,
            u64::from(idx),
            tx_scope,
        )
        .expect("extra intent");
        tx_state
            .add_pending_intent(
                FederationOnchainPaymentIntentRecord::pending(extra_intent).expect("record"),
            )
            .expect("add pending");
    }

    let target_intent = FederationOnchainPaymentIntent::new(
        "target-signed-quote".to_string(),
        FederationOperationId::from_bytes([200; 32]),
        bitcoin::ScriptBuf::from_bytes(vec![0x51]),
        10_000,
        1_000,
        0,
        1,
        100,
        tx_scope,
    )
    .expect("target intent");
    let (fixture_state, signed) = signed_fixture_for_intent(target_intent, 500);
    let proposal_id = signed.proposal_id;
    let accepted = fixture_state
        .accepted_transaction(proposal_id)
        .expect("accepted")
        .clone();
    tx_state
        .add_pending_intent(
            fixture_state
                .intents()
                .values()
                .next()
                .expect("intent")
                .clone()
                .into_pending_for_test(),
        )
        .expect("add target intent");
    tx_state
        .accept_transaction(
            accepted.proposal.clone(),
            accepted.policy_audit.clone(),
            accepted.accepted_operation_id,
            accepted.accepted_operation_index,
        )
        .expect("accept target");
    persist_transaction_state(&localstore, &tx_state).await;
    persist_reserved_inputs(&localstore, &tx_state).await;

    let signed_entry = appended_entry(
        &mut journal,
        runtime.transaction_signed_operation(signed.clone()),
    );
    persist_journal_entry(&localstore, &signed_entry).await;
    mint.apply_accepted_federation_operation(&signed_entry)
        .await
        .expect("apply signed");
    tx_state
        .mark_transaction_signed(proposal_id, signed.clone())
        .expect("ref sign");

    let broadcast_intent = FederationTransactionBroadcastIntent {
        proposal_id,
        txid: signed.txid,
        settlement_policy: runtime
            .onchain_transaction_settlement_policy(tx_scope)
            .expect("policy"),
    };
    let intent_entry = appended_entry(
        &mut journal,
        runtime.transaction_broadcast_intent_operation(broadcast_intent),
    );
    persist_journal_entry(&localstore, &intent_entry).await;
    mint.apply_accepted_federation_operation(&intent_entry)
        .await
        .expect("apply broadcast intent");
    tx_state
        .authorize_transaction_broadcast(
            broadcast_intent,
            intent_entry.operation_id,
            intent_entry.index,
        )
        .expect("ref broadcast intent");

    let confirm_obs = FederationTransactionBroadcastObservation {
        proposal_id,
        txid: signed.txid,
        observer: runtime.member_id(),
        attempt: 1,
        result: FederationTransactionBroadcastResult::Confirmed {
            block_hash: BlockHash::from_byte_array([88; 32]),
            block_height: 300,
            confirmation_depth: 2,
        },
        detail: None,
    };
    let obs_entry = appended_entry(
        &mut journal,
        runtime
            .transaction_broadcast_observation_operation(confirm_obs.clone())
            .expect("broadcast obs envelope"),
    );
    persist_journal_entry(&localstore, &obs_entry).await;
    mint.apply_accepted_federation_operation(&obs_entry)
        .await
        .expect("apply broadcast observation");
    tx_state
        .record_transaction_broadcast_observation(
            confirm_obs,
            obs_entry.operation_id,
            obs_entry.index,
        )
        .expect("ref broadcast obs");

    let durable_tx = mint
        .federation_transaction_state()
        .await
        .expect("durable tx state");
    assert_eq!(durable_tx, tx_state);
    assert_eq!(
        durable_tx
            .accepted_transaction(proposal_id)
            .expect("confirmed tx")
            .state,
        FederationAcceptedTransactionState::Confirmed
    );
    assert_eq!(
        durable_tx.materialized_rows().expect("durable tx rows"),
        tx_state.materialized_rows().expect("ref tx rows")
    );
    assert!(durable_tx
        .completed_payment_for_quote("target-signed-quote")
        .expect("completed payment")
        .is_some());
}

trait PendingRecordExt {
    fn into_pending_for_test(self) -> FederationOnchainPaymentIntentRecord;
}

impl PendingRecordExt for FederationOnchainPaymentIntentRecord {
    fn into_pending_for_test(self) -> FederationOnchainPaymentIntentRecord {
        FederationOnchainPaymentIntentRecord::pending(self.intent).expect("pending")
    }
}
