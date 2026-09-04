//! Standing NIP-17 inbox listener
//!
//! Subscribes a set of relays for NIP-59 gift wraps (kind `1059`) addressed to
//! a Nostr identity, unwraps them (two layers of NIP-44) and delivers the
//! inner rumor to a [`NostrInboxListener`] callback. The relay pool reconnects
//! automatically; relays may re-deliver events after a reconnect, so consumers
//! must de-duplicate by [`Nip17Event::wrap_id`].
//!
//! The listener deliberately performs no further protocol interpretation: the
//! rumor's kind and content are for the consumer to handle (e.g. a NUT-18
//! payment request payload for kind `14` rumors).

use std::sync::{Arc, Mutex};

use nostr_sdk::prelude::{
    Client, ClientNotification, EventId, Filter, Keys, Kind, PublicKey, RelayUrl, SecretKey,
    SignerAuthenticator, StreamExt, Timestamp, UnsignedEvent, UnwrappedGift,
};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};

/// A gift wrap that was addressed to the inbox identity and successfully
/// unwrapped
#[derive(Debug, Clone)]
pub struct Nip17Event {
    /// ID of the (ephemeral) kind `1059` gift wrap event — use it to
    /// de-duplicate deliveries across relay reconnects and restarts
    pub wrap_id: EventId,
    /// `created_at` of the gift wrap (NIP-59 randomizes/backdates it)
    pub wrap_created_at: Timestamp,
    /// Author of the verified seal — the real sender of the rumor
    pub sender: PublicKey,
    /// The unwrapped, unsigned rumor (commonly kind `14` for chat/DM payloads)
    pub rumor: UnsignedEvent,
}

/// Callback receiving unwrapped inbox events
///
/// Implementations must be non-blocking; do any expensive work (token claims,
/// database writes) on a separate task.
pub trait NostrInboxListener: Send + Sync {
    /// Called once per successfully unwrapped gift wrap
    fn on_event(&self, event: Nip17Event);
}

/// A standing NIP-17 inbox listener for a single Nostr identity
///
/// Create with [`NostrInbox::new`], then call [`NostrInbox::start`] to spawn
/// the relay pump and [`NostrInbox::stop`] to shut it down.
#[derive(Debug)]
pub struct NostrInbox {
    keys: Keys,
    relays: Vec<RelayUrl>,
    since: Option<Timestamp>,
    /// Token of the current run. `CancellationToken::cancel()` is permanent,
    /// so `start()` replaces it with a fresh one — otherwise a pump started
    /// after `stop()` would observe the stale cancellation and exit
    /// immediately.
    cancel: Mutex<CancellationToken>,
}

impl NostrInbox {
    /// Create a new inbox listener
    ///
    /// # Arguments
    ///
    /// * `secret_key` - The identity's secret key; gift wraps addressed to its
    ///   public key are unwrapped with it
    /// * `relays` - Relays to subscribe; must be non-empty
    /// * `since` - Optional lower bound for the relay `since` filter. Because
    ///   NIP-59 backdates gift wraps, pick a generous lookback window instead
    ///   of "now".
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoRelays`] if `relays` is empty.
    pub fn new(
        secret_key: SecretKey,
        relays: Vec<RelayUrl>,
        since: Option<Timestamp>,
    ) -> Result<Self> {
        if relays.is_empty() {
            return Err(Error::NoRelays);
        }
        Ok(Self {
            keys: Keys::new(secret_key),
            relays,
            since,
            cancel: Mutex::new(CancellationToken::new()),
        })
    }

    /// Public key of the inbox identity
    pub fn pubkey(&self) -> PublicKey {
        self.keys.public_key()
    }

    /// Connect to the relays, activate the subscription and spawn the
    /// background pump that delivers events to `listener`
    ///
    /// Returns once the subscription is active. Events are delivered until
    /// [`NostrInbox::stop`] is called. `start()` replaces any previous run:
    /// calling it on an already-running inbox stops the old pump before
    /// starting the new one, and restarting after [`NostrInbox::stop`] works
    /// as expected.
    ///
    /// # Panics
    ///
    /// Panics if the internal cancel-token mutex is poisoned.
    ///
    /// # Errors
    ///
    /// Returns an error if a relay cannot be added or the subscription cannot
    /// be created.
    pub async fn start(&self, listener: Arc<dyn NostrInboxListener>) -> Result<()> {
        // Arm a fresh token for this run, stopping any previous run first. A
        // cancelled token can never be "un-cancelled", so reusing it would
        // make the new pump exit immediately.
        let cancel = {
            let mut current = self
                .cancel
                .lock()
                .expect("inbox cancel token mutex poisoned");
            current.cancel();
            *current = CancellationToken::new();
            current.clone()
        };

        let client = Client::builder()
            .authenticator(SignerAuthenticator::new(self.keys.clone()))
            .build();

        for relay in &self.relays {
            client
                .add_relay(relay.clone())
                .await
                .map_err(|e| Error::Relay(format!("add relay {relay}: {e}")))?;
        }

        client.connect().await;

        let mut filter = Filter::new()
            .kind(Kind::GiftWrap)
            .pubkey(self.keys.public_key());
        if let Some(since) = self.since {
            filter = filter.since(since);
        }

        // Take the notification stream before subscribing so no event can slip
        // between subscription creation and the receive loop.
        let mut notifications = client.notifications();

        client
            .subscribe(filter)
            .await
            .map_err(|e| Error::Subscription(e.to_string()))?;

        let keys = self.keys.clone();
        tokio::spawn(async move {
            loop {
                let notification = tokio::select! {
                    _ = cancel.cancelled() => break,
                    notification = notifications.next() => notification,
                };

                match notification {
                    Some(ClientNotification::Event { event, .. }) => {
                        // Defense in depth: never trust a relay to honor the filter.
                        if event.kind != Kind::GiftWrap {
                            continue;
                        }
                        match UnwrappedGift::from_gift_wrap_async(&keys, &event).await {
                            Ok(unwrapped) => listener.on_event(Nip17Event {
                                wrap_id: event.id,
                                wrap_created_at: event.created_at,
                                sender: unwrapped.sender,
                                rumor: unwrapped.rumor,
                            }),
                            Err(e) => {
                                // Not encrypted for us or malformed — log and keep going
                                tracing::debug!("inbox: unwrap gift wrap {} failed: {e}", event.id);
                            }
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            client.disconnect().await;
        });

        Ok(())
    }

    /// Stop listening: cancels the pump and disconnects from the relays
    ///
    /// # Panics
    ///
    /// Panics if the internal cancel-token mutex is poisoned.
    pub fn stop(&self) {
        self.cancel
            .lock()
            .expect("inbox cancel token mutex poisoned")
            .cancel();
    }
}

impl Drop for NostrInbox {
    /// Dropping the inbox stops the pump: dropping a `CancellationToken` does
    /// not cancel it, and the spawned pump holds its own clone plus the relay
    /// client and listener, so without this the task (and its relay
    /// connections) would leak for the lifetime of the process.
    fn drop(&mut self) {
        if let Ok(token) = self.cancel.lock() {
            token.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys;

    struct NoopListener;

    impl NostrInboxListener for NoopListener {
        fn on_event(&self, _event: Nip17Event) {}
    }

    #[test]
    fn new_requires_at_least_one_relay() {
        let secret = keys::generate_secret_key();
        let result = NostrInbox::new(secret, Vec::new(), None);
        assert!(matches!(result, Err(Error::NoRelays)));
    }

    #[test]
    fn pubkey_matches_secret_key() {
        let secret = keys::parse_secret_key(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .expect("valid secret key");
        let relay = RelayUrl::parse("wss://relay.example.com").expect("valid relay url");
        let inbox = NostrInbox::new(secret, vec![relay], None).expect("inbox");
        assert_eq!(
            inbox.pubkey().to_hex(),
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        );
    }

    /// Regression test: a `CancellationToken` stays cancelled forever, so a
    /// pump started after `stop()` used to exit immediately while `start()`
    /// still reported success. `start()` must arm a fresh token per run.
    #[tokio::test]
    async fn restart_after_stop_arms_a_fresh_token() {
        let secret = keys::generate_secret_key();
        // Unreachable relay: connect/subscribe are best-effort in the pool,
        // so `start()` still succeeds without network access.
        let relay = RelayUrl::parse("ws://127.0.0.1:1").expect("valid relay url");
        let inbox = NostrInbox::new(secret, vec![relay], None).expect("inbox");

        inbox
            .start(Arc::new(NoopListener))
            .await
            .expect("first start");
        inbox.stop();
        assert!(inbox.cancel.lock().expect("mutex").is_cancelled());

        inbox.start(Arc::new(NoopListener)).await.expect("restart");
        assert!(!inbox.cancel.lock().expect("mutex").is_cancelled());

        inbox.stop();
    }

    /// Dropping a running inbox must cancel the pump's token: dropping a
    /// `CancellationToken` alone does not cancel it, and the pump holds its
    /// own clone.
    #[tokio::test]
    async fn drop_cancels_running_pump() {
        let secret = keys::generate_secret_key();
        let relay = RelayUrl::parse("ws://127.0.0.1:1").expect("valid relay url");
        let inbox = NostrInbox::new(secret, vec![relay], None).expect("inbox");

        // Grab the run's token before dropping the inbox.
        let token = inbox.cancel.lock().expect("mutex").clone();
        inbox.start(Arc::new(NoopListener)).await.expect("start");
        drop(inbox);

        assert!(token.is_cancelled());
    }
}
