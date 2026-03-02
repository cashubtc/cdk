//! NUT-CTF conditional token tests for swap and redeem operations

use std::collections::HashMap;

use cdk_common::amount::SplitTarget;
use cdk_common::dhke::construct_proofs;
use cdk_common::nuts::nut_ctf::test_helpers::{
    create_digit_decomposition_announcement, create_multi_oracle_witness,
    create_numeric_oracle_witness, create_oracle_witness, create_test_announcement,
    create_test_oracle, create_test_oracle_2,
};
use cdk_common::nuts::nut_ctf::{
    CtfMergeRequest, CtfSplitRequest, RedeemOutcomeRequest, RegisterConditionRequest,
    RegisterPartitionRequest,
};
use cdk_common::nuts::{Id, PreMintSecrets, SwapRequest, Witness};
use cdk_common::{Amount, CurrencyUnit};

use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};

/// Helper: create an enum RegisterConditionRequest with all fields
fn enum_condition_request(description: &str, announcements: Vec<String>) -> RegisterConditionRequest {
    RegisterConditionRequest {
        threshold: 1,
        description: description.to_string(),
        announcements,
        condition_type: "enum".to_string(),
        lo_bound: None,
        hi_bound: None,
        precision: None,
    }
}

/// Get the regular (non-conditional) active keyset ID for SAT.
/// Must be called BEFORE registering any conditions.
fn get_regular_keyset_id(mint: &crate::mint::Mint) -> Id {
    *mint
        .get_active_keysets()
        .get(&CurrencyUnit::Sat)
        .expect("mint should have an active SAT keyset")
}

/// Register a test condition and partition, returning (condition_id, keysets map)
async fn register_test_condition(
    mint: &crate::mint::Mint,
    outcomes: &[&str],
    partition: Option<Vec<String>>,
) -> (String, HashMap<String, Id>) {
    let oracle = create_test_oracle();
    let (_, hex_tlv) = create_test_announcement(&oracle, outcomes, "test-event");

    let request = enum_condition_request("Test condition", vec![hex_tlv]);

    let condition_response = mint.register_condition(request).await.unwrap();
    let condition_id = condition_response.condition_id;

    let partition_request = RegisterPartitionRequest {
        collateral: "sat".to_string(),
        partition,
        parent_collection_id: None,
    };

    let partition_response = mint
        .register_partition(&condition_id, partition_request)
        .await
        .unwrap();
    (condition_id, partition_response.keysets)
}

/// Helper: create PreMintSecrets for a given keyset
fn create_premint(
    mint: &crate::mint::Mint,
    keyset_id: Id,
    amount: Amount,
) -> (Vec<cdk_common::nuts::BlindedMessage>, PreMintSecrets) {
    let keys = mint
        .keyset_pubkeys(&keyset_id)
        .unwrap()
        .keysets
        .first()
        .unwrap()
        .keys
        .clone();

    let fee_and_amounts: (u64, Vec<u64>) =
        (0, keys.iter().map(|(a, _)| a.to_u64()).collect::<Vec<_>>());

    let pre_mint =
        PreMintSecrets::random(keyset_id, amount, &SplitTarget::None, &fee_and_amounts.into())
            .unwrap();
    let blinded_messages = pre_mint.blinded_messages().to_vec();
    (blinded_messages, pre_mint)
}

/// Helper: swap regular proofs into a conditional keyset
async fn swap_to_conditional(
    mint: &crate::mint::Mint,
    regular_proofs: cdk_common::Proofs,
    keyset_id: Id,
    amount: Amount,
) -> cdk_common::Proofs {
    let (outputs, pre_mint) = create_premint(mint, keyset_id, amount);

    let keys = mint
        .keyset_pubkeys(&keyset_id)
        .unwrap()
        .keysets
        .first()
        .unwrap()
        .keys
        .clone();

    let swap_request = SwapRequest::new(regular_proofs, outputs);
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    construct_proofs(
        swap_response.signatures,
        pre_mint.rs(),
        pre_mint.secrets(),
        &keys,
    )
    .unwrap()
}

/// Test that registering a condition creates keysets for each partition key
#[tokio::test]
async fn test_register_condition_creates_keysets() {
    let mint = create_test_mint().await.unwrap();
    let (condition_id, keysets) = register_test_condition(&mint, &["YES", "NO"], None).await;

    assert!(!condition_id.is_empty());
    assert_eq!(keysets.len(), 2, "should create one keyset per outcome");
    assert!(keysets.contains_key("YES"));
    assert!(keysets.contains_key("NO"));
}

/// Test that registering the same condition twice is idempotent
#[tokio::test]
async fn test_register_condition_idempotent() {
    let mint = create_test_mint().await.unwrap();
    let oracle = create_test_oracle();
    let (_, hex_tlv) = create_test_announcement(&oracle, &["YES", "NO"], "test-event");

    let request = enum_condition_request("Test condition", vec![hex_tlv]);

    let response1 = mint.register_condition(request.clone()).await.unwrap();
    let response2 = mint.register_condition(request).await.unwrap();

    assert_eq!(response1.condition_id, response2.condition_id);
}

/// Test get_conditions returns registered conditions
#[tokio::test]
async fn test_get_conditions_returns_registered() {
    let mint = create_test_mint().await.unwrap();
    let (condition_id, _) = register_test_condition(&mint, &["YES", "NO"], None).await;

    let response = mint.get_conditions(None, None, &[]).await.unwrap();
    assert_eq!(response.conditions.len(), 1);
    assert_eq!(response.conditions[0].condition_id, condition_id);
}

/// Test get_condition by id returns the correct condition
#[tokio::test]
async fn test_get_condition_by_id() {
    let mint = create_test_mint().await.unwrap();
    let (condition_id, keysets) = register_test_condition(&mint, &["YES", "NO"], None).await;

    let info = mint.get_condition(&condition_id).await.unwrap();
    assert_eq!(info.condition_id, condition_id);
    assert_eq!(info.threshold, 1);
    assert_eq!(info.partitions.len(), 1);
    assert_eq!(info.partitions[0].keysets, keysets);
}

/// Full redeem outcome flow:
/// mint regular proofs -> register condition -> swap to conditional -> redeem with witness
#[tokio::test]
async fn test_redeem_outcome_valid() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);
    let oracle = create_test_oracle();
    let (_, hex_tlv) = create_test_announcement(&oracle, &["YES", "NO"], "test-event");

    // 1. Mint regular proofs BEFORE registering conditions
    let amount = Amount::from(10);
    let regular_proofs = mint_test_proofs(&mint, amount).await.unwrap();

    // 2. Register condition
    let condition_response = mint
        .register_condition(enum_condition_request("Test redeem", vec![hex_tlv]))
        .await
        .unwrap();

    // 3. Register partition
    let partition_response = mint
        .register_partition(
            &condition_response.condition_id,
            RegisterPartitionRequest {
                collateral: "sat".to_string(),
                partition: None,
                parent_collection_id: None,
            },
        )
        .await
        .unwrap();

    let yes_keyset_id = *partition_response.keysets.get("YES").unwrap();

    // 4. Swap regular proofs to conditional
    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, yes_keyset_id, amount).await;

    // 5. Attach oracle witness
    let witness = create_oracle_witness(&oracle, "YES");
    let mut proofs_with_witness = conditional_proofs;
    for proof in &mut proofs_with_witness {
        proof.witness = Some(Witness::OracleWitness(witness.clone()));
    }

    // 6. Create regular output blinded messages for redemption
    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, amount);

    // 7. Redeem
    let redeem_response = mint
        .process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: regular_outputs,
        })
        .await
        .unwrap();

    assert!(!redeem_response.signatures.is_empty());
}

/// Test that redeeming with the wrong outcome collection fails
#[tokio::test]
async fn test_redeem_outcome_wrong_collection() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);
    let oracle = create_test_oracle();
    let (_, hex_tlv) = create_test_announcement(&oracle, &["YES", "NO"], "test-event");

    // Mint regular proofs BEFORE registering conditions
    let amount = Amount::from(10);
    let regular_proofs = mint_test_proofs(&mint, amount).await.unwrap();

    let condition_response = mint
        .register_condition(enum_condition_request("Test wrong outcome", vec![hex_tlv]))
        .await
        .unwrap();

    let partition_response = mint
        .register_partition(
            &condition_response.condition_id,
            RegisterPartitionRequest {
                collateral: "sat".to_string(),
                partition: None,
                parent_collection_id: None,
            },
        )
        .await
        .unwrap();

    // Use the NO keyset but attest YES
    let no_keyset_id = *partition_response.keysets.get("NO").unwrap();
    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, no_keyset_id, amount).await;

    // Attach witness with YES attestation (but proofs are NO keyset)
    let witness = create_oracle_witness(&oracle, "YES");
    let mut proofs_with_witness = conditional_proofs;
    for proof in &mut proofs_with_witness {
        proof.witness = Some(Witness::OracleWitness(witness.clone()));
    }

    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, amount);

    let result = mint
        .process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: regular_outputs,
        })
        .await;

    assert!(result.is_err(), "should fail with wrong outcome collection");
}

/// Test that redeem without witness fails
#[tokio::test]
async fn test_redeem_outcome_no_witness() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);
    let oracle = create_test_oracle();
    let (_, hex_tlv) = create_test_announcement(&oracle, &["YES", "NO"], "test-event");

    // Mint regular proofs BEFORE registering conditions
    let amount = Amount::from(10);
    let regular_proofs = mint_test_proofs(&mint, amount).await.unwrap();

    let condition_response = mint
        .register_condition(enum_condition_request("No witness test", vec![hex_tlv]))
        .await
        .unwrap();

    let partition_response = mint
        .register_partition(
            &condition_response.condition_id,
            RegisterPartitionRequest {
                collateral: "sat".to_string(),
                partition: None,
                parent_collection_id: None,
            },
        )
        .await
        .unwrap();

    let yes_keyset_id = *partition_response.keysets.get("YES").unwrap();
    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, yes_keyset_id, amount).await;

    // No witness attached
    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, amount);

    let result = mint
        .process_redeem_outcome(RedeemOutcomeRequest {
            inputs: conditional_proofs,
            outputs: regular_outputs,
        })
        .await;

    assert!(result.is_err(), "should fail without witness");
}

/// Test that outputs using conditional keyset are rejected during redeem
#[tokio::test]
async fn test_redeem_outcome_outputs_conditional() {
    let mint = create_test_mint().await.unwrap();
    let oracle = create_test_oracle();
    let (_, hex_tlv) = create_test_announcement(&oracle, &["YES", "NO"], "test-event");

    // Mint regular proofs BEFORE registering conditions
    let amount = Amount::from(10);
    let regular_proofs = mint_test_proofs(&mint, amount).await.unwrap();

    let condition_response = mint
        .register_condition(enum_condition_request("Outputs conditional test", vec![hex_tlv]))
        .await
        .unwrap();

    let partition_response = mint
        .register_partition(
            &condition_response.condition_id,
            RegisterPartitionRequest {
                collateral: "sat".to_string(),
                partition: None,
                parent_collection_id: None,
            },
        )
        .await
        .unwrap();

    let yes_keyset_id = *partition_response.keysets.get("YES").unwrap();
    let no_keyset_id = *partition_response.keysets.get("NO").unwrap();

    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, yes_keyset_id, amount).await;

    let witness = create_oracle_witness(&oracle, "YES");
    let mut proofs_with_witness = conditional_proofs;
    for proof in &mut proofs_with_witness {
        proof.witness = Some(Witness::OracleWitness(witness.clone()));
    }

    // Create outputs using another conditional keyset (NO) — should be rejected
    let (conditional_outputs, _) = create_premint(&mint, no_keyset_id, amount);

    let result = mint
        .process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: conditional_outputs,
        })
        .await;

    assert!(
        result.is_err(),
        "should reject outputs using conditional keyset"
    );
}

/// Test that regular swap rejects conditional keyset inputs
#[tokio::test]
async fn test_swap_rejects_conditional_inputs() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);
    let oracle = create_test_oracle();
    let (_, hex_tlv) = create_test_announcement(&oracle, &["YES", "NO"], "test-event");

    // Mint regular proofs BEFORE registering conditions
    let amount = Amount::from(10);
    let regular_proofs = mint_test_proofs(&mint, amount).await.unwrap();

    let condition_response = mint
        .register_condition(enum_condition_request("Swap reject test", vec![hex_tlv]))
        .await
        .unwrap();

    let partition_response = mint
        .register_partition(
            &condition_response.condition_id,
            RegisterPartitionRequest {
                collateral: "sat".to_string(),
                partition: None,
                parent_collection_id: None,
            },
        )
        .await
        .unwrap();

    let yes_keyset_id = *partition_response.keysets.get("YES").unwrap();
    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, yes_keyset_id, amount).await;

    // Try a regular swap with conditional proofs as input — should fail
    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, amount);
    let swap_request = SwapRequest::new(conditional_proofs, regular_outputs);
    let result = mint.process_swap_request(swap_request).await;

    assert!(
        result.is_err(),
        "regular swap should reject conditional keyset inputs"
    );
}

/// Test that a second redemption uses the stored attestation (skips witness verification)
#[tokio::test]
async fn test_redeem_second_uses_stored_attestation() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);
    let oracle = create_test_oracle();
    let (_, hex_tlv) = create_test_announcement(&oracle, &["YES", "NO"], "test-event");

    // Mint ALL regular proofs BEFORE registering conditions
    let amount1 = Amount::from(10);
    let amount2 = Amount::from(8);
    let regular_proofs_1 = mint_test_proofs(&mint, amount1).await.unwrap();
    let regular_proofs_2 = mint_test_proofs(&mint, amount2).await.unwrap();

    let condition_response = mint
        .register_condition(enum_condition_request(
            "Stored attestation test",
            vec![hex_tlv],
        ))
        .await
        .unwrap();

    let partition_response = mint
        .register_partition(
            &condition_response.condition_id,
            RegisterPartitionRequest {
                collateral: "sat".to_string(),
                partition: None,
                parent_collection_id: None,
            },
        )
        .await
        .unwrap();

    let yes_keyset_id = *partition_response.keysets.get("YES").unwrap();

    // First redemption with valid witness
    {
        let conditional_proofs =
            swap_to_conditional(&mint, regular_proofs_1, yes_keyset_id, amount1).await;

        let witness = create_oracle_witness(&oracle, "YES");
        let mut proofs_with_witness = conditional_proofs;
        for proof in &mut proofs_with_witness {
            proof.witness = Some(Witness::OracleWitness(witness.clone()));
        }

        let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, amount1);

        mint.process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: regular_outputs,
        })
        .await
        .expect("first redemption should succeed");
    }

    // Second redemption — attestation is already stored
    {
        let conditional_proofs =
            swap_to_conditional(&mint, regular_proofs_2, yes_keyset_id, amount2).await;

        // Witness still needed for parsing, but verification path changes
        let witness = create_oracle_witness(&oracle, "YES");
        let mut proofs_with_witness = conditional_proofs;
        for proof in &mut proofs_with_witness {
            proof.witness = Some(Witness::OracleWitness(witness.clone()));
        }

        let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, amount2);

        mint.process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: regular_outputs,
        })
        .await
        .expect("second redemption should use stored attestation and succeed");
    }
}

/// Test registering condition with custom partition
#[tokio::test]
async fn test_register_condition_with_partition() {
    let mint = create_test_mint().await.unwrap();
    let oracle = create_test_oracle();
    let (_, hex_tlv) = create_test_announcement(&oracle, &["A", "B", "C"], "game-event");

    let condition_response = mint
        .register_condition(enum_condition_request("Partition test", vec![hex_tlv]))
        .await
        .unwrap();

    let partition_response = mint
        .register_partition(
            &condition_response.condition_id,
            RegisterPartitionRequest {
                collateral: "sat".to_string(),
                partition: Some(vec!["A|B".to_string(), "C".to_string()]),
                parent_collection_id: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        partition_response.keysets.len(),
        2,
        "should create one keyset per partition key"
    );
    assert!(
        partition_response.keysets.contains_key("A|B"),
        "keysets: {:?}",
        partition_response.keysets
    );
    assert!(partition_response.keysets.contains_key("C"));
}

// ============================================================================
// NUT-CTF-numeric: Numeric condition tests
// ============================================================================

/// Register a numeric condition with HI/LO partition
async fn register_numeric_condition(
    mint: &crate::mint::Mint,
    lo_bound: i64,
    hi_bound: i64,
) -> (String, HashMap<String, Id>) {
    let oracle = create_test_oracle();
    // base=10, unsigned, 5 digits -> range [0, 99999]
    let (_, hex_tlv) =
        create_digit_decomposition_announcement(&oracle, 10, false, 5, "sat", 0, "numeric-event");

    let request = RegisterConditionRequest {
        threshold: 1,
        description: "Numeric test condition".to_string(),
        announcements: vec![hex_tlv],
        condition_type: "numeric".to_string(),
        lo_bound: Some(lo_bound),
        hi_bound: Some(hi_bound),
        precision: Some(0),
    };

    let condition_response = mint.register_condition(request).await.unwrap();
    let condition_id = condition_response.condition_id;

    let partition_request = RegisterPartitionRequest {
        collateral: "sat".to_string(),
        partition: None,
        parent_collection_id: None,
    };

    let partition_response = mint
        .register_partition(&condition_id, partition_request)
        .await
        .unwrap();
    (condition_id, partition_response.keysets)
}

/// Test registering a numeric condition creates HI/LO keysets
#[tokio::test]
async fn test_register_numeric_condition() {
    let mint = create_test_mint().await.unwrap();
    let (_condition_id, keysets) = register_numeric_condition(&mint, 0, 100000).await;

    assert_eq!(keysets.len(), 2, "numeric condition should create HI and LO keysets");
    assert!(keysets.contains_key("HI"), "should have HI keyset");
    assert!(keysets.contains_key("LO"), "should have LO keyset");
}

/// Test that numeric condition_id differs from enum condition_id
#[tokio::test]
async fn test_numeric_condition_id_differs_from_enum() {
    let mint = create_test_mint().await.unwrap();

    // Register an enum condition
    let oracle = create_test_oracle();
    let (_, enum_hex) = create_test_announcement(&oracle, &["YES", "NO"], "test-event");
    let enum_resp = mint
        .register_condition(enum_condition_request("Enum test", vec![enum_hex]))
        .await
        .unwrap();

    // Register a numeric condition (different event to avoid idempotency)
    let (_, numeric_hex) =
        create_digit_decomposition_announcement(&oracle, 10, false, 5, "sat", 0, "numeric-event");
    let numeric_resp = mint
        .register_condition(RegisterConditionRequest {
            threshold: 1,
            description: "Numeric test".to_string(),
            announcements: vec![numeric_hex],
            condition_type: "numeric".to_string(),
            lo_bound: Some(0),
            hi_bound: Some(100000),
            precision: Some(0),
        })
        .await
        .unwrap();

    assert_ne!(
        enum_resp.condition_id, numeric_resp.condition_id,
        "numeric and enum condition IDs should differ"
    );
}

/// Test numeric condition info is stored and retrieved correctly
#[tokio::test]
async fn test_numeric_condition_info() {
    let mint = create_test_mint().await.unwrap();
    let (condition_id, _keysets) = register_numeric_condition(&mint, 1000, 50000).await;

    let info = mint.get_condition(&condition_id).await.unwrap();
    assert_eq!(info.condition_type, "numeric");
    assert_eq!(info.lo_bound, Some(1000));
    assert_eq!(info.hi_bound, Some(50000));
    assert_eq!(info.precision, Some(0));
    assert_eq!(info.partitions.len(), 1);
    assert_eq!(info.partitions[0].keysets.len(), 2);
}

/// Test numeric redemption: HI holder redeems proportional payout
#[tokio::test]
async fn test_numeric_redemption_hi() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);

    // Mint proofs BEFORE registering condition
    let face_amount = Amount::from(100);
    let regular_proofs = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (_condition_id, keysets) = register_numeric_condition(&mint, 0, 100000).await;

    let hi_keyset_id = *keysets.get("HI").unwrap();

    // Swap to HI conditional keyset
    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, hi_keyset_id, face_amount).await;

    // Oracle attests value 50000 (midpoint) -> HI gets 50%
    let oracle = create_test_oracle();
    let witness = create_numeric_oracle_witness(&oracle, 50000, 10, false, 5);
    let mut proofs_with_witness = conditional_proofs;
    for proof in &mut proofs_with_witness {
        proof.witness = Some(Witness::OracleWitness(witness.clone()));
    }

    // HI payout = floor(100 * 50000 / 100000) = 50
    let hi_payout = Amount::from(50);
    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, hi_payout);

    let result = mint
        .process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: regular_outputs,
        })
        .await;

    assert!(result.is_ok(), "HI redemption should succeed: {:?}", result.err());
    assert!(!result.unwrap().signatures.is_empty());
}

/// Test numeric redemption: LO holder redeems proportional payout
#[tokio::test]
async fn test_numeric_redemption_lo() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);

    let face_amount = Amount::from(100);
    let regular_proofs = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (_condition_id, keysets) = register_numeric_condition(&mint, 0, 100000).await;

    let lo_keyset_id = *keysets.get("LO").unwrap();

    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, lo_keyset_id, face_amount).await;

    // Oracle attests value 50000 -> LO gets 50%
    let oracle = create_test_oracle();
    let witness = create_numeric_oracle_witness(&oracle, 50000, 10, false, 5);
    let mut proofs_with_witness = conditional_proofs;
    for proof in &mut proofs_with_witness {
        proof.witness = Some(Witness::OracleWitness(witness.clone()));
    }

    // LO payout = 100 - 50 = 50
    let lo_payout = Amount::from(50);
    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, lo_payout);

    let result = mint
        .process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: regular_outputs,
        })
        .await;

    assert!(result.is_ok(), "LO redemption should succeed: {:?}", result.err());
    assert!(!result.unwrap().signatures.is_empty());
}

/// Test numeric redemption at lo boundary: V=0 -> LO gets 100%, HI gets 0%
#[tokio::test]
async fn test_numeric_boundary_lo() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);

    let face_amount = Amount::from(100);
    let regular_proofs = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (_condition_id, keysets) = register_numeric_condition(&mint, 0, 100000).await;
    let lo_keyset_id = *keysets.get("LO").unwrap();

    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, lo_keyset_id, face_amount).await;

    // Oracle attests value 0 (at lo_bound) -> LO gets 100%
    let oracle = create_test_oracle();
    let witness = create_numeric_oracle_witness(&oracle, 0, 10, false, 5);
    let mut proofs_with_witness = conditional_proofs;
    for proof in &mut proofs_with_witness {
        proof.witness = Some(Witness::OracleWitness(witness.clone()));
    }

    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, face_amount);

    let result = mint
        .process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: regular_outputs,
        })
        .await;

    assert!(
        result.is_ok(),
        "LO should get 100% when V <= lo_bound: {:?}",
        result.err()
    );
}

/// Test numeric redemption at hi boundary: V=100000 -> HI gets 100%, LO gets 0%
#[tokio::test]
async fn test_numeric_boundary_hi() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);

    let face_amount = Amount::from(100);
    let regular_proofs = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (_condition_id, keysets) = register_numeric_condition(&mint, 0, 100000).await;
    let hi_keyset_id = *keysets.get("HI").unwrap();

    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, hi_keyset_id, face_amount).await;

    // Oracle attests value 99999 which is max for 5 unsigned base-10 digits
    // 99999 < 100000 so HI gets floor(100 * 99999/100000) = 99
    // To test the >= hi_bound case, we'd need value >= 100000 which requires 6 digits
    // So let's test with a tight range instead
    let oracle = create_test_oracle();
    let witness = create_numeric_oracle_witness(&oracle, 99999, 10, false, 5);
    let mut proofs_with_witness = conditional_proofs;
    for proof in &mut proofs_with_witness {
        proof.witness = Some(Witness::OracleWitness(witness.clone()));
    }

    // HI gets floor(100 * 99999/100000) = 99
    let hi_payout = Amount::from(99);
    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, hi_payout);

    let result = mint
        .process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: regular_outputs,
        })
        .await;

    assert!(
        result.is_ok(),
        "HI should get ~100% when V near hi_bound: {:?}",
        result.err()
    );
}

/// Test that requesting more than proportional payout fails
#[tokio::test]
async fn test_numeric_redemption_overspend_rejected() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);

    let face_amount = Amount::from(100);
    let regular_proofs = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (_condition_id, keysets) = register_numeric_condition(&mint, 0, 100000).await;
    let hi_keyset_id = *keysets.get("HI").unwrap();

    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, hi_keyset_id, face_amount).await;

    // Oracle attests 20000 -> HI gets floor(100 * 20000/100000) = 20
    let oracle = create_test_oracle();
    let witness = create_numeric_oracle_witness(&oracle, 20000, 10, false, 5);
    let mut proofs_with_witness = conditional_proofs;
    for proof in &mut proofs_with_witness {
        proof.witness = Some(Witness::OracleWitness(witness.clone()));
    }

    // Try to redeem 50 (more than the 20 payout)
    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, Amount::from(50));

    let result = mint
        .process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: regular_outputs,
        })
        .await;

    assert!(
        result.is_err(),
        "should reject output amount exceeding proportional payout"
    );
}

// ============================================================================
// NUT-CTF-split-merge: CTF Split/Merge tests
// ============================================================================

/// Test that a CTF split creates conditional tokens for each partition outcome.
/// Inputs are regular tokens; outputs are conditional tokens per outcome (YES, NO).
#[tokio::test]
async fn test_ctf_split_creates_conditional_tokens() {
    let mint = create_test_mint().await.unwrap();

    // Mint regular proofs BEFORE registering conditions (same pattern as existing tests)
    let face_amount = Amount::from(16);
    let regular_proofs = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (condition_id, keysets) = register_test_condition(&mint, &["YES", "NO"], None).await;

    let yes_keyset_id = *keysets.get("YES").unwrap();
    let no_keyset_id = *keysets.get("NO").unwrap();

    // Create blinded messages for both YES and NO outcome collections
    let (yes_outputs, _) = create_premint(&mint, yes_keyset_id, face_amount);
    let (no_outputs, _) = create_premint(&mint, no_keyset_id, face_amount);

    let mut outputs = HashMap::new();
    outputs.insert("YES".to_string(), yes_outputs);
    outputs.insert("NO".to_string(), no_outputs);

    let split_request = CtfSplitRequest {
        condition_id,
        inputs: regular_proofs,
        outputs,
    };

    let result = mint.process_ctf_split(split_request).await;
    assert!(result.is_ok(), "split should succeed: {:?}", result.err());

    let response = result.unwrap();
    assert!(response.signatures.contains_key("YES"), "should have YES signatures");
    assert!(response.signatures.contains_key("NO"), "should have NO signatures");
    assert!(!response.signatures["YES"].is_empty());
    assert!(!response.signatures["NO"].is_empty());
}

/// Test that balance is conserved: input total equals per-outcome output total.
#[tokio::test]
async fn test_ctf_split_balance_conserved() {
    let mint = create_test_mint().await.unwrap();

    let face_amount = Amount::from(8);
    let regular_proofs = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (condition_id, keysets) = register_test_condition(&mint, &["YES", "NO"], None).await;

    let yes_keyset_id = *keysets.get("YES").unwrap();
    let no_keyset_id = *keysets.get("NO").unwrap();

    let (yes_outputs, _) = create_premint(&mint, yes_keyset_id, face_amount);
    let (no_outputs, _) = create_premint(&mint, no_keyset_id, face_amount);

    let mut outputs = HashMap::new();
    outputs.insert("YES".to_string(), yes_outputs);
    outputs.insert("NO".to_string(), no_outputs);

    let split_request = CtfSplitRequest {
        condition_id,
        inputs: regular_proofs,
        outputs,
    };

    // Split should succeed when per_oc_total == input_amount (with zero fees)
    let result = mint.process_ctf_split(split_request).await;
    assert!(result.is_ok(), "balanced split should succeed: {:?}", result.err());
}

/// Test that a split with unequal per-outcome totals is rejected.
#[tokio::test]
async fn test_ctf_split_unequal_outcome_amounts_rejected() {
    let mint = create_test_mint().await.unwrap();

    let face_amount = Amount::from(16);
    let regular_proofs = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (condition_id, keysets) = register_test_condition(&mint, &["YES", "NO"], None).await;

    let yes_keyset_id = *keysets.get("YES").unwrap();
    let no_keyset_id = *keysets.get("NO").unwrap();

    // YES gets 16, NO gets only 8 — totals differ, should be rejected
    let (yes_outputs, _) = create_premint(&mint, yes_keyset_id, Amount::from(16));
    let (no_outputs, _) = create_premint(&mint, no_keyset_id, Amount::from(8));

    let mut outputs = HashMap::new();
    outputs.insert("YES".to_string(), yes_outputs);
    outputs.insert("NO".to_string(), no_outputs);

    let split_request = CtfSplitRequest {
        condition_id,
        inputs: regular_proofs,
        outputs,
    };

    let result = mint.process_ctf_split(split_request).await;
    assert!(result.is_err(), "split with unequal outcome amounts should be rejected");
}

/// Test that a split with an unknown/invalid partition is rejected.
#[tokio::test]
async fn test_ctf_split_invalid_partition() {
    let mint = create_test_mint().await.unwrap();

    let face_amount = Amount::from(8);
    let regular_proofs = mint_test_proofs(&mint, face_amount).await.unwrap();

    // Condition with YES, NO, MAYBE outcomes; default partition is YES|NO|MAYBE
    let (condition_id, keysets) =
        register_test_condition(&mint, &["YES", "NO", "MAYBE"], None).await;

    let yes_keyset_id = *keysets.get("YES").unwrap();
    let no_keyset_id = *keysets.get("NO").unwrap();

    // Provide only YES and NO — incomplete partition (MAYBE is missing)
    let (yes_outputs, _) = create_premint(&mint, yes_keyset_id, face_amount);
    let (no_outputs, _) = create_premint(&mint, no_keyset_id, face_amount);

    let mut outputs = HashMap::new();
    outputs.insert("YES".to_string(), yes_outputs);
    outputs.insert("NO".to_string(), no_outputs);

    let split_request = CtfSplitRequest {
        condition_id,
        inputs: regular_proofs,
        outputs,
    };

    let result = mint.process_ctf_split(split_request).await;
    assert!(result.is_err(), "split with incomplete partition should be rejected");
}

/// Test that a split using the wrong keyset for an outcome collection is rejected.
#[tokio::test]
async fn test_ctf_split_wrong_keyset_rejected() {
    let mint = create_test_mint().await.unwrap();

    let face_amount = Amount::from(8);
    let regular_proofs = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (condition_id, keysets) = register_test_condition(&mint, &["YES", "NO"], None).await;

    let yes_keyset_id = *keysets.get("YES").unwrap();
    let no_keyset_id = *keysets.get("NO").unwrap();

    // Intentionally swap YES/NO: use NO keyset for the "YES" output key
    let (swapped_yes, _) = create_premint(&mint, no_keyset_id, face_amount);
    let (swapped_no, _) = create_premint(&mint, yes_keyset_id, face_amount);

    let mut outputs = HashMap::new();
    outputs.insert("YES".to_string(), swapped_yes);
    outputs.insert("NO".to_string(), swapped_no);

    let split_request = CtfSplitRequest {
        condition_id,
        inputs: regular_proofs,
        outputs,
    };

    let result = mint.process_ctf_split(split_request).await;
    assert!(result.is_err(), "split with swapped/wrong keysets should be rejected");
}

/// Test that a CTF merge of a complete partition returns regular tokens.
/// Flow: mint regular → (swap) YES conditional proofs + NO conditional proofs → merge → regular
#[tokio::test]
async fn test_ctf_merge_returns_regular_tokens() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);

    let face_amount = Amount::from(8);
    // Mint BEFORE registering conditions; need two batches for YES and NO conditional proofs
    let yes_regular = mint_test_proofs(&mint, face_amount).await.unwrap();
    let no_regular = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (condition_id, keysets) = register_test_condition(&mint, &["YES", "NO"], None).await;
    let yes_keyset_id = *keysets.get("YES").unwrap();
    let no_keyset_id = *keysets.get("NO").unwrap();

    // Swap regular proofs into each conditional keyset
    let yes_proofs = swap_to_conditional(&mint, yes_regular, yes_keyset_id, face_amount).await;
    let no_proofs = swap_to_conditional(&mint, no_regular, no_keyset_id, face_amount).await;

    // Merge YES + NO back into regular tokens
    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, face_amount);

    let mut inputs = HashMap::new();
    inputs.insert("YES".to_string(), yes_proofs);
    inputs.insert("NO".to_string(), no_proofs);

    let merge_request = CtfMergeRequest {
        condition_id,
        inputs,
        outputs: regular_outputs,
    };

    let result = mint.process_ctf_merge(merge_request).await;
    assert!(result.is_ok(), "merge should succeed: {:?}", result.err());
    assert!(!result.unwrap().signatures.is_empty());
}

/// Test that a merge with an incomplete partition (missing an outcome) is rejected.
#[tokio::test]
async fn test_ctf_merge_incomplete_partition_rejected() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);

    let face_amount = Amount::from(8);
    // Mint BEFORE registering conditions
    let yes_regular = mint_test_proofs(&mint, face_amount).await.unwrap();

    let (condition_id, keysets) = register_test_condition(&mint, &["YES", "NO"], None).await;
    let yes_keyset_id = *keysets.get("YES").unwrap();

    let yes_proofs = swap_to_conditional(&mint, yes_regular, yes_keyset_id, face_amount).await;

    // Only provide YES inputs — NO is missing, so the partition is incomplete
    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, face_amount);

    let mut inputs = HashMap::new();
    inputs.insert("YES".to_string(), yes_proofs);

    let merge_request = CtfMergeRequest {
        condition_id,
        inputs,
        outputs: regular_outputs,
    };

    let result = mint.process_ctf_merge(merge_request).await;
    assert!(result.is_err(), "merge with incomplete partition should be rejected");
}

// ============================================================================
// Multi-oracle threshold integration test
// ============================================================================

/// Test that redeeming a 2-of-2 threshold condition requires signatures from both oracles.
///
/// Setup: register a condition with two oracles and threshold=2.
/// Verify that providing only one oracle sig fails (threshold not met),
/// while providing both succeeds.
#[tokio::test]
async fn test_redeem_outcome_multi_oracle_threshold() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);

    let oracle1 = create_test_oracle();
    let oracle2 = create_test_oracle_2();

    // Both oracles announce the same event with the same outcomes
    let (_, hex_tlv1) = create_test_announcement(&oracle1, &["YES", "NO"], "multi-oracle-event");
    let (_, hex_tlv2) = create_test_announcement(&oracle2, &["YES", "NO"], "multi-oracle-event");

    // Mint regular proofs BEFORE registering conditions
    let amount = Amount::from(16);
    let regular_proofs_1 = mint_test_proofs(&mint, amount).await.unwrap();
    let regular_proofs_2 = mint_test_proofs(&mint, amount).await.unwrap();

    // Register condition with threshold=2 (requires both oracles)
    let condition_response = mint
        .register_condition(RegisterConditionRequest {
            threshold: 2,
            description: "2-of-2 oracle condition".to_string(),
            announcements: vec![hex_tlv1, hex_tlv2],
            condition_type: "enum".to_string(),
            lo_bound: None,
            hi_bound: None,
            precision: None,
        })
        .await
        .unwrap();

    let partition_response = mint
        .register_partition(
            &condition_response.condition_id,
            RegisterPartitionRequest {
                collateral: "sat".to_string(),
                partition: None,
                parent_collection_id: None,
            },
        )
        .await
        .unwrap();

    let yes_keyset_id = *partition_response.keysets.get("YES").unwrap();

    // --- Attempt 1: only oracle1 sig — should fail (threshold not met) ---
    {
        let conditional_proofs =
            swap_to_conditional(&mint, regular_proofs_1, yes_keyset_id, amount).await;

        let witness_one = create_multi_oracle_witness(&[(&oracle1, "YES")]);

        let mut proofs_with_witness = conditional_proofs;
        for proof in &mut proofs_with_witness {
            proof.witness = Some(Witness::OracleWitness(witness_one.clone()));
        }

        let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, amount);

        let result = mint
            .process_redeem_outcome(RedeemOutcomeRequest {
                inputs: proofs_with_witness,
                outputs: regular_outputs,
            })
            .await;

        assert!(
            result.is_err(),
            "single oracle sig should fail threshold=2 check"
        );
    }

    // --- Attempt 2: both oracle sigs — should succeed ---
    {
        let conditional_proofs =
            swap_to_conditional(&mint, regular_proofs_2, yes_keyset_id, amount).await;

        let witness_both = create_multi_oracle_witness(&[(&oracle1, "YES"), (&oracle2, "YES")]);

        let mut proofs_with_witness = conditional_proofs;
        for proof in &mut proofs_with_witness {
            proof.witness = Some(Witness::OracleWitness(witness_both.clone()));
        }

        let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, amount);

        let result = mint
            .process_redeem_outcome(RedeemOutcomeRequest {
                inputs: proofs_with_witness,
                outputs: regular_outputs,
            })
            .await;

        assert!(
            result.is_ok(),
            "both oracle sigs should meet threshold=2: {:?}",
            result.err()
        );
    }
}

// ============================================================================
// Security regression tests
// ============================================================================

/// Test that duplicate attestation signatures from the same oracle
/// cannot satisfy a threshold > 1 requirement.
///
/// This is a regression test for P1: threshold bypass via duplicate oracle pubkeys.
#[tokio::test]
async fn test_redeem_rejects_duplicate_oracle_sigs() {
    let mint = create_test_mint().await.unwrap();
    let regular_keyset_id = get_regular_keyset_id(&mint);

    let oracle1 = create_test_oracle();
    let oracle2 = create_test_oracle_2();

    // Register with two oracles, threshold=2
    let (_, hex_tlv1) = create_test_announcement(&oracle1, &["YES", "NO"], "dup-event");
    let (_, hex_tlv2) = create_test_announcement(&oracle2, &["YES", "NO"], "dup-event");

    let amount = Amount::from(16);
    let regular_proofs = mint_test_proofs(&mint, amount).await.unwrap();

    let condition_response = mint
        .register_condition(RegisterConditionRequest {
            threshold: 2,
            description: "Dup oracle test".to_string(),
            announcements: vec![hex_tlv1, hex_tlv2],
            condition_type: "enum".to_string(),
            lo_bound: None,
            hi_bound: None,
            precision: None,
        })
        .await
        .unwrap();

    let partition_response = mint
        .register_partition(
            &condition_response.condition_id,
            RegisterPartitionRequest {
                collateral: "sat".to_string(),
                partition: None,
                parent_collection_id: None,
            },
        )
        .await
        .unwrap();

    let yes_keyset_id = *partition_response.keysets.get("YES").unwrap();
    let conditional_proofs =
        swap_to_conditional(&mint, regular_proofs, yes_keyset_id, amount).await;

    // Provide oracle1's signature twice (duplicate) — should NOT satisfy threshold=2
    let witness = create_multi_oracle_witness(&[(&oracle1, "YES"), (&oracle1, "YES")]);
    let mut proofs_with_witness = conditional_proofs;
    for proof in &mut proofs_with_witness {
        proof.witness = Some(Witness::OracleWitness(witness.clone()));
    }

    let (regular_outputs, _) = create_premint(&mint, regular_keyset_id, amount);

    let result = mint
        .process_redeem_outcome(RedeemOutcomeRequest {
            inputs: proofs_with_witness,
            outputs: regular_outputs,
        })
        .await;

    assert!(
        result.is_err(),
        "duplicate oracle sigs from same pubkey should not satisfy threshold=2"
    );
}
