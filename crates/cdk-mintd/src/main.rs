//! CDK MINTD
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::sync::Arc;

use anyhow::Result;
use cdk_mintd::cli::CLIArgs;
use cdk_mintd::config::LoggingConfig;
use cdk_mintd::{get_work_directory, load_settings, setup_tracing};
use clap::Parser;
use tokio::runtime::Runtime;

fn main() -> Result<()> {
    let rt = Arc::new(Runtime::new()?);

    let rt_clone = Arc::clone(&rt);

    rt.block_on(async {
        let args = CLIArgs::parse();
        let work_dir = get_work_directory(&args).await?;
        let settings = load_settings(&work_dir, args.config)?;

        if args.enable_logging {
            setup_tracing(&work_dir, &LoggingConfig::default())?;
        }

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
