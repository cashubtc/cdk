#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

pub mod error;
/// Protocol types and functionality for the CDK payment processor
pub mod proto;

pub use proto::cdk_payment_processor_client::CdkPaymentProcessorClient;
pub use proto::cdk_payment_processor_server::CdkPaymentProcessorServer;
pub use proto::{PaymentProcessorClient, PaymentProcessorServer};
#[doc(hidden)]
pub use tonic;
