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

use std::sync::Arc;
use std::time::Duration;

use nostr_sdk::{
    Client, Event, EventId, Filter, JsonUtil, Keys, Kind, PublicKey, RelayOptions,
    RelayPoolNotification, RelayUrl, SecretKey, Timestamp, UnsignedEvent,
};
use tokio::sync::{broadcast::error::RecvError, Mutex};
use tokio::task::JoinHandle;
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
    /// Verified ID of the rumor
    pub rumor_id: EventId,
    /// The unwrapped, unsigned rumor (commonly kind `14` for chat/DM payloads)
    pub rumor: UnsignedEvent,
}

/// Strictly validate and unwrap a NIP-17/NIP-59 gift wrap.
///
/// This performs the checks needed at the mobile trust boundary in this order:
/// the outer kind, ID, signature and recipient tag; the seal kind, ID and
/// signature; then the rumor kind, ID and author binding to the seal.
///
/// # Errors
///
/// Returns an error when any envelope layer is malformed, fails cryptographic
/// verification, has the wrong kind, targets another identity, or has an
/// inconsistent author.
pub fn unwrap_gift_wrap(keys: &Keys, gift_wrap: &Event) -> Result<Nip17Event> {
    if gift_wrap.kind != Kind::GiftWrap {
        return Err(Error::InvalidGiftWrap(format!(
            "expected kind {}, got {}",
            Kind::GiftWrap.as_u16(),
            gift_wrap.kind.as_u16()
        )));
    }

    gift_wrap
        .verify()
        .map_err(|e| Error::InvalidGiftWrap(e.to_string()))?;

    let recipient = keys.public_key().to_hex();
    let addressed_to_identity = gift_wrap.tags.iter().any(|tag| {
        let values = tag.as_slice();
        values.first().is_some_and(|value| value == "p")
            && values.get(1).is_some_and(|value| value == &recipient)
    });
    if !addressed_to_identity {
        return Err(Error::WrongRecipient);
    }

    let seal_json = crate::nip44::decrypt(keys.secret_key(), &gift_wrap.pubkey, &gift_wrap.content)
        .map_err(|e| Error::InvalidSeal(e.to_string()))?;
    let seal = Event::from_json(seal_json).map_err(|e| Error::InvalidSeal(e.to_string()))?;

    if seal.kind != Kind::Seal {
        return Err(Error::InvalidSeal(format!(
            "expected kind {}, got {}",
            Kind::Seal.as_u16(),
            seal.kind.as_u16()
        )));
    }
    seal.verify()
        .map_err(|e| Error::InvalidSeal(e.to_string()))?;

    let rumor_json = crate::nip44::decrypt(keys.secret_key(), &seal.pubkey, &seal.content)
        .map_err(|e| Error::InvalidRumor(e.to_string()))?;
    let rumor =
        UnsignedEvent::from_json(rumor_json).map_err(|e| Error::InvalidRumor(e.to_string()))?;

    if rumor.kind != Kind::PrivateDirectMessage {
        return Err(Error::InvalidRumor(format!(
            "expected kind {}, got {}",
            Kind::PrivateDirectMessage.as_u16(),
            rumor.kind.as_u16()
        )));
    }
    let rumor_id = rumor
        .id
        .ok_or_else(|| Error::InvalidRumor("missing event ID".to_string()))?;
    rumor
        .verify_id()
        .map_err(|e| Error::InvalidRumor(e.to_string()))?;

    if rumor.pubkey != seal.pubkey {
        return Err(Error::SenderMismatch);
    }

    Ok(Nip17Event {
        wrap_id: gift_wrap.id,
        wrap_created_at: gift_wrap.created_at,
        sender: seal.pubkey,
        rumor_id,
        rumor,
    })
}

/// Callback receiving unwrapped inbox events
///
/// Implementations must be non-blocking; do any expensive work (token claims,
/// database writes) on a separate task.
pub trait NostrInboxListener: Send + Sync {
    /// Called once per successfully unwrapped gift wrap
    fn on_event(&self, event: Nip17Event);
}

#[derive(Debug)]
struct InboxRun {
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

/// A standing NIP-17 inbox listener for a single Nostr identity.
///
/// Create with [`NostrInbox::new`], then call [`NostrInbox::start`] to spawn
/// the relay pump and [`NostrInbox::stop`] to shut it down.
#[derive(Debug)]
pub struct NostrInbox {
    keys: Keys,
    relays: Vec<RelayUrl>,
    since: Option<Timestamp>,
    run: Mutex<Option<InboxRun>>,
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
        Self::from_keys(Keys::new(secret_key), relays, since)
    }

    /// Create an inbox from an existing Nostr identity.
    ///
    /// This is the preferred constructor when the caller already owns a typed
    /// identity and wants the inbox and other Nostr clients to share it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoRelays`] if `relays` is empty.
    pub fn from_keys(keys: Keys, relays: Vec<RelayUrl>, since: Option<Timestamp>) -> Result<Self> {
        if relays.is_empty() {
            return Err(Error::NoRelays);
        }
        Ok(Self {
            keys,
            relays,
            since,
            run: Mutex::new(None),
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
    /// [`NostrInbox::stop`] is called. Calling `start()` while the inbox is
    /// already running is an idempotent no-op. Use [`NostrInbox::restart`] to
    /// replace the active listener and connection explicitly.
    ///
    /// # Errors
    ///
    /// Returns an error if a relay cannot be added or the subscription cannot
    /// be created.
    pub async fn start(&self, listener: Arc<dyn NostrInboxListener>) -> Result<()> {
        let mut current = self.run.lock().await;
        match current.as_ref() {
            Some(run) if !run.handle.is_finished() => return Ok(()),
            Some(_) => {
                if let Some(run) = current.take() {
                    let _ = run.handle.await;
                }
            }
            None => {}
        }

        *current = Some(self.spawn_run(listener).await?);
        Ok(())
    }

    /// Stop any active run and start a fresh subscription with `listener`.
    ///
    /// The configured `since` floor is reused unchanged, including across
    /// reconnects and explicit restarts.
    ///
    /// # Errors
    ///
    /// Returns an error if a relay cannot be added or the subscription cannot
    /// be created.
    pub async fn restart(&self, listener: Arc<dyn NostrInboxListener>) -> Result<()> {
        let mut current = self.run.lock().await;
        if let Some(run) = current.take() {
            Self::shutdown_run(run).await;
        }
        *current = Some(self.spawn_run(listener).await?);
        Ok(())
    }

    async fn spawn_run(&self, listener: Arc<dyn NostrInboxListener>) -> Result<InboxRun> {
        let cancel = CancellationToken::new();

        let client = Client::new(self.keys.clone());

        for relay in &self.relays {
            // nostr-sdk automatically replays subscriptions after reconnect.
            // Its adaptive retry interval is bounded at 60 seconds; start at
            // two seconds so a mobile connection recovers promptly.
            let options = RelayOptions::new()
                .reconnect(true)
                .retry_interval(Duration::from_secs(2))
                .adjust_retry_interval(true);
            client
                .pool()
                .add_relay(relay.clone(), options)
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

        if let Err(e) = client.subscribe(filter, None).await {
            client.shutdown().await;
            return Err(Error::Subscription(e.to_string()));
        }

        let run_cancel = cancel.clone();
        let keys = self.keys.clone();
        let handle = tokio::spawn(async move {
            loop {
                let notification = tokio::select! {
                    biased;
                    _ = run_cancel.cancelled() => break,
                    notification = notifications.recv() => notification,
                };

                match notification {
                    Ok(RelayPoolNotification::Event { event, .. }) => {
                        match unwrap_gift_wrap(&keys, &event) {
                            Ok(unwrapped) => {
                                // Cancellation can race with validation. Check
                                // once more immediately before crossing the
                                // callback boundary.
                                if run_cancel.is_cancelled() {
                                    break;
                                }
                                listener.on_event(unwrapped);
                            }
                            Err(e) => {
                                // Not encrypted for us or malformed — log and keep going
                                tracing::debug!("inbox: unwrap gift wrap {} failed: {e}", event.id);
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(RecvError::Lagged(skipped)) => {
                        tracing::warn!("inbox: notification stream lagged; skipped {skipped}");
                    }
                    Err(RecvError::Closed) => break,
                }
            }
            client.shutdown().await;
        });

        Ok(InboxRun { cancel, handle })
    }

    async fn shutdown_run(run: InboxRun) {
        run.cancel.cancel();
        let _ = run.handle.await;
    }

    /// Stop listening and wait for the pump and any in-progress callback to
    /// finish. When this method returns, the stopped run cannot invoke another
    /// callback. Calling it while stopped is an idempotent no-op.
    pub async fn stop(&self) {
        let mut current = self.run.lock().await;
        if let Some(run) = current.take() {
            Self::shutdown_run(run).await;
        }
    }

    /// Return whether an inbox pump is currently active.
    pub async fn is_running(&self) -> bool {
        self.run
            .lock()
            .await
            .as_ref()
            .is_some_and(|run| !run.handle.is_finished())
    }
}

impl Drop for NostrInbox {
    /// Dropping the inbox stops the pump: dropping a `CancellationToken` does
    /// not cancel it, and the spawned pump holds its own clone plus the relay
    /// client and listener, so without this the task (and its relay
    /// connections) would leak for the lifetime of the process.
    fn drop(&mut self) {
        if let Ok(mut current) = self.run.try_lock() {
            if let Some(run) = current.take() {
                run.cancel.cancel();
                run.handle.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nostr_sdk::nips::nip44::Version;
    use nostr_sdk::{EventBuilder, Tag};

    use super::*;
    use crate::keys;

    struct NoopListener;

    impl NostrInboxListener for NoopListener {
        fn on_event(&self, _event: Nip17Event) {}
    }

    fn rumor(author: PublicKey, receiver: PublicKey, kind: Kind, content: &str) -> UnsignedEvent {
        let mut rumor = UnsignedEvent::new(
            author,
            Timestamp::from_secs(1_700_000_000),
            kind,
            [Tag::public_key(receiver)],
            content,
        );
        rumor.ensure_id();
        rumor
    }

    fn make_seal(sender: &Keys, receiver: PublicKey, rumor: &UnsignedEvent, kind: Kind) -> Event {
        let content = nostr_sdk::nips::nip44::encrypt(
            sender.secret_key(),
            &receiver,
            rumor.as_json(),
            Version::V2,
        )
        .expect("rumor encrypts");
        EventBuilder::new(kind, content)
            .custom_created_at(Timestamp::from_secs(1_700_000_001))
            .sign_with_keys(sender)
            .expect("seal signs")
    }

    fn wrap_seal(receiver: PublicKey, recipient_tag: PublicKey, seal: &Event) -> Event {
        let ephemeral = Keys::generate();
        let content = nostr_sdk::nips::nip44::encrypt(
            ephemeral.secret_key(),
            &receiver,
            seal.as_json(),
            Version::V2,
        )
        .expect("seal encrypts");
        EventBuilder::new(Kind::GiftWrap, content)
            .tag(Tag::public_key(recipient_tag))
            .custom_created_at(Timestamp::from_secs(1_700_000_002))
            .sign_with_keys(&ephemeral)
            .expect("gift wrap signs")
    }

    fn valid_envelope() -> (Keys, Keys, UnsignedEvent, Event, Event) {
        let sender = Keys::generate();
        let receiver = Keys::generate();
        let rumor = rumor(
            sender.public_key(),
            receiver.public_key(),
            Kind::PrivateDirectMessage,
            "cashuAexample",
        );
        let seal = make_seal(&sender, receiver.public_key(), &rumor, Kind::Seal);
        let wrap = wrap_seal(receiver.public_key(), receiver.public_key(), &seal);
        (sender, receiver, rumor, seal, wrap)
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

    #[test]
    fn strict_unwrap_returns_verified_rumor_and_outer_id() {
        let (sender, receiver, rumor, _seal, wrap) = valid_envelope();

        let unwrapped = unwrap_gift_wrap(&receiver, &wrap).expect("gift wrap unwraps");

        assert_eq!(unwrapped.wrap_id, wrap.id);
        assert_eq!(unwrapped.sender, sender.public_key());
        assert_eq!(unwrapped.rumor_id, rumor.id.expect("rumor ID"));
        assert_eq!(unwrapped.rumor.content, "cashuAexample");
    }

    #[test]
    fn strict_unwrap_rejects_invalid_outer_id_and_signature() {
        let (_sender, receiver, _rumor, _seal, wrap) = valid_envelope();

        let mut invalid_id = wrap.clone();
        invalid_id.content.push('x');
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &invalid_id),
            Err(Error::InvalidGiftWrap(_))
        ));

        let mut invalid_signature = wrap;
        invalid_signature.sig = EventBuilder::text_note("other")
            .sign_with_keys(&Keys::generate())
            .expect("other event signs")
            .sig;
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &invalid_signature),
            Err(Error::InvalidGiftWrap(_))
        ));
    }

    #[test]
    fn strict_unwrap_rejects_invalid_seal_id_and_signature() {
        let (_sender, receiver, _rumor, seal, _wrap) = valid_envelope();

        let mut invalid_id = seal.clone();
        invalid_id.content.push('x');
        let wrap = wrap_seal(receiver.public_key(), receiver.public_key(), &invalid_id);
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &wrap),
            Err(Error::InvalidSeal(_))
        ));

        let mut invalid_signature = seal;
        invalid_signature.sig = EventBuilder::text_note("other")
            .sign_with_keys(&Keys::generate())
            .expect("other event signs")
            .sig;
        let wrap = wrap_seal(
            receiver.public_key(),
            receiver.public_key(),
            &invalid_signature,
        );
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &wrap),
            Err(Error::InvalidSeal(_))
        ));
    }

    #[test]
    fn strict_unwrap_rejects_incorrect_kinds() {
        let (sender, receiver, valid_rumor, _seal, wrap) = valid_envelope();

        let mut wrong_outer_kind = wrap;
        wrong_outer_kind.kind = Kind::TextNote;
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &wrong_outer_kind),
            Err(Error::InvalidGiftWrap(_))
        ));

        let wrong_seal = make_seal(&sender, receiver.public_key(), &valid_rumor, Kind::TextNote);
        let wrap = wrap_seal(receiver.public_key(), receiver.public_key(), &wrong_seal);
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &wrap),
            Err(Error::InvalidSeal(_))
        ));

        let wrong_rumor = rumor(
            sender.public_key(),
            receiver.public_key(),
            Kind::TextNote,
            "not a NIP-17 rumor",
        );
        let seal = make_seal(&sender, receiver.public_key(), &wrong_rumor, Kind::Seal);
        let wrap = wrap_seal(receiver.public_key(), receiver.public_key(), &seal);
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &wrap),
            Err(Error::InvalidRumor(_))
        ));
    }

    #[test]
    fn strict_unwrap_rejects_bad_rumor_id_and_author_mismatch() {
        let (sender, receiver, mut invalid_id, _seal, _wrap) = valid_envelope();
        invalid_id.content.push('x');
        let seal = make_seal(&sender, receiver.public_key(), &invalid_id, Kind::Seal);
        let wrap = wrap_seal(receiver.public_key(), receiver.public_key(), &seal);
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &wrap),
            Err(Error::InvalidRumor(_))
        ));

        let impersonated = Keys::generate();
        let mismatched = rumor(
            impersonated.public_key(),
            receiver.public_key(),
            Kind::PrivateDirectMessage,
            "spoofed",
        );
        let seal = make_seal(&sender, receiver.public_key(), &mismatched, Kind::Seal);
        let wrap = wrap_seal(receiver.public_key(), receiver.public_key(), &seal);
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &wrap),
            Err(Error::SenderMismatch)
        ));
    }

    #[test]
    fn strict_unwrap_requires_recipient_tag_and_rumor_id() {
        let (sender, receiver, mut rumor, seal, _wrap) = valid_envelope();
        let other = Keys::generate();
        let wrong_recipient_tag = wrap_seal(receiver.public_key(), other.public_key(), &seal);
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &wrong_recipient_tag),
            Err(Error::WrongRecipient)
        ));

        rumor.id = None;
        let seal = make_seal(&sender, receiver.public_key(), &rumor, Kind::Seal);
        let wrap = wrap_seal(receiver.public_key(), receiver.public_key(), &seal);
        assert!(matches!(
            unwrap_gift_wrap(&receiver, &wrap),
            Err(Error::InvalidRumor(_))
        ));
    }

    #[tokio::test]
    async fn start_stop_and_restart_are_idempotent() {
        let secret = keys::generate_secret_key();
        let relay = RelayUrl::parse("ws://127.0.0.1:1").expect("valid relay url");
        let inbox = NostrInbox::new(secret, vec![relay], None).expect("inbox");

        inbox
            .start(Arc::new(NoopListener))
            .await
            .expect("first start");
        assert!(inbox.is_running().await);
        let first_cancel = inbox
            .run
            .lock()
            .await
            .as_ref()
            .expect("active run")
            .cancel
            .clone();

        inbox
            .start(Arc::new(NoopListener))
            .await
            .expect("second start is a no-op");
        assert!(!first_cancel.is_cancelled());

        inbox
            .restart(Arc::new(NoopListener))
            .await
            .expect("restart");
        assert!(first_cancel.is_cancelled());
        assert!(inbox.is_running().await);

        inbox.stop().await;
        assert!(!inbox.is_running().await);
        inbox.stop().await;

        inbox
            .start(Arc::new(NoopListener))
            .await
            .expect("start after stop");
        assert!(inbox.is_running().await);
        inbox.stop().await;
    }

    #[tokio::test]
    async fn drop_cancels_running_pump() {
        let secret = keys::generate_secret_key();
        let relay = RelayUrl::parse("ws://127.0.0.1:1").expect("valid relay url");
        let inbox = NostrInbox::new(secret, vec![relay], None).expect("inbox");

        inbox.start(Arc::new(NoopListener)).await.expect("start");
        let token = inbox
            .run
            .lock()
            .await
            .as_ref()
            .expect("active run")
            .cancel
            .clone();
        drop(inbox);

        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn stop_waits_for_run_and_prevents_later_work() {
        let secret = keys::generate_secret_key();
        let relay = RelayUrl::parse("ws://127.0.0.1:1").expect("valid relay url");
        let inbox = NostrInbox::new(secret, vec![relay], None).expect("inbox");
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let completions = Arc::new(AtomicUsize::new(0));
        let task_completions = completions.clone();
        let handle = tokio::spawn(async move {
            task_cancel.cancelled().await;
            tokio::task::yield_now().await;
            task_completions.fetch_add(1, Ordering::SeqCst);
        });
        *inbox.run.lock().await = Some(InboxRun { cancel, handle });

        inbox.stop().await;
        assert_eq!(completions.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(completions.load(Ordering::SeqCst), 1);
    }
}
