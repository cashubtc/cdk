//! CDK lightning backend for greenlight

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::fs;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail};
use async_trait::async_trait;
use bitcoin::Network;
use cdk::cdk_lightning::{
    self, to_unit, CreateInvoiceResponse, MintLightning, MintMeltSettings, PayInvoiceResponse,
    PaymentQuoteResponse, Settings,
};
use cdk::mint::FeeReserve;
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteState};
use cdk::util::unix_time;
use cdk::{mint, Amount as CDKAmount, Bolt11Invoice};
use error::Error;
use futures::{Stream, StreamExt};
use gl_client::credentials::{self, Device};
use gl_client::node::ClnClient;
use gl_client::pb::cln::listinvoices_invoices::ListinvoicesInvoicesStatus;
use gl_client::pb::cln::listpays_pays::ListpaysPaysStatus;
use gl_client::pb::cln::{
    amount_or_any, Amount as CLNAmount, AmountOrAny, GetinfoRequest, InvoiceRequest,
    ListinvoicesRequest, ListpaysRequest, PayRequest, PayResponse, WaitanyinvoiceRequest,
    WaitanyinvoiceResponse,
};
use gl_client::scheduler::Scheduler;
use gl_client::signer::Signer;
use tokio::sync::Mutex;
use uuid::Uuid;

pub mod error;

/// Greenlight mint backend
#[derive(Clone)]
pub struct Greenlight {
    signer: Signer,
    signer_tx: Option<tokio::sync::mpsc::Sender<()>>,
    creds: credentials::Device,
    node: Arc<Mutex<ClnClient>>,
    network: gl_client::bitcoin::Network,
    fee_reserve: FeeReserve,
    mint_settings: MintMeltSettings,
    melt_settings: MintMeltSettings,
}

impl Greenlight {
    /// Create new ['Cln]
    pub async fn new(
        seed: &[u8],
        work_dir: PathBuf,
        network: Network,
        fee_reserve: FeeReserve,
        mint_settings: MintMeltSettings,
        melt_settings: MintMeltSettings,
    ) -> anyhow::Result<Self> {
        let network: gl_client::bitcoin::Network = match network {
            Network::Bitcoin => gl_client::bitcoin::Network::Bitcoin,
            Network::Testnet => gl_client::bitcoin::Network::Testnet,
            Network::Regtest => gl_client::bitcoin::Network::Regtest,
            _ => bail!("Unsupported network"),
        };

        let greenlight_dir = work_dir.join("greenlight");

        let device_cert_path = greenlight_dir.join("client.crt");
        let device_key_path = greenlight_dir.join("client-key.pem");

        let device_creds_path = work_dir.join("device_creds");

        let (device_cert, device_key) = if let (Ok(_), Ok(_)) = (
            fs::metadata(&device_cert_path),
            fs::metadata(&device_key_path),
        ) {
            (
                fs::read_to_string(device_cert_path)?,
                fs::read_to_string(device_key_path)?,
            )
        } else {
            tracing::error!("Could not find device cert and/or key");
            tracing::debug!("Device cert path: {:?}", device_cert_path);
            tracing::debug!("Device key path: {:?}", device_key_path);
            bail!("Device cert and/or key unknown");
        };

        let developer_creds = credentials::Nobody {
            cert: device_cert.into_bytes(),
            key: device_key.into_bytes(),
            ..Default::default()
        };

        let signer = Signer::new(seed.to_vec(), network, developer_creds.clone())?;

        let scheduler_unauth = Scheduler::new(network, developer_creds.clone()).await?;

        let creds = match fs::metadata(&device_creds_path) {
            Ok(_) => {
                tracing::info!("Node has already been registered.");
                tracing::info!("Authenticating from device file.");
                let bytes = fs::read(device_creds_path)?;
                Device::from_bytes(bytes)
            }
            Err(_) => {
                tracing::info!("Node has not been registered");
                tracing::info!("Registering Node ...");

                let auth_response =
                    scheduler_unauth
                        .register(&signer, None)
                        .await
                        .map_err(|err| {
                            tracing::error!("Could not register node");
                            err
                        })?;

                tracing::info!("Greenlight node registered");

                let creds = Device::from_bytes(auth_response.creds);
                fs::write(device_creds_path, creds.to_bytes())?;

                creds
            }
        };

        let scheduler_auth = scheduler_unauth.authenticate(creds.clone()).await?;

        let signer = Signer::new(seed.to_vec(), network, creds.clone())?;

        let mut node: gl_client::node::ClnClient = scheduler_auth.node().await?;
        let info = node
            .getinfo(GetinfoRequest::default())
            .await
            .map_err(|x| anyhow!(x.to_string()))?;

        tracing::info!("Greenlight node started.");
        tracing::debug!("Info {:?}", info);

        let node = Arc::new(Mutex::new(node));

        Ok(Self {
            signer,
            signer_tx: None,
            node,
            creds,
            network,
            fee_reserve,
            mint_settings,
            melt_settings,
        })
    }

    /// Start greenlight signer
    pub fn start_signer(&mut self) -> Result<(), Error> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let signer_clone = self.signer.clone();

        self.signer_tx = Some(tx);
        tokio::spawn(async move {
            if let Err(err) = signer_clone.run_forever(rx).await {
                tracing::error!("Error starting signer{:?}", err);
            }
        });

        Ok(())
    }

    /// Get last pay index for cln
    async fn get_last_pay_index(&self) -> Result<Option<u64>, Error> {
        let mut cln_client = self.node.lock().await;

        let invoice_res = cln_client
            .list_invoices(ListinvoicesRequest {
                index: None,
                invstring: None,
                label: None,
                limit: None,
                offer_id: None,
                payment_hash: None,
                start: None,
            })
            .await
            .map_err(|err| anyhow!("Could not list invoices: {}", err))?;

        match invoice_res.into_inner().invoices.last() {
            Some(last_invoice) => Ok(last_invoice.pay_index),
            None => Ok(None),
        }
    }

    async fn check_pay_invoice_status(
        &self,
        bolt11: String,
    ) -> Result<MeltQuoteState, cdk_lightning::Error> {
        let mut cln_client = self.node.lock().await;
        let cln_response = cln_client
            .list_pays(ListpaysRequest {
                bolt11: Some(bolt11),
                payment_hash: None,
                status: None,
            })
            .await
            .map_err(|err| anyhow!("Could not list invoices: {}", err))?;

        let pay = cln_response.into_inner().pays;

        let state = match pay.first() {
            Some(pay) => match pay.status() {
                ListpaysPaysStatus::Complete => MeltQuoteState::Paid,
                ListpaysPaysStatus::Pending => MeltQuoteState::Pending,
                ListpaysPaysStatus::Failed => MeltQuoteState::Unpaid,
            },
            None => MeltQuoteState::Unpaid,
        };

        Ok(state)
    }
}

#[async_trait]
impl MintLightning for Greenlight {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: true,
            unit: CurrencyUnit::Msat,
            mint_settings: self.mint_settings,
            melt_settings: self.melt_settings,
        }
    }

    async fn wait_any_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        let last_pay_index = self.get_last_pay_index().await?;

        let scheduler = Scheduler::new(self.network, self.creds.clone()).await?;

        let cln_client: ClnClient = scheduler.node().await?;

        Ok(futures::stream::unfold(
            (cln_client, last_pay_index),
            |(mut cln_client, mut last_pay_idx)| async move {
                loop {
                    let invoice_res = cln_client
                        .wait_any_invoice(WaitanyinvoiceRequest {
                            timeout: None,
                            lastpay_index: last_pay_idx,
                        })
                        .await;

                    let invoice: WaitanyinvoiceResponse = match invoice_res {
                        Ok(invoice) => invoice,
                        Err(e) => {
                            tracing::warn!("Error fetching invoice: {e}");
                            // Let's not spam CLN with requests on failure
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            // Retry same request
                            continue;
                        }
                    }
                    .into_inner();

                    last_pay_idx = invoice.pay_index;

                    break Some((invoice.label, (cln_client, last_pay_idx)));
                }
            },
        )
        .boxed())
    }

    async fn get_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt11Request,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let invoice_amount_msat = melt_quote_request
            .request
            .amount_milli_satoshis()
            .ok_or(Error::UnknownInvoiceAmount)?;

        let amount = to_unit(
            invoice_amount_msat,
            &CurrencyUnit::Msat,
            &melt_quote_request.unit,
        )?;

        let relative_fee_reserve =
            (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;

        let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

        let fee = match relative_fee_reserve > absolute_fee_reserve {
            true => relative_fee_reserve,
            false => absolute_fee_reserve,
        };

        Ok(PaymentQuoteResponse {
            request_lookup_id: melt_quote_request.request.payment_hash().to_string(),
            amount,
            fee,
        })
    }

    async fn pay_invoice(
        &self,
        melt_quote: mint::MeltQuote,
        partial_amount: Option<CDKAmount>,
        max_fee_amount: Option<CDKAmount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let pay_state = self
            .check_pay_invoice_status(melt_quote.request.to_string())
            .await?;

        let mut cln_client = self.node.lock().await;

        let maxfee = max_fee_amount
            .map(|amount| {
                let max_fee_msat = to_unit(amount, &melt_quote.unit, &CurrencyUnit::Msat)?;
                Ok::<CLNAmount, Self::Err>(CLNAmount {
                    msat: max_fee_msat.into(),
                })
            })
            .transpose()?;

        let invoice = melt_quote.request.clone();

        match pay_state {
            MeltQuoteState::Paid => {
                tracing::debug!("Melt attempted on invoice already paid");
                return Err(Self::Err::InvoiceAlreadyPaid);
            }
            MeltQuoteState::Pending => {
                tracing::debug!("Melt attempted on invoice already pending");
                return Err(Self::Err::InvoicePaymentPending);
            }
            MeltQuoteState::Unpaid => (),
        }

        let partial_amount = partial_amount
            .map(|amount| {
                let max_fee_msat = to_unit(amount, &melt_quote.unit, &CurrencyUnit::Msat)?;
                Ok::<CLNAmount, Self::Err>(CLNAmount {
                    msat: max_fee_msat.into(),
                })
            })
            .transpose()?;

        let cln_response = cln_client
            .pay(PayRequest {
                bolt11: invoice,
                maxfee,
                amount_msat: partial_amount,
                ..Default::default()
            })
            .await
            .map_err(|_| anyhow!("Tonic Error"))?;

        let PayResponse {
            payment_preimage,
            amount_sent_msat,
            payment_hash,
            ..
        } = cln_response.into_inner();
        let amount_sent_msat = amount_sent_msat.map(|x| x.msat).unwrap_or_default();

        let total_spent = to_unit(amount_sent_msat, &CurrencyUnit::Msat, &melt_quote.unit)?;

        let response = PayInvoiceResponse {
            payment_hash: String::from_utf8(payment_hash).map_err(|_| anyhow!("Utf8 error"))?,
            payment_preimage: Some(
                String::from_utf8(payment_preimage).map_err(|_| anyhow!("Utf8 Error"))?,
            ),
            status: MeltQuoteState::Paid,
            total_spent,
        };

        Ok(response)
    }

    async fn create_invoice(
        &self,
        amount: CDKAmount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err> {
        let time_now = unix_time();
        assert!(unix_expiry > time_now);
        let mut node = self.node.lock().await;

        let amount_msat = to_unit(amount, unit, &CurrencyUnit::Msat)?;

        let amount_msat = AmountOrAny {
            value: Some(amount_or_any::Value::Amount(CLNAmount {
                msat: amount_msat.into(),
            })),
        };

        let label = Uuid::new_v4().to_string();

        let response = node
            .invoice(InvoiceRequest {
                amount_msat: Some(amount_msat),
                description,
                label: label.to_string(),
                expiry: Some(unix_expiry - time_now),
                fallbacks: vec![],
                preimage: None,
                cltv: None,
                deschashonly: None,
            })
            .await
            .map_err(|err| anyhow!(err.to_string()))?
            .into_inner();

        let request = response.bolt11;

        let bolt11 = Bolt11Invoice::from_str(&request)?;

        Ok(CreateInvoiceResponse {
            request: Bolt11Invoice::from_str(&request)?,
            request_lookup_id: label,
            expiry: bolt11.expires_at().map(|a| a.as_secs()),
        })
    }

    async fn check_invoice_status(
        &self,
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let mut cln_client = self.node.lock().await;

        let cln_response = cln_client
            .list_invoices(ListinvoicesRequest {
                payment_hash: None,
                label: Some(request_lookup_id.to_string()),
                invstring: None,
                offer_id: None,
                index: None,
                limit: None,
                start: None,
            })
            .await
            .map_err(|err| anyhow!(err.to_string()))?;

        let status = match cln_response.into_inner().invoices.first() {
            Some(invoice_response) => cln_invoice_status_to_mint_state(invoice_response.status()),
            None => {
                tracing::info!(
                    "Check invoice called on unknown look up id: {}",
                    request_lookup_id
                );
                return Err(Error::WrongClnResponse.into());
            }
        };

        Ok(status)
    }
}

fn cln_invoice_status_to_mint_state(status: ListinvoicesInvoicesStatus) -> MintQuoteState {
    match status {
        ListinvoicesInvoicesStatus::Unpaid => MintQuoteState::Unpaid,
        ListinvoicesInvoicesStatus::Paid => MintQuoteState::Paid,
        ListinvoicesInvoicesStatus::Expired => MintQuoteState::Unpaid,
    }
}
