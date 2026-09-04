//! End-to-end NIP-17 inbox listener test.
//!
//! Spins up a local `nostr-rs-relay`, publishes a NIP-59 gift wrap carrying a
//! kind `14` rumor to a receiver identity, and asserts that
//! [`cdk_nostr::inbox::NostrInbox`] delivers the unwrapped rumor to its
//! listener — including after a stop/start restart.
//!
//! ## Requirements
//!
//! - `nostr-rs-relay` must be on `PATH` (provided by the Nix `regtest`
//!   devShell); the test skips gracefully otherwise.

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use cdk_nostr::inbox::{Nip17Event, NostrInbox, NostrInboxListener};
use nostr::prelude::{
    EventId, Filter, FinalizeEvent, Keys, Kind, PrivateDirectMessageBuilder, RelayUrl,
};
use nostr_sdk::prelude::{Client as NostrClient, SignerAuthenticator};
use tokio::sync::mpsc;

/// Manage a local `nostr-rs-relay` subprocess on a free port.
struct NostrRelay {
    child: Option<Child>,
    port: u16,
}

impl NostrRelay {
    /// Start a local `nostr-rs-relay` on a free TCP port.
    ///
    /// Returns `None` if `nostr-rs-relay` is not on `PATH` (e.g. running
    /// outside the Nix devShell), so the test can be skipped.
    fn start() -> Option<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        drop(listener);

        let db_dir = std::env::temp_dir().join(format!("nostr-relay-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&db_dir).ok()?;

        let config_path = db_dir.join("config.toml");
        let config = format!(
            r#"[network]
port = {port}
address = "127.0.0.1"
"#
        );
        std::fs::write(&config_path, config).ok()?;

        let child = Command::new("nostr-rs-relay")
            .arg("--config")
            .arg(&config_path)
            .arg("--db")
            .arg(&db_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;

        Some(Self {
            child: Some(child),
            port,
        })
    }

    fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }
}

impl Drop for NostrRelay {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Poll the relay's TCP port until it accepts connections or the timeout expires.
async fn wait_for_relay(port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until the gift wrap is fetchable from the relay, so the test does not
/// depend on publish/subscribe timing.
async fn wait_for_gift_wrap(relay_url: &RelayUrl, wrap_id: EventId, timeout: Duration) -> bool {
    let client = NostrClient::new();
    if client.add_relay(relay_url.clone()).await.is_err() {
        return false;
    }
    client.connect().await;

    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        let events = client
            .fetch_events(Filter::new().id(wrap_id))
            .timeout(Duration::from_secs(2))
            .await;
        match events {
            Ok(events) if !events.is_empty() => return true,
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

/// Publish a gift wrap carrying `content` as a kind `14` rumor to `receiver`.
async fn publish_gift_wrap(
    relay_url: &RelayUrl,
    sender: &Keys,
    receiver: &Keys,
    content: &str,
) -> EventId {
    let client = NostrClient::builder()
        .authenticator(SignerAuthenticator::new(sender.clone()))
        .build();
    client
        .add_relay(relay_url.clone())
        .await
        .expect("add relay");
    client.connect().await;

    let gift_wrap = PrivateDirectMessageBuilder::new(receiver.public_key(), content)
        .finalize(sender)
        .expect("build gift wrap");
    let output = client
        .send_event(&gift_wrap)
        .broadcast()
        .await
        .expect("publish gift wrap");

    output.value
}

/// Listener that forwards events into a channel for assertions.
struct ChannelListener {
    tx: mpsc::UnboundedSender<Nip17Event>,
}

impl NostrInboxListener for ChannelListener {
    fn on_event(&self, event: Nip17Event) {
        let _ = self.tx.send(event);
    }
}

/// Receive events until `wrap_id` arrives (relay replays stored gift wraps on
/// a fresh subscription, so already-seen wraps may be delivered again first).
async fn recv_until(
    rx: &mut mpsc::UnboundedReceiver<Nip17Event>,
    wrap_id: EventId,
    timeout: Duration,
) -> Nip17Event {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let event = tokio::time::timeout(remaining, rx.recv())
            .await
            .expect("inbox delivers gift wrap within timeout")
            .expect("channel open");
        if event.wrap_id == wrap_id {
            return event;
        }
    }
}

#[tokio::test]
async fn nip17_inbox_receives_unwrapped_rumor_and_survives_restart() {
    let Some(relay) = NostrRelay::start() else {
        eprintln!("nostr-rs-relay not on PATH; skipping nip17_inbox_e2e");
        return;
    };
    assert!(
        wait_for_relay(relay.port, Duration::from_secs(15)).await,
        "relay did not start listening"
    );
    let relay_url = RelayUrl::parse(&relay.ws_url()).expect("valid relay url");

    let sender = Keys::generate();
    let receiver = Keys::generate();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let inbox = NostrInbox::new(receiver.secret_key().clone(), vec![relay_url.clone()], None)
        .expect("create inbox");
    assert_eq!(inbox.pubkey(), receiver.public_key());

    // First delivery: start the inbox, then publish.
    inbox
        .start(Arc::new(ChannelListener { tx: tx.clone() }))
        .await
        .expect("start inbox");

    let content_one =
        r#"{"id":"request-one","mint":"http://localhost:3338","unit":"sat","proofs":[]}"#;
    let wrap_one = publish_gift_wrap(&relay_url, &sender, &receiver, content_one).await;
    assert!(
        wait_for_gift_wrap(&relay_url, wrap_one, Duration::from_secs(10)).await,
        "gift wrap one not stored by relay"
    );

    let event = recv_until(&mut rx, wrap_one, Duration::from_secs(15)).await;
    assert_eq!(event.wrap_id, wrap_one);
    assert_eq!(event.sender, sender.public_key());
    assert_eq!(event.rumor.kind, Kind::PrivateDirectMessage);
    assert_eq!(event.rumor.pubkey, sender.public_key());
    assert!(
        event
            .rumor
            .tags
            .public_keys()
            .any(|public_key| public_key == receiver.public_key()),
        "rumor must identify the receiver"
    );
    assert_eq!(event.rumor.content, content_one);
    // NIP-59: rumors carry an id (ensured by make_seal) but are unsigned
    assert!(event.rumor.id.is_some());

    // Restart: stop, then start again; a wrap published meanwhile must still
    // be delivered (the relay stores it and the fresh subscription replays
    // the window).
    inbox.stop();

    let content_two =
        r#"{"id":"request-two","mint":"http://localhost:3338","unit":"sat","proofs":[]}"#;
    let wrap_two = publish_gift_wrap(&relay_url, &sender, &receiver, content_two).await;
    assert!(
        wait_for_gift_wrap(&relay_url, wrap_two, Duration::from_secs(10)).await,
        "gift wrap two not stored by relay"
    );

    inbox
        .start(Arc::new(ChannelListener { tx }))
        .await
        .expect("restart inbox");

    let event = recv_until(&mut rx, wrap_two, Duration::from_secs(15)).await;
    assert_eq!(event.wrap_id, wrap_two);
    assert_eq!(event.rumor.content, content_two);

    inbox.stop();
}
