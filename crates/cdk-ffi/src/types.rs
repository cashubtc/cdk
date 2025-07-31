//! FFI-compatible types

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

use cdk::nuts::{CurrencyUnit as CdkCurrencyUnit, State as CdkState};
use cdk::Amount as CdkAmount;

// use cdk::Melted as CdkMelted;
use crate::error::FfiError;

/// FFI-compatible Amount type
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct Amount {
    pub value: u64,
}

impl Amount {
    pub fn new(value: u64) -> Self {
        Self { value }
    }

    pub fn zero() -> Self {
        Self { value: 0 }
    }

    pub fn is_zero(&self) -> bool {
        self.value == 0
    }

    pub fn convert_unit(
        &self,
        current_unit: CurrencyUnit,
        target_unit: CurrencyUnit,
    ) -> Result<Amount, FfiError> {
        Ok(CdkAmount::from(self.value)
            .convert_unit(&current_unit.into(), &target_unit.into())
            .map(Into::into)?)
    }
}

impl From<CdkAmount> for Amount {
    fn from(amount: CdkAmount) -> Self {
        Self {
            value: u64::from(amount),
        }
    }
}

impl From<Amount> for CdkAmount {
    fn from(amount: Amount) -> Self {
        CdkAmount::from(amount.value)
    }
}

/// FFI-compatible Currency Unit
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CurrencyUnit {
    Sat,
    Msat,
    Usd,
    Eur,
}

impl From<CdkCurrencyUnit> for CurrencyUnit {
    fn from(unit: CdkCurrencyUnit) -> Self {
        match unit {
            CdkCurrencyUnit::Sat => CurrencyUnit::Sat,
            CdkCurrencyUnit::Msat => CurrencyUnit::Msat,
            CdkCurrencyUnit::Usd => CurrencyUnit::Usd,
            CdkCurrencyUnit::Eur => CurrencyUnit::Eur,
            _ => CurrencyUnit::Sat, // default fallback
        }
    }
}

impl From<CurrencyUnit> for CdkCurrencyUnit {
    fn from(unit: CurrencyUnit) -> Self {
        match unit {
            CurrencyUnit::Sat => CdkCurrencyUnit::Sat,
            CurrencyUnit::Msat => CdkCurrencyUnit::Msat,
            CurrencyUnit::Usd => CdkCurrencyUnit::Usd,
            CurrencyUnit::Eur => CdkCurrencyUnit::Eur,
        }
    }
}

/// FFI-compatible Mint URL
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MintUrl {
    pub url: String,
}

impl MintUrl {
    pub fn new(url: String) -> Result<Self, FfiError> {
        // Validate URL format
        url::Url::parse(&url).map_err(|e| FfiError::Generic {
            msg: format!("Invalid URL: {}", e),
        })?;

        Ok(Self { url })
    }
}

impl From<cdk::mint_url::MintUrl> for MintUrl {
    fn from(mint_url: cdk::mint_url::MintUrl) -> Self {
        Self {
            url: mint_url.to_string(),
        }
    }
}

impl TryFrom<MintUrl> for cdk::mint_url::MintUrl {
    type Error = FfiError;

    fn try_from(mint_url: MintUrl) -> Result<Self, Self::Error> {
        cdk::mint_url::MintUrl::from_str(&mint_url.url)
            .map_err(|e| FfiError::Generic { msg: e.to_string() })
    }
}

/// FFI-compatible Proof state
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ProofState {
    Unspent,
    Pending,
    Spent,
    Reserved,
    PendingSpent,
}

impl From<CdkState> for ProofState {
    fn from(state: CdkState) -> Self {
        match state {
            CdkState::Unspent => ProofState::Unspent,
            CdkState::Pending => ProofState::Pending,
            CdkState::Spent => ProofState::Spent,
            CdkState::Reserved => ProofState::Reserved,
            CdkState::PendingSpent => ProofState::PendingSpent,
        }
    }
}

impl From<ProofState> for CdkState {
    fn from(state: ProofState) -> Self {
        match state {
            ProofState::Unspent => CdkState::Unspent,
            ProofState::Pending => CdkState::Pending,
            ProofState::Spent => CdkState::Spent,
            ProofState::Reserved => CdkState::Reserved,
            ProofState::PendingSpent => CdkState::PendingSpent,
        }
    }
}

/// FFI-compatible Token
#[derive(Debug, uniffi::Object)]
pub struct Token {
    pub(crate) inner: cdk::nuts::Token,
}

impl From<cdk::nuts::Token> for Token {
    fn from(token: cdk::nuts::Token) -> Self {
        Self { inner: token }
    }
}

impl From<Token> for cdk::nuts::Token {
    fn from(token: Token) -> Self {
        token.inner
    }
}

#[uniffi::export]
impl Token {
    /// Create a new Token from string
    #[uniffi::constructor]
    pub fn from_string(token_str: String) -> Result<Token, FfiError> {
        let token = cdk::nuts::Token::from_str(&token_str)
            .map_err(|e| FfiError::Generic { msg: e.to_string() })?;
        Ok(Token { inner: token })
    }

    /// Get the token as a string
    pub fn to_string(&self) -> String {
        self.inner.to_string()
    }

    /// Get the total value of the token
    pub fn value(&self) -> Result<Amount, FfiError> {
        Ok(self.inner.value()?.into())
    }

    /// Get the memo from the token
    pub fn memo(&self) -> Option<String> {
        self.inner.memo().clone()
    }

    /// Get the currency unit
    pub fn unit(&self) -> Option<CurrencyUnit> {
        self.inner.unit().map(Into::into)
    }

    /// Get the mint URL
    pub fn mint_url(&self) -> Result<MintUrl, FfiError> {
        Ok(self.inner.mint_url()?.into())
    }

    /// Get proofs from the token (simplified - no keyset filtering for now)
    pub fn proofs_simple(&self) -> Result<Proofs, FfiError> {
        // For now, return empty keysets to get all proofs
        let empty_keysets = vec![];
        let proofs = self.inner.proofs(&empty_keysets)?;
        Ok(proofs
            .into_iter()
            .map(|p| std::sync::Arc::new(p.into()))
            .collect())
    }

    /// Convert to V3 string format
    pub fn to_v3_string(&self) -> String {
        self.inner.to_v3_string()
    }
}

/// FFI-compatible SendMemo
#[derive(Debug, Clone, uniffi::Record)]
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

/// FFI-compatible SplitTarget
#[derive(Debug, Clone, uniffi::Enum)]
pub enum SplitTarget {
    /// Default target; least amount of proofs
    None,
    /// Target amount for wallet to have most proofs that add up to value
    Value { amount: Amount },
    /// Specific amounts to split into (must equal amount being split)
    Values { amounts: Vec<Amount> },
}

impl From<SplitTarget> for cdk::amount::SplitTarget {
    fn from(target: SplitTarget) -> Self {
        match target {
            SplitTarget::None => cdk::amount::SplitTarget::None,
            SplitTarget::Value { amount } => cdk::amount::SplitTarget::Value(amount.into()),
            SplitTarget::Values { amounts } => {
                cdk::amount::SplitTarget::Values(amounts.into_iter().map(Into::into).collect())
            }
        }
    }
}

impl From<cdk::amount::SplitTarget> for SplitTarget {
    fn from(target: cdk::amount::SplitTarget) -> Self {
        match target {
            cdk::amount::SplitTarget::None => SplitTarget::None,
            cdk::amount::SplitTarget::Value(amount) => SplitTarget::Value {
                amount: amount.into(),
            },
            cdk::amount::SplitTarget::Values(amounts) => SplitTarget::Values {
                amounts: amounts.into_iter().map(Into::into).collect(),
            },
        }
    }
}

/// FFI-compatible SendKind
#[derive(Debug, Clone, uniffi::Enum)]
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

impl From<SendKind> for cdk_common::wallet::SendKind {
    fn from(kind: SendKind) -> Self {
        match kind {
            SendKind::OnlineExact => cdk_common::wallet::SendKind::OnlineExact,
            SendKind::OnlineTolerance { tolerance } => {
                cdk_common::wallet::SendKind::OnlineTolerance(tolerance.into())
            }
            SendKind::OfflineExact => cdk_common::wallet::SendKind::OfflineExact,
            SendKind::OfflineTolerance { tolerance } => {
                cdk_common::wallet::SendKind::OfflineTolerance(tolerance.into())
            }
        }
    }
}

impl From<cdk_common::wallet::SendKind> for SendKind {
    fn from(kind: cdk_common::wallet::SendKind) -> Self {
        match kind {
            cdk_common::wallet::SendKind::OnlineExact => SendKind::OnlineExact,
            cdk_common::wallet::SendKind::OnlineTolerance(tolerance) => SendKind::OnlineTolerance {
                tolerance: tolerance.into(),
            },
            cdk_common::wallet::SendKind::OfflineExact => SendKind::OfflineExact,
            cdk_common::wallet::SendKind::OfflineTolerance(tolerance) => {
                SendKind::OfflineTolerance {
                    tolerance: tolerance.into(),
                }
            }
        }
    }
}

/// FFI-compatible Send options
#[derive(Debug, Clone, uniffi::Record)]
pub struct SendOptions {
    /// Memo
    pub memo: Option<SendMemo>,
    /// Spending conditions (JSON string for now - simplified)
    pub conditions: Option<String>,
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
        // Parse spending conditions from JSON string if provided
        let conditions = if let Some(cond_str) = opts.conditions {
            // For now, we'll parse this as JSON. In a full implementation,
            // you might want to create a proper FFI type for SpendingConditions
            serde_json::from_str(&cond_str).ok()
        } else {
            None
        };

        cdk::wallet::SendOptions {
            memo: opts.memo.map(Into::into),
            conditions,
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
        // Convert spending conditions to JSON string if present
        let conditions = if let Some(cond) = opts.conditions {
            serde_json::to_string(&cond).ok()
        } else {
            None
        };

        Self {
            memo: opts.memo.map(Into::into),
            conditions,
            amount_split_target: opts.amount_split_target.into(),
            send_kind: opts.send_kind.into(),
            include_fee: opts.include_fee,
            max_proofs: opts.max_proofs.map(|p| p as u32),
            metadata: opts.metadata,
        }
    }
}

/// FFI-compatible SecretKey
#[derive(Debug, Clone, uniffi::Record)]
pub struct SecretKey {
    /// Hex-encoded secret key (64 characters)
    pub hex: String,
}

impl SecretKey {
    /// Create a new SecretKey from hex string
    pub fn from_hex(hex: String) -> Result<Self, FfiError> {
        // Validate hex string length (should be 64 characters for 32 bytes)
        if hex.len() != 64 {
            return Err(FfiError::Generic {
                msg: "Secret key hex must be exactly 64 characters (32 bytes)".to_string(),
            });
        }

        // Validate hex format
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(FfiError::Generic {
                msg: "Secret key hex contains invalid characters".to_string(),
            });
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
#[derive(Debug, Clone, uniffi::Record)]
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

/// FFI-compatible Proof
#[derive(Debug, uniffi::Object)]
pub struct Proof {
    pub(crate) inner: cdk::nuts::Proof,
}

impl From<cdk::nuts::Proof> for Proof {
    fn from(proof: cdk::nuts::Proof) -> Self {
        Self { inner: proof }
    }
}

impl From<Proof> for cdk::nuts::Proof {
    fn from(proof: Proof) -> Self {
        proof.inner
    }
}

#[uniffi::export]
impl Proof {
    /// Get the amount
    pub fn amount(&self) -> Amount {
        self.inner.amount.into()
    }

    /// Get the secret as string
    pub fn secret(&self) -> String {
        self.inner.secret.to_string()
    }

    /// Get the unblinded signature (C) as string
    pub fn c(&self) -> String {
        self.inner.c.to_string()
    }

    /// Get the keyset ID as string
    pub fn keyset_id(&self) -> String {
        self.inner.keyset_id.to_string()
    }

    /// Get the witness as JSON string
    pub fn witness(&self) -> Option<String> {
        self.inner
            .witness
            .as_ref()
            .map(|w| serde_json::to_string(w).unwrap_or_default())
    }

    /// Check if proof is active with given keyset IDs
    pub fn is_active(&self, active_keyset_ids: Vec<String>) -> bool {
        use cdk::nuts::Id;
        let ids: Vec<Id> = active_keyset_ids
            .into_iter()
            .filter_map(|id| Id::from_str(&id).ok())
            .collect();
        self.inner.is_active(&ids)
    }

    /// Get the Y value (hash_to_curve of secret)
    pub fn y(&self) -> Result<String, FfiError> {
        Ok(self.inner.y()?.to_string())
    }
}

/// FFI-compatible Proofs (vector of Proof)
pub type Proofs = Vec<std::sync::Arc<Proof>>;

/// Helper functions for Proofs
pub fn proofs_total_amount(proofs: &Proofs) -> Result<Amount, FfiError> {
    let cdk_proofs: Vec<cdk::nuts::Proof> = proofs.iter().map(|p| p.inner.clone()).collect();
    use cdk::nuts::ProofsMethods;
    Ok(cdk_proofs.total_amount()?.into())
}

/// FFI-compatible MintQuote
#[derive(Debug, uniffi::Object)]
pub struct MintQuote {
    inner: cdk::wallet::MintQuote,
}

impl From<cdk::wallet::MintQuote> for MintQuote {
    fn from(quote: cdk::wallet::MintQuote) -> Self {
        Self { inner: quote }
    }
}

impl From<MintQuote> for cdk::wallet::MintQuote {
    fn from(quote: MintQuote) -> Self {
        quote.inner
    }
}

#[uniffi::export]
impl MintQuote {
    /// Get quote ID
    pub fn id(&self) -> String {
        self.inner.id.clone()
    }

    /// Get quote amount
    pub fn amount(&self) -> Option<Amount> {
        self.inner.amount.map(Into::into)
    }

    /// Get currency unit
    pub fn unit(&self) -> CurrencyUnit {
        self.inner.unit.clone().into()
    }

    /// Get payment request
    pub fn request(&self) -> String {
        self.inner.request.clone()
    }

    /// Get quote state
    pub fn state(&self) -> QuoteState {
        self.inner.state.into()
    }

    /// Get expiry timestamp
    pub fn expiry(&self) -> u64 {
        self.inner.expiry
    }

    /// Get mint URL
    pub fn mint_url(&self) -> MintUrl {
        self.inner.mint_url.clone().into()
    }

    /// Get total amount (amount + fees)
    pub fn total_amount(&self) -> Amount {
        self.inner.total_amount().into()
    }

    /// Check if quote is expired
    pub fn is_expired(&self, current_time: u64) -> bool {
        self.inner.is_expired(current_time)
    }

    /// Get amount that can be minted
    pub fn amount_mintable(&self) -> Amount {
        self.inner.amount_mintable().into()
    }

    /// Get amount issued
    pub fn amount_issued(&self) -> Amount {
        self.inner.amount_issued.into()
    }

    /// Get amount paid
    pub fn amount_paid(&self) -> Amount {
        self.inner.amount_paid.into()
    }
}

/// FFI-compatible MeltQuote
#[derive(Debug, uniffi::Object)]
pub struct MeltQuote {
    inner: cdk::wallet::MeltQuote,
}

impl From<cdk::wallet::MeltQuote> for MeltQuote {
    fn from(quote: cdk::wallet::MeltQuote) -> Self {
        Self { inner: quote }
    }
}

impl From<MeltQuote> for cdk::wallet::MeltQuote {
    fn from(quote: MeltQuote) -> Self {
        quote.inner
    }
}

#[uniffi::export]
impl MeltQuote {
    /// Get quote ID
    pub fn id(&self) -> String {
        self.inner.id.clone()
    }

    /// Get quote amount
    pub fn amount(&self) -> Amount {
        self.inner.amount.into()
    }

    /// Get currency unit
    pub fn unit(&self) -> CurrencyUnit {
        self.inner.unit.clone().into()
    }

    /// Get payment request
    pub fn request(&self) -> String {
        self.inner.request.clone()
    }

    /// Get fee reserve
    pub fn fee_reserve(&self) -> Amount {
        self.inner.fee_reserve.into()
    }

    /// Get quote state
    pub fn state(&self) -> QuoteState {
        self.inner.state.into()
    }

    /// Get expiry timestamp
    pub fn expiry(&self) -> u64 {
        self.inner.expiry
    }

    /// Get payment preimage
    pub fn payment_preimage(&self) -> Option<String> {
        self.inner.payment_preimage.clone()
    }
}

/// FFI-compatible QuoteState
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum QuoteState {
    Unpaid,
    Paid,
    Pending,
    Issued,
}

impl From<cdk::nuts::nut05::QuoteState> for QuoteState {
    fn from(state: cdk::nuts::nut05::QuoteState) -> Self {
        match state {
            cdk::nuts::nut05::QuoteState::Unpaid => QuoteState::Unpaid,
            cdk::nuts::nut05::QuoteState::Paid => QuoteState::Paid,
            cdk::nuts::nut05::QuoteState::Pending => QuoteState::Pending,
            cdk::nuts::nut05::QuoteState::Unknown => QuoteState::Unpaid,
            cdk::nuts::nut05::QuoteState::Failed => QuoteState::Unpaid,
        }
    }
}

impl From<cdk::nuts::MintQuoteState> for QuoteState {
    fn from(state: cdk::nuts::MintQuoteState) -> Self {
        match state {
            cdk::nuts::MintQuoteState::Unpaid => QuoteState::Unpaid,
            cdk::nuts::MintQuoteState::Paid => QuoteState::Paid,
            cdk::nuts::MintQuoteState::Issued => QuoteState::Issued,
        }
    }
}

// Note: MeltQuoteState is the same as nut05::QuoteState, so we don't need a separate impl

/// FFI-compatible PreparedSend
#[derive(Debug, uniffi::Object)]
pub struct PreparedSend {
    inner: Mutex<Option<cdk::wallet::PreparedSend>>,
    id: String,
    amount: Amount,
    proofs: Proofs,
}

impl From<cdk::wallet::PreparedSend> for PreparedSend {
    fn from(prepared: cdk::wallet::PreparedSend) -> Self {
        let id = format!("{:?}", prepared); // Use debug format as ID
        let amount = prepared.amount().into();
        let proofs = prepared
            .proofs()
            .iter()
            .cloned()
            .map(|p| std::sync::Arc::new(p.into()))
            .collect();
        Self {
            inner: Mutex::new(Some(prepared)),
            id,
            amount,
            proofs,
        }
    }
}

#[uniffi::export]
impl PreparedSend {
    /// Get the prepared send ID
    pub fn id(&self) -> String {
        self.id.clone()
    }

    /// Get the amount to send
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Get the proofs that will be used
    pub fn proofs(&self) -> Proofs {
        self.proofs.clone()
    }

    /// Get the total fee for this send operation
    pub fn fee(&self) -> Amount {
        if let Ok(guard) = self.inner.lock() {
            if let Some(ref inner) = *guard {
                inner.fee().into()
            } else {
                Amount::new(0)
            }
        } else {
            Amount::new(0)
        }
    }

    /// Confirm the prepared send and create a token
    pub async fn confirm(
        self: std::sync::Arc<Self>,
        memo: Option<String>,
    ) -> Result<Token, FfiError> {
        let inner = {
            if let Ok(mut guard) = self.inner.lock() {
                guard.take()
            } else {
                return Err(FfiError::Generic {
                    msg: "Failed to acquire lock on PreparedSend".to_string(),
                });
            }
        };

        if let Some(inner) = inner {
            let send_memo = memo.map(|m| cdk::wallet::SendMemo::for_token(&m));
            let token = inner.confirm(send_memo).await?;
            Ok(token.into())
        } else {
            Err(FfiError::Generic {
                msg: "PreparedSend has already been consumed or cancelled".to_string(),
            })
        }
    }

    /// Cancel the prepared send operation
    pub async fn cancel(self: std::sync::Arc<Self>) -> Result<(), FfiError> {
        let inner = {
            if let Ok(mut guard) = self.inner.lock() {
                guard.take()
            } else {
                return Err(FfiError::Generic {
                    msg: "Failed to acquire lock on PreparedSend".to_string(),
                });
            }
        };

        if let Some(inner) = inner {
            inner.cancel().await?;
            Ok(())
        } else {
            Err(FfiError::Generic {
                msg: "PreparedSend has already been consumed or cancelled".to_string(),
            })
        }
    }
}

/// FFI-compatible Melted result
#[derive(Debug, Clone, uniffi::Record)]
pub struct Melted {
    pub state: QuoteState,
    pub preimage: Option<String>,
    pub change: Option<Proofs>,
    pub amount: Amount,
    pub fee_paid: Amount,
}

// Simplified implementation - not implementing From trait for now
// impl From<CdkMelted> for Melted {
//     fn from(melted: CdkMelted) -> Self {
//         Self {
//             state: QuoteState::Paid,
//             preimage: melted.preimage,
//             change: melted.change.map(|c| c.into_iter().map(Into::into).collect()),
//             amount: melted.amount.into(),
//             fee_paid: melted.fee_paid.into(),
//         }
//     }
// }

/// FFI-compatible MeltOptions
#[derive(Debug, Clone, uniffi::Enum)]
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
