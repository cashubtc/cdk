//! Node.js native addon for CDK React Native integration tests.
//!
//! This thin wrapper exposes the same wallet operations that the Dart and Swift
//! binding tests exercise, allowing Jest to make real Rust FFI calls.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use cdk_ffi::database::WalletStore;
use cdk_ffi::wallet::{generate_mnemonic as ffi_generate_mnemonic, WalletConfig};
use cdk_ffi::Wallet as FfiWallet;

// ---------------------------------------------------------------------------
// Helper: convert cdk_ffi::FfiError → napi::Error
// ---------------------------------------------------------------------------

fn to_napi_err(e: cdk_ffi::FfiError) -> napi::Error {
    napi::Error::from_reason(format!("{e}"))
}

// ---------------------------------------------------------------------------
// MintQuote result object
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct MintQuoteResult {
    pub id: String,
    pub request: String,
    pub state: String,
}

// ---------------------------------------------------------------------------
// Proof result object
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct ProofResult {
    pub amount: i64,
    pub secret: String,
    pub c: String,
    pub keyset_id: String,
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Generate a random BIP-39 mnemonic (12 words).
#[napi]
pub fn generate_mnemonic() -> Result<String> {
    ffi_generate_mnemonic().map_err(to_napi_err)
}

// ---------------------------------------------------------------------------
// Wallet wrapper
// ---------------------------------------------------------------------------

#[napi]
pub struct Wallet {
    inner: Arc<FfiWallet>,
}

#[napi]
impl Wallet {
    /// Create a new Wallet backed by SQLite.
    ///
    /// This mirrors the Dart/Swift test setup:
    ///   Wallet(mintUrl, unit: .sat, mnemonic, store: .sqlite(path), config)
    #[napi(constructor)]
    pub fn new(
        mint_url: String,
        unit: String,
        mnemonic: String,
        db_path: String,
        target_proof_count: Option<i32>,
    ) -> Result<Self> {
        let currency_unit = match unit.to_lowercase().as_str() {
            "sat" => cdk_ffi::CurrencyUnit::Sat,
            "msat" => cdk_ffi::CurrencyUnit::Msat,
            "usd" => cdk_ffi::CurrencyUnit::Usd,
            "eur" => cdk_ffi::CurrencyUnit::Eur,
            other => {
                return Err(napi::Error::from_reason(format!(
                    "Unknown currency unit: {other}"
                )))
            }
        };

        let store = WalletStore::Sqlite { path: db_path };
        let config = WalletConfig {
            target_proof_count: target_proof_count.map(|v| v as u32),
        };

        let wallet = FfiWallet::new(mint_url, currency_unit, mnemonic, store, config)
            .map_err(to_napi_err)?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Get total balance.
    #[napi]
    pub async fn total_balance(&self) -> Result<i64> {
        let amount = self.inner.total_balance().await.map_err(to_napi_err)?;
        Ok(amount.value as i64)
    }

    /// Create a mint quote.
    #[napi]
    pub async fn mint_quote(
        &self,
        payment_method: String,
        amount: Option<i64>,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<MintQuoteResult> {
        let method = match payment_method.to_lowercase().as_str() {
            "bolt11" => cdk_ffi::PaymentMethod::Bolt11,
            "bolt12" => cdk_ffi::PaymentMethod::Bolt12,
            other => cdk_ffi::PaymentMethod::Custom {
                method: other.to_string(),
            },
        };

        let ffi_amount = amount.map(|v| cdk_ffi::Amount { value: v as u64 });

        let quote = self
            .inner
            .mint_quote(method, ffi_amount, description, extra)
            .await
            .map_err(to_napi_err)?;

        Ok(MintQuoteResult {
            id: quote.id,
            request: quote.request,
            state: format!("{:?}", quote.state),
        })
    }

    /// Mint tokens for a paid quote.
    #[napi]
    pub async fn mint(
        &self,
        quote_id: String,
        split_target: String,
        _spending_conditions: Option<String>,
    ) -> Result<Vec<ProofResult>> {
        let target = match split_target.to_lowercase().as_str() {
            "none" => cdk_ffi::SplitTarget::None,
            _ => cdk_ffi::SplitTarget::None,
        };

        let proofs = self
            .inner
            .mint(quote_id, target, None)
            .await
            .map_err(to_napi_err)?;

        Ok(proofs
            .into_iter()
            .map(|p| ProofResult {
                amount: p.amount.value as i64,
                secret: p.secret,
                c: p.c,
                keyset_id: p.keyset_id,
            })
            .collect())
    }
}
