//! Nostr Wallet Connect (NIP-47) integration for the CDK wallet.
//!
//! This module bridges the transport-agnostic [`cdk_nwc`] wallet service to a
//! Cashu [`Wallet`]. [`WalletNwcHandler`] implements [`cdk_nwc::NwcRequestHandler`]
//! by mapping each NIP-47 command onto wallet operations:
//!
//! | NIP-47 command      | Wallet operation                                   |
//! |---------------------|----------------------------------------------------|
//! | `get_info`          | static capability advertisement                    |
//! | `get_balance`       | [`Wallet::total_balance`]                           |
//! | `make_invoice`      | [`Wallet::mint_quote`] (bolt11)                     |
//! | `pay_invoice`       | [`Wallet::melt_quote`] + [`Wallet::prepare_melt`]   |
//! | `lookup_invoice`    | transaction history + active mint quotes           |
//! | `list_transactions` | [`Wallet::list_transactions`]                       |
//!
//! ## Units
//!
//! All NIP-47 amounts are **millisatoshis**. Cashu wallets denominated in `Sat`
//! are converted with a ×1000 factor; sub-satoshi millisat amounts that cannot
//! be represented exactly are rejected rather than silently rounded. Only `Sat`
//! and `Msat` wallets are supported.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::Network;
use cdk_common::nut00::KnownMethod;
use cdk_common::wallet::{Transaction, TransactionDirection};
use cdk_common::{PaymentMethod, SECP256K1};
use cdk_nwc::nip47::{
    ErrorCode, GetBalanceResponse, GetInfoResponse, ListTransactionsRequest, LookupInvoiceRequest,
    LookupInvoiceResponse, MakeInvoiceRequest, MakeInvoiceResponse, Method, NIP47Error,
    PayInvoiceRequest, PayInvoiceResponse, TransactionType,
};
use cdk_nwc::service::SUPPORTED_METHODS;
use lightning_invoice::Bolt11Invoice;
use nostr_sdk::Timestamp;
use tracing::instrument;

use crate::error::Error;
use crate::nuts::{CurrencyUnit, SecretKey};
use crate::{amount, Amount, Wallet};

/// Derive the NWC wallet-service secret key from a wallet seed.
///
/// Uses NIP-06 BIP-32 derivation under account index `1`
/// (`m/44'/1237'/1'/0/0`), keeping it distinct from the npub.cash key
/// (`m/44'/1237'/0'/0/0`) so a single seed yields independent identities. The
/// derived key never equals raw seed material, so it cannot be used to recover
/// the seed. Deriving from the seed keeps the connection URI stable across
/// restarts.
///
/// # Errors
///
/// Returns an error if the key derivation fails.
pub fn derive_nwc_secret_key_from_seed(seed: &[u8; 64]) -> Result<SecretKey, Error> {
    let path = DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(44)?,
        ChildNumber::from_hardened_idx(1237)?,
        ChildNumber::from_hardened_idx(1)?,
        ChildNumber::from_normal_idx(0)?,
        ChildNumber::from_normal_idx(0)?,
    ]);

    let xpriv = Xpriv::new_master(Network::Bitcoin, seed)?;

    Ok(SecretKey::from(
        xpriv.derive_priv(&SECP256K1, &path)?.private_key,
    ))
}

impl Wallet {
    /// Derive the NWC wallet-service secret key from this wallet's seed.
    ///
    /// See [`derive_nwc_secret_key_from_seed`] for the derivation path.
    ///
    /// # Errors
    ///
    /// Returns an error if the key derivation fails.
    pub fn derive_nwc_secret_key(&self) -> Result<SecretKey, Error> {
        derive_nwc_secret_key_from_seed(&self.seed)
    }

    /// Build a [`WalletNwcHandler`] for this wallet.
    ///
    /// `max_payment_msat` optionally caps the amount of any single `pay_invoice`
    /// request (in millisatoshis); pass `None` for no cap.
    pub fn nwc_handler(&self, max_payment_msat: Option<u64>) -> WalletNwcHandler {
        WalletNwcHandler::new(Arc::new(self.clone()), max_payment_msat)
    }
}

/// A [`cdk_nwc::NwcRequestHandler`] backed by a Cashu [`Wallet`].
#[derive(Debug, Clone)]
pub struct WalletNwcHandler {
    wallet: Arc<Wallet>,
    max_payment_msat: Option<u64>,
}

impl WalletNwcHandler {
    /// Create a new handler.
    ///
    /// `max_payment_msat` optionally caps the amount of any single `pay_invoice`
    /// request (in millisatoshis).
    pub fn new(wallet: Arc<Wallet>, max_payment_msat: Option<u64>) -> Self {
        Self {
            wallet,
            max_payment_msat,
        }
    }
}

/// Build a NIP-47 error with the given code.
fn nip47_err(code: ErrorCode, message: impl Into<String>) -> NIP47Error {
    NIP47Error {
        code,
        message: message.into(),
    }
}

/// Map an [`Amount`] conversion error to a NIP-47 error.
fn amount_conversion_error(err: amount::Error, unit: &CurrencyUnit) -> NIP47Error {
    match err {
        amount::Error::CannotConvertUnits => {
            nip47_err(ErrorCode::Other, format!("unsupported wallet unit: {unit}"))
        }
        amount::Error::AmountOverflow => nip47_err(
            ErrorCode::Internal,
            "amount overflow converting wallet units",
        ),
        other => nip47_err(ErrorCode::Internal, other.to_string()),
    }
}

/// Convert a wallet [`Amount`] to millisatoshis for the given unit.
fn amount_to_msat(amount: Amount, unit: &CurrencyUnit) -> Result<u64, NIP47Error> {
    let value = u64::from(amount);
    Amount::new(value, unit.clone())
        .to_msat()
        .map_err(|e| amount_conversion_error(e, unit))
}

/// Convert millisatoshis to a wallet [`Amount`] for the given unit.
///
/// For `Sat` wallets, millisat amounts that are not whole satoshis are rejected
/// rather than rounded.
fn msat_to_amount(msat: u64, unit: &CurrencyUnit) -> Result<Amount, NIP47Error> {
    if unit == &CurrencyUnit::Sat && msat % 1000 != 0 {
        return Err(nip47_err(
            ErrorCode::Other,
            "sub-satoshi amounts are not supported by this wallet",
        ));
    }

    let amount = Amount::new(msat, CurrencyUnit::Msat);
    match unit {
        CurrencyUnit::Sat => amount.to_sat().map(Amount::from),
        CurrencyUnit::Msat => amount.to_msat().map(Amount::from),
        other => amount.convert_to(other).map(Into::into),
    }
    .map_err(|e| amount_conversion_error(e, unit))
}

/// Extract the hex payment hash from a bolt11 invoice string.
fn payment_hash_of(invoice: &str) -> Option<String> {
    Bolt11Invoice::from_str(invoice)
        .ok()
        .map(|i| i.payment_hash().to_string())
}

/// Extract the creation timestamp from a bolt11 invoice string.
fn invoice_created_at(invoice: &str) -> Option<Timestamp> {
    Bolt11Invoice::from_str(invoice)
        .ok()
        .map(|i| Timestamp::from(i.duration_since_epoch().as_secs()))
}

/// Map a wallet [`Error`] from a melt operation to the appropriate NIP-47 code.
fn melt_error(err: &Error) -> NIP47Error {
    match err {
        Error::InsufficientFunds => nip47_err(
            ErrorCode::InsufficientBalance,
            "insufficient balance to pay invoice",
        ),
        Error::PaymentFailed => nip47_err(ErrorCode::PaymentFailed, "payment failed"),
        other => nip47_err(ErrorCode::Internal, other.to_string()),
    }
}

/// Map a wallet [`Error`] from a mint quote operation to the appropriate NIP-47 code.
fn mint_quote_error(err: &Error) -> NIP47Error {
    match err {
        Error::InvoiceDescriptionUnsupported => nip47_err(
            ErrorCode::Other,
            "mint does not support invoice descriptions for bolt11 mint quotes",
        ),
        Error::UnsupportedUnit => nip47_err(
            ErrorCode::Other,
            "wallet unit is not supported by this mint for bolt11 mint quotes",
        ),
        other => nip47_err(ErrorCode::Internal, other.to_string()),
    }
}

/// Convert a wallet [`Transaction`] into a NIP-47 transaction object.
fn transaction_to_nip47(
    tx: &Transaction,
    unit: &CurrencyUnit,
) -> Result<LookupInvoiceResponse, NIP47Error> {
    let transaction_type = match tx.direction {
        TransactionDirection::Incoming => TransactionType::Incoming,
        TransactionDirection::Outgoing => TransactionType::Outgoing,
    };

    let payment_hash = tx
        .payment_request
        .as_deref()
        .and_then(payment_hash_of)
        .unwrap_or_default();

    Ok(LookupInvoiceResponse {
        transaction_type: Some(transaction_type),
        state: Some(cdk_nwc::nip47::TransactionState::Settled),
        invoice: tx.payment_request.clone(),
        description: tx.memo.clone(),
        description_hash: None,
        preimage: tx.payment_proof.clone(),
        payment_hash,
        amount: amount_to_msat(tx.amount, unit)?,
        fees_paid: amount_to_msat(tx.fee, unit)?,
        created_at: Timestamp::from(tx.timestamp),
        expires_at: None,
        settled_at: Some(Timestamp::from(tx.timestamp)),
        metadata: None,
    })
}

#[async_trait]
impl cdk_nwc::NwcRequestHandler for WalletNwcHandler {
    #[instrument(skip(self))]
    async fn get_info(&self) -> Result<GetInfoResponse, NIP47Error> {
        let methods = SUPPORTED_METHODS
            .iter()
            .filter_map(|m| Method::from_str(m).ok())
            .collect();

        Ok(GetInfoResponse {
            alias: Some("CDK Cashu Wallet".to_string()),
            color: None,
            pubkey: None,
            network: Some("mainnet".to_string()),
            block_height: None,
            block_hash: None,
            methods,
            notifications: Vec::new(),
        })
    }

    #[instrument(skip(self))]
    async fn get_balance(&self) -> Result<GetBalanceResponse, NIP47Error> {
        let balance = self
            .wallet
            .total_balance()
            .await
            .map_err(|e| nip47_err(ErrorCode::Internal, e.to_string()))?;

        Ok(GetBalanceResponse {
            balance: amount_to_msat(balance, &self.wallet.unit)?,
        })
    }

    #[instrument(skip(self))]
    async fn make_invoice(
        &self,
        request: MakeInvoiceRequest,
    ) -> Result<MakeInvoiceResponse, NIP47Error> {
        if request.description_hash.is_some() {
            return Err(nip47_err(
                ErrorCode::Other,
                "description_hash is not supported by Cashu mint quotes",
            ));
        }

        let amount = msat_to_amount(request.amount, &self.wallet.unit)?;

        let quote = self
            .wallet
            .mint_quote(
                PaymentMethod::Known(KnownMethod::Bolt11),
                Some(amount),
                request.description.clone(),
                None,
            )
            .await
            .map_err(|e| mint_quote_error(&e))?;

        let payment_hash = payment_hash_of(&quote.request);

        Ok(MakeInvoiceResponse {
            invoice: quote.request,
            payment_hash,
            description: request.description,
            description_hash: request.description_hash,
            preimage: None,
            amount: Some(request.amount),
            created_at: None,
            expires_at: Some(Timestamp::from(quote.expiry)),
        })
    }

    #[instrument(skip(self))]
    async fn pay_invoice(
        &self,
        request: PayInvoiceRequest,
    ) -> Result<PayInvoiceResponse, NIP47Error> {
        let invoice = Bolt11Invoice::from_str(&request.invoice)
            .map_err(|e| nip47_err(ErrorCode::Other, format!("invalid bolt11 invoice: {e}")))?;

        // The invoice must carry its own amount: paying amountless invoices
        // would require an amount override, which is not supported here.
        let invoice_msat = invoice.amount_milli_satoshis().ok_or_else(|| {
            nip47_err(
                ErrorCode::Other,
                "amountless invoices are not supported; invoice must specify an amount",
            )
        })?;

        // A redundant `amount` is accepted only when it matches the invoice;
        // a mismatch is rejected rather than silently paying a different sum.
        if let Some(requested) = request.amount {
            if requested != invoice_msat {
                return Err(nip47_err(
                    ErrorCode::Other,
                    "requested amount does not match the invoice amount",
                ));
            }
        }

        // Per-payment cap enforcement (defense in depth, before any state changes).
        if let Some(max_payment_msat) = self.max_payment_msat {
            if invoice_msat > max_payment_msat {
                return Err(nip47_err(
                    ErrorCode::QuotaExceeded,
                    "payment exceeds max_payment_msat",
                ));
            }
        }

        let quote = self
            .wallet
            .melt_quote(
                PaymentMethod::Known(KnownMethod::Bolt11),
                request.invoice.clone(),
                None,
                None,
            )
            .await
            .map_err(|e| melt_error(&e))?;

        let prepared = self
            .wallet
            .prepare_melt(&quote.id, HashMap::new())
            .await
            .map_err(|e| melt_error(&e))?;

        let finalized = prepared.confirm().await.map_err(|e| melt_error(&e))?;

        let preimage = finalized.payment_proof().unwrap_or_default().to_string();
        let fees_paid = amount_to_msat(finalized.fee_paid(), &self.wallet.unit)?;

        Ok(PayInvoiceResponse {
            preimage,
            fees_paid: Some(fees_paid),
        })
    }

    #[instrument(skip(self))]
    async fn lookup_invoice(
        &self,
        request: LookupInvoiceRequest,
    ) -> Result<LookupInvoiceResponse, NIP47Error> {
        let target_hash = request
            .payment_hash
            .clone()
            .or_else(|| request.invoice.as_deref().and_then(payment_hash_of))
            .ok_or_else(|| {
                nip47_err(
                    ErrorCode::Other,
                    "either payment_hash or invoice is required",
                )
            })?;

        let unit = &self.wallet.unit;

        // Settled transactions (incoming and outgoing).
        let transactions = self
            .wallet
            .list_transactions(None)
            .await
            .map_err(|e| nip47_err(ErrorCode::Internal, e.to_string()))?;

        for tx in &transactions {
            if tx
                .payment_request
                .as_deref()
                .and_then(payment_hash_of)
                .as_deref()
                == Some(target_hash.as_str())
            {
                return transaction_to_nip47(tx, unit);
            }
        }

        // Outstanding (unpaid) invoices we issued.
        let quotes = self
            .wallet
            .get_active_mint_quotes()
            .await
            .map_err(|e| nip47_err(ErrorCode::Internal, e.to_string()))?;

        for quote in quotes {
            if payment_hash_of(&quote.request).as_deref() == Some(target_hash.as_str()) {
                let amount = quote
                    .amount
                    .map(|a| amount_to_msat(a, unit))
                    .transpose()?
                    .unwrap_or_default();
                let created_at =
                    invoice_created_at(&quote.request).unwrap_or_else(|| Timestamp::from(0));

                return Ok(LookupInvoiceResponse {
                    transaction_type: Some(TransactionType::Incoming),
                    state: Some(cdk_nwc::nip47::TransactionState::Pending),
                    invoice: Some(quote.request.clone()),
                    description: None,
                    description_hash: None,
                    preimage: None,
                    payment_hash: target_hash,
                    amount,
                    fees_paid: 0,
                    created_at,
                    expires_at: Some(Timestamp::from(quote.expiry)),
                    settled_at: None,
                    metadata: None,
                });
            }
        }

        Err(nip47_err(ErrorCode::NotFound, "invoice not found"))
    }

    #[instrument(skip(self))]
    async fn list_transactions(
        &self,
        request: ListTransactionsRequest,
    ) -> Result<Vec<LookupInvoiceResponse>, NIP47Error> {
        let direction = request.transaction_type.map(|t| match t {
            TransactionType::Incoming => TransactionDirection::Incoming,
            TransactionType::Outgoing => TransactionDirection::Outgoing,
        });

        let unit = &self.wallet.unit;

        let transactions = self
            .wallet
            .list_transactions(direction)
            .await
            .map_err(|e| nip47_err(ErrorCode::Internal, e.to_string()))?;

        let from = request.from.map(|t| t.as_secs());
        let until = request.until.map(|t| t.as_secs());

        let filtered = transactions.into_iter().filter(|tx| {
            from.is_none_or(|f| tx.timestamp >= f) && until.is_none_or(|u| tx.timestamp <= u)
        });

        let offset = request.offset.unwrap_or(0) as usize;
        let limit = request.limit.map(|l| l as usize);

        let mut out = Vec::new();
        for tx in filtered.skip(offset) {
            if let Some(limit) = limit {
                if out.len() >= limit {
                    break;
                }
            }
            out.push(transaction_to_nip47(&tx, unit)?);
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cdk_common::database::WalletDatabase;
    use cdk_common::mint_url::MintUrl;
    use cdk_common::wallet::MintQuote;

    use crate::nuts::{MintInfo, MintMethodSettings, NUT04Settings, Nuts};
    use crate::wallet::test_utils::MockMintConnector;

    use super::*;

    const TEST_BOLT11: &str = "lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq";

    #[test]
    fn sat_amounts_convert_to_and_from_msat() {
        assert_eq!(
            amount_to_msat(Amount::from(5u64), &CurrencyUnit::Sat).expect("to msat"),
            5000
        );
        assert_eq!(
            amount_to_msat(Amount::from(7u64), &CurrencyUnit::Msat).expect("msat passthrough"),
            7
        );
        assert_eq!(
            msat_to_amount(5000, &CurrencyUnit::Sat).expect("from msat"),
            Amount::from(5u64)
        );
        assert_eq!(
            msat_to_amount(9, &CurrencyUnit::Msat).expect("msat passthrough"),
            Amount::from(9u64)
        );
    }

    #[test]
    fn sub_satoshi_msat_is_rejected_for_sat_wallet() {
        let err = msat_to_amount(500, &CurrencyUnit::Sat).expect_err("sub-sat rejected");
        assert_eq!(err.code, ErrorCode::Other);
    }

    #[test]
    fn nwc_service_key_is_distinct_from_npubcash_key() {
        let seed = [0x24u8; 64];
        let nwc = derive_nwc_secret_key_from_seed(&seed).expect("nwc key");

        // npub.cash uses account index 0 (`m/44'/1237'/0'/0/0`); the NWC key
        // must not collide with it.
        let npub_path = DerivationPath::from_str("m/44'/1237'/0'/0/0").expect("npub path");
        let xpriv = Xpriv::new_master(Network::Bitcoin, &seed).expect("master key");
        let npub = xpriv
            .derive_priv(&SECP256K1, &npub_path)
            .expect("derive npub")
            .private_key;

        assert_ne!(nwc.to_secret_bytes(), npub.secret_bytes());
        // Never raw seed material.
        assert_ne!(&nwc.to_secret_bytes()[..], &seed[..32]);
    }

    fn sample_transaction(timestamp: u64) -> Transaction {
        Transaction {
            mint_url: MintUrl::from_str("https://mint.example.com").expect("mint url"),
            direction: TransactionDirection::Incoming,
            amount: Amount::from(10u64),
            fee: Amount::from(1u64),
            unit: CurrencyUnit::Sat,
            ys: vec![SecretKey::generate().public_key()],
            timestamp,
            memo: Some("coffee".to_string()),
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: None,
        }
    }

    #[test]
    fn transaction_maps_to_settled_nip47_object_in_msat() {
        let tx = sample_transaction(1_700_000_000);
        let mapped = transaction_to_nip47(&tx, &CurrencyUnit::Sat).expect("map tx");

        assert_eq!(mapped.transaction_type, Some(TransactionType::Incoming));
        assert_eq!(
            mapped.state,
            Some(cdk_nwc::nip47::TransactionState::Settled)
        );
        assert_eq!(mapped.amount, 10_000);
        assert_eq!(mapped.fees_paid, 1_000);
        assert_eq!(mapped.description.as_deref(), Some("coffee"));
        assert!(mapped.settled_at.is_some());
        assert_eq!(mapped.payment_hash, "");
    }

    #[tokio::test]
    async fn list_transactions_keeps_newest_first_before_pagination() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let wallet = Wallet::new(
            "https://mint.example.com",
            CurrencyUnit::Sat,
            localstore.clone(),
            [0x42; 64],
            None,
        )
        .expect("wallet");

        localstore
            .add_transaction(sample_transaction(1_700_000_000))
            .await
            .expect("add older transaction");
        localstore
            .add_transaction(sample_transaction(1_700_000_100))
            .await
            .expect("add newer transaction");

        let handler = WalletNwcHandler::new(Arc::new(wallet), None);
        let transactions = cdk_nwc::NwcRequestHandler::list_transactions(
            &handler,
            ListTransactionsRequest {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .expect("list transactions");

        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].created_at.as_secs(), 1_700_000_100);
    }

    #[tokio::test]
    async fn lookup_pending_invoice_uses_bolt11_created_at_and_quote_expiry() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let mint_url = MintUrl::from_str("https://mint.example.com").expect("mint url");
        let wallet = Wallet::new(
            "https://mint.example.com",
            CurrencyUnit::Sat,
            localstore.clone(),
            [0x42; 64],
            None,
        )
        .expect("wallet");
        let expiry = 9_999_999_999;

        localstore
            .add_mint_quote(MintQuote::new(
                "quote-id".to_string(),
                mint_url,
                PaymentMethod::Known(KnownMethod::Bolt11),
                Some(Amount::from(10u64)),
                CurrencyUnit::Sat,
                TEST_BOLT11.to_string(),
                expiry,
                None,
            ))
            .await
            .expect("add mint quote");

        let handler = WalletNwcHandler::new(Arc::new(wallet), None);
        let payment_hash = payment_hash_of(TEST_BOLT11).expect("payment hash");
        let invoice_created_at = invoice_created_at(TEST_BOLT11).expect("invoice created at");
        let transaction = cdk_nwc::NwcRequestHandler::lookup_invoice(
            &handler,
            LookupInvoiceRequest {
                payment_hash: Some(payment_hash),
                invoice: None,
            },
        )
        .await
        .expect("lookup invoice");

        assert_eq!(
            transaction.state,
            Some(cdk_nwc::nip47::TransactionState::Pending)
        );
        assert_eq!(transaction.created_at, invoice_created_at);
        assert_eq!(transaction.expires_at, Some(Timestamp::from(expiry)));
    }

    #[tokio::test]
    async fn make_invoice_rejects_description_hash() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let wallet = Wallet::new(
            "https://mint.example.com",
            CurrencyUnit::Sat,
            localstore,
            [0x24; 64],
            None,
        )
        .expect("wallet");

        let handler = WalletNwcHandler::new(Arc::new(wallet), None);
        let err = cdk_nwc::NwcRequestHandler::make_invoice(
            &handler,
            MakeInvoiceRequest {
                amount: 1_000,
                description: None,
                description_hash: Some("00".repeat(32)),
                expiry: None,
            },
        )
        .await
        .expect_err("description hash is unsupported");

        assert_eq!(err.code, ErrorCode::Other);
        assert!(err.message.contains("description_hash"));
    }

    #[tokio::test]
    async fn make_invoice_rejects_description_when_mint_does_not_support_it() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_mint_info_response(Ok(MintInfo::new().nuts(Nuts::new().nut04(
            NUT04Settings::new(
                vec![MintMethodSettings {
                    method: PaymentMethod::Known(KnownMethod::Bolt11),
                    unit: CurrencyUnit::Sat,
                    method_name: None,
                    min_amount: Some(Amount::from(1_u64)),
                    max_amount: Some(Amount::from(500_000_u64)),
                    options: Some(crate::nuts::nut04::MintMethodOptions::Bolt11 {
                        description: false,
                    }),
                }],
                false,
            ),
        ))));

        let wallet = crate::wallet::WalletBuilder::new()
            .mint_url(MintUrl::from_str("https://mint.example.com").expect("mint url"))
            .unit(CurrencyUnit::Sat)
            .localstore(localstore)
            .seed([0x42; 64])
            .shared_client(mock_client)
            .build()
            .expect("wallet");

        let handler = WalletNwcHandler::new(Arc::new(wallet), None);
        let err = cdk_nwc::NwcRequestHandler::make_invoice(
            &handler,
            MakeInvoiceRequest {
                amount: 1_000,
                description: Some("zap receipt".to_string()),
                description_hash: None,
                expiry: None,
            },
        )
        .await
        .expect_err("invoice descriptions are unsupported");

        assert_eq!(err.code, ErrorCode::Other);
        assert!(err.message.contains("invoice descriptions"));
    }

    #[tokio::test]
    async fn get_info_advertises_supported_methods() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let wallet = Wallet::new(
            "https://mint.example.com",
            CurrencyUnit::Sat,
            localstore,
            [0x42; 64],
            None,
        )
        .expect("wallet");

        let handler = WalletNwcHandler::new(Arc::new(wallet), None);
        let info = cdk_nwc::NwcRequestHandler::get_info(&handler)
            .await
            .expect("get_info");

        assert_eq!(info.alias.as_deref(), Some("CDK Cashu Wallet"));
        assert!(!info.methods.is_empty());
        assert!(info.network.as_deref() == Some("mainnet"));
    }

    #[tokio::test]
    async fn get_balance_reports_zero_for_empty_wallet() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let wallet = Wallet::new(
            "https://mint.example.com",
            CurrencyUnit::Sat,
            localstore,
            [0x42; 64],
            None,
        )
        .expect("wallet");

        let handler = WalletNwcHandler::new(Arc::new(wallet), None);
        let balance = cdk_nwc::NwcRequestHandler::get_balance(&handler)
            .await
            .expect("get_balance");

        assert_eq!(balance.balance, 0);
    }

    #[tokio::test]
    async fn pay_invoice_rejects_amountless_invoice() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let wallet = Wallet::new(
            "https://mint.example.com",
            CurrencyUnit::Sat,
            localstore,
            [0x42; 64],
            None,
        )
        .expect("wallet");

        let handler = WalletNwcHandler::new(Arc::new(wallet), None);
        let err = cdk_nwc::NwcRequestHandler::pay_invoice(
            &handler,
            PayInvoiceRequest {
                id: None,
                invoice: "lnbc1u1p4ydwah".to_string(),
                amount: None,
            },
        )
        .await
        .expect_err("invalid invoice should be rejected");

        assert!(matches!(err.code, ErrorCode::Other));
    }

    #[tokio::test]
    async fn pay_invoice_rejects_amount_mismatch() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let wallet = Wallet::new(
            "https://mint.example.com",
            CurrencyUnit::Sat,
            localstore,
            [0x42; 64],
            None,
        )
        .expect("wallet");

        let handler = WalletNwcHandler::new(Arc::new(wallet), None);
        let err = cdk_nwc::NwcRequestHandler::pay_invoice(
            &handler,
            PayInvoiceRequest {
                id: None,
                invoice: TEST_BOLT11.to_string(),
                amount: Some(999_999),
            },
        )
        .await
        .expect_err("amount mismatch should be rejected");

        assert_eq!(err.code, ErrorCode::Other);
        assert!(err.message.contains("does not match"));
    }

    #[tokio::test]
    async fn pay_invoice_rejects_over_max_payment_msat() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let wallet = Wallet::new(
            "https://mint.example.com",
            CurrencyUnit::Sat,
            localstore,
            [0x42; 64],
            None,
        )
        .expect("wallet");

        let handler = WalletNwcHandler::new(Arc::new(wallet), Some(1_000));
        let err = cdk_nwc::NwcRequestHandler::pay_invoice(
            &handler,
            PayInvoiceRequest {
                id: None,
                invoice: TEST_BOLT11.to_string(),
                amount: None,
            },
        )
        .await
        .expect_err("over-cap payment should be rejected");

        assert_eq!(err.code, ErrorCode::QuotaExceeded);
    }

    #[tokio::test]
    async fn lookup_invoice_returns_not_found_for_unknown_hash() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let wallet = Wallet::new(
            "https://mint.example.com",
            CurrencyUnit::Sat,
            localstore,
            [0x42; 64],
            None,
        )
        .expect("wallet");

        let handler = WalletNwcHandler::new(Arc::new(wallet), None);
        let err = cdk_nwc::NwcRequestHandler::lookup_invoice(
            &handler,
            LookupInvoiceRequest {
                payment_hash: Some("00".repeat(32)),
                invoice: None,
            },
        )
        .await
        .expect_err("unknown invoice should return NotFound");

        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn lookup_invoice_requires_payment_hash_or_invoice() {
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.expect("db"));
        let wallet = Wallet::new(
            "https://mint.example.com",
            CurrencyUnit::Sat,
            localstore,
            [0x42; 64],
            None,
        )
        .expect("wallet");

        let handler = WalletNwcHandler::new(Arc::new(wallet), None);
        let err = cdk_nwc::NwcRequestHandler::lookup_invoice(
            &handler,
            LookupInvoiceRequest {
                payment_hash: None,
                invoice: None,
            },
        )
        .await
        .expect_err("missing hash/invoice should error");

        assert_eq!(err.code, ErrorCode::Other);
        assert!(err.message.contains("required"));
    }
}
