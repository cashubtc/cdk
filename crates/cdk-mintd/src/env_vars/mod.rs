#![allow(missing_docs)]
//! Environment variables module
//!
//! This module contains all environment variable definitions and parsing logic
//! organized by component.

mod common;
mod database;
mod info;
mod limits;
mod ln;
mod mint_info;
mod onchain;

mod auth;
#[cfg(feature = "bdk")]
mod bdk;
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
pub use auth::*;
#[cfg(feature = "bdk")]
pub use bdk::*;
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
pub use limits::*;
pub use ln::*;
#[cfg(feature = "lnbits")]
pub use lnbits::*;
#[cfg(feature = "lnd")]
pub use lnd::*;
#[cfg(feature = "management-rpc")]
pub use management_rpc::*;
pub use mint_info::*;
pub use onchain::*;
#[cfg(feature = "prometheus")]
pub use prometheus::*;

use crate::config::{DatabaseEngine, LnBackend, OnchainBackend, Settings};

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

        // Parse auth database configuration from environment variables
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

        self.info = self.info.clone().from_env();
        self.mint_info = self.mint_info.clone().from_env();
        self.ln = self.ln.clone().from_env();
        self.onchain = Some(self.onchain.clone().unwrap_or_default().from_env());
        self.limits = self.limits.clone().from_env();

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

        #[cfg(feature = "cln")]
        {
            let cln = self.cln.clone().unwrap_or_default().from_env();
            if cln.rpc_path.as_os_str().is_empty() {
                self.cln = None;
            } else {
                self.cln = Some(cln);
            }
        }

        #[cfg(feature = "lnbits")]
        {
            let lnbits = self.lnbits.clone().unwrap_or_default().from_env();
            if lnbits.admin_api_key.is_empty() {
                self.lnbits = None;
            } else {
                self.lnbits = Some(lnbits);
            }
        }

        #[cfg(feature = "fakewallet")]
        {
            // Fake wallet has defaults so it is always Some if feature enabled
            self.fake_wallet = Some(self.fake_wallet.clone().unwrap_or_default().from_env());
        }

        #[cfg(feature = "lnd")]
        {
            let lnd = self.lnd.clone().unwrap_or_default().from_env();
            if lnd.address.is_empty() {
                self.lnd = None;
            } else {
                self.lnd = Some(lnd);
            }
        }

        #[cfg(feature = "ldk-node")]
        {
            let ldk_node = self.ldk_node.clone().unwrap_or_default().from_env();
            if ldk_node.bitcoin_network.is_none() && ldk_node.esplora_url.is_none() {
                self.ldk_node = None;
            } else {
                self.ldk_node = Some(ldk_node);
            }
        }

        #[cfg(feature = "grpc-processor")]
        {
            let grpc_processor = self.grpc_processor.clone().unwrap_or_default().from_env();
            if grpc_processor.supported_units.is_empty() {
                self.grpc_processor = None;
            } else {
                self.grpc_processor = Some(grpc_processor);
            }
        }

        #[cfg(feature = "bdk")]
        {
            let bdk = self.bdk.clone().unwrap_or_default().from_env();
            if bdk.network.is_none() && bdk.mnemonic.is_none() {
                self.bdk = None;
            } else {
                self.bdk = Some(bdk);
            }
        }

        match self.ln.ln_backend {
            #[cfg(feature = "cln")]
            LnBackend::Cln => {}
            #[cfg(feature = "lnbits")]
            LnBackend::LNbits => {}
            #[cfg(feature = "fakewallet")]
            LnBackend::FakeWallet => {}
            #[cfg(feature = "lnd")]
            LnBackend::Lnd => {}
            #[cfg(feature = "ldk-node")]
            LnBackend::LdkNode => {}
            #[cfg(feature = "grpc-processor")]
            LnBackend::GrpcProcessor => {}
            LnBackend::None => {
                if let Some(ref onchain) = self.onchain {
                    if onchain.onchain_backend == OnchainBackend::None {
                        bail!("At least one payment backend (Lightning or On-chain) must be set");
                    }
                } else {
                    bail!("At least one payment backend (Lightning or On-chain) must be set");
                }
            }
            #[allow(unreachable_patterns)]
            _ => bail!("Selected Ln backend is not enabled in this build"),
        }

        Ok(self.clone())
    }
}
