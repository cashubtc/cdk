//! CLAnd

use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread::sleep,
    time::Duration,
};

use anyhow::{anyhow, bail, Result};

/// Clnd
pub struct Clnd {
    data_dir: PathBuf,
    bitcoin_data_dir: PathBuf,
    addr: PathBuf,
    child: Option<Child>,
    bitcoin_rpc_user: String,
    bitcoin_rpc_password: String,
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
            child: None,
            bitcoin_rpc_user,
            bitcoin_rpc_password,
        }
    }

    /// Start clnd
    pub fn start_clnd(&mut self) -> Result<()> {
        let mut cmd = Command::new("lightningd");
        cmd.arg(format!(
            "--bitcoin-datadir={}",
            self.bitcoin_data_dir.to_string_lossy()
        ));
        cmd.arg("--network=regtest");
        cmd.arg(format!(
            "--lightning-dir={}",
            self.data_dir.to_string_lossy()
        ));
        cmd.arg(format!("--bitcoin-rpcuser={}", self.bitcoin_rpc_user));
        cmd.arg(format!(
            "--bitcoin-rpcpassword={}",
            self.bitcoin_rpc_password
        ));

        cmd.arg(format!("--bind-addr={}", self.addr.to_string_lossy()));
        cmd.arg(format!(
            "--log-file={}",
            self.data_dir.join("debug.log").to_string_lossy()
        ));

        // Send output to dev null
        cmd.stdout(Stdio::null());

        let child = cmd.spawn()?;

        self.child = Some(child);

        // Let clnd start up
        sleep(Duration::from_secs(5));

        Ok(())
    }

    pub fn pid(&self) -> Result<u32> {
        let child = self.child.as_ref().ok_or(anyhow!("Unknow child"))?;

        Ok(child.id())
    }

    /// Stop clnd
    pub fn stop_clnd(&mut self) -> Result<()> {
        let child = self.child.take();

        match child {
            Some(mut child) => {
                child.kill()?;
            }
            None => bail!("No child to kill"),
        }

        Ok(())
    }
}

impl Drop for Clnd {
    fn drop(&mut self) {
        tracing::info!("Dropping clnd");
        if let Err(err) = self.stop_clnd() {
            tracing::error!("Could not stop clnd: {}", err);
        }
    }
}
