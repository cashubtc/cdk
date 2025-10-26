use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use cdk_integration_tests::cli::{init_logging, CommonArgs};
use cdk_integration_tests::init_regtest::start_regtest_end;
use clap::Parser;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{oneshot, Notify};
use tokio::time::timeout;

#[derive(Parser)]
#[command(name = "start-regtest")]
#[command(about = "Start regtest environment", long_about = None)]
struct Args {
    #[command(flatten)]
    common: CommonArgs,

    /// Working directory path
    work_dir: String,

    /// Skip LDK node initialization (for interactive mode where mint will create its own LDK node)
    #[arg(long)]
    skip_ldk: bool,
}

fn signal_progress(work_dir: &Path) {
    let mut pipe = OpenOptions::new()
        .write(true)
        .open(work_dir.join("progress_pipe"))
        .expect("Failed to open pipe");

    pipe.write_all(b"checkpoint1\n")
        .expect("Failed to write to pipe");
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging based on CLI arguments
    init_logging(args.common.enable_logging, args.common.log_level);

    let temp_dir = PathBuf::from_str(&args.work_dir)?;

    let shutdown_regtest = Arc::new(Notify::new());
    let shutdown_clone = Arc::clone(&shutdown_regtest);
    let shutdown_clone_two = Arc::clone(&shutdown_regtest);

    let temp_dir_clone = temp_dir.clone();
    let skip_ldk = args.skip_ldk;

    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        if let Err(e) = start_regtest_end(&temp_dir_clone, tx, shutdown_clone, skip_ldk).await {
            tracing::error!("Error starting regtest: {:?}", e);
            panic!("Error starting regtest: {:?}", e);
        }
    });

    match timeout(Duration::from_secs(300), rx).await {
        Ok(_) => {
            tracing::info!("Regtest set up");
            signal_progress(&temp_dir);
        }
        Err(_) => {
            tracing::error!("regtest setup timed out after 5 minutes");
            anyhow::bail!("Could not set up regtest");
        }
    }

    let shutdown_future = async {
        // Wait for SIGTERM or SIGINT (Ctrl+C) signal
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM");
                println!("\nReceived SIGTERM, shutting down...");
            }
            _ = sigint.recv() => {
                tracing::info!("Received SIGINT/Ctrl+C");
                println!("\nReceived Ctrl+C, shutting down...");
            }
        }

        shutdown_clone_two.notify_waiters();
    };

    shutdown_future.await;

    Ok(())
}
