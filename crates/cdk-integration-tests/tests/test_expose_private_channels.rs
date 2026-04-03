//! Test for CLN expose_private_channels feature
//!
//! Verifies that when expose_private_channels is enabled, bolt11 invoices
//! include route hints for private (unannounced) channels.
//!
//! Topology:
//!   CLN-1 has public channels (to LND-1, LND-2) and a private channel (to CLN-2).
//!   With expose_private_channels=true, private channels become route hint
//!   candidates. CLN selects among all candidates per invoice, so the private
//!   channel may not appear in every invoice but must appear in at least one.
//!
//! Requires regtest environment with CLN nodes running.

use std::str::FromStr;

use anyhow::Result;
use cashu::Bolt11Invoice;
use cdk::nuts::CurrencyUnit;
use cdk_common::payment::{Bolt11IncomingPaymentOptions, IncomingPaymentOptions, MintPayment};
use cdk_integration_tests::init_regtest::{
    create_cln_backend_with_options, generate_block, get_cln_dir, init_bitcoin_client,
};
use cdk_integration_tests::ln_regtest::ln_client::{ClnClient, LightningClient};

fn get_work_dir() -> std::path::PathBuf {
    match std::env::var("CDK_ITESTS_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => panic!("CDK_ITESTS_DIR not set"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_expose_private_channels() -> Result<()> {
    // Skip if not in regtest environment
    if std::env::var("CDK_TEST_REGTEST").is_err() {
        return Ok(());
    }

    let work_dir = get_work_dir();

    // Connect to CLN-1 and CLN-2
    let cln_one_dir = get_cln_dir(&work_dir, "one");
    let cln_two_dir = get_cln_dir(&work_dir, "two");

    let cln_one = ClnClient::new(cln_one_dir, None).await?;
    let cln_two = ClnClient::new(cln_two_dir, None).await?;

    // Open a private channel from CLN-1 to CLN-2
    let cln_two_info = cln_two.get_connect_info().await?;
    cln_one
        .connect_peer(
            cln_two_info.pubkey.clone(),
            cln_two_info.address.clone(),
            cln_two_info.port,
        )
        .await
        .ok(); // May already be connected

    // Only open private channel if it doesn't already exist
    if let Err(e) = cln_one
        .open_private_channel(100_000, &cln_two_info.pubkey, Some(50_000))
        .await
    {
        println!("Private channel already exists or could not be opened: {e}");
    } else {
        // Mine blocks to confirm the new channel
        let bitcoin_client = init_bitcoin_client()?;
        generate_block(&bitcoin_client)?;
    }

    // Wait for channels to be active
    cln_one.wait_channels_active().await?;

    // Create backend with expose_private_channels = true
    let cln_backend = create_cln_backend_with_options(&cln_one, true).await?;

    let max_attempts = 100;
    let mut found = false;

    for i in 0..max_attempts {
        let amount = cdk_common::amount::Amount::new(10_000, CurrencyUnit::Msat);
        let response = cln_backend
            .create_incoming_payment_request(IncomingPaymentOptions::Bolt11(
                Bolt11IncomingPaymentOptions {
                    amount,
                    description: Some(format!("test exposed {i}")),
                    unix_expiry: None,
                },
            ))
            .await?;

        let invoice = Bolt11Invoice::from_str(&response.request)?;
        let hints = invoice.route_hints();

        let has_private_channel_hint = hints.iter().any(|hint| {
            hint.0
                .iter()
                .any(|hop| hop.src_node_id.to_string() == cln_two_info.pubkey)
        });

        println!(
            "Invoice {i}: private_channel_hint={has_private_channel_hint}, total_hints={}",
            hints.len()
        );

        if has_private_channel_hint {
            println!("Private channel hint found on attempt {i}");
            found = true;
            break;
        }
    }

    assert!(
        found,
        "None of {max_attempts} invoices included the private channel route hint. \
         expose_private_channels=true should make private channels route hint candidates."
    );

    Ok(())
}
