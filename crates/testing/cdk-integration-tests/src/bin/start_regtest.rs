use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use cashu::Amount;
use cdk_integration_tests::cli::{init_logging, CommonArgs};
use cdk_integration_tests::init_regtest::start_regtest_end;
use cdk_ldk_node::CdkLdkNode;
use clap::Parser;
use ldk_node::lightning::ln::msgs::SocketAddress;
use tokio::signal;
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

    let ldk_work_dir = temp_dir.join("ldk_mint");
    let cdk_ldk = CdkLdkNode::new(
        bitcoin::Network::Regtest,
        cdk_ldk_node::ChainSource::BitcoinRpc(cdk_ldk_node::BitcoinRpcConfig {
            host: "127.0.0.1".to_string(),
            port: 18443,
            user: "testuser".to_string(),
            password: "testpass".to_string(),
        }),
        cdk_ldk_node::GossipSource::P2P,
        ldk_work_dir.to_string_lossy().to_string(),
        cdk_common::common::FeeReserve {
            min_fee_reserve: Amount::ZERO,
            percent_fee_reserve: 0.0,
        },
        vec![SocketAddress::TcpIpV4 {
            addr: [127, 0, 0, 1],
            port: 8092,
        }],
        None,
    )?;

    let inner_node = cdk_ldk.node();

    let temp_dir_clone = temp_dir.clone();

    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        start_regtest_end(&temp_dir_clone, tx, shutdown_clone, Some(inner_node))
            .await
            .expect("Error starting regtest");
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
        // Wait for Ctrl+C signal
        signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C handler");
        tracing::info!("Shutdown signal received");
        println!("\nReceived Ctrl+C, shutting down mints...");
        shutdown_clone_two.notify_waiters();
    };

    shutdown_future.await;

    Ok(())
}
