use anyhow::Result;
use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use ohttp_gateway::cli::Cli;
use ohttp_gateway::{gateway, key_config};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();

    let cli = Cli::parse();

    // Get work directory and construct OHTTP keys path
    let work_dir = cli.get_work_dir()?;
    let ohttp_keys_path = work_dir.join("ohttp_keys.json");

    // Load or generate OHTTP keys
    let ohttp = key_config::load_or_generate_keys(&ohttp_keys_path)?;

    // HTTP client is set up within the gateway handlers

    // Create the Axum app
    let app = Router::new()
        .route(
            "/.well-known/ohttp-gateway",
            post(gateway::handle_ohttp_request).get(gateway::handle_gateway_get),
        )
        .route("/ohttp-keys", get(gateway::handle_ohttp_keys))
        // Catch-all route to handle any path with OHTTP requests
        .fallback(gateway::handle_ohttp_request)
        .layer(axum::extract::Extension(ohttp))
        .layer(axum::extract::Extension(cli.backend_url.clone()));

    // Create TCP listener
    let addr = format!("0.0.0.0:{}", cli.port);

    tracing::info!("OHTTP Gateway listening on: {}", addr);
    tracing::info!("Forwarding requests to: {}", cli.backend_url);

    // Run the server
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn init_logging() {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::DEBUG.into())
        .from_env_lossy()
        .add_directive("hyper=info".parse().unwrap())
        .add_directive("tower_http=debug".parse().unwrap());

    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(env_filter)
        .init();

    tracing::info!("Logging initialized");
}
