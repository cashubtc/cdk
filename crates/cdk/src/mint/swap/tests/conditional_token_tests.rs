//! NUT-28 conditional token tests for swap and redeem operations

use std::collections::HashMap;

use cdk_common::amount::SplitTarget;
use cdk_common::dhke::construct_proofs;
use cdk_common::nuts::nut28::test_helpers::{
    create_oracle_witness, create_test_announcement, create_test_oracle,
};
use cdk_common::nuts::nut28::{
    RedeemOutcomeRequest, RegisterConditionRequest, RegisterPartitionRequest,
};
use cdk_common::nuts::{Id, PreMintSecrets, SwapRequest, Witness};
use cdk_common::{Amount, CurrencyUnit};

use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};

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

    // Step 1: Register the condition
    let request = RegisterConditionRequest {
        threshold: 1,
        description: "Test condition".to_string(),
        announcements: vec![hex_tlv],
    };

    let condition_response = mint.register_condition(request).await.unwrap();
    let condition_id = condition_response.condition_id;

    // Step 2: Register the partition
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

    let request = RegisterConditionRequest {
        threshold: 1,
        description: "Test condition".to_string(),
        announcements: vec![hex_tlv],
    };

    let response1 = mint.register_condition(request.clone()).await.unwrap();
    let response2 = mint.register_condition(request).await.unwrap();

    assert_eq!(response1.condition_id, response2.condition_id);
}

/// Test get_conditions returns registered conditions
#[tokio::test]
async fn test_get_conditions_returns_registered() {
    let mint = create_test_mint().await.unwrap();
    let (condition_id, _) = register_test_condition(&mint, &["YES", "NO"], None).await;

    let response = mint.get_conditions().await.unwrap();
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
        .register_condition(RegisterConditionRequest {
            threshold: 1,
            description: "Test redeem".to_string(),
            announcements: vec![hex_tlv],
        })
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
        .register_condition(RegisterConditionRequest {
            threshold: 1,
            description: "Test wrong outcome".to_string(),
            announcements: vec![hex_tlv],
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
        .register_condition(RegisterConditionRequest {
            threshold: 1,
            description: "No witness test".to_string(),
            announcements: vec![hex_tlv],
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
        .register_condition(RegisterConditionRequest {
            threshold: 1,
            description: "Outputs conditional test".to_string(),
            announcements: vec![hex_tlv],
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
        .register_condition(RegisterConditionRequest {
            threshold: 1,
            description: "Swap reject test".to_string(),
            announcements: vec![hex_tlv],
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
        .register_condition(RegisterConditionRequest {
            threshold: 1,
            description: "Stored attestation test".to_string(),
            announcements: vec![hex_tlv],
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
        .register_condition(RegisterConditionRequest {
            threshold: 1,
            description: "Partition test".to_string(),
            announcements: vec![hex_tlv],
        })
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
