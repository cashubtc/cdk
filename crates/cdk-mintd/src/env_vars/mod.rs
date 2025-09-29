//! Environment variables module
//!
//! This module contains all environment variable definitions and parsing logic
//! organized by component.

mod common;
mod database;
mod info;
mod ln;
mod mint_info;

#[cfg(feature = "auth")]
mod auth;
#[cfg(feature = "cln")]
mod cln;
#[cfg(feature = "fakewallet")]
mod fake_wallet;
#[cfg(feature = "grpc-processor")]
mod grpc_processor;
#[cfg(feature = "ldk-node")]
mod ldk_node;
#[cfg(feature = "lnbits")]
mod lnbits;
#[cfg(feature = "lnd")]
mod lnd;
#[cfg(feature = "management-rpc")]
mod management_rpc;
#[cfg(feature = "prometheus")]
mod prometheus;

use std::env;
use std::str::FromStr;

use anyhow::{anyhow, bail, Result};
#[cfg(feature = "auth")]
pub use auth::*;
#[cfg(feature = "cln")]
pub use cln::*;
pub use common::*;
pub use database::*;
#[cfg(feature = "fakewallet")]
pub use fake_wallet::*;
#[cfg(feature = "grpc-processor")]
pub use grpc_processor::*;
#[cfg(feature = "ldk-node")]
pub use ldk_node::*;
pub use ln::*;
#[cfg(feature = "lnbits")]
pub use lnbits::*;
#[cfg(feature = "lnd")]
pub use lnd::*;
#[cfg(feature = "management-rpc")]
pub use management_rpc::*;
pub use mint_info::*;
#[cfg(feature = "prometheus")]
pub use prometheus::*;

use crate::config::{DatabaseEngine, PaymentBackendType, Settings};

impl Settings {
    pub fn from_env(&mut self) -> Result<Self> {
        if let Ok(database) = env::var(DATABASE_ENV_VAR) {
            let engine = DatabaseEngine::from_str(&database).map_err(|err| anyhow!(err))?;
            self.database.engine = engine;
        }

        // Parse PostgreSQL-specific configuration from environment variables
        if self.database.engine == DatabaseEngine::Postgres {
            self.database.postgres = Some(
                self.database
                    .postgres
                    .clone()
                    .unwrap_or_default()
                    .from_env(),
            );
        }

        // Parse auth database configuration from environment variables (when auth is enabled)
        #[cfg(feature = "auth")]
        {
            self.auth_database = Some(crate::config::AuthDatabase {
                postgres: Some(
                    self.auth_database
                        .clone()
                        .unwrap_or_default()
                        .postgres
                        .unwrap_or_default()
                        .from_env(),
                ),
            });
        }

        self.info = self.info.clone().from_env();
        self.mint_info = self.mint_info.clone().from_env();
        self.payment_backend = self.payment_backend.clone().from_env();

        #[cfg(feature = "auth")]
        {
            // Check env vars for auth config even if None
            let auth = self.auth.clone().unwrap_or_default().from_env();

            // Only set auth if auth_enabled flag is true
            if auth.auth_enabled {
                self.auth = Some(auth);
            } else {
                self.auth = None;
            }
        }

        #[cfg(feature = "management-rpc")]
        {
            self.mint_management_rpc = Some(
                self.mint_management_rpc
                    .clone()
                    .unwrap_or_default()
                    .from_env(),
            );
        }

        #[cfg(feature = "prometheus")]
        {
            self.prometheus = Some(self.prometheus.clone().unwrap_or_default().from_env());
        }

        match self.payment_backend.name {
            #[cfg(feature = "cln")]
            PaymentBackendType::Cln => {
                self.cln = Some(self.cln.clone().unwrap_or_default().from_env());
            }
            #[cfg(feature = "lnbits")]
            PaymentBackendType::LNbits => {
                self.lnbits = Some(self.lnbits.clone().unwrap_or_default().from_env());
            }
            #[cfg(feature = "fakewallet")]
            PaymentBackendType::FakeWallet => {
                self.fake_wallet = Some(self.fake_wallet.clone().unwrap_or_default().from_env());
            }
            #[cfg(feature = "lnd")]
            PaymentBackendType::Lnd => {
                self.lnd = Some(self.lnd.clone().unwrap_or_default().from_env());
            }
            #[cfg(feature = "ldk-node")]
            PaymentBackendType::LdkNode => {
                self.ldk_node = Some(self.ldk_node.clone().unwrap_or_default().from_env());
            }
            #[cfg(feature = "grpc-processor")]
            PaymentBackendType::GrpcProcessor => {
                self.grpc_processor =
                    Some(self.grpc_processor.clone().unwrap_or_default().from_env());
            }
            PaymentBackendType::None => bail!("Ln backend must be set"),
            #[allow(unreachable_patterns)]
            _ => bail!("Selected Ln backend is not enabled in this build"),
        }

        Ok(self.clone())
    }
}
