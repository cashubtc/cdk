//! Nostr Wallet Connect (NIP-47) wallet **service**.
//!
//! This is the side that holds the funds and answers commands. It owns the
//! Nostr relay connection, publishes the kind `13194` info event advertising
//! capabilities, listens for kind `23194` requests from the authorized client,
//! decrypts and validates them, dispatches to a [`NwcRequestHandler`], and
//! publishes the encrypted kind `23195` response.
//!
//! Security properties enforced here (monetary software — defense in depth):
//! - **Authorization**: only events authored by the single client public key
//!   issued in the connection URI are processed (enforced both by the relay
//!   subscription filter and an explicit author check).
//! - **Replay / idempotency**: each request event id is processed at most once,
//!   so relay re-delivery cannot trigger a duplicate `pay_invoice`.
//! - **Freshness**: requests carrying an expired NIP-40 `expiration` tag are
//!   dropped, and only events created after the service started are considered.
//! - **No information leak / no panics**: every handler error is mapped to a
//!   NIP-47 error response; the relay loop never aborts on a single bad request.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use nostr_sdk::nips::nip47::{
    ErrorCode, NIP47Error, NostrWalletConnectURI, Request, RequestParams, Response,
    ResponseResult,
};
use nostr_sdk::nips::{nip04, nip44};
use nostr_sdk::prelude::*;
use nostr_sdk::{Client as NostrClient, Keys, PublicKey, RelayUrl, SecretKey};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::handler::NwcRequestHandler;

/// Commands advertised in the info event and reported by `get_info`.
///
/// Order is the canonical advertisement order; it is also the set of commands
/// this service will actually dispatch — anything else returns
/// [`ErrorCode::NotImplemented`].
pub const SUPPORTED_METHODS: [&str; 6] = [
    "pay_invoice",
    "make_invoice",
    "lookup_invoice",
    "list_transactions",
    "get_balance",
    "get_info",
];

/// Maximum number of recently-seen request event ids kept for replay
/// protection. Bounded to keep memory usage flat on long-running services.
const DEDUP_CAPACITY: usize = 10_000;

/// Encryption scheme negotiated for a single request/response exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encryption {
    /// NIP-44 v2 (preferred).
    Nip44,
    /// NIP-04 (legacy, still widely used by NWC clients).
    Nip04,
}

/// Configuration for an [`NwcService`].
#[derive(Debug, Clone)]
pub struct NwcServiceConfig {
    /// Keys of the wallet service (the "signer"). Its public key is the one
    /// advertised in the connection URI.
    pub service_keys: Keys,
    /// The client secret embedded in the connection URI. The public key derived
    /// from it is the only key authorized to send requests.
    pub client_secret: SecretKey,
    /// Relays the service connects to and listens on. Must be non-empty.
    pub relays: Vec<RelayUrl>,
    /// Optional lightning address advertised in the connection URI (`lud16`).
    pub lud16: Option<String>,
}

/// A NIP-47 wallet service bound to a single client connection.
#[derive(Debug, Clone)]
pub struct NwcService {
    service_keys: Keys,
    client_secret: SecretKey,
    client_pubkey: PublicKey,
    relays: Vec<RelayUrl>,
    lud16: Option<String>,
}

impl NwcService {
    /// Create a new service from configuration.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoRelays`] if no relays are configured.
    pub fn new(config: NwcServiceConfig) -> Result<Self> {
        if config.relays.is_empty() {
            return Err(Error::NoRelays);
        }

        let client_pubkey = Keys::new(config.client_secret.clone()).public_key();

        Ok(Self {
            service_keys: config.service_keys,
            client_secret: config.client_secret,
            client_pubkey,
            relays: config.relays,
            lud16: config.lud16,
        })
    }

    /// Public key of the wallet service (advertised in the connection URI).
    pub fn service_pubkey(&self) -> PublicKey {
        self.service_keys.public_key()
    }

    /// Public key of the authorized client.
    pub fn client_pubkey(&self) -> PublicKey {
        self.client_pubkey
    }

    /// Build the `nostr+walletconnect://` connection URI to hand to the client.
    pub fn connection_uri(&self) -> NostrWalletConnectURI {
        NostrWalletConnectURI::new(
            self.service_keys.public_key(),
            self.relays.clone(),
            self.client_secret.clone(),
            self.lud16.clone(),
        )
    }

    /// Connect to the relays, publish the info event, and service requests
    /// until `cancel` is triggered.
    ///
    /// This future runs the relay notification loop and only returns when the
    /// loop ends (cancellation) or a fatal relay/connection error occurs.
    ///
    /// # Errors
    ///
    /// Returns an error if relays cannot be added, the info event cannot be
    /// published, or the subscription cannot be created. Per-request failures
    /// never bubble up here — they are answered with a NIP-47 error response.
    pub async fn run<H>(&self, handler: Arc<H>, cancel: CancellationToken) -> Result<()>
    where
        H: NwcRequestHandler + 'static,
    {
        let client = NostrClient::new(self.service_keys.clone());

        for relay in &self.relays {
            client
                .add_relay(relay.clone())
                .await
                .map_err(|e| Error::Relay(format!("add relay {relay}: {e}")))?;
        }

        client.connect().await;

        self.publish_info_event(&client).await?;

        // Only consider requests created from now on, addressed to us, authored
        // by the authorized client.
        let filter = Filter::new()
            .kind(Kind::WalletConnectRequest)
            .author(self.client_pubkey)
            .pubkey(self.service_keys.public_key())
            .since(Timestamp::now());

        client
            .subscribe(filter, None)
            .await
            .map_err(|e| Error::Subscription(e.to_string()))?;

        let dedup: Arc<Mutex<Dedup>> = Arc::new(Mutex::new(Dedup::new(DEDUP_CAPACITY)));

        let service_keys = self.service_keys.clone();
        let client_pubkey = self.client_pubkey;
        let client_for_send = client.clone();

        let res = client
            .handle_notifications(move |notification| {
                let handler = handler.clone();
                let dedup = dedup.clone();
                let service_keys = service_keys.clone();
                let client = client_for_send.clone();
                let cancel = cancel.clone();
                async move {
                    if cancel.is_cancelled() {
                        return Ok(true);
                    }

                    let RelayPoolNotification::Event { event, .. } = notification else {
                        return Ok(false);
                    };

                    // Defense in depth: the filter already constrains the author,
                    // but never trust a relay to honor it.
                    if event.pubkey != client_pubkey || event.kind != Kind::WalletConnectRequest {
                        return Ok(false);
                    }

                    if event.is_expired() {
                        tracing::debug!("Dropping expired NWC request {}", event.id);
                        return Ok(false);
                    }

                    // Replay protection: process each request id at most once.
                    if !dedup.lock().await.insert(event.id) {
                        tracing::debug!("Dropping duplicate NWC request {}", event.id);
                        return Ok(false);
                    }

                    handle_request(&service_keys, &client, handler.as_ref(), &event).await;

                    Ok(false)
                }
            })
            .await;

        client.disconnect().await;

        res.map_err(|e| Error::Subscription(e.to_string()))
    }

    /// Publish the kind `13194` info event advertising supported commands and
    /// encryption schemes.
    async fn publish_info_event(&self, client: &NostrClient) -> Result<()> {
        let content = SUPPORTED_METHODS.join(" ");
        let encryption_tag = Tag::custom(
            TagKind::Custom(std::borrow::Cow::Borrowed("encryption")),
            ["nip44_v2".to_string(), "nip04".to_string()],
        );

        let event = EventBuilder::new(Kind::WalletConnectInfo, content)
            .tags([encryption_tag])
            .sign_with_keys(&self.service_keys)
            .map_err(|e| Error::Event(e.to_string()))?;

        client
            .send_event(&event)
            .await
            .map_err(|e| Error::Event(format!("publish info event: {e}")))?;

        Ok(())
    }
}

/// Decrypt, dispatch, and respond to a single request event.
///
/// Any failure is logged and (where possible) answered with a NIP-47 error
/// response. This function never panics and never returns an error to the
/// caller — keeping the relay loop alive is part of the security contract.
async fn handle_request<H>(
    service_keys: &Keys,
    client: &NostrClient,
    handler: &H,
    event: &Event,
) where
    H: NwcRequestHandler + ?Sized,
{
    let secret = service_keys.secret_key();

    let (request, encryption) =
        match decrypt_request(secret, &event.pubkey, &event.content) {
            Ok(parsed) => parsed,
            Err(e) => {
                tracing::warn!("Failed to decode NWC request {}: {e}", event.id);
                return;
            }
        };

    let response = dispatch(handler, request).await;

    if let Err(e) =
        send_response(service_keys, client, &event.pubkey, event.id, &response, encryption).await
    {
        tracing::warn!("Failed to send NWC response for {}: {e}", event.id);
    }
}

/// Try NIP-44 first, then fall back to NIP-04, and parse the request JSON.
fn decrypt_request(
    secret: &SecretKey,
    author: &PublicKey,
    content: &str,
) -> Result<(Request, Encryption)> {
    let (plaintext, encryption) = match nip44::decrypt(secret, author, content) {
        Ok(plaintext) => (plaintext, Encryption::Nip44),
        Err(_) => {
            let plaintext = nip04::decrypt(secret, author, content)
                .map_err(|e| Error::Encryption(e.to_string()))?;
            (plaintext, Encryption::Nip04)
        }
    };

    let request: Request =
        serde_json::from_str(&plaintext).map_err(|e| Error::Protocol(e.to_string()))?;

    Ok((request, encryption))
}

/// Dispatch a decoded request to the handler and build the NIP-47 response.
async fn dispatch<H>(handler: &H, request: Request) -> Response
where
    H: NwcRequestHandler + ?Sized,
{
    let result_type = request.method;

    let result: std::result::Result<ResponseResult, NIP47Error> = match request.params {
        RequestParams::GetInfo => handler.get_info().await.map(ResponseResult::GetInfo),
        RequestParams::GetBalance => handler.get_balance().await.map(ResponseResult::GetBalance),
        RequestParams::MakeInvoice(params) => handler
            .make_invoice(params)
            .await
            .map(ResponseResult::MakeInvoice),
        RequestParams::PayInvoice(params) => handler
            .pay_invoice(params)
            .await
            .map(ResponseResult::PayInvoice),
        RequestParams::LookupInvoice(params) => handler
            .lookup_invoice(params)
            .await
            .map(ResponseResult::LookupInvoice),
        RequestParams::ListTransactions(params) => handler
            .list_transactions(params)
            .await
            .map(ResponseResult::ListTransactions),
        // Commands outside the supported set.
        _ => Err(NIP47Error {
            code: ErrorCode::NotImplemented,
            message: format!("method {} is not implemented", result_type.as_str()),
        }),
    };

    match result {
        Ok(result) => Response {
            result_type,
            error: None,
            result: Some(result),
        },
        Err(error) => Response {
            result_type,
            error: Some(error),
            result: None,
        },
    }
}

/// Encrypt and publish the response event (kind `23195`).
async fn send_response(
    service_keys: &Keys,
    client: &NostrClient,
    client_pubkey: &PublicKey,
    request_id: EventId,
    response: &Response,
    encryption: Encryption,
) -> Result<()> {
    let payload = serde_json::to_string(response)?;
    let secret = service_keys.secret_key();

    let content = match encryption {
        Encryption::Nip44 => nip44::encrypt(secret, client_pubkey, payload, nip44::Version::V2)
            .map_err(|e| Error::Encryption(e.to_string()))?,
        Encryption::Nip04 => nip04::encrypt(secret, client_pubkey, payload)
            .map_err(|e| Error::Encryption(e.to_string()))?,
    };

    let event = EventBuilder::new(Kind::WalletConnectResponse, content)
        .tags([Tag::public_key(*client_pubkey), Tag::event(request_id)])
        .sign_with_keys(service_keys)
        .map_err(|e| Error::Event(e.to_string()))?;

    client
        .send_event(&event)
        .await
        .map_err(|e| Error::Event(format!("publish response: {e}")))?;

    Ok(())
}

/// Bounded set of recently-seen request event ids for replay protection.
#[derive(Debug)]
struct Dedup {
    seen: HashSet<EventId>,
    order: VecDeque<EventId>,
    capacity: usize,
}

impl Dedup {
    fn new(capacity: usize) -> Self {
        Self {
            seen: HashSet::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    /// Insert an id; returns `true` if it was newly inserted (i.e. should be
    /// processed), `false` if it was already seen.
    fn insert(&mut self, id: EventId) -> bool {
        if !self.seen.insert(id) {
            return false;
        }
        self.order.push_back(id);
        if self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_rejects_repeat_and_evicts_oldest() {
        let mut dedup = Dedup::new(2);
        let keys = Keys::generate();
        let mk = |content: &str| {
            EventBuilder::new(Kind::TextNote, content)
                .sign_with_keys(&keys)
                .expect("sign dummy event")
                .id
        };
        let a = mk("a");
        let b = mk("b");
        let c = mk("c");

        assert!(dedup.insert(a));
        assert!(!dedup.insert(a)); // duplicate
        assert!(dedup.insert(b));
        assert!(dedup.insert(c)); // evicts `a`
        assert!(dedup.insert(a)); // `a` evicted, treated as new again
    }

    use async_trait::async_trait;
    use nostr_sdk::nips::nip47::{
        GetBalanceResponse, GetInfoResponse, ListTransactionsRequest, LookupInvoiceRequest,
        LookupInvoiceResponse, MakeInvoiceRequest, MakeInvoiceResponse, PayInvoiceRequest,
        PayInvoiceResponse, Request,
    };

    /// Canned handler: `get_balance` returns a fixed value, everything else errors.
    struct MockHandler;

    #[async_trait]
    impl crate::handler::NwcRequestHandler for MockHandler {
        async fn get_info(&self) -> std::result::Result<GetInfoResponse, NIP47Error> {
            Err(NIP47Error {
                code: ErrorCode::Internal,
                message: "no".into(),
            })
        }
        async fn get_balance(&self) -> std::result::Result<GetBalanceResponse, NIP47Error> {
            Ok(GetBalanceResponse { balance: 5000 })
        }
        async fn make_invoice(
            &self,
            _: MakeInvoiceRequest,
        ) -> std::result::Result<MakeInvoiceResponse, NIP47Error> {
            unreachable!()
        }
        async fn pay_invoice(
            &self,
            _: PayInvoiceRequest,
        ) -> std::result::Result<PayInvoiceResponse, NIP47Error> {
            unreachable!()
        }
        async fn lookup_invoice(
            &self,
            _: LookupInvoiceRequest,
        ) -> std::result::Result<LookupInvoiceResponse, NIP47Error> {
            unreachable!()
        }
        async fn list_transactions(
            &self,
            _: ListTransactionsRequest,
        ) -> std::result::Result<Vec<LookupInvoiceResponse>, NIP47Error> {
            unreachable!()
        }
    }

    /// Encrypt a request the way a client would, with the chosen scheme.
    fn encrypt_request(
        client_secret: &SecretKey,
        service_pubkey: &PublicKey,
        request: &Request,
        scheme: Encryption,
    ) -> String {
        let json = serde_json::to_string(request).expect("serialize request");
        match scheme {
            Encryption::Nip44 => {
                nip44::encrypt(client_secret, service_pubkey, json, nip44::Version::V2)
                    .expect("nip44 encrypt")
            }
            Encryption::Nip04 => {
                nip04::encrypt(client_secret, service_pubkey, json).expect("nip04 encrypt")
            }
        }
    }

    #[test]
    fn decrypt_request_roundtrips_both_schemes() {
        let service = Keys::generate();
        let client = Keys::generate();
        let request = Request::get_balance();

        for scheme in [Encryption::Nip44, Encryption::Nip04] {
            let content = encrypt_request(
                client.secret_key(),
                &service.public_key(),
                &request,
                scheme,
            );

            let (decoded, detected) =
                decrypt_request(service.secret_key(), &client.public_key(), &content)
                    .expect("decrypt request");

            assert_eq!(detected, scheme);
            assert_eq!(decoded.method, request.method);
        }
    }

    #[tokio::test]
    async fn dispatch_get_balance_ok() {
        let response = dispatch(&MockHandler, Request::get_balance()).await;
        assert!(response.error.is_none());
        match response.result {
            Some(ResponseResult::GetBalance(b)) => assert_eq!(b.balance, 5000),
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_unsupported_method_is_not_implemented() {
        // pay_keysend is outside the supported set.
        let request = Request::pay_keysend(nostr_sdk::nips::nip47::PayKeysendRequest {
            id: None,
            amount: 1000,
            pubkey: "00".repeat(32),
            preimage: None,
            tlv_records: Vec::new(),
        });

        let response = dispatch(&MockHandler, request).await;
        let error = response.error.expect("unsupported method should error");
        assert_eq!(error.code, ErrorCode::NotImplemented);
        assert!(response.result.is_none());
    }
}
