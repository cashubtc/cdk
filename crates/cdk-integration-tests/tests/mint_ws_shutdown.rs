//! WebSocket connections follow the mint's lifecycle: a stopping mint closes
//! them and waits for them, and a stopped mint refuses new ones.
//!
//! Runs the real `cdk-axum` router over an in-process mint, with a real
//! WebSocket client.

use std::sync::Arc;
use std::time::Duration;

use cdk::mint::Mint;
use cdk_integration_tests::mint_connector_test;
use futures::StreamExt;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::Message;

/// Serve the mint's router on an ephemeral port and return its WebSocket URL.
async fn serve(mint: Arc<Mint>) -> String {
    let router = cdk_axum::create_mint_router(mint, vec!["bolt11".to_string()])
        .await
        .expect("build mint router");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve mint router");
    });

    format!("ws://{addr}/v1/ws")
}

#[tokio::test]
async fn stopping_mint_closes_websockets_and_refuses_new_ones() {
    let mint = mint_connector_test::build_test_mint().await;
    let url = serve(Arc::clone(&mint)).await;

    let (mut socket, _) = connect_async(&url).await.expect("websocket connects");

    // The stop returns only after the connection's task has ended, so it also
    // shows that the mint waits for its connections.
    tokio::time::timeout(Duration::from_secs(10), mint.stop())
        .await
        .expect("stop must not be held up by an idle websocket")
        .expect("mint stops");

    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("the mint closes the websocket")
        .expect("a close frame, not an abrupt end")
        .expect("a valid frame");
    match message {
        Message::Close(Some(frame)) => assert_eq!(frame.code, CloseCode::Away),
        other => panic!("expected a going-away close frame, got {other:?}"),
    }

    assert!(
        connect_async(&url).await.is_err(),
        "a stopped mint must refuse new websocket connections"
    );

    mint.start().await.expect("mint restarts");
    let (_socket, _) = connect_async(&url)
        .await
        .expect("a restarted mint accepts websocket connections");

    mint.stop().await.expect("mint stops");
}

/// A single in-memory connection with bounded buffering in each direction.
struct SingleConnection(Option<tokio::io::DuplexStream>);

impl axum::serve::Listener for SingleConnection {
    type Io = tokio::io::DuplexStream;
    type Addr = ();

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.0.take() {
            Some(stream) => (stream, ()),
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn stop_interrupts_a_blocked_websocket_write() {
    use futures::SinkExt;
    use tokio::io::AsyncReadExt;

    let mint = mint_connector_test::build_test_mint().await;
    let router = cdk_axum::create_mint_router(mint.clone(), vec!["bolt11".to_string()])
        .await
        .unwrap();
    let (client, server) = tokio::io::duplex(64);
    let server = tokio::spawn(async move {
        axum::serve(SingleConnection(Some(server)), router)
            .await
            .unwrap();
    });
    let (mut socket, _) = tokio_tungstenite::client_async("ws://localhost/v1/ws", client)
        .await
        .unwrap();
    socket
        .send(Message::Ping(vec![0; 125].into()))
        .await
        .unwrap();
    // The server has started writing its response. The 64-byte buffer
    // cannot hold the rest of the frame, and the client stops reading here.
    socket.get_mut().read_u8().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), mint.stop())
        .await
        .expect("blocked write must not keep the connection alive")
        .unwrap();
    server.abort();
}
