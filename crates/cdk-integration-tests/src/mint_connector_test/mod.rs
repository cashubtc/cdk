//! Behavioral test suite for [`MintConnector`] implementations.
//!
//! Mirrors the `mint_db_test!` pattern: [`mint_connector_test!`] generates one
//! `#[tokio::test]` per body, each fed a connector by a backend-specific
//! factory. Every backend drives a real mint, so the bodies assert the full
//! connector contract, including the mint -> swap -> melt write path. Run over
//! the direct in-process connector and over a real `HttpClient` against a live
//! `cdk-axum` server, the same body proves the transport and the mint speak the
//! same language.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk::amount::{FeeAndAmounts, SplitTarget};
use cdk::dhke::construct_proofs;
use cdk::mint::QuoteId;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::nut17::Kind;
use cdk::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, BlindedMessage, CheckStateRequest, CurrencyUnit,
    Id, Keys, MeltQuoteBolt11Request, MeltQuoteState, MeltRequest, MintQuoteBolt11Request,
    MintQuoteState, MintRequest, PaymentMethod, PreMintSecrets, Proofs, PublicKey, RestoreRequest,
    SecretKey, SwapRequest,
};
use cdk::subscription::{Params, SubId};
use cdk::wallet::MintConnector;
use cdk::ws::{WsMethodRequest, WsRequest};
use cdk::{Amount, Mint};
use cdk_common::{MeltQuoteCreateResponse, MeltQuoteRequest, MintQuoteRequest, MintQuoteResponse};
use cdk_fake_wallet::create_fake_invoice;

use crate::init_pure_tests::create_and_start_in_memory_test_mint;

/// Build a fresh in-memory test mint for a connector backend.
pub async fn build_test_mint() -> Mint {
    create_and_start_in_memory_test_mint()
        .await
        .expect("failed to build test mint")
}

fn random_pubkey() -> PublicKey {
    SecretKey::generate().public_key()
}

/// [NUT-01] The mint exposes at least one active keyset's keys.
pub async fn keys_roundtrip<C: MintConnector>(conn: C) {
    let keysets = conn.get_mint_keys().await.expect("get_mint_keys");
    assert!(!keysets.is_empty(), "mint must expose at least one keyset");
}

/// [NUT-02] The mint advertises at least one keyset.
pub async fn keysets_roundtrip<C: MintConnector>(conn: C) {
    let response = conn.get_mint_keysets().await.expect("get_mint_keysets");
    assert!(!response.keysets.is_empty(), "mint must advertise a keyset");
}

/// [NUT-01] A keyset advertised by the mint can be fetched by its id.
pub async fn keyset_by_id<C: MintConnector>(conn: C) {
    let response = conn.get_mint_keysets().await.expect("get_mint_keysets");
    let id = response.keysets.first().expect("a keyset").id;
    let keyset = conn.get_mint_keyset(id).await.expect("get_mint_keyset");
    assert_eq!(keyset.id, id, "returned keyset must match requested id");
}

/// [NUT-01] Fetching a well-formed but unknown keyset id must error, not return
/// an empty keyset. Exercises the error-response round-trip over each transport.
pub async fn unknown_keyset_rejected<C: MintConnector>(conn: C) {
    // Valid id format the mint does not have (version 0x00 + arbitrary bytes).
    let unknown = Id::from_str("00deadbeef123456").expect("valid id format");
    let result = conn.get_mint_keyset(unknown).await;
    assert!(
        result.is_err(),
        "an unknown keyset id must be rejected, got {result:?}"
    );
}

/// [NUT-06] The mint returns its info.
pub async fn mint_info<C: MintConnector>(conn: C) {
    let info = conn.get_mint_info().await.expect("get_mint_info");
    assert!(info.name.is_some(), "test mint advertises a name");
}

/// [NUT-07] Checking state returns one entry per requested Y.
pub async fn check_state<C: MintConnector>(conn: C) {
    let ys = vec![random_pubkey(), random_pubkey()];
    let response = conn
        .post_check_state(CheckStateRequest { ys: ys.clone() })
        .await
        .expect("post_check_state");
    assert_eq!(response.states.len(), ys.len(), "one state per requested Y");
}

/// [NUT-13] Restoring an output the mint never signed yields no signatures.
pub async fn restore<C: MintConnector>(conn: C) {
    let keyset_id = conn
        .get_mint_keys()
        .await
        .expect("mint keys")
        .first()
        .expect("an active keyset")
        .id;
    let unissued = BlindedMessage::new(Amount::from(1), keyset_id, random_pubkey());
    let response = conn
        .post_restore(RestoreRequest {
            outputs: vec![unissued],
        })
        .await
        .expect("post_restore");
    assert!(
        response.outputs.is_empty() && response.signatures.is_empty(),
        "an unissued output must not restore a signature"
    );
}

/// The mint's active keyset, its id and keys, fetched in a single call.
/// `get_mint_keys` returns full keysets, so no separate `get_mint_keyset` is
/// needed (that endpoint is covered by `keyset_by_id`).
async fn active_keyset<C: MintConnector>(conn: &C) -> (Id, Keys) {
    let keyset = conn
        .get_mint_keys()
        .await
        .expect("mint keys")
        .into_iter()
        .next()
        .expect("an active keyset");
    (keyset.id, keyset.keys)
}

/// Zero-fee split target: allow every power-of-two denomination.
fn fee_and_amounts() -> FeeAndAmounts {
    (0u64, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into()
}

/// Request a bolt11 mint quote for `amount` and wait for the FakeWallet to
/// settle it, returning the settled quote id.
async fn settled_mint_quote<C: MintConnector>(conn: &C, amount: Amount) -> String {
    let quote = conn
        .post_mint_quote(MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount,
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }))
        .await
        .expect("mint quote");
    let quote_id = match quote {
        MintQuoteResponse::Bolt11(r) => r.quote,
        _ => panic!("expected a bolt11 quote"),
    };

    // FakeWallet settles the invoice after a short delay. The budget is
    // deliberately generous (~15s): every write-path body polls here on a
    // single-threaded runtime the HTTP backend also serves axum on, so a loaded
    // CI runner can starve the settle well past a tight window.
    let mut paid = false;
    for _ in 0..150 {
        if let MintQuoteResponse::Bolt11(r) = conn
            .get_mint_quote_status(PaymentMethod::BOLT11, &quote_id)
            .await
            .expect("quote status")
        {
            if r.state == MintQuoteState::Paid {
                paid = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(paid, "fake wallet should settle the mint quote");
    quote_id
}

/// Mint `amount` sats of proofs through the connector: request and settle a
/// bolt11 quote, `post_mint`, then unblind the returned signatures into
/// spendable [`Proofs`]. Returns the proofs along with the active keyset id and
/// keys used, so callers can swap or melt without re-fetching.
async fn mint_proofs<C: MintConnector>(conn: &C, amount: Amount) -> (Proofs, Id, Keys) {
    let (keyset_id, keys) = active_keyset(conn).await;
    let quote_id = settled_mint_quote(conn, amount).await;

    let premint = PreMintSecrets::random(
        keyset_id,
        amount,
        &SplitTarget::default(),
        &fee_and_amounts(),
    )
    .expect("premint secrets");

    let response = conn
        .post_mint(
            &PaymentMethod::BOLT11,
            MintRequest {
                quote: quote_id,
                outputs: premint.blinded_messages(),
                signature: None,
            },
        )
        .await
        .expect("mint");

    let proofs = construct_proofs(response.signatures, premint.rs(), premint.secrets(), &keys)
        .expect("construct proofs");
    (proofs, keyset_id, keys)
}

/// [NUT-04] Minting against a settled bolt11 quote yields proofs of that value.
pub async fn mint_flow<C: MintConnector>(conn: C) {
    let amount = Amount::from(64);
    let (proofs, _, _) = mint_proofs(&conn, amount).await;
    assert_eq!(
        proofs.total_amount().expect("total"),
        amount,
        "minted proofs total the requested amount"
    );
}

/// [NUT-03] Swapping minted proofs returns a fresh set of equal total value.
pub async fn swap_flow<C: MintConnector>(conn: C) {
    let (proofs, keyset_id, keys) = mint_proofs(&conn, Amount::from(64)).await;
    let total = proofs.total_amount().expect("total");

    let preswap = PreMintSecrets::random(
        keyset_id,
        total,
        &SplitTarget::default(),
        &fee_and_amounts(),
    )
    .expect("premint secrets");
    let response = conn
        .post_swap(SwapRequest::new(proofs, preswap.blinded_messages()))
        .await
        .expect("swap");

    let new_proofs = construct_proofs(response.signatures, preswap.rs(), preswap.secrets(), &keys)
        .expect("construct proofs");
    assert_eq!(
        new_proofs.total_amount().expect("total"),
        total,
        "swap preserves total value"
    );
}

/// [NUT-05] Melting minted proofs settles a bolt11 invoice.
pub async fn melt_flow<C: MintConnector>(conn: C) {
    let invoice = create_fake_invoice(2_000, String::new()); // 2 sat
    let quote = conn
        .post_melt_quote(MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        }))
        .await
        .expect("melt quote");
    let (quote_id, needed) = match quote {
        MeltQuoteCreateResponse::Bolt11(r) => (r.quote, r.amount + r.fee_reserve),
        _ => panic!("expected a bolt11 melt quote"),
    };

    // Mint exactly the quote's amount + fee reserve so the melt balances without
    // change outputs.
    let (proofs, _, _) = mint_proofs(&conn, needed).await;

    let response = conn
        .post_melt(
            &PaymentMethod::BOLT11,
            MeltRequest::new(quote_id.clone(), proofs, None),
        )
        .await
        .expect("melt");
    assert_eq!(
        response.state(),
        MeltQuoteState::Paid,
        "fake wallet settles the melt"
    );

    // The status read must agree with the settle result over the same transport.
    let status = conn
        .get_melt_quote_status(PaymentMethod::BOLT11, &quote_id)
        .await
        .expect("melt quote status");
    assert_eq!(
        status.state(),
        MeltQuoteState::Paid,
        "melt quote status reports Paid after settlement"
    );
}

/// [NUT-29] Batch-minting two settled quotes issues their combined value, batch
/// status returns one entry per quote, and a malformed quote id is rejected
/// rather than silently dropped.
pub async fn batch_flow<C: MintConnector>(conn: C) {
    let (keyset_id, keys) = active_keyset(&conn).await;
    let (a1, a2) = (Amount::from(16), Amount::from(48));
    let id1 = settled_mint_quote(&conn, a1).await;
    let id2 = settled_mint_quote(&conn, a2).await;
    let total = a1 + a2;

    // bolt11 batch mint derives each quote's amount from the quote itself, so
    // `quote_amounts` is omitted and the shared outputs cover the combined total.
    let premint = PreMintSecrets::random(
        keyset_id,
        total,
        &SplitTarget::default(),
        &fee_and_amounts(),
    )
    .expect("premint secrets");
    let response = conn
        .post_batch_mint(
            &PaymentMethod::BOLT11,
            BatchMintRequest {
                quotes: vec![id1.clone(), id2.clone()],
                quote_amounts: None,
                outputs: premint.blinded_messages(),
                signatures: None,
            },
        )
        .await
        .expect("batch mint");

    let proofs = construct_proofs(response.signatures, premint.rs(), premint.secrets(), &keys)
        .expect("construct proofs");
    assert_eq!(
        proofs.total_amount().expect("total"),
        total,
        "batch mint issues the combined quote value"
    );

    let statuses = conn
        .post_batch_check_mint_quote_status(
            &PaymentMethod::BOLT11,
            BatchCheckMintQuoteRequest {
                quotes: vec![id1.clone(), id2.clone()],
            },
        )
        .await
        .expect("batch check");
    assert_eq!(statuses.len(), 2, "one status per requested quote");

    // A malformed quote id must surface an error, not be silently dropped
    // (which would return a shorter, misaligned status list).
    let malformed = conn
        .post_batch_check_mint_quote_status(
            &PaymentMethod::BOLT11,
            BatchCheckMintQuoteRequest {
                quotes: vec![id1, "not-a-quote".to_string()],
            },
        )
        .await;
    assert!(
        malformed.is_err(),
        "an invalid quote id must be rejected, got {malformed:?}"
    );
}

/// [NUT-17] Open a raw stream and run a subscribe handshake over it. Exercises
/// `open_stream` on both the connector and the server (the mint's protocol
/// runner), independent of the transport carrying the stream.
pub async fn open_stream_subscribe<C: MintConnector + Sync>(conn: C) {
    let (mut tx, mut rx) = conn.open_stream().await.expect("open_stream");

    let sub_id = "connector-test-sub";
    let request = WsRequest::from((
        WsMethodRequest::Subscribe(Params {
            kind: Kind::Bolt11MintQuote,
            filters: vec![QuoteId::new().to_string()],
            id: Arc::new(SubId::from(sub_id)),
        }),
        0,
    ));

    tx.send(serde_json::to_string(&request).expect("serialize request"))
        .await
        .expect("send subscribe");

    let reply = rx
        .recv()
        .await
        .expect("stream still open")
        .expect("receive a reply");
    assert!(
        reply.contains("OK"),
        "subscribe should be acknowledged: {reply}"
    );
    assert!(
        reply.contains(sub_id),
        "reply should name the subscription: {reply}"
    );
}

/// Generate one `#[tokio::test]` per shared body, each fed by `$make_conn_fn`.
///
/// The factory has signature `async fn(&str) -> C where C: MintConnector`; the
/// `&str` is the test name, for per-test isolation if a backend needs it.
#[macro_export]
macro_rules! mint_connector_test {
    ($make_conn_fn:ident) => {
        $crate::mint_connector_test!(
            $make_conn_fn,
            keys_roundtrip,
            keysets_roundtrip,
            keyset_by_id,
            unknown_keyset_rejected,
            mint_info,
            check_state,
            restore,
            mint_flow,
            swap_flow,
            melt_flow,
            batch_flow,
            open_stream_subscribe,
        );
    };
    ($make_conn_fn:ident, $($name:ident),+ $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                let conn = $make_conn_fn(stringify!($name)).await;
                $crate::mint_connector_test::$name(conn).await;
            }
        )+
    };
}
