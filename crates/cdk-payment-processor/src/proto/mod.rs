use std::str::FromStr;

use cdk_common::payment::{
    CreateIncomingPaymentResponse, MakePaymentResponse as CdkMakePaymentResponse,
    PaymentIdentifier as CdkPaymentIdentifier, WaitPaymentResponse,
};
use cdk_common::{CurrencyUnit, MeltOptions as CdkMeltOptions};

mod client;
mod server;

pub use client::PaymentProcessorClient;
pub use server::PaymentProcessorServer;

tonic::include_proto!("cdk_payment_processor");

impl From<CdkPaymentIdentifier> for PaymentIdentifier {
    fn from(value: CdkPaymentIdentifier) -> Self {
        match value {
            CdkPaymentIdentifier::Label(id) => Self {
                r#type: PaymentIdentifierType::Label.into(),
                value: Some(payment_identifier::Value::Id(id)),
            },
            CdkPaymentIdentifier::OfferId(id) => Self {
                r#type: PaymentIdentifierType::OfferId.into(),
                value: Some(payment_identifier::Value::Id(id)),
            },
            CdkPaymentIdentifier::PaymentHash(hash) => Self {
                r#type: PaymentIdentifierType::PaymentHash.into(),
                value: Some(payment_identifier::Value::Hash(hex::encode(hash))),
            },
            CdkPaymentIdentifier::Bolt12PaymentHash(hash) => Self {
                r#type: PaymentIdentifierType::Bolt12PaymentHash.into(),
                value: Some(payment_identifier::Value::Hash(hex::encode(hash))),
            },
            CdkPaymentIdentifier::CustomId(id) => Self {
                r#type: PaymentIdentifierType::CustomId.into(),
                value: Some(payment_identifier::Value::Id(id)),
            },
            CdkPaymentIdentifier::PaymentId(hash) => Self {
                r#type: PaymentIdentifierType::PaymentId.into(),
                value: Some(payment_identifier::Value::Hash(hex::encode(hash))),
            },
        }
    }
}

impl TryFrom<PaymentIdentifier> for CdkPaymentIdentifier {
    type Error = crate::error::Error;

    fn try_from(value: PaymentIdentifier) -> Result<Self, Self::Error> {
        match (value.r#type(), value.value) {
            (PaymentIdentifierType::Label, Some(payment_identifier::Value::Id(id))) => {
                Ok(CdkPaymentIdentifier::Label(id))
            }
            (PaymentIdentifierType::OfferId, Some(payment_identifier::Value::Id(id))) => {
                Ok(CdkPaymentIdentifier::OfferId(id))
            }
            (PaymentIdentifierType::PaymentHash, Some(payment_identifier::Value::Hash(hash))) => {
                let decoded = hex::decode(hash)?;
                let hash_array: [u8; 32] = decoded
                    .try_into()
                    .map_err(|_| crate::error::Error::InvalidHash)?;
                Ok(CdkPaymentIdentifier::PaymentHash(hash_array))
            }
            (
                PaymentIdentifierType::Bolt12PaymentHash,
                Some(payment_identifier::Value::Hash(hash)),
            ) => {
                let decoded = hex::decode(hash)?;
                let hash_array: [u8; 32] = decoded
                    .try_into()
                    .map_err(|_| crate::error::Error::InvalidHash)?;
                Ok(CdkPaymentIdentifier::Bolt12PaymentHash(hash_array))
            }
            (PaymentIdentifierType::CustomId, Some(payment_identifier::Value::Id(id))) => {
                Ok(CdkPaymentIdentifier::CustomId(id))
            }
            _ => Err(crate::error::Error::InvalidPaymentIdentifier),
        }
    }
}

impl TryFrom<MakePaymentResponse> for CdkMakePaymentResponse {
    type Error = crate::error::Error;
    fn try_from(value: MakePaymentResponse) -> Result<Self, Self::Error> {
        let status = value.status().as_str_name().parse()?;
        let payment_proof = value.payment_proof;
        let total_spent = value.total_spent.into();
        let unit = CurrencyUnit::from_str(&value.unit)?;
        let payment_identifier = value
            .payment_identifier
            .ok_or(crate::error::Error::InvalidPaymentIdentifier)?;
        Ok(Self {
            payment_lookup_id: payment_identifier.try_into()?,
            payment_proof,
            status,
            total_spent,
            unit,
        })
    }
}

impl From<CdkMakePaymentResponse> for MakePaymentResponse {
    fn from(value: CdkMakePaymentResponse) -> Self {
        Self {
            payment_identifier: Some(value.payment_lookup_id.into()),
            payment_proof: value.payment_proof,
            status: QuoteState::from(value.status).into(),
            total_spent: value.total_spent.into(),
            unit: value.unit.to_string(),
        }
    }
}

impl From<CreateIncomingPaymentResponse> for CreatePaymentResponse {
    fn from(value: CreateIncomingPaymentResponse) -> Self {
        Self {
            request_identifier: Some(value.request_lookup_id.into()),
            request: value.request,
            expiry: value.expiry,
        }
    }
}

impl TryFrom<CreatePaymentResponse> for CreateIncomingPaymentResponse {
    type Error = crate::error::Error;

    fn try_from(value: CreatePaymentResponse) -> Result<Self, Self::Error> {
        let request_identifier = value
            .request_identifier
            .ok_or(crate::error::Error::InvalidPaymentIdentifier)?;
        Ok(Self {
            request_lookup_id: request_identifier.try_into()?,
            request: value.request,
            expiry: value.expiry,
        })
    }
}

impl From<cdk_common::payment::PaymentQuoteResponse> for PaymentQuoteResponse {
    fn from(value: cdk_common::payment::PaymentQuoteResponse) -> Self {
        Self {
            request_identifier: value.request_lookup_id.map(|i| i.into()),
            amount: value.amount.into(),
            fee: value.fee.into(),
            unit: value.unit.to_string(),
            state: QuoteState::from(value.state).into(),
        }
    }
}

impl From<PaymentQuoteResponse> for cdk_common::payment::PaymentQuoteResponse {
    fn from(value: PaymentQuoteResponse) -> Self {
        let state_val = value.state();
        let request_identifier = value.request_identifier;

        Self {
            request_lookup_id: request_identifier
                .map(|i| i.try_into().expect("valid request identifier")),
            amount: value.amount.into(),
            fee: value.fee.into(),
            unit: CurrencyUnit::from_str(&value.unit).unwrap_or_default(),
            state: state_val.into(),
        }
    }
}

impl From<MeltOptions> for CdkMeltOptions {
    fn from(value: MeltOptions) -> Self {
        match value.options.expect("option defined") {
            melt_options::Options::Mpp(mpp) => Self::Mpp {
                mpp: cashu::nuts::nut15::Mpp {
                    amount: mpp.amount.into(),
                },
            },
            melt_options::Options::Amountless(amountless) => Self::Amountless {
                amountless: cashu::nuts::nut23::Amountless {
                    amount_msat: amountless.amount_msat.into(),
                },
            },
        }
    }
}

impl From<CdkMeltOptions> for MeltOptions {
    fn from(value: CdkMeltOptions) -> Self {
        match value {
            CdkMeltOptions::Mpp { mpp } => Self {
                options: Some(melt_options::Options::Mpp(Mpp {
                    amount: mpp.amount.into(),
                })),
            },
            CdkMeltOptions::Amountless { amountless } => Self {
                options: Some(melt_options::Options::Amountless(Amountless {
                    amount_msat: amountless.amount_msat.into(),
                })),
            },
        }
    }
}

impl From<QuoteState> for cdk_common::nuts::MeltQuoteState {
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

impl From<cdk_common::nuts::MeltQuoteState> for QuoteState {
    fn from(value: cdk_common::nuts::MeltQuoteState) -> Self {
        match value {
            cdk_common::nuts::MeltQuoteState::Unpaid => Self::Unpaid,
            cdk_common::nuts::MeltQuoteState::Paid => Self::Paid,
            cdk_common::nuts::MeltQuoteState::Pending => Self::Pending,
            cdk_common::nuts::MeltQuoteState::Unknown => Self::Unknown,
            cdk_common::nuts::MeltQuoteState::Failed => Self::Failed,
        }
    }
}

impl From<cdk_common::nuts::MintQuoteState> for QuoteState {
    fn from(value: cdk_common::nuts::MintQuoteState) -> Self {
        match value {
            cdk_common::nuts::MintQuoteState::Unpaid => Self::Unpaid,
            cdk_common::nuts::MintQuoteState::Paid => Self::Paid,
            cdk_common::nuts::MintQuoteState::Issued => Self::Issued,
        }
    }
}

impl From<WaitPaymentResponse> for WaitIncomingPaymentResponse {
    fn from(value: WaitPaymentResponse) -> Self {
        Self {
            payment_identifier: Some(value.payment_identifier.into()),
            payment_amount: value.payment_amount.into(),
            unit: value.unit.to_string(),
            payment_id: value.payment_id,
        }
    }
}

impl TryFrom<WaitIncomingPaymentResponse> for WaitPaymentResponse {
    type Error = crate::error::Error;

    fn try_from(value: WaitIncomingPaymentResponse) -> Result<Self, Self::Error> {
        let payment_identifier = value
            .payment_identifier
            .ok_or(crate::error::Error::InvalidPaymentIdentifier)?
            .try_into()?;

        Ok(Self {
            payment_identifier,
            payment_amount: value.payment_amount.into(),
            unit: CurrencyUnit::from_str(&value.unit)?,
            payment_id: value.payment_id,
        })
    }
}
