//! `MintConnector` suite over a real `HttpClient` talking to a live in-process
//! `cdk-axum` server built on the same mint the direct backend uses. This is the
//! sanity check that the transport and the mint server speak the same language,
//! the same way `mint_db_test!` sanity-checks a database backend.

use std::sync::Arc;

use cdk::wallet::HttpClient;
use cdk_integration_tests::mint_connector_test;

/// Serve the real `cdk-axum` router over a fresh mint on an ephemeral port and
/// return an `HttpClient` pointed at it.
async fn make_http(_test_name: &str) -> HttpClient {
    let mint = Arc::new(mint_connector_test::build_test_mint().await);

    // The test mint registers bolt11 and a "paypal" custom method; pass both so
    // the per-method routers exist (the shared suite only hits core routes, but
    // matching the mint keeps the server realistic).
    let router =
        cdk_axum::create_mint_router(mint, vec!["bolt11".to_string(), "paypal".to_string()])
            .await
            .expect("build mint router");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");

    // Detached: the task owns the router (and the Arc<Mint>) for the lifetime of
    // the #[tokio::test] runtime, which tears it down when the test ends.
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve mint router");
    });

    HttpClient::new(format!("http://{addr}").parse().expect("mint url"), None)
}

mint_connector_test!(make_http);
