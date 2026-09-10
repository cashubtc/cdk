use anyhow::Result;
use cdk::wallet::{NostrRequestStatus, WalletRepository};

use crate::terminal::escape_control;

pub async fn check_requests(wallet_repository: &WalletRepository) -> Result<()> {
    if let Some(wallet) = wallet_repository.get_wallets().await.first() {
        if !wallet
            .localstore
            .kv_list("cdk_cli", "pending_nostr_requests")
            .await?
            .is_empty()
        {
            tracing::warn!("Legacy CLI payment requests are not resumable through the new request store; recreate them");
        }
    }
    let requests = wallet_repository.list_nostr_requests().await?;
    if requests.is_empty() {
        println!("No stored payment requests found.");
    }
    for request in requests {
        let id = request
            .request
            .payment_id
            .as_deref()
            .ok_or(cdk::Error::InvalidPaymentRequest)?;
        if matches!(
            request.status,
            NostrRequestStatus::Completed | NostrRequestStatus::Cancelled
        ) {
            continue;
        }
        match wallet_repository.check_nostr_request(id).await {
            Ok(request) => match request.status {
                NostrRequestStatus::Completed => println!(
                    "Received {} from request {}",
                    request.received.ok_or(cdk::Error::InvalidPaymentRequest)?,
                    escape_control(id)
                ),
                NostrRequestStatus::Receiving => println!(
                    "Request {} has a receive operation awaiting recovery",
                    escape_control(id)
                ),
                NostrRequestStatus::Pending => {
                    println!("Request {} is still pending", escape_control(id))
                }
                NostrRequestStatus::Cancelled => {}
            },
            Err(error) => tracing::warn!(
                "Could not check request {}: {}",
                escape_control(id),
                escape_control(&error.to_string())
            ),
        }
    }
    Ok(())
}
