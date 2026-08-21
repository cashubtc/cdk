#![allow(clippy::unwrap_used)]

//! NUT-XX: mint quote lookup by public key
//!
//! <https://github.com/cashubtc/nuts/blob/get-quotes-by-pubkeys/xx.md>

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bip39::Mnemonic;
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use cdk::mint::{Mint, MintBuilder, MintMeltLimits};
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nutxx::{mint_quote_lookup_msg_to_sign, MAX_LOOKUP_PUBKEYS};
use cdk::nuts::{CurrencyUnit, MintQuoteBolt11Request, PaymentMethod, PublicKey, SecretKey};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk::{Amount, Error, MintQuoteRequest};
use cdk_fake_wallet::FakeWallet;

async fn test_mint() -> Mint {
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
        .with_name("nutxx test mint".to_string())
        .with_description("nutxx test mint".to_string())
        .with_urls(vec!["https://test-mint".to_string()]);

    let mint = builder
        .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();
    mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
        .await
        .unwrap();
    mint
}

/// Create a NUT-20 locked bolt11 mint quote owned by `pubkey`.
async fn locked_quote(mint: &Mint, pubkey: PublicKey) {
    mint.get_mint_quote(MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
        amount: Amount::new(100, CurrencyUnit::Sat).into(),
        unit: CurrencyUnit::Sat,
        description: None,
        pubkey: Some(pubkey),
    }))
    .await
    .unwrap();
}

/// Sign the lookup message the way a spec-conformant wallet does.
async fn sign_lookup(
    mint: &Mint,
    secret_key: &SecretKey,
) -> bitcoin::secp256k1::schnorr::Signature {
    let mint_pubkey = mint.mint_info().await.unwrap().pubkey.unwrap();
    let msg = mint_quote_lookup_msg_to_sign(&mint_pubkey, &secret_key.public_key());
    secret_key.sign(&msg).unwrap()
}

/// A mint that serves the endpoint must say so in its NUT-06 info response, otherwise wallets
/// have no way to discover it.
#[tokio::test]
async fn support_is_advertised_in_mint_info() {
    let mint = test_mint().await;
    let mint_info = mint.mint_info().await.unwrap();

    assert!(mint_info.nuts.nutxx.supported);

    let json = serde_json::to_value(&mint_info).unwrap();
    assert_eq!(json["nuts"]["XX"]["supported"], true);
}

/// A valid signature returns the quotes locked to that pubkey.
#[tokio::test]
async fn signed_lookup_returns_own_quotes() {
    let mint = test_mint().await;
    let owner = SecretKey::generate();
    let pubkey = owner.public_key();
    locked_quote(&mint, pubkey).await;

    let signature = sign_lookup(&mint, &owner).await;
    let quotes = mint
        .get_mint_quote_by_pubkey(vec![pubkey], vec![signature])
        .await
        .unwrap();

    assert_eq!(quotes.len(), 1);
    assert_eq!(
        quotes[0].method(),
        PaymentMethod::Known(KnownMethod::Bolt11)
    );
}

/// The signature covers SHA256(preimage), matching the NUT. `PublicKey::verify` hashes its
/// argument, so passing a digest instead of the preimage would verify a double hash and reject
/// conformant wallets.
#[tokio::test]
async fn signature_is_over_a_single_hash_of_the_preimage() {
    let mint = test_mint().await;
    let owner = SecretKey::generate();
    let pubkey = owner.public_key();
    locked_quote(&mint, pubkey).await;

    let mint_pubkey = mint.mint_info().await.unwrap().pubkey.unwrap();
    let msg = mint_quote_lookup_msg_to_sign(&mint_pubkey, &pubkey);

    // Signing the preimage is accepted...
    let signature = owner.sign(&msg).unwrap();
    assert!(mint
        .get_mint_quote_by_pubkey(vec![pubkey], vec![signature])
        .await
        .is_ok());

    // ...and signing the digest of the preimage is not.
    let digest = Sha256Hash::hash(&msg).to_byte_array();
    let double_hashed = owner.sign(&digest).unwrap();
    assert!(matches!(
        mint.get_mint_quote_by_pubkey(vec![pubkey], vec![double_hashed])
            .await,
        Err(Error::SignatureMissingOrInvalid)
    ));
}

/// "The mint MUST reject the request unless every signature is valid" — an empty or short
/// signature array must not silently skip verification.
#[tokio::test]
async fn missing_signatures_are_rejected() {
    let mint = test_mint().await;
    let victim = SecretKey::generate();
    let victim_pubkey = victim.public_key();
    locked_quote(&mint, victim_pubkey).await;

    // No signature at all.
    assert!(matches!(
        mint.get_mint_quote_by_pubkey(vec![victim_pubkey], vec![])
            .await,
        Err(Error::SignatureMissingOrInvalid)
    ));

    // Fewer signatures than pubkeys: one valid signature must not authorise a second pubkey.
    let attacker = SecretKey::generate();
    let attacker_signature = sign_lookup(&mint, &attacker).await;
    assert!(matches!(
        mint.get_mint_quote_by_pubkey(
            vec![attacker.public_key(), victim_pubkey],
            vec![attacker_signature]
        )
        .await,
        Err(Error::SignatureMissingOrInvalid)
    ));
}

/// A signature from one key must not unlock a different key's quotes.
#[tokio::test]
async fn signature_from_another_key_is_rejected() {
    let mint = test_mint().await;
    let victim_pubkey = SecretKey::generate().public_key();
    locked_quote(&mint, victim_pubkey).await;

    let attacker = SecretKey::generate();
    let attacker_signature = sign_lookup(&mint, &attacker).await;

    assert!(matches!(
        mint.get_mint_quote_by_pubkey(vec![victim_pubkey], vec![attacker_signature])
            .await,
        Err(Error::SignatureMissingOrInvalid)
    ));
}

/// A signature bound to a different mint must not be replayable here.
#[tokio::test]
async fn signature_for_another_mint_is_rejected() {
    let mint = test_mint().await;
    let owner = SecretKey::generate();
    let pubkey = owner.public_key();
    locked_quote(&mint, pubkey).await;

    let other_mint_pubkey = SecretKey::generate().public_key();
    let msg = mint_quote_lookup_msg_to_sign(&other_mint_pubkey, &pubkey);
    let signature = owner.sign(&msg).unwrap();

    assert!(matches!(
        mint.get_mint_quote_by_pubkey(vec![pubkey], vec![signature])
            .await,
        Err(Error::SignatureMissingOrInvalid)
    ));
}

/// An anonymous caller cannot ask the mint for unbounded signature verification.
#[tokio::test]
async fn oversized_request_is_rejected() {
    let mint = test_mint().await;

    let pubkeys: Vec<PublicKey> = (0..MAX_LOOKUP_PUBKEYS + 1)
        .map(|_| SecretKey::generate().public_key())
        .collect();

    assert!(matches!(
        mint.get_mint_quote_by_pubkey(pubkeys, vec![]).await,
        Err(Error::BatchSizeExceeded { .. })
    ));
}
