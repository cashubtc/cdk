//! Wallet-side verification of a mint's transparency log (NUT-XX).
//!
//! Implements the client behavior the NUT draft asks of wallets:
//!
//! > Wallets SHOULD remember the largest verified, witness-cosigned
//! > checkpoint per mint and require new checkpoints to be append-only
//! > consistent with it.
//!
//! Trust model: trust-on-first-use for the mint's log-signing key. On
//! first contact the wallet pins `(origin, pubkey, tree_size, root_hash)`
//! in its local store; on every later check it requires the same key, the
//! same origin, and an RFC 6962 consistency proof from the pinned tree to
//! the presented one. A mint that rewrites or rolls back its history can
//! no longer produce such a proof, and the wallet reports it instead of
//! silently accepting the new view.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use cdk_tlog::checkpoint::{verify_checkpoint_signature, SignedCheckpoint};
use cdk_tlog::merkle::{verify_consistency, Hash};
use ed25519_dalek::VerifyingKey;

use super::Wallet;
use crate::Error;

const KV_PRIMARY: &str = "cdk_transparency";
const KV_PUBKEY: &str = "pubkey";
const KV_ORIGIN: &str = "origin";
const KV_SIZE: &str = "size";
const KV_ROOT: &str = "root";

/// Outcome of one transparency log verification pass against a mint.
///
/// Only transport/parse failures surface as [`Error`]; every *verification*
/// outcome — including the hostile ones — is a variant here, so wallet UIs
/// can distinguish "mint is fine", "mint doesn't support this", and "mint
/// presented a history inconsistent with what it previously committed to".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransparencyStatus {
    /// The checkpoint verified. On first contact `previous_size` is
    /// `None` (nothing was pinned yet — trust-on-first-use); afterwards it
    /// carries the pinned size the consistency proof was checked against.
    Verified {
        /// The verified checkpoint's tree size, now pinned.
        size: u64,
        /// The verified checkpoint's root hash, now pinned.
        root: Hash,
        /// The previously pinned size, if this wasn't first contact.
        previous_size: Option<u64>,
    },
    /// The mint presented a checkpoint *smaller* than one it previously
    /// signed — history has been rolled back.
    RollbackDetected {
        /// The size this wallet has pinned.
        pinned_size: u64,
        /// The smaller size the mint just presented.
        presented_size: u64,
    },
    /// Same tree size as pinned, different root — a rewrite at constant
    /// size (equivocation).
    RootMismatch {
        /// The tree size at which the two roots disagree.
        size: u64,
    },
    /// The consistency proof from the pinned tree to the presented one
    /// did not verify — the new tree is not an append-only extension of
    /// what the mint previously committed to.
    InconsistentHistory {
        /// The pinned size the proof was requested from.
        pinned_size: u64,
        /// The presented size the proof was requested to.
        presented_size: u64,
    },
    /// The mint's log-signing key or origin changed since it was pinned.
    /// NUT-XX: "Wallets and auditors SHOULD treat unexplained log key
    /// changes as suspicious."
    IdentityChanged,
}

impl Wallet {
    /// Fetches the mint's latest transparency checkpoint, verifies it, and
    /// updates this wallet's pinned view of the mint's log (see the module
    /// docs for the trust model).
    ///
    /// Returns [`Error`] only for transport or format problems (endpoint
    /// unreachable, malformed note). All verification verdicts, including
    /// detected tampering, come back as a [`TransparencyStatus`].
    pub async fn verify_transparency_log(&self) -> Result<TransparencyStatus, Error> {
        let pubkey_response = self.client.get_audit_pubkey().await?;
        let checkpoint_response = self.client.get_audit_checkpoint().await?;

        let presented_key = decode_pubkey(&pubkey_response.pubkey)?;
        let origin = pubkey_response.origin;

        // Enforce the pinned identity, or pin it on first contact.
        let secondary = self.transparency_kv_secondary();
        let pinned_key = self.localstore.kv_read(KV_PRIMARY, &secondary, KV_PUBKEY).await?;
        let pinned_origin = self.localstore.kv_read(KV_PRIMARY, &secondary, KV_ORIGIN).await?;
        if let (Some(key), Some(pinned_origin)) = (&pinned_key, &pinned_origin) {
            if key.as_slice() != presented_key.as_bytes()
                || pinned_origin.as_slice() != origin.as_bytes()
            {
                return Ok(TransparencyStatus::IdentityChanged);
            }
        }
        // Otherwise: first contact, nothing pinned yet.

        let signed = SignedCheckpoint::parse(&checkpoint_response.checkpoint)
            .map_err(|e| Error::Custom(format!("malformed checkpoint note: {e}")))?;
        if signed.checkpoint.origin != origin {
            return Ok(TransparencyStatus::IdentityChanged);
        }

        // The mint's own signature must verify against the (pinned or
        // just-presented) log key.
        let mut signature_ok = false;
        for line in &signed.signatures {
            if verify_checkpoint_signature(&signed.checkpoint, &origin, &presented_key, line)
                .is_ok()
            {
                signature_ok = true;
                break;
            }
        }
        if !signature_ok {
            return Err(Error::Custom(
                "checkpoint carries no valid signature from the mint's log key".to_string(),
            ));
        }

        let presented_size = signed.checkpoint.size;
        let presented_root = signed.checkpoint.root_hash;

        let pinned = self.read_pinned_checkpoint(&secondary).await?;
        let previous_size = match pinned {
            None => None,
            Some((pinned_size, pinned_root)) => {
                if presented_size < pinned_size {
                    return Ok(TransparencyStatus::RollbackDetected {
                        pinned_size,
                        presented_size,
                    });
                }
                if presented_size == pinned_size {
                    if presented_root != pinned_root {
                        return Ok(TransparencyStatus::RootMismatch { size: pinned_size });
                    }
                    // Nothing new; identical checkpoint re-verified.
                    return Ok(TransparencyStatus::Verified {
                        size: presented_size,
                        root: presented_root,
                        previous_size: Some(pinned_size),
                    });
                }

                // Grew: require an append-only consistency proof from the
                // pinned tree to the presented one.
                let proof_response = self
                    .client
                    .get_audit_consistency_proof(pinned_size, presented_size)
                    .await?;
                let proof = decode_proof(&proof_response.proof)?;
                let consistent = verify_consistency(
                    pinned_size,
                    pinned_root,
                    presented_size,
                    presented_root,
                    &proof,
                )
                .unwrap_or(false);
                if !consistent {
                    return Ok(TransparencyStatus::InconsistentHistory {
                        pinned_size,
                        presented_size,
                    });
                }
                Some(pinned_size)
            }
        };

        // Everything checked out: advance (or create) the pin.
        self.write_pin(
            &secondary,
            &origin,
            &presented_key,
            presented_size,
            &presented_root,
        )
        .await?;

        Ok(TransparencyStatus::Verified {
            size: presented_size,
            root: presented_root,
            previous_size,
        })
    }

    /// The pinned `(size, root)` for this wallet's mint, if any — the
    /// largest checkpoint this wallet has verified so far.
    pub async fn pinned_transparency_checkpoint(&self) -> Result<Option<(u64, Hash)>, Error> {
        let secondary = self.transparency_kv_secondary();
        self.read_pinned_checkpoint(&secondary).await
    }

    /// Per-mint KV secondary namespace: mint URLs contain characters the
    /// KV alphabet forbids, so key by their hash.
    fn transparency_kv_secondary(&self) -> String {
        use bitcoin::hashes::{sha256, Hash as BitcoinHash};
        hex::encode(sha256::Hash::hash(self.mint_url.to_string().as_bytes()).to_byte_array())
    }

    async fn read_pinned_checkpoint(&self, secondary: &str) -> Result<Option<(u64, Hash)>, Error> {
        let size = self.localstore.kv_read(KV_PRIMARY, secondary, KV_SIZE).await?;
        let root = self.localstore.kv_read(KV_PRIMARY, secondary, KV_ROOT).await?;
        Ok(match (size, root) {
            (Some(size), Some(root)) if size.len() == 8 && root.len() == 32 => {
                let size = u64::from_be_bytes(size.try_into().expect("checked len"));
                let root: Hash = root.try_into().expect("checked len");
                Some((size, root))
            }
            _ => None,
        })
    }

    async fn write_pin(
        &self,
        secondary: &str,
        origin: &str,
        key: &VerifyingKey,
        size: u64,
        root: &Hash,
    ) -> Result<(), Error> {
        self.localstore
            .kv_write(KV_PRIMARY, secondary, KV_ORIGIN, origin.as_bytes())
            .await?;
        self.localstore
            .kv_write(KV_PRIMARY, secondary, KV_PUBKEY, key.as_bytes())
            .await?;
        self.localstore
            .kv_write(KV_PRIMARY, secondary, KV_SIZE, &size.to_be_bytes())
            .await?;
        self.localstore
            .kv_write(KV_PRIMARY, secondary, KV_ROOT, root)
            .await?;
        Ok(())
    }
}

fn decode_pubkey(b64: &str) -> Result<VerifyingKey, Error> {
    let bytes = BASE64
        .decode(b64)
        .map_err(|e| Error::Custom(format!("invalid log pubkey base64: {e}")))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Custom("log pubkey is not 32 bytes".to_string()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| Error::Custom(format!("invalid log pubkey: {e}")))
}

fn decode_proof(hex_nodes: &[String]) -> Result<Vec<Hash>, Error> {
    hex_nodes
        .iter()
        .map(|h| {
            hex::decode(h)
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| Error::Custom("invalid proof node hex".to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_tlog::checkpoint::{sign_checkpoint, Checkpoint};
    use cdk_tlog::merkle::{consistency_proof, leaf_hash, root_from_leaves};
    use ed25519_dalek::SigningKey;
    use rand_core::OsRng;

    use super::*;
    use crate::wallet::mint_connector::{
        AuditCheckpointResponse, AuditConsistencyResponse, AuditPubkeyResponse,
    };
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock, MockMintConnector,
    };

    const ORIGIN: &str = "test-mint.example.com/transparency-log";

    fn leaves(n: u8) -> Vec<Hash> {
        (0..n).map(|i| leaf_hash(&[i])).collect()
    }

    fn stage_mint_view(
        mock: &MockMintConnector,
        key: &SigningKey,
        leaves: &[Hash],
        consistency_from: Option<(u64, &[Hash])>,
    ) {
        let checkpoint = Checkpoint::new(ORIGIN, leaves.len() as u64, root_from_leaves(leaves));
        let signed = sign_checkpoint(checkpoint, ORIGIN, key);

        *mock.audit_pubkey_response.lock().unwrap() = Some(Ok(AuditPubkeyResponse {
            origin: ORIGIN.to_string(),
            pubkey: BASE64.encode(key.verifying_key().as_bytes()),
            signature_scheme: "ed25519".to_string(),
        }));
        *mock.audit_checkpoint_response.lock().unwrap() = Some(Ok(AuditCheckpointResponse {
            checkpoint: signed.to_note(),
            sigsum_proof: None,
        }));
        if let Some((first, proof_leaves)) = consistency_from {
            let proof = consistency_proof(first, proof_leaves).unwrap();
            *mock.audit_consistency_response.lock().unwrap() =
                Some(Ok(AuditConsistencyResponse {
                    first,
                    second: proof_leaves.len() as u64,
                    proof: proof.iter().map(hex::encode).collect(),
                }));
        }
    }

    #[tokio::test]
    async fn first_contact_pins_then_growth_requires_consistency() {
        let db = create_test_db().await;
        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;
        let key = SigningKey::generate(&mut OsRng);

        // First contact: TOFU pin at size 3.
        let all = leaves(3);
        stage_mint_view(&mock, &key, &all, None);
        let status = wallet.verify_transparency_log().await.unwrap();
        assert!(matches!(
            status,
            TransparencyStatus::Verified {
                size: 3,
                previous_size: None,
                ..
            }
        ));
        assert_eq!(
            wallet
                .pinned_transparency_checkpoint()
                .await
                .unwrap()
                .map(|(size, _)| size),
            Some(3)
        );

        // Log grows to 7 with a valid consistency proof: verified, pin advances.
        let all = leaves(7);
        stage_mint_view(&mock, &key, &all, Some((3, &all)));
        let status = wallet.verify_transparency_log().await.unwrap();
        assert!(matches!(
            status,
            TransparencyStatus::Verified {
                size: 7,
                previous_size: Some(3),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn rollback_and_rewrites_are_detected() {
        let db = create_test_db().await;
        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;
        let key = SigningKey::generate(&mut OsRng);

        let all = leaves(5);
        stage_mint_view(&mock, &key, &all, None);
        wallet.verify_transparency_log().await.unwrap();

        // Rollback: the mint presents a smaller (signed!) tree.
        stage_mint_view(&mock, &key, &leaves(3), None);
        assert_eq!(
            wallet.verify_transparency_log().await.unwrap(),
            TransparencyStatus::RollbackDetected {
                pinned_size: 5,
                presented_size: 3,
            }
        );

        // Rewrite at constant size: same size, different leaves.
        let mut forged = leaves(5);
        forged[0] = leaf_hash(b"rewritten history");
        stage_mint_view(&mock, &key, &forged, None);
        assert_eq!(
            wallet.verify_transparency_log().await.unwrap(),
            TransparencyStatus::RootMismatch { size: 5 }
        );

        // Growth whose consistency proof doesn't connect to the pinned
        // tree: forged history, grown to size 8.
        let mut forged_grown = forged.clone();
        forged_grown.extend(leaves(8)[5..].iter().copied());
        stage_mint_view(&mock, &key, &forged_grown, Some((5, &forged_grown)));
        assert_eq!(
            wallet.verify_transparency_log().await.unwrap(),
            TransparencyStatus::InconsistentHistory {
                pinned_size: 5,
                presented_size: 8,
            }
        );

        // The pin must not have moved through any of the failures.
        assert_eq!(
            wallet
                .pinned_transparency_checkpoint()
                .await
                .unwrap()
                .map(|(size, _)| size),
            Some(5)
        );
    }

    #[tokio::test]
    async fn changed_log_key_is_flagged() {
        let db = create_test_db().await;
        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;

        let key = SigningKey::generate(&mut OsRng);
        stage_mint_view(&mock, &key, &leaves(3), None);
        wallet.verify_transparency_log().await.unwrap();

        // Same origin, different signing key.
        let other_key = SigningKey::generate(&mut OsRng);
        stage_mint_view(&mock, &other_key, &leaves(4), None);
        assert_eq!(
            wallet.verify_transparency_log().await.unwrap(),
            TransparencyStatus::IdentityChanged
        );
    }
}
