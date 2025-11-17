//! P2PK SIG_ALL tests for melt functionality
//!
//! These tests verify that the mint correctly enforces SIG_ALL flag behavior
//! during melt operations.

use cdk_common::dhke::construct_proofs;
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::nuts::{Conditions, SigFlag, SpendingConditions};
use cdk_common::Amount;

use crate::test_helpers::nut10::{create_test_keypair, unzip3, TestMintHelper};

/// Test: P2PK with SIG_ALL flag requires transaction signature
///
/// Creates P2PK proofs with SIG_ALL flag and verifies:
/// 1. Melting without signature is rejected
/// 2. Melting with SIG_INPUTS signatures (individual proof signatures) is rejected
/// 3. Melting with SIG_ALL signature (transaction signature) succeeds
#[tokio::test]
async fn test_p2pk_sig_all_requires_transaction_signature() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Generate keypair for P2PK
    let (alice_secret, alice_pubkey) = create_test_keypair();
    println!("Alice pubkey: {}", alice_pubkey);

    // Step 1: Create regular unencumbered proofs that we'll swap for P2PK proofs
    // Invoice is 10 sats, fee reserve is 100% (10 sats), so we need 20 sats total
    let input_amount = Amount::from(20);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create P2PK blinded messages (outputs locked to alice_pubkey) with SIG_ALL
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(
            Conditions::new(
                None,                  // no locktime
                None,                  // no additional pubkeys
                None,                  // no refund keys
                None,                  // default num_sigs (1)
                Some(SigFlag::SigAll), // SIG_ALL flag
                None,                  // no num_sigs_refund
            )
            .unwrap(),
        ),
    );
    println!("Created P2PK spending conditions with SIG_ALL flag");

    // Split the input amount into power-of-2 denominations
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let split_display: Vec<String> = split_amounts.iter().map(|a| a.to_string()).collect();
    println!("Split {} into [{}]", input_amount, split_display.join("+"));

    // Create blinded messages for each split amount
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    println!(
        "Created {} P2PK outputs locked to alice",
        p2pk_outputs.len()
    );

    // Step 3: Swap regular proofs for P2PK proofs (no signature needed on inputs)
    let swap_request = cdk_common::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint
        .process_swap_request(swap_request)
        .await
        .expect("Failed to swap for P2PK proofs");
    println!("Swap successful! Got BlindSignatures for our P2PK outputs");

    // Step 4: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    let proof_amounts: Vec<String> = p2pk_proofs.iter().map(|p| p.amount.to_string()).collect();
    println!(
        "Constructed {} P2PK proof(s) [{}]",
        p2pk_proofs.len(),
        proof_amounts.join("+")
    );

    // Step 5: Create a real melt quote that we'll use for all tests
    use std::str::FromStr;

    use cdk_common::SpendingConditionVerification;
    let bolt11_str = "lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq";
    let bolt11 = cdk_common::Bolt11Invoice::from_str(bolt11_str).unwrap();

    let melt_quote_request = cdk_common::MeltQuoteBolt11Request {
        request: bolt11,
        unit: cdk_common::CurrencyUnit::Sat,
        options: None,
    };

    let melt_quote = mint
        .get_melt_quote(MeltQuoteRequest::Bolt11(melt_quote_request))
        .await
        .unwrap();
    println!("Created melt quote: {}", melt_quote.quote);

    // Step 6: Try to melt P2PK proof WITHOUT signature (should fail)

    let melt_request_no_sig =
        cdk_common::MeltRequest::new(melt_quote.quote.clone(), p2pk_proofs.clone().into(), None);

    let result = melt_request_no_sig.verify_spending_conditions();
    assert!(result.is_err(), "Should fail without signature");
    println!("✓ Melting WITHOUT signature failed verification as expected");

    // Also verify the actual melt fails
    let melt_result = mint.melt(&melt_request_no_sig).await;
    assert!(
        melt_result.is_err(),
        "Actual melt should also fail without signature"
    );
    println!("✓ Actual melt WITHOUT signature also failed as expected");

    // Step 7: Sign all proofs individually (SIG_INPUTS way) - should fail for SIG_ALL
    let mut melt_request_sig_inputs =
        cdk_common::MeltRequest::new(melt_quote.quote.clone(), p2pk_proofs.clone().into(), None);

    // Sign each proof individually (SIG_INPUTS mode)
    for proof in melt_request_sig_inputs.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = melt_request_sig_inputs.verify_spending_conditions();
    assert!(
        result.is_err(),
        "Should fail - SIG_INPUTS signatures not valid for SIG_ALL"
    );
    println!("✓ Melting with SIG_INPUTS signatures failed verification as expected");

    // Also verify the actual melt fails
    let melt_result = mint.melt(&melt_request_sig_inputs).await;
    assert!(
        melt_result.is_err(),
        "Actual melt should also fail with SIG_INPUTS signatures"
    );
    println!("✓ Actual melt with SIG_INPUTS signatures also failed as expected");

    // Step 8: Sign the transaction with SIG_ALL and perform the melt
    let mut melt_request =
        cdk_common::MeltRequest::new(melt_quote.quote.clone(), p2pk_proofs.clone().into(), None);

    // Use sign_sig_all to sign the transaction (signature goes on first proof's witness)
    melt_request.sign_sig_all(alice_secret.clone()).unwrap();

    // Verify spending conditions pass
    melt_request.verify_spending_conditions().unwrap();
    println!("✓ P2PK SIG_ALL spending conditions verified successfully");

    // Perform the actual melt - this also verifies spending conditions internally
    let melt_response = mint.melt(&melt_request).await.unwrap();
    println!("✓ Melt operation completed successfully!");
    println!("  Quote state: {:?}", melt_response.state);
    assert_eq!(melt_response.quote, melt_quote.quote);
}
