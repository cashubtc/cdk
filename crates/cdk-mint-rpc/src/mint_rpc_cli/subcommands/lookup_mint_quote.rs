use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::LookupQuoteRequest;

/// Command to look up a specific mint quote
///
/// This command retrieves detailed information about a specific mint quote by its ID.
/// Mint quotes represent requests to mint tokens in exchange for external payments.
/// The command displays comprehensive quote details including state, amounts, payment
/// information, payment history, and token issuance records.
///
/// # Arguments
/// * `quote_id` - The quote ID to look up
#[derive(Args)]
pub struct LookupMintQuoteCommand {
    /// The quote ID to look up
    quote_id: String,
}

/// Executes the lookup_mint_quote command against the mint server
///
/// This function sends an RPC request to retrieve detailed mint quote information from
/// the mint and displays it in a formatted output. If the quote is not found, it returns
/// an error. The output includes all quote details such as ID, state, unit, amount, payment
/// amounts, issued amounts, payment method, timestamps, and optional fields like lookup ID
/// and pubkey. If payments or issuances are present, they are displayed in separate sections
/// with their respective details.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `args` - The command arguments containing the quote ID to look up
pub async fn lookup_mint_quote(
    client: &mut CdkMintReportingClient<Channel>,
    args: &LookupMintQuoteCommand,
) -> Result<()> {
    let response = client
        .lookup_mint_quote(Request::new(LookupQuoteRequest {
            quote_id: args.quote_id.clone(),
        }))
        .await?;

    let quote = response
        .into_inner()
        .quote
        .ok_or_else(|| anyhow::anyhow!("Quote not found"))?;

    println!("Mint Quote Details");
    println!("{}", "=".repeat(50));
    println!("ID:              {}", quote.id);
    println!("State:           {}", quote.state);
    println!("Unit:            {}", quote.unit);
    println!(
        "Amount:          {}",
        quote
            .amount
            .map(|a| a.to_string())
            .unwrap_or("-".to_string())
    );
    println!("Amount Paid:     {}", quote.amount_paid);
    println!("Amount Issued:   {}", quote.amount_issued);
    println!("Payment Method:  {}", quote.payment_method);
    println!("Created:         {}", quote.created_time);
    println!("Request:         {}", quote.request);

    if let Some(lookup_id) = &quote.request_lookup_id {
        println!(
            "Lookup ID:       {} ({})",
            lookup_id, quote.request_lookup_id_kind
        );
    }

    if let Some(pubkey) = &quote.pubkey {
        println!("Pubkey:          {}", pubkey);
    }

    // Display payments
    if !quote.payments.is_empty() {
        println!("\nPayments ({}):", quote.payments.len());
        for (i, p) in quote.payments.iter().enumerate() {
            println!("  [{}] ID:     {}", i + 1, p.payment_id);
            println!("      Amount: {}", p.amount);
            println!("      Time:   {}", p.time);
        }
    }

    // Display issuances
    if !quote.issuances.is_empty() {
        println!("\nIssuances ({}):", quote.issuances.len());
        for (i, iss) in quote.issuances.iter().enumerate() {
            println!("  [{}] Amount: {}", i + 1, iss.amount);
            println!("      Time:   {}", iss.time);
        }
    }

    Ok(())
}
