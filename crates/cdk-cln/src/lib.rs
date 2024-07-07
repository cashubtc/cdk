//! CDK lightning backend for CLN

use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdk::cdk_lightning::{
    self, to_unit, CreateInvoiceResponse, MintLightning, PayInvoiceResponse, PaymentQuoteResponse,
    Settings,
};
use cdk::mint::FeeReserve;
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteState};
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

#[derive(Clone)]
pub struct Cln {
    rpc_socket: PathBuf,
    cln_client: Arc<Mutex<cln_rpc::ClnRpc>>,
    fee_reserve: FeeReserve,
    min_melt_amount: u64,
    max_melt_amount: u64,
    min_mint_amount: u64,
    max_mint_amount: u64,
    mint_enabled: bool,
    melt_enabled: bool,
}

impl Cln {
    pub async fn new(
        rpc_socket: PathBuf,
        fee_reserve: FeeReserve,
        min_melt_amount: u64,
        max_melt_amount: u64,
        min_mint_amount: u64,
        max_mint_amount: u64,
    ) -> Result<Self, Error> {
        let cln_client = cln_rpc::ClnRpc::new(&rpc_socket).await?;

        Ok(Self {
            rpc_socket,
            cln_client: Arc::new(Mutex::new(cln_client)),
            fee_reserve,
            min_mint_amount,
            max_mint_amount,
            min_melt_amount,
            max_melt_amount,
            mint_enabled: true,
            melt_enabled: true,
        })
    }
}

#[async_trait]
impl MintLightning for Cln {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: true,
            min_mint_amount: self.min_mint_amount,
            max_mint_amount: self.max_mint_amount,
            min_melt_amount: self.min_melt_amount,
            max_melt_amount: self.max_melt_amount,
            unit: CurrencyUnit::Msat,
            mint_enabled: self.mint_enabled,
            melt_enabled: self.melt_enabled,
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

        let relative_fee_reserve = (self.fee_reserve.percent_fee_reserve * amount as f32) as u64;

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
        partial_msats: Option<u64>,
        max_fee_msats: Option<u64>,
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
                maxfee: max_fee_msats.map(CLN_Amount::from_msat),
                description: None,
                partial_msat: partial_msats.map(CLN_Amount::from_msat),
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
                    total_spent_msats: pay_response.amount_sent_msat.msat(),
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
        amount_msats: u64,
        description: String,
        unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err> {
        let time_now = unix_time();
        assert!(unix_expiry > time_now);

        let mut cln_client = self.cln_client.lock().await;

        let label = Uuid::new_v4().to_string();
        let amount_msat = AmountOrAny::Amount(CLN_Amount::from_msat(amount_msats));
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
            cln_rpc::Response::Invoice(invoice_res) => Ok(CreateInvoiceResponse {
                request_lookup_id: label,
                request: Bolt11Invoice::from_str(&invoice_res.bolt11)?,
            }),
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
                return Err(Error::Custom("CLN returned wrong response kind".to_string()).into());
            }
        };

        Ok(status)
    }
}

impl Cln {
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
