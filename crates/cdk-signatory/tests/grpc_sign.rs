//! gRPC round trip for the identity signing key.
//!
//! Exercises both sides of the `Sign` RPC, which also covers the schema-version
//! handshake between client and server.
#![cfg(all(feature = "grpc", feature = "sqlite", not(target_arch = "wasm32")))]

use std::sync::Arc;

use cdk_signatory::db_signatory::DbSignatory;
use cdk_signatory::identity;
use cdk_signatory::signatory::Signatory;
use cdk_signatory::{start_grpc_server_with_incoming, SignatoryRpcClient};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

#[tokio::test]
async fn sign_round_trips_over_grpc() {
    let store = Arc::new(
        cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory db"),
    );
    let signatory = Arc::new(
        DbSignatory::new(
            store,
            b"test-seed-for-grpc-signing",
            Default::default(),
            Default::default(),
        )
        .await
        .expect("DbSignatory::new"),
    );
    let expected = signatory.keysets().await.expect("keysets");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        if let Err(err) =
            start_grpc_server_with_incoming(signatory, TcpListenerStream::new(listener)).await
        {
            tracing::error!("signatory server stopped: {err}");
        }
    });

    let client = SignatoryRpcClient::new("127.0.0.1", addr.port(), None)
        .await
        .expect("connect to signatory");

    let payload = b"an arbitrary stream of bytes".to_vec();
    let signature = client.sign(payload.clone()).await.expect("sign over grpc");

    let keysets = client.keysets().await.expect("keysets over grpc");
    assert_eq!(
        keysets.pubkey, expected.pubkey,
        "the identity pubkey must survive the proto round trip"
    );

    identity::verify(&keysets.pubkey, &payload, &signature)
        .expect("signature from the remote signatory must verify");
    assert!(
        identity::verify(&keysets.pubkey, b"tampered", &signature).is_err(),
        "a tampered payload must not verify"
    );
}
