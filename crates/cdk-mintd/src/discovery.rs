//! Optional startup publication of NIP-87 mint announcements.

use std::fmt;
use std::time::Duration;

use cdk_nostr::nip87::{mint_announcement, validate_announcement};
use cdk_nostr::nostr::prelude::{Event, FinalizeEvent, Keys, RelayUrl};
use cdk_nostr::nostr_sdk::prelude::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::broadcast;

/// NIP-87 startup publication settings. Omit the section to disable discovery.
#[derive(Clone, Serialize, Deserialize)]
pub struct MintDiscovery {
    /// Stable Nostr signing key, stored as an `env:` or `file:` secret reference.
    pub secret_key: String,
    /// Relays on which to announce the mint; at least one is required.
    pub relays: Vec<RelayUrl>,
    /// Explicit NIP-87 network: mainnet, testnet, signet or regtest.
    pub network: String,
}

impl fmt::Debug for MintDiscovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MintDiscovery")
            .field("secret_key", &"[redacted]")
            .field("relays", &self.relays)
            .field("network", &self.network)
            .finish()
    }
}

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("mint discovery requires at least one relay")]
    NoRelays,
    #[error("invalid mint discovery Nostr secret key")]
    InvalidKey,
    #[error(transparent)]
    Announcement(#[from] cdk_nostr::nip87::Error),
    #[error("could not sign mint discovery announcement")]
    Signing,
}

impl MintDiscovery {
    pub(crate) fn validate(&self, mint_url: &str) -> Result<(), Error> {
        if self.relays.is_empty() {
            return Err(Error::NoRelays);
        }
        Keys::parse(self.secret_key.trim()).map_err(|_| Error::InvalidKey)?;
        validate_announcement(mint_url, &self.network)?;
        Ok(())
    }

    pub(crate) fn announcement(
        &self,
        mint_url: &str,
        info: &cdk::nuts::MintInfo,
    ) -> Result<Event, Error> {
        self.validate(mint_url)?;
        let keys = Keys::parse(self.secret_key.trim()).map_err(|_| Error::InvalidKey)?;
        mint_announcement(mint_url, info, &self.network)?
            .finalize(&keys)
            .map_err(|_| Error::Signing)
    }
}

/// Retry only unacknowledged relays, reusing the signed event. Each attempt
/// owns a short-lived client; dropping it also stops its reconnect tasks.
pub(crate) async fn publish(
    event: Event,
    mut pending: Vec<RelayUrl>,
    mut shutdown: broadcast::Receiver<()>,
) {
    // Nostr timestamps have one-second precision. Wait a full second after
    // signing before sending: a subsequent daemon restart then signs with a
    // strictly newer timestamp instead of losing the relay's event-ID tie-break.
    // This future runs alongside HTTP serving and remains cancellable.
    tokio::select! {
        biased;
        _ = shutdown.recv() => return,
        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
    }
    let mut delay = Duration::from_secs(60);
    pending.sort_unstable();
    pending.dedup();
    loop {
        let client = Client::new();
        let attempt = async {
            for relay in &pending {
                client.add_relay(relay.clone()).await?;
            }
            client.try_connect().timeout(Duration::from_secs(10)).await;
            client.send_event(&event).await
        };
        tokio::select! {
            biased;
            _ = shutdown.recv() => return,
            result = tokio::time::timeout(Duration::from_secs(30), attempt) => {
                match result {
                    Ok(Ok(output)) => {
                        pending.retain(|relay| !output.success.contains_key(relay));
                    }
                    _ => tracing::warn!("Mint discovery publication attempt failed"),
                }
            }
        }
        // SDK drop shuts down the relay pool without delaying daemon shutdown.
        drop(client);
        if pending.is_empty() {
            tracing::info!(event_id = %event.id, "Mint discovery announcement acknowledged by all relays");
            return;
        }
        tracing::warn!(
            remaining = pending.len(),
            retry_seconds = delay.as_secs(),
            "Retrying mint discovery publication"
        );
        tokio::select! {
            biased;
            _ = shutdown.recv() => return,
            _ = tokio::time::sleep(delay) => {}
        }
        delay = (delay * 2).min(Duration::from_secs(3600));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use axum::extract::ws::{Message, WebSocketUpgrade};
    use axum::routing::get;
    use axum::Router;
    use cdk_nostr::nostr::prelude::{EventBuilder, Kind};
    use tokio::sync::mpsc;

    use super::*;

    fn event() -> Event {
        EventBuilder::new(Kind::Custom(38172), "")
            .finalize(&Keys::generate())
            .expect("signed event")
    }

    async fn relay(
        accept_after: usize,
    ) -> (
        RelayUrl,
        mpsc::Receiver<Option<Event>>,
        tokio::task::JoinHandle<()>,
    ) {
        let (tx, rx) = mpsc::channel(8);
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/",
            get(move |ws: WebSocketUpgrade| {
                let tx = tx.clone();
                let attempts = Arc::clone(&attempts);
                async move {
                    ws.on_upgrade(move |mut socket| async move {
                        while let Some(Ok(message)) = socket.recv().await {
                            let text = match message {
                                Message::Text(text) => text,
                                Message::Close(_) => break,
                                _ => continue,
                            };
                            let value: serde_json::Value =
                                serde_json::from_str(&text).expect("message");
                            if value[0] != "EVENT" {
                                continue;
                            }
                            let event: Event =
                                serde_json::from_value(value[1].clone()).expect("event");
                            event.verify().expect("valid signature");
                            let accepted = attempts.fetch_add(1, Ordering::SeqCst) >= accept_after;
                            let reply = serde_json::json!(["OK", event.id.to_hex(), accepted, ""]);
                            socket
                                .send(Message::Text(reply.to_string().into()))
                                .await
                                .expect("ack");
                            tx.send(Some(event)).await.expect("received event");
                        }
                        let _ = tx.send(None).await;
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let url = RelayUrl::parse(&format!("ws://{}", listener.local_addr().expect("address")))
            .expect("URL");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("relay server");
        });
        (url, rx, handle)
    }

    #[tokio::test]
    async fn publishes_signed_event_and_finishes_after_acknowledgement() {
        let (url, mut received, relay) = relay(0).await;
        let event = event();
        let (shutdown, rx) = broadcast::channel(1);
        let started = tokio::time::Instant::now();
        let publisher = tokio::spawn(publish(event.clone(), vec![url.clone(), url], rx));
        let delivered = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("delivery timeout")
            .expect("delivered event");
        assert_eq!(delivered, Some(event));
        assert!(started.elapsed() >= Duration::from_secs(1));
        tokio::time::timeout(Duration::from_secs(5), publisher)
            .await
            .expect("publisher stopped")
            .expect("publisher task");
        drop(shutdown);
        relay.abort();
    }

    #[tokio::test]
    async fn partial_rejection_keeps_retry_pending_and_shutdown_cancels_it() {
        let (accepted, mut accepted_events, accepted_relay) = relay(0).await;
        let (rejected, mut rejected_events, rejected_relay) = relay(usize::MAX).await;
        let (shutdown, rx) = broadcast::channel(1);
        let publisher = tokio::spawn(publish(event(), vec![accepted, rejected], rx));
        tokio::time::timeout(Duration::from_secs(5), async {
            accepted_events
                .recv()
                .await
                .expect("accepted event")
                .expect("event");
            rejected_events
                .recv()
                .await
                .expect("rejected event")
                .expect("event");
            assert!(accepted_events
                .recv()
                .await
                .expect("accepted relay disconnected")
                .is_none());
            assert!(rejected_events
                .recv()
                .await
                .expect("rejected relay disconnected")
                .is_none());
        })
        .await
        .expect("both relays received event");
        assert!(
            !publisher.is_finished(),
            "a rejected event must remain pending"
        );
        shutdown.send(()).expect("shutdown");
        tokio::time::timeout(Duration::from_secs(1), publisher)
            .await
            .expect("prompt shutdown")
            .expect("publisher task");
        accepted_relay.abort();
        rejected_relay.abort();
    }

    #[tokio::test]
    async fn retries_only_failed_relays_with_the_same_signed_event() {
        let (accepted, mut accepted_events, accepted_relay) = relay(0).await;
        let (retry, mut retry_events, retry_relay) = relay(1).await;
        let (shutdown, rx) = broadcast::channel(1);
        let event = event();
        let publisher = tokio::spawn(publish(event.clone(), vec![accepted, retry], rx));
        tokio::time::timeout(Duration::from_secs(5), async {
            assert_eq!(
                accepted_events.recv().await.expect("delivery"),
                Some(event.clone())
            );
            assert_eq!(
                retry_events.recv().await.expect("first attempt"),
                Some(event.clone())
            );
            assert!(accepted_events.recv().await.expect("disconnect").is_none());
            assert!(retry_events.recv().await.expect("disconnect").is_none());
        })
        .await
        .expect("first attempt finished");
        // Both clients have disconnected and publication is now in backoff.
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::time::resume();
        let retried = tokio::time::timeout(Duration::from_secs(5), retry_events.recv())
            .await
            .expect("retry timeout")
            .expect("retry");
        assert_eq!(retried, Some(event));
        tokio::time::timeout(Duration::from_secs(5), publisher)
            .await
            .expect("publisher stopped")
            .expect("publisher task");
        assert!(
            accepted_events.try_recv().is_err(),
            "acknowledged relay must not be retried"
        );
        drop(shutdown);
        accepted_relay.abort();
        retry_relay.abort();
    }

    #[tokio::test]
    async fn already_cancelled_publication_does_not_connect() {
        let (url, mut received, relay) = relay(0).await;
        let (shutdown, rx) = broadcast::channel(1);
        shutdown.send(()).expect("shutdown");
        publish(event(), vec![url], rx).await;
        assert!(received.try_recv().is_err());
        relay.abort();
    }

    #[test]
    fn validates_configuration_without_disclosing_the_key() {
        let secret = Keys::generate().secret_key().to_secret_hex();
        let mut config = MintDiscovery {
            secret_key: secret.clone(),
            relays: vec![RelayUrl::parse("wss://relay.example").expect("URL")],
            network: "regtest".to_owned(),
        };
        assert!(!format!("{config:?}").contains(&secret));
        config
            .validate("https://mint.example")
            .expect("valid configuration");
        config.network = "bitcoin".to_owned();
        assert!(config.validate("https://mint.example").is_err());
        config.network = "regtest".to_owned();
        config.secret_key = "invalid secret".to_owned();
        assert_eq!(
            config
                .validate("https://mint.example")
                .expect_err("key")
                .to_string(),
            "invalid mint discovery Nostr secret key"
        );
        config.relays.clear();
        assert!(matches!(
            config.validate("https://mint.example"),
            Err(Error::NoRelays)
        ));
    }
}
