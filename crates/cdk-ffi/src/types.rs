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
#[derive(Debug, Clone, uniffi::Record)]
pub struct Token {
    pub token: String,
}

impl Token {
    pub fn new(token: String) -> Self {
        Self { token }
    }
}

impl From<cdk::nuts::Token> for Token {
    fn from(token: cdk::nuts::Token) -> Self {
        Self {
            token: token.to_string(),
        }
    }
}

impl TryFrom<Token> for cdk::nuts::Token {
    type Error = FfiError;

    fn try_from(token: Token) -> Result<Self, Self::Error> {
        cdk::nuts::Token::from_str(&token.token)
            .map_err(|e| FfiError::Generic { msg: e.to_string() })
    }
}

/// FFI-compatible Send options
#[derive(Debug, Clone, uniffi::Record)]
pub struct SendOptions {
    pub offline: bool,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self { offline: false }
    }
}

impl From<SendOptions> for cdk::wallet::SendOptions {
    fn from(opts: SendOptions) -> Self {
        use cdk::amount::SplitTarget;
        use cdk_common::wallet::SendKind;

        let send_kind = if opts.offline {
            SendKind::OfflineExact
        } else {
            SendKind::OnlineExact
        };

        cdk::wallet::SendOptions {
            memo: None,
            conditions: None,
            amount_split_target: SplitTarget::None,
            send_kind,
            include_fee: false,
            max_proofs: None,
            metadata: HashMap::new(),
        }
    }
}

/// FFI-compatible Receive options
#[derive(Debug, Clone, uniffi::Record)]
pub struct ReceiveOptions {
    pub check_spendable: bool,
}

impl Default for ReceiveOptions {
    fn default() -> Self {
        Self {
            check_spendable: true,
        }
    }
}

impl From<ReceiveOptions> for cdk::wallet::ReceiveOptions {
    fn from(_opts: ReceiveOptions) -> Self {
        use std::collections::HashMap;

        use cdk::amount::SplitTarget;

        cdk::wallet::ReceiveOptions {
            amount_split_target: SplitTarget::None,
            p2pk_signing_keys: Vec::new(),
            preimages: Vec::new(),
            metadata: HashMap::new(),
        }
    }
}

/// FFI-compatible Proof
#[derive(Debug, Clone, uniffi::Record)]
pub struct Proof {
    pub amount: Amount,
    pub secret: String,
    pub c: String,
    pub witness: Option<String>,
}

impl From<cdk::nuts::Proof> for Proof {
    fn from(proof: cdk::nuts::Proof) -> Self {
        Self {
            amount: proof.amount.into(),
            secret: proof.secret.to_string(),
            c: proof.c.to_string(),
            witness: proof
                .witness
                .map(|w| serde_json::to_string(&w).unwrap_or_default()),
        }
    }
}

impl TryFrom<Proof> for cdk::nuts::Proof {
    type Error = FfiError;

    fn try_from(proof: Proof) -> Result<Self, Self::Error> {
        use cdk::nuts::{Id, PublicKey};
        use cdk::secret::Secret;

        let secret = Secret::from_str(&proof.secret)
            .map_err(|e| FfiError::Generic { msg: e.to_string() })?;
        let c =
            PublicKey::from_str(&proof.c).map_err(|e| FfiError::Generic { msg: e.to_string() })?;
        let witness = if let Some(w) = proof.witness {
            Some(serde_json::from_str(&w).map_err(|e| FfiError::Generic { msg: e.to_string() })?)
        } else {
            None
        };

        Ok(cdk::nuts::Proof {
            amount: proof.amount.into(),
            secret,
            c,
            witness,
            keyset_id: Id::from_bytes(&[0u8; 8])
                .unwrap_or_else(|_| panic!("Failed to create keyset ID")),
            dleq: None,
        })
    }
}

/// FFI-compatible Proofs (vector of Proof)
pub type Proofs = Vec<Proof>;

/// FFI-compatible MintQuote
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintQuote {
    pub id: String,
    pub amount: Amount,
    pub unit: CurrencyUnit,
    pub request: String,
    pub state: QuoteState,
    pub expiry: Option<u64>,
}

impl From<cdk::wallet::MintQuote> for MintQuote {
    fn from(quote: cdk::wallet::MintQuote) -> Self {
        Self {
            id: quote.id,
            // Handle optional amount
            amount: quote.amount.unwrap_or_default().into(),
            unit: quote.unit.into(),
            request: quote.request,
            state: QuoteState::Unpaid, // Simplified mapping
            expiry: Some(quote.expiry),
        }
    }
}

/// FFI-compatible MeltQuote
#[derive(Debug, Clone, uniffi::Record)]
pub struct MeltQuote {
    pub id: String,
    pub amount: Amount,
    pub unit: CurrencyUnit,
    pub request: String,
    pub fee_reserve: Amount,
    pub state: QuoteState,
    pub expiry: Option<u64>,
}

impl From<cdk::wallet::MeltQuote> for MeltQuote {
    fn from(quote: cdk::wallet::MeltQuote) -> Self {
        Self {
            id: quote.id,
            amount: quote.amount.into(),
            unit: quote.unit.into(),
            request: quote.request,
            fee_reserve: quote.fee_reserve.into(),
            state: QuoteState::Unpaid, // Simplified mapping
            expiry: Some(quote.expiry),
        }
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
        let proofs = prepared.proofs().iter().cloned().map(Into::into).collect();
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

/// FFI-compatible SplitTarget
#[derive(Debug, Clone, uniffi::Enum)]
pub enum SplitTarget {
    None,
    Value { target: Amount },
    Send { count: u32 },
}

impl From<SplitTarget> for cdk::amount::SplitTarget {
    fn from(target: SplitTarget) -> Self {
        match target {
            SplitTarget::None => cdk::amount::SplitTarget::None,
            SplitTarget::Value { target } => cdk::amount::SplitTarget::Value(target.into()),
            SplitTarget::Send { count } => {
                // Create values for split target - this is a simplified approach
                let values: Vec<cdk::Amount> = (0..count).map(|_| cdk::Amount::from(1)).collect();
                cdk::amount::SplitTarget::Values(values)
            }
        }
    }
}

/// FFI-compatible MeltOptions
#[derive(Debug, Clone, uniffi::Record)]
pub struct MeltOptions {
    pub fee_reserve: Option<Amount>,
}

impl Default for MeltOptions {
    fn default() -> Self {
        Self { fee_reserve: None }
    }
}

// MeltOptions type may not exist in current CDK version
// Using a simplified approach for melt options
