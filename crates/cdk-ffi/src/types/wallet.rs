//! Wallet-related FFI types

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::amount::{Amount, SplitTarget};
use super::proof::{Proofs, SpendingConditions};
use crate::error::FfiError;
use crate::token::Token;
use crate::{CurrencyUnit, MintUrl};

/// FFI-compatible SendMemo
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMemo {
    /// Memo text
    pub memo: String,
    /// Include memo in token
    pub include_memo: bool,
}

impl From<SendMemo> for cdk::wallet::SendMemo {
    fn from(memo: SendMemo) -> Self {
        cdk::wallet::SendMemo {
            memo: memo.memo,
            include_memo: memo.include_memo,
        }
    }
}

impl From<cdk::wallet::SendMemo> for SendMemo {
    fn from(memo: cdk::wallet::SendMemo) -> Self {
        Self {
            memo: memo.memo,
            include_memo: memo.include_memo,
        }
    }
}

impl SendMemo {
    /// Convert SendMemo to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode SendMemo from JSON string
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn decode_send_memo(json: String) -> Result<SendMemo, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode SendMemo to JSON string
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn encode_send_memo(memo: SendMemo) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&memo)?)
}

/// FFI-compatible SendKind
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SendKind {
    /// Allow online swap before send if wallet does not have exact amount
    OnlineExact,
    /// Prefer offline send if difference is less than tolerance
    OnlineTolerance { tolerance: Amount },
    /// Wallet cannot do an online swap and selected proof must be exactly send amount
    OfflineExact,
    /// Wallet must remain offline but can over pay if below tolerance
    OfflineTolerance { tolerance: Amount },
}

impl From<SendKind> for cdk::wallet::SendKind {
    fn from(kind: SendKind) -> Self {
        match kind {
            SendKind::OnlineExact => cdk::wallet::SendKind::OnlineExact,
            SendKind::OnlineTolerance { tolerance } => {
                cdk::wallet::SendKind::OnlineTolerance(tolerance.into())
            }
            SendKind::OfflineExact => cdk::wallet::SendKind::OfflineExact,
            SendKind::OfflineTolerance { tolerance } => {
                cdk::wallet::SendKind::OfflineTolerance(tolerance.into())
            }
        }
    }
}

impl From<cdk::wallet::SendKind> for SendKind {
    fn from(kind: cdk::wallet::SendKind) -> Self {
        match kind {
            cdk::wallet::SendKind::OnlineExact => SendKind::OnlineExact,
            cdk::wallet::SendKind::OnlineTolerance(tolerance) => SendKind::OnlineTolerance {
                tolerance: tolerance.into(),
            },
            cdk::wallet::SendKind::OfflineExact => SendKind::OfflineExact,
            cdk::wallet::SendKind::OfflineTolerance(tolerance) => SendKind::OfflineTolerance {
                tolerance: tolerance.into(),
            },
        }
    }
}

/// FFI-compatible Send options
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendOptions {
    /// Memo
    pub memo: Option<SendMemo>,
    /// Spending conditions
    pub conditions: Option<SpendingConditions>,
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// Send kind
    pub send_kind: SendKind,
    /// Include fee
    pub include_fee: bool,
    /// Maximum number of proofs to include in the token
    pub max_proofs: Option<u32>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            memo: None,
            conditions: None,
            amount_split_target: SplitTarget::None,
            send_kind: SendKind::OnlineExact,
            include_fee: false,
            max_proofs: None,
            metadata: HashMap::new(),
        }
    }
}

impl From<SendOptions> for cdk::wallet::SendOptions {
    fn from(opts: SendOptions) -> Self {
        cdk::wallet::SendOptions {
            memo: opts.memo.map(Into::into),
            conditions: opts.conditions.and_then(|c| c.try_into().ok()),
            amount_split_target: opts.amount_split_target.into(),
            send_kind: opts.send_kind.into(),
            include_fee: opts.include_fee,
            max_proofs: opts.max_proofs.map(|p| p as usize),
            metadata: opts.metadata,
        }
    }
}

impl From<cdk::wallet::SendOptions> for SendOptions {
    fn from(opts: cdk::wallet::SendOptions) -> Self {
        Self {
            memo: opts.memo.map(Into::into),
            conditions: opts.conditions.map(Into::into),
            amount_split_target: opts.amount_split_target.into(),
            send_kind: opts.send_kind.into(),
            include_fee: opts.include_fee,
            max_proofs: opts.max_proofs.map(|p| p as u32),
            metadata: opts.metadata,
        }
    }
}

impl SendOptions {
    /// Convert SendOptions to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode SendOptions from JSON string
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn decode_send_options(json: String) -> Result<SendOptions, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode SendOptions to JSON string
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn encode_send_options(options: SendOptions) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&options)?)
}

/// FFI-compatible SecretKey
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretKey {
    /// Hex-encoded secret key (64 characters)
    pub hex: String,
}

impl SecretKey {
    /// Create a new SecretKey from hex string
    pub fn from_hex(hex: String) -> Result<Self, FfiError> {
        // Validate hex string length (should be 64 characters for 32 bytes)
        if hex.len() != 64 {
            return Err(FfiError::internal(
                "Secret key hex must be exactly 64 characters (32 bytes)",
            ));
        }

        // Validate hex format
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(FfiError::internal(
                "Secret key hex contains invalid characters",
            ));
        }

        Ok(Self { hex })
    }

    /// Generate a random secret key
    pub fn random() -> Self {
        use cdk::nuts::SecretKey as CdkSecretKey;
        let secret_key = CdkSecretKey::generate();
        Self {
            hex: secret_key.to_secret_hex(),
        }
    }
}

impl From<SecretKey> for cdk::nuts::SecretKey {
    fn from(key: SecretKey) -> Self {
        // This will panic if hex is invalid, but we validate in from_hex()
        cdk::nuts::SecretKey::from_hex(&key.hex).expect("Invalid secret key hex")
    }
}

impl From<cdk::nuts::SecretKey> for SecretKey {
    fn from(key: cdk::nuts::SecretKey) -> Self {
        Self {
            hex: key.to_secret_hex(),
        }
    }
}

/// FFI-compatible Receive options
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiveOptions {
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// P2PK signing keys
    pub p2pk_signing_keys: Vec<SecretKey>,
    /// Preimages for HTLC conditions
    pub preimages: Vec<String>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

impl Default for ReceiveOptions {
    fn default() -> Self {
        Self {
            amount_split_target: SplitTarget::None,
            p2pk_signing_keys: Vec::new(),
            preimages: Vec::new(),
            metadata: HashMap::new(),
        }
    }
}

impl From<ReceiveOptions> for cdk::wallet::ReceiveOptions {
    fn from(opts: ReceiveOptions) -> Self {
        cdk::wallet::ReceiveOptions {
            amount_split_target: opts.amount_split_target.into(),
            p2pk_signing_keys: opts.p2pk_signing_keys.into_iter().map(Into::into).collect(),
            preimages: opts.preimages,
            metadata: opts.metadata,
        }
    }
}

impl From<cdk::wallet::ReceiveOptions> for ReceiveOptions {
    fn from(opts: cdk::wallet::ReceiveOptions) -> Self {
        Self {
            amount_split_target: opts.amount_split_target.into(),
            p2pk_signing_keys: opts.p2pk_signing_keys.into_iter().map(Into::into).collect(),
            preimages: opts.preimages,
            metadata: opts.metadata,
        }
    }
}

impl ReceiveOptions {
    /// Convert ReceiveOptions to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode ReceiveOptions from JSON string
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn decode_receive_options(json: String) -> Result<ReceiveOptions, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode ReceiveOptions to JSON string
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn encode_receive_options(options: ReceiveOptions) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&options)?)
}

/// FFI-compatible PreparedSend
///
/// This wraps the data from a prepared send operation along with a reference
/// to the wallet. The actual PreparedSend<'a> from cdk has a lifetime parameter
/// that doesn't work with FFI, so we store the wallet and cached data separately.
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Object))]
pub struct PreparedSend {
    wallet: std::sync::Arc<cdk::Wallet>,
    operation_id: uuid::Uuid,
    amount: Amount,
    options: cdk::wallet::SendOptions,
    proofs_to_swap: cdk::nuts::Proofs,
    proofs_to_send: cdk::nuts::Proofs,
    swap_fee: Amount,
    send_fee: Amount,
}

impl std::fmt::Debug for PreparedSend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSend")
            .field("operation_id", &self.operation_id)
            .field("amount", &self.amount)
            .finish()
    }
}

impl PreparedSend {
    /// Create a new FFI PreparedSend from a cdk::wallet::PreparedSend and wallet
    pub fn new(
        wallet: std::sync::Arc<cdk::Wallet>,
        prepared: &cdk::wallet::PreparedSend<'_>,
    ) -> Self {
        Self {
            wallet,
            operation_id: prepared.operation_id(),
            amount: prepared.amount().into(),
            options: prepared.options().clone(),
            proofs_to_swap: prepared.proofs_to_swap().clone(),
            proofs_to_send: prepared.proofs_to_send().clone(),
            swap_fee: prepared.swap_fee().into(),
            send_fee: prepared.send_fee().into(),
        }
    }
}

#[cfg_attr(feature = "uniffi-bindings", uniffi::export(async_runtime = "tokio"))]
impl PreparedSend {
    /// Get the operation ID for this prepared send
    pub fn operation_id(&self) -> String {
        self.operation_id.to_string()
    }

    /// Get the amount to send
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Get the proofs that will be used
    pub fn proofs(&self) -> Proofs {
        let mut all_proofs: Vec<_> = self
            .proofs_to_swap
            .iter()
            .cloned()
            .map(|p| p.into())
            .collect();
        all_proofs.extend(self.proofs_to_send.iter().cloned().map(|p| p.into()));
        all_proofs
    }

    /// Get the total fee for this send operation
    pub fn fee(&self) -> Amount {
        Amount::new(self.swap_fee.value + self.send_fee.value)
    }

    /// Confirm the prepared send and create a token
    pub async fn confirm(
        self: std::sync::Arc<Self>,
        memo: Option<String>,
    ) -> Result<Token, FfiError> {
        let send_memo = memo.map(|m| cdk::wallet::SendMemo::for_token(&m));
        let token = self
            .wallet
            .confirm_send(
                self.operation_id,
                self.amount.into(),
                self.options.clone(),
                self.proofs_to_swap.clone(),
                self.proofs_to_send.clone(),
                self.swap_fee.into(),
                self.send_fee.into(),
                send_memo,
            )
            .await?;

        Ok(token.into())
    }

    /// Cancel the prepared send operation
    pub async fn cancel(self: std::sync::Arc<Self>) -> Result<(), FfiError> {
        self.wallet
            .cancel_send(
                self.operation_id,
                self.proofs_to_swap.clone(),
                self.proofs_to_send.clone(),
            )
            .await?;
        Ok(())
    }
}

/// FFI-compatible FinalizedMelt result
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FinalizedMelt {
    pub quote_id: String,
    pub state: super::quote::QuoteState,
    pub preimage: Option<String>,
    pub change: Option<Proofs>,
    pub amount: Amount,
    pub fee_paid: Amount,
}

impl From<cdk_common::common::FinalizedMelt> for FinalizedMelt {
    fn from(finalized: cdk_common::common::FinalizedMelt) -> Self {
        Self {
            quote_id: finalized.quote_id().to_string(),
            state: finalized.state().into(),
            preimage: finalized.payment_proof().map(|s: &str| s.to_string()),
            change: finalized
                .change()
                .map(|proofs| proofs.iter().cloned().map(|p| p.into()).collect()),
            amount: finalized.amount().into(),
            fee_paid: finalized.fee_paid().into(),
        }
    }
}

/// FFI-compatible PreparedMelt
///
/// This wraps the data from a prepared melt operation along with a reference
/// to the wallet. The actual PreparedMelt<'a> from cdk has a lifetime parameter
/// that doesn't work with FFI, so we store the wallet and cached data separately.
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Object))]
pub struct PreparedMelt {
    wallet: std::sync::Arc<cdk::Wallet>,
    operation_id: uuid::Uuid,
    quote: cdk_common::wallet::MeltQuote,
    proofs: cdk::nuts::Proofs,
    proofs_to_swap: cdk::nuts::Proofs,
    swap_fee: Amount,
    input_fee: Amount,
    input_fee_without_swap: Amount,
    metadata: HashMap<String, String>,
}

impl std::fmt::Debug for PreparedMelt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedMelt")
            .field("operation_id", &self.operation_id)
            .field("quote_id", &self.quote.id)
            .field("amount", &self.quote.amount)
            .finish()
    }
}

impl PreparedMelt {
    /// Create a new FFI PreparedMelt from a cdk::wallet::PreparedMelt and wallet
    pub fn new(
        wallet: std::sync::Arc<cdk::Wallet>,
        prepared: &cdk::wallet::PreparedMelt<'_>,
    ) -> Self {
        Self {
            wallet,
            operation_id: prepared.operation_id(),
            quote: prepared.quote().clone(),
            proofs: prepared.proofs().clone(),
            proofs_to_swap: prepared.proofs_to_swap().clone(),
            swap_fee: prepared.swap_fee().into(),
            input_fee: prepared.input_fee().into(),
            input_fee_without_swap: prepared.input_fee_without_swap().into(),
            metadata: HashMap::new(),
        }
    }
}

#[cfg_attr(feature = "uniffi-bindings", uniffi::export(async_runtime = "tokio"))]
impl PreparedMelt {
    /// Get the operation ID for this prepared melt
    pub fn operation_id(&self) -> String {
        self.operation_id.to_string()
    }

    /// Get the quote ID
    pub fn quote_id(&self) -> String {
        self.quote.id.clone()
    }

    /// Get the amount to be melted
    pub fn amount(&self) -> Amount {
        self.quote.amount.into()
    }

    /// Get the fee reserve from the quote
    pub fn fee_reserve(&self) -> Amount {
        self.quote.fee_reserve.into()
    }

    /// Get the swap fee
    pub fn swap_fee(&self) -> Amount {
        self.swap_fee
    }

    /// Get the input fee
    pub fn input_fee(&self) -> Amount {
        self.input_fee
    }

    /// Get the total fee (swap fee + input fee)
    pub fn total_fee(&self) -> Amount {
        Amount::new(self.swap_fee.value + self.input_fee.value)
    }

    /// Returns true if a swap would be performed (proofs_to_swap is not empty)
    pub fn requires_swap(&self) -> bool {
        !self.proofs_to_swap.is_empty()
    }

    /// Get the total fee if swap is performed (current default behavior)
    pub fn total_fee_with_swap(&self) -> Amount {
        Amount::new(self.swap_fee.value + self.input_fee.value)
    }

    /// Get the input fee if swap is skipped (fee on all proofs sent directly)
    pub fn input_fee_without_swap(&self) -> Amount {
        self.input_fee_without_swap
    }

    /// Get the fee savings from skipping the swap
    pub fn fee_savings_without_swap(&self) -> Amount {
        let total_with = self.swap_fee.value + self.input_fee.value;
        let total_without = self.input_fee_without_swap.value;
        if total_with > total_without {
            Amount::new(total_with - total_without)
        } else {
            Amount::new(0)
        }
    }

    /// Get the expected change amount if swap is skipped
    pub fn change_amount_without_swap(&self) -> Amount {
        use cdk::nuts::nut00::ProofsMethods;
        let all_proofs_total = self.proofs.total_amount().unwrap_or(cdk::Amount::ZERO)
            + self
                .proofs_to_swap
                .total_amount()
                .unwrap_or(cdk::Amount::ZERO);
        let needed =
            self.quote.amount + self.quote.fee_reserve + self.input_fee_without_swap.into();
        all_proofs_total
            .checked_sub(needed)
            .map(|a| a.into())
            .unwrap_or(Amount::new(0))
    }

    /// Get the proofs that will be used
    pub fn proofs(&self) -> Proofs {
        self.proofs.iter().cloned().map(|p| p.into()).collect()
    }

    /// Confirm the prepared melt and execute the payment
    pub async fn confirm(&self) -> Result<FinalizedMelt, FfiError> {
        self.confirm_with_options(MeltConfirmOptions::default())
            .await
    }

    /// Confirm the prepared melt with custom options
    pub async fn confirm_with_options(
        &self,
        options: MeltConfirmOptions,
    ) -> Result<FinalizedMelt, FfiError> {
        let finalized = self
            .wallet
            .confirm_prepared_melt_with_options(
                self.operation_id,
                self.quote.clone(),
                self.proofs.clone(),
                self.proofs_to_swap.clone(),
                self.input_fee.into(),
                self.input_fee_without_swap.into(),
                self.metadata.clone(),
                options.into(),
            )
            .await?;

        Ok(finalized.into())
    }

    /// Cancel the prepared melt and release reserved proofs
    pub async fn cancel(&self) -> Result<(), FfiError> {
        self.wallet
            .cancel_prepared_melt(
                self.operation_id,
                self.proofs.clone(),
                self.proofs_to_swap.clone(),
            )
            .await?;
        Ok(())
    }
}

/// FFI-compatible MeltOptions
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MeltOptions {
    /// MPP (Multi-Part Payments) options
    Mpp { amount: Amount },
    /// Amountless options
    Amountless { amount_msat: Amount },
}

impl From<MeltOptions> for cdk::nuts::MeltOptions {
    fn from(opts: MeltOptions) -> Self {
        match opts {
            MeltOptions::Mpp { amount } => {
                let cdk_amount: cdk::Amount = amount.into();
                cdk::nuts::MeltOptions::new_mpp(cdk_amount)
            }
            MeltOptions::Amountless { amount_msat } => {
                let cdk_amount: cdk::Amount = amount_msat.into();
                cdk::nuts::MeltOptions::new_amountless(cdk_amount)
            }
        }
    }
}

impl From<cdk::nuts::MeltOptions> for MeltOptions {
    fn from(opts: cdk::nuts::MeltOptions) -> Self {
        match opts {
            cdk::nuts::MeltOptions::Mpp { mpp } => MeltOptions::Mpp {
                amount: mpp.amount.into(),
            },
            cdk::nuts::MeltOptions::Amountless { amountless } => MeltOptions::Amountless {
                amount_msat: amountless.amount_msat.into(),
            },
        }
    }
}

/// Restored Data
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Restored {
    pub spent: Amount,
    pub unspent: Amount,
    pub pending: Amount,
}

impl From<cdk::wallet::Restored> for Restored {
    fn from(restored: cdk::wallet::Restored) -> Self {
        Self {
            spent: restored.spent.into(),
            unspent: restored.unspent.into(),
            pending: restored.pending.into(),
        }
    }
}

/// FFI-compatible options for confirming a melt operation
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeltConfirmOptions {
    /// Skip the pre-melt swap and send proofs directly to melt.
    /// When true, saves swap input fees but gets change from melt instead.
    pub skip_swap: bool,
}

impl From<MeltConfirmOptions> for cdk::wallet::MeltConfirmOptions {
    fn from(opts: MeltConfirmOptions) -> Self {
        cdk::wallet::MeltConfirmOptions {
            skip_swap: opts.skip_swap,
        }
    }
}

impl From<cdk::wallet::MeltConfirmOptions> for MeltConfirmOptions {
    fn from(opts: cdk::wallet::MeltConfirmOptions) -> Self {
        Self {
            skip_swap: opts.skip_swap,
        }
    }
}

/// FFI-compatible WalletKey
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletKey {
    /// Mint Url
    pub mint_url: MintUrl,
    /// Currency Unit
    pub unit: CurrencyUnit,
}

impl TryFrom<WalletKey> for cdk::WalletKey {
    type Error = FfiError;

    fn try_from(value: WalletKey) -> Result<Self, Self::Error> {
        Ok(Self {
            mint_url: value.mint_url.try_into()?,
            unit: value.unit.into(),
        })
    }
}

impl From<cdk::WalletKey> for WalletKey {
    fn from(value: cdk::WalletKey) -> Self {
        Self {
            mint_url: value.mint_url.into(),
            unit: value.unit.into(),
        }
    }
}
