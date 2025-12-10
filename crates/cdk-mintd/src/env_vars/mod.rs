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

use crate::config::{DatabaseEngine, PaymentBackendKind, Settings};

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
        // The following section is providing backwards compatability #1127
        // todo -- remove this section
        if self.payment_backend.kind == PaymentBackendKind::None
            && self.ln.ln_backend != PaymentBackendKind::None
        {
            self.using_deprecated_config = Some(true);
            let mut ln = self.ln.clone();
            // old ln_backend to kind
            ln.kind = ln.ln_backend.clone();
            // old specific settings to payment_backend
            ln.lnbits = self.lnbits.clone();
            ln.fake_wallet = self.fake_wallet.clone();
            ln.cln = self.cln.clone();
            ln.lnd = self.lnd.clone();
            ln.grpc_processor = self.grpc_processor.clone();
            self.payment_backend = ln;
        }
        #[cfg(feature = "auth")]
        {
            // Check env vars for auth config even if None
            let auth = self.auth.clone().unwrap_or_default().from_env();

            // Only set auth if the auth_enabled flag is true
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

        match self.payment_backend.kind {
            #[cfg(feature = "cln")]
            PaymentBackendKind::Cln => {
                self.payment_backend.cln = Some(
                    self.payment_backend
                        .cln
                        .clone()
                        .unwrap_or_default()
                        .from_env(),
                );
            }
            #[cfg(feature = "lnbits")]
            PaymentBackendKind::LNbits => {
                self.payment_backend.lnbits = Some(
                    self.payment_backend
                        .lnbits
                        .clone()
                        .unwrap_or_default()
                        .from_env(),
                );
            }
            #[cfg(feature = "fakewallet")]
            PaymentBackendKind::FakeWallet => {
                self.payment_backend.fake_wallet = Some(
                    self.payment_backend
                        .fake_wallet
                        .clone()
                        .unwrap_or_default()
                        .from_env(),
                );
            }
            #[cfg(feature = "lnd")]
            PaymentBackendKind::Lnd => {
                self.payment_backend.lnd = Some(
                    self.payment_backend
                        .lnd
                        .clone()
                        .unwrap_or_default()
                        .from_env(),
                );
            }
            #[cfg(feature = "ldk-node")]
            PaymentBackendKind::LdkNode => {
                self.payment_backend.ldk_node = Some(
                    self.payment_backend
                        .ldk_node
                        .clone()
                        .unwrap_or_default()
                        .from_env(),
                );
            }
            #[cfg(feature = "grpc-processor")]
            PaymentBackendKind::GrpcProcessor => {
                self.payment_backend.grpc_processor = Some(
                    self.payment_backend
                        .grpc_processor
                        .clone()
                        .unwrap_or_default()
                        .from_env(),
                );
            }
            PaymentBackendKind::None => bail!("Ln backend must be set"),
            #[allow(unreachable_patterns)]
            _ => bail!("Selected Ln backend is not enabled in this build"),
        }

        Ok(self.clone())
    }
}
