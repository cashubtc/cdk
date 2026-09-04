use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::Token;
use cdk::wallet::{ReceiveOptions, WalletRepository};
use cdk_common::PaymentRequestPayload;
use nostr::prelude::{Filter, Keys, Kind, PublicKey, SecretKey, UnwrappedGift};
use nostr_sdk::prelude::{Client, SignerAuthenticator};

use super::create_request::StoredNostrWaitInfo;
use crate::terminal::escape_control;
use crate::utils::get_or_create_wallet;

fn unaccepted_mint_warning(request_id: &str, mint_url: &MintUrl) -> String {
    format!(
        "Ignoring payment for request {} from unaccepted mint {}",
        escape_control(request_id),
        escape_control(&mint_url.to_string())
    )
}

pub async fn check_requests(wallet_repository: &WalletRepository) -> Result<()> {
    let wallets = wallet_repository.get_wallets().await;

    if let Some(wallet) = wallets.first() {
        let keys = wallet
            .localstore
            .kv_list("cdk_cli", "pending_nostr_requests")
            .await?;

        if keys.is_empty() {
            println!("No stored payment requests found.");
            return Ok(());
        }

        println!("Checking {} stored Nostr payment requests...", keys.len());

        for key in keys {
            if let Some(val) = wallet
                .localstore
                .kv_read("cdk_cli", "pending_nostr_requests", &key)
                .await?
            {
                let info: StoredNostrWaitInfo = serde_json::from_slice(&val)?;

                let secret_key = SecretKey::from_str(&info.secret_key_hex)?;
                let keys = Keys::new(secret_key);
                let pubkey = PublicKey::from_hex(&info.pubkey_hex)?;

                let client = Client::builder()
                    .authenticator(SignerAuthenticator::new(keys.clone()))
                    .build();
                for r in &info.relays {
                    client.add_relay(r).await?;
                }
                client.connect().await;

                let filter = Filter::new().pubkey(pubkey).kind(Kind::GiftWrap);
                let events = client
                    .fetch_events(filter)
                    .timeout(Duration::from_secs(10))
                    .await?;

                for event in events {
                    if let Ok(unwrapped) = UnwrappedGift::from_gift_wrap(&keys, &event) {
                        if let Ok(payload) =
                            serde_json::from_str::<PaymentRequestPayload>(&unwrapped.rumor.content)
                        {
                            if !info.accepts_mint(&payload.mint) {
                                tracing::warn!("{}", unaccepted_mint_warning(&key, &payload.mint));
                                continue;
                            }

                            let token = Token::new(
                                payload.mint.clone(),
                                payload.proofs,
                                payload.memo,
                                payload.unit.clone(),
                            );

                            let token_str = token.to_string();
                            let mint_url = token.mint_url()?;
                            let unit = token.unit().unwrap_or_default();

                            // Get or create wallet for the token's mint
                            let wallet =
                                get_or_create_wallet(wallet_repository, &mint_url, &unit).await?;

                            match wallet.receive(&token_str, ReceiveOptions::default()).await {
                                Ok(amount) => {
                                    if amount > cdk::Amount::ZERO {
                                        println!("Received {} from request {}", amount, key);
                                    }
                                }
                                Err(e) => {
                                    // Silently ignore already claimed proofs if that's what the error is
                                    // or print if it's something else.
                                    // For now, let's just log it.
                                    tracing::debug!(
                                        "Failed to receive token for {}: {}",
                                        escape_control(&key),
                                        escape_control(&e.to_string())
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unaccepted_mint_warning_escapes_terminal_controls() {
        let mint =
            MintUrl::from_str("https://example.com/\u{1b}]52;c;clipboard\u{07}\r\u{85}\u{202e}")
                .expect("mint URL should parse");
        let output = unaccepted_mint_warning("request", &mint);

        assert_eq!(
            output,
            "Ignoring payment for request request from unaccepted mint \
             https://example.com/\\e]52;c;clipboard\\a\\r\\u{85}\\u{202e}"
        );
        assert!(!output
            .chars()
            .any(|ch| { ch.is_control() || cdk_common::terminal::is_bidi_control_character(ch) }));
    }
}
