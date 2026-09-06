#![allow(missing_docs)]

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::mint::MintRequest;
use cdk::wallet::receive::ReceiveRequest;
use cdk::wallet::send::{SendRequest, SendStatus};
use cdk::wallet::{WalletIdentity, WalletManagerBuilder};
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
    let mint_url = MintUrl::from_str("https://testnut.cashudevkit.org")?;
    let unit = CurrencyUnit::Sat;

    // Generate a seed
    let mnemonic = Mnemonic::generate(12)?;
    let seed = mnemonic.to_seed_normalized("");
    println!("Generated mnemonic: {}", mnemonic);

    // Create the WalletManager
    let localstore = Arc::new(memory::empty().await?);
    let manager = WalletManagerBuilder::new()
        .with_store(localstore)
        .with_seed(seed)
        .build()
        .await?;
    println!("Created WalletManager");

    let identity = WalletIdentity {
        mint_url: mint_url.clone(),
        unit: unit.clone(),
    };
    let mint_wallet = manager.wallet(identity.clone()).await?;
    println!("Added mint: {}", mint_url);

    // ========================================
    // 1. FUND: Mint some tokens to start
    // ========================================
    let mint_amount = Amount::from(100);
    println!("\n--- 1. FUNDING WALLET ---");
    println!("Minting {} sats...", mint_amount);

    let mint_session = mint_wallet
        .request_mint(MintRequest::bolt11(mint_amount))
        .await?;

    // Wait for quote to be paid (automatic with test mint)
    mint_session.wait(Duration::from_secs(60)).await?;

    let balance = mint_wallet.balance().await?.available;
    println!("Wallet funded. Balance: {} sats", balance);

    // ========================================
    // 2. SEND: Create a token
    // ========================================
    let send_amount = Amount::from(50);
    println!("\n--- 2. CREATING SEND ---");
    println!("Preparing to send {} sats...", send_amount);

    let send_plan = mint_wallet.plan_send(SendRequest::new(send_amount)).await?;
    let operation_id = send_plan.operation_id();
    let receipt = send_plan.execute().await?;

    println!("Token created (Send Operation ID: {})", operation_id);
    println!("Token: {}", receipt.token);

    let balance_after_send = mint_wallet.balance().await?.available;
    println!("Balance after send: {} sats", balance_after_send);

    // ========================================
    // 3. INSPECT: Check pending status
    // ========================================
    println!("\n--- 3. INSPECTING STATUS ---");

    // Get all pending sends
    let pending_sends = mint_wallet.pending_send_ids().await?;
    println!("Pending sends count: {}", pending_sends.len());

    for id in &pending_sends {
        println!("- ID: {}", id);
    }

    // Check specific status
    let status = mint_wallet.send_status(operation_id).await?;
    println!("Send status: {}", status);

    if status == SendStatus::Unclaimed {
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

    let reclaimed_amount = mint_wallet.reclaim_send(operation_id).await?;
    println!("Reclaimed {} sats", reclaimed_amount);

    // ========================================
    // 5. VERIFY: Check final state
    // ========================================
    println!("\n--- 5. VERIFYING STATE ---");

    // Check pending sends again
    let pending_after = mint_wallet.pending_send_ids().await?;
    println!("Pending sends after revocation: {}", pending_after.len());

    // Check final balance
    let final_balance = mint_wallet.balance().await?.available;
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
    let send_plan_2 = mint_wallet
        .plan_send(SendRequest::new(send_amount_2))
        .await?;
    let operation_id_2 = send_plan_2.operation_id();
    let receipt_2 = send_plan_2.execute().await?;
    println!("Token created: {}", receipt_2.token);

    // Create a receiver wallet
    println!("Creating receiver wallet...");
    let receiver_seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let receiver_store = Arc::new(memory::empty().await?);
    let receiver_manager = WalletManagerBuilder::new()
        .with_store(receiver_store)
        .with_seed(receiver_seed)
        .build()
        .await?;
    let receiver_mint_wallet = receiver_manager.wallet(identity).await?;

    // Receiver claims the token
    println!("Receiver claiming token...");
    let received_amount = receiver_mint_wallet
        .receive(ReceiveRequest::new(receipt_2.token.to_string()))
        .await?
        .amount;
    println!("Receiver got {} sats", received_amount);

    // Check status from sender side
    println!("Checking status from sender...");
    let status_2 = mint_wallet.send_status(operation_id_2).await?;
    println!("Send status: {}", status_2);

    if status_2 == SendStatus::Claimed {
        println!("Token confirmed as claimed.");
    } else {
        println!("WARNING: Token should be claimed but status says false.");
    }

    // Verify pending sends is empty
    let pending_final = mint_wallet.pending_send_ids().await?;
    println!("Pending sends count: {}", pending_final.len());

    if pending_final.is_empty() {
        println!("SUCCESS: Saga finalized and removed from pending.");
    } else {
        println!("WARNING: Pending sends not empty.");
    }

    Ok(())
}
