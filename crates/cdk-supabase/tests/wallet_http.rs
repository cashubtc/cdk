#![cfg(feature = "wallet")]
#![doc = "Deterministic HTTP behavior tests for the Supabase wallet backend."]

use std::time::{SystemTime, UNIX_EPOCH};

use bitcoin::base64::engine::general_purpose;
use bitcoin::base64::Engine as _;
use cdk_common::database::Error as DatabaseError;
use cdk_common::database::WalletDatabase;
use cdk_supabase::{Error, SupabaseWalletDatabase};
use mockito::Matcher;
use serde_json::json;
use url::Url;

fn extract_schema_versions(schema_sql: &str) -> Vec<u32> {
    const PREFIX: &str = "VALUES ('schema_version', '";

    schema_sql
        .lines()
        .filter_map(|line| {
            let start = line.find(PREFIX)? + PREFIX.len();
            let end = line[start..].find('\'')? + start;
            line[start..end].parse().ok()
        })
        .collect()
}

fn schema_info_query() -> Matcher {
    Matcher::AllOf(vec![
        Matcher::UrlEncoded("key".to_string(), "eq.schema_version".to_string()),
        Matcher::UrlEncoded("select".to_string(), "value".to_string()),
    ])
}

fn jwt_with_expiry(exp: u64) -> String {
    let header = general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let payload = general_purpose::URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#).as_bytes());

    format!("{header}.{payload}.signature")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_secs()
}

#[test]
fn supabase_wallet_database_implements_wallet_database() {
    fn assert_wallet_database<T: WalletDatabase<DatabaseError>>() {}

    assert_wallet_database::<SupabaseWalletDatabase>();
}

#[test]
fn schema_sql_tracks_required_schema_version() {
    let schema_sql = SupabaseWalletDatabase::get_schema_sql();
    let versions = extract_schema_versions(&schema_sql);

    assert!(
        !versions.is_empty(),
        "embedded schema should expose a schema version"
    );
    assert!(schema_sql.contains("CREATE TABLE IF NOT EXISTS schema_info"));
    assert!(schema_sql.contains("CREATE TABLE IF NOT EXISTS p2pk_signing_key"));
    assert_eq!(
        versions.last().copied(),
        Some(SupabaseWalletDatabase::REQUIRED_SCHEMA_VERSION)
    );
    assert_eq!(
        versions.iter().max().copied(),
        Some(SupabaseWalletDatabase::REQUIRED_SCHEMA_VERSION)
    );
}

#[tokio::test]
async fn schema_compatibility_uses_api_key_without_jwt() {
    let mut server = mockito::Server::new_async().await;
    let required_version = SupabaseWalletDatabase::REQUIRED_SCHEMA_VERSION.to_string();
    let mock = server
        .mock("GET", "/rest/v1/schema_info")
        .match_query(schema_info_query())
        .match_header("apikey", "anon-key")
        .match_header("authorization", "Bearer anon-key")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(r#"[{{"value":"{required_version}"}}]"#))
        .create_async()
        .await;

    let db = SupabaseWalletDatabase::new(
        Url::parse(&server.url()).expect("mock server URL should parse"),
        "anon-key".to_string(),
    )
    .await
    .expect("database should initialize");

    db.check_schema_compatibility()
        .await
        .expect("required schema version should pass");

    mock.assert_async().await;
}

#[tokio::test]
async fn schema_compatibility_reports_outdated_schema() {
    let mut server = mockito::Server::new_async().await;
    let found_version = SupabaseWalletDatabase::REQUIRED_SCHEMA_VERSION - 1;
    let mock = server
        .mock("GET", "/rest/v1/schema_info")
        .match_query(schema_info_query())
        .match_header("apikey", "anon-key")
        .match_header("authorization", "Bearer anon-key")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(r#"[{{"value":"{found_version}"}}]"#))
        .create_async()
        .await;

    let db = SupabaseWalletDatabase::new(
        Url::parse(&server.url()).expect("mock server URL should parse"),
        "anon-key".to_string(),
    )
    .await
    .expect("database should initialize");

    let error = db
        .check_schema_compatibility()
        .await
        .expect_err("outdated schema should fail");

    match error {
        Error::SchemaMismatch { required, found } => {
            assert_eq!(required, SupabaseWalletDatabase::REQUIRED_SCHEMA_VERSION);
            assert_eq!(found, found_version);
        }
        other => panic!("expected schema mismatch error, got {other:?}"),
    }

    mock.assert_async().await;
}

#[tokio::test]
async fn schema_compatibility_reports_missing_schema_info() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/rest/v1/schema_info")
        .match_query(schema_info_query())
        .match_header("apikey", "anon-key")
        .match_header("authorization", "Bearer anon-key")
        .with_status(404)
        .with_body(r#"{"message":"relation \"schema_info\" does not exist"}"#)
        .create_async()
        .await;

    let db = SupabaseWalletDatabase::new(
        Url::parse(&server.url()).expect("mock server URL should parse"),
        "anon-key".to_string(),
    )
    .await
    .expect("database should initialize");

    let error = db
        .check_schema_compatibility()
        .await
        .expect_err("missing schema_info should fail");

    match error {
        Error::SchemaNotInitialized => {}
        other => panic!("expected schema not initialized error, got {other:?}"),
    }

    mock.assert_async().await;
}

#[tokio::test]
async fn call_rpc_uses_jwt_token_and_serializes_json_body() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/rest/v1/rpc/update_proofs_atomic")
        .match_header("apikey", "anon-key")
        .match_header("authorization", "Bearer jwt-token")
        .match_header("content-type", "application/json")
        .match_body(Matcher::Json(json!({ "proofs": [] })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"updated":1}"#)
        .create_async()
        .await;

    let db = SupabaseWalletDatabase::new(
        Url::parse(&server.url()).expect("mock server URL should parse"),
        "anon-key".to_string(),
    )
    .await
    .expect("database should initialize");
    db.set_jwt_token(Some("jwt-token".to_string())).await;

    let response = db
        .call_rpc("update_proofs_atomic", r#"{"proofs":[]}"#)
        .await
        .expect("RPC request should succeed");

    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).expect("RPC response should be JSON"),
        json!({ "updated": 1 })
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn schema_compatibility_refreshes_expiring_supabase_tokens() {
    let mut server = mockito::Server::new_async().await;
    let refreshed_token = "fresh-access-token";
    let refreshed_auth_header = format!("Bearer {refreshed_token}");
    let required_version = SupabaseWalletDatabase::REQUIRED_SCHEMA_VERSION.to_string();

    let refresh_mock = server
        .mock("POST", "/auth/v1/token")
        .match_query(Matcher::UrlEncoded(
            "grant_type".to_string(),
            "refresh_token".to_string(),
        ))
        .match_header("apikey", "anon-key")
        .match_header("content-type", "application/json")
        .match_body(Matcher::Json(json!({ "refresh_token": "refresh-token" })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"access_token":"{refreshed_token}","refresh_token":"rotated-refresh-token","expires_in":3600}}"#
        ))
        .create_async()
        .await;

    let schema_mock = server
        .mock("GET", "/rest/v1/schema_info")
        .match_query(schema_info_query())
        .match_header("apikey", "anon-key")
        .match_header("authorization", refreshed_auth_header.as_str())
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(r#"[{{"value":"{required_version}"}}]"#))
        .create_async()
        .await;

    let db = SupabaseWalletDatabase::with_supabase_auth(
        Url::parse(&server.url()).expect("mock server URL should parse"),
        "anon-key".to_string(),
    )
    .await
    .expect("database should initialize");
    db.set_refresh_token(Some("refresh-token".to_string()))
        .await;
    db.set_jwt_token(Some(jwt_with_expiry(unix_now().saturating_sub(1))))
        .await;

    db.check_schema_compatibility()
        .await
        .expect("token refresh should make schema check succeed");

    assert_eq!(db.get_jwt_token().await.as_deref(), Some(refreshed_token));
    refresh_mock.assert_async().await;
    schema_mock.assert_async().await;
}
