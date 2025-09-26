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

/// Enum representing the payment identifier for an incoming payment
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IncomingPaymentIdentifier {
    /// BOLT11 payment hash (32 bytes)
    Bolt11PaymentHash([u8; 32]),
    /// BOLT12 offer ID (string)
    Bolt12OfferId(String),
}

#[derive(Clone)]
pub struct Database {
    kv_store: DynMintKVStore,
}

impl Database {
    pub fn new(kv_store: DynMintKVStore) -> Self {
        Self { kv_store }
    }

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

    pub async fn store_quote_payment(
        &self,
        quote_id: &QuoteId,
        payment_status: PaymentStatus,
    ) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        // Store forward mapping: quote_id -> payment_hash
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

    pub async fn load_payment_status_by_quote_id(
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

    /// Store BOLT12 incoming payment request mapping: local_offer_id -> quote_id
    /// Also stores reverse lookup: quote_id -> offer_id
    pub async fn store_bolt12_request(
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
            &local_offer_id,
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
            &serde_json::to_vec(&payment_identifier)?,
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))
    }

    /// Get quote ID by local offer ID for BOLT12 incoming payments
    pub async fn get_quote_id_by_local_offer_id(
        &self,
        local_offer_id: &str,
    ) -> Result<Option<QuoteId>, Error> {
        if let Some(quote_id_bytes) = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                INCOMING_BOLT12_PAYMENTS_NAMESPACE,
                &local_offer_id,
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
                serde_json::from_slice(&payment_identifier_bytes)?;
            return Ok(Some(payment_identifier));
        }
        Ok(None)
    }
}
