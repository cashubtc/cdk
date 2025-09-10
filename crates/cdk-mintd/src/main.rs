//! CDK MINTD
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::sync::Arc;

use anyhow::Result;
use cdk_mintd::cli::CLIArgs;
use cdk_mintd::{get_work_directory, load_settings};
use clap::Parser;
use tokio::runtime::Runtime;

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

        // Create OHTTP gateway router if enabled
        let mut routers = vec![];

        if let Some(ohttp_config) = &settings.ohttp_gateway {
            if ohttp_config.enabled {
                match cdk_mintd::create_ohttp_gateway_router(&settings, &work_dir) {
                    Ok(router) => {
                        tracing::info!("OHTTP gateway enabled and router created");
                        routers.push(router);
                    }
                    Err(e) => {
                        tracing::error!("Failed to create OHTTP gateway router: {}", e);
                        return Err(e);
                    }
                }
            }
        }

        cdk_mintd::run_mintd(
            &work_dir,
            &settings,
            password,
            args.enable_logging,
            Some(rt_clone),
            routers,
        )
        .await
    })
}
