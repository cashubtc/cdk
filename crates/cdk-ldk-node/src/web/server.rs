use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::{get, post};
use axum::{middleware, Router};

use crate::web::csrf::ensure_csrf_token;
use crate::web::handlers::{
    balance_page, channels_page, close_channel_page, dashboard, force_close_channel_page,
    get_new_address, invoices_page, onchain_confirm_page, onchain_page, open_channel_page,
    payments_page, post_close_channel, post_confirm_onchain, post_create_bolt11,
    post_create_bolt12, post_force_close_channel, post_open_channel, post_open_private_channel,
    post_pay_bolt11, post_pay_bolt12, post_send_onchain, send_payments_page, AppState,
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
            .route(
                "/onchain/confirm",
                get(onchain_confirm_page).post(post_confirm_onchain),
            )
            .route("/onchain/new-address", post(get_new_address))
            // Channel management
            .route("/channels", get(channels_page))
            .route("/channels/open", get(open_channel_page))
            .route("/channels/open", post(post_open_channel))
            .route("/channels/open/private", post(post_open_private_channel))
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
            .layer(middleware::from_fn(ensure_csrf_token))
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use axum::Router;
    use cdk_common::common::FeeReserve;
    use ldk_node::bitcoin::Network;
    use tower::ServiceExt;

    use super::WebServer;
    use crate::{CdkLdkNodeBuilder, ChainSource, GossipSource};

    const NODE_ID: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    async fn test_router() -> (tempfile::TempDir, Router) {
        let storage = tempfile::tempdir().unwrap();
        let mut node = CdkLdkNodeBuilder::new(
            Network::Regtest,
            ChainSource::Esplora("http://127.0.0.1:1".to_owned()),
            GossipSource::P2P,
            storage.path().to_str().unwrap().to_owned(),
            FeeReserve {
                min_fee_reserve: 0.into(),
                percent_fee_reserve: 0.0,
            },
            vec!["127.0.0.1:0".parse().unwrap()],
            Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap()),
        )
        .build()
        .unwrap();
        // A separate, never-started node without announcement configuration
        // distinguishes private NotRunning from public ChannelCreationFailed.
        let mut builder = ldk_node::Builder::new();
        builder.set_network(Network::Regtest);
        builder.set_storage_dir_path(
            storage
                .path()
                .join("private-dispatch")
                .display()
                .to_string(),
        );
        node.inner = Arc::new(builder.build().unwrap());
        (storage, WebServer::new(Arc::new(node)).create_router())
    }

    async fn csrf_session(router: &Router) -> (String, String) {
        let response = router
            .clone()
            .oneshot(Request::get("/channels/open").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let token = cookie.split_once('=').unwrap().1.to_string();
        let html = String::from_utf8(
            to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("action=\"/channels/open\""));
        assert!(html.contains(&format!("name=\"_csrf\" value=\"{token}\"")));
        assert!(!html.contains("/channels/open/private"));
        (cookie, token)
    }

    fn fields(token: &str) -> Vec<(&str, String)> {
        vec![
            ("_csrf", token.to_string()),
            ("node_id", NODE_ID.to_string()),
            ("address", "127.0.0.1".to_string()),
            ("port", "9735".to_string()),
            ("amount_sats", "500000".to_string()),
        ]
    }

    async fn post_form(router: &Router, cookie: &str, fields: &[(&str, String)]) -> Response {
        router
            .clone()
            .oneshot(
                Request::post("/channels/open/private")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", cookie)
                    .body(Body::from(serde_urlencoded::to_string(fields).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn json_body(response: Response) -> serde_json::Value {
        assert_eq!(response.headers()["content-type"], "application/json");
        serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn private_channel_endpoint_requires_csrf() {
        let (_storage, router) = test_router().await;
        let response = router
            .oneshot(
                Request::post("/channels/open/private")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(json_body(response).await["error"], "csrf_failed");
    }

    #[tokio::test]
    async fn private_channel_endpoint_rejects_missing_or_mismatched_csrf() {
        let (_storage, router) = test_router().await;
        let (cookie, token) = csrf_session(&router).await;
        let mut missing = fields(&token);
        missing.remove(0);
        for fields in [missing, fields(&"0".repeat(64))] {
            let response = post_form(&router, &cookie, &fields).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = json_body(response).await;
            assert_eq!(body["error"], "csrf_failed");
            assert_eq!(body["outcome"], "not_started");
        }
    }

    #[tokio::test]
    async fn private_channel_endpoint_validates_before_dispatch() {
        let (_storage, router) = test_router().await;
        let (cookie, token) = csrf_session(&router).await;
        for (name, value) in [
            ("node_id", "invalid"),
            ("address", "[::1"),
            ("port", "0"),
            ("port", "65536"),
            ("amount_sats", "0"),
            ("amount_sats", "-1"),
            ("amount_sats", "18446744073709551616"),
            ("amount_sats", "0.5"),
        ] {
            let mut input = fields(&token);
            input.iter_mut().find(|(key, _)| *key == name).unwrap().1 = value.to_string();
            let response = post_form(&router, &cookie, &input).await;
            // Calling the stopped LDK node would return 500 instead.
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}={value}");
            let body = json_body(response).await;
            assert_eq!(body["error"], "invalid_request");
            assert_eq!(body["outcome"], "not_started");
        }
    }

    #[tokio::test]
    async fn private_channel_endpoint_rejects_unknown_and_duplicate_fields() {
        let (_storage, router) = test_router().await;
        let (cookie, token) = csrf_session(&router).await;
        for (name, value) in [
            ("visibility", "public"),
            ("announce_channel", "true"),
            ("push_btc", "100"),
            ("push_amount_sats", "100"),
            ("amount_sats", "500000"),
            ("node_id", NODE_ID),
            ("_csrf", token.as_str()),
        ] {
            let mut input = fields(&token);
            input.push((name, value.to_string()));
            let response = post_form(&router, &cookie, &input).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}");
            assert_eq!(json_body(response).await["error"], "invalid_request");
        }
    }

    #[tokio::test]
    async fn private_channel_endpoint_uses_the_private_ldk_api() {
        let (_storage, router) = test_router().await;
        let (cookie, token) = csrf_session(&router).await;
        let response = post_form(&router, &cookie, &fields(&token)).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = json_body(response).await;
        assert_eq!(body["error"], "channel_open_failed");
        assert_eq!(body["message"], ldk_node::NodeError::NotRunning.to_string());
        assert_eq!(body["outcome"], "unknown");
    }

    #[test]
    fn production_router_does_not_use_permissive_cors() {
        let server_source = include_str!("server.rs");

        assert!(!server_source.contains(concat!("CorsLayer", "::permissive")));
    }
}
