//! CDK lightning backend for CLN

use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdk::cdk_lightning::{self, BalanceResponse, InvoiceInfo, MintLightning, PayInvoiceResponse};
use cdk::types::InvoiceStatus;
use cdk::util::hex;
use cdk::{Amount, Bolt11Invoice, Sha256};
use cln_rpc::model::requests::{
    InvoiceRequest, ListfundsRequest, ListinvoicesRequest, PayRequest, WaitanyinvoiceRequest,
};
use cln_rpc::model::responses::{
    ListfundsOutputsStatus, ListinvoicesInvoicesStatus, PayStatus, WaitanyinvoiceResponse,
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
    last_pay_index: Option<u64>,
}

impl Cln {
    pub async fn new(rpc_socket: PathBuf, last_pay_index: Option<u64>) -> Result<Self, Error> {
        let cln_client = cln_rpc::ClnRpc::new(&rpc_socket).await?;

        Ok(Self {
            rpc_socket,
            cln_client: Arc::new(Mutex::new(cln_client)),
            last_pay_index,
        })
    }
}

#[async_trait]
impl MintLightning for Cln {
    type Err = cdk_lightning::Error;

    async fn get_invoice(
        &self,
        amount: Amount,
        hash: &str,
        description: &str,
    ) -> Result<InvoiceInfo, Self::Err> {
        let mut cln_client = cln_rpc::ClnRpc::new(&self.rpc_socket).await?;

        let cln_response = cln_client
            .call(cln_rpc::Request::Invoice(InvoiceRequest {
                amount_msat: AmountOrAny::Amount(CLN_Amount::from_sat(amount.into())),
                description: description.to_string(),
                label: Uuid::new_v4().to_string(),
                expiry: None,
                fallbacks: None,
                preimage: None,
                cltv: None,
                deschashonly: Some(true),
                exposeprivatechannels: None,
            }))
            .await
            .map_err(Error::from)?;

        match cln_response {
            cln_rpc::Response::Invoice(invoice_response) => {
                let invoice = Bolt11Invoice::from_str(&invoice_response.bolt11)?;
                let payment_hash = Sha256::from_str(&invoice_response.payment_hash.to_string())
                    .map_err(|_e| Error::Custom("Hash error".to_string()))?;
                let invoice_info = InvoiceInfo::new(
                    &payment_hash.to_string(),
                    hash,
                    invoice,
                    amount,
                    InvoiceStatus::Unpaid,
                    "",
                    None,
                );

                Ok(invoice_info)
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                return Err(cdk_lightning::Error::from(Error::WrongClnResponse));
            }
        }
    }

    async fn wait_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = (Bolt11Invoice, Option<u64>)> + Send>>, Self::Err> {
        let last_pay_index = self.last_pay_index;

        let cln_client = cln_rpc::ClnRpc::new(&self.rpc_socket).await?;

        Ok(futures::stream::unfold(
            (cln_client, last_pay_index),
            |(mut cln_client, mut last_pay_idx)| async move {
                loop {
                    // info!("Waiting for index: {last_pay_idx:?}");
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

                    if let Some(bolt11) = invoice.bolt11 {
                        if let Ok(invoice) = Bolt11Invoice::from_str(&bolt11) {
                            break Some(((invoice, last_pay_idx), (cln_client, last_pay_idx)));
                        }
                    }
                }
            },
        )
        .boxed())
    }

    async fn check_invoice_status(
        &self,
        payment_hash: &Sha256,
    ) -> Result<InvoiceStatus, Self::Err> {
        let mut cln_client = cln_rpc::ClnRpc::new(&self.rpc_socket).await?;

        let cln_response = cln_client
            .call(Request::ListInvoices(ListinvoicesRequest {
                payment_hash: Some(payment_hash.to_string()),
                label: None,
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
                let i = invoice_response.invoices[0].clone();

                cln_invoice_status_to_status(i.status)
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                return Err(Error::Custom("CLN returned wrong response kind".to_string()).into());
            }
        };

        Ok(status)
    }

    async fn pay_invoice(
        &self,
        bolt11: Bolt11Invoice,
        partial_msat: Option<Amount>,
        max_fee: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let mut cln_client = self.cln_client.lock().await;
        let cln_response = cln_client
            .call(Request::Pay(PayRequest {
                bolt11: bolt11.to_string(),
                amount_msat: None,
                label: None,
                riskfactor: None,
                maxfeepercent: None,
                retry_for: None,
                maxdelay: None,
                exemptfee: None,
                localinvreqid: None,
                exclude: None,
                maxfee: max_fee.map(|a| CLN_Amount::from_msat(a.to_msat())),
                description: None,
                partial_msat: partial_msat.map(|a| CLN_Amount::from_msat(a.to_msat())),
            }))
            .await
            .map_err(Error::from)?;

        let response = match cln_response {
            cln_rpc::Response::Pay(pay_response) => {
                let status = match pay_response.status {
                    PayStatus::COMPLETE => InvoiceStatus::Paid,
                    PayStatus::PENDING => InvoiceStatus::InFlight,
                    PayStatus::FAILED => InvoiceStatus::Unpaid,
                };
                PayInvoiceResponse {
                    payment_preimage: Some(hex::encode(pay_response.payment_preimage.to_vec())),
                    payment_hash: Sha256::from_str(&pay_response.payment_hash.to_string())
                        .map_err(|_| Error::Custom("Hash Error".to_string()))?,
                    status,
                    total_spent: Amount::from_msat(pay_response.amount_sent_msat.msat()),
                }
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                return Err(cdk_lightning::Error::from(Error::WrongClnResponse));
            }
        };

        Ok(response)
    }

    async fn get_balance(&self) -> Result<BalanceResponse, Self::Err> {
        let mut cln_client = self.cln_client.lock().await;
        let cln_response = cln_client
            .call(Request::ListFunds(ListfundsRequest { spent: None }))
            .await
            .map_err(Error::from)?;

        let balance = match cln_response {
            cln_rpc::Response::ListFunds(funds_response) => {
                let mut on_chain_total = CLN_Amount::from_msat(0);
                let mut on_chain_spendable = CLN_Amount::from_msat(0);
                let mut ln = CLN_Amount::from_msat(0);

                for output in funds_response.outputs {
                    match output.status {
                        ListfundsOutputsStatus::UNCONFIRMED => {
                            on_chain_total = on_chain_total + output.amount_msat;
                        }
                        ListfundsOutputsStatus::IMMATURE => {
                            on_chain_total = on_chain_total + output.amount_msat;
                        }
                        ListfundsOutputsStatus::CONFIRMED => {
                            on_chain_total = on_chain_total + output.amount_msat;
                            on_chain_spendable = on_chain_spendable + output.amount_msat;
                        }
                        ListfundsOutputsStatus::SPENT => (),
                    }
                }

                for channel in funds_response.channels {
                    ln = ln + channel.our_amount_msat;
                }

                BalanceResponse {
                    on_chain_spendable: Amount::from_msat(on_chain_spendable.msat()),
                    on_chain_total: Amount::from_msat(on_chain_total.msat()),
                    ln: Amount::from_msat(ln.msat()),
                }
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                return Err(Error::WrongClnResponse.into());
            }
        };

        Ok(balance)
    }

    async fn create_invoice(
        &self,
        amount: Amount,
        description: String,
    ) -> Result<Bolt11Invoice, Self::Err> {
        let mut cln_client = self.cln_client.lock().await;

        let amount_msat = AmountOrAny::Amount(CLN_Amount::from_msat(amount.to_msat()));
        let cln_response = cln_client
            .call(cln_rpc::Request::Invoice(InvoiceRequest {
                amount_msat,
                description,
                label: Uuid::new_v4().to_string(),
                expiry: Some(3600),
                fallbacks: None,
                preimage: None,
                cltv: None,
                deschashonly: None,
                exposeprivatechannels: None,
            }))
            .await
            .map_err(Error::from)?;

        let invoice = match cln_response {
            cln_rpc::Response::Invoice(invoice_res) => {
                Bolt11Invoice::from_str(&invoice_res.bolt11)?
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                return Err(Error::WrongClnResponse.into());
            }
        };

        Ok(invoice)
    }
}

pub fn fee_reserve(invoice_amount: Amount) -> Amount {
    let fee_reserse = (u64::from(invoice_amount) as f64 * 0.01) as u64;

    Amount::from(fee_reserse)
}

pub fn cln_invoice_status_to_status(status: ListinvoicesInvoicesStatus) -> InvoiceStatus {
    match status {
        ListinvoicesInvoicesStatus::UNPAID => InvoiceStatus::Unpaid,
        ListinvoicesInvoicesStatus::PAID => InvoiceStatus::Paid,
        ListinvoicesInvoicesStatus::EXPIRED => InvoiceStatus::Expired,
    }
}
