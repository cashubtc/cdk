use super::super::FederationMode;
use super::*;

#[tokio::test]
async fn verify_federation_inputs_rejects_oversized_secret_and_witness_content() {
    use cdk_common::nuts::{P2PKWitness, Witness};

    let runtime_config = runtime_config();
    let keyset_id = runtime_config.public_config().keysets[0].id;
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;

    let oversized_secret = Proof::new(
        Amount::from(1),
        keyset_id,
        Secret::new("s".repeat(crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1)),
        SecretKey::generate().public_key(),
    );
    let oversized_secret_proofs = vec![oversized_secret];
    let secret_err = mint
        .verify_federation_inputs_with_config(&runtime_config, &oversized_secret_proofs)
        .expect_err("oversized federation proof secret should fail");

    assert!(
        matches!(
            secret_err,
            Error::ProofContentTooLarge {
                actual,
                max: crate::mint::verification::MAX_PROOF_CONTENT_LEN,
            } if actual == crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1
        ),
        "expected oversized federation secret ProofContentTooLarge, got: {:?}",
        secret_err
    );

    let mut oversized_witness = Proof::new(
        Amount::from(1),
        keyset_id,
        Secret::new("small-secret"),
        SecretKey::generate().public_key(),
    );
    oversized_witness.witness = Some(Witness::P2PKWitness(P2PKWitness {
        signatures: vec!["s".repeat(crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1)],
    }));
    let oversized_witness_proofs = vec![oversized_witness];
    let witness_err = mint
        .verify_federation_inputs_with_config(&runtime_config, &oversized_witness_proofs)
        .expect_err("oversized federation proof witness should fail");

    assert!(
        matches!(
            witness_err,
            Error::ProofContentTooLarge {
                max: crate::mint::verification::MAX_PROOF_CONTENT_LEN,
                ..
            }
        ),
        "expected oversized federation witness ProofContentTooLarge, got: {:?}",
        witness_err
    );
}

#[test]
fn federation_mode_defaults_to_standalone() {
    assert_eq!(FederationMode::default(), FederationMode::Standalone);
    assert!(!FederationMode::default().is_member());
    assert!(FederationMode::Member.is_member());
}

#[test]
fn runtime_config_caches_the_validated_public_config_digest() {
    let runtime_config = runtime_config();

    assert_eq!(
        runtime_config.config_digest(),
        runtime_config
            .public_config()
            .config_digest()
            .expect("validated public config digest")
    );
}

fn assert_requires_consensus(err: Error, operation: &str) {
    let message = err.to_string();
    assert!(
        message.contains("requires consensus acceptance"),
        "unexpected member-mode guard error: {message}"
    );
    assert!(
        message.contains(operation),
        "guard error should name {operation}: {message}"
    );
}

#[tokio::test]
async fn federation_member_rejects_direct_standalone_value_paths() {
    let runtime_config = runtime_config();
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;

    let mint_input = MintInput::Single(MintRequest {
        quote: QuoteId::new(),
        outputs: vec![output_for_runtime(&runtime_config, b"direct member mint")],
        signature: None,
    });
    let err = mint
        .process_mint_request(mint_input)
        .await
        .expect_err("federation member must not use standalone mint");
    assert_requires_consensus(err, "mint issuance");

    let swap = SwapRequest::new(
        vec![proof_for_runtime(&runtime_config, "direct-member-swap")],
        vec![output_for_runtime(&runtime_config, b"direct member swap")],
    );
    let err = mint
        .process_swap_request(swap)
        .await
        .expect_err("federation member must not use standalone swap");
    assert_requires_consensus(err, "swap");

    let melt = MeltRequest::new(
        QuoteId::new(),
        vec![proof_for_runtime(&runtime_config, "direct-member-melt")],
        None,
    );
    let err = mint
        .melt(&melt)
        .await
        .expect_err("federation member must not use standalone melt");
    assert_requires_consensus(err, "melt");

    let err = mint
        .blind_sign(vec![output_for_runtime(
            &runtime_config,
            b"direct member raw signing",
        )])
        .await
        .expect_err("federation member must not use raw standalone signing");
    assert_requires_consensus(err, "raw blind signing");
}

#[tokio::test]
async fn apply_accepted_federation_operation_applies_accepted_and_applying_entries() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();

    let accepted_quote = QuoteId::new();
    let accepted_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            accepted_quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&accepted_quote, 1),
        ),
    );
    persist_journal_entry(&localstore, &accepted_entry).await;
    assert!(mint
        .apply_accepted_federation_operation(&accepted_entry)
        .await
        .expect("accepted apply"));
    assert!(localstore
        .get_mint_quote(&accepted_quote)
        .await
        .expect("accepted quote lookup")
        .is_some());

    let applying_quote = QuoteId::new();
    let mut applying_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            applying_quote.clone(),
            bolt11_quote_request(2),
            bolt11_quote_response(&applying_quote, 2),
        ),
    );
    applying_entry.application = FederationApplicationState::Applying {};
    persist_journal_entry(&localstore, &applying_entry).await;
    assert!(mint
        .apply_accepted_federation_operation(&applying_entry)
        .await
        .expect("applying apply"));
    assert!(localstore
        .get_mint_quote(&applying_quote)
        .await
        .expect("applying quote lookup")
        .is_some());
}

#[tokio::test]
async fn apply_accepted_federation_operation_skips_applied_entries() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let mut entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    entry.application = FederationApplicationState::Applied {};

    assert!(!mint
        .apply_accepted_federation_operation(&entry)
        .await
        .expect("applied entry skip"));
    assert!(localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .is_none());
}

#[tokio::test]
async fn apply_accepted_federation_operation_rejects_terminal_local_statuses() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;

    for status in [
        FederationOperationStatus::Rejected,
        FederationOperationStatus::Failed,
    ] {
        let mut journal = InMemoryFederationJournal::new();
        let quote = QuoteId::new();
        let mut entry = appended_entry(
            &mut journal,
            runtime_config.mint_quote_operation(
                quote.clone(),
                bolt11_quote_request(1),
                bolt11_quote_response(&quote, 1),
            ),
        );
        entry.application = FederationApplicationState::from_status(status);

        let err = mint
            .apply_accepted_federation_operation(&entry)
            .await
            .expect_err("terminal local status must not apply");
        assert!(
            err.to_string().contains("is not accepted for apply"),
            "unexpected terminal-status error: {err}"
        );
        assert!(localstore
            .get_mint_quote(&quote)
            .await
            .expect("quote lookup")
            .is_none());
    }
}

#[tokio::test]
async fn apply_accepted_federation_operation_retries_matching_failed_local_status() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );

    persist_journal_entry(&localstore, &entry).await;
    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.set_federation_operation_application_state(
        entry.operation_id,
        &FederationApplicationState::Failed {},
    )
    .await
    .expect("mark operation failed");
    tx.commit().await.expect("commit failed status");

    assert!(mint
        .apply_accepted_federation_operation(&entry)
        .await
        .expect("matching accepted operation should retry stale failed local status"));

    assert!(localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .is_some());
    let persisted = localstore
        .get_federation_journal_entry_by_operation_id(entry.operation_id)
        .await
        .expect("journal lookup")
        .expect("journal entry");
    assert_eq!(persisted.status(), FederationOperationStatus::Applied);
}

#[tokio::test]
async fn apply_accepted_federation_operation_rejects_rejected_local_status_retry() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );

    persist_journal_entry(&localstore, &entry).await;
    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.set_federation_operation_application_state(
        entry.operation_id,
        &FederationApplicationState::ConsensusRejected {},
    )
    .await
    .expect("mark operation rejected");
    tx.commit().await.expect("commit rejected status");

    let err = mint
        .apply_accepted_federation_operation(&entry)
        .await
        .expect_err("rejected local status remains terminal");
    assert!(
        err.to_string().contains("is not accepted for apply"),
        "unexpected rejected-status error: {err}"
    );
    assert!(localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .is_none());
}

#[test]
fn runtime_config_validates_public_and_private_configs() {
    let runtime_config = runtime_config();

    assert_eq!(
        runtime_config.federation_id(),
        runtime_config.public_config().federation_id
    );
    assert_eq!(runtime_config.member_id().get(), 1);
}

#[test]
fn runtime_config_construction_still_rejects_malformed_public_config() {
    let valid = runtime_config();
    let private_config = valid.private_config().clone();

    let mut mismatched_aggregate = valid.public_config().clone();
    let keyset = &mut mismatched_aggregate.keysets[0];
    let amount = *keyset
        .aggregate_keys
        .keys()
        .next()
        .expect("aggregate amount");
    let member_id = *keyset.public_shares.keys().next().expect("keyset member");
    let public_share = keyset
        .public_shares
        .get_mut(&member_id)
        .and_then(|shares| shares.get_mut(&amount))
        .expect("member public share");
    *public_share = BlsPublicKeyShare::new(
        public_share.signer_id(),
        BlsSecretKey::from_reduced_bytes(&[99; 32]).public_key_g2(),
    );
    assert!(matches!(
        FederationRuntimeConfig::new(mismatched_aggregate, private_config.clone()),
        Err(Error::Federation(
            FederationError::AggregateKeyMismatch { .. }
        ))
    ));

    let mut missing_transcript_signature = valid.public_config().clone();
    missing_transcript_signature
        .setup_transcript
        .as_mut()
        .expect("setup transcript")
        .signatures
        .remove(&valid.member_id());
    assert!(matches!(
        FederationRuntimeConfig::new(missing_transcript_signature, private_config),
        Err(Error::Federation(
            FederationError::DkgTranscriptMismatch { .. }
        ))
    ));
}
