use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use cdk::mint::Mint;
use tokio::sync::Notify;
use tower_http::cors::CorsLayer;

pub async fn start_mint(addr: &str, port: u16, mint: Mint) -> Result<()> {
    let mint_arc = Arc::new(mint);

    let v1_service = cdk_axum::create_mint_router(Arc::clone(&mint_arc))
        .await
        .unwrap();

    let mint_service = Router::new()
        .merge(v1_service)
        .layer(CorsLayer::permissive());

    let mint = Arc::clone(&mint_arc);

    let shutdown = Arc::new(Notify::new());

    tokio::spawn({
        let shutdown = Arc::clone(&shutdown);
        async move { mint.wait_for_paid_invoices(shutdown).await }
    });

    println!("Staring Axum server");
    axum::Server::bind(&format!("{}:{}", addr, port).as_str().parse().unwrap())
        .serve(mint_service.into_make_service())
        .await?;

    Ok(())
}
