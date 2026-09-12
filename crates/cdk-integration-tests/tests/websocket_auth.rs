//! Self-contained NUT-22 WebSocket authentication test.
//!
//! Stands up an in-process mint whose NUT-17 `/v1/ws` endpoint is protected by
//! blind auth, serves it over a real TCP socket, and verifies that a `Wallet`
//! connects over the WebSocket and authenticates in-band (the NUT-22
//! `authenticate` command) before its subscription is accepted. Blind auth is
//! fully offline, so no OIDC server is needed: the BAT is minted directly from
//! the mint and seeded into the wallet's store.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRequestParts, Request};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bip39::Mnemonic;
use cdk::amount::{Amount, SplitTarget};
use cdk::cdk_database::WalletDatabase;
use cdk::dhke::construct_proofs;
use cdk::error::{ErrorCode, ErrorResponse};
use cdk::mint::{Mint, MintBuilder, MintMeltLimits};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    AuthProof, AuthRequired, CurrencyUnit, MintQuoteState, NotificationPayload, PaymentMethod,
    PreMintSecrets, State,
};
use cdk::secret::Secret;
use cdk::types::FeeReserve;
use cdk::wallet::{WalletBuilder, WalletSubscription};
use cdk_common::wallet::ProofInfo;
use cdk_fake_wallet::FakeWallet;
use cdk_sqlite::wallet::memory;
use futures::{SinkExt, StreamExt};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::{Error as WsHandshakeError, Message};

/// Build an auth-enabled mint whose only protected endpoint is `/v1/ws`.
/// `/v1/auth/blind/mint` is left unprotected (no clear auth), so no OIDC/CAT is
/// involved anywhere in this test.
async fn build_ws_protected_mint(auth: AuthRequired) -> Mint {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let auth_db = Arc::new(
        cdk_sqlite::mint::MintSqliteAuthDatabase::new(":memory:")
            .await
            .expect("auth db"),
    );

    let mut mint_builder = MintBuilder::new(db.clone());

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };
    let ln_fake_backend = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        2,
        CurrencyUnit::Sat,
    );

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            Arc::new(ln_fake_backend),
        )
        .await
        .expect("payment processor");

    let ws_endpoint = ProtectedEndpoint::new(Method::Get, RoutePath::Ws);
    let clear_endpoints = match auth {
        AuthRequired::Clear => vec![ws_endpoint.clone()],
        AuthRequired::Blind => vec![],
    };

    mint_builder = mint_builder.with_auth(
        auth_db,
        "https://example.com/.well-known/openid-configuration".to_string(),
        "test-client".to_string(),
        clear_endpoints,
    );

    if auth == AuthRequired::Blind {
        mint_builder = mint_builder.with_blind_auth(50, vec![ws_endpoint]);
    }

    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mint = mint_builder
        .build_with_seed(db, &mnemonic.to_seed_normalized(""))
        .await
        .expect("mint");

    mint.start().await.expect("start mint");
    mint
}

/// Mint one valid blind auth proof (BAT) directly from the mint's auth keyset,
/// without going through the OIDC-gated HTTP mint endpoint.
async fn mint_bat(mint: &Mint) -> cdk::nuts::Proof {
    let auth_keyset_id = *mint
        .get_active_keysets()
        .get(&CurrencyUnit::Auth)
        .expect("auth keyset active");

    let keys = mint
        .auth_pubkeys()
        .expect("auth pubkeys")
        .keysets
        .into_iter()
        .next()
        .expect("one auth keyset")
        .keys;

    // The auth keyset supports only amount 1.
    let fee_and_amounts = (0u64, vec![1u64]).into();
    let premint = PreMintSecrets::random(
        auth_keyset_id,
        Amount::from(1),
        &SplitTarget::Value(1.into()),
        &fee_and_amounts,
    )
    .expect("premint secrets");

    let mut signatures = Vec::new();
    for message in premint.blinded_messages() {
        signatures.push(mint.auth_blind_sign(&message).await.expect("blind sign"));
    }

    construct_proofs(signatures, premint.rs(), premint.secrets(), &keys)
        .expect("construct proofs")
        .into_iter()
        .next()
        .expect("one auth proof")
}

/// Serve a router on an ephemeral local port and return its URL.
async fn serve_router(router: Router) -> MintUrl {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve mint");
    });
    MintUrl::from_str(&format!("http://{addr}")).expect("mint url")
}

async fn mint_router(mint: Arc<Mint>) -> Router {
    cdk_axum::create_mint_router(mint, vec!["bolt11".to_string()])
        .await
        .expect("mint router")
}

/// Serve a mint on an ephemeral local port and return its URL.
async fn serve(mint: Arc<Mint>) -> MintUrl {
    serve_router(mint_router(mint).await).await
}

/// Serve a mint that behaves like one predating in-band authentication: the
/// blind auth token has to be in the upgrade header or the upgrade is refused.
async fn serve_header_only_blind_auth(mint: Arc<Mint>) -> MintUrl {
    let router = mint_router(mint)
        .await
        .layer(middleware::from_fn(reject_header_less_ws));
    serve_router(router).await
}

/// Serve a mint that accepts the websocket upgrade and then never answers.
///
/// Models a hostile mint, or a MITM on a plaintext `ws://` connection, that
/// keeps the wallet's consumer blocked instead of refusing it outright.
async fn serve_silent_ws(mint: Arc<Mint>) -> MintUrl {
    let router = mint_router(mint)
        .await
        .layer(middleware::from_fn(swallow_ws));
    serve_router(router).await
}

async fn swallow_ws(request: Request, next: Next) -> Response {
    if !request.uri().path().ends_with("/v1/ws") {
        return next.run(request).await;
    }

    let (mut parts, _body) = request.into_parts();
    match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upgrade) => {
            upgrade.on_upgrade(|mut socket| async move { while socket.recv().await.is_some() {} })
        }
        Err(rejection) => rejection.into_response(),
    }
}

async fn reject_header_less_ws(request: Request, next: Next) -> Response {
    if request.uri().path().ends_with("/v1/ws") && !request.headers().contains_key("Blind-auth") {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            "blind auth header required",
        )
            .into_response();
    }
    next.run(request).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_connects_and_authenticates_in_band() {
    let mint = Arc::new(build_ws_protected_mint(AuthRequired::Blind).await);
    let mint_url = serve(mint.clone()).await;

    let db = Arc::new(memory::empty().await.expect("wallet db"));
    let wallet = WalletBuilder::new()
        .mint_url(mint_url.clone())
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("wallet");

    // Fetch mint info so the wallet learns `/v1/ws` needs blind auth and builds
    // its auth wallet (backed by the same localstore we seed below).
    wallet
        .fetch_mint_info()
        .await
        .expect("mint info")
        .expect("mint info present");

    // Seed one BAT into the wallet's store as an unspent Auth proof. The wallet
    // turns it into the `authenticate` token on its own. Keep an `AuthProof`
    // copy so we can later assert the mint actually spent it.
    let bat = mint_bat(&mint).await;
    let auth_proof: AuthProof = bat.clone().try_into().expect("auth proof");
    let bat_info = ProofInfo::new(bat, mint_url.clone(), State::Unspent, CurrencyUnit::Auth)
        .expect("proof info");
    db.update_proofs(vec![bat_info], vec![])
        .await
        .expect("seed bat");

    // Something to subscribe to. The mint quote endpoint is not protected.
    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(10.into()), None, None)
        .await
        .expect("mint quote");

    // Opening this subscription connects to the protected `/v1/ws`. If the
    // in-band authentication did not succeed, the mint would reject the
    // subscribe with error 31001 and no notification would arrive.
    let mut subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![quote
            .id
            .clone()]))
        .await
        .expect("subscribe over authenticated websocket");

    let msg = timeout(Duration::from_secs(15), subscription.recv())
        .await
        .expect("timed out waiting for a notification")
        .expect("subscription closed without a notification");

    match msg.into_inner() {
        NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote.to_string(), quote.id);
            assert_eq!(response.state, MintQuoteState::Unpaid);
        }
        other => panic!("unexpected notification: {other:?}"),
    }

    // The notification alone does not prove the websocket authenticated: the
    // wallet falls back to HTTP polling of the (unprotected) quote-status
    // endpoint if the WS stream fails. Prove the in-band `authenticate` actually
    // ran by checking the mint spent the BAT (polling never touches it). A
    // freshly spent proof is no longer spendable.
    let spendable = mint.check_blind_auth_proof_spendable(auth_proof).await;
    assert!(
        spendable.is_err(),
        "expected the BAT to be spent by in-band websocket auth, but it was still \
         spendable (was the subscription served over the HTTP poll fallback?): {spendable:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_falls_back_to_the_blind_auth_header() {
    let mint = Arc::new(build_ws_protected_mint(AuthRequired::Blind).await);
    let mint_url = serve_header_only_blind_auth(mint.clone()).await;

    let db = Arc::new(memory::empty().await.expect("wallet db"));
    let wallet = WalletBuilder::new()
        .mint_url(mint_url.clone())
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("wallet");

    wallet
        .fetch_mint_info()
        .await
        .expect("mint info")
        .expect("mint info present");

    let bat = mint_bat(&mint).await;
    let auth_proof: AuthProof = bat.clone().try_into().expect("auth proof");
    let bat_info = ProofInfo::new(bat, mint_url.clone(), State::Unspent, CurrencyUnit::Auth)
        .expect("proof info");
    db.update_proofs(vec![bat_info], vec![])
        .await
        .expect("seed bat");

    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(10.into()), None, None)
        .await
        .expect("mint quote");

    // The header-less upgrade is refused here, and a refused upgrade retires
    // the websocket for the life of the process. The wallet has to retry with
    // the header instead of degrading to HTTP polling.
    let mut subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![quote
            .id
            .clone()]))
        .await
        .expect("subscribe over the header-authenticated websocket");

    let msg = timeout(Duration::from_secs(15), subscription.recv())
        .await
        .expect("timed out waiting for a notification")
        .expect("subscription closed without a notification");

    match msg.into_inner() {
        NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote.to_string(), quote.id);
        }
        other => panic!("unexpected notification: {other:?}"),
    }

    // As above: only the websocket path spends the BAT, so a spent token proves
    // the notification did not come from the HTTP poll fallback.
    let spendable = mint.check_blind_auth_proof_spendable(auth_proof).await;
    assert!(
        spendable.is_err(),
        "expected the BAT to be spent by the header upgrade, but it was still \
         spendable (was the subscription served over the HTTP poll fallback?): {spendable:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_auth_websocket_rejects_a_header_less_upgrade() {
    let mint = Arc::new(build_ws_protected_mint(AuthRequired::Clear).await);
    let mint_url = serve(mint.clone()).await;

    let ws_url = mint_url
        .to_string()
        .replacen("http://", "ws://", 1)
        .trim_end_matches('/')
        .to_string()
        + "/v1/ws";

    // Clear auth has no in-band command, so an upgrade without a `Clear-auth`
    // header must be refused at the HTTP layer rather than left to idle until
    // the mint's authentication timeout.
    let err = connect_async(&ws_url)
        .await
        .expect_err("upgrade to a clear-auth protected endpoint must fail");

    match err {
        WsHandshakeError::Http(response) => {
            // NUT-00: the mint answers errors with 400 and a coded body.
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = response.body().as_ref().expect("error body");
            let error: ErrorResponse = serde_json::from_slice(body).expect("error response");
            assert_eq!(error.code, ErrorCode::ClearAuthRequired);
        }
        other => panic!("expected the upgrade to be refused, got: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_blind_auth_tokens_stop_draining_the_wallet() {
    // Mirrors `MAX_CONSECUTIVE_AUTH_FAILURES` in `cdk::wallet::subscription`.
    const MAX_AUTH_ATTEMPTS: usize = 3;
    const SEEDED_TOKENS: usize = 5;

    let mint = Arc::new(build_ws_protected_mint(AuthRequired::Blind).await);
    let mint_url = serve(mint.clone()).await;

    let db = Arc::new(memory::empty().await.expect("wallet db"));
    let wallet = WalletBuilder::new()
        .mint_url(mint_url.clone())
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("wallet");

    wallet
        .fetch_mint_info()
        .await
        .expect("mint info")
        .expect("mint info present");

    // Seed tokens the mint will reject: the secret no longer matches the
    // signature, which is what a stale token looks like to the mint.
    let mut invalid = Vec::new();
    for _ in 0..SEEDED_TOKENS {
        let mut bat = mint_bat(&mint).await;
        bat.secret = Secret::generate();
        invalid.push(
            ProofInfo::new(bat, mint_url.clone(), State::Unspent, CurrencyUnit::Auth)
                .expect("proof info"),
        );
    }
    db.update_proofs(invalid, vec![]).await.expect("seed bats");

    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(10.into()), None, None)
        .await
        .expect("mint quote");

    let _subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![quote
            .id
            .clone()]))
        .await
        .expect("subscribe");

    let remaining = || {
        let db = db.clone();
        let mint_url = mint_url.clone();
        async move {
            db.get_proofs(
                Some(mint_url),
                Some(CurrencyUnit::Auth),
                Some(vec![State::Unspent]),
                None,
            )
            .await
            .expect("auth proofs")
            .len()
        }
    };

    let expected = SEEDED_TOKENS - MAX_AUTH_ATTEMPTS;
    timeout(Duration::from_secs(60), async {
        while remaining().await > expected {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("wallet kept retrying past its blind auth failure limit");

    // Once the failures are terminal the consumer falls back to HTTP polling and
    // must not spend another token, however long it runs.
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(
        remaining().await,
        expected,
        "a terminal websocket authentication failure must stop spending tokens"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mint_that_never_answers_authenticate_does_not_pin_the_wallet() {
    // Mirrors `MAX_CONSECUTIVE_AUTH_FAILURES` in `cdk::wallet::subscription`.
    const MAX_AUTH_ATTEMPTS: usize = 3;
    const SEEDED_TOKENS: usize = 5;

    let mint = Arc::new(build_ws_protected_mint(AuthRequired::Blind).await);
    let mint_url = serve_silent_ws(mint.clone()).await;

    let db = Arc::new(memory::empty().await.expect("wallet db"));
    let wallet = WalletBuilder::new()
        .mint_url(mint_url.clone())
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("wallet");

    wallet
        .fetch_mint_info()
        .await
        .expect("mint info")
        .expect("mint info present");

    let mut bats = Vec::new();
    for _ in 0..SEEDED_TOKENS {
        let bat = mint_bat(&mint).await;
        bats.push(
            ProofInfo::new(bat, mint_url.clone(), State::Unspent, CurrencyUnit::Auth)
                .expect("proof info"),
        );
    }
    db.update_proofs(bats, vec![]).await.expect("seed bats");

    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(10.into()), None, None)
        .await
        .expect("mint quote");

    let _subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![quote
            .id
            .clone()]))
        .await
        .expect("subscribe");

    let remaining = || {
        let db = db.clone();
        let mint_url = mint_url.clone();
        async move {
            db.get_proofs(
                Some(mint_url),
                Some(CurrencyUnit::Auth),
                Some(vec![State::Unspent]),
                None,
            )
            .await
            .expect("auth proofs")
            .len()
        }
    };

    // Without a client-side timeout the very first authenticate blocks forever,
    // the consumer never retries, and exactly one token is spent.
    let expected = SEEDED_TOKENS - MAX_AUTH_ATTEMPTS;
    timeout(Duration::from_secs(180), async {
        while remaining().await > expected {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("wallet stayed blocked waiting for an authenticate response");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn too_many_failed_authenticates_close_the_connection() {
    // Mirrors `MAX_FAILED_AUTH_ATTEMPTS` in `cdk_axum::ws`.
    const MAX_FAILED_AUTH_ATTEMPTS: usize = 3;

    let mint = Arc::new(build_ws_protected_mint(AuthRequired::Blind).await);
    let mint_url = serve(mint.clone()).await;

    let ws_url = mint_url
        .to_string()
        .replacen("http://", "ws://", 1)
        .trim_end_matches('/')
        .to_string()
        + "/v1/ws";

    let (mut socket, _) = connect_async(&ws_url).await.expect("upgrade");

    // Each rejected token costs the mint a signature verification on a
    // connection nobody has paid for, so the mint must stop taking them well
    // before its authentication timeout expires.
    for id in 0..MAX_FAILED_AUTH_ATTEMPTS {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "authenticate",
            "params": {"token": "not-a-valid-bat"},
            "id": id,
        });
        socket
            .send(Message::Text(request.to_string().into()))
            .await
            .expect("send authenticate");

        let response = timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("mint answers each attempt")
            .expect("stream open")
            .expect("no transport error");
        let Message::Text(text) = response else {
            panic!("expected a text response, got: {response:?}");
        };
        let response: serde_json::Value = serde_json::from_str(&text).expect("json response");
        assert_eq!(
            response["error"]["code"],
            ErrorCode::BlindAuthFailed.to_code()
        );
    }

    // The mint closes on its own, well inside its 30 second auth timeout.
    let closed = timeout(Duration::from_secs(5), async {
        while let Some(message) = socket.next().await {
            match message {
                Ok(Message::Close(_)) | Err(_) => return,
                Ok(_) => continue,
            }
        }
    })
    .await;

    assert!(
        closed.is_ok(),
        "the mint must close a connection that keeps failing to authenticate"
    );
}
