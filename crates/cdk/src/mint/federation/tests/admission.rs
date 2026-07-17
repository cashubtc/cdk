use super::*;

#[test]
fn runtime_config_builds_single_and_batch_mint_operations() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let single_input = super::MintInput::Single(MintRequest {
        quote: quote.clone(),
        outputs: vec![output_for_runtime(&runtime_config, b"single mint")],
        signature: None,
    });
    runtime_config
        .validate_federated_mint_submission(&single_input)
        .expect("valid mint admission");
    let single = runtime_config
        .mint_operation(single_input)
        .expect("single mint operation");

    match single.operation {
        FederationOperation::Mint(operation) => match operation.request {
            FederationMintRequest::Single(request) => {
                assert_eq!(request.quote, quote.to_string());
            }
            FederationMintRequest::Batch(_) => panic!("expected single mint request"),
        },
        _ => panic!("expected mint operation"),
    }

    let quote_one = QuoteId::new();
    let quote_two = QuoteId::new();
    let quote_three = QuoteId::new();
    let batch = runtime_config
        .mint_operation(super::MintInput::Batch(BatchMintRequest {
            quotes: vec![quote_one.clone(), quote_two.clone()],
            quote_amounts: Some(vec![Amount::from(1), Amount::from(1)]),
            outputs: vec![output_for_runtime(&runtime_config, b"batch mint")],
            signatures: None,
        }))
        .expect("batch mint operation");

    assert_eq!(batch.operation.conflict_keys().len(), 2);
    let mut journal = InMemoryFederationJournal::new();
    journal.append(batch).expect("append batch mint");

    let overlapping_batch = runtime_config
        .mint_operation(super::MintInput::Batch(BatchMintRequest {
            quotes: vec![quote_two, quote_three],
            quote_amounts: Some(vec![Amount::from(1), Amount::from(1)]),
            outputs: vec![output_for_runtime(
                &runtime_config,
                b"overlapping batch mint",
            )],
            signatures: None,
        }))
        .expect("overlapping batch mint operation");

    assert!(matches!(
        journal.append(overlapping_batch),
        Err(FederationError::ConflictingOperation { .. })
    ));
}

#[test]
fn runtime_config_rejects_sliding_window_mint_output_attack() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let output_a = output_for_runtime(&runtime_config, b"sliding-window-output-a");
    let output_b = output_for_runtime(&runtime_config, b"sliding-window-output-b");
    let output_c = output_for_runtime(&runtime_config, b"sliding-window-output-c");
    let output_d = output_for_runtime(&runtime_config, b"sliding-window-output-d");

    let envelope_for_outputs = |outputs| {
        runtime_config
            .mint_operation(super::MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs,
                signature: None,
            }))
            .expect("mint operation")
    };

    let first = envelope_for_outputs(vec![output_a.clone(), output_b.clone(), output_c.clone()]);
    let overlapping_one =
        envelope_for_outputs(vec![output_b.clone(), output_c.clone(), output_d.clone()]);
    let overlapping_two =
        envelope_for_outputs(vec![output_c.clone(), output_d.clone(), output_a.clone()]);
    let overlapping_three = envelope_for_outputs(vec![output_d, output_a, output_b]);

    let first_operation_id = first.operation_id().expect("first operation id");
    let attempted_operation_ids = [
        overlapping_one
            .operation_id()
            .expect("overlap one operation id"),
        overlapping_two
            .operation_id()
            .expect("overlap two operation id"),
        overlapping_three
            .operation_id()
            .expect("overlap three operation id"),
    ];
    for attempted_operation_id in attempted_operation_ids {
        assert_ne!(first_operation_id, attempted_operation_id);
    }

    let mut journal = InMemoryFederationJournal::new();
    let first_response = journal
        .append(first.clone())
        .expect("append first sliding-window mint");
    let duplicate_response = journal
        .append(first)
        .expect("append duplicate sliding-window mint");

    assert_eq!(first_response.entry().operation_id, first_operation_id);
    assert_eq!(duplicate_response.entry().operation_id, first_operation_id);

    for envelope in [overlapping_one, overlapping_two, overlapping_three] {
        assert!(matches!(
            journal.append(envelope),
            Err(FederationError::ConflictingOperation { .. })
        ));
    }

    let entries = journal.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, first_operation_id);
    assert_eq!(entries[0].envelope.operation.conflict_keys().len(), 1);
}

#[test]
fn runtime_config_rejects_non_v3_mint_outputs_before_consensus() {
    let runtime_config = runtime_config();
    let input = super::MintInput::Single(MintRequest {
        quote: QuoteId::new(),
        outputs: vec![non_v3_output_for_runtime(&runtime_config)],
        signature: None,
    });

    assert!(matches!(
        runtime_config.validate_federated_mint_submission(&input),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
    assert!(matches!(
        runtime_config.mint_operation(input),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[test]
fn runtime_config_rejects_duplicate_outputs_before_consensus() {
    let runtime_config = runtime_config();
    let output = output_for_runtime(&runtime_config, b"duplicate-output admission");
    let duplicate_outputs = vec![output.clone(), output];

    let single_mint = super::MintInput::Single(MintRequest {
        quote: QuoteId::new(),
        outputs: duplicate_outputs.clone(),
        signature: None,
    });
    assert!(matches!(
        runtime_config.validate_federated_mint_submission(&single_mint),
        Err(Error::DuplicateOutputs)
    ));
    assert!(matches!(
        runtime_config.mint_operation(single_mint),
        Err(Error::DuplicateOutputs)
    ));

    let batch_mint = super::MintInput::Batch(BatchMintRequest {
        quotes: vec![QuoteId::new(), QuoteId::new()],
        quote_amounts: Some(vec![Amount::from(1), Amount::from(1)]),
        outputs: duplicate_outputs.clone(),
        signatures: None,
    });
    assert!(matches!(
        runtime_config.validate_federated_mint_submission(&batch_mint),
        Err(Error::DuplicateOutputs)
    ));
    assert!(matches!(
        runtime_config.mint_operation(batch_mint),
        Err(Error::DuplicateOutputs)
    ));

    let swap = SwapRequest::new(
        vec![proof_for_runtime(
            &runtime_config,
            "duplicate-output admission swap",
        )],
        duplicate_outputs.clone(),
    );
    assert!(matches!(
        runtime_config.validate_federated_swap_submission(&swap),
        Err(Error::DuplicateOutputs)
    ));
    assert!(matches!(
        runtime_config.swap_operation(swap),
        Err(Error::DuplicateOutputs)
    ));

    let mut change_outputs = duplicate_outputs;
    for output in &mut change_outputs {
        output.amount = Amount::ZERO;
    }
    let melt = MeltRequest::new(
        QuoteId::new(),
        vec![proof_for_runtime(
            &runtime_config,
            "duplicate-output admission melt",
        )],
        Some(change_outputs),
    );
    assert!(matches!(
        runtime_config.validate_federated_melt_submission(&melt),
        Err(Error::DuplicateOutputs)
    ));
    assert!(matches!(
        runtime_config.melt_operation(melt),
        Err(Error::DuplicateOutputs)
    ));

    let envelope =
        runtime_config.operation_envelope(FederationOperation::Mint(FederationMintOperation {
            request: FederationMintRequest::Single(MintRequest {
                quote: QuoteId::new().to_string(),
                outputs: vec![
                    output_for_runtime(&runtime_config, b"duplicate direct output"),
                    output_for_runtime(&runtime_config, b"duplicate direct output"),
                ],
                signature: None,
            }),
        }));
    assert!(matches!(
        runtime_config.validate_federated_operation_submission(&envelope.operation),
        Err(Error::DuplicateOutputs)
    ));
}

#[test]
fn runtime_config_rejects_unsupported_wallet_protocol_version() {
    let runtime_config = runtime_config();
    let mut public_config = runtime_config.public_config().clone();
    let private_config = runtime_config.private_config().clone();
    let expected = public_config.wallet_protocol_version;
    let found = expected.saturating_add(1);
    public_config.wallet_protocol_version = found;

    assert!(matches!(
        FederationRuntimeConfig::new(public_config, private_config),
        Err(cdk_common::Error::Federation(
            FederationError::ProtocolVersionMismatch {
                component: "wallet",
                expected: actual_expected,
                found: actual_found,
            }
        )) if actual_expected == expected && actual_found == found
    ));
}

#[test]
fn runtime_config_rejects_non_v3_swap_restore_and_change_outputs() {
    let runtime_config = runtime_config();
    let output = non_v3_output_for_runtime(&runtime_config);
    let swap = SwapRequest::new(Vec::new(), vec![output.clone()]);
    let restore = RestoreRequest {
        outputs: vec![output.clone()],
    };
    let melt = MeltRequest::new(QuoteId::new(), Vec::new(), Some(vec![output]));

    assert!(matches!(
        runtime_config.validate_federated_swap_submission(&swap),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
    assert!(matches!(
        runtime_config.validate_restore_outputs(&restore),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
    assert!(matches!(
        runtime_config.validate_federated_melt_submission(&melt),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
    assert!(matches!(
        runtime_config.melt_operation(melt),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[test]
fn runtime_config_operation_submission_admission_rejects_invalid_wallet_operations() {
    let runtime_config = runtime_config();
    let non_v3_output = non_v3_output_for_runtime(&runtime_config);
    let mint =
        runtime_config.operation_envelope(FederationOperation::Mint(FederationMintOperation {
            request: FederationMintRequest::Single(MintRequest {
                quote: QuoteId::new().to_string(),
                outputs: vec![non_v3_output.clone()],
                signature: None,
            }),
        }));
    let swap =
        runtime_config.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(Vec::new(), vec![non_v3_output.clone()]),
        }));
    let melt =
        runtime_config.operation_envelope(FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(
                QuoteId::new().to_string(),
                Vec::new(),
                Some(vec![non_v3_output.clone()]),
            ),
        }));
    let mut inactive_keyset = runtime_config.public_config().keysets[0].clone();
    inactive_keyset.active = false;
    let rotation = runtime_config.operation_envelope(FederationOperation::KeysetRotation(
        FederationKeysetRotationOperation {
            keyset: inactive_keyset,
        },
    ));

    for envelope in [mint, swap, melt, rotation] {
        assert!(runtime_config
            .validate_federated_operation_submission(&envelope.operation)
            .is_err());
    }
}

#[test]
fn runtime_config_operation_submission_admission_accepts_valid_wallet_operations() {
    let runtime_config = runtime_config();
    let mint = runtime_config
        .mint_operation(super::MintInput::Single(MintRequest {
            quote: QuoteId::new(),
            outputs: vec![output_for_runtime(&runtime_config, b"valid routed mint")],
            signature: None,
        }))
        .expect("mint operation");
    let swap = runtime_config
        .swap_operation(SwapRequest::new(
            Vec::new(),
            vec![output_for_runtime(&runtime_config, b"valid routed swap")],
        ))
        .expect("swap operation");
    let mut change = output_for_runtime(&runtime_config, b"valid routed melt change");
    change.amount = Amount::ZERO;
    let melt = runtime_config
        .melt_operation(MeltRequest::new(
            QuoteId::new(),
            Vec::new(),
            Some(vec![change]),
        ))
        .expect("melt operation");
    let rotation_runtime_config = runtime_config_with_inactive_rotation_keyset();
    let mut rotation_keyset = rotation_runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    rotation_keyset.active = true;
    let rotation = rotation_runtime_config.operation_envelope(FederationOperation::KeysetRotation(
        FederationKeysetRotationOperation {
            keyset: rotation_keyset,
        },
    ));

    for envelope in [mint, swap, melt] {
        runtime_config
            .validate_federated_operation_submission(&envelope.operation)
            .expect("valid wallet operation admission");
    }
    rotation_runtime_config
        .validate_federated_operation_submission(&rotation.operation)
        .expect("valid keyset rotation admission");
}

#[test]
fn runtime_config_admission_rejects_invalid_swap_and_melt_proofs_before_consensus() {
    use cdk_common::nuts::{P2PKWitness, Witness};

    let runtime_config = runtime_config();
    let mut invalid_swap_proof = proof_for_runtime(&runtime_config, "invalid swap admission");
    invalid_swap_proof.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"invalid swap admission signature",
    ));
    let swap = SwapRequest::new(
        vec![invalid_swap_proof],
        vec![output_for_runtime(
            &runtime_config,
            b"invalid swap admission output",
        )],
    );

    assert!(runtime_config
        .validate_federated_swap_submission(&swap)
        .is_err());
    assert!(runtime_config.swap_operation(swap).is_err());

    let oversized_swap_secret =
        Secret::new("s".repeat(crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1));
    let oversized_swap_proof = proof_for_secret(
        &runtime_config.public_config().keysets[0],
        2,
        oversized_swap_secret,
    );
    let oversized_swap = SwapRequest::new(
        vec![oversized_swap_proof],
        vec![output_for_runtime(
            &runtime_config,
            b"oversized swap admission output",
        )],
    );

    assert!(matches!(
        runtime_config.validate_federated_swap_submission(&oversized_swap),
        Err(Error::ProofContentTooLarge {
            actual,
            max: crate::mint::verification::MAX_PROOF_CONTENT_LEN,
        }) if actual == crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1
    ));
    assert!(matches!(
        runtime_config.swap_operation(oversized_swap),
        Err(Error::ProofContentTooLarge { .. })
    ));

    let duplicate_swap_proof = proof_for_runtime(&runtime_config, "duplicate swap admission");
    let duplicate_swap = SwapRequest::new(
        vec![duplicate_swap_proof.clone(), duplicate_swap_proof],
        vec![output_for_runtime(
            &runtime_config,
            b"duplicate swap admission output",
        )],
    );

    assert!(matches!(
        runtime_config.validate_federated_swap_submission(&duplicate_swap),
        Err(Error::DuplicateInputs)
    ));
    assert!(matches!(
        runtime_config.swap_operation(duplicate_swap),
        Err(Error::DuplicateInputs)
    ));

    let mut invalid_melt_proof = proof_for_runtime(&runtime_config, "invalid melt admission");
    invalid_melt_proof.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"invalid melt admission signature",
    ));
    let mut change_output = output_for_runtime(&runtime_config, b"invalid melt admission change");
    change_output.amount = Amount::ZERO;
    let melt = MeltRequest::new(
        QuoteId::new(),
        vec![invalid_melt_proof],
        Some(vec![change_output]),
    );

    assert!(runtime_config
        .validate_federated_melt_submission(&melt)
        .is_err());
    assert!(runtime_config.melt_operation(melt).is_err());

    let mut oversized_witness_melt_proof =
        proof_for_runtime(&runtime_config, "oversized melt admission witness");
    oversized_witness_melt_proof.witness = Some(Witness::P2PKWitness(P2PKWitness {
        signatures: vec!["s".repeat(crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1)],
    }));
    let mut oversized_melt_change =
        output_for_runtime(&runtime_config, b"oversized melt admission change");
    oversized_melt_change.amount = Amount::ZERO;
    let oversized_melt = MeltRequest::new(
        QuoteId::new(),
        vec![oversized_witness_melt_proof],
        Some(vec![oversized_melt_change]),
    );

    assert!(matches!(
        runtime_config.validate_federated_melt_submission(&oversized_melt),
        Err(Error::ProofContentTooLarge {
            max: crate::mint::verification::MAX_PROOF_CONTENT_LEN,
            ..
        })
    ));
    assert!(matches!(
        runtime_config.melt_operation(oversized_melt),
        Err(Error::ProofContentTooLarge { .. })
    ));

    let duplicate_melt_proof = proof_for_runtime(&runtime_config, "duplicate melt admission");
    let duplicate_melt = MeltRequest::new(
        QuoteId::new(),
        vec![duplicate_melt_proof.clone(), duplicate_melt_proof],
        None,
    );

    assert!(matches!(
        runtime_config.validate_federated_melt_submission(&duplicate_melt),
        Err(Error::DuplicateInputs)
    ));
    assert!(matches!(
        runtime_config.melt_operation(duplicate_melt),
        Err(Error::DuplicateInputs)
    ));
}

#[test]
fn runtime_config_verifies_aggregate_bls_proofs() {
    let runtime_config = runtime_config();
    let proof = proof_for_runtime(&runtime_config, "valid");

    runtime_config
        .verify_proofs(&vec![proof])
        .expect("aggregate BLS proof should verify");
}

#[test]
fn runtime_config_rejects_invalid_aggregate_bls_proofs() {
    let runtime_config = runtime_config();
    let mut proof = proof_for_runtime(&runtime_config, "invalid");
    proof.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(b"not a valid signature"));

    assert!(runtime_config.verify_proofs(&vec![proof]).is_err());
}

#[test]
fn runtime_config_batch_verification_matches_scalar_boundary_and_rejects_corruption() {
    let runtime_config = runtime_config();
    let proofs = (0..super::super::FEDERATION_BLS_PROOF_BATCH_MIN)
        .map(|index| proof_for_runtime(&runtime_config, &format!("batch-proof-{index}")))
        .collect::<Vec<_>>();

    runtime_config
        .verify_proofs(&proofs[..proofs.len() - 1].to_vec())
        .expect("proofs below the cutoff use valid scalar verification");
    runtime_config
        .verify_proofs(&proofs)
        .expect("proofs at the cutoff use valid batch verification");

    for index in [0, proofs.len() / 2, proofs.len() - 1] {
        let mut corrupted = proofs.clone();
        corrupted[index].c = PublicKey::from(BlsG1PublicKey::hash_to_curve(
            format!("corrupted batch proof {index}").as_bytes(),
        ));
        assert!(runtime_config.verify_proofs(&corrupted).is_err());
    }
}

#[test]
fn runtime_config_rejects_non_v3_aggregate_proofs() {
    let runtime_config = runtime_config();
    let mut proof = proof_for_runtime(&runtime_config, "non-v3");
    let keyset = &runtime_config.public_config().keysets[0];
    proof.keyset_id = Id::v2_from_data(
        &keyset.keys(),
        &keyset.unit,
        keyset.input_fee_ppk,
        keyset.final_expiry,
    );

    assert!(matches!(
        runtime_config.verify_proofs(&vec![proof]),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[test]
fn runtime_config_builds_swap_operations() {
    let runtime_config = runtime_config();
    let swap = SwapRequest::new(
        Vec::new(),
        vec![output_for_runtime(&runtime_config, b"swap operation")],
    );
    let envelope = runtime_config.swap_operation(swap).expect("swap operation");

    assert_eq!(envelope.federation_id, runtime_config.federation_id());
    assert!(matches!(envelope.operation, FederationOperation::Swap(_)));
    assert_eq!(envelope.operation.conflict_keys().len(), 0);
}

#[test]
fn runtime_config_builds_melt_operations() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let mut change_output = output_for_runtime(&runtime_config, b"melt operation change");
    change_output.amount = Amount::ZERO;
    let melt = MeltRequest::new(quote.clone(), Vec::new(), Some(vec![change_output]))
        .prefer_async(true)
        .fee_index(42);
    let envelope = runtime_config.melt_operation(melt).expect("melt operation");

    assert_eq!(envelope.federation_id, runtime_config.federation_id());
    assert_eq!(envelope.operation.conflict_keys().len(), 1);
    match envelope.operation {
        FederationOperation::Melt(operation) => {
            assert_eq!(operation.request.quote(), &quote.to_string());
            assert!(operation.request.is_prefer_async());
            assert_eq!(operation.request.selected_fee_index(), Some(42));
        }
        _ => panic!("expected melt operation"),
    }
}

#[test]
fn runtime_config_accepts_zero_amount_melt_change_placeholders() {
    let runtime_config = runtime_config();
    let mut output = output_for_runtime(&runtime_config, b"zero amount change placeholder");
    output.amount = Amount::ZERO;
    let melt = MeltRequest::new(QuoteId::new(), Vec::new(), Some(vec![output.clone()]));
    let restore = RestoreRequest {
        outputs: vec![output.clone()],
    };

    assert!(runtime_config.validate_melt_change_outputs(&melt).is_ok());
    assert!(runtime_config.validate_restore_outputs(&restore).is_ok());
    assert!(matches!(
        runtime_config.validate_output_keysets(&[output]),
        Err(cdk_common::Error::Federation(
            FederationError::UnknownFederatedKeysetAmount { .. }
        ))
    ));
}

#[test]
fn runtime_config_signs_accepted_mint_operations() {
    let runtime_config = runtime_config();
    let keyset = &runtime_config.public_config().keysets[0];
    let blinded_message = BlsG1PublicKey::hash_to_curve(b"cdk axum federation mint share");
    let input = super::MintInput::Single(MintRequest {
        quote: QuoteId::new(),
        outputs: vec![BlindedMessage::new(
            Amount::from(1),
            keyset.id,
            PublicKey::from(blinded_message),
        )],
        signature: None,
    });
    let operation_id = runtime_config
        .mint_operation(input.clone())
        .expect("mint operation")
        .operation_id()
        .expect("operation id");

    let share_set = runtime_config
        .mint_signature_shares(operation_id, &input)
        .expect("signature shares");
    let share = share_set.shares.first().expect("signature share");
    let public_share = keyset
        .public_shares
        .get(&runtime_config.member_id())
        .expect("member shares")
        .get(&Amount::from(1))
        .expect("amount share");

    assert_eq!(share_set.operation_id, operation_id);
    assert_eq!(share_set.member_id, runtime_config.member_id());
    assert_eq!(share.operation_id, operation_id);
    assert!(
        verify_blind_signature_share(&share.share, &blinded_message, public_share)
            .expect("share verification")
    );

    let response = runtime_config
        .mint_share_response(operation_id, &input)
        .expect("mint share response");
    let signature = response.signatures.first().expect("signature share");

    assert_eq!(response.signatures.len(), 1);
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, keyset.id);
    assert!(signature.dleq.is_none());
    assert_eq!(
        signature.c.as_bls_g1().expect("response share point"),
        share.share.signature()
    );
}

#[test]
fn runtime_config_signs_accepted_swap_operations() {
    let runtime_config = runtime_config();
    let keyset = &runtime_config.public_config().keysets[0];
    let blinded_message = BlsG1PublicKey::hash_to_curve(b"cdk federation swap share");
    let swap = SwapRequest::new(
        Vec::new(),
        vec![BlindedMessage::new(
            Amount::from(1),
            keyset.id,
            PublicKey::from(blinded_message),
        )],
    );
    let operation_id = runtime_config
        .swap_operation(swap.clone())
        .expect("swap operation")
        .operation_id()
        .expect("operation id");

    let share_set = runtime_config
        .swap_signature_shares(operation_id, &swap)
        .expect("signature shares");
    let share = share_set.shares.first().expect("signature share");
    let public_share = keyset
        .public_shares
        .get(&runtime_config.member_id())
        .expect("member shares")
        .get(&Amount::from(1))
        .expect("amount share");

    assert_eq!(share_set.operation_id, operation_id);
    assert_eq!(share_set.member_id, runtime_config.member_id());
    assert_eq!(share.operation_id, operation_id);
    assert!(
        verify_blind_signature_share(&share.share, &blinded_message, public_share)
            .expect("share verification")
    );

    let response = runtime_config
        .swap_share_response(operation_id, &swap)
        .expect("swap share response");
    let signature = response.signatures.first().expect("signature share");

    assert_eq!(response.signatures.len(), 1);
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, keyset.id);
    assert!(signature.dleq.is_none());
    assert_eq!(
        signature.c.as_bls_g1().expect("response share point"),
        share.share.signature()
    );
}
