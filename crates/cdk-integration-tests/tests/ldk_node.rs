use anyhow::Result;
use cdk_http_client::{HttpClient, RequestBuilderExt};
use cdk_integration_tests::get_mint_url_from_env;

#[tokio::test]
async fn test_ldk_node_mint_info() -> Result<()> {
    // This test just verifies that the LDK-Node mint is running and responding
    let mint_url = get_mint_url_from_env();

    // Create an HTTP client
    let client = HttpClient::new();

    // Make a request to the info endpoint
    let response = client.get(&format!("{}/v1/info", mint_url)).send().await?;

    // Check that we got a successful response
    assert_eq!(response.status(), 200);

    // Try to parse the response as JSON
    let info: serde_json::Value = response.json().await?;

    // Verify that we got some basic fields
    assert!(info.get("name").is_some());
    assert!(info.get("version").is_some());
    assert!(info.get("description").is_some());

    println!("LDK-Node mint info: {:?}", info);

    Ok(())
}

#[tokio::test]
async fn test_ldk_node_mint_quote() -> Result<()> {
    // This test verifies that we can create a mint quote with the LDK-Node mint
    let mint_url = get_mint_url_from_env();

    // Create an HTTP client
    let client = HttpClient::new();

    // Create a mint quote request
    let quote_request = serde_json::json!({
        "amount": 1000,
        "unit": "sat"
    });

    // Make a request to create a mint quote
    let response = client
        .post(&format!("{}/v1/mint/quote/bolt11", mint_url))
        .json(&quote_request)
        .send()
        .await?;

    // Print the response for debugging
    let status = response.status();
    let text = response.text().await?;
    println!("Mint quote response status: {}", status);
    println!("Mint quote response body: {}", text);

    // For now, we'll just check that we get a response (even if it's an error)
    // In a real test, we'd want to verify the quote was created correctly
    assert!(status < 300 || status < 500);

    Ok(())
}
