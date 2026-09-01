//! Portable application-facing wallet API.
//!
//! Rust applications and generated UniFFI bindings use these same objects.
//! Protocol-level proof, keyset, swap, authentication, and database APIs remain
//! available from `cdk` for applications that deliberately need the engine.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use cdk::nuts::nut00::ProofsMethods;

use crate::database::WalletStore;
use crate::error::FfiError;
use crate::token::Token;
use crate::types::{
    ActiveSubscription, Amount, CurrencyUnit, MintUrl, PaymentMethod, PaymentPlan, PendingPayment,
    QuoteState, Restored, SendKind, SendPlan, SubscribeParams, Transaction, TransactionDirection,
};
use crate::wallet::{RateLimit, Wallet, WalletConfig};
use crate::wallet_repository::{WalletRepository, WalletRepositoryConfig};

/// Configuration for opening one mint-and-unit wallet.
#[derive(Clone, uniffi::Record)]
pub struct WalletOpenRequest {
    /// Mint URL.
    pub mint_url: String,
    /// Currency unit managed by this wallet.
    pub unit: CurrencyUnit,
    /// BIP-39 mnemonic used for deterministic wallet keys.
    pub mnemonic: String,
    /// Durable wallet storage.
    pub store: WalletStore,
    /// Optional operational tuning. Defaults are suitable for most apps.
    #[uniffi(default = None)]
    pub config: Option<WalletConfig>,
}

impl fmt::Debug for WalletOpenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletOpenRequest")
            .field("mint_url", &self.mint_url)
            .field("unit", &self.unit)
            .field("mnemonic", &"[REDACTED]")
            .field("store", &"[REDACTED]")
            .field("config", &self.config)
            .finish()
    }
}

/// Configuration for opening a multi-mint wallet.
#[derive(Clone, uniffi::Record)]
pub struct CashuWalletOpenRequest {
    /// BIP-39 mnemonic used by every mint wallet in the portfolio.
    pub mnemonic: String,
    /// Durable wallet storage.
    pub store: WalletStore,
    /// Optional shared proxy URL.
    #[uniffi(default = None)]
    pub proxy_url: Option<String>,
    /// Optional repository-wide request pacing.
    #[uniffi(default = None)]
    pub rate_limit: Option<RateLimit>,
}

impl fmt::Debug for CashuWalletOpenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CashuWalletOpenRequest")
            .field("mnemonic", &"[REDACTED]")
            .field("store", &"[REDACTED]")
            .field("proxy_url", &self.proxy_url)
            .field("rate_limit", &self.rate_limit)
            .finish()
    }
}

/// Identity and unit of a mint wallet.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WalletIdentity {
    /// Mint URL.
    pub mint_url: MintUrl,
    /// Currency unit.
    pub unit: CurrencyUnit,
}

/// Balances split by spendability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Record)]
pub struct WalletBalance {
    /// Funds that can be spent now.
    pub available: Amount,
    /// Funds waiting for a mint-side result.
    pub pending: Amount,
    /// Funds reserved by a prepared local operation.
    pub reserved: Amount,
}

/// Whether synchronization may contact the mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SyncPolicy {
    /// Read local state only.
    LocalOnly,
    /// Reconcile sagas, quotes, pending proofs, and payments with the mint.
    Online,
}

/// Result of one explicit wallet synchronization.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncReport {
    /// Wallet reconciled by this synchronization pass.
    pub wallet: WalletIdentity,
    /// Balance after synchronization.
    pub balance: WalletBalance,
    /// Interrupted operations completed successfully.
    pub recovered_operations: u64,
    /// Interrupted operations rolled back safely.
    pub compensated_operations: u64,
    /// Operations that remain pending.
    pub pending_operations: u64,
    /// Operations that could not be reconciled.
    pub failed_operations: u64,
    /// Paid mint quotes claimed during synchronization.
    pub claimed_amount: Amount,
    /// Pending proofs restored to the available balance.
    pub recovered_amount: Amount,
    /// Pending payments finalized during synchronization.
    pub finalized_payments: u64,
}

/// One wallet and its current balances.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WalletBalanceEntry {
    /// Wallet whose balance was read.
    pub wallet: WalletIdentity,
    /// Spendability breakdown.
    pub balance: WalletBalance,
}

/// A request to create ecash for an incoming payment.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintRequest {
    /// Payment rail offered to the payer.
    pub method: PaymentMethod,
    /// Requested amount. Some payment rails support an unspecified amount.
    #[uniffi(default = None)]
    pub amount: Option<Amount>,
    /// Human-readable payment description.
    #[uniffi(default = None)]
    pub description: Option<String>,
    /// Payment-method-specific JSON understood by the mint.
    #[uniffi(default = None)]
    pub extra: Option<String>,
}

/// Public state of a minting session. Internal quote locks and signing keys are
/// deliberately not part of the application API.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MintSessionState {
    /// Stable quote ID.
    pub id: String,
    /// Payment request to present to the payer.
    pub payment_request: String,
    /// Current mint-reported state.
    pub state: QuoteState,
    /// Requested amount, when fixed.
    pub amount: Option<Amount>,
    /// Amount received by the mint.
    pub amount_paid: Amount,
    /// Amount already issued into this wallet.
    pub amount_claimed: Amount,
    /// Quote expiry as a Unix timestamp.
    pub expires_at: u64,
    /// Payment rail used by this session.
    pub method: PaymentMethod,
}

/// Durable handle for an incoming mint quote.
#[derive(uniffi::Object)]
pub struct MintSession {
    wallet: Arc<cdk::Wallet>,
    quote_id: String,
    initial_state: MintSessionState,
}

impl fmt::Debug for MintSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MintSession")
            .field("quote_id", &self.quote_id)
            .finish_non_exhaustive()
    }
}

impl MintSession {
    fn from_quote(wallet: Arc<cdk::Wallet>, quote: cdk::wallet::MintQuote) -> Self {
        let initial_state = mint_session_state(&quote);
        Self {
            wallet,
            quote_id: quote.id,
            initial_state,
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl MintSession {
    /// Stable quote ID used to resume this session.
    pub fn id(&self) -> String {
        self.quote_id.clone()
    }

    /// State captured when this handle was created, available without a network request.
    pub fn initial_state(&self) -> MintSessionState {
        self.initial_state.clone()
    }

    /// Refresh the quote from the mint.
    pub async fn refresh(&self) -> Result<MintSessionState, FfiError> {
        let quote = self.wallet.check_mint_quote_status(&self.quote_id).await?;
        Ok(mint_session_state(&quote))
    }

    /// Claim all currently paid, unissued value into the wallet.
    pub async fn claim(&self) -> Result<Amount, FfiError> {
        let proofs = self
            .wallet
            .mint(&self.quote_id, cdk::amount::SplitTarget::None, None)
            .await?;
        Ok(proofs.total_amount()?.into())
    }
}

fn mint_session_state(quote: &cdk::wallet::MintQuote) -> MintSessionState {
    MintSessionState {
        id: quote.id.clone(),
        payment_request: quote.request.clone(),
        state: quote.state.into(),
        amount: quote.amount.map(Into::into),
        amount_paid: quote.amount_paid.into(),
        amount_claimed: quote.amount_issued.into(),
        expires_at: quote.expiry,
        method: quote.payment_method.clone().into(),
    }
}

/// High-level send request. Protocol-specific proof and condition controls are
/// intentionally part of the advanced engine API instead.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SendRequest {
    /// Value to transfer.
    pub amount: Amount,
    /// Memo embedded in the token.
    #[uniffi(default = None)]
    pub memo: Option<String>,
    /// Online/offline selection behavior.
    pub mode: SendKind,
    /// Add input fees so the receiver obtains the exact requested value.
    #[uniffi(default = true)]
    pub include_fee: bool,
    /// Application metadata stored with the transaction.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

/// High-level token receipt request.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ReceiveRequest {
    /// Token to redeem.
    pub token: Arc<Token>,
    /// Application metadata stored with the transaction.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

/// Receipt for a successfully redeemed token.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ReceiveReceipt {
    /// Value credited to the wallet.
    pub amount: Amount,
    /// Wallet that accepted the token.
    pub wallet: WalletIdentity,
}

/// A typed outgoing payment target.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum PaymentTarget {
    /// BOLT11 invoice, optionally amountless.
    Bolt11 {
        invoice: String,
        amount_msat: Option<Amount>,
    },
    /// BOLT12 offer and requested milli-satoshi amount.
    Bolt12 { offer: String, amount_msat: Amount },
    /// Bitcoin address with an amount and optional absolute fee ceiling.
    Onchain {
        address: String,
        amount: Amount,
        max_fee: Option<Amount>,
    },
    /// Extension payment rail.
    Custom {
        method: String,
        request: String,
        amount: Option<Amount>,
        extra: Option<String>,
    },
}

/// Request for one or more payment quotes.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PaymentQuoteRequest {
    /// Destination and payment rail.
    pub target: PaymentTarget,
    /// Application metadata persisted when a quote is prepared.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

/// Public payment quote preview.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PaymentQuote {
    /// Stable quote ID.
    pub id: String,
    /// Amount delivered by the mint.
    pub amount: Amount,
    /// Maximum mint fee reserved for the payment.
    pub fee_reserve: Amount,
    /// Current quote state.
    pub state: QuoteState,
    /// Expiry as a Unix timestamp.
    pub expires_at: u64,
    /// Estimated confirmation target for on-chain payments.
    pub estimated_blocks: Option<u32>,
    /// Payment rail.
    pub method: PaymentMethod,
}

/// A quote that can be prepared into a proof-reserving payment plan.
#[derive(uniffi::Object)]
pub struct PaymentSession {
    wallet: Arc<cdk::Wallet>,
    quote: cdk::wallet::MeltQuote,
    metadata: HashMap<String, String>,
    select_before_prepare: bool,
}

impl fmt::Debug for PaymentSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentSession")
            .field("quote_id", &self.quote.id)
            .field("amount", &self.quote.amount)
            .field("fee_reserve", &self.quote.fee_reserve)
            .finish_non_exhaustive()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PaymentSession {
    /// Immutable quote details for UI review.
    pub fn quote(&self) -> PaymentQuote {
        payment_quote(&self.quote)
    }

    /// Reserve proofs and produce a confirm-or-cancel payment plan.
    pub async fn prepare(&self) -> Result<Arc<PaymentPlan>, FfiError> {
        let quote = if self.select_before_prepare {
            self.wallet
                .select_onchain_melt_quote(self.quote.clone())
                .await?
        } else {
            self.quote.clone()
        };
        let prepared = self
            .wallet
            .prepare_melt(&quote.id, self.metadata.clone())
            .await?;
        Ok(Arc::new(PaymentPlan::new(
            Arc::clone(&self.wallet),
            &prepared,
        )))
    }
}

fn payment_quote(quote: &cdk::wallet::MeltQuote) -> PaymentQuote {
    PaymentQuote {
        id: quote.id.clone(),
        amount: quote.amount.into(),
        fee_reserve: quote.fee_reserve.into(),
        state: quote.state.into(),
        expires_at: quote.expiry,
        estimated_blocks: quote.estimated_blocks,
        method: quote.payment_method.clone().into(),
    }
}

/// Transaction-history filter.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct HistoryQuery {
    /// Restrict to incoming or outgoing transactions.
    #[uniffi(default = None)]
    pub direction: Option<TransactionDirection>,
    /// Maximum number of newest entries to return.
    #[uniffi(default = None)]
    pub limit: Option<u32>,
}

/// Request to move the maximum currently spendable balance between two mint
/// wallets over Lightning.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CrossMintTransferRequest {
    /// Wallet that pays the Lightning invoice.
    pub source: WalletIdentity,
    /// Wallet that receives newly issued ecash.
    pub destination: WalletIdentity,
    /// Application metadata stored with the source transaction.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

/// Successful cross-mint transfer result.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CrossMintTransferReceipt {
    /// Durable source operation ID.
    pub operation_id: String,
    /// Destination wallet.
    pub destination: WalletIdentity,
    /// Quote claimed at the destination.
    pub destination_quote_id: String,
    /// Amount issued at the destination.
    pub amount: Amount,
    /// Actual source-side fee.
    pub source_fee: Amount,
}

/// Source payment succeeded, but destination issuance still needs to be
/// claimed. Calling `synchronize(Online)` will retry all such paid quotes.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CrossMintClaimPending {
    /// Durable source operation ID.
    pub operation_id: String,
    /// Destination wallet.
    pub destination: WalletIdentity,
    /// Paid destination quote that remains claimable.
    pub destination_quote_id: String,
    /// Amount expected at the destination.
    pub amount: Amount,
    /// Actual source-side fee.
    pub source_fee: Amount,
    /// Why the immediate issuance attempt failed.
    pub error_message: String,
    /// Whether retrying can be useful.
    pub retryable: bool,
}

/// Outcome of confirming a cross-mint transfer.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CrossMintTransferOutcome {
    /// Source payment and destination issuance both completed.
    Completed { receipt: CrossMintTransferReceipt },
    /// Source payment completed; destination issuance is durably recoverable.
    ClaimPending { pending: CrossMintClaimPending },
}

/// Durable, reviewable cross-mint transfer plan.
#[derive(uniffi::Object)]
pub struct PreparedCrossMintTransfer {
    source: Arc<cdk::Wallet>,
    destination: Arc<cdk::Wallet>,
    operation_id: uuid::Uuid,
    destination_identity: WalletIdentity,
    destination_quote_id: String,
    amount: Amount,
    maximum_fee: Amount,
}

impl fmt::Debug for PreparedCrossMintTransfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedCrossMintTransfer")
            .field("operation_id", &self.operation_id)
            .field("destination", &self.destination_identity)
            .field("destination_quote_id", &self.destination_quote_id)
            .field("amount", &self.amount)
            .field("maximum_fee", &self.maximum_fee)
            .finish_non_exhaustive()
    }
}

impl PreparedCrossMintTransfer {
    fn new(
        source: Arc<cdk::Wallet>,
        destination: Arc<cdk::Wallet>,
        prepared: &cdk::wallet::PreparedMelt,
    ) -> Result<Self, FfiError> {
        let cdk::wallet::PreparedMeltPurpose::CrossMintTransfer {
            destination_mint_url,
            destination_unit,
            destination_quote_id,
        } = prepared.purpose()
        else {
            return Err(cdk::Error::InvalidOperationState.into());
        };
        if destination.mint_url != *destination_mint_url || destination.unit != *destination_unit {
            return Err(cdk::Error::InvalidOperationState.into());
        }
        let maximum_fee = prepared
            .quote()
            .fee_reserve
            .checked_add(prepared.input_fee_without_swap())
            .ok_or(cdk::Error::AmountOverflow)?;

        Ok(Self {
            source,
            destination,
            operation_id: prepared.operation_id(),
            destination_identity: WalletIdentity {
                mint_url: destination_mint_url.clone().into(),
                unit: destination_unit.clone().into(),
            },
            destination_quote_id: destination_quote_id.clone(),
            amount: prepared.amount().into(),
            maximum_fee: maximum_fee.into(),
        })
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PreparedCrossMintTransfer {
    /// Durable operation ID used to resume this plan.
    pub fn operation_id(&self) -> String {
        self.operation_id.to_string()
    }

    /// Amount expected at the destination.
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Maximum combined mint and input fee.
    pub fn maximum_fee(&self) -> Amount {
        self.maximum_fee
    }

    /// Destination wallet.
    pub fn destination(&self) -> WalletIdentity {
        self.destination_identity.clone()
    }

    /// Quote that will issue ecash at the destination.
    pub fn destination_quote_id(&self) -> String {
        self.destination_quote_id.clone()
    }

    /// Pay the destination quote and claim its ecash.
    pub async fn confirm(&self) -> Result<CrossMintTransferOutcome, FfiError> {
        let finalized = self
            .source
            .confirm_prepared_melt_with_options(
                self.operation_id,
                cdk::wallet::MeltConfirmOptions::skip_swap(),
            )
            .await?;
        let source_fee: Amount = finalized.fee_paid().into();

        match self
            .destination
            .mint(
                &self.destination_quote_id,
                cdk::amount::SplitTarget::None,
                None,
            )
            .await
        {
            Ok(proofs) => Ok(CrossMintTransferOutcome::Completed {
                receipt: CrossMintTransferReceipt {
                    operation_id: self.operation_id.to_string(),
                    destination: self.destination_identity.clone(),
                    destination_quote_id: self.destination_quote_id.clone(),
                    amount: proofs.total_amount()?.into(),
                    source_fee,
                },
            }),
            Err(error) => {
                let retryable = !error.is_definitive_failure();
                Ok(CrossMintTransferOutcome::ClaimPending {
                    pending: CrossMintClaimPending {
                        operation_id: self.operation_id.to_string(),
                        destination: self.destination_identity.clone(),
                        destination_quote_id: self.destination_quote_id.clone(),
                        amount: self.amount,
                        source_fee,
                        error_message: error.to_string(),
                        retryable,
                    },
                })
            }
        }
    }

    /// Cancel the local plan and release its source proofs.
    pub async fn cancel(&self) -> Result<(), FfiError> {
        self.source.cancel_prepared_melt(self.operation_id).await?;
        Ok(())
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Wallet {
    /// Open a single mint-and-unit wallet without contacting the mint.
    #[uniffi::constructor]
    pub fn open(request: WalletOpenRequest) -> Result<Self, FfiError> {
        Self::new_advanced(
            request.mint_url,
            request.unit,
            request.mnemonic,
            request.store,
            request.config.unwrap_or_default(),
        )
    }

    /// Mint and unit managed by this wallet.
    pub fn identity(&self) -> WalletIdentity {
        WalletIdentity {
            mint_url: self.inner().mint_url.clone().into(),
            unit: self.inner().unit.clone().into(),
        }
    }

    /// Read the available, pending, and reserved balances in one call.
    pub async fn balance(&self) -> Result<WalletBalance, FfiError> {
        wallet_balance(self.inner()).await
    }

    /// Explicitly reconcile wallet state according to the requested network policy.
    pub async fn synchronize(&self, policy: SyncPolicy) -> Result<SyncReport, FfiError> {
        synchronize_wallet(self.inner(), policy).await
    }

    /// Create an incoming-payment session.
    pub async fn request_minting(
        &self,
        request: MintRequest,
    ) -> Result<Arc<MintSession>, FfiError> {
        let quote = self
            .inner()
            .mint_quote(
                request.method.into(),
                request.amount.map(Into::into),
                request.description,
                request.extra,
            )
            .await?;
        let session = MintSession::from_quote(Arc::clone(self.inner()), quote);
        Ok(Arc::new(session))
    }

    /// Resume a locally known incoming-payment session by quote ID.
    pub async fn minting_session(&self, quote_id: String) -> Result<Arc<MintSession>, FfiError> {
        let quote = self
            .inner()
            .localstore
            .get_mint_quote(&quote_id)
            .await
            .map_err(cdk::Error::from)?
            .ok_or(cdk::Error::UnknownQuote)?;
        let session = MintSession::from_quote(Arc::clone(self.inner()), quote);
        Ok(Arc::new(session))
    }

    /// Select and reserve proofs for an ecash transfer.
    pub async fn plan_send(&self, request: SendRequest) -> Result<Arc<SendPlan>, FfiError> {
        let options = cdk::wallet::SendOptions {
            memo: request
                .memo
                .map(|memo| cdk::wallet::SendMemo::for_token(&memo)),
            send_kind: request.mode.into(),
            include_fee: request.include_fee,
            metadata: request.metadata,
            ..Default::default()
        };

        let prepared = self
            .inner()
            .prepare_send(request.amount.into(), options)
            .await?;
        Ok(Arc::new(SendPlan::new(Arc::clone(self.inner()), &prepared)))
    }

    /// Resume a prepared send after a process restart.
    pub async fn send_plan(&self, operation_id: String) -> Result<Arc<SendPlan>, FfiError> {
        let operation_id = parse_operation_id(&operation_id)?;
        let prepared = self.inner().prepared_send(operation_id).await?;
        Ok(Arc::new(SendPlan::new(Arc::clone(self.inner()), &prepared)))
    }

    /// Validate and redeem a token into this wallet.
    pub async fn accept(&self, request: ReceiveRequest) -> Result<ReceiveReceipt, FfiError> {
        let options = cdk::wallet::ReceiveOptions {
            metadata: request.metadata,
            ..Default::default()
        };
        let amount = self
            .inner()
            .receive(&request.token.to_string(), options)
            .await?;
        Ok(ReceiveReceipt {
            amount: amount.into(),
            wallet: self.identity(),
        })
    }

    /// Quote an outgoing payment. On-chain requests return one session per fee option.
    pub async fn quote_payment(
        &self,
        request: PaymentQuoteRequest,
    ) -> Result<Vec<Arc<PaymentSession>>, FfiError> {
        let metadata = request.metadata;
        let sessions = match request.target {
            PaymentTarget::Bolt11 {
                invoice,
                amount_msat,
            } => {
                let options = amount_msat.map(cdk::nuts::MeltOptions::new_amountless);
                let quote = self
                    .inner()
                    .melt_quote::<cdk::nuts::PaymentMethod, _>(
                        cdk::nuts::PaymentMethod::from("bolt11"),
                        invoice,
                        options,
                        None,
                    )
                    .await?;
                vec![payment_session(self.inner(), quote, metadata, false)]
            }
            PaymentTarget::Bolt12 { offer, amount_msat } => {
                let quote = self
                    .inner()
                    .melt_quote::<cdk::nuts::PaymentMethod, _>(
                        cdk::nuts::PaymentMethod::from("bolt12"),
                        offer,
                        Some(cdk::nuts::MeltOptions::new_amountless(amount_msat)),
                        None,
                    )
                    .await?;
                vec![payment_session(self.inner(), quote, metadata, false)]
            }
            PaymentTarget::Onchain {
                address,
                amount,
                max_fee,
            } => self
                .inner()
                .quote_onchain_melt_options(&address, amount.into(), max_fee.map(Into::into))
                .await?
                .into_iter()
                .map(|quote| payment_session(self.inner(), quote, metadata.clone(), true))
                .collect(),
            PaymentTarget::Custom {
                method,
                request,
                amount,
                extra,
            } => {
                let options = amount.map(cdk::nuts::MeltOptions::new_amountless);
                let quote = self
                    .inner()
                    .melt_quote::<cdk::nuts::PaymentMethod, _>(
                        cdk::nuts::PaymentMethod::from(method),
                        request,
                        options,
                        extra,
                    )
                    .await?;
                vec![payment_session(self.inner(), quote, metadata, false)]
            }
        };
        Ok(sessions)
    }

    /// Resume a prepared outgoing payment after a process restart.
    pub async fn payment_plan(&self, operation_id: String) -> Result<Arc<PaymentPlan>, FfiError> {
        let operation_id = parse_operation_id(&operation_id)?;
        let prepared = self.inner().prepared_melt(operation_id).await?;
        Ok(Arc::new(PaymentPlan::new(
            Arc::clone(self.inner()),
            &prepared,
        )))
    }

    /// Resume a payment that the mint is still processing.
    pub async fn pending_payment(
        &self,
        operation_id: String,
    ) -> Result<Arc<PendingPayment>, FfiError> {
        let operation_id = parse_operation_id(&operation_id)?;
        let pending = self.inner().pending_melt(operation_id).await?;
        Ok(Arc::new(PendingPayment::new(
            Arc::clone(self.inner()),
            &pending,
        )))
    }

    /// Read wallet transaction history.
    pub async fn history(&self, query: HistoryQuery) -> Result<Vec<Transaction>, FfiError> {
        let mut transactions = self
            .inner()
            .list_transactions(query.direction.map(Into::into))
            .await?;
        if let Some(limit) = query.limit {
            transactions.truncate(limit as usize);
        }
        Ok(transactions.into_iter().map(Into::into).collect())
    }

    /// Subscribe to wallet events. Dropping the returned handle cancels the subscription.
    pub async fn events(
        &self,
        params: SubscribeParams,
    ) -> Result<Arc<ActiveSubscription>, FfiError> {
        let engine_params: cdk::nuts::nut17::Params<Arc<String>> = params.into();
        let subscription_id = engine_params.id.to_string();
        let subscription = self.inner().subscribe(engine_params).await?;
        Ok(Arc::new(ActiveSubscription::new(
            subscription,
            subscription_id,
        )))
    }

    /// Re-scan deterministic wallet history from the seed.
    pub async fn restore_from_seed(&self) -> Result<Restored, FfiError> {
        Ok(self.inner().restore().await?.into())
    }
}

fn payment_session(
    wallet: &Arc<cdk::Wallet>,
    quote: cdk::wallet::MeltQuote,
    metadata: HashMap<String, String>,
    select_before_prepare: bool,
) -> Arc<PaymentSession> {
    Arc::new(PaymentSession {
        wallet: Arc::clone(wallet),
        quote,
        metadata,
        select_before_prepare,
    })
}

fn parse_operation_id(operation_id: &str) -> Result<uuid::Uuid, FfiError> {
    uuid::Uuid::parse_str(operation_id)
        .map_err(|error| FfiError::invalid_input(format!("Invalid operation ID: {error}")))
}

async fn wallet_balance(wallet: &cdk::Wallet) -> Result<WalletBalance, FfiError> {
    Ok(WalletBalance {
        available: wallet.total_balance().await?.into(),
        pending: wallet.total_pending_balance().await?.into(),
        reserved: wallet.total_reserved_balance().await?.into(),
    })
}

async fn synchronize_wallet(
    wallet: &cdk::Wallet,
    policy: SyncPolicy,
) -> Result<SyncReport, FfiError> {
    let (recovery, claimed_amount, recovered_amount, finalized_payments) = match policy {
        SyncPolicy::LocalOnly => (
            cdk::wallet::RecoveryReport::default(),
            cdk::Amount::ZERO,
            cdk::Amount::ZERO,
            0,
        ),
        SyncPolicy::Online => {
            let recovery = wallet.recover_incomplete_sagas().await?;
            let claimed_amount = wallet.mint_unissued_quotes().await?;
            let recovered_amount = wallet.check_all_pending_proofs().await?;
            let finalized_payments = wallet.finalize_pending_melts().await?.len() as u64;
            (
                recovery,
                claimed_amount,
                recovered_amount,
                finalized_payments,
            )
        }
    };

    Ok(SyncReport {
        wallet: WalletIdentity {
            mint_url: wallet.mint_url.clone().into(),
            unit: wallet.unit.clone().into(),
        },
        balance: wallet_balance(wallet).await?,
        recovered_operations: recovery.recovered as u64,
        compensated_operations: recovery.compensated as u64,
        pending_operations: recovery.skipped as u64,
        failed_operations: recovery.failed as u64,
        claimed_amount: claimed_amount.into(),
        recovered_amount: recovered_amount.into(),
        finalized_payments,
    })
}

/// Multi-mint root object. It owns shared storage, seed, transport settings,
/// and the set of per-mint wallets.
#[derive(uniffi::Object)]
pub struct CashuWallet {
    repository: Arc<cdk::wallet::WalletRepository>,
}

impl fmt::Debug for CashuWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CashuWallet").finish_non_exhaustive()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl CashuWallet {
    /// Open all locally configured mint wallets without network access.
    #[uniffi::constructor]
    pub fn open(request: CashuWalletOpenRequest) -> Result<Self, FfiError> {
        let repository = WalletRepository::new_with_config(
            request.mnemonic,
            request.store,
            WalletRepositoryConfig {
                proxy_url: request.proxy_url,
                rate_limit: request.rate_limit,
            },
        )?;
        Ok(Self {
            repository: repository.inner(),
        })
    }

    /// Return a mint wallet, creating its local configuration when absent.
    pub async fn wallet(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
    ) -> Result<Arc<Wallet>, FfiError> {
        let mint_url = mint_url.try_into()?;
        let wallet = self
            .repository
            .get_or_create_wallet(mint_url, unit.into(), None)
            .await?;
        Ok(Arc::new(Wallet::from_inner(Arc::new(wallet))))
    }

    /// List all configured mint wallets.
    pub async fn wallets(&self) -> Vec<Arc<Wallet>> {
        self.repository
            .get_wallets()
            .await
            .into_iter()
            .map(|wallet| Arc::new(Wallet::from_inner(Arc::new(wallet))))
            .collect()
    }

    /// Read balances for every configured mint wallet.
    pub async fn balances(&self) -> Result<Vec<WalletBalanceEntry>, FfiError> {
        let wallets = self.repository.get_wallets().await;
        let mut entries = Vec::with_capacity(wallets.len());
        for wallet in wallets {
            entries.push(WalletBalanceEntry {
                wallet: WalletIdentity {
                    mint_url: wallet.mint_url.clone().into(),
                    unit: wallet.unit.clone().into(),
                },
                balance: wallet_balance(&wallet).await?,
            });
        }
        Ok(entries)
    }

    /// Create a durable maximum-balance transfer plan between two configured
    /// mint wallets.
    pub async fn plan_cross_mint_transfer(
        &self,
        request: CrossMintTransferRequest,
    ) -> Result<Arc<PreparedCrossMintTransfer>, FfiError> {
        let source = repository_wallet(&self.repository, &request.source).await?;
        let destination = repository_wallet(&self.repository, &request.destination).await?;
        let prepared = source
            .prepare_cross_mint_transfer(&destination, request.metadata)
            .await?;
        Ok(Arc::new(PreparedCrossMintTransfer::new(
            Arc::new(source),
            Arc::new(destination),
            &prepared,
        )?))
    }

    /// Resume a prepared cross-mint transfer from its source operation ID.
    pub async fn cross_mint_transfer_plan(
        &self,
        operation_id: String,
    ) -> Result<Arc<PreparedCrossMintTransfer>, FfiError> {
        let operation_id = parse_operation_id(&operation_id)?;
        for source in self.repository.get_wallets().await {
            let prepared = match source.prepared_melt(operation_id).await {
                Ok(prepared) => prepared,
                Err(cdk::Error::InvalidOperationState | cdk::Error::OperationNotFound) => continue,
                Err(error) => return Err(error.into()),
            };
            let cdk::wallet::PreparedMeltPurpose::CrossMintTransfer {
                destination_mint_url,
                destination_unit,
                ..
            } = prepared.purpose()
            else {
                return Err(cdk::Error::InvalidOperationState.into());
            };
            let destination = self
                .repository
                .get_or_create_wallet(destination_mint_url.clone(), destination_unit.clone(), None)
                .await?;
            return Ok(Arc::new(PreparedCrossMintTransfer::new(
                Arc::new(source),
                Arc::new(destination),
                &prepared,
            )?));
        }

        Err(cdk::Error::OperationNotFound.into())
    }

    /// Synchronize every configured mint wallet.
    pub async fn synchronize(&self, policy: SyncPolicy) -> Result<Vec<SyncReport>, FfiError> {
        let wallets = self.repository.get_wallets().await;
        let mut reports = Vec::with_capacity(wallets.len());
        for wallet in wallets {
            reports.push(synchronize_wallet(&wallet, policy).await?);
        }
        Ok(reports)
    }

    /// Read history across all configured mint wallets.
    pub async fn history(&self, query: HistoryQuery) -> Result<Vec<Transaction>, FfiError> {
        let mut transactions = self
            .repository
            .list_transactions(query.direction.map(Into::into))
            .await?;
        if let Some(limit) = query.limit {
            transactions.truncate(limit as usize);
        }
        Ok(transactions.into_iter().map(Into::into).collect())
    }
}

async fn repository_wallet(
    repository: &cdk::wallet::WalletRepository,
    identity: &WalletIdentity,
) -> Result<cdk::Wallet, FfiError> {
    repository
        .get_or_create_wallet(
            identity.mint_url.clone().try_into()?,
            identity.unit.clone().into(),
            None,
        )
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::custom_wallet_store;
    use crate::sqlite::WalletSqliteDatabase;

    const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn memory_store() -> WalletStore {
        custom_wallet_store(
            WalletSqliteDatabase::new_in_memory().expect("in-memory wallet database should open"),
        )
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn open_request_debug_redacts_seed_and_store() {
        let request = WalletOpenRequest {
            mint_url: "https://mint.example.com".to_string(),
            unit: CurrencyUnit::Sat,
            mnemonic: MNEMONIC.to_string(),
            store: memory_store(),
            config: None,
        };

        let output = format!("{request:?}");
        assert!(!output.contains(MNEMONIC));
        assert!(output.contains("[REDACTED]"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn single_wallet_local_queries_do_not_require_network() {
        let wallet = Wallet::open(WalletOpenRequest {
            mint_url: "https://mint.example.com".to_string(),
            unit: CurrencyUnit::Sat,
            mnemonic: MNEMONIC.to_string(),
            store: memory_store(),
            config: None,
        })
        .expect("wallet should open");

        let identity = wallet.identity();
        assert_eq!(identity.mint_url.url, "https://mint.example.com");
        assert_eq!(identity.unit, CurrencyUnit::Sat);
        assert_eq!(wallet.balance().await.unwrap(), WalletBalance::default());

        let report = wallet.synchronize(SyncPolicy::LocalOnly).await.unwrap();
        assert_eq!(report.wallet, identity);
        assert_eq!(report.balance, WalletBalance::default());
        assert_eq!(report.recovered_operations, 0);
        assert_eq!(report.compensated_operations, 0);

        let quote = cdk::wallet::MintQuote::new(
            "local-quote".to_string(),
            wallet.inner().mint_url.clone(),
            cdk::nuts::PaymentMethod::BOLT11,
            Some(cdk::Amount::from(21)),
            wallet.inner().unit.clone(),
            "lnbc-local".to_string(),
            4_000_000_000,
            None,
        );
        wallet
            .inner()
            .localstore
            .add_mint_quote(quote)
            .await
            .unwrap();
        let session = wallet
            .minting_session("local-quote".to_string())
            .await
            .expect("a persisted mint session should resume offline");
        assert_eq!(session.id(), "local-quote");
        assert_eq!(session.initial_state().payment_request, "lnbc-local");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multi_mint_root_identifies_each_balance() {
        let root = CashuWallet::open(CashuWalletOpenRequest {
            mnemonic: MNEMONIC.to_string(),
            store: memory_store(),
            proxy_url: None,
            rate_limit: None,
        })
        .expect("portfolio should open");
        root.wallet(
            MintUrl {
                url: "https://mint.example.com".to_string(),
            },
            CurrencyUnit::Sat,
        )
        .await
        .expect("wallet should be configured");

        let balances = root.balances().await.unwrap();
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].wallet.mint_url.url, "https://mint.example.com");
        assert_eq!(balances[0].balance, WalletBalance::default());
    }
}
