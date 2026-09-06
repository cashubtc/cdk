//! Incoming-payment sessions that issue ecash.

use std::fmt;
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use super::advanced::MintClaimOptions;
use super::operation::{OperationKind, OperationReference, OperationState};
use super::{MintQuote, Wallet, WalletIdentity};
use crate::nuts::{MintQuoteState, PaymentMethod};
use crate::{Amount, Error};

/// Stable identifier for an incoming mint quote.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MintQuoteId(String);

impl MintQuoteId {
    /// Create an identifier from the mint-provided value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the mint-provided value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MintQuoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for MintQuoteId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Lifecycle of an incoming minting session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MintState {
    /// The payer has not completed payment.
    Unpaid,
    /// The mint received value and ecash can be claimed.
    Paid,
    /// All paid value has been issued into this wallet.
    Issued,
}

impl fmt::Display for MintState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unpaid => "unpaid",
            Self::Paid => "paid",
            Self::Issued => "issued",
        })
    }
}

impl From<MintQuoteState> for MintState {
    fn from(value: MintQuoteState) -> Self {
        match value {
            MintQuoteState::Unpaid => Self::Unpaid,
            MintQuoteState::Paid => Self::Paid,
            MintQuoteState::Issued => Self::Issued,
        }
    }
}

/// Request for an incoming payment that will issue ecash.
#[derive(Debug, Clone)]
pub struct MintRequest {
    /// Payment rail offered to the payer.
    pub method: PaymentMethod,
    /// Requested amount, or `None` for a variable-amount rail.
    pub amount: Option<Amount>,
    /// Human-readable payment description.
    pub description: Option<String>,
    /// Payment-method-specific JSON understood by the mint.
    pub extra: Option<String>,
}

impl MintRequest {
    /// Create a mint request for a payment rail and optional fixed amount.
    pub fn new(method: PaymentMethod, amount: Option<Amount>) -> Self {
        Self {
            method,
            amount,
            description: None,
            extra: None,
        }
    }

    /// Create a fixed-amount BOLT11 mint request.
    pub fn bolt11(amount: Amount) -> Self {
        Self::new(PaymentMethod::BOLT11, Some(amount))
    }

    /// Attach a payer-visible description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Public state of an incoming payment session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintSessionState {
    /// Stable quote identifier.
    pub id: MintQuoteId,
    /// Payment request to present to the payer.
    pub payment_request: String,
    /// Current mint-reported state.
    pub state: MintState,
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

/// Receipt for successfully claimed incoming value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintReceipt {
    /// Quote that was claimed.
    pub quote_id: MintQuoteId,
    /// Value issued for this result.
    ///
    /// `claim` reports the quote's cumulative issued value. `receipts` reports
    /// each newly issued batch; `wait` reports that batch or, when already
    /// issued locally, the cumulative value. Use `refresh().amount_claimed`
    /// for an explicit cumulative total.
    pub amount: Amount,
    /// Wallet that received the value.
    pub wallet: WalletIdentity,
}

/// Durable handle for an incoming mint quote.
#[derive(Clone)]
pub struct MintSession {
    wallet: Wallet,
    quote_id: MintQuoteId,
    #[cfg(not(target_arch = "wasm32"))]
    quote: MintQuote,
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
    pub(super) fn from_quote(wallet: Wallet, quote: MintQuote) -> Self {
        let quote_id = MintQuoteId::new(quote.id.clone());
        Self {
            wallet,
            quote_id,
            #[cfg(not(target_arch = "wasm32"))]
            quote: quote.clone(),
            initial_state: mint_session_state(&quote),
        }
    }

    /// Stable identifier used to resume this session after a restart.
    pub fn id(&self) -> &MintQuoteId {
        &self.quote_id
    }

    /// State captured when the handle was created, without network access.
    pub fn initial_state(&self) -> &MintSessionState {
        &self.initial_state
    }

    /// Wallet that owns this mint session.
    pub fn wallet_identity(&self) -> WalletIdentity {
        self.wallet.identity()
    }

    /// Refresh this quote from the mint.
    pub async fn refresh(&self) -> Result<MintSessionState, Error> {
        let previous_paid = self
            .wallet
            .localstore
            .get_mint_quote(self.quote_id.as_str())
            .await?
            .map(|quote| quote.amount_paid)
            .unwrap_or_default();
        let quote = self
            .wallet
            .check_mint_quote_status(self.quote_id.as_str())
            .await?;
        let state = mint_session_state(&quote);
        if state.amount_paid > previous_paid {
            self.wallet
                .publish_mint_payment_event(self.quote_id.clone(), state.amount_paid);
        }
        self.wallet.publish_operation_event(
            OperationReference::MintQuote(self.quote_id.clone()),
            OperationKind::Mint,
            match state.state {
                MintState::Unpaid => OperationState::AwaitingPayment,
                MintState::Paid => OperationState::Ready,
                MintState::Issued => OperationState::Completed,
            },
            state.amount,
        );
        Ok(state)
    }

    /// Claim all paid value for this quote.
    ///
    /// This operation is idempotent after issuance. Reusable payment rails
    /// check for additional paid value when the local quote is fully issued.
    pub async fn claim(&self) -> Result<MintReceipt, Error> {
        self.claim_with(MintClaimOptions::default()).await
    }

    /// Claim paid value with explicit proof denomination or locking controls.
    ///
    /// This operation is idempotent after issuance.
    pub async fn claim_with(&self, options: MintClaimOptions) -> Result<MintReceipt, Error> {
        // Only single-use invoices are terminal after issuance. For BOLT12,
        // on-chain and extension rails, the mint engine refreshes a fully
        // issued local quote to discover subsequent payments.
        if self.initial_state.method == PaymentMethod::BOLT11 {
            if let Some(amount) =
                claimed_mint_quote_amount(&self.wallet, self.quote_id.as_str()).await?
            {
                let receipt = MintReceipt {
                    quote_id: self.quote_id.clone(),
                    amount,
                    wallet: self.wallet.identity(),
                };
                self.publish_claimed(&receipt).await;
                return Ok(receipt);
            }
        }

        let result = self
            .wallet
            .mint(
                self.quote_id.as_str(),
                options.amount_split_target,
                options.conditions,
            )
            .await;
        let amount = match result {
            Ok(proofs) => {
                match claimed_mint_quote_amount(&self.wallet, self.quote_id.as_str()).await? {
                    Some(amount) => amount,
                    None => proofs.into_iter().try_fold(Amount::ZERO, |total, proof| {
                        total.checked_add(proof.amount).ok_or(Error::AmountOverflow)
                    })?,
                }
            }
            Err(error) => {
                match claimed_mint_quote_amount(&self.wallet, self.quote_id.as_str()).await? {
                    Some(amount) => amount,
                    None => return Err(error),
                }
            }
        };

        let receipt = MintReceipt {
            quote_id: self.quote_id.clone(),
            amount,
            wallet: self.wallet.identity(),
        };
        self.publish_claimed(&receipt).await;
        Ok(receipt)
    }

    /// Stream each newly paid and issued batch for a reusable quote.
    ///
    /// Fixed-amount and single-use quotes normally yield one receipt. Variable
    /// or reusable payment rails can yield more until the stream is dropped.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn receipts(
        &self,
        options: MintClaimOptions,
    ) -> impl futures::Stream<Item = Result<MintReceipt, Error>> + Unpin + '_ {
        let quote_id = self.quote_id.clone();
        let identity = self.wallet.identity();
        let wallet = self.wallet.clone();
        Box::pin(
            self.wallet
                .proof_stream(
                    self.quote.clone(),
                    options.amount_split_target,
                    options.conditions,
                )
                .then(move |result| {
                    let quote_id = quote_id.clone();
                    let identity = identity.clone();
                    let wallet = wallet.clone();
                    async move {
                        let proofs = result?;
                        let receipt = MintReceipt {
                            quote_id: quote_id.clone(),
                            amount: Amount::try_sum(proofs.iter().map(|proof| proof.amount))?,
                            wallet: identity,
                        };
                        wallet.publish_operation_event(
                            OperationReference::MintQuote(quote_id.clone()),
                            OperationKind::Mint,
                            OperationState::Completed,
                            Some(receipt.amount),
                        );
                        wallet.publish_balance_event().await;
                        wallet.publish_quote_transactions(quote_id.as_str()).await;
                        Ok(receipt)
                    }
                }),
        )
    }

    /// Wait for payment and claim the quote using default output controls.
    pub async fn wait(&self, timeout: Duration) -> Result<MintReceipt, Error> {
        self.wait_with(MintClaimOptions::default(), timeout).await
    }

    /// Wait for payment and claim the quote using explicit output controls.
    pub async fn wait_with(
        &self,
        options: MintClaimOptions,
        timeout: Duration,
    ) -> Result<MintReceipt, Error> {
        if let Some(amount) =
            claimed_mint_quote_amount(&self.wallet, self.quote_id.as_str()).await?
        {
            return Ok(MintReceipt {
                quote_id: self.quote_id.clone(),
                amount,
                wallet: self.wallet.identity(),
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut receipts = Box::pin(self.receipts(options));
            return tokio::time::timeout(timeout, receipts.next())
                .await
                .map_err(|_| Error::Timeout)?
                .ok_or(Error::Internal)?;
        }

        #[cfg(target_arch = "wasm32")]
        {
            use futures::future::{select, Either};

            let wait_for_payment = std::pin::pin!(async {
                loop {
                    let state = self.refresh().await?;
                    if state.state != MintState::Unpaid || state.amount_paid > state.amount_claimed
                    {
                        return self.claim_with(options.clone()).await;
                    }
                    wasm_sleep(Duration::from_millis(500)).await;
                }
            });
            let timeout = std::pin::pin!(wasm_sleep(timeout));

            match select(wait_for_payment, timeout).await {
                Either::Left((receipt, _)) => receipt,
                Either::Right(((), _)) => Err(Error::Timeout),
            }
        }
    }

    async fn publish_claimed(&self, receipt: &MintReceipt) {
        self.wallet.publish_operation_event(
            OperationReference::MintQuote(receipt.quote_id.clone()),
            OperationKind::Mint,
            OperationState::Completed,
            Some(receipt.amount),
        );
        self.wallet.publish_balance_event().await;
        self.wallet
            .publish_quote_transactions(receipt.quote_id.as_str())
            .await;
    }
}

#[cfg(target_arch = "wasm32")]
async fn wasm_sleep(duration: Duration) {
    let maximum_chunk = Duration::from_millis(u64::from(u32::MAX));
    let mut remaining = duration;
    while !remaining.is_zero() {
        let chunk = remaining.min(maximum_chunk);
        gloo_timers::future::TimeoutFuture::new(chunk.as_millis() as u32).await;
        remaining = remaining.saturating_sub(chunk);
    }
}

async fn claimed_mint_quote_amount(
    wallet: &Wallet,
    quote_id: &str,
) -> Result<Option<Amount>, Error> {
    let Some(quote) = wallet.localstore.get_mint_quote(quote_id).await? else {
        return Ok(None);
    };
    ensure_mint_quote_belongs_to_wallet(wallet, &quote)?;
    // Reusable rails track issuance through amounts and may retain their
    // pre-issuance state until the next remote refresh.
    let fully_issued = quote.state == MintQuoteState::Issued
        || (quote.amount_issued > Amount::ZERO && quote.amount_issued == quote.amount_paid);
    Ok(fully_issued.then_some(quote.amount_issued))
}

fn ensure_mint_quote_belongs_to_wallet(wallet: &Wallet, quote: &MintQuote) -> Result<(), Error> {
    if quote.mint_url != wallet.mint_url {
        return Err(Error::IncorrectMint);
    }
    if quote.unit != wallet.unit {
        return Err(Error::UnsupportedUnit);
    }
    Ok(())
}

fn mint_session_state(quote: &MintQuote) -> MintSessionState {
    MintSessionState {
        id: MintQuoteId::new(quote.id.clone()),
        payment_request: quote.request.clone(),
        state: quote.state.into(),
        amount: quote.amount,
        amount_paid: quote.amount_paid,
        amount_claimed: quote.amount_issued,
        expires_at: quote.expiry,
        method: quote.payment_method.clone(),
    }
}

impl Wallet {
    /// Create an incoming-payment session.
    pub async fn request_mint(&self, request: MintRequest) -> Result<MintSession, Error> {
        let quote = self
            .mint_quote(
                request.method,
                request.amount,
                request.description,
                request.extra,
            )
            .await?;
        let session = MintSession::from_quote(self.clone(), quote);
        self.publish_operation_event(
            OperationReference::MintQuote(session.id().clone()),
            OperationKind::Mint,
            match session.initial_state().state {
                MintState::Unpaid => OperationState::AwaitingPayment,
                MintState::Paid => OperationState::Ready,
                MintState::Issued => OperationState::Completed,
            },
            session.initial_state().amount,
        );
        Ok(session)
    }

    /// Resume a locally known incoming-payment session.
    pub async fn resume_mint(&self, quote_id: MintQuoteId) -> Result<MintSession, Error> {
        let quote = self
            .localstore
            .get_mint_quote(quote_id.as_str())
            .await?
            .ok_or(Error::UnknownQuote)?;
        ensure_mint_quote_belongs_to_wallet(self, &quote)?;
        Ok(MintSession::from_quote(self.clone(), quote))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::MintQuoteResponse;

    use super::*;
    use crate::nuts::{MintQuoteBolt12Response, SecretKey};
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock, test_mint_quote, test_mint_url,
        MockMintConnector,
    };

    #[tokio::test]
    async fn claim_reusable_quote_discovers_subsequent_payments() {
        let store = create_test_db().await;
        let connector = Arc::new(MockMintConnector::new());
        connector.enable_mint_signing();
        let wallet = create_test_wallet_with_mock(store.clone(), connector.clone()).await;
        let mut quote = test_mint_quote(test_mint_url());
        quote.payment_method = PaymentMethod::BOLT12;
        quote.amount = None;
        quote.state = MintQuoteState::Issued;
        quote.amount_paid = Amount::from(8);
        quote.amount_issued = Amount::from(8);
        let signing_key = SecretKey::generate();
        quote.secret_key = Some(signing_key.clone());
        store.add_mint_quote(quote.clone()).await.unwrap();
        let session = MintSession::from_quote(wallet.clone(), quote.clone());

        for (paid, issued) in [(20, 8), (20, 20), (27, 20)] {
            connector.set_mint_quote_status_response(
                &quote.id,
                MintQuoteResponse::Bolt12(MintQuoteBolt12Response {
                    quote: quote.id.clone(),
                    request: quote.request.clone(),
                    amount: None,
                    unit: quote.unit.clone(),
                    method: PaymentMethod::BOLT12,
                    expiry: Some(quote.expiry),
                    pubkey: signing_key.public_key(),
                    amount_paid: Amount::from(paid),
                    amount_issued: Amount::from(issued),
                    updated_at: 0,
                }),
            );
            assert_eq!(session.claim().await.unwrap().amount, Amount::from(paid));
        }
        assert_eq!(wallet.balance().await.unwrap().available, Amount::from(19));
        assert_eq!(connector.post_mint_requests.lock().unwrap().len(), 2);
    }
}
