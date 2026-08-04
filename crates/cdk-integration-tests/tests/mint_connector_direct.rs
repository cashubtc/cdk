//! `MintConnector` suite over the in-process `DirectMintConnection` backend.

use cdk_integration_tests::direct_connection::DirectMintConnection;
use cdk_integration_tests::mint_connector_test;

async fn make_direct(_test_name: &str) -> DirectMintConnection {
    DirectMintConnection::new(mint_connector_test::build_test_mint().await)
}

mint_connector_test!(make_direct);
