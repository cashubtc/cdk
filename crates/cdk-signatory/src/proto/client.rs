use std::path::Path;
use std::str::FromStr;

use cdk_common::error::Error;
use cdk_common::grpc::VERSION_SIGNATORY_HEADER;
use cdk_common::{BlindSignature, BlindedMessage, Proof};
use tonic::metadata::MetadataValue;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::proto;
use crate::proto::signatory_client::SignatoryClient;
use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets};

/// A client for the Signatory service.
#[allow(missing_debug_implementations)]
pub struct SignatoryRpcClient {
    client: SignatoryClient<tonic::transport::Channel>,
    url: String,
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

/// Helper function to add version header to a request
fn with_version_header<T>(mut request: tonic::Request<T>) -> tonic::Request<T> {
    let version_str = (proto::Constants::SchemaVersion as u8).to_string();
    request.metadata_mut().insert(
        VERSION_SIGNATORY_HEADER,
        MetadataValue::from_str(&version_str).expect("valid version string"),
    );
    request
}

impl SignatoryRpcClient {
    /// Create a new RemoteSigner from a tonic transport channel.
    pub async fn new<A: AsRef<Path>>(url: String, tls_dir: Option<A>) -> Result<Self, ClientError> {
        #[cfg(not(target_arch = "wasm32"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        let channel = if let Some(tls_dir) = tls_dir {
            let tls_dir = tls_dir.as_ref();
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

        Ok(Self {
            client: SignatoryClient::new(channel),
            url,
        })
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
            .blind_sign(with_version_header(tonic::Request::new(req)))
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
            .verify_proofs(with_version_header(tonic::Request::new(req)))
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
            .keysets(with_version_header(tonic::Request::new(
                super::EmptyRequest {},
            )))
            .await
            .map(|response| handle_error!(response, keysets).try_into())
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    #[tracing::instrument(skip(self))]
    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
        let req: super::RotationRequest = args.into();
        self.client
            .clone()
            .rotate_keyset(with_version_header(tonic::Request::new(req)))
            .await
            .map(|response| handle_error!(response, keyset).try_into())
            .map_err(|e| Error::Custom(e.to_string()))?
    }
}
