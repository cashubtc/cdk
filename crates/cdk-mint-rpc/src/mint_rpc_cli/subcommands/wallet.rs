use anyhow::Result;
use cdk_common::terminal::escape_control;
use clap::Args;
use tonic::Request;

use crate::wallet::{
    CreateDepositAddressRequest, GetBalanceRequest, ListAddressesRequest, ListTransactionsRequest,
};
use crate::InterceptedWalletServiceClient;

/// Pagination arguments for wallet list commands.
#[derive(Debug, Args)]
pub struct WalletPaginationCommand {
    /// Maximum records to return; zero uses the server default.
    #[arg(long, default_value_t = 0)]
    limit: u32,
    /// Records to skip.
    #[arg(long, default_value_t = 0)]
    offset: u32,
}

/// Creates and prints an address for operator deposits.
pub async fn create_wallet_deposit_address(
    client: &mut InterceptedWalletServiceClient,
) -> Result<()> {
    let response = client
        .create_deposit_address(Request::new(CreateDepositAddressRequest {}))
        .await?
        .into_inner();

    println!("address: {}", response.address);

    Ok(())
}

/// Prints the BDK on-chain wallet balance.
pub async fn get_wallet_balance(client: &mut InterceptedWalletServiceClient) -> Result<()> {
    let balance = client
        .get_balance(Request::new(GetBalanceRequest {}))
        .await?
        .into_inner();

    println!(
        "network:                {}",
        escape_control(&balance.network)
    );
    println!("synced height:          {}", balance.synced_height);
    println!("confirmed:              {} sat", balance.confirmed_sat);
    println!(
        "trusted pending:        {} sat",
        balance.trusted_pending_sat
    );
    println!(
        "untrusted pending:      {} sat",
        balance.untrusted_pending_sat
    );
    println!("immature:               {} sat", balance.immature_sat);
    println!(
        "trusted spendable:      {} sat",
        balance.trusted_spendable_sat
    );
    println!("total:                  {} sat", balance.total_sat);

    Ok(())
}

/// Prints a page of BDK on-chain wallet transactions.
pub async fn list_wallet_transactions(
    client: &mut InterceptedWalletServiceClient,
    command: &WalletPaginationCommand,
) -> Result<()> {
    let response = client
        .list_transactions(Request::new(ListTransactionsRequest {
            limit: command.limit,
            offset: command.offset,
        }))
        .await?
        .into_inner();

    println!("total: {}", response.total);
    for transaction in response.transactions {
        println!(
            "txid: {}, received: {} sat, sent: {} sat, fee: {}, delta: {} sat, height: {}, confirmation_time: {}, first_seen: {}",
            escape_control(&transaction.txid),
            transaction.received_sat,
            transaction.sent_sat,
            transaction
                .fee_sat
                .map(|fee| format!("{fee} sat"))
                .unwrap_or_else(|| "unknown".to_string()),
            transaction.balance_delta_sat,
            transaction
                .confirmation_height
                .map(|height| height.to_string())
                .unwrap_or_else(|| "unconfirmed".to_string()),
            transaction
                .confirmation_time
                .map(|timestamp| timestamp.to_string())
                .unwrap_or_else(|| "none".to_string()),
            transaction
                .first_seen
                .map(|timestamp| timestamp.to_string())
                .unwrap_or_else(|| "none".to_string()),
        );
        for input in transaction.inputs {
            println!(
                "  input: txid {}, vout {}, amount: {}, address: {}",
                escape_control(&input.txid),
                input.vout,
                input
                    .amount_sat
                    .map(|amount| format!("{amount} sat"))
                    .unwrap_or_else(|| "unknown".to_string()),
                escape_control(input.address.as_deref().unwrap_or("unknown")),
            );
        }
        for output in transaction.outputs {
            println!(
                "  output: vout {}, address: {}, amount: {} sat, quote_id: {}",
                output.vout,
                escape_control(&output.address),
                output.amount_sat,
                escape_control(output.quote_id.as_deref().unwrap_or("none")),
            );
        }
    }

    Ok(())
}

/// Prints a page of addresses revealed by the BDK on-chain wallet.
pub async fn list_wallet_addresses(
    client: &mut InterceptedWalletServiceClient,
    command: &WalletPaginationCommand,
) -> Result<()> {
    let response = client
        .list_addresses(Request::new(ListAddressesRequest {
            limit: command.limit,
            offset: command.offset,
        }))
        .await?
        .into_inner();

    println!("total: {}", response.total);
    for address in response.addresses {
        println!(
            "address: {}, keychain: {}, index: {}, used: {}, balance: {} sat, confirmed: {} sat",
            escape_control(&address.address),
            address.keychain().as_str_name(),
            address.derivation_index,
            address.used,
            address.balance_sat,
            address.confirmed_balance_sat,
        );
    }

    Ok(())
}
