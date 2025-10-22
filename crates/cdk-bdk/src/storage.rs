//! BDK storage operations using KV store

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use cdk_common::bitcoin::hashes::Hash;
use cdk_common::bitcoin::{OutPoint, Txid};
use cdk_common::database::MintKVStore;
use cdk_common::payment::{MakePaymentResponse, WaitPaymentResponse};
use cdk_common::QuoteId;

use crate::error::Error;

/// Primary namespace for BDK KV store operations
pub const BDK_NAMESPACE: &str = "bdk";

/// Secondary namespace for pending incoming transactions
pub const PENDING_INCOMING_NAMESPACE: &str = "pending_incoming";

/// Secondary namespace for pending outgoing transactions
pub const PENDING_OUTGOING_NAMESPACE: &str = "pending_outgoing";

/// Utility functions for OutPoint serialization to database-safe strings

/// Encode an OutPoint to a database-safe hex string
/// Encodes the OutPoint's txid and vout as hex without colons
fn encode_outpoint_for_db(outpoint: &OutPoint) -> String {
    let mut bytes = Vec::with_capacity(36); // 32 bytes txid + 4 bytes vout
    bytes.extend_from_slice(&outpoint.txid.to_raw_hash().to_byte_array());
    bytes.extend_from_slice(&outpoint.vout.to_le_bytes());
    cdk_common::util::hex::encode(bytes)
}

/// Decode an OutPoint from a database-safe hex string
fn decode_outpoint_from_db(s: &str) -> Result<OutPoint, Error> {
    let bytes = cdk_common::util::hex::decode(s).map_err(|e| {
        Error::KvStore(cdk_common::database::Error::Internal(format!(
            "Hex decode error: {}",
            e
        )))
    })?;
    if bytes.len() != 36 {
        return Err(Error::KvStore(cdk_common::database::Error::Internal(
            "Invalid outpoint hex length".to_string(),
        )));
    }

    let mut txid_bytes = [0u8; 32];
    txid_bytes.copy_from_slice(&bytes[0..32]);
    let hash = Hash::from_byte_array(txid_bytes);
    let txid = Txid::from_raw_hash(hash);

    let mut vout_bytes = [0u8; 4];
    vout_bytes.copy_from_slice(&bytes[32..36]);
    let vout = u32::from_le_bytes(vout_bytes);

    Ok(OutPoint { txid, vout })
}

/// BDK KV store operations
#[derive(Clone)]
pub struct BdkStorage {
    kv_store: Arc<dyn MintKVStore<Err = cdk_common::database::Error> + Send + Sync>,
}

impl BdkStorage {
    /// Create a new BdkStorage instance
    pub fn new(
        kv_store: Arc<dyn MintKVStore<Err = cdk_common::database::Error> + Send + Sync>,
    ) -> Self {
        Self { kv_store }
    }

    /// Store a pending incoming transaction
    pub async fn store_pending_incoming_tx(
        &self,
        outpoint: OutPoint,
        response: WaitPaymentResponse,
    ) -> Result<(), Error> {
        let serialized = serde_json::to_vec(&response).map_err(Error::from)?;
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            PENDING_INCOMING_NAMESPACE,
            &encode_outpoint_for_db(&outpoint),
            &serialized,
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Store a pending outgoing transaction
    pub async fn store_pending_outgoing_tx(
        &self,
        quote_id: QuoteId,
        response: MakePaymentResponse,
    ) -> Result<(), Error> {
        let serialized = serde_json::to_vec(&response).map_err(Error::from)?;
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            PENDING_OUTGOING_NAMESPACE,
            &quote_id.to_string(),
            &serialized,
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Get all pending incoming transactions
    pub async fn get_pending_incoming_txs(
        &self,
    ) -> Result<HashMap<OutPoint, WaitPaymentResponse>, Error> {
        let keys = self
            .kv_store
            .kv_list(BDK_NAMESPACE, PENDING_INCOMING_NAMESPACE)
            .await
            .map_err(Error::from)?;

        let mut pending_txs = HashMap::new();

        for key in keys {
            if let Some(data) = self
                .kv_store
                .kv_read(BDK_NAMESPACE, PENDING_INCOMING_NAMESPACE, &key)
                .await
                .map_err(Error::from)?
            {
                if let (Ok(outpoint), Ok(response)) = (
                    decode_outpoint_from_db(&key),
                    serde_json::from_slice::<WaitPaymentResponse>(&data),
                ) {
                    pending_txs.insert(outpoint, response);
                }
            }
        }

        Ok(pending_txs)
    }

    /// Get all pending outgoing transactions
    pub async fn get_pending_outgoing_txs(
        &self,
    ) -> Result<HashMap<QuoteId, MakePaymentResponse>, Error> {
        let keys = self
            .kv_store
            .kv_list(BDK_NAMESPACE, PENDING_OUTGOING_NAMESPACE)
            .await
            .map_err(Error::from)?;

        let mut pending_txs = HashMap::new();

        for key in keys {
            if let Some(data) = self
                .kv_store
                .kv_read(BDK_NAMESPACE, PENDING_OUTGOING_NAMESPACE, &key)
                .await
                .map_err(Error::from)?
            {
                if let (Ok(outpoint), Ok(response)) = (
                    QuoteId::from_str(&key),
                    serde_json::from_slice::<MakePaymentResponse>(&data),
                ) {
                    pending_txs.insert(outpoint, response);
                }
            }
        }

        Ok(pending_txs)
    }

    /// Remove a pending incoming transaction
    pub async fn remove_pending_incoming_tx(&self, outpoint: &OutPoint) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        tx.kv_remove(
            BDK_NAMESPACE,
            PENDING_INCOMING_NAMESPACE,
            &encode_outpoint_for_db(outpoint),
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Remove a pending outgoing transaction
    pub async fn remove_pending_outgoing_tx(&self, quote_id: &QuoteId) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        tx.kv_remove(
            BDK_NAMESPACE,
            PENDING_OUTGOING_NAMESPACE,
            &quote_id.to_string(),
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_encode_decode_outpoint() {
        // Create a test OutPoint
        let txid_bytes = [0u8; 32];
        let txid = Txid::from_raw_hash(cdk_common::bitcoin::hashes::Hash::from_byte_array(
            txid_bytes,
        ));
        let vout = 1u32;
        let original_outpoint = cdk_common::bitcoin::OutPoint::new(txid, vout);

        // Encode it
        let encoded = encode_outpoint_for_db(&original_outpoint);

        // Decode it back
        let decoded_outpoint =
            decode_outpoint_from_db(&encoded).expect("Should decode successfully");

        // Check they're equal
        assert_eq!(original_outpoint.txid, decoded_outpoint.txid);
        assert_eq!(original_outpoint.vout, decoded_outpoint.vout);
        assert_eq!(original_outpoint, decoded_outpoint);

        // Check that the encoded string doesn't contain a colon
        assert!(
            !encoded.contains(':'),
            "OutPoint encoding should not contain colons for database safety"
        );

        // Check that the length is correct (32 bytes txid + 4 bytes vout = 64 hex chars)
        assert_eq!(
            encoded.len(),
            72,
            "Encoded OutPoint should be 72 hex characters (36 bytes = 72 hex chars)"
        );
    }

    #[test]
    fn test_encode_decode_with_real_txid() {
        // Use a real-looking txid (from mainnet block 1 coinbase)
        let txid_str = "0e3e2357e806b6cdb1f70b54c3a3a17b6714ee1f0e68bebb44a74b1efd512098";
        let txid = Txid::from_str(txid_str).expect("Should parse txid");
        let vout = 42u32;
        let original_outpoint = cdk_common::bitcoin::OutPoint::new(txid, vout);

        // Encode it
        let encoded = encode_outpoint_for_db(&original_outpoint);
        println!("Original outpoint: {}", original_outpoint.to_string());
        println!("Encoded outpoint: {}", encoded);

        // Decode it back
        let decoded = decode_outpoint_from_db(&encoded).expect("Should decode successfully");

        assert_eq!(original_outpoint, decoded);
        assert!(!encoded.contains(':'));
    }

    #[test]
    fn test_invalid_hex_decoding() {
        // Test with invalid hex
        let result = decode_outpoint_from_db("invalid_hex");
        assert!(result.is_err(), "Should fail with invalid hex");

        // Test with wrong length
        let result = decode_outpoint_from_db("00".repeat(35).as_str()); // 70 characters = 35 bytes, not 36
        assert!(result.is_err(), "Should fail with wrong length");
    }
}
