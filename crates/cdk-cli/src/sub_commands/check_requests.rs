use std::collections::HashSet;
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::Token;
use cdk::wallet::payment_request::parse_nostr_payment_payload;
use cdk::wallet::{ReceiveOptions, WalletRepository};
use nostr_sdk::{Filter, Keys, Kind, PublicKey, SecretKey, Timestamp};

use super::create_request::StoredNostrWaitInfo;
use crate::terminal::escape_control;
use crate::utils::get_or_create_wallet;

/// Maximum relay events inspected for any one persisted request.
///
/// This bounds work for legacy records whose safe time range starts at the
/// Unix epoch without introducing another time cutoff that could hide a
/// still-pending payment.
const NOSTR_MAX_EVENTS_PER_REQUEST: usize = 256;

fn payment_events_filter(pubkey: PublicKey, since: Timestamp, until: Option<Timestamp>) -> Filter {
    let filter = Filter::new()
        .pubkey(pubkey)
        .kind(Kind::GiftWrap)
        .since(since)
        .limit(NOSTR_MAX_EVENTS_PER_REQUEST);

    match until {
        Some(until) => filter.until(until),
        None => filter,
    }
}

fn next_history_until(
    since: Timestamp,
    event_timestamps: impl IntoIterator<Item = Timestamp>,
    page_len: usize,
) -> u64 {
    if page_len < NOSTR_MAX_EVENTS_PER_REQUEST {
        return since.as_secs();
    }

    event_timestamps
        .into_iter()
        .map(|timestamp| timestamp.as_secs())
        .min()
        .unwrap_or_else(|| since.as_secs())
        .max(since.as_secs())
}

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
                let mut info: StoredNostrWaitInfo = serde_json::from_slice(&val)?;

                let secret_key = SecretKey::from_str(&info.secret_key_hex)?;
                let keys = Keys::new(secret_key);
                let pubkey = PublicKey::from_hex(&info.pubkey_hex)?;

                let client = nostr_sdk::Client::new(keys);
                for r in &info.relays {
                    client.add_relay(r).await?;
                }
                client.connect().await;

                let since = info.nostr_query_since(Timestamp::now().as_secs());
                let fresh_filter = payment_events_filter(pubkey, since, None);
                let fresh_events = client
                    .fetch_events(fresh_filter, Duration::from_secs(10))
                    .await?
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>();
                let mut events = fresh_events.clone();

                let mut history_page =
                    match info.history_until.filter(|until| *until > since.as_secs()) {
                        Some(until) => {
                            let filter =
                                payment_events_filter(pubkey, since, Some(Timestamp::from(until)));
                            Some(
                                client
                                    .fetch_events(filter, Duration::from_secs(10))
                                    .await?
                                    .iter()
                                    .cloned()
                                    .collect::<Vec<_>>(),
                            )
                        }
                        None => None,
                    };
                let cursor_page = history_page.as_ref().unwrap_or(&fresh_events);
                let next_history_cursor = next_history_until(
                    since,
                    cursor_page.iter().map(|event| event.created_at),
                    cursor_page.len(),
                );
                let next_boundary_event_ids = cursor_page
                    .iter()
                    .filter(|event| event.created_at.as_secs() == next_history_cursor)
                    .map(|event| event.id.to_hex())
                    .collect();
                if let Some(history_page) = history_page.as_mut() {
                    history_page.retain(|event| {
                        !info.history_boundary_event_ids.contains(&event.id.to_hex())
                    });
                }
                if let Some(history_page) = history_page {
                    events.extend(history_page);
                }
                let mut seen_event_ids = HashSet::new();
                events.retain(|event| seen_event_ids.insert(event.id));

                let mut receive_failed = false;
                for event in events {
                    if let Ok(unwrapped) = client.unwrap_gift_wrap(&event).await {
                        if let Ok(payload) = parse_nostr_payment_payload(&unwrapped.rumor.content) {
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
                            let receive_wallet =
                                get_or_create_wallet(wallet_repository, &mint_url, &unit).await?;

                            match receive_wallet
                                .receive(&token_str, ReceiveOptions::default())
                                .await
                            {
                                Ok(amount) => {
                                    if amount > cdk::Amount::ZERO {
                                        println!("Received {} from request {}", amount, key);
                                    }
                                }
                                Err(e) => {
                                    receive_failed |= !e.is_definitive_failure()
                                        && !matches!(&e, cdk::Error::TokenAlreadySpent);
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

                if !receive_failed {
                    info.history_until = Some(next_history_cursor);
                    info.history_boundary_event_ids = next_boundary_event_ids;
                }
                let val = serde_json::to_vec(&info)?;
                wallet
                    .localstore
                    .kv_write("cdk_cli", "pending_nostr_requests", &key, &val)
                    .await?;
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

    #[test]
    fn payment_event_filter_bounds_events_without_an_end_time() {
        let pubkey = Keys::generate().public_key();
        let filter = payment_events_filter(pubkey, Timestamp::from(0), None);
        let value = serde_json::to_value(filter).expect("filter should serialize");

        assert_eq!(
            value.get("limit").and_then(serde_json::Value::as_u64),
            Some(NOSTR_MAX_EVENTS_PER_REQUEST as u64)
        );
        assert!(value.get("until").is_none());
        assert_eq!(
            value.get("since").and_then(serde_json::Value::as_u64),
            Some(0)
        );
    }

    #[test]
    fn history_cursor_moves_backward_one_bounded_page_at_a_time() {
        let since = Timestamp::from(100);
        let timestamps = [Timestamp::from(500), Timestamp::from(300)];

        assert_eq!(
            next_history_until(since, timestamps, NOSTR_MAX_EVENTS_PER_REQUEST),
            300
        );
        assert_eq!(next_history_until(since, timestamps, 2), 100);
    }

    #[test]
    fn historical_filter_has_inclusive_upper_cursor() {
        let pubkey = Keys::generate().public_key();
        let filter =
            payment_events_filter(pubkey, Timestamp::from(100), Some(Timestamp::from(299)));
        let value = serde_json::to_value(filter).expect("filter should serialize");

        assert_eq!(
            value.get("until").and_then(serde_json::Value::as_u64),
            Some(299)
        );
    }
}
