use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::common::FeeReserve;
use cdk_common::database::mint::{CompletedOperationsDatabase, SagaDatabase};
use cdk_common::database::{
    MintDatabase, MintFederationJournalDatabase, MintProofsDatabase, MintQuotesDatabase,
    MintSignaturesDatabase,
};
use cdk_common::error::Error;
use cdk_common::federation::{
    generate_trusted_dealer_configs, FederatedKeyset, FederationError,
    FederationJournalConsensusEvidence, FederationJournalDigest, FederationJournalEntry,
    FederationKeysetRotationOperation, FederationMeltOperation, FederationMeltQuoteOperation,
    FederationMeltQuotePaymentObservation, FederationMintOperation, FederationMintQuoteOperation,
    FederationMintQuotePaymentObservation, FederationMintRequest, FederationOperation,
    FederationOperationEnvelope, FederationOperationId, FederationOperationStatus,
    FederationPaymentObservationContext, FederationPaymentObservationPolicy,
    FederationPaymentObservationQuarantineReason, FederationPaymentTrustModel,
    FederationRestoreOperation, FederationSwapOperation, InMemoryFederationConsensus,
    InMemoryFederationJournal, MemberId, ThresholdParams, TrustedDealerKeysetSpec,
    TrustedDealerMember,
};
use cdk_common::mint::{MeltSagaState, SagaStateEnum};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut00::KnownMethod;
use cdk_common::nuts::nut01::{verify_blind_signature_share, BlsG1PublicKey, BlsSecretKey};
use cdk_common::nuts::nut05::MeltQuoteCustomRequest;
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, DynMintPayment, Event,
    IncomingPaymentOptions, MakePaymentResponse, MintPayment, OutgoingPaymentOptions,
    PaymentIdentifier, PaymentQuoteResponse, SettingsResponse, WaitPaymentResponse,
};
use cdk_common::quote_id::QuoteId;
use cdk_common::secret::Secret;
use cdk_common::{
    Amount, BatchMintRequest, BlindedMessage, Conditions, CurrencyUnit, Id, MeltQuoteBolt11Request,
    MeltQuoteRequest, MeltQuoteState, MeltRequest, MintQuoteBolt11Request, MintQuoteBolt11Response,
    MintQuoteRequest, MintQuoteResponse, MintQuoteState, MintRequest, PaymentMethod, Proof,
    ProofsMethods, PublicKey, RestoreRequest, SecretKey, SigFlag, SpendingConditions, State,
    SwapRequest,
};
use cdk_sqlite::mint::{memory, MintSqliteDatabase};
use tokio::sync::Mutex;

mod admission;
mod melt;
mod mint;
mod payment_observation;
mod swap;

use crate::mint::{Mint, MintBuilder, MintInput, MintMeltLimits};

use super::{FederationMode, FederationRuntimeConfig};

fn production_source(source: &str) -> &str {
    source
        .split_once("\n#[cfg(test)]\nmod tests;")
        .or_else(|| source.split_once("\n#[cfg(test)]\nmod tests {"))
        .map(|(production, _tests)| production)
        .expect("source file contains test module marker")
}

#[test]
fn production_federation_payment_paths_are_backend_agnostic() {
    let source = production_source(include_str!("../federation.rs"));

    for forbidden in [
        "cdk_fake_wallet",
        "FakeWallet",
        "fakewallet",
        "fake_wallet",
        "fake-wallet",
        "CLN",
        "Cln",
        "LND",
        "Lnd",
        "LNBits",
        "Lnbits",
        "LDK",
        "Ldk",
    ] {
        assert!(
                !source.contains(forbidden),
                "production federation mint code must not depend on concrete payment backend `{forbidden}`"
            );
    }

    for required in [
        "FederationMintQuotePaymentObservation",
        "FederationMeltQuotePaymentObservation",
        "payment_verification_policy",
        "MintQuoteState::Paid",
        "MeltQuoteState::Paid",
        "MakePaymentResponse",
        "payment_observation_record_from_entry",
        "federation_payment_observation_can_finalize",
    ] {
        assert!(
            source.contains(required),
            "production federation mint code should keep payment handling on generic `{required}`"
        );
    }
}

fn runtime_config() -> FederationRuntimeConfig {
    let member = TrustedDealerMember::new(
        cdk_common::federation::MemberId::new(1).expect("member id"),
        MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
        MintUrl::from_str("https://fed1.example.com").expect("federation api url"),
        SecretKey::from_slice(&[1; 32]).expect("identity secret"),
    );
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
        ThresholdParams::with_bft_consensus(1, 1).expect("params"),
        vec![member],
        vec![keyset],
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

fn runtime_config_with_inactive_rotation_keyset() -> FederationRuntimeConfig {
    let member = TrustedDealerMember::new(
        cdk_common::federation::MemberId::new(1).expect("member id"),
        MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
        MintUrl::from_str("https://fed1.example.com").expect("federation api url"),
        SecretKey::from_slice(&[1; 32]).expect("identity secret"),
    );
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
        ThresholdParams::with_bft_consensus(1, 1).expect("params"),
        vec![member],
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

#[tokio::test]
async fn verify_federation_inputs_rejects_oversized_secret_and_witness_content() {
    use cdk_common::nuts::{P2PKWitness, Witness};

    let runtime_config = runtime_config();
    let keyset_id = runtime_config.public_config().keysets[0].id;
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;

    let oversized_secret = Proof::new(
        Amount::from(1),
        keyset_id,
        Secret::new("s".repeat(crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1)),
        SecretKey::generate().public_key(),
    );
    let oversized_secret_proofs = vec![oversized_secret];
    let secret_err = mint
        .verify_federation_inputs_with_config(&runtime_config, &oversized_secret_proofs)
        .expect_err("oversized federation proof secret should fail");

    assert!(
        matches!(
            secret_err,
            Error::ProofContentTooLarge {
                actual,
                max: crate::mint::verification::MAX_PROOF_CONTENT_LEN,
            } if actual == crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1
        ),
        "expected oversized federation secret ProofContentTooLarge, got: {:?}",
        secret_err
    );

    let mut oversized_witness = Proof::new(
        Amount::from(1),
        keyset_id,
        Secret::new("small-secret"),
        SecretKey::generate().public_key(),
    );
    oversized_witness.witness = Some(Witness::P2PKWitness(P2PKWitness {
        signatures: vec!["s".repeat(crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1)],
    }));
    let oversized_witness_proofs = vec![oversized_witness];
    let witness_err = mint
        .verify_federation_inputs_with_config(&runtime_config, &oversized_witness_proofs)
        .expect_err("oversized federation proof witness should fail");

    assert!(
        matches!(
            witness_err,
            Error::ProofContentTooLarge {
                max: crate::mint::verification::MAX_PROOF_CONTENT_LEN,
                ..
            }
        ),
        "expected oversized federation witness ProofContentTooLarge, got: {:?}",
        witness_err
    );
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
        self.make_payment_calls.fetch_add(1, Ordering::Relaxed);
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

async fn federated_test_mint_with_counting_payment_processor(
    runtime_config: FederationRuntimeConfig,
) -> (Mint, Arc<MintSqliteDatabase>, Arc<AtomicUsize>) {
    let make_payment_calls = Arc::new(AtomicUsize::new(0));
    let payment_processor = Arc::new(CountingBackendAgnosticPaymentProcessor {
        make_payment_calls: Arc::clone(&make_payment_calls),
    });
    let (mint, localstore) =
        federated_test_mint_with_payment_processor(runtime_config, payment_processor).await;

    (mint, localstore, make_payment_calls)
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

async fn federated_test_mint_with_reserve_checking_payment_processor(
    runtime_config: FederationRuntimeConfig,
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
    let conflict_key = envelope.operation.conflict_key();
    let conflict_keys = envelope.operation.conflict_keys();
    FederationJournalEntry {
        index,
        operation_id,
        conflict_key,
        conflict_keys,
        envelope,
        previous_order_digest: FederationJournalDigest::from_bytes([index as u8; 32]),
        order_digest: FederationJournalDigest::from_bytes([(index + 1) as u8; 32]),
        operation_outcome_hash: None,
        state_digest: None,
        consensus_evidence: None,
        status: FederationOperationStatus::Accepted,
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
    failure_reason: Option<&'a str>,
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
            failure_reason: fixture.failure_reason.map(str::to_string),
            observed_time: None,
        },
    ))
}

fn operation_id_with_selector(selector: u64) -> FederationOperationId {
    let mut bytes = [0; 32];
    bytes[..8].copy_from_slice(&selector.to_be_bytes());
    FederationOperationId::from_bytes(bytes)
}

fn pending_melt_quote_request(amount_msat: u64) -> MeltQuoteRequest {
    let description = cdk_fake_wallet::FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
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

async fn setup_pending_federated_melt(
    mint: &Mint,
    localstore: &Arc<MintSqliteDatabase>,
    runtime_config: &FederationRuntimeConfig,
    journal: &mut InMemoryFederationJournal,
) -> (QuoteId, Vec<PublicKey>) {
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(mint, &quote_entry).await;

    let proofs = vec![
        proof_for_runtime(runtime_config, "melt-input-one"),
        proof_for_runtime(runtime_config, "melt-input-two"),
    ];
    let input_ys = proofs.ys().expect("melt input ys");
    let melt_request = MeltRequest::new(quote.clone(), proofs, None);
    let melt_entry = appended_entry(
        journal,
        runtime_config
            .melt_operation(melt_request)
            .expect("melt operation"),
    );
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
                    failure_reason: None,
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

#[test]
fn federation_mode_defaults_to_standalone() {
    assert_eq!(FederationMode::default(), FederationMode::Standalone);
    assert!(!FederationMode::default().is_member());
    assert!(FederationMode::Member.is_member());
}

fn assert_requires_consensus(err: Error, operation: &str) {
    let message = err.to_string();
    assert!(
        message.contains("requires consensus acceptance"),
        "unexpected member-mode guard error: {message}"
    );
    assert!(
        message.contains(operation),
        "guard error should name {operation}: {message}"
    );
}

#[tokio::test]
async fn federation_member_rejects_direct_standalone_value_paths() {
    let runtime_config = runtime_config();
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;

    let mint_input = MintInput::Single(MintRequest {
        quote: QuoteId::new(),
        outputs: vec![output_for_runtime(&runtime_config, b"direct member mint")],
        signature: None,
    });
    let err = mint
        .process_mint_request(mint_input)
        .await
        .expect_err("federation member must not use standalone mint");
    assert_requires_consensus(err, "mint issuance");

    let swap = SwapRequest::new(
        vec![proof_for_runtime(&runtime_config, "direct-member-swap")],
        vec![output_for_runtime(&runtime_config, b"direct member swap")],
    );
    let err = mint
        .process_swap_request(swap)
        .await
        .expect_err("federation member must not use standalone swap");
    assert_requires_consensus(err, "swap");

    let melt = MeltRequest::new(
        QuoteId::new(),
        vec![proof_for_runtime(&runtime_config, "direct-member-melt")],
        None,
    );
    let err = mint
        .melt(&melt)
        .await
        .expect_err("federation member must not use standalone melt");
    assert_requires_consensus(err, "melt");

    let err = mint
        .blind_sign(vec![output_for_runtime(
            &runtime_config,
            b"direct member raw signing",
        )])
        .await
        .expect_err("federation member must not use raw standalone signing");
    assert_requires_consensus(err, "raw blind signing");
}

#[tokio::test]
async fn apply_accepted_federation_operation_applies_accepted_and_applying_entries() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();

    let accepted_quote = QuoteId::new();
    let accepted_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            accepted_quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&accepted_quote, 1),
        ),
    );
    persist_journal_entry(&localstore, &accepted_entry).await;
    assert!(mint
        .apply_accepted_federation_operation(&accepted_entry)
        .await
        .expect("accepted apply"));
    assert!(localstore
        .get_mint_quote(&accepted_quote)
        .await
        .expect("accepted quote lookup")
        .is_some());

    let applying_quote = QuoteId::new();
    let mut applying_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            applying_quote.clone(),
            bolt11_quote_request(2),
            bolt11_quote_response(&applying_quote, 2),
        ),
    );
    applying_entry.status = FederationOperationStatus::Applying;
    persist_journal_entry(&localstore, &applying_entry).await;
    assert!(mint
        .apply_accepted_federation_operation(&applying_entry)
        .await
        .expect("applying apply"));
    assert!(localstore
        .get_mint_quote(&applying_quote)
        .await
        .expect("applying quote lookup")
        .is_some());
}

#[tokio::test]
async fn apply_accepted_federation_operation_skips_applied_entries() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let mut entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    entry.status = FederationOperationStatus::Applied;

    assert!(!mint
        .apply_accepted_federation_operation(&entry)
        .await
        .expect("applied entry skip"));
    assert!(localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .is_none());
}

#[tokio::test]
async fn apply_accepted_federation_operation_rejects_terminal_local_statuses() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;

    for status in [
        FederationOperationStatus::Rejected,
        FederationOperationStatus::Failed,
    ] {
        let mut journal = InMemoryFederationJournal::new();
        let quote = QuoteId::new();
        let mut entry = appended_entry(
            &mut journal,
            runtime_config.mint_quote_operation(
                quote.clone(),
                bolt11_quote_request(1),
                bolt11_quote_response(&quote, 1),
            ),
        );
        entry.status = status;

        let err = mint
            .apply_accepted_federation_operation(&entry)
            .await
            .expect_err("terminal local status must not apply");
        assert!(
            err.to_string().contains("is not accepted for apply"),
            "unexpected terminal-status error: {err}"
        );
        assert!(localstore
            .get_mint_quote(&quote)
            .await
            .expect("quote lookup")
            .is_none());
    }
}

#[test]
fn runtime_config_validates_public_and_private_configs() {
    let runtime_config = runtime_config();

    assert_eq!(
        runtime_config.federation_id(),
        runtime_config.public_config().federation_id
    );
    assert_eq!(runtime_config.member_id().get(), 1);
}

#[test]
fn runtime_config_keyset_rotation_requires_local_secret_share() {
    let mut runtime_config = runtime_config_with_inactive_rotation_keyset();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;
    runtime_config
        .private_config
        .keyset_secret_shares
        .remove(&next_keyset.id);

    let err = runtime_config
        .apply_keyset_rotation(&FederationKeysetRotationOperation {
            keyset: next_keyset,
        })
        .expect_err("rotation without local secret share must fail");

    assert!(matches!(
        err,
        Error::Federation(FederationError::MissingPrivateKeyset { .. })
    ));
}

#[test]
fn runtime_config_keyset_rotation_rejects_wrong_local_secret_share() {
    let mut runtime_config = runtime_config_with_inactive_rotation_keyset();
    let old_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("active keyset")
        .clone();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;
    let wrong_shares = runtime_config
        .private_config
        .keyset_secret_shares
        .get(&old_keyset.id)
        .expect("old keyset secret shares")
        .clone();
    runtime_config
        .private_config
        .keyset_secret_shares
        .insert(next_keyset.id, wrong_shares);

    let err = runtime_config
        .apply_keyset_rotation(&FederationKeysetRotationOperation {
            keyset: next_keyset,
        })
        .expect_err("rotation with wrong local secret share must fail");

    assert!(matches!(
        err,
        Error::Federation(FederationError::PrivateShareMismatch { .. })
    ));
}

#[test]
fn runtime_config_builds_mint_quote_consensus_operations() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let envelope = runtime_config.mint_quote_operation(
        quote.clone(),
        bolt11_quote_request(10),
        bolt11_quote_response(&quote, 10),
    );

    assert_eq!(envelope.federation_id, runtime_config.federation_id());
    let quote_string = quote.to_string();
    assert_eq!(envelope.operation.quote(), Some(quote_string.as_str()));
    assert!(matches!(
        envelope.operation,
        FederationOperation::MintQuote(_)
    ));

    let mut consensus = InMemoryFederationConsensus::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
    );
    let first_response = consensus
        .submit_envelope(envelope.clone())
        .expect("submit quote");
    let duplicate_response = consensus
        .submit_envelope(envelope)
        .expect("duplicate quote");
    let conflict = runtime_config.mint_quote_operation(
        quote.clone(),
        bolt11_quote_request(11),
        bolt11_quote_response(&quote, 11),
    );

    assert_eq!(first_response.operation_id, duplicate_response.operation_id);
    assert!(matches!(
        consensus.submit_envelope(conflict),
        Err(FederationError::ConflictingOperation { .. })
    ));
}

#[test]
fn runtime_config_admission_validates_quote_and_payment_observation_submissions() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let mint_quote = FederationMintQuoteOperation {
        quote: quote.to_string(),
        request: bolt11_quote_request(10),
        response: bolt11_quote_response(&quote, 10),
    };
    runtime_config
        .validate_federated_mint_quote_submission(&mint_quote)
        .expect("valid mint quote submission");

    let mut mismatched_response_quote = mint_quote.clone();
    mismatched_response_quote.response = bolt11_quote_response(&QuoteId::new(), 10);
    assert!(matches!(
        runtime_config.validate_federated_mint_quote_submission(&mismatched_response_quote),
        Err(Error::UnknownQuote)
    ));

    let mut empty_payment_request = mint_quote.clone();
    if let MintQuoteResponse::Bolt11(response) = &mut empty_payment_request.response {
        response.request.clear();
    }
    assert!(runtime_config
        .validate_federated_mint_quote_submission(&empty_payment_request)
        .is_err());

    let mint_observation = FederationMintQuotePaymentObservation {
        quote: quote.to_string(),
        observer: runtime_config.member_id(),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
        state: MintQuoteState::Paid,
        amount_paid: Amount::from(10),
        amount_issued: Amount::ZERO,
        payment_id: None,
        payment_proof: Some("settled".to_string()),
        observed_time: None,
    };
    runtime_config
        .validate_federated_mint_payment_observation_submission(&mint_observation)
        .expect("valid mint payment observation");

    let mut unknown_mint_observer = mint_observation.clone();
    unknown_mint_observer.observer = MemberId::new(99).expect("unknown member");
    assert!(matches!(
        runtime_config
            .validate_federated_mint_payment_observation_submission(&unknown_mint_observer),
        Err(Error::Federation(FederationError::UnknownMember { .. }))
    ));

    let mut invalid_mint_method = mint_observation.clone();
    invalid_mint_method.method = PaymentMethod::Custom("bad/method".to_string());
    assert!(matches!(
        runtime_config.validate_federated_mint_payment_observation_submission(&invalid_mint_method),
        Err(Error::InvalidPaymentMethod)
    ));

    let mut over_issued_mint = mint_observation.clone();
    over_issued_mint.amount_issued = Amount::from(11);
    assert!(runtime_config
        .validate_federated_mint_payment_observation_submission(&over_issued_mint)
        .is_err());

    let melt_quote = FederationMeltQuoteOperation {
        quote: quote.to_string(),
        request: pending_melt_quote_request(1_000),
    };
    runtime_config
        .validate_federated_melt_quote_submission(&melt_quote)
        .expect("valid melt quote submission");

    let empty_melt_quote = FederationMeltQuoteOperation {
        quote: quote.to_string(),
        request: MeltQuoteRequest::Custom(MeltQuoteCustomRequest {
            method: "custom".to_string(),
            request: String::new(),
            unit: CurrencyUnit::Sat,
            extra: serde_json::Value::Null,
        }),
    };
    assert!(runtime_config
        .validate_federated_melt_quote_submission(&empty_melt_quote)
        .is_err());

    let melt_observation = FederationMeltQuotePaymentObservation {
        quote: quote.to_string(),
        observer: runtime_config.member_id(),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
        state: MeltQuoteState::Paid,
        amount: Amount::from(9),
        fee_reserve: Amount::from(1),
        payment_id: None,
        payment_proof: Some("preimage".to_string()),
        failure_reason: None,
        observed_time: None,
    };
    runtime_config
        .validate_federated_melt_payment_observation_submission(&melt_observation)
        .expect("valid melt payment observation");

    let mut invalid_melt_method = melt_observation.clone();
    invalid_melt_method.method = PaymentMethod::Custom("bad/method".to_string());
    assert!(matches!(
        runtime_config.validate_federated_melt_payment_observation_submission(&invalid_melt_method),
        Err(Error::InvalidPaymentMethod)
    ));

    let quarantine = runtime_config.payment_observation_quarantine(
        FederationPaymentObservationContext::MintQuote,
        quote.clone(),
        FederationPaymentObservationQuarantineReason::ConflictingEvidence,
        vec![operation_id_with_selector(1)],
    );
    match &quarantine.operation {
        FederationOperation::PaymentObservationQuarantine(operation) => runtime_config
            .validate_federated_payment_observation_quarantine_submission(operation)
            .expect("valid quarantine submission"),
        operation => panic!("unexpected operation: {operation:?}"),
    }

    let empty_evidence = runtime_config.payment_observation_quarantine(
        FederationPaymentObservationContext::MintQuote,
        quote,
        FederationPaymentObservationQuarantineReason::ConflictingEvidence,
        Vec::new(),
    );
    match &empty_evidence.operation {
        FederationOperation::PaymentObservationQuarantine(operation) => assert!(runtime_config
            .validate_federated_payment_observation_quarantine_submission(operation)
            .is_err()),
        operation => panic!("unexpected operation: {operation:?}"),
    }
}

#[tokio::test]
async fn apply_federation_mint_quote_uses_consensus_logical_time() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(10),
            bolt11_quote_response(&quote, 10),
        ),
    );

    persist_and_apply_entry(&mint, &entry).await;

    let stored = localstore
        .get_mint_quote(&quote)
        .await
        .expect("mint quote lookup")
        .expect("mint quote");
    assert_eq!(
        stored.created_time,
        super::federation_accepted_logical_time(&entry)
    );
}

#[tokio::test]
async fn federation_mint_lifecycle_publishes_quote_events_after_commit() {
    let runtime_config = runtime_config();
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let mut subscription = mint
        .pubsub_manager()
        .subscribe(cdk_common::subscription::Params {
            kind: cdk_common::nut17::Kind::Bolt11MintQuote,
            filters: vec![quote.to_string()],
            id: Arc::new(cdk_common::subscription::SubId::from(
                "federation-mint-lifecycle",
            )),
        })
        .expect("subscribe to mint quote");

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let created_event =
        tokio::time::timeout(std::time::Duration::from_secs(1), subscription.recv())
            .await
            .expect("created quote event timeout")
            .expect("created quote event");
    match created_event.into_inner() {
        cdk_common::NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote, quote);
            assert_eq!(response.state, MintQuoteState::Unpaid);
        }
        payload => panic!("unexpected created quote payload: {payload:?}"),
    }

    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("federation-mint-lifecycle-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let paid_event = tokio::time::timeout(std::time::Duration::from_secs(1), subscription.recv())
        .await
        .expect("paid quote event timeout")
        .expect("paid quote event");
    match paid_event.into_inner() {
        cdk_common::NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote, quote);
            assert_eq!(response.state, MintQuoteState::Paid);
        }
        payload => panic!("unexpected paid quote payload: {payload:?}"),
    }

    let output = output_for_runtime(&runtime_config, b"federation mint lifecycle output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output],
                signature: None,
            }))
            .expect("mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let issued_event = tokio::time::timeout(std::time::Duration::from_secs(1), subscription.recv())
        .await
        .expect("issued quote event timeout")
        .expect("issued quote event");
    match issued_event.into_inner() {
        cdk_common::NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote, quote);
            assert_eq!(response.state, MintQuoteState::Issued);
        }
        payload => panic!("unexpected issued quote payload: {payload:?}"),
    }
}

#[tokio::test]
async fn apply_federation_keyset_rotation_marks_operation_applied() {
    let runtime_config = runtime_config();
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let keyset = runtime_config.public_config().keysets[0].clone();
    let entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation { keyset },
        )),
    );

    persist_and_apply_entry(&mint, &entry).await;

    let persisted = mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(
        persisted
            .iter()
            .find(|candidate| candidate.operation_id == entry.operation_id)
            .expect("rotation entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn effective_federation_config_replays_keyset_rotations() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let old_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("old active keyset")
        .clone();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;
    let entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation {
                keyset: next_keyset.clone(),
            },
        )),
    );

    persist_and_apply_entry(&mint, &entry).await;

    let effective = mint
        .effective_federation_config()
        .await
        .expect("effective config")
        .expect("federation config");
    assert!(
        !effective
            .public_config()
            .keysets
            .iter()
            .find(|keyset| keyset.id == old_keyset.id)
            .expect("old keyset")
            .active
    );
    assert!(
        effective
            .public_config()
            .keysets
            .iter()
            .find(|keyset| keyset.id == next_keyset.id)
            .expect("next keyset")
            .active
    );
    assert!(matches!(
        effective.validate_output_keysets(&[output_for_keyset(
            &old_keyset,
            b"old output after rotation"
        )]),
        Err(cdk_common::Error::Federation(
            FederationError::InactiveFederatedKeyset { .. }
        ))
    ));
    effective
        .validate_output_keysets(&[output_for_keyset(
            &next_keyset,
            b"next output after rotation",
        )])
        .expect("next keyset signs after rotation");
    effective
        .verify_proofs(&vec![proof_for_keyset(&old_keyset, 2, "old proof")])
        .expect("old proofs remain verifiable after rotation");

    let second_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation {
                keyset: old_keyset.clone(),
            },
        )),
    );
    persist_and_apply_entry(&mint, &second_entry).await;

    let reverted = mint
        .effective_federation_config()
        .await
        .expect("reverted effective config")
        .expect("federation config");
    assert!(
        reverted
            .public_config()
            .keysets
            .iter()
            .find(|keyset| keyset.id == old_keyset.id)
            .expect("old keyset")
            .active
    );
    assert!(
        !reverted
            .public_config()
            .keysets
            .iter()
            .find(|keyset| keyset.id == next_keyset.id)
            .expect("next keyset")
            .active
    );
}

#[tokio::test]
async fn effective_federation_keyset_responses_use_rotated_config() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let old_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("old active keyset")
        .clone();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;
    let rotation_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation {
                keyset: next_keyset.clone(),
            },
        )),
    );
    persist_and_apply_entry(&mint, &rotation_entry).await;

    let pubkeys = mint
        .effective_federation_pubkeys()
        .await
        .expect("effective pubkeys")
        .expect("federation pubkeys");
    let keysets = mint
        .effective_federation_keysets()
        .await
        .expect("effective keysets")
        .expect("federation keysets");
    let old_specific = mint
        .effective_federation_keyset_pubkeys(&old_keyset.id)
        .await
        .expect("old specific keyset")
        .expect("federation specific keyset");
    let next_specific = mint
        .effective_federation_keyset_pubkeys(&next_keyset.id)
        .await
        .expect("next specific keyset")
        .expect("federation specific keyset");

    assert_eq!(pubkeys.keysets.len(), 1);
    assert_eq!(pubkeys.keysets[0].id, next_keyset.id);
    assert_eq!(pubkeys.keysets[0].keys, next_keyset.keys());
    assert_eq!(keysets.keysets.len(), 2);
    assert!(
        !keysets
            .keysets
            .iter()
            .find(|keyset| keyset.id == old_keyset.id)
            .expect("old keyset response")
            .active
    );
    assert!(
        keysets
            .keysets
            .iter()
            .find(|keyset| keyset.id == next_keyset.id)
            .expect("next keyset response")
            .active
    );
    assert_eq!(old_specific.keysets[0].id, old_keyset.id);
    assert_eq!(old_specific.keysets[0].active, Some(false));
    assert_eq!(next_specific.keysets[0].id, next_keyset.id);
    assert_eq!(next_specific.keysets[0].active, Some(true));
}

#[tokio::test]
async fn restore_returns_old_keyset_signature_after_rotation() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let old_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("old active keyset")
        .clone();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;

    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(2),
            bolt11_quote_response(&quote, 2),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(2),
            Amount::ZERO,
            Some("old-restore-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let old_outputs = vec![
        output_for_keyset(&old_keyset, b"old restore output zero"),
        output_for_keyset(&old_keyset, b"old restore output one"),
    ];
    let input = MintInput::Single(MintRequest {
        quote: quote.clone(),
        outputs: old_outputs.clone(),
        signature: None,
    });
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(input)
            .expect("old mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let rotation_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation {
                keyset: next_keyset,
            },
        )),
    );
    persist_and_apply_entry(&mint, &rotation_entry).await;
    let effective = mint
        .effective_federation_config()
        .await
        .expect("effective config")
        .expect("federation config");
    assert!(matches!(
        effective.validate_output_keysets(&old_outputs),
        Err(cdk_common::Error::Federation(
            FederationError::InactiveFederatedKeyset { .. }
        ))
    ));

    let restored = mint
        .restore(RestoreRequest {
            outputs: vec![old_outputs[1].clone()],
        })
        .await
        .expect("restore old signature");
    assert_eq!(restored.outputs, vec![old_outputs[1].clone()]);
    assert_eq!(restored.signatures.len(), 1);
    assert_eq!(restored.signatures[0].keyset_id, old_keyset.id);

    let share_records = localstore
        .get_federation_signature_shares_for_blinded_message(restored.outputs[0].blinded_secret)
        .await
        .expect("stored federation restore signature share");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, mint_entry.operation_id);
    assert_eq!(share_records[0].output_index, 1);
    assert_eq!(share_records[0].keyset_id, old_keyset.id);
    assert_eq!(
        share_records[0].accepted_operation_index,
        Some(mint_entry.index)
    );
    assert_eq!(
        share_records[0].consensus_evidence,
        mint_entry.consensus_evidence
    );
    assert_eq!(
        restored.signatures[0].c,
        PublicKey::from(share_records[0].share.signature())
    );

    let operation_output_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 1)
        .await
        .expect("stored federation restore signature share by operation output");
    assert_eq!(operation_output_records, share_records);
}
