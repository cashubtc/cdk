use std::path::Path;

use cdk_common::error::Error;
use cdk_common::{BlindSignature, BlindedMessage, Proof};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use super::{blind_sign_response, boolean_response, key_rotation_response, keys_response};
use crate::proto::signatory_client::SignatoryClient;
use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets};

/// A client for the Signatory service.
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

impl SignatoryRpcClient {
    /// Create a new RemoteSigner from a tonic transport channel.
    pub async fn new<A: AsRef<Path>>(url: String, tls_dir: Option<A>) -> Result<Self, ClientError> {
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

#[async_trait::async_trait]
impl Signatory for SignatoryRpcClient {
    fn name(&self) -> String {
        format!("Rpc Signatory {}", self.url)
    }

    async fn blind_sign(&self, request: Vec<BlindedMessage>) -> Result<Vec<BlindSignature>, Error> {
        let req = super::BlindedMessages {
            blinded_messages: request
                .into_iter()
                .map(|blind_message| blind_message.into())
                .collect(),
        };

        self.client
            .clone()
            .blind_sign(req)
            .await
            .map(|response| {
                match response
                    .into_inner()
                    .result
                    .ok_or(Error::Custom("Internal error".to_owned()))?
                {
                    blind_sign_response::Result::Sigs(sigs) => sigs
                        .blind_signatures
                        .into_iter()
                        .map(|blinded_signature| blinded_signature.try_into())
                        .collect(),
                    blind_sign_response::Result::Error(err) => Err(err.into()),
                }
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn verify_proofs(&self, proofs: Vec<Proof>) -> Result<(), Error> {
        let req: super::Proofs = proofs.into();
        self.client
            .clone()
            .verify_proofs(req)
            .await
            .map(|response| {
                match response
                    .into_inner()
                    .result
                    .ok_or(Error::Custom("Internal error".to_owned()))?
                {
                    boolean_response::Result::Success(bool) => {
                        if bool {
                            Ok(())
                        } else {
                            Err(Error::SignatureMissingOrInvalid)
                        }
                    }
                    boolean_response::Result::Error(err) => Err(err.into()),
                }
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
        self.client
            .clone()
            .keysets(super::EmptyRequest {})
            .await
            .map(|response| {
                match response
                    .into_inner()
                    .result
                    .ok_or(Error::Custom("Internal error".to_owned()))?
                {
                    keys_response::Result::Keysets(keyset) => keyset.try_into(),
                    keys_response::Result::Error(err) => Err(err.into()),
                }
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
        let req: super::RotationRequest = args.into();
        self.client
            .clone()
            .rotate_keyset(req)
            .await
            .map(|response| {
                match response
                    .into_inner()
                    .result
                    .ok_or(Error::Custom("Internal error".to_owned()))?
                {
                    key_rotation_response::Result::Keyset(keyset) => keyset.try_into(),
                    key_rotation_response::Result::Error(err) => Err(err.into()),
                }
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }
}
