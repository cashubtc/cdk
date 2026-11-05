//! A BOLT11 mint quote must be for the amount requested before it is stored.

use std::sync::Arc;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use cdk_common::nut23::QuoteState;
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};

use super::*;
use crate::wallet::test_utils::{create_test_db, create_test_wallet_with_mock, MockMintConnector};

fn invoice(amount_msat: Option<u64>) -> String {
    let private_key = SecretKey::from_slice(&[42; 32]).expect("valid test key");
    let builder = InvoiceBuilder::new(Currency::Bitcoin)
        .description("mint quote check".to_string())
        .payment_hash(Sha256Hash::hash(b"mint quote check"))
        .payment_secret(PaymentSecret([21; 32]))
        .current_timestamp()
        .min_final_cltv_expiry_delta(144);
    let signed = match amount_msat {
        Some(msat) => builder
            .amount_milli_satoshis(msat)
            .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &private_key)),
        None => {
            builder.build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &private_key))
        }
    };
    signed.expect("test invoice should build").to_string()
}

fn response(
    amount: Option<u64>,
    unit: Option<CurrencyUnit>,
    invoice_msat: Option<u64>,
) -> MintQuoteBolt11Response<String> {
    MintQuoteBolt11Response {
        quote: "mint-quote".to_string(),
        request: invoice(invoice_msat),
        amount: amount.map(Amount::from),
        unit,
        method: PaymentMethod::Known(KnownMethod::Bolt11),
        amount_paid: Amount::ZERO,
        amount_issued: Amount::ZERO,
        updated_at: 0,
        state: QuoteState::Unpaid,
        expiry: Some(2_000_000_000),
        pubkey: None,
    }
}

#[test]
fn accepts_matching_quotes() {
    let sat = Some(CurrencyUnit::Sat);
    let msat = Some(CurrencyUnit::Msat);
    let usd = Some(CurrencyUnit::Usd);
    for (resp, unit, requested) in [
        (
            response(Some(100), sat.clone(), Some(100_000)),
            CurrencyUnit::Sat,
            100,
        ),
        (
            response(Some(1_500), msat, Some(1_500)),
            CurrencyUnit::Msat,
            1_500,
        ),
        // An older mint may omit amount and unit; the invoice still matches.
        (response(None, None, Some(100_000)), CurrencyUnit::Sat, 100),
        // A usd quote's invoice is in sats at the mint's rate.
        (
            response(Some(5), usd, Some(7_777_000)),
            CurrencyUnit::Usd,
            5,
        ),
    ] {
        check_bolt11_mint_quote(&resp, &unit, Amount::from(requested)).unwrap();
    }
}

#[test]
fn refuses_wrong_invoice_amount() {
    let sat = Some(CurrencyUnit::Sat);
    for invoice_msat in [100_000_000, 99_000, 100_001] {
        let resp = response(Some(100), sat.clone(), Some(invoice_msat));
        assert!(matches!(
            check_bolt11_mint_quote(&resp, &CurrencyUnit::Sat, Amount::from(100)),
            Err(Error::IncorrectQuoteAmount)
        ));
    }
}

#[test]
fn refuses_wrong_invoice_when_mint_omits_amount_and_unit() {
    let resp = response(None, None, Some(100_000_000));
    assert!(matches!(
        check_bolt11_mint_quote(&resp, &CurrencyUnit::Sat, Amount::from(100)),
        Err(Error::IncorrectQuoteAmount)
    ));
}

#[test]
fn sat_to_msat_overflow_is_refused() {
    // Matching quote fields, so only the conversion can fail.
    let resp = response(Some(u64::MAX), Some(CurrencyUnit::Sat), Some(100_000));
    assert!(matches!(
        check_bolt11_mint_quote(&resp, &CurrencyUnit::Sat, Amount::from(u64::MAX)),
        Err(Error::AmountOverflow)
    ));
}

#[test]
fn refuses_wrong_quote_amount() {
    let resp = response(Some(100_000), Some(CurrencyUnit::Sat), Some(100_000));
    assert!(matches!(
        check_bolt11_mint_quote(&resp, &CurrencyUnit::Sat, Amount::from(100)),
        Err(Error::IncorrectQuoteAmount)
    ));
}

#[test]
fn refuses_amountless_invoice() {
    let resp = response(Some(100), Some(CurrencyUnit::Sat), None);
    assert!(matches!(
        check_bolt11_mint_quote(&resp, &CurrencyUnit::Sat, Amount::from(100)),
        Err(Error::InvoiceAmountUndefined)
    ));
}

#[test]
fn refuses_other_unit() {
    let resp = response(Some(100), Some(CurrencyUnit::Usd), Some(100_000));
    assert!(matches!(
        check_bolt11_mint_quote(&resp, &CurrencyUnit::Sat, Amount::from(100)),
        Err(Error::UnitMismatch)
    ));
}

async fn mint_quote_with(
    resp: MintQuoteBolt11Response<String>,
) -> (Result<MintQuote, Error>, usize) {
    let db = create_test_db().await;
    let client = Arc::new(MockMintConnector::new());
    client.push_post_mint_quote_response(Ok(MintQuoteResponse::Bolt11(resp)));
    let wallet = create_test_wallet_with_mock(db.clone(), client).await;
    let result = wallet
        .mint_quote(
            PaymentMethod::Known(KnownMethod::Bolt11),
            Some(Amount::from(100)),
            None,
            None,
        )
        .await;
    let stored = db.get_mint_quotes().await.unwrap().len();
    (result, stored)
}

#[tokio::test]
async fn honest_quote_is_stored() {
    let (result, stored) =
        mint_quote_with(response(Some(100), Some(CurrencyUnit::Sat), Some(100_000))).await;
    result.unwrap();
    assert_eq!(stored, 1);
}

#[tokio::test]
async fn wrong_invoice_is_refused_before_storing() {
    // A sat/msat mix-up at the mint: 100 sat quoted, 100,000 sat invoiced.
    let (result, stored) = mint_quote_with(response(
        Some(100),
        Some(CurrencyUnit::Sat),
        Some(100_000_000),
    ))
    .await;
    assert!(matches!(result, Err(Error::IncorrectQuoteAmount)));
    assert_eq!(stored, 0);
}
