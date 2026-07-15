use super::*;

#[test]
fn runtime_config_keyset_rotation_requires_local_secret_share() {
    let mut runtime_config = runtime_config_with_inactive_rotation_keyset();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;
    runtime_config
        .private_config
        .keyset_secret_shares
        .remove(&next_keyset.id);

    let err = runtime_config
        .apply_keyset_rotation(&FederationKeysetRotationOperation {
            keyset: next_keyset,
        })
        .expect_err("rotation without local secret share must fail");

    assert!(matches!(
        err,
        Error::Federation(FederationError::MissingPrivateKeyset { .. })
    ));
}

#[test]
fn runtime_config_keyset_rotation_rejects_wrong_local_secret_share() {
    let mut runtime_config = runtime_config_with_inactive_rotation_keyset();
    let old_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("active keyset")
        .clone();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;
    let wrong_shares = runtime_config
        .private_config
        .keyset_secret_shares
        .get(&old_keyset.id)
        .expect("old keyset secret shares")
        .clone();
    runtime_config
        .private_config
        .keyset_secret_shares
        .insert(next_keyset.id, wrong_shares);

    let err = runtime_config
        .apply_keyset_rotation(&FederationKeysetRotationOperation {
            keyset: next_keyset,
        })
        .expect_err("rotation with wrong local secret share must fail");

    assert!(matches!(
        err,
        Error::Federation(FederationError::PrivateShareMismatch { .. })
    ));
}

#[tokio::test]
async fn apply_federation_keyset_rotation_marks_operation_applied() {
    let runtime_config = runtime_config();
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let keyset = runtime_config.public_config().keysets[0].clone();
    let entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation { keyset },
        )),
    );

    persist_and_apply_entry(&mint, &entry).await;

    let persisted = mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(
        persisted
            .iter()
            .find(|candidate| candidate.operation_id == entry.operation_id)
            .expect("rotation entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn effective_federation_config_replays_keyset_rotations() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let old_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("old active keyset")
        .clone();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;
    let entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation {
                keyset: next_keyset.clone(),
            },
        )),
    );

    persist_and_apply_entry(&mint, &entry).await;

    let effective = mint
        .effective_federation_config()
        .await
        .expect("effective config")
        .expect("federation config");
    assert!(
        !effective
            .public_config()
            .keysets
            .iter()
            .find(|keyset| keyset.id == old_keyset.id)
            .expect("old keyset")
            .active
    );
    assert!(
        effective
            .public_config()
            .keysets
            .iter()
            .find(|keyset| keyset.id == next_keyset.id)
            .expect("next keyset")
            .active
    );
    assert!(matches!(
        effective.validate_output_keysets(&[output_for_keyset(
            &old_keyset,
            b"old output after rotation"
        )]),
        Err(cdk_common::Error::Federation(
            FederationError::InactiveFederatedKeyset { .. }
        ))
    ));
    effective
        .validate_output_keysets(&[output_for_keyset(
            &next_keyset,
            b"next output after rotation",
        )])
        .expect("next keyset signs after rotation");
    effective
        .verify_proofs(&vec![proof_for_keyset(&old_keyset, 2, "old proof")])
        .expect("old proofs remain verifiable after rotation");

    let second_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation {
                keyset: old_keyset.clone(),
            },
        )),
    );
    persist_and_apply_entry(&mint, &second_entry).await;

    let reverted = mint
        .effective_federation_config()
        .await
        .expect("reverted effective config")
        .expect("federation config");
    assert!(
        reverted
            .public_config()
            .keysets
            .iter()
            .find(|keyset| keyset.id == old_keyset.id)
            .expect("old keyset")
            .active
    );
    assert!(
        !reverted
            .public_config()
            .keysets
            .iter()
            .find(|keyset| keyset.id == next_keyset.id)
            .expect("next keyset")
            .active
    );
}

#[tokio::test]
async fn effective_federation_config_reuses_cached_rotation_index() {
    let (mint, _) = federated_test_mint(runtime_config()).await;
    mint.federation_effective_config_rebuild_count
        .store(0, std::sync::atomic::Ordering::SeqCst);

    mint.effective_federation_config()
        .await
        .expect("first effective config")
        .expect("federation config");
    mint.effective_federation_config()
        .await
        .expect("cached effective config")
        .expect("federation config");

    assert_eq!(
        mint.federation_effective_config_rebuild_count
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "an unchanged federation config must load the rotation index only once"
    );
}

#[tokio::test]
async fn effective_federation_keyset_responses_use_rotated_config() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let old_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("old active keyset")
        .clone();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;
    let rotation_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation {
                keyset: next_keyset.clone(),
            },
        )),
    );
    persist_and_apply_entry(&mint, &rotation_entry).await;

    let pubkeys = mint
        .effective_federation_pubkeys()
        .await
        .expect("effective pubkeys")
        .expect("federation pubkeys");
    let keysets = mint
        .effective_federation_keysets()
        .await
        .expect("effective keysets")
        .expect("federation keysets");
    let old_specific = mint
        .effective_federation_keyset_pubkeys(&old_keyset.id)
        .await
        .expect("old specific keyset")
        .expect("federation specific keyset");
    let next_specific = mint
        .effective_federation_keyset_pubkeys(&next_keyset.id)
        .await
        .expect("next specific keyset")
        .expect("federation specific keyset");

    assert_eq!(pubkeys.keysets.len(), 1);
    assert_eq!(pubkeys.keysets[0].id, next_keyset.id);
    assert_eq!(pubkeys.keysets[0].keys, next_keyset.keys());
    assert_eq!(keysets.keysets.len(), 2);
    assert!(
        !keysets
            .keysets
            .iter()
            .find(|keyset| keyset.id == old_keyset.id)
            .expect("old keyset response")
            .active
    );
    assert!(
        keysets
            .keysets
            .iter()
            .find(|keyset| keyset.id == next_keyset.id)
            .expect("next keyset response")
            .active
    );
    assert_eq!(old_specific.keysets[0].id, old_keyset.id);
    assert_eq!(old_specific.keysets[0].active, Some(false));
    assert_eq!(next_specific.keysets[0].id, next_keyset.id);
    assert_eq!(next_specific.keysets[0].active, Some(true));
}

#[tokio::test]
async fn restore_returns_old_keyset_signature_after_rotation() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let old_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("old active keyset")
        .clone();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;

    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(2),
            bolt11_quote_response(&quote, 2),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(2),
            Amount::ZERO,
            Some("old-restore-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let old_outputs = vec![
        output_for_keyset(&old_keyset, b"old restore output zero"),
        output_for_keyset(&old_keyset, b"old restore output one"),
    ];
    let input = MintInput::Single(MintRequest {
        quote: quote.clone(),
        outputs: old_outputs.clone(),
        signature: None,
    });
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(input)
            .expect("old mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let rotation_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation {
                keyset: next_keyset,
            },
        )),
    );
    persist_and_apply_entry(&mint, &rotation_entry).await;
    let effective = mint
        .effective_federation_config()
        .await
        .expect("effective config")
        .expect("federation config");
    assert!(matches!(
        effective.validate_output_keysets(&old_outputs),
        Err(cdk_common::Error::Federation(
            FederationError::InactiveFederatedKeyset { .. }
        ))
    ));

    let restored = mint
        .restore(RestoreRequest {
            outputs: vec![old_outputs[1].clone()],
        })
        .await
        .expect("restore old signature");
    assert_eq!(restored.outputs, vec![old_outputs[1].clone()]);
    assert_eq!(restored.signatures.len(), 1);
    assert_eq!(restored.signatures[0].keyset_id, old_keyset.id);

    let share_records = localstore
        .get_federation_signature_shares_for_blinded_message(restored.outputs[0].blinded_secret)
        .await
        .expect("stored federation restore signature share");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, mint_entry.operation_id);
    assert_eq!(share_records[0].output_index, 1);
    assert_eq!(share_records[0].keyset_id, old_keyset.id);
    assert_eq!(
        share_records[0].accepted_operation_index,
        Some(mint_entry.index)
    );
    assert_eq!(
        share_records[0].consensus_evidence,
        mint_entry.consensus_evidence
    );
    assert_eq!(
        restored.signatures[0].c,
        PublicKey::from(share_records[0].share.signature())
    );

    let operation_output_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 1)
        .await
        .expect("stored federation restore signature share by operation output");
    assert_eq!(operation_output_records, share_records);
}
