#![allow(missing_docs)]

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::wallet::SendOptions;
use cdk::Amount;
use cdk_sqlite::wallet::memory;

/// This example demonstrates the ability to revoke a send operation.
///
/// It shows:
/// - Funding a wallet
/// - Creating a send (generating a token)
/// - Viewing pending sends
/// - Checking send status
/// - Revoking the send (reclaiming funds)
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configuration
    let mint_url = MintUrl::from_str("https://fake.thesimplekid.dev")?;
    let unit = CurrencyUnit::Sat;

    // Generate a seed
    let mnemonic = Mnemonic::generate(12)?;
    let seed = mnemonic.to_seed_normalized("");
    println!("Generated mnemonic: {}", mnemonic);

    // Create the MultiMintWallet
    let localstore = Arc::new(memory::empty().await?);
    let wallet = MultiMintWallet::new(localstore, seed, unit.clone()).await?;
    println!("Created MultiMintWallet");

    // Add a mint to the wallet
    wallet.add_mint(mint_url.clone()).await?;
    println!("Added mint: {}", mint_url);

    // ========================================
    // 1. FUND: Mint some tokens to start
    // ========================================
    let mint_amount = Amount::from(100);
    println!("\n--- 1. FUNDING WALLET ---");
    println!("Minting {} sats...", mint_amount);

    let mint_quote = wallet.mint_quote(&mint_url, mint_amount, None).await?;

    // Wait for quote to be paid (automatic with fake mint)
    let _proofs = wallet
        .wait_for_mint_quote(
            &mint_url,
            &mint_quote.id,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await?;

    let balance = wallet.total_balance().await?;
    println!("Wallet funded. Balance: {} sats", balance);

    // ========================================
    // 2. SEND: Create a token
    // ========================================
    let send_amount = Amount::from(50);
    println!("\n--- 2. CREATING SEND ---");
    println!("Preparing to send {} sats...", send_amount);

    // Prepare and confirm the send
    let prepared_send = wallet
        .prepare_send(mint_url.clone(), send_amount, SendOptions::default())
        .await?;

    let operation_id = prepared_send.operation_id();
    let token = prepared_send.confirm(None).await?;

    println!("Token created (Send Operation ID: {})", operation_id);
    println!("Token: {}", token);

    let balance_after_send = wallet.total_balance().await?;
    println!("Balance after send: {} sats", balance_after_send);

    // ========================================
    // 3. INSPECT: Check pending status
    // ========================================
    println!("\n--- 3. INSPECTING STATUS ---");

    // Get all pending sends
    let pending_sends = wallet.get_pending_sends().await?;
    println!("Pending sends count: {}", pending_sends.len());

    for (mint, id) in &pending_sends {
        println!("- Mint: {}, ID: {}", mint, id);
    }

    // Check specific status
    let claimed = wallet
        .check_send_status(mint_url.clone(), operation_id)
        .await?;
    println!("Is token claimed? {}", claimed);

    if !claimed {
        println!("Token is unclaimed. Revocation possible.");
    } else {
        println!("Token already claimed. Cannot revoke.");
        return Ok(());
    }

    // ========================================
    // 4. REVOKE: Reclaim the funds
    // ========================================
    println!("\n--- 4. REVOKING SEND ---");
    println!("Revoking operation {}...", operation_id);

    let reclaimed_amount = wallet.revoke_send(mint_url.clone(), operation_id).await?;
    println!("Reclaimed {} sats", reclaimed_amount);

    // ========================================
    // 5. VERIFY: Check final state
    // ========================================
    println!("\n--- 5. VERIFYING STATE ---");

    // Check pending sends again
    let pending_after = wallet.get_pending_sends().await?;
    println!("Pending sends after revocation: {}", pending_after.len());

    // Check final balance
    let final_balance = wallet.total_balance().await?;
    println!("Final balance: {} sats", final_balance);

    if final_balance > balance_after_send {
        println!("SUCCESS: Funds restored!");
    } else {
        println!("WARNING: Balance did not increase.");
    }

    // Note on fees
    if final_balance < mint_amount {
        println!("(Note: Final balance may be slightly less than original due to mint fees)");
    }

    // ========================================
    // 6. FINALIZE: Send and Claim (Happy Path)
    // ========================================
    println!("\n--- 6. SEND AND FINALIZE (Happy Path) ---");
    let send_amount_2 = Amount::from(20);
    println!("Sending {} sats to be claimed...", send_amount_2);

    // Create a new send
    let prepared_send_2 = wallet
        .prepare_send(mint_url.clone(), send_amount_2, SendOptions::default())
        .await?;
    let operation_id_2 = prepared_send_2.operation_id();
    let token_2 = prepared_send_2.confirm(None).await?;
    println!("Token created: {}", token_2);

    // Create a receiver wallet
    println!("Creating receiver wallet...");
    let receiver_seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let receiver_store = Arc::new(memory::empty().await?);
    let receiver_wallet = MultiMintWallet::new(receiver_store, receiver_seed, unit).await?;
    receiver_wallet.add_mint(mint_url.clone()).await?;

    // Receiver claims the token
    println!("Receiver claiming token...");
    let received_amount = receiver_wallet
        .receive(
            &token_2.to_string(),
            cdk::wallet::MultiMintReceiveOptions::default(),
        )
        .await?;
    println!("Receiver got {} sats", received_amount);

    // Check status from sender side
    println!("Checking status from sender...");
    let claimed_2 = wallet
        .check_send_status(mint_url.clone(), operation_id_2)
        .await?;
    println!("Is token claimed? {}", claimed_2);

    if claimed_2 {
        println!("Token confirmed as claimed.");
    } else {
        println!("WARNING: Token should be claimed but status says false.");
    }

    // Verify pending sends is empty
    let pending_final = wallet.get_pending_sends().await?;
    println!("Pending sends count: {}", pending_final.len());

    if pending_final.is_empty() {
        println!("SUCCESS: Saga finalized and removed from pending.");
    } else {
        println!("WARNING: Pending sends not empty.");
    }

    Ok(())
}
