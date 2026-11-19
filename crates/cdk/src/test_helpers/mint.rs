#![cfg(test)]
//! Test helpers for creating test mints and related utilities

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cdk_common::amount::SplitTarget;
use cdk_common::dhke::construct_proofs;
use cdk_common::nut00::KnownMethod;
use cdk_common::nuts::{BlindedMessage, CurrencyUnit, Id, PaymentMethod, PreMintSecrets, Proofs};
use cdk_common::{
    Amount, MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteState, MintRequest,
};
use cdk_fake_wallet::FakeWallet;
use tokio::time::sleep;

use crate::mint::{Mint, MintBuilder, MintMeltLimits};
use crate::types::{FeeReserve, QuoteTTL};
use crate::Error;

thread_local! {
    /// Thread-local storage for test failure flags.
    /// Using thread-local instead of env vars prevents race conditions
    /// when tests run in parallel (each test thread has its own copy).
    static TEST_FAILURES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Sets a failure flag for the current thread only.
/// Use this instead of `std::env::set_var("TEST_FAIL_X", "1")`.
#[cfg(test)]
pub(crate) fn set_fail_for(operation: &str) {
    TEST_FAILURES.with(|failures| {
        failures.borrow_mut().push(operation.to_string());
    });
}

/// Clears a failure flag for the current thread only.
/// Use this instead of `std::env::remove_var("TEST_FAIL_X")`.
#[cfg(test)]
pub(crate) fn clear_fail_for(operation: &str) {
    TEST_FAILURES.with(|failures| {
        failures.borrow_mut().retain(|s| s != operation);
    });
}

#[cfg(test)]
pub(crate) fn should_fail_in_test() -> bool {
    TEST_FAILURES.with(|failures| failures.borrow().contains(&"GENERAL".to_string()))
}

#[cfg(test)]
pub(crate) fn should_fail_for(operation: &str) -> bool {
    TEST_FAILURES.with(|failures| failures.borrow().contains(&operation.to_string()))
}

/// Consumes a failure flag so subsequent calls can succeed.
pub(crate) fn take_fail_for(operation: &str) -> bool {
    TEST_FAILURES.with(|failures| {
        let mut failures = failures.borrow_mut();
        match failures.iter().position(|failure| failure == operation) {
            Some(index) => {
                failures.remove(index);
                true
            }
            None => false,
        }
    })
}

/// Creates and starts a test mint with in-memory storage and a fake payment backend.
///
/// This mint can be used for unit tests without requiring external dependencies
/// like a payment backend or persistent databases.
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

    let fake_payment_backend = FakeWallet::new(
        fee_reserve.clone(),
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
            Arc::new(fake_payment_backend),
        )
        .await?;

    let mnemonic = Mnemonic::generate(12).map_err(|e| Error::Custom(e.to_string()))?;

    mint_builder = mint_builder
        .with_name("test mint".to_string())
        .with_description("test mint for unit tests".to_string())
        .with_urls(vec!["https://test-mint".to_string()])
        .with_pubkey(crate::nuts::SecretKey::generate().public_key());

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
            .check_mint_quotes(
                &[cdk_common::QuoteId::from_str(&mint_quote.quote).unwrap()],
                None,
            )
            .await
            .unwrap()
            .first()
            .unwrap()
            .clone()
            .into();

        if check.state == MintQuoteState::Paid {
            break;
        }

        sleep(Duration::from_secs(1)).await;
    }

    let keysets = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();

    let keys = mint
        .keyset_pubkeys(&keysets)?
        .keysets
        .first()
        .unwrap()
        .keys
        .clone();

    let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());

    let premint_secrets =
        PreMintSecrets::random(keysets, amount, &SplitTarget::None, &fees.into()).unwrap();

    let request = MintRequest {
        quote: mint_quote.quote,
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let mint_res = mint
        .process_mint_request(crate::mint::MintInput::Single(request.try_into().unwrap()))
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

/// Models abandoned executions whose recorded leases have expired.
pub(crate) async fn expire_melt_leases(mint: &Mint) {
    for quote in mint.localstore().get_melt_quotes().await.unwrap() {
        if quote.is_locked() {
            let mut tx = mint.localstore().begin_transaction().await.unwrap();
            tx.renew_melt_quote_lease(&quote.id, &quote.melt_lock, 0)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    }
}

/// Spend keys of v3 proofs, by secret point
pub type V3Keys = HashMap<cdk_common::PublicKey, cdk_common::SecretKey>;

/// Mint v3 proofs on the active sat keyset (which must be v3), keeping
/// each proof's spend key
///
/// Minting onto a v3 keyset is a transaction: the quote is locked to a
/// fresh key that signs the quote input's digest.
pub async fn mint_v3_test_proofs(mint: &Mint, amount: Amount) -> Result<(Proofs, V3Keys), Error> {
    use cdk_common::nut00::transaction::MintQuoteState as QuoteInputState;
    use cdk_common::nuts::nut10::nutroot::sign_input_key_path;
    use cdk_common::secret::Secret;

    let lock_key = cdk_common::SecretKey::generate();
    let mint_quote: MintQuoteBolt11Response<_> = mint
        .get_mint_quote(
            MintQuoteBolt11Request {
                amount,
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: Some(lock_key.public_key()),
            }
            .into(),
        )
        .await?
        .into();
    let quote_id = cdk_common::QuoteId::from_str(&mint_quote.quote).unwrap();

    loop {
        let check: MintQuoteBolt11Response<_> = mint
            .check_mint_quotes(std::slice::from_ref(&quote_id), None)
            .await
            .unwrap()
            .first()
            .unwrap()
            .clone()
            .into();
        if check.state == MintQuoteState::Paid {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }

    let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
    assert_eq!(
        keyset_id.get_version(),
        cdk_common::nut02::KeySetVersion::Version02,
        "the active sat keyset must be v3"
    );
    let keys = mint
        .keyset_pubkeys(&keyset_id)?
        .keysets
        .first()
        .unwrap()
        .keys
        .clone();
    let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());
    let amounts = amount.split_targeted(&SplitTarget::None, &fees.into())?;

    let mut spend_keys = V3Keys::new();
    let secrets = amounts
        .iter()
        .map(|_| {
            let (secret, key) = Secret::generate_v3();
            spend_keys.insert(key.public_key(), key);
            secret
        })
        .collect();
    let premint_secrets = PreMintSecrets::from_secrets(keyset_id, amounts, secrets)?;

    let mut request = MintRequest {
        quote: quote_id.clone(),
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };
    let issued = mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await?
        .unwrap()
        .amount_issued()
        .into();
    let transaction = request.transaction(&QuoteInputState {
        lock_key: lock_key.public_key(),
        issued,
    })?;
    request.signature = Some(sign_input_key_path(&transaction, 0, &lock_key)?.to_string());

    let mint_res = mint
        .process_mint_request(crate::mint::MintInput::Single(request))
        .await?;

    let proofs = construct_proofs(
        mint_res.signatures,
        premint_secrets.rs(),
        premint_secrets.secrets(),
        &keys,
    )?;
    Ok((proofs, spend_keys))
}

/// Sign every v3 input of `transaction` by the key path with its kept key
pub fn sign_v3_inputs(
    transaction: &cdk_common::nut00::transaction::Transaction,
    proofs: &mut Proofs,
    keys: &V3Keys,
) -> Result<(), Error> {
    cdk_common::nuts::nut10::nutroot::sign_proofs_key_path(transaction, proofs, |point| {
        keys.get(point).cloned()
    })?;
    Ok(())
}
