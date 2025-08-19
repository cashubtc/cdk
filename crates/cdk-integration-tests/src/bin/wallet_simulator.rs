use std::env;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use cashu::nuts::{CurrencyUnit, PaymentMethod};
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::MintQuoteState;
use cdk::wallet::{HttpClient, SendOptions, Wallet, WalletBuilder};
use rand::Rng;
use tokio::time::{sleep, timeout};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== CDK Wallet Simulator Starting ===");

    // Read environment variables with defaults
    let mint_url = env::var("MINT_URL").unwrap_or_else(|_| "http://127.0.0.1:8085".to_string());
    let unit_str = env::var("CURRENCY_UNIT").unwrap_or_else(|_| "Sat".to_string());
    let transactions_count: usize = env::var("TRANSACTION_COUNT")
        .unwrap_or_else(|_| "100".to_string())
        .parse()
        .unwrap_or(100);

    // Parse currency unit
    let unit = CurrencyUnit::from_str(&unit_str)?;

    println!("Configuration:");
    println!("  Mint URL: {}", mint_url);
    println!("  Currency Unit: {:?}", unit);
    println!("  Transactions Count: {}", transactions_count);

    // Generate a random seed for the wallet
    let seed: [u8; 64] = rand::rng().random();

    // Create a SQLite wallet database
    let temp_dir = std::env::temp_dir();
    let wallet_path = temp_dir.join(format!("wallet_simulator_{}.db", rand::random::<u64>()));
    println!(" Wallet db path: {}", wallet_path.to_string_lossy());

    let localstore = Arc::new(
        cdk_sqlite::WalletSqliteDatabase::new(wallet_path.to_str().unwrap())
            .await
            .expect("Could not create wallet database"),
    );

    // Create mint connector
    let connector = HttpClient::new(mint_url.parse()?, None);

    // Create wallet
    let wallet = WalletBuilder::new()
        .mint_url(mint_url.parse()?)
        .unit(unit.clone())
        .localstore(localstore)
        .seed(seed)
        .client(connector)
        .build()?;

    let wallet = Arc::new(wallet);

    // Get mint info to determine limits
    println!("\nFetching mint information...");
    let start_time = Instant::now();
    let mint_info = wallet
        .get_mint_info()
        .await?
        .ok_or_else(|| anyhow!("Could not get mint info"))?;
    let mint_info_time = start_time.elapsed();
    println!("Mint info retrieved in {} ms", mint_info_time.as_millis());

    // Get mint limits for the unit and bolt11 payment method
    let bolt11_method = PaymentMethod::Bolt11;
    let mint_limits = mint_info
        .nuts
        .nut04
        .get_settings(&unit, &bolt11_method)
        .ok_or_else(|| {
            anyhow!(
                "No mint settings found for unit {:?} and method {:?}",
                unit,
                bolt11_method
            )
        })?;

    let melt_limits = mint_info
        .nuts
        .nut05
        .get_settings(&unit, &bolt11_method)
        .ok_or_else(|| {
            anyhow!(
                "No melt settings found for unit {:?} and method {:?}",
                unit,
                bolt11_method
            )
        })?;

    // Validate limits
    let min_mint_amount = mint_limits.min_amount.unwrap_or(Amount::from(1));
    let max_mint_amount = mint_limits.max_amount.unwrap_or(Amount::from(1000000));
    let min_melt_amount = melt_limits.min_amount.unwrap_or(Amount::from(1));
    let max_melt_amount = melt_limits.max_amount.unwrap_or(Amount::from(1000000));

    // Assert that min and max are both >= 1, and max is greater than min
    assert!(
        min_mint_amount >= Amount::from(1),
        "Min mint amount must be >= 1"
    );
    assert!(
        max_mint_amount >= Amount::from(1),
        "Max mint amount must be >= 1"
    );
    assert!(
        max_mint_amount > min_mint_amount,
        "Max mint amount must be greater than min mint amount"
    );
    assert!(
        min_melt_amount >= Amount::from(1),
        "Min melt amount must be >= 1"
    );
    assert!(
        max_melt_amount >= Amount::from(1),
        "Max melt amount must be >= 1"
    );
    assert!(
        max_melt_amount > min_melt_amount,
        "Max melt amount must be greater than min melt amount"
    );

    println!("Mint limits: {} - {}", min_mint_amount, max_mint_amount);
    println!("Melt limits: {} - {}", min_melt_amount, max_melt_amount);

    // Fund the wallet with an initial amount
    println!("\nFunding wallet with initial amount...");
    let initial_amount = Amount::from(10000); // 10,000 sats or equivalent
    if let Err(e) = fund_wallet(wallet.clone(), initial_amount).await {
        eprintln!("Failed to fund wallet: {}", e);
        bail!("Could not fund wallet, aborting simulation");
    }
    println!("Wallet funded successfully");

    // Track successful transactions and timing
    let mut successful_transactions = 0;
    let mut mint_count = 0;
    let mut swap_count = 0;
    let mut melt_count = 0;
    let mut total_mint_time = Duration::new(0, 0);
    let mut total_swap_time = Duration::new(0, 0);
    let mut total_melt_time = Duration::new(0, 0);

    // Track failed transactions
    let mut failed_transactions = 0;
    let mut failed_mint_count = 0;
    let mut failed_swap_count = 0;
    let mut failed_melt_count = 0;

    // Perform transactions
    println!("\nPerforming {} transactions...", transactions_count);
    let total_start_time = Instant::now();
    for i in 0..transactions_count {
        // Choose transaction type (roughly even split)
        let transaction_type = i % 3;
        let transaction_start = Instant::now();

        let result = match transaction_type {
            0 => {
                // Mint transaction
                let result =
                    perform_mint_transaction(wallet.clone(), min_mint_amount, max_mint_amount)
                        .await;
                if result.is_ok() {
                    successful_transactions += 1;
                    mint_count += 1;
                    total_mint_time += transaction_start.elapsed();
                } else {
                    failed_transactions += 1;
                    failed_mint_count += 1;
                }
                result.map(|_| "Mint").map_err(|e| (e, "Mint"))
            }
            1 => {
                // Swap transaction
                let result = perform_swap_transaction(wallet.clone()).await;
                if result.is_ok() {
                    successful_transactions += 1;
                    swap_count += 1;
                    total_swap_time += transaction_start.elapsed();
                } else {
                    failed_transactions += 1;
                    failed_swap_count += 1;
                }
                result.map(|_| "Swap").map_err(|e| (e, "Swap"))
            }
            2 => {
                // Melt transaction (if we have enough balance)
                let result =
                    perform_melt_transaction(wallet.clone(), min_melt_amount, max_melt_amount)
                        .await;
                if result.is_ok() {
                    successful_transactions += 1;
                    melt_count += 1;
                    total_melt_time += transaction_start.elapsed();
                } else {
                    failed_transactions += 1;
                    failed_melt_count += 1;
                }
                result.map(|_| "Melt").map_err(|e| (e, "Melt"))
            }
            _ => unreachable!(),
        };

        // Print error message if transaction failed
        if let Err((error, transaction_type)) = result {
            eprintln!("{} transaction failed: {}", transaction_type, error);
        }

        // Small delay between transactions
        sleep(Duration::from_millis(100)).await;
    }
    let total_transaction_time = total_start_time.elapsed();

    // Calculate average times
    let avg_mint_time = if mint_count > 0 {
        total_mint_time / mint_count as u32
    } else {
        Duration::new(0, 0)
    };

    let avg_swap_time = if swap_count > 0 {
        total_swap_time / swap_count as u32
    } else {
        Duration::new(0, 0)
    };

    let avg_melt_time = if melt_count > 0 {
        total_melt_time / melt_count as u32
    } else {
        Duration::new(0, 0)
    };

    // Print summary
    println!("\n=== Transaction Summary ===");
    println!("Total transactions attempted: {}", transactions_count);
    println!("Successful transactions: {}", successful_transactions);
    println!("Failed transactions: {}", failed_transactions);
    println!(
        "  Mints: {} successful, {} failed (avg time: {} ms)",
        mint_count,
        failed_mint_count,
        avg_mint_time.as_millis()
    );
    println!(
        "  Swaps: {} successful, {} failed (avg time: {} ms)",
        swap_count,
        failed_swap_count,
        avg_swap_time.as_millis()
    );
    println!(
        "  Melts: {} successful, {} failed (avg time: {} ms)",
        melt_count,
        failed_melt_count,
        avg_melt_time.as_millis()
    );
    println!(
        "Total transaction time: {} ms",
        total_transaction_time.as_millis()
    );

    // Perform restore

    let p = wallet.check_all_pending_proofs().await?;

    println!("Amount pending {p}");

    println!("\nPerforming restore...");

    // Check balance consistency
    let original_balance = wallet.total_balance().await?;

    let t = wallet.total_pending_balance().await?;

    let original_balance = original_balance + t;

    let restored_wallet = perform_restore(&mint_url, unit, seed).await?;

    let restored_balance = restored_wallet.total_balance().await?;

    if original_balance == restored_balance {
        println!("Balance check successful: {}", original_balance);
    } else {
        println!(
            "Balance mismatch! Original: {}, Restored: {}",
            original_balance, restored_balance
        );
        bail!("Balance mismatch after restore");
    }

    println!("Balance: {}", original_balance);
    println!("=== Wallet Simulation Completed Successfully ===");

    Ok(())
}

async fn fund_wallet(wallet: Arc<Wallet>, amount: Amount) -> Result<()> {
    // Request a mint quote
    let quote = wallet.mint_quote(amount, None).await?;

    // Check the quote state in a loop with a timeout
    let timeout_duration = Duration::from_secs(60);
    let start = std::time::Instant::now();

    loop {
        let status = wallet.mint_quote_state(&quote.id).await?;

        if status.state == MintQuoteState::Paid {
            break;
        }

        if start.elapsed() >= timeout_duration {
            return Err(anyhow!("Timeout while waiting for mint quote to be paid"));
        }

        sleep(Duration::from_secs(1)).await;
    }

    // Mint the received amount
    let _proofs = wallet.mint(&quote.id, SplitTarget::default(), None).await?;
    println!("Wallet funded with {}", amount);

    Ok(())
}

async fn perform_mint_transaction(
    wallet: Arc<Wallet>,
    min_amount: Amount,
    max_amount: Amount,
) -> Result<()> {
    // Generate a random amount between min and max
    let min_value = u64::from(min_amount);
    let max_value = u64::from(max_amount);

    // Ensure we have a valid range
    if min_value >= max_value {
        return Err(anyhow!(
            "Invalid mint amount range: min {} >= max {}",
            min_value,
            max_value
        ));
    }

    let amount_value = rand::rng().random_range(min_value..=max_value);
    let amount = Amount::from(amount_value);

    // Request a mint quote
    let quote = wallet.mint_quote(amount, None).await?;

    // Wait for quote to be paid
    let timeout_duration = Duration::from_secs(30);
    match timeout(timeout_duration, async {
        loop {
            let status = wallet.mint_quote_state(&quote.id).await?;
            if status.state == MintQuoteState::Paid {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    {
        Ok(_) => {
            // Mint the received amount
            let _proofs = wallet.mint(&quote.id, SplitTarget::default(), None).await?;
            Ok(())
        }
        Err(_) => Err(anyhow!("Timeout waiting for mint quote to be paid")),
    }
}

async fn perform_swap_transaction(wallet: Arc<Wallet>) -> Result<()> {
    // Get the current balance
    let balance = wallet.total_balance().await?;

    if balance < Amount::from(1) {
        return Err(anyhow!("Insufficient balance for swap"));
    }

    // Swap a random amount between 1 and the wallet's maximum balance
    let max_value = u64::from(balance);
    let swap_amount_value = rand::rng().random_range(1..=max_value);
    let swap_amount = Amount::from(swap_amount_value);

    // Prepare send
    let prepared_send = wallet
        .prepare_send(swap_amount, SendOptions::default())
        .await?;

    // Send (this will internally swap)
    let token = prepared_send.confirm(None).await?;

    wallet
        .receive(&token.to_string(), Default::default())
        .await?;

    Ok(())
}

async fn perform_melt_transaction(
    wallet: Arc<Wallet>,
    min_amount: Amount,
    max_amount: Amount,
) -> Result<()> {
    // Get the current balance
    let balance = wallet.total_balance().await?;

    // Use the minimum of the wallet balance and the max_amount from mint limits
    let effective_max = if balance < max_amount {
        balance
    } else {
        max_amount
    };

    // Generate a random amount between min_amount and effective_max
    let min_value = u64::from(min_amount);
    let max_value = u64::from(effective_max);

    // Ensure we have a valid range
    if min_value >= max_value {
        // If min is >= max, just use the min amount if we have sufficient balance
        if balance >= min_amount {
            // Use min_amount but make sure it doesn't exceed balance
            let amount = if min_amount <= balance {
                min_amount
            } else {
                balance
            };

            // For melt, we need an invoice to pay
            // We'll create a fake invoice for testing purposes
            let invoice =
                cdk_fake_wallet::create_fake_invoice(u64::from(amount) * 1000, "".to_string());

            // Melt the invoice
            let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

            // Attempt to melt
            let _melted = wallet.melt(&melt_quote.id).await?;

            return Ok(());
        } else {
            return Err(anyhow!("Insufficient balance for melt"));
        }
    }

    let amount_value = rand::rng().random_range(min_value..=max_value);
    let amount = Amount::from(amount_value);

    // For melt, we need an invoice to pay
    // We'll create a fake invoice for testing purposes
    let invoice = cdk_fake_wallet::create_fake_invoice(u64::from(amount) * 1000, "".to_string());

    // Melt the invoice
    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // Attempt to melt
    let _melted = wallet.melt(&melt_quote.id).await?;

    Ok(())
}

async fn perform_restore(
    mint_url: &str,
    unit: CurrencyUnit,
    seed: [u8; 64],
) -> Result<Arc<Wallet>> {
    // Create a new wallet with the same seed for restore
    let temp_dir = std::env::temp_dir();
    let wallet_path = temp_dir.join(format!("restore_wallet_{}.db", rand::random::<u64>()));

    let localstore = Arc::new(
        cdk_sqlite::WalletSqliteDatabase::new(wallet_path.to_str().unwrap())
            .await
            .expect("Could not create restore wallet database"),
    );

    let connector = HttpClient::new(mint_url.parse()?, None);

    let wallet = WalletBuilder::new()
        .mint_url(mint_url.parse()?)
        .unit(unit.clone())
        .localstore(localstore)
        .seed(seed)
        .client(connector)
        .build()?;

    let wallet = Arc::new(wallet);

    // Perform restore
    wallet.restore().await?;

    Ok(wallet)
}
