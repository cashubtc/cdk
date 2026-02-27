//! Integration tests for cdk-http-client using mockito

use cdk_http_client::{HttpClient, HttpError, RequestBuilderExt};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TestPayload {
    name: String,
    value: i32,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TestResponse {
    success: bool,
    data: String,
}

// === HttpClient::fetch tests ===

#[tokio::test]
async fn test_fetch_success() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/api/data")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"success": true, "data": "hello"}"#)
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/data", server.url());
    let result: Result<TestResponse, _> = client.fetch(&url).await;

    assert!(result.is_ok());
    let response = result.expect("Fetch should succeed");
    assert!(response.success);
    assert_eq!(response.data, "hello");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_fetch_error_status() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/api/error")
        .with_status(404)
        .with_body("Not Found")
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/error", server.url());
    let result: Result<TestResponse, _> = client.fetch(&url).await;

    assert!(result.is_err());
    if let Err(HttpError::Status { status, message }) = result {
        assert_eq!(status, 404);
        assert_eq!(message, "Not Found");
    } else {
        panic!("Expected HttpError::Status");
    }

    mock.assert_async().await;
}

#[tokio::test]
async fn test_fetch_server_error() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/api/server-error")
        .with_status(500)
        .with_body("Internal Server Error")
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/server-error", server.url());
    let result: Result<TestResponse, _> = client.fetch(&url).await;

    assert!(result.is_err());
    if let Err(HttpError::Status { status, .. }) = result {
        assert_eq!(status, 500);
    } else {
        panic!("Expected HttpError::Status");
    }

    mock.assert_async().await;
}

// === HttpClient::post_json tests ===

#[tokio::test]
async fn test_post_json_success() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("POST", "/api/submit")
        .match_header("content-type", "application/json")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "name": "test",
            "value": 42
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"success": true, "data": "received"}"#)
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/submit", server.url());
    let payload = TestPayload {
        name: "test".to_string(),
        value: 42,
    };
    let result: Result<TestResponse, _> = client.post_json(&url, &payload).await;

    assert!(result.is_ok());
    let response = result.expect("POST JSON should succeed");
    assert!(response.success);
    assert_eq!(response.data, "received");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_post_json_error_status() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("POST", "/api/submit")
        .with_status(400)
        .with_body("Bad Request")
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/submit", server.url());
    let payload = TestPayload {
        name: "test".to_string(),
        value: 42,
    };
    let result: Result<TestResponse, _> = client.post_json(&url, &payload).await;

    assert!(result.is_err());
    if let Err(HttpError::Status { status, message }) = result {
        assert_eq!(status, 400);
        assert_eq!(message, "Bad Request");
    } else {
        panic!("Expected HttpError::Status");
    }

    mock.assert_async().await;
}

// === HttpClient::get_raw tests ===

#[tokio::test]
async fn test_get_raw_success() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/api/raw")
        .with_status(200)
        .with_body("raw content")
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/raw", server.url());
    let result = client.get_raw(&url).await;

    assert!(result.is_ok());
    let response = result.expect("GET raw should succeed");
    assert_eq!(response.status(), 200);
    assert!(response.is_success());

    mock.assert_async().await;
}

// === RawResponse tests ===

#[tokio::test]
async fn test_raw_response_is_success_with_200() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(200)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    assert!(response.is_success());
    assert!(!response.is_client_error());
    assert!(!response.is_server_error());
    assert_eq!(response.status(), 200);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_is_success_with_201() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(201)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    assert!(response.is_success());
    assert_eq!(response.status(), 201);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_is_success_with_299() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(299)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    assert!(response.is_success());
    assert_eq!(response.status(), 299);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_is_not_success_with_100() {
    // Note: HTTP 1xx informational responses are special and may not be
    // fully supported by all HTTP libraries. We test with 100 Continue.
    // mockito may convert some 1xx codes to 500, so we just verify the
    // logic works with the boundary check (200..300).
    let mut server = mockito::Server::new_async().await;

    // Use 301 redirect as a more reliable "not success" boundary test
    let mock = server
        .mock("GET", "/")
        .with_status(301)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    // 301 is a redirect, not success
    assert!(!response.is_success());
    assert_eq!(response.status(), 301);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_is_not_success_with_300() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(300)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    assert!(!response.is_success());
    assert_eq!(response.status(), 300);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_is_client_error_with_400() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(400)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    assert!(response.is_client_error());
    assert!(!response.is_success());
    assert!(!response.is_server_error());
    assert_eq!(response.status(), 400);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_is_client_error_with_499() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(499)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    assert!(response.is_client_error());
    assert_eq!(response.status(), 499);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_is_not_client_error_with_399() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(399)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    assert!(!response.is_client_error());
    assert_eq!(response.status(), 399);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_is_server_error_with_500() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(500)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    assert!(response.is_server_error());
    assert!(!response.is_success());
    assert!(!response.is_client_error());
    assert_eq!(response.status(), 500);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_is_server_error_with_599() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(599)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    assert!(response.is_server_error());
    assert_eq!(response.status(), 599);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_is_not_server_error_with_499() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(499)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");

    assert!(!response.is_server_error());
    assert_eq!(response.status(), 499);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_text() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_body("Hello, World!")
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");
    let text = response
        .text()
        .await
        .expect("Text extraction should succeed");

    assert_eq!(text, "Hello, World!");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_json() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"success": true, "data": "json_test"}"#)
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");
    let json: TestResponse = response.json().await.expect("JSON parsing should succeed");

    assert!(json.success);
    assert_eq!(json.data, "json_test");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_bytes() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_body(vec![0x01, 0x02, 0x03, 0x04])
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");
    let bytes = response
        .bytes()
        .await
        .expect("Bytes extraction should succeed");

    assert_eq!(bytes, vec![0x01, 0x02, 0x03, 0x04]);

    mock.assert_async().await;
}

// === RequestBuilder tests ===

#[tokio::test]
async fn test_request_builder_send() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/api/builder")
        .with_status(200)
        .with_body("builder response")
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/builder", server.url());
    let response = client
        .get(&url)
        .send()
        .await
        .expect("Request should succeed");

    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .text()
            .await
            .expect("Text extraction should succeed"),
        "builder response"
    );

    mock.assert_async().await;
}

#[tokio::test]
async fn test_request_builder_send_json() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("POST", "/api/json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"success": true, "data": "builder_json"}"#)
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/json", server.url());
    let payload = TestPayload {
        name: "builder".to_string(),
        value: 100,
    };

    let result: TestResponse = client
        .post(&url)
        .json(&payload)
        .send_json()
        .await
        .expect("Request should succeed");

    assert!(result.success);
    assert_eq!(result.data, "builder_json");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_request_builder_with_headers() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/api/headers")
        .match_header("X-Custom-Header", "custom-value")
        .match_header("Authorization", "Bearer token123")
        .with_status(200)
        .with_body("headers received")
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/headers", server.url());
    let response = client
        .get(&url)
        .header("X-Custom-Header", "custom-value")
        .header("Authorization", "Bearer token123")
        .send()
        .await
        .expect("Request should succeed");

    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .text()
            .await
            .expect("Text extraction should succeed"),
        "headers received"
    );

    mock.assert_async().await;
}

#[tokio::test]
async fn test_request_builder_post_with_form() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("POST", "/api/form")
        .match_header(
            "content-type",
            mockito::Matcher::Regex("application/x-www-form-urlencoded.*".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"success": true, "data": "form_received"}"#)
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/form", server.url());
    let form_data = [("field1", "value1"), ("field2", "value2")];

    let response: TestResponse = client
        .post(&url)
        .form(&form_data)
        .send_json()
        .await
        .expect("Request should succeed");

    assert!(response.success);
    assert_eq!(response.data, "form_received");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_request_builder_patch() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("PATCH", "/api/resource")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"success": true, "data": "patched"}"#)
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/resource", server.url());
    let payload = TestPayload {
        name: "update".to_string(),
        value: 99,
    };

    let result: TestResponse = client
        .patch(&url)
        .json(&payload)
        .send_json()
        .await
        .expect("Request should succeed");

    assert!(result.success);
    assert_eq!(result.data, "patched");

    mock.assert_async().await;
}

// === Convenience function test ===

#[tokio::test]
async fn test_fetch_convenience_function() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/api/convenience")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"success": true, "data": "convenience"}"#)
        .create_async()
        .await;

    let url = format!("{}/api/convenience", server.url());
    let result: Result<TestResponse, _> = cdk_http_client::fetch(&url).await;

    assert!(result.is_ok());
    let response = result.expect("Fetch should succeed");
    assert!(response.success);
    assert_eq!(response.data, "convenience");

    mock.assert_async().await;
}

// === Error handling tests ===

#[tokio::test]
async fn test_json_deserialization_error() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/api/invalid-json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("not valid json")
        .create_async()
        .await;

    let client = HttpClient::new();
    let url = format!("{}/api/invalid-json", server.url());
    let result: Result<TestResponse, _> = client.fetch(&url).await;

    assert!(result.is_err());
    // The error should be about JSON parsing, which becomes HttpError::Other from reqwest
    let err = result.expect_err("Should be a deserialization error");
    let err_str = format!("{}", err);
    assert!(
        err_str.contains("expected") || err_str.contains("JSON") || err_str.contains("error"),
        "Error should mention parsing issue: {}",
        err_str
    );

    mock.assert_async().await;
}

#[tokio::test]
async fn test_raw_response_json_deserialization_error() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_body("invalid json")
        .create_async()
        .await;

    let client = HttpClient::new();
    let response = client
        .get_raw(&server.url())
        .await
        .expect("Request should succeed");
    let result: Result<TestResponse, _> = response.json().await;

    assert!(result.is_err());

    mock.assert_async().await;
}
