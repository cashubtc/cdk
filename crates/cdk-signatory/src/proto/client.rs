use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::error::Error;
use cdk_common::grpc::{VersionInterceptor, VERSION_SIGNATORY_HEADER};
use cdk_common::stream::{BackoffPolicy, SupervisedStream};
use cdk_common::{BlindSignature, BlindedMessage, Proof};
use tokio::sync::watch;
use tonic::codegen::InterceptedService;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::proto;
use crate::proto::signatory_client::SignatoryClient;
use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets};

/// Largest delay between keyset subscription reconnect attempts.
const KEYSET_RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(30);

type InnerClient = SignatoryClient<InterceptedService<Channel, VersionInterceptor>>;

/// A client for the Signatory service.
#[allow(missing_debug_implementations)]
pub struct SignatoryRpcClient {
    client: InnerClient,
    url: String,
    /// Latest keyset snapshot maintained by a background subscription task.
    keyset_updates: watch::Receiver<SignatoryKeysets>,
}

#[derive(thiserror::Error, Debug)]
/// Client Signatory Error
pub enum ClientError {
    /// Transport error
    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),

    /// IO-related errors
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Signatory Error
    #[error(transparent)]
    Signatory(#[from] cdk_common::error::Error),

    /// Invalid URL
    #[error("Invalid URL")]
    InvalidUrl,
}

impl SignatoryRpcClient {
    /// Create a new RemoteSigner from a tonic transport channel.
    pub async fn new(addr: &str, port: u16, tls_dir: Option<PathBuf>) -> Result<Self, ClientError> {
        #[cfg(not(target_arch = "wasm32"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        let scheme = if tls_dir.is_some() { "https" } else { "http" };
        let url = format!("{scheme}://{addr}:{port}");

        let channel = if let Some(tls_dir) = tls_dir {
            let server_root_ca_cert = std::fs::read_to_string(tls_dir.join("ca.pem"))?;
            let server_root_ca_cert = Certificate::from_pem(server_root_ca_cert);
            let client_cert = std::fs::read_to_string(tls_dir.join("client.pem"))?;
            let client_key = std::fs::read_to_string(tls_dir.join("client.key"))?;
            let client_identity = Identity::from_pem(client_cert, client_key);
            let tls = ClientTlsConfig::new()
                .ca_certificate(server_root_ca_cert)
                .identity(client_identity);

            Channel::from_shared(url.clone())
                .map_err(|_| ClientError::InvalidUrl)?
                .tls_config(tls)?
                .connect()
                .await?
        } else {
            Channel::from_shared(url.clone())
                .map_err(|_| ClientError::InvalidUrl)?
                .connect()
                .await?
        };

        let version = (proto::Constants::SchemaVersion as u8).to_string();
        let interceptor = VersionInterceptor::new(VERSION_SIGNATORY_HEADER, version);

        let mut client = SignatoryClient::with_interceptor(channel, interceptor);

        // Seed the watch with the current keysets so the mint always has a
        // valid snapshot, even before the first streamed update arrives.
        let initial = fetch_keysets(&mut client).await?;
        let (keyset_updates_tx, keyset_updates) = watch::channel(initial);

        // Keep the keyset watch fresh in the background. `SupervisedStream` owns
        // the reconnect/backoff/shutdown machinery; the task stops once every
        // watch receiver is dropped, which `closed()` observes. On every
        // (re)connect the server sends the current snapshot first, so a dropped
        // connection re-injects the latest keysets.
        //
        // One `Arc` handle publishes from inside the subscription; a second
        // drives the shutdown future. `Sender::closed` keys on receiver drop
        // (senders do not matter), so both handles observe the same shutdown.
        let keyset_updates_tx = Arc::new(keyset_updates_tx);
        let shutdown_tx = Arc::clone(&keyset_updates_tx);
        let subscription_client = client.clone();
        tokio::spawn(async move {
            let mut subscription = KeysetSubscription {
                client: subscription_client,
                keyset_updates_tx,
            };
            subscription.supervise(shutdown_tx.closed()).await;
        });

        Ok(Self {
            client,
            url,
            keyset_updates,
        })
    }
}

/// One unary keysets fetch, used to seed the subscription watch.
async fn fetch_keysets(client: &mut InnerClient) -> Result<SignatoryKeysets, ClientError> {
    let mut response = client
        .keysets(tonic::Request::new(super::EmptyRequest {}))
        .await
        .map_err(|e| ClientError::Signatory(Error::Custom(e.to_string())))?
        .into_inner();

    if let Some(err) = response.error.take() {
        return Err(ClientError::Signatory(err.into()));
    }

    response
        .keysets
        .ok_or_else(|| ClientError::Signatory(Error::Custom("Internal error".to_owned())))?
        .try_into()
        .map_err(ClientError::Signatory)
}

/// Convert a streamed `KeysResponse` into a keyset snapshot.
fn keys_response_into_keysets(
    mut response: super::KeysResponse,
) -> Result<SignatoryKeysets, Error> {
    if let Some(err) = response.error.take() {
        return Err(err.into());
    }

    response
        .keysets
        .ok_or_else(|| Error::Custom("Internal error".to_owned()))?
        .try_into()
}

/// Background subscription that keeps the keyset watch fresh from the signatory.
struct KeysetSubscription {
    client: InnerClient,
    keyset_updates_tx: Arc<watch::Sender<SignatoryKeysets>>,
}

#[async_trait::async_trait]
impl SupervisedStream for KeysetSubscription {
    type Item = super::KeysResponse;
    type ConnectError = tonic::Status;
    type StreamError = tonic::Status;
    type Stream = tonic::Streaming<super::KeysResponse>;

    fn name(&self) -> &str {
        "signatory keysets"
    }

    fn backoff_policy(&self) -> BackoffPolicy {
        BackoffPolicy {
            initial_connect_backoff: Duration::from_secs(1),
            max_connect_backoff: KEYSET_RECONNECT_MAX_BACKOFF,
        }
    }

    async fn connect(&mut self) -> Result<Self::Stream, tonic::Status> {
        self.client
            .subscribe_keysets(tonic::Request::new(super::EmptyRequest {}))
            .await
            .map(tonic::Response::into_inner)
    }

    async fn on_message(&mut self, message: super::KeysResponse) {
        match keys_response_into_keysets(message) {
            Ok(keysets) => {
                self.keyset_updates_tx.send_replace(keysets);
            }
            Err(err) => {
                tracing::warn!("Invalid keyset update from signatory: {err}");
            }
        }
    }
}

macro_rules! handle_error {
    ($x:expr, $y:ident, scalar) => {{
        let mut obj = $x.into_inner();
        if let Some(err) = obj.error.take() {
            return Err(err.into());
        }

        obj.$y
    }};
    ($x:expr, $y:ident) => {{
        let mut obj = $x.into_inner();
        if let Some(err) = obj.error.take() {
            return Err(err.into());
        }

        obj.$y
            .take()
            .ok_or(Error::Custom("Internal error".to_owned()))?
    }};
}

#[async_trait::async_trait]
impl Signatory for SignatoryRpcClient {
    fn name(&self) -> String {
        format!("Rpc Signatory {}", self.url)
    }

    #[tracing::instrument(skip_all)]
    async fn blind_sign(&self, request: Vec<BlindedMessage>) -> Result<Vec<BlindSignature>, Error> {
        let req = super::BlindedMessages {
            blinded_messages: request
                .into_iter()
                .map(|blind_message| blind_message.into())
                .collect(),
        };

        self.client
            .clone()
            .blind_sign(tonic::Request::new(req))
            .await
            .map(|response| {
                handle_error!(response, sigs)
                    .blind_signatures
                    .into_iter()
                    .map(|blinded_signature| blinded_signature.try_into())
                    .collect()
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    #[tracing::instrument(skip_all)]
    async fn verify_proofs(&self, proofs: Vec<Proof>) -> Result<(), Error> {
        let req: super::Proofs = proofs.into();
        self.client
            .clone()
            .verify_proofs(tonic::Request::new(req))
            .await
            .map(|response| {
                if handle_error!(response, success, scalar) {
                    Ok(())
                } else {
                    // The server only reports `success: false` for a DHKE failure, which the
                    // in-process signatory surfaces as `TokenNotVerified`.
                    Err(Error::DHKE(cdk_common::dhke::Error::TokenNotVerified))
                }
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    #[tracing::instrument(skip_all)]
    async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
        self.client
            .clone()
            .keysets(tonic::Request::new(super::EmptyRequest {}))
            .await
            .map(|response| handle_error!(response, keysets).try_into())
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    #[tracing::instrument(skip_all)]
    async fn subscribe_keysets(&self) -> Result<watch::Receiver<SignatoryKeysets>, Error> {
        Ok(self.keyset_updates.clone())
    }

    #[tracing::instrument(skip(self))]
    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
        let req: super::RotationRequest = args.into();
        self.client
            .clone()
            .rotate_keyset(tonic::Request::new(req))
            .await
            .map(|response| handle_error!(response, keyset).try_into())
            .map_err(|e| Error::Custom(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::nut02::KeySetVersion;
    use cdk_common::secret::Secret;
    use cdk_common::{Amount, CurrencyUnit, SecretKey};
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;

    use super::*;
    use crate::db_signatory::DbSignatory;
    use crate::proto::server::start_grpc_server_with_incoming;

    /// Serves a fresh `DbSignatory` over gRPC on a local port and returns it with a client.
    async fn remote_signatory() -> (Arc<DbSignatory>, SignatoryRpcClient) {
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let signatory = Arc::new(
            DbSignatory::new(
                store,
                b"remote-verify",
                Default::default(),
                Default::default(),
            )
            .await
            .expect("DbSignatory::new"),
        );
        signatory
            .rotate_keyset(RotateKeyArguments {
                unit: CurrencyUnit::Sat,
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: 0,
                keyset_id_type: KeySetVersion::Version00,
                final_expiry: None,
            })
            .await
            .expect("rotate_keyset");

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        tokio::spawn(start_grpc_server_with_incoming(
            signatory.clone(),
            TcpListenerStream::new(listener),
        ));

        let client = SignatoryRpcClient::new("127.0.0.1", port, None)
            .await
            .expect("connect to signatory");

        (signatory, client)
    }

    #[tokio::test]
    async fn verify_proofs_reports_an_invalid_signature_like_the_local_signatory() {
        let (local, remote) = remote_signatory().await;
        let keyset = local.keysets().await.expect("keysets").keysets[0].clone();

        // A proof on a known keyset and amount whose `C` was not signed by the mint.
        let forged = Proof::new(
            Amount::from(1),
            keyset.id,
            Secret::generate(),
            SecretKey::generate().public_key(),
        );

        let local_err = local
            .verify_proofs(vec![forged.clone()])
            .await
            .expect_err("local signatory rejects a forged proof");
        let remote_err = remote
            .verify_proofs(vec![forged])
            .await
            .expect_err("remote signatory rejects a forged proof");

        assert!(matches!(
            local_err,
            Error::DHKE(cdk_common::dhke::Error::TokenNotVerified)
        ));
        assert!(
            matches!(
                remote_err,
                Error::DHKE(cdk_common::dhke::Error::TokenNotVerified)
            ),
            "unexpected remote error: {remote_err:?}"
        );
    }

    #[tokio::test]
    async fn verify_proofs_reports_an_unknown_keyset_like_the_local_signatory() {
        let (local, remote) = remote_signatory().await;
        let keyset = local.keysets().await.expect("keysets").keysets[0].clone();

        // The amount is not part of the keyset, so no key exists to verify it with.
        let proof = Proof::new(
            Amount::from(16),
            keyset.id,
            Secret::generate(),
            SecretKey::generate().public_key(),
        );

        let local_err = local
            .verify_proofs(vec![proof.clone()])
            .await
            .expect_err("local signatory rejects the proof");
        let remote_err = remote
            .verify_proofs(vec![proof])
            .await
            .expect_err("remote signatory rejects the proof");

        assert!(matches!(local_err, Error::UnknownKeySet));
        assert!(
            matches!(remote_err, Error::UnknownKeySet),
            "unexpected remote error: {remote_err:?}"
        );
    }
}
