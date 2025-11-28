//! LND

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::time::sleep;

use crate::util::{ProcessHandle, ProcessManager};

/// Lnd
#[derive(Clone)]
pub struct Lnd {
    pub addr: PathBuf,
    pub data_dir: PathBuf,
    pub bitcoin_data_dir: PathBuf,
    pub rpc_listen: String,
    pub bitcoin_rpc_user: String,
    pub bitcoin_rpc_password: String,
    pub zmq_raw_block: String,
    pub zmq_raw_tx: String,
    process_handle: Option<ProcessHandle>,
}

impl Lnd {
    /// Create new [`Lnd`]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bitcoin_data_dir: PathBuf,
        data_dir: PathBuf,
        addr: PathBuf,
        rpc_listen: String,
        bitcoin_rpc_user: String,
        bitcoin_rpc_password: String,
        zmq_raw_block: String,
        zmq_raw_tx: String,
    ) -> Self {
        Self {
            data_dir,
            bitcoin_data_dir,
            addr,
            rpc_listen,
            bitcoin_rpc_user,
            bitcoin_rpc_password,
            zmq_raw_block,
            zmq_raw_tx,
            process_handle: None,
        }
    }

    /// Start lnd using ProcessManager
    pub async fn start_lnd(
        &mut self,
        process_mgr: &ProcessManager,
        name: &str,
    ) -> Result<ProcessHandle> {
        let start = Instant::now();
        tracing::info!("Starting LND node: {}", name);

        let cmd = crate::cmd!(
            "lnd",
            "--bitcoin.active",
            "--bitcoin.regtest",
            "--bitcoin.node=bitcoind",
            format!(
                "--bitcoind.config={}",
                self.bitcoin_data_dir
                    .join("regtest/settings.json")
                    .display()
            ),
            format!("--bitcoind.dir={}", self.bitcoin_data_dir.display()),
            format!("--bitcoind.rpcuser={}", self.bitcoin_rpc_user),
            format!("--bitcoind.rpcpass={}", self.bitcoin_rpc_password),
            format!("--rpclisten={}", self.rpc_listen),
            "--norest",
            format!("--lnddir={}", self.data_dir.display()),
            format!("--bitcoind.zmqpubrawblock={}", self.zmq_raw_block),
            format!("--bitcoind.zmqpubrawtx={}", self.zmq_raw_tx),
            "--noseedbackup",
            format!("--listen={}", self.addr.display()),
            format!("--externalip={}", self.addr.display())
        );

        let handle = process_mgr
            .spawn_daemon(&format!("lnd-{}", name), cmd)
            .await?;
        self.process_handle = Some(handle.clone());

        // Minimal sleep to let LND process spawn - RPC readiness is polled in init_lnd_node_async
        // Reduced from 10s to 1s (just enough for process startup)
        sleep(Duration::from_secs(1)).await;

        tracing::info!(
            "LND node {} spawned in {:.2}s",
            name,
            start.elapsed().as_secs_f64()
        );
        Ok(handle)
    }

    pub fn pid(&self) -> Option<u32> {
        self.process_handle.as_ref().and_then(|h| h.pid())
    }
}

impl Drop for Lnd {
    fn drop(&mut self) {
        tracing::info!("Dropping lnd");
        // ProcessHandle will handle cleanup automatically in its own Drop
    }
}
