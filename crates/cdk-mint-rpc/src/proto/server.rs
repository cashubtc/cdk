use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use cdk::mint::Mint;
use thiserror::Error;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};

use crate::cdk_mint_management_server::CdkMintManagementServer;
use crate::cdk_mint_reporting_server::CdkMintReportingServer;

/// Error
#[derive(Debug, Error)]
pub enum Error {
    /// Parse error
    #[error(transparent)]
    Parse(#[from] std::net::AddrParseError),
    /// Transport error
    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),
    /// Io error
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// CDK Mint RPC Server
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintRPCServer {
    socket_addr: SocketAddr,
    mint: Arc<Mint>,
    shutdown: Arc<Notify>,
    handle: Option<Arc<JoinHandle<Result<(), Error>>>>,
}

impl MintRPCServer {
    /// Creates a new MintRPCServer instance
    ///
    /// # Arguments
    /// * `addr` - The address to bind to
    /// * `port` - The port to listen on
    /// * `mint` - The Mint instance to serve
    pub fn new(addr: &str, port: u16, mint: Arc<Mint>) -> Result<Self, Error> {
        Ok(Self {
            socket_addr: format!("{addr}:{port}").parse()?,
            mint,
            shutdown: Arc::new(Notify::new()),
            handle: None,
        })
    }

    /// Starts the RPC server
    ///
    /// # Arguments
    /// * `tls_dir` - Optional directory containing TLS certificates
    ///
    /// If TLS directory is provided, it must contain:
    /// - server.pem: Server certificate
    /// - server.key: Server private key
    /// - ca.pem: CA certificate for client authentication
    pub async fn start(&mut self, tls_dir: Option<PathBuf>) -> Result<(), Error> {
        tracing::info!("Starting RPC server {}", self.socket_addr);

        #[cfg(not(target_arch = "wasm32"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        let server = match tls_dir {
            Some(tls_dir) => {
                tracing::info!("TLS configuration found, starting secure server");
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

                Server::builder()
                    .tls_config(tls_config)?
                    .add_service(CdkMintManagementServer::new(self.clone()))
                    .add_service(CdkMintReportingServer::new(self.clone()))
            }
            None => {
                tracing::warn!("No valid TLS configuration found, starting insecure server");
                Server::builder()
                    .add_service(CdkMintManagementServer::new(self.clone()))
                    .add_service(CdkMintReportingServer::new(self.clone()))
            }
        };

        let shutdown = self.shutdown.clone();
        let addr = self.socket_addr;

        self.handle = Some(Arc::new(tokio::spawn(async move {
            let server = server.serve_with_shutdown(addr, async {
                shutdown.notified().await;
            });

            server.await?;
            Ok(())
        })));

        Ok(())
    }

    /// Stops the RPC server gracefully
    pub async fn stop(&self) -> Result<(), Error> {
        self.shutdown.notify_one();
        if let Some(handle) = &self.handle {
            while !handle.is_finished() {
                tracing::info!("Waitning for mint rpc server to stop");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        tracing::info!("Mint rpc server stopped");
        Ok(())
    }

    /// Returns the Mint instance
    pub fn mint(&self) -> Arc<Mint> {
        Arc::clone(&self.mint)
    }
}

impl Drop for MintRPCServer {
    fn drop(&mut self) {
        tracing::debug!("Dropping mint rpc server");
        self.shutdown.notify_one();
    }
}
