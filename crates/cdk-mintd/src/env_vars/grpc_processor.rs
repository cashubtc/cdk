//! gRPC Payment Processor environment variables

use std::env;

use cdk::nuts::CurrencyUnit;

use crate::config::GrpcProcessor;

// gRPC Payment Processor environment variables
pub const ENV_GRPC_PROCESSOR_SUPPORTED_UNITS: &str =
    "CDK_MINTD_PAYMENT_BACKEND_GRPC_PAYMENT_PROCESSOR_SUPPORTED_UNITS";
pub const ENV_GRPC_PROCESSOR_ADDRESS: &str =
    "CDK_MINTD_PAYMENT_BACKEND_GRPC_PAYMENT_PROCESSOR_ADDRESS";
pub const ENV_GRPC_PROCESSOR_PORT: &str = "CDK_MINTD_PAYMENT_BACKEND_GRPC_PAYMENT_PROCESSOR_PORT";
pub const ENV_GRPC_PROCESSOR_TLS_DIR: &str =
    "CDK_MINTD_PAYMENT_BACKEND_GRPC_PAYMENT_PROCESSOR_TLS_DIR";

impl GrpcProcessor {
    pub fn from_env(mut self) -> Self {
        if let Ok(units_str) = env::var(ENV_GRPC_PROCESSOR_SUPPORTED_UNITS) {
            if let Ok(units) = units_str
                .split(',')
                .map(|s| s.trim().parse())
                .collect::<Result<Vec<CurrencyUnit>, _>>()
            {
                self.supported_units = units;
            }
        }

        if let Ok(addr) = env::var(ENV_GRPC_PROCESSOR_ADDRESS) {
            self.addr = addr;
        }

        if let Ok(port) = env::var(ENV_GRPC_PROCESSOR_PORT) {
            if let Ok(port) = port.parse() {
                self.port = port;
            }
        }

        if let Ok(tls_dir) = env::var(ENV_GRPC_PROCESSOR_TLS_DIR) {
            self.tls_dir = Some(tls_dir.into());
        }

        self
    }
}
