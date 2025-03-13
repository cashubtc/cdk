pub mod error;
pub mod proto;

use std::any::Any;

pub use proto::cdk_payment_processor_client::CdkPaymentProcessorClient;
pub use proto::cdk_payment_processor_server::CdkPaymentProcessorServer;
pub use proto::{PaymentProcessorClient, PaymentProcessorServer};
#[doc(hidden)]
pub use tonic;

impl cdk_common::payment::BaseMintSettings for proto::SettingsResponse {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
