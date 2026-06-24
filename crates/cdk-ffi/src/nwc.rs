//! FFI bindings for the Nostr Wallet Connect (NIP-47) wallet service.
//!
//! Exposes an [`NwcService`] object that turns a CDK [`Wallet`] into a NIP-47
//! wallet service: it generates a `nostr+walletconnect://` connection URI to
//! hand to a Nostr app, then listens on the configured relays and answers the
//! supported commands (`get_info`, `get_balance`, `make_invoice`,
//! `pay_invoice`, `lookup_invoice`, `list_transactions`) using the wallet.

use std::sync::{Arc, Mutex};

use cdk::wallet::WalletNwcHandler;
use cdk_nwc::{NwcService as CdkNwcService, NwcServiceConfig};
use nostr_sdk::{Keys, RelayUrl, SecretKey};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::FfiError;
use crate::wallet::Wallet;

/// A NIP-47 Nostr Wallet Connect wallet service bound to a CDK wallet.
///
/// Create one with [`NwcService::create`] (new connection) or
/// [`NwcService::restore`] (existing connection from a persisted client
/// secret), call [`NwcService::connection_uri`] to obtain the URI for the
/// Nostr app, then [`NwcService::start`] to begin servicing requests.
#[derive(uniffi::Object)]
pub struct NwcService {
    service: CdkNwcService,
    handler: Arc<WalletNwcHandler>,
    task: Mutex<Option<(JoinHandle<()>, CancellationToken)>>,
}

impl NwcService {
    /// Shared construction logic for [`Self::create`] and [`Self::restore`].
    fn build(
        wallet: &Arc<Wallet>,
        relays: Vec<String>,
        service_keys: Keys,
        client_secret: SecretKey,
        budget_msat: Option<u64>,
    ) -> Result<Self, FfiError> {
        if relays.is_empty() {
            return Err(FfiError::internal("at least one relay is required"));
        }

        let relays = relays
            .iter()
            .map(|r| {
                RelayUrl::parse(r)
                    .map_err(|e| FfiError::internal(format!("invalid relay {r}: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let cdk_wallet = wallet.inner().as_ref().clone();
        let handler = Arc::new(WalletNwcHandler::new(cdk_wallet, budget_msat));

        let service = CdkNwcService::new(NwcServiceConfig {
            service_keys,
            client_secret,
            relays,
            lud16: None,
        })
        .map_err(|e| FfiError::internal(e.to_string()))?;

        Ok(Self {
            service,
            handler,
            task: Mutex::new(None),
        })
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl NwcService {
    /// Create a new wallet service with a freshly generated client connection.
    ///
    /// # Arguments
    ///
    /// * `wallet` - The CDK wallet that backs the service.
    /// * `relays` - Relay URLs the service connects to and listens on.
    /// * `service_secret_key` - Secret key of the wallet service (the signer).
    ///   Accepts hex or bech32 `nsec`. Derive a stable one from the wallet seed
    ///   with [`nwc_derive_service_secret_key_from_seed`].
    /// * `budget_msat` - Optional cap (in millisatoshis) on any single
    ///   `pay_invoice` request.
    ///
    /// # Errors
    ///
    /// Returns an error if a key or relay URL is invalid, or no relays are given.
    #[uniffi::constructor]
    pub fn create(
        wallet: Arc<Wallet>,
        relays: Vec<String>,
        service_secret_key: String,
        budget_msat: Option<u64>,
    ) -> Result<Self, FfiError> {
        let service_keys = parse_keys(&service_secret_key)?;
        let client_secret = SecretKey::generate();
        Self::build(&wallet, relays, service_keys, client_secret, budget_msat)
    }

    /// Restore a wallet service for an existing connection.
    ///
    /// Use this to rebuild a service after a restart from a persisted client
    /// secret, so the previously issued connection URI keeps working.
    ///
    /// # Arguments
    ///
    /// * `client_secret_key` - The client secret from the original connection
    ///   URI (hex or `nsec`).
    ///
    /// See [`Self::create`] for the other arguments.
    ///
    /// # Errors
    ///
    /// Returns an error if a key or relay URL is invalid, or no relays are given.
    #[uniffi::constructor]
    pub fn restore(
        wallet: Arc<Wallet>,
        relays: Vec<String>,
        service_secret_key: String,
        client_secret_key: String,
        budget_msat: Option<u64>,
    ) -> Result<Self, FfiError> {
        let service_keys = parse_keys(&service_secret_key)?;
        let client_secret = parse_secret_key(&client_secret_key)?;
        Self::build(&wallet, relays, service_keys, client_secret, budget_msat)
    }

    /// The `nostr+walletconnect://` connection URI to hand to the Nostr app.
    pub fn connection_uri(&self) -> String {
        self.service.connection_uri().to_string()
    }

    /// Hex-encoded public key of the wallet service (advertised in the URI).
    pub fn service_pubkey(&self) -> String {
        self.service.service_pubkey().to_hex()
    }

    /// Hex-encoded public key of the authorized client.
    pub fn client_pubkey(&self) -> String {
        self.service.client_pubkey().to_hex()
    }

    /// Start servicing requests in the background.
    ///
    /// Connects to the relays, publishes the info event, and begins answering
    /// commands. Returns immediately; the service runs until [`Self::stop`] is
    /// called. Per-request failures are answered with NIP-47 error responses
    /// and logged rather than surfaced here.
    ///
    /// # Errors
    ///
    /// Returns an error if the service is already running.
    // `async` is required so uniffi drives this on the tokio runtime, which
    // `tokio::spawn` needs; the body itself does not await.
    #[allow(clippy::unused_async)]
    pub async fn start(&self) -> Result<(), FfiError> {
        let mut guard = self
            .task
            .lock()
            .map_err(|_| FfiError::internal("nwc service lock poisoned"))?;

        if guard.is_some() {
            return Err(FfiError::internal("nwc service is already running"));
        }

        let cancel = CancellationToken::new();
        let service = self.service.clone();
        let handler = self.handler.clone();
        let run_cancel = cancel.clone();

        let handle = tokio::spawn(async move {
            if let Err(e) = service.run(handler, run_cancel).await {
                tracing::error!("NWC service stopped with error: {e}");
            }
        });

        *guard = Some((handle, cancel));
        Ok(())
    }

    /// Stop the background service if it is running.
    pub async fn stop(&self) -> Result<(), FfiError> {
        let task = {
            let mut guard = self
                .task
                .lock()
                .map_err(|_| FfiError::internal("nwc service lock poisoned"))?;
            guard.take()
        };

        if let Some((handle, cancel)) = task {
            cancel.cancel();
            handle.abort();
            let _ = handle.await;
        }

        Ok(())
    }

    /// Whether the background service is currently running.
    pub fn is_running(&self) -> bool {
        self.task.lock().map(|g| g.is_some()).unwrap_or(false)
    }
}

/// Derive the NWC wallet-service secret key from a wallet seed.
///
/// Returns a hex-encoded secret key for use as `service_secret_key`. Deriving
/// from the seed keeps the connection URI stable across restarts. Uses the
/// NIP-06 path `m/44'/1237'/1'/0/0`, distinct from the npub.cash key.
///
/// # Errors
///
/// Returns an error if the seed is shorter than 64 bytes or derivation fails.
#[uniffi::export]
pub fn nwc_derive_service_secret_key_from_seed(seed: Vec<u8>) -> Result<String, FfiError> {
    if seed.len() < 64 {
        return Err(FfiError::internal("Seed must be at least 64 bytes"));
    }

    let seed: [u8; 64] = seed[..64]
        .try_into()
        .map_err(|_| FfiError::internal("Failed to read wallet seed bytes"))?;

    let secret_key = cdk::wallet::derive_nwc_secret_key_from_seed(&seed)
        .map_err(|e| FfiError::internal(format!("Failed to derive secret key: {e}")))?;

    Ok(secret_key.to_secret_hex())
}

/// Get the hex-encoded public key for a Nostr secret key (hex or `nsec`).
///
/// # Errors
///
/// Returns an error if the secret key is invalid.
#[uniffi::export]
pub fn nwc_get_pubkey(nostr_secret_key: String) -> Result<String, FfiError> {
    Ok(parse_keys(&nostr_secret_key)?.public_key().to_hex())
}

/// Parse a Nostr secret key (hex or bech32 `nsec`) into [`Keys`].
fn parse_keys(key: &str) -> Result<Keys, FfiError> {
    Ok(Keys::new(parse_secret_key(key)?))
}

/// Parse a Nostr secret key from either hex or bech32 `nsec`.
fn parse_secret_key(key: &str) -> Result<SecretKey, FfiError> {
    SecretKey::parse(key).map_err(|e| FfiError::internal(format!("invalid secret key: {e}")))
}
