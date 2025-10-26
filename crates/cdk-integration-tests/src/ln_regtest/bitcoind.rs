//! Bitcoind

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;

use crate::util::{ProcessHandle, ProcessManager};

/// Bitcoind
#[derive(Clone)]
pub struct Bitcoind {
    pub rpc_user: String,
    pub rpc_password: String,
    pub data_dir: PathBuf,
    pub zmq_raw_block: String,
    pub zmq_raw_tx: String,
    process_handle: Option<ProcessHandle>,
}

impl Bitcoind {
    /// Create new [`Bitcoind`]
    pub fn new(
        data_dir: PathBuf,
        rpc_user: String,
        rpc_password: String,
        zmq_raw_block: String,
        zmq_raw_tx: String,
    ) -> Self {
        Bitcoind {
            rpc_user,
            rpc_password,
            data_dir,
            zmq_raw_block,
            zmq_raw_tx,
            process_handle: None,
        }
    }

    /// Start bitcoind using ProcessManager
    pub async fn start_bitcoind(&mut self, process_mgr: &ProcessManager) -> Result<()> {
        let start = Instant::now();
        tracing::info!("Starting bitcoind");

        std::fs::create_dir_all(&self.data_dir)?;
        tracing::debug!("Created bitcoind data dir: {}", self.data_dir.display());

        let cmd = crate::cmd!(
            "bitcoind",
            "-regtest",
            format!("-datadir={}", self.data_dir.display()),
            "-fallbackfee=0.00001",
            "-rpcallowip=0.0.0.0/0",
            format!("-rpcuser={}", self.rpc_user),
            format!("-rpcpassword={}", self.rpc_password),
            format!("-zmqpubrawblock={}", self.zmq_raw_block),
            format!("-zmqpubrawtx={}", self.zmq_raw_tx),
            "-deprecatedrpc=warnings"
        );

        let handle = process_mgr.spawn_daemon("bitcoind", cmd).await?;
        self.process_handle = Some(handle);

        // No sleep - bitcoind readiness is verified via RPC polling in init_bitcoin_client_async
        // settings.json is polled before LND initialization

        tracing::info!(
            "Bitcoind process spawned in {:.2}s",
            start.elapsed().as_secs_f64()
        );
        Ok(())
    }

    pub fn pid(&self) -> Option<u32> {
        self.process_handle.as_ref().and_then(|h| h.pid())
    }
}

impl Drop for Bitcoind {
    fn drop(&mut self) {
        let pid = self.pid();
        tracing::warn!(
            ?pid,
            has_process_handle = self.process_handle.is_some(),
            "Dropping Bitcoind struct - this will trigger ProcessHandle cleanup"
        );
        // ProcessHandle will handle cleanup automatically in its own Drop
    }
}
