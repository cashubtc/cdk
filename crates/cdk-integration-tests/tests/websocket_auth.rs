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

use bip39::Mnemonic;
use cdk::amount::{Amount, SplitTarget};
use cdk::cdk_database::WalletDatabase;
use cdk::dhke::construct_proofs;
use cdk::mint::{Mint, MintBuilder, MintMeltLimits};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    AuthProof, CurrencyUnit, MintQuoteState, NotificationPayload, PaymentMethod, PreMintSecrets,
    State,
};
use cdk::types::FeeReserve;
use cdk::wallet::{WalletBuilder, WalletSubscription};
use cdk_common::wallet::ProofInfo;
use cdk_fake_wallet::FakeWallet;
use cdk_sqlite::wallet::memory;
use tokio::time::{timeout, Duration};

/// Build an auth-enabled mint whose only blind-auth-protected endpoint is
/// `/v1/ws`. `/v1/auth/blind/mint` is left unprotected (no clear auth), so no
/// OIDC/CAT is involved anywhere in this test.
async fn build_ws_protected_mint() -> Mint {
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

    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mint = mint_builder
        .with_auth(
            auth_db,
            "https://example.com/.well-known/openid-configuration".to_string(),
            "test-client".to_string(),
            vec![],
        )
        .with_blind_auth(50, vec![ProtectedEndpoint::new(Method::Get, RoutePath::Ws)])
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

/// Serve a mint on an ephemeral local port and return its URL.
async fn serve(mint: Arc<Mint>) -> MintUrl {
    let router = cdk_axum::create_mint_router(mint, vec!["bolt11".to_string()])
        .await
        .expect("mint router");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve mint");
    });
    MintUrl::from_str(&format!("http://{addr}")).expect("mint url")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_connects_and_authenticates_in_band() {
    let mint = Arc::new(build_ws_protected_mint().await);
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
