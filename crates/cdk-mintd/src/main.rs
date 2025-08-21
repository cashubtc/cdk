//! CDK MINTD
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::sync::Arc;

use anyhow::Result;
use cdk_mintd::cli::CLIArgs;
use cdk_mintd::{get_work_directory, load_settings};
use clap::Parser;
use tokio::runtime::Runtime;

// Ensure at least one lightning backend is enabled at compile time
#[cfg(not(any(
    feature = "cln",
    feature = "lnbits",
    feature = "lnd",
    feature = "ldk-node",
    feature = "fakewallet",
    feature = "grpc-processor"
)))]
compile_error!(
    "At least one lightning backend feature must be enabled: cln, lnbits, lnd, ldk-node, fakewallet, or grpc-processor"
);

fn main() -> Result<()> {
    let rt = Arc::new(Runtime::new()?);

    let rt_clone = Arc::clone(&rt);

    rt.block_on(async {
        let args = CLIArgs::parse();
        let work_dir = get_work_directory(&args).await?;
        let settings = load_settings(&work_dir, args.config)?;

        #[cfg(feature = "sqlcipher")]
        let password = Some(CLIArgs::parse().password);

        #[cfg(not(feature = "sqlcipher"))]
        let password = None;

        cdk_mintd::run_mintd(
            &work_dir,
            &settings,
            password,
            args.enable_logging,
            Some(rt_clone),
        )
        .await
    })
}
