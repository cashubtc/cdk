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
        observation_epoch: 0,
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
    runtime.public_config_mut().payment_observation_policy =
        FederationPaymentObservationPolicy::member_quorum(2);
    second_runtime
        .public_config_mut()
        .payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);

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
            .chain_observation_operation(first_observation.clone())
            .expect("first observation"),
    );
    let second_entry = appended_entry(
        &mut journal,
        second_runtime
            .chain_observation_operation(second_observation.clone())
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
        observation_epoch: 0,
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
                .expect("minority reorg state")
                .utxo(outpoint)
                .expect("utxo")
                .state,
            FederationWalletUtxoState::Spendable,
            "one observer's reorg evidence must not transition the UTXO"
        );
    }

    let second_reorg = FederationChainObservation::Reorg(FederationDepositReorgObservation {
        observer: second_runtime.member_id(),
        ..FederationDepositReorgObservation {
            outpoint,
            disconnected_block_hash: BlockHash::from_byte_array([42; 32]),
            disconnected_block_height: 200,
            observation_epoch: 0,
            observer: runtime.member_id(),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Shared,
                "shared-regtest-source",
            )
            .expect("source"),
        }
    });
    let second_reorg_entry = appended_entry(
        &mut journal,
        second_runtime
            .chain_observation_operation(second_reorg)
            .expect("second reorg operation"),
    );
    for member in &members {
        persist_and_apply_entry(member, &second_reorg_entry).await;
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

    let mut first_reconfirmation = first_observation.clone();
    let FederationChainObservation::Deposit(first_reconfirmation_deposit) =
        &mut first_reconfirmation
    else {
        panic!("confirmed deposit observation");
    };
    first_reconfirmation_deposit.observation_epoch = 1;
    let mut second_reconfirmation = second_observation.clone();
    let FederationChainObservation::Deposit(second_reconfirmation_deposit) =
        &mut second_reconfirmation
    else {
        panic!("confirmed deposit observation");
    };
    second_reconfirmation_deposit.observation_epoch = 1;
    let first_reconfirmation_entry = appended_entry(
        &mut journal,
        runtime
            .chain_observation_operation(first_reconfirmation)
            .expect("first reconfirmation"),
    );
    let second_reconfirmation_entry = appended_entry(
        &mut journal,
        second_runtime
            .chain_observation_operation(second_reconfirmation)
            .expect("second reconfirmation"),
    );
    for member in &members {
        persist_and_apply_entry(member, &first_reconfirmation_entry).await;
        assert_eq!(
            member
                .federation_chain_observation_state()
                .await
                .expect("recovering state")
                .utxo(outpoint)
                .expect("utxo")
                .state,
            FederationWalletUtxoState::Reorged
        );
        persist_and_apply_entry(member, &second_reconfirmation_entry).await;
        let recovered = member
            .federation_chain_observation_state()
            .await
            .expect("recovered state");
        let recovered_utxo = recovered.utxo(outpoint).expect("utxo");
        assert_eq!(recovered_utxo.state, FederationWalletUtxoState::Spendable);
        assert_eq!(recovered_utxo.observation_epoch, 1);
        assert_eq!(recovered_utxo.matching_confirmed_observers(), 2);
        assert!(!recovered_utxo.reorg_observations.is_empty());
    }

    let (restart_source, restart_store) = federated_test_mint(runtime.clone()).await;
    for entry in [
        &allocation_entry,
        &first_entry,
        &second_entry,
        &reorg_entry,
        &second_reorg_entry,
        &first_reconfirmation_entry,
        &second_reconfirmation_entry,
    ] {
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

    let digest = members[0]
        .federation_materialized_state_snapshot()
        .await
        .expect("snapshot")
        .digest()
        .expect("digest");
    for member in &members[1..] {
        assert_eq!(
            member
                .federation_materialized_state_snapshot()
                .await
                .expect("snapshot")
                .digest()
                .expect("digest"),
            digest
        );
    }
}

#[tokio::test]
async fn paid_observation_after_pre_liability_reorg_is_definitively_unpaid() {
    let (mut runtime, application_id) = runtime_with_address_application();
    let mut second_runtime = second_member_runtime_with_address_application();
    runtime.public_config_mut().payment_observation_policy =
        FederationPaymentObservationPolicy::member_quorum(2);
    second_runtime
        .public_config_mut()
        .payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);
    let (mint, store) = federated_test_mint(runtime.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote_id = QuoteId::new();

    let allocation_entry = appended_entry(
        &mut journal,
        runtime
            .address_allocation_operation(
                quote_id.to_string(),
                application_id,
                3,
                cdk_common::federation::FederationAddressKeychain::External,
            )
            .expect("allocation operation"),
    );
    persist_and_apply_entry(&mint, &allocation_entry).await;
    let allocation = mint
        .federation_address_allocation(
            cdk_common::federation::FederationAddressAllocationScope {
                application_id,
                application_key_epoch: 3,
            },
            &quote_id.to_string(),
        )
        .await
        .expect("allocation lookup")
        .expect("allocation");
    let application = runtime
        .cdk_onchain_application()
        .expect("active application");
    let script = runtime
        .public_config()
        .frost_wallet
        .as_ref()
        .expect("wallet config")
        .derive_cdk_onchain_script(
            runtime.public_config(),
            application_id,
            3,
            allocation.keychain,
            allocation.derivation_index,
        )
        .expect("allocated script");
    let address =
        cdk_common::bitcoin::Address::from_script(script.as_script(), application.network)
            .expect("allocated address")
            .to_string();
    let wallet_key = SecretKey::from_slice(&[24; 32]).expect("wallet key");
    let quote_entry = appended_entry(
        &mut journal,
        runtime.operation_envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: quote_id.to_string(),
                request_lookup_id: PaymentIdentifier::QuoteId(quote_id.clone()),
                request: MintQuoteRequest::Onchain(
                    cdk_common::nuts::nut30::MintQuoteOnchainRequest {
                        unit: CurrencyUnit::Sat,
                        pubkey: wallet_key.public_key(),
                    },
                ),
                response: MintQuoteResponse::Onchain(
                    cdk_common::nuts::nut30::MintQuoteOnchainResponse {
                        quote: quote_id.to_string(),
                        request: address,
                        unit: CurrencyUnit::Sat,
                        method: PaymentMethod::Known(KnownMethod::Onchain),
                        expiry: None,
                        pubkey: wallet_key.public_key(),
                        amount_paid: Amount::ZERO,
                        amount_issued: Amount::ZERO,
                        updated_at: 0,
                    },
                ),
            },
        )),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    for (member_runtime, depth) in [(&runtime, 2), (&second_runtime, 3)] {
        let entry = appended_entry(
            &mut journal,
            member_runtime
                .chain_observation_operation(confirmed_observation(
                    member_runtime,
                    application_id,
                    &allocation,
                    member_runtime.member_id(),
                    depth,
                ))
                .expect("chain observation"),
        );
        persist_and_apply_entry(&mint, &entry).await;
    }

    let first_payment = mint
        .probe_federation_mint_quote_payment(&quote_id)
        .await
        .expect("payment probe")
        .expect("spendable payment");
    let mut second_payment = first_payment.clone();
    second_payment.observer = second_runtime.member_id();
    let first_payment_entry = appended_entry(
        &mut journal,
        runtime.operation_envelope(FederationOperation::MintQuotePayment(first_payment)),
    );
    persist_and_apply_entry(&mint, &first_payment_entry).await;

    let reorg_entry = appended_entry(
        &mut journal,
        runtime
            .chain_observation_operation(FederationChainObservation::Reorg(
                FederationDepositReorgObservation {
                    outpoint: OutPoint::new(Txid::from_byte_array([41; 32]), 1),
                    disconnected_block_hash: BlockHash::from_byte_array([42; 32]),
                    disconnected_block_height: 200,
                    observation_epoch: 0,
                    observer: runtime.member_id(),
                    source: FederationChainSourceDescriptor::new(
                        FederationChainSourceScope::Shared,
                        "shared-regtest-source",
                    )
                    .expect("source"),
                },
            ))
            .expect("reorg operation"),
    );
    persist_and_apply_entry(&mint, &reorg_entry).await;

    let second_reorg_entry = appended_entry(
        &mut journal,
        second_runtime
            .chain_observation_operation(FederationChainObservation::Reorg(
                FederationDepositReorgObservation {
                    outpoint: OutPoint::new(Txid::from_byte_array([41; 32]), 1),
                    disconnected_block_hash: BlockHash::from_byte_array([42; 32]),
                    disconnected_block_height: 200,
                    observation_epoch: 0,
                    observer: second_runtime.member_id(),
                    source: FederationChainSourceDescriptor::new(
                        FederationChainSourceScope::Shared,
                        "shared-regtest-source",
                    )
                    .expect("source"),
                },
            ))
            .expect("second reorg operation"),
    );
    persist_and_apply_entry(&mint, &second_reorg_entry).await;

    let second_payment_entry = appended_entry(
        &mut journal,
        second_runtime.operation_envelope(FederationOperation::MintQuotePayment(second_payment)),
    );
    persist_journal_entry(&store, &second_payment_entry).await;
    let error = mint
        .apply_federation_journal_entries(std::slice::from_ref(&second_payment_entry))
        .await
        .expect_err("reorged output cannot support a paid observation");
    assert!(matches!(error, cdk_common::Error::UnpaidQuote));
    assert!(error.is_definitive_failure());
    assert_eq!(
        store
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote")
            .state(),
        MintQuoteState::Unpaid
    );
}

#[tokio::test]
async fn accepted_onchain_outputs_pay_one_quote_and_reuse_existing_mint_flow() {
    let (mut runtime, application_id) = runtime_with_address_application();
    let mut second_runtime = second_member_runtime_with_address_application();
    runtime.public_config_mut().payment_observation_policy =
        FederationPaymentObservationPolicy::member_quorum(2);
    second_runtime
        .public_config_mut()
        .payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);
    let (mint, store) = federated_test_mint(runtime.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote_id = QuoteId::new();

    let allocation_entry = appended_entry(
        &mut journal,
        runtime
            .address_allocation_operation(
                quote_id.to_string(),
                application_id,
                3,
                cdk_common::federation::FederationAddressKeychain::External,
            )
            .expect("allocation operation"),
    );
    persist_and_apply_entry(&mint, &allocation_entry).await;
    let allocation = mint
        .federation_address_allocation(
            cdk_common::federation::FederationAddressAllocationScope {
                application_id,
                application_key_epoch: 3,
            },
            &quote_id.to_string(),
        )
        .await
        .expect("allocation lookup")
        .expect("allocation");
    let application = runtime
        .cdk_onchain_application()
        .expect("active application");
    let script = runtime
        .public_config()
        .frost_wallet
        .as_ref()
        .expect("wallet config")
        .derive_cdk_onchain_script(
            runtime.public_config(),
            application_id,
            3,
            allocation.keychain,
            allocation.derivation_index,
        )
        .expect("allocated script");
    let address =
        cdk_common::bitcoin::Address::from_script(script.as_script(), application.network)
            .expect("allocated address")
            .to_string();
    let wallet_key = SecretKey::from_slice(&[23; 32]).expect("wallet key");
    let quote_request =
        MintQuoteRequest::Onchain(cdk_common::nuts::nut30::MintQuoteOnchainRequest {
            unit: CurrencyUnit::Sat,
            pubkey: wallet_key.public_key(),
        });
    let quote_response =
        MintQuoteResponse::Onchain(cdk_common::nuts::nut30::MintQuoteOnchainResponse {
            quote: quote_id.to_string(),
            request: address.clone(),
            unit: CurrencyUnit::Sat,
            method: PaymentMethod::Known(KnownMethod::Onchain),
            expiry: None,
            pubkey: wallet_key.public_key(),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
        });
    let quote_entry = appended_entry(
        &mut journal,
        runtime.operation_envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: quote_id.to_string(),
                request_lookup_id: PaymentIdentifier::QuoteId(quote_id.clone()),
                request: quote_request,
                response: quote_response,
            },
        )),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let stored_quote = store
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.request, address);
    assert_eq!(stored_quote.state(), MintQuoteState::Unpaid);

    let first_chain_entry = appended_entry(
        &mut journal,
        runtime
            .chain_observation_operation(confirmed_observation(
                &runtime,
                application_id,
                &allocation,
                runtime.member_id(),
                2,
            ))
            .expect("first chain observation"),
    );
    persist_and_apply_entry(&mint, &first_chain_entry).await;
    assert!(mint
        .probe_federation_mint_quote_payment(&quote_id)
        .await
        .expect("delayed payment probe")
        .is_none());
    let second_chain_entry = appended_entry(
        &mut journal,
        second_runtime
            .chain_observation_operation(confirmed_observation(
                &second_runtime,
                application_id,
                &allocation,
                second_runtime.member_id(),
                3,
            ))
            .expect("second chain observation"),
    );
    persist_and_apply_entry(&mint, &second_chain_entry).await;

    let first_payment = mint
        .probe_federation_mint_quote_payment(&quote_id)
        .await
        .expect("payment probe")
        .expect("accepted payment");
    assert_eq!(first_payment.amount_paid, Amount::from(25_000));
    let mut second_payment = first_payment.clone();
    second_payment.observer = second_runtime.member_id();
    let first_payment_entry = appended_entry(
        &mut journal,
        runtime.operation_envelope(FederationOperation::MintQuotePayment(first_payment)),
    );
    let second_payment_entry = appended_entry(
        &mut journal,
        second_runtime.operation_envelope(FederationOperation::MintQuotePayment(second_payment)),
    );
    persist_and_apply_entry(&mint, &first_payment_entry).await;
    assert_eq!(
        store
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote")
            .state(),
        MintQuoteState::Unpaid,
        "one member observation cannot credit the quote"
    );
    persist_and_apply_entry(&mint, &second_payment_entry).await;
    let paid_quote = store
        .get_mint_quote(&quote_id)
        .await
        .expect("paid quote lookup")
        .expect("paid quote");
    assert_eq!(paid_quote.state(), MintQuoteState::Paid);
    assert_eq!(paid_quote.amount_paid().value(), 25_000);
    let outpoint = OutPoint::new(Txid::from_byte_array([41; 32]), 1);
    assert!(
        mint.federation_chain_observation_state()
            .await
            .expect("wallet state")
            .utxo(outpoint)
            .expect("payment output")
            .liability_issued
    );
    assert!(mint
        .probe_federation_mint_quote_payment(&quote_id)
        .await
        .expect("duplicate probe")
        .is_none());

    let keyset = runtime.public_config().keysets[0].clone();
    let output = output_for_keyset(&keyset, b"federated on-chain mint output");
    let mut mint_request = MintRequest {
        quote: quote_id.clone(),
        outputs: vec![output.clone()],
        signature: None,
    };
    mint_request
        .sign(&wallet_key)
        .expect("ordinary NUT-20 mint signature");
    let mint_entry = appended_entry(
        &mut journal,
        runtime
            .mint_operation(MintInput::Single(mint_request))
            .expect("ordinary mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;
    assert!(store
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("mint signature")
        .first()
        .is_some_and(Option::is_some));

    let second_outpoint = OutPoint::new(Txid::from_byte_array([43; 32]), 0);
    for (member_runtime, depth) in [(&runtime, 2), (&second_runtime, 3)] {
        let mut observation = confirmed_observation(
            member_runtime,
            application_id,
            &allocation,
            member_runtime.member_id(),
            depth,
        );
        let FederationChainObservation::Deposit(deposit) = &mut observation else {
            panic!("deposit observation");
        };
        deposit.outpoint = second_outpoint;
        deposit.txout.value = BitcoinAmount::from_sat(5_000);
        deposit.confirmation = Some(FederationDepositConfirmation {
            block_hash: BlockHash::from_byte_array([44; 32]),
            block_height: 201,
            confirmation_depth: depth,
        });
        let entry = appended_entry(
            &mut journal,
            member_runtime
                .chain_observation_operation(observation)
                .expect("additional chain observation"),
        );
        persist_and_apply_entry(&mint, &entry).await;
    }
    let updated_payment = mint
        .probe_federation_mint_quote_payment(&quote_id)
        .await
        .expect("updated payment probe")
        .expect("additional accepted payment");
    assert_eq!(updated_payment.amount_paid, Amount::from(30_000));
    assert_eq!(updated_payment.amount_issued, Amount::from(1));
    let mut second_updated_payment = updated_payment.clone();
    second_updated_payment.observer = second_runtime.member_id();
    for (member_runtime, observation) in [
        (&runtime, updated_payment),
        (&second_runtime, second_updated_payment),
    ] {
        let entry = appended_entry(
            &mut journal,
            member_runtime.operation_envelope(FederationOperation::MintQuotePayment(observation)),
        );
        persist_and_apply_entry(&mint, &entry).await;
    }
    assert_eq!(
        store
            .get_mint_quote(&quote_id)
            .await
            .expect("updated quote lookup")
            .expect("updated quote")
            .amount_paid()
            .value(),
        30_000
    );
    assert!(
        mint.federation_chain_observation_state()
            .await
            .expect("updated wallet state")
            .utxo(second_outpoint)
            .expect("additional output")
            .liability_issued
    );

    let wrong_quote_id = QuoteId::new();
    let wrong_allocation_entry = appended_entry(
        &mut journal,
        runtime
            .address_allocation_operation(
                wrong_quote_id.to_string(),
                application_id,
                3,
                cdk_common::federation::FederationAddressKeychain::External,
            )
            .expect("wrong-address allocation"),
    );
    persist_and_apply_entry(&mint, &wrong_allocation_entry).await;
    let wrong_quote_entry = appended_entry(
        &mut journal,
        runtime.operation_envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: wrong_quote_id.to_string(),
                request_lookup_id: PaymentIdentifier::QuoteId(wrong_quote_id.clone()),
                request: MintQuoteRequest::Onchain(
                    cdk_common::nuts::nut30::MintQuoteOnchainRequest {
                        unit: CurrencyUnit::Sat,
                        pubkey: wallet_key.public_key(),
                    },
                ),
                response: MintQuoteResponse::Onchain(
                    cdk_common::nuts::nut30::MintQuoteOnchainResponse {
                        quote: wrong_quote_id.to_string(),
                        request: "bcrt1-unrelated-or-wrong-network-address".to_string(),
                        unit: CurrencyUnit::Sat,
                        method: PaymentMethod::Known(KnownMethod::Onchain),
                        expiry: None,
                        pubkey: wallet_key.public_key(),
                        amount_paid: Amount::ZERO,
                        amount_issued: Amount::ZERO,
                        updated_at: 0,
                    },
                ),
            },
        )),
    );
    persist_journal_entry(&store, &wrong_quote_entry).await;
    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&wrong_quote_entry))
            .await,
        Err(cdk_common::Error::Federation(
            cdk_common::federation::FederationError::OperationStateInvalid { .. }
        ))
    ));
    assert!(store
        .get_mint_quote(&wrong_quote_id)
        .await
        .expect("wrong quote lookup")
        .is_none());

    drop(mint);
    let restarted = restart_federated_test_mint(runtime, store).await;
    let restarted_quote = restarted
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("restart quote lookup")
        .expect("restart quote");
    assert_eq!(restarted_quote.request, address);
    assert_eq!(restarted_quote.amount_paid().value(), 30_000);
    assert_eq!(
        restarted
            .federation_address_allocation(
                cdk_common::federation::FederationAddressAllocationScope {
                    application_id,
                    application_key_epoch: 3,
                },
                &quote_id.to_string(),
            )
            .await
            .expect("restart allocation"),
        Some(allocation)
    );
}
