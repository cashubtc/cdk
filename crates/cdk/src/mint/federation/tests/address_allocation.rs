use cdk_common::federation::{
    FederationAddressAllocationScope, FederationAddressKeychain, FederationFrostActiveEpochs,
    FederationFrostApplicationConfig, FederationFrostApplicationId, FederationFrostEpochState,
    FederationFrostWalletConfig, FederationJournalAppendOutcome,
    FederationOnchainConfirmationPolicy,
};

use super::*;

pub(super) fn attach_address_application(
    runtime: &mut FederationRuntimeConfig,
) -> FederationFrostApplicationId {
    let application_id = FederationFrostApplicationId::from_bytes([7; 32]).expect("application");
    runtime.public_config.frost_wallet = Some(FederationFrostWalletConfig {
        version: 1,
        roots: Vec::new(),
        cdk_onchain_applications: vec![FederationFrostApplicationConfig {
            version: 1,
            application_id,
            application_key_epoch: 3,
            root_key_epoch: 1,
            state: FederationFrostEpochState::Active,
            derivation_version: 1,
            authorization_policy: [8; 32],
            threshold: 1,
            verification_shares: BTreeMap::from([
                (
                    MemberId::new(1).expect("member"),
                    [
                        2, 121, 190, 102, 126, 249, 220, 187, 172, 85, 160, 98, 149, 206, 135, 11,
                        7, 2, 155, 252, 219, 45, 206, 40, 217, 89, 242, 129, 91, 22, 248, 23, 152,
                    ],
                ),
                (
                    MemberId::new(2).expect("member"),
                    [
                        2, 121, 190, 102, 126, 249, 220, 187, 172, 85, 160, 98, 149, 206, 135, 11,
                        7, 2, 155, 252, 219, 45, 206, 40, 217, 89, 242, 129, 91, 22, 248, 23, 152,
                    ],
                ),
            ]),
            derived_group_key: [
                2, 121, 190, 102, 126, 249, 220, 187, 172, 85, 160, 98, 149, 206, 135, 11, 7, 2,
                155, 252, 219, 45, 206, 40, 217, 89, 242, 129, 91, 22, 248, 23, 152,
            ],
            chain_code: [9; 32],
            external_descriptor: "test-external".to_string(),
            internal_descriptor: "test-internal".to_string(),
            descriptor_commitment: [10; 32],
            network: cdk_common::bitcoin::Network::Regtest,
            confirmation_policy: FederationOnchainConfirmationPolicy::new(2)
                .expect("confirmation policy"),
            fee_policy_version: 1,
        }],
        registered_consumers: Vec::new(),
        active_epochs: FederationFrostActiveEpochs {
            root_key_epoch: 1,
            application_key_epochs: BTreeMap::from([(application_id, 3)]),
        },
    });
    application_id
}

pub(super) fn runtime_with_address_application(
) -> (FederationRuntimeConfig, FederationFrostApplicationId) {
    let mut runtime = runtime_config();
    let application_id = attach_address_application(&mut runtime);
    (runtime, application_id)
}

pub(super) fn second_member_runtime_with_address_application() -> FederationRuntimeConfig {
    let members = (1..=2)
        .map(|id| {
            TrustedDealerMember::new(
                MemberId::new(id).expect("member id"),
                MintUrl::from_str(&format!("https://mint{id}.example.com"))
                    .expect("public mint url"),
                MintUrl::from_str(&format!("https://fed{id}.example.com"))
                    .expect("federation api url"),
                SecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
            )
        })
        .collect();
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![BlsSecretKey::from_reduced_bytes(&[2; 32])],
        )]),
    );
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(2, 1).expect("params"),
        members,
        vec![keyset],
    )
    .expect("trusted dealer bundle");
    let private = bundle
        .private_configs
        .get(&MemberId::new(2).expect("member id"))
        .expect("member two private config")
        .clone();
    let mut runtime =
        FederationRuntimeConfig::new(bundle.public_config, private).expect("runtime config");
    attach_address_application(&mut runtime);
    runtime
}

fn allocation_entry(
    journal: &mut InMemoryFederationJournal,
    runtime: &FederationRuntimeConfig,
    application_id: FederationFrostApplicationId,
    quote: &str,
    keychain: FederationAddressKeychain,
) -> FederationJournalEntry {
    appended_entry(
        journal,
        runtime
            .address_allocation_operation(quote, application_id, 3, keychain)
            .expect("allocation envelope"),
    )
}

fn scope(application_id: FederationFrostApplicationId) -> FederationAddressAllocationScope {
    FederationAddressAllocationScope {
        application_id,
        application_key_epoch: 3,
    }
}

#[tokio::test]
async fn sequential_duplicate_conflicting_and_keychain_allocations_are_deterministic() {
    let (runtime, application_id) = runtime_with_address_application();
    let second_runtime = second_member_runtime_with_address_application();
    assert!(runtime
        .address_allocation_operation(
            "wrong-epoch",
            application_id,
            2,
            FederationAddressKeychain::External,
        )
        .is_err());
    let (mint, localstore) = federated_test_mint(runtime.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let first = allocation_entry(
        &mut journal,
        &runtime,
        application_id,
        "quote-a",
        FederationAddressKeychain::External,
    );
    let second = allocation_entry(
        &mut journal,
        &runtime,
        application_id,
        "quote-b",
        FederationAddressKeychain::External,
    );
    let internal = allocation_entry(
        &mut journal,
        &runtime,
        application_id,
        "change-a",
        FederationAddressKeychain::Internal,
    );
    for entry in [&first, &second, &internal] {
        persist_and_apply_entry(&mint, entry).await;
    }

    let state = mint
        .federation_address_allocation_state()
        .await
        .expect("allocation state");
    let wallet = state.wallet(scope(application_id)).expect("wallet state");
    assert_eq!(wallet.external_next_index, 2);
    assert_eq!(wallet.internal_next_index, 1);
    assert_eq!(wallet.allocations["quote-a"].derivation_index, 0);
    assert_eq!(wallet.allocations["quote-b"].derivation_index, 1);
    assert_eq!(wallet.allocations["change-a"].derivation_index, 0);

    assert!(!mint
        .apply_accepted_federation_operation(&first)
        .await
        .expect("replayed applied entry"));
    let state_after_replay = mint
        .federation_address_allocation_state()
        .await
        .expect("state after replay");
    assert_eq!(state_after_replay, state);

    let other_member_retry = second_runtime
        .address_allocation_operation(
            "quote-a",
            application_id,
            3,
            FederationAddressKeychain::External,
        )
        .expect("other member retry");
    assert_eq!(
        other_member_retry.operation_id().expect("retry id"),
        first.operation_id
    );
    assert!(matches!(
        journal
            .append(other_member_retry)
            .expect("semantic duplicate"),
        FederationJournalAppendOutcome::Duplicate(_)
    ));

    let conflicting = runtime
        .address_allocation_operation(
            "quote-a",
            application_id,
            3,
            FederationAddressKeychain::Internal,
        )
        .expect("conflicting envelope");
    assert!(journal.append(conflicting).is_err());
    assert_eq!(
        localstore
            .get_federation_journal_entries()
            .await
            .expect("journal")
            .len(),
        3
    );
}

#[tokio::test]
async fn concurrent_member_requests_and_offline_catch_up_converge_without_gaps() {
    let (runtime, application_id) = runtime_with_address_application();
    let second_runtime = second_member_runtime_with_address_application();
    let mut journal = InMemoryFederationJournal::new();
    let entries = (0..24)
        .map(|index| {
            let author = if index % 2 == 0 {
                &runtime
            } else {
                &second_runtime
            };
            allocation_entry(
                &mut journal,
                author,
                application_id,
                &format!("concurrent-quote-{index}"),
                FederationAddressKeychain::External,
            )
        })
        .collect::<Vec<_>>();

    let mut members = Vec::new();
    for _ in 0..5 {
        members.push(federated_test_mint(runtime.clone()).await.0);
    }
    for member in &members[..4] {
        for entry in &entries {
            persist_and_apply_entry(member, entry).await;
        }
    }
    for entry in &entries {
        persist_and_apply_entry(&members[4], entry).await;
    }

    let expected = members[0]
        .federation_address_allocation_state()
        .await
        .expect("expected state");
    let expected_wallet = expected.wallet(scope(application_id)).expect("wallet");
    assert_eq!(expected_wallet.external_next_index, 24);
    assert_eq!(
        expected_wallet
            .allocations
            .values()
            .map(|allocation| allocation.derivation_index)
            .collect::<BTreeSet<_>>(),
        (0..24).collect()
    );
    let expected_digest = members[0]
        .federation_materialized_state_snapshot(&entries)
        .await
        .expect("snapshot")
        .digest()
        .expect("digest");
    for member in &members[1..] {
        assert_eq!(
            member
                .federation_address_allocation_state()
                .await
                .expect("member state"),
            expected
        );
        assert_eq!(
            member
                .federation_materialized_state_snapshot(&entries)
                .await
                .expect("member snapshot")
                .digest()
                .expect("member digest"),
            expected_digest
        );
    }
}

#[tokio::test]
async fn crash_boundaries_recover_only_consensus_accepted_indexes() {
    let (runtime, application_id) = runtime_with_address_application();
    let (mint, localstore) = federated_test_mint(runtime.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let unaccepted = runtime
        .address_allocation_operation(
            "before-consensus",
            application_id,
            3,
            FederationAddressKeychain::External,
        )
        .expect("unaccepted operation");
    assert!(mint
        .federation_address_allocation_state()
        .await
        .expect("empty state")
        .allocation(scope(application_id), "before-consensus")
        .is_none());
    drop(unaccepted);

    let accepted = allocation_entry(
        &mut journal,
        &runtime,
        application_id,
        "accepted-before-apply",
        FederationAddressKeychain::External,
    );
    persist_journal_entry(&localstore, &accepted).await;
    assert!(mint
        .federation_address_allocation_state()
        .await
        .expect("pre-apply state")
        .allocation(scope(application_id), "accepted-before-apply")
        .is_none());

    assert!(mint
        .apply_accepted_federation_operation(&accepted)
        .await
        .expect("startup replay"));
    let allocation = mint
        .federation_address_allocation(scope(application_id), "accepted-before-apply")
        .await
        .expect("lookup")
        .expect("accepted mapping");
    assert_eq!(allocation.derivation_index, 0);
    drop(mint);

    let fake_wallet = cdk_fake_wallet::FakeWallet::new(
        FeeReserve {
            min_fee_reserve: Amount::from(1),
            percent_fee_reserve: 1.0,
        },
        HashMap::new(),
        HashSet::new(),
        0,
        CurrencyUnit::Sat,
    );
    let mut builder = MintBuilder::new(localstore.clone()).with_federation_config(runtime);
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            Arc::new(fake_wallet),
        )
        .await
        .expect("payment processor");
    let restarted = builder
        .build_with_seed(localstore, &[9; 64])
        .await
        .expect("restarted mint");
    assert_eq!(
        restarted
            .federation_address_allocation(scope(application_id), "accepted-before-apply")
            .await
            .expect("restart lookup")
            .expect("restart mapping")
            .derivation_index,
        0
    );
}

#[tokio::test]
async fn materialized_digest_changes_with_mapping_and_counter_state() {
    let (runtime, application_id) = runtime_with_address_application();
    let (first_mint, _) = federated_test_mint(runtime.clone()).await;
    let (second_mint, _) = federated_test_mint(runtime.clone()).await;
    let (counter_mint, _) = federated_test_mint(runtime.clone()).await;
    let mut first_journal = InMemoryFederationJournal::new();
    let mut second_journal = InMemoryFederationJournal::new();
    let first_entries = [
        allocation_entry(
            &mut first_journal,
            &runtime,
            application_id,
            "quote-a",
            FederationAddressKeychain::External,
        ),
        allocation_entry(
            &mut first_journal,
            &runtime,
            application_id,
            "quote-b",
            FederationAddressKeychain::External,
        ),
    ];
    let second_entries = [
        allocation_entry(
            &mut second_journal,
            &runtime,
            application_id,
            "quote-b",
            FederationAddressKeychain::External,
        ),
        allocation_entry(
            &mut second_journal,
            &runtime,
            application_id,
            "quote-a",
            FederationAddressKeychain::External,
        ),
    ];
    for entry in &first_entries {
        persist_and_apply_entry(&first_mint, entry).await;
    }
    for entry in &second_entries {
        persist_and_apply_entry(&second_mint, entry).await;
    }
    let first_digest = first_mint
        .federation_materialized_state_snapshot(&first_entries)
        .await
        .expect("first snapshot")
        .digest()
        .expect("first digest");
    let altered_mapping_digest = second_mint
        .federation_materialized_state_snapshot(&second_entries)
        .await
        .expect("second snapshot")
        .digest()
        .expect("second digest");
    assert_ne!(first_digest, altered_mapping_digest);

    persist_and_apply_entry(&counter_mint, &first_entries[0]).await;
    let counter_digest = counter_mint
        .federation_materialized_state_snapshot(&first_entries[..1])
        .await
        .expect("counter snapshot")
        .digest()
        .expect("counter digest");
    assert_ne!(first_digest, counter_digest);
}
use std::collections::BTreeSet;
