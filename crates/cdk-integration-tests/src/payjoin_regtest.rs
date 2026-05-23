//! Local Payjoin v2 services for onchain regtest.

use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// Running local Payjoin services for regtest.
pub struct PayjoinRegtestServices {
    /// Payjoin directory URL.
    pub directory_url: String,
    /// OHTTP relay URL.
    pub ohttp_relay_url: String,
    /// DER-encoded directory TLS certificate.
    pub cert_der: Vec<u8>,
    /// Path to the DER-encoded directory TLS certificate.
    pub cert_path: String,
    redis: Child,
    directory_handle:
        tokio::task::JoinHandle<std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>>,
    relay_handle:
        tokio::task::JoinHandle<std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>>,
}

impl PayjoinRegtestServices {
    /// Start Redis, a Payjoin Directory, and an OHTTP relay on free localhost ports.
    pub async fn start(work_dir: &Path) -> Result<Self> {
        let payjoin_dir = work_dir.join("payjoin");
        std::fs::create_dir_all(&payjoin_dir)?;

        let cert = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "0.0.0.0".to_string(),
        ])?;
        let cert_der = cert.serialize_der()?;
        let key_der = cert.serialize_private_key_der();
        let cert_path = payjoin_dir.join("directory-cert.der");
        std::fs::write(&cert_path, &cert_der)?;

        let redis_port = free_port()?;
        let mut redis = start_redis(redis_port, &payjoin_dir)?;
        wait_for_tcp("127.0.0.1", redis_port, &mut redis).await?;

        let ohttp_config = payjoin_directory::gen_ohttp_server_config()?;
        let (directory_port, directory_handle) =
            payjoin_directory::listen_tcp_with_tls_on_free_port(
                format!("127.0.0.1:{redis_port}"),
                Duration::from_secs(payjoin_directory::DEFAULT_TIMEOUT_SECS),
                (cert_der.clone(), key_der),
                ohttp_config.into(),
            )
            .await
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        let directory_url = format!("https://localhost:{directory_port}");

        let mut root_store = rustls::RootCertStore::empty();
        root_store
            .add(rustls::pki_types::CertificateDer::from(cert_der.clone()))
            .context("add local Payjoin directory cert to relay root store")?;
        let gateway = ohttp_relay::GatewayUri::from_str(&directory_url)
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        let (relay_port, relay_handle) = ohttp_relay::listen_tcp_on_free_port(gateway, root_store)
            .await
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        let ohttp_relay_url = format!("http://127.0.0.1:{relay_port}");

        Ok(Self {
            directory_url,
            ohttp_relay_url,
            cert_der,
            cert_path: cert_path.to_string_lossy().to_string(),
            redis,
            directory_handle,
            relay_handle,
        })
    }
}

impl Drop for PayjoinRegtestServices {
    fn drop(&mut self) {
        self.directory_handle.abort();
        self.relay_handle.abort();
        let _ = self.redis.kill();
    }
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

fn start_redis(port: u16, work_dir: &Path) -> Result<Child> {
    Command::new("redis-server")
        .arg("--bind")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--save")
        .arg("")
        .arg("--appendonly")
        .arg("no")
        .arg("--dir")
        .arg(work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("start redis-server for Payjoin regtest")
}

async fn wait_for_tcp(host: &str, port: u16, child: &mut Child) -> Result<()> {
    let addr = format!("{host}:{port}");
    for _ in 0..50 {
        if TcpStream::connect(&addr).is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            bail!("redis-server exited before becoming ready: {status}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("timed out waiting for redis-server on {addr}")
}
