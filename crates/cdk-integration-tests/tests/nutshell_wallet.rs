use std::time::Duration;

use cdk_fake_wallet::create_fake_invoice;
use cdk_http_client::HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::sleep;
use cdk_http_client::RequestBuilderExt;

/// Response from the invoice creation endpoint
#[derive(Debug, Serialize, Deserialize)]
struct InvoiceResponse {
    payment_request: String,
    checking_id: Option<String>,
}

/// Maximum number of attempts to check invoice payment status
const MAX_PAYMENT_CHECK_ATTEMPTS: u8 = 20;
/// Delay between payment status checks in milliseconds
const PAYMENT_CHECK_DELAY_MS: u64 = 500;
/// Default test amount in satoshis
const DEFAULT_TEST_AMOUNT: u64 = 10000;

/// Helper function to mint tokens via Lightning invoice
async fn mint_tokens(base_url: &str, amount: u64) -> String {
    let client = HttpClient::new();

    // Create an invoice for the specified amount
    let invoice_url = format!("{}/lightning/create_invoice?amount={}", base_url, amount);

    let invoice_response: InvoiceResponse = client
        .post(&invoice_url)
        .send()
        .await
        .expect("Failed to send invoice creation request")
        .json()
        .await
        .expect("Failed to parse invoice response");

    println!("Created invoice: {}", invoice_response.payment_request);

    invoice_response.payment_request
}

/// Helper function to wait for payment confirmation
async fn wait_for_payment_confirmation(base_url: &str, payment_request: &str) {
    let client = HttpClient::new();
    let check_url = format!(
        "{}/lightning/invoice_state?payment_request={}",
        base_url, payment_request
    );

    let mut payment_confirmed = false;

    for attempt in 1..=MAX_PAYMENT_CHECK_ATTEMPTS {
        println!(
            "Checking invoice state (attempt {}/{})...",
            attempt, MAX_PAYMENT_CHECK_ATTEMPTS
        );

        let response = client
            .get(&check_url)
            .send()
            .await
            .expect("Failed to send payment check request");

        if response.is_success() {
            let state: Value = response
                .json()
                .await
                .expect("Failed to parse payment state response");
            println!("Payment state: {:?}", state);

            if let Some(result) = state.get("result") {
                if result == 1 {
                    payment_confirmed = true;
                    break;
                }
            }
        } else {
            println!("Failed to check payment state: {}", response.status());
        }

        sleep(Duration::from_millis(PAYMENT_CHECK_DELAY_MS)).await;
    }

    if !payment_confirmed {
        panic!("Payment not confirmed after maximum attempts");
    }
}

/// Helper function to get the current wallet balance
async fn get_wallet_balance(base_url: &str) -> u64 {
    let client = HttpClient::new();
    let balance_url = format!("{}/balance", base_url);

    let balance: Value = client
        .fetch(&balance_url)
        .await
        .expect("Failed to fetch balance");

    balance["balance"]
        .as_u64()
        .expect("Could not parse balance as u64")
}

/// Test the Nutshell wallet's ability to mint tokens from a Lightning invoice
#[tokio::test]
async fn test_nutshell_wallet_mint() {
    // Get the wallet URL from environment variable
    let base_url = std::env::var("WALLET_URL").expect("Wallet url is not set");

    // Step 1: Create an invoice and mint tokens
    let amount = DEFAULT_TEST_AMOUNT;
    let payment_request = mint_tokens(&base_url, amount).await;

    // Step 2: Wait for the invoice to be paid
    wait_for_payment_confirmation(&base_url, &payment_request).await;

    // Step 3: Check the wallet balance
    let available_balance = get_wallet_balance(&base_url).await;

    // Verify the balance is at least the amount we minted
    assert!(
        available_balance >= amount,
        "Balance should be at least {} but was {}",
        amount,
        available_balance
    );
}

/// Test the Nutshell wallet's ability to mint tokens from a Lightning invoice
#[tokio::test]
async fn test_nutshell_wallet_swap() {
    // Get the wallet URL from environment variable
    let base_url = std::env::var("WALLET_URL").expect("Wallet url is not set");

    // Step 1: Create an invoice and mint tokens
    let amount = DEFAULT_TEST_AMOUNT;
    let payment_request = mint_tokens(&base_url, amount).await;

    // Step 2: Wait for the invoice to be paid
    wait_for_payment_confirmation(&base_url, &payment_request).await;

    let send_amount = 100;
    let send_url = format!("{}/send?amount={}", base_url, send_amount);
    let client = HttpClient::new();

    let response: Value = client
        .post(&send_url)
        .send()
        .await
        .expect("Failed to send payment check request")
        .json()
        .await
        .expect("Valid json");

    // Extract the token and remove the surrounding quotes
    let token_with_quotes = response
        .get("token")
        .expect("Missing token")
        .as_str()
        .expect("Token is not a string");
    let token = token_with_quotes.trim_matches('"');

    let receive_url = format!("{}/receive?token={}", base_url, token);

    let response: Value = client
        .post(&receive_url)
        .send()
        .await
        .expect("Failed to receive request")
        .json()
        .await
        .expect("Valid json");

    let balance = response
        .get("balance")
        .expect("Bal in response")
        .as_u64()
        .expect("Valid num");
    let initial_balance = response
        .get("initial_balance")
        .expect("Bal in response")
        .as_u64()
        .expect("Valid num");

    let token_received = balance - initial_balance;

    let fee = 1;
    assert_eq!(token_received, send_amount - fee);
}

/// Test the Nutshell wallet's ability to melt tokens to pay a Lightning invoice
#[tokio::test]
async fn test_nutshell_wallet_melt() {
    // Get the wallet URL from environment variable
    let base_url = std::env::var("WALLET_URL").expect("Wallet url is not set");

    // Step 1: Create an invoice and mint tokens
    let amount = DEFAULT_TEST_AMOUNT;
    let payment_request = mint_tokens(&base_url, amount).await;

    // Step 2: Wait for the invoice to be paid
    wait_for_payment_confirmation(&base_url, &payment_request).await;

    // Get initial balance
    let initial_balance = get_wallet_balance(&base_url).await;
    println!("Initial balance: {}", initial_balance);

    // Step 3: Create a fake invoice to pay
    let payment_amount = 1000; // 1000 sats
    let fake_invoice = create_fake_invoice(payment_amount, "Test payment".to_string());
    let pay_url = format!("{}/lightning/pay_invoice?bolt11={}", base_url, fake_invoice);
    let client = HttpClient::new();

    // Step 4: Pay the invoice
    let _response: Value = client
        .post(&pay_url)
        .send()
        .await
        .expect("Failed to send pay request")
        .json()
        .await
        .expect("Failed to parse pay response");

    let final_balance = get_wallet_balance(&base_url).await;
    println!("Final balance: {}", final_balance);

    assert!(
        initial_balance > final_balance,
        "Balance should decrease after payment"
    );

    let balance_difference = initial_balance - final_balance;
    println!("Balance decreased by: {}", balance_difference);

    // The balance difference should be at least the payment amount
    assert!(
        balance_difference >= (payment_amount / 1000),
        "Balance should decrease by at least the payment amount ({}) but decreased by {}",
        payment_amount,
        balance_difference
    );
}
