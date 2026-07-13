use bitcoin::hashes::Hash;
use bitcoin::{Amount as BitcoinAmount, BlockHash, OutPoint, TxOut, Txid};
use cdk_common::federation::{
    FederationAllocatedAddressRef, FederationChainObservation, FederationChainSourceDescriptor,
    FederationChainSourceScope, FederationDepositConfirmation, FederationDepositObservation,
    FederationDepositReorgObservation, FederationWalletUtxoState,
};

use super::address_allocation::{
    runtime_with_address_application, second_member_runtime_with_address_application,
};
use super::*;

fn confirmed_observation(
    runtime: &FederationRuntimeConfig,
    application_id: cdk_common::federation::FederationFrostApplicationId,
    allocation: &cdk_common::federation::FederationAddressAllocation,
    observer: MemberId,
    depth: u32,
) -> FederationChainObservation {
    let wallet = runtime
        .public_config()
        .frost_wallet
        .as_ref()
        .expect("wallet config");
    let script = wallet
        .derive_cdk_onchain_script(
            runtime.public_config(),
            application_id,
            3,
            allocation.keychain,
            allocation.derivation_index,
        )
        .expect("allocated script");
    FederationChainObservation::Deposit(FederationDepositObservation {
        allocation: FederationAllocatedAddressRef {
            application_id,
            application_key_epoch: 3,
            quote: allocation.quote.clone(),
            keychain: allocation.keychain,
            derivation_index: allocation.derivation_index,
        },
        outpoint: OutPoint::new(Txid::from_byte_array([41; 32]), 1),
        txout: TxOut {
            value: BitcoinAmount::from_sat(25_000),
            script_pubkey: script,
        },
        confirmation: Some(FederationDepositConfirmation {
            block_hash: BlockHash::from_byte_array([42; 32]),
            block_height: 200,
            confirmation_depth: depth,
        }),
        observer,
        source: FederationChainSourceDescriptor::new(
            FederationChainSourceScope::Shared,
            "shared-regtest-source",
        )
        .expect("source"),
    })
}

#[tokio::test]
async fn accepted_member_quorum_materializes_and_catches_up_identically() {
    let (mut runtime, application_id) = runtime_with_address_application();
    let mut second_runtime = second_member_runtime_with_address_application();
    runtime.public_config.payment_observation_policy =
        FederationPaymentObservationPolicy::member_quorum(2);
    second_runtime.public_config.payment_observation_policy =
        FederationPaymentObservationPolicy::member_quorum(2);

    let mut journal = InMemoryFederationJournal::new();
    let allocation_entry = appended_entry(
        &mut journal,
        runtime
            .address_allocation_operation(
                "quote-chain-1",
                application_id,
                3,
                cdk_common::federation::FederationAddressKeychain::External,
            )
            .expect("allocation operation"),
    );
    let allocation = cdk_common::federation::FederationAddressAllocation {
        quote: "quote-chain-1".to_string(),
        keychain: cdk_common::federation::FederationAddressKeychain::External,
        derivation_index: 0,
        operation_id: allocation_entry.operation_id,
        operation_index: allocation_entry.index,
    };
    let first_observation = confirmed_observation(
        &runtime,
        application_id,
        &allocation,
        runtime.member_id(),
        2,
    );
    let second_observation = confirmed_observation(
        &second_runtime,
        application_id,
        &allocation,
        second_runtime.member_id(),
        3,
    );
    let first_entry = appended_entry(
        &mut journal,
        runtime
            .chain_observation_operation(first_observation)
            .expect("first observation"),
    );
    let second_entry = appended_entry(
        &mut journal,
        second_runtime
            .chain_observation_operation(second_observation)
            .expect("second observation"),
    );

    let mut members = Vec::new();
    for _ in 0..5 {
        members.push(federated_test_mint(runtime.clone()).await.0);
    }
    for member in &members[..4] {
        persist_and_apply_entry(member, &allocation_entry).await;
        persist_and_apply_entry(member, &first_entry).await;
        assert_eq!(
            member
                .federation_chain_observation_state()
                .await
                .expect("state")
                .utxo(OutPoint::new(Txid::from_byte_array([41; 32]), 1))
                .expect("utxo")
                .state,
            FederationWalletUtxoState::Confirmed
        );
        persist_and_apply_entry(member, &second_entry).await;
    }
    for entry in [&allocation_entry, &first_entry, &second_entry] {
        persist_and_apply_entry(&members[4], entry).await;
    }

    let outpoint = OutPoint::new(Txid::from_byte_array([41; 32]), 1);
    let expected = members[0]
        .federation_chain_observation_state()
        .await
        .expect("expected state");
    let utxo = expected.utxo(outpoint).expect("utxo");
    assert_eq!(utxo.state, FederationWalletUtxoState::Spendable);
    assert_eq!(utxo.matching_confirmed_observers(), 2);
    assert_eq!(utxo.source_summary().shared_sources, 1);
    for member in &members[1..] {
        assert_eq!(
            member
                .federation_chain_observation_state()
                .await
                .expect("member state"),
            expected
        );
    }
    assert!(!members[0]
        .apply_accepted_federation_operation(&second_entry)
        .await
        .expect("applied replay"));

    let reorg = FederationChainObservation::Reorg(FederationDepositReorgObservation {
        outpoint,
        disconnected_block_hash: BlockHash::from_byte_array([42; 32]),
        disconnected_block_height: 200,
        observer: runtime.member_id(),
        source: FederationChainSourceDescriptor::new(
            FederationChainSourceScope::Shared,
            "shared-regtest-source",
        )
        .expect("source"),
    });
    let reorg_entry = appended_entry(
        &mut journal,
        runtime
            .chain_observation_operation(reorg)
            .expect("reorg operation"),
    );
    for member in &members {
        persist_and_apply_entry(member, &reorg_entry).await;
        assert_eq!(
            member
                .federation_chain_observation_state()
                .await
                .expect("reorg state")
                .utxo(outpoint)
                .expect("utxo")
                .state,
            FederationWalletUtxoState::Reorged
        );
    }

    let (restart_source, restart_store) = federated_test_mint(runtime.clone()).await;
    for entry in [&allocation_entry, &first_entry, &second_entry, &reorg_entry] {
        persist_and_apply_entry(&restart_source, entry).await;
    }
    let before_restart = restart_source
        .federation_chain_observation_state()
        .await
        .expect("pre-restart state");
    drop(restart_source);
    let restarted = restart_federated_test_mint(runtime, restart_store).await;
    assert_eq!(
        restarted
            .federation_chain_observation_state()
            .await
            .expect("restart state"),
        before_restart
    );

    let entries = members[0]
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("entries");
    let digest = members[0]
        .federation_materialized_state_snapshot(&entries)
        .await
        .expect("snapshot")
        .digest()
        .expect("digest");
    for member in &members[1..] {
        let member_entries = member
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("entries");
        assert_eq!(
            member
                .federation_materialized_state_snapshot(&member_entries)
                .await
                .expect("snapshot")
                .digest()
                .expect("digest"),
            digest
        );
    }
}
