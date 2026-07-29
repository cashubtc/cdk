use std::collections::BTreeMap;
use std::sync::Arc;

use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
use bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
use bitcoin::transaction::Version;
use bitcoin::{
    Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use cdk_common::federation::{
    FederationAcceptedTransactionState, FederationAddressKeychain, FederationAllocatedAddressRef,
    FederationFrostApplicationId, FederationFrostInputSignatureAudit,
    FederationOnchainPaymentBatch, FederationOnchainPaymentIntent,
    FederationOnchainPaymentIntentRecord, FederationOperationId, FederationSignedTransaction,
    FederationTransactionBroadcastIntent, FederationTransactionBroadcastObservation,
    FederationTransactionBroadcastResult, FederationTransactionChangeOutput,
    FederationTransactionInput, FederationTransactionPolicyAudit, FederationTransactionProposal,
    FederationTransactionRecipientOutput, FederationTransactionScope, FederationTransactionState,
    FederationWalletLivenessSchedule,
};

use super::super::transaction_state::{
    TRANSACTION_STATE_KEY, TRANSACTION_STATE_PRIMARY_NAMESPACE,
    TRANSACTION_STATE_SECONDARY_NAMESPACE,
};
use super::address_allocation::runtime_with_address_application;
use super::*;

fn signed_fixture(
    application_id: FederationFrostApplicationId,
) -> (FederationTransactionState, FederationSignedTransaction) {
    let scope = FederationTransactionScope {
        root_key_epoch: 1,
        application_id,
        application_key_epoch: 3,
        network: Network::Regtest,
        policy_version: 1,
    };
    let intent = FederationOnchainPaymentIntent::new(
        "signed-quote".to_string(),
        FederationOperationId::from_bytes([4; 32]),
        ScriptBuf::from_bytes(vec![0x51]),
        10_000,
        1_000,
        0,
        1,
        1,
        scope,
    )
    .expect("intent");
    signed_fixture_for_intent(intent, 500)
}

pub(super) fn signed_fixture_for_intent(
    intent: FederationOnchainPaymentIntent,
    fee_sat: u64,
) -> (FederationTransactionState, FederationSignedTransaction) {
    let secp = Secp256k1::new();
    let secret = SecretKey::from_slice(&[3; 32]).expect("secret key");
    let keypair = Keypair::from_secret_key(&secp, &secret);
    let (output_key, _) = XOnlyPublicKey::from_keypair(&keypair);
    let mut input_script = vec![0x51, 0x20];
    input_script.extend_from_slice(&output_key.serialize());
    let input_script = ScriptBuf::from_bytes(input_script);
    let scope = intent.scope;
    let batch = FederationOnchainPaymentBatch::new(vec![intent.clone()], intent.creation_time)
        .expect("batch");
    let outpoint = OutPoint::new(Txid::from_byte_array([9; 32]), 0);
    let change_sat = 9_500_u64;
    let previous_output = TxOut {
        value: Amount::from_sat(
            intent
                .amount_sat
                .checked_add(fee_sat)
                .and_then(|value| value.checked_add(change_sat))
                .expect("input value"),
        ),
        script_pubkey: input_script.clone(),
    };
    let selected_input = FederationTransactionInput {
        outpoint,
        previous_output: previous_output.clone(),
        allocation: FederationAllocatedAddressRef {
            application_id: scope.application_id,
            application_key_epoch: scope.application_key_epoch,
            quote: "signed-deposit".to_string(),
            keychain: FederationAddressKeychain::External,
            derivation_index: 0,
        },
    };
    let recipient = TxOut {
        value: Amount::from_sat(intent.amount_sat),
        script_pubkey: intent.destination_script.clone(),
    };
    let change = TxOut {
        value: Amount::from_sat(change_sat),
        script_pubkey: input_script,
    };
    let unsigned_transaction = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: outpoint,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        }],
        output: vec![recipient.clone(), change.clone()],
    };
    let proposal = FederationTransactionProposal::new(
        &batch,
        MemberId::new(1).expect("member"),
        unsigned_transaction,
        vec![selected_input],
        vec![FederationTransactionRecipientOutput {
            intent_id: intent.intent_id().expect("intent id"),
            output_index: 0,
            output: recipient,
            fee_contribution_sat: fee_sat,
        }],
        Some(FederationTransactionChangeOutput {
            derivation_index: 0,
            output_index: 1,
            output: change,
        }),
        fee_sat,
        None,
    )
    .expect("proposal");
    let sighash = SighashCache::new(&proposal.unsigned_transaction)
        .taproot_key_spend_signature_hash(
            0,
            &Prevouts::All(std::slice::from_ref(&previous_output)),
            TapSighashType::Default,
        )
        .expect("Taproot sighash")
        .to_byte_array();
    let policy_audit = FederationTransactionPolicyAudit {
        proposal_id: proposal.proposal_id().expect("proposal id"),
        txid: proposal.unsigned_transaction.compute_txid(),
        fee_sat: proposal.fee_sat,
        fee_rate_sat_per_kwu: proposal.fee_rate_sat_per_kwu,
        weight: proposal.estimated_signed_weight(),
        fee_contributions: BTreeMap::from([(intent.intent_id().expect("intent id"), fee_sat)]),
        taproot_sighashes: vec![sighash],
    };
    let mut state = FederationTransactionState::new();
    state
        .add_pending_intent(
            FederationOnchainPaymentIntentRecord::pending(intent).expect("intent record"),
        )
        .expect("pending intent");
    state
        .accept_transaction(
            proposal.clone(),
            policy_audit,
            FederationOperationId::from_bytes([10; 32]),
            10,
        )
        .expect("accepted proposal");
    let accepted = state
        .accepted_transaction(proposal.proposal_id().expect("proposal id"))
        .expect("accepted transaction")
        .clone();
    let signature = secp
        .sign_schnorr_no_aux_rand(&Message::from_digest(sighash), &keypair)
        .serialize()
        .to_vec();
    let signer_set = FederationWalletLivenessSchedule::new(
        proposal.proposal_id().expect("proposal id").to_bytes(),
        [
            MemberId::new(1).expect("first member"),
            MemberId::new(2).expect("second member"),
        ],
        2,
    )
    .expect("wallet liveness schedule")
    .signing_plan(1)
    .expect("first signing plan")
    .signer_set
    .iter()
    .map(|member| member.get())
    .collect();
    let mut transaction = proposal.unsigned_transaction.clone();
    transaction.input[0].witness.push(&signature);
    let signed = FederationSignedTransaction::new(
        &accepted,
        &transaction,
        vec![FederationFrostInputSignatureAudit {
            input_index: 0,
            session_id: [11; 32],
            attempt: 1,
            signer_set,
            package_hash: [12; 32],
            signature,
        }],
    )
    .expect("signed transaction");
    signed
        .verify_signatures(&proposal)
        .expect("signed transaction verifies");
    (state, signed)
}

pub(super) async fn persist_transaction_state(
    store: &Arc<MintSqliteDatabase>,
    state: &FederationTransactionState,
) {
    let mut transaction = store.begin_transaction().await.expect("begin state write");
    transaction
        .kv_write(
            TRANSACTION_STATE_PRIMARY_NAMESPACE,
            TRANSACTION_STATE_SECONDARY_NAMESPACE,
            TRANSACTION_STATE_KEY,
            &state.to_bytes().expect("transaction state bytes"),
        )
        .await
        .expect("write transaction state");
    transaction
        .commit()
        .await
        .expect("commit transaction state");
}

#[tokio::test]
async fn signed_operation_converges_exact_bytes_and_rejects_conflicting_replay() {
    let (runtime, application_id) = runtime_with_address_application();
    let (initial_state, signed) = signed_fixture(application_id);
    let envelope = runtime.transaction_signed_operation(signed.clone());
    let entry = accepted_entry_without_journal(20, envelope);
    let (first, first_store) = federated_test_mint(runtime.clone()).await;
    let (second, second_store) = federated_test_mint(runtime.clone()).await;
    persist_transaction_state(&first_store, &initial_state).await;
    persist_transaction_state(&second_store, &initial_state).await;
    let mut unregistered_audit = signed.clone();
    unregistered_audit.input_signatures[0].signer_set = vec![99];
    assert!(first
        .validate_federation_signed_transaction_submission(&unregistered_audit)
        .await
        .is_err());
    persist_journal_entry(&first_store, &entry).await;
    persist_journal_entry(&second_store, &entry).await;

    assert!(first
        .apply_accepted_federation_operation(&entry)
        .await
        .expect("first member apply"));
    assert!(second
        .apply_accepted_federation_operation(&entry)
        .await
        .expect("second member catch-up apply"));
    let first_state = first
        .federation_transaction_state()
        .await
        .expect("first state");
    let second_state = second
        .federation_transaction_state()
        .await
        .expect("second state");
    assert_eq!(first_state, second_state);
    let durable = first_state
        .accepted_transaction(signed.proposal_id)
        .expect("accepted transaction");
    assert_eq!(durable.state, FederationAcceptedTransactionState::Signed);
    assert_eq!(durable.signed.as_ref(), Some(&signed));
    assert!(!first
        .apply_accepted_federation_operation(&entry)
        .await
        .expect("exact replay"));

    let restarted = restart_federated_test_mint(runtime, Arc::clone(&first_store)).await;
    assert_eq!(
        restarted
            .federation_transaction_state()
            .await
            .expect("restarted signed state"),
        first_state
    );

    let mut conflicting = signed;
    conflicting.input_signatures[0].package_hash[0] ^= 1;
    assert!(first
        .validate_federation_signed_transaction_submission(&conflicting)
        .await
        .is_err());
}

#[tokio::test]
async fn broadcast_operations_persist_intent_before_results_and_resume_after_restart() {
    let (runtime, application_id) = runtime_with_address_application();
    let (initial_state, signed) = signed_fixture(application_id);
    let scope = initial_state
        .accepted_transaction(signed.proposal_id)
        .expect("accepted transaction")
        .proposal
        .scope;
    let (first, first_store) = federated_test_mint(runtime.clone()).await;
    let (second, second_store) = federated_test_mint(runtime.clone()).await;
    persist_transaction_state(&first_store, &initial_state).await;
    persist_transaction_state(&second_store, &initial_state).await;

    let signed_entry =
        accepted_entry_without_journal(20, runtime.transaction_signed_operation(signed.clone()));
    for (mint, store) in [(&first, &first_store), (&second, &second_store)] {
        persist_journal_entry(store, &signed_entry).await;
        assert!(mint
            .apply_accepted_federation_operation(&signed_entry)
            .await
            .expect("apply signed transaction"));
    }
    let premature = FederationTransactionBroadcastObservation {
        proposal_id: signed.proposal_id,
        txid: signed.txid,
        observer: runtime.member_id(),
        attempt: 1,
        result: FederationTransactionBroadcastResult::Unknown,
        detail: Some("ambiguous response".to_string()),
    };
    assert!(first
        .validate_federation_transaction_broadcast_observation_submission(&premature)
        .await
        .is_err());

    let settlement_policy = runtime
        .onchain_transaction_settlement_policy(scope)
        .expect("settlement policy");
    let mut wrong_policy = settlement_policy;
    wrong_policy.minimum_confirmations = wrong_policy.minimum_confirmations.saturating_add(1);
    assert!(first
        .validate_federation_transaction_broadcast_intent_submission(
            &FederationTransactionBroadcastIntent {
                proposal_id: signed.proposal_id,
                txid: signed.txid,
                settlement_policy: wrong_policy,
            }
        )
        .await
        .is_err());

    let intent_entry = accepted_entry_without_journal(
        21,
        runtime.transaction_broadcast_intent_operation(FederationTransactionBroadcastIntent {
            proposal_id: signed.proposal_id,
            txid: signed.txid,
            settlement_policy,
        }),
    );
    for (mint, store) in [(&first, &first_store), (&second, &second_store)] {
        persist_journal_entry(store, &intent_entry).await;
        assert!(mint
            .apply_accepted_federation_operation(&intent_entry)
            .await
            .expect("apply broadcast intent"));
    }

    let unknown_entry = accepted_entry_without_journal(
        22,
        runtime
            .transaction_broadcast_observation_operation(premature)
            .expect("unknown observation"),
    );
    for (mint, store) in [(&first, &first_store), (&second, &second_store)] {
        persist_journal_entry(store, &unknown_entry).await;
        assert!(mint
            .apply_accepted_federation_operation(&unknown_entry)
            .await
            .expect("apply ambiguous result"));
    }
    assert_eq!(
        first
            .federation_transaction_state()
            .await
            .expect("pending state")
            .accepted_transaction(signed.proposal_id)
            .expect("transaction")
            .state,
        FederationAcceptedTransactionState::BroadcastPending
    );

    let already_known_entry = accepted_entry_without_journal(
        23,
        runtime
            .transaction_broadcast_observation_operation(
                FederationTransactionBroadcastObservation {
                    proposal_id: signed.proposal_id,
                    txid: signed.txid,
                    observer: runtime.member_id(),
                    attempt: 2,
                    result: FederationTransactionBroadcastResult::AlreadyKnown,
                    detail: None,
                },
            )
            .expect("already-known observation"),
    );
    for (mint, store) in [(&first, &first_store), (&second, &second_store)] {
        persist_journal_entry(store, &already_known_entry).await;
        assert!(mint
            .apply_accepted_federation_operation(&already_known_entry)
            .await
            .expect("apply already-known result"));
    }
    let first_state = first
        .federation_transaction_state()
        .await
        .expect("first broadcast state");
    assert_eq!(
        first_state,
        second
            .federation_transaction_state()
            .await
            .expect("second broadcast state")
    );
    let durable = first_state
        .accepted_transaction(signed.proposal_id)
        .expect("durable broadcast");
    assert_eq!(durable.state, FederationAcceptedTransactionState::Broadcast);
    assert_eq!(durable.signed.as_ref(), Some(&signed));
    let broadcast = durable.broadcast.as_ref().expect("broadcast");
    assert_eq!(broadcast.observations.len(), 1);
    assert!(broadcast.first_success.is_some());

    let restarted = restart_federated_test_mint(runtime, Arc::clone(&first_store)).await;
    assert_eq!(
        restarted
            .federation_transaction_state()
            .await
            .expect("restarted broadcast state"),
        first_state
    );
}
