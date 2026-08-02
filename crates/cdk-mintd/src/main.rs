//! CDK MINTD

use std::sync::Arc;

use anyhow::Result;
use cdk_mintd::cli::CLIArgs;
use cdk_mintd::{get_work_directory, load_settings_from_args};
use clap::Parser;
use tokio::runtime::Runtime;

fn main() -> Result<()> {
    let rt = Runtime::new()?;
    let args = CLIArgs::parse();
    let rt = Arc::new(rt);
    let rt_clone = Arc::clone(&rt);
    rt.block_on(async {
        let work_dir = get_work_directory(&args).await?;
        let settings = load_settings_from_args(&work_dir, &args)?;

        #[cfg(feature = "sqlcipher")]
        let password = Some(args.password.clone());

        #[cfg(not(feature = "sqlcipher"))]
        let password = None;

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
