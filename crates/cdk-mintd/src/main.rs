//! CDK MINTD

use std::sync::Arc;

use anyhow::Result;
use cdk_mintd::cli::CLIArgs;
use cdk_mintd::{get_work_directory, load_settings_from_args};
use clap::Parser;
use tokio::runtime::Runtime;

fn main() -> Result<()> {
    let rt = Arc::new(Runtime::new()?);

    let rt_clone = Arc::clone(&rt);

    rt.block_on(async {
        let args = CLIArgs::parse();
        let work_dir = get_work_directory(&args).await?;
        let settings = load_settings_from_args(&work_dir, &args)?;

        #[cfg(feature = "sqlcipher")]
        let password = Some(CLIArgs::parse().password);

        #[cfg(not(feature = "sqlcipher"))]
        let password = None;

        if let Some(cdk_mintd::cli::Subcommand::MigrateNutshell { nutshell_db }) = args.subcommand {
            let _guard = if args.enable_logging {
                Some(cdk_mintd::setup_tracing(&work_dir, &settings.info.logging)?)
            } else {
                None
            };
            cdk_mintd::migrate::run_migration(&work_dir, &settings, &nutshell_db, password).await?;
            return Ok(());
        }

        cdk_mintd::run_mintd(
            &work_dir,
            &settings,
            password,
            args.enable_logging,
            Some(rt_clone),
            vec![],
        )
        .await
    })
}
