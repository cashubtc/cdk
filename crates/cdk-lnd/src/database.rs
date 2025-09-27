use cdk_common::database::mint::DynMintKVStore;
use cdk_common::util::hex;
use cdk_common::QuoteId;

use crate::error::Error;
use crate::LndPaymentStatus;

const PRIMARY_NAMESPACE: &str = "cdk_lnd_lightning_backend";
const SECONDARY_NAMESPACE: &str = "payment_indices";
const OUTGOING_PAYMENTS_NAMESPACE: &str = "outgoing_payments";
const INCOMING_BOLT11_PAYMENTS_NAMESPACE: &str = "incoming_bolt11_payments";
const QUOTE_ID_LOOKUP_NAMESPACE: &str = "quote_id_lookup";

// Index storage keys
const LAST_ADD_INDEX_KV_KEY: &str = "last_add_index";
const LAST_SETTLE_INDEX_KV_KEY: &str = "last_settle_index";

/// Struct representing LND invoice subscription indices
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InvoiceIndices {
    /// Last add index for new invoices
    pub add_index: Option<u64>,
    /// Last settle index for settled invoices
    pub settle_index: Option<u64>,
}

impl InvoiceIndices {
    /// Create new InvoiceIndices with specified values
    pub fn new(add_index: Option<u64>, settle_index: Option<u64>) -> Self {
        Self {
            add_index,
            settle_index,
        }
    }

    /// Get the add index, defaulting to 0 if None
    pub fn add_index_or_default(&self) -> u64 {
        self.add_index.unwrap_or(0)
    }

    /// Get the settle index, defaulting to 0 if None
    pub fn settle_index_or_default(&self) -> u64 {
        self.settle_index.unwrap_or(0)
    }
}

/// Enum representing the payment identifier for an incoming payment
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IncomingPaymentIdentifier {
    /// BOLT11 payment hash (32 bytes)
    Bolt11PaymentHash([u8; 32]),
}

/// Database wrapper for LND backend operations
#[derive(Clone)]
pub struct Database {
    kv_store: DynMintKVStore,
}

impl Database {
    /// Create a new Database instance
    pub fn new(kv_store: DynMintKVStore) -> Self {
        Self { kv_store }
    }

    /// Store payment status for a quote ID
    pub async fn store_quote_payment(
        &self,
        quote_id: &QuoteId,
        payment_status: LndPaymentStatus,
    ) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        // Store forward mapping: quote_id -> payment_status
        tx.kv_write(
            PRIMARY_NAMESPACE,
            OUTGOING_PAYMENTS_NAMESPACE,
            quote_id.to_string().as_str(),
            serde_json::to_vec(&payment_status)
                .map_err(|e| Error::Database(e.to_string()))?
                .as_slice(),
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))
    }

    /// Load payment status by quote ID
    pub async fn load_payment_status_by_quote_id(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<LndPaymentStatus>, Error> {
        if let Some(payment_status_bytes) = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                OUTGOING_PAYMENTS_NAMESPACE,
                quote_id.to_string().as_str(),
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            let payment_status: LndPaymentStatus = serde_json::from_slice(&payment_status_bytes)
                .map_err(|e| Error::Database(e.to_string()))?;
            return Ok(Some(payment_status));
        }
        Ok(None)
    }

    /// Store BOLT11 incoming payment request mapping: payment_hash -> quote_id
    /// Also stores reverse lookup: quote_id -> payment_hash
    pub async fn store_bolt11_request(
        &self,
        payment_hash: &[u8; 32],
        quote_id: &QuoteId,
    ) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        // Store forward mapping: payment_hash -> quote_id
        tx.kv_write(
            PRIMARY_NAMESPACE,
            INCOMING_BOLT11_PAYMENTS_NAMESPACE,
            &hex::encode(payment_hash),
            &quote_id.to_bytes(),
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        // Store reverse lookup: quote_id -> payment identifier
        let payment_identifier = IncomingPaymentIdentifier::Bolt11PaymentHash(*payment_hash);
        tx.kv_write(
            PRIMARY_NAMESPACE,
            QUOTE_ID_LOOKUP_NAMESPACE,
            &quote_id.to_string(),
            &serde_json::to_vec(&payment_identifier).map_err(|e| Error::Database(e.to_string()))?,
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))
    }

    /// Get quote ID by payment hash for BOLT11 incoming payments
    pub async fn get_quote_id_by_payment_hash(
        &self,
        payment_hash: &[u8; 32],
    ) -> Result<Option<QuoteId>, Error> {
        if let Some(quote_id_bytes) = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                INCOMING_BOLT11_PAYMENTS_NAMESPACE,
                &hex::encode(payment_hash),
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            let quote_id =
                QuoteId::from_bytes(&quote_id_bytes).map_err(|e| Error::Database(e.to_string()))?;
            return Ok(Some(quote_id));
        }
        Ok(None)
    }

    /// Get payment identifier (BOLT11 hash or BOLT12 offer ID) by quote ID for incoming payments
    pub async fn get_incoming_payment_identifier_by_quote_id(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<IncomingPaymentIdentifier>, Error> {
        if let Some(payment_identifier_bytes) = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                QUOTE_ID_LOOKUP_NAMESPACE,
                &quote_id.to_string(),
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            let payment_identifier: IncomingPaymentIdentifier =
                serde_json::from_slice(&payment_identifier_bytes)
                    .map_err(|e| Error::Database(e.to_string()))?;
            return Ok(Some(payment_identifier));
        }
        Ok(None)
    }

    /// Get last add and settle indices from KV store
    pub async fn get_last_indices(&self) -> Result<InvoiceIndices, Error> {
        let add_index = if let Some(stored_index) = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                LAST_ADD_INDEX_KV_KEY,
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            if let Ok(index_str) = std::str::from_utf8(stored_index.as_slice()) {
                index_str.parse::<u64>().ok()
            } else {
                None
            }
        } else {
            None
        };

        let settle_index = if let Some(stored_index) = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                LAST_SETTLE_INDEX_KV_KEY,
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            if let Ok(index_str) = std::str::from_utf8(stored_index.as_slice()) {
                index_str.parse::<u64>().ok()
            } else {
                None
            }
        } else {
            None
        };

        let indices = InvoiceIndices::new(add_index, settle_index);
        tracing::debug!(
            "LND: Retrieved last indices from KV store - add_index: {:?}, settle_index: {:?}",
            indices.add_index,
            indices.settle_index
        );
        Ok(indices)
    }

    /// Store add and settle indices to KV store
    pub async fn store_indices(&self, add_index: u64, settle_index: u64) -> Result<(), Error> {
        let indices = InvoiceIndices::new(Some(add_index), Some(settle_index));
        self.store_invoice_indices(&indices).await
    }

    /// Store invoice indices struct to KV store
    pub async fn store_invoice_indices(&self, indices: &InvoiceIndices) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        // Only store indices that are Some
        if let Some(add_index) = indices.add_index {
            let add_index_str = add_index.to_string();
            tx.kv_write(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                LAST_ADD_INDEX_KV_KEY,
                add_index_str.as_bytes(),
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?;
        }

        if let Some(settle_index) = indices.settle_index {
            let settle_index_str = settle_index.to_string();
            tx.kv_write(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                LAST_SETTLE_INDEX_KV_KEY,
                settle_index_str.as_bytes(),
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?;
        }

        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        tracing::debug!(
            "LND: Stored updated indices - add_index: {:?}, settle_index: {:?}",
            indices.add_index,
            indices.settle_index
        );
        Ok(())
    }
}
