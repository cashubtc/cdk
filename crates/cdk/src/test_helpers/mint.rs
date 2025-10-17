#![cfg(test)]
//! Test helpers for creating test mints and related utilities

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cdk_common::amount::SplitTarget;
use cdk_common::dhke::construct_proofs;
use cdk_common::nuts::{BlindedMessage, CurrencyUnit, Id, PaymentMethod, PreMintSecrets, Proofs};
use cdk_common::{
    Amount, MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteState, MintRequest,
};
use cdk_fake_wallet::FakeWallet;
use tokio::time::sleep;

use crate::mint::{Mint, MintBuilder, MintMeltLimits};
use crate::types::{FeeReserve, QuoteTTL};
use crate::Error;

#[cfg(test)]
pub(crate) fn should_fail_in_test() -> bool {
    // Some condition that determines when to fail in tests
    std::env::var("TEST_FAIL").is_ok()
}

#[cfg(test)]
pub(crate) fn should_fail_for(operation: &str) -> bool {
    // Check for specific failure modes using environment variables
    // Format: TEST_FAIL_<OPERATION>
    let var_name = format!("TEST_FAIL_{}", operation);
    std::env::var(&var_name).is_ok()
}

/// Creates and starts a test mint with in-memory storage and a fake Lightning backend.
///
/// This mint can be used for unit tests without requiring external dependencies
/// like Lightning nodes or persistent databases.
///
/// # Example
///
/// ```
/// use cdk::test_helpers::mint::create_test_mint;
///
/// #[tokio::test]
/// async fn test_something() {
///     let mint = create_test_mint().await.unwrap();
///     // Use the mint for testing
/// }
/// ```
pub async fn create_test_mint() -> Result<Mint, Error> {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await?);

    let mut mint_builder = MintBuilder::new(db.clone());

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let ln_fake_backend = FakeWallet::new(
        fee_reserve.clone(),
        HashMap::default(),
        HashSet::default(),
        2,
        CurrencyUnit::Sat,
    );

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 10_000),
            Arc::new(ln_fake_backend),
        )
        .await?;

    let mnemonic = Mnemonic::generate(12).map_err(|e| Error::Custom(e.to_string()))?;

    mint_builder = mint_builder
        .with_name("test mint".to_string())
        .with_description("test mint for unit tests".to_string())
        .with_urls(vec!["https://test-mint".to_string()]);

    let quote_ttl = QuoteTTL::new(10000, 10000);

    let mint = mint_builder
        .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
        .await?;

    mint.set_quote_ttl(quote_ttl).await?;

    mint.start().await?;

    Ok(mint)
}

/// Creates test proofs by performing a mock mint operation.
///
/// This helper creates valid proofs for the given amount by:
/// 1. Creating blinded messages
/// 2. Performing a swap to get signatures
/// 3. Constructing valid proofs from the signatures
///
/// # Arguments
///
/// * `mint` - The test mint to use for creating proofs
/// * `amount` - The total amount to create proofs for
pub async fn mint_test_proofs(mint: &Mint, amount: Amount) -> Result<Proofs, Error> {
    // Just use fund_mint_with_proofs which creates proofs via swap
    let mint_quote: MintQuoteBolt11Response<_> = mint
        .get_mint_quote(
            MintQuoteBolt11Request {
                amount,
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }
            .into(),
        )
        .await?
        .into();

    loop {
        let check: MintQuoteBolt11Response<_> = mint
            .check_mint_quote(&cdk_common::QuoteId::from_str(&mint_quote.quote).unwrap())
            .await
            .unwrap()
            .into();

        if check.state == MintQuoteState::Paid {
            break;
        }

        sleep(Duration::from_secs(1)).await;
    }

    let keysets = mint
        .get_active_keysets()
        .get(&CurrencyUnit::Sat)
        .unwrap()
        .clone();

    let keys = mint
        .keyset_pubkeys(&keysets)?
        .keysets
        .first()
        .unwrap()
        .keys
        .clone();

    let fees: (u64, Vec<u64>) = (
        0,
        keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>().into(),
    );

    let premint_secrets =
        PreMintSecrets::random(keysets, amount, &SplitTarget::None, &fees.into()).unwrap();

    let request = MintRequest {
        quote: mint_quote.quote,
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let mint_res = mint
        .process_mint_request(request.try_into().unwrap())
        .await?;

    Ok(construct_proofs(
        mint_res.signatures,
        premint_secrets.rs(),
        premint_secrets.secrets(),
        &keys,
    )?)
}

/// Creates test blinded messages for the given amount.
///
/// This is useful for testing operations that require blinded messages as input.
///
/// # Arguments
///
/// * `mint` - The test mint (used to get the active keyset)
/// * `amount` - The total amount to create blinded messages for
///
/// # Returns
///
/// A tuple containing:
/// - Vector of blinded messages
/// - PreMintSecrets (needed to construct proofs later)
pub async fn create_test_blinded_messages(
    mint: &Mint,
    amount: Amount,
) -> Result<(Vec<BlindedMessage>, PreMintSecrets), Error> {
    let keyset_id = get_active_keyset_id(mint).await?;
    let split_target = SplitTarget::default();
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint = PreMintSecrets::random(keyset_id, amount, &split_target, &fee_and_amounts)?;
    let blinded_messages = pre_mint.blinded_messages().to_vec();

    Ok((blinded_messages, pre_mint))
}

/// Gets the active keyset ID from the mint.
pub async fn get_active_keyset_id(mint: &Mint) -> Result<Id, Error> {
    let keys = mint
        .pubkeys()
        .keysets
        .first()
        .ok_or(Error::Internal)?
        .clone();
    keys.verify_id()?;
    Ok(keys.id)
}
