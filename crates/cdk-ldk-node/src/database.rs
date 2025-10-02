use cdk_common::database::mint::DynMintKVStore;
use cdk_common::util::hex;
use cdk_common::QuoteId;
use ldk_node::lightning::offers::offer::OfferId;
use ldk_node::lightning_types::payment::PaymentHash;
use serde::{Deserialize, Serialize};

use crate::error::Error;

const PRIMARY_NAMESPACE: &str = "cdk_ldk_lightning_backend";
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

/// Payment status response for LDK
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentStatus {
    /// Payment status
    pub status: cdk_common::nuts::MeltQuoteState,
    /// Payment hash (32 bytes)
    pub payment_hash: [u8; 32],
    /// Payment ID from LDK (32 bytes)
    pub payment_id: [u8; 32],
    /// Payment proof (preimage as hex string)
    pub payment_proof: Option<String>,
    /// Total amount spent
    pub total_spent: cdk_common::Amount,
}

impl PaymentStatus {
    pub fn unpaid(&self) -> Result<(), cdk_common::payment::Error> {
        use cdk_common::nuts::MeltQuoteState;
        match self.status {
            MeltQuoteState::Unpaid | MeltQuoteState::Unknown | MeltQuoteState::Failed => Ok(()),
            MeltQuoteState::Paid => {
                tracing::debug!("Melt attempted on invoice already paid");
                Err(cdk_common::payment::Error::InvoiceAlreadyPaid)
            }
            MeltQuoteState::Pending => {
                tracing::debug!("Melt attempted on invoice already pending");
                Err(cdk_common::payment::Error::InvoicePaymentPending)
            }
        }
    }
}

#[derive(Clone)]
pub struct Database {
    kv_store: DynMintKVStore,
}

impl Database {
    pub fn new(kv_store: DynMintKVStore) -> Self {
        Self { kv_store }
    }

    /// Store outgoing payment status for a quote
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

    /// Load outgoing payment status by quote ID
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

    /// Store BOLT11 incoming payment request mapping: payment_hash -> quote_id
    /// Also stores reverse lookup: quote_id -> payment_hash
    pub async fn store_incoming_bolt11_payment(
        &self,
        payment_hash: &PaymentHash,
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
            &hex::encode(payment_hash.0),
            &quote_id.to_bytes(),
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        // Store reverse lookup: quote_id -> payment identifier
        let payment_identifier = IncomingPaymentIdentifier::Bolt11PaymentHash(payment_hash.0);
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

    /// Store BOLT12 incoming payment request mapping: offer_id -> quote_id
    /// Also stores reverse lookup: quote_id -> offer_id
    pub async fn store_incoming_bolt12_payment(
        &self,
        offer_id: &OfferId,
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
            hex::encode(offer_id.0).as_str(),
            &quote_id.to_bytes(),
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        // Store reverse lookup: quote_id -> payment identifier
        let payment_identifier = IncomingPaymentIdentifier::Bolt12OfferId(offer_id.to_string());
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

    /// Get quote ID by payment hash for BOLT11 incoming payments
    pub async fn get_quote_id_by_incoming_bolt11_hash(
        &self,
        payment_hash: &PaymentHash,
    ) -> Result<Option<QuoteId>, Error> {
        if let Some(quote_id_bytes) = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                INCOMING_BOLT11_PAYMENTS_NAMESPACE,
                &hex::encode(payment_hash.0),
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

    /// Get quote ID by offer ID for BOLT12 incoming payments
    pub async fn get_quote_id_by_incoming_bolt12_offer(
        &self,
        offer_id: &OfferId,
    ) -> Result<Option<QuoteId>, Error> {
        if let Some(quote_id_bytes) = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                INCOMING_BOLT12_PAYMENTS_NAMESPACE,
                hex::encode(offer_id.0).as_str(),
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
