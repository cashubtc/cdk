#![cfg(test)]
//! Test helpers for creating test mints and related utilities

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bip39::Mnemonic;
use cdk_common::amount::SplitTarget;
use cdk_common::dhke::construct_proofs;
use cdk_common::nut00::KnownMethod;
use cdk_common::nuts::{BlindedMessage, CurrencyUnit, Id, PaymentMethod, PreMintSecrets, Proofs};
use cdk_common::payment::{
    self, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse,
    MintPayment, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse, SettingsResponse,
    WaitPaymentResponse,
};
use cdk_common::{
    Amount, MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteState, MintRequest,
};
use cdk_fake_wallet::FakeWallet;
use futures::Stream;
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
        .with_urls(vec!["https://test-mint".to_string()]);

    let quote_ttl = QuoteTTL::new(10000, 10000);

    let mint = mint_builder
        .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
        .await?;

    mint.set_quote_ttl(quote_ttl).await?;

    mint.start().await?;

    Ok(mint)
}

/// A payment backend that records how many outgoing payments were dispatched.
///
/// Nothing else in the workspace observes whether `make_payment` was called, and
/// asserting that it was not is the only direct way to prove the mint refused an
/// operation before parting with anything.
pub struct CountingPaymentBackend {
    inner: FakeWallet,
    payments: Arc<AtomicUsize>,
}

impl CountingPaymentBackend {
    /// Wraps a `FakeWallet` and hands back the counter it increments.
    pub fn new() -> (Arc<Self>, Arc<AtomicUsize>) {
        let payments = Arc::new(AtomicUsize::new(0));
        let fee_reserve = FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        };

        let backend = Arc::new(Self {
            inner: FakeWallet::new(
                fee_reserve,
                HashMap::default(),
                HashSet::default(),
                2,
                CurrencyUnit::Sat,
            ),
            payments: Arc::clone(&payments),
        });

        (backend, payments)
    }
}

#[async_trait]
impl MintPayment for CountingPaymentBackend {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        self.inner.get_settings().await
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        self.inner.create_incoming_payment_request(options).await
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        self.inner.get_payment_quote(unit, options).await
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.payments.fetch_add(1, Ordering::SeqCst);
        self.inner.make_payment(unit, options).await
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        self.inner.wait_payment_event().await
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.inner.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream()
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        self.inner
            .check_incoming_payment_status(payment_identifier)
            .await
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.inner.check_outgoing_payment(payment_identifier).await
    }
}

/// Builds a test mint against a caller-supplied payment backend.
pub async fn create_test_mint_with_backend(
    backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync>,
) -> Result<Mint, Error> {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await?);
    create_test_mint_on(db, backend).await
}

/// Builds a test mint on a caller-supplied database and payment backend.
pub async fn create_test_mint_on<DB>(
    db: Arc<DB>,
    backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync>,
) -> Result<Mint, Error>
where
    DB: cdk_common::database::MintDatabase<cdk_common::database::Error>
        + cdk_common::database::MintKeysDatabase<Err = cdk_common::database::Error>
        + Send
        + Sync
        + 'static,
{
    let mut mint_builder = MintBuilder::new(db.clone());

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            backend,
        )
        .await?;

    let mnemonic = Mnemonic::generate(12).map_err(|e| Error::Custom(e.to_string()))?;

    mint_builder = mint_builder
        .with_name("test mint".to_string())
        .with_description("test mint for unit tests".to_string())
        .with_urls(vec!["https://test-mint".to_string()]);

    let mint = mint_builder
        .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
        .await?;

    mint.set_quote_ttl(QuoteTTL::new(10000, 10000)).await?;
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
            .check_mint_quotes(&[cdk_common::QuoteId::from_str(&mint_quote.quote).unwrap()])
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
