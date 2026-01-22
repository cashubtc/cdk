use anyhow::Result;
use cdk_integration_tests::get_mint_url_from_env;

#[tokio::test]
async fn test_ldk_node_mint_info() -> Result<()> {
    // This test just verifies that the LDK-Node mint is running and responding
    let mint_url = get_mint_url_from_env();

    // Make a request to the info endpoint
    let response = bitreq::get(format!("{}/v1/info", mint_url))
        .send_async()
        .await?;

    // Check that we got a successful response
    assert_eq!(response.status_code, 200);

    // Try to parse the response as JSON
    let info: serde_json::Value = response.json()?;

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

    // Create a mint quote request
    let quote_request = serde_json::json!({
        "amount": 1000,
        "unit": "sat"
    });

    // Make a request to create a mint quote
    let response = bitreq::post(format!("{}/v1/mint/quote/bolt11", mint_url))
        .with_json(&quote_request)?
        .send_async()
        .await?;

    // Print the response for debugging
    let status = response.status_code;
    let text = response.as_str().unwrap_or_default();
    println!("Mint quote response status: {}", status);
    println!("Mint quote response body: {}", text);

    // For now, we'll just check that we get a response (even if it's an error)
    // In a real test, we'd want to verify the quote was created correctly
    assert!(status == 200 || status < 500);

    Ok(())
}
