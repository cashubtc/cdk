use cdk_common::federation::{
    FederationAddressAllocationScope, FederationAddressKeychain, FederationDkgCeremonyId,
    FederationFrostApplicationConfig, FederationFrostApplicationId,
    FederationFrostRootPublicConfig, FederationFrostWalletConfig, FederationJournalAppendOutcome,
    FederationOnchainConfirmationPolicy,
};

use super::*;

pub(super) fn attach_address_application(
    runtime: &mut FederationRuntimeConfig,
) -> FederationFrostApplicationId {
    let verification_share = [
        2, 121, 190, 102, 126, 249, 220, 187, 172, 85, 160, 98, 149, 206, 135, 11, 7, 2, 155,
        252, 219, 45, 206, 40, 217, 89, 242, 129, 91, 22, 248, 23, 152,
    ];
    let participant_mapping = runtime
        .public_config
        .members
        .keys()
        .map(|member| (*member, member.get()))
        .collect::<BTreeMap<_, _>>();
    let verification_shares = runtime
        .public_config
        .members
        .keys()
        .map(|member| (*member, verification_share))
        .collect::<BTreeMap<_, _>>();
    let mut canonical_public_package = b"CDKFRT01".to_vec();
    canonical_public_package.extend_from_slice(&1u64.to_be_bytes());
    canonical_public_package.extend_from_slice(&1u16.to_be_bytes());
    canonical_public_package.extend_from_slice(&1u16.to_be_bytes());
    canonical_public_package.extend_from_slice(
        &runtime
            .public_config
            .threshold_params
            .signature_threshold
            .to_be_bytes(),
    );
    canonical_public_package.extend_from_slice(
        &u16::try_from(verification_shares.len())
            .expect("test FROST roster fits u16")
            .to_be_bytes(),
    );
    for (member, share) in &verification_shares {
        canonical_public_package.extend_from_slice(&member.get().to_be_bytes());
        canonical_public_package.extend_from_slice(share);
    }
    canonical_public_package.extend_from_slice(&verification_share);
    canonical_public_package.extend_from_slice(&[5; 32]);
    let root = FederationFrostRootPublicConfig {
        version: 1,
        federation_id: runtime.public_config.federation_id,
        ceremony_id: FederationDkgCeremonyId::from_bytes([6; 32]),
        root_key_epoch: 1,
        dkg_protocol_version: 1,
        ciphersuite_version: 1,
        threshold: runtime
            .public_config
            .threshold_params
            .signature_threshold,
        participant_mapping,
        verification_shares,
        untweaked_group_key: verification_share,
        transcript_binding: [5; 32],
        canonical_public_package,
    };
    let application = FederationFrostApplicationConfig::new_cdk_onchain(
        &runtime.public_config,
        &root,
        3,
        cdk_common::bitcoin::Network::Regtest,
        FederationOnchainConfirmationPolicy::new(2).expect("confirmation policy"),
        1,
    )
    .expect("valid test CDK on-chain application");
    let application_id = application.application_id;
    runtime.public_config.frost_wallet = Some(FederationFrostWalletConfig {
        version: 1,
        root,
        cdk_onchain_application: application,
        bark_application: None,
    });
    application_id
}

pub(super) fn runtime_with_address_application(
) -> (FederationRuntimeConfig, FederationFrostApplicationId) {
    let mut runtime = runtime_configs_with_signature_threshold(2).remove(0);
    let application_id = attach_address_application(&mut runtime);
    (runtime, application_id)
}

pub(super) fn second_member_runtime_with_address_application() -> FederationRuntimeConfig {
    let mut runtime = runtime_configs_with_signature_threshold(2).remove(1);
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
        .federation_materialized_state_snapshot()
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
                .federation_materialized_state_snapshot()
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
        .federation_materialized_state_snapshot()
        .await
        .expect("first snapshot")
        .digest()
        .expect("first digest");
    let altered_mapping_digest = second_mint
        .federation_materialized_state_snapshot()
        .await
        .expect("second snapshot")
        .digest()
        .expect("second digest");
    assert_ne!(first_digest, altered_mapping_digest);

    persist_and_apply_entry(&counter_mint, &first_entries[0]).await;
    let counter_digest = counter_mint
        .federation_materialized_state_snapshot()
        .await
        .expect("counter snapshot")
        .digest()
        .expect("counter digest");
    assert_ne!(first_digest, counter_digest);
}
use std::collections::BTreeSet;
