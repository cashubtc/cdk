//! Mint management RPC server and service registration.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use cdk::mint::Mint;
use cdk_common::grpc::create_version_check_interceptor;
use thiserror::Error;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::Status;

mod info;
mod keyset;
mod payment_method;
mod quote;
#[cfg(test)]
mod test_utils;
mod wallet;

use crate::info::mint_info_service_server::MintInfoServiceServer;
use crate::keyset::keyset_service_server::KeysetServiceServer;
use crate::payment_method::payment_method_service_server::PaymentMethodServiceServer;
use crate::quote::quote_service_server::QuoteServiceServer;
use crate::wallet::wallet_service_server::WalletServiceServer;
use crate::DynWalletInfoProvider;

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

/// Failure returned when a management mutation is not currently allowed.
#[derive(Debug, Error)]
pub enum MintMutationGuardError {
    /// The mutation conflicts with the mint's current lifecycle state.
    #[error("{0}")]
    FailedPrecondition(String),
    /// The lifecycle state could not be checked.
    #[error("{0}")]
    Internal(String),
}

/// Checks whether management RPC mutations are currently allowed.
#[tonic::async_trait]
pub trait MintMutationGuard: Send + Sync {
    /// Returns successfully when a mutation may proceed.
    async fn check(&self) -> Result<(), MintMutationGuardError>;
}

/// CDK Mint RPC Server
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintRPCServer {
    socket_addr: SocketAddr,
    mint: Arc<Mint>,
    mutation_guard: Option<Arc<dyn MintMutationGuard>>,
    allow_mint_quote_payment_override: bool,
    wallet_info_provider: Option<DynWalletInfoProvider>,
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
            mutation_guard: None,
            allow_mint_quote_payment_override: false,
            wallet_info_provider: None,
            shutdown: Arc::new(Notify::new()),
            handle: None,
        })
    }

    /// Adds a guard that runs before every mutating management RPC.
    pub fn with_mutation_guard(mut self, guard: Arc<dyn MintMutationGuard>) -> Self {
        self.mutation_guard = Some(guard);
        self
    }

    /// Enables or disables management RPC mint quote state overrides.
    ///
    /// Disabled by default because the paid state records a payment without
    /// confirmation from the configured payment backend.
    pub fn with_mint_quote_payment_override(mut self, enabled: bool) -> Self {
        self.allow_mint_quote_payment_override = enabled;
        self
    }

    async fn ensure_mutation_allowed(&self) -> Result<(), Status> {
        let Some(guard) = &self.mutation_guard else {
            return Ok(());
        };

        guard.check().await.map_err(|error| match error {
            MintMutationGuardError::FailedPrecondition(message) => {
                Status::failed_precondition(message)
            }
            MintMutationGuardError::Internal(message) => Status::internal(message),
        })
    }

    /// Configures the on-chain wallet management provider.
    pub fn with_wallet_info_provider(mut self, provider: DynWalletInfoProvider) -> Self {
        self.wallet_info_provider = Some(provider);
        self
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
                    .add_service(MintInfoServiceServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ))
                    .add_service(KeysetServiceServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ))
                    .add_service(PaymentMethodServiceServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ))
                    .add_service(QuoteServiceServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ))
                    .add_service(WalletServiceServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ))
            }
            None => {
                tracing::warn!("No valid TLS configuration found, starting insecure server");
                Server::builder()
                    .add_service(MintInfoServiceServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ))
                    .add_service(KeysetServiceServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ))
                    .add_service(PaymentMethodServiceServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ))
                    .add_service(QuoteServiceServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ))
                    .add_service(WalletServiceServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ))
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
}

impl Drop for MintRPCServer {
    fn drop(&mut self) {
        tracing::debug!("Dropping mint rpc server");
        self.shutdown.notify_one();
    }
}

fn page_limit(requested: u32, default: u32, maximum: u32) -> Result<usize, Status> {
    let limit = match requested {
        0 => default,
        requested if requested <= maximum => requested,
        requested => {
            return Err(Status::invalid_argument(format!(
                "Requested page size {requested} exceeds maximum {maximum}"
            )));
        }
    };

    Ok(limit as usize)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tonic::{Code, Request};

    use super::test_utils::{create_test_rpc_server, RejectingMutationGuard, UNKNOWN_QUOTE_ID};
    use crate::info::mint_info_service_server::MintInfoService;
    use crate::keyset::keyset_service_server::KeysetService;
    use crate::payment_method::payment_method_service_server::PaymentMethodService;
    use crate::quote::quote_service_server::QuoteService;
    use crate::wallet::wallet_service_server::WalletService;

    #[tokio::test]
    async fn test_mutation_guard_rejects_updates_without_blocking_reads() {
        let server = create_test_rpc_server()
            .await
            .with_mutation_guard(Arc::new(RejectingMutationGuard));

        let keyset_error = KeysetService::rotate_next_keyset(
            &server,
            Request::new(crate::keyset::RotateNextKeysetRequest {
                unit: "sat".to_owned(),
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: Some(1),
                use_keyset_v2: Some(true),
                final_expiry: None,
            }),
        )
        .await
        .expect_err("keyset mutation should be rejected");

        assert_eq!(keyset_error.code(), Code::FailedPrecondition);
        assert_eq!(keyset_error.message(), "configuration restart pending");

        let quote_ttl_error = QuoteService::update_quote_ttl(
            &server,
            Request::new(crate::quote::UpdateQuoteTtlRequest {
                mint_ttl: Some(60),
                melt_ttl: None,
            }),
        )
        .await
        .expect_err("quote TTL mutation should be rejected");

        assert_eq!(quote_ttl_error.code(), Code::FailedPrecondition);
        assert_eq!(quote_ttl_error.message(), "configuration restart pending");

        let quote_state_error = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: UNKNOWN_QUOTE_ID.to_owned(),
                state: crate::quote::MintQuoteState::Paid.into(),
            }),
        )
        .await
        .expect_err("quote-state mutation should be rejected");

        assert_eq!(quote_state_error.code(), Code::FailedPrecondition);
        assert_eq!(quote_state_error.message(), "configuration restart pending");

        let mint_method_error = PaymentMethodService::update_mint_method(
            &server,
            Request::new(crate::payment_method::UpdateMintMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt11".to_owned(),
                min_amount: Some(1),
                max_amount: None,
                options: None,
                method_name: None,
            }),
        )
        .await
        .expect_err("mint-method mutation should be rejected");

        assert_eq!(mint_method_error.code(), Code::FailedPrecondition);
        assert_eq!(mint_method_error.message(), "configuration restart pending");

        let melt_method_error = PaymentMethodService::update_melt_method(
            &server,
            Request::new(crate::payment_method::UpdateMeltMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt11".to_owned(),
                min_amount: Some(1),
                max_amount: None,
                options: None,
                method_name: None,
            }),
        )
        .await
        .expect_err("melt-method mutation should be rejected");

        assert_eq!(melt_method_error.code(), Code::FailedPrecondition);
        assert_eq!(melt_method_error.message(), "configuration restart pending");

        let disabled_error = PaymentMethodService::update_disabled(
            &server,
            Request::new(crate::payment_method::UpdateDisabledRequest {
                mint_disabled: Some(true),
                melt_disabled: None,
            }),
        )
        .await
        .expect_err("disabled mutation should be rejected");

        assert_eq!(disabled_error.code(), Code::FailedPrecondition);
        assert_eq!(disabled_error.message(), "configuration restart pending");

        // A request that changes nothing is still a mutation RPC: the guard
        // runs before the both-flags-omitted early return
        let no_flags_error = PaymentMethodService::update_disabled(
            &server,
            Request::new(crate::payment_method::UpdateDisabledRequest {
                mint_disabled: None,
                melt_disabled: None,
            }),
        )
        .await
        .expect_err("no-flags disabled mutation should be rejected");

        assert_eq!(no_flags_error.code(), Code::FailedPrecondition);
        assert_eq!(no_flags_error.message(), "configuration restart pending");

        let deposit_address_error = WalletService::create_deposit_address(
            &server,
            Request::new(crate::wallet::CreateDepositAddressRequest {}),
        )
        .await
        .expect_err("deposit-address mutation should be rejected");

        assert_eq!(deposit_address_error.code(), Code::FailedPrecondition);
        assert_eq!(
            deposit_address_error.message(),
            "configuration restart pending"
        );

        let info_motd_error = MintInfoService::update_motd(
            &server,
            Request::new(crate::info::UpdateMotdRequest {
                motd: "hello".to_owned(),
            }),
        )
        .await
        .expect_err("info mutation should be rejected");

        assert_eq!(info_motd_error.code(), Code::FailedPrecondition);
        assert_eq!(info_motd_error.message(), "configuration restart pending");

        assert!(
            MintInfoService::get_info(&server, Request::new(crate::info::GetInfoRequest {}))
                .await
                .expect("info read should remain available")
                .into_inner()
                .motd
                .is_none()
        );
    }
}
