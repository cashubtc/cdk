use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use cdk::mint::{Mint, MintKeySetInfo, MintQuote};
use cdk::nuts::nut04::MintMethodSettings;
use cdk::nuts::nut05::MeltMethodSettings;
use cdk::nuts::{CurrencyUnit, MintInfo, MintQuoteState, PaymentMethod};
use cdk::types::QuoteTTL;
use cdk::Amount;
use cdk_common::grpc::create_version_check_interceptor;
use cdk_common::payment::WaitPaymentResponse;
use thiserror::Error;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status};

use crate::info::mint_info_service_server::{MintInfoService, MintInfoServiceServer};
use crate::keyset::keyset_service_server::{KeysetService, KeysetServiceServer};
use crate::payment_method::payment_method_service_server::{
    PaymentMethodService, PaymentMethodServiceServer,
};
use crate::quote::quote_service_server::{QuoteService, QuoteServiceServer};
use crate::wallet::wallet_service_server::{WalletService, WalletServiceServer};
use crate::{DynWalletInfoProvider, WalletAddressPage, WalletTransactionPage};

const DEFAULT_TRANSACTION_LIMIT: u32 = 20;
const MAX_TRANSACTION_LIMIT: u32 = 100;
const DEFAULT_ADDRESS_LIMIT: u32 = 100;
const MAX_ADDRESS_LIMIT: u32 = 1_000;

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

    fn ensure_mint_quote_state_override_allowed(&self) -> Result<(), Status> {
        if !self.allow_mint_quote_payment_override {
            return Err(Status::permission_denied(
                "Mint quote state override is disabled",
            ));
        }

        Ok(())
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

    /// Applies a mutation to the mint's info and returns the info in effect
    /// after the update
    async fn update_mint_info_with(
        &self,
        mutate: impl FnOnce(&mut MintInfo) + Send,
    ) -> Result<MintInfo, Status> {
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        mutate(&mut info);

        self.mint
            .set_mint_info(info.clone())
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(info)
    }

    /// Rotates to the next keyset for the given unit
    async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        amounts: Vec<u64>,
        input_fee_ppk: Option<u64>,
        use_keyset_v2: Option<bool>,
        final_expiry: Option<u64>,
    ) -> Result<MintKeySetInfo, Status> {
        self.ensure_mutation_allowed().await?;
        self.mint
            .rotate_keyset(
                unit,
                amounts,
                input_fee_ppk.unwrap_or(0),
                use_keyset_v2.unwrap_or(true),
                final_expiry,
            )
            .await
            .map_err(|_| Status::invalid_argument("Could not rotate keyset".to_string()))
    }

    /// Returns the mint's quote time-to-live settings
    async fn quote_ttl(&self) -> Result<QuoteTTL, Status> {
        self.mint
            .quote_ttl()
            .await
            .map_err(|err| Status::internal(err.to_string()))
    }

    /// Updates the mint's quote time-to-live settings, keeping the current
    /// value of any setting that is not given
    ///
    /// Returns the settings in effect after the update.
    async fn set_quote_ttl(
        &self,
        mint_ttl: Option<u64>,
        melt_ttl: Option<u64>,
    ) -> Result<QuoteTTL, Status> {
        let current_ttl = self.quote_ttl().await?;

        let quote_ttl = QuoteTTL {
            mint_ttl: mint_ttl.unwrap_or(current_ttl.mint_ttl),
            melt_ttl: melt_ttl.unwrap_or(current_ttl.melt_ttl),
        };

        self.mint
            .set_quote_ttl(quote_ttl)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(quote_ttl)
    }

    /// Records a payment against a mint quote as though the payment backend
    /// had reported it, marking the quote paid
    ///
    /// Returns the quote as it stands after the update.
    async fn set_mint_quote_paid(&self, quote_id: &str) -> Result<MintQuote, Status> {
        self.ensure_mint_quote_state_override_allowed()?;

        let quote_id = quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote id".to_string()))?;

        let mint_quote = self
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::invalid_argument("Could not find quote".to_string()))?
            .ok_or(Status::invalid_argument("Could not find quote".to_string()))?;

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
            .map_err(|_| Status::internal("Could not get quote in transaction".to_string()))?
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

        self.mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::invalid_argument("Could not find quote".to_string()))?
            .ok_or(Status::invalid_argument("Could not find quote".to_string()))
    }

    fn wallet_info_provider(&self) -> Result<&DynWalletInfoProvider, Status> {
        self.wallet_info_provider.as_ref().ok_or_else(|| {
            Status::failed_precondition("No on-chain wallet information provider is configured")
        })
    }

    /// Updates the settings of one mint (NUT-04) payment method, keeping the
    /// current value of any setting that is not given
    ///
    /// Returns the method settings in effect after the update.
    async fn set_mint_method(
        &self,
        unit: &str,
        method: &str,
        min_amount: Option<u64>,
        max_amount: Option<u64>,
        options: Option<cdk::nuts::nut04::MintMethodOptions>,
        method_name: Option<String>,
    ) -> Result<MintMethodSettings, Status> {
        self.ensure_mutation_allowed().await?;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let unit = CurrencyUnit::from_str(unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let payment_method = PaymentMethod::from_str(method)
            .map_err(|_| Status::invalid_argument("Invalid method".to_string()))?;

        self.mint
            .get_payment_processor(unit.clone(), payment_method.clone())
            .map_err(|_| Status::invalid_argument("Unit payment method pair is not supported"))?;

        let current_nut04_settings = info.nuts.nut04.remove_settings(&unit, &payment_method);

        let updated_method_settings = MintMethodSettings {
            method: payment_method,
            unit,
            method_name: method_name.or_else(|| {
                current_nut04_settings
                    .as_ref()
                    .and_then(|s| s.method_name.clone())
            }),
            min_amount: min_amount
                .map(Amount::from)
                .or_else(|| current_nut04_settings.as_ref().and_then(|s| s.min_amount)),
            max_amount: max_amount
                .map(Amount::from)
                .or_else(|| current_nut04_settings.as_ref().and_then(|s| s.max_amount)),
            options: options.or_else(|| {
                current_nut04_settings
                    .as_ref()
                    .and_then(|s| s.options.clone())
            }),
        };

        info.nuts
            .nut04
            .methods
            .push(updated_method_settings.clone());

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(updated_method_settings)
    }

    /// Enables or disables minting and melting for the whole mint, keeping
    /// the current value of any flag that is not given
    ///
    /// Returns the (minting disabled, melting disabled) flags in effect after
    /// the update, applied in a single write.
    async fn set_disabled(
        &self,
        mint_disabled: Option<bool>,
        melt_disabled: Option<bool>,
    ) -> Result<(bool, bool), Status> {
        self.ensure_mutation_allowed().await?;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        if mint_disabled.is_none() && melt_disabled.is_none() {
            return Ok((info.nuts.nut04.disabled, info.nuts.nut05.disabled));
        }

        if let Some(disabled) = mint_disabled {
            info.nuts.nut04.disabled = disabled;
        }

        if let Some(disabled) = melt_disabled {
            info.nuts.nut05.disabled = disabled;
        }

        let flags = (info.nuts.nut04.disabled, info.nuts.nut05.disabled);

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(flags)
    }

    /// Updates the settings of one melt (NUT-05) payment method, keeping the
    /// current value of any setting that is not given
    ///
    /// Returns the method settings in effect after the update.
    async fn set_melt_method(
        &self,
        unit: &str,
        method: &str,
        min_amount: Option<u64>,
        max_amount: Option<u64>,
        options: Option<cdk::nuts::nut05::MeltMethodOptions>,
        method_name: Option<String>,
    ) -> Result<MeltMethodSettings, Status> {
        self.ensure_mutation_allowed().await?;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let unit = CurrencyUnit::from_str(unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let payment_method = PaymentMethod::from_str(method)
            .map_err(|_| Status::invalid_argument("Invalid method".to_string()))?;

        self.mint
            .get_payment_processor(unit.clone(), payment_method.clone())
            .map_err(|_| Status::invalid_argument("Unit payment method pair is not supported"))?;

        let current_nut05_settings = info.nuts.nut05.remove_settings(&unit, &payment_method);

        let updated_method_settings = MeltMethodSettings {
            method: payment_method,
            unit,
            method_name: method_name.or_else(|| {
                current_nut05_settings
                    .as_ref()
                    .and_then(|s| s.method_name.clone())
            }),
            min_amount: min_amount
                .map(Amount::from)
                .or_else(|| current_nut05_settings.as_ref().and_then(|s| s.min_amount)),
            max_amount: max_amount
                .map(Amount::from)
                .or_else(|| current_nut05_settings.as_ref().and_then(|s| s.max_amount)),
            options: options.or_else(|| {
                current_nut05_settings
                    .as_ref()
                    .and_then(|s| s.options.clone())
            }),
        };

        info.nuts
            .nut05
            .methods
            .push(updated_method_settings.clone());

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(updated_method_settings)
    }
}

impl Drop for MintRPCServer {
    fn drop(&mut self) {
        tracing::debug!("Dropping mint rpc server");
        self.shutdown.notify_one();
    }
}

/// Converts an empty string to `None`, treating empty updates as clears
fn non_empty(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Maps mint info contacts into their proto representation
fn info_contacts(contact: Option<Vec<cdk::nuts::ContactInfo>>) -> Vec<crate::info::ContactInfo> {
    contact
        .unwrap_or_default()
        .into_iter()
        .map(|c| crate::info::ContactInfo {
            method: c.method,
            info: c.info,
        })
        .collect()
}

#[tonic::async_trait]
impl MintInfoService for MintRPCServer {
    /// Returns the mint's public metadata
    async fn get_info(
        &self,
        _request: Request<crate::info::GetInfoRequest>,
    ) -> Result<Response<crate::info::GetInfoResponse>, Status> {
        let info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(crate::info::GetInfoResponse {
            name: info.name,
            version: info.version.map(|v| v.to_string()),
            description: info.description,
            long_description: info.description_long,
            contact: info_contacts(info.contact),
            motd: info.motd,
            icon_url: info.icon_url,
            urls: info.urls.unwrap_or_default(),
            tos_url: info.tos_url,
        }))
    }

    /// Sets the mint's name
    async fn update_name(
        &self,
        request: Request<crate::info::UpdateNameRequest>,
    ) -> Result<Response<crate::info::UpdateNameResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let name = non_empty(request.into_inner().name);
        let info = self.update_mint_info_with(|info| info.name = name).await?;
        Ok(Response::new(crate::info::UpdateNameResponse {
            name: info.name,
        }))
    }

    /// Sets the mint's message of the day
    async fn update_motd(
        &self,
        request: Request<crate::info::UpdateMotdRequest>,
    ) -> Result<Response<crate::info::UpdateMotdResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let motd = non_empty(request.into_inner().motd);
        let info = self.update_mint_info_with(|info| info.motd = motd).await?;
        Ok(Response::new(crate::info::UpdateMotdResponse {
            motd: info.motd,
        }))
    }

    /// Sets the mint's short description
    async fn update_short_description(
        &self,
        request: Request<crate::info::UpdateShortDescriptionRequest>,
    ) -> Result<Response<crate::info::UpdateShortDescriptionResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let description = non_empty(request.into_inner().description);
        let info = self
            .update_mint_info_with(|info| info.description = description)
            .await?;
        Ok(Response::new(crate::info::UpdateShortDescriptionResponse {
            description: info.description,
        }))
    }

    /// Sets the mint's long description
    async fn update_long_description(
        &self,
        request: Request<crate::info::UpdateLongDescriptionRequest>,
    ) -> Result<Response<crate::info::UpdateLongDescriptionResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let long_description = non_empty(request.into_inner().long_description);
        let info = self
            .update_mint_info_with(|info| info.description_long = long_description)
            .await?;
        Ok(Response::new(crate::info::UpdateLongDescriptionResponse {
            long_description: info.description_long,
        }))
    }

    /// Sets the mint's icon URL
    async fn update_icon_url(
        &self,
        request: Request<crate::info::UpdateIconUrlRequest>,
    ) -> Result<Response<crate::info::UpdateIconUrlResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let icon_url = non_empty(request.into_inner().icon_url);
        let info = self
            .update_mint_info_with(|info| info.icon_url = icon_url)
            .await?;
        Ok(Response::new(crate::info::UpdateIconUrlResponse {
            icon_url: info.icon_url,
        }))
    }

    /// Sets the mint's terms of service URL
    async fn update_tos_url(
        &self,
        request: Request<crate::info::UpdateTosUrlRequest>,
    ) -> Result<Response<crate::info::UpdateTosUrlResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let tos_url = non_empty(request.into_inner().tos_url);
        let info = self
            .update_mint_info_with(|info| info.tos_url = tos_url)
            .await?;
        Ok(Response::new(crate::info::UpdateTosUrlResponse {
            tos_url: info.tos_url,
        }))
    }

    /// Adds a mint URL
    async fn add_url(
        &self,
        request: Request<crate::info::AddUrlRequest>,
    ) -> Result<Response<crate::info::AddUrlResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let url = request.into_inner().url;
        if url.is_empty() {
            return Err(Status::invalid_argument(
                "URL must not be empty".to_string(),
            ));
        }
        let info = self
            .update_mint_info_with(|info| {
                let urls = info.urls.get_or_insert_with(Vec::new);
                if !urls.contains(&url) {
                    urls.push(url);
                }
            })
            .await?;
        Ok(Response::new(crate::info::AddUrlResponse {
            urls: info.urls.unwrap_or_default(),
        }))
    }

    /// Removes a mint URL
    async fn remove_url(
        &self,
        request: Request<crate::info::RemoveUrlRequest>,
    ) -> Result<Response<crate::info::RemoveUrlResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let url = request.into_inner().url;
        let info = self
            .update_mint_info_with(|info| {
                let mut urls = info.urls.take().unwrap_or_default();
                urls.retain(|u| u != &url);
                info.urls = if urls.is_empty() { None } else { Some(urls) };
            })
            .await?;
        Ok(Response::new(crate::info::RemoveUrlResponse {
            urls: info.urls.unwrap_or_default(),
        }))
    }

    /// Adds a contact entry
    async fn add_contact(
        &self,
        request: Request<crate::info::AddContactRequest>,
    ) -> Result<Response<crate::info::AddContactResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let request = request.into_inner();
        if request.method.is_empty() {
            return Err(Status::invalid_argument(
                "Contact method must not be empty".to_string(),
            ));
        }
        if request.info.is_empty() {
            return Err(Status::invalid_argument(
                "Contact info must not be empty".to_string(),
            ));
        }
        let contact = cdk::nuts::ContactInfo::new(request.method, request.info);
        let info = self
            .update_mint_info_with(|info| {
                let contacts = info.contact.get_or_insert_with(Vec::new);
                if !contacts.contains(&contact) {
                    contacts.push(contact);
                }
            })
            .await?;
        Ok(Response::new(crate::info::AddContactResponse {
            contact: info_contacts(info.contact),
        }))
    }

    /// Removes a contact entry
    async fn remove_contact(
        &self,
        request: Request<crate::info::RemoveContactRequest>,
    ) -> Result<Response<crate::info::RemoveContactResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let request = request.into_inner();
        let contact = cdk::nuts::ContactInfo::new(request.method, request.info);
        let info = self
            .update_mint_info_with(|info| {
                if let Some(contacts) = info.contact.as_mut() {
                    contacts.retain(|c| c != &contact);
                }
                if info.contact.as_ref().is_some_and(Vec::is_empty) {
                    info.contact = None;
                }
            })
            .await?;
        Ok(Response::new(crate::info::RemoveContactResponse {
            contact: info_contacts(info.contact),
        }))
    }
}

#[tonic::async_trait]
impl KeysetService for MintRPCServer {
    /// Rotates to the next keyset for the specified currency unit
    async fn rotate_next_keyset(
        &self,
        request: Request<crate::keyset::RotateNextKeysetRequest>,
    ) -> Result<Response<crate::keyset::RotateNextKeysetResponse>, Status> {
        let request = request.into_inner();

        let unit = CurrencyUnit::from_str(&request.unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let keyset_info = self
            .rotate_keyset(
                unit,
                request.amounts,
                request.input_fee_ppk,
                request.use_keyset_v2,
                request.final_expiry,
            )
            .await?;

        Ok(Response::new(crate::keyset::RotateNextKeysetResponse {
            id: keyset_info.id.to_string(),
            unit: keyset_info.unit.to_string(),
            amounts: keyset_info.amounts,
            input_fee_ppk: keyset_info.input_fee_ppk,
        }))
    }
}

#[tonic::async_trait]
impl QuoteService for MintRPCServer {
    /// Gets the mint's quote time-to-live settings
    async fn get_quote_ttl(
        &self,
        _request: Request<crate::quote::GetQuoteTtlRequest>,
    ) -> Result<Response<crate::quote::GetQuoteTtlResponse>, Status> {
        let ttl = self.quote_ttl().await?;

        Ok(Response::new(crate::quote::GetQuoteTtlResponse {
            mint_ttl: ttl.mint_ttl,
            melt_ttl: ttl.melt_ttl,
        }))
    }

    /// Updates the mint's quote time-to-live settings
    async fn update_quote_ttl(
        &self,
        request: Request<crate::quote::UpdateQuoteTtlRequest>,
    ) -> Result<Response<crate::quote::UpdateQuoteTtlResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let request = request.into_inner();

        let ttl = self
            .set_quote_ttl(request.mint_ttl, request.melt_ttl)
            .await?;

        Ok(Response::new(crate::quote::UpdateQuoteTtlResponse {
            mint_ttl: ttl.mint_ttl,
            melt_ttl: ttl.melt_ttl,
        }))
    }

    /// Force-marks a mint quote as paid
    async fn update_mint_quote_state(
        &self,
        request: Request<crate::quote::UpdateMintQuoteStateRequest>,
    ) -> Result<Response<crate::quote::UpdateMintQuoteStateResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let request = request.into_inner();

        match request.state() {
            crate::quote::MintQuoteState::Paid => (),
            crate::quote::MintQuoteState::Unpaid => {
                return Err(Status::invalid_argument(
                    "Cannot unpay a quote: payments cannot be retracted".to_string(),
                ));
            }
            crate::quote::MintQuoteState::Issued => {
                return Err(Status::invalid_argument(
                    "Cannot issue a quote: no signatures would back the issuance".to_string(),
                ));
            }
            crate::quote::MintQuoteState::Unspecified => {
                return Err(Status::invalid_argument(
                    "Quote state is required".to_string(),
                ));
            }
        }

        let mint_quote = self.set_mint_quote_paid(&request.quote_id).await?;

        Ok(Response::new(crate::quote::UpdateMintQuoteStateResponse {
            quote_id: mint_quote.id.to_string(),
            state: crate::quote::MintQuoteState::from(mint_quote.state()).into(),
        }))
    }
}

#[tonic::async_trait]
impl WalletService for MintRPCServer {
    /// Creates an on-chain address for operator deposits.
    async fn create_deposit_address(
        &self,
        _request: Request<crate::wallet::CreateDepositAddressRequest>,
    ) -> Result<Response<crate::wallet::CreateDepositAddressResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let address = self
            .wallet_info_provider()?
            .create_deposit_address()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(crate::wallet::CreateDepositAddressResponse {
            address,
        }))
    }

    /// Gets the on-chain wallet balance.
    async fn get_balance(
        &self,
        _request: Request<crate::wallet::GetBalanceRequest>,
    ) -> Result<Response<crate::wallet::GetBalanceResponse>, Status> {
        let balance = self
            .wallet_info_provider()?
            .get_balance()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(balance))
    }

    /// Lists on-chain wallet transactions.
    async fn list_transactions(
        &self,
        request: Request<crate::wallet::ListTransactionsRequest>,
    ) -> Result<Response<crate::wallet::ListTransactionsResponse>, Status> {
        let request = request.into_inner();
        let limit = page_limit(
            request.limit,
            DEFAULT_TRANSACTION_LIMIT,
            MAX_TRANSACTION_LIMIT,
        )?;

        let WalletTransactionPage {
            transactions,
            total,
        } = self
            .wallet_info_provider()?
            .list_transactions(request.offset as usize, limit)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(crate::wallet::ListTransactionsResponse {
            transactions,
            total,
        }))
    }

    /// Lists addresses revealed by the on-chain wallet.
    async fn list_addresses(
        &self,
        request: Request<crate::wallet::ListAddressesRequest>,
    ) -> Result<Response<crate::wallet::ListAddressesResponse>, Status> {
        let request = request.into_inner();
        let limit = page_limit(request.limit, DEFAULT_ADDRESS_LIMIT, MAX_ADDRESS_LIMIT)?;

        let WalletAddressPage { addresses, total } = self
            .wallet_info_provider()?
            .list_addresses(request.offset as usize, limit)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(crate::wallet::ListAddressesResponse {
            addresses,
            total,
        }))
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

impl From<MintQuoteState> for crate::quote::MintQuoteState {
    fn from(state: MintQuoteState) -> Self {
        match state {
            MintQuoteState::Unpaid => Self::Unpaid,
            MintQuoteState::Paid => Self::Paid,
            MintQuoteState::Issued => Self::Issued,
        }
    }
}

#[tonic::async_trait]
impl PaymentMethodService for MintRPCServer {
    /// Updates the settings of one mint (NUT-04) payment method
    async fn update_mint_method(
        &self,
        request: Request<crate::payment_method::UpdateMintMethodRequest>,
    ) -> Result<Response<crate::payment_method::UpdateMintMethodResponse>, Status> {
        let request = request.into_inner();

        if request.options.is_some()
            && PaymentMethod::from_str(&request.method).is_ok_and(|method| !method.is_bolt11())
        {
            return Err(Status::invalid_argument(
                "Options can only be set on the bolt11 method".to_string(),
            ));
        }

        let options = request
            .options
            .map(|options| cdk::nuts::nut04::MintMethodOptions::Bolt11 {
                description: options.description,
            });

        let settings = self
            .set_mint_method(
                &request.unit,
                &request.method,
                request.min_amount,
                request.max_amount,
                options,
                request.method_name,
            )
            .await?;

        Ok(Response::new(settings.into()))
    }

    /// Updates the settings of one melt (NUT-05) payment method
    async fn update_melt_method(
        &self,
        request: Request<crate::payment_method::UpdateMeltMethodRequest>,
    ) -> Result<Response<crate::payment_method::UpdateMeltMethodResponse>, Status> {
        let request = request.into_inner();

        if request.options.is_some()
            && PaymentMethod::from_str(&request.method).is_ok_and(|method| !method.is_bolt11())
        {
            return Err(Status::invalid_argument(
                "Options can only be set on the bolt11 method".to_string(),
            ));
        }

        let options = request
            .options
            .map(|options| cdk::nuts::nut05::MeltMethodOptions::Bolt11 {
                amountless: options.amountless,
            });

        let settings = self
            .set_melt_method(
                &request.unit,
                &request.method,
                request.min_amount,
                request.max_amount,
                options,
                request.method_name,
            )
            .await?;

        Ok(Response::new(settings.into()))
    }

    /// Enables or disables minting and melting for the whole mint
    async fn update_disabled(
        &self,
        request: Request<crate::payment_method::UpdateDisabledRequest>,
    ) -> Result<Response<crate::payment_method::UpdateDisabledResponse>, Status> {
        let request = request.into_inner();

        let (mint_disabled, melt_disabled) = self
            .set_disabled(request.mint_disabled, request.melt_disabled)
            .await?;

        Ok(Response::new(
            crate::payment_method::UpdateDisabledResponse {
                mint_disabled,
                melt_disabled,
            },
        ))
    }
}

impl From<MintMethodSettings> for crate::payment_method::UpdateMintMethodResponse {
    fn from(settings: MintMethodSettings) -> Self {
        let options = settings.options.and_then(|options| match options {
            cdk::nuts::nut04::MintMethodOptions::Bolt11 { description }
            | cdk::nuts::nut04::MintMethodOptions::Bolt12 { description } => {
                Some(crate::payment_method::Bolt11MintMethodOptions { description })
            }
            _ => None,
        });

        Self {
            unit: settings.unit.to_string(),
            method: settings.method.to_string(),
            min_amount: settings.min_amount.map(u64::from),
            max_amount: settings.max_amount.map(u64::from),
            options,
            method_name: settings.method_name,
        }
    }
}

impl From<MeltMethodSettings> for crate::payment_method::UpdateMeltMethodResponse {
    fn from(settings: MeltMethodSettings) -> Self {
        let options = settings.options.map(|options| match options {
            cdk::nuts::nut05::MeltMethodOptions::Bolt11 { amountless } => {
                crate::payment_method::Bolt11MeltMethodOptions { amountless }
            }
        });

        Self {
            unit: settings.unit.to_string(),
            method: settings.method.to_string(),
            min_amount: settings.min_amount.map(u64::from),
            max_amount: settings.max_amount.map(u64::from),
            options,
            method_name: settings.method_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use bip39::Mnemonic;
    use cdk::amount::SplitTarget;
    use cdk::mint::{MintBuilder, MintInput, MintMeltLimits};
    use cdk::nuts::{CurrencyUnit, MintRequest, PaymentMethod, PreMintSecrets};
    use cdk::types::QuoteTTL;
    use cdk_common::nut00::KnownMethod;
    use cdk_common::MintQuoteBolt11Request;
    use cdk_fake_wallet::FakeWallet;
    use tonic::{Code, Request};

    use super::*;

    struct TestWalletInfoProvider;

    #[async_trait::async_trait]
    impl crate::WalletInfoProvider for TestWalletInfoProvider {
        async fn create_deposit_address(&self) -> Result<String, crate::WalletInfoError> {
            Ok("bcrt1qoperatordeposit".to_string())
        }

        async fn get_balance(
            &self,
        ) -> Result<crate::wallet::GetBalanceResponse, crate::WalletInfoError> {
            Ok(crate::wallet::GetBalanceResponse {
                confirmed_sat: 21,
                trusted_pending_sat: 2,
                untrusted_pending_sat: 3,
                immature_sat: 4,
                trusted_spendable_sat: 23,
                total_sat: 30,
                network: "regtest".to_string(),
                synced_height: 123,
            })
        }

        async fn list_transactions(
            &self,
            offset: usize,
            limit: usize,
        ) -> Result<crate::WalletTransactionPage, crate::WalletInfoError> {
            Ok(crate::WalletTransactionPage {
                transactions: vec![crate::wallet::WalletTransaction {
                    txid: format!("{offset}:{limit}"),
                    inputs: vec![crate::wallet::WalletTransactionInput {
                        txid: "previous-txid".to_string(),
                        vout: 1,
                        amount_sat: Some(42_000),
                        address: Some("bcrt1qinput".to_string()),
                    }],
                    outputs: vec![crate::wallet::WalletTransactionOutput {
                        vout: 2,
                        address: "bcrt1qoutput".to_string(),
                        amount_sat: 21_000,
                        quote_id: Some("quote-id".to_string()),
                    }],
                    ..Default::default()
                }],
                total: 7,
            })
        }

        async fn list_addresses(
            &self,
            offset: usize,
            limit: usize,
        ) -> Result<crate::WalletAddressPage, crate::WalletInfoError> {
            Ok(crate::WalletAddressPage {
                addresses: vec![crate::wallet::WalletAddress {
                    address: format!("{offset}:{limit}"),
                    ..Default::default()
                }],
                total: 9,
            })
        }
    }

    /// A well-formed quote id that no test mint has issued
    const UNKNOWN_QUOTE_ID: &str = "019820ab-cdef-7000-8000-000000000000";

    async fn create_test_rpc_server() -> MintRPCServer {
        create_test_rpc_server_with_payment_delay(2).await
    }

    /// Builds a test server whose fake payment backend waits `payment_delay`
    /// seconds before reporting a quote paid
    ///
    /// Tests that drive quote state themselves pass a delay long enough that
    /// the backend never reports a payment of its own.
    async fn create_test_rpc_server_with_payment_delay(payment_delay: u64) -> MintRPCServer {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());

        let mut mint_builder = MintBuilder::new(db.clone());

        let fee_reserve = cdk::types::FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        };

        let fake_backend = FakeWallet::new(
            fee_reserve,
            HashMap::default(),
            HashSet::default(),
            payment_delay,
            CurrencyUnit::Sat,
        );

        mint_builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(fake_backend),
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
            mutation_guard: None,
            allow_mint_quote_payment_override: false,
            wallet_info_provider: None,
            shutdown: Arc::new(Notify::new()),
            handle: None,
        }
    }

    #[derive(Debug)]
    struct RejectingMutationGuard;

    #[tonic::async_trait]
    impl MintMutationGuard for RejectingMutationGuard {
        async fn check(&self) -> Result<(), MintMutationGuardError> {
            Err(MintMutationGuardError::FailedPrecondition(
                "configuration restart pending".to_owned(),
            ))
        }
    }

    #[tokio::test]
    async fn wallet_service_requires_a_configured_provider() {
        let server = create_test_rpc_server().await;

        let error = server
            .get_balance(Request::new(crate::wallet::GetBalanceRequest {}))
            .await
            .expect_err("wallet provider should be required");

        assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    }

    #[tokio::test]
    async fn wallet_service_returns_provider_data_and_applies_page_defaults() {
        let server = create_test_rpc_server()
            .await
            .with_wallet_info_provider(Arc::new(TestWalletInfoProvider));

        let deposit_address = server
            .create_deposit_address(Request::new(crate::wallet::CreateDepositAddressRequest {}))
            .await
            .expect("create deposit address")
            .into_inner();
        assert_eq!(deposit_address.address, "bcrt1qoperatordeposit");

        let balance = server
            .get_balance(Request::new(crate::wallet::GetBalanceRequest {}))
            .await
            .expect("get balance")
            .into_inner();
        assert_eq!(balance.confirmed_sat, 21);
        assert_eq!(balance.total_sat, 30);
        assert_eq!(balance.network, "regtest");
        assert_eq!(balance.synced_height, 123);

        let transactions = server
            .list_transactions(Request::new(crate::wallet::ListTransactionsRequest {
                limit: 0,
                offset: 2,
            }))
            .await
            .expect("list transactions")
            .into_inner();
        assert_eq!(transactions.total, 7);
        assert_eq!(transactions.transactions[0].txid, "2:20");
        let input = &transactions.transactions[0].inputs[0];
        assert_eq!(input.txid, "previous-txid");
        assert_eq!(input.vout, 1);
        assert_eq!(input.amount_sat, Some(42_000));
        assert_eq!(input.address.as_deref(), Some("bcrt1qinput"));
        let output = &transactions.transactions[0].outputs[0];
        assert_eq!(output.vout, 2);
        assert_eq!(output.address, "bcrt1qoutput");
        assert_eq!(output.amount_sat, 21_000);
        assert_eq!(output.quote_id.as_deref(), Some("quote-id"));

        let addresses = server
            .list_addresses(Request::new(crate::wallet::ListAddressesRequest {
                limit: 3,
                offset: 4,
            }))
            .await
            .expect("list addresses")
            .into_inner();
        assert_eq!(addresses.total, 9);
        assert_eq!(addresses.addresses[0].address, "4:3");
    }

    #[test]
    fn wallet_service_rejects_oversized_pages() {
        let error = page_limit(101, DEFAULT_TRANSACTION_LIMIT, MAX_TRANSACTION_LIMIT)
            .expect_err("oversized page should fail");

        assert_eq!(error.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_keyset_service_rotate_next_keyset() {
        let server = create_test_rpc_server().await;

        let response = KeysetService::rotate_next_keyset(
            &server,
            Request::new(crate::keyset::RotateNextKeysetRequest {
                unit: "sat".to_string(),
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: Some(1),
                use_keyset_v2: Some(true),
                final_expiry: None,
            }),
        )
        .await
        .unwrap();

        let response = response.into_inner();
        assert!(!response.id.is_empty());
        assert_eq!(response.unit, "sat");
        assert_eq!(response.amounts, vec![1, 2, 4, 8]);
        assert_eq!(response.input_fee_ppk, 1);
    }

    #[tokio::test]
    async fn test_quote_service_get_quote_ttl() {
        let server = create_test_rpc_server().await;

        let response =
            QuoteService::get_quote_ttl(&server, Request::new(crate::quote::GetQuoteTtlRequest {}))
                .await
                .unwrap();

        let response = response.into_inner();
        assert_eq!(response.mint_ttl, 10000);
        assert_eq!(response.melt_ttl, 10000);
    }

    #[tokio::test]
    async fn test_quote_service_update_quote_ttl_keeps_omitted_setting() {
        let server = create_test_rpc_server().await;

        let response = QuoteService::update_quote_ttl(
            &server,
            Request::new(crate::quote::UpdateQuoteTtlRequest {
                mint_ttl: Some(60),
                melt_ttl: None,
            }),
        )
        .await
        .unwrap();

        let response = response.into_inner();
        assert_eq!(response.mint_ttl, 60);
        assert_eq!(response.melt_ttl, 10000);

        let persisted =
            QuoteService::get_quote_ttl(&server, Request::new(crate::quote::GetQuoteTtlRequest {}))
                .await
                .unwrap()
                .into_inner();
        assert_eq!(persisted.mint_ttl, 60);
        assert_eq!(persisted.melt_ttl, 10000);
    }

    /// Creates a bolt11 mint quote for `amount` sats and returns its id
    async fn create_test_mint_quote(server: &MintRPCServer, amount: u64) -> String {
        let response = server
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: amount.into(),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap();

        response.quote().to_string()
    }

    /// Issues the full paid amount of a quote, leaving it in the issued state
    async fn issue_test_mint_quote(server: &MintRPCServer, quote_id: &str, amount: u64) {
        let keyset_id = *server
            .mint
            .get_active_keysets()
            .get(&CurrencyUnit::Sat)
            .unwrap();
        let keys = server
            .mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect());
        let premint = PreMintSecrets::random(
            keyset_id,
            Amount::from(amount),
            &SplitTarget::None,
            &fees.into(),
        )
        .unwrap();

        server
            .mint
            .process_mint_request(MintInput::Single(MintRequest {
                quote: quote_id.parse().unwrap(),
                outputs: premint.blinded_messages().to_vec(),
                signature: None,
            }))
            .await
            .unwrap();
    }

    /// Returns the amount recorded as paid against a quote
    async fn amount_paid(server: &MintRPCServer, quote_id: &str) -> u64 {
        server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .amount_paid()
            .value()
    }

    #[tokio::test]
    async fn test_quote_service_update_mint_quote_state_marks_quote_paid() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;

        assert_eq!(amount_paid(&server, &quote_id).await, 0);

        let response = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.quote_id, quote_id);
        assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);
        assert_eq!(amount_paid(&server, &quote_id).await, 100);
    }

    #[tokio::test]
    async fn test_quote_service_update_mint_quote_state_paid_twice_pays_once() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;

        for _ in 0..2 {
            let response = QuoteService::update_mint_quote_state(
                &server,
                Request::new(crate::quote::UpdateMintQuoteStateRequest {
                    quote_id: quote_id.clone(),
                    state: crate::quote::MintQuoteState::Paid.into(),
                }),
            )
            .await
            .unwrap()
            .into_inner();

            assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);
        }

        assert_eq!(amount_paid(&server, &quote_id).await, 100);
    }

    #[tokio::test]
    async fn test_quote_service_update_mint_quote_state_reports_state_of_issued_quote() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 32).await;

        QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
            }),
        )
        .await
        .unwrap();
        issue_test_mint_quote(&server, &quote_id, 32).await;

        // The request asks for Paid; an already-issued quote stays Issued
        let response = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.state(), crate::quote::MintQuoteState::Issued);
        assert_eq!(amount_paid(&server, &quote_id).await, 32);
    }

    #[tokio::test]
    async fn test_quote_service_update_mint_quote_state_rejects_unsupported_states() {
        let server = create_test_rpc_server().await;

        // An unsupported state is rejected before the quote is looked up
        for (state, expected) in [
            (
                crate::quote::MintQuoteState::Unpaid,
                "Cannot unpay a quote: payments cannot be retracted",
            ),
            (
                crate::quote::MintQuoteState::Issued,
                "Cannot issue a quote: no signatures would back the issuance",
            ),
            (
                crate::quote::MintQuoteState::Unspecified,
                "Quote state is required",
            ),
        ] {
            let status = QuoteService::update_mint_quote_state(
                &server,
                Request::new(crate::quote::UpdateMintQuoteStateRequest {
                    quote_id: UNKNOWN_QUOTE_ID.to_string(),
                    state: state.into(),
                }),
            )
            .await
            .unwrap_err();

            assert_eq!(status.code(), tonic::Code::InvalidArgument);
            assert_eq!(
                status.message(),
                expected,
                "unexpected rejection for {state:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_quote_service_update_mint_quote_state_unknown_quote() {
        let server = create_test_rpc_server()
            .await
            .with_mint_quote_payment_override(true);

        let status = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: UNKNOWN_QUOTE_ID.to_string(),
                state: crate::quote::MintQuoteState::Paid.into(),
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(status.message(), "Could not find quote");
    }

    #[tokio::test]
    async fn test_mint_quote_state_overrides_are_disabled_by_default() {
        let server = create_test_rpc_server_with_payment_delay(3600).await;
        let quote_id = create_test_mint_quote(&server, 100).await;

        let status = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
            }),
        )
        .await
        .expect_err("payment override should be disabled");

        assert_eq!(status.code(), tonic::Code::PermissionDenied);
        assert_eq!(status.message(), "Mint quote state override is disabled");
        assert_eq!(amount_paid(&server, &quote_id).await, 0);
    }

    #[tokio::test]
    async fn test_mint_info_service_get_info_round_trip() {
        let server = create_test_rpc_server().await;

        let response =
            MintInfoService::get_info(&server, Request::new(crate::info::GetInfoRequest {}))
                .await
                .unwrap()
                .into_inner();

        assert_eq!(response.name.as_deref(), Some("test mint"));
        assert_eq!(response.description.as_deref(), Some("test mint"));
        assert!(response.tos_url.is_none());
        assert!(response.urls.is_empty());
        assert!(response.contact.is_empty());
    }

    #[tokio::test]
    async fn test_mint_info_service_update_motd_sets_and_clears() {
        let server = create_test_rpc_server().await;

        let set = MintInfoService::update_motd(
            &server,
            Request::new(crate::info::UpdateMotdRequest {
                motd: "hello".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(set.motd.as_deref(), Some("hello"));

        let cleared = MintInfoService::update_motd(
            &server,
            Request::new(crate::info::UpdateMotdRequest {
                motd: String::new(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(cleared.motd.is_none());

        let info = MintInfoService::get_info(&server, Request::new(crate::info::GetInfoRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert!(info.motd.is_none());
    }

    #[tokio::test]
    async fn test_mint_info_service_update_tos_url() {
        let server = create_test_rpc_server().await;
        let tos = "https://example.com/terms";

        let response = MintInfoService::update_tos_url(
            &server,
            Request::new(crate::info::UpdateTosUrlRequest {
                tos_url: tos.to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(response.tos_url.as_deref(), Some(tos));

        let info = MintInfoService::get_info(&server, Request::new(crate::info::GetInfoRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(info.tos_url.as_deref(), Some(tos));
    }

    #[tokio::test]
    async fn test_mint_info_service_add_url_is_idempotent() {
        let server = create_test_rpc_server().await;
        let url = "https://mint.example.com";

        let first = MintInfoService::add_url(
            &server,
            Request::new(crate::info::AddUrlRequest {
                url: url.to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(first.urls, vec![url.to_owned()]);

        let second = MintInfoService::add_url(
            &server,
            Request::new(crate::info::AddUrlRequest {
                url: url.to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(second.urls, vec![url.to_owned()]);
    }

    #[tokio::test]
    async fn test_mint_info_service_add_url_rejects_empty() {
        let server = create_test_rpc_server().await;

        let error = MintInfoService::add_url(
            &server,
            Request::new(crate::info::AddUrlRequest { url: String::new() }),
        )
        .await
        .expect_err("empty URL should be rejected");

        assert_eq!(error.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_mint_info_service_remove_url_absent_is_noop() {
        let server = create_test_rpc_server().await;
        let url = "https://mint.example.com";

        MintInfoService::add_url(
            &server,
            Request::new(crate::info::AddUrlRequest {
                url: url.to_owned(),
            }),
        )
        .await
        .unwrap();

        let missing = MintInfoService::remove_url(
            &server,
            Request::new(crate::info::RemoveUrlRequest {
                url: "https://absent.example.com".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(missing.urls, vec![url.to_owned()]);

        let removed = MintInfoService::remove_url(
            &server,
            Request::new(crate::info::RemoveUrlRequest {
                url: url.to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(removed.urls.is_empty());
    }

    #[tokio::test]
    async fn test_mint_info_service_contacts_add_remove_idempotent() {
        let server = create_test_rpc_server().await;

        let added = MintInfoService::add_contact(
            &server,
            Request::new(crate::info::AddContactRequest {
                method: "email".to_owned(),
                info: "mint@example.com".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(added.contact.len(), 1);

        let duplicate = MintInfoService::add_contact(
            &server,
            Request::new(crate::info::AddContactRequest {
                method: "email".to_owned(),
                info: "mint@example.com".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(duplicate.contact.len(), 1);

        let empty_method_error = MintInfoService::add_contact(
            &server,
            Request::new(crate::info::AddContactRequest {
                method: String::new(),
                info: "mint@example.com".to_owned(),
            }),
        )
        .await
        .expect_err("empty contact method should be rejected");
        assert_eq!(empty_method_error.code(), Code::InvalidArgument);

        let removed = MintInfoService::remove_contact(
            &server,
            Request::new(crate::info::RemoveContactRequest {
                method: "email".to_owned(),
                info: "mint@example.com".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(removed.contact.is_empty());

        let absent = MintInfoService::remove_contact(
            &server,
            Request::new(crate::info::RemoveContactRequest {
                method: "email".to_owned(),
                info: "mint@example.com".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(absent.contact.is_empty());
    }

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

    #[test]
    fn test_update_mint_method_response_maps_bolt12_description_options() {
        let settings = MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt12),
            unit: CurrencyUnit::Sat,
            method_name: Some("Bolt12".to_string()),
            min_amount: Some(Amount::from(1)),
            max_amount: Some(Amount::from(1_000)),
            options: Some(cdk::nuts::nut04::MintMethodOptions::Bolt12 { description: true }),
        };

        let response: crate::payment_method::UpdateMintMethodResponse = settings.into();

        assert_eq!(response.unit, "sat");
        assert_eq!(response.method, "bolt12");
        assert_eq!(response.min_amount, Some(1));
        assert_eq!(response.max_amount, Some(1_000));
        assert_eq!(
            response.options,
            Some(crate::payment_method::Bolt11MintMethodOptions { description: true })
        );
        assert_eq!(response.method_name.as_deref(), Some("Bolt12"));
    }

    #[tokio::test]
    async fn test_payment_method_service_update_mint_method_keeps_omitted_settings() {
        let server = create_test_rpc_server().await;

        let response = PaymentMethodService::update_mint_method(
            &server,
            Request::new(crate::payment_method::UpdateMintMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt11".to_owned(),
                min_amount: Some(1),
                max_amount: Some(1_000),
                options: Some(crate::payment_method::Bolt11MintMethodOptions { description: true }),
                method_name: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.unit, "sat");
        assert_eq!(response.method, "bolt11");
        assert_eq!(response.min_amount, Some(1));
        assert_eq!(response.max_amount, Some(1_000));
        assert_eq!(
            response.options,
            Some(crate::payment_method::Bolt11MintMethodOptions { description: true })
        );

        let response = PaymentMethodService::update_mint_method(
            &server,
            Request::new(crate::payment_method::UpdateMintMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt11".to_owned(),
                min_amount: None,
                max_amount: Some(5_000),
                options: None,
                method_name: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.min_amount, Some(1));
        assert_eq!(response.max_amount, Some(5_000));
        assert_eq!(
            response.options,
            Some(crate::payment_method::Bolt11MintMethodOptions { description: true })
        );

        let settings = server
            .mint
            .mint_info()
            .await
            .unwrap()
            .nuts
            .nut04
            .get_settings(
                &CurrencyUnit::Sat,
                &PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .unwrap();
        assert_eq!(settings.min_amount, Some(Amount::from(1)));
        assert_eq!(settings.max_amount, Some(Amount::from(5_000)));
    }

    #[tokio::test]
    async fn test_payment_method_service_update_mint_method_rejects_unknown_pair() {
        let server = create_test_rpc_server().await;

        let error = PaymentMethodService::update_mint_method(
            &server,
            Request::new(crate::payment_method::UpdateMintMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt12".to_owned(),
                min_amount: None,
                max_amount: None,
                options: None,
                method_name: None,
            }),
        )
        .await
        .expect_err("method without a payment processor should be rejected");

        // The message separates this from the options guard, which must not
        // fire on a request that carries no options
        assert_eq!(error.code(), Code::InvalidArgument);
        assert_eq!(error.message(), "Unit payment method pair is not supported");
    }

    #[tokio::test]
    async fn test_payment_method_service_rejects_options_on_non_bolt11_method() {
        let server = create_test_rpc_server().await;

        let error = PaymentMethodService::update_mint_method(
            &server,
            Request::new(crate::payment_method::UpdateMintMethodRequest {
                unit: "sat".to_owned(),
                method: "onchain".to_owned(),
                min_amount: None,
                max_amount: None,
                options: Some(crate::payment_method::Bolt11MintMethodOptions { description: true }),
                method_name: None,
            }),
        )
        .await
        .expect_err("bolt11 options on an onchain method should be rejected");

        // The processor check also rejects this pair; the message shows the guard fired
        assert_eq!(error.code(), Code::InvalidArgument);
        assert_eq!(
            error.message(),
            "Options can only be set on the bolt11 method"
        );

        let error = PaymentMethodService::update_melt_method(
            &server,
            Request::new(crate::payment_method::UpdateMeltMethodRequest {
                unit: "sat".to_owned(),
                method: "onchain".to_owned(),
                min_amount: None,
                max_amount: None,
                options: Some(crate::payment_method::Bolt11MeltMethodOptions { amountless: true }),
                method_name: None,
            }),
        )
        .await
        .expect_err("bolt11 options on an onchain method should be rejected");

        assert_eq!(error.code(), Code::InvalidArgument);
        assert_eq!(
            error.message(),
            "Options can only be set on the bolt11 method"
        );
    }

    #[tokio::test]
    async fn test_payment_method_service_update_disabled_keeps_omitted_flag() {
        let server = create_test_rpc_server().await;

        let response = PaymentMethodService::update_disabled(
            &server,
            Request::new(crate::payment_method::UpdateDisabledRequest {
                mint_disabled: Some(true),
                melt_disabled: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert!(response.mint_disabled);
        assert!(!response.melt_disabled);
        let info = server.mint.mint_info().await.unwrap();
        assert!(info.nuts.nut04.disabled);
        assert!(!info.nuts.nut05.disabled);

        let response = PaymentMethodService::update_disabled(
            &server,
            Request::new(crate::payment_method::UpdateDisabledRequest {
                mint_disabled: Some(false),
                melt_disabled: Some(true),
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert!(!response.mint_disabled);
        assert!(response.melt_disabled);
        let info = server.mint.mint_info().await.unwrap();
        assert!(!info.nuts.nut04.disabled);
        assert!(info.nuts.nut05.disabled);
    }

    #[tokio::test]
    async fn test_payment_method_service_update_disabled_with_no_flags_changes_nothing() {
        let server = create_test_rpc_server().await;

        // Set both flags first; on a fresh server every flag is already false
        PaymentMethodService::update_disabled(
            &server,
            Request::new(crate::payment_method::UpdateDisabledRequest {
                mint_disabled: Some(true),
                melt_disabled: Some(true),
            }),
        )
        .await
        .unwrap();

        let response = PaymentMethodService::update_disabled(
            &server,
            Request::new(crate::payment_method::UpdateDisabledRequest {
                mint_disabled: None,
                melt_disabled: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert!(response.mint_disabled);
        assert!(response.melt_disabled);
        let info = server.mint.mint_info().await.unwrap();
        assert!(info.nuts.nut04.disabled);
        assert!(info.nuts.nut05.disabled);
    }

    #[tokio::test]
    async fn test_payment_method_service_update_melt_method_keeps_omitted_settings() {
        let server = create_test_rpc_server().await;

        PaymentMethodService::update_melt_method(
            &server,
            Request::new(crate::payment_method::UpdateMeltMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt11".to_owned(),
                min_amount: Some(2),
                max_amount: Some(2_000),
                options: Some(crate::payment_method::Bolt11MeltMethodOptions { amountless: true }),
                method_name: None,
            }),
        )
        .await
        .unwrap();

        let response = PaymentMethodService::update_melt_method(
            &server,
            Request::new(crate::payment_method::UpdateMeltMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt11".to_owned(),
                min_amount: None,
                max_amount: None,
                options: None,
                method_name: Some("Lightning".to_owned()),
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.min_amount, Some(2));
        assert_eq!(response.max_amount, Some(2_000));
        assert_eq!(
            response.options,
            Some(crate::payment_method::Bolt11MeltMethodOptions { amountless: true })
        );
        assert_eq!(response.method_name, Some("Lightning".to_owned()));
    }
}
