//! CDK MINTD
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

// Ensure at least one lightning backend is enabled at compile time
#[cfg(not(any(
    feature = "cln",
    feature = "lnbits",
    feature = "lnd",
    feature = "fakewallet",
    feature = "grpc-processor"
)))]
compile_error!(
    "At least one lightning backend feature must be enabled: cln, lnbits, lnd, fakewallet, or grpc-processor"
);

use anyhow::Result;
use cdk_mintd::cli::CLIArgs;
use cdk_mintd::{get_work_directory, load_settings, setup_tracing};
use clap::Parser;
use tokio::main;

#[main]
async fn main() -> Result<()> {
    let args = CLIArgs::parse();

    if args.enable_logging {
        setup_tracing();
    }

    let work_dir = get_work_directory(&args).await?;

    let settings = load_settings(&work_dir, args.config)?;

    #[cfg(feature = "sqlcipher")]
    let password = Some(CLIArgs::parse().password);

    #[cfg(not(feature = "sqlcipher"))]
    let password = None;

    cdk_mintd::run_mintd(&work_dir, &settings, password).await
}
