use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock, Weak};

#[cfg(feature = "federation-bark")]
use cdk_common::bitcoin::hashes::{sha256, Hash};
use cdk_common::database;
use cdk_common::database::mint::{
    FederationCheckpointSelector, FederationJournalEntryKey, FederationPaymentObservationSelection,
    FederationSignatureShareKey,
};
use cdk_common::error::Error;
use cdk_common::federation::{
    FederatedKeyset, FederationAddressAllocationScope, FederationAddressAllocationState,
    FederationAddressKeychain, FederationApplicationState, FederationBlindSignatureShareSet,
    FederationError, FederationJournalEntry, FederationKeysetRotationOperation,
    FederationMaterializedStateKey, FederationMaterializedStateKind, FederationMeltOperation,
    FederationMeltQuoteOperation, FederationMeltQuotePaymentObservation, FederationMintOperation,
    FederationMintQuoteOperation, FederationMintQuotePaymentObservation,
    FederationOnchainMeltPaymentReceipt, FederationOperation, FederationOperationConflictKey,
    FederationOperationId, FederationOperationStatus, FederationPaymentObservation,
    FederationPaymentObservationContext, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, FederationSignatureShareRecord, FederationStoredState,
    FederationSwapOperation, FederationTransactionState, MemberId,
};
use cdk_common::mint::{
    MeltQuote, MeltSagaState, MintQuote, Operation, OperationKind, SagaStateEnum,
};
use cdk_common::payment::PaymentIdentifier;
use cdk_common::{
    Amount, BlindSignature, BlindedMessage, CurrencyUnit, MeltRequest, MintQuoteBolt11Response,
    MintQuoteBolt12Response, MintQuoteCustomResponse, MintQuoteOnchainResponse, MintQuoteRequest,
    MintQuoteResponse, MintQuoteState, MintResponse, NotificationPayload, Proofs, ProofsMethods,
    PublicKey, QuoteId, SpendingConditionVerification, State, SwapRequest, SwapResponse,
};
#[cfg(feature = "federation-bark")]
use cdk_federation_bark::{
    FederationBarkCpfpPackageSet, FederationBarkCpfpPublication, FederationBarkOperation,
    FederationBarkRecoveryState, FederationBarkSemanticPhase,
};
use futures::future::BoxFuture;

use super::configuration_and_admission::{
    FederationProofVerificationContext, FederationRuntimeConfig,
    FEDERATION_EXTERNAL_ACTION_ACTOR_LEASE_ITEMS,
};
use super::helpers::{
    ensure_melt_quote_matches_payment_observation, ensure_melt_quote_payment_proof_matches,
    ensure_mint_quote_matches, ensure_same_federation_journal_entry_for_retry,
    expected_federation_mint_amounts, federation_accepted_order,
    federation_melt_quote_immutable_terms_match, federation_operation_status_label,
    federation_operation_uuid, federation_payment_identifier_from_label,
    federation_payment_identifier_label, federation_share_signatures, melt_quote_payment_response,
    mint_input_from_federation_request, mint_quote_from_federation_operation,
    mint_quote_state_label, payment_observation_record_from_entry, quote_id_from_federation,
    validate_federation_mint_quote_states_for_signing, validate_federation_onchain_melt_receipt,
};
#[cfg(feature = "federation-bark")]
use super::projection;
use super::transaction_state::persist_pending_onchain_intent;
use crate::mint::melt::ensure_melt_quote_matches_request;
use crate::mint::melt::shared::MeltNotifications;
use crate::mint::swap::{plan_swap, SwapPlan};
use crate::mint::verification::{self, Verification};
use crate::mint::{Mint, MintInput};
use crate::types::PaymentProcessorKey;

const FEDERATION_SIGNATURE_SHARE_CACHE_CAPACITY: usize = 4096;

type BatchApplyOutcome = (FederationOperationId, bool, FederationApplicationState);
type BatchApplyFuture<'a> = BoxFuture<'a, Result<Vec<BatchApplyOutcome>, Error>>;

struct CachedSignatureShares {
    db: Weak<dyn database::MintDatabase<database::Error> + Send + Sync>,
    shares: Vec<FederationSignatureShareRecord>,
}

#[derive(Default)]
struct FederationSignatureShareCache {
    entries: HashMap<(usize, FederationOperationId), CachedSignatureShares>,
    order: VecDeque<(usize, FederationOperationId)>,
}

fn federation_signature_share_cache() -> &'static Mutex<FederationSignatureShareCache> {
    static CACHE: OnceLock<Mutex<FederationSignatureShareCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(FederationSignatureShareCache::default()))
}

fn db_signature_share_cache_key(
    db: &database::DynMintDatabase,
    operation_id: FederationOperationId,
) -> (usize, FederationOperationId) {
    (Arc::as_ptr(db) as *const () as usize, operation_id)
}

fn cache_committed_signature_shares(
    db: &database::DynMintDatabase,
    operation_id: FederationOperationId,
    shares: Vec<FederationSignatureShareRecord>,
) {
    if shares.is_empty() {
        return;
    }
    let key = db_signature_share_cache_key(db, operation_id);
    let mut cache = match federation_signature_share_cache().lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    let cached = CachedSignatureShares {
        db: Arc::downgrade(db),
        shares,
    };
    if let Entry::Occupied(mut entry) = cache.entries.entry(key) {
        entry.insert(cached);
        return;
    }
    while cache.entries.len() >= FEDERATION_SIGNATURE_SHARE_CACHE_CAPACITY {
        match cache.order.pop_front() {
            Some(evicted) => {
                cache.entries.remove(&evicted);
            }
            None => break,
        }
    }
    cache.entries.insert(key, cached);
    cache.order.push_back(key);
}

fn cached_committed_signature_shares(
    db: &database::DynMintDatabase,
    operation_id: FederationOperationId,
) -> Option<Vec<FederationSignatureShareRecord>> {
    let key = db_signature_share_cache_key(db, operation_id);
    let mut cache = match federation_signature_share_cache().lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    let matches = cache
        .entries
        .get(&key)
        .and_then(|entry| entry.db.upgrade().map(|live| Arc::ptr_eq(&live, db)))
        .unwrap_or(false);
    if matches {
        cache.entries.get(&key).map(|entry| entry.shares.clone())
    } else {
        cache.entries.remove(&key);
        None
    }
}

fn match_signature_shares_for_outputs(
    operation_id: FederationOperationId,
    member_id: MemberId,
    outputs: &[BlindedMessage],
    shares: Vec<FederationSignatureShareRecord>,
) -> Result<Vec<BlindSignature>, Error> {
    let mut by_output_index: HashMap<u32, FederationSignatureShareRecord> =
        HashMap::with_capacity(outputs.len());
    for share in shares {
        if share.member_id == member_id {
            by_output_index.entry(share.output_index).or_insert(share);
        }
    }
    let mut signatures = Vec::with_capacity(outputs.len());
    for (index, output) in outputs.iter().enumerate() {
        let output_index = u32::try_from(index)
            .map_err(|_| Error::Custom("federation output index overflow".to_string()))?;
        let share = by_output_index
            .remove(&output_index)
            .filter(|share| {
                share.member_id == member_id
                    && share.output_index == output_index
                    && share.blinded_secret == output.blinded_secret
                    && share.keyset_id == output.keyset_id
                    && share.amount == output.amount
            })
            .ok_or_else(|| {
                Error::Custom(format!(
                    "missing persisted federation signature share for operation {operation_id} output {output_index}"
                ))
            })?;
        signatures.push(crate::mint::federation_signature_share_to_blind_signature(
            share,
        ));
    }
    Ok(signatures)
}

fn is_retryable_federation_batch_apply_error(error: &Error) -> bool {
    matches!(
        error,
        Error::Database(database::Error::Locked | database::Error::ConcurrentUpdate)
            | Error::ConcurrentUpdate
    )
}

fn is_batchable_federation_operation(operation: &FederationOperation) -> bool {
    matches!(
        operation,
        FederationOperation::Swap(_)
            | FederationOperation::Mint(_)
            | FederationOperation::MintQuote(_)
            | FederationOperation::MintQuotePayment(_)
            | FederationOperation::MeltQuote(_)
            | FederationOperation::MeltQuotePayment(_)
    )
}

enum BatchPostCommitNotification {
    ProofsSpent(Vec<PublicKey>),
    MintQuoteCreated(MintQuote),
    MintQuotePayment(MintQuote, Amount<CurrencyUnit>),
    MintQuoteIssue(MintQuote, Amount<CurrencyUnit>),
    Melt(MeltNotifications),
}

impl BatchPostCommitNotification {
    fn publish(self, mint: &Mint) {
        match self {
            Self::ProofsSpent(ys) => {
                for y in ys {
                    mint.pubsub_manager.proof_state((y, State::Spent));
                }
            }
            Self::MintQuoteCreated(quote) => {
                if let Err(err) = mint.publish_federation_mint_quote_created(&quote) {
                    tracing::warn!(
                        quote_id = %quote.id,
                        "Could not publish federation mint quote creation notification: {err}"
                    );
                }
            }
            Self::MintQuotePayment(quote, amount_paid) => {
                mint.pubsub_manager.mint_quote_payment(&quote, amount_paid);
            }
            Self::MintQuoteIssue(quote, amount_issued) => {
                mint.pubsub_manager.mint_quote_issue(&quote, amount_issued);
            }
            Self::Melt(notifications) => {
                notifications.publish(&mint.pubsub_manager);
            }
        }
    }
}

enum BatchEntryError {
    NeedsSingleOpFallback,
    Apply(Error),
}

impl From<Error> for BatchEntryError {
    fn from(error: Error) -> Self {
        Self::Apply(error)
    }
}

impl From<database::Error> for BatchEntryError {
    fn from(error: database::Error) -> Self {
        Self::Apply(Error::from(error))
    }
}

impl From<FederationError> for BatchEntryError {
    fn from(error: FederationError) -> Self {
        Self::Apply(Error::from(error))
    }
}

struct PreparedBatchSwap {
    swap_plan: SwapPlan,
    input_ys: Vec<PublicKey>,
    blinded_secrets: Vec<PublicKey>,
    pre_signed_shares: Option<Result<FederationBlindSignatureShareSet, Error>>,
}

struct PreparedBatchMint {
    input: MintInput,
    outputs_amount: Amount<CurrencyUnit>,
    blinded_secrets: Vec<PublicKey>,
    quote_ids: Vec<QuoteId>,
    pre_signed_shares: Option<Result<FederationBlindSignatureShareSet, Error>>,
}

enum PreparedBatchEntry {
    Swap(Result<PreparedBatchSwap, Error>),
    Mint(Result<PreparedBatchMint, Error>),
    MintQuote(Result<MintQuote, Error>),
    MintQuotePayment(Result<(QuoteId, FederationPaymentObservationRecord), Error>),
    MeltQuote(Result<MeltQuote, Error>),
    MeltQuotePayment(Result<FederationPaymentObservationRecord, Error>),
}

struct FederationActiveMeltPayment {
    active: std::sync::Arc<std::sync::Mutex<HashSet<QuoteId>>>,
    quote_id: QuoteId,
}

impl Drop for FederationActiveMeltPayment {
    fn drop(&mut self) {
        let mut active = match self.active.lock() {
            Ok(active) => active,
            Err(poisoned) => poisoned.into_inner(),
        };
        active.remove(&self.quote_id);
    }
}

async fn finish_federation_transaction<T>(
    transaction: database::mint::DynMintTransaction,
    entry: &FederationJournalEntry,
    phase: &'static str,
    outcome: Result<T, Error>,
) -> Result<T, Error> {
    match outcome {
        Ok(value) => {
            transaction.commit().await.map_err(|err| {
                tracing::error!(
                    operation_id = %entry.operation_id,
                    operation_kind = %entry.envelope.operation.kind(),
                    transaction_phase = phase,
                    "Could not commit federation operation transaction: {err}"
                );
                Error::from(err)
            })?;
            Ok(value)
        }
        Err(error) => {
            transaction.rollback().await.map_err(|rollback_error| {
                tracing::error!(
                    operation_id = %entry.operation_id,
                    operation_kind = %entry.envelope.operation.kind(),
                    transaction_phase = phase,
                    apply_error = %error,
                    "Could not roll back federation operation transaction: {rollback_error}"
                );
                Error::from(rollback_error)
            })?;
            Err(error)
        }
    }
}

/// Whether no operation can change a mint quote's payment state any more.
///
/// A Bolt11 invoice is paid once; an issued Bolt11 quote is final. Bolt12
/// offers and on-chain addresses can be paid again at any time, so their
/// quotes never become final.
fn mint_quote_payments_are_final(quote: &MintQuote) -> bool {
    quote.payment_method == cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt11)
        && quote.state() == MintQuoteState::Issued
}

/// Whether the quote a payment observation names is final, so the
/// observation is not recorded: nothing it says can change the quote.
///
/// Quote state is consensus state (every member stores it and the
/// materialized-state digest covers it), so every member decides alike. A
/// final quote's earlier observations are retired, and a member may already
/// have deleted them; recording a replay of one would differ between
/// members that deleted it and members that did not.
#[cfg(feature = "federation-bark")]
async fn payment_observation_quote_is_final(
    tx: &mut database::mint::DynMintTransaction,
    observation: &FederationPaymentObservation,
) -> Result<bool, Error> {
    let quote_id = quote_id_from_federation(observation.quote())?;
    Ok(match observation.context() {
        FederationPaymentObservationContext::MintQuote => tx
            .get_mint_quote(&quote_id)
            .await?
            .is_some_and(|quote| mint_quote_payments_are_final(&quote)),
        FederationPaymentObservationContext::MeltQuote => tx
            .get_melt_quote(&quote_id)
            .await?
            .is_some_and(|quote| quote.state == cdk_common::MeltQuoteState::Paid),
    })
}

/// Record a payment observation unless its quote is final.
#[cfg(feature = "federation-bark")]
async fn record_live_payment_observation(
    tx: &mut database::mint::DynMintTransaction,
    record: &FederationPaymentObservationRecord,
) -> Result<(), Error> {
    if !payment_observation_quote_is_final(tx, &record.observation).await? {
        tx.add_federation_payment_observation(record, None).await?;
    }
    Ok(())
}

/// Retire the payment observations of `quote` if it became final, at the
/// journal index of the operation applying now.
pub(super) async fn retire_final_mint_quote_observations(
    tx: &mut database::mint::DynMintTransaction,
    quote: &MintQuote,
    operation_index: u64,
) -> Result<(), Error> {
    if mint_quote_payments_are_final(quote) {
        tx.retire_federation_payment_observations(
            FederationPaymentObservationContext::MintQuote,
            &quote.id.to_string(),
            operation_index,
        )
        .await?;
        tx.settle_federation_application_quote(&quote.id.to_string(), operation_index)
            .await?;
    }
    Ok(())
}

/// Record on the application projection that a Bolt11 mint quote was paid
/// by the operation at `operation_index`.
///
/// The Bark receive bound to it needs no more evidence once the quote is
/// paid: an invoice is paid once.
pub(super) async fn settle_paid_mint_quote_application(
    tx: &mut database::mint::DynMintTransaction,
    quote: &MintQuote,
    operation_index: u64,
) -> Result<(), Error> {
    if quote.payment_method
        == cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt11)
        && quote.state() != MintQuoteState::Unpaid
    {
        tx.settle_federation_application_quote(&quote.id.to_string(), operation_index)
            .await?;
    }
    Ok(())
}

/// Settle a melt quote after the payment observation applying at
/// `operation_index`.
///
/// A paid melt quote is final: its payment observations retire. For an
/// on-chain payment its intent is settled; once every quote the confirmed
/// transaction pays is settled, the transaction lineage is terminal and its
/// reserved inputs are spent, leaving tombstones. A pending melt rolled back
/// by this observation needs no more evidence either: the application
/// action bound to it is told so, though the quote itself may be melted
/// again.
async fn settle_paid_melt_quote(
    tx: &mut database::mint::DynMintTransaction,
    quote_id: &QuoteId,
    previous_state: cdk_common::MeltQuoteState,
    onchain: bool,
    operation_index: u64,
) -> Result<(), Error> {
    let Some(state) = tx.get_melt_quote(quote_id).await?.map(|quote| quote.state) else {
        return Ok(());
    };
    if previous_state == cdk_common::MeltQuoteState::Pending
        && matches!(
            state,
            cdk_common::MeltQuoteState::Unpaid | cdk_common::MeltQuoteState::Failed
        )
    {
        tx.settle_federation_application_quote(&quote_id.to_string(), operation_index)
            .await?;
        return Ok(());
    }
    if state != cdk_common::MeltQuoteState::Paid {
        return Ok(());
    }
    tx.retire_federation_payment_observations(
        FederationPaymentObservationContext::MeltQuote,
        &quote_id.to_string(),
        operation_index,
    )
    .await?;
    tx.settle_federation_application_quote(&quote_id.to_string(), operation_index)
        .await?;
    if onchain {
        let mut transactions =
            FederationStoredState::<FederationTransactionState>::load(tx).await?;
        if let Some((batch_id, inputs)) =
            transactions.settle_completed_intent(&quote_id.to_string(), operation_index)?
        {
            let mut chain = FederationStoredState::<
                cdk_common::federation::FederationChainObservationState,
            >::load(tx)
            .await?;
            // An input that left its reservation (a deep reorg) keeps the
            // lineage readable: it is not made terminal and nothing is spent.
            if inputs.iter().all(|outpoint| {
                chain.utxo(*outpoint).is_some_and(|utxo| {
                    utxo.state == cdk_common::federation::FederationWalletUtxoState::Reserved
                })
            }) {
                chain.spend_reserved_utxos(&inputs, operation_index)?;
                transactions.retire_settled_lineage(batch_id, operation_index)?;
                chain.save(tx).await?;
            }
        }
        transactions.save(tx).await?;
    }
    Ok(())
}

pub(super) async fn finish_federation_apply_transaction<T>(
    mut transaction: database::mint::DynMintTransaction,
    entry: &FederationJournalEntry,
    outcome: Result<T, Error>,
) -> Result<T, Error> {
    let outcome = match outcome {
        Ok(value) => match transaction
            .record_federation_operation_application_state_deferred(
                entry,
                &FederationApplicationState::Applied {},
            )
            .await
        {
            Ok(()) => transaction
                .advance_federation_application_frontier()
                .await
                .map(|()| value)
                .map_err(Error::from),
            Err(err) => Err(Error::from(err)),
        },
        Err(error) => Err(error),
    };

    finish_federation_transaction(transaction, entry, "apply", outcome).await
}

#[cfg(feature = "federation-bark")]
#[derive(Debug, Clone, Copy)]
pub(super) struct FederationBarkResultObservationKey {
    pub(super) action_id: cdk_federation_bark::FederationBarkActionId,
    pub(super) status: cdk_federation_bark::FederationBarkObservationStatus,
    pub(super) payment_hash: cdk_federation_bark::FederationBarkDigest,
    pub(super) source_scope: cdk_federation_bark::FederationBarkDigest,
    pub(super) amount: Amount,
    pub(super) fee: Amount,
    pub(super) result_hash: cdk_federation_bark::FederationBarkDigest,
}

#[cfg(feature = "federation-bark")]
const FEDERATION_BARK_CERTIFICATE_OBSERVATION_ID_DOMAIN: &[u8] =
    b"cdk/federation/bark-certificate-observation-id/v1";

#[cfg(feature = "federation-bark")]
enum FederationBarkRecoveryMutation {
    Reserve {
        package_set: FederationBarkCpfpPackageSet,
    },
    Publish {
        package_set: FederationBarkCpfpPackageSet,
        registered_operation_id: FederationOperationId,
        publication: FederationBarkCpfpPublication,
    },
}

#[cfg(feature = "federation-bark")]
fn federation_bark_certificate_observation_id(
    operation_id: FederationOperationId,
    observer: MemberId,
) -> FederationOperationId {
    let mut bytes =
        Vec::with_capacity(FEDERATION_BARK_CERTIFICATE_OBSERVATION_ID_DOMAIN.len() + 32 + 2);
    bytes.extend_from_slice(FEDERATION_BARK_CERTIFICATE_OBSERVATION_ID_DOMAIN);
    bytes.extend_from_slice(&operation_id.to_bytes());
    bytes.extend_from_slice(&observer.get().to_be_bytes());
    FederationOperationId::from_bytes(sha256::Hash::hash(&bytes).to_byte_array())
}

#[cfg(all(test, feature = "federation-bark"))]
pub(super) fn federation_bark_matching_result_observers(
    projection: &cdk_federation_bark::FederationBarkProjectionSnapshot,
    key: FederationBarkResultObservationKey,
) -> std::collections::BTreeSet<MemberId> {
    projection
        .observations
        .iter()
        .filter_map(|record| match &record.operation {
            cdk_federation_bark::FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == key.action_id
                    && observation.status == key.status
                    && observation.payment_hash == Some(key.payment_hash)
                    && observation.source_scope == key.source_scope
                    && observation.amount == key.amount
                    && observation.fee == key.fee
                    && observation.result_hash == key.result_hash =>
            {
                Some(observation.observer)
            }
            _ => None,
        })
        .collect()
}

#[cfg(feature = "federation-bark")]
#[derive(Debug, Clone)]
pub(super) enum FederationBarkPaymentObservationTemplate {
    Mint(FederationMintQuotePaymentObservation),
    Melt(FederationMeltQuotePaymentObservation),
}

#[cfg(feature = "federation-bark")]
impl FederationBarkPaymentObservationTemplate {
    fn with_observer(&self, observer: MemberId) -> FederationPaymentObservation {
        match self {
            Self::Mint(template) => {
                let mut observation = template.clone();
                observation.observer = observer;
                FederationPaymentObservation::MintQuote(observation)
            }
            Self::Melt(template) => {
                let mut observation = template.clone();
                observation.observer = observer;
                FederationPaymentObservation::MeltQuote(observation)
            }
        }
    }
}

#[cfg(feature = "federation-bark")]
pub(super) fn federation_bark_result_payment_records(
    action_observations: &[cdk_federation_bark::FederationBarkObservationRecord],
    expected: &cdk_federation_bark::FederationBarkMemberObservation,
    required_bark_observers: u16,
    template: &FederationBarkPaymentObservationTemplate,
) -> Vec<FederationPaymentObservationRecord> {
    let Some(payment_hash) = expected.payment_hash else {
        return Vec::new();
    };
    let key = FederationBarkResultObservationKey {
        action_id: expected.action_id,
        status: expected.status,
        payment_hash,
        source_scope: expected.source_scope,
        amount: expected.amount,
        fee: expected.fee,
        result_hash: expected.result_hash,
    };
    let mut matching_by_observer = std::collections::BTreeMap::new();
    for record in action_observations {
        let FederationBarkOperation::MemberObservation(candidate) = &record.operation else {
            continue;
        };
        if candidate.quote.as_deref() != expected.quote.as_deref()
            || candidate.action_id != key.action_id
            || candidate.status != key.status
            || candidate.payment_hash != Some(key.payment_hash)
            || candidate.source_scope != key.source_scope
            || candidate.amount != key.amount
            || candidate.fee != key.fee
            || candidate.result_hash != key.result_hash
        {
            continue;
        }
        matching_by_observer
            .entry(candidate.observer)
            .and_modify(
                |existing: &mut &cdk_federation_bark::FederationBarkObservationRecord| {
                    if record.operation_index < existing.operation_index {
                        *existing = record;
                    }
                },
            )
            .or_insert(record);
    }
    if matching_by_observer.len() < usize::from(required_bark_observers) {
        return Vec::new();
    }

    let mut records = matching_by_observer
        .into_iter()
        .map(|(observer, record)| FederationPaymentObservationRecord {
            operation_id: record.operation_id,
            operation_index: record.operation_index,
            observation: template.with_observer(observer),
        })
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record.operation_index);
    records
}

impl Mint {
    fn claim_active_federation_melt_payment(
        &self,
        quote_id: &QuoteId,
    ) -> Option<FederationActiveMeltPayment> {
        let mut active = match self.federation_active_melt_payments.lock() {
            Ok(active) => active,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !active.insert(quote_id.clone()) {
            return None;
        }

        Some(FederationActiveMeltPayment {
            active: std::sync::Arc::clone(&self.federation_active_melt_payments),
            quote_id: quote_id.clone(),
        })
    }

    /// Compute the current external-action lease actor.
    ///
    /// The accepted envelope author holds the first lease epoch. Authority
    /// then rotates deterministically through the roster every
    /// [`FEDERATION_EXTERNAL_ACTION_ACTOR_LEASE_ITEMS`] accepted journal items,
    /// so an offline or Byzantine author cannot veto the payment forever.
    /// The lease is derived only from consensus-ordered state (the melt
    /// operation's journal index and the roster), so every member with the
    /// same journal frontier computes the same single actor, and takeover is
    /// safe because the operation's external identity and recovery state are
    /// durable before any side effect.
    pub(in crate::mint) async fn federation_external_action_lease_actor(
        &self,
        config: &FederationRuntimeConfig,
        author: MemberId,
        operation_index: u64,
    ) -> Result<MemberId, Error> {
        let decision_operation_index = self
            .localstore
            .get_federation_journal_tip()
            .await?
            .map_or(0, |tip| tip.index.saturating_add(1));
        Self::federation_external_action_lease_actor_at_index(
            config,
            author,
            operation_index,
            decision_operation_index,
        )
    }

    /// Recompute an external-action lease at the exact consensus index where
    /// the decision was ordered. Catch-up must use the accepted plan's index
    /// rather than a later journal tip, otherwise future operations can change
    /// the actor expected for immutable history.
    pub(in crate::mint) fn federation_external_action_lease_actor_at_index(
        config: &FederationRuntimeConfig,
        author: MemberId,
        operation_index: u64,
        decision_operation_index: u64,
    ) -> Result<MemberId, Error> {
        let members: Vec<MemberId> = config.public_config().members.keys().copied().collect();
        let author_position = members
            .iter()
            .position(|member| *member == author)
            .ok_or(Error::Internal)?;
        let lease_epoch = decision_operation_index
            .saturating_sub(operation_index.saturating_add(1))
            / FEDERATION_EXTERNAL_ACTION_ACTOR_LEASE_ITEMS;
        let epoch_steps = usize::try_from(lease_epoch % members.len() as u64).unwrap_or_default();
        let actor_position = (author_position + epoch_steps) % members.len();
        Ok(members[actor_position])
    }

    /// Return the deterministic current payment/planning actor for one applied melt.
    pub async fn federation_melt_payment_actor(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<MemberId, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let entry = self
            .localstore
            .get_federation_journal_entry(FederationJournalEntryKey::Operation(operation_id), None)
            .await?
            .ok_or(Error::Internal)?;
        if entry.status() != FederationOperationStatus::Applied
            || !matches!(entry.envelope.operation, FederationOperation::Melt(_))
        {
            return Err(Error::Internal);
        }
        let author = entry
            .envelope
            .verify_authorization(config.public_config())?;
        self.federation_external_action_lease_actor(&config, author, entry.index)
            .await
    }

    /// Return the replaceable receive-worker actor for one applied Bark receive intent.
    #[cfg(feature = "federation-bark")]
    pub async fn federation_bark_receive_actor(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<MemberId, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let entry = self
            .localstore
            .get_federation_journal_entry(FederationJournalEntryKey::Operation(operation_id), None)
            .await?
            .ok_or(Error::Internal)?;
        if entry.status() != FederationOperationStatus::Applied
            || !matches!(
                projection::bark_operation(&entry.envelope.operation),
                Ok(cdk_federation_bark::FederationBarkOperation::ReceiveIntent(
                    _
                ))
            )
        {
            return Err(Error::Internal);
        }
        let author = entry
            .envelope
            .verify_authorization(config.public_config())?;
        self.federation_external_action_lease_actor(&config, author, entry.index)
            .await
    }

    #[cfg(feature = "federation-bark")]
    async fn federation_bark_result_payment_records(
        &self,
        entry: &FederationJournalEntry,
        observation: &cdk_federation_bark::FederationBarkMemberObservation,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let store = self.federation_bark_store()?;
        let mut observations = store
            .bark_action_observations(observation.action_id)
            .await?;
        if !observations
            .iter()
            .any(|record| record.operation_id == entry.operation_id)
        {
            observations.push(cdk_federation_bark::FederationBarkObservationRecord {
                operation_id: entry.operation_id,
                operation_index: entry.index,
                operation: FederationBarkOperation::MemberObservation(observation.clone()),
            });
        }

        let Some(action) = store.bark_action(observation.action_id).await? else {
            return Err(Error::Internal);
        };
        let Some(template) = self
            .federation_bark_payment_observation_template(&action, observation)
            .await?
        else {
            return Ok(Vec::new());
        };
        Ok(federation_bark_result_payment_records(
            &observations,
            observation,
            config.public_config().threshold_params.signature_threshold,
            &template,
        ))
    }

    #[cfg(feature = "federation-bark")]
    async fn federation_bark_certificate_payment_records(
        &self,
        entry: &FederationJournalEntry,
        certificate: &cdk_federation_bark::FederationBarkEvidenceCertificate,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Error> {
        let observer = certificate
            .attestations
            .first()
            .map(|attestation| attestation.signer)
            .ok_or(Error::Internal)?;
        let observation = certificate.observation_for(observer);
        let action = self
            .federation_bark_action(certificate.binding.action_id)
            .await?
            .ok_or(Error::Internal)?;
        let template = self
            .federation_bark_payment_observation_template(&action, &observation)
            .await?
            .ok_or(Error::Internal)?;
        Ok(certificate
            .signers()
            .map(|observer| FederationPaymentObservationRecord {
                operation_id: federation_bark_certificate_observation_id(
                    entry.operation_id,
                    observer,
                ),
                operation_index: entry.index,
                observation: template.with_observer(observer),
            })
            .collect())
    }

    #[cfg(feature = "federation-bark")]
    async fn federation_bark_transition_certificate_payment_records(
        &self,
        entry: &FederationJournalEntry,
        transition: &cdk_federation_bark::FederationBarkActionTransition,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Error> {
        let certificate = transition
            .evidence_certificate
            .as_deref()
            .ok_or(Error::Internal)?;
        let observer = certificate
            .attestations
            .first()
            .map(|attestation| attestation.signer)
            .ok_or(Error::Internal)?;
        let observation = certificate.observation_for(observer);
        let current = self
            .federation_bark_action(transition.action_id)
            .await?
            .ok_or(Error::Internal)?;
        let operation = FederationBarkOperation::ActionTransition(transition.clone());
        let action = current.advanced(entry, &operation)?;
        let template = self
            .federation_bark_payment_observation_template(&action, &observation)
            .await?
            .ok_or(Error::Internal)?;
        Ok(certificate
            .signers()
            .map(|observer| FederationPaymentObservationRecord {
                operation_id: federation_bark_certificate_observation_id(
                    entry.operation_id,
                    observer,
                ),
                operation_index: entry.index,
                observation: template.with_observer(observer),
            })
            .collect())
    }

    #[cfg(feature = "federation-bark")]
    async fn federation_bark_payment_observation_template(
        &self,
        action: &cdk_federation_bark::FederationBarkActionRecord,
        observation: &cdk_federation_bark::FederationBarkMemberObservation,
    ) -> Result<Option<FederationBarkPaymentObservationTemplate>, Error> {
        match (action.phase, observation.status) {
            (
                cdk_federation_bark::FederationBarkSemanticPhase::ReceiveClaimed,
                cdk_federation_bark::FederationBarkObservationStatus::Succeeded,
            ) => {
                let FederationBarkOperation::ActionTransition(transition) = &action.operation
                else {
                    return Err(Error::Internal);
                };
                let intent = self
                    .federation_bark_receive_intent(action.origin_operation_id)
                    .await?
                    .ok_or(Error::Internal)?;
                let quote_id =
                    quote_id_from_federation(observation.quote.as_deref().ok_or(Error::Internal)?)?;
                let quote = self
                    .localstore
                    .get_mint_quote(&quote_id)
                    .await?
                    .ok_or(Error::UnknownQuote)?;
                let spendable = transition
                    .vtxo_transitions
                    .iter()
                    .filter(|mutation| {
                        mutation.previous_state.is_none()
                            && mutation.new_state
                                == cdk_federation_bark::FederationBarkVtxoState::Spendable
                    })
                    .try_fold(Amount::ZERO, |sum, mutation| {
                        sum.checked_add(mutation.amount).ok_or(Error::Internal)
                    })?;
                if !quote.payment_method.is_bolt11()
                    || quote.unit != CurrencyUnit::Sat
                    || spendable != observation.amount
                    || spendable < intent.cashu_amount
                {
                    return Err(Error::InvalidPaymentRequest);
                }
                Ok(Some(FederationBarkPaymentObservationTemplate::Mint(
                    FederationMintQuotePaymentObservation {
                        quote: quote.id.to_string(),
                        observer: observation.observer,
                        method: quote.payment_method.clone(),
                        state: MintQuoteState::Paid,
                        amount_paid: intent.cashu_amount,
                        amount_issued: Amount::ZERO,
                        payment_id: Some(PaymentIdentifier::QuoteId(quote.id.clone())),
                        payment_proof: None,
                    },
                )))
            }
            (
                cdk_federation_bark::FederationBarkSemanticPhase::PaymentSucceeded,
                cdk_federation_bark::FederationBarkObservationStatus::Succeeded,
            ) => {
                let FederationBarkOperation::ActionTransition(transition) = &action.operation
                else {
                    return Err(Error::Internal);
                };
                let plan_entry = self
                    .localstore
                    .get_federation_journal_entry(
                        FederationJournalEntryKey::Operation(transition.plan_operation_id),
                        None,
                    )
                    .await?
                    .ok_or(Error::Internal)?;
                let FederationBarkOperation::ActionPlan(plan) =
                    projection::bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(Error::Internal);
                };
                cdk_federation_bark::validate_federation_bark_send_succeeded_result(
                    &plan, transition,
                )?;
                let proof = transition
                    .artifacts
                    .iter()
                    .find(|artifact| {
                        artifact.kind == cdk_federation_bark::FederationBarkArtifactKind::Proof
                    })
                    .ok_or(Error::Internal)?;
                self.federation_bark_melt_payment_template(
                    observation,
                    &plan,
                    cdk_common::MeltQuoteState::Paid,
                    Some(crate::util::hex::encode(&proof.bytes)),
                )
                .await
                .map(Some)
            }
            (
                cdk_federation_bark::FederationBarkSemanticPhase::RevocationCompleted,
                cdk_federation_bark::FederationBarkObservationStatus::Failed,
            ) => {
                let FederationBarkOperation::ActionTransition(completed) = &action.operation else {
                    return Err(Error::Internal);
                };
                let revocation_entry = self
                    .localstore
                    .get_federation_journal_entry(
                        FederationJournalEntryKey::Operation(completed.plan_operation_id),
                        None,
                    )
                    .await?
                    .ok_or(Error::Internal)?;
                let FederationBarkOperation::ActionPlan(revocation) =
                    projection::bark_operation(&revocation_entry.envelope.operation)?
                else {
                    return Err(Error::Internal);
                };
                if revocation.kind
                    != cdk_federation_bark::FederationBarkActionKind::FailedSendRevocation
                {
                    return Err(Error::Internal);
                }
                let failed_entry = self
                    .localstore
                    .get_federation_journal_entry(
                        FederationJournalEntryKey::Operation(revocation.predecessor_operation_id),
                        None,
                    )
                    .await?
                    .ok_or(Error::Internal)?;
                let FederationBarkOperation::ActionTransition(failed) =
                    projection::bark_operation(&failed_entry.envelope.operation)?
                else {
                    return Err(Error::Internal);
                };
                let plan_entry = self
                    .localstore
                    .get_federation_journal_entry(
                        FederationJournalEntryKey::Operation(failed.plan_operation_id),
                        None,
                    )
                    .await?
                    .ok_or(Error::Internal)?;
                let FederationBarkOperation::ActionPlan(plan) =
                    projection::bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(Error::Internal);
                };
                self.federation_bark_melt_payment_template(
                    observation,
                    &plan,
                    cdk_common::MeltQuoteState::Failed,
                    Some(completed.result_hash.to_hex()),
                )
                .await
                .map(Some)
            }
            _ => Ok(None),
        }
    }

    #[cfg(feature = "federation-bark")]
    async fn federation_bark_melt_payment_template(
        &self,
        observation: &cdk_federation_bark::FederationBarkMemberObservation,
        plan: &cdk_federation_bark::FederationBarkActionPlan,
        state: cdk_common::MeltQuoteState,
        payment_proof: Option<String>,
    ) -> Result<FederationBarkPaymentObservationTemplate, Error> {
        if plan.kind != cdk_federation_bark::FederationBarkActionKind::LightningSend {
            return Err(Error::Internal);
        }
        let payment_hash = plan.payment_hash.ok_or(Error::Internal)?;
        let quote_id =
            quote_id_from_federation(observation.quote.as_deref().ok_or(Error::Internal)?)?;
        let quote = self
            .localstore
            .get_melt_quote(&quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;
        if !quote.payment_method.is_bolt11()
            || quote.unit != CurrencyUnit::Sat
            || observation.payment_hash != Some(payment_hash)
            || observation.amount != plan.external_amount
            || observation.fee != plan.fee_amount
        {
            return Err(Error::InvalidPaymentRequest);
        }
        Ok(FederationBarkPaymentObservationTemplate::Melt(
            FederationMeltQuotePaymentObservation {
                quote: quote.id.to_string(),
                observer: observation.observer,
                method: quote.payment_method.clone(),
                state,
                amount: quote.amount().into(),
                fee_reserve: quote.fee_reserve().into(),
                total_spent: match state {
                    cdk_common::MeltQuoteState::Paid => Some(
                        plan.external_amount
                            .checked_add(plan.fee_amount)
                            .ok_or(Error::AmountOverflow)?,
                    ),
                    _ => None,
                },
                payment_id: Some(PaymentIdentifier::PaymentHash(payment_hash.to_bytes())),
                payment_proof,
            },
        ))
    }

    /// Probe an incoming payment backend without mutating federated quote state.
    pub async fn probe_federation_mint_quote_payment(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<FederationMintQuotePaymentObservation>, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;
        if quote.payment_method.is_onchain() && config.public_config().frost_wallet.is_some() {
            if quote.unit != CurrencyUnit::Sat {
                return Err(Error::UnsupportedUnit);
            }
            let Some(payment) = self
                .federation_chain_observation_state()
                .await?
                .quote_payment_at_or_before(&quote.id.to_string(), u64::MAX)?
            else {
                return Ok(None);
            };
            // Internal settlements credit the quote apart from its chain
            // outputs, which are all this observation reports.
            let mut tx = self.localstore.begin_transaction().await?;
            let internally_settled = self
                .federation_internal_settlement_amount_before(&mut tx, &quote, u64::MAX)
                .await;
            tx.rollback().await?;
            let chain_credited = quote
                .amount_paid()
                .checked_sub(&internally_settled?)
                .map_err(|_| Error::AmountOverflow)?;
            if payment.amount_sat <= chain_credited.value() {
                return Ok(None);
            }
            return Ok(Some(FederationMintQuotePaymentObservation {
                quote: quote.id.to_string(),
                observer: config.member_id(),
                method: quote.payment_method.clone(),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(payment.amount_sat),
                amount_issued: quote.amount_issued().into(),
                payment_id: Some(PaymentIdentifier::PaymentId(payment.payment_id)),
                payment_proof: None,
            }));
        }
        if quote.state() == MintQuoteState::Issued {
            return Ok(None);
        }
        let processor = self
            .payment_processors
            .get(&PaymentProcessorKey::new(
                quote.unit.clone(),
                quote.payment_method.clone(),
            ))
            .ok_or(Error::UnsupportedUnit)?;
        let payments = processor
            .check_incoming_payment_status(&quote.request_lookup_id)
            .await?;
        let mut observed_quote = quote.clone();
        let mut payment_id = None;
        let mut payment_label: Option<String> = None;

        for payment in payments {
            let candidate_payment_id =
                federation_payment_identifier_from_label(&payment.payment_id);
            let durable_payment_label = federation_payment_identifier_label(&candidate_payment_id);
            if payment.payment_amount.value() == 0
                || observed_quote.payment_ids().contains(&&payment.payment_id)
                || observed_quote
                    .payment_ids()
                    .contains(&&durable_payment_label)
            {
                continue;
            }
            let amount = payment.payment_amount.convert_to(&observed_quote.unit)?;
            // Settlement identity is canonical: every member selects the
            // smallest durable payment label, independent of backend return
            // order, so exact-match evidence groups agree across members.
            if payment_label
                .as_deref()
                .is_none_or(|current| durable_payment_label.as_str() < current)
            {
                payment_label = Some(durable_payment_label);
                payment_id = Some(candidate_payment_id);
            }
            observed_quote.add_payment(amount, payment.payment_id, None)?;
        }

        if observed_quote.state() != MintQuoteState::Paid
            || observed_quote.amount_paid().value() == 0
        {
            return Ok(None);
        }
        if payment_id.is_none() {
            payment_id = observed_quote
                .payments
                .iter()
                .map(|payment| federation_payment_identifier_from_label(&payment.payment_id))
                .min_by_key(federation_payment_identifier_label);
        }

        Ok(Some(FederationMintQuotePaymentObservation {
            quote: observed_quote.id.to_string(),
            observer: config.member_id(),
            method: observed_quote.payment_method.clone(),
            state: MintQuoteState::Paid,
            amount_paid: observed_quote.amount_paid().into(),
            amount_issued: observed_quote.amount_issued().into(),
            payment_id,
            payment_proof: None,
        }))
    }

    /// Probe or initiate an outgoing federated payment without finalizing its saga.
    pub async fn probe_federation_melt_quote_payment(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<FederationMeltQuotePaymentObservation>, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;
        if quote.state != cdk_common::MeltQuoteState::Pending {
            return Ok(None);
        }
        let Some(saga) = self.localstore.get_melt_saga_by_quote_id(quote_id).await? else {
            return Ok(None);
        };
        let operation_id = saga
            .federation_operation_id
            .as_deref()
            .ok_or(Error::Internal)?
            .parse::<FederationOperationId>()?;
        if quote.payment_method.is_onchain() && config.public_config().frost_wallet.is_some() {
            if quote.unit != CurrencyUnit::Sat {
                return Err(Error::UnsupportedUnit);
            }
            let Some(receipt) = self
                .federation_transaction_state()
                .await?
                .completed_payment_for_quote(&quote.id.to_string())?
            else {
                return Ok(None);
            };
            validate_federation_onchain_melt_receipt(&quote, operation_id, &receipt)?;
            let total_spent = Amount::from(
                receipt
                    .amount_sat
                    .checked_add(receipt.fee_sat)
                    .ok_or(Error::AmountOverflow)?,
            );
            return Ok(Some(FederationMeltQuotePaymentObservation {
                quote: quote.id.to_string(),
                observer: config.member_id(),
                method: quote.payment_method.clone(),
                state: cdk_common::MeltQuoteState::Paid,
                amount: quote.amount().into(),
                fee_reserve: quote.fee_reserve().into(),
                total_spent: Some(total_spent),
                payment_id: Some(PaymentIdentifier::QuoteId(quote.id.clone())),
                payment_proof: Some(receipt.outpoint.to_string()),
            }));
        }
        let entry = self
            .localstore
            .get_federation_journal_entry(FederationJournalEntryKey::Operation(operation_id), None)
            .await?
            .ok_or(Error::Internal)?;
        let author = entry
            .envelope
            .verify_authorization(config.public_config())?;
        let lease_actor = self
            .federation_external_action_lease_actor(&config, author, entry.index)
            .await?;
        let is_payment_actor = lease_actor == config.member_id();
        let _active_payment = if is_payment_actor {
            let Some(active_payment) = self.claim_active_federation_melt_payment(quote_id) else {
                return Ok(None);
            };
            Some(active_payment)
        } else {
            None
        };

        let payment = match saga.state {
            SagaStateEnum::Melt(MeltSagaState::SetupComplete) if is_payment_actor => {
                let Some(payment) = self
                    .attempt_federation_external_melt_payment(quote.clone(), saga)
                    .await?
                else {
                    return Ok(None);
                };
                payment
            }
            SagaStateEnum::Melt(
                MeltSagaState::SetupComplete
                | MeltSagaState::PaymentAttempted
                | MeltSagaState::PaymentPending
                | MeltSagaState::PaymentFailed
                | MeltSagaState::Finalizing,
            ) => self.check_melt_payment_status(&quote).await?,
            SagaStateEnum::Swap(_) => return Err(Error::Internal),
        };

        match payment.status {
            cdk_common::MeltQuoteState::Paid => {}
            cdk_common::MeltQuoteState::Failed | cdk_common::MeltQuoteState::Unpaid
                if is_payment_actor => {}
            cdk_common::MeltQuoteState::Failed
            | cdk_common::MeltQuoteState::Unpaid
            | cdk_common::MeltQuoteState::Pending
            | cdk_common::MeltQuoteState::Unknown => return Ok(None),
        }

        Ok(Some(FederationMeltQuotePaymentObservation {
            quote: quote.id.to_string(),
            observer: config.member_id(),
            method: quote.payment_method.clone(),
            state: payment.status,
            amount: quote.amount().into(),
            fee_reserve: quote.fee_reserve().into(),
            total_spent: match payment.status {
                cdk_common::MeltQuoteState::Paid => Some(
                    crate::mint::melt::shared::total_spent_for_quote_unit(
                        &payment.total_spent,
                        &quote.unit,
                    )?
                    .into(),
                ),
                _ => None,
            },
            payment_id: Some(payment.payment_lookup_id),
            payment_proof: payment.payment_proof,
        }))
    }

    async fn persisted_signature_shares_for_outputs(
        &self,
        operation_id: FederationOperationId,
        outputs: &[BlindedMessage],
    ) -> Result<Vec<BlindSignature>, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let member_id = config.member_id();
        if let Some(cached) = cached_committed_signature_shares(&self.localstore, operation_id) {
            if let Ok(signatures) =
                match_signature_shares_for_outputs(operation_id, member_id, outputs, cached)
            {
                return Ok(signatures);
            }
        }
        let shares = self
            .localstore
            .get_federation_signature_shares(FederationSignatureShareKey::Operation(operation_id))
            .await?;
        match_signature_shares_for_outputs(operation_id, member_id, outputs, shares)
    }

    /// Return this member's persisted wallet-facing mint share response.
    pub async fn persisted_mint_share_response(
        &self,
        operation_id: FederationOperationId,
        input: &MintInput,
    ) -> Result<MintResponse, Error> {
        Ok(MintResponse {
            signatures: self
                .persisted_signature_shares_for_outputs(operation_id, input.outputs())
                .await?,
        })
    }

    /// Return this member's persisted wallet-facing swap share response.
    pub async fn persisted_swap_share_response(
        &self,
        operation_id: FederationOperationId,
        request: &SwapRequest,
    ) -> Result<SwapResponse, Error> {
        Ok(SwapResponse {
            signatures: self
                .persisted_signature_shares_for_outputs(operation_id, request.outputs())
                .await?,
        })
    }

    pub(crate) fn reject_standalone_federation_path(&self, operation: &str) -> Result<(), Error> {
        if self.federation_config().is_some() {
            return Err(Error::Custom(format!(
                "federation member requires consensus acceptance before {operation}"
            )));
        }

        Ok(())
    }

    /// Return the federation runtime config after replaying applied config transitions.
    pub async fn effective_federation_config(
        &self,
    ) -> Result<Option<Arc<FederationRuntimeConfig>>, Error> {
        if self.federation_config().is_none() {
            return Ok(None);
        }
        if let Some(effective) = self.federation_effective_config.load_full() {
            return Ok(Some(effective));
        }

        self.rebuild_effective_federation_config(false)
            .await
            .map(Some)
    }

    /// Rebuild the effective federation config from applied keyset rotations.
    ///
    /// With `force` unset, a config another caller built meanwhile is reused.
    pub(crate) async fn rebuild_effective_federation_config(
        &self,
        force: bool,
    ) -> Result<Arc<FederationRuntimeConfig>, Error> {
        let config = self.federation_config().ok_or(Error::Internal)?;
        let _rebuild = self.federation_effective_config_rebuild.lock().await;
        if !force {
            if let Some(effective) = self.federation_effective_config.load_full() {
                return Ok(effective);
            }
        }

        #[cfg(test)]
        self.federation_effective_config_rebuild_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut effective = config.clone();
        let mut rotations = self
            .localstore
            .get_federation_materialized_state_keys(FederationMaterializedStateKind::KeysetRotation)
            .await?
            .into_iter()
            .filter_map(|record| match record.key {
                FederationMaterializedStateKey::KeysetRotation { keyset } => {
                    Some((record.operation_index, record.operation_id, keyset))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        rotations
            .sort_by_key(|(operation_index, operation_id, _)| (*operation_index, *operation_id));
        for (_, _, keyset) in rotations {
            effective =
                effective.apply_keyset_rotation(&FederationKeysetRotationOperation { keyset })?;
        }

        let effective = Arc::new(effective);
        self.federation_effective_config
            .store(Some(Arc::clone(&effective)));
        Ok(effective)
    }

    /// Run `f` on the federation's current keysets, if this mint is a member.
    ///
    /// Reads the effective config once built, else the configured one.
    pub(crate) fn with_federation_keysets<R, F>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&[FederatedKeyset]) -> R,
    {
        let config = self.federation_config()?;
        let effective = self.federation_effective_config.load();
        let keysets = match effective.as_deref() {
            Some(effective) => &effective.public_config().keysets,
            None => &config.public_config().keysets,
        };
        Some(f(keysets))
    }

    /// Apply accepted federation journal entries to this member's local mint database.
    ///
    /// This is intentionally idempotent so startup replay, duplicate operation
    /// submission, and private REST catch-up can all use the same path.
    pub async fn apply_federation_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<Vec<FederationOperationId>, Error> {
        let outcomes = self
            .apply_federation_journal_entries_batch(entries, false)
            .await?;
        Ok(outcomes
            .into_iter()
            .filter_map(|(operation_id, newly_applied, _)| newly_applied.then_some(operation_id))
            .collect())
    }

    /// Apply a batch of accepted federation journal entries, coalescing
    /// contiguous batchable operations into a single database transaction with
    /// savepoint-isolated deterministic rejections and one frontier update.
    ///
    /// Returns `(operation_id, newly_applied, terminal_application_state)` in
    /// input order. When `record_definitive_rejections` is true, deterministic
    /// apply rejections are persisted as [`FederationApplicationState::Rejected`]
    /// inside the same transaction without aborting earlier or later valid
    /// operations in the unit.
    pub async fn apply_federation_journal_entries_batch(
        &self,
        entries: &[FederationJournalEntry],
        record_definitive_rejections: bool,
    ) -> Result<Vec<(FederationOperationId, bool, FederationApplicationState)>, Error> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        let operation_ids = entries
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>();
        let persisted_states = self
            .localstore
            .get_federation_journal_application_states(&operation_ids)
            .await?;

        let mut outcomes = Vec::with_capacity(entries.len());
        let mut pending_batch: Vec<&FederationJournalEntry> = Vec::new();

        for entry in entries {
            let persisted = persisted_states
                .get(&entry.operation_id)
                .map(|(_, state)| state);
            match persisted {
                Some(
                    state @ (FederationApplicationState::Applied {}
                    | FederationApplicationState::Rejected { .. }
                    | FederationApplicationState::ConsensusRejected {}),
                ) => {
                    if !pending_batch.is_empty() {
                        let segment = std::mem::take(&mut pending_batch);
                        outcomes.extend(
                            self.apply_prepared_batch_segment(
                                &segment,
                                record_definitive_rejections,
                            )
                            .await?,
                        );
                    }
                    if !record_definitive_rejections
                        && state.status() == FederationOperationStatus::Rejected
                    {
                        return Err(Error::Custom(format!(
                            "federation operation {} has local status {} and is not accepted for apply",
                            entry.operation_id,
                            federation_operation_status_label(state.status())
                        )));
                    }
                    outcomes.push((entry.operation_id, false, state.clone()));
                    continue;
                }
                Some(FederationApplicationState::Failed {}) => {
                    if !pending_batch.is_empty() {
                        let segment = std::mem::take(&mut pending_batch);
                        outcomes.extend(
                            self.apply_prepared_batch_segment(
                                &segment,
                                record_definitive_rejections,
                            )
                            .await?,
                        );
                    }
                    outcomes.push(
                        self.apply_single_entry_for_batch(entry, record_definitive_rejections)
                            .await?,
                    );
                    continue;
                }
                None | Some(FederationApplicationState::Accepted {}) => {}
            }

            match entry.status() {
                FederationOperationStatus::Applied => {
                    if !pending_batch.is_empty() {
                        let segment = std::mem::take(&mut pending_batch);
                        outcomes.extend(
                            self.apply_prepared_batch_segment(
                                &segment,
                                record_definitive_rejections,
                            )
                            .await?,
                        );
                    }
                    outcomes.push((entry.operation_id, false, entry.application.clone()));
                    continue;
                }
                FederationOperationStatus::Rejected => {
                    if !pending_batch.is_empty() {
                        let segment = std::mem::take(&mut pending_batch);
                        outcomes.extend(
                            self.apply_prepared_batch_segment(
                                &segment,
                                record_definitive_rejections,
                            )
                            .await?,
                        );
                    }
                    if !record_definitive_rejections {
                        return Err(Error::Custom(format!(
                            "federation operation {} has local status {} and is not accepted for apply",
                            entry.operation_id,
                            federation_operation_status_label(entry.status())
                        )));
                    }
                    outcomes.push((entry.operation_id, false, entry.application.clone()));
                    continue;
                }
                FederationOperationStatus::Failed => {
                    if !pending_batch.is_empty() {
                        let segment = std::mem::take(&mut pending_batch);
                        outcomes.extend(
                            self.apply_prepared_batch_segment(
                                &segment,
                                record_definitive_rejections,
                            )
                            .await?,
                        );
                    }
                    if !record_definitive_rejections {
                        return Err(Error::Custom(format!(
                            "federation operation {} has local status {} and is not accepted for apply",
                            entry.operation_id,
                            federation_operation_status_label(entry.status())
                        )));
                    }
                    outcomes.push(
                        self.apply_single_entry_for_batch(entry, record_definitive_rejections)
                            .await?,
                    );
                    continue;
                }
                FederationOperationStatus::Accepted => {}
            }

            if is_batchable_federation_operation(&entry.envelope.operation) {
                pending_batch.push(entry);
            } else {
                if !pending_batch.is_empty() {
                    let segment = std::mem::take(&mut pending_batch);
                    outcomes.extend(
                        self.apply_prepared_batch_segment(&segment, record_definitive_rejections)
                            .await?,
                    );
                }
                outcomes.push(
                    self.apply_single_entry_for_batch(entry, record_definitive_rejections)
                        .await?,
                );
            }
        }

        if !pending_batch.is_empty() {
            outcomes.extend(
                self.apply_prepared_batch_segment(&pending_batch, record_definitive_rejections)
                    .await?,
            );
        }

        Ok(outcomes)
    }

    async fn apply_single_entry_for_batch(
        &self,
        entry: &FederationJournalEntry,
        record_definitive_rejections: bool,
    ) -> Result<(FederationOperationId, bool, FederationApplicationState), Error> {
        let mut replay_entry = entry.clone();
        if record_definitive_rejections
            && replay_entry.status() == FederationOperationStatus::Failed
        {
            replay_entry.application = FederationApplicationState::Accepted {};
        }
        match self
            .apply_accepted_federation_operation(&replay_entry)
            .await
        {
            Ok(applied) => Ok((
                entry.operation_id,
                applied,
                FederationApplicationState::Applied {},
            )),
            Err(err) if !record_definitive_rejections => Err(err),
            Err(err) if is_retryable_federation_batch_apply_error(&err) => Err(err),
            Err(err) if super::is_definitive_federation_apply_rejection(&err) => {
                let rejection = FederationApplicationState::Rejected {
                    response: cdk_common::error::ErrorResponse::from(err),
                };
                let mut tx = self.localstore.begin_transaction().await?;
                let persist = async {
                    tx.record_federation_operation_application_state_deferred(entry, &rejection)
                        .await?;
                    tx.advance_federation_application_frontier().await
                }
                .await;
                match persist {
                    Ok(()) => tx.commit().await?,
                    Err(db_err) => {
                        tx.rollback().await?;
                        return Err(db_err.into());
                    }
                }
                Ok((entry.operation_id, false, rejection))
            }
            Err(err) => {
                let mut tx = self.localstore.begin_transaction().await?;
                let persist = tx
                    .record_federation_operation_application_state_deferred(
                        entry,
                        &FederationApplicationState::Failed {},
                    )
                    .await;
                match persist {
                    Ok(()) => tx.commit().await?,
                    Err(db_err) => {
                        tx.rollback().await?;
                        return Err(db_err.into());
                    }
                }
                Err(err)
            }
        }
    }

    fn prepare_batch_entry(
        &self,
        config: &FederationRuntimeConfig,
        entry: &FederationJournalEntry,
    ) -> PreparedBatchEntry {
        match &entry.envelope.operation {
            FederationOperation::Swap(operation) => {
                let prepared = (|| {
                    let input_proofs = operation.request.inputs();
                    if input_proofs.is_empty() {
                        return Err(Error::TransactionUnbalanced(
                            0,
                            operation.request.output_amount()?.to_u64(),
                            0,
                        ));
                    }
                    let input_verification = self
                        .verify_federation_inputs_with_config_and_context(
                            config,
                            entry.operation_id,
                            input_proofs,
                            FederationProofVerificationContext::SwapApply,
                        )?;
                    operation.request.verify_spending_conditions()?;
                    // Consensus orders completion but does not attest its wall-clock time.
                    let swap_plan = plan_swap(
                        &config.keysets(),
                        self.max_outputs,
                        input_verification,
                        input_proofs,
                        operation.request.outputs(),
                        federation_operation_uuid(entry.operation_id),
                        Some(0),
                    )?;
                    let input_ys = input_proofs.ys()?;
                    let blinded_secrets = operation
                        .request
                        .outputs()
                        .iter()
                        .map(|output| output.blinded_secret)
                        .collect::<Vec<PublicKey>>();
                    Ok(PreparedBatchSwap {
                        swap_plan,
                        input_ys,
                        blinded_secrets,
                        pre_signed_shares: None,
                    })
                })();
                PreparedBatchEntry::Swap(prepared)
            }
            FederationOperation::Mint(operation) => {
                let prepared = (|| {
                    let input = mint_input_from_federation_request(&operation.request)?;
                    input.validate()?;
                    let output_verification = verification::verify_outputs(
                        &config.keysets(),
                        self.max_outputs,
                        input.outputs(),
                    )?;
                    let blinded_secrets = input
                        .outputs()
                        .iter()
                        .map(|output| output.blinded_secret)
                        .collect::<Vec<PublicKey>>();
                    let quote_ids = input.quote_ids();
                    Ok(PreparedBatchMint {
                        input,
                        outputs_amount: output_verification.amount,
                        blinded_secrets,
                        quote_ids,
                        pre_signed_shares: None,
                    })
                })();
                PreparedBatchEntry::Mint(prepared)
            }
            FederationOperation::MintQuote(operation) => {
                PreparedBatchEntry::MintQuote(mint_quote_from_federation_operation(operation))
            }
            FederationOperation::MintQuotePayment(observation) => {
                let prepared = (|| {
                    let quote_id = quote_id_from_federation(&observation.quote)?;
                    let record = payment_observation_record_from_entry(entry)?;
                    Ok((quote_id, record))
                })();
                PreparedBatchEntry::MintQuotePayment(prepared)
            }
            FederationOperation::MeltQuote(operation) => {
                let prepared = (|| {
                    let quote_id = quote_id_from_federation(&operation.quote)?;
                    let quote = operation.into_melt_quote(quote_id)?;
                    ensure_melt_quote_matches_request(&quote, &operation.request)?;
                    Ok(quote)
                })();
                PreparedBatchEntry::MeltQuote(prepared)
            }
            FederationOperation::MeltQuotePayment(_) => {
                PreparedBatchEntry::MeltQuotePayment(payment_observation_record_from_entry(entry))
            }
            _ => unreachable!("non-batchable federation operation routed to batch segment"),
        }
    }

    async fn federation_history_start(&self) -> Result<Option<u64>, Error> {
        let transfer_base = self
            .localstore
            .get_federation_checkpoint(FederationCheckpointSelector::StateTransferBase)
            .await?
            .map(|base| base.journal_next_index);
        let retention_floor = self
            .localstore
            .get_federation_retention_floor()
            .await?
            .map(|floor| floor.journal_start);
        Ok(transfer_base.max(retention_floor))
    }

    fn apply_prepared_batch_segment<'a>(
        &'a self,
        entries: &'a [&'a FederationJournalEntry],
        record_definitive_rejections: bool,
    ) -> BatchApplyFuture<'a> {
        Box::pin(async move {
            if entries.is_empty() {
                return Ok(Vec::new());
            }

            let config = self
                .effective_federation_config()
                .await?
                .ok_or(Error::Internal)?;
            let needs_history_start = entries.iter().any(|entry| {
                matches!(
                    entry.envelope.operation,
                    FederationOperation::Mint(_)
                        | FederationOperation::MintQuote(_)
                        | FederationOperation::MintQuotePayment(_)
                )
            });
            let history_start = if needs_history_start {
                self.federation_history_start().await?
            } else {
                None
            };

            // Run pure CPU verification & planning before acquiring the DB
            // write transaction so SQLite's write_gate is never held during BLS
            // proof verification or output checks.
            let mut prepared_entries = entries
                .iter()
                .map(|entry| Some(self.prepare_batch_entry(&config, entry)))
                .collect::<Vec<_>>();

            // Pre-sign valid Swap and Mint outputs off the async worker threads
            // and outside the DB write transaction. Shares for entries that
            // later fail stateful DB checks are dropped without persistence.
            let mut signing_indices = Vec::new();
            let mut signing_requests = Vec::new();
            for (idx, (entry, prepared)) in entries.iter().zip(&prepared_entries).enumerate() {
                match (&entry.envelope.operation, prepared.as_ref()) {
                    (
                        FederationOperation::Swap(operation),
                        Some(PreparedBatchEntry::Swap(Ok(_))),
                    ) => {
                        signing_indices.push(idx);
                        signing_requests.push((
                            (*entry).clone(),
                            operation.request.outputs().to_vec(),
                            "swap",
                        ));
                    }
                    (
                        FederationOperation::Mint(_),
                        Some(PreparedBatchEntry::Mint(Ok(prepared_mint))),
                    ) => {
                        signing_indices.push(idx);
                        signing_requests.push((
                            (*entry).clone(),
                            prepared_mint.input.outputs().to_vec(),
                            "mint",
                        ));
                    }
                    _ => {}
                }
            }
            if !signing_requests.is_empty() {
                let signed_results = config.batch_sign_accepted_outputs(signing_requests).await?;
                for (idx, signed_result) in signing_indices.into_iter().zip(signed_results) {
                    match prepared_entries[idx].as_mut() {
                        Some(PreparedBatchEntry::Swap(Ok(prepared_swap))) => {
                            prepared_swap.pre_signed_shares = Some(signed_result);
                        }
                        Some(PreparedBatchEntry::Mint(Ok(prepared_mint))) => {
                            prepared_mint.pre_signed_shares = Some(signed_result);
                        }
                        _ => {}
                    }
                }
            }

            let mut results = Vec::with_capacity(entries.len());
            let mut offset = 0usize;

            const FEDERATION_BATCH_APPLY_SAVEPOINT: &str = "fed_apply_entry";

            while offset < entries.len() {
                let mut tx = self.localstore.begin_transaction().await?;
                let mut tx_results = Vec::with_capacity(entries.len() - offset);
                let mut tx_notifications = Vec::new();
                let mut tx_shares = Vec::new();
                let mut fallback_index: Option<usize> = None;

                for idx in offset..entries.len() {
                    let entry = entries[idx];
                    let Some(prepared) = prepared_entries[idx].take() else {
                        let _ = tx.rollback().await;
                        return Err(Error::Internal);
                    };
                    if let Err(sp_err) = tx
                        .begin_federation_apply_savepoint(FEDERATION_BATCH_APPLY_SAVEPOINT)
                        .await
                    {
                        let _ = tx.rollback().await;
                        return Err(sp_err.into());
                    }

                    let outcome = self
                        .apply_prepared_batch_entry_in_tx(
                            &mut tx,
                            &config,
                            entry,
                            prepared,
                            history_start,
                        )
                        .await;

                    match outcome {
                        Ok((entry_notifications, entry_shares)) => {
                            let mark_result = tx
                                .record_federation_operation_application_state_deferred(
                                    entry,
                                    &FederationApplicationState::Applied {},
                                )
                                .await;
                            match mark_result {
                                Ok(()) => {
                                    if let Err(rel_err) = tx
                                        .release_federation_apply_savepoint(
                                            FEDERATION_BATCH_APPLY_SAVEPOINT,
                                        )
                                        .await
                                    {
                                        let _ = tx.rollback().await;
                                        return Err(rel_err.into());
                                    }
                                    tx_results.push((
                                        entry.operation_id,
                                        true,
                                        FederationApplicationState::Applied {},
                                    ));
                                    tx_notifications.extend(entry_notifications);
                                    if !entry_shares.is_empty() {
                                        tx_shares.push((entry.operation_id, entry_shares));
                                    }
                                }
                                Err(db_err) => {
                                    let _ = tx.rollback().await;
                                    return Err(db_err.into());
                                }
                            }
                        }
                        Err(BatchEntryError::NeedsSingleOpFallback) => {
                            if let Err(rb_err) = tx
                                .rollback_federation_apply_savepoint(
                                    FEDERATION_BATCH_APPLY_SAVEPOINT,
                                )
                                .await
                            {
                                let _ = tx.rollback().await;
                                return Err(rb_err.into());
                            }
                            fallback_index = Some(idx);
                            break;
                        }
                        Err(BatchEntryError::Apply(err))
                            if is_retryable_federation_batch_apply_error(&err) =>
                        {
                            let _ = tx.rollback().await;
                            return Err(err);
                        }
                        Err(BatchEntryError::Apply(err))
                            if record_definitive_rejections
                                && super::is_definitive_federation_apply_rejection(&err) =>
                        {
                            if let Err(rb_err) = tx
                                .rollback_federation_apply_savepoint(
                                    FEDERATION_BATCH_APPLY_SAVEPOINT,
                                )
                                .await
                            {
                                let _ = tx.rollback().await;
                                return Err(rb_err.into());
                            }
                            let rejection = FederationApplicationState::Rejected {
                                response: cdk_common::error::ErrorResponse::from(err),
                            };
                            if let Err(mark_err) = tx
                                .record_federation_operation_application_state_deferred(
                                    entry, &rejection,
                                )
                                .await
                            {
                                let _ = tx.rollback().await;
                                return Err(mark_err.into());
                            }
                            tx_results.push((entry.operation_id, false, rejection));
                        }
                        Err(BatchEntryError::Apply(err)) => {
                            if let Err(rb_err) = tx
                                .rollback_federation_apply_savepoint(
                                    FEDERATION_BATCH_APPLY_SAVEPOINT,
                                )
                                .await
                            {
                                let _ = tx.rollback().await;
                                return Err(rb_err.into());
                            }
                            if record_definitive_rejections {
                                if let Err(mark_err) = tx
                                    .record_federation_operation_application_state_deferred(
                                        entry,
                                        &FederationApplicationState::Failed {},
                                    )
                                    .await
                                {
                                    let _ = tx.rollback().await;
                                    return Err(mark_err.into());
                                }
                                if !tx_results.is_empty() {
                                    if let Err(frontier_err) =
                                        tx.advance_federation_application_frontier().await
                                    {
                                        let _ = tx.rollback().await;
                                        return Err(frontier_err.into());
                                    }
                                }
                                tx.commit().await?;
                                for notification in tx_notifications {
                                    notification.publish(self);
                                }
                                for (operation_id, shares) in tx_shares {
                                    cache_committed_signature_shares(
                                        &self.localstore,
                                        operation_id,
                                        shares,
                                    );
                                }
                            } else if !tx_results.is_empty() {
                                if let Err(frontier_err) =
                                    tx.advance_federation_application_frontier().await
                                {
                                    let _ = tx.rollback().await;
                                    return Err(frontier_err.into());
                                }
                                tx.commit().await?;
                                for notification in tx_notifications {
                                    notification.publish(self);
                                }
                                for (operation_id, shares) in tx_shares {
                                    cache_committed_signature_shares(
                                        &self.localstore,
                                        operation_id,
                                        shares,
                                    );
                                }
                            } else {
                                tx.rollback().await?;
                            }
                            return Err(err);
                        }
                    }
                }

                if !tx_results.is_empty() {
                    if let Err(frontier_err) = tx.advance_federation_application_frontier().await {
                        let _ = tx.rollback().await;
                        return Err(frontier_err.into());
                    }
                    tx.commit().await?;
                    for notification in tx_notifications {
                        notification.publish(self);
                    }
                    for (operation_id, shares) in tx_shares {
                        cache_committed_signature_shares(&self.localstore, operation_id, shares);
                    }
                    results.extend(tx_results);
                } else {
                    tx.rollback().await?;
                }

                match fallback_index {
                    Some(idx) => {
                        results.push(
                            self.apply_single_entry_for_batch(
                                entries[idx],
                                record_definitive_rejections,
                            )
                            .await?,
                        );
                        offset = idx.saturating_add(1);
                    }
                    None => break,
                }
            }

            Ok(results)
        })
    }

    async fn apply_prepared_batch_entry_in_tx(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        config: &FederationRuntimeConfig,
        entry: &FederationJournalEntry,
        prepared: PreparedBatchEntry,
        history_start: Option<u64>,
    ) -> Result<
        (
            Vec<BatchPostCommitNotification>,
            Vec<FederationSignatureShareRecord>,
        ),
        BatchEntryError,
    > {
        match (&entry.envelope.operation, prepared) {
            (FederationOperation::Swap(operation), PreparedBatchEntry::Swap(prepared)) => {
                let prepared = prepared?;
                let input_proofs = operation.request.inputs();
                let mut proofs = match tx
                    .add_proofs(input_proofs.clone(), None, &prepared.swap_plan.operation)
                    .await
                {
                    Ok(proofs) => proofs,
                    Err(database::Error::Duplicate | database::Error::AttemptUpdateSpentProof) => {
                        tx.get_proofs(&prepared.input_ys).await?
                    }
                    Err(err) => return Err(err.into()),
                };

                let existing_signatures =
                    tx.get_blind_signatures(&prepared.blinded_secrets).await?;
                let signatures_present = existing_signatures
                    .iter()
                    .filter(|sig| sig.is_some())
                    .count();

                match proofs.state {
                    State::Unspent if signatures_present == 0 => {}
                    State::Unspent if signatures_present == existing_signatures.len() => {
                        return Err(Error::BlindedMessageAlreadySigned.into());
                    }
                    State::Unspent => {
                        return Err(Error::Custom(
                            "federation swap found partially persisted signatures for fresh inputs"
                                .to_string(),
                        )
                        .into());
                    }
                    State::Spent
                        if signatures_present == existing_signatures.len()
                            && signatures_present > 0 =>
                    {
                        return Err(BatchEntryError::NeedsSingleOpFallback);
                    }
                    State::Spent => {
                        return Err(Error::TokenAlreadySpent.into());
                    }
                    State::Pending | State::Reserved | State::PendingSpent => {
                        return Err(Error::TokenPending.into());
                    }
                }

                Mint::update_proofs_state(tx, &mut proofs, State::Spent).await?;

                let mut signature_share_records = Vec::new();
                if signatures_present == 0 {
                    let share_set = match prepared.pre_signed_shares {
                        Some(share_set) => share_set?,
                        None => config.swap_signature_shares(entry, &operation.request)?,
                    };
                    signature_share_records =
                        FederationSignatureShareRecord::from_accepted_journal_entry(
                            &share_set, entry,
                        )?;
                    let signatures = federation_share_signatures(share_set);
                    tx.add_federation_signature_shares(&signature_share_records)
                        .await?;
                    tx.add_blinded_messages(
                        None,
                        operation.request.outputs(),
                        &prepared.swap_plan.operation,
                    )
                    .await?;
                    tx.add_blind_signatures(&prepared.blinded_secrets, &signatures, None)
                        .await?;
                    tx.add_completed_operation(
                        &prepared.swap_plan.operation,
                        &prepared.swap_plan.fee_breakdown.per_keyset,
                    )
                    .await?;
                }

                #[cfg(test)]
                if crate::test_helpers::mint::should_fail_for("FEDERATION_SWAP_APPLY_BEFORE_STATUS")
                {
                    return Err(Error::Internal.into());
                }

                Ok((
                    vec![BatchPostCommitNotification::ProofsSpent(prepared.input_ys)],
                    signature_share_records,
                ))
            }
            (FederationOperation::Mint(_), PreparedBatchEntry::Mint(prepared)) => {
                let prepared = prepared?;
                for quote_id in &prepared.quote_ids {
                    Self::require_prior_applied_quote_operation_in_tx(
                        tx,
                        FederationOperationConflictKey::MintQuote {
                            quote: quote_id.to_string(),
                        },
                        entry.index,
                        history_start,
                    )
                    .await?;
                }

                let mut quotes = Vec::with_capacity(prepared.quote_ids.len());
                for quote_id in &prepared.quote_ids {
                    quotes.push(
                        tx.get_mint_quote(quote_id)
                            .await?
                            .ok_or(Error::UnknownQuote)?,
                    );
                }

                let (payment_method, unit) = crate::mint::issue::uniform_method_and_unit(
                    quotes.iter().map(|quote| &**quote),
                )?;
                if prepared.outputs_amount.unit() != &unit {
                    return Err(Error::UnsupportedUnit.into());
                }
                let expected_amounts = expected_federation_mint_amounts(
                    &prepared.input,
                    &quotes,
                    prepared.outputs_amount.clone(),
                )?;
                let total_expected = expected_amounts.iter().try_fold(0_u64, |total, amount| {
                    total
                        .checked_add(amount.value())
                        .ok_or(Error::AmountOverflow)
                })?;
                if prepared.outputs_amount.value() != total_expected {
                    return Err(Error::TransactionUnbalanced(
                        total_expected,
                        prepared.outputs_amount.value(),
                        0,
                    )
                    .into());
                }

                prepared
                    .input
                    .verify_quote_signatures(quotes.iter().map(|quote| &**quote))?;

                let existing_signatures =
                    tx.get_blind_signatures(&prepared.blinded_secrets).await?;
                let signatures_present = existing_signatures
                    .iter()
                    .filter(|sig| sig.is_some())
                    .count();
                if signatures_present != 0 && signatures_present != existing_signatures.len() {
                    return Err(Error::Custom(
                        "federation mint replay found partially persisted signatures".to_string(),
                    )
                    .into());
                }
                validate_federation_mint_quote_states_for_signing(
                    &quotes,
                    &expected_amounts,
                    signatures_present,
                    existing_signatures.len(),
                )?;

                for (quote, amount_issued) in quotes.iter().zip(&expected_amounts) {
                    let required_amount = quote
                        .amount_issued()
                        .checked_add(amount_issued)
                        .map_err(|_| Error::AmountOverflow)?;
                    let observations = tx
                        .get_federation_payment_observations(
                            FederationPaymentObservationSelection::Live {
                                context: FederationPaymentObservationContext::MintQuote,
                                quote: &quote.id.to_string(),
                            },
                        )
                        .await?
                        .into_iter()
                        .map(|observation| observation.record)
                        .collect::<Vec<_>>();
                    if !self
                        .federation_mint_payment_evidence_covers_amount_with_history_start(
                            tx,
                            quote,
                            &required_amount,
                            &observations,
                            entry.index,
                            history_start,
                        )
                        .await?
                    {
                        return Err(Error::UnpaidQuote.into());
                    }
                }

                // Consensus orders completion but does not attest its wall-clock time.
                let completed_at = Some(0);
                let operation_record = if prepared.input.is_batch() {
                    Operation::new(
                        federation_operation_uuid(entry.operation_id),
                        OperationKind::BatchMint,
                        prepared.outputs_amount.clone().into(),
                        Amount::ZERO,
                        Amount::ZERO,
                        completed_at,
                        Some(payment_method.clone()),
                    )
                } else {
                    Operation::new(
                        federation_operation_uuid(entry.operation_id),
                        OperationKind::Mint,
                        prepared.outputs_amount.clone().into(),
                        Amount::ZERO,
                        Amount::ZERO,
                        completed_at,
                        Some(payment_method.clone()),
                    )
                };

                let share_set = match prepared.pre_signed_shares {
                    Some(share_set) => share_set?,
                    None => config.mint_signature_shares(entry, &prepared.input)?,
                };
                let signature_share_records =
                    FederationSignatureShareRecord::from_accepted_journal_entry(&share_set, entry)?;
                tx.add_federation_signature_shares(&signature_share_records)
                    .await?;
                let mut notifications = Vec::new();

                if signatures_present == 0 {
                    let signatures = federation_share_signatures(share_set);
                    let quote_id = match prepared.input.is_batch() {
                        true => None,
                        false => prepared.quote_ids.first().cloned(),
                    };
                    tx.add_blinded_messages(
                        quote_id.as_ref(),
                        prepared.input.outputs(),
                        &operation_record,
                    )
                    .await?;
                    tx.add_blind_signatures(&prepared.blinded_secrets, &signatures, quote_id)
                        .await?;
                    let fee_by_keyset = HashMap::new();
                    tx.add_completed_operation(&operation_record, &fee_by_keyset)
                        .await?;
                }

                for (quote, amount_issued) in quotes.iter_mut().zip(expected_amounts) {
                    match quote.state() {
                        MintQuoteState::Paid => {
                            quote.add_issuance(amount_issued)?;
                            tx.update_mint_quote(quote).await?;
                            notifications.push(BatchPostCommitNotification::MintQuoteIssue(
                                (**quote).clone(),
                                quote.amount_issued(),
                            ));
                        }
                        MintQuoteState::Issued => {
                            if quote.amount_issued() < amount_issued {
                                return Err(Error::IssuedQuote.into());
                            }
                        }
                        MintQuoteState::Unpaid => {
                            return Err(Error::UnpaidQuote.into());
                        }
                    }
                    retire_final_mint_quote_observations(tx, quote, entry.index).await?;
                }

                #[cfg(test)]
                if crate::test_helpers::mint::should_fail_for("FEDERATION_MINT_APPLY_BEFORE_STATUS")
                {
                    return Err(Error::Internal.into());
                }

                Ok((notifications, signature_share_records))
            }
            (
                FederationOperation::MintQuote(operation),
                PreparedBatchEntry::MintQuote(prepared),
            ) => {
                self.validate_federation_onchain_mint_quote_allocation_in_tx(tx, operation)
                    .await?;
                let proposed_quote = prepared?;
                let mut quote = match tx.get_mint_quote(&proposed_quote.id).await? {
                    Some(existing) => {
                        ensure_mint_quote_matches(&existing, &proposed_quote)?;
                        existing
                    }
                    None => {
                        tx.add_mint_quote(proposed_quote.clone()).await?;
                        database::mint::Acquired::from(proposed_quote)
                    }
                };
                let observations = tx
                    .get_federation_payment_observations(
                        FederationPaymentObservationSelection::Live {
                            context: FederationPaymentObservationContext::MintQuote,
                            quote: &operation.quote,
                        },
                    )
                    .await?
                    .into_iter()
                    .map(|observation| observation.record)
                    .collect::<Vec<_>>();
                let paid_notification = self
                    .apply_mint_payment_observations_to_quote_with_history_start(
                        tx,
                        &mut quote,
                        &observations,
                        history_start,
                    )
                    .await?
                    .map(|amount_paid| ((*quote).clone(), amount_paid));
                let mut notifications = vec![BatchPostCommitNotification::MintQuoteCreated(
                    (*quote).clone(),
                )];
                if let Some((paid_quote, amount_paid)) = paid_notification {
                    notifications.push(BatchPostCommitNotification::MintQuotePayment(
                        paid_quote,
                        amount_paid,
                    ));
                }
                Ok((notifications, Vec::new()))
            }
            (
                FederationOperation::MintQuotePayment(observation),
                PreparedBatchEntry::MintQuotePayment(prepared),
            ) => {
                let (quote_id, record) = prepared?;
                Self::require_prior_applied_quote_operation_in_tx(
                    tx,
                    FederationOperationConflictKey::MintQuote {
                        quote: quote_id.to_string(),
                    },
                    entry.index,
                    history_start,
                )
                .await?;
                let mut quote = tx.get_mint_quote(&quote_id).await?;
                if quote.is_none()
                    && observation.method.is_onchain()
                    && self
                        .federation_config()
                        .is_some_and(|runtime| runtime.public_config().frost_wallet.is_some())
                {
                    return Err(Error::UnknownQuote.into());
                }
                if let Some(quote) = quote.as_deref() {
                    self.validate_federation_onchain_payment_observation(
                        tx,
                        quote,
                        observation,
                        record.operation_index,
                        false,
                    )
                    .await?;
                }
                if quote.as_deref().is_some_and(mint_quote_payments_are_final) {
                    return Ok((Vec::new(), Vec::new()));
                }
                tx.add_federation_payment_observation(&record, None).await?;
                let observations = tx
                    .get_federation_payment_observations(
                        FederationPaymentObservationSelection::Live {
                            context: FederationPaymentObservationContext::MintQuote,
                            quote: &observation.quote,
                        },
                    )
                    .await?
                    .into_iter()
                    .map(|observation| observation.record)
                    .collect::<Vec<_>>();
                let mut notifications = Vec::new();
                if let Some(mut quote) = quote.take() {
                    if let Some(amount_paid) = self
                        .apply_mint_payment_observations_to_quote_with_history_start(
                            tx,
                            &mut quote,
                            &observations,
                            history_start,
                        )
                        .await?
                    {
                        notifications.push(BatchPostCommitNotification::MintQuotePayment(
                            (*quote).clone(),
                            amount_paid,
                        ));
                    }
                    settle_paid_mint_quote_application(tx, &quote, entry.index).await?;
                }
                Ok((notifications, Vec::new()))
            }
            (
                FederationOperation::MeltQuote(operation),
                PreparedBatchEntry::MeltQuote(prepared),
            ) => {
                let quote = prepared?;
                if let Some(existing) = tx.get_melt_quote(&quote.id).await? {
                    ensure_melt_quote_matches_request(&existing, &operation.request)?;
                    if !federation_melt_quote_immutable_terms_match(&existing, &quote) {
                        return Err(Error::InvalidPaymentRequest.into());
                    }
                } else {
                    tx.add_melt_quote(quote).await?;
                }
                Ok((Vec::new(), Vec::new()))
            }
            (
                FederationOperation::MeltQuotePayment(observation),
                PreparedBatchEntry::MeltQuotePayment(prepared),
            ) => {
                let record = prepared?;
                let notifications = self
                    .apply_federation_melt_quote_payment_record_in_transaction(
                        tx,
                        config,
                        entry,
                        observation,
                        &record,
                    )
                    .await?;
                Ok((
                    vec![BatchPostCommitNotification::Melt(notifications)],
                    Vec::new(),
                ))
            }
            _ => unreachable!("prepared batch entry must match operation variant"),
        }
    }

    /// Apply one accepted federation operation to this member's local mint database.
    ///
    /// This is the deterministic apply boundary used by journal replay, live
    /// finalization, and catch-up. Admission checks may reject malformed
    /// requests earlier, but accepted-order apply remains the authoritative
    /// stateful validity decision. Already applied entries are skipped for
    /// idempotent replay; locally rejected or failed entries are never
    /// materialized again through this path.
    ///
    /// Returns a boxed future so callers do not embed the whole apply state
    /// machine in their own; see [`Self::federation_operation_apply_future`].
    pub fn apply_accepted_federation_operation<'a>(
        &'a self,
        entry: &'a FederationJournalEntry,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(self.apply_accepted_federation_operation_inner(entry))
    }

    async fn apply_accepted_federation_operation_inner(
        &self,
        entry: &FederationJournalEntry,
    ) -> Result<bool, Error> {
        if let Some(persisted_entry) = self
            .localstore
            .get_federation_journal_entry(
                FederationJournalEntryKey::Operation(entry.operation_id),
                None,
            )
            .await?
        {
            match persisted_entry.status() {
                FederationOperationStatus::Applied => return Ok(false),
                FederationOperationStatus::Rejected => {
                    return Err(Error::Custom(format!(
                        "federation operation {} has local status {} and is not accepted for apply",
                        persisted_entry.operation_id,
                        federation_operation_status_label(persisted_entry.status())
                    )));
                }
                FederationOperationStatus::Failed => {
                    ensure_same_federation_journal_entry_for_retry(&persisted_entry, entry)?;
                }
                FederationOperationStatus::Accepted => {}
            }
        }

        match entry.status() {
            FederationOperationStatus::Accepted => {}
            FederationOperationStatus::Applied => return Ok(false),
            FederationOperationStatus::Rejected | FederationOperationStatus::Failed => {
                return Err(Error::Custom(format!(
                    "federation operation {} has local status {} and is not accepted for apply",
                    entry.operation_id,
                    federation_operation_status_label(entry.status())
                )));
            }
        }

        self.federation_operation_apply_future(entry)?.await?;
        Ok(true)
    }

    /// Build the future that applies one accepted operation by kind.
    ///
    /// This is a plain function returning boxed futures, so the caller's
    /// frame holds a pointer instead of every operation's state machine.
    /// Without it, debug builds overflow the default 2 MiB thread stack on
    /// the melt path, where submission, finalization and apply nest.
    fn federation_operation_apply_future<'a>(
        &'a self,
        entry: &'a FederationJournalEntry,
    ) -> Result<BoxFuture<'a, Result<(), Error>>, Error> {
        Ok(match &entry.envelope.operation {
            FederationOperation::AddressAllocation(operation) => Box::pin(async move {
                self.apply_federation_address_allocation(entry, operation)
                    .await
                    .map(drop)
            }),
            FederationOperation::ChainObservation(observation) => Box::pin(async move {
                self.apply_federation_chain_observation(entry, observation)
                    .await
                    .map(drop)
            }),
            FederationOperation::MintQuote(operation) => {
                Box::pin(self.apply_federation_mint_quote(entry, operation))
            }
            FederationOperation::MintQuotePayment(observation) => {
                Box::pin(self.apply_federation_mint_quote_payment(entry, observation))
            }
            FederationOperation::Mint(operation) => {
                Box::pin(self.apply_federation_mint(entry, operation))
            }
            FederationOperation::MeltQuote(operation) => {
                Box::pin(self.apply_federation_melt_quote(entry, operation))
            }
            FederationOperation::MeltQuotePayment(observation) => {
                Box::pin(self.apply_federation_melt_quote_payment(entry, observation))
            }
            FederationOperation::Swap(operation) => {
                Box::pin(self.apply_federation_swap(entry, operation))
            }
            FederationOperation::Melt(operation) => {
                Box::pin(self.apply_federation_melt(entry, operation))
            }
            FederationOperation::KeysetRotation(operation) => {
                Box::pin(self.apply_federation_keyset_rotation(entry, operation))
            }
            FederationOperation::TransactionProposal(proposal) => Box::pin(async move {
                self.apply_federation_transaction_proposal(entry, proposal)
                    .await
                    .map(drop)
            }),
            FederationOperation::TransactionSigned(signed) => Box::pin(async move {
                self.apply_federation_signed_transaction(entry, signed)
                    .await
                    .map(drop)
            }),
            FederationOperation::TransactionBroadcastIntent(intent) => Box::pin(async move {
                self.apply_federation_transaction_broadcast_intent(entry, intent)
                    .await
                    .map(drop)
            }),
            FederationOperation::TransactionBroadcastObservation(observation) => {
                Box::pin(async move {
                    self.apply_federation_transaction_broadcast_observation(entry, observation)
                        .await
                        .map(drop)
                })
            }
            #[cfg(feature = "federation-bark")]
            FederationOperation::Application(_) => {
                let operation = cdk_federation_bark::validate_federation_bark_operation(
                    &entry.envelope.operation,
                )?;
                Box::pin(async move { self.apply_federation_bark(entry, &operation).await })
            }
            #[cfg(not(feature = "federation-bark"))]
            FederationOperation::Application(_) => {
                return Err(FederationError::ApplicationOperationInvalid {
                    reason: "federation application support is disabled",
                }
                .into())
            }
        })
    }

    #[cfg(feature = "federation-bark")]
    async fn apply_federation_bark(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationBarkOperation,
    ) -> Result<(), Error> {
        operation.validate()?;
        if self
            .federation_bark_operation_recorded(entry, operation)
            .await?
        {
            // The projection already holds this exact operation, committed
            // together with its payment and recovery effects. An operation
            // re-ordered after its validity window is already applied; its
            // validation against the action's current phase would fail.
            let transaction = self.localstore.begin_transaction().await?;
            return finish_federation_apply_transaction(transaction, entry, Ok(())).await;
        }
        self.validate_federation_bark_operation_submission(operation, Some(entry.index))
            .await?;
        if let FederationBarkOperation::ReceiveIntent(intent) = operation {
            let runtime_config = self.federation_config().ok_or(Error::Internal)?;
            intent.validate_secret_acknowledgements(runtime_config.public_config())?;
        }
        let application = match &entry.envelope.operation {
            FederationOperation::Application(application) => application,
            _ => return Err(Error::Internal),
        };
        let payment_records = match operation {
            FederationBarkOperation::MemberObservation(observation) => {
                self.federation_bark_result_payment_records(entry, observation)
                    .await?
            }
            FederationBarkOperation::EvidenceCertificate(certificate) => {
                self.federation_bark_certificate_payment_records(entry, certificate)
                    .await?
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.evidence_certificate.is_some() =>
            {
                self.federation_bark_transition_certificate_payment_records(entry, transition)
                    .await?
            }
            FederationBarkOperation::ReceiveIntent(_)
            | FederationBarkOperation::ReceivePrepared(_)
            | FederationBarkOperation::ActionPlan(_)
            | FederationBarkOperation::ActionTransition(_) => Vec::new(),
        };
        let representative = payment_records
            .iter()
            .find(|record| record.operation_id == entry.operation_id)
            .or_else(|| payment_records.last())
            .cloned();
        let recovery_mutation = self.federation_bark_recovery_mutation(operation).await?;

        if let Some((representative, FederationPaymentObservation::MeltQuote(observation))) =
            representative
                .as_ref()
                .map(|record| (record, &record.observation))
        {
            // Bark melt payment evidence: the projection rows, every
            // observation and the melt outcome commit together with the
            // applied mark, or not at all.
            let config = self
                .effective_federation_config()
                .await?
                .ok_or(Error::Internal)?;
            let mut transaction = self.localstore.begin_transaction().await?;
            let outcome = async {
                transaction
                    .apply_federation_application_operation(entry, application)
                    .await?;
                for record in &payment_records {
                    record_live_payment_observation(&mut transaction, record).await?;
                }

                self.apply_federation_melt_quote_payment_record_in_transaction(
                    &mut transaction,
                    &config,
                    entry,
                    observation,
                    representative,
                )
                .await
            }
            .await;
            let notifications =
                finish_federation_apply_transaction(transaction, entry, outcome).await?;
            notifications.publish(&self.pubsub_manager);
            return Ok(());
        }

        let mut transaction = self.localstore.begin_transaction().await?;
        let outcome = async {
            transaction
                .apply_federation_application_operation(entry, application)
                .await?;
            if let Some(mutation) = recovery_mutation {
                Self::apply_federation_bark_recovery_mutation(&mut transaction, entry, mutation)
                    .await?;
            }
            for record in &payment_records {
                record_live_payment_observation(&mut transaction, record).await?;
            }
            let mut paid_notification = None;
            if let Some(FederationPaymentObservation::MintQuote(observation)) =
                representative.as_ref().map(|record| &record.observation)
            {
                let quote_id = quote_id_from_federation(&observation.quote)?;
                let mut quote = transaction
                    .get_mint_quote(&quote_id)
                    .await?
                    .ok_or(Error::UnknownQuote)?;
                let observations = transaction
                    .get_federation_payment_observations(
                        FederationPaymentObservationSelection::Live {
                            context: FederationPaymentObservationContext::MintQuote,
                            quote: &observation.quote,
                        },
                    )
                    .await?
                    .into_iter()
                    .map(|observation| observation.record)
                    .collect::<Vec<_>>();
                if let Some(amount_paid) = self
                    .apply_mint_payment_observations_to_quote(
                        &mut transaction,
                        &mut quote,
                        &observations,
                    )
                    .await?
                {
                    paid_notification = Some(((*quote).clone(), amount_paid));
                }
                settle_paid_mint_quote_application(&mut transaction, &quote, entry.index).await?;
            }
            Ok(paid_notification)
        }
        .await;
        let paid_notification =
            finish_federation_apply_transaction(transaction, entry, outcome).await?;
        if let Some((quote, amount_paid)) = paid_notification {
            self.pubsub_manager.mint_quote_payment(&quote, amount_paid);
        }
        Ok(())
    }

    /// Whether the Bark projection recorded `entry`'s operation id for its
    /// action (as the action origin, a transition or an observation), or the
    /// action is retired.
    #[cfg(feature = "federation-bark")]
    async fn federation_bark_operation_recorded(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationBarkOperation,
    ) -> Result<bool, Error> {
        let store = self.federation_bark_store()?;
        let action_id = operation.action_id();
        let Some(action) = store.bark_action(action_id).await? else {
            return Ok(false);
        };
        // A retired action changes no more. Its transitions and observations
        // may already be deleted, so this is decided from the action alone.
        if action.retired_operation_index().is_some()
            || action.origin_operation_id == entry.operation_id
            || store
                .bark_action_transitions(action_id)
                .await?
                .iter()
                .any(|transition| transition.operation_id == entry.operation_id)
        {
            return Ok(true);
        }
        Ok(store
            .bark_action_observations(action_id)
            .await?
            .iter()
            .any(|observation| observation.operation_id == entry.operation_id))
    }

    #[cfg(feature = "federation-bark")]
    async fn federation_bark_recovery_mutation(
        &self,
        operation: &FederationBarkOperation,
    ) -> Result<Option<FederationBarkRecoveryMutation>, Error> {
        let FederationBarkOperation::ActionTransition(transition) = operation else {
            return Ok(None);
        };
        match transition.phase {
            FederationBarkSemanticPhase::ExitRegistered => {
                let package_sets = transition
                    .artifacts
                    .iter()
                    .filter_map(|artifact| {
                        FederationBarkCpfpPackageSet::from_artifact(artifact).ok()
                    })
                    .collect::<Vec<_>>();
                let [package_set] = package_sets.as_slice() else {
                    return Err(Error::Internal);
                };
                Ok(Some(FederationBarkRecoveryMutation::Reserve {
                    package_set: package_set.clone(),
                }))
            }
            FederationBarkSemanticPhase::ExitPublished => {
                let publications = transition
                    .artifacts
                    .iter()
                    .filter_map(|artifact| {
                        FederationBarkCpfpPublication::from_artifact(artifact).ok()
                    })
                    .collect::<Vec<_>>();
                let [publication] = publications.as_slice() else {
                    return Err(Error::Internal);
                };
                let registered_operation_id = self
                    .federation_bark_store()?
                    .bark_action_transitions(transition.action_id)
                    .await?
                    .iter()
                    .find(|record| {
                        record.action_id == transition.action_id
                            && record.phase == FederationBarkSemanticPhase::ExitRegistered
                    })
                    .map(|record| record.operation_id)
                    .ok_or(Error::Internal)?;
                let registered_entry = self
                    .localstore()
                    .get_federation_journal_entry(
                        FederationJournalEntryKey::Operation(registered_operation_id),
                        None,
                    )
                    .await?
                    .ok_or(Error::Internal)?;
                let FederationBarkOperation::ActionTransition(registered) =
                    projection::bark_operation(&registered_entry.envelope.operation)?
                else {
                    return Err(Error::Internal);
                };
                let package_sets = registered
                    .artifacts
                    .iter()
                    .filter_map(|artifact| {
                        FederationBarkCpfpPackageSet::from_artifact(artifact).ok()
                    })
                    .collect::<Vec<_>>();
                let [package_set] = package_sets.as_slice() else {
                    return Err(Error::Internal);
                };
                let expected = FederationBarkCpfpPublication::new(
                    package_set,
                    publication.signed_children.clone(),
                )?;
                if publication != &expected {
                    return Err(Error::Internal);
                }
                Ok(Some(FederationBarkRecoveryMutation::Publish {
                    package_set: package_set.clone(),
                    registered_operation_id,
                    publication: publication.clone(),
                }))
            }
            _ => Ok(None),
        }
    }

    #[cfg(feature = "federation-bark")]
    async fn apply_federation_bark_recovery_mutation(
        transaction: &mut database::mint::DynMintTransaction,
        entry: &FederationJournalEntry,
        mutation: FederationBarkRecoveryMutation,
    ) -> Result<(), Error> {
        let mut chain_state = cdk_common::federation::FederationStoredState::<
            cdk_common::federation::FederationChainObservationState,
        >::load(transaction)
        .await?;
        let mut recovery_state = cdk_common::federation::FederationStoredState::<
            FederationBarkRecoveryState,
        >::load(transaction)
        .await?;
        match mutation {
            FederationBarkRecoveryMutation::Reserve { package_set } => {
                let outpoints = package_set
                    .packages
                    .iter()
                    .flat_map(|package| &package.recovery_inputs)
                    .map(|input| input.outpoint)
                    .collect::<Vec<_>>();
                if recovery_state.reserve(&package_set, entry.operation_id)? {
                    chain_state.reserve_utxos(&outpoints)?;
                } else if outpoints.iter().any(|outpoint| {
                    chain_state.utxo(*outpoint).is_none_or(|utxo| {
                        utxo.state != cdk_common::federation::FederationWalletUtxoState::Reserved
                    })
                }) {
                    return Err(Error::Internal);
                }
            }
            FederationBarkRecoveryMutation::Publish {
                package_set,
                registered_operation_id,
                publication,
            } => {
                let expected = FederationBarkCpfpPublication::new(
                    &package_set,
                    publication.signed_children.clone(),
                )?;
                if publication != expected {
                    return Err(Error::Internal);
                }
                let outpoints = package_set
                    .packages
                    .iter()
                    .flat_map(|package| &package.recovery_inputs)
                    .map(|input| input.outpoint)
                    .collect::<Vec<_>>();
                if recovery_state.publish(
                    &package_set,
                    registered_operation_id,
                    entry.operation_id,
                    entry.index,
                )? {
                    chain_state.spend_reserved_utxos(&outpoints, entry.index)?;
                } else if outpoints
                    .iter()
                    .any(|outpoint| !chain_state.is_spent(*outpoint))
                {
                    return Err(Error::Internal);
                }
            }
        }
        chain_state.save(transaction).await?;
        recovery_state.save(transaction).await
    }

    async fn require_prior_applied_melt_quote(
        &self,
        quote: &QuoteId,
        max_operation_index: u64,
    ) -> Result<(), Error> {
        self.require_prior_applied_quote_operation(
            FederationOperationConflictKey::MeltQuote {
                quote: quote.to_string(),
            },
            max_operation_index,
        )
        .await
    }

    async fn require_prior_applied_mint_quote(
        &self,
        quote: &QuoteId,
        max_operation_index: u64,
    ) -> Result<(), Error> {
        self.require_prior_applied_quote_operation(
            FederationOperationConflictKey::MintQuote {
                quote: quote.to_string(),
            },
            max_operation_index,
        )
        .await
    }

    async fn require_prior_applied_mint_quotes(
        &self,
        quotes: &[QuoteId],
        max_operation_index: u64,
    ) -> Result<(), Error> {
        for quote in quotes {
            self.require_prior_applied_mint_quote(quote, max_operation_index)
                .await?;
        }
        Ok(())
    }

    async fn require_prior_applied_quote_operation_in_tx(
        tx: &mut database::mint::DynMintTransaction,
        quote_key: FederationOperationConflictKey,
        max_operation_index: u64,
        history_start: Option<u64>,
    ) -> Result<(), Error> {
        let applied_before = tx
            .get_federation_journal_entry(FederationJournalEntryKey::ConflictKey(&quote_key), None)
            .await?
            .is_some_and(|entry| {
                entry.index < max_operation_index
                    && entry.status() == FederationOperationStatus::Applied
            });

        if applied_before {
            return Ok(());
        }
        let Some(history_start) = history_start else {
            return Err(Error::UnknownQuote);
        };
        if history_start > max_operation_index {
            return Err(Error::UnknownQuote);
        }
        let installed = match &quote_key {
            FederationOperationConflictKey::MintQuote { quote } => tx
                .get_mint_quote(&quote_id_from_federation(quote)?)
                .await?
                .is_some(),
            FederationOperationConflictKey::MeltQuote { quote } => tx
                .get_melt_quote(&quote_id_from_federation(quote)?)
                .await?
                .is_some(),
            _ => false,
        };
        if installed {
            Ok(())
        } else {
            Err(Error::UnknownQuote)
        }
    }

    /// Requires that the operation creating a quote was applied before
    /// `max_operation_index`.
    ///
    /// A quote's conflict key is reserved by exactly one journal entry: the
    /// quote operation itself. Any later operation with the same key is
    /// rejected by consensus and never applied. Reading that one entry
    /// therefore answers the question without scanning the journal.
    async fn require_prior_applied_quote_operation(
        &self,
        quote_key: FederationOperationConflictKey,
        max_operation_index: u64,
    ) -> Result<(), Error> {
        let applied_before = self
            .localstore
            .get_federation_journal_entry(FederationJournalEntryKey::ConflictKey(&quote_key), None)
            .await?
            .is_some_and(|entry| {
                entry.index < max_operation_index
                    && entry.status() == FederationOperationStatus::Applied
            });

        if applied_before {
            return Ok(());
        }
        // A quote created below the first journal entry this member holds
        // (a state transfer base, or the retention floor once older entries
        // are deleted) has no journal row here. Operations apply in order, so
        // its stored row is the proof that the creating operation was
        // applied before `max_operation_index`.
        let transfer_base = self
            .localstore
            .get_federation_checkpoint(FederationCheckpointSelector::StateTransferBase)
            .await?
            .map(|base| base.journal_next_index);
        let retention_floor = self
            .localstore
            .get_federation_retention_floor()
            .await?
            .map(|floor| floor.journal_start);
        let Some(history_start) = transfer_base.max(retention_floor) else {
            return Err(Error::UnknownQuote);
        };
        if history_start > max_operation_index {
            return Err(Error::UnknownQuote);
        }
        let installed = match &quote_key {
            FederationOperationConflictKey::MintQuote { quote } => self
                .localstore
                .get_mint_quote(&quote_id_from_federation(quote)?)
                .await?
                .is_some(),
            FederationOperationConflictKey::MeltQuote { quote } => self
                .localstore
                .get_melt_quote(&quote_id_from_federation(quote)?)
                .await?
                .is_some(),
            _ => false,
        };
        if installed {
            Ok(())
        } else {
            Err(Error::UnknownQuote)
        }
    }

    fn federation_payment_observation_can_finalize_at_or_before(
        &self,
        candidate: &FederationPaymentObservation,
        observations: &[FederationPaymentObservationRecord],
        max_operation_index: u64,
    ) -> Result<bool, Error> {
        let config = self.federation_config().ok_or(Error::Internal)?;
        let policy = *config
            .public_config()
            .payment_verification_policy(candidate.context());
        let mut eligible = observations
            .iter()
            .filter(|record| record.operation_index <= max_operation_index)
            .cloned()
            .collect::<Vec<_>>();
        let is_onchain = match candidate {
            FederationPaymentObservation::MintQuote(observation) => observation.method.is_onchain(),
            FederationPaymentObservation::MeltQuote(observation) => observation.method.is_onchain(),
        };
        if is_onchain {
            eligible.sort_by_key(|record| record.operation_index);
            let mut latest_by_observer = HashMap::new();
            for record in eligible {
                latest_by_observer.insert(record.observation.observer(), record);
            }
            eligible = latest_by_observer.into_values().collect();
            eligible.sort_by_key(|record| record.operation_index);
        }
        let required = policy.required_matching_observations();
        let quorum = candidate.quorum(eligible.iter().map(|record| &record.observation), required);

        match policy.evaluate_quorum(&quorum) {
            Ok(()) => {
                let report = FederationPaymentObservationReport::from_records(
                    candidate.context(),
                    candidate.quote(),
                    required,
                    &eligible,
                );
                Ok(!report.has_competing_quorums())
            }
            Err(FederationError::PaymentObservationQuorumNotSatisfied { .. }) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Amount credited to `quote` by internal melt settlements whose melt
    /// applied before `before_operation_index`.
    ///
    /// A melt paying a request of this federation settles inside the accepted
    /// melt's application: the mint quote is credited with the melt quote id
    /// as payment id, and no payment backend sees the payment, so no payment
    /// observation exists for it. The accepted, applied melt is the consensus
    /// evidence instead. A payment counts only when its melt quote is paid
    /// for exactly this quote's request, unit and amount, and that melt
    /// applied before the operation being checked, so each settlement counts
    /// once and every member and replay sees the same amount.
    pub(super) async fn federation_internal_settlement_amount_before(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        quote: &MintQuote,
        before_operation_index: u64,
    ) -> Result<Amount<CurrencyUnit>, Error> {
        self.federation_internal_settlement_amount_before_with_history_start(
            tx,
            quote,
            before_operation_index,
            None,
        )
        .await
    }

    async fn federation_internal_settlement_amount_before_with_history_start(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        quote: &MintQuote,
        before_operation_index: u64,
        preloaded_history_start: Option<Option<u64>>,
    ) -> Result<Amount<CurrencyUnit>, Error> {
        let mut total = Amount::new(0, quote.unit.clone());
        let mut counted = HashSet::new();
        for payment in &quote.payments {
            let Ok(melt_quote_id) = QuoteId::from_str(&payment.payment_id) else {
                continue;
            };
            if !counted.insert(melt_quote_id.clone()) {
                continue;
            }
            let Some(melt_quote) = tx.get_melt_quote(&melt_quote_id).await? else {
                continue;
            };
            if melt_quote.state != cdk_common::MeltQuoteState::Paid
                || melt_quote.unit != quote.unit
                || payment.amount.unit() != &quote.unit
                || melt_quote.request.to_string() != quote.request
                || melt_quote.amount().value() != payment.amount.value()
            {
                continue;
            }
            if !self
                .federation_melt_applied_before_with_history_start(
                    tx,
                    &melt_quote_id,
                    before_operation_index,
                    preloaded_history_start,
                )
                .await?
            {
                continue;
            }
            total = total
                .checked_add(&payment.amount)
                .map_err(|_| Error::AmountOverflow)?;
        }
        Ok(total)
    }

    /// Whether the accepted melt of `melt_quote` applied before
    /// `before_operation_index`.
    async fn federation_melt_applied_before_with_history_start(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        melt_quote: &QuoteId,
        before_operation_index: u64,
        preloaded_history_start: Option<Option<u64>>,
    ) -> Result<bool, Error> {
        let key = FederationOperationConflictKey::Melt {
            quote: melt_quote.to_string(),
        };
        let mut before = before_operation_index;
        // A conflicting melt on the same quote is ordered but rejected; look
        // past it for the applied one.
        while let Some(entry) = tx
            .get_federation_journal_entry(
                FederationJournalEntryKey::ConflictKey(&key),
                Some(before),
            )
            .await?
        {
            if entry.status() == FederationOperationStatus::Applied
                && matches!(entry.envelope.operation, FederationOperation::Melt(_))
            {
                return Ok(true);
            }
            if entry.index >= before {
                return Err(Error::Internal);
            }
            before = entry.index;
        }
        // No retained melt is ordered before the operation. A melt applied
        // later does not count. Otherwise the melt left the retained history
        // (a state transfer base or the retention floor), which lies at or
        // before the operation being checked.
        if tx
            .get_federation_journal_entry(FederationJournalEntryKey::ConflictKey(&key), None)
            .await?
            .is_some_and(|entry| entry.status() == FederationOperationStatus::Applied)
        {
            return Ok(false);
        }
        let history_start = match preloaded_history_start {
            Some(history_start) => history_start,
            None => self.federation_history_start().await?,
        };
        Ok(history_start.is_some_and(|history_start| history_start <= before_operation_index))
    }

    /// Whether consensus evidence covers `required_amount` paid to `quote`
    /// before the operation at `before_operation_index`.
    ///
    /// Two kinds of evidence exist: a payment observation quorum for payments
    /// a backend saw, and applied internal settlements. On-chain observations
    /// report only the accepted chain outputs, so internal settlements add to
    /// them. Other methods' observations report the quote's cumulative paid
    /// amount, which may already include an internal settlement, so the two
    /// are alternatives there and never add up.
    async fn federation_mint_payment_evidence_covers_amount(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        quote: &MintQuote,
        required_amount: &Amount<CurrencyUnit>,
        observations: &[FederationPaymentObservationRecord],
        before_operation_index: u64,
    ) -> Result<bool, Error> {
        self.federation_mint_payment_evidence_covers_amount_with_history_start(
            tx,
            quote,
            required_amount,
            observations,
            before_operation_index,
            None,
        )
        .await
    }

    async fn federation_mint_payment_evidence_covers_amount_with_history_start(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        quote: &MintQuote,
        required_amount: &Amount<CurrencyUnit>,
        observations: &[FederationPaymentObservationRecord],
        before_operation_index: u64,
        history_start: Option<u64>,
    ) -> Result<bool, Error> {
        let internal = self
            .federation_internal_settlement_amount_before_with_history_start(
                tx,
                quote,
                before_operation_index,
                Some(history_start),
            )
            .await?;
        if &internal >= required_amount {
            return Ok(true);
        }
        let observed_required = if quote.payment_method.is_onchain() {
            required_amount
                .checked_sub(&internal)
                .map_err(|_| Error::AmountOverflow)?
        } else {
            required_amount.clone()
        };
        self.federation_mint_payment_quorum_covers_amount_at_or_before(
            quote,
            &observed_required,
            observations,
            before_operation_index.saturating_sub(1),
        )
    }

    fn federation_mint_payment_quorum_covers_amount_at_or_before(
        &self,
        quote: &MintQuote,
        required_amount: &Amount<CurrencyUnit>,
        observations: &[FederationPaymentObservationRecord],
        max_operation_index: u64,
    ) -> Result<bool, Error> {
        for record in observations
            .iter()
            .filter(|record| record.operation_index <= max_operation_index)
        {
            let FederationPaymentObservation::MintQuote(observation) = &record.observation else {
                continue;
            };
            if observation.quote != quote.id.to_string()
                || observation.method != quote.payment_method
                || observation.state != MintQuoteState::Paid
                || observation.amount_paid < required_amount.clone().into()
            {
                continue;
            }
            if self.federation_payment_observation_can_finalize_at_or_before(
                &record.observation,
                observations,
                max_operation_index,
            )? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn apply_federation_mint_quote(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationMintQuoteOperation,
    ) -> Result<(), Error> {
        self.validate_federation_onchain_mint_quote_allocation(operation)
            .await?;
        let proposed_quote = mint_quote_from_federation_operation(operation)?;
        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            let mut quote = match tx.get_mint_quote(&proposed_quote.id).await? {
                Some(existing) => {
                    ensure_mint_quote_matches(&existing, &proposed_quote)?;
                    existing
                }
                None => {
                    tx.add_mint_quote(proposed_quote.clone()).await?;
                    database::mint::Acquired::from(proposed_quote)
                }
            };

            let observations = tx
                .get_federation_payment_observations(FederationPaymentObservationSelection::Live {
                    context: FederationPaymentObservationContext::MintQuote,
                    quote: &operation.quote,
                })
                .await?
                .into_iter()
                .map(|observation| observation.record)
                .collect::<Vec<_>>();
            let paid_notification = self
                .apply_mint_payment_observations_to_quote(&mut tx, &mut quote, &observations)
                .await?
                .map(|amount_paid| ((*quote).clone(), amount_paid));

            Ok(((*quote).clone(), paid_notification))
        }
        .await;
        let (published_quote, paid_notification) =
            finish_federation_apply_transaction(tx, entry, outcome).await?;

        self.publish_federation_mint_quote_created(&published_quote)?;
        if let Some((quote, amount_paid)) = paid_notification {
            self.pubsub_manager.mint_quote_payment(&quote, amount_paid);
        }

        Ok(())
    }

    async fn validate_federation_onchain_mint_quote_allocation_in_tx(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        operation: &FederationMintQuoteOperation,
    ) -> Result<(), Error> {
        let Some(runtime) = self.federation_config() else {
            return Ok(());
        };
        if runtime.public_config().frost_wallet.is_none()
            || !matches!(operation.request, MintQuoteRequest::Onchain(_))
        {
            return Ok(());
        }
        let (MintQuoteRequest::Onchain(request), MintQuoteResponse::Onchain(response)) =
            (&operation.request, &operation.response)
        else {
            return Err(Error::InvalidPaymentMethod);
        };
        let quote_id = quote_id_from_federation(&operation.quote)?;
        if operation.request_lookup_id != PaymentIdentifier::QuoteId(quote_id)
            || response.unit != request.unit
            || response.pubkey != request.pubkey
            || response.expiry.is_some()
            || response.amount_paid != Amount::ZERO
            || response.amount_issued != Amount::ZERO
        {
            return Err(FederationError::OperationStateInvalid {
                reason: "federated on-chain mint quote response does not match its request",
            }
            .into());
        }

        let application = runtime.cdk_onchain_application()?;
        let state = FederationStoredState::<FederationAddressAllocationState>::load(tx).await?;
        let allocation = state
            .allocation(
                FederationAddressAllocationScope {
                    application_id: application.application_id,
                    application_key_epoch: application.application_key_epoch,
                },
                &operation.quote,
            )
            .ok_or(FederationError::AddressAllocationInvalid {
                reason: "federated on-chain mint quote has no accepted address allocation",
            })?;
        if allocation.keychain != FederationAddressKeychain::External {
            return Err(FederationError::AddressAllocationInvalid {
                reason: "federated on-chain mint quote requires an external address allocation",
            }
            .into());
        }
        let wallet = runtime
            .public_config()
            .frost_wallet
            .as_ref()
            .ok_or(Error::Internal)?;
        let script = wallet.derive_cdk_onchain_script(
            runtime.public_config(),
            application.application_id,
            application.application_key_epoch,
            allocation.keychain,
            allocation.derivation_index,
        )?;
        let expected_address =
            cdk_common::bitcoin::Address::from_script(script.as_script(), application.network)
                .map_err(|_| FederationError::OperationStateInvalid {
                    reason: "federated on-chain mint quote address is not representable",
                })?;
        if response.request != expected_address.to_string() {
            return Err(FederationError::OperationStateInvalid {
                reason: "federated on-chain mint quote address does not match the derived address",
            }
            .into());
        }
        Ok(())
    }

    async fn validate_federation_onchain_mint_quote_allocation(
        &self,
        operation: &FederationMintQuoteOperation,
    ) -> Result<(), Error> {
        let Some(runtime) = self.federation_config() else {
            return Ok(());
        };
        if runtime.public_config().frost_wallet.is_none()
            || !matches!(operation.request, MintQuoteRequest::Onchain(_))
        {
            return Ok(());
        }
        let (MintQuoteRequest::Onchain(request), MintQuoteResponse::Onchain(response)) =
            (&operation.request, &operation.response)
        else {
            return Err(Error::InvalidPaymentMethod);
        };
        let quote_id = quote_id_from_federation(&operation.quote)?;
        if operation.request_lookup_id != PaymentIdentifier::QuoteId(quote_id)
            || response.unit != request.unit
            || response.pubkey != request.pubkey
            || response.expiry.is_some()
            || response.amount_paid != Amount::ZERO
            || response.amount_issued != Amount::ZERO
        {
            return Err(FederationError::OperationStateInvalid {
                reason: "federated on-chain mint quote response does not match its request",
            }
            .into());
        }

        let application = runtime.cdk_onchain_application()?;
        let allocation = self
            .federation_address_allocation(
                FederationAddressAllocationScope {
                    application_id: application.application_id,
                    application_key_epoch: application.application_key_epoch,
                },
                &operation.quote,
            )
            .await?
            .ok_or(FederationError::AddressAllocationInvalid {
                reason: "federated on-chain mint quote has no accepted address allocation",
            })?;
        if allocation.keychain != FederationAddressKeychain::External {
            return Err(FederationError::AddressAllocationInvalid {
                reason: "federated on-chain mint quote requires an external address allocation",
            }
            .into());
        }
        let wallet = runtime
            .public_config()
            .frost_wallet
            .as_ref()
            .ok_or(Error::Internal)?;
        let script = wallet.derive_cdk_onchain_script(
            runtime.public_config(),
            application.application_id,
            application.application_key_epoch,
            allocation.keychain,
            allocation.derivation_index,
        )?;
        let expected_address =
            cdk_common::bitcoin::Address::from_script(script.as_script(), application.network)
                .map_err(|_| FederationError::OperationStateInvalid {
                    reason: "federated on-chain mint quote address is not representable",
                })?;
        if response.request != expected_address.to_string() {
            return Err(FederationError::OperationStateInvalid {
                reason: "federated on-chain mint quote address does not match the derived address",
            }
            .into());
        }
        Ok(())
    }

    fn publish_federation_mint_quote_created(&self, quote: &MintQuote) -> Result<(), Error> {
        match quote.payment_method {
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt11) => {
                let response: MintQuoteBolt11Response<QuoteId> = quote.clone().into();
                self.pubsub_manager
                    .publish(NotificationPayload::MintQuoteBolt11Response(response));
            }
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt12) => {
                let response: MintQuoteBolt12Response<QuoteId> = quote.clone().try_into()?;
                self.pubsub_manager
                    .publish(NotificationPayload::MintQuoteBolt12Response(response));
            }
            cdk_common::PaymentMethod::Custom(ref method) => {
                if let Ok(response) = MintQuoteCustomResponse::try_from(quote.clone()) {
                    self.pubsub_manager
                        .publish(NotificationPayload::CustomMintQuoteResponse(
                            method.clone(),
                            response,
                        ));
                }
            }
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Onchain) => {
                let response: MintQuoteOnchainResponse<QuoteId> = quote.clone().try_into()?;
                self.pubsub_manager
                    .publish(NotificationPayload::MintQuoteOnchainResponse(response));
            }
        }

        Ok(())
    }

    async fn apply_federation_keyset_rotation(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationKeysetRotationOperation,
    ) -> Result<(), Error> {
        self.effective_federation_config()
            .await?
            .ok_or(Error::Internal)?
            .apply_keyset_rotation(operation)?;

        let transaction = self.localstore.begin_transaction().await?;
        finish_federation_apply_transaction(transaction, entry, Ok(())).await?;
        self.rebuild_effective_federation_config(true).await?;
        Ok(())
    }

    async fn apply_federation_mint_quote_payment(
        &self,
        entry: &FederationJournalEntry,
        observation: &FederationMintQuotePaymentObservation,
    ) -> Result<(), Error> {
        let quote_id = quote_id_from_federation(&observation.quote)?;
        self.require_prior_applied_mint_quote(&quote_id, entry.index)
            .await?;
        let record = payment_observation_record_from_entry(entry)?;
        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            let mut quote = tx.get_mint_quote(&quote_id).await?;
            if quote.is_none()
                && observation.method.is_onchain()
                && self
                    .federation_config()
                    .is_some_and(|runtime| runtime.public_config().frost_wallet.is_some())
            {
                return Err(Error::UnknownQuote);
            }
            if let Some(quote) = quote.as_deref() {
                self.validate_federation_onchain_payment_observation(
                    &mut tx,
                    quote,
                    observation,
                    record.operation_index,
                    false,
                )
                .await?;
            }
            if quote.as_deref().is_some_and(mint_quote_payments_are_final) {
                return Ok(None);
            }
            tx.add_federation_payment_observation(&record, None).await?;
            let observations = tx
                .get_federation_payment_observations(FederationPaymentObservationSelection::Live {
                    context: FederationPaymentObservationContext::MintQuote,
                    quote: &observation.quote,
                })
                .await?
                .into_iter()
                .map(|observation| observation.record)
                .collect::<Vec<_>>();
            let mut paid_notification = None;
            if let Some(mut quote) = quote.take() {
                if let Some(amount_paid) = self
                    .apply_mint_payment_observations_to_quote(&mut tx, &mut quote, &observations)
                    .await?
                {
                    paid_notification = Some(((*quote).clone(), amount_paid));
                }
                settle_paid_mint_quote_application(&mut tx, &quote, entry.index).await?;
            }
            Ok(paid_notification)
        }
        .await;
        let paid_notification = finish_federation_apply_transaction(tx, entry, outcome).await?;

        if let Some((quote, amount_paid)) = paid_notification {
            self.pubsub_manager.mint_quote_payment(&quote, amount_paid);
        }

        Ok(())
    }

    pub(super) async fn apply_mint_payment_observations_to_quote(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        quote: &mut database::mint::Acquired<MintQuote>,
        observations: &[FederationPaymentObservationRecord],
    ) -> Result<Option<Amount<CurrencyUnit>>, Error> {
        self.apply_mint_payment_observations_to_quote_with_history_start(
            tx,
            quote,
            observations,
            None,
        )
        .await
    }

    async fn apply_mint_payment_observations_to_quote_with_history_start(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        quote: &mut database::mint::Acquired<MintQuote>,
        observations: &[FederationPaymentObservationRecord],
        preloaded_history_start: Option<u64>,
    ) -> Result<Option<Amount<CurrencyUnit>>, Error> {
        let mut paid_notification = None;
        // On-chain observations report only accepted chain outputs. An
        // internal settlement credits the quote apart from them, so it is
        // left out when comparing; other methods report the quote's
        // cumulative paid amount.
        let internally_settled = if quote.payment_method.is_onchain() {
            self.federation_internal_settlement_amount_before_with_history_start(
                tx,
                quote,
                u64::MAX,
                Some(preloaded_history_start),
            )
            .await?
        } else {
            Amount::new(0, quote.unit.clone())
        };

        for record in observations {
            let FederationPaymentObservation::MintQuote(observation) = &record.observation else {
                continue;
            };
            if quote.payment_method != observation.method {
                return Err(Error::InvalidPaymentMethod);
            }

            let can_finalize = self.federation_payment_observation_can_finalize_at_or_before(
                &record.observation,
                observations,
                record.operation_index,
            )?;
            let observed_paid = observation
                .amount_paid
                .with_unit(quote.unit.clone())
                .checked_add(&internally_settled)
                .map_err(|_| Error::AmountOverflow)?;
            if can_finalize && observed_paid > quote.amount_paid() {
                self.validate_federation_onchain_payment_observation(
                    tx,
                    quote,
                    observation,
                    record.operation_index,
                    true,
                )
                .await?;
                let additional_amount = observed_paid
                    .checked_sub(&quote.amount_paid())
                    .map_err(|_| Error::AmountOverflow)?;
                let payment_id = observation
                    .payment_id
                    .as_ref()
                    .map(federation_payment_identifier_label)
                    .or_else(|| observation.payment_proof.clone())
                    .unwrap_or_else(|| {
                        format!(
                            "federation:{}:{}",
                            record.operation_id,
                            mint_quote_state_label(observation.state)
                        )
                    });
                // Payment observations currently carry no wall-clock evidence.
                // Preserve deterministic replay without storing journal order
                // in the Unix timestamp field.
                quote.add_payment(additional_amount, payment_id, Some(0))?;
                tx.update_mint_quote(quote).await?;
                paid_notification = Some(quote.amount_paid());
            }
        }

        Ok(paid_notification)
    }

    async fn validate_federation_onchain_payment_observation(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        quote: &MintQuote,
        observation: &FederationMintQuotePaymentObservation,
        max_operation_index: u64,
        mark_liability: bool,
    ) -> Result<(), Error> {
        let Some(runtime) = self.federation_config() else {
            return Ok(());
        };
        if runtime.public_config().frost_wallet.is_none() || !quote.payment_method.is_onchain() {
            return Ok(());
        }
        if quote.unit != CurrencyUnit::Sat
            || observation.method != quote.payment_method
            || observation.state != MintQuoteState::Paid
            || observation.payment_proof.is_some()
        {
            return Err(Error::InvalidPaymentRequest);
        }
        let mut state = cdk_common::federation::FederationStoredState::<
            cdk_common::federation::FederationChainObservationState,
        >::load(tx)
        .await?;
        let payment = state
            .quote_payment_at_or_before(&observation.quote, max_operation_index)?
            .ok_or(Error::UnpaidQuote)?;
        if u64::from(observation.amount_paid) != payment.amount_sat
            || observation.payment_id != Some(PaymentIdentifier::PaymentId(payment.payment_id))
        {
            return Err(FederationError::ChainObservationStateInvalid {
                reason: "on-chain payment observation does not match accepted wallet outputs",
            }
            .into());
        }
        if mark_liability {
            state.mark_quote_liability_issued_at_or_before(
                &observation.quote,
                max_operation_index,
                payment.amount_sat,
                payment.payment_id,
            )?;
            state.save(tx).await?;
        }
        Ok(())
    }

    async fn apply_federation_mint(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationMintOperation,
    ) -> Result<(), Error> {
        let operation_id = entry.operation_id;
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let input = mint_input_from_federation_request(&operation.request)?;
        input.validate()?;
        let output_verification =
            verification::verify_outputs(&config.keysets(), self.max_outputs, input.outputs())?;
        let outputs_amount = output_verification.amount;
        let blinded_secrets = input
            .outputs()
            .iter()
            .map(|output| output.blinded_secret)
            .collect::<Vec<PublicKey>>();
        let quote_ids = input.quote_ids();
        self.require_prior_applied_mint_quotes(&quote_ids, entry.index)
            .await?;

        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            let mut quotes = Vec::with_capacity(quote_ids.len());
            for quote_id in &quote_ids {
                quotes.push(
                    tx.get_mint_quote(quote_id)
                        .await?
                        .ok_or(Error::UnknownQuote)?,
                );
            }

            let (payment_method, unit) =
                crate::mint::issue::uniform_method_and_unit(quotes.iter().map(|quote| &**quote))?;
            if outputs_amount.unit() != &unit {
                return Err(Error::UnsupportedUnit);
            }
            let expected_amounts =
                expected_federation_mint_amounts(&input, &quotes, outputs_amount.clone())?;
            let total_expected = expected_amounts.iter().try_fold(0_u64, |total, amount| {
                total
                    .checked_add(amount.value())
                    .ok_or(Error::AmountOverflow)
            })?;
            if outputs_amount.value() != total_expected {
                return Err(Error::TransactionUnbalanced(
                    total_expected,
                    outputs_amount.value(),
                    0,
                ));
            }

            input.verify_quote_signatures(quotes.iter().map(|quote| &**quote))?;

            let existing_signatures = tx.get_blind_signatures(&blinded_secrets).await?;
            let signatures_present = existing_signatures
                .iter()
                .filter(|sig| sig.is_some())
                .count();
            if signatures_present != 0 && signatures_present != existing_signatures.len() {
                return Err(Error::Custom(
                    "federation mint replay found partially persisted signatures".to_string(),
                ));
            }
            validate_federation_mint_quote_states_for_signing(
                &quotes,
                &expected_amounts,
                signatures_present,
                existing_signatures.len(),
            )?;

            for (quote, amount_issued) in quotes.iter().zip(&expected_amounts) {
                let required_amount = quote
                    .amount_issued()
                    .checked_add(amount_issued)
                    .map_err(|_| Error::AmountOverflow)?;
                let observations = tx
                    .get_federation_payment_observations(
                        FederationPaymentObservationSelection::Live {
                            context: FederationPaymentObservationContext::MintQuote,
                            quote: &quote.id.to_string(),
                        },
                    )
                    .await?
                    .into_iter()
                    .map(|observation| observation.record)
                    .collect::<Vec<_>>();
                if !self
                    .federation_mint_payment_evidence_covers_amount(
                        &mut tx,
                        quote,
                        &required_amount,
                        &observations,
                        entry.index,
                    )
                    .await?
                {
                    return Err(Error::UnpaidQuote);
                }
            }

            // Consensus orders completion but does not attest its wall-clock time.
            let completed_at = Some(0);
            let operation_record = if input.is_batch() {
                Operation::new(
                    federation_operation_uuid(operation_id),
                    OperationKind::BatchMint,
                    outputs_amount.clone().into(),
                    Amount::ZERO,
                    Amount::ZERO,
                    completed_at,
                    Some(payment_method.clone()),
                )
            } else {
                Operation::new(
                    federation_operation_uuid(operation_id),
                    OperationKind::Mint,
                    outputs_amount.clone().into(),
                    Amount::ZERO,
                    Amount::ZERO,
                    completed_at,
                    Some(payment_method.clone()),
                )
            };

            let share_set = config.mint_signature_shares(entry, &input)?;
            let signature_share_records =
                FederationSignatureShareRecord::from_accepted_journal_entry(&share_set, entry)?;
            tx.add_federation_signature_shares(&signature_share_records)
                .await?;
            let mut issued_notifications = Vec::new();

            if signatures_present == 0 {
                let signatures = federation_share_signatures(share_set);
                let quote_id = match input.is_batch() {
                    true => None,
                    false => quote_ids.first().cloned(),
                };
                tx.add_blinded_messages(quote_id.as_ref(), input.outputs(), &operation_record)
                    .await?;
                tx.add_blind_signatures(&blinded_secrets, &signatures, quote_id)
                    .await?;
                let fee_by_keyset = HashMap::new();
                tx.add_completed_operation(&operation_record, &fee_by_keyset)
                    .await?;
            }

            for (quote, amount_issued) in quotes.iter_mut().zip(expected_amounts) {
                match quote.state() {
                    MintQuoteState::Paid => {
                        quote.add_issuance(amount_issued)?;
                        tx.update_mint_quote(quote).await?;
                        issued_notifications.push((quote.clone(), quote.amount_issued()));
                    }
                    MintQuoteState::Issued => {
                        if quote.amount_issued() < amount_issued {
                            return Err(Error::IssuedQuote);
                        }
                    }
                    MintQuoteState::Unpaid => {
                        return Err(Error::UnpaidQuote);
                    }
                }
                retire_final_mint_quote_observations(&mut tx, quote, entry.index).await?;
            }

            #[cfg(test)]
            if crate::test_helpers::mint::should_fail_for("FEDERATION_MINT_APPLY_BEFORE_STATUS") {
                return Err(Error::Internal);
            }

            Ok((issued_notifications, signature_share_records))
        }
        .await;
        let (issued_notifications, signature_share_records) =
            finish_federation_apply_transaction(tx, entry, outcome).await?;

        cache_committed_signature_shares(&self.localstore, operation_id, signature_share_records);
        for (quote, amount_issued) in issued_notifications {
            self.pubsub_manager.mint_quote_issue(&quote, amount_issued);
        }

        Ok(())
    }

    async fn apply_federation_swap(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationSwapOperation,
    ) -> Result<(), Error> {
        let operation_id = entry.operation_id;
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let input_proofs = operation.request.inputs();
        if input_proofs.is_empty() {
            return Err(Error::TransactionUnbalanced(
                0,
                operation.request.output_amount()?.to_u64(),
                0,
            ));
        }

        let input_verification = self.verify_federation_inputs_with_config_and_context(
            &config,
            operation_id,
            input_proofs,
            FederationProofVerificationContext::SwapApply,
        )?;
        operation.request.verify_spending_conditions()?;
        // Consensus orders completion but does not attest its wall-clock time.
        let SwapPlan {
            fee_breakdown,
            operation: operation_record,
        } = plan_swap(
            &config.keysets(),
            self.max_outputs,
            input_verification,
            input_proofs,
            operation.request.outputs(),
            federation_operation_uuid(operation_id),
            Some(0),
        )?;
        let input_ys = input_proofs.ys()?;
        let blinded_secrets = operation
            .request
            .outputs()
            .iter()
            .map(|output| output.blinded_secret)
            .collect::<Vec<PublicKey>>();
        let completed_swap = self
            .localstore
            .get_completed_operation(operation_record.id())
            .await?
            .is_some_and(|operation| operation.kind() == OperationKind::Swap);

        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            let mut proofs = match tx
                .add_proofs(input_proofs.clone(), None, &operation_record)
                .await
            {
                Ok(proofs) => proofs,
                Err(database::Error::Duplicate | database::Error::AttemptUpdateSpentProof) => {
                    tx.get_proofs(&input_ys).await?
                }
                Err(err) => return Err(err.into()),
            };

            let existing_signatures = tx.get_blind_signatures(&blinded_secrets).await?;
            let signatures_present = existing_signatures
                .iter()
                .filter(|sig| sig.is_some())
                .count();

            match proofs.state {
                State::Unspent if signatures_present == 0 => {}
                State::Unspent if signatures_present == existing_signatures.len() => {
                    return Err(Error::BlindedMessageAlreadySigned)
                }
                State::Unspent => {
                    return Err(Error::Custom(
                        "federation swap found partially persisted signatures for fresh inputs"
                            .to_string(),
                    ))
                }
                State::Spent
                    if signatures_present == existing_signatures.len()
                        && signatures_present > 0
                        && completed_swap =>
                {
                    let share_set = config.swap_signature_shares(entry, &operation.request)?;
                    let signature_share_records =
                        FederationSignatureShareRecord::from_accepted_journal_entry(
                            &share_set, entry,
                        )?;
                    tx.add_federation_signature_shares(&signature_share_records)
                        .await?;
                    return Ok((false, signature_share_records));
                }
                State::Spent => {
                    return Err(Error::TokenAlreadySpent);
                }
                State::Pending | State::Reserved | State::PendingSpent => {
                    return Err(Error::TokenPending);
                }
            }

            Mint::update_proofs_state(&mut tx, &mut proofs, State::Spent).await?;

            let mut signature_share_records = Vec::new();
            if signatures_present == 0 {
                let share_set = config.swap_signature_shares(entry, &operation.request)?;
                signature_share_records =
                    FederationSignatureShareRecord::from_accepted_journal_entry(&share_set, entry)?;
                let signatures = federation_share_signatures(share_set);
                tx.add_federation_signature_shares(&signature_share_records)
                    .await?;
                tx.add_blinded_messages(None, operation.request.outputs(), &operation_record)
                    .await?;
                tx.add_blind_signatures(&blinded_secrets, &signatures, None)
                    .await?;
                tx.add_completed_operation(&operation_record, &fee_breakdown.per_keyset)
                    .await?;
            }

            #[cfg(test)]
            if crate::test_helpers::mint::should_fail_for("FEDERATION_SWAP_APPLY_BEFORE_STATUS") {
                return Err(Error::Internal);
            }

            Ok((true, signature_share_records))
        }
        .await;
        let (notify_spent, signature_share_records) =
            finish_federation_apply_transaction(tx, entry, outcome).await?;

        cache_committed_signature_shares(&self.localstore, operation_id, signature_share_records);
        if notify_spent {
            for y in input_ys {
                self.pubsub_manager.proof_state((y, State::Spent));
            }
        }

        Ok(())
    }

    async fn apply_federation_melt_quote(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationMeltQuoteOperation,
    ) -> Result<(), Error> {
        let quote_id = quote_id_from_federation(&operation.quote)?;
        let quote = operation.into_melt_quote(quote_id)?;
        ensure_melt_quote_matches_request(&quote, &operation.request)?;

        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            if let Some(existing) = tx.get_melt_quote(&quote.id).await? {
                ensure_melt_quote_matches_request(&existing, &operation.request)?;
                if !federation_melt_quote_immutable_terms_match(&existing, &quote) {
                    return Err(Error::InvalidPaymentRequest);
                }
            } else {
                tx.add_melt_quote(quote).await?;
            }
            Ok(())
        }
        .await;
        finish_federation_apply_transaction(tx, entry, outcome).await
    }

    async fn apply_federation_melt_quote_payment(
        &self,
        entry: &FederationJournalEntry,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<(), Error> {
        let record = payment_observation_record_from_entry(entry)?;
        self.apply_federation_melt_quote_payment_record(entry, observation, &record)
            .await
    }

    /// Apply one melt payment observation in one transaction.
    ///
    /// The observation, the melt outcome it triggers (finalization, rollback
    /// or quote state update) and the applied mark commit together.
    /// Notifications are published only after the commit. Any error rolls
    /// back everything, including the observation row, and leaves the entry
    /// unapplied so replay retries it.
    async fn apply_federation_melt_quote_payment_record(
        &self,
        entry: &FederationJournalEntry,
        observation: &FederationMeltQuotePaymentObservation,
        record: &FederationPaymentObservationRecord,
    ) -> Result<(), Error> {
        // Rebuilding the effective config reads the database, so load it
        // before the transaction holds the connection.
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let mut transaction = self.localstore.begin_transaction().await?;
        let outcome = self
            .apply_federation_melt_quote_payment_record_in_transaction(
                &mut transaction,
                &config,
                entry,
                observation,
                record,
            )
            .await;
        let notifications =
            finish_federation_apply_transaction(transaction, entry, outcome).await?;
        notifications.publish(&self.pubsub_manager);
        Ok(())
    }

    /// Add a melt payment observation and apply its outcome inside `tx`.
    ///
    /// Takes the same branch decisions as a standalone apply would, reading
    /// the quote and the saga inside `tx`. Returns the notifications to
    /// publish after the caller commits.
    async fn apply_federation_melt_quote_payment_record_in_transaction(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        config: &FederationRuntimeConfig,
        entry: &FederationJournalEntry,
        observation: &FederationMeltQuotePaymentObservation,
        record: &FederationPaymentObservationRecord,
    ) -> Result<MeltNotifications, Error> {
        let operation_id = entry.operation_id;
        let quote_id = quote_id_from_federation(&observation.quote)?;

        // Take the quote lock before any row lock, matching finalization and
        // rollback.
        tx.lock_quotes(std::slice::from_ref(&quote_id)).await?;
        let mut quote = tx
            .get_melt_quote(&quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?
            .inner();

        // A paid quote is final: no observation can change it, so none is
        // recorded. Its earlier observations are retired and a member may
        // already have deleted them. A melt saga still finalizing after a
        // crash completes from its own recorded payment result.
        if quote.state == cdk_common::MeltQuoteState::Paid {
            let mut notifications = MeltNotifications::default();
            if let Some(saga) = tx.get_melt_saga_by_quote_id(&quote_id).await? {
                if saga.state == SagaStateEnum::Melt(MeltSagaState::Finalizing) {
                    let melt_operation_id = saga
                        .federation_operation_id
                        .as_deref()
                        .ok_or(Error::Internal)?
                        .parse::<FederationOperationId>()?;
                    notifications.extend(
                        Self::resume_paid_federation_melt_finalization_in_transaction(
                            tx,
                            config,
                            &mut quote,
                            &saga,
                            melt_operation_id,
                        )
                        .await?,
                    );
                }
            }
            return Ok(notifications);
        }

        ensure_melt_quote_matches_payment_observation(&quote, observation)?;
        let previous_state = quote.state;
        let onchain_receipt = self
            .validate_federation_onchain_melt_payment_observation_in_transaction(
                tx,
                &quote,
                observation,
            )
            .await?;

        tx.add_federation_payment_observation(record, None).await?;
        let observations = tx
            .get_federation_payment_observations(FederationPaymentObservationSelection::Live {
                context: FederationPaymentObservationContext::MeltQuote,
                quote: &observation.quote,
            })
            .await?
            .into_iter()
            .map(|observation| observation.record)
            .collect::<Vec<_>>();
        let can_finalize = self.federation_payment_observation_can_finalize_at_or_before(
            &record.observation,
            &observations,
            record.operation_index,
        )?;

        let mut notifications = MeltNotifications::default();
        if quote.state == cdk_common::MeltQuoteState::Paid
            && observation.state != cdk_common::MeltQuoteState::Paid
        {
            // A terminal paid settlement wins over later stale pending/unpaid
            // observations from other members. The evidence is still persisted
            // above so operators can audit observer lag or disagreement.
        } else if quote.state == cdk_common::MeltQuoteState::Unpaid
            && observation.state == cdk_common::MeltQuoteState::Pending
            && tx.get_melt_saga_by_quote_id(&quote_id).await?.is_none()
        {
            // Failed melt recovery rolls the quote back to unpaid and clears
            // saga state. Late pending observations from other members must not
            // resurrect a melt that no longer has a pending saga to finalize.
        } else if quote.state != observation.state {
            if can_finalize {
                match observation.state {
                    cdk_common::MeltQuoteState::Paid => {
                        notifications.extend(
                            Self::apply_paid_melt_quote_payment_observation_in_transaction(
                                tx,
                                config,
                                operation_id,
                                &mut quote,
                                observation,
                                onchain_receipt.as_ref(),
                            )
                            .await?,
                        );
                    }
                    cdk_common::MeltQuoteState::Unpaid | cdk_common::MeltQuoteState::Failed => {
                        match tx.get_melt_saga_by_quote_id(&quote_id).await? {
                            Some(saga) => {
                                let payment_response = melt_quote_payment_response(
                                    operation_id,
                                    &quote,
                                    observation,
                                    onchain_receipt.as_ref(),
                                )?;
                                notifications.extend(
                                    crate::mint::saga_recovery::process_federation_melt_saga_outcome_in_transaction(
                                        tx,
                                        config,
                                        &saga,
                                        &mut quote,
                                        &payment_response,
                                    )
                                    .await?,
                                );
                            }
                            None => {
                                Self::update_melt_quote_payment_observation_state_in_transaction(
                                    tx,
                                    &quote_id,
                                    observation,
                                )
                                .await?;
                            }
                        }
                    }
                    cdk_common::MeltQuoteState::Pending | cdk_common::MeltQuoteState::Unknown => {
                        Self::update_melt_quote_payment_observation_state_in_transaction(
                            tx,
                            &quote_id,
                            observation,
                        )
                        .await?;
                    }
                }
            }
        } else if can_finalize {
            ensure_melt_quote_payment_proof_matches(&quote, observation)?;
            if observation.state == cdk_common::MeltQuoteState::Paid {
                if let Some(saga) = tx.get_melt_saga_by_quote_id(&quote_id).await? {
                    let melt_operation_id = saga
                        .federation_operation_id
                        .as_deref()
                        .ok_or(Error::Internal)?
                        .parse::<FederationOperationId>()?;
                    notifications.extend(
                        Self::resume_paid_federation_melt_finalization_in_transaction(
                            tx,
                            config,
                            &mut quote,
                            &saga,
                            melt_operation_id,
                        )
                        .await?,
                    );
                }
            }
        }

        settle_paid_melt_quote(
            tx,
            &quote_id,
            previous_state,
            onchain_receipt.is_some(),
            entry.index,
        )
        .await?;

        #[cfg(test)]
        if crate::test_helpers::mint::should_fail_for("FEDERATION_MELT_PAYMENT_APPLY_BEFORE_COMMIT")
        {
            return Err(Error::Internal);
        }

        Ok(notifications)
    }

    async fn apply_paid_melt_quote_payment_observation_in_transaction(
        tx: &mut database::mint::DynMintTransaction,
        config: &FederationRuntimeConfig,
        operation_id: FederationOperationId,
        quote: &mut MeltQuote,
        observation: &FederationMeltQuotePaymentObservation,
        onchain_receipt: Option<&FederationOnchainMeltPaymentReceipt>,
    ) -> Result<MeltNotifications, Error> {
        let saga = tx.get_melt_saga_by_quote_id(&quote.id).await?.ok_or(
            FederationError::OperationStateInvalid {
                reason: "federation paid melt observation found no pending melt saga",
            },
        )?;
        let payment_response =
            melt_quote_payment_response(operation_id, quote, observation, onchain_receipt)?;
        crate::mint::saga_recovery::process_federation_melt_saga_outcome_in_transaction(
            tx,
            config,
            &saga,
            quote,
            &payment_response,
        )
        .await
    }

    /// Validate an on-chain melt payment observation outside any transaction.
    #[cfg(test)]
    pub(super) async fn validate_federation_onchain_melt_payment_observation(
        &self,
        quote: &MeltQuote,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<Option<FederationOnchainMeltPaymentReceipt>, Error> {
        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = self
            .validate_federation_onchain_melt_payment_observation_in_transaction(
                &mut tx,
                quote,
                observation,
            )
            .await;
        tx.rollback().await?;
        outcome
    }

    /// Validate an on-chain melt payment observation against the ordered
    /// transaction state and the melt saga, both read inside `tx`.
    async fn validate_federation_onchain_melt_payment_observation_in_transaction(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        quote: &MeltQuote,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<Option<FederationOnchainMeltPaymentReceipt>, Error> {
        let Some(runtime) = self.federation_config() else {
            return Ok(None);
        };
        if runtime.public_config().frost_wallet.is_none() || !quote.payment_method.is_onchain() {
            return Ok(None);
        }
        if quote.unit != CurrencyUnit::Sat
            || observation.state != cdk_common::MeltQuoteState::Paid
            || observation.payment_id != Some(PaymentIdentifier::QuoteId(quote.id.clone()))
        {
            return Err(FederationError::OperationStateInvalid {
                reason: "federation on-chain melt observation does not match the quote identity",
            }
            .into());
        }
        let receipt = FederationStoredState::<FederationTransactionState>::load(tx)
            .await?
            .completed_payment_for_quote(&quote.id.to_string())?
            .ok_or(FederationError::OperationStateInvalid {
                reason:
                    "federation on-chain melt observation has no completed payment in ordered state",
            })?;
        let melt_operation_id = match tx.get_melt_saga_by_quote_id(&quote.id).await? {
            Some(saga) => saga
                .federation_operation_id
                .as_deref()
                .ok_or(Error::Internal)?
                .parse::<FederationOperationId>()?,
            None if quote.state == cdk_common::MeltQuoteState::Paid => receipt.melt_operation_id,
            None => {
                return Err(FederationError::OperationStateInvalid {
                    reason: "federation on-chain melt observation has no accepted melt operation",
                }
                .into());
            }
        };
        validate_federation_onchain_melt_receipt(quote, melt_operation_id, &receipt)?;
        let expected_total_spent = Amount::from(
            receipt
                .amount_sat
                .checked_add(receipt.fee_sat)
                .ok_or(Error::AmountOverflow)?,
        );
        if observation
            .total_spent
            .is_some_and(|total_spent| total_spent != expected_total_spent)
        {
            return Err(FederationError::OperationStateInvalid {
                reason:
                    "federation on-chain melt observation total_spent does not match the receipt",
            }
            .into());
        }
        let expected_proof = receipt.outpoint.to_string();
        if observation.payment_proof.as_deref() != Some(expected_proof.as_str()) {
            return Err(FederationError::OperationStateInvalid {
                reason:
                    "federation on-chain melt observation proof does not match the payment outpoint",
            }
            .into());
        }
        Ok(Some(receipt))
    }

    async fn update_melt_quote_payment_observation_state_in_transaction(
        tx: &mut database::mint::DynMintTransaction,
        quote_id: &QuoteId,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<(), Error> {
        if observation.state == cdk_common::MeltQuoteState::Unknown {
            tracing::warn!(
                "Federated melt quote {} observed unknown backend payment status; leaving durable quote state unchanged",
                quote_id
            );
            return Ok(());
        }

        let mut quote = tx
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;
        ensure_melt_quote_matches_payment_observation(&quote, observation)?;
        if quote.state != observation.state {
            tx.update_melt_quote_state(
                &mut quote,
                observation.state,
                observation.payment_proof.clone(),
            )
            .await?;
        } else {
            ensure_melt_quote_payment_proof_matches(&quote, observation)?;
        }
        Ok(())
    }

    async fn apply_federation_melt(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationMeltOperation,
    ) -> Result<(), Error> {
        let operation_id = entry.operation_id;
        let request = MeltRequest::<QuoteId>::try_from(operation.request.clone())
            .map_err(|_| Error::UnknownQuote)?;
        self.require_prior_applied_melt_quote(request.quote(), entry.index)
            .await?;
        let melt_remains_pending = match self.melt_federation(&request, operation_id).await {
            Ok(response) => match response.state() {
                cdk_common::MeltQuoteState::Pending => true,
                cdk_common::MeltQuoteState::Paid => false,
                cdk_common::MeltQuoteState::Unpaid
                | cdk_common::MeltQuoteState::Failed
                | cdk_common::MeltQuoteState::Unknown => return Err(Error::Internal),
            },
            Err(Error::PaymentFailed) => true,
            Err(err) => return Err(err),
        };

        let mut transaction = self.localstore.begin_transaction().await?;
        let outcome = async {
            let quote = transaction
                .get_melt_quote(request.quote())
                .await?
                .ok_or(Error::UnknownQuote)?;
            if melt_remains_pending {
                let runtime = self.federation_config().ok_or(Error::Internal)?;
                persist_pending_onchain_intent(
                    &mut transaction,
                    runtime,
                    &quote,
                    operation_id,
                    federation_accepted_order(entry),
                )
                .await
            } else if quote.state == cdk_common::MeltQuoteState::Paid {
                Ok(())
            } else {
                Err(Error::Internal)
            }
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }

    pub(in crate::mint) async fn verify_federation_inputs_with_context(
        &self,
        operation_id: FederationOperationId,
        inputs: &Proofs,
        context: FederationProofVerificationContext,
    ) -> Result<Verification, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        self.verify_federation_inputs_with_config_and_context(
            &config,
            operation_id,
            inputs,
            context,
        )
    }

    #[cfg(test)]
    pub(crate) fn verify_federation_inputs_with_config(
        &self,
        config: &FederationRuntimeConfig,
        inputs: &Proofs,
    ) -> Result<Verification, Error> {
        self.verify_federation_inputs_with_config_inner(config, None, inputs, None)
    }

    fn verify_federation_inputs_with_config_and_context(
        &self,
        config: &FederationRuntimeConfig,
        operation_id: FederationOperationId,
        inputs: &Proofs,
        context: FederationProofVerificationContext,
    ) -> Result<Verification, Error> {
        self.verify_federation_inputs_with_config_inner(
            config,
            Some(operation_id),
            inputs,
            Some(context),
        )
    }

    fn verify_federation_inputs_with_config_inner(
        &self,
        config: &FederationRuntimeConfig,
        operation_id: Option<FederationOperationId>,
        inputs: &Proofs,
        context: Option<FederationProofVerificationContext>,
    ) -> Result<Verification, Error> {
        verification::verify_inputs_bounds(self.max_inputs, inputs)?;

        match (operation_id, context) {
            (Some(operation_id), Some(context)) => config
                .verify_proofs_for_operation_with_performance_context(
                    operation_id,
                    inputs,
                    context,
                )?,
            (Some(operation_id), None) => {
                config.verify_proofs_for_operation(operation_id, inputs)?
            }
            (None, Some(context)) => {
                config.verify_proofs_with_performance_context(inputs, context)?
            }
            (None, None) => config.verify_proofs(inputs)?,
        }

        verification::verify_inputs_amount(&config.keysets(), inputs)
    }
}
