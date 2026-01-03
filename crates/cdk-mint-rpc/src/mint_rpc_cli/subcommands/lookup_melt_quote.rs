use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::LookupQuoteRequest;

/// Command to look up a specific melt quote
///
/// This command retrieves detailed information about a specific melt quote by its ID.
/// Melt quotes represent requests to redeem tokens in exchange for external payments.
/// The command displays comprehensive quote details including state, amounts, payment
/// information, and optional melt configuration options.
#[derive(Args)]
pub struct LookupMeltQuoteCommand {
    /// The quote ID to look up
    quote_id: String,
}

/// Executes the lookup_melt_quote command against the mint server
///
/// This function sends an RPC request to retrieve detailed melt quote information from
/// the mint and displays it in a formatted output. If the quote is not found, it returns
/// an error. The output includes all quote details such as ID, state, unit, amount, fee
/// reserve, payment method, timestamps, and optional fields like lookup ID, paid time, and
/// payment preimage. If melt options are present, they are displayed with their specific
/// configuration details.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `args` - The command arguments containing the quote ID to look up
pub async fn lookup_melt_quote(
    client: &mut CdkMintReportingClient<Channel>,
    args: &LookupMeltQuoteCommand,
) -> Result<()> {
    let response = client
        .lookup_melt_quote(Request::new(LookupQuoteRequest {
            quote_id: args.quote_id.clone(),
        }))
        .await?;

    let quote = response
        .into_inner()
        .quote
        .ok_or_else(|| anyhow::anyhow!("Quote not found"))?;

    println!("Melt Quote Details");
    println!("{}", "=".repeat(50));
    println!("ID:              {}", quote.id);
    println!("State:           {}", quote.state);
    println!("Unit:            {}", quote.unit);
    println!("Amount:          {}", quote.amount);
    println!("Fee Reserve:     {}", quote.fee_reserve);
    println!("Payment Method:  {}", quote.payment_method);
    println!("Created:         {}", quote.created_time);
    println!("Request:         {}", quote.request);

    if let Some(lookup_id) = &quote.request_lookup_id {
        println!("Lookup ID:       {}", lookup_id);
    }

    if let Some(paid_time) = quote.paid_time {
        println!("Paid Time:       {}", paid_time);
    }

    if let Some(preimage) = &quote.payment_preimage {
        println!("Preimage:        {}", preimage);
    }

    // Display melt options if present
    if let Some(options) = &quote.options {
        if let Some(opts) = &options.options {
            println!("\nMelt Options:");
            match opts {
                crate::melt_options::Options::Mpp(mpp) => {
                    println!("  Type:   MPP (Multi-Path Payment)");
                    println!("  Amount: {}", mpp.amount);
                }
                crate::melt_options::Options::Amountless(amountless) => {
                    println!("  Type:       Amountless");
                    println!("  Amount msat: {}", amountless.amount_msat);
                }
            }
        }
    }

    Ok(())
}
