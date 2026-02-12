//! This module contains the generated gRPC server code for the Signatory service.
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use cdk_common::grpc::create_version_check_interceptor;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_stream::Stream;
use tonic::metadata::MetadataMap;
use tonic::transport::server::Connected;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status};

use crate::proto::{self, signatory_server};
use crate::signatory::Signatory;

/// The server implementation for the Signatory service.
pub struct CdkSignatoryServer<S, T>
where
    S: Signatory + Send + Sync + 'static,
    T: SignatoryLoader<S> + 'static,
{
    loader: T,
    _phantom: std::marker::PhantomData<S>,
}

impl<S, T> CdkSignatoryServer<S, T>
where
    S: Signatory + Send + Sync + 'static,
    T: SignatoryLoader<S> + 'static,
{
    pub fn new(loader: T) -> Self {
        Self {
            loader,
            _phantom: std::marker::PhantomData,
        }
    }

    async fn load_signatory(&self, metadata: &MetadataMap) -> Result<Arc<S>, Status> {
        self.loader
            .load_signatory(metadata)
            .await
            .map_err(|_| Status::internal("Failed to load signatory"))
    }
}

#[tonic::async_trait]
impl<S, T> signatory_server::Signatory for CdkSignatoryServer<S, T>
where
    S: Signatory + Send + Sync + 'static,
    T: SignatoryLoader<S> + 'static,
{
    #[tracing::instrument(skip_all)]
    async fn blind_sign(
        &self,
        request: Request<proto::BlindedMessages>,
    ) -> Result<Response<proto::BlindSignResponse>, Status> {
        let metadata = request.metadata();
        let signatory = self.load_signatory(metadata).await?;

        let blinded_messages = request.into_inner().blinded_messages;
        let mut converted_messages = Vec::with_capacity(blinded_messages.len());
        for msg in blinded_messages {
            converted_messages.push(msg.try_into()?);
        }

        let result = match signatory.blind_sign(converted_messages).await {
            Ok(blind_signatures) => proto::BlindSignResponse {
                sigs: Some(proto::BlindSignatures {
                    blind_signatures: blind_signatures
                        .into_iter()
                        .map(|blind_sign| blind_sign.into())
                        .collect(),
                }),
                ..Default::default()
            },
            Err(err) => proto::BlindSignResponse {
                error: Some(err.into()),
                ..Default::default()
            },
        };

        Ok(Response::new(result))
    }

    #[tracing::instrument(skip_all)]
    async fn verify_proofs(
        &self,
        request: Request<proto::Proofs>,
    ) -> Result<Response<proto::BooleanResponse>, Status> {
        let metadata = request.metadata();
        let signatory = self.load_signatory(metadata).await?;

        let proofs = request.into_inner().proof;

        let mut converted_proofs = Vec::with_capacity(proofs.len());
        for p in proofs {
            converted_proofs.push(p.try_into()?);
        }

        let result = match signatory.verify_proofs(converted_proofs).await {
            Ok(()) => proto::BooleanResponse {
                success: true,
                ..Default::default()
            },

            Err(cdk_common::Error::DHKE(_)) => proto::BooleanResponse {
                success: false,
                ..Default::default()
            },
            Err(err) => proto::BooleanResponse {
                error: Some(err.into()),
                ..Default::default()
            },
        };

        Ok(Response::new(result))
    }

    async fn keysets(
        &self,
        request: Request<proto::EmptyRequest>,
    ) -> Result<Response<proto::KeysResponse>, Status> {
        let metadata = request.metadata();
        let signatory = self.load_signatory(metadata).await?;
        let result = match signatory.keysets().await {
            Ok(result) => proto::KeysResponse {
                keysets: Some(result.into()),
                ..Default::default()
            },
            Err(err) => proto::KeysResponse {
                error: Some(err.into()),
                ..Default::default()
            },
        };

        Ok(Response::new(result))
    }

    async fn rotate_keyset(
        &self,
        request: Request<proto::RotationRequest>,
    ) -> Result<Response<proto::KeyRotationResponse>, Status> {
        let metadata = request.metadata();
        let signatory = self.load_signatory(metadata).await?;
        let mint_keyset_info = match signatory
            .rotate_keyset(request.into_inner().try_into()?)
            .await
        {
            Ok(result) => proto::KeyRotationResponse {
                keyset: Some(result.into()),
                ..Default::default()
            },
            Err(err) => proto::KeyRotationResponse {
                error: Some(err.into()),
                ..Default::default()
            },
        };

        Ok(Response::new(mint_keyset_info))
    }
}

/// Trait for loading a signatory instance from gRPC metadata
#[async_trait::async_trait]
pub trait SignatoryLoader<S>: Send + Sync {
    /// Loads the signatory instance based on the provided metadata.
    async fn load_signatory(&self, metadata: &MetadataMap) -> Result<Arc<S>, ()>;
}

#[async_trait::async_trait]
impl<T> SignatoryLoader<T> for Arc<T>
where
    T: Signatory + Send + Sync + 'static,
{
    async fn load_signatory(&self, _metadata: &MetadataMap) -> Result<Arc<T>, ()> {
        Ok(self.clone())
    }
}

/// Error type for the gRPC server
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Transport error
    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),
    /// Io error
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Runs the signatory server
pub async fn start_grpc_server<S, T, I: AsRef<Path>>(
    signatory_loader: T,
    addr: SocketAddr,
    tls_dir: Option<I>,
) -> Result<(), Error>
where
    S: Signatory + Send + Sync + 'static,
    T: SignatoryLoader<S> + 'static,
{
    tracing::info!("Starting RPC server {}", addr);

    #[cfg(not(target_arch = "wasm32"))]
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    let mut server = match tls_dir {
        Some(tls_dir) => {
            tracing::info!("TLS configuration found, starting secure server");
            let tls_dir = tls_dir.as_ref();
            let server_pem_path = tls_dir.join("server.pem");
            let server_key_path = tls_dir.join("server.key");
            let ca_pem_path = tls_dir.join("ca.pem");

            if !server_pem_path.exists() {
                tracing::error!(
                    "Server certificate file does not exist: {}",
                    server_pem_path.display()
                );
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "Server certificate file not found: {}",
                        server_pem_path.display()
                    ),
                )));
            }

            if !server_key_path.exists() {
                tracing::error!(
                    "Server key file does not exist: {}",
                    server_key_path.display()
                );
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Server key file not found: {}", server_key_path.display()),
                )));
            }

            if !ca_pem_path.exists() {
                tracing::error!(
                    "CA certificate file does not exist: {}",
                    ca_pem_path.display()
                );
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("CA certificate file not found: {}", ca_pem_path.display()),
                )));
            }

            let cert = std::fs::read_to_string(&server_pem_path)?;
            let key = std::fs::read_to_string(&server_key_path)?;
            let client_ca_cert = std::fs::read_to_string(&ca_pem_path)?;
            let client_ca_cert = Certificate::from_pem(client_ca_cert);
            let server_identity = Identity::from_pem(cert, key);
            let tls_config = ServerTlsConfig::new()
                .identity(server_identity)
                .client_ca_root(client_ca_cert);

            Server::builder().tls_config(tls_config)?
        }
        None => {
            tracing::warn!("No valid TLS configuration found, starting insecure server");
            Server::builder()
        }
    };

    server
        .add_service(signatory_server::SignatoryServer::with_interceptor(
            CdkSignatoryServer::new(signatory_loader),
            create_version_check_interceptor(cdk_common::SIGNATORY_PROTOCOL_VERSION),
        ))
        .serve(addr)
        .await?;
    Ok(())
}

/// Starts the gRPC signatory server with an incoming stream of connections.
pub async fn start_grpc_server_with_incoming<S, T, I, IO, IE>(
    signatory_loader: T,
    incoming: I,
) -> Result<(), Error>
where
    S: Signatory + Send + Sync + 'static,
    T: SignatoryLoader<S> + 'static,
    I: Stream<Item = Result<IO, IE>>,
    IO: AsyncRead + AsyncWrite + Connected + Unpin + Send + 'static,
    IE: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    Server::builder()
        .add_service(signatory_server::SignatoryServer::with_interceptor(
            CdkSignatoryServer::new(signatory_loader),
            create_version_check_interceptor(cdk_common::SIGNATORY_PROTOCOL_VERSION),
        ))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
