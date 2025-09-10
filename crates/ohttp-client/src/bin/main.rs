use anyhow::Result;
use clap::Parser;
use ohttp_client::cli::Cli;
use ohttp_client::client::execute_command;
use tracing::info;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_logging()?;

    let cli = Cli::parse();

    info!("OHTTP Client v{}", env!("CARGO_PKG_VERSION"));

    // Execute the requested command
    if let Err(e) = execute_command(cli).await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }

    Ok(())
}

fn init_logging() -> Result<()> {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::DEBUG.into())
        .from_env()?;

    tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_env_filter(env_filter)
        .compact()
        .init();

    Ok(())
}
