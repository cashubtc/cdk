use std::time::Duration;

use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::nuts::{PaymentMethod, ProofsMethods};
use cdk_integration_tests::init_pure_tests::{
    create_mint_with_onchain, create_test_wallet_for_mint, setup_tracing,
};
use cdk_integration_tests::init_regtest::{
    init_bitcoin_client, BITCOIND_ADDR, BITCOIN_RPC_PASS, BITCOIN_RPC_USER,
};
use tokio::time::sleep;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_minting() {
    setup_tracing();

    if !cdk_integration_tests::is_regtest_env() {
        return;
    }

    let mnemonic = Mnemonic::generate(12).unwrap();
    let rpc_host = BITCOIND_ADDR.split(':').next().unwrap().to_string();
    let rpc_port = BITCOIND_ADDR
        .split(':')
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();

    let mint = create_mint_with_onchain(
        mnemonic.clone(),
        rpc_host,
        rpc_port,
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
    )
    .await
    .expect("Failed to create mint");

    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create wallet");

    // Get a mint quote
    let mint_amount = 100_000;
    let quote = wallet
        .mint_quote(
            PaymentMethod::Known(cdk::nuts::nut00::KnownMethod::Onchain),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get mint quote");

    let address = quote.request.clone();
    println!("Mint address: {}", address);

    // Fund the address using bitcoind
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");
    bitcoin_client
        .send_to_address(&address, mint_amount)
        .expect("Failed to send BTC");

    // Mine blocks to confirm the transaction
    let mine_addr = bitcoin_client
        .get_new_address()
        .expect("Failed to get address");
    bitcoin_client
        .generate_blocks(&mine_addr, 3)
        .expect("Failed to mine blocks");

    // Wait for the mint to see the payment
    println!("Waiting for mint to detect payment...");

    // We use wait_and_mint_quote which handles the polling/waiting
    let proofs = wallet
        .wait_and_mint_quote(
            quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(120),
        )
        .await
        .expect("Failed to mint proofs");

    assert_eq!(proofs.total_amount().unwrap(), mint_amount.into());
    assert_eq!(wallet.total_balance().await.unwrap(), mint_amount.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_melt() {
    setup_tracing();

    if !cdk_integration_tests::is_regtest_env() {
        return;
    }

    let mnemonic = Mnemonic::generate(12).unwrap();
    let rpc_host = BITCOIND_ADDR.split(':').next().unwrap().to_string();
    let rpc_port = BITCOIND_ADDR
        .split(':')
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();

    let mint = create_mint_with_onchain(
        mnemonic.clone(),
        rpc_host,
        rpc_port,
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
    )
    .await
    .expect("Failed to create mint");

    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create wallet");

    // Fund wallet first using fake lightning (since it's easier for setup)
    // Actually, let's just fund it via onchain minting first to be sure
    let mint_amount = 200_000;
    let quote = wallet
        .mint_quote(
            PaymentMethod::Known(cdk::nuts::nut00::KnownMethod::Onchain),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get mint quote");

    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");
    bitcoin_client
        .send_to_address(&quote.request, mint_amount)
        .expect("Failed to send BTC");
    let mine_addr = bitcoin_client
        .get_new_address()
        .expect("Failed to get address");
    bitcoin_client
        .generate_blocks(&mine_addr, 3)
        .expect("Failed to mine blocks");

    wallet
        .wait_and_mint_quote(
            quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(120),
        )
        .await
        .expect("Failed to fund wallet");

    assert_eq!(wallet.total_balance().await.unwrap(), mint_amount.into());

    // Now melt back to an onchain address
    let dest_address = bitcoin_client
        .get_new_address()
        .expect("Failed to get dest address");
    let _melt_amount = 100_000;

    let melt_quote = wallet
        .melt_quote(
            PaymentMethod::Known(cdk::nuts::nut00::KnownMethod::Onchain),
            dest_address.clone(),
            None,
            None,
        )
        .await
        .expect("Failed to get melt quote");

    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .expect("Failed to prepare melt");

    let melt_result = prepared.confirm().await.expect("Failed to confirm melt");

    // Onchain melt is async, so it should be pending/paid depending on implementation
    println!("Melt status: {:?}", melt_result.state());

    // Mine blocks to confirm the melt batch
    // Batch processor might need some time to wake up (default 10s)
    sleep(Duration::from_secs(15)).await;
    bitcoin_client
        .generate_blocks(&mine_addr, 1)
        .expect("Failed to mine melt block");

    // Check balance in dest address
    // This is bitcoind balance check, but simpler to just check wallet balance decreased
    assert!(wallet.total_balance().await.unwrap() < 100_000.into());
}
