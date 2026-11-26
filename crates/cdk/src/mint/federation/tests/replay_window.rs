//! Operations re-ordered after their validity window.
//!
//! Duplicate and conflict detection only consults operations ordered in the
//! last `FEDERATION_OPERATION_VALIDITY_SESSIONS` sessions, on every member. An
//! operation older than that, freshly stamped by a member allowed to stamp it,
//! is therefore appended as a new accepted entry with its old operation id.
//! These tests apply such a replay after the original and check that the
//! mint's own state rejects it deterministically or treats it as already
//! applied, and that the materialized state does not change.

use bitcoin::hashes::Hash;
use bitcoin::{Amount as BitcoinAmount, BlockHash, OutPoint, Txid};
use cdk_common::database::mint::{FederationJournalEntryKey, FederationSignatureShareKey};
use cdk_common::federation::{
    FederationAcceptedTransactionState, FederationAddressAllocation,
    FederationAddressAllocationScope, FederationAddressKeychain, FederationChainObservation,
    FederationChainObservationState, FederationChainSourceDescriptor, FederationChainSourceScope,
    FederationDepositConfirmation, FederationDepositReorgObservation, FederationFrostApplicationId,
    FederationSignedTransaction, FederationTransactionBroadcastIntent,
    FederationTransactionBroadcastObservation, FederationTransactionBroadcastResult,
    FederationTransactionInput, FederationTransactionProposalId, FederationTransactionState,
    FederationWalletUtxo, FederationWalletUtxoState, MemberConfigPrivate,
    FEDERATION_OPERATION_VALIDITY_SESSIONS,
};

#[cfg(feature = "federation-bark")]
use super::address_allocation::attach_address_application;
use super::address_allocation::{
    runtime_with_address_application, second_member_runtime_with_address_application,
};
use super::chain_observation::confirmed_observation;
use super::transaction_policy::{
    build_proposal, intents, replacement_proposal, spendable_input, structurally_signed,
    ProposalOptions,
};
use super::transaction_signing::{persist_transaction_state, signed_fixture};
use super::*;
use crate::mint::is_definitive_federation_apply_rejection;

/// First session in which nothing ordered in session zero guards any more.
const REPLAY_SESSION: u64 = FEDERATION_OPERATION_VALIDITY_SESSIONS + 1;

/// What the mint does with an operation re-ordered after its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowReplay {
    /// The apply path recognised the operation's effect and changed nothing.
    AlreadyApplied,
    /// The apply path returned a deterministic rejection.
    Rejected,
}

/// Build the journal entry a member appends for `envelope` re-stamped by
/// `author` in [`REPLAY_SESSION`], after the entries of `journal`.
pub(super) fn window_replay_entry(
    journal: &InMemoryFederationJournal,
    envelope: &FederationOperationEnvelope,
    author: &MemberConfigPrivate,
) -> FederationJournalEntry {
    window_replay_entry_at(
        journal.next_index(),
        journal.latest_order_digest(),
        envelope,
        author,
    )
}

/// Like [`window_replay_entry`], directly after `previous`.
pub(super) fn window_replay_entry_after(
    previous: &FederationJournalEntry,
    envelope: &FederationOperationEnvelope,
    author: &MemberConfigPrivate,
) -> FederationJournalEntry {
    window_replay_entry_at(previous.index + 1, previous.order_digest, envelope, author)
}

fn window_replay_entry_at(
    index: u64,
    previous_order_digest: FederationJournalDigest,
    envelope: &FederationOperationEnvelope,
    author: &MemberConfigPrivate,
) -> FederationJournalEntry {
    let restamped = envelope
        .clone()
        .for_session(REPLAY_SESSION, author)
        .expect("re-stamp operation");
    let operation_id = restamped.operation_id().expect("operation id");
    assert_eq!(
        operation_id,
        envelope.operation_id().expect("original operation id"),
        "a fresh stamp keeps the operation id"
    );
    let mut entry = accepted_entry_without_journal(index, restamped);
    entry.previous_order_digest = previous_order_digest;
    entry.consensus_evidence = Some(FederationJournalConsensusEvidence::new(
        REPLAY_SESSION,
        index,
        0,
        author.member_id,
        0,
    ));
    entry
}

/// Persist and apply a re-ordered operation and classify the result.
///
/// The replay must leave the materialized-state digest unchanged and must
/// never end in a retryable failure, which would stall application.
pub(super) async fn apply_window_replay(
    mint: &Mint,
    localstore: &Arc<MintSqliteDatabase>,
    entry: &FederationJournalEntry,
) -> WindowReplay {
    let before = mint
        .federation_state_snapshot_rows()
        .await
        .expect("materialized state before replay");
    persist_journal_entry(localstore, entry).await;
    let outcome = match mint.apply_accepted_federation_operation(entry).await {
        Ok(_) => {
            let stored = localstore
                .get_federation_journal_entry(
                    FederationJournalEntryKey::Operation(entry.operation_id),
                    None,
                )
                .await
                .expect("replayed entry lookup")
                .expect("replayed entry");
            assert_eq!(stored.index, entry.index, "lookups return the latest entry");
            assert_eq!(stored.status(), FederationOperationStatus::Applied);
            WindowReplay::AlreadyApplied
        }
        Err(error) => {
            assert!(
                is_definitive_federation_apply_rejection(&error),
                "a replay must be rejected deterministically, not fail: {error}"
            );
            WindowReplay::Rejected
        }
    };
    let after = mint
        .federation_state_snapshot_rows()
        .await
        .expect("materialized state after replay");
    let changed = after
        .iter()
        .filter(|row| !before.contains(row))
        .map(|row| format!("{:?}", row.row))
        .collect::<Vec<_>>();
    assert!(
        changed.is_empty() && before.len() == after.len(),
        "a replay must not change materialized state ({outcome:?}): {changed:#?}"
    );
    outcome
}

#[tokio::test]
async fn mint_quote_replayed_after_its_window_is_already_applied_by_the_quote_row() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let envelope = runtime_config.mint_quote_operation(
        quote.clone(),
        bolt11_quote_request(5),
        bolt11_quote_response(&quote, 5),
    );
    let entry = appended_entry(&mut journal, envelope.clone());
    persist_and_apply_entry(&mint, &entry).await;

    let replay = window_replay_entry(&journal, &envelope, runtime_config.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &replay).await,
        WindowReplay::AlreadyApplied
    );
}

#[tokio::test]
async fn mint_quote_payment_replayed_after_its_window_does_not_pay_twice() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(5),
            bolt11_quote_response(&quote, 5),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let observation = runtime_config.mint_quote_payment_observation(
        quote.clone(),
        PaymentMethod::Known(KnownMethod::Bolt11),
        MintQuoteState::Paid,
        Amount::from(5),
        Amount::ZERO,
        Some("window-replay-payment".to_string()),
    );
    let observation_entry = appended_entry(&mut journal, observation.clone());
    persist_and_apply_entry(&mint, &observation_entry).await;

    let replay = window_replay_entry(&journal, &observation, runtime_config.private_config());
    let outcome = apply_window_replay(&mint, &localstore, &replay).await;
    let stored = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored.amount_paid(), Amount::new(5, CurrencyUnit::Sat));
    assert_eq!(outcome, WindowReplay::AlreadyApplied);
}

#[tokio::test]
async fn mint_replayed_after_its_window_is_rejected_by_the_issued_quote() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    for envelope in [
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("window-replay-mint-payment".to_string()),
        ),
    ] {
        let entry = appended_entry(&mut journal, envelope);
        persist_and_apply_entry(&mint, &entry).await;
    }
    let output = output_for_runtime(&runtime_config, b"window replay mint output");
    let envelope = runtime_config
        .mint_operation(MintInput::Single(MintRequest {
            quote: quote.clone(),
            outputs: vec![output.clone()],
            signature: None,
        }))
        .expect("mint operation");
    let entry = appended_entry(&mut journal, envelope.clone());
    persist_and_apply_entry(&mint, &entry).await;

    let replay = window_replay_entry(&journal, &envelope, runtime_config.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &replay).await,
        WindowReplay::Rejected
    );
    let stored = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored.amount_issued(), Amount::new(1, CurrencyUnit::Sat));
}

#[tokio::test]
async fn swap_replayed_after_its_window_is_already_applied_by_its_spent_proofs() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let input = proof_for_runtime(&runtime_config, "window replay swap input");
    let output = output_for_runtime(&runtime_config, b"window replay swap output");
    let envelope = runtime_config
        .swap_operation(SwapRequest::new(vec![input], vec![output.clone()]))
        .expect("swap operation");
    let entry = appended_entry(&mut journal, envelope.clone());
    persist_and_apply_entry(&mint, &entry).await;

    let replay = window_replay_entry(&journal, &envelope, runtime_config.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &replay).await,
        WindowReplay::AlreadyApplied
    );
    let shares = localstore
        .get_federation_signature_shares(FederationSignatureShareKey::Output {
            operation_id: entry.operation_id,
            output_index: 0,
        })
        .await
        .expect("swap shares");
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].accepted_operation_index, Some(entry.index));
}

#[tokio::test]
async fn keyset_rotation_replayed_after_its_window_changes_nothing() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;
    let envelope = runtime_config.operation_envelope(FederationOperation::KeysetRotation(
        FederationKeysetRotationOperation {
            keyset: next_keyset,
        },
    ));
    let entry = appended_entry(&mut journal, envelope.clone());
    persist_and_apply_entry(&mint, &entry).await;

    let replay = window_replay_entry(&journal, &envelope, runtime_config.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &replay).await,
        WindowReplay::AlreadyApplied
    );
}

#[tokio::test]
async fn melt_quote_replayed_after_its_window_is_already_applied_by_the_quote_row() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let envelope = melt_quote_op(
        &runtime_config,
        QuoteId::new(),
        pending_melt_quote_request(1_000),
    );
    let entry = appended_entry(&mut journal, envelope.clone());
    persist_and_apply_entry(&mint, &entry).await;

    let replay = window_replay_entry(&journal, &envelope, runtime_config.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &replay).await,
        WindowReplay::AlreadyApplied
    );
}

#[tokio::test]
async fn melt_and_its_payment_evidence_replayed_after_their_window_change_nothing() {
    let configs = runtime_configs();
    let runtime_config = configs[0].clone();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let proofs = vec![
        proof_for_runtime(&runtime_config, "window replay melt input one"),
        proof_for_runtime(&runtime_config, "window replay melt input two"),
    ];
    let melt = runtime_config
        .melt_operation(MeltRequest::new(quote.clone(), proofs, None))
        .expect("melt operation");
    let melt_entry = appended_entry(&mut journal, melt.clone());
    persist_and_apply_entry(&mint, &melt_entry).await;
    let observations = [1, 2].map(|observer| {
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("window replay melt"),
            },
        )
    });
    for observation in &observations {
        let entry = appended_entry(&mut journal, observation.clone());
        persist_and_apply_entry(&mint, &entry).await;
    }
    let paid = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(paid.state, MeltQuoteState::Paid);

    let melt_replay = window_replay_entry(&journal, &melt, runtime_config.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &melt_replay).await,
        WindowReplay::AlreadyApplied
    );
    let observation_replay =
        window_replay_entry_after(&melt_replay, &observations[1], configs[1].private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &observation_replay).await,
        WindowReplay::AlreadyApplied
    );
    let paid = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(paid.state, MeltQuoteState::Paid);
}

/// History entry ordered directly after `previous`, outside the in-memory
/// journal, for history that continues after a replay was persisted.
fn entry_after(
    previous: &FederationJournalEntry,
    envelope: FederationOperationEnvelope,
) -> FederationJournalEntry {
    accepted_entry_without_journal(previous.index + 1, envelope)
}

/// Two members of an on-chain federation whose deposits need both observers.
fn quorum_onchain_members() -> (
    FederationRuntimeConfig,
    FederationRuntimeConfig,
    FederationFrostApplicationId,
) {
    let (mut first, application_id) = runtime_with_address_application();
    let mut second = second_member_runtime_with_address_application();
    for member in [&mut first, &mut second] {
        member.public_config_mut().payment_observation_policy =
            FederationPaymentObservationPolicy::member_quorum(2);
    }
    (first, second, application_id)
}

const DEPOSIT_QUOTE: &str = "window-replay-deposit";

fn deposit_outpoint() -> OutPoint {
    OutPoint::new(Txid::from_byte_array([41; 32]), 1)
}

/// Allocate an address and make one deposit to it spendable with both
/// members' confirmed observations. Returns the allocation and the
/// observation envelopes, first member first.
async fn spendable_deposit(
    members: [&FederationRuntimeConfig; 2],
    application_id: FederationFrostApplicationId,
    mint: &Mint,
    journal: &mut InMemoryFederationJournal,
    value_sat: u64,
) -> (
    FederationAddressAllocation,
    Vec<FederationOperationEnvelope>,
) {
    let allocation_entry = appended_entry(
        journal,
        members[0]
            .address_allocation_operation(
                DEPOSIT_QUOTE,
                application_id,
                3,
                FederationAddressKeychain::External,
            )
            .expect("allocation operation"),
    );
    persist_and_apply_entry(mint, &allocation_entry).await;
    let allocation = mint
        .federation_address_allocation(
            FederationAddressAllocationScope {
                application_id,
                application_key_epoch: 3,
            },
            DEPOSIT_QUOTE,
        )
        .await
        .expect("allocation lookup")
        .expect("allocation");
    let mut deposits = Vec::new();
    for (member, depth) in [(members[0], 2), (members[1], 3)] {
        let mut observation = confirmed_observation(
            member,
            application_id,
            &allocation,
            member.member_id(),
            depth,
        );
        let FederationChainObservation::Deposit(deposit) = &mut observation else {
            panic!("deposit observation");
        };
        deposit.txout.value = BitcoinAmount::from_sat(value_sat);
        let envelope = member
            .chain_observation_operation(observation)
            .expect("deposit observation");
        let entry = appended_entry(journal, envelope.clone());
        persist_and_apply_entry(mint, &entry).await;
        deposits.push(envelope);
    }
    assert_eq!(
        deposit_utxo(mint).await.state,
        FederationWalletUtxoState::Spendable
    );
    (allocation, deposits)
}

async fn deposit_utxo(mint: &Mint) -> FederationWalletUtxo {
    mint.federation_chain_observation_state()
        .await
        .expect("chain state")
        .utxo(deposit_outpoint())
        .expect("deposit UTXO")
        .clone()
}

#[tokio::test]
async fn address_allocation_replayed_after_its_window_is_already_applied_by_the_quote_mapping() {
    let (runtime, application_id) = runtime_with_address_application();
    let second = second_member_runtime_with_address_application();
    let (mint, localstore) = federated_test_mint(runtime.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let envelope = runtime
        .address_allocation_operation(
            "window-replay-allocation",
            application_id,
            3,
            FederationAddressKeychain::External,
        )
        .expect("allocation operation");
    let entry = appended_entry(&mut journal, envelope.clone());
    persist_and_apply_entry(&mint, &entry).await;
    let later = appended_entry(
        &mut journal,
        runtime
            .address_allocation_operation(
                "window-replay-later-allocation",
                application_id,
                3,
                FederationAddressKeychain::External,
            )
            .expect("later allocation operation"),
    );
    persist_and_apply_entry(&mint, &later).await;

    let replay = window_replay_entry(&journal, &envelope, second.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &replay).await,
        WindowReplay::AlreadyApplied
    );
    let allocation = mint
        .federation_address_allocation(
            FederationAddressAllocationScope {
                application_id,
                application_key_epoch: 3,
            },
            "window-replay-allocation",
        )
        .await
        .expect("allocation lookup")
        .expect("allocation");
    assert_eq!(allocation.derivation_index, 0);
    assert_eq!(allocation.operation_index, entry.index);
}

#[tokio::test]
async fn deposit_observation_replayed_after_its_window_changes_nothing_even_once_reserved() {
    let (runtime, second, application_id) = quorum_onchain_members();
    let (mint, localstore) = federated_test_mint(runtime.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (_, deposits) = spendable_deposit(
        [&runtime, &second],
        application_id,
        &mint,
        &mut journal,
        40_000,
    )
    .await;

    let replay = window_replay_entry(&journal, &deposits[0], runtime.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &replay).await,
        WindowReplay::AlreadyApplied
    );

    // Spend the deposit: an accepted transaction proposal reserves it.
    let (batch, records) = intents(&runtime);
    let mut transaction_state = FederationTransactionState::new();
    for record in records.values().cloned() {
        transaction_state
            .add_pending_intent(record)
            .expect("pending intent");
    }
    persist_transaction_state(&localstore, &transaction_state).await;
    let utxo = deposit_utxo(&mint).await;
    let proposal = build_proposal(
        &runtime,
        &batch,
        vec![FederationTransactionInput {
            outpoint: utxo.outpoint,
            previous_output: utxo.txout.clone(),
            allocation: utxo.allocation.clone(),
        }],
        ProposalOptions::default(),
    );
    let proposal_entry = entry_after(
        &replay,
        runtime
            .transaction_proposal_operation(proposal)
            .expect("proposal operation"),
    );
    persist_and_apply_entry(&mint, &proposal_entry).await;
    let reserved = deposit_utxo(&mint).await;
    assert_eq!(reserved.state, FederationWalletUtxoState::Reserved);

    let first_replay =
        window_replay_entry_after(&proposal_entry, &deposits[0], runtime.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &first_replay).await,
        WindowReplay::AlreadyApplied
    );
    let second_replay =
        window_replay_entry_after(&first_replay, &deposits[1], second.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &second_replay).await,
        WindowReplay::AlreadyApplied
    );
    assert_eq!(deposit_utxo(&mint).await, reserved);
}

#[tokio::test]
async fn reorg_observation_replayed_after_its_window_changes_nothing_even_after_reconfirmation() {
    let (runtime, second, application_id) = quorum_onchain_members();
    let (mint, localstore) = federated_test_mint(runtime.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (allocation, deposits) = spendable_deposit(
        [&runtime, &second],
        application_id,
        &mint,
        &mut journal,
        25_000,
    )
    .await;
    let mut reorgs = Vec::new();
    for member in [&runtime, &second] {
        let envelope = member
            .chain_observation_operation(FederationChainObservation::Reorg(
                FederationDepositReorgObservation {
                    outpoint: deposit_outpoint(),
                    disconnected_block_hash: BlockHash::from_byte_array([42; 32]),
                    disconnected_block_height: 200,
                    observation_epoch: 0,
                    observer: member.member_id(),
                    source: FederationChainSourceDescriptor::new(
                        FederationChainSourceScope::Shared,
                        "shared-regtest-source",
                    )
                    .expect("source"),
                },
            ))
            .expect("reorg operation");
        let entry = appended_entry(&mut journal, envelope.clone());
        persist_and_apply_entry(&mint, &entry).await;
        reorgs.push(envelope);
    }
    assert_eq!(
        deposit_utxo(&mint).await.state,
        FederationWalletUtxoState::Reorged
    );

    let replay = window_replay_entry(&journal, &reorgs[0], runtime.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &replay).await,
        WindowReplay::AlreadyApplied
    );

    // Both members see the deposit again, in a different block.
    let mut previous = replay;
    for (member, depth) in [(&runtime, 2), (&second, 3)] {
        let mut observation = confirmed_observation(
            member,
            application_id,
            &allocation,
            member.member_id(),
            depth,
        );
        let FederationChainObservation::Deposit(deposit) = &mut observation else {
            panic!("deposit observation");
        };
        deposit.observation_epoch = 1;
        deposit.confirmation = Some(FederationDepositConfirmation {
            block_hash: BlockHash::from_byte_array([44; 32]),
            block_height: 201,
            confirmation_depth: depth,
        });
        let entry = entry_after(
            &previous,
            member
                .chain_observation_operation(observation)
                .expect("reconfirmation"),
        );
        persist_and_apply_entry(&mint, &entry).await;
        previous = entry;
    }
    let reconfirmed = deposit_utxo(&mint).await;
    assert_eq!(reconfirmed.state, FederationWalletUtxoState::Spendable);
    assert_eq!(reconfirmed.observation_epoch, 1);
    assert_eq!(
        reconfirmed.block,
        Some((BlockHash::from_byte_array([44; 32]), 201))
    );

    for (envelope, author) in [
        (&reorgs[0], runtime.private_config()),
        (&reorgs[1], second.private_config()),
        (&deposits[0], runtime.private_config()),
    ] {
        let replay = window_replay_entry_after(&previous, envelope, author);
        assert_eq!(
            apply_window_replay(&mint, &localstore, &replay).await,
            WindowReplay::AlreadyApplied
        );
        previous = replay;
    }
    assert_eq!(deposit_utxo(&mint).await, reconfirmed);
}

/// Persist a structurally signed transaction for an accepted proposal
/// directly, as a signing round would before it submits `TransactionSigned`.
async fn sign_accepted_transaction(
    mint: &Mint,
    localstore: &Arc<MintSqliteDatabase>,
    proposal_id: FederationTransactionProposalId,
) -> FederationSignedTransaction {
    let mut state = mint
        .federation_transaction_state()
        .await
        .expect("transaction state");
    let accepted = state
        .accepted_transaction(proposal_id)
        .expect("accepted transaction")
        .clone();
    let signed = structurally_signed(accepted.proposal, accepted.policy_audit)
        .signed
        .expect("structurally signed transaction");
    state
        .mark_transaction_signed(proposal_id, signed.clone())
        .expect("signed transaction");
    persist_transaction_state(localstore, &state).await;
    signed
}

async fn accepted_transaction_state(
    mint: &Mint,
    proposal_id: FederationTransactionProposalId,
) -> FederationAcceptedTransactionState {
    mint.federation_transaction_state()
        .await
        .expect("transaction state")
        .accepted_transaction(proposal_id)
        .expect("accepted transaction")
        .state
}

#[tokio::test]
async fn transaction_lineage_replayed_after_its_window_changes_nothing_once_replaced_and_confirmed()
{
    let (runtime, _) = runtime_with_address_application();
    let second = second_member_runtime_with_address_application();
    let (mint, localstore) = federated_test_mint(runtime.clone()).await;
    let mut chain_state = FederationChainObservationState::new();
    let input = spendable_input(&runtime, &mut chain_state, 21, 40_000);
    let (batch, records) = intents(&runtime);
    let mut transaction_state = FederationTransactionState::new();
    for record in records.values().cloned() {
        transaction_state
            .add_pending_intent(record)
            .expect("pending intent");
    }
    let mut setup = localstore.begin_transaction().await.expect("begin setup");
    cdk_common::federation::write_federation_state(&mut setup, chain_state)
        .await
        .expect("write chain state");
    cdk_common::federation::write_federation_state(&mut setup, transaction_state)
        .await
        .expect("write transaction state");
    setup.commit().await.expect("commit setup");

    let original = build_proposal(&runtime, &batch, vec![input], ProposalOptions::default());
    let original_id = original.proposal_id().expect("original id");
    let original_proposal = runtime
        .transaction_proposal_operation(original.clone())
        .expect("original proposal operation");
    let original_entry = accepted_entry_without_journal(1, original_proposal.clone());
    persist_and_apply_entry(&mint, &original_entry).await;

    let replay = window_replay_entry_after(
        &original_entry,
        &original_proposal,
        runtime.private_config(),
    );
    assert_eq!(
        apply_window_replay(&mint, &localstore, &replay).await,
        WindowReplay::AlreadyApplied
    );

    let original_signed = sign_accepted_transaction(&mint, &localstore, original_id).await;
    let settlement_policy = runtime
        .onchain_transaction_settlement_policy(original.scope)
        .expect("settlement policy");
    let original_intent =
        second.transaction_broadcast_intent_operation(FederationTransactionBroadcastIntent {
            proposal_id: original_id,
            txid: original_signed.txid,
            settlement_policy,
        });
    let intent_entry = entry_after(&replay, original_intent.clone());
    persist_and_apply_entry(&mint, &intent_entry).await;
    let original_observation = runtime
        .transaction_broadcast_observation_operation(FederationTransactionBroadcastObservation {
            proposal_id: original_id,
            txid: original_signed.txid,
            observer: runtime.member_id(),
            attempt: 1,
            result: FederationTransactionBroadcastResult::Unknown,
            detail: None,
        })
        .expect("original observation");
    let observation_entry = entry_after(&intent_entry, original_observation.clone());
    persist_and_apply_entry(&mint, &observation_entry).await;
    assert_eq!(
        accepted_transaction_state(&mint, original_id).await,
        FederationAcceptedTransactionState::BroadcastPending
    );

    let replacement = replacement_proposal(&runtime, &batch, &original, 500);
    let replacement_id = replacement.proposal_id().expect("replacement id");
    let replacement_proposal_envelope = runtime
        .transaction_proposal_operation(replacement)
        .expect("replacement operation");
    let replacement_entry = entry_after(&observation_entry, replacement_proposal_envelope.clone());
    persist_and_apply_entry(&mint, &replacement_entry).await;
    assert_eq!(
        accepted_transaction_state(&mint, original_id).await,
        FederationAcceptedTransactionState::Replaced { by: replacement_id }
    );

    let mut previous = replacement_entry;
    for (envelope, author) in [
        (&original_proposal, runtime.private_config()),
        (&original_intent, runtime.private_config()),
        (&original_observation, runtime.private_config()),
    ] {
        let replay = window_replay_entry_after(&previous, envelope, author);
        assert_eq!(
            apply_window_replay(&mint, &localstore, &replay).await,
            WindowReplay::AlreadyApplied,
            "{} replayed after replacement",
            envelope.operation.kind()
        );
        previous = replay;
    }

    // The replacement is broadcast and confirms; the original is abandoned.
    let replacement_signed = sign_accepted_transaction(&mint, &localstore, replacement_id).await;
    let replacement_intent =
        runtime.transaction_broadcast_intent_operation(FederationTransactionBroadcastIntent {
            proposal_id: replacement_id,
            txid: replacement_signed.txid,
            settlement_policy,
        });
    let entry = entry_after(&previous, replacement_intent.clone());
    persist_and_apply_entry(&mint, &entry).await;
    previous = entry;
    let mut confirmations = Vec::new();
    for member in [&runtime, &second] {
        let envelope = member
            .transaction_broadcast_observation_operation(
                FederationTransactionBroadcastObservation {
                    proposal_id: replacement_id,
                    txid: replacement_signed.txid,
                    observer: member.member_id(),
                    attempt: 2,
                    result: FederationTransactionBroadcastResult::Confirmed {
                        block_hash: BlockHash::from_byte_array([61; 32]),
                        block_height: 300,
                        confirmation_depth: settlement_policy.minimum_confirmations,
                    },
                    detail: None,
                },
            )
            .expect("confirmation");
        let entry = entry_after(&previous, envelope.clone());
        persist_and_apply_entry(&mint, &entry).await;
        previous = entry;
        confirmations.push((envelope, member.private_config()));
    }
    assert_eq!(
        accepted_transaction_state(&mint, replacement_id).await,
        FederationAcceptedTransactionState::Confirmed
    );
    assert_eq!(
        accepted_transaction_state(&mint, original_id).await,
        FederationAcceptedTransactionState::Abandoned
    );

    for (envelope, author) in [
        (&original_proposal, runtime.private_config()),
        (&original_intent, second.private_config()),
        (&original_observation, runtime.private_config()),
        (&replacement_proposal_envelope, runtime.private_config()),
        (&replacement_intent, second.private_config()),
        (&confirmations[0].0, confirmations[0].1),
        (&confirmations[1].0, confirmations[1].1),
    ] {
        let replay = window_replay_entry_after(&previous, envelope, author);
        assert_eq!(
            apply_window_replay(&mint, &localstore, &replay).await,
            WindowReplay::AlreadyApplied,
            "{} replayed after confirmation",
            envelope.operation.kind()
        );
        previous = replay;
    }
}

#[tokio::test]
async fn signed_transaction_and_broadcast_replayed_after_their_window_change_nothing() {
    let (runtime, application_id) = runtime_with_address_application();
    let second = second_member_runtime_with_address_application();
    let (initial_state, signed) = signed_fixture(application_id);
    let scope = initial_state
        .accepted_transaction(signed.proposal_id)
        .expect("accepted transaction")
        .proposal
        .scope;
    let (mint, localstore) = federated_test_mint(runtime.clone()).await;
    persist_transaction_state(&localstore, &initial_state).await;
    let settlement_policy = runtime
        .onchain_transaction_settlement_policy(scope)
        .expect("settlement policy");

    let signed_envelope = runtime.transaction_signed_operation(signed.clone());
    let intent_envelope =
        second.transaction_broadcast_intent_operation(FederationTransactionBroadcastIntent {
            proposal_id: signed.proposal_id,
            txid: signed.txid,
            settlement_policy,
        });
    let observation = |member: &FederationRuntimeConfig,
                       attempt: u32,
                       result: FederationTransactionBroadcastResult| {
        member
            .transaction_broadcast_observation_operation(
                FederationTransactionBroadcastObservation {
                    proposal_id: signed.proposal_id,
                    txid: signed.txid,
                    observer: member.member_id(),
                    attempt,
                    result,
                    detail: None,
                },
            )
            .expect("broadcast observation")
    };
    let confirmed = FederationTransactionBroadcastResult::Confirmed {
        block_hash: BlockHash::from_byte_array([62; 32]),
        block_height: 400,
        confirmation_depth: settlement_policy.minimum_confirmations,
    };
    let unknown = observation(&runtime, 1, FederationTransactionBroadcastResult::Unknown);
    let already_known = observation(
        &runtime,
        2,
        FederationTransactionBroadcastResult::AlreadyKnown,
    );
    let first_confirmed = observation(&runtime, 3, confirmed);
    let second_confirmed = observation(&second, 1, confirmed);
    let mut history = Vec::<FederationJournalEntry>::new();
    for envelope in [
        &signed_envelope,
        &intent_envelope,
        &unknown,
        &already_known,
        &first_confirmed,
        &second_confirmed,
    ] {
        let entry = match history.last() {
            Some(previous) => entry_after(previous, envelope.clone()),
            None => accepted_entry_without_journal(20, envelope.clone()),
        };
        persist_and_apply_entry(&mint, &entry).await;
        history.push(entry);
    }
    assert_eq!(
        accepted_transaction_state(&mint, signed.proposal_id).await,
        FederationAcceptedTransactionState::Confirmed
    );

    let mut previous = history.pop().expect("history");
    for (envelope, author, expected) in [
        (
            &signed_envelope,
            second.private_config(),
            WindowReplay::AlreadyApplied,
        ),
        (
            &intent_envelope,
            runtime.private_config(),
            WindowReplay::AlreadyApplied,
        ),
        // Superseded attempts of the same observer are stale.
        (&unknown, runtime.private_config(), WindowReplay::Rejected),
        (
            &already_known,
            runtime.private_config(),
            WindowReplay::Rejected,
        ),
        (
            &first_confirmed,
            runtime.private_config(),
            WindowReplay::AlreadyApplied,
        ),
        (
            &second_confirmed,
            second.private_config(),
            WindowReplay::AlreadyApplied,
        ),
    ] {
        let replay = window_replay_entry_after(&previous, envelope, author);
        assert_eq!(
            apply_window_replay(&mint, &localstore, &replay).await,
            expected,
            "{} replayed after confirmation",
            envelope.operation.kind()
        );
        previous = replay;
    }
}

/// Members of a federation with an accepted Bark application.
#[cfg(feature = "federation-bark")]
fn runtimes_with_bark_application() -> Vec<FederationRuntimeConfig> {
    use cdk_common::bitcoin::secp256k1::Secp256k1;
    use cdk_federation_bark::{
        federation_bark_frost_applications, FederationBarkFrostApplicationConfig,
        FederationBarkPolicyConfig, FederationBarkReceiveAntiDosPolicy,
        FederationBarkRecoveryApplicationConfig, FederationBarkRecoverySetupConfig,
    };

    runtime_configs_with_signature_threshold(2)
        .into_iter()
        .map(|mut runtime| {
            attach_address_application(&mut runtime);
            let public = runtime.public_config().clone();
            let wallet = public.frost_wallet.clone().expect("wallet config");
            let ark_server = cdk_common::bitcoin::secp256k1::SecretKey::from_slice(&[72; 32])
                .expect("Ark server key")
                .public_key(&Secp256k1::new());
            let bark = FederationBarkFrostApplicationConfig::new(
                &public,
                &wallet.root,
                1,
                cdk_common::bitcoin::Network::Regtest,
                ark_server,
                FederationBarkPolicyConfig::new(
                    0,
                    0,
                    10,
                    5_000,
                    18,
                    18,
                    144,
                    18,
                    30,
                    FederationBarkReceiveAntiDosPolicy::NoProof,
                )
                .expect("Bark policy"),
            )
            .expect("Bark application");
            let recovery = FederationBarkRecoveryApplicationConfig::new(
                &public,
                &wallet.root,
                FederationBarkRecoverySetupConfig {
                    application_key_epoch: 1,
                    network: cdk_common::bitcoin::Network::Regtest,
                    minimum_confirmations: 2,
                    fee_policy_version: 1,
                },
            )
            .expect("Bark recovery application");
            let applications =
                federation_bark_frost_applications(&public, &wallet.root, bark, recovery)
                    .expect("Bark applications");
            runtime.public_config_mut().frost_wallet = Some(
                cdk_common::federation::FederationFrostWalletConfig::new(
                    &public,
                    wallet.root,
                    wallet.cdk_onchain_application,
                    applications,
                )
                .expect("wallet config with Bark"),
            );
            runtime
        })
        .collect()
}

#[cfg(feature = "federation-bark")]
#[tokio::test]
async fn bark_intent_and_observation_replayed_after_their_window_are_already_applied() {
    use cdk_federation_bark::{
        ark_server_commitment, federation_bark_application, federation_bark_application_operation,
        federation_bark_description_hash, federation_bark_mailbox_scope, FederationBarkActionId,
        FederationBarkDigest, FederationBarkMemberObservation, FederationBarkObservationStatus,
        FederationBarkOperation, FederationBarkReceiveIntent, FederationBarkReceiveProof,
        FederationBarkReceiveSecretAcknowledgement, FederationBarkReceiveSecretBinding,
        FederationBarkUnit, FederationBarkVtxoLocator, FEDERATION_BARK_OPERATION_VERSION,
    };

    let members = runtimes_with_bark_application();
    let actor = &members[0];
    let public = actor.public_config();
    let application = federation_bark_application(public)
        .expect("Bark application")
        .expect("accepted Bark application");
    let digest = |seed: u8| FederationBarkDigest::from_bytes([seed; 32]).expect("Bark digest");
    let mailbox_id = [
        0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87,
        0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16,
        0xf8, 0x17, 0x98,
    ];
    let mut intent = FederationBarkReceiveIntent {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: FederationBarkActionId::from_bytes([1; 32]).expect("action ID"),
        quote: "window-replay-bark-quote".to_string(),
        actor: actor.member_id(),
        cashu_amount: Amount::from(100),
        gross_amount: Amount::from(100),
        receive_fee: Amount::ZERO,
        minimum_net_amount: Amount::from(100),
        unit: FederationBarkUnit::Sat,
        description_hash: federation_bark_description_hash(None).expect("description hash"),
        description: None,
        payment_hash: digest(2),
        preimage_commitment: digest(3),
        secret_acknowledgements: Vec::new(),
        destination: FederationBarkVtxoLocator::vtxo_custody(7),
        mailbox_scope: federation_bark_mailbox_scope(&mailbox_id).expect("mailbox scope"),
        mailbox_id,
        ark_server: ark_server_commitment(&application.ark_server_public_key)
            .expect("Ark server commitment"),
        network: application.network,
        minimum_cltv_delta: application.policy.minimum_incoming_cltv_delta,
        fee_policy_version: 1,
        anti_dos_policy_version: 1,
        receive_proof: FederationBarkReceiveProof::NoProof,
        expires_at: 1_900_000_000,
    };
    let binding = FederationBarkReceiveSecretBinding::from_intent(public.federation_id, &intent);
    intent.secret_acknowledgements = members
        .iter()
        .map(|member| {
            FederationBarkReceiveSecretAcknowledgement::new_signed(
                &binding,
                member.private_config(),
            )
            .expect("receive-secret acknowledgement")
        })
        .collect();
    let observer = &members[1];
    let observation = observer.operation_envelope(
        federation_bark_application_operation(FederationBarkOperation::MemberObservation(
            FederationBarkMemberObservation {
                version: FEDERATION_BARK_OPERATION_VERSION,
                action_id: intent.action_id,
                quote: Some(intent.quote.clone()),
                observer: observer.member_id(),
                observation_epoch: 1,
                source_scope: intent.ark_server,
                payment_hash: Some(intent.payment_hash),
                status: FederationBarkObservationStatus::Pending,
                amount: Amount::ZERO,
                fee: Amount::ZERO,
                result_hash: digest(6),
            },
        ))
        .expect("Bark observation operation"),
    );
    let intent = actor.operation_envelope(
        federation_bark_application_operation(FederationBarkOperation::ReceiveIntent(intent))
            .expect("Bark application operation"),
    );

    let (mint, localstore) = federated_test_mint(actor.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    for envelope in [&intent, &observation] {
        let entry = appended_entry(&mut journal, envelope.clone());
        persist_and_apply_entry(&mint, &entry).await;
    }

    let intent_replay = window_replay_entry(&journal, &intent, actor.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &intent_replay).await,
        WindowReplay::AlreadyApplied
    );
    let observation_replay =
        window_replay_entry_after(&intent_replay, &observation, observer.private_config());
    assert_eq!(
        apply_window_replay(&mint, &localstore, &observation_replay).await,
        WindowReplay::AlreadyApplied
    );
}
