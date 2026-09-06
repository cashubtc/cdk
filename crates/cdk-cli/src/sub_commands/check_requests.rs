use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use cdk::cdk_database::{self, WalletDatabase};
use cdk::wallet::WalletManager;

use super::create_request::StoredNostrWaitInfo;
use crate::terminal::escape_control;

pub async fn check_requests(
    wallet_manager: &WalletManager,
    localstore: &Arc<dyn WalletDatabase<cdk_database::Error> + Send + Sync>,
) -> Result<()> {
    let keys = localstore
        .kv_list("cdk_cli", "pending_nostr_requests")
        .await?;

    if keys.is_empty() {
        println!("No stored payment requests found.");
        return Ok(());
    }

    println!("Checking {} stored Nostr payment requests...", keys.len());

    for key in keys {
        if let Some(val) = localstore
            .kv_read("cdk_cli", "pending_nostr_requests", &key)
            .await?
        {
            let info: StoredNostrWaitInfo = serde_json::from_slice(&val)?;
            let receiver =
                match wallet_manager.resume_payment_request_receiver(info.into_receiver_state()) {
                    Ok(receiver) => receiver,
                    Err(error) => {
                        tracing::warn!(
                            "Could not restore payment request {}: {}",
                            escape_control(&key),
                            escape_control(&error.to_string())
                        );
                        continue;
                    }
                };

            match receiver.receive_with_timeout(Duration::from_secs(10)).await {
                Ok(Some(amount)) => {
                    println!("Received {} from request {}", amount, key);
                    localstore
                        .kv_remove("cdk_cli", "pending_nostr_requests", &key)
                        .await?;
                }
                Ok(None) => {}
                Err(error) => tracing::debug!(
                    "Failed to receive payment for {}: {}",
                    escape_control(&key),
                    escape_control(&error.to_string())
                ),
            }
        }
    }

    Ok(())
}
