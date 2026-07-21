use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::common::FeeReserve;
use cdk_common::database::mint::{
    CompletedOperationsDatabase, FederationApplicationDatabase, SagaDatabase,
};
use cdk_common::database::{
    MintDatabase, MintFederationJournalDatabase, MintProofsDatabase, MintQuotesDatabase,
    MintSignaturesDatabase,
};
use cdk_common::error::Error;
use cdk_common::federation::{
    generate_trusted_dealer_configs, FederatedKeyset, FederationApplicationState, FederationError,
    FederationJournalConsensusEvidence, FederationJournalDigest, FederationJournalEntry,
    FederationKeysetRotationOperation, FederationMeltOperation, FederationMeltQuoteMetadata,
    FederationMeltQuoteOperation, FederationMeltQuotePaymentObservation, FederationMintOperation,
    FederationMintQuoteOperation, FederationMintQuotePaymentObservation, FederationMintRequest,
    FederationOperation, FederationOperationEnvelope, FederationOperationId,
    FederationOperationStatus, FederationPaymentObservation, FederationPaymentObservationContext,
    FederationPaymentObservationPolicy, FederationPaymentObservationReport,
    FederationSwapOperation, InMemoryFederationJournal, MemberId, ThresholdParams,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk_common::mint::{MeltSagaState, SagaStateEnum};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut00::KnownMethod;
use cdk_common::nuts::nut01::{
    verify_blind_signature_share, BlsG1PublicKey, BlsPublicKeyShare, BlsSecretKey,
};
use cdk_common::nuts::nut05::MeltQuoteCustomRequest;
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, DynMintPayment, Event,
    IncomingPaymentOptions, MakePaymentResponse, MintPayment, OutgoingPaymentOptions,
    PaymentIdentifier, PaymentQuoteResponse, SettingsResponse, WaitPaymentResponse,
};
use cdk_common::quote_id::QuoteId;
use cdk_common::secret::Secret;
use cdk_common::{
    Amount, BatchMintRequest, BlindSignature, BlindedMessage, Conditions, CurrencyUnit, Id,
    MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltQuoteCreateResponse,
    MeltQuoteCustomResponse, MeltQuoteRequest, MeltQuoteState, MeltRequest, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintQuoteRequest, MintQuoteResponse, MintQuoteState, MintRequest,
    PaymentMethod, Proof, ProofsMethods, PublicKey, RestoreRequest, SecretKey, SigFlag,
    SpendingConditions, State, SwapRequest,
};
use cdk_sqlite::mint::{memory, MintSqliteDatabase};
use tokio::sync::{Mutex, Semaphore};

mod address_allocation;
mod admission;
mod chain_observation;
mod keyset;
mod melt;
mod mint;
mod payment_observation;
mod quote;
mod runtime;
mod swap;
mod transaction_policy;
mod transaction_signing;

use crate::mint::{Mint, MintBuilder, MintInput, MintMeltLimits};

use super::{
    federation_bark_matching_result_observers,
    federation_bark_result_payment_records_from_projection,
    FederationBarkPaymentObservationTemplate, FederationBarkResultObservationKey,
    FederationRuntimeConfig,
};

fn runtime_configs() -> Vec<FederationRuntimeConfig> {
    let members = (1..=2)
        .map(|id| {
            TrustedDealerMember::new(
                cdk_common::federation::MemberId::new(id).expect("member id"),
                MintUrl::from_str(&format!("https://mint{id}.example.com"))
                    .expect("public mint url"),
                MintUrl::from_str(&format!("https://fed{id}.example.com"))
                    .expect("federation api url"),
                SecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
            )
        })
        .collect();
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![BlsSecretKey::from_reduced_bytes(&[2; 32])],
        )]),
    );
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(2, 1).expect("params"),
        members,
        vec![keyset],
    )
    .expect("trusted dealer bundle");
    bundle
        .private_configs
        .values()
        .cloned()
        .map(|private_config| {
            FederationRuntimeConfig::new(bundle.public_config.clone(), private_config)
                .expect("runtime config")
        })
        .collect()
}

fn runtime_config() -> FederationRuntimeConfig {
    runtime_configs().remove(0)
}

#[test]
fn external_action_lease_replay_uses_the_accepted_decision_index() {
    let config = runtime_config();
    let author = MemberId::new(1).expect("author");
    let accepted_actor =
        Mint::federation_external_action_lease_actor_at_index(&config, author, 40, 41)
            .expect("accepted lease actor");
    let later_actor =
        Mint::federation_external_action_lease_actor_at_index(&config, author, 40, 49)
            .expect("later lease actor");

    assert_eq!(accepted_actor, author);
    assert_eq!(later_actor, MemberId::new(2).expect("rotated actor"));
    assert_eq!(
        Mint::federation_external_action_lease_actor_at_index(&config, author, 40, 41)
            .expect("replayed accepted actor"),
        accepted_actor
    );
}

fn runtime_config_with_inactive_rotation_keyset() -> FederationRuntimeConfig {
    let members = (1..=2)
        .map(|id| {
            TrustedDealerMember::new(
                cdk_common::federation::MemberId::new(id).expect("member id"),
                MintUrl::from_str(&format!("https://mint{id}.example.com"))
                    .expect("public mint url"),
                MintUrl::from_str(&format!("https://fed{id}.example.com"))
                    .expect("federation api url"),
                SecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
            )
        })
        .collect();
    let old_keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![BlsSecretKey::from_reduced_bytes(&[2; 32])],
        )]),
    );
    let next_keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        false,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![BlsSecretKey::from_reduced_bytes(&[7; 32])],
        )]),
    );
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(2, 1).expect("params"),
        members,
        vec![old_keyset, next_keyset],
    )
    .expect("trusted dealer bundle");
    let private_config = bundle
        .private_configs
        .get(&cdk_common::federation::MemberId::new(1).expect("member id"))
        .expect("private config")
        .clone();

    FederationRuntimeConfig::new(bundle.public_config.clone(), private_config)
        .expect("runtime config")
}

fn runtime_config_with_payment_policy(
    policy: FederationPaymentObservationPolicy,
) -> FederationRuntimeConfig {
    let members = (1..=3)
        .map(|id| {
            TrustedDealerMember::new(
                MemberId::new(id).expect("member id"),
                MintUrl::from_str(&format!("https://mint{id}.example.com"))
                    .expect("public mint url"),
                MintUrl::from_str(&format!("https://fed{id}.example.com"))
                    .expect("federation api url"),
                SecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
            )
        })
        .collect::<Vec<_>>();
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[2; 32]),
                BlsSecretKey::from_reduced_bytes(&[3; 32]),
            ],
        )]),
    );
    let mut bundle = generate_trusted_dealer_configs(
        ThresholdParams::new(3, 2, 2).expect("params"),
        members,
        vec![keyset],
    )
    .expect("trusted dealer bundle");
    bundle.public_config.payment_observation_policy = policy;
    let private_config = bundle
        .private_configs
        .get(&MemberId::new(1).expect("member id"))
        .expect("private config")
        .clone();

    FederationRuntimeConfig::new(bundle.public_config, private_config).expect("runtime config")
}

async fn federated_test_mint(
    runtime_config: FederationRuntimeConfig,
) -> (Mint, Arc<MintSqliteDatabase>) {
    let fake_wallet = cdk_fake_wallet::FakeWallet::new(
        FeeReserve {
            min_fee_reserve: Amount::from(1),
            percent_fee_reserve: 1.0,
        },
        HashMap::new(),
        HashSet::new(),
        0,
        CurrencyUnit::Sat,
    );
    federated_test_mint_with_payment_processor(runtime_config, Arc::new(fake_wallet)).await
}

async fn restart_federated_test_mint(
    runtime_config: FederationRuntimeConfig,
    localstore: Arc<MintSqliteDatabase>,
) -> Mint {
    let fake_wallet = cdk_fake_wallet::FakeWallet::new(
        FeeReserve {
            min_fee_reserve: Amount::from(1),
            percent_fee_reserve: 1.0,
        },
        HashMap::new(),
        HashSet::new(),
        0,
        CurrencyUnit::Sat,
    );
    let mut builder = MintBuilder::new(localstore.clone()).with_federation_config(runtime_config);
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            Arc::new(fake_wallet),
        )
        .await
        .expect("payment processor");
    builder
        .build_with_seed(localstore, &[9; 64])
        .await
        .expect("restarted mint")
}

async fn federated_test_mint_with_payment_processor(
    runtime_config: FederationRuntimeConfig,
    payment_processor: DynMintPayment,
) -> (Mint, Arc<MintSqliteDatabase>) {
    let localstore = Arc::new(memory::empty().await.expect("mint db"));
    let mut builder = MintBuilder::new(localstore.clone()).with_federation_config(runtime_config);
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            payment_processor,
        )
        .await
        .expect("payment processor");
    let mint = builder
        .build_with_seed(localstore.clone(), &[9; 64])
        .await
        .expect("mint");

    (mint, localstore)
}

struct BackendAgnosticPaymentProcessor;

#[async_trait]
impl MintPayment for BackendAgnosticPaymentProcessor {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        Ok(SettingsResponse {
            unit: CurrencyUnit::Sat.to_string(),
            bolt11: Some(Bolt11Settings {
                mpp: false,
                amountless: false,
                invoice_description: false,
            }),
            bolt12: None,
            onchain: None,
            custom: HashMap::new(),
        })
    }

    async fn create_incoming_payment_request(
        &self,
        _options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        Err(payment::Error::UnsupportedPaymentOption)
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(options) => {
                let amount_msat = options.bolt11.amount_milli_satoshis().ok_or_else(|| {
                    payment::Error::Custom(
                        "test backend requires amount-bearing Bolt11 invoices".to_string(),
                    )
                })?;
                let amount = Amount::new(amount_msat, CurrencyUnit::Msat).convert_to(unit)?;
                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                        *options.bolt11.payment_hash().as_ref(),
                    )),
                    amount,
                    fee: Amount::new(1, unit.clone()),
                    state: MeltQuoteState::Unpaid,
                    extra_json: None,
                    estimated_blocks: None,
                    fee_options: None,
                })
            }
            _ => Err(payment::Error::UnsupportedPaymentOption),
        }
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(options) => Ok(MakePaymentResponse {
                payment_lookup_id: PaymentIdentifier::PaymentHash(
                    *options.bolt11.payment_hash().as_ref(),
                ),
                payment_proof: None,
                status: MeltQuoteState::Pending,
                total_spent: Amount::new(0, unit.clone()),
            }),
            _ => Err(payment::Error::UnsupportedPaymentOption),
        }
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Event> + Send>>, Self::Err> {
        Ok(Box::pin(futures::stream::pending()))
    }

    fn is_payment_event_stream_active(&self) -> bool {
        false
    }

    fn cancel_payment_event_stream(&self) {}

    async fn check_incoming_payment_status(
        &self,
        _payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        Err(payment::Error::UnsupportedPaymentOption)
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        Ok(MakePaymentResponse {
            payment_lookup_id: payment_identifier.clone(),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: Amount::new(0, CurrencyUnit::Sat),
        })
    }
}

struct CountingBackendAgnosticPaymentProcessor {
    make_payment_calls: Arc<AtomicUsize>,
    first_payment_started: Option<Arc<Semaphore>>,
    release_first_payment: Option<Arc<Semaphore>>,
}

struct QuoteCountingBackendAgnosticPaymentProcessor {
    quote_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl MintPayment for QuoteCountingBackendAgnosticPaymentProcessor {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        BackendAgnosticPaymentProcessor.get_settings().await
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .create_incoming_payment_request(options)
            .await
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        self.quote_calls.fetch_add(1, Ordering::Relaxed);
        BackendAgnosticPaymentProcessor
            .get_payment_quote(unit, options)
            .await
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .make_payment(unit, options)
            .await
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Event> + Send>>, Self::Err> {
        BackendAgnosticPaymentProcessor.wait_payment_event().await
    }

    fn is_payment_event_stream_active(&self) -> bool {
        BackendAgnosticPaymentProcessor.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        BackendAgnosticPaymentProcessor.cancel_payment_event_stream();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        BackendAgnosticPaymentProcessor
            .check_incoming_payment_status(payment_identifier)
            .await
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .check_outgoing_payment(payment_identifier)
            .await
    }
}

#[async_trait]
impl MintPayment for CountingBackendAgnosticPaymentProcessor {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        BackendAgnosticPaymentProcessor.get_settings().await
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .create_incoming_payment_request(options)
            .await
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .get_payment_quote(unit, options)
            .await
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let call_index = self.make_payment_calls.fetch_add(1, Ordering::Relaxed);
        if call_index == 0 {
            if let (Some(started), Some(release)) =
                (&self.first_payment_started, &self.release_first_payment)
            {
                started.add_permits(1);
                release
                    .acquire()
                    .await
                    .map_err(|_| payment::Error::Custom("payment release closed".to_string()))?
                    .forget();
            }
        }
        BackendAgnosticPaymentProcessor
            .make_payment(unit, options)
            .await
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Event> + Send>>, Self::Err> {
        BackendAgnosticPaymentProcessor.wait_payment_event().await
    }

    fn is_payment_event_stream_active(&self) -> bool {
        BackendAgnosticPaymentProcessor.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        BackendAgnosticPaymentProcessor.cancel_payment_event_stream();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        BackendAgnosticPaymentProcessor
            .check_incoming_payment_status(payment_identifier)
            .await
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .check_outgoing_payment(payment_identifier)
            .await
    }
}

async fn federated_test_mint_with_backend_agnostic_payment(
    runtime_config: FederationRuntimeConfig,
) -> (Mint, Arc<MintSqliteDatabase>) {
    federated_test_mint_with_payment_processor(
        runtime_config,
        Arc::new(BackendAgnosticPaymentProcessor),
    )
    .await
}

async fn standalone_test_mint_with_backend_agnostic_payment() -> (Mint, Arc<MintSqliteDatabase>) {
    let localstore = Arc::new(memory::empty().await.expect("mint db"));
    let mut builder = MintBuilder::new(localstore.clone());
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            Arc::new(BackendAgnosticPaymentProcessor),
        )
        .await
        .expect("payment processor");
    let mint = builder
        .build_with_seed(localstore.clone(), &[9; 64])
        .await
        .expect("mint");
    (mint, localstore)
}

async fn federated_test_mint_with_counting_payment_processor(
    runtime_config: FederationRuntimeConfig,
) -> (Mint, Arc<MintSqliteDatabase>, Arc<AtomicUsize>) {
    let make_payment_calls = Arc::new(AtomicUsize::new(0));
    let payment_processor = Arc::new(CountingBackendAgnosticPaymentProcessor {
        make_payment_calls: Arc::clone(&make_payment_calls),
        first_payment_started: None,
        release_first_payment: None,
    });
    let (mint, localstore) =
        federated_test_mint_with_payment_processor(runtime_config, payment_processor).await;

    (mint, localstore, make_payment_calls)
}

async fn federated_test_mint_with_blocking_payment_processor(
    runtime_config: FederationRuntimeConfig,
) -> (
    Mint,
    Arc<MintSqliteDatabase>,
    Arc<AtomicUsize>,
    Arc<Semaphore>,
    Arc<Semaphore>,
) {
    let make_payment_calls = Arc::new(AtomicUsize::new(0));
    let first_payment_started = Arc::new(Semaphore::new(0));
    let release_first_payment = Arc::new(Semaphore::new(0));
    let payment_processor = Arc::new(CountingBackendAgnosticPaymentProcessor {
        make_payment_calls: Arc::clone(&make_payment_calls),
        first_payment_started: Some(Arc::clone(&first_payment_started)),
        release_first_payment: Some(Arc::clone(&release_first_payment)),
    });
    let (mint, localstore) =
        federated_test_mint_with_payment_processor(runtime_config, payment_processor).await;

    (
        mint,
        localstore,
        make_payment_calls,
        first_payment_started,
        release_first_payment,
    )
}

#[derive(Debug, Clone)]
struct ExpectedMeltReserve {
    quote: QuoteId,
    input_ys: Vec<PublicKey>,
}

struct ReserveCheckingPaymentProcessor {
    localstore: Arc<MintSqliteDatabase>,
    expected: Arc<Mutex<Option<ExpectedMeltReserve>>>,
    make_payment_calls: Arc<AtomicUsize>,
    payment_state: MeltQuoteState,
    payment_total_spent: u64,
}

impl ReserveCheckingPaymentProcessor {
    async fn assert_reserve_persisted_before_payment(&self) -> Result<(), payment::Error> {
        let expected = self.expected.lock().await.clone().ok_or_else(|| {
            payment::Error::Custom("missing melt reserve expectation".to_string())
        })?;

        let quote = self
            .localstore
            .get_melt_quote(&expected.quote)
            .await
            .map_err(|err| payment::Error::Custom(format!("melt quote lookup failed: {err}")))?
            .ok_or_else(|| payment::Error::Custom("melt quote missing".to_string()))?;
        if quote.state != MeltQuoteState::Pending {
            return Err(payment::Error::Custom(format!(
                "melt quote was not pending before payment attempt: {}",
                quote.state
            )));
        }

        let saga = self
            .localstore
            .get_melt_saga_by_quote_id(&expected.quote)
            .await
            .map_err(|err| payment::Error::Custom(format!("melt saga lookup failed: {err}")))?
            .ok_or_else(|| payment::Error::Custom("melt saga missing".to_string()))?;
        if saga.state != SagaStateEnum::Melt(MeltSagaState::PaymentAttempted) {
            return Err(payment::Error::Custom(format!(
                "melt saga did not record payment attempt before backend call: {}",
                saga.state.state()
            )));
        }

        let proof_states = self
            .localstore
            .get_proofs_states(&expected.input_ys)
            .await
            .map_err(|err| payment::Error::Custom(format!("proof state lookup failed: {err}")))?;
        let expected_states = vec![Some(State::Pending); expected.input_ys.len()];
        if proof_states != expected_states {
            return Err(payment::Error::Custom(format!(
                "proofs were not pending before payment attempt: {proof_states:?}"
            )));
        }

        Ok(())
    }
}

#[async_trait]
impl MintPayment for ReserveCheckingPaymentProcessor {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        BackendAgnosticPaymentProcessor.get_settings().await
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .create_incoming_payment_request(options)
            .await
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .get_payment_quote(unit, options)
            .await
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.assert_reserve_persisted_before_payment().await?;
        self.make_payment_calls.fetch_add(1, Ordering::Relaxed);
        let mut response = BackendAgnosticPaymentProcessor
            .make_payment(unit, options)
            .await?;
        response.status = self.payment_state;
        response.total_spent = Amount::new(self.payment_total_spent, unit.clone());
        Ok(response)
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Event> + Send>>, Self::Err> {
        BackendAgnosticPaymentProcessor.wait_payment_event().await
    }

    fn is_payment_event_stream_active(&self) -> bool {
        BackendAgnosticPaymentProcessor.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        BackendAgnosticPaymentProcessor.cancel_payment_event_stream();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        BackendAgnosticPaymentProcessor
            .check_incoming_payment_status(payment_identifier)
            .await
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .check_outgoing_payment(payment_identifier)
            .await
    }
}

async fn federated_test_mint_with_reserve_checking_payment_processor(
    runtime_config: FederationRuntimeConfig,
) -> (
    Mint,
    Arc<MintSqliteDatabase>,
    Arc<Mutex<Option<ExpectedMeltReserve>>>,
    Arc<AtomicUsize>,
) {
    federated_test_mint_with_reserve_checking_payment_response(
        runtime_config,
        MeltQuoteState::Pending,
        0,
    )
    .await
}

async fn federated_test_mint_with_reserve_checking_payment_response(
    runtime_config: FederationRuntimeConfig,
    payment_state: MeltQuoteState,
    payment_total_spent: u64,
) -> (
    Mint,
    Arc<MintSqliteDatabase>,
    Arc<Mutex<Option<ExpectedMeltReserve>>>,
    Arc<AtomicUsize>,
) {
    let localstore = Arc::new(memory::empty().await.expect("mint db"));
    let expected = Arc::new(Mutex::new(None));
    let make_payment_calls = Arc::new(AtomicUsize::new(0));
    let payment_processor = Arc::new(ReserveCheckingPaymentProcessor {
        localstore: Arc::clone(&localstore),
        expected: Arc::clone(&expected),
        make_payment_calls: Arc::clone(&make_payment_calls),
        payment_state,
        payment_total_spent,
    });
    let mut builder = MintBuilder::new(localstore.clone()).with_federation_config(runtime_config);
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            payment_processor,
        )
        .await
        .expect("payment processor");
    let mint = builder
        .build_with_seed(localstore.clone(), &[9; 64])
        .await
        .expect("mint");

    (mint, localstore, expected, make_payment_calls)
}

fn appended_entry(
    journal: &mut InMemoryFederationJournal,
    envelope: FederationOperationEnvelope,
) -> FederationJournalEntry {
    journal.append(envelope).expect("append").entry().clone()
}

fn accepted_entry_without_journal(
    index: u64,
    envelope: FederationOperationEnvelope,
) -> FederationJournalEntry {
    let operation_id = envelope.operation_id().expect("operation id");
    let conflict_keys = envelope.operation.conflict_keys();
    let previous_order_digest = FederationJournalDigest::from_bytes([index as u8; 32]);
    let order_digest = FederationJournalDigest::from_bytes([(index + 1) as u8; 32]);
    FederationJournalEntry {
        index,
        operation_id,
        conflict_keys,
        envelope,
        previous_order_digest,
        order_digest,
        consensus_evidence: None,
        application: FederationApplicationState::Accepted {},
    }
}

async fn persist_and_apply_entry(mint: &Mint, entry: &FederationJournalEntry) {
    let localstore = mint.localstore();
    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.add_federation_journal_entry(entry)
        .await
        .expect("persist journal entry");
    tx.commit().await.expect("commit journal entry");

    mint.apply_federation_journal_entries(std::slice::from_ref(entry))
        .await
        .expect("apply journal entry");
}

async fn persist_journal_entry(
    localstore: &Arc<MintSqliteDatabase>,
    entry: &FederationJournalEntry,
) {
    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.add_federation_journal_entry(entry)
        .await
        .expect("persist journal entry");
    tx.commit().await.expect("commit journal entry");
}

#[derive(Debug, Clone)]
struct MeltPaymentObservationFixture<'a> {
    observer: u16,
    state: MeltQuoteState,
    amount: Amount,
    fee_reserve: Amount,
    payment_id: Option<PaymentIdentifier>,
    payment_proof: Option<&'a str>,
}

fn melt_payment_observation_envelope(
    runtime_config: &FederationRuntimeConfig,
    quote: &QuoteId,
    fixture: MeltPaymentObservationFixture<'_>,
) -> FederationOperationEnvelope {
    runtime_config.operation_envelope(FederationOperation::MeltQuotePayment(
        FederationMeltQuotePaymentObservation {
            quote: quote.to_string(),
            observer: MemberId::new(fixture.observer).expect("observer"),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: fixture.state,
            amount: fixture.amount,
            fee_reserve: fixture.fee_reserve,
            payment_id: fixture.payment_id,
            payment_proof: fixture.payment_proof.map(str::to_string),
        },
    ))
}

fn fake_melt_quote_request(amount_msat: u64, payment_state: MeltQuoteState) -> MeltQuoteRequest {
    let description = cdk_fake_wallet::FakeInvoiceDescription {
        pay_invoice_state: payment_state,
        check_payment_state: payment_state,
        pay_err: false,
        check_err: false,
    };
    let invoice = cdk_fake_wallet::create_fake_invoice(
        amount_msat,
        serde_json::to_string(&description).expect("fake invoice description"),
    );

    MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: invoice,
        unit: CurrencyUnit::Sat,
        options: None,
    })
}

fn pending_melt_quote_request(amount_msat: u64) -> MeltQuoteRequest {
    fake_melt_quote_request(amount_msat, MeltQuoteState::Pending)
}

fn test_melt_quote_response(
    quote: &QuoteId,
    request: &MeltQuoteRequest,
) -> MeltQuoteCreateResponse<QuoteId> {
    match request {
        MeltQuoteRequest::Bolt11(request) => {
            MeltQuoteCreateResponse::Bolt11(MeltQuoteBolt11Response {
                quote: quote.clone(),
                amount: Amount::from(
                    request
                        .request
                        .amount_milli_satoshis()
                        .unwrap_or(1_000)
                        .div_ceil(1_000),
                ),
                fee_reserve: Amount::from(1),
                state: MeltQuoteState::Unpaid,
                expiry: 4_000_000_000,
                payment_preimage: None,
                change: None,
                request: Some(request.request.to_string()),
                unit: Some(request.unit.clone()),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
            })
        }
        MeltQuoteRequest::Custom(request) => MeltQuoteCreateResponse::Custom((
            PaymentMethod::from(request.method.as_str()),
            MeltQuoteCustomResponse {
                quote: quote.clone(),
                method: PaymentMethod::from(request.method.as_str()),
                amount: Amount::from(1),
                fee_reserve: Some(Amount::from(1)),
                state: MeltQuoteState::Unpaid,
                expiry: 4_000_000_000,
                payment_preimage: None,
                change: None,
                request: Some(request.request.clone()),
                unit: Some(request.unit.clone()),
                extra: serde_json::Value::Null,
            },
        )),
        _ => panic!("unsupported federation melt quote test fixture"),
    }
}

fn melt_quote_op(
    runtime_config: &FederationRuntimeConfig,
    quote: QuoteId,
    request: MeltQuoteRequest,
) -> FederationOperationEnvelope {
    let response = test_melt_quote_response(&quote, &request);
    runtime_config.melt_quote_operation(
        quote,
        request,
        response,
        FederationMeltQuoteMetadata::default(),
    )
}

fn locally_authored_melt_operation<T>(
    runtime_config: &FederationRuntimeConfig,
    candidate: impl FnOnce(u16) -> (MeltRequest<QuoteId>, T),
) -> (FederationOperationEnvelope, T) {
    let (request, fixture) = candidate(0);
    let envelope = runtime_config
        .melt_operation(request)
        .expect("melt operation");
    (envelope, fixture)
}

async fn setup_pending_federated_melt(
    mint: &Mint,
    localstore: &Arc<MintSqliteDatabase>,
    runtime_config: &FederationRuntimeConfig,
    journal: &mut InMemoryFederationJournal,
) -> (QuoteId, Vec<PublicKey>) {
    setup_pending_federated_melt_with_label(mint, localstore, runtime_config, journal, "melt").await
}

async fn setup_pending_federated_melt_with_label(
    mint: &Mint,
    localstore: &Arc<MintSqliteDatabase>,
    runtime_config: &FederationRuntimeConfig,
    journal: &mut InMemoryFederationJournal,
    label: &str,
) -> (QuoteId, Vec<PublicKey>) {
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        journal,
        melt_quote_op(
            runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
    );
    persist_and_apply_entry(mint, &quote_entry).await;

    let proofs = vec![
        proof_for_runtime(runtime_config, &format!("{label}-input-one")),
        proof_for_runtime(runtime_config, &format!("{label}-input-two")),
    ];
    let input_ys = proofs.ys().expect("melt input ys");
    let melt_envelope = runtime_config
        .melt_operation(MeltRequest::new(quote.clone(), proofs, None))
        .expect("melt operation");
    let melt_entry = appended_entry(journal, melt_envelope);
    persist_and_apply_entry(mint, &melt_entry).await;

    let pending_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(pending_quote.state, MeltQuoteState::Pending);
    assert_eq!(pending_quote.amount(), Amount::new(1, CurrencyUnit::Sat));
    assert_eq!(
        pending_quote.fee_reserve(),
        Amount::new(1, CurrencyUnit::Sat)
    );
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_some(),
        "pending federated melt should keep saga metadata"
    );

    let proof_states = localstore
        .get_proofs_states(&input_ys)
        .await
        .expect("proof states");
    assert_eq!(
        proof_states,
        vec![Some(State::Pending), Some(State::Pending)]
    );

    (quote, input_ys)
}

async fn apply_paid_melt_observation_quorum(
    mint: &Mint,
    runtime_config: &FederationRuntimeConfig,
    journal: &mut InMemoryFederationJournal,
    quote: &QuoteId,
    payment_proof: &str,
) {
    for observer in [1, 2] {
        let observation = appended_entry(
            journal,
            melt_payment_observation_envelope(
                runtime_config,
                quote,
                MeltPaymentObservationFixture {
                    observer,
                    state: MeltQuoteState::Paid,
                    amount: Amount::from(1),
                    fee_reserve: Amount::from(1),
                    payment_id: None,
                    payment_proof: Some(payment_proof),
                },
            ),
        );
        persist_and_apply_entry(mint, &observation).await;
    }
}

async fn set_melt_quote_state(
    localstore: &Arc<MintSqliteDatabase>,
    quote: &QuoteId,
    state: MeltQuoteState,
) {
    let mut tx = localstore.begin_transaction().await.expect("begin");
    let mut stored_quote = tx
        .get_melt_quote(quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    tx.update_melt_quote_state(&mut stored_quote, state, None)
        .await
        .expect("update melt quote state");
    tx.commit().await.expect("commit melt quote state");
}

fn output_for_runtime(runtime_config: &FederationRuntimeConfig, label: &[u8]) -> BlindedMessage {
    let keyset = &runtime_config.public_config().keysets[0];
    output_for_keyset(keyset, label)
}

fn output_for_keyset(keyset: &FederatedKeyset, label: &[u8]) -> BlindedMessage {
    BlindedMessage::new(
        Amount::from(1),
        keyset.id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(label)),
    )
}

fn proof_for_runtime(runtime_config: &FederationRuntimeConfig, label: &str) -> Proof {
    let keyset = &runtime_config.public_config().keysets[0];
    proof_for_keyset(keyset, 2, label)
}

fn proof_for_keyset(keyset: &FederatedKeyset, aggregate_secret_seed: u8, label: &str) -> Proof {
    let secret = Secret::new(format!("federated-proof-{label}"));
    proof_for_secret(keyset, aggregate_secret_seed, secret)
}

fn proof_for_secret(keyset: &FederatedKeyset, aggregate_secret_seed: u8, secret: Secret) -> Proof {
    let aggregate_secret = BlsSecretKey::from_reduced_bytes(&[aggregate_secret_seed; 32]);
    let signature = BlsG1PublicKey::hash_to_curve(secret.as_bytes()).mul(&aggregate_secret);

    Proof::new(
        Amount::from(1),
        keyset.id,
        secret,
        PublicKey::from(signature),
    )
}

async fn assert_rejected_swap_left_no_materialization(
    localstore: &MintSqliteDatabase,
    operation_id: FederationOperationId,
    input_ys: &[PublicKey],
    outputs: &[BlindedMessage],
    context: &str,
) {
    let expected_empty_states = vec![None; input_ys.len()];
    assert_eq!(
        localstore
            .get_proofs_states(input_ys)
            .await
            .expect("rejected swap proof states"),
        expected_empty_states,
        "{context}: rejected swap must fail before proof state storage"
    );

    let blinded_secrets = outputs
        .iter()
        .map(|output| output.blinded_secret)
        .collect::<Vec<_>>();
    let expected_empty_signatures = vec![None; blinded_secrets.len()];
    assert_eq!(
        localstore
            .get_blind_signatures(&blinded_secrets)
            .await
            .expect("rejected swap blind signatures"),
        expected_empty_signatures,
        "{context}: rejected swap must fail before blind signature storage"
    );

    for output_index in 0..outputs.len() {
        assert!(
            localstore
                .get_federation_signature_shares_for_output(operation_id, output_index as u32)
                .await
                .expect("rejected swap output share lookup")
                .is_empty(),
            "{context}: rejected swap must fail before output share storage"
        );
    }

    for blinded_secret in blinded_secrets {
        assert!(
            localstore
                .get_federation_signature_shares_for_blinded_message(blinded_secret)
                .await
                .expect("rejected swap blinded-message share lookup")
                .is_empty(),
            "{context}: rejected swap must fail before blinded-message share storage"
        );
    }

    assert!(
        localstore
            .get_completed_operation(&super::federation_operation_uuid(operation_id))
            .await
            .expect("rejected swap completed operation lookup")
            .is_none(),
        "{context}: rejected swap must fail before completed-operation storage"
    );
}

async fn assert_rejected_melt_left_no_materialization(
    localstore: &MintSqliteDatabase,
    operation_id: FederationOperationId,
    quote: &QuoteId,
    input_ys: &[PublicKey],
    change_outputs: &[BlindedMessage],
    make_payment_calls: &AtomicUsize,
    context: &str,
) {
    let expected_empty_states = vec![None; input_ys.len()];
    assert_eq!(
        localstore
            .get_proofs_states(input_ys)
            .await
            .expect("rejected melt proof states"),
        expected_empty_states,
        "{context}: rejected melt must fail before proof state storage"
    );
    assert!(
        localstore
            .get_melt_saga_by_quote_id(quote)
            .await
            .expect("rejected melt saga lookup")
            .is_none(),
        "{context}: rejected melt must fail before saga storage"
    );

    let stored_quote = localstore
        .get_melt_quote(quote)
        .await
        .expect("rejected melt quote lookup")
        .expect("melt quote should remain stored");
    assert_eq!(
        stored_quote.state,
        MeltQuoteState::Unpaid,
        "{context}: rejected melt must leave the quote unpaid"
    );

    let blinded_secrets = change_outputs
        .iter()
        .map(|output| output.blinded_secret)
        .collect::<Vec<_>>();
    if !blinded_secrets.is_empty() {
        let expected_empty_signatures = vec![None; blinded_secrets.len()];
        assert_eq!(
            localstore
                .get_blind_signatures(&blinded_secrets)
                .await
                .expect("rejected melt change signatures"),
            expected_empty_signatures,
            "{context}: rejected melt must fail before change signature storage"
        );
    }

    for output_index in 0..change_outputs.len() {
        assert!(
            localstore
                .get_federation_signature_shares_for_output(operation_id, output_index as u32)
                .await
                .expect("rejected melt output share lookup")
                .is_empty(),
            "{context}: rejected melt must fail before output share storage"
        );
    }

    for blinded_secret in blinded_secrets {
        assert!(
            localstore
                .get_federation_signature_shares_for_blinded_message(blinded_secret)
                .await
                .expect("rejected melt blinded-message share lookup")
                .is_empty(),
            "{context}: rejected melt must fail before blinded-message share storage"
        );
    }

    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        0,
        "{context}: rejected melt must fail before payment attempt"
    );
}

fn p2pk_locked_proof_for_runtime(
    runtime_config: &FederationRuntimeConfig,
    sig_flag: SigFlag,
) -> (Proof, SecretKey) {
    let signing_key = SecretKey::generate();
    let secret: Secret = SpendingConditions::new_p2pk(
        signing_key.public_key(),
        Some(Conditions {
            sig_flag,
            ..Default::default()
        }),
    )
    .try_into()
    .expect("p2pk locked secret");

    (
        proof_for_secret(&runtime_config.public_config().keysets[0], 2, secret),
        signing_key,
    )
}

fn htlc_locked_proof_for_runtime(
    runtime_config: &FederationRuntimeConfig,
    preimage: &str,
    signing_key: &SecretKey,
    sig_flag: SigFlag,
) -> Proof {
    let secret: Secret = SpendingConditions::new_htlc(
        preimage.to_string(),
        Some(Conditions {
            pubkeys: Some(vec![signing_key.public_key()]),
            sig_flag,
            ..Default::default()
        }),
    )
    .expect("htlc locked condition")
    .try_into()
    .expect("htlc locked secret");

    proof_for_secret(&runtime_config.public_config().keysets[0], 2, secret)
}

fn non_v3_output_for_runtime(runtime_config: &FederationRuntimeConfig) -> BlindedMessage {
    let keyset = &runtime_config.public_config().keysets[0];
    let non_v3_keyset_id = Id::v2_from_data(
        &keyset.keys(),
        &keyset.unit,
        keyset.input_fee_ppk,
        keyset.final_expiry,
    );
    BlindedMessage::new(
        Amount::from(1),
        non_v3_keyset_id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"non-v3 mint output")),
    )
}

fn bolt11_quote_request(amount: u64) -> MintQuoteRequest {
    MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
        amount: Amount::from(amount),
        unit: CurrencyUnit::Sat,
        description: None,
        pubkey: None,
    })
}

fn locked_bolt11_quote_request(amount: u64, pubkey: PublicKey) -> MintQuoteRequest {
    MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
        amount: Amount::from(amount),
        unit: CurrencyUnit::Sat,
        description: None,
        pubkey: Some(pubkey),
    })
}

fn bolt11_quote_response(quote: &QuoteId, amount: u64) -> MintQuoteResponse<String> {
    MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
        quote: quote.to_string(),
        request: format!("invoice-{quote}"),
        amount: Some(Amount::from(amount)),
        unit: Some(CurrencyUnit::Sat),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
        amount_paid: Amount::ZERO,
        amount_issued: Amount::ZERO,
        updated_at: 0,
        state: MintQuoteState::Unpaid,
        expiry: None,
        pubkey: None,
    })
}

fn locked_bolt11_quote_response(
    quote: &QuoteId,
    amount: u64,
    pubkey: PublicKey,
) -> MintQuoteResponse<String> {
    MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
        quote: quote.to_string(),
        request: format!("invoice-{quote}"),
        amount: Some(Amount::from(amount)),
        unit: Some(CurrencyUnit::Sat),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
        amount_paid: Amount::ZERO,
        amount_issued: Amount::ZERO,
        updated_at: 0,
        state: MintQuoteState::Unpaid,
        expiry: None,
        pubkey: Some(pubkey),
    })
}

async fn persist_paid_locked_mint_quote(
    mint: &Mint,
    journal: &mut InMemoryFederationJournal,
    runtime_config: &FederationRuntimeConfig,
    quote: &QuoteId,
    pubkey: PublicKey,
    payment_proof: &str,
) {
    persist_paid_locked_mint_quote_amount(
        mint,
        journal,
        runtime_config,
        quote,
        1,
        pubkey,
        payment_proof,
    )
    .await;
}

async fn persist_paid_locked_mint_quote_amount(
    mint: &Mint,
    journal: &mut InMemoryFederationJournal,
    runtime_config: &FederationRuntimeConfig,
    quote: &QuoteId,
    amount: u64,
    pubkey: PublicKey,
    payment_proof: &str,
) {
    let quote_entry = appended_entry(
        journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            locked_bolt11_quote_request(amount, pubkey),
            locked_bolt11_quote_response(quote, amount, pubkey),
        ),
    );
    persist_and_apply_entry(mint, &quote_entry).await;

    let paid_entry = appended_entry(
        journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(amount),
            Amount::ZERO,
            Some(payment_proof.to_string()),
        ),
    );
    persist_and_apply_entry(mint, &paid_entry).await;
}

fn sign_batch_mint_request_for_quotes(
    request: &mut BatchMintRequest<QuoteId>,
    signing_keys: &[SecretKey],
) {
    let signatures = request
        .quotes
        .iter()
        .zip(signing_keys)
        .map(|(quote, signing_key)| {
            Some(
                request
                    .sign_quote(quote, signing_key)
                    .expect("batch NUT-20 quote signature"),
            )
        })
        .collect();
    request.signatures = Some(signatures);
}
