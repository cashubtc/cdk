use cdk_common::database::mint::DynMintKVStore;
use cdk_common::QuoteId;

use crate::error::Error;
use crate::PaymentStatus;

const PRIMARY_NAMESPACE: &str = "cdk_cln_lightning_backend";
const SECONDARY_NAMESPACE: &str = "payment_indices";
const LAST_PAY_INDEX_KEY: &str = "last_pay_index";
const OUTGOING_PAYMENTS_NAMESPACE: &str = "outgoing_payments";

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
}
