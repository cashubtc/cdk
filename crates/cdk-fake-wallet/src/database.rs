use cdk_common::database::mint::DynMintKVStore;
use cdk_common::nuts::MeltQuoteState;
use cdk_common::payment::{PaymentIdentifier, WaitPaymentResponse};
use cdk_common::util::hex;
use cdk_common::QuoteId;
use serde::{Deserialize, Serialize};

use crate::error::Error;

const PRIMARY_NAMESPACE: &str = "cdk_fake_wallet_lightning_backend";
const OUTGOING_PAYMENTS_NAMESPACE: &str = "outgoing_payments";
const INCOMING_BOLT11_PAYMENTS_NAMESPACE: &str = "incoming_bolt11_payments";
const INCOMING_BOLT12_PAYMENTS_NAMESPACE: &str = "incoming_bolt12_payments";
const QUOTE_ID_LOOKUP_NAMESPACE: &str = "quote_id_lookup";
const PAYMENT_STATES_NAMESPACE: &str = "payment_states";
const INCOMING_PAYMENT_RESPONSES_NAMESPACE: &str = "incoming_payment_responses";
const PAID_INVOICES_NAMESPACE: &str = "paid_invoices";

/// Enum representing the payment identifier for an incoming payment
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IncomingPaymentIdentifier {
    /// BOLT11 payment hash (32 bytes)
    Bolt11PaymentHash([u8; 32]),
    /// BOLT12 offer ID (string)
    Bolt12OfferId(String),
}

/// Outgoing payment status for fake wallet
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutgoingPaymentStatus {
    /// Payment identifier
    pub payment_identifier: PaymentIdentifier,
    /// Payment status
    pub status: cdk_common::nuts::MeltQuoteState,
    /// Total amount spent
    pub total_spent: cdk_common::Amount,
}

#[derive(Clone)]
pub struct Database {
    kv_store: DynMintKVStore,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("kv_store", &"<DynMintKVStore>")
            .finish()
    }
}

impl Database {
    pub fn new(kv_store: DynMintKVStore) -> Self {
        Self { kv_store }
    }

    /// Helper function to execute a transaction with a write operation
    async fn execute_transaction_write(
        &self,
        namespace: &str,
        sub_namespace: &str,
        key: &str,
        data: &[u8],
    ) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        tx.kv_write(namespace, sub_namespace, key, data)
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))
    }

    /// Store outgoing payment status by quote ID
    pub async fn store_quote_payment(
        &self,
        quote_id: &QuoteId,
        payment_status: OutgoingPaymentStatus,
    ) -> Result<(), Error> {
        self.execute_transaction_write(
            PRIMARY_NAMESPACE,
            OUTGOING_PAYMENTS_NAMESPACE,
            quote_id.to_string().as_str(),
            serde_json::to_vec(&payment_status)?.as_slice(),
        )
        .await
    }

    /// Load outgoing payment status by quote ID
    pub async fn load_payment_status_by_quote_id(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<OutgoingPaymentStatus>, Error> {
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
            let payment_status: OutgoingPaymentStatus =
                serde_json::from_slice(&payment_status_bytes)?;
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
        // Store forward mapping: offer_id -> quote_id
        self.execute_transaction_write(
            PRIMARY_NAMESPACE,
            INCOMING_BOLT12_PAYMENTS_NAMESPACE,
            local_offer_id,
            &quote_id.to_bytes(),
        )
        .await?;

        // Store reverse lookup: quote_id -> payment identifier
        let payment_identifier =
            IncomingPaymentIdentifier::Bolt12OfferId(local_offer_id.to_string());
        self.execute_transaction_write(
            PRIMARY_NAMESPACE,
            QUOTE_ID_LOOKUP_NAMESPACE,
            &quote_id.to_string(),
            &serde_json::to_vec(&payment_identifier)?,
        )
        .await
    }

    /// Store BOLT11 incoming payment request mapping: payment_hash -> quote_id
    /// Also stores reverse lookup: quote_id -> payment_hash
    pub async fn store_bolt11_request(
        &self,
        payment_hash: &[u8; 32],
        quote_id: &QuoteId,
    ) -> Result<(), Error> {
        // Store forward mapping: payment_hash -> quote_id
        self.execute_transaction_write(
            PRIMARY_NAMESPACE,
            INCOMING_BOLT11_PAYMENTS_NAMESPACE,
            &hex::encode(payment_hash),
            &quote_id.to_bytes(),
        )
        .await?;

        // Store reverse lookup: quote_id -> payment identifier
        let payment_identifier = IncomingPaymentIdentifier::Bolt11PaymentHash(*payment_hash);
        self.execute_transaction_write(
            PRIMARY_NAMESPACE,
            QUOTE_ID_LOOKUP_NAMESPACE,
            &quote_id.to_string(),
            &serde_json::to_vec(&payment_identifier)?,
        )
        .await
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

    /// Store payment state by payment hash/identifier
    pub async fn store_payment_state(
        &self,
        payment_hash: &str,
        state: MeltQuoteState,
    ) -> Result<(), Error> {
        self.execute_transaction_write(
            PRIMARY_NAMESPACE,
            PAYMENT_STATES_NAMESPACE,
            payment_hash,
            &serde_json::to_vec(&state)?,
        )
        .await
    }

    /// Get payment state by payment hash/identifier
    pub async fn get_payment_state(
        &self,
        payment_hash: &str,
    ) -> Result<Option<MeltQuoteState>, Error> {
        if let Some(state_bytes) = self
            .kv_store
            .kv_read(PRIMARY_NAMESPACE, PAYMENT_STATES_NAMESPACE, payment_hash)
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            let state: MeltQuoteState = serde_json::from_slice(&state_bytes)?;
            return Ok(Some(state));
        }
        Ok(None)
    }

    /// Store incoming payment responses for a payment identifier
    pub async fn store_incoming_payment_responses(
        &self,
        payment_identifier: &PaymentIdentifier,
        responses: &[WaitPaymentResponse],
    ) -> Result<(), Error> {
        self.execute_transaction_write(
            PRIMARY_NAMESPACE,
            INCOMING_PAYMENT_RESPONSES_NAMESPACE,
            &payment_identifier.to_string(),
            &serde_json::to_vec(responses)?,
        )
        .await
    }

    /// Get incoming payment responses for a payment identifier
    pub async fn get_incoming_payment_responses(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Error> {
        if let Some(responses_bytes) = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                INCOMING_PAYMENT_RESPONSES_NAMESPACE,
                &payment_identifier.to_string(),
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            let responses: Vec<WaitPaymentResponse> = serde_json::from_slice(&responses_bytes)?;
            return Ok(responses);
        }
        Ok(vec![])
    }

    /// Add an incoming payment response to existing responses
    pub async fn add_incoming_payment_response(
        &self,
        payment_identifier: &PaymentIdentifier,
        response: WaitPaymentResponse,
    ) -> Result<(), Error> {
        let mut responses = self
            .get_incoming_payment_responses(payment_identifier)
            .await?;
        responses.push(response);
        self.store_incoming_payment_responses(payment_identifier, &responses)
            .await
    }

    /// Mark an invoice as paid by its payment hash
    pub async fn mark_invoice_as_paid(&self, payment_hash: &[u8; 32]) -> Result<(), Error> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| Error::Database(e.to_string()))?
            .as_secs();

        self.execute_transaction_write(
            PRIMARY_NAMESPACE,
            PAID_INVOICES_NAMESPACE,
            &hex::encode(payment_hash),
            &serde_json::to_vec(&timestamp)?,
        )
        .await
    }

    /// Check if an invoice has already been paid by its payment hash
    pub async fn is_invoice_paid(&self, payment_hash: &[u8; 32]) -> Result<bool, Error> {
        let result = self
            .kv_store
            .kv_read(
                PRIMARY_NAMESPACE,
                PAID_INVOICES_NAMESPACE,
                &hex::encode(payment_hash),
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        Ok(result.is_some())
    }
}
