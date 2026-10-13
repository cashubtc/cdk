//! Bitcoind

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use bitcoincore_rpc::{Auth, Client, RpcApi};

fn prefixed_path_arg(prefix: &str, path: &Path) -> OsString {
    let mut argument = OsString::from(prefix);
    argument.push(path);
    argument
}

/// Bitcoind
pub struct Bitcoind {
    rpc_user: String,
    rpc_password: String,
    addr: PathBuf,
    data_dir: PathBuf,
    child: Option<Child>,
    zmq_raw_block: String,
    zmq_raw_tx: String,
}

impl Bitcoind {
    /// Create new [`Bitcoind`]
    pub fn new(
        data_dir: PathBuf,
        addr: PathBuf,
        rpc_user: String,
        rpc_password: String,
        zmq_raw_block: String,
        zmq_raw_tx: String,
    ) -> Self {
        Self {
            rpc_user,
            rpc_password,
            addr,
            data_dir,
            child: None,
            zmq_raw_block,
            zmq_raw_tx,
        }
    }

    /// Start bitcoind
    pub fn start_bitcoind(&mut self) -> Result<()> {
        println!("Starting bitcoind");

        std::fs::create_dir_all(&self.data_dir)?;
        println!("created dir: {}", self.data_dir.display());

        let mut cmd = Command::new("bitcoind");

        cmd.arg("-regtest");
        // Bitcoin Core's test-only setting avoids syncing its disposable
        // fixture wallet for every generated block.
        cmd.arg("-unsafesqlitesync");
        cmd.arg(prefixed_path_arg("-datadir=", &self.data_dir));
        cmd.arg("-fallbackfee=0.00001");
        cmd.arg("-rpcallowip=0.0.0.0/0");
        cmd.arg(format!("-rpcuser={}", self.rpc_user));
        cmd.arg(format!("-rpcpassword={}", self.rpc_password));
        cmd.arg(format!("-zmqpubrawblock={}", self.zmq_raw_block));
        cmd.arg(format!("-zmqpubrawtx={}", self.zmq_raw_tx));
        cmd.arg("-deprecatedrpc=warnings");

        //        cmd.arg(format!("-bind={}", self.addr.to_string_lossy()));

        // Send output to dev null
        cmd.stdout(Stdio::null());

        let child = cmd.spawn().map_err(|e| {
            anyhow!(
                "Failed to spawn bitcoind process: {}. Is 'bitcoind' installed and in your PATH?",
                e
            )
        })?;

        self.child = Some(child);

        let client = Client::new(
            &format!("http://{}", self.addr.display()),
            Auth::UserPass(self.rpc_user.clone(), self.rpc_password.clone()),
        )?;
        let deadline = Instant::now() + Duration::from_secs(120);

        loop {
            match client.get_blockchain_info() {
                Ok(_) => return Ok(()),
                Err(error) => {
                    if let Some(child) = self.child.as_mut() {
                        if let Some(status) = child.try_wait()? {
                            bail!("bitcoind exited before RPC became ready: {status}");
                        }
                    }
                    if Instant::now() >= deadline {
                        bail!("bitcoind RPC did not become ready within 120 seconds: {error}");
                    }
                    sleep(Duration::from_millis(200));
                }
            }
        }
    }

    pub fn pid(&self) -> Result<u32> {
        let child = self.child.as_ref().ok_or(anyhow!("Unknown child"))?;

        Ok(child.id())
    }

    /// Stop bitcoind
    pub fn stop_bitcoind(&mut self) -> Result<()> {
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

impl Drop for Bitcoind {
    fn drop(&mut self) {
        tracing::info!("Dropping bitcoind");
        if let Err(err) = self.stop_bitcoind() {
            tracing::error!("Could not stop bitcoind: {}", err);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    use super::*;

    #[test]
    fn datadir_argument_preserves_non_utf8_bytes() {
        let path = PathBuf::from(OsString::from_vec(b"data-\xff".to_vec()));

        assert_eq!(
            prefixed_path_arg("-datadir=", &path).as_bytes(),
            b"-datadir=data-\xff"
        );
    }
}
