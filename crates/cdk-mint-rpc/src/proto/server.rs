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
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status};

use crate::cdk_mint_server::{CdkMint, CdkMintServer};
use crate::wallet::wallet_service_server::{WalletService, WalletServiceServer};
use crate::{
    ContactInfo, GetInfoRequest, GetInfoResponse, GetQuoteTtlRequest, GetQuoteTtlResponse,
    RotateNextKeysetRequest, RotateNextKeysetResponse, UpdateContactRequest,
    UpdateDescriptionRequest, UpdateIconUrlRequest, UpdateMotdRequest, UpdateNameRequest,
    UpdateNut04QuoteRequest, UpdateNut04Request, UpdateNut05Request, UpdateQuoteTtlRequest,
    UpdateResponse, UpdateTosUrlRequest, UpdateUrlRequest,
};
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

/// CDK Mint RPC Server
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintRPCServer {
    socket_addr: SocketAddr,
    mint: Arc<Mint>,
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
            wallet_info_provider: None,
            shutdown: Arc::new(Notify::new()),
            handle: None,
        })
    }

    /// Configures the read-only on-chain wallet information provider.
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
                    .add_service(CdkMintServer::with_interceptor(
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
                    .add_service(CdkMintServer::with_interceptor(
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

    fn wallet_info_provider(&self) -> Result<&DynWalletInfoProvider, Status> {
        self.wallet_info_provider.as_ref().ok_or_else(|| {
            Status::failed_precondition("No on-chain wallet information provider is configured")
        })
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

    /// Updates the mint's message of the day
    async fn update_motd(
        &self,
        request: Request<UpdateMotdRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
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

#[tonic::async_trait]
impl WalletService for MintRPCServer {
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

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
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
    use crate::{GetInfoRequest, UpdateTosUrlRequest};

    struct TestWalletInfoProvider;

    #[async_trait::async_trait]
    impl crate::WalletInfoProvider for TestWalletInfoProvider {
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

    async fn create_test_rpc_server() -> MintRPCServer {
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
            wallet_info_provider: None,
            shutdown: Arc::new(Notify::new()),
            handle: None,
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
}
