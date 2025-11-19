//! CLAnd

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;

use crate::util::{ProcessHandle, ProcessManager};

/// Clnd
#[derive(Clone)]
pub struct Clnd {
    pub data_dir: PathBuf,
    pub bitcoin_data_dir: PathBuf,
    pub addr: PathBuf,
    pub bitcoin_rpc_user: String,
    pub bitcoin_rpc_password: String,
    process_handle: Option<ProcessHandle>,
}

impl Clnd {
    /// Create new [`Clnd`]
    pub fn new(
        bitcoin_data_dir: PathBuf,
        data_dir: PathBuf,
        addr: PathBuf,
        bitcoin_rpc_user: String,
        bitcoin_rpc_password: String,
    ) -> Self {
        Self {
            data_dir,
            bitcoin_data_dir,
            addr,
            bitcoin_rpc_user,
            bitcoin_rpc_password,
            process_handle: None,
        }
    }

    /// Start clnd using ProcessManager
    pub async fn start_clnd(
        &mut self,
        process_mgr: &ProcessManager,
        name: &str,
    ) -> Result<ProcessHandle> {
        let start = Instant::now();
        tracing::info!("Starting CLN node: {}", name);

        let cmd = crate::cmd!(
            "lightningd",
            format!("--bitcoin-datadir={}", self.bitcoin_data_dir.display()),
            "--network=regtest",
            "--experimental-offers",
            format!("--lightning-dir={}", self.data_dir.display()),
            format!("--bitcoin-rpcuser={}", self.bitcoin_rpc_user),
            format!("--bitcoin-rpcpassword={}", self.bitcoin_rpc_password),
            format!("--bind-addr={}", self.addr.display()),
            format!("--log-file={}", self.data_dir.join("debug.log").display())
        );

        let handle = process_mgr
            .spawn_daemon(&format!("cln-{}", name), cmd)
            .await?;
        self.process_handle = Some(handle.clone());

        // No sleep needed - readiness is checked via socket polling in init_cln_node_async

        tracing::info!(
            "CLN node {} started successfully in {:.2}s",
            name,
            start.elapsed().as_secs_f64()
        );
        Ok(handle)
    }

    pub fn pid(&self) -> Option<u32> {
        self.process_handle.as_ref().and_then(|h| h.pid())
    }
}

impl Drop for Clnd {
    fn drop(&mut self) {
        tracing::info!("Dropping clnd");
        // ProcessHandle will handle cleanup automatically in its own Drop
    }
}
