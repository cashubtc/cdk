// This file is deprecated and will be removed in a future version.
// Use grpc_processor.rs instead.

use std::env;

use cdk::nuts::CurrencyUnit;

use crate::config::GrpcProcessor;
use crate::env_vars::grpc_processor::*;

// These constants are kept for backward compatibility
pub const ENV_GRPC_PAYMENT_WALLET_SUPPORTED_UNITS: &str = ENV_GRPC_PROCESSOR_SUPPORTED_UNITS;
pub const ENV_GRPC_PAYMENT_PROCESSOR_ADDRESS: &str = ENV_GRPC_PROCESSOR_ADDRESS;
pub const ENV_GRPC_PAYMENT_PROCESSOR_PORT: &str = ENV_GRPC_PROCESSOR_PORT;
pub const ENV_GRPC_PAYMENT_PROCESSOR_TLS_DIR: &str = ENV_GRPC_PROCESSOR_TLS_DIR;

impl GrpcProcessor {
    // This implementation is kept for backward compatibility
    // The actual implementation is in grpc_processor.rs
    pub fn from_env(self) -> Self {
        // Delegate to the new implementation
        #[cfg(feature = "grpc-processor")]
        {
            return self.from_env();
        }
        
        #[cfg(not(feature = "grpc-processor"))]
        {
            self
        }
    }
}
