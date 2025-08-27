use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;

use crate::web::handlers::{
    balance_page, channels_page, close_channel_page, dashboard, force_close_channel_page,
    get_new_address, invoices_page, onchain_confirm_page, onchain_page, open_channel_page,
    payments_page, post_close_channel, post_create_bolt11, post_create_bolt12,
    post_force_close_channel, post_open_channel, post_pay_bolt11, post_pay_bolt12,
    post_send_onchain, send_payments_page, AppState,
};
use crate::web::static_files::static_handler;
use crate::CdkLdkNode;

pub struct WebServer {
    pub node: Arc<CdkLdkNode>,
}

impl WebServer {
    pub fn new(node: Arc<CdkLdkNode>) -> Self {
        Self { node }
    }

    pub fn create_router(&self) -> Router {
        let state = AppState {
            node: self.node.clone(),
        };

        tracing::debug!("Serving static files from embedded assets");

        Router::new()
            // Dashboard
            .route("/", get(dashboard))
            // Balance and onchain operations
            .route("/balance", get(balance_page))
            .route("/onchain", get(onchain_page))
            .route("/onchain/send", post(post_send_onchain))
            .route("/onchain/confirm", get(onchain_confirm_page))
            .route("/onchain/new-address", post(get_new_address))
            // Channel management
            .route("/channels", get(channels_page))
            .route("/channels/open", get(open_channel_page))
            .route("/channels/open", post(post_open_channel))
            .route("/channels/close", get(close_channel_page))
            .route("/channels/close", post(post_close_channel))
            .route("/channels/force-close", get(force_close_channel_page))
            .route("/channels/force-close", post(post_force_close_channel))
            // Invoice creation
            .route("/invoices", get(invoices_page))
            .route("/invoices/bolt11", post(post_create_bolt11))
            .route("/invoices/bolt12", post(post_create_bolt12))
            // Payment sending and history
            .route("/payments", get(payments_page))
            .route("/payments/send", get(send_payments_page))
            .route("/payments/bolt11", post(post_pay_bolt11))
            .route("/payments/bolt12", post(post_pay_bolt12))
            // Static files - now embedded
            .route("/static/{*file}", get(static_handler))
            .layer(ServiceBuilder::new().layer(CorsLayer::permissive()))
            .with_state(state)
    }

    pub async fn serve(&self, addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        let app = self.create_router();

        tracing::info!("Starting web server on {}", addr);
        let listener = tokio::net::TcpListener::bind(addr).await?;

        tracing::info!("Web interface available at: http://{}", addr);
        axum::serve(listener, app).await?;

        Ok(())
    }
}
