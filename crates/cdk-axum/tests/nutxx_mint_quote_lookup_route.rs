#![allow(clippy::unwrap_used)]

//! NUT-XX route: `POST /v1/mint/quote/pubkey`
//!
//! The lookup is method-agnostic, so it must be served by the main v1 router rather than the
//! per-method custom router, which is only mounted when custom methods are configured.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bip39::Mnemonic;
use cdk::mint::{Mint, MintBuilder, MintMeltLimits};
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nutxx::mint_quote_lookup_msg_to_sign;
use cdk::nuts::{CurrencyUnit, MintQuoteBolt11Request, PaymentMethod, SecretKey};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk::{Amount, MintQuoteRequest};
use cdk_fake_wallet::FakeWallet;
use tower::ServiceExt;

async fn test_mint() -> Arc<Mint> {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
    let mut builder = MintBuilder::new(db.clone());

    let backend = FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        },
        HashMap::default(),
        HashSet::default(),
        2,
        CurrencyUnit::Sat,
    );

    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            Arc::new(backend),
        )
        .await
        .unwrap();

    let mnemonic = Mnemonic::generate(12).unwrap();
    builder = builder
        .with_name("nutxx route test".to_string())
        .with_description("nutxx route test".to_string())
        .with_urls(vec!["https://test-mint".to_string()]);

    let mint = builder
        .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();
    mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
        .await
        .unwrap();
    Arc::new(mint)
}

async fn signed_request_body(mint: &Mint, owner: &SecretKey) -> String {
    let mint_pubkey = mint.mint_info().await.unwrap().pubkey.unwrap();
    let pubkey = owner.public_key();
    let msg = mint_quote_lookup_msg_to_sign(&mint_pubkey, &pubkey);
    let signature = owner.sign(&msg).unwrap();

    format!(
        r#"{{"pubkeys":["{}"],"pubkey_signatures":["{}"]}}"#,
        pubkey.to_hex(),
        signature
    )
}

fn lookup_request(body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/mint/quote/pubkey")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

/// The endpoint answers on a plain bolt11 mint with no custom payment methods configured,
/// and the body matches `PostMintQuotesByPubkeyResponse`: an object with a `quotes` array
/// whose elements are flat NUT-04 quote objects.
#[tokio::test]
async fn lookup_route_is_served_without_custom_methods() {
    let mint = test_mint().await;
    let owner = SecretKey::generate();

    mint.get_mint_quote(MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
        amount: Amount::new(100, CurrencyUnit::Sat).into(),
        unit: CurrencyUnit::Sat,
        description: None,
        pubkey: Some(owner.public_key()),
    }))
    .await
    .unwrap();

    let body = signed_request_body(&mint, &owner).await;
    let router = cdk_axum::create_mint_router(mint, vec![]).await.unwrap();

    let response = router.oneshot(lookup_request(body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let quotes = json
        .get("quotes")
        .expect("response is an object with a `quotes` field")
        .as_array()
        .expect("`quotes` is an array");
    assert_eq!(quotes.len(), 1);

    // Flat NUT-04 quote object, not an externally tagged `{"Bolt11": …}` envelope.
    assert!(quotes[0].get("Bolt11").is_none());
    assert!(quotes[0].get("quote").is_some());
    assert_eq!(quotes[0]["method"], "bolt11");
    assert_eq!(quotes[0]["pubkey"], owner.public_key().to_hex());
}

/// Configuring custom payment methods must not shadow or break the route.
#[tokio::test]
async fn lookup_route_is_served_with_custom_methods() {
    let mint = test_mint().await;
    let owner = SecretKey::generate();
    let body = signed_request_body(&mint, &owner).await;

    let router = cdk_axum::create_mint_router(mint, vec!["paypal".to_string()])
        .await
        .unwrap();

    let response = router.oneshot(lookup_request(body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// A request with no signatures is refused at the HTTP layer.
#[tokio::test]
async fn unsigned_request_is_refused() {
    let mint = test_mint().await;
    let victim = SecretKey::generate().public_key();

    let router = cdk_axum::create_mint_router(mint, vec![]).await.unwrap();
    let body = format!(
        r#"{{"pubkeys":["{}"],"pubkey_signatures":[]}}"#,
        victim.to_hex()
    );

    let response = router.oneshot(lookup_request(body)).await.unwrap();
    assert_ne!(response.status(), StatusCode::OK);
}
