//! FFI-compatible types

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

use cdk::nuts::{CurrencyUnit as CdkCurrencyUnit, State as CdkState};
use cdk::Amount as CdkAmount;
use cdk_common::pub_sub::SubId;
use serde::{Deserialize, Serialize};

use crate::error::FfiError;

/// FFI-compatible Amount type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
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

    pub fn add(&self, other: Amount) -> Result<Amount, FfiError> {
        let self_amount = CdkAmount::from(self.value);
        let other_amount = CdkAmount::from(other.value);
        self_amount
            .checked_add(other_amount)
            .map(Into::into)
            .ok_or(FfiError::AmountOverflow)
    }

    pub fn subtract(&self, other: Amount) -> Result<Amount, FfiError> {
        let self_amount = CdkAmount::from(self.value);
        let other_amount = CdkAmount::from(other.value);
        self_amount
            .checked_sub(other_amount)
            .map(Into::into)
            .ok_or(FfiError::AmountOverflow)
    }

    pub fn multiply(&self, factor: u64) -> Result<Amount, FfiError> {
        let self_amount = CdkAmount::from(self.value);
        let factor_amount = CdkAmount::from(factor);
        self_amount
            .checked_mul(factor_amount)
            .map(Into::into)
            .ok_or(FfiError::AmountOverflow)
    }

    pub fn divide(&self, divisor: u64) -> Result<Amount, FfiError> {
        if divisor == 0 {
            return Err(FfiError::DivisionByZero);
        }
        let self_amount = CdkAmount::from(self.value);
        let divisor_amount = CdkAmount::from(divisor);
        self_amount
            .checked_div(divisor_amount)
            .map(Into::into)
            .ok_or(FfiError::AmountOverflow)
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum CurrencyUnit {
    Sat,
    Msat,
    Usd,
    Eur,
    Auth,
    Custom { unit: String },
}

impl From<CdkCurrencyUnit> for CurrencyUnit {
    fn from(unit: CdkCurrencyUnit) -> Self {
        match unit {
            CdkCurrencyUnit::Sat => CurrencyUnit::Sat,
            CdkCurrencyUnit::Msat => CurrencyUnit::Msat,
            CdkCurrencyUnit::Usd => CurrencyUnit::Usd,
            CdkCurrencyUnit::Eur => CurrencyUnit::Eur,
            CdkCurrencyUnit::Auth => CurrencyUnit::Auth,
            CdkCurrencyUnit::Custom(s) => CurrencyUnit::Custom { unit: s },
            _ => CurrencyUnit::Sat, // Default for unknown units
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
            CurrencyUnit::Auth => CdkCurrencyUnit::Auth,
            CurrencyUnit::Custom { unit } => CdkCurrencyUnit::Custom(unit),
        }
    }
}

/// FFI-compatible Mint URL
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
pub struct MintUrl {
    pub url: String,
}

impl MintUrl {
    pub fn new(url: String) -> Result<Self, FfiError> {
        // Validate URL format
        url::Url::parse(&url).map_err(|e| FfiError::InvalidUrl { msg: e.to_string() })?;

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
            .map_err(|e| FfiError::InvalidUrl { msg: e.to_string() })
    }
}

/// FFI-compatible Proof state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
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

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl FromStr for Token {
    type Err = FfiError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let token = cdk::nuts::Token::from_str(s)
            .map_err(|e| FfiError::InvalidToken { msg: e.to_string() })?;
        Ok(Token { inner: token })
    }
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
    pub fn from_string(encoded_token: String) -> Result<Token, FfiError> {
        let token = cdk::nuts::Token::from_str(&encoded_token)
            .map_err(|e| FfiError::InvalidToken { msg: e.to_string() })?;
        Ok(Token { inner: token })
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

    /// Convert token to raw bytes
    pub fn to_raw_bytes(&self) -> Result<Vec<u8>, FfiError> {
        Ok(self.inner.to_raw_bytes()?)
    }

    /// Encode token to string representation
    pub fn encode(&self) -> String {
        self.to_string()
    }

    /// Decode token from string representation
    #[uniffi::constructor]
    pub fn decode(encoded_token: String) -> Result<Token, FfiError> {
        encoded_token.parse()
    }
}

/// FFI-compatible SendMemo
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
#[uniffi::export]
pub fn decode_send_memo(json: String) -> Result<SendMemo, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode SendMemo to JSON string
#[uniffi::export]
pub fn encode_send_memo(memo: SendMemo) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&memo)?)
}

/// FFI-compatible SplitTarget
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
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
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
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
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
#[uniffi::export]
pub fn decode_send_options(json: String) -> Result<SendOptions, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode SendOptions to JSON string
#[uniffi::export]
pub fn encode_send_options(options: SendOptions) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&options)?)
}

/// FFI-compatible SecretKey
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
            return Err(FfiError::InvalidHex {
                msg: "Secret key hex must be exactly 64 characters (32 bytes)".to_string(),
            });
        }

        // Validate hex format
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(FfiError::InvalidHex {
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
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
#[uniffi::export]
pub fn decode_receive_options(json: String) -> Result<ReceiveOptions, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode ReceiveOptions to JSON string
#[uniffi::export]
pub fn encode_receive_options(options: ReceiveOptions) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&options)?)
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

    /// Get the witness
    pub fn witness(&self) -> Option<Witness> {
        self.inner.witness.as_ref().map(|w| w.clone().into())
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

    /// Get the DLEQ proof if present
    pub fn dleq(&self) -> Option<ProofDleq> {
        self.inner.dleq.as_ref().map(|d| d.clone().into())
    }

    /// Check if proof has DLEQ proof
    pub fn has_dleq(&self) -> bool {
        self.inner.dleq.is_some()
    }
}

/// FFI-compatible Proofs (vector of Proof)
pub type Proofs = Vec<std::sync::Arc<Proof>>;

/// FFI-compatible DLEQ proof for proofs
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ProofDleq {
    /// e value (hex-encoded SecretKey)
    pub e: String,
    /// s value (hex-encoded SecretKey)
    pub s: String,
    /// r value - blinding factor (hex-encoded SecretKey)
    pub r: String,
}

/// FFI-compatible DLEQ proof for blind signatures
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct BlindSignatureDleq {
    /// e value (hex-encoded SecretKey)
    pub e: String,
    /// s value (hex-encoded SecretKey)
    pub s: String,
}

impl From<cdk::nuts::ProofDleq> for ProofDleq {
    fn from(dleq: cdk::nuts::ProofDleq) -> Self {
        Self {
            e: dleq.e.to_secret_hex(),
            s: dleq.s.to_secret_hex(),
            r: dleq.r.to_secret_hex(),
        }
    }
}

impl From<ProofDleq> for cdk::nuts::ProofDleq {
    fn from(dleq: ProofDleq) -> Self {
        Self {
            e: cdk::nuts::SecretKey::from_hex(&dleq.e).expect("Invalid e hex"),
            s: cdk::nuts::SecretKey::from_hex(&dleq.s).expect("Invalid s hex"),
            r: cdk::nuts::SecretKey::from_hex(&dleq.r).expect("Invalid r hex"),
        }
    }
}

impl From<cdk::nuts::BlindSignatureDleq> for BlindSignatureDleq {
    fn from(dleq: cdk::nuts::BlindSignatureDleq) -> Self {
        Self {
            e: dleq.e.to_secret_hex(),
            s: dleq.s.to_secret_hex(),
        }
    }
}

impl From<BlindSignatureDleq> for cdk::nuts::BlindSignatureDleq {
    fn from(dleq: BlindSignatureDleq) -> Self {
        Self {
            e: cdk::nuts::SecretKey::from_hex(&dleq.e).expect("Invalid e hex"),
            s: cdk::nuts::SecretKey::from_hex(&dleq.s).expect("Invalid s hex"),
        }
    }
}

/// Helper functions for Proofs
pub fn proofs_total_amount(proofs: &Proofs) -> Result<Amount, FfiError> {
    let cdk_proofs: Vec<cdk::nuts::Proof> = proofs.iter().map(|p| p.inner.clone()).collect();
    use cdk::nuts::ProofsMethods;
    Ok(cdk_proofs.total_amount()?.into())
}

/// FFI-compatible MintQuote
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MintQuote {
    /// Quote ID
    pub id: String,
    /// Quote amount
    pub amount: Option<Amount>,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Payment request
    pub request: String,
    /// Quote state
    pub state: QuoteState,
    /// Expiry timestamp
    pub expiry: u64,
    /// Mint URL
    pub mint_url: MintUrl,
    /// Amount issued
    pub amount_issued: Amount,
    /// Amount paid
    pub amount_paid: Amount,
    /// Payment method
    pub payment_method: PaymentMethod,
    /// Secret key (optional, hex-encoded)
    pub secret_key: Option<String>,
}

impl From<cdk::wallet::MintQuote> for MintQuote {
    fn from(quote: cdk::wallet::MintQuote) -> Self {
        Self {
            id: quote.id.clone(),
            amount: quote.amount.map(Into::into),
            unit: quote.unit.clone().into(),
            request: quote.request.clone(),
            state: quote.state.into(),
            expiry: quote.expiry,
            mint_url: quote.mint_url.clone().into(),
            amount_issued: quote.amount_issued.into(),
            amount_paid: quote.amount_paid.into(),
            payment_method: quote.payment_method.into(),
            secret_key: quote.secret_key.map(|sk| sk.to_secret_hex()),
        }
    }
}

impl TryFrom<MintQuote> for cdk::wallet::MintQuote {
    type Error = FfiError;

    fn try_from(quote: MintQuote) -> Result<Self, Self::Error> {
        let secret_key = quote
            .secret_key
            .map(|hex| cdk::nuts::SecretKey::from_hex(&hex))
            .transpose()
            .map_err(|e| FfiError::InvalidCryptographicKey { msg: e.to_string() })?;

        Ok(Self {
            id: quote.id,
            amount: quote.amount.map(Into::into),
            unit: quote.unit.into(),
            request: quote.request,
            state: quote.state.into(),
            expiry: quote.expiry,
            mint_url: quote.mint_url.try_into()?,
            amount_issued: quote.amount_issued.into(),
            amount_paid: quote.amount_paid.into(),
            payment_method: quote.payment_method.into(),
            secret_key,
        })
    }
}

impl MintQuote {
    /// Get total amount (amount + fees)
    pub fn total_amount(&self) -> Amount {
        if let Some(amount) = self.amount {
            Amount::new(amount.value + self.amount_paid.value - self.amount_issued.value)
        } else {
            Amount::zero()
        }
    }

    /// Check if quote is expired
    pub fn is_expired(&self, current_time: u64) -> bool {
        current_time > self.expiry
    }

    /// Get amount that can be minted
    pub fn amount_mintable(&self) -> Amount {
        Amount::new(self.amount_paid.value - self.amount_issued.value)
    }

    /// Convert MintQuote to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode MintQuote from JSON string
#[uniffi::export]
pub fn decode_mint_quote(json: String) -> Result<MintQuote, FfiError> {
    let quote: cdk::wallet::MintQuote = serde_json::from_str(&json)?;
    Ok(quote.into())
}

/// Encode MintQuote to JSON string
#[uniffi::export]
pub fn encode_mint_quote(quote: MintQuote) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&quote)?)
}

/// FFI-compatible MintQuoteBolt11Response
#[derive(Debug, uniffi::Object)]
pub struct MintQuoteBolt11Response {
    /// Quote ID
    pub quote: String,
    /// Request string
    pub request: String,
    /// State of the quote
    pub state: QuoteState,
    /// Expiry timestamp (optional)
    pub expiry: Option<u64>,
    /// Amount (optional)
    pub amount: Option<Amount>,
    /// Unit (optional)
    pub unit: Option<CurrencyUnit>,
    /// Pubkey (optional)
    pub pubkey: Option<String>,
}

impl From<cdk::nuts::MintQuoteBolt11Response<String>> for MintQuoteBolt11Response {
    fn from(response: cdk::nuts::MintQuoteBolt11Response<String>) -> Self {
        Self {
            quote: response.quote,
            request: response.request,
            state: response.state.into(),
            expiry: response.expiry,
            amount: response.amount.map(Into::into),
            unit: response.unit.map(Into::into),
            pubkey: response.pubkey.map(|p| p.to_string()),
        }
    }
}

#[uniffi::export]
impl MintQuoteBolt11Response {
    /// Get quote ID
    pub fn quote(&self) -> String {
        self.quote.clone()
    }

    /// Get request string
    pub fn request(&self) -> String {
        self.request.clone()
    }

    /// Get state
    pub fn state(&self) -> QuoteState {
        self.state.clone()
    }

    /// Get expiry
    pub fn expiry(&self) -> Option<u64> {
        self.expiry
    }

    /// Get amount
    pub fn amount(&self) -> Option<Amount> {
        self.amount
    }

    /// Get unit
    pub fn unit(&self) -> Option<CurrencyUnit> {
        self.unit.clone()
    }

    /// Get pubkey
    pub fn pubkey(&self) -> Option<String> {
        self.pubkey.clone()
    }
}

/// FFI-compatible MeltQuoteBolt11Response
#[derive(Debug, uniffi::Object)]
pub struct MeltQuoteBolt11Response {
    /// Quote ID
    pub quote: String,
    /// Amount
    pub amount: Amount,
    /// Fee reserve
    pub fee_reserve: Amount,
    /// State of the quote
    pub state: QuoteState,
    /// Expiry timestamp
    pub expiry: u64,
    /// Payment preimage (optional)
    pub payment_preimage: Option<String>,
    /// Request string (optional)
    pub request: Option<String>,
    /// Unit (optional)
    pub unit: Option<CurrencyUnit>,
}

impl From<cdk::nuts::MeltQuoteBolt11Response<String>> for MeltQuoteBolt11Response {
    fn from(response: cdk::nuts::MeltQuoteBolt11Response<String>) -> Self {
        Self {
            quote: response.quote,
            amount: response.amount.into(),
            fee_reserve: response.fee_reserve.into(),
            state: response.state.into(),
            expiry: response.expiry,
            payment_preimage: response.payment_preimage,
            request: response.request,
            unit: response.unit.map(Into::into),
        }
    }
}

#[uniffi::export]
impl MeltQuoteBolt11Response {
    /// Get quote ID
    pub fn quote(&self) -> String {
        self.quote.clone()
    }

    /// Get amount
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Get fee reserve
    pub fn fee_reserve(&self) -> Amount {
        self.fee_reserve
    }

    /// Get state
    pub fn state(&self) -> QuoteState {
        self.state.clone()
    }

    /// Get expiry
    pub fn expiry(&self) -> u64 {
        self.expiry
    }

    /// Get payment preimage
    pub fn payment_preimage(&self) -> Option<String> {
        self.payment_preimage.clone()
    }

    /// Get request
    pub fn request(&self) -> Option<String> {
        self.request.clone()
    }

    /// Get unit
    pub fn unit(&self) -> Option<CurrencyUnit> {
        self.unit.clone()
    }
}

/// FFI-compatible PaymentMethod
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum PaymentMethod {
    /// Bolt11 payment type
    Bolt11,
    /// Bolt12 payment type
    Bolt12,
    /// Custom payment type
    Custom { method: String },
}

impl From<cdk::nuts::PaymentMethod> for PaymentMethod {
    fn from(method: cdk::nuts::PaymentMethod) -> Self {
        match method {
            cdk::nuts::PaymentMethod::Bolt11 => Self::Bolt11,
            cdk::nuts::PaymentMethod::Bolt12 => Self::Bolt12,
            cdk::nuts::PaymentMethod::Custom(s) => Self::Custom { method: s },
        }
    }
}

impl From<PaymentMethod> for cdk::nuts::PaymentMethod {
    fn from(method: PaymentMethod) -> Self {
        match method {
            PaymentMethod::Bolt11 => Self::Bolt11,
            PaymentMethod::Bolt12 => Self::Bolt12,
            PaymentMethod::Custom { method } => Self::Custom(method),
        }
    }
}

/// FFI-compatible MeltQuote
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MeltQuote {
    /// Quote ID
    pub id: String,
    /// Quote amount
    pub amount: Amount,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Payment request
    pub request: String,
    /// Fee reserve
    pub fee_reserve: Amount,
    /// Quote state
    pub state: QuoteState,
    /// Expiry timestamp
    pub expiry: u64,
    /// Payment preimage
    pub payment_preimage: Option<String>,
    /// Payment method
    pub payment_method: PaymentMethod,
}

impl From<cdk::wallet::MeltQuote> for MeltQuote {
    fn from(quote: cdk::wallet::MeltQuote) -> Self {
        Self {
            id: quote.id.clone(),
            amount: quote.amount.into(),
            unit: quote.unit.clone().into(),
            request: quote.request.clone(),
            fee_reserve: quote.fee_reserve.into(),
            state: quote.state.into(),
            expiry: quote.expiry,
            payment_preimage: quote.payment_preimage.clone(),
            payment_method: quote.payment_method.into(),
        }
    }
}

impl TryFrom<MeltQuote> for cdk::wallet::MeltQuote {
    type Error = FfiError;

    fn try_from(quote: MeltQuote) -> Result<Self, Self::Error> {
        Ok(Self {
            id: quote.id,
            amount: quote.amount.into(),
            unit: quote.unit.into(),
            request: quote.request,
            fee_reserve: quote.fee_reserve.into(),
            state: quote.state.into(),
            expiry: quote.expiry,
            payment_preimage: quote.payment_preimage,
            payment_method: quote.payment_method.into(),
        })
    }
}

impl MeltQuote {
    /// Convert MeltQuote to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode MeltQuote from JSON string
#[uniffi::export]
pub fn decode_melt_quote(json: String) -> Result<MeltQuote, FfiError> {
    let quote: cdk::wallet::MeltQuote = serde_json::from_str(&json)?;
    Ok(quote.into())
}

/// Encode MeltQuote to JSON string
#[uniffi::export]
pub fn encode_melt_quote(quote: MeltQuote) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&quote)?)
}

/// FFI-compatible QuoteState
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
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

impl From<QuoteState> for cdk::nuts::nut05::QuoteState {
    fn from(state: QuoteState) -> Self {
        match state {
            QuoteState::Unpaid => cdk::nuts::nut05::QuoteState::Unpaid,
            QuoteState::Paid => cdk::nuts::nut05::QuoteState::Paid,
            QuoteState::Pending => cdk::nuts::nut05::QuoteState::Pending,
            QuoteState::Issued => cdk::nuts::nut05::QuoteState::Paid, // Map issued to paid for melt quotes
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

impl From<QuoteState> for cdk::nuts::MintQuoteState {
    fn from(state: QuoteState) -> Self {
        match state {
            QuoteState::Unpaid => cdk::nuts::MintQuoteState::Unpaid,
            QuoteState::Paid => cdk::nuts::MintQuoteState::Paid,
            QuoteState::Issued => cdk::nuts::MintQuoteState::Issued,
            QuoteState::Pending => cdk::nuts::MintQuoteState::Paid, // Map pending to paid
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

#[uniffi::export(async_runtime = "tokio")]
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

// MeltQuoteState is just an alias for nut05::QuoteState, so we don't need a separate implementation

impl From<cdk_common::common::Melted> for Melted {
    fn from(melted: cdk_common::common::Melted) -> Self {
        Self {
            state: melted.state.into(),
            preimage: melted.preimage,
            change: melted.change.map(|proofs| {
                proofs
                    .into_iter()
                    .map(|p| std::sync::Arc::new(p.into()))
                    .collect()
            }),
            amount: melted.amount.into(),
            fee_paid: melted.fee_paid.into(),
        }
    }
}

/// FFI-compatible MeltOptions
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
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

/// FFI-compatible MintVersion
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MintVersion {
    /// Mint Software name
    pub name: String,
    /// Mint Version
    pub version: String,
}

impl From<cdk::nuts::MintVersion> for MintVersion {
    fn from(version: cdk::nuts::MintVersion) -> Self {
        Self {
            name: version.name,
            version: version.version,
        }
    }
}

impl From<MintVersion> for cdk::nuts::MintVersion {
    fn from(version: MintVersion) -> Self {
        Self {
            name: version.name,
            version: version.version,
        }
    }
}

impl MintVersion {
    /// Convert MintVersion to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode MintVersion from JSON string
#[uniffi::export]
pub fn decode_mint_version(json: String) -> Result<MintVersion, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode MintVersion to JSON string
#[uniffi::export]
pub fn encode_mint_version(version: MintVersion) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&version)?)
}

/// FFI-compatible ContactInfo
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ContactInfo {
    /// Contact Method i.e. nostr
    pub method: String,
    /// Contact info i.e. npub...
    pub info: String,
}

impl From<cdk::nuts::ContactInfo> for ContactInfo {
    fn from(contact: cdk::nuts::ContactInfo) -> Self {
        Self {
            method: contact.method,
            info: contact.info,
        }
    }
}

impl From<ContactInfo> for cdk::nuts::ContactInfo {
    fn from(contact: ContactInfo) -> Self {
        Self {
            method: contact.method,
            info: contact.info,
        }
    }
}

impl ContactInfo {
    /// Convert ContactInfo to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode ContactInfo from JSON string
#[uniffi::export]
pub fn decode_contact_info(json: String) -> Result<ContactInfo, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode ContactInfo to JSON string
#[uniffi::export]
pub fn encode_contact_info(info: ContactInfo) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&info)?)
}

/// FFI-compatible SupportedSettings
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
pub struct SupportedSettings {
    /// Setting supported
    pub supported: bool,
}

impl From<cdk::nuts::nut06::SupportedSettings> for SupportedSettings {
    fn from(settings: cdk::nuts::nut06::SupportedSettings) -> Self {
        Self {
            supported: settings.supported,
        }
    }
}

impl From<SupportedSettings> for cdk::nuts::nut06::SupportedSettings {
    fn from(settings: SupportedSettings) -> Self {
        Self {
            supported: settings.supported,
        }
    }
}

/// FFI-compatible Nuts settings (simplified - only includes basic boolean flags)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct Nuts {
    /// NUT07 Settings - Token state check
    pub nut07_supported: bool,
    /// NUT08 Settings - Lightning fee return
    pub nut08_supported: bool,
    /// NUT09 Settings - Restore signature
    pub nut09_supported: bool,
    /// NUT10 Settings - Spending conditions
    pub nut10_supported: bool,
    /// NUT11 Settings - Pay to Public Key Hash
    pub nut11_supported: bool,
    /// NUT12 Settings - DLEQ proofs
    pub nut12_supported: bool,
    /// NUT14 Settings - Hashed Time Locked Contracts
    pub nut14_supported: bool,
    /// NUT20 Settings - Web sockets
    pub nut20_supported: bool,
    /// Supported currency units for minting
    pub mint_units: Vec<CurrencyUnit>,
    /// Supported currency units for melting
    pub melt_units: Vec<CurrencyUnit>,
}

impl From<cdk::nuts::Nuts> for Nuts {
    fn from(nuts: cdk::nuts::Nuts) -> Self {
        Self {
            nut07_supported: nuts.nut07.supported,
            nut08_supported: nuts.nut08.supported,
            nut09_supported: nuts.nut09.supported,
            nut10_supported: nuts.nut10.supported,
            nut11_supported: nuts.nut11.supported,
            nut12_supported: nuts.nut12.supported,
            nut14_supported: nuts.nut14.supported,
            nut20_supported: nuts.nut20.supported,
            mint_units: nuts
                .supported_mint_units()
                .into_iter()
                .map(|u| u.clone().into())
                .collect(),
            melt_units: nuts
                .supported_melt_units()
                .into_iter()
                .map(|u| u.clone().into())
                .collect(),
        }
    }
}

impl Nuts {
    /// Convert Nuts to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode Nuts from JSON string
#[uniffi::export]
pub fn decode_nuts(json: String) -> Result<Nuts, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode Nuts to JSON string
#[uniffi::export]
pub fn encode_nuts(nuts: Nuts) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&nuts)?)
}

/// FFI-compatible MintInfo
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MintInfo {
    /// name of the mint and should be recognizable
    pub name: Option<String>,
    /// hex pubkey of the mint  
    pub pubkey: Option<String>,
    /// implementation name and the version running
    pub version: Option<MintVersion>,
    /// short description of the mint
    pub description: Option<String>,
    /// long description
    pub description_long: Option<String>,
    /// Contact info
    pub contact: Option<Vec<ContactInfo>>,
    /// shows which NUTs the mint supports
    pub nuts: Nuts,
    /// Mint's icon URL
    pub icon_url: Option<String>,
    /// Mint's endpoint URLs
    pub urls: Option<Vec<String>>,
    /// message of the day that the wallet must display to the user
    pub motd: Option<String>,
    /// server unix timestamp
    pub time: Option<u64>,
    /// terms of url service of the mint
    pub tos_url: Option<String>,
}

impl From<cdk::nuts::MintInfo> for MintInfo {
    fn from(info: cdk::nuts::MintInfo) -> Self {
        Self {
            name: info.name,
            pubkey: info.pubkey.map(|p| p.to_string()),
            version: info.version.map(Into::into),
            description: info.description,
            description_long: info.description_long,
            contact: info
                .contact
                .map(|contacts| contacts.into_iter().map(Into::into).collect()),
            nuts: info.nuts.into(),
            icon_url: info.icon_url,
            urls: info.urls,
            motd: info.motd,
            time: info.time,
            tos_url: info.tos_url,
        }
    }
}

impl From<MintInfo> for cdk_common::nuts::MintInfo {
    fn from(info: MintInfo) -> Self {
        Self {
            name: info.name,
            pubkey: info.pubkey.and_then(|p| p.parse().ok()),
            version: info.version.map(Into::into),
            description: info.description,
            description_long: info.description_long,
            contact: info
                .contact
                .map(|contacts| contacts.into_iter().map(Into::into).collect()),
            nuts: cdk_common::nuts::Nuts::default(), // Simplified conversion
            icon_url: info.icon_url,
            urls: info.urls,
            motd: info.motd,
            time: info.time,
            tos_url: info.tos_url,
        }
    }
}

impl MintInfo {
    /// Convert MintInfo to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode MintInfo from JSON string
#[uniffi::export]
pub fn decode_mint_info(json: String) -> Result<MintInfo, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode MintInfo to JSON string
#[uniffi::export]
pub fn encode_mint_info(info: MintInfo) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&info)?)
}

/// FFI-compatible Conditions (for spending conditions)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct Conditions {
    /// Unix locktime after which refund keys can be used
    pub locktime: Option<u64>,
    /// Additional Public keys (as hex strings)
    pub pubkeys: Vec<String>,
    /// Refund keys (as hex strings)
    pub refund_keys: Vec<String>,
    /// Number of signatures required (default 1)
    pub num_sigs: Option<u64>,
    /// Signature flag (0 = SigInputs, 1 = SigAll)
    pub sig_flag: u8,
    /// Number of refund signatures required (default 1)
    pub num_sigs_refund: Option<u64>,
}

impl From<cdk::nuts::nut11::Conditions> for Conditions {
    fn from(conditions: cdk::nuts::nut11::Conditions) -> Self {
        Self {
            locktime: conditions.locktime,
            pubkeys: conditions
                .pubkeys
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.to_string())
                .collect(),
            refund_keys: conditions
                .refund_keys
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.to_string())
                .collect(),
            num_sigs: conditions.num_sigs,
            sig_flag: match conditions.sig_flag {
                cdk::nuts::nut11::SigFlag::SigInputs => 0,
                cdk::nuts::nut11::SigFlag::SigAll => 1,
            },
            num_sigs_refund: conditions.num_sigs_refund,
        }
    }
}

impl TryFrom<Conditions> for cdk::nuts::nut11::Conditions {
    type Error = FfiError;

    fn try_from(conditions: Conditions) -> Result<Self, Self::Error> {
        let pubkeys = if conditions.pubkeys.is_empty() {
            None
        } else {
            Some(
                conditions
                    .pubkeys
                    .into_iter()
                    .map(|s| {
                        s.parse().map_err(|e| FfiError::InvalidCryptographicKey {
                            msg: format!("Invalid pubkey: {}", e),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )
        };

        let refund_keys = if conditions.refund_keys.is_empty() {
            None
        } else {
            Some(
                conditions
                    .refund_keys
                    .into_iter()
                    .map(|s| {
                        s.parse().map_err(|e| FfiError::InvalidCryptographicKey {
                            msg: format!("Invalid refund key: {}", e),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )
        };

        let sig_flag = match conditions.sig_flag {
            0 => cdk::nuts::nut11::SigFlag::SigInputs,
            1 => cdk::nuts::nut11::SigFlag::SigAll,
            _ => {
                return Err(FfiError::Generic {
                    msg: "Invalid sig_flag value".to_string(),
                })
            }
        };

        Ok(Self {
            locktime: conditions.locktime,
            pubkeys,
            refund_keys,
            num_sigs: conditions.num_sigs,
            sig_flag,
            num_sigs_refund: conditions.num_sigs_refund,
        })
    }
}

impl Conditions {
    /// Convert Conditions to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode Conditions from JSON string
#[uniffi::export]
pub fn decode_conditions(json: String) -> Result<Conditions, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode Conditions to JSON string
#[uniffi::export]
pub fn encode_conditions(conditions: Conditions) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&conditions)?)
}

/// FFI-compatible Witness
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
pub enum Witness {
    /// P2PK Witness
    P2PK {
        /// Signatures
        signatures: Vec<String>,
    },
    /// HTLC Witness  
    HTLC {
        /// Preimage
        preimage: String,
        /// Optional signatures
        signatures: Option<Vec<String>>,
    },
}

impl From<cdk::nuts::Witness> for Witness {
    fn from(witness: cdk::nuts::Witness) -> Self {
        match witness {
            cdk::nuts::Witness::P2PKWitness(p2pk) => Self::P2PK {
                signatures: p2pk.signatures,
            },
            cdk::nuts::Witness::HTLCWitness(htlc) => Self::HTLC {
                preimage: htlc.preimage,
                signatures: htlc.signatures,
            },
        }
    }
}

impl From<Witness> for cdk::nuts::Witness {
    fn from(witness: Witness) -> Self {
        match witness {
            Witness::P2PK { signatures } => {
                Self::P2PKWitness(cdk::nuts::nut11::P2PKWitness { signatures })
            }
            Witness::HTLC {
                preimage,
                signatures,
            } => Self::HTLCWitness(cdk::nuts::nut14::HTLCWitness {
                preimage,
                signatures,
            }),
        }
    }
}

/// FFI-compatible SpendingConditions
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
pub enum SpendingConditions {
    /// P2PK (Pay to Public Key) conditions
    P2PK {
        /// The public key (as hex string)
        pubkey: String,
        /// Additional conditions
        conditions: Option<Conditions>,
    },
    /// HTLC (Hash Time Locked Contract) conditions
    HTLC {
        /// Hash of the preimage (as hex string)
        hash: String,
        /// Additional conditions
        conditions: Option<Conditions>,
    },
}

impl From<cdk::nuts::SpendingConditions> for SpendingConditions {
    fn from(spending_conditions: cdk::nuts::SpendingConditions) -> Self {
        match spending_conditions {
            cdk::nuts::SpendingConditions::P2PKConditions { data, conditions } => Self::P2PK {
                pubkey: data.to_string(),
                conditions: conditions.map(Into::into),
            },
            cdk::nuts::SpendingConditions::HTLCConditions { data, conditions } => Self::HTLC {
                hash: data.to_string(),
                conditions: conditions.map(Into::into),
            },
        }
    }
}

/// FFI-compatible Transaction
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct Transaction {
    /// Transaction ID
    pub id: TransactionId,
    /// Mint URL
    pub mint_url: MintUrl,
    /// Transaction direction
    pub direction: TransactionDirection,
    /// Amount
    pub amount: Amount,
    /// Fee
    pub fee: Amount,
    /// Currency Unit
    pub unit: CurrencyUnit,
    /// Proof Ys (Y values from proofs)
    pub ys: Vec<PublicKey>,
    /// Unix timestamp
    pub timestamp: u64,
    /// Memo
    pub memo: Option<String>,
    /// User-defined metadata
    pub metadata: HashMap<String, String>,
    /// Quote ID if this is a mint or melt transaction
    pub quote_id: Option<String>,
}

impl From<cdk_common::wallet::Transaction> for Transaction {
    fn from(tx: cdk_common::wallet::Transaction) -> Self {
        Self {
            id: tx.id().into(),
            mint_url: tx.mint_url.into(),
            direction: tx.direction.into(),
            amount: tx.amount.into(),
            fee: tx.fee.into(),
            unit: tx.unit.into(),
            ys: tx.ys.into_iter().map(Into::into).collect(),
            timestamp: tx.timestamp,
            memo: tx.memo,
            metadata: tx.metadata,
            quote_id: tx.quote_id,
        }
    }
}

/// Convert FFI Transaction to CDK Transaction
impl TryFrom<Transaction> for cdk_common::wallet::Transaction {
    type Error = FfiError;

    fn try_from(tx: Transaction) -> Result<Self, Self::Error> {
        let cdk_ys: Result<Vec<cdk_common::nuts::PublicKey>, _> =
            tx.ys.into_iter().map(|pk| pk.try_into()).collect();
        let cdk_ys = cdk_ys?;

        Ok(Self {
            mint_url: tx.mint_url.try_into()?,
            direction: tx.direction.into(),
            amount: tx.amount.into(),
            fee: tx.fee.into(),
            unit: tx.unit.into(),
            ys: cdk_ys,
            timestamp: tx.timestamp,
            memo: tx.memo,
            metadata: tx.metadata,
            quote_id: tx.quote_id,
        })
    }
}

impl Transaction {
    /// Convert Transaction to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode Transaction from JSON string
#[uniffi::export]
pub fn decode_transaction(json: String) -> Result<Transaction, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode Transaction to JSON string
#[uniffi::export]
pub fn encode_transaction(transaction: Transaction) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&transaction)?)
}

/// FFI-compatible TransactionDirection
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum TransactionDirection {
    /// Incoming transaction (i.e., receive or mint)
    Incoming,
    /// Outgoing transaction (i.e., send or melt)
    Outgoing,
}

impl From<cdk_common::wallet::TransactionDirection> for TransactionDirection {
    fn from(direction: cdk_common::wallet::TransactionDirection) -> Self {
        match direction {
            cdk_common::wallet::TransactionDirection::Incoming => TransactionDirection::Incoming,
            cdk_common::wallet::TransactionDirection::Outgoing => TransactionDirection::Outgoing,
        }
    }
}

impl From<TransactionDirection> for cdk_common::wallet::TransactionDirection {
    fn from(direction: TransactionDirection) -> Self {
        match direction {
            TransactionDirection::Incoming => cdk_common::wallet::TransactionDirection::Incoming,
            TransactionDirection::Outgoing => cdk_common::wallet::TransactionDirection::Outgoing,
        }
    }
}

/// FFI-compatible TransactionId
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
pub struct TransactionId {
    /// Hex-encoded transaction ID (64 characters)
    pub hex: String,
}

impl TransactionId {
    /// Create a new TransactionId from hex string
    pub fn from_hex(hex: String) -> Result<Self, FfiError> {
        // Validate hex string length (should be 64 characters for 32 bytes)
        if hex.len() != 64 {
            return Err(FfiError::InvalidHex {
                msg: "Transaction ID hex must be exactly 64 characters (32 bytes)".to_string(),
            });
        }

        // Validate hex format
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(FfiError::InvalidHex {
                msg: "Transaction ID hex contains invalid characters".to_string(),
            });
        }

        Ok(Self { hex })
    }

    /// Create from proofs
    pub fn from_proofs(proofs: &Proofs) -> Result<Self, FfiError> {
        let cdk_proofs: Vec<cdk::nuts::Proof> = proofs.iter().map(|p| p.inner.clone()).collect();
        let id = cdk_common::wallet::TransactionId::from_proofs(cdk_proofs)?;
        Ok(Self {
            hex: id.to_string(),
        })
    }
}

impl From<cdk_common::wallet::TransactionId> for TransactionId {
    fn from(id: cdk_common::wallet::TransactionId) -> Self {
        Self {
            hex: id.to_string(),
        }
    }
}

impl TryFrom<TransactionId> for cdk_common::wallet::TransactionId {
    type Error = FfiError;

    fn try_from(id: TransactionId) -> Result<Self, Self::Error> {
        cdk_common::wallet::TransactionId::from_hex(&id.hex)
            .map_err(|e| FfiError::InvalidHex { msg: e.to_string() })
    }
}

/// FFI-compatible AuthProof
#[cfg(feature = "auth")]
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct AuthProof {
    /// Keyset ID
    pub keyset_id: String,
    /// Secret message
    pub secret: String,
    /// Unblinded signature (C)
    pub c: String,
    /// Y value (hash_to_curve of secret)
    pub y: String,
}

#[cfg(feature = "auth")]
impl From<cdk_common::AuthProof> for AuthProof {
    fn from(auth_proof: cdk_common::AuthProof) -> Self {
        Self {
            keyset_id: auth_proof.keyset_id.to_string(),
            secret: auth_proof.secret.to_string(),
            c: auth_proof.c.to_string(),
            y: auth_proof
                .y()
                .map(|y| y.to_string())
                .unwrap_or_else(|_| "".to_string()),
        }
    }
}

#[cfg(feature = "auth")]
impl TryFrom<AuthProof> for cdk_common::AuthProof {
    type Error = FfiError;

    fn try_from(auth_proof: AuthProof) -> Result<Self, Self::Error> {
        use std::str::FromStr;
        Ok(Self {
            keyset_id: cdk_common::Id::from_str(&auth_proof.keyset_id)
                .map_err(|e| FfiError::Serialization { msg: e.to_string() })?,
            secret: {
                use std::str::FromStr;
                cdk_common::secret::Secret::from_str(&auth_proof.secret)
                    .map_err(|e| FfiError::Serialization { msg: e.to_string() })?
            },
            c: cdk_common::PublicKey::from_str(&auth_proof.c)
                .map_err(|e| FfiError::InvalidCryptographicKey { msg: e.to_string() })?,
            dleq: None, // FFI doesn't expose DLEQ proofs for simplicity
        })
    }
}

#[cfg(feature = "auth")]
impl AuthProof {
    /// Convert AuthProof to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode AuthProof from JSON string
#[cfg(feature = "auth")]
#[uniffi::export]
pub fn decode_auth_proof(json: String) -> Result<AuthProof, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode AuthProof to JSON string
#[cfg(feature = "auth")]
#[uniffi::export]
pub fn encode_auth_proof(proof: AuthProof) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&proof)?)
}

impl TryFrom<SpendingConditions> for cdk::nuts::SpendingConditions {
    type Error = FfiError;

    fn try_from(spending_conditions: SpendingConditions) -> Result<Self, Self::Error> {
        match spending_conditions {
            SpendingConditions::P2PK { pubkey, conditions } => {
                let pubkey = pubkey
                    .parse()
                    .map_err(|e| FfiError::InvalidCryptographicKey {
                        msg: format!("Invalid pubkey: {}", e),
                    })?;
                let conditions = conditions.map(|c| c.try_into()).transpose()?;
                Ok(Self::P2PKConditions {
                    data: pubkey,
                    conditions,
                })
            }
            SpendingConditions::HTLC { hash, conditions } => {
                let hash = hash
                    .parse()
                    .map_err(|e| FfiError::InvalidCryptographicKey {
                        msg: format!("Invalid hash: {}", e),
                    })?;
                let conditions = conditions.map(|c| c.try_into()).transpose()?;
                Ok(Self::HTLCConditions {
                    data: hash,
                    conditions,
                })
            }
        }
    }
}

/// FFI-compatible SubscriptionKind
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum SubscriptionKind {
    /// Bolt 11 Melt Quote
    Bolt11MeltQuote,
    /// Bolt 11 Mint Quote
    Bolt11MintQuote,
    /// Bolt 12 Mint Quote
    Bolt12MintQuote,
    /// Proof State
    ProofState,
}

impl From<SubscriptionKind> for cdk::nuts::nut17::Kind {
    fn from(kind: SubscriptionKind) -> Self {
        match kind {
            SubscriptionKind::Bolt11MeltQuote => cdk::nuts::nut17::Kind::Bolt11MeltQuote,
            SubscriptionKind::Bolt11MintQuote => cdk::nuts::nut17::Kind::Bolt11MintQuote,
            SubscriptionKind::Bolt12MintQuote => cdk::nuts::nut17::Kind::Bolt12MintQuote,
            SubscriptionKind::ProofState => cdk::nuts::nut17::Kind::ProofState,
        }
    }
}

impl From<cdk::nuts::nut17::Kind> for SubscriptionKind {
    fn from(kind: cdk::nuts::nut17::Kind) -> Self {
        match kind {
            cdk::nuts::nut17::Kind::Bolt11MeltQuote => SubscriptionKind::Bolt11MeltQuote,
            cdk::nuts::nut17::Kind::Bolt11MintQuote => SubscriptionKind::Bolt11MintQuote,
            cdk::nuts::nut17::Kind::Bolt12MintQuote => SubscriptionKind::Bolt12MintQuote,
            cdk::nuts::nut17::Kind::ProofState => SubscriptionKind::ProofState,
        }
    }
}

/// FFI-compatible SubscribeParams
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct SubscribeParams {
    /// Subscription kind
    pub kind: SubscriptionKind,
    /// Filters
    pub filters: Vec<String>,
    /// Subscription ID (optional, will be generated if not provided)
    pub id: Option<String>,
}

impl From<SubscribeParams> for cdk_common::subscription::Params {
    fn from(params: SubscribeParams) -> Self {
        let sub_id = params
            .id
            .map(|id| SubId::from(id.as_str()))
            .unwrap_or_else(|| {
                // Generate a random ID
                let uuid = uuid::Uuid::new_v4();
                SubId::from(uuid.to_string().as_str())
            });

        cdk::nuts::nut17::Params {
            kind: params.kind.into(),
            filters: params.filters,
            id: sub_id,
        }
    }
}

impl SubscribeParams {
    /// Convert SubscribeParams to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode SubscribeParams from JSON string
#[uniffi::export]
pub fn decode_subscribe_params(json: String) -> Result<SubscribeParams, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode SubscribeParams to JSON string
#[uniffi::export]
pub fn encode_subscribe_params(params: SubscribeParams) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&params)?)
}

/// FFI-compatible ActiveSubscription
#[derive(uniffi::Object)]
pub struct ActiveSubscription {
    inner: std::sync::Arc<tokio::sync::Mutex<cdk::wallet::subscription::ActiveSubscription>>,
    pub sub_id: String,
}

impl ActiveSubscription {
    pub(crate) fn new(
        inner: cdk::wallet::subscription::ActiveSubscription,
        sub_id: String,
    ) -> Self {
        Self {
            inner: std::sync::Arc::new(tokio::sync::Mutex::new(inner)),
            sub_id,
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl ActiveSubscription {
    /// Get the subscription ID
    pub fn id(&self) -> String {
        self.sub_id.clone()
    }

    /// Receive the next notification
    pub async fn recv(&self) -> Result<NotificationPayload, FfiError> {
        let mut guard = self.inner.lock().await;
        guard
            .recv()
            .await
            .ok_or(FfiError::Generic {
                msg: "Subscription closed".to_string(),
            })
            .map(Into::into)
    }

    /// Try to receive a notification without blocking
    pub async fn try_recv(&self) -> Result<Option<NotificationPayload>, FfiError> {
        let mut guard = self.inner.lock().await;
        guard
            .try_recv()
            .map(|opt| opt.map(Into::into))
            .map_err(|e| FfiError::Generic {
                msg: format!("Failed to receive notification: {}", e),
            })
    }
}

/// FFI-compatible NotificationPayload
#[derive(Debug, Clone, uniffi::Enum)]
pub enum NotificationPayload {
    /// Proof state update
    ProofState { proof_states: Vec<ProofStateUpdate> },
    /// Mint quote update
    MintQuoteUpdate {
        quote: std::sync::Arc<MintQuoteBolt11Response>,
    },
    /// Melt quote update
    MeltQuoteUpdate {
        quote: std::sync::Arc<MeltQuoteBolt11Response>,
    },
}

impl From<cdk::nuts::NotificationPayload<String>> for NotificationPayload {
    fn from(payload: cdk::nuts::NotificationPayload<String>) -> Self {
        match payload {
            cdk::nuts::NotificationPayload::ProofState(states) => NotificationPayload::ProofState {
                proof_states: vec![states.into()],
            },
            cdk::nuts::NotificationPayload::MintQuoteBolt11Response(quote_resp) => {
                NotificationPayload::MintQuoteUpdate {
                    quote: std::sync::Arc::new(quote_resp.into()),
                }
            }
            cdk::nuts::NotificationPayload::MeltQuoteBolt11Response(quote_resp) => {
                NotificationPayload::MeltQuoteUpdate {
                    quote: std::sync::Arc::new(quote_resp.into()),
                }
            }
            _ => {
                // For now, handle other notification types as empty ProofState
                NotificationPayload::ProofState {
                    proof_states: vec![],
                }
            }
        }
    }
}

/// FFI-compatible ProofStateUpdate
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ProofStateUpdate {
    /// Y value (hash_to_curve of secret)
    pub y: String,
    /// Current state
    pub state: ProofState,
    /// Optional witness data
    pub witness: Option<String>,
}

impl From<cdk::nuts::nut07::ProofState> for ProofStateUpdate {
    fn from(proof_state: cdk::nuts::nut07::ProofState) -> Self {
        Self {
            y: proof_state.y.to_string(),
            state: proof_state.state.into(),
            witness: proof_state.witness.map(|w| format!("{:?}", w)),
        }
    }
}

impl ProofStateUpdate {
    /// Convert ProofStateUpdate to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode ProofStateUpdate from JSON string
#[uniffi::export]
pub fn decode_proof_state_update(json: String) -> Result<ProofStateUpdate, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode ProofStateUpdate to JSON string
#[uniffi::export]
pub fn encode_proof_state_update(update: ProofStateUpdate) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&update)?)
}

/// FFI-compatible KeySetInfo
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct KeySetInfo {
    pub id: String,
    pub unit: CurrencyUnit,
    pub active: bool,
    /// Input fee per thousand (ppk)
    pub input_fee_ppk: u64,
}

impl From<cdk_common::nuts::KeySetInfo> for KeySetInfo {
    fn from(keyset: cdk_common::nuts::KeySetInfo) -> Self {
        Self {
            id: keyset.id.to_string(),
            unit: keyset.unit.into(),
            active: keyset.active,
            input_fee_ppk: keyset.input_fee_ppk,
        }
    }
}

impl From<KeySetInfo> for cdk_common::nuts::KeySetInfo {
    fn from(keyset: KeySetInfo) -> Self {
        use std::str::FromStr;
        Self {
            id: cdk_common::nuts::Id::from_str(&keyset.id).unwrap_or_else(|_| {
                // Create a dummy keyset for empty mint keys
                use std::collections::BTreeMap;
                let empty_map = BTreeMap::new();
                let empty_keys = cdk_common::nut01::MintKeys::new(empty_map);
                cdk_common::nuts::Id::from(&empty_keys)
            }),
            unit: keyset.unit.into(),
            active: keyset.active,
            final_expiry: None,
            input_fee_ppk: keyset.input_fee_ppk,
        }
    }
}

impl KeySetInfo {
    /// Convert KeySetInfo to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode KeySetInfo from JSON string
#[uniffi::export]
pub fn decode_key_set_info(json: String) -> Result<KeySetInfo, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode KeySetInfo to JSON string
#[uniffi::export]
pub fn encode_key_set_info(info: KeySetInfo) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&info)?)
}

/// FFI-compatible PublicKey
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
pub struct PublicKey {
    /// Hex-encoded public key
    pub hex: String,
}

impl From<cdk_common::nuts::PublicKey> for PublicKey {
    fn from(key: cdk_common::nuts::PublicKey) -> Self {
        Self {
            hex: key.to_string(),
        }
    }
}

impl TryFrom<PublicKey> for cdk_common::nuts::PublicKey {
    type Error = FfiError;

    fn try_from(key: PublicKey) -> Result<Self, Self::Error> {
        key.hex
            .parse()
            .map_err(|e| FfiError::InvalidCryptographicKey {
                msg: format!("Invalid public key: {}", e),
            })
    }
}

/// FFI-compatible Keys (simplified - contains only essential info)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct Keys {
    /// Keyset ID
    pub id: String,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Map of amount to public key hex (simplified from BTreeMap)
    pub keys: HashMap<u64, String>,
}

impl From<cdk_common::nuts::Keys> for Keys {
    fn from(keys: cdk_common::nuts::Keys) -> Self {
        // Keys doesn't have id and unit - we'll need to get these from context
        // For now, use placeholder values
        Self {
            id: "unknown".to_string(), // This should come from KeySet
            unit: CurrencyUnit::Sat,   // This should come from KeySet
            keys: keys
                .keys()
                .iter()
                .map(|(amount, pubkey)| (u64::from(*amount), pubkey.to_string()))
                .collect(),
        }
    }
}

impl TryFrom<Keys> for cdk_common::nuts::Keys {
    type Error = FfiError;

    fn try_from(keys: Keys) -> Result<Self, Self::Error> {
        use std::collections::BTreeMap;
        use std::str::FromStr;

        // Convert the HashMap to BTreeMap with proper types
        let mut keys_map = BTreeMap::new();
        for (amount_u64, pubkey_hex) in keys.keys {
            let amount = cashu::Amount::from(amount_u64);
            let pubkey = cashu::PublicKey::from_str(&pubkey_hex)
                .map_err(|e| FfiError::InvalidCryptographicKey { msg: e.to_string() })?;
            keys_map.insert(amount, pubkey);
        }

        Ok(cdk_common::nuts::Keys::new(keys_map))
    }
}

impl Keys {
    /// Convert Keys to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode Keys from JSON string
#[uniffi::export]
pub fn decode_keys(json: String) -> Result<Keys, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode Keys to JSON string
#[uniffi::export]
pub fn encode_keys(keys: Keys) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&keys)?)
}

/// FFI-compatible KeySet
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct KeySet {
    /// Keyset ID
    pub id: String,
    /// Currency unit  
    pub unit: CurrencyUnit,
    /// The keys (map of amount to public key hex)
    pub keys: HashMap<u64, String>,
    /// Optional expiry timestamp
    pub final_expiry: Option<u64>,
}

impl From<cashu::KeySet> for KeySet {
    fn from(keyset: cashu::KeySet) -> Self {
        Self {
            id: keyset.id.to_string(),
            unit: keyset.unit.into(),
            keys: keyset
                .keys
                .keys()
                .iter()
                .map(|(amount, pubkey)| (u64::from(*amount), pubkey.to_string()))
                .collect(),
            final_expiry: keyset.final_expiry,
        }
    }
}

impl TryFrom<KeySet> for cashu::KeySet {
    type Error = FfiError;

    fn try_from(keyset: KeySet) -> Result<Self, Self::Error> {
        use std::collections::BTreeMap;
        use std::str::FromStr;

        // Convert id
        let id = cashu::Id::from_str(&keyset.id)
            .map_err(|e| FfiError::Serialization { msg: e.to_string() })?;

        // Convert unit
        let unit: cashu::CurrencyUnit = keyset.unit.into();

        // Convert keys
        let mut keys_map = BTreeMap::new();
        for (amount_u64, pubkey_hex) in keyset.keys {
            let amount = cashu::Amount::from(amount_u64);
            let pubkey = cashu::PublicKey::from_str(&pubkey_hex)
                .map_err(|e| FfiError::InvalidCryptographicKey { msg: e.to_string() })?;
            keys_map.insert(amount, pubkey);
        }
        let keys = cashu::Keys::new(keys_map);

        Ok(cashu::KeySet {
            id,
            unit,
            keys,
            final_expiry: keyset.final_expiry,
        })
    }
}

impl KeySet {
    /// Convert KeySet to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode KeySet from JSON string
#[uniffi::export]
pub fn decode_key_set(json: String) -> Result<KeySet, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode KeySet to JSON string
#[uniffi::export]
pub fn encode_key_set(keyset: KeySet) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&keyset)?)
}

/// FFI-compatible ProofInfo
#[derive(Debug, Clone, uniffi::Record)]
pub struct ProofInfo {
    /// Proof
    pub proof: std::sync::Arc<Proof>,
    /// Y value (hash_to_curve of secret)
    pub y: PublicKey,
    /// Mint URL
    pub mint_url: MintUrl,
    /// Proof state
    pub state: ProofState,
    /// Proof Spending Conditions
    pub spending_condition: Option<SpendingConditions>,
    /// Currency unit
    pub unit: CurrencyUnit,
}

impl From<cdk_common::common::ProofInfo> for ProofInfo {
    fn from(info: cdk_common::common::ProofInfo) -> Self {
        Self {
            proof: std::sync::Arc::new(info.proof.into()),
            y: info.y.into(),
            mint_url: info.mint_url.into(),
            state: info.state.into(),
            spending_condition: info.spending_condition.map(Into::into),
            unit: info.unit.into(),
        }
    }
}

/// Decode ProofInfo from JSON string
#[uniffi::export]
pub fn decode_proof_info(json: String) -> Result<ProofInfo, FfiError> {
    let info: cdk_common::common::ProofInfo = serde_json::from_str(&json)?;
    Ok(info.into())
}

/// Encode ProofInfo to JSON string
#[uniffi::export]
pub fn encode_proof_info(info: ProofInfo) -> Result<String, FfiError> {
    // Convert to cdk_common::common::ProofInfo for serialization
    let cdk_info = cdk_common::common::ProofInfo {
        proof: info.proof.inner.clone(),
        y: info.y.try_into()?,
        mint_url: info.mint_url.try_into()?,
        state: info.state.into(),
        spending_condition: info.spending_condition.and_then(|c| c.try_into().ok()),
        unit: info.unit.into(),
    };
    Ok(serde_json::to_string(&cdk_info)?)
}

// State enum removed - using ProofState instead

/// FFI-compatible Id (for keyset IDs)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
pub struct Id {
    pub hex: String,
}

impl From<cdk_common::nuts::Id> for Id {
    fn from(id: cdk_common::nuts::Id) -> Self {
        Self {
            hex: id.to_string(),
        }
    }
}

impl From<Id> for cdk_common::nuts::Id {
    fn from(id: Id) -> Self {
        use std::str::FromStr;
        Self::from_str(&id.hex).unwrap_or_else(|_| {
            // Create a dummy keyset for empty mint keys
            use std::collections::BTreeMap;
            let empty_map = BTreeMap::new();
            let empty_keys = cdk_common::nut01::MintKeys::new(empty_map);
            cdk_common::nuts::Id::from(&empty_keys)
        })
    }
}
