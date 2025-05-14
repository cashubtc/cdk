use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use cdk_integration_tests::init_regtest::{get_temp_dir, start_regtest_end};
use tokio::signal;
use tokio::sync::{oneshot, Notify};
use tokio::time::timeout;
use tracing_subscriber::EnvFilter;

fn signal_progress() {
    let temp_dir = get_temp_dir();
    let mut pipe = OpenOptions::new()
        .write(true)
        .open(temp_dir.join("progress_pipe"))
        .expect("Failed to open pipe");

    pipe.write_all(b"checkpoint1\n")
        .expect("Failed to write to pipe");
}

#[tokio::main]
async fn main() -> Result<()> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";
    let h2_filter = "h2=warn";
    let rustls_filter = "rustls=warn";

    let env_filter = EnvFilter::new(format!(
        "{default_filter},{sqlx_filter},{hyper_filter},{h2_filter},{rustls_filter}"
    ));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let shutdown_regtest = Arc::new(Notify::new());
    let shutdown_clone = shutdown_regtest.clone();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        start_regtest_end(tx, shutdown_clone)
            .await
            .expect("Error starting regtest");
    });

    match timeout(Duration::from_secs(300), rx).await {
        Ok(_) => {
            tracing::info!("Regtest set up");
            signal_progress();
        }
        Err(_) => {
            tracing::error!("regtest setup timed out after 5 minutes");
            bail!("Could not set up regtest");
        }
    }

    signal::ctrl_c().await?;

    Ok(())
}
