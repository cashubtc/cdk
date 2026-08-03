use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::routing::{get, post};
use axum::{Json, Router};
use cdk_iroh::{IrohConfig, IrohNode, IrohServer, IrohTransport};
use serde_json::{json, Value};

use super::http_client::HttpClient;
use super::MintConnector;
use crate::mint_url::MintUrl;
use crate::nuts::SwapRequest;
use crate::wallet::subscription::websocket_url;

async fn mint_info() -> Json<Value> {
    Json(serde_json::to_value(crate::nuts::MintInfo::default()).expect("serialize mint info"))
}

async fn swap() -> Json<Value> {
    Json(json!({ "signatures": [] }))
}

async fn websocket(upgrade: WebSocketUpgrade) -> impl axum::response::IntoResponse {
    upgrade.on_upgrade(websocket_echo)
}

async fn websocket_echo(mut socket: WebSocket) {
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            Message::Text(_) | Message::Binary(_) => {
                if socket.send(message).await.is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }
}

#[tokio::test]
async fn wallet_connector_uses_generic_iroh_transport() {
    let router = Router::new()
        .route("/v1/info", get(mint_info))
        .route("/v1/swap", post(swap))
        .route("/v1/ws", get(websocket));
    let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let server_node = IrohNode::ephemeral(IrohConfig::static_only().with_bind_addr(bind_addr))
        .await
        .expect("create Iroh server endpoint");
    let client_node = IrohNode::client(
        IrohConfig::static_only()
            .with_bind_addr(bind_addr)
            .with_ticket(server_node.endpoint_ticket()),
    )
    .await
    .expect("create Iroh client endpoint");
    let server = IrohServer::start(server_node.clone(), router);
    let mint_url =
        MintUrl::from_str(&format!("iroh://{}", server_node.endpoint_id())).expect("Iroh mint URL");
    let transport =
        IrohTransport::with_node(client_node.clone(), cdk_http_client::Async::default());
    let client = HttpClient::with_transport(mint_url.clone(), transport.clone(), None);

    client.get_mint_info().await.expect("GET mint info");
    client
        .post_swap(SwapRequest::new(Vec::new(), Vec::new()))
        .await
        .expect("POST swap");

    let default_alias: super::HttpClient =
        super::HttpClient::with_transport(mint_url.clone(), transport, None);
    default_alias
        .get_mint_info()
        .await
        .expect("feature-selected connector uses Iroh");

    let ws_url = websocket_url(&mint_url).expect("supported NUT-17 URL");
    let (mut sender, mut receiver) = client
        .connect_websocket(ws_url.as_str(), &[])
        .await
        .expect("NUT-17 WebSocket");
    sender
        .send("subscription".to_string())
        .await
        .expect("send WebSocket frame");
    assert_eq!(
        receiver
            .recv()
            .await
            .expect("WebSocket response")
            .expect("valid WebSocket frame"),
        "subscription"
    );
    sender.close().await.expect("close WebSocket");

    client_node.close().await;
    server.shutdown().await.expect("shutdown Iroh server");
}

#[test]
fn nut17_scheme_mapping_preserves_iroh_and_rejects_unknown_schemes() {
    let https = MintUrl::from_str("https://fixture.invalid").expect("HTTPS URL");
    assert_eq!(websocket_url(&https).expect("HTTPS WS").scheme(), "wss");
    let http = MintUrl::from_str("http://fixture.invalid").expect("HTTP URL");
    assert_eq!(websocket_url(&http).expect("HTTP WS").scheme(), "ws");
    let iroh = MintUrl::from_str(&format!(
        "iroh://{}",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    ))
    .expect("Iroh URL");
    assert_eq!(websocket_url(&iroh).expect("Iroh WS").scheme(), "iroh");
    let ftp = MintUrl::from_str("ftp://fixture.invalid").expect("FTP URL");
    assert!(websocket_url(&ftp).is_err());
}
