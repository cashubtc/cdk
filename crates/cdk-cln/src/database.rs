//! Database module for Core Lightning (CLN) backend operations.
//!
//! This module provides database storage and retrieval functionality for CLN lightning backend,
//! including management of incoming and outgoing payments, payment status tracking, and
//! quote ID mappings for both BOLT11 and BOLT12 payments.

use cdk_common::database::mint::DynMintKVStore;
use cdk_common::util::hex;
use cdk_common::QuoteId;
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::PaymentStatus;

const PRIMARY_NAMESPACE: &str = "cdk_cln_lightning_backend";
const SECONDARY_NAMESPACE: &str = "payment_indices";
const LAST_PAY_INDEX_KEY: &str = "last_pay_index";
const OUTGOING_PAYMENTS_NAMESPACE: &str = "outgoing_payments";
const INCOMING_BOLT11_PAYMENTS_NAMESPACE: &str = "incoming_bolt11_payments";
const INCOMING_BOLT12_PAYMENTS_NAMESPACE: &str = "incoming_bolt12_payments";
const QUOTE_ID_LOOKUP_NAMESPACE: &str = "quote_id_lookup";

/// Payment identifier for incoming payments, supporting both BOLT11 and BOLT12 protocols.
///
/// This enum distinguishes between different types of incoming payment identifiers
/// to enable proper lookup and reverse lookup operations in the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IncomingPaymentIdentifier {
    /// BOLT11 payment hash (32 bytes)
    ///
    /// Used for traditional Lightning Network invoices where the payment is identified
    /// by its SHA256 hash.
    Bolt11PaymentHash([u8; 32]),
    /// BOLT12 offer ID (string)
    ///
    /// Used for BOLT12 offers where the payment is identified by a string-based offer ID.
    /// This enables more flexible payment flows with reusable payment requests.
    Bolt12OfferId(String),
}

/// Database wrapper for Core Lightning (CLN) backend operations.
///
/// Provides a high-level interface for storing and retrieving payment information,
/// quote mappings, and payment indices. Handles both incoming and outgoing payments
/// with support for BOLT11 and BOLT12 protocols.
///
/// # Example
/// ```rust,ignore
/// use cdk_cln::database::Database;
///
/// let database = Database::new(kv_store);
/// let quote_id = QuoteId::new();
/// let payment_hash = [0u8; 32];
///
/// // Store incoming BOLT11 payment
/// database.store_incoming_bolt11_payment(&payment_hash, &quote_id).await?;
///
/// // Retrieve quote ID by payment hash
/// let retrieved_quote = database.get_quote_id_by_incoming_bolt11_hash(&payment_hash).await?;
/// ```
#[derive(Clone)]
pub struct Database {
    kv_store: DynMintKVStore,
}

impl Database {
    /// Create a new Database instance.
    ///
    /// # Arguments
    /// * `kv_store` - The key-value store implementation to use for persistence
    ///
    /// # Returns
    /// A new Database instance
    pub fn new(kv_store: DynMintKVStore) -> Self {
        Self { kv_store }
    }

    /// Load the last payment index from the database.
    ///
    /// This retrieves the last processed payment index for CLN's payment monitoring.
    /// Used to track which payments have already been processed to avoid duplicates.
    ///
    /// # Returns
    /// - `Ok(Some(index))` if a last pay index exists in the database
    /// - `Ok(None)` if no last pay index has been stored yet
    /// - `Err(Error)` if there was a database error
    pub async fn load_last_pay_index(&self) -> Result<Option<u64>, Error> {
        if let Some(stored_index) = self
            .kv_store
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, LAST_PAY_INDEX_KEY)
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            if let Ok(index_str) = std::str::from_utf8(&stored_index) {
                if let Ok(index) = index_str.parse::<u64>() {
                    return Ok(Some(index));
                }
            }
        }
        Ok(None)
    }

    /// Store the last payment index in the database.
    ///
    /// This updates the last processed payment index for CLN's payment monitoring.
    /// Essential for maintaining payment processing continuity across restarts.
    ///
    /// # Arguments
    /// * `index` - The payment index to store as the last processed index
    ///
    /// # Returns
    /// - `Ok(())` if the index was successfully stored
    /// - `Err(Error)` if there was a database error
    pub async fn store_last_pay_index(&self, index: u64) -> Result<(), Error> {
        let index_str = index.to_string();
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;
        tx.kv_write(
            PRIMARY_NAMESPACE,
            SECONDARY_NAMESPACE,
            LAST_PAY_INDEX_KEY,
            index_str.as_bytes(),
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))
    }

    /// Store outgoing payment status for a quote ID.
    ///
    /// This stores the payment status information for outgoing payments, allowing
    /// tracking of payment state, fees, and completion status.
    ///
    /// # Arguments
    /// * `quote_id` - The quote ID associated with this payment
    /// * `payment_status` - The current status and details of the payment
    ///
    /// # Returns
    /// - `Ok(())` if the payment status was successfully stored
    /// - `Err(Error)` if there was a database or serialization error
    pub async fn store_outgoing_payment(
        &self,
        quote_id: &QuoteId,
        payment_status: PaymentStatus,
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
            serde_json::to_vec(&payment_status)?.as_slice(),
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))
    }

    /// Load outgoing payment status by quote ID.
    ///
    /// Retrieves the stored payment status information for a given quote ID,
    /// including payment state, fees, and completion details.
    ///
    /// # Arguments
    /// * `quote_id` - The quote ID to look up
    ///
    /// # Returns
    /// - `Ok(Some(status))` if payment status was found for the quote ID
    /// - `Ok(None)` if no payment status exists for the quote ID
    /// - `Err(Error)` if there was a database or deserialization error
    pub async fn load_outgoing_payment_status(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<PaymentStatus>, Error> {
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
            let payment_status: PaymentStatus = serde_json::from_slice(&payment_status_bytes)?;
            return Ok(Some(payment_status));
        }
        Ok(None)
    }

    /// Store BOLT12 incoming payment request mapping.
    ///
    /// Creates a bidirectional mapping between a BOLT12 offer ID and quote ID.
    /// This enables looking up quote IDs by offer ID and vice versa for BOLT12 payments.
    ///
    /// # Arguments
    /// * `local_offer_id` - The local offer ID from CLN for the BOLT12 payment
    /// * `quote_id` - The quote ID associated with this payment request
    ///
    /// # Returns
    /// - `Ok(())` if the mapping was successfully stored
    /// - `Err(Error)` if there was a database or serialization error
    pub async fn store_incoming_bolt12_payment(
        &self,
        local_offer_id: &str,
        quote_id: &QuoteId,
    ) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        // Store forward mapping: offer_id -> quote_id
        tx.kv_write(
            PRIMARY_NAMESPACE,
            INCOMING_BOLT12_PAYMENTS_NAMESPACE,
            local_offer_id,
            &quote_id.to_bytes(),
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        // Store reverse lookup: quote_id -> payment identifier
        let payment_identifier =
            IncomingPaymentIdentifier::Bolt12OfferId(local_offer_id.to_string());
        tx.kv_write(
            PRIMARY_NAMESPACE,
            QUOTE_ID_LOOKUP_NAMESPACE,
            &quote_id.to_string(),
            &serde_json::to_vec(&payment_identifier)?,
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))
    }

    /// Store BOLT11 incoming payment request mapping.
    ///
    /// Creates a bidirectional mapping between a BOLT11 payment hash and quote ID.
    /// This enables looking up quote IDs by payment hash and vice versa for BOLT11 payments.
    ///
    /// # Arguments
    /// * `payment_hash` - The 32-byte payment hash from the BOLT11 invoice
    /// * `quote_id` - The quote ID associated with this payment request
    ///
    /// # Returns
    /// - `Ok(())` if the mapping was successfully stored
    /// - `Err(Error)` if there was a database or serialization error
    pub async fn store_incoming_bolt11_payment(
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
            &serde_json::to_vec(&payment_identifier)?,
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))
    }

    /// Retrieve quote ID by local offer ID for BOLT12 incoming payments.
    ///
    /// Looks up the quote ID associated with a BOLT12 offer ID. Used to find
    /// the corresponding quote when a BOLT12 payment is received.
    ///
    /// # Arguments
    /// * `local_offer_id` - The local offer ID to look up
    ///
    /// # Returns
    /// - `Ok(Some(quote_id))` if a quote ID was found for the offer ID
    /// - `Ok(None)` if no quote ID exists for the offer ID
    /// - `Err(Error)` if there was a database error
    pub async fn get_quote_id_by_incoming_bolt12_offer(
        &self,
        local_offer_id: &str,
    ) -> Result<Option<QuoteId>, Error> {
        if let Some(quote_id_bytes) = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                INCOMING_BOLT12_PAYMENTS_NAMESPACE,
                local_offer_id,
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

    /// Retrieve quote ID by payment hash for BOLT11 incoming payments.
    ///
    /// Looks up the quote ID associated with a BOLT11 payment hash. Used to find
    /// the corresponding quote when a BOLT11 payment is received.
    ///
    /// # Arguments
    /// * `payment_hash` - The 32-byte payment hash to look up
    ///
    /// # Returns
    /// - `Ok(Some(quote_id))` if a quote ID was found for the payment hash
    /// - `Ok(None)` if no quote ID exists for the payment hash
    /// - `Err(Error)` if there was a database error
    pub async fn get_quote_id_by_incoming_bolt11_hash(
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

    /// Retrieve payment identifier by quote ID for incoming payments.
    ///
    /// Performs reverse lookup to find the payment identifier (BOLT11 hash or BOLT12 offer ID)
    /// associated with a quote ID. Used to determine the original payment type and identifier.
    ///
    /// # Arguments
    /// * `quote_id` - The quote ID to look up
    ///
    /// # Returns
    /// - `Ok(Some(identifier))` if a payment identifier was found for the quote ID
    /// - `Ok(None)` if no payment identifier exists for the quote ID
    /// - `Err(Error)` if there was a database or deserialization error
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
                serde_json::from_slice(&payment_identifier_bytes)?;
            return Ok(Some(payment_identifier));
        }
        Ok(None)
    }
}
