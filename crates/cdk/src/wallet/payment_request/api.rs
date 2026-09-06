//! NUT-18 payment plans and receiver handles.

use std::fmt;
#[cfg(feature = "nostr")]
use std::str::FromStr;
#[cfg(feature = "nostr")]
use std::time::Duration;

use crate::mint_url::MintUrl;
use crate::nuts::{CurrencyUnit, PublicKey};
use crate::wallet::operation::{OperationId, OperationKind, OperationReference, OperationState};
use crate::wallet::{
    CreateRequestParams, PayRequestOptions, PreparedPaymentRequest, Wallet, WalletIdentity,
    WalletManager,
};
use crate::{Amount, Error};

/// Limits enforced while preparing a NUT-18 request payment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestPaymentLimits {
    /// Maximum receiver-selected method fee.
    pub maximum_method_fee: Option<Amount>,
    /// Maximum total wallet debit, including method and mint input fees.
    pub maximum_total_amount: Option<Amount>,
}

/// Request to pay a NUT-18 Cashu payment request.
#[derive(Clone)]
pub struct RequestPayment {
    /// Receiver-provided protocol payment request.
    pub payment_request: cdk_common::PaymentRequest,
    /// Amount used when the receiver left the amount open.
    pub amount: Option<Amount>,
    /// Specific mint to use in a multi-mint manager, or automatic selection.
    pub mint: Option<MintUrl>,
    /// Fee and total-debit limits enforced before a plan is returned.
    pub limits: RequestPaymentLimits,
}

impl fmt::Debug for RequestPayment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestPayment")
            .field("payment_request", &"[REDACTED]")
            .field("amount", &self.amount)
            .field("mint", &self.mint)
            .field("limits", &self.limits)
            .finish()
    }
}

impl RequestPayment {
    /// Pay a fixed-amount request using automatic mint selection.
    pub fn new(payment_request: cdk_common::PaymentRequest) -> Self {
        Self {
            payment_request,
            amount: None,
            mint: None,
            limits: RequestPaymentLimits::default(),
        }
    }

    /// Supply an amount for an open-amount request.
    pub fn with_amount(mut self, amount: Amount) -> Self {
        self.amount = Some(amount);
        self
    }

    /// Require a specific mint when planning through a wallet manager.
    pub fn with_mint(mut self, mint_url: MintUrl) -> Self {
        self.mint = Some(mint_url);
        self
    }

    /// Enforce method-fee and total-debit limits during planning.
    pub fn with_limits(mut self, limits: RequestPaymentLimits) -> Self {
        self.limits = limits;
        self
    }
}

/// Receipt for a NUT-18 token payment delivered to its advertised transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestPaymentReceipt {
    /// Durable send operation used for the delivered token.
    pub operation_id: OperationId,
    /// Wallet selected for the payment.
    pub wallet: WalletIdentity,
    /// Receiver-requested value before method and input fees.
    pub requested_amount: Amount,
    /// Total value debited from the wallet.
    pub total_amount: Amount,
}

/// Reviewable NUT-18 payment with funds already reserved.
#[derive(Clone)]
#[must_use = "execute or cancel the plan to release its reserved funds"]
pub struct RequestPaymentPlan {
    prepared: PreparedPaymentRequest,
    operation_id: OperationId,
    wallet: WalletIdentity,
    requested_amount: Amount,
    method: Option<String>,
    method_fee: Amount,
    payment_amount: Amount,
    input_fee: Amount,
    total_amount: Amount,
}

impl fmt::Debug for RequestPaymentPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestPaymentPlan")
            .field("operation_id", &self.operation_id)
            .field("wallet", &self.wallet)
            .field("requested_amount", &self.requested_amount)
            .field("method", &self.method)
            .field("method_fee", &self.method_fee)
            .field("payment_amount", &self.payment_amount)
            .field("input_fee", &self.input_fee)
            .field("total_amount", &self.total_amount)
            .finish_non_exhaustive()
    }
}

impl RequestPaymentPlan {
    fn from_prepared(prepared: PreparedPaymentRequest) -> Self {
        Self {
            operation_id: prepared.operation_id().into(),
            wallet: WalletIdentity::new(prepared.mint_url().clone(), prepared.unit().clone()),
            requested_amount: prepared.requested_amount(),
            method: prepared.method().map(str::to_owned),
            method_fee: prepared.method_fee(),
            payment_amount: prepared.payment_amount(),
            input_fee: prepared.input_fee(),
            total_amount: prepared.total_amount(),
            prepared,
        }
    }

    /// Durable send operation identifier.
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    /// Wallet selected for this payment.
    pub fn wallet(&self) -> &WalletIdentity {
        &self.wallet
    }

    /// Receiver-requested value before fees.
    pub const fn requested_amount(&self) -> Amount {
        self.requested_amount
    }

    /// Receiver-selected method, when the request restricted methods.
    pub fn method(&self) -> Option<&str> {
        self.method.as_deref()
    }

    /// Receiver-selected method fee.
    pub const fn method_fee(&self) -> Amount {
        self.method_fee
    }

    /// Requested value plus the receiver-selected method fee.
    pub const fn payment_amount(&self) -> Amount {
        self.payment_amount
    }

    /// Mint input fees charged to construct the delivered token.
    pub const fn input_fee(&self) -> Amount {
        self.input_fee
    }

    /// Total value reserved and debited when confirmed.
    pub const fn total_amount(&self) -> Amount {
        self.total_amount
    }

    /// Create and deliver the token to the request's selected transport.
    pub async fn execute(&self) -> Result<RequestPaymentReceipt, Error> {
        self.prepared.confirm().await?;
        let receipt = RequestPaymentReceipt {
            operation_id: self.operation_id,
            wallet: self.wallet.clone(),
            requested_amount: self.requested_amount,
            total_amount: self.total_amount,
        };
        self.prepared.wallet().publish_operation_event(
            OperationReference::Workflow(self.operation_id),
            OperationKind::Send,
            OperationState::Pending,
            Some(self.requested_amount),
        );
        self.prepared.wallet().publish_balance_event().await;
        self.prepared
            .wallet()
            .publish_transaction_events(self.operation_id.as_uuid())
            .await;
        Ok(receipt)
    }

    /// Cancel the plan and release its reserved funds.
    pub async fn cancel(&self) -> Result<(), Error> {
        self.prepared.cancel().await?;
        self.prepared.wallet().publish_operation_event(
            OperationReference::Workflow(self.operation_id),
            OperationKind::Send,
            OperationState::Canceled,
            Some(self.requested_amount),
        );
        self.prepared.wallet().publish_balance_event().await;
        Ok(())
    }
}

/// Receiver-side spending lock advertised by a NUT-18 request.
#[derive(Clone)]
pub enum PaymentRequestLock {
    /// Require signatures from the provided P2PK keys.
    P2pk {
        /// Accepted signing keys.
        public_keys: Vec<PublicKey>,
        /// Required signatures.
        signatures_required: u64,
    },
    /// Require an HTLC hash, optionally together with P2PK signatures.
    HtlcHash {
        /// SHA-256 hash encoded as hexadecimal.
        hash: String,
        /// Optional accepted signing keys.
        public_keys: Vec<PublicKey>,
        /// Required signatures when keys are present.
        signatures_required: u64,
    },
    /// Derive an HTLC from a preimage, optionally together with P2PK signatures.
    HtlcPreimage {
        /// Secret preimage. Debug output is always redacted.
        preimage: String,
        /// Optional accepted signing keys.
        public_keys: Vec<PublicKey>,
        /// Required signatures when keys are present.
        signatures_required: u64,
    },
}

impl fmt::Debug for PaymentRequestLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::P2pk {
                public_keys,
                signatures_required,
            } => f
                .debug_struct("P2pk")
                .field("public_keys", public_keys)
                .field("signatures_required", signatures_required)
                .finish(),
            Self::HtlcHash {
                hash,
                public_keys,
                signatures_required,
            } => f
                .debug_struct("HtlcHash")
                .field("hash", hash)
                .field("public_keys", public_keys)
                .field("signatures_required", signatures_required)
                .finish(),
            Self::HtlcPreimage {
                public_keys,
                signatures_required,
                ..
            } => f
                .debug_struct("HtlcPreimage")
                .field("preimage", &"[REDACTED]")
                .field("public_keys", public_keys)
                .field("signatures_required", signatures_required)
                .finish(),
        }
    }
}

/// Delivery transport advertised by a receiver-created NUT-18 request.
#[derive(Debug, Clone, Default)]
pub enum PaymentRequestTransport {
    /// Deliver the encoded request out of band.
    #[default]
    OutOfBand,
    /// POST the payment payload to an HTTP endpoint.
    Http(url::Url),
    /// Deliver through Nostr gift wrapping on these relays.
    #[cfg(feature = "nostr")]
    Nostr(Vec<String>),
}

/// How a receiver's mint list constrains payer selection.
#[derive(Debug, Clone, Default)]
pub enum PaymentRequestMintPolicy {
    /// Accept any mint.
    #[default]
    Any,
    /// Accept only the listed mints.
    Strict(Vec<MintUrl>),
    /// Prefer the listed mints but allow another compatible mint.
    Preferred(Vec<MintUrl>),
}

/// Request to create a receiver-side NUT-18 payment request.
#[derive(Debug, Clone)]
pub struct CreatePaymentRequest {
    /// Requested value, or `None` to let the payer choose.
    pub amount: Option<Amount>,
    /// Currency unit requested from the payer.
    pub unit: CurrencyUnit,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Optional receiver-enforced spending lock.
    pub lock: Option<PaymentRequestLock>,
    /// Delivery transport.
    pub transport: PaymentRequestTransport,
    /// Accepted or preferred mints.
    pub mint_policy: PaymentRequestMintPolicy,
    /// Payment rails the payer's mint must support.
    pub supported_methods: Vec<cdk_common::SupportedMethod>,
}

impl CreatePaymentRequest {
    /// Create an out-of-band request accepting any mint.
    pub fn new(unit: CurrencyUnit) -> Self {
        Self {
            amount: None,
            unit,
            description: None,
            lock: None,
            transport: PaymentRequestTransport::default(),
            mint_policy: PaymentRequestMintPolicy::default(),
            supported_methods: Vec::new(),
        }
    }
}

impl From<CreatePaymentRequest> for CreateRequestParams {
    fn from(request: CreatePaymentRequest) -> Self {
        let (pubkeys, num_sigs, hash, preimage) = match request.lock {
            None => (None, 1, None, None),
            Some(PaymentRequestLock::P2pk {
                public_keys,
                signatures_required,
            }) => (
                Some(public_keys.into_iter().map(|key| key.to_hex()).collect()),
                signatures_required,
                None,
                None,
            ),
            Some(PaymentRequestLock::HtlcHash {
                hash,
                public_keys,
                signatures_required,
            }) => (
                (!public_keys.is_empty())
                    .then(|| public_keys.into_iter().map(|key| key.to_hex()).collect()),
                signatures_required,
                Some(hash),
                None,
            ),
            Some(PaymentRequestLock::HtlcPreimage {
                preimage,
                public_keys,
                signatures_required,
            }) => (
                (!public_keys.is_empty())
                    .then(|| public_keys.into_iter().map(|key| key.to_hex()).collect()),
                signatures_required,
                None,
                Some(preimage),
            ),
        };
        let (transport, http_url, nostr_relays) = match request.transport {
            PaymentRequestTransport::OutOfBand => ("none".to_owned(), None, None),
            PaymentRequestTransport::Http(url) => ("http".to_owned(), Some(url.to_string()), None),
            #[cfg(feature = "nostr")]
            PaymentRequestTransport::Nostr(relays) => ("nostr".to_owned(), None, Some(relays)),
        };
        let (mints, mint_preferred) = match request.mint_policy {
            PaymentRequestMintPolicy::Any => (None, None),
            PaymentRequestMintPolicy::Strict(mints) => (
                Some(mints.into_iter().map(|mint| mint.to_string()).collect()),
                Some(false),
            ),
            PaymentRequestMintPolicy::Preferred(mints) => (
                Some(mints.into_iter().map(|mint| mint.to_string()).collect()),
                Some(true),
            ),
        };

        Self {
            amount: request.amount.map(Into::into),
            unit: request.unit.to_string(),
            description: request.description,
            pubkeys,
            num_sigs,
            hash,
            preimage,
            transport,
            http_url,
            nostr_relays,
            mints,
            mint_preferred,
            supported_methods: request.supported_methods,
        }
    }
}

/// Handle that waits for a Nostr-delivered request payment and redeems it.
#[derive(Clone)]
pub struct PaymentRequestReceiver {
    #[cfg(feature = "nostr")]
    manager: WalletManager,
    #[cfg(feature = "nostr")]
    info: crate::wallet::payment_request::NostrWaitInfo,
}

/// Persistable listener state for a Nostr-delivered payment request.
#[cfg(feature = "nostr")]
#[derive(Clone)]
pub struct PaymentRequestReceiverState {
    /// Secret key required to unwrap gift-wrapped payments.
    pub secret_key_hex: String,
    /// Relays carrying request payments.
    pub relays: Vec<String>,
    /// Public key used by the listener filter.
    pub public_key_hex: String,
    /// Mints accepted or preferred by the request.
    pub mints: Vec<MintUrl>,
    /// Whether unlisted mints remain acceptable.
    pub mint_preferred: Option<bool>,
}

#[cfg(feature = "nostr")]
impl fmt::Debug for PaymentRequestReceiverState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentRequestReceiverState")
            .field("secret_key_hex", &"[REDACTED]")
            .field("relays", &self.relays)
            .field("public_key_hex", &self.public_key_hex)
            .field("mints", &self.mints)
            .field("mint_preferred", &self.mint_preferred)
            .finish()
    }
}

impl fmt::Debug for PaymentRequestReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentRequestReceiver")
            .finish_non_exhaustive()
    }
}

impl PaymentRequestReceiver {
    /// Export the listener credentials for durable application storage.
    #[cfg(feature = "nostr")]
    pub fn state(&self) -> PaymentRequestReceiverState {
        PaymentRequestReceiverState {
            secret_key_hex: self.info.keys.secret_key().to_secret_hex(),
            relays: self.info.relays.clone(),
            public_key_hex: self.info.pubkey.to_hex(),
            mints: self.info.mints.clone(),
            mint_preferred: self.info.mint_preferred,
        }
    }

    /// Wait for the first matching Nostr payment and redeem it.
    #[cfg(feature = "nostr")]
    pub async fn receive(&self) -> Result<Amount, Error> {
        self.manager.wait_for_nostr_payment(self.info.clone()).await
    }

    /// Wait up to `timeout` for a matching payment.
    ///
    /// Returns `None` when the deadline elapses. If redemption already started,
    /// it may have changed durable state; use [`Wallet::synchronize`] to
    /// reconcile an interrupted receive before retrying.
    #[cfg(feature = "nostr")]
    pub async fn receive_with_timeout(&self, timeout: Duration) -> Result<Option<Amount>, Error> {
        match tokio::time::timeout(timeout, self.receive()).await {
            Ok(result) => result.map(Some),
            Err(_) => Ok(None),
        }
    }
}

/// Receiver-created NUT-18 request and its optional transport listener.
#[derive(Clone)]
pub struct CreatedPaymentRequest {
    /// Encoded protocol request to present to the payer.
    pub payment_request: cdk_common::PaymentRequest,
    /// Listener required for Nostr transport; absent for out-of-band or HTTP requests.
    pub receiver: Option<PaymentRequestReceiver>,
}

impl fmt::Debug for CreatedPaymentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedPaymentRequest")
            .field("payment_request", &"[REDACTED]")
            .field("has_receiver", &self.receiver.is_some())
            .finish()
    }
}

async fn request_payment_plan(
    prepared: PreparedPaymentRequest,
    limits: RequestPaymentLimits,
) -> Result<RequestPaymentPlan, Error> {
    let limit_result = prepared.check_limits(PayRequestOptions {
        max_method_fee: limits.maximum_method_fee,
        max_total_amount: limits.maximum_total_amount,
    });
    if let Err(error) = limit_result {
        if let Err(cancel_error) = prepared.cancel().await {
            tracing::warn!(
                "Could not cancel request-payment plan after a limit check failed: {}",
                cancel_error
            );
        }
        return Err(error);
    }
    Ok(RequestPaymentPlan::from_prepared(prepared))
}

impl Wallet {
    /// Select and reserve funds for a NUT-18 payment request.
    pub async fn plan_request_payment(
        &self,
        request: RequestPayment,
    ) -> Result<RequestPaymentPlan, Error> {
        if request
            .mint
            .as_ref()
            .is_some_and(|mint_url| mint_url != &self.mint_url)
        {
            return Err(Error::IncorrectMint);
        }
        let prepared = self
            .prepare_pay_request(request.payment_request, request.amount)
            .await?;
        let plan = request_payment_plan(prepared, request.limits).await?;
        self.publish_operation_event(
            OperationReference::Workflow(plan.operation_id()),
            OperationKind::Send,
            OperationState::AwaitingExecution,
            Some(plan.requested_amount()),
        );
        self.publish_balance_event().await;
        Ok(plan)
    }

    /// Prepare and execute a NUT-18 request payment.
    pub async fn pay_request(
        &self,
        request: RequestPayment,
    ) -> Result<RequestPaymentReceipt, Error> {
        self.plan_request_payment(request).await?.execute().await
    }
}

impl WalletManager {
    /// Select a compatible wallet and reserve funds for a NUT-18 payment request.
    pub async fn plan_request_payment(
        &self,
        request: RequestPayment,
    ) -> Result<RequestPaymentPlan, Error> {
        let prepared = self
            .prepare_pay_request(request.payment_request, request.mint, request.amount)
            .await?;
        let plan = request_payment_plan(prepared, request.limits).await?;
        plan.prepared.wallet().publish_operation_event(
            OperationReference::Workflow(plan.operation_id()),
            OperationKind::Send,
            OperationState::AwaitingExecution,
            Some(plan.requested_amount()),
        );
        plan.prepared.wallet().publish_balance_event().await;
        Ok(plan)
    }

    /// Select a wallet, prepare, and execute a NUT-18 request payment.
    pub async fn pay_request(
        &self,
        request: RequestPayment,
    ) -> Result<RequestPaymentReceipt, Error> {
        self.plan_request_payment(request).await?.execute().await
    }

    /// Create a receiver-side NUT-18 request and optional Nostr listener.
    pub async fn create_payment_request(
        &self,
        request: CreatePaymentRequest,
    ) -> Result<CreatedPaymentRequest, Error> {
        let params = request.into();
        #[cfg(feature = "nostr")]
        {
            let (payment_request, receiver) = self.create_request(params).await?;
            Ok(CreatedPaymentRequest {
                payment_request,
                receiver: receiver.map(|info| PaymentRequestReceiver {
                    manager: self.clone(),
                    info,
                }),
            })
        }
        #[cfg(not(feature = "nostr"))]
        {
            Ok(CreatedPaymentRequest {
                payment_request: self.create_request(params).await?,
                receiver: None,
            })
        }
    }

    /// Restore a Nostr payment-request receiver from previously exported state.
    #[cfg(feature = "nostr")]
    pub fn resume_payment_request_receiver(
        &self,
        state: PaymentRequestReceiverState,
    ) -> Result<PaymentRequestReceiver, Error> {
        let keys = nostr_sdk::Keys::parse(&state.secret_key_hex)
            .map_err(|error| Error::Custom(format!("Invalid receiver secret key: {error}")))?;
        let public_key = nostr_sdk::PublicKey::from_str(&state.public_key_hex)
            .map_err(|error| Error::Custom(format!("Invalid receiver public key: {error}")))?;
        if keys.public_key != public_key {
            return Err(Error::Custom(
                "Payment-request receiver keys do not match".to_owned(),
            ));
        }

        Ok(PaymentRequestReceiver {
            manager: self.clone(),
            info: crate::wallet::payment_request::NostrWaitInfo {
                keys,
                relays: state.relays,
                pubkey: public_key,
                mints: state.mints,
                mint_preferred: state.mint_preferred,
            },
        })
    }
}
