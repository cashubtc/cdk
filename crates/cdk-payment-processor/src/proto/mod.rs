//! Proto types for payment processor

use std::str::FromStr;

use cdk_common::payment::{
    CreateIncomingPaymentResponse, MakePaymentResponse as CdkMakePaymentResponse,
};
use cdk_common::{Bolt11Invoice, CurrencyUnit, MeltQuoteBolt11Request, PaymentMethod};
use melt_options::Options;
mod client;
mod server;

pub use client::PaymentProcessorClient;
pub use server::PaymentProcessorServer;

tonic::include_proto!("cdk_payment_processor");

impl TryFrom<MakePaymentResponse> for CdkMakePaymentResponse {
    type Error = crate::error::Error;
    fn try_from(value: MakePaymentResponse) -> Result<Self, Self::Error> {
        Ok(Self {
            payment_lookup_id: value.payment_lookup_id.clone(),
            payment_proof: value.payment_proof.clone(),
            status: value.status().as_str_name().parse()?,
            total_spent: value.total_spent.into(),
            unit: value.unit.parse()?,
        })
    }
}

impl From<CdkMakePaymentResponse> for MakePaymentResponse {
    fn from(value: CdkMakePaymentResponse) -> Self {
        Self {
            payment_lookup_id: value.payment_lookup_id.clone(),
            payment_proof: value.payment_proof.clone(),
            status: QuoteState::from(value.status).into(),
            total_spent: value.total_spent.into(),
            unit: value.unit.to_string(),
        }
    }
}

impl From<CreateIncomingPaymentResponse> for CreatePaymentResponse {
    fn from(value: CreateIncomingPaymentResponse) -> Self {
        Self {
            request_lookup_id: value.request_lookup_id,
            request: value.request.to_string(),
            expiry: value.expiry,
        }
    }
}

impl TryFrom<CreatePaymentResponse> for CreateIncomingPaymentResponse {
    type Error = crate::error::Error;

    fn try_from(value: CreatePaymentResponse) -> Result<Self, Self::Error> {
        Ok(Self {
            request_lookup_id: value.request_lookup_id,
            request: value.request,
            expiry: value.expiry,
        })
    }
}

impl From<&MeltQuoteBolt11Request> for PaymentQuoteRequest {
    fn from(value: &MeltQuoteBolt11Request) -> Self {
        Self {
            request: value.request.to_string(),
            unit: value.unit.to_string(),
            options: value.options.map(|o| o.into()),
        }
    }
}

impl From<cdk_common::payment::PaymentQuoteResponse> for PaymentQuoteResponse {
    fn from(value: cdk_common::payment::PaymentQuoteResponse) -> Self {
        Self {
            request_lookup_id: value.request_lookup_id,
            amount: value.amount.into(),
            fee: value.fee.into(),
            state: QuoteState::from(value.state).into(),
            melt_options: value.options.map(|o| o.into()),
        }
    }
}

impl From<cdk_common::nut05::MeltOptions> for MeltOptions {
    fn from(value: cdk_common::nut05::MeltOptions) -> Self {
        Self {
            options: Some(value.into()),
        }
    }
}

impl From<cdk_common::nut05::MeltOptions> for Options {
    fn from(value: cdk_common::nut05::MeltOptions) -> Self {
        match value {
            cdk_common::MeltOptions::Mpp { mpp } => Self::Mpp(Mpp {
                amount: mpp.amount.into(),
            }),
            cdk_common::MeltOptions::Amountless { amountless } => Self::Amountless(Amountless {
                amount_msat: amountless.amount_msat.into(),
            }),
        }
    }
}

impl From<MeltOptions> for cdk_common::nut05::MeltOptions {
    fn from(value: MeltOptions) -> Self {
        let options = value.options.expect("option defined");
        match options {
            Options::Mpp(mpp) => cdk_common::MeltOptions::new_mpp(mpp.amount),
            Options::Amountless(amountless) => {
                cdk_common::MeltOptions::new_amountless(amountless.amount_msat)
            }
        }
    }
}

impl From<PaymentQuoteOptions> for cdk_common::payment::PaymentQuoteOptions {
    fn from(value: PaymentQuoteOptions) -> Self {
        let melt_options = value.melt_options.expect("option defined");

        // Extract the Bolt12Options from the oneof field
        let payment_quote_options::MeltOptions::Bolt12(bolt12) = melt_options;
        Self::Bolt12 {
            invoice: bolt12.invoice,
        }
    }
}

impl From<cdk_common::payment::PaymentQuoteOptions> for PaymentQuoteOptions {
    fn from(value: cdk_common::payment::PaymentQuoteOptions) -> Self {
        match value {
            cdk_common::payment::PaymentQuoteOptions::Bolt12 { invoice } => Self {
                melt_options: Some(payment_quote_options::MeltOptions::Bolt12(Bolt12Options {
                    invoice,
                })),
            },
        }
    }
}

impl From<PaymentQuoteResponse> for cdk_common::payment::PaymentQuoteResponse {
    fn from(value: PaymentQuoteResponse) -> Self {
        Self {
            request_lookup_id: value.request_lookup_id.clone(),
            amount: value.amount.into(),
            fee: value.fee.into(),
            state: value.state().into(),
            options: value.melt_options.map(|o| o.into()),
        }
    }
}

impl From<QuoteState> for cdk_common::nut05::QuoteState {
    fn from(value: QuoteState) -> Self {
        match value {
            QuoteState::Unpaid => Self::Unpaid,
            QuoteState::Paid => Self::Paid,
            QuoteState::Pending => Self::Pending,
            QuoteState::Unknown => Self::Unknown,
            QuoteState::Failed => Self::Failed,
            QuoteState::Issued => Self::Unknown,
        }
    }
}

impl From<cdk_common::nut05::QuoteState> for QuoteState {
    fn from(value: cdk_common::nut05::QuoteState) -> Self {
        match value {
            cdk_common::MeltQuoteState::Unpaid => Self::Unpaid,
            cdk_common::MeltQuoteState::Paid => Self::Paid,
            cdk_common::MeltQuoteState::Pending => Self::Pending,
            cdk_common::MeltQuoteState::Unknown => Self::Unknown,
            cdk_common::MeltQuoteState::Failed => Self::Failed,
        }
    }
}

impl From<cdk_common::nut04::QuoteState> for QuoteState {
    fn from(value: cdk_common::nut04::QuoteState) -> Self {
        match value {
            cdk_common::MintQuoteState::Unpaid => Self::Unpaid,
            cdk_common::MintQuoteState::Paid => Self::Paid,
            cdk_common::MintQuoteState::Issued => Self::Issued,
        }
    }
}

impl From<cdk_common::mint::MeltQuote> for MeltQuote {
    fn from(value: cdk_common::mint::MeltQuote) -> Self {
        Self {
            id: value.id.to_string(),
            unit: value.unit.to_string(),
            amount: value.amount.into(),
            request: value.request.to_string(),
            fee_reserve: value.fee_reserve.into(),
            state: QuoteState::from(value.state).into(),
            expiry: value.expiry,
            payment_preimage: value.payment_preimage,
            request_lookup_id: value.request_lookup_id,
            msat_to_pay: value.msat_to_pay.map(|a| a.into()),
            payment_method: value.payment_method.to_string(),
            created_time: value.created_time,
            paid_time: value.paid_time,
        }
    }
}

impl TryFrom<MeltQuote> for cdk_common::mint::MeltQuote {
    type Error = crate::error::Error;

    fn try_from(value: MeltQuote) -> Result<Self, Self::Error> {
        let bolt11 = Bolt11Invoice::from_str(&value.request)?;

        Ok(Self {
            id: value
                .id
                .parse()
                .map_err(|_| crate::error::Error::InvalidId)?,
            unit: value.unit.parse()?,
            amount: value.amount.into(),
            request: cdk_common::mint::MeltPaymentRequest::Bolt11 { bolt11 },
            fee_reserve: value.fee_reserve.into(),
            state: cdk_common::nut05::QuoteState::from(value.state()),
            expiry: value.expiry,
            payment_preimage: value.payment_preimage,
            request_lookup_id: value.request_lookup_id,
            msat_to_pay: value.msat_to_pay.map(|a| a.into()),
            created_time: value.created_time,
            paid_time: value.paid_time,
            payment_method: PaymentMethod::from_str(&value.payment_method)?,
        })
    }
}

impl TryFrom<PaymentQuoteRequest> for MeltQuoteBolt11Request {
    type Error = crate::error::Error;

    fn try_from(value: PaymentQuoteRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            request: Bolt11Invoice::from_str(&value.request)?,
            unit: CurrencyUnit::from_str(&value.unit)?,
            options: value.options.map(|o| o.into()),
        })
    }
}
