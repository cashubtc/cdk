//! CDK lightning backend for CLN

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdk::amount::Amount;
use cdk::cdk_lightning::{
    self, to_unit, CreateInvoiceResponse, MintLightning, PayInvoiceResponse, PaymentQuoteResponse,
    Settings,
};
use cdk::mint::FeeReserve;
use cdk::nuts::{
    CurrencyUnit, MeltMethodSettings, MeltQuoteBolt11Request, MeltQuoteState, MintMethodSettings,
    MintQuoteState,
};
use cdk::util::{hex, unix_time};
use cdk::{mint, Bolt11Invoice};
use cln_rpc::model::requests::{
    InvoiceRequest, ListinvoicesRequest, ListpaysRequest, PayRequest, WaitanyinvoiceRequest,
};
use cln_rpc::model::responses::{
    ListinvoicesInvoicesStatus, ListpaysPaysStatus, PayStatus, WaitanyinvoiceResponse,
};
use cln_rpc::model::Request;
use cln_rpc::primitives::{Amount as CLN_Amount, AmountOrAny};
use error::Error;
use futures::{Stream, StreamExt};
use tokio::sync::Mutex;
use uuid::Uuid;

pub mod error;

/// CLN mint backend
#[derive(Clone)]
pub struct Cln {
    rpc_socket: PathBuf,
    cln_client: Arc<Mutex<cln_rpc::ClnRpc>>,
    fee_reserve: FeeReserve,
    mint_settings: MintMethodSettings,
    melt_settings: MeltMethodSettings,
}

impl Cln {
    /// Create new [`Cln`]
    pub async fn new(
        rpc_socket: PathBuf,
        fee_reserve: FeeReserve,
        mint_settings: MintMethodSettings,
        melt_settings: MeltMethodSettings,
    ) -> Result<Self, Error> {
        let cln_client = cln_rpc::ClnRpc::new(&rpc_socket).await?;

        Ok(Self {
            rpc_socket,
            cln_client: Arc::new(Mutex::new(cln_client)),
            fee_reserve,
            mint_settings,
            melt_settings,
        })
    }
}

#[async_trait]
impl MintLightning for Cln {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: true,
            unit: CurrencyUnit::Msat,
            mint_settings: self.mint_settings.clone(),
            melt_settings: self.melt_settings.clone(),
            invoice_description: true,
        }
    }

    async fn wait_any_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        let last_pay_index = self.get_last_pay_index().await?;
        let cln_client = cln_rpc::ClnRpc::new(&self.rpc_socket).await?;

        Ok(futures::stream::unfold(
            (cln_client, last_pay_index),
            |(mut cln_client, mut last_pay_idx)| async move {
                loop {
                    let invoice_res = cln_client
                        .call(cln_rpc::Request::WaitAnyInvoice(WaitanyinvoiceRequest {
                            timeout: None,
                            lastpay_index: last_pay_idx,
                        }))
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
                    .try_into()
                    .expect("Wrong response from CLN");

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
            fee: fee.into(),
            state: MeltQuoteState::Unpaid,
        })
    }

    async fn pay_invoice(
        &self,
        melt_quote: mint::MeltQuote,
        partial_amount: Option<Amount>,
        max_fee: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let mut cln_client = self.cln_client.lock().await;

        let pay_state =
            check_pay_invoice_status(&mut cln_client, melt_quote.request.to_string()).await?;

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

        let cln_response = cln_client
            .call(Request::Pay(PayRequest {
                bolt11: melt_quote.request.to_string(),
                amount_msat: None,
                label: None,
                riskfactor: None,
                maxfeepercent: None,
                retry_for: None,
                maxdelay: None,
                exemptfee: None,
                localinvreqid: None,
                exclude: None,
                maxfee: max_fee
                    .map(|a| {
                        let msat = to_unit(a, &melt_quote.unit, &CurrencyUnit::Msat)?;
                        Ok::<cln_rpc::primitives::Amount, Self::Err>(CLN_Amount::from_msat(
                            msat.into(),
                        ))
                    })
                    .transpose()?,
                description: None,
                partial_msat: partial_amount
                    .map(|a| {
                        let msat = to_unit(a, &melt_quote.unit, &CurrencyUnit::Msat)?;

                        Ok::<cln_rpc::primitives::Amount, Self::Err>(CLN_Amount::from_msat(
                            msat.into(),
                        ))
                    })
                    .transpose()?,
            }))
            .await
            .map_err(Error::from)?;

        let response = match cln_response {
            cln_rpc::Response::Pay(pay_response) => {
                let status = match pay_response.status {
                    PayStatus::COMPLETE => MeltQuoteState::Paid,
                    PayStatus::PENDING => MeltQuoteState::Pending,
                    PayStatus::FAILED => MeltQuoteState::Unpaid,
                };
                PayInvoiceResponse {
                    payment_preimage: Some(hex::encode(pay_response.payment_preimage.to_vec())),
                    payment_hash: pay_response.payment_hash.to_string(),
                    status,
                    total_spent: to_unit(
                        pay_response.amount_sent_msat.msat(),
                        &CurrencyUnit::Msat,
                        &melt_quote.unit,
                    )?,
                }
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                return Err(cdk_lightning::Error::from(Error::WrongClnResponse));
            }
        };

        Ok(response)
    }

    async fn create_invoice(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err> {
        let time_now = unix_time();
        assert!(unix_expiry > time_now);

        let mut cln_client = self.cln_client.lock().await;

        let label = Uuid::new_v4().to_string();

        let amount = to_unit(amount, unit, &CurrencyUnit::Msat)?;
        let amount_msat = AmountOrAny::Amount(CLN_Amount::from_msat(amount.into()));

        let cln_response = cln_client
            .call(cln_rpc::Request::Invoice(InvoiceRequest {
                amount_msat,
                description,
                label: label.clone(),
                expiry: Some(unix_expiry - time_now),
                fallbacks: None,
                preimage: None,
                cltv: None,
                deschashonly: None,
                exposeprivatechannels: None,
            }))
            .await
            .map_err(Error::from)?;

        match cln_response {
            cln_rpc::Response::Invoice(invoice_res) => {
                let request = Bolt11Invoice::from_str(&invoice_res.bolt11)?;
                let expiry = request.expires_at().map(|t| t.as_secs());

                Ok(CreateInvoiceResponse {
                    request_lookup_id: label,
                    request,
                    expiry,
                })
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                Err(Error::WrongClnResponse.into())
            }
        }
    }

    async fn check_invoice_status(
        &self,
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let mut cln_client = self.cln_client.lock().await;

        let cln_response = cln_client
            .call(Request::ListInvoices(ListinvoicesRequest {
                payment_hash: None,
                label: Some(request_lookup_id.to_string()),
                invstring: None,
                offer_id: None,
                index: None,
                limit: None,
                start: None,
            }))
            .await
            .map_err(Error::from)?;

        let status = match cln_response {
            cln_rpc::Response::ListInvoices(invoice_response) => {
                match invoice_response.invoices.first() {
                    Some(invoice_response) => {
                        cln_invoice_status_to_mint_state(invoice_response.status)
                    }
                    None => {
                        tracing::info!(
                            "Check invoice called on unknown look up id: {}",
                            request_lookup_id
                        );
                        return Err(Error::WrongClnResponse.into());
                    }
                }
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                return Err(Error::WrongClnResponse.into());
            }
        };

        Ok(status)
    }
}

impl Cln {
    /// Get last pay index for cln
    async fn get_last_pay_index(&self) -> Result<Option<u64>, Error> {
        let mut cln_client = self.cln_client.lock().await;
        let cln_response = cln_client
            .call(cln_rpc::Request::ListInvoices(ListinvoicesRequest {
                index: None,
                invstring: None,
                label: None,
                limit: None,
                offer_id: None,
                payment_hash: None,
                start: None,
            }))
            .await
            .map_err(Error::from)?;

        match cln_response {
            cln_rpc::Response::ListInvoices(invoice_res) => match invoice_res.invoices.last() {
                Some(last_invoice) => Ok(last_invoice.pay_index),
                None => Ok(None),
            },
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                Err(Error::WrongClnResponse)
            }
        }
    }
}

fn cln_invoice_status_to_mint_state(status: ListinvoicesInvoicesStatus) -> MintQuoteState {
    match status {
        ListinvoicesInvoicesStatus::UNPAID => MintQuoteState::Unpaid,
        ListinvoicesInvoicesStatus::PAID => MintQuoteState::Paid,
        ListinvoicesInvoicesStatus::EXPIRED => MintQuoteState::Unpaid,
    }
}

async fn check_pay_invoice_status(
    cln_client: &mut cln_rpc::ClnRpc,
    bolt11: String,
) -> Result<MeltQuoteState, cdk_lightning::Error> {
    let cln_response = cln_client
        .call(Request::ListPays(ListpaysRequest {
            bolt11: Some(bolt11),
            payment_hash: None,
            status: None,
        }))
        .await
        .map_err(Error::from)?;

    let state = match cln_response {
        cln_rpc::Response::ListPays(pay_response) => {
            let pay = pay_response.pays.first();

            match pay {
                Some(pay) => match pay.status {
                    ListpaysPaysStatus::COMPLETE => MeltQuoteState::Paid,
                    ListpaysPaysStatus::PENDING => MeltQuoteState::Pending,
                    ListpaysPaysStatus::FAILED => MeltQuoteState::Unpaid,
                },
                None => MeltQuoteState::Unpaid,
            }
        }
        _ => {
            tracing::warn!("CLN returned wrong response kind. When checking pay status");
            return Err(cdk_lightning::Error::from(Error::WrongClnResponse));
        }
    };

    Ok(state)
}
