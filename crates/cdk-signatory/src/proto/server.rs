//! This module contains the generated gRPC server code for the Signatory service.
use std::net::SocketAddr;
use std::path::Path;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_stream::Stream;
use tonic::transport::server::Connected;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status};

use crate::proto::{self, signatory_server};
use crate::signatory::Signatory;

/// The server implementation for the Signatory service.
pub struct CdkSignatoryServer<T>
where
    T: Signatory + Send + Sync + 'static,
{
    inner: T,
}

#[tonic::async_trait]
impl<T> signatory_server::Signatory for CdkSignatoryServer<T>
where
    T: Signatory + Send + Sync + 'static,
{
    #[tracing::instrument(skip_all)]
    async fn blind_sign(
        &self,
        request: Request<proto::BlindedMessages>,
    ) -> Result<Response<proto::BlindSignResponse>, Status> {
        let result = match self
            .inner
            .blind_sign(
                request
                    .into_inner()
                    .blinded_messages
                    .into_iter()
                    .map(|blind_message| blind_message.try_into())
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .await
        {
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
        let result = match self
            .inner
            .verify_proofs(
                request
                    .into_inner()
                    .proof
                    .into_iter()
                    .map(|x| x.try_into())
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .await
        {
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
        _request: Request<proto::EmptyRequest>,
    ) -> Result<Response<proto::KeysResponse>, Status> {
        let result = match self.inner.keysets().await {
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
        let mint_keyset_info = match self
            .inner
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
pub async fn start_grpc_server<T, I: AsRef<Path>>(
    signatory: T,
    addr: SocketAddr,
    tls_dir: Option<I>,
) -> Result<(), Error>
where
    T: Signatory + Send + Sync + 'static,
{
    tracing::info!("Starting RPC server {}", addr);

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
        .add_service(signatory_server::SignatoryServer::new(CdkSignatoryServer {
            inner: signatory,
        }))
        .serve(addr)
        .await?;
    Ok(())
}

/// Starts the gRPC signatory server with an incoming stream of connections.
pub async fn start_grpc_server_with_incoming<T, I, IO, IE>(
    signatory: T,
    incoming: I,
) -> Result<(), Error>
where
    T: Signatory + Send + Sync + 'static,
    I: Stream<Item = Result<IO, IE>>,
    IO: AsyncRead + AsyncWrite + Connected + Unpin + Send + 'static,
    IE: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    Server::builder()
        .add_service(signatory_server::SignatoryServer::new(CdkSignatoryServer {
            inner: signatory,
        }))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
