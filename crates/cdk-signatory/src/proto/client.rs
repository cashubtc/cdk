use std::path::PathBuf;
use std::time::Duration;

use cdk_common::error::Error;
use cdk_common::grpc::{VersionInterceptor, VERSION_SIGNATORY_HEADER};
use cdk_common::{BlindSignature, BlindedMessage, Proof};
use tokio::sync::watch;
use tonic::codegen::InterceptedService;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::proto;
use crate::proto::signatory_client::SignatoryClient;
use crate::signatory::{
    ReconstructDleqArguments, RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets,
};

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

        // Keep the keyset watch fresh in the background. On every (re)connect the
        // server sends the current snapshot first, so a dropped connection
        // re-injects the latest keysets on reconnect.
        tokio::spawn(keyset_subscription_loop(client.clone(), keyset_updates_tx));

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

/// Subscribe to the signatory keyset stream, forwarding every snapshot into the
/// watch channel and reconnecting with exponential backoff on failure.
///
/// The loop exits once every receiver has been dropped (the mint is gone).
async fn keyset_subscription_loop(
    mut client: InnerClient,
    updates: watch::Sender<SignatoryKeysets>,
) {
    let mut backoff = Duration::from_secs(1);

    loop {
        if updates.is_closed() {
            break;
        }

        match client
            .subscribe_keysets(tonic::Request::new(super::EmptyRequest {}))
            .await
        {
            Ok(response) => {
                backoff = Duration::from_secs(1);
                let mut stream = response.into_inner();
                loop {
                    match stream.message().await {
                        Ok(Some(message)) => match keys_response_into_keysets(message) {
                            Ok(keysets) => {
                                updates.send_replace(keysets);
                            }
                            Err(err) => {
                                tracing::warn!("Invalid keyset update from signatory: {err}");
                            }
                        },
                        Ok(None) => {
                            tracing::debug!("Signatory closed the keyset stream");
                            break;
                        }
                        Err(status) => {
                            tracing::warn!("Keyset stream error: {status}");
                            break;
                        }
                    }
                }
            }
            Err(status) => {
                tracing::warn!("Could not subscribe to signatory keysets: {status}");
            }
        }

        if updates.is_closed() {
            break;
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(KEYSET_RECONNECT_MAX_BACKOFF);
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
                    Err(Error::SignatureMissingOrInvalid)
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

    #[tracing::instrument(skip_all)]
    async fn reconstruct_dleq(
        &self,
        args: ReconstructDleqArguments,
    ) -> Result<BlindSignature, Error> {
        let req: super::ReconstructDleqRequest = args.into();
        self.client
            .clone()
            .reconstruct_dleq(tonic::Request::new(req))
            .await
            .map(|response| handle_error!(response, blind_signature).try_into())
            .map_err(|e| Error::Custom(e.to_string()))?
    }
}
