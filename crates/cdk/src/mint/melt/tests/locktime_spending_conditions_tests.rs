//! Locktime tests for melt functionality
//!
//! These tests verify that the mint correctly validates locktime spending conditions
//! during melt operations, including spending after locktime expiry.

use std::str::FromStr;

use cdk_common::dhke::construct_proofs;
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::nuts::{Conditions, SigFlag, SpendingConditions};
use cdk_common::{Amount, SpendingConditionVerification};

use crate::test_helpers::nut10::{create_test_keypair, unzip3, TestMintHelper};
use crate::util::unix_time;

/// Test: P2PK with locktime - spending after expiry
///
/// Creates P2PK proofs with locktime and verifies:
/// 1. Melting before locktime with wrong key fails
/// 2. Melting after locktime with any key succeeds (anyone-can-spend)
#[tokio::test]
async fn test_p2pk_post_locktime_anyone_can_spend() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Generate keypairs
    let (_alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, _bob_pubkey) = create_test_keypair();

    println!("Alice pubkey: {}", alice_pubkey);

    // Step 1: Create regular unencumbered proofs
    let input_amount = Amount::from(20);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create P2PK spending conditions with locktime in the past (already expired)
    // Locktime is 1 hour ago - so it's already expired
    let locktime = unix_time() - 3600;

    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: Some(locktime),     // Locktime in the past (expired)
            pubkeys: None,                // no additional pubkeys
            refund_keys: None,            // NO refund keys - anyone can spend!
            num_sigs: None,               // default (1)
            sig_flag: SigFlag::SigInputs, // SIG_INPUTS flag
            num_sigs_refund: None,        // default (1)
        }),
    );
    println!(
        "Created P2PK spending conditions with expired locktime: {}",
        locktime
    );

    // Split the input amount
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let split_display: Vec<String> = split_amounts.iter().map(|a| a.to_string()).collect();
    println!("Split {} into [{}]", input_amount, split_display.join("+"));

    // Create blinded messages
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );
    println!("Created {} P2PK outputs with locktime", p2pk_outputs.len());

    // Step 3: Swap for P2PK proofs
    let swap_request = cdk_common::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint
        .process_swap_request(swap_request)
        .await
        .expect("Failed to swap for P2PK proofs");
    println!("Swap successful! Got BlindSignatures");

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

    // Step 5: Create a real melt quote
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

    // Step 6: Try to melt with Bob's signature (wrong key, but locktime expired so should work)
    let mut proofs_bob_signed = p2pk_proofs.clone();

    // Sign with Bob's key (not Alice's)
    for proof in proofs_bob_signed.iter_mut() {
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let melt_request_bob =
        cdk_common::MeltRequest::new(melt_quote.quote.clone(), proofs_bob_signed.into(), None);

    // After locktime expiry, anyone can spend (signature verification is skipped)
    melt_request_bob.verify_spending_conditions().unwrap();
    println!("✓ Post-locktime spending conditions verified successfully (anyone-can-spend)");

    // Perform the actual melt
    let melt_response = mint.melt(&melt_request_bob).await.unwrap();
    println!("✓ Melt operation completed successfully with Bob's key after locktime!");
    println!("  Quote state: {:?}", melt_response.state);
    assert_eq!(melt_response.quote, melt_quote.quote);
}

/// Test: P2PK with future locktime - must use correct key before expiry
///
/// Creates P2PK proofs with future locktime and verifies:
/// 1. Melting with wrong key before locktime fails
/// 2. Melting with correct key before locktime succeeds
#[tokio::test]
async fn test_p2pk_before_locktime_requires_correct_key() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Generate keypairs
    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, _bob_pubkey) = create_test_keypair();

    println!("Alice pubkey: {}", alice_pubkey);

    // Step 1: Create regular unencumbered proofs
    let input_amount = Amount::from(20);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create P2PK spending conditions with locktime FAR in the future
    // Locktime is 1 year from now - definitely not expired yet
    let locktime = unix_time() + 365 * 24 * 60 * 60;

    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(
            Conditions::new(
                Some(locktime),           // Locktime in the future
                None,                     // no additional pubkeys
                None,                     // no refund keys
                None,                     // default num_sigs (1)
                Some(SigFlag::SigInputs), // SIG_INPUTS flag
                None,                     // no num_sigs_refund
            )
            .unwrap(),
        ),
    );
    println!(
        "Created P2PK spending conditions with future locktime: {}",
        locktime
    );

    // Split the input amount
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let split_display: Vec<String> = split_amounts.iter().map(|a| a.to_string()).collect();
    println!("Split {} into [{}]", input_amount, split_display.join("+"));

    // Create blinded messages
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );
    println!("Created {} P2PK outputs with locktime", p2pk_outputs.len());

    // Step 3: Swap for P2PK proofs
    let swap_request = cdk_common::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint
        .process_swap_request(swap_request)
        .await
        .expect("Failed to swap for P2PK proofs");
    println!("Swap successful! Got BlindSignatures");

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

    // Step 5: Create a real melt quote
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

    // Step 6: Try to melt with Bob's signature (wrong key, locktime not expired)
    let mut proofs_bob_signed = p2pk_proofs.clone();

    // Sign with Bob's key (not Alice's)
    for proof in proofs_bob_signed.iter_mut() {
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let melt_request_bob =
        cdk_common::MeltRequest::new(melt_quote.quote.clone(), proofs_bob_signed.into(), None);

    // Before locktime expiry, wrong key should fail
    let result = melt_request_bob.verify_spending_conditions();
    assert!(
        result.is_err(),
        "Should fail with wrong key before locktime"
    );
    println!("✓ Melting with Bob's key before locktime failed verification as expected");

    // Also verify the actual melt fails
    let melt_result = mint.melt(&melt_request_bob).await;
    assert!(
        melt_result.is_err(),
        "Actual melt should also fail with wrong key"
    );
    println!("✓ Actual melt with Bob's key before locktime also failed as expected");

    // Step 7: Now melt with Alice's signature (correct key)
    let mut proofs_alice_signed = p2pk_proofs.clone();

    // Sign with Alice's key (correct)
    for proof in proofs_alice_signed.iter_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let melt_request_alice =
        cdk_common::MeltRequest::new(melt_quote.quote.clone(), proofs_alice_signed.into(), None);

    // Verify spending conditions pass
    melt_request_alice.verify_spending_conditions().unwrap();
    println!("✓ Pre-locktime spending conditions verified successfully with Alice's key");

    // Perform the actual melt
    let melt_response = mint.melt(&melt_request_alice).await.unwrap();
    println!("✓ Melt operation completed successfully with Alice's key before locktime!");
    println!("  Quote state: {:?}", melt_response.state);
    assert_eq!(melt_response.quote, melt_quote.quote);
}
