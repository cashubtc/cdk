//! HTLC SIG_ALL tests for melt functionality
//!
//! These tests verify that the mint correctly enforces SIG_ALL flag behavior for HTLC
//! during melt operations.

use std::str::FromStr;

use cdk_common::dhke::construct_proofs;
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::nuts::{Conditions, SigFlag, SpendingConditions};
use cdk_common::{Amount, SpendingConditionVerification};

use crate::test_helpers::nut10::{
    create_test_hash_and_preimage, create_test_keypair, unzip3, TestMintHelper,
};

/// Test: HTLC SIG_ALL requiring preimage and one signature
///
/// Creates HTLC-locked proofs with SIG_ALL flag and verifies:
/// 1. Melting with only preimage fails (signature required)
/// 2. Melting with only SIG_INPUTS signatures fails (SIG_ALL required)
/// 3. Melting with both preimage and SIG_ALL signature succeeds
#[tokio::test]
async fn test_htlc_sig_all_requiring_preimage_and_one_signature() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Generate keypair for Alice
    let (alice_secret, alice_pubkey) = create_test_keypair();

    // Create hash and preimage
    let (hash, preimage) = create_test_hash_and_preimage();

    println!("Alice pubkey: {}", alice_pubkey);
    println!("Hash: {}", hash);
    println!("Preimage: {}", preimage);

    // Step 1: Mint regular proofs (enough to cover invoice + fees)
    let input_amount = Amount::from(20);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create HTLC spending conditions with SIG_ALL flag (hash locked to Alice's key)
    let spending_conditions = SpendingConditions::new_htlc_hash(
        &hash,
        Some(Conditions {
            locktime: None,
            pubkeys: Some(vec![alice_pubkey]),
            refund_keys: None,
            num_sigs: None,            // Default (1)
            sig_flag: SigFlag::SigAll, // <-- SIG_ALL flag
            num_sigs_refund: None,
        }),
    )
    .unwrap();
    println!("Created HTLC spending conditions with SIG_ALL flag");

    // Step 3: Create HTLC blinded messages (outputs)
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let split_display: Vec<String> = split_amounts.iter().map(|a| a.to_string()).collect();
    println!("Split {} into [{}]", input_amount, split_display.join("+"));

    let (htlc_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );
    println!(
        "Created {} HTLC outputs locked to alice with hash",
        htlc_outputs.len()
    );

    // Step 4: Swap regular proofs for HTLC proofs (no signature needed on inputs)
    let swap_request = cdk_common::SwapRequest::new(input_proofs.clone(), htlc_outputs.clone());
    let swap_response = mint
        .process_swap_request(swap_request)
        .await
        .expect("Failed to swap for HTLC proofs");
    println!("Swap successful! Got BlindSignatures for our HTLC outputs");

    // Step 5: Construct the HTLC proofs
    let htlc_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    let proof_amounts: Vec<String> = htlc_proofs.iter().map(|p| p.amount.to_string()).collect();
    println!(
        "Constructed {} HTLC proof(s) [{}]",
        htlc_proofs.len(),
        proof_amounts.join("+")
    );

    // Step 6: Create a real melt quote that we'll use for all tests
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

    // Step 7: Try to melt with only preimage (should fail - signature required)
    let mut proofs_preimage_only = htlc_proofs.clone();
    // Add only preimage to first proof (no signature)
    proofs_preimage_only[0].add_preimage(preimage.clone());

    let melt_request_preimage_only =
        cdk_common::MeltRequest::new(melt_quote.quote.clone(), proofs_preimage_only.into(), None);

    let result = melt_request_preimage_only.verify_spending_conditions();
    assert!(
        result.is_err(),
        "Should fail with only preimage (no signature)"
    );
    println!("✓ Melting with ONLY preimage failed verification as expected");

    let melt_result = mint.melt(&melt_request_preimage_only).await;
    assert!(
        melt_result.is_err(),
        "Actual melt should also fail with only preimage"
    );
    println!("✓ Actual melt with ONLY preimage also failed as expected");

    // Step 8: Try to melt with SIG_INPUTS signatures (should fail - SIG_ALL required)
    let mut melt_request_sig_inputs =
        cdk_common::MeltRequest::new(melt_quote.quote.clone(), htlc_proofs.clone().into(), None);

    // Add preimage to first proof
    melt_request_sig_inputs.inputs_mut()[0].add_preimage(preimage.clone());

    // Sign each proof individually (SIG_INPUTS mode) - this should fail for SIG_ALL
    for proof in melt_request_sig_inputs.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = melt_request_sig_inputs.verify_spending_conditions();
    assert!(
        result.is_err(),
        "Should fail - SIG_INPUTS signatures not valid for SIG_ALL"
    );
    println!("✓ Melting with SIG_INPUTS signatures failed verification as expected");

    let melt_result = mint.melt(&melt_request_sig_inputs).await;
    assert!(
        melt_result.is_err(),
        "Actual melt should also fail with SIG_INPUTS signatures"
    );
    println!("✓ Actual melt with SIG_INPUTS signatures also failed as expected");

    // Step 9: Now melt with correct preimage + SIG_ALL signature
    let mut melt_request =
        cdk_common::MeltRequest::new(melt_quote.quote.clone(), htlc_proofs.clone().into(), None);

    // Add preimage to first proof
    melt_request.inputs_mut()[0].add_preimage(preimage.clone());

    // Use sign_sig_all to sign the transaction (signature goes on first proof's witness)
    melt_request.sign_sig_all(alice_secret.clone()).unwrap();

    // Verify spending conditions pass
    melt_request.verify_spending_conditions().unwrap();
    println!("✓ HTLC SIG_ALL spending conditions verified successfully");

    // Perform the actual melt - this also verifies spending conditions internally
    let melt_response = mint.melt(&melt_request).await.unwrap();
    println!("✓ Melt operation completed successfully!");
    println!("  Quote state: {:?}", melt_response.state);
    assert_eq!(melt_response.quote, melt_quote.quote);
}
