//! Parity of standalone and federated swap, mint and melt rules.
//!
//! Standalone mints and federated application keep separate implementations
//! of the swap, mint and melt rules (decision 2 of the 2026-11-23 federation
//! review plan). These tests run the same request vectors through both and
//! assert that the outcomes agree.
//!
//! # Paths
//!
//! - Standalone: `Mint::process_swap_request`, `Mint::process_mint_request`,
//!   `Mint::get_mint_quote`, `Mint::get_melt_quote` and `Mint::melt` on a mint
//!   without federation config.
//! - Federated: the member's quote preparation (`prepare_mint_quote`,
//!   `prepare_melt_quote`), payment probes (`probe_federation_*_payment`) and
//!   `apply_federation_journal_entries` for the accepted operation. Swap, mint
//!   and melt envelopes are built raw, without the admission builders, so the
//!   application rules decide every vector. Entries are ordered directly
//!   (`accepted_entry_without_journal`), not through the consensus journal,
//!   so consensus-level duplicate and conflict rejection does not hide the
//!   application rule under test.
//!
//! Both mints use the same amounts (powers of two up to 128 sat), the same
//! input fee (`INPUT_FEE_PPK`) and the same scripted payment backend
//! ([`ParityPaymentProcessor`]). A vector is a description (amounts, faults);
//! each side materializes it against its own keysets.
//!
//! # What is compared
//!
//! - **Signatures.** The keys differ (a standalone signatory keyset against
//!   aggregate BLS keys), so signature bytes and keyset ids are not
//!   comparable. Compared per output position: whether it was signed, its
//!   amount, the keyset's position in activation order (initial or rotated),
//!   and whether the unblinded proofs verify against the mint's published
//!   keys. The federation uses a signature threshold of 1, so the member's
//!   stored share is the aggregate signature; with a higher threshold,
//!   aggregation happens in the wallet and is outside these paths. DLEQ proofs
//!   exist only for secp256k1 keysets and are not compared.
//! - **Fees and change.** Every completed operation's kind, issued and
//!   redeemed amounts, fee collected, payment amount and payment fee; the melt
//!   change amounts through the signature comparison. Operation ids and
//!   completion times are excluded: federated application derives ids from
//!   the operation id and records time zero, deliberately, for deterministic
//!   replay.
//! - **Spent proofs.** The stored state of each request input, by position
//!   (the Ys themselves differ because each side mints its own proofs).
//! - **Quotes.** Mint quote state, amount, amount paid and amount issued; melt
//!   quote state, amount, fee reserve and payment proof.
//! - **Errors.** The error variant, its message, and whether a member would
//!   record it as a definitive rejection (`is_definitive_federation_apply_rejection`).
//!
//! # Divergences fixed
//!
//! The first run of this test found four, all in federated application and
//! fixed there: outputs on an unknown or inactive keyset now report the
//! standalone errors (and every output-validation failure is a definitive
//! rejection), and a Bolt11 mint must issue exactly the quote amount.
//!
//! # Excluded on purpose
//!
//! - Idempotent replay of the same accepted operation: federated application
//!   treats it as already applied, a standalone mint has no such notion.
//! - Consensus duplicate and conflict keys (a second mint or melt on the same
//!   quote inside the validity window): a federated layer with no standalone
//!   counterpart, covered by `replay_window` and the `cdk-common` journal tests.
//! - Admission checks (`swap_operation`, `mint_operation`, `melt_operation`):
//!   they run before ordering and only reject earlier.
//! - Batch-size, NUT-29 method and NUT-04 limit checks, which read the local
//!   mint info and run at admission in federation mode (finding 1 of the plan).
//!   Batch mints are therefore not covered: a standalone mint rejects them
//!   unless NUT-29 is configured.

use std::mem::Discriminant;
use std::sync::Mutex as StdMutex;

use cdk_common::amount::SplitTarget;
use cdk_common::dhke::construct_proofs;
use cdk_common::mint::OperationKind;
use cdk_common::nuts::{Keys, PreMintSecrets};
use cdk_common::payment::Bolt11OutgoingPaymentOptions;
use cdk_common::Proofs;

use super::super::helpers::mint_request_from_input;
use super::*;
use crate::mint::{is_definitive_federation_apply_rejection, UnitConfig};

/// Keyset amounts on both sides.
const AMOUNTS: [u64; 8] = [1, 2, 4, 8, 16, 32, 64, 128];
/// Input fee on both sides: one sat for up to ten inputs.
const INPUT_FEE_PPK: u64 = 100;
/// Fee reserve the scripted backend quotes for every melt.
const MELT_FEE_RESERVE: u64 = 4;
/// Routing fee the scripted backend reports for every paid melt.
const MELT_ROUTING_FEE: u64 = 1;
/// Payment proof the scripted backend reports for every paid melt.
const MELT_PAYMENT_PROOF: &str = "parity-payment-proof";

/// A payment backend whose answers the test scripts.
///
/// Incoming payments are reported only after [`Self::mark_paid`]. Outgoing
/// payments always succeed with [`MELT_ROUTING_FEE`].
#[derive(Debug, Default)]
struct ParityPaymentProcessor {
    incoming: StdMutex<HashMap<String, WaitPaymentResponse>>,
    outgoing: StdMutex<HashMap<String, MakePaymentResponse>>,
}

impl ParityPaymentProcessor {
    fn mark_paid(&self, lookup_id: &PaymentIdentifier, amount: u64) {
        self.incoming
            .lock()
            .expect("incoming payments lock")
            .insert(
                lookup_id.to_string(),
                WaitPaymentResponse {
                    payment_identifier: lookup_id.clone(),
                    payment_amount: Amount::new(amount, CurrencyUnit::Sat),
                    payment_id: format!("parity-payment-{lookup_id}"),
                },
            );
    }

    fn bolt11_amount(
        options: &Bolt11OutgoingPaymentOptions,
        unit: &CurrencyUnit,
    ) -> Result<Amount<CurrencyUnit>, payment::Error> {
        let amount_msat = options.bolt11.amount_milli_satoshis().ok_or_else(|| {
            payment::Error::Custom("parity backend requires an amount".to_string())
        })?;
        Ok(Amount::new(amount_msat, CurrencyUnit::Msat).convert_to(unit)?)
    }
}

#[async_trait]
impl MintPayment for ParityPaymentProcessor {
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
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let IncomingPaymentOptions::Bolt11(options) = options else {
            return Err(payment::Error::UnsupportedPaymentOption);
        };
        let amount_msat = options.amount.convert_to(&CurrencyUnit::Msat)?.value();
        let invoice = cdk_fake_wallet::create_fake_invoice(amount_msat, "parity".to_string());
        Ok(CreateIncomingPaymentResponse {
            request_lookup_id: PaymentIdentifier::PaymentHash(*invoice.payment_hash().as_ref()),
            request: invoice.to_string(),
            expiry: None,
            extra_json: None,
        })
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let OutgoingPaymentOptions::Bolt11(options) = options else {
            return Err(payment::Error::UnsupportedPaymentOption);
        };
        Ok(PaymentQuoteResponse {
            request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                *options.bolt11.payment_hash().as_ref(),
            )),
            amount: Self::bolt11_amount(&options, unit)?,
            fee: Amount::new(MELT_FEE_RESERVE, unit.clone()),
            state: MeltQuoteState::Unpaid,
            extra_json: None,
            estimated_blocks: None,
            fee_options: None,
        })
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let OutgoingPaymentOptions::Bolt11(options) = options else {
            return Err(payment::Error::UnsupportedPaymentOption);
        };
        let amount = Self::bolt11_amount(&options, unit)?;
        let lookup_id = PaymentIdentifier::PaymentHash(*options.bolt11.payment_hash().as_ref());
        let response = MakePaymentResponse {
            payment_lookup_id: lookup_id.clone(),
            payment_proof: Some(MELT_PAYMENT_PROOF.to_string()),
            status: MeltQuoteState::Paid,
            total_spent: Amount::new(amount.value() + MELT_ROUTING_FEE, unit.clone()),
        };
        self.outgoing
            .lock()
            .expect("outgoing payments lock")
            .insert(lookup_id.to_string(), response.clone());
        Ok(response)
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
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        Ok(self
            .incoming
            .lock()
            .expect("incoming payments lock")
            .get(&payment_identifier.to_string())
            .cloned()
            .into_iter()
            .collect())
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        Ok(self
            .outgoing
            .lock()
            .expect("outgoing payments lock")
            .get(&payment_identifier.to_string())
            .cloned()
            .unwrap_or_else(|| MakePaymentResponse {
                payment_lookup_id: payment_identifier.clone(),
                payment_proof: None,
                status: MeltQuoteState::Unpaid,
                total_spent: Amount::new(0, CurrencyUnit::Sat),
            }))
    }
}

/// A federated keyset over [`AMOUNTS`] with one coefficient per amount, so
/// the signature threshold is 1.
fn parity_keyset_spec(active: bool, seed: u8) -> TrustedDealerKeysetSpec {
    TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        active,
        INPUT_FEE_PPK,
        None,
        AMOUNTS
            .iter()
            .zip(seed..)
            .map(|(amount, seed)| {
                (
                    Amount::from(*amount),
                    vec![BlsSecretKey::from_reduced_bytes(&[seed; 32])],
                )
            })
            .collect(),
    )
}

/// Two members, signature threshold 1, an active keyset and an inactive one
/// for rotation.
fn parity_runtime_config() -> FederationRuntimeConfig {
    let members = (1..=2)
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
        .collect();
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(2, 1).expect("params"),
        members,
        vec![parity_keyset_spec(true, 20), parity_keyset_spec(false, 60)],
    )
    .expect("trusted dealer bundle");
    let private_config = bundle
        .private_configs
        .get(&MemberId::new(1).expect("member id"))
        .expect("private config")
        .clone();

    FederationRuntimeConfig::new(bundle.public_config, private_config).expect("runtime config")
}

/// Error identity compared across the two paths.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ErrorKind {
    variant: Discriminant<Error>,
    message: String,
    /// Whether a federation member would record this error as the
    /// operation's deterministic rejection rather than a retryable failure.
    definitive: bool,
}

impl ErrorKind {
    fn of(error: &Error) -> Self {
        Self {
            variant: std::mem::discriminant(error),
            message: error.to_string(),
            definitive: is_definitive_federation_apply_rejection(error),
        }
    }
}

/// One output after a request: its amount and the activation position of
/// its keyset, or `None` if it was not signed.
type SignedOutput = Option<(u64, usize)>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SignedOutputs {
    outputs: Vec<SignedOutput>,
    /// Whether every signed output unblinds to a proof the mint verifies.
    proofs_verify: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OperationSummary {
    kind: String,
    total_issued: u64,
    total_redeemed: u64,
    fee_collected: u64,
    payment_amount: Option<u64>,
    payment_fee: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MintQuoteSummary {
    state: MintQuoteState,
    amount: Option<u64>,
    amount_paid: u64,
    amount_issued: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MeltQuoteSummary {
    state: MeltQuoteState,
    amount: u64,
    fee_reserve: u64,
    payment_proof: Option<String>,
}

/// Everything compared after one step of a vector.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    step: &'static str,
    result: Result<(), ErrorKind>,
    outputs: SignedOutputs,
    input_states: Vec<Option<State>>,
    mint_quote: Option<MintQuoteSummary>,
    melt_quote: Option<MeltQuoteSummary>,
    operations: Vec<OperationSummary>,
}

#[derive(Debug)]
enum Path {
    Standalone,
    Federated {
        config: Box<FederationRuntimeConfig>,
        /// Index of the next ordered entry.
        next_index: u64,
        /// Order digest the next ordered entry chains to.
        order_digest: FederationJournalDigest,
    },
}

/// One mint driven through either path.
#[derive(Debug)]
struct Driver {
    path: Path,
    mint: Mint,
    localstore: Arc<MintSqliteDatabase>,
    processor: Arc<ParityPaymentProcessor>,
    /// Keysets in activation order.
    keysets: Vec<Id>,
}

/// A mint quote the driver created.
#[derive(Debug, Clone)]
struct ParityMintQuote {
    id: QuoteId,
    lookup_id: PaymentIdentifier,
    amount: u64,
}

impl Driver {
    async fn standalone() -> Self {
        let localstore = Arc::new(memory::empty().await.expect("mint db"));
        let processor = Arc::new(ParityPaymentProcessor::default());
        let mut builder = MintBuilder::new(localstore.clone());
        builder
            .configure_unit(
                CurrencyUnit::Sat,
                UnitConfig {
                    amounts: AMOUNTS.to_vec(),
                    input_fee_ppk: INPUT_FEE_PPK,
                },
            )
            .expect("unit config");
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                processor.clone(),
            )
            .await
            .expect("payment processor");
        let mint = builder
            .build_with_seed(localstore.clone(), &[9; 64])
            .await
            .expect("standalone mint");
        Self::new(Path::Standalone, mint, localstore, processor)
    }

    async fn federated() -> Self {
        let config = parity_runtime_config();
        let localstore = Arc::new(memory::empty().await.expect("mint db"));
        let processor = Arc::new(ParityPaymentProcessor::default());
        let mut builder =
            MintBuilder::new(localstore.clone()).with_federation_config(config.clone());
        #[cfg(feature = "federation-bark")]
        builder.set_federation_bark_store(localstore.clone());
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                processor.clone(),
            )
            .await
            .expect("payment processor");
        let mint = builder
            .build_with_seed(localstore.clone(), &[9; 64])
            .await
            .expect("federated mint");
        Self::new(
            Path::Federated {
                config: Box::new(config),
                next_index: 0,
                order_digest: InMemoryFederationJournal::new().latest_order_digest(),
            },
            mint,
            localstore,
            processor,
        )
    }

    fn new(
        path: Path,
        mint: Mint,
        localstore: Arc<MintSqliteDatabase>,
        processor: Arc<ParityPaymentProcessor>,
    ) -> Self {
        let mut driver = Self {
            path,
            mint,
            localstore,
            processor,
            keysets: Vec::new(),
        };
        let active = driver.active_keyset();
        let keyset = driver.mint.keyset_pubkeys(&active).expect("active keyset");
        assert_eq!(
            keyset.keysets[0].input_fee_ppk, INPUT_FEE_PPK,
            "both sides must charge the same input fee"
        );
        assert_eq!(
            keyset.keysets[0]
                .keys
                .iter()
                .map(|(amount, _)| amount.to_u64())
                .collect::<Vec<_>>(),
            AMOUNTS.to_vec(),
            "both sides must sign the same amounts"
        );
        driver.keysets.push(active);
        driver
    }

    fn active_keyset(&self) -> Id {
        *self
            .mint
            .get_active_keysets()
            .get(&CurrencyUnit::Sat)
            .expect("active sat keyset")
    }

    /// A keyset id of the active keyset's version that no keyset has.
    fn unknown_keyset(&self) -> Id {
        let mut bytes = self.active_keyset().to_bytes();
        let last = bytes.last_mut().expect("keyset id bytes");
        *last ^= 0xff;
        Id::from_bytes(&bytes).expect("unknown keyset id")
    }

    fn keys(&self, keyset: &Id) -> Keys {
        self.mint
            .keyset_pubkeys(keyset)
            .expect("keyset keys")
            .keysets
            .first()
            .expect("keyset")
            .keys
            .clone()
    }

    async fn federation_config(&self) -> Arc<FederationRuntimeConfig> {
        self.mint
            .effective_federation_config()
            .await
            .expect("effective config")
            .expect("federation config")
    }

    /// Order `operation` after every earlier entry and apply it.
    ///
    /// The entry is chained to the previous one but not checked for
    /// consensus conflicts, so the application rule decides.
    async fn apply(&mut self, operation: FederationOperation) -> Result<(), Error> {
        let Path::Federated {
            config,
            next_index,
            order_digest,
        } = &mut self.path
        else {
            panic!("only federated drivers order operations");
        };
        let mut entry =
            accepted_entry_without_journal(*next_index, config.operation_envelope(operation));
        let mut next_digest = [0xa5; 32];
        next_digest[..8].copy_from_slice(&next_index.to_be_bytes());
        entry.previous_order_digest = *order_digest;
        entry.order_digest = FederationJournalDigest::from_bytes(next_digest);
        *order_digest = entry.order_digest;
        *next_index += 1;
        persist_journal_entry(&self.localstore, &entry).await;
        Box::pin(
            self.mint
                .apply_federation_journal_entries(std::slice::from_ref(&entry)),
        )
        .await
        .map(drop)
    }

    /// Payment observers needed for a quorum in `context`.
    fn required_observers(&self, context: FederationPaymentObservationContext) -> Vec<MemberId> {
        let Path::Federated { config, .. } = &self.path else {
            panic!("only federated drivers observe payments");
        };
        let required = config
            .public_config()
            .payment_verification_policy(context)
            .required_matching_observations();
        config
            .public_config()
            .members
            .keys()
            .copied()
            .take(usize::from(required))
            .collect()
    }

    /// Rotate to a new active keyset with the same amounts and fee.
    async fn rotate_keyset(&mut self) {
        match &self.path {
            Path::Standalone => {
                self.mint
                    .rotate_keyset(
                        CurrencyUnit::Sat,
                        AMOUNTS.to_vec(),
                        INPUT_FEE_PPK,
                        true,
                        None,
                    )
                    .await
                    .expect("standalone rotation");
            }
            Path::Federated { ref config, .. } => {
                let mut keyset = config
                    .public_config()
                    .keysets
                    .iter()
                    .find(|keyset| !keyset.active)
                    .expect("inactive federated keyset")
                    .clone();
                keyset.active = true;
                self.apply(FederationOperation::KeysetRotation(
                    FederationKeysetRotationOperation { keyset },
                ))
                .await
                .expect("federated rotation");
            }
        }
        let active = self.active_keyset();
        assert!(
            !self.keysets.contains(&active),
            "rotation activates a new keyset"
        );
        self.keysets.push(active);
    }

    async fn create_mint_quote(&mut self, amount: u64) -> ParityMintQuote {
        let request = bolt11_quote_request(amount);
        let id = match &self.path {
            Path::Standalone => {
                let response: MintQuoteResponse<String> = self
                    .mint
                    .get_mint_quote(request)
                    .await
                    .expect("standalone mint quote")
                    .into();
                QuoteId::from_str(response.quote()).expect("quote id")
            }
            Path::Federated { config, .. } => {
                let (response, lookup_id) = self
                    .mint
                    .prepare_mint_quote(request.clone())
                    .await
                    .expect("prepared mint quote");
                let response: MintQuoteResponse<String> = response.into();
                let id = QuoteId::from_str(response.quote()).expect("quote id");
                let envelope = config.mint_quote_operation_with_lookup_id(
                    id.clone(),
                    lookup_id,
                    request,
                    response,
                );
                self.apply(envelope.operation)
                    .await
                    .expect("federated mint quote");
                id
            }
        };
        let lookup_id = self
            .localstore
            .get_mint_quote(&id)
            .await
            .expect("mint quote lookup")
            .expect("mint quote")
            .request_lookup_id
            .clone();
        ParityMintQuote {
            id,
            lookup_id,
            amount,
        }
    }

    /// Pay `quote` at the backend and let the mint observe it the way it
    /// does in production: a status check, or a member payment quorum.
    async fn pay_mint_quote(&mut self, quote: &ParityMintQuote) {
        self.processor.mark_paid(&quote.lookup_id, quote.amount);
        match self.path {
            Path::Standalone => {
                self.mint
                    .check_mint_quote(&quote.id)
                    .await
                    .expect("standalone quote check");
            }
            Path::Federated { .. } => {
                let observation = self
                    .mint
                    .probe_federation_mint_quote_payment(&quote.id)
                    .await
                    .expect("mint payment probe")
                    .expect("paid mint quote observation");
                for observer in
                    self.required_observers(FederationPaymentObservationContext::MintQuote)
                {
                    let mut observation = observation.clone();
                    observation.observer = observer;
                    self.apply(FederationOperation::MintQuotePayment(observation))
                        .await
                        .expect("mint payment observation");
                }
            }
        }
    }

    async fn mint(&mut self, input: MintInput) -> Result<(), Error> {
        match self.path {
            Path::Standalone => self.mint.process_mint_request(input).await.map(drop),
            Path::Federated { .. } => {
                self.apply(FederationOperation::Mint(FederationMintOperation {
                    request: mint_request_from_input(input),
                }))
                .await
            }
        }
    }

    async fn swap(&mut self, request: SwapRequest) -> Result<(), Error> {
        match self.path {
            Path::Standalone => self.mint.process_swap_request(request).await.map(drop),
            Path::Federated { .. } => {
                self.apply(FederationOperation::Swap(FederationSwapOperation {
                    request,
                }))
                .await
            }
        }
    }

    async fn create_melt_quote(&mut self, amount: u64) -> QuoteId {
        let request = MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: cdk_fake_wallet::create_fake_invoice(
                amount * 1_000,
                "parity melt".to_string(),
            ),
            unit: CurrencyUnit::Sat,
            options: None,
        });
        match &self.path {
            Path::Standalone => self
                .mint
                .get_melt_quote(request)
                .await
                .expect("standalone melt quote")
                .quote()
                .expect("melt quote id")
                .clone(),
            Path::Federated { config, .. } => {
                let (response, metadata) = self
                    .mint
                    .prepare_melt_quote(request.clone())
                    .await
                    .expect("prepared melt quote");
                let id = response.quote().expect("melt quote id").clone();
                let envelope = config.melt_quote_operation(id.clone(), request, response, metadata);
                self.apply(envelope.operation)
                    .await
                    .expect("federated melt quote");
                id
            }
        }
    }

    /// Melt and wait for the payment outcome: the standalone saga's
    /// completion, or the payment actor's probe and a member quorum.
    async fn melt(&mut self, request: MeltRequest<QuoteId>) -> Result<(), Error> {
        match self.path {
            Path::Standalone => {
                let pending = Box::pin(self.mint.melt(&request)).await?;
                pending.await.map(drop)
            }
            Path::Federated { .. } => {
                let quote = request.quote().clone();
                self.apply(FederationOperation::Melt(FederationMeltOperation {
                    request: MeltRequest::new(
                        quote.to_string(),
                        request.inputs().clone(),
                        request.outputs().clone(),
                    ),
                }))
                .await?;
                let observation = Box::pin(self.mint.probe_federation_melt_quote_payment(&quote))
                    .await
                    .expect("melt payment probe")
                    .expect("paid melt observation");
                for observer in
                    self.required_observers(FederationPaymentObservationContext::MeltQuote)
                {
                    let mut observation = observation.clone();
                    observation.observer = observer;
                    self.apply(FederationOperation::MeltQuotePayment(observation))
                        .await
                        .expect("melt payment observation");
                }
                Ok(())
            }
        }
    }

    /// Mint proofs of exactly `amounts` through this side's mint path.
    async fn fund(&mut self, amounts: &[u64]) -> Proofs {
        let quote = self.create_mint_quote(amounts.iter().sum()).await;
        self.pay_mint_quote(&quote).await;
        let outputs = outputs(self.active_keyset(), amounts);
        self.mint(single_mint(&quote, &outputs))
            .await
            .expect("funding mint");
        let signatures = self.signatures(&outputs).await;
        let keyset = outputs.keyset_id;
        let proofs = construct_proofs(
            signatures
                .into_iter()
                .map(|signature| signature.expect("funding signature"))
                .collect(),
            outputs.rs(),
            outputs.secrets(),
            &self.keys(&keyset),
        )
        .expect("funding proofs");

        // The signature comparison relies on `verify` telling valid proofs
        // from invalid ones on both sides.
        assert!(self.verify(proofs.clone()).await, "funded proofs verify");
        let mut forged = proofs[0].clone();
        forged.secret = Secret::new("parity forged secret");
        assert!(!self.verify(vec![forged]).await, "forged proof is rejected");

        proofs
    }

    async fn signatures(&self, outputs: &PreMintSecrets) -> Vec<Option<BlindSignature>> {
        let blinded_secrets = outputs
            .blinded_messages()
            .iter()
            .map(|output| output.blinded_secret)
            .collect::<Vec<_>>();
        self.localstore
            .get_blind_signatures(&blinded_secrets)
            .await
            .expect("blind signatures")
    }

    async fn verify(&self, proofs: Proofs) -> bool {
        match &self.path {
            Path::Standalone => self.mint.verify_proofs(proofs).await.is_ok(),
            Path::Federated { .. } => self
                .federation_config()
                .await
                .verify_proofs(&proofs)
                .is_ok(),
        }
    }

    async fn signed_outputs(&self, outputs: &PreMintSecrets) -> SignedOutputs {
        let signatures = self.signatures(outputs).await;
        let mut proofs_verify = true;
        let mut signed = Vec::with_capacity(signatures.len());
        for (signature, premint) in signatures.into_iter().zip(outputs.iter()) {
            let Some(signature) = signature else {
                signed.push(None);
                continue;
            };
            let slot = self
                .keysets
                .iter()
                .position(|keyset| *keyset == signature.keyset_id)
                .expect("signature from a known keyset");
            signed.push(Some((signature.amount.to_u64(), slot)));
            let keys = self.keys(&signature.keyset_id);
            let proofs = construct_proofs(
                vec![signature],
                vec![premint.r.clone()],
                vec![premint.secret.clone()],
                &keys,
            )
            .expect("unblinded proof");
            proofs_verify &= self.verify(proofs).await;
        }
        SignedOutputs {
            outputs: signed,
            proofs_verify,
        }
    }

    async fn operations(&self) -> Vec<OperationSummary> {
        let mut operations = self
            .localstore
            .get_completed_operations()
            .await
            .expect("completed operations")
            .into_iter()
            .map(|operation| OperationSummary {
                kind: operation.kind().to_string(),
                total_issued: operation.total_issued().to_u64(),
                total_redeemed: operation.total_redeemed().to_u64(),
                fee_collected: operation.fee_collected().to_u64(),
                payment_amount: operation.payment_amount().map(|amount| amount.to_u64()),
                payment_fee: operation.payment_fee().map(|amount| amount.to_u64()),
            })
            .collect::<Vec<_>>();
        operations.sort();
        operations
    }

    async fn mint_quote_summary(&self, quote: &QuoteId) -> MintQuoteSummary {
        let quote = self
            .localstore
            .get_mint_quote(quote)
            .await
            .expect("mint quote lookup")
            .expect("mint quote");
        MintQuoteSummary {
            state: quote.state(),
            amount: quote.amount.as_ref().map(|amount| amount.value()),
            amount_paid: quote.amount_paid().value(),
            amount_issued: quote.amount_issued().value(),
        }
    }

    async fn melt_quote_summary(&self, quote: &QuoteId) -> MeltQuoteSummary {
        let quote = self
            .localstore
            .get_melt_quote(quote)
            .await
            .expect("melt quote lookup")
            .expect("melt quote");
        MeltQuoteSummary {
            state: quote.state,
            amount: quote.amount().value(),
            fee_reserve: quote.fee_reserve().value(),
            payment_proof: quote.payment_proof.clone(),
        }
    }

    /// Record everything compared after a step.
    async fn observe(&self, step: Step<'_>) -> Observed {
        let input_states = match step.inputs.is_empty() {
            true => Vec::new(),
            false => self
                .localstore
                .get_proofs_states(&step.inputs.ys().expect("input ys"))
                .await
                .expect("input states"),
        };
        let mint_quote = match step.mint_quote {
            Some(quote) => Some(self.mint_quote_summary(quote).await),
            None => None,
        };
        let melt_quote = match step.melt_quote {
            Some(quote) => Some(self.melt_quote_summary(quote).await),
            None => None,
        };
        Observed {
            step: step.name,
            result: step.result.as_ref().map(drop).map_err(ErrorKind::of),
            outputs: self.signed_outputs(step.outputs).await,
            input_states,
            mint_quote,
            melt_quote,
            operations: self.operations().await,
        }
    }
}

/// What a step touched, for [`Driver::observe`].
struct Step<'a> {
    name: &'static str,
    result: &'a Result<(), Error>,
    inputs: &'a Proofs,
    outputs: &'a PreMintSecrets,
    mint_quote: Option<&'a QuoteId>,
    melt_quote: Option<&'a QuoteId>,
}

impl<'a> Step<'a> {
    fn new(
        name: &'static str,
        result: &'a Result<(), Error>,
        inputs: &'a Proofs,
        outputs: &'a PreMintSecrets,
    ) -> Self {
        Self {
            name,
            result,
            inputs,
            outputs,
            mint_quote: None,
            melt_quote: None,
        }
    }

    fn mint_quote(mut self, quote: &'a QuoteId) -> Self {
        self.mint_quote = Some(quote);
        self
    }

    fn melt_quote(mut self, quote: &'a QuoteId) -> Self {
        self.melt_quote = Some(quote);
        self
    }
}

/// Fresh outputs of exactly `amounts` on `keyset`.
fn outputs(keyset: Id, amounts: &[u64]) -> PreMintSecrets {
    let fee_and_amounts = (0, AMOUNTS.to_vec()).into();
    let mut outputs = PreMintSecrets::new(keyset);
    for amount in amounts {
        let single = PreMintSecrets::random(
            keyset,
            Amount::from(*amount),
            &SplitTarget::None,
            &fee_and_amounts,
        )
        .expect("output");
        assert_eq!(single.len(), 1, "output amounts must be keyset amounts");
        outputs.combine(single);
    }
    outputs
}

fn single_mint(quote: &ParityMintQuote, outputs: &PreMintSecrets) -> MintInput {
    MintInput::Single(MintRequest {
        quote: quote.id.clone(),
        outputs: outputs.blinded_messages(),
        signature: None,
    })
}

fn select(proofs: &Proofs, amounts: &[u64]) -> Proofs {
    let mut remaining = proofs.clone();
    amounts
        .iter()
        .map(|amount| {
            let position = remaining
                .iter()
                .position(|proof| proof.amount.to_u64() == *amount)
                .expect("funded proof of amount");
            remaining.remove(position)
        })
        .collect()
}

/// Run `vector` on a standalone and a federated mint and return both runs.
async fn run_both<F>(vector: F) -> (Vec<Observed>, Vec<Observed>)
where
    F: for<'a> Fn(&'a mut Driver) -> Pin<Box<dyn std::future::Future<Output = Vec<Observed>> + 'a>>,
{
    let mut standalone = Box::pin(Driver::standalone()).await;
    let standalone_run = vector(&mut standalone).await;
    let mut federated = Box::pin(Driver::federated()).await;
    let federated_run = vector(&mut federated).await;
    (standalone_run, federated_run)
}

/// Assert that both runs agree step by step.
fn assert_parity(standalone: &[Observed], federated: &[Observed]) {
    assert_eq!(standalone.len(), federated.len(), "step count");
    for (standalone, federated) in standalone.iter().zip(federated) {
        assert_eq!(
            standalone, federated,
            "standalone and federated outcomes differ at step {}",
            standalone.step
        );
    }
}

fn step<'a>(run: &'a [Observed], name: &str) -> &'a Observed {
    run.iter()
        .find(|observed| observed.step == name)
        .expect("step in run")
}

fn assert_error(observed: &Observed, expected: &Error) {
    let error = observed.result.as_ref().expect_err("step must be rejected");
    assert_eq!(
        error.variant,
        std::mem::discriminant(expected),
        "step {} rejected with {}",
        observed.step,
        error.message
    );
}

impl Observed {
    fn message(&self) -> &str {
        self.result
            .as_ref()
            .map_or_else(|error| error.message.as_str(), |()| "")
    }
}

fn assert_ok(observed: &Observed) {
    assert_eq!(
        observed.result,
        Ok(()),
        "step {} must succeed",
        observed.step
    );
}

/// Swap `inputs` (amounts of funded proofs) into fresh `outputs` amounts.
async fn swap_step(
    driver: &mut Driver,
    name: &'static str,
    inputs: &Proofs,
    outputs: &PreMintSecrets,
) -> Observed {
    let result = driver
        .swap(SwapRequest::new(inputs.clone(), outputs.blinded_messages()))
        .await;
    driver
        .observe(Step::new(name, &result, inputs, outputs))
        .await
}

#[tokio::test]
async fn swap_valid() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[8, 4]).await;
            let outputs = outputs(driver.active_keyset(), &[8, 2, 1]);
            vec![swap_step(driver, "swap", &funded, &outputs).await]
        })
    })
    .await;

    let swap = step(&standalone, "swap");
    assert_ok(swap);
    assert_eq!(
        swap.outputs,
        SignedOutputs {
            outputs: vec![Some((8, 0)), Some((2, 0)), Some((1, 0))],
            proofs_verify: true,
        }
    );
    assert_eq!(swap.input_states, vec![Some(State::Spent); 2]);
    assert!(swap.operations.contains(&OperationSummary {
        kind: OperationKind::Swap.to_string(),
        total_issued: 11,
        total_redeemed: 12,
        fee_collected: 1,
        payment_amount: None,
        payment_fee: None,
    }));
    assert_parity(&standalone, &federated);
}

#[tokio::test]
async fn swap_without_fee_is_unbalanced() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[8, 4]).await;
            let outputs = outputs(driver.active_keyset(), &[8, 4]);
            vec![swap_step(driver, "swap", &funded, &outputs).await]
        })
    })
    .await;

    let swap = step(&standalone, "swap");
    assert_error(swap, &Error::TransactionUnbalanced(0, 0, 0));
    assert_eq!(swap.input_states, vec![None; 2]);
    assert_eq!(swap.outputs.outputs, vec![None; 2]);
    assert_parity(&standalone, &federated);
}

#[tokio::test]
async fn swap_with_insufficient_inputs() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[4]).await;
            let outputs = outputs(driver.active_keyset(), &[8]);
            vec![swap_step(driver, "swap", &funded, &outputs).await]
        })
    })
    .await;

    let swap = step(&standalone, "swap");
    assert_error(swap, &Error::TransactionUnbalanced(0, 0, 0));
    assert_eq!(swap.input_states, vec![None]);
    assert_parity(&standalone, &federated);
}

#[tokio::test]
async fn swap_paying_more_than_the_fee_is_unbalanced() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[8, 4]).await;
            let outputs = outputs(driver.active_keyset(), &[8]);
            vec![swap_step(driver, "swap", &funded, &outputs).await]
        })
    })
    .await;

    let swap = step(&standalone, "swap");
    assert_error(swap, &Error::TransactionUnbalanced(0, 0, 0));
    assert_eq!(swap.input_states, vec![None; 2]);
    assert_parity(&standalone, &federated);
}

#[tokio::test]
async fn swap_double_spend() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[8, 4]).await;
            let first = outputs(driver.active_keyset(), &[8, 2, 1]);
            let second = outputs(driver.active_keyset(), &[8, 2, 1]);
            vec![
                swap_step(driver, "first spend", &funded, &first).await,
                swap_step(driver, "second spend", &funded, &second).await,
            ]
        })
    })
    .await;

    assert_ok(step(&standalone, "first spend"));
    let second = step(&standalone, "second spend");
    assert_error(second, &Error::TokenAlreadySpent);
    assert_eq!(second.input_states, vec![Some(State::Spent); 2]);
    assert_eq!(second.outputs.outputs, vec![None; 3]);
    assert_parity(&standalone, &federated);
}

#[tokio::test]
async fn swap_duplicate_outputs() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[8, 4]).await;
            let mut duplicated = outputs(driver.active_keyset(), &[4, 2, 1]);
            let repeated = duplicated.secrets[0].clone();
            duplicated.secrets.insert(0, repeated);
            vec![swap_step(driver, "swap", &funded, &duplicated).await]
        })
    })
    .await;

    let swap = step(&standalone, "swap");
    assert_error(swap, &Error::DuplicateOutputs);
    assert_eq!(swap.input_states, vec![None; 2]);
    assert_parity(&standalone, &federated);
}

#[tokio::test]
async fn swap_duplicate_inputs() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[8]).await;
            let inputs = vec![funded[0].clone(), funded[0].clone()];
            let outputs = outputs(driver.active_keyset(), &[8, 4, 2, 1]);
            vec![swap_step(driver, "swap", &inputs, &outputs).await]
        })
    })
    .await;

    let swap = step(&standalone, "swap");
    assert_error(swap, &Error::DuplicateInputs);
    assert_eq!(swap.input_states, vec![None; 2]);
    assert_parity(&standalone, &federated);
}

#[tokio::test]
async fn swap_to_unknown_keyset() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[8, 4]).await;
            let outputs = outputs(driver.unknown_keyset(), &[8, 2, 1]);
            vec![swap_step(driver, "swap", &funded, &outputs).await]
        })
    })
    .await;

    let swap = step(&standalone, "swap");
    assert_error(swap, &Error::UnknownKeySet);
    assert_eq!(swap.input_states, vec![None; 2]);
    assert_parity(&standalone, &federated);
}

const INACTIVE_OUTPUTS: &str = "outputs on inactive keyset";

#[tokio::test]
async fn swap_across_keyset_rotation() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[8, 4, 16]).await;
            let previous = driver.active_keyset();
            driver.rotate_keyset().await;
            let current = driver.active_keyset();
            let to_inactive = outputs(previous, &[8, 2, 1]);
            let to_active = outputs(current, &[8, 2, 1]);
            let old_inputs = select(&funded, &[8, 4]);
            let fresh_inputs = select(&funded, &[16]);
            let to_active_from_fresh = outputs(current, &[8, 4, 2, 1]);
            vec![
                swap_step(driver, INACTIVE_OUTPUTS, &old_inputs, &to_inactive).await,
                swap_step(
                    driver,
                    "inputs from inactive keyset",
                    &old_inputs,
                    &to_active,
                )
                .await,
                swap_step(
                    driver,
                    "second inactive-keyset input",
                    &fresh_inputs,
                    &to_active_from_fresh,
                )
                .await,
            ]
        })
    })
    .await;

    let rejected = step(&standalone, INACTIVE_OUTPUTS);
    assert_error(rejected, &Error::InactiveKeyset);
    assert_eq!(rejected.input_states, vec![None; 2]);
    let accepted = step(&standalone, "inputs from inactive keyset");
    assert_ok(accepted);
    assert_eq!(
        accepted.outputs,
        SignedOutputs {
            outputs: vec![Some((8, 1)), Some((2, 1)), Some((1, 1))],
            proofs_verify: true,
        }
    );
    assert_eq!(accepted.input_states, vec![Some(State::Spent); 2]);
    assert_ok(step(&standalone, "second inactive-keyset input"));
    assert_parity(&standalone, &federated);
}

/// A mint attempt on a quote, observed with the quote.
async fn mint_step(
    driver: &mut Driver,
    name: &'static str,
    quote: &ParityMintQuote,
    outputs: &PreMintSecrets,
) -> Observed {
    let result = driver.mint(single_mint(quote, outputs)).await;
    driver
        .observe(Step::new(name, &result, &Vec::new(), outputs).mint_quote(&quote.id))
        .await
}

#[tokio::test]
async fn mint_unpaid_quote() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let quote = driver.create_mint_quote(15).await;
            let outputs = outputs(driver.active_keyset(), &[8, 4, 2, 1]);
            vec![mint_step(driver, "mint", &quote, &outputs).await]
        })
    })
    .await;

    let mint = step(&standalone, "mint");
    assert_error(mint, &Error::UnpaidQuote);
    assert_eq!(
        mint.mint_quote,
        Some(MintQuoteSummary {
            state: MintQuoteState::Unpaid,
            amount: Some(15),
            amount_paid: 0,
            amount_issued: 0,
        })
    );
    assert_parity(&standalone, &federated);
}

#[tokio::test]
async fn mint_paid_quote_then_issued_quote() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let quote = driver.create_mint_quote(15).await;
            driver.pay_mint_quote(&quote).await;
            let first = outputs(driver.active_keyset(), &[8, 4, 2, 1]);
            let second = outputs(driver.active_keyset(), &[8, 4, 2, 1]);
            vec![
                mint_step(driver, "mint paid quote", &quote, &first).await,
                mint_step(driver, "mint issued quote", &quote, &second).await,
            ]
        })
    })
    .await;

    let paid = step(&standalone, "mint paid quote");
    assert_ok(paid);
    assert_eq!(
        paid.outputs,
        SignedOutputs {
            outputs: vec![Some((8, 0)), Some((4, 0)), Some((2, 0)), Some((1, 0))],
            proofs_verify: true,
        }
    );
    assert_eq!(
        paid.mint_quote,
        Some(MintQuoteSummary {
            state: MintQuoteState::Issued,
            amount: Some(15),
            amount_paid: 15,
            amount_issued: 15,
        })
    );
    let issued = step(&standalone, "mint issued quote");
    assert_error(issued, &Error::IssuedQuote);
    assert_eq!(issued.outputs.outputs, vec![None; 4]);
    assert_parity(&standalone, &federated);
}

const BELOW_QUOTE: &str = "outputs below quote";
const ABOVE_QUOTE: &str = "outputs above quote";

fn paid_unissued_quote() -> MintQuoteSummary {
    MintQuoteSummary {
        state: MintQuoteState::Paid,
        amount: Some(15),
        amount_paid: 15,
        amount_issued: 0,
    }
}

#[tokio::test]
async fn mint_outputs_below_quote_amount() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let quote = driver.create_mint_quote(15).await;
            driver.pay_mint_quote(&quote).await;
            let under = outputs(driver.active_keyset(), &[8, 4]);
            vec![mint_step(driver, BELOW_QUOTE, &quote, &under).await]
        })
    })
    .await;

    let mismatch = step(&standalone, BELOW_QUOTE);
    assert_error(mismatch, &Error::TransactionUnbalanced(0, 0, 0));
    assert_eq!(
        mismatch.message(),
        "Inputs: `15`, Outputs: `12`, Expected Fee: `0`"
    );
    assert_eq!(mismatch.mint_quote, Some(paid_unissued_quote()));
    assert_parity(&standalone, &federated);
}

#[tokio::test]
async fn mint_outputs_above_quote_amount() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let quote = driver.create_mint_quote(15).await;
            driver.pay_mint_quote(&quote).await;
            let over = outputs(driver.active_keyset(), &[16]);
            vec![mint_step(driver, ABOVE_QUOTE, &quote, &over).await]
        })
    })
    .await;

    let mismatch = step(&standalone, ABOVE_QUOTE);
    assert_error(mismatch, &Error::TransactionUnbalanced(0, 0, 0));
    assert_eq!(
        mismatch.message(),
        "Inputs: `15`, Outputs: `16`, Expected Fee: `0`"
    );
    assert_eq!(mismatch.mint_quote, Some(paid_unissued_quote()));
    assert_parity(&standalone, &federated);
}

/// Melt `inputs` against a fresh 10 sat quote with blank change outputs.
async fn melt_step(driver: &mut Driver, name: &'static str, inputs: &Proofs) -> Observed {
    let quote = driver.create_melt_quote(10).await;
    let change = PreMintSecrets::blank(driver.active_keyset(), Amount::from(MELT_FEE_RESERVE))
        .expect("blank change outputs");
    let result = driver
        .melt(MeltRequest::new(
            quote.clone(),
            inputs.clone(),
            Some(change.blinded_messages()),
        ))
        .await;
    driver
        .observe(Step::new(name, &result, inputs, &change).melt_quote(&quote))
        .await
}

#[tokio::test]
async fn melt_with_fee_reserve_and_change() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[16]).await;
            vec![melt_step(driver, "melt", &funded).await]
        })
    })
    .await;

    let melt = step(&standalone, "melt");
    assert_ok(melt);
    assert_eq!(
        melt.melt_quote,
        Some(MeltQuoteSummary {
            state: MeltQuoteState::Paid,
            amount: 10,
            fee_reserve: MELT_FEE_RESERVE,
            payment_proof: Some(MELT_PAYMENT_PROOF.to_string()),
        })
    );
    assert_eq!(melt.input_states, vec![Some(State::Spent)]);
    // 16 in, 1 input fee, 10 paid, 1 routing fee: 4 change.
    let change = melt
        .outputs
        .outputs
        .iter()
        .flatten()
        .map(|(amount, _)| amount)
        .sum::<u64>();
    assert_eq!(change, 16 - 1 - 10 - MELT_ROUTING_FEE);
    assert!(melt.outputs.proofs_verify);
    assert_parity(&standalone, &federated);
}

#[tokio::test]
async fn melt_with_insufficient_inputs() {
    let (standalone, federated) = run_both(|driver| {
        Box::pin(async move {
            let funded = driver.fund(&[8]).await;
            vec![melt_step(driver, "melt", &funded).await]
        })
    })
    .await;

    let melt = step(&standalone, "melt");
    assert!(melt.result.is_err(), "melt with 8 sat for 15 sat must fail");
    assert_eq!(melt.input_states, vec![None]);
    assert_eq!(
        melt.melt_quote.as_ref().map(|quote| quote.state),
        Some(MeltQuoteState::Unpaid)
    );
    assert_parity(&standalone, &federated);
}
