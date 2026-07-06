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
    FederationJournalConsensusEvidence, FederationJournalEntry, FederationKeysetRotationOperation,
    FederationMeltOperation, FederationMeltQuoteOperation, FederationMeltQuotePaymentObservation,
    FederationMintOperation, FederationMintQuoteOperation, FederationMintQuotePaymentObservation,
    FederationMintRequest, FederationOperation, FederationOperationEnvelope, FederationOperationId,
    FederationOperationStatus, FederationPaymentObservationContext,
    FederationPaymentObservationPolicy, FederationPaymentObservationQuarantineReason,
    FederationPaymentTrustModel, FederationRestoreOperation, FederationSwapOperation,
    InMemoryFederationConsensus, InMemoryFederationJournal, MemberId, ThresholdParams,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
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

fn appended_entry(
    journal: &mut InMemoryFederationJournal,
    envelope: FederationOperationEnvelope,
) -> FederationJournalEntry {
    journal.append(envelope).expect("append").entry().clone()
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

fn mint_payment_observation_envelope(
    runtime_config: &FederationRuntimeConfig,
    quote: &QuoteId,
    observer: u16,
    amount_paid: u64,
    payment_proof: &str,
) -> FederationOperationEnvelope {
    runtime_config.operation_envelope(FederationOperation::MintQuotePayment(
        FederationMintQuotePaymentObservation {
            quote: quote.to_string(),
            observer: MemberId::new(observer).expect("observer"),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(amount_paid),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some(payment_proof.to_string()),
            observed_time: None,
        },
    ))
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

#[test]
fn runtime_config_builds_mint_payment_observation_operations() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let paid = runtime_config.mint_quote_payment_observation(
        quote.clone(),
        PaymentMethod::Known(KnownMethod::Bolt11),
        MintQuoteState::Paid,
        Amount::from(10),
        Amount::ZERO,
        Some("settled".to_string()),
    );

    assert_eq!(paid.federation_id, runtime_config.federation_id());
    assert!(matches!(
        paid.operation,
        FederationOperation::MintQuotePayment(_)
    ));

    let mut consensus = InMemoryFederationConsensus::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
    );
    consensus.submit_envelope(paid).expect("submit paid");
    let conflicting_paid = runtime_config.mint_quote_payment_observation(
        quote,
        PaymentMethod::Known(KnownMethod::Bolt11),
        MintQuoteState::Paid,
        Amount::from(10),
        Amount::ZERO,
        Some("different-settlement".to_string()),
    );

    assert!(matches!(
        consensus.submit_envelope(conflicting_paid),
        Err(FederationError::ConflictingOperation { .. })
    ));
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
async fn apply_federation_mint_uses_rotated_keyset_for_signing() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
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

    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("rotated-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let effective = mint
        .effective_federation_config()
        .await
        .expect("effective config")
        .expect("federation config");
    let output = output_for_keyset(&next_keyset, b"rotated mint output");
    let input = MintInput::Single(MintRequest {
        quote: quote.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let mint_entry = appended_entry(
        &mut journal,
        effective.mint_operation(input).expect("mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored signatures");
    let signature = signatures
        .first()
        .and_then(Option::as_ref)
        .expect("rotated blind signature");
    assert_eq!(signature.keyset_id, next_keyset.id);
    assert_eq!(signature.amount, Amount::from(1));

    let share_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("stored federation signature shares");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, mint_entry.operation_id);
    assert_eq!(share_records[0].output_index, 0);
    assert_eq!(share_records[0].member_id, effective.member_id());
    assert_eq!(share_records[0].keyset_id, next_keyset.id);
    assert_eq!(share_records[0].amount, Amount::from(1));
    assert_eq!(share_records[0].blinded_secret, output.blinded_secret);
    assert_eq!(
        share_records[0].accepted_operation_index,
        Some(mint_entry.index)
    );
    assert_eq!(
        share_records[0].consensus_evidence,
        mint_entry.consensus_evidence
    );

    let blinded_message_share_records = localstore
        .get_federation_signature_shares_for_blinded_message(output.blinded_secret)
        .await
        .expect("stored federation signature shares by blinded message");
    assert_eq!(blinded_message_share_records, share_records);
}

#[tokio::test]
async fn restore_returns_persisted_accepted_mint_shares_by_original_output_index() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let keyset = runtime_config.public_config().keysets[0].clone();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(3),
            bolt11_quote_response(&quote, 3),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(3),
            Amount::ZERO,
            Some("restore-persisted-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let outputs = vec![
        output_for_keyset(&keyset, b"restore accepted mint output zero"),
        output_for_keyset(&keyset, b"restore accepted mint output one"),
        output_for_keyset(&keyset, b"restore accepted mint output two"),
    ];
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: outputs.clone(),
                signature: None,
            }))
            .expect("mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let restore_outputs = vec![outputs[2].clone(), outputs[0].clone()];
    let restored = mint
        .restore(RestoreRequest {
            outputs: restore_outputs.clone(),
        })
        .await
        .expect("restore accepted mint outputs");
    assert_eq!(restored.outputs, restore_outputs);
    assert_eq!(restored.signatures.len(), restored.outputs.len());

    for (restored_position, original_index) in [2_u32, 0_u32].into_iter().enumerate() {
        let restored_output = &restored.outputs[restored_position];
        let restored_signature = &restored.signatures[restored_position];
        let share_records = localstore
            .get_federation_signature_shares_for_output(mint_entry.operation_id, original_index)
            .await
            .expect("stored federation signature share by original output index");
        assert_eq!(share_records.len(), 1);
        let share_record = &share_records[0];
        assert_eq!(share_record.operation_id, mint_entry.operation_id);
        assert_eq!(share_record.output_index, original_index);
        assert_eq!(share_record.member_id, runtime_config.member_id());
        assert_eq!(share_record.keyset_id, keyset.id);
        assert_eq!(share_record.amount, Amount::from(1));
        assert_eq!(share_record.blinded_secret, restored_output.blinded_secret);
        assert_eq!(
            share_record.accepted_operation_index,
            Some(mint_entry.index)
        );
        assert_eq!(
            share_record.consensus_evidence,
            mint_entry.consensus_evidence
        );
        assert_eq!(restored_signature.keyset_id, keyset.id);
        assert_eq!(restored_signature.amount, Amount::from(1));
        assert_eq!(
            restored_signature.c,
            PublicKey::from(share_record.share.signature())
        );

        let blinded_message_share_records = localstore
            .get_federation_signature_shares_for_blinded_message(restored_output.blinded_secret)
            .await
            .expect("stored federation signature share by blinded message");
        assert_eq!(blinded_message_share_records, share_records);
    }
}

#[tokio::test]
async fn apply_federation_mint_rejects_unpaid_quote_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let output = output_for_runtime(&runtime_config, b"unpaid federation mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::UnpaidQuote)
    ));

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("unpaid mint signature lookup");
    assert_eq!(signatures.len(), 1);
    assert!(
        signatures.iter().all(Option::is_none),
        "unpaid quote must fail before blind signature storage"
    );

    let share_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("unpaid mint share lookup");
    assert!(
        share_records.is_empty(),
        "unpaid quote must fail before federation signature share storage"
    );

    let blinded_message_share_records = localstore
        .get_federation_signature_shares_for_blinded_message(output.blinded_secret)
        .await
        .expect("unpaid blinded-message share lookup");
    assert!(
        blinded_message_share_records.is_empty(),
        "unpaid quote must fail before blinded-message share storage"
    );

    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Unpaid);

    let stored_entries = localstore
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    let stored_mint_entry = stored_entries
        .iter()
        .find(|entry| entry.operation_id == mint_entry.operation_id)
        .expect("stored mint entry");
    assert_eq!(
        stored_mint_entry.status,
        FederationOperationStatus::Accepted
    );
}

#[tokio::test]
async fn apply_federation_mint_rejects_expired_quote_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let expiry = 1;
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                quote: quote.to_string(),
                request: format!("invoice-{quote}"),
                amount: Some(Amount::from(1)),
                unit: Some(CurrencyUnit::Sat),
                state: MintQuoteState::Unpaid,
                expiry: Some(expiry),
                pubkey: None,
            }),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("expired-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let output = output_for_runtime(&runtime_config, b"expired federation mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("expired mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::ExpiredQuote(expired_at, accepted_time))
            if expired_at == expiry && accepted_time > expiry
    ));

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("expired mint signature lookup");
    assert_eq!(
        signatures,
        vec![None],
        "expired quote must fail before blind signature storage"
    );
    let share_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("expired mint share lookup");
    assert!(
        share_records.is_empty(),
        "expired quote must fail before federation share storage"
    );
    let blinded_message_share_records = localstore
        .get_federation_signature_shares_for_blinded_message(output.blinded_secret)
        .await
        .expect("expired blinded-message share lookup");
    assert!(
        blinded_message_share_records.is_empty(),
        "expired quote must fail before blinded-message share storage"
    );
    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Paid);
    assert_eq!(
        stored_quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
    assert!(
        localstore
            .get_completed_operations()
            .await
            .expect("completed operation lookup")
            .is_empty(),
        "expired quote must fail before completed-operation storage"
    );
}

#[tokio::test]
async fn apply_federation_mint_rejects_duplicate_outputs_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
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
            Some("duplicate-output-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let output = output_for_runtime(&runtime_config, b"duplicate federation mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone(), output.clone()],
                signature: None,
            }))
            .expect("duplicate-output mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::DuplicateOutputs)
    ));

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("duplicate-output mint signature lookup");
    assert_eq!(
        signatures,
        vec![None],
        "duplicate outputs must fail before blind signature storage"
    );
    for output_index in [0, 1] {
        let share_records = localstore
            .get_federation_signature_shares_for_output(mint_entry.operation_id, output_index)
            .await
            .expect("duplicate-output mint share lookup");
        assert!(
            share_records.is_empty(),
            "duplicate outputs must fail before federation share storage"
        );
    }
    let blinded_message_share_records = localstore
        .get_federation_signature_shares_for_blinded_message(output.blinded_secret)
        .await
        .expect("duplicate-output blinded-message share lookup");
    assert!(
        blinded_message_share_records.is_empty(),
        "duplicate outputs must fail before blinded-message share storage"
    );
    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Paid);
    assert_eq!(
        stored_quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
    assert!(
        localstore
            .get_completed_operations()
            .await
            .expect("completed operation lookup")
            .is_empty(),
        "duplicate outputs must fail before completed-operation storage"
    );
}

#[tokio::test]
async fn apply_federation_batch_mint_rejects_quote_amount_mismatch_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
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
            Some("amount-mismatch-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let output = output_for_runtime(&runtime_config, b"amount mismatch federation mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Batch(BatchMintRequest {
                quotes: vec![quote.clone()],
                quote_amounts: Some(vec![Amount::from(2)]),
                outputs: vec![output.clone()],
                signatures: None,
            }))
            .expect("amount-mismatch batch mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::TransactionUnbalanced(2, 1, 0))
    ));

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("amount-mismatch mint signature lookup");
    assert_eq!(
        signatures,
        vec![None],
        "amount mismatch must fail before blind signature storage"
    );
    let share_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("amount-mismatch mint share lookup");
    assert!(
        share_records.is_empty(),
        "amount mismatch must fail before federation share storage"
    );
    let blinded_message_share_records = localstore
        .get_federation_signature_shares_for_blinded_message(output.blinded_secret)
        .await
        .expect("amount-mismatch blinded-message share lookup");
    assert!(
        blinded_message_share_records.is_empty(),
        "amount mismatch must fail before blinded-message share storage"
    );
    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Paid);
    assert_eq!(
        stored_quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
    assert!(
        localstore
            .get_completed_operations()
            .await
            .expect("completed operation lookup")
            .is_empty(),
        "amount mismatch must fail before completed-operation storage"
    );
}

#[tokio::test]
async fn apply_federation_mint_persists_accepted_materialization_rows() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("accepted-materialization-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let output = output_for_runtime(&runtime_config, b"accepted materialized mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("accepted materialization mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Issued);
    assert_eq!(
        stored_quote.amount_issued(),
        Amount::new(1, CurrencyUnit::Sat)
    );

    let operation_uuid = super::federation_operation_uuid(mint_entry.operation_id);
    let completed_operation = localstore
        .get_completed_operation(&operation_uuid)
        .await
        .expect("completed operation lookup")
        .expect("completed operation");
    assert_eq!(
        completed_operation.kind(),
        cdk_common::mint::OperationKind::Mint
    );
    assert_eq!(completed_operation.total_issued(), Amount::from(1));
    assert_eq!(completed_operation.total_redeemed(), Amount::ZERO);
    assert_eq!(completed_operation.fee_collected(), Amount::ZERO);
    assert_eq!(
        completed_operation.payment_method(),
        Some(PaymentMethod::Known(KnownMethod::Bolt11))
    );

    let blinded_secrets = localstore
        .get_blinded_secrets_by_operation_id(&operation_uuid)
        .await
        .expect("blinded secrets by operation id");
    assert_eq!(blinded_secrets, vec![output.blinded_secret]);

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored blind signature");
    let signature = signatures[0].as_ref().expect("accepted mint signature");
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, output.keyset_id);

    let share_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("stored federation signature share");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, mint_entry.operation_id);
    assert_eq!(share_records[0].output_index, 0);
    assert_eq!(share_records[0].member_id, runtime_config.member_id());
    assert_eq!(share_records[0].keyset_id, output.keyset_id);
    assert_eq!(share_records[0].amount, Amount::from(1));
    assert_eq!(share_records[0].blinded_secret, output.blinded_secret);
    assert_eq!(
        share_records[0].accepted_operation_index,
        Some(mint_entry.index)
    );
    assert_eq!(
        share_records[0].consensus_evidence,
        mint_entry.consensus_evidence
    );

    let stored_entries = localstore
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    let stored_mint_entry = stored_entries
        .iter()
        .find(|entry| entry.operation_id == mint_entry.operation_id)
        .expect("stored mint entry");
    assert_eq!(stored_mint_entry.status, FederationOperationStatus::Applied);
}

#[tokio::test]
async fn apply_federation_mint_accepts_nut20_locked_quote() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let signing_key = SecretKey::generate();
    let quote = QuoteId::new();
    persist_paid_locked_mint_quote(
        &mint,
        &mut journal,
        &runtime_config,
        &quote,
        signing_key.public_key(),
        "nut20-valid-payment",
    )
    .await;

    let output = output_for_runtime(&runtime_config, b"nut20 valid mint output");
    let mut request = MintRequest {
        quote: quote.clone(),
        outputs: vec![output.clone()],
        signature: None,
    };
    request.sign(&signing_key).expect("nut20 signature");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(request))
            .expect("locked mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored locked mint signature");
    assert!(
        signatures[0].is_some(),
        "valid NUT-20 locked quote must receive a member share"
    );
    let quote_state = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(quote_state.state(), MintQuoteState::Issued);
    assert_eq!(
        quote_state.amount_issued(),
        Amount::new(1, CurrencyUnit::Sat)
    );
}

#[tokio::test]
async fn apply_federation_mint_rejects_missing_nut20_signature_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let signing_key = SecretKey::generate();
    let quote = QuoteId::new();
    persist_paid_locked_mint_quote(
        &mint,
        &mut journal,
        &runtime_config,
        &quote,
        signing_key.public_key(),
        "nut20-missing-payment",
    )
    .await;

    let output = output_for_runtime(&runtime_config, b"nut20 missing signature output");
    let input = MintInput::Single(MintRequest {
        quote: quote.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(input)
            .expect("missing signature mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(cdk_common::Error::SignatureMissingOrInvalid)
    ));

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("missing signature output lookup");
    assert_eq!(
        signatures,
        vec![None],
        "missing NUT-20 signature must fail before any member share is stored"
    );
}

#[tokio::test]
async fn apply_federation_mint_rejects_nut20_output_mutation_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let signing_key = SecretKey::generate();
    let quote = QuoteId::new();
    persist_paid_locked_mint_quote(
        &mint,
        &mut journal,
        &runtime_config,
        &quote,
        signing_key.public_key(),
        "nut20-mutated-payment",
    )
    .await;

    let signed_output = output_for_runtime(&runtime_config, b"nut20 signed mint output");
    let mutated_output = output_for_runtime(&runtime_config, b"nut20 mutated mint output");
    let mut request = MintRequest {
        quote: quote.clone(),
        outputs: vec![signed_output.clone()],
        signature: None,
    };
    request.sign(&signing_key).expect("nut20 signature");
    request.outputs = vec![mutated_output.clone()];
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(request))
            .expect("mutated locked mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(cdk_common::Error::SignatureMissingOrInvalid)
    ));

    let signatures = localstore
        .get_blind_signatures(&[signed_output.blinded_secret, mutated_output.blinded_secret])
        .await
        .expect("mutated signature lookup");
    assert_eq!(
        signatures,
        vec![None, None],
        "mutated NUT-20 output set must fail before any member share is stored"
    );
}

#[tokio::test]
async fn apply_federation_batch_mint_accepts_nut20_locked_quotes() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let first_key = SecretKey::generate();
    let second_key = SecretKey::generate();
    let first_quote = QuoteId::new();
    let second_quote = QuoteId::new();
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &first_quote,
        1,
        first_key.public_key(),
        "batch-nut20-first-payment",
    )
    .await;
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &second_quote,
        1,
        second_key.public_key(),
        "batch-nut20-second-payment",
    )
    .await;

    let outputs = vec![
        output_for_runtime(&runtime_config, b"batch nut20 first output"),
        output_for_runtime(&runtime_config, b"batch nut20 second output"),
    ];
    let mut request = BatchMintRequest {
        quotes: vec![first_quote.clone(), second_quote.clone()],
        quote_amounts: None,
        outputs: outputs.clone(),
        signatures: None,
    };
    sign_batch_mint_request_for_quotes(&mut request, &[first_key, second_key]);
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Batch(request))
            .expect("batch locked mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let blinded_secrets = outputs
        .iter()
        .map(|output| output.blinded_secret)
        .collect::<Vec<_>>();
    let signatures = localstore
        .get_blind_signatures(&blinded_secrets)
        .await
        .expect("stored batch locked mint signatures");
    assert_eq!(signatures.len(), 2);
    assert!(
        signatures.iter().all(Option::is_some),
        "valid batch NUT-20 locked quotes must receive member shares"
    );
    for quote in [&first_quote, &second_quote] {
        let quote_state = localstore
            .get_mint_quote(quote)
            .await
            .expect("quote lookup")
            .expect("quote");
        assert_eq!(quote_state.state(), MintQuoteState::Issued);
        assert_eq!(
            quote_state.amount_issued(),
            Amount::new(1, CurrencyUnit::Sat)
        );
    }
}

#[tokio::test]
async fn apply_federation_batch_mint_rejects_missing_nut20_signature_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let first_key = SecretKey::generate();
    let second_key = SecretKey::generate();
    let first_quote = QuoteId::new();
    let second_quote = QuoteId::new();
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &first_quote,
        1,
        first_key.public_key(),
        "batch-nut20-missing-first-payment",
    )
    .await;
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &second_quote,
        1,
        second_key.public_key(),
        "batch-nut20-missing-second-payment",
    )
    .await;

    let outputs = vec![
        output_for_runtime(&runtime_config, b"batch nut20 missing first output"),
        output_for_runtime(&runtime_config, b"batch nut20 missing second output"),
    ];
    let mut request = BatchMintRequest {
        quotes: vec![first_quote.clone(), second_quote.clone()],
        quote_amounts: None,
        outputs: outputs.clone(),
        signatures: None,
    };
    sign_batch_mint_request_for_quotes(&mut request, &[first_key, second_key]);
    if let Some(signatures) = request.signatures.as_mut() {
        signatures[1] = None;
    }
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Batch(request))
            .expect("batch missing signature mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(cdk_common::Error::SignatureMissingOrInvalid)
    ));
    let blinded_secrets = outputs
        .iter()
        .map(|output| output.blinded_secret)
        .collect::<Vec<_>>();
    let signatures = localstore
        .get_blind_signatures(&blinded_secrets)
        .await
        .expect("missing batch signature lookup");
    assert_eq!(
        signatures,
        vec![None, None],
        "batch missing NUT-20 signature must fail before any member share is stored"
    );
}

#[tokio::test]
async fn apply_federation_batch_mint_rejects_nut20_output_mutation_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let first_key = SecretKey::generate();
    let second_key = SecretKey::generate();
    let first_quote = QuoteId::new();
    let second_quote = QuoteId::new();
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &first_quote,
        1,
        first_key.public_key(),
        "batch-nut20-mutated-first-payment",
    )
    .await;
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &second_quote,
        1,
        second_key.public_key(),
        "batch-nut20-mutated-second-payment",
    )
    .await;

    let signed_outputs = vec![
        output_for_runtime(&runtime_config, b"batch nut20 signed first output"),
        output_for_runtime(&runtime_config, b"batch nut20 signed second output"),
    ];
    let mutated_outputs = vec![
        output_for_runtime(&runtime_config, b"batch nut20 mutated first output"),
        output_for_runtime(&runtime_config, b"batch nut20 mutated second output"),
    ];
    let mut request = BatchMintRequest {
        quotes: vec![first_quote.clone(), second_quote.clone()],
        quote_amounts: None,
        outputs: signed_outputs.clone(),
        signatures: None,
    };
    sign_batch_mint_request_for_quotes(&mut request, &[first_key, second_key]);
    request.outputs = mutated_outputs.clone();
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Batch(request))
            .expect("batch mutated signature mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(cdk_common::Error::SignatureMissingOrInvalid)
    ));
    let blinded_secrets = signed_outputs
        .iter()
        .chain(mutated_outputs.iter())
        .map(|output| output.blinded_secret)
        .collect::<Vec<_>>();
    let signatures = localstore
        .get_blind_signatures(&blinded_secrets)
        .await
        .expect("mutated batch signature lookup");
    assert_eq!(
        signatures,
        vec![None, None, None, None],
        "batch mutated NUT-20 output set must fail before any member share is stored"
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
async fn apply_federation_swap_spends_old_proofs_and_signs_rotated_outputs() {
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
    let rotation_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation {
                keyset: next_keyset.clone(),
            },
        )),
    );
    persist_and_apply_entry(&mint, &rotation_entry).await;

    let input = proof_for_keyset(&old_keyset, 2, "rotated swap input");
    let output = output_for_keyset(&next_keyset, b"rotated swap output");
    let swap = SwapRequest::new(vec![input.clone()], vec![output.clone()]);
    let effective = mint
        .effective_federation_config()
        .await
        .expect("effective config")
        .expect("federation config");
    let swap_entry = appended_entry(
        &mut journal,
        effective.swap_operation(swap).expect("swap operation"),
    );
    persist_and_apply_entry(&mint, &swap_entry).await;

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored signatures");
    let signature = signatures
        .first()
        .and_then(Option::as_ref)
        .expect("rotated swap blind signature");
    assert_eq!(signature.keyset_id, next_keyset.id);

    let share_records = localstore
        .get_federation_signature_shares_for_output(swap_entry.operation_id, 0)
        .await
        .expect("stored federation swap signature shares");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, swap_entry.operation_id);
    assert_eq!(share_records[0].output_index, 0);
    assert_eq!(share_records[0].member_id, effective.member_id());
    assert_eq!(share_records[0].keyset_id, next_keyset.id);
    assert_eq!(share_records[0].amount, Amount::from(1));
    assert_eq!(share_records[0].blinded_secret, output.blinded_secret);
    assert_eq!(
        share_records[0].accepted_operation_index,
        Some(swap_entry.index)
    );
    assert_eq!(
        share_records[0].consensus_evidence,
        swap_entry.consensus_evidence
    );

    let input_states = mint
        .check_state(&cdk_common::CheckStateRequest {
            ys: vec![input.y().expect("input y")],
        })
        .await
        .expect("input states");
    assert_eq!(input_states.states[0].state, State::Spent);
}

#[tokio::test]
async fn apply_federation_swap_persists_accepted_materialization_rows() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let input = proof_for_runtime(&runtime_config, "accepted materialized swap input");
    let input_y = input.y().expect("swap input y");
    let output = output_for_runtime(&runtime_config, b"accepted materialized swap output");
    let swap_entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(vec![input.clone()], vec![output.clone()]))
            .expect("accepted materialization swap operation"),
    );
    persist_and_apply_entry(&mint, &swap_entry).await;

    let operation_uuid = super::federation_operation_uuid(swap_entry.operation_id);
    let completed_operation = localstore
        .get_completed_operation(&operation_uuid)
        .await
        .expect("completed operation lookup")
        .expect("completed operation");
    assert_eq!(
        completed_operation.kind(),
        cdk_common::mint::OperationKind::Swap
    );
    assert_eq!(completed_operation.total_issued(), Amount::from(1));
    assert_eq!(completed_operation.total_redeemed(), Amount::from(1));
    assert_eq!(completed_operation.fee_collected(), Amount::ZERO);
    assert_eq!(completed_operation.payment_method(), None);

    assert_eq!(
        localstore
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state"),
        vec![Some(State::Spent)]
    );
    assert_eq!(
        localstore
            .get_proof_ys_by_operation_id(&operation_uuid)
            .await
            .expect("proof ys by operation id"),
        vec![input_y]
    );
    assert_eq!(
        localstore
            .get_blinded_secrets_by_operation_id(&operation_uuid)
            .await
            .expect("blinded secrets by operation id"),
        vec![output.blinded_secret]
    );

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored swap blind signature");
    let signature = signatures[0].as_ref().expect("accepted swap signature");
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, output.keyset_id);

    let share_records = localstore
        .get_federation_signature_shares_for_output(swap_entry.operation_id, 0)
        .await
        .expect("stored federation swap signature share");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, swap_entry.operation_id);
    assert_eq!(share_records[0].output_index, 0);
    assert_eq!(share_records[0].member_id, runtime_config.member_id());
    assert_eq!(share_records[0].keyset_id, output.keyset_id);
    assert_eq!(share_records[0].amount, Amount::from(1));
    assert_eq!(share_records[0].blinded_secret, output.blinded_secret);
    assert_eq!(
        share_records[0].accepted_operation_index,
        Some(swap_entry.index)
    );
    assert_eq!(
        share_records[0].consensus_evidence,
        swap_entry.consensus_evidence
    );

    let stored_entries = localstore
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    let stored_swap_entry = stored_entries
        .iter()
        .find(|entry| entry.operation_id == swap_entry.operation_id)
        .expect("stored swap entry");
    assert_eq!(stored_swap_entry.status, FederationOperationStatus::Applied);
}

#[tokio::test]
async fn apply_federation_swap_accepts_p2pk_locked_input() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (mut input, signing_key) =
        p2pk_locked_proof_for_runtime(&runtime_config, SigFlag::SigInputs);
    input.sign_p2pk(signing_key).expect("p2pk input witness");
    let output = output_for_runtime(&runtime_config, b"p2pk federated swap output");

    let entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(vec![input.clone()], vec![output.clone()]))
            .expect("p2pk swap operation"),
    );
    persist_and_apply_entry(&mint, &entry).await;

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored p2pk swap signatures");
    assert!(
        signatures[0].is_some(),
        "valid P2PK-locked federated swap output must receive a member share"
    );

    let input_states = mint
        .check_state(&cdk_common::CheckStateRequest {
            ys: vec![input.y().expect("input y")],
        })
        .await
        .expect("input states");
    assert_eq!(input_states.states[0].state, State::Spent);
}

#[tokio::test]
async fn apply_federation_swap_accepts_htlc_locked_input() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let signing_key = SecretKey::generate();
    let preimage = crate::util::hex::encode([42; 32]);
    let mut input =
        htlc_locked_proof_for_runtime(&runtime_config, &preimage, &signing_key, SigFlag::SigInputs);
    input.add_preimage(preimage);
    input.sign_p2pk(signing_key).expect("htlc input witness");
    let output = output_for_runtime(&runtime_config, b"htlc federated swap output");

    let entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(vec![input.clone()], vec![output.clone()]))
            .expect("htlc swap operation"),
    );
    persist_and_apply_entry(&mint, &entry).await;

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored htlc swap signatures");
    assert!(
        signatures[0].is_some(),
        "valid HTLC-locked federated swap output must receive a member share"
    );

    let input_states = mint
        .check_state(&cdk_common::CheckStateRequest {
            ys: vec![input.y().expect("input y")],
        })
        .await
        .expect("input states");
    assert_eq!(input_states.states[0].state, State::Spent);
}

#[tokio::test]
async fn apply_federation_swap_rejects_sig_all_output_mutation_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (input, signing_key) = p2pk_locked_proof_for_runtime(&runtime_config, SigFlag::SigAll);
    let signed_output = output_for_runtime(&runtime_config, b"sig-all signed swap output");
    let mutated_output = output_for_runtime(&runtime_config, b"sig-all mutated swap output");
    let mut swap = SwapRequest::new(vec![input], vec![signed_output.clone()]);
    swap.sign_sig_all(signing_key).expect("sig_all witness");
    *swap.outputs_mut() = vec![mutated_output.clone()];

    assert!(
        runtime_config
            .validate_federated_swap_submission(&swap)
            .is_err(),
        "SIG_ALL output mutation must be rejected before consensus admission"
    );

    let entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
            request: swap,
        })),
    );
    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.add_federation_journal_entry(&entry)
        .await
        .expect("persist mutated sig_all swap");
    tx.commit().await.expect("commit mutated sig_all swap");

    assert!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&entry))
            .await
            .is_err(),
        "SIG_ALL witness signed for a different output set must be rejected"
    );

    let signatures = localstore
        .get_blind_signatures(&[signed_output.blinded_secret, mutated_output.blinded_secret])
        .await
        .expect("sig_all output signatures");
    assert_eq!(
        signatures,
        vec![None, None],
        "SIG_ALL output mutation must fail before any federation share is stored"
    );
}

#[tokio::test]
async fn apply_federation_swap_rejects_sliding_window_input_reuse_attack() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let inputs = vec![
        proof_for_runtime(&runtime_config, "sliding-swap-input-a"),
        proof_for_runtime(&runtime_config, "sliding-swap-input-b"),
        proof_for_runtime(&runtime_config, "sliding-swap-input-c"),
    ];
    let input_ys = inputs.ys().expect("input ys");
    let output_a = output_for_runtime(&runtime_config, b"sliding-swap-output-a");
    let output_b = output_for_runtime(&runtime_config, b"sliding-swap-output-b");
    let output_c = output_for_runtime(&runtime_config, b"sliding-swap-output-c");
    let output_d = output_for_runtime(&runtime_config, b"sliding-swap-output-d");
    let output_e = output_for_runtime(&runtime_config, b"sliding-swap-output-e");
    let output_f = output_for_runtime(&runtime_config, b"sliding-swap-output-f");

    let first_entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(
                inputs.clone(),
                vec![output_a.clone(), output_b.clone(), output_c.clone()],
            ))
            .expect("first swap operation"),
    );
    persist_and_apply_entry(&mint, &first_entry).await;

    mint.apply_federation_journal_entries(std::slice::from_ref(&first_entry))
        .await
        .expect("same swap replay remains idempotent");

    let signed_outputs = localstore
        .get_blind_signatures(&[
            output_a.blinded_secret,
            output_b.blinded_secret,
            output_c.blinded_secret,
        ])
        .await
        .expect("accepted swap signatures");
    assert_eq!(signed_outputs.iter().filter(|sig| sig.is_some()).count(), 3);

    for outputs in [
        vec![output_d.clone(), output_e.clone(), output_f.clone()],
        vec![output_b.clone(), output_c.clone(), output_d.clone()],
        vec![output_c.clone(), output_d.clone(), output_a.clone()],
        vec![output_d.clone(), output_a.clone(), output_b.clone()],
    ] {
        let entry = appended_entry(
            &mut journal,
            runtime_config
                .swap_operation(SwapRequest::new(inputs.clone(), outputs))
                .expect("conflicting swap operation"),
        );
        let mut tx = localstore.begin_transaction().await.expect("begin");
        tx.add_federation_journal_entry(&entry)
            .await
            .expect("persist conflicting swap");
        tx.commit().await.expect("commit conflicting swap");

        assert!(matches!(
            mint.apply_federation_journal_entries(std::slice::from_ref(&entry))
                .await,
            Err(cdk_common::Error::TokenAlreadySpent)
        ));
    }

    let unsigned_outputs = localstore
        .get_blind_signatures(&[
            output_d.blinded_secret,
            output_e.blinded_secret,
            output_f.blinded_secret,
        ])
        .await
        .expect("unsigned conflicting swap outputs");
    assert_eq!(
        unsigned_outputs,
        vec![None, None, None],
        "conflicting swap outputs must not receive member shares"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("input states"),
        vec![Some(State::Spent), Some(State::Spent), Some(State::Spent)]
    );
}

#[tokio::test]
async fn process_federation_melt_change_signs_rotated_keyset() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
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

    let effective = mint
        .effective_federation_config()
        .await
        .expect("effective config")
        .expect("federation config");
    let quote = QuoteId::new();
    let change_output = output_for_keyset(&next_keyset, b"rotated melt change");
    let change_secret = change_output.blinded_secret;
    let operation_id = effective
        .melt_operation(MeltRequest::new(
            quote.clone(),
            Vec::new(),
            Some(vec![change_output.clone()]),
        ))
        .expect("melt operation")
        .operation_id()
        .expect("operation id");
    let consensus_evidence =
        FederationJournalConsensusEvidence::new(7, 11, 13, effective.member_id(), 17);
    let signature_share_provenance =
        super::super::melt::shared::FederationSignatureShareProvenance::new(
            operation_id,
            23,
            Some(consensus_evidence),
        );

    let db = mint.localstore();
    let (change_sigs, tx) = super::super::melt::shared::process_federation_melt_change(
        &mint,
        &db,
        &quote,
        operation_id,
        signature_share_provenance,
        Amount::new(3, CurrencyUnit::Sat),
        Amount::new(2, CurrencyUnit::Sat),
        Amount::ZERO.with_unit(CurrencyUnit::Sat),
        vec![change_output],
    )
    .await
    .expect("process federated change");
    tx.commit().await.expect("commit change signatures");

    let signatures = localstore
        .get_blind_signatures(&[change_secret])
        .await
        .expect("stored change signature");
    let signature = signatures
        .first()
        .and_then(Option::as_ref)
        .expect("rotated melt change signature");
    assert_eq!(signature.keyset_id, next_keyset.id);
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(
        change_sigs.expect("change response")[0].keyset_id,
        next_keyset.id
    );

    let share_records = localstore
        .get_federation_signature_shares_for_blinded_message(change_secret)
        .await
        .expect("stored federation change signature shares");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, operation_id);
    assert_eq!(share_records[0].output_index, 0);
    assert_eq!(share_records[0].member_id, effective.member_id());
    assert_eq!(share_records[0].keyset_id, next_keyset.id);
    assert_eq!(share_records[0].amount, Amount::from(1));
    assert_eq!(share_records[0].blinded_secret, change_secret);
    assert_eq!(share_records[0].accepted_operation_index, Some(23));
    assert_eq!(
        share_records[0].consensus_evidence,
        Some(consensus_evidence)
    );
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

#[tokio::test]
async fn real_policy_mint_payment_observations_finalize_from_stored_quorum() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(10),
            bolt11_quote_response(&quote, 10),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let first_observation = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 1, 10, "settled"),
    );
    persist_and_apply_entry(&mint, &first_observation).await;
    let quote_after_single = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(quote_after_single.state(), MintQuoteState::Unpaid);
    assert_eq!(
        quote_after_single.amount_paid(),
        Amount::new(0, CurrencyUnit::Sat)
    );

    let observations_after_single = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MintQuote,
            &quote.to_string(),
        )
        .await
        .expect("single observation rows");
    assert_eq!(observations_after_single.len(), 1);

    let matching_second = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 2, 10, "settled"),
    );
    persist_and_apply_entry(&mint, &matching_second).await;
    let quote_after_quorum = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(quote_after_quorum.state(), MintQuoteState::Paid);
    assert_eq!(
        quote_after_quorum.amount_paid(),
        Amount::new(10, CurrencyUnit::Sat)
    );

    let conflicting_third = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 3, 11, "different-settlement"),
    );
    persist_and_apply_entry(&mint, &conflicting_third).await;
    let quote_after_conflict = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(
        quote_after_conflict.amount_paid(),
        Amount::new(10, CurrencyUnit::Sat)
    );

    let observations_after_conflict = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MintQuote,
            &quote.to_string(),
        )
        .await
        .expect("conflicting observation rows");
    assert_eq!(observations_after_conflict.len(), 3);
    assert_eq!(
        observations_after_conflict[2].observation.observer(),
        MemberId::new(3).expect("member three")
    );
}

#[tokio::test]
async fn mint_payment_observation_metadata_sets_payment_record_id_and_time() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let payment_id = PaymentIdentifier::CustomId("metadata-payment".to_string());

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(7),
            bolt11_quote_response(&quote, 7),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let first_observation = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::MintQuotePayment(
            FederationMintQuotePaymentObservation {
                quote: quote.to_string(),
                observer: MemberId::new(1).expect("member one"),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(7),
                amount_issued: Amount::ZERO,
                payment_id: Some(payment_id.clone()),
                payment_proof: None,
                observed_time: Some(111),
            },
        )),
    );
    persist_and_apply_entry(&mint, &first_observation).await;

    let matching_second = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::MintQuotePayment(
            FederationMintQuotePaymentObservation {
                quote: quote.to_string(),
                observer: MemberId::new(2).expect("member two"),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(7),
                amount_issued: Amount::ZERO,
                payment_id: Some(payment_id.clone()),
                payment_proof: None,
                observed_time: Some(222),
            },
        )),
    );
    persist_and_apply_entry(&mint, &matching_second).await;

    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Paid);
    assert_eq!(stored_quote.payments.len(), 1);
    assert_eq!(
        stored_quote.payments[0].payment_id,
        "custom:metadata-payment"
    );
    assert_eq!(stored_quote.payments[0].time, 222);
}

#[tokio::test]
async fn payment_quarantine_blocks_mint_signing() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let first_observation = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 1, 1, "settled"),
    );
    persist_and_apply_entry(&mint, &first_observation).await;
    let matching_second = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 2, 1, "settled"),
    );
    persist_and_apply_entry(&mint, &matching_second).await;
    let quote_after_quorum = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(quote_after_quorum.state(), MintQuoteState::Paid);

    let quarantine_entry = appended_entry(
        &mut journal,
        runtime_config.payment_observation_quarantine(
            FederationPaymentObservationContext::MintQuote,
            quote.clone(),
            FederationPaymentObservationQuarantineReason::ConflictingEvidence,
            vec![first_observation.operation_id, matching_second.operation_id],
        ),
    );
    persist_and_apply_entry(&mint, &quarantine_entry).await;

    let output = output_for_runtime(&runtime_config, b"quarantined mint output");
    let input = MintInput::Single(MintRequest {
        quote: quote.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(input)
            .expect("mint operation"),
    );
    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.add_federation_journal_entry(&mint_entry)
        .await
        .expect("persist mint entry");
    tx.commit().await.expect("commit mint entry");
    let err = mint
        .apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
        .await
        .expect_err("quarantined quote must not mint");
    assert!(
        err.to_string().contains("quarantined"),
        "unexpected error: {err}"
    );

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("signature lookup");
    assert!(signatures[0].is_none());
}

#[tokio::test]
async fn future_quarantine_does_not_suppress_prior_mint_payment_replay() {
    let runtime_config =
        runtime_config_with_payment_policy(FederationPaymentObservationPolicy::default());
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    let first_observation = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 1, 1, "settled"),
    );
    let conflicting_observation = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 2, 2, "conflicting"),
    );
    let quarantine_entry = appended_entry(
        &mut journal,
        runtime_config.payment_observation_quarantine(
            FederationPaymentObservationContext::MintQuote,
            quote.clone(),
            FederationPaymentObservationQuarantineReason::ConflictingEvidence,
            vec![
                first_observation.operation_id,
                conflicting_observation.operation_id,
            ],
        ),
    );
    let entries = vec![
        quote_entry,
        first_observation,
        conflicting_observation,
        quarantine_entry,
    ];

    let mut tx = localstore.begin_transaction().await.expect("begin");
    for entry in &entries {
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist journal entry");
    }
    tx.commit().await.expect("commit journal entries");

    for entry in &entries {
        mint.apply_federation_journal_entries(std::slice::from_ref(entry))
            .await
            .expect("apply journal entry");
    }

    let quote_after_replay = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(quote_after_replay.amount_paid().value(), 1);
    assert_eq!(quote_after_replay.state(), MintQuoteState::Paid);

    let output = output_for_runtime(&runtime_config, b"future quarantine mint output");
    let input = MintInput::Single(MintRequest {
        quote: quote.clone(),
        outputs: vec![output],
        signature: None,
    });
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(input)
            .expect("mint operation"),
    );
    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.add_federation_journal_entry(&mint_entry)
        .await
        .expect("persist mint entry");
    tx.commit().await.expect("commit mint entry");

    let err = mint
        .apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
        .await
        .expect_err("quarantined quote must not mint after replay");
    assert!(
        err.to_string().contains("quarantined"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn real_policy_non_elected_melt_member_reserves_without_payment_attempt() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let operation_id = operation_id_with_selector(1);
    assert_eq!(
        runtime_config.elected_melt_payment_member(operation_id),
        MemberId::new(2).expect("member two")
    );
    assert!(!runtime_config.should_attempt_melt_payment(operation_id));

    let make_payment_calls = Arc::new(AtomicUsize::new(0));
    let payment_processor = Arc::new(CountingBackendAgnosticPaymentProcessor {
        make_payment_calls: Arc::clone(&make_payment_calls),
    });
    let (mint, localstore) =
        federated_test_mint_with_payment_processor(runtime_config.clone(), payment_processor).await;
    let quote = QuoteId::new();
    mint.get_federation_melt_quote(quote.clone(), pending_melt_quote_request(1_000))
        .await
        .expect("federation melt quote");
    let input_ys;
    let request = {
        let proofs = vec![
            proof_for_runtime(&runtime_config, "non-elected-melt-input-one"),
            proof_for_runtime(&runtime_config, "non-elected-melt-input-two"),
        ];
        input_ys = proofs.ys().expect("input ys");
        MeltRequest::new(quote.clone(), proofs, None)
    };

    let response = mint
        .melt_federation(&request, operation_id)
        .await
        .expect("non-elected federation melt reserve");

    assert_eq!(response.state(), MeltQuoteState::Pending);
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 0);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_some(),
        "non-elected member should keep pending saga for later payment observation"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states"),
        vec![Some(State::Pending), Some(State::Pending)]
    );
}

#[test]
fn melt_payment_actor_selection_is_deterministic_for_real_policies() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let real_policy_config = runtime_config_with_payment_policy(policy);
    let member_one_operation = operation_id_with_selector(0);
    let member_two_operation = operation_id_with_selector(1);

    assert_eq!(
        real_policy_config.elected_melt_payment_member(member_one_operation),
        MemberId::new(1).expect("member one")
    );
    assert!(real_policy_config.should_attempt_melt_payment(member_one_operation));
    assert_eq!(
        real_policy_config.elected_melt_payment_member(member_two_operation),
        MemberId::new(2).expect("member two")
    );
    assert!(!real_policy_config.should_attempt_melt_payment(member_two_operation));
    assert!(runtime_config().should_attempt_melt_payment(member_two_operation));
}

#[tokio::test]
async fn internal_melt_to_mint_settlement_updates_both_quotes_in_accepted_melt() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let invoice = cdk_fake_wallet::create_fake_invoice(1_000, "internal settlement".to_string());

    let mint_quote = QuoteId::new();
    let mint_quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            mint_quote.clone(),
            bolt11_quote_request(1),
            MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                quote: mint_quote.to_string(),
                request: invoice.to_string(),
                amount: Some(Amount::from(1)),
                unit: Some(CurrencyUnit::Sat),
                state: MintQuoteState::Unpaid,
                expiry: None,
                pubkey: None,
            }),
        ),
    );
    persist_and_apply_entry(&mint, &mint_quote_entry).await;

    let melt_quote = QuoteId::new();
    let melt_quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(
            melt_quote.clone(),
            MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }),
        ),
    );
    persist_and_apply_entry(&mint, &melt_quote_entry).await;

    let proofs = vec![
        proof_for_runtime(&runtime_config, "internal-settlement-input-one"),
        proof_for_runtime(&runtime_config, "internal-settlement-input-two"),
    ];
    let input_ys = proofs.ys().expect("input ys");
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(MeltRequest::new(melt_quote.clone(), proofs, None))
            .expect("melt operation"),
    );
    persist_and_apply_entry(&mint, &melt_entry).await;

    let stored_mint_quote = localstore
        .get_mint_quote(&mint_quote)
        .await
        .expect("mint quote lookup")
        .expect("mint quote");
    assert_eq!(stored_mint_quote.state(), MintQuoteState::Paid);
    assert_eq!(
        stored_mint_quote.amount_paid(),
        Amount::new(1, CurrencyUnit::Sat)
    );
    assert_eq!(stored_mint_quote.payments.len(), 1);
    assert_eq!(
        stored_mint_quote.payments[0].payment_id,
        melt_quote.to_string()
    );
    assert_eq!(
        stored_mint_quote.payments[0].time,
        super::federation_accepted_logical_time(&melt_entry)
    );

    let stored_melt_quote = localstore
        .get_melt_quote(&melt_quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(stored_melt_quote.state, MeltQuoteState::Paid);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&melt_quote)
            .await
            .expect("melt saga lookup")
            .is_none(),
        "internal settlement should complete and delete the melt saga"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states"),
        vec![Some(State::Spent), Some(State::Spent)]
    );
    assert!(
        localstore
            .get_federation_payment_observations(
                FederationPaymentObservationContext::MeltQuote,
                &melt_quote.to_string(),
            )
            .await
            .expect("melt payment observations")
            .is_empty(),
        "internal settlement should not require a separate melt payment observation"
    );
}

#[tokio::test]
async fn real_policy_melt_payment_observations_finalize_from_stored_quorum() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    let first_observation = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 1,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("paid-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &first_observation).await;

    let quote_after_single = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_single.state, MeltQuoteState::Pending);
    assert_eq!(quote_after_single.payment_proof, None);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_some(),
        "single real-backend observation must not finalize the saga"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after single observation"),
        vec![Some(State::Pending), Some(State::Pending)]
    );

    let observations_after_single = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("single melt observation rows");
    assert_eq!(observations_after_single.len(), 1);

    let matching_second = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 2,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("paid-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &matching_second).await;

    let quote_after_quorum = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_quorum.state, MeltQuoteState::Paid);
    assert_eq!(
        quote_after_quorum.payment_proof,
        Some("paid-preimage".to_string())
    );
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_none(),
        "matching quorum should clear the pending melt saga"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after quorum"),
        vec![Some(State::Spent), Some(State::Spent)]
    );

    let conflicting_third = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 3,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("different-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &conflicting_third).await;

    let quote_after_conflict = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_conflict.state, MeltQuoteState::Paid);
    assert_eq!(
        quote_after_conflict.payment_proof,
        Some("paid-preimage".to_string())
    );

    let observations_after_conflict = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("conflicting melt observation rows");
    assert_eq!(observations_after_conflict.len(), 3);
    assert_eq!(
        observations_after_conflict[2].observation.observer(),
        MemberId::new(3).expect("member three")
    );
}

#[tokio::test]
async fn stale_pending_melt_observation_after_paid_settlement_is_idempotent() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    let paid_observation = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 1,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("paid-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &paid_observation).await;

    let quote_after_paid = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_paid.state, MeltQuoteState::Paid);
    assert_eq!(
        quote_after_paid.payment_proof,
        Some("paid-preimage".to_string())
    );

    let stale_pending = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 2,
                state: MeltQuoteState::Pending,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &stale_pending).await;

    let quote_after_stale = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_stale.state, MeltQuoteState::Paid);
    assert_eq!(
        quote_after_stale.payment_proof,
        Some("paid-preimage".to_string())
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after stale pending"),
        vec![Some(State::Spent), Some(State::Spent)]
    );
    let observations = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("observation rows");
    assert_eq!(observations.len(), 2);
}

#[tokio::test]
async fn stale_pending_melt_observation_after_failed_rollback_is_idempotent() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    let failed_observation = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 1,
                state: MeltQuoteState::Failed,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: Some("route failed"),
            },
        ),
    );
    persist_and_apply_entry(&mint, &failed_observation).await;

    let quote_after_failed = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_failed.state, MeltQuoteState::Unpaid);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_none(),
        "failed rollback should clear the pending melt saga"
    );

    let stale_pending = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 2,
                state: MeltQuoteState::Pending,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &stale_pending).await;

    let quote_after_stale = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_stale.state, MeltQuoteState::Unpaid);
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after stale pending"),
        vec![None, None]
    );
    let observations = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("observation rows");
    assert_eq!(observations.len(), 2);
}

#[tokio::test]
async fn real_policy_failed_melt_observations_rollback_from_stored_quorum() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    let first_observation = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 1,
                state: MeltQuoteState::Failed,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: Some("route failed"),
            },
        ),
    );
    persist_and_apply_entry(&mint, &first_observation).await;

    let quote_after_single = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_single.state, MeltQuoteState::Pending);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_some(),
        "single failed observation must not compensate pending inputs"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after single failed observation"),
        vec![Some(State::Pending), Some(State::Pending)]
    );

    let matching_second = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 2,
                state: MeltQuoteState::Failed,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: Some("route failed"),
            },
        ),
    );
    persist_and_apply_entry(&mint, &matching_second).await;

    let quote_after_quorum = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_quorum.state, MeltQuoteState::Unpaid);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_none(),
        "matching failed quorum should clear the pending melt saga"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after rollback"),
        vec![None, None]
    );

    let conflicting_paid = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 3,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("late-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &conflicting_paid).await;

    let quote_after_conflict = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_conflict.state, MeltQuoteState::Unpaid);
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after conflict"),
        vec![None, None]
    );

    let observations_after_conflict = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("conflicting melt observation rows");
    assert_eq!(observations_after_conflict.len(), 3);
}

#[test]
fn runtime_config_builds_single_and_batch_mint_operations() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let single_input = super::MintInput::Single(MintRequest {
        quote: quote.clone(),
        outputs: vec![output_for_runtime(&runtime_config, b"single mint")],
        signature: None,
    });
    runtime_config
        .validate_federated_mint_submission(&single_input)
        .expect("valid mint admission");
    let single = runtime_config
        .mint_operation(single_input)
        .expect("single mint operation");

    match single.operation {
        FederationOperation::Mint(operation) => match operation.request {
            FederationMintRequest::Single(request) => {
                assert_eq!(request.quote, quote.to_string());
            }
            FederationMintRequest::Batch(_) => panic!("expected single mint request"),
        },
        _ => panic!("expected mint operation"),
    }

    let quote_one = QuoteId::new();
    let quote_two = QuoteId::new();
    let quote_three = QuoteId::new();
    let batch = runtime_config
        .mint_operation(super::MintInput::Batch(BatchMintRequest {
            quotes: vec![quote_one.clone(), quote_two.clone()],
            quote_amounts: Some(vec![Amount::from(1), Amount::from(1)]),
            outputs: vec![output_for_runtime(&runtime_config, b"batch mint")],
            signatures: None,
        }))
        .expect("batch mint operation");

    assert_eq!(batch.operation.conflict_keys().len(), 2);
    let mut consensus = InMemoryFederationConsensus::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
    );
    consensus.submit_envelope(batch).expect("submit batch mint");

    let overlapping_batch = runtime_config
        .mint_operation(super::MintInput::Batch(BatchMintRequest {
            quotes: vec![quote_two, quote_three],
            quote_amounts: Some(vec![Amount::from(1), Amount::from(1)]),
            outputs: vec![output_for_runtime(
                &runtime_config,
                b"overlapping batch mint",
            )],
            signatures: None,
        }))
        .expect("overlapping batch mint operation");

    assert!(matches!(
        consensus.submit_envelope(overlapping_batch),
        Err(FederationError::ConflictingOperation { .. })
    ));
}

#[test]
fn runtime_config_rejects_sliding_window_mint_output_attack() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let output_a = output_for_runtime(&runtime_config, b"sliding-window-output-a");
    let output_b = output_for_runtime(&runtime_config, b"sliding-window-output-b");
    let output_c = output_for_runtime(&runtime_config, b"sliding-window-output-c");
    let output_d = output_for_runtime(&runtime_config, b"sliding-window-output-d");

    let envelope_for_outputs = |outputs| {
        runtime_config
            .mint_operation(super::MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs,
                signature: None,
            }))
            .expect("mint operation")
    };

    let first = envelope_for_outputs(vec![output_a.clone(), output_b.clone(), output_c.clone()]);
    let overlapping_one =
        envelope_for_outputs(vec![output_b.clone(), output_c.clone(), output_d.clone()]);
    let overlapping_two =
        envelope_for_outputs(vec![output_c.clone(), output_d.clone(), output_a.clone()]);
    let overlapping_three = envelope_for_outputs(vec![output_d, output_a, output_b]);

    let first_operation_id = first.operation_id().expect("first operation id");
    let attempted_operation_ids = [
        overlapping_one
            .operation_id()
            .expect("overlap one operation id"),
        overlapping_two
            .operation_id()
            .expect("overlap two operation id"),
        overlapping_three
            .operation_id()
            .expect("overlap three operation id"),
    ];
    for attempted_operation_id in attempted_operation_ids {
        assert_ne!(first_operation_id, attempted_operation_id);
    }

    let mut consensus = InMemoryFederationConsensus::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
    );
    let first_response = consensus
        .submit_envelope(first.clone())
        .expect("submit first sliding-window mint");
    let duplicate_response = consensus
        .submit_envelope(first)
        .expect("duplicate sliding-window mint");

    assert_eq!(first_response.operation_id, first_operation_id);
    assert_eq!(duplicate_response.operation_id, first_operation_id);

    for envelope in [overlapping_one, overlapping_two, overlapping_three] {
        assert!(matches!(
            consensus.submit_envelope(envelope),
            Err(FederationError::ConflictingOperation { .. })
        ));
    }

    let entries = consensus.journal().entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, first_operation_id);
    assert_eq!(entries[0].envelope.operation.conflict_keys().len(), 1);
}

#[test]
fn runtime_config_rejects_non_v3_mint_outputs_before_consensus() {
    let runtime_config = runtime_config();
    let input = super::MintInput::Single(MintRequest {
        quote: QuoteId::new(),
        outputs: vec![non_v3_output_for_runtime(&runtime_config)],
        signature: None,
    });

    assert!(matches!(
        runtime_config.validate_federated_mint_submission(&input),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
    assert!(matches!(
        runtime_config.mint_operation(input),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[test]
fn runtime_config_rejects_unsupported_protocol_versions() {
    let runtime_config = runtime_config();
    let mut public_config = runtime_config.public_config().clone();
    let private_config = runtime_config.private_config().clone();
    let expected = public_config.protocol_versions.rest_api;
    let found = expected.saturating_add(1);
    public_config.protocol_versions.rest_api = found;

    assert!(matches!(
        FederationRuntimeConfig::new(public_config, private_config),
        Err(cdk_common::Error::Federation(
            FederationError::ProtocolVersionMismatch {
                component: "rest_api",
                expected: actual_expected,
                found: actual_found,
            }
        )) if actual_expected == expected && actual_found == found
    ));
}

#[test]
fn runtime_config_rejects_non_v3_swap_restore_and_change_outputs() {
    let runtime_config = runtime_config();
    let output = non_v3_output_for_runtime(&runtime_config);
    let swap = SwapRequest::new(Vec::new(), vec![output.clone()]);
    let restore = RestoreRequest {
        outputs: vec![output.clone()],
    };
    let melt = MeltRequest::new(QuoteId::new(), Vec::new(), Some(vec![output]));

    assert!(matches!(
        runtime_config.validate_federated_swap_submission(&swap),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
    assert!(matches!(
        runtime_config.validate_restore_outputs(&restore),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
    assert!(matches!(
        runtime_config.validate_federated_melt_submission(&melt),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
    assert!(matches!(
        runtime_config.melt_operation(melt),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[test]
fn runtime_config_operation_submission_admission_rejects_invalid_wallet_operations() {
    let runtime_config = runtime_config();
    let non_v3_output = non_v3_output_for_runtime(&runtime_config);
    let mint =
        runtime_config.operation_envelope(FederationOperation::Mint(FederationMintOperation {
            request: FederationMintRequest::Single(MintRequest {
                quote: QuoteId::new().to_string(),
                outputs: vec![non_v3_output.clone()],
                signature: None,
            }),
        }));
    let swap =
        runtime_config.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(Vec::new(), vec![non_v3_output.clone()]),
        }));
    let melt =
        runtime_config.operation_envelope(FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(
                QuoteId::new().to_string(),
                Vec::new(),
                Some(vec![non_v3_output.clone()]),
            ),
        }));
    let restore = runtime_config.operation_envelope(FederationOperation::Restore(
        FederationRestoreOperation {
            request: RestoreRequest {
                outputs: vec![non_v3_output],
            },
        },
    ));
    let mut inactive_keyset = runtime_config.public_config().keysets[0].clone();
    inactive_keyset.active = false;
    let rotation = runtime_config.operation_envelope(FederationOperation::KeysetRotation(
        FederationKeysetRotationOperation {
            keyset: inactive_keyset,
        },
    ));

    for envelope in [mint, swap, melt, restore, rotation] {
        assert!(runtime_config
            .validate_federated_operation_submission(&envelope.operation)
            .is_err());
    }
}

#[test]
fn runtime_config_operation_submission_admission_accepts_valid_wallet_operations() {
    let runtime_config = runtime_config();
    let mint = runtime_config
        .mint_operation(super::MintInput::Single(MintRequest {
            quote: QuoteId::new(),
            outputs: vec![output_for_runtime(&runtime_config, b"valid routed mint")],
            signature: None,
        }))
        .expect("mint operation");
    let swap = runtime_config
        .swap_operation(SwapRequest::new(
            Vec::new(),
            vec![output_for_runtime(&runtime_config, b"valid routed swap")],
        ))
        .expect("swap operation");
    let mut change = output_for_runtime(&runtime_config, b"valid routed melt change");
    change.amount = Amount::ZERO;
    let melt = runtime_config
        .melt_operation(MeltRequest::new(
            QuoteId::new(),
            Vec::new(),
            Some(vec![change]),
        ))
        .expect("melt operation");
    let restore = runtime_config.operation_envelope(FederationOperation::Restore(
        FederationRestoreOperation {
            request: RestoreRequest {
                outputs: vec![output_for_runtime(&runtime_config, b"valid routed restore")],
            },
        },
    ));
    let rotation_runtime_config = runtime_config_with_inactive_rotation_keyset();
    let mut rotation_keyset = rotation_runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    rotation_keyset.active = true;
    let rotation = rotation_runtime_config.operation_envelope(FederationOperation::KeysetRotation(
        FederationKeysetRotationOperation {
            keyset: rotation_keyset,
        },
    ));

    for envelope in [mint, swap, melt, restore] {
        runtime_config
            .validate_federated_operation_submission(&envelope.operation)
            .expect("valid wallet operation admission");
    }
    rotation_runtime_config
        .validate_federated_operation_submission(&rotation.operation)
        .expect("valid keyset rotation admission");
}

#[test]
fn runtime_config_admission_rejects_invalid_swap_and_melt_proofs_before_consensus() {
    let runtime_config = runtime_config();
    let mut invalid_swap_proof = proof_for_runtime(&runtime_config, "invalid swap admission");
    invalid_swap_proof.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"invalid swap admission signature",
    ));
    let swap = SwapRequest::new(
        vec![invalid_swap_proof],
        vec![output_for_runtime(
            &runtime_config,
            b"invalid swap admission output",
        )],
    );

    assert!(runtime_config
        .validate_federated_swap_submission(&swap)
        .is_err());
    assert!(runtime_config.swap_operation(swap).is_err());

    let mut invalid_melt_proof = proof_for_runtime(&runtime_config, "invalid melt admission");
    invalid_melt_proof.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"invalid melt admission signature",
    ));
    let mut change_output = output_for_runtime(&runtime_config, b"invalid melt admission change");
    change_output.amount = Amount::ZERO;
    let melt = MeltRequest::new(
        QuoteId::new(),
        vec![invalid_melt_proof],
        Some(vec![change_output]),
    );

    assert!(runtime_config
        .validate_federated_melt_submission(&melt)
        .is_err());
    assert!(runtime_config.melt_operation(melt).is_err());
}

#[test]
fn runtime_config_verifies_aggregate_bls_proofs() {
    let runtime_config = runtime_config();
    let proof = proof_for_runtime(&runtime_config, "valid");

    runtime_config
        .verify_proofs(&vec![proof])
        .expect("aggregate BLS proof should verify");
}

#[test]
fn runtime_config_rejects_invalid_aggregate_bls_proofs() {
    let runtime_config = runtime_config();
    let mut proof = proof_for_runtime(&runtime_config, "invalid");
    proof.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(b"not a valid signature"));

    assert!(runtime_config.verify_proofs(&vec![proof]).is_err());
}

#[test]
fn runtime_config_rejects_non_v3_aggregate_proofs() {
    let runtime_config = runtime_config();
    let mut proof = proof_for_runtime(&runtime_config, "non-v3");
    let keyset = &runtime_config.public_config().keysets[0];
    proof.keyset_id = Id::v2_from_data(
        &keyset.keys(),
        &keyset.unit,
        keyset.input_fee_ppk,
        keyset.final_expiry,
    );

    assert!(matches!(
        runtime_config.verify_proofs(&vec![proof]),
        Err(cdk_common::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[test]
fn runtime_config_builds_swap_operations() {
    let runtime_config = runtime_config();
    let swap = SwapRequest::new(
        Vec::new(),
        vec![output_for_runtime(&runtime_config, b"swap operation")],
    );
    let envelope = runtime_config.swap_operation(swap).expect("swap operation");

    assert_eq!(envelope.federation_id, runtime_config.federation_id());
    assert!(matches!(envelope.operation, FederationOperation::Swap(_)));
    assert_eq!(envelope.operation.conflict_keys().len(), 0);
}

#[test]
fn runtime_config_builds_melt_operations() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let mut change_output = output_for_runtime(&runtime_config, b"melt operation change");
    change_output.amount = Amount::ZERO;
    let melt = MeltRequest::new(quote.clone(), Vec::new(), Some(vec![change_output]))
        .prefer_async(true)
        .fee_index(42);
    let envelope = runtime_config.melt_operation(melt).expect("melt operation");

    assert_eq!(envelope.federation_id, runtime_config.federation_id());
    assert_eq!(envelope.operation.conflict_keys().len(), 1);
    match envelope.operation {
        FederationOperation::Melt(operation) => {
            assert_eq!(operation.request.quote(), &quote.to_string());
            assert!(operation.request.is_prefer_async());
            assert_eq!(operation.request.selected_fee_index(), Some(42));
        }
        _ => panic!("expected melt operation"),
    }
}

#[test]
fn runtime_config_accepts_zero_amount_melt_change_placeholders() {
    let runtime_config = runtime_config();
    let mut output = output_for_runtime(&runtime_config, b"zero amount change placeholder");
    output.amount = Amount::ZERO;
    let melt = MeltRequest::new(QuoteId::new(), Vec::new(), Some(vec![output.clone()]));
    let restore = RestoreRequest {
        outputs: vec![output.clone()],
    };

    assert!(runtime_config.validate_melt_change_outputs(&melt).is_ok());
    assert!(runtime_config.validate_restore_outputs(&restore).is_ok());
    assert!(matches!(
        runtime_config.validate_output_keysets(&[output]),
        Err(cdk_common::Error::Federation(
            FederationError::UnknownFederatedKeysetAmount { .. }
        ))
    ));
}

#[test]
fn runtime_config_signs_accepted_mint_operations() {
    let runtime_config = runtime_config();
    let keyset = &runtime_config.public_config().keysets[0];
    let blinded_message = BlsG1PublicKey::hash_to_curve(b"cdk axum federation mint share");
    let input = super::MintInput::Single(MintRequest {
        quote: QuoteId::new(),
        outputs: vec![BlindedMessage::new(
            Amount::from(1),
            keyset.id,
            PublicKey::from(blinded_message),
        )],
        signature: None,
    });
    let operation_id = runtime_config
        .mint_operation(input.clone())
        .expect("mint operation")
        .operation_id()
        .expect("operation id");

    let share_set = runtime_config
        .mint_signature_shares(operation_id, &input)
        .expect("signature shares");
    let share = share_set.shares.first().expect("signature share");
    let public_share = keyset
        .public_shares
        .get(&runtime_config.member_id())
        .expect("member shares")
        .get(&Amount::from(1))
        .expect("amount share");

    assert_eq!(share_set.operation_id, operation_id);
    assert_eq!(share_set.member_id, runtime_config.member_id());
    assert_eq!(share.operation_id, operation_id);
    assert!(
        verify_blind_signature_share(&share.share, &blinded_message, public_share)
            .expect("share verification")
    );

    let response = runtime_config
        .mint_share_response(operation_id, &input)
        .expect("mint share response");
    let signature = response.signatures.first().expect("signature share");

    assert_eq!(response.signatures.len(), 1);
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, keyset.id);
    assert!(signature.dleq.is_none());
    assert_eq!(
        signature.c.as_bls_g1().expect("response share point"),
        share.share.signature()
    );
}

#[test]
fn runtime_config_signs_accepted_swap_operations() {
    let runtime_config = runtime_config();
    let keyset = &runtime_config.public_config().keysets[0];
    let blinded_message = BlsG1PublicKey::hash_to_curve(b"cdk federation swap share");
    let swap = SwapRequest::new(
        Vec::new(),
        vec![BlindedMessage::new(
            Amount::from(1),
            keyset.id,
            PublicKey::from(blinded_message),
        )],
    );
    let operation_id = runtime_config
        .swap_operation(swap.clone())
        .expect("swap operation")
        .operation_id()
        .expect("operation id");

    let share_set = runtime_config
        .swap_signature_shares(operation_id, &swap)
        .expect("signature shares");
    let share = share_set.shares.first().expect("signature share");
    let public_share = keyset
        .public_shares
        .get(&runtime_config.member_id())
        .expect("member shares")
        .get(&Amount::from(1))
        .expect("amount share");

    assert_eq!(share_set.operation_id, operation_id);
    assert_eq!(share_set.member_id, runtime_config.member_id());
    assert_eq!(share.operation_id, operation_id);
    assert!(
        verify_blind_signature_share(&share.share, &blinded_message, public_share)
            .expect("share verification")
    );

    let response = runtime_config
        .swap_share_response(operation_id, &swap)
        .expect("swap share response");
    let signature = response.signatures.first().expect("signature share");

    assert_eq!(response.signatures.len(), 1);
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, keyset.id);
    assert!(signature.dleq.is_none());
    assert_eq!(
        signature.c.as_bls_g1().expect("response share point"),
        share.share.signature()
    );
}
