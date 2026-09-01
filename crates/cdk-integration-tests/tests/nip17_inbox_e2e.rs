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

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use cdk_nostr::inbox::{Nip17Event, NostrInbox, NostrInboxListener};
use nostr_sdk::{Client as NostrClient, EventBuilder, EventId, Filter, Keys, Kind, RelayUrl};
use tokio::sync::mpsc;

/// Manage a local `nostr-rs-relay` subprocess on a free port.
struct NostrRelay {
    child: Option<Child>,
    port: u16,
    config_path: PathBuf,
    db_dir: PathBuf,
}

impl NostrRelay {
    fn spawn_child(config_path: &Path, db_dir: &Path) -> io::Result<Child> {
        Command::new("nostr-rs-relay")
            .arg("--config")
            .arg(config_path)
            .arg("--db")
            .arg(db_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
    }

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

        let child = Self::spawn_child(&config_path, &db_dir).ok()?;

        Some(Self {
            child: Some(child),
            port,
            config_path,
            db_dir,
        })
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn restart(&mut self) -> io::Result<()> {
        self.stop();
        self.child = Some(Self::spawn_child(&self.config_path, &self.db_dir)?);
        Ok(())
    }

    fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }
}

impl Drop for NostrRelay {
    fn drop(&mut self) {
        self.stop();
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
    let client = NostrClient::new(Keys::generate());
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
            .fetch_events(Filter::new().id(wrap_id), Duration::from_secs(2))
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
    let client = NostrClient::new(sender.clone());
    client
        .add_relay(relay_url.clone())
        .await
        .expect("add relay");
    client.connect().await;

    let rumor = EventBuilder::new(Kind::from_u16(14), content).build(sender.public_key());
    let output = client
        .gift_wrap_to(vec![relay_url.clone()], &receiver.public_key(), rumor, None)
        .await
        .expect("publish gift wrap");

    output.val
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
    let Some(mut relay) = NostrRelay::start() else {
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
    assert_eq!(event.rumor.kind, Kind::from_u16(14));
    assert_eq!(event.rumor.pubkey, sender.public_key());
    assert_eq!(event.rumor.content, content_one);
    // NIP-59 rumors are unsigned but their ID is present and was verified by
    // the inbox before delivery.
    assert_eq!(event.rumor.id, Some(event.rumor_id));

    // Relay reconnect: interrupt the transport without restarting the inbox.
    // nostr-sdk must reconnect with its bounded adaptive backoff and replay the
    // same fixed subscription floor.
    relay.restart().expect("restart relay process");
    assert!(
        wait_for_relay(relay.port, Duration::from_secs(15)).await,
        "restarted relay did not start listening"
    );

    let reconnect_content = r#"{"id":"request-after-reconnect"}"#;
    let reconnect_wrap = publish_gift_wrap(&relay_url, &sender, &receiver, reconnect_content).await;
    assert!(
        wait_for_gift_wrap(&relay_url, reconnect_wrap, Duration::from_secs(10)).await,
        "gift wrap after reconnect not stored by relay"
    );
    let event = recv_until(&mut rx, reconnect_wrap, Duration::from_secs(15)).await;
    assert_eq!(event.rumor.content, reconnect_content);

    // Restart: stop, then start again; a wrap published meanwhile must still
    // be delivered (the relay stores it and the fresh subscription replays
    // the window).
    inbox.stop().await;
    while rx.try_recv().is_ok() {}

    let content_two =
        r#"{"id":"request-two","mint":"http://localhost:3338","unit":"sat","proofs":[]}"#;
    let wrap_two = publish_gift_wrap(&relay_url, &sender, &receiver, content_two).await;
    assert!(
        wait_for_gift_wrap(&relay_url, wrap_two, Duration::from_secs(10)).await,
        "gift wrap two not stored by relay"
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        rx.try_recv().is_err(),
        "the stopped inbox invoked a callback after stop completed"
    );

    inbox
        .start(Arc::new(ChannelListener { tx }))
        .await
        .expect("restart inbox");

    let event = recv_until(&mut rx, wrap_two, Duration::from_secs(15)).await;
    assert_eq!(event.wrap_id, wrap_two);
    assert_eq!(event.rumor.content, content_two);

    inbox.stop().await;
}
