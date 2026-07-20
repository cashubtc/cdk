use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use cdk::mint::{Mint, MintQuote};
use cdk::nuts::nut04::MintMethodSettings;
use cdk::nuts::nut05::MeltMethodSettings;
use cdk::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod};
use cdk::types::QuoteTTL;
use cdk::Amount;
use cdk_common::grpc::create_version_check_interceptor;
use cdk_common::payment::WaitPaymentResponse;
use thiserror::Error;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status};

use crate::cdk_mint_server::{CdkMint, CdkMintServer};
use crate::{
    ApplyConfigurationRequest, ApplyConfigurationResponse, ConfigurationError,
    ConfigurationManager, ConfigurationMutationGuard, ConfigurationSnapshot, ContactInfo,
    DiscardPendingConfigurationRequest, GetConfigurationRequest, GetConfigurationResponse,
    GetInfoRequest, GetInfoResponse, GetQuoteTtlRequest, GetQuoteTtlResponse,
    RotateNextKeysetRequest, RotateNextKeysetResponse, UpdateContactRequest,
    UpdateDescriptionRequest, UpdateIconUrlRequest, UpdateMotdRequest, UpdateNameRequest,
    UpdateNut04QuoteRequest, UpdateNut04Request, UpdateNut05Request, UpdateQuoteTtlRequest,
    UpdateResponse, UpdateTosUrlRequest, UpdateUrlRequest,
};

/// Error
#[derive(Debug, Error)]
pub enum Error {
    /// The RPC server has already been prepared.
    #[error("Mint RPC server is already prepared")]
    AlreadyPrepared,
    /// The RPC server has already been started.
    #[error("Mint RPC server is already started")]
    AlreadyStarted,
    /// The RPC server must be prepared before it can start serving.
    #[error("Mint RPC server has not been prepared")]
    NotPrepared,
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

struct PreparedServer {
    router: tonic::transport::server::Router,
    incoming: TcpListenerStream,
}

impl From<ConfigurationError> for Status {
    fn from(error: ConfigurationError) -> Self {
        match error {
            ConfigurationError::Invalid { message } => Self::invalid_argument(message),
            ConfigurationError::FailedPrecondition { message } => {
                Self::failed_precondition(message)
            }
            ConfigurationError::Busy { message } => Self::aborted(message),
            ConfigurationError::Internal { message } => Self::internal(message),
        }
    }
}

fn configuration_response(snapshot: ConfigurationSnapshot) -> GetConfigurationResponse {
    GetConfigurationResponse {
        active_toml: snapshot.active_toml,
        pending_toml: snapshot.pending_toml,
        restart_required: snapshot.restart_required,
    }
}

/// CDK Mint RPC Server
#[allow(missing_debug_implementations)]
pub struct MintRPCServer {
    socket_addr: SocketAddr,
    mint: Arc<Mint>,
    configuration_manager: Arc<dyn ConfigurationManager>,
    configuration_lock: Arc<Mutex<()>>,
    shutdown: Arc<Notify>,
    prepared: Option<PreparedServer>,
    handle: Option<Arc<JoinHandle<Result<(), Error>>>>,
}

impl Clone for MintRPCServer {
    fn clone(&self) -> Self {
        Self {
            socket_addr: self.socket_addr,
            mint: self.mint.clone(),
            configuration_manager: self.configuration_manager.clone(),
            configuration_lock: self.configuration_lock.clone(),
            shutdown: self.shutdown.clone(),
            prepared: None,
            handle: self.handle.clone(),
        }
    }
}

impl MintRPCServer {
    /// Creates a new MintRPCServer instance
    ///
    /// # Arguments
    /// * `addr` - The address to bind to
    /// * `port` - The port to listen on
    /// * `mint` - The Mint instance to serve
    /// * `configuration_manager` - The daemon configuration manager
    pub fn new(
        addr: &str,
        port: u16,
        mint: Arc<Mint>,
        configuration_manager: Arc<dyn ConfigurationManager>,
    ) -> Result<Self, Error> {
        Ok(Self {
            socket_addr: format!("{addr}:{port}").parse()?,
            mint,
            configuration_manager,
            configuration_lock: Arc::new(Mutex::new(())),
            shutdown: Arc::new(Notify::new()),
            prepared: None,
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
        self.prepare(tls_dir).await?;
        self.start_prepared().await
    }

    /// Binds the RPC listener and validates the complete server configuration.
    ///
    /// This reserves the configured address but does not accept connections or
    /// serve requests. Call [`Self::start_prepared`] after the daemon has
    /// completed its startup activation.
    pub async fn prepare(&mut self, tls_dir: Option<PathBuf>) -> Result<(), Error> {
        if self.handle.is_some() {
            return Err(Error::AlreadyStarted);
        }
        if self.prepared.is_some() {
            return Err(Error::AlreadyPrepared);
        }

        tracing::info!("Preparing RPC server {}", self.socket_addr);

        #[cfg(not(target_arch = "wasm32"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        // Bind before constructing and spawning the serving future so callers
        // receive address-in-use and other listener errors synchronously.
        let listener = tokio::net::TcpListener::bind(self.socket_addr).await?;
        self.socket_addr = listener.local_addr()?;
        let incoming = TcpListenerStream::new(listener);

        let router = match tls_dir {
            Some(tls_dir) => {
                tracing::info!("TLS configuration found, preparing secure server");
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

                Server::builder().tls_config(tls_config)?.add_service(
                    CdkMintServer::with_interceptor(
                        self.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::MINT_RPC_PROTOCOL_VERSION,
                        ),
                    ),
                )
            }
            None => {
                tracing::warn!("No valid TLS configuration found, preparing insecure server");
                Server::builder().add_service(CdkMintServer::with_interceptor(
                    self.clone(),
                    create_version_check_interceptor(
                        cdk_common::grpc::VERSION_HEADER,
                        cdk_common::MINT_RPC_PROTOCOL_VERSION,
                    ),
                ))
            }
        };

        self.prepared = Some(PreparedServer { router, incoming });
        Ok(())
    }

    /// Starts serving from a listener previously created by [`Self::prepare`].
    pub async fn start_prepared(&mut self) -> Result<(), Error> {
        if self.handle.is_some() {
            return Err(Error::AlreadyStarted);
        }

        let PreparedServer { router, incoming } = self.prepared.take().ok_or(Error::NotPrepared)?;

        let shutdown = self.shutdown.clone();

        self.handle = Some(Arc::new(tokio::spawn(async move {
            let server = router.serve_with_incoming_shutdown(incoming, async {
                shutdown.notified().await;
            });

            server.await?;
            Ok(())
        })));

        tracing::info!("Started RPC server {}", self.socket_addr);
        Ok(())
    }

    /// Stops the RPC server gracefully
    pub async fn stop(&self) -> Result<(), Error> {
        self.shutdown.notify_one();
        if let Some(handle) = &self.handle {
            while !handle.is_finished() {
                tracing::info!("Waiting for mint RPC server to stop");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        tracing::info!("Mint rpc server stopped");
        Ok(())
    }

    async fn begin_immediate_configuration_mutation(
        &self,
    ) -> Result<Box<dyn ConfigurationMutationGuard>, Status> {
        let mutation = self
            .configuration_manager
            .acquire_configuration_mutation()
            .await
            .map_err(Status::from)?;
        let snapshot = self
            .configuration_manager
            .get_configuration()
            .await
            .map_err(Status::from)?;
        if snapshot.pending_toml.is_some() {
            return Err(Status::failed_precondition(
                "a complete configuration document is pending; restart to activate it or discard it before applying immediate configuration updates",
            ));
        }

        Ok(mutation)
    }
}

impl Drop for MintRPCServer {
    fn drop(&mut self) {
        tracing::debug!("Dropping mint rpc server");
        self.shutdown.notify_one();
    }
}

#[tonic::async_trait]
impl CdkMint for MintRPCServer {
    /// Returns information about the mint
    async fn get_info(
        &self,
        _request: Request<GetInfoRequest>,
    ) -> Result<Response<GetInfoResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let total_issued = self
            .mint
            .total_issued()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let total_issued: Amount = Amount::try_sum(total_issued.values().cloned())
            .map_err(|_| Status::internal("Overflow".to_string()))?;

        let total_redeemed = self
            .mint
            .total_redeemed()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let total_redeemed: Amount = Amount::try_sum(total_redeemed.values().cloned())
            .map_err(|_| Status::internal("Overflow".to_string()))?;

        let contact = info
            .contact
            .unwrap_or_default()
            .into_iter()
            .map(|c| ContactInfo {
                method: c.method,
                info: c.info,
            })
            .collect();

        let response = Response::new(GetInfoResponse {
            name: info.name,
            description: info.description,
            long_description: info.description_long,
            version: info.version.map(|v| v.to_string()),
            contact,
            motd: info.motd,
            icon_url: info.icon_url,
            tos_url: info.tos_url,
            urls: info.urls.unwrap_or_default(),
            total_issued: total_issued.into(),
            total_redeemed: total_redeemed.into(),
        });

        Ok(response)
    }

    /// Returns the active and pending mint daemon configuration.
    async fn get_configuration(
        &self,
        _request: Request<GetConfigurationRequest>,
    ) -> Result<Response<GetConfigurationResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let snapshot = self
            .configuration_manager
            .get_configuration()
            .await
            .map_err(Status::from)?;

        Ok(Response::new(configuration_response(snapshot)))
    }

    /// Validates or applies a complete mint daemon configuration document.
    async fn apply_configuration(
        &self,
        request: Request<ApplyConfigurationRequest>,
    ) -> Result<Response<ApplyConfigurationResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self
            .configuration_manager
            .acquire_configuration_mutation()
            .await
            .map_err(Status::from)?;
        let request = request.into_inner();
        let outcome = self
            .configuration_manager
            .apply_configuration(request.config_toml, request.validate_only)
            .await
            .map_err(Status::from)?;

        Ok(Response::new(ApplyConfigurationResponse {
            restart_required: outcome.restart_required,
            changed_fields: outcome.changed_fields,
        }))
    }

    /// Discards restart-required mint daemon configuration.
    async fn discard_pending_configuration(
        &self,
        _request: Request<DiscardPendingConfigurationRequest>,
    ) -> Result<Response<GetConfigurationResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self
            .configuration_manager
            .acquire_configuration_mutation()
            .await
            .map_err(Status::from)?;
        let snapshot = self
            .configuration_manager
            .discard_pending_configuration()
            .await
            .map_err(Status::from)?;

        Ok(Response::new(configuration_response(snapshot)))
    }

    /// Updates the mint's message of the day
    async fn update_motd(
        &self,
        request: Request<UpdateMotdRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let motd = request.into_inner().motd;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        info.motd = Some(motd);

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's short description
    async fn update_short_description(
        &self,
        request: Request<UpdateDescriptionRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let description = request.into_inner().description;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.description = Some(description);

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's long description
    async fn update_long_description(
        &self,
        request: Request<UpdateDescriptionRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let description = request.into_inner().description;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.description_long = Some(description);

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's name
    async fn update_name(
        &self,
        request: Request<UpdateNameRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let name = request.into_inner().name;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.name = Some(name);

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's icon URL
    async fn update_icon_url(
        &self,
        request: Request<UpdateIconUrlRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let icon_url = request.into_inner().icon_url;

        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.icon_url = Some(icon_url);

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's terms of service URL
    async fn update_tos_url(
        &self,
        request: Request<UpdateTosUrlRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let tos_url = request.into_inner().tos_url;

        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.tos_url = Some(tos_url);

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Adds a URL to the mint's list of URLs
    async fn add_url(
        &self,
        request: Request<UpdateUrlRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let url = request.into_inner().url;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        let mut urls = info.urls.unwrap_or_default();
        urls.push(url);

        info.urls = Some(urls.clone());

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Removes a URL from the mint's list of URLs
    async fn remove_url(
        &self,
        request: Request<UpdateUrlRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let url = request.into_inner().url;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        let urls = info.urls;
        let mut urls = urls.clone().unwrap_or_default();

        urls.retain(|u| u != &url);

        let urls = if urls.is_empty() { None } else { Some(urls) };

        info.urls = urls;

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Adds a contact method to the mint's contact information
    async fn add_contact(
        &self,
        request: Request<UpdateContactRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let request_inner = request.into_inner();
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.contact
            .get_or_insert_with(Vec::new)
            .push(cdk::nuts::ContactInfo::new(
                request_inner.method,
                request_inner.info,
            ));

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }
    /// Removes a contact method from the mint's contact information
    async fn remove_contact(
        &self,
        request: Request<UpdateContactRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let request_inner = request.into_inner();
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        if let Some(contact) = info.contact.as_mut() {
            let contact_info =
                cdk::nuts::ContactInfo::new(request_inner.method, request_inner.info);
            contact.retain(|x| x != &contact_info);

            self.mint
                .set_mint_info(info)
                .await
                .map_err(|err| Status::internal(err.to_string()))?;
        }
        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's NUT-04 (mint) settings
    async fn update_nut04(
        &self,
        request: Request<UpdateNut04Request>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let mut nut04_settings = info.nuts.nut04.clone();

        let request_inner = request.into_inner();

        let unit = CurrencyUnit::from_str(&request_inner.unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let payment_method = PaymentMethod::from_str(&request_inner.method)
            .map_err(|_| Status::invalid_argument("Invalid method".to_string()))?;

        self.mint
            .get_payment_processor(unit.clone(), payment_method.clone())
            .map_err(|_| Status::invalid_argument("Unit payment method pair is not supported"))?;

        let current_nut04_settings = nut04_settings.remove_settings(&unit, &payment_method);

        let mut methods = nut04_settings.methods.clone();

        // Create options from the request
        let options = if let Some(options) = request_inner.options {
            Some(cdk::nuts::nut04::MintMethodOptions::Bolt11 {
                description: options.description,
            })
        } else if let Some(current_settings) = current_nut04_settings.as_ref() {
            current_settings.options.clone()
        } else {
            None
        };

        let updated_method_settings = MintMethodSettings {
            method: payment_method,
            unit,
            method_name: request_inner.method_name.or_else(|| {
                current_nut04_settings
                    .as_ref()
                    .and_then(|s| s.method_name.clone())
            }),
            min_amount: request_inner
                .min_amount
                .map(Amount::from)
                .or_else(|| current_nut04_settings.as_ref().and_then(|s| s.min_amount)),
            max_amount: request_inner
                .max_amount
                .map(Amount::from)
                .or_else(|| current_nut04_settings.as_ref().and_then(|s| s.max_amount)),
            options,
        };

        methods.push(updated_method_settings);

        nut04_settings.methods = methods;

        if let Some(disabled) = request_inner.disabled {
            nut04_settings.disabled = disabled;
        }

        info.nuts.nut04 = nut04_settings;

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's NUT-05 (melt) settings
    async fn update_nut05(
        &self,
        request: Request<UpdateNut05Request>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        let mut nut05_settings = info.nuts.nut05.clone();

        let request_inner = request.into_inner();

        let unit = CurrencyUnit::from_str(&request_inner.unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let payment_method = PaymentMethod::from_str(&request_inner.method)
            .map_err(|_| Status::invalid_argument("Invalid method".to_string()))?;

        self.mint
            .get_payment_processor(unit.clone(), payment_method.clone())
            .map_err(|_| Status::invalid_argument("Unit payment method pair is not supported"))?;

        let current_nut05_settings = nut05_settings.remove_settings(&unit, &payment_method);

        let mut methods = nut05_settings.methods;

        // Create options from the request
        let options = if let Some(options) = request_inner.options {
            Some(cdk::nuts::nut05::MeltMethodOptions::Bolt11 {
                amountless: options.amountless,
            })
        } else if let Some(current_settings) = current_nut05_settings.as_ref() {
            current_settings.options.clone()
        } else {
            None
        };

        let updated_method_settings = MeltMethodSettings {
            method: payment_method,
            unit,
            method_name: request_inner.method_name.or_else(|| {
                current_nut05_settings
                    .as_ref()
                    .and_then(|s| s.method_name.clone())
            }),
            min_amount: request_inner
                .min_amount
                .map(Amount::from)
                .or_else(|| current_nut05_settings.as_ref().and_then(|s| s.min_amount)),
            max_amount: request_inner
                .max_amount
                .map(Amount::from)
                .or_else(|| current_nut05_settings.as_ref().and_then(|s| s.max_amount)),
            options,
        };

        methods.push(updated_method_settings);
        nut05_settings.methods = methods;

        if let Some(disabled) = request_inner.disabled {
            nut05_settings.disabled = disabled;
        }

        info.nuts.nut05 = nut05_settings;

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's quote time-to-live settings
    async fn update_quote_ttl(
        &self,
        request: Request<UpdateQuoteTtlRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let _database_configuration = self.begin_immediate_configuration_mutation().await?;
        let current_ttl = self
            .mint
            .quote_ttl()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let request = request.into_inner();

        let quote_ttl = QuoteTTL {
            mint_ttl: request.mint_ttl.unwrap_or(current_ttl.mint_ttl),
            melt_ttl: request.melt_ttl.unwrap_or(current_ttl.melt_ttl),
        };

        self.mint
            .set_quote_ttl(quote_ttl)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(UpdateResponse {}))
    }

    /// Gets the mint's quote time-to-live settings
    async fn get_quote_ttl(
        &self,
        _request: Request<GetQuoteTtlRequest>,
    ) -> Result<Response<GetQuoteTtlResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let ttl = self
            .mint
            .quote_ttl()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(GetQuoteTtlResponse {
            mint_ttl: ttl.mint_ttl,
            melt_ttl: ttl.melt_ttl,
        }))
    }

    /// Updates a specific NUT-04 quote's state
    async fn update_nut04_quote(
        &self,
        request: Request<UpdateNut04QuoteRequest>,
    ) -> Result<Response<UpdateNut04QuoteRequest>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let request = request.into_inner();
        let quote_id = request
            .quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote id".to_string()))?;

        let state = MintQuoteState::from_str(&request.state)
            .map_err(|_| Status::invalid_argument("Invalid quote state".to_string()))?;

        let mint_quote = self
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::invalid_argument("Could not find quote".to_string()))?
            .ok_or(Status::invalid_argument("Could not find quote".to_string()))?;

        match state {
            MintQuoteState::Paid => {
                // Create a dummy payment response
                let response = WaitPaymentResponse {
                    payment_id: mint_quote.request_lookup_id.to_string(),
                    payment_amount: mint_quote.clone().amount.unwrap_or(cdk::Amount::new(
                        mint_quote.amount_paid().value(),
                        mint_quote.unit.clone(),
                    )),
                    payment_identifier: mint_quote.request_lookup_id.clone(),
                };

                let localstore = self.mint.localstore();
                let mut tx = localstore
                    .begin_transaction()
                    .await
                    .map_err(|_| Status::internal("Could not start db transaction".to_string()))?;

                // Re-fetch the mint quote within the transaction to lock it
                let mut mint_quote = tx
                    .get_mint_quote(&quote_id)
                    .await
                    .map_err(|_| {
                        Status::internal("Could not get quote in transaction".to_string())
                    })?
                    .ok_or(Status::invalid_argument(
                        "Quote not found in transaction".to_string(),
                    ))?;

                let should_notify = self
                    .mint
                    .pay_mint_quote(&mut tx, &mut mint_quote, response)
                    .await
                    .map_err(|_| Status::internal("Could not process payment".to_string()))?;

                tx.commit()
                    .await
                    .map_err(|_| Status::internal("Could not commit db transaction".to_string()))?;

                // Publish notification AFTER transaction commits
                if should_notify {
                    self.mint
                        .pubsub_manager()
                        .mint_quote_payment(&mint_quote, mint_quote.amount_paid());
                }
            }
            _ => {
                // Create a new quote with the same values
                let quote = MintQuote::new(
                    Some(mint_quote.id.clone()),          // id
                    mint_quote.request.clone(),           // request
                    mint_quote.unit.clone(),              // unit
                    mint_quote.amount.clone(),            // amount
                    mint_quote.expiry,                    // expiry
                    mint_quote.request_lookup_id.clone(), // request_lookup_id
                    mint_quote.pubkey,                    // pubkey
                    mint_quote.amount_issued(),           // amount_issued
                    mint_quote.amount_paid(),             // amount_paid
                    mint_quote.payment_method.clone(),    // method
                    0,                                    // created_at
                    0,                                    // updated_at
                    vec![],                               // blinded_messages
                    vec![],                               // payment_ids
                    None,                                 // extra_json
                );

                let mint_store = self.mint.localstore();
                let mut tx = mint_store
                    .begin_transaction()
                    .await
                    .map_err(|_| Status::internal("Could not update quote".to_string()))?;
                tx.add_mint_quote(quote.clone())
                    .await
                    .map_err(|_| Status::internal("Could not update quote".to_string()))?;
                tx.commit()
                    .await
                    .map_err(|_| Status::internal("Could not update quote".to_string()))?;
            }
        }

        let mint_quote = self
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::invalid_argument("Could not find quote".to_string()))?
            .ok_or(Status::invalid_argument("Could not find quote".to_string()))?;

        Ok(Response::new(UpdateNut04QuoteRequest {
            state: mint_quote.state().to_string(),
            quote_id: mint_quote.id.to_string(),
        }))
    }

    /// Rotates to the next keyset for the specified currency unit
    async fn rotate_next_keyset(
        &self,
        request: Request<RotateNextKeysetRequest>,
    ) -> Result<Response<RotateNextKeysetResponse>, Status> {
        let _configuration = self.configuration_lock.lock().await;
        let request = request.into_inner();

        let unit = CurrencyUnit::from_str(&request.unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let amounts = request.amounts;

        let keyset_info = self
            .mint
            .rotate_keyset(
                unit,
                amounts,
                request.input_fee_ppk.unwrap_or(0),
                request.use_keyset_v2.unwrap_or(true),
                request.final_expiry,
            )
            .await
            .map_err(|_| Status::invalid_argument("Could not rotate keyset".to_string()))?;

        Ok(Response::new(RotateNextKeysetResponse {
            id: keyset_info.id.to_string(),
            unit: keyset_info.unit.to_string(),
            amounts: keyset_info.amounts,
            input_fee_ppk: keyset_info.input_fee_ppk,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use bip39::Mnemonic;
    use cdk::mint::{MintBuilder, MintMeltLimits};
    use cdk::nuts::{CurrencyUnit, PaymentMethod};
    use cdk::types::QuoteTTL;
    use cdk_common::nut00::KnownMethod;
    use cdk_fake_wallet::FakeWallet;
    use tonic::Request;

    use super::*;
    use crate::cdk_mint_server::CdkMint;
    use crate::{
        ApplyConfigurationOutcome, ApplyConfigurationRequest, ConfigurationError,
        ConfigurationManager, ConfigurationMutationGuard, ConfigurationSnapshot,
        DiscardPendingConfigurationRequest, GetConfigurationRequest, GetInfoRequest,
        UpdateTosUrlRequest,
    };

    #[derive(Debug)]
    struct TestConfigurationMutationGuard;

    impl ConfigurationMutationGuard for TestConfigurationMutationGuard {}

    #[derive(Debug)]
    struct TestConfigurationManager {
        snapshot: Mutex<ConfigurationSnapshot>,
        apply_outcome: ApplyConfigurationOutcome,
        last_apply: Mutex<Option<(String, bool)>>,
        mutation_acquisitions: AtomicUsize,
    }

    impl Default for TestConfigurationManager {
        fn default() -> Self {
            Self {
                snapshot: Mutex::new(ConfigurationSnapshot {
                    active_toml: "format_version = 1\n".to_string(),
                    pending_toml: None,
                    restart_required: false,
                }),
                apply_outcome: ApplyConfigurationOutcome {
                    restart_required: true,
                    changed_fields: vec!["ln".to_string()],
                },
                last_apply: Mutex::new(None),
                mutation_acquisitions: AtomicUsize::new(0),
            }
        }
    }

    #[tonic::async_trait]
    impl ConfigurationManager for TestConfigurationManager {
        async fn acquire_configuration_mutation(
            &self,
        ) -> Result<Box<dyn ConfigurationMutationGuard>, ConfigurationError> {
            self.mutation_acquisitions.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(TestConfigurationMutationGuard))
        }

        async fn get_configuration(&self) -> Result<ConfigurationSnapshot, ConfigurationError> {
            Ok(self.snapshot.lock().await.clone())
        }

        async fn apply_configuration(
            &self,
            config_toml: String,
            validate_only: bool,
        ) -> Result<ApplyConfigurationOutcome, ConfigurationError> {
            *self.last_apply.lock().await = Some((config_toml, validate_only));
            Ok(self.apply_outcome.clone())
        }

        async fn discard_pending_configuration(
            &self,
        ) -> Result<ConfigurationSnapshot, ConfigurationError> {
            let mut snapshot = self.snapshot.lock().await;
            snapshot.pending_toml = None;
            snapshot.restart_required = false;
            Ok(snapshot.clone())
        }
    }

    async fn create_test_rpc_server() -> MintRPCServer {
        create_test_rpc_server_with_manager(Arc::new(TestConfigurationManager::default())).await
    }

    async fn create_test_rpc_server_with_manager(
        configuration_manager: Arc<dyn ConfigurationManager>,
    ) -> MintRPCServer {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());

        let mut mint_builder = MintBuilder::new(db.clone());

        let fee_reserve = cdk::types::FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        };

        let ln_fake = FakeWallet::new(
            fee_reserve,
            HashMap::default(),
            HashSet::default(),
            2,
            CurrencyUnit::Sat,
        );

        mint_builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(ln_fake),
            )
            .await
            .unwrap();

        let mnemonic = Mnemonic::generate(12).unwrap();

        mint_builder = mint_builder
            .with_name("test mint".to_string())
            .with_description("test mint".to_string());

        let mint = mint_builder
            .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
            .await
            .unwrap();

        mint.set_quote_ttl(QuoteTTL::new(10000, 10000))
            .await
            .unwrap();

        mint.start().await.unwrap();

        MintRPCServer {
            socket_addr: "127.0.0.1:0".parse().unwrap(),
            mint: Arc::new(mint),
            configuration_manager,
            configuration_lock: Arc::new(Mutex::new(())),
            shutdown: Arc::new(Notify::new()),
            prepared: None,
            handle: None,
        }
    }

    #[tokio::test]
    async fn start_reports_listener_bind_errors() {
        let occupied_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let occupied_address = occupied_listener
            .local_addr()
            .expect("test listener should have a local address");
        let mut server = create_test_rpc_server().await;
        server.socket_addr = occupied_address;

        let error = server
            .start(None)
            .await
            .expect_err("starting on an occupied address must fail");

        assert!(
            matches!(error, Error::Io(source) if source.kind() == std::io::ErrorKind::AddrInUse)
        );
    }

    #[tokio::test]
    async fn prepared_server_does_not_serve_until_started() {
        let mut server = create_test_rpc_server().await;

        server
            .prepare(None)
            .await
            .expect("RPC server should prepare");

        assert!(server.prepared.is_some());
        assert!(server.handle.is_none());

        server
            .start_prepared()
            .await
            .expect("prepared RPC server should start");

        assert!(server.prepared.is_none());
        assert!(server.handle.is_some());
        server.stop().await.expect("RPC server should stop");
    }

    #[tokio::test]
    async fn start_prepared_requires_prepare() {
        let mut server = create_test_rpc_server().await;

        let error = server
            .start_prepared()
            .await
            .expect_err("unprepared RPC server must not start");

        assert!(matches!(error, Error::NotPrepared));
    }

    #[tokio::test]
    async fn configuration_handlers_delegate_to_manager() {
        let manager = Arc::new(TestConfigurationManager::default());
        {
            let mut snapshot = manager.snapshot.lock().await;
            snapshot.pending_toml = Some("format_version = 1\nname = \"pending\"\n".to_string());
            snapshot.restart_required = true;
        }
        let server = create_test_rpc_server_with_manager(manager.clone()).await;

        let snapshot = server
            .get_configuration(Request::new(GetConfigurationRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(snapshot.active_toml, "format_version = 1\n");
        assert!(snapshot.pending_toml.is_some());
        assert!(snapshot.restart_required);
        assert_eq!(manager.mutation_acquisitions.load(Ordering::SeqCst), 0);

        let document = "format_version = 1\nname = \"new\"\n".to_string();
        let outcome = server
            .apply_configuration(Request::new(ApplyConfigurationRequest {
                config_toml: document.clone(),
                validate_only: true,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(outcome.restart_required);
        assert_eq!(outcome.changed_fields, vec!["ln"]);
        assert_eq!(*manager.last_apply.lock().await, Some((document, true)));
        assert_eq!(manager.mutation_acquisitions.load(Ordering::SeqCst), 1);

        let snapshot = server
            .discard_pending_configuration(Request::new(DiscardPendingConfigurationRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert!(snapshot.pending_toml.is_none());
        assert!(!snapshot.restart_required);
        assert_eq!(manager.mutation_acquisitions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn configuration_errors_map_to_tonic_status_codes() {
        let invalid = Status::from(ConfigurationError::Invalid {
            message: "bad input".to_string(),
        });
        let precondition = Status::from(ConfigurationError::FailedPrecondition {
            message: "not initialized".to_string(),
        });
        let busy = Status::from(ConfigurationError::Busy {
            message: "retry".to_string(),
        });
        let internal = Status::from(ConfigurationError::Internal {
            message: "database unavailable".to_string(),
        });

        assert_eq!(invalid.code(), tonic::Code::InvalidArgument);
        assert_eq!(precondition.code(), tonic::Code::FailedPrecondition);
        assert_eq!(busy.code(), tonic::Code::Aborted);
        assert_eq!(internal.code(), tonic::Code::Internal);
    }

    #[tokio::test]
    async fn test_get_info_tos_url_none_when_not_set() {
        let server = create_test_rpc_server().await;

        let response = server
            .get_info(Request::new(GetInfoRequest {}))
            .await
            .unwrap();

        assert!(response.into_inner().tos_url.is_none());
    }

    #[tokio::test]
    async fn test_get_info_includes_tos_url() {
        let server = create_test_rpc_server().await;
        let tos = "https://example.com/tos";

        let mut info = server.mint.mint_info().await.unwrap();
        info.tos_url = Some(tos.to_string());
        server.mint.set_mint_info(info).await.unwrap();

        let response = server
            .get_info(Request::new(GetInfoRequest {}))
            .await
            .unwrap();

        assert_eq!(response.into_inner().tos_url.unwrap(), tos);
    }

    #[tokio::test]
    async fn test_update_tos_url() {
        let server = create_test_rpc_server().await;
        let tos = "https://example.com/terms";

        server
            .update_tos_url(Request::new(UpdateTosUrlRequest {
                tos_url: tos.to_string(),
            }))
            .await
            .unwrap();

        let response = server
            .get_info(Request::new(GetInfoRequest {}))
            .await
            .unwrap();

        assert_eq!(response.into_inner().tos_url.unwrap(), tos);
    }

    #[tokio::test]
    async fn immediate_configuration_updates_are_rejected_while_a_document_is_pending() {
        let manager = Arc::new(TestConfigurationManager::default());
        {
            let mut snapshot = manager.snapshot.lock().await;
            snapshot.pending_toml = Some("name = \"pending\"\n".to_string());
            snapshot.restart_required = true;
        }
        let server = create_test_rpc_server_with_manager(manager.clone()).await;
        let original_name = server.mint.mint_info().await.unwrap().name;

        let error = server
            .update_name(Request::new(UpdateNameRequest {
                name: "must not be applied".to_string(),
            }))
            .await
            .expect_err("immediate update must be rejected");

        assert_eq!(error.code(), tonic::Code::FailedPrecondition);
        assert_eq!(server.mint.mint_info().await.unwrap().name, original_name);
        assert_eq!(manager.mutation_acquisitions.load(Ordering::SeqCst), 1);
    }
}
