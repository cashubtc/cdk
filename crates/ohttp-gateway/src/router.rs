use std::path::Path;

use anyhow::{anyhow, Result};
use axum::routing::post;
use axum::Router;
use url::Url;

use crate::{gateway, key_config};

/// Creates an OHTTP gateway router that forwards encapsulated requests to the specified backend
pub fn create_ohttp_gateway_router<P: AsRef<Path>>(
    backend_url: &str,
    ohttp_keys_path: P,
) -> Result<Router> {
    // Parse and validate the backend URL
    let backend_url = Url::parse(backend_url)
        .map_err(|e| anyhow!("Failed to parse backend URL '{}': {}", backend_url, e))?;

    tracing::info!("Creating OHTTP gateway router");
    tracing::info!("Backend URL: {}", backend_url);
    tracing::info!("OHTTP keys file: {:?}", ohttp_keys_path.as_ref());

    // Load or generate OHTTP keys
    let ohttp = key_config::load_or_generate_keys(&ohttp_keys_path)?;

    // Create the router with OHTTP gateway endpoints
    let router = Router::new()
        .route(
            "/.well-known/ohttp-gateway",
            post(gateway::handle_ohttp_request).get(gateway::handle_gateway_get),
        )
        .layer(axum::extract::Extension(ohttp))
        .layer(axum::extract::Extension(backend_url));

    tracing::info!("OHTTP gateway router created successfully");

    Ok(router)
}
