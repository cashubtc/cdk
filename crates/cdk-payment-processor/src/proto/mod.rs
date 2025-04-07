//! Proto types for payment processor

use std::str::FromStr;

use cdk_common::payment::{
    CreateIncomingPaymentResponse, MakePaymentResponse as CdkMakePaymentResponse,
    PaymentIdentifier as CdkPaymentIdentifier,
};
use cdk_common::util::hex;
use cdk_common::{Bolt11Invoice, CurrencyUnit, MeltQuoteBolt11Request};
use melt_options::Options;
mod client;
mod server;

pub use client::PaymentProcessorClient;
pub use server::PaymentProcessorServer;

tonic::include_proto!("cdk_payment_processor");

/// Convert from cdk PaymentIdentifier to proto PaymentIdentifier
fn cdk_payment_id_to_proto(value: &CdkPaymentIdentifier) -> PaymentIdentifier {
    match value {
        CdkPaymentIdentifier::PaymentHash(hash) => PaymentIdentifier {
            r#type: PaymentIdentifierType::PaymentHash as i32,
            value: Some(payment_identifier::Value::Hash(hex::encode(hash))),
        },
        CdkPaymentIdentifier::OfferId(offer_id) => PaymentIdentifier {
            r#type: PaymentIdentifierType::OfferId as i32,
            value: Some(payment_identifier::Value::Id(offer_id.clone())),
        },
        CdkPaymentIdentifier::Label(label) => PaymentIdentifier {
            r#type: PaymentIdentifierType::Label as i32,
            value: Some(payment_identifier::Value::Id(label.clone())),
        },
        CdkPaymentIdentifier::Bolt12PaymentHash(hash) => PaymentIdentifier {
            r#type: PaymentIdentifierType::Bolt12PaymentHash as i32,
            value: Some(payment_identifier::Value::Hash(hex::encode(hash))),
        },
        CdkPaymentIdentifier::CustomId(id) => PaymentIdentifier {
            r#type: PaymentIdentifierType::CustomId as i32,
            value: Some(payment_identifier::Value::Id(id.clone())),
        },
    }
}

/// Convert from proto PaymentIdentifier to cdk PaymentIdentifier
fn proto_to_cdk_payment_id(
    value: &PaymentIdentifier,
) -> Result<CdkPaymentIdentifier, crate::error::Error> {
    let value_str = match &value.value {
        Some(payment_identifier::Value::Hash(hash)) => hash.as_str(),
        Some(payment_identifier::Value::Id(id)) => id.as_str(),
        None => return Err(crate::error::Error::InvalidId),
    };

    match value.r#type() {
        PaymentIdentifierType::PaymentHash => Ok(CdkPaymentIdentifier::PaymentHash(
            hex::decode(value_str)
                .map_err(|_| crate::error::Error::InvalidHash)?
                .try_into()
                .map_err(|_| crate::error::Error::InvalidHash)?,
        )),
        PaymentIdentifierType::OfferId => Ok(CdkPaymentIdentifier::OfferId(value_str.to_string())),
        PaymentIdentifierType::Label => Ok(CdkPaymentIdentifier::Label(value_str.to_string())),
        PaymentIdentifierType::Bolt12PaymentHash => Ok(CdkPaymentIdentifier::Bolt12PaymentHash(
            hex::decode(value_str)
                .map_err(|_| crate::error::Error::InvalidHash)?
                .try_into()
                .map_err(|_| crate::error::Error::InvalidHash)?,
        )),
        PaymentIdentifierType::CustomId => {
            Ok(CdkPaymentIdentifier::CustomId(value_str.to_string()))
        }
    }
}

impl TryFrom<MakePaymentResponse> for CdkMakePaymentResponse {
    type Error = crate::error::Error;
    fn try_from(value: MakePaymentResponse) -> Result<Self, Self::Error> {
        let payment_lookup_id = value
            .payment_identifier
            .as_ref()
            .ok_or(crate::error::Error::InvalidId)
            .and_then(proto_to_cdk_payment_id)?;

        Ok(Self {
            payment_lookup_id,
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
            payment_identifier: Some(cdk_payment_id_to_proto(&value.payment_lookup_id)),
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
            request_identifier: Some(cdk_payment_id_to_proto(&value.request_lookup_id)),
            request: value.request.to_string(),
            expiry: value.expiry,
        }
    }
}

impl TryFrom<CreatePaymentResponse> for CreateIncomingPaymentResponse {
    type Error = crate::error::Error;

    fn try_from(value: CreatePaymentResponse) -> Result<Self, Self::Error> {
        let request_lookup_id = value
            .request_identifier
            .as_ref()
            .ok_or(crate::error::Error::InvalidId)
            .and_then(proto_to_cdk_payment_id)?;

        Ok(Self {
            request_lookup_id,
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
            request_type: OutgoingPaymentRequestType::Bolt11Invoice as i32,
        }
    }
}

impl From<cdk_common::payment::PaymentQuoteResponse> for PaymentQuoteResponse {
    fn from(value: cdk_common::payment::PaymentQuoteResponse) -> Self {
        Self {
            request_identifier: Some(cdk_payment_id_to_proto(&value.request_lookup_id)),
            amount: value.amount.into(),
            fee: value.fee.into(),
            state: QuoteState::from(value.state).into(),
            melt_options: value.options.map(|o| o.into()),
        }
    }
}

impl From<cdk_common::nut23::MeltOptions> for MeltOptions {
    fn from(value: cdk_common::nut23::MeltOptions) -> Self {
        Self {
            options: Some(value.into()),
        }
    }
}

impl From<cdk_common::nut23::MeltOptions> for Options {
    fn from(value: cdk_common::nut23::MeltOptions) -> Self {
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

impl From<MeltOptions> for cdk_common::nut23::MeltOptions {
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
            invoice: bolt12.invoice.map(|i| i.into_bytes()),
        }
    }
}

impl From<cdk_common::payment::PaymentQuoteOptions> for PaymentQuoteOptions {
    fn from(value: cdk_common::payment::PaymentQuoteOptions) -> Self {
        match value {
            cdk_common::payment::PaymentQuoteOptions::Bolt12 { invoice } => Self {
                melt_options: Some(payment_quote_options::MeltOptions::Bolt12(Bolt12Options {
                    invoice: invoice.map(|invoice| String::from_utf8(invoice).unwrap_or_default()),
                })),
            },
        }
    }
}

impl TryFrom<PaymentQuoteResponse> for cdk_common::payment::PaymentQuoteResponse {
    type Error = crate::error::Error;

    fn try_from(value: PaymentQuoteResponse) -> Result<Self, Self::Error> {
        let request_lookup_id = value
            .request_identifier
            .as_ref()
            .ok_or(crate::error::Error::InvalidId)
            .and_then(proto_to_cdk_payment_id)?;

        Ok(Self {
            request_lookup_id,
            amount: value.amount.into(),
            fee: value.fee.into(),
            state: value.state().into(),
            options: value.melt_options.map(|o| o.into()),
        })
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

impl From<cdk_common::nut23::QuoteState> for QuoteState {
    fn from(value: cdk_common::nut23::QuoteState) -> Self {
        match value {
            cdk_common::MintQuoteState::Unpaid => Self::Unpaid,
            cdk_common::MintQuoteState::Paid => Self::Paid,
            cdk_common::MintQuoteState::Issued => Self::Issued,
            cdk_common::MintQuoteState::Pending => Self::Pending,
        }
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
