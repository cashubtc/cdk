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
use cdk_tlog::checkpoint::{verify_checkpoint_signature, verify_cosignature, SignedCheckpoint};
use cdk_tlog::merkle::{verify_consistency, Hash, TreeHead};
use ed25519_dalek::VerifyingKey;

use super::Wallet;
use crate::Error;

/// A witness this wallet trusts to cosign a mint's checkpoints (NUT-XX
/// Witnessing). The wallet only counts cosignature lines that verify
/// against one of these.
#[derive(Debug, Clone)]
pub struct TrustedWitness {
    /// The name the witness signs cosignature lines with.
    pub name: String,
    /// The witness's Ed25519 public key.
    pub public_key: VerifyingKey,
}

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
        /// Names of trusted witnesses whose cosignatures on this
        /// checkpoint verified (empty when verifying without witnesses).
        cosigned_by: Vec<String>,
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
    /// Fewer trusted witnesses than required had verifiably cosigned the
    /// presented checkpoint. The pin was not advanced. NUT-XX: wallets
    /// SHOULD pin the largest verified, *witness-cosigned* checkpoint.
    InsufficientCosignatures {
        /// The presented checkpoint's tree size.
        size: u64,
        /// How many verified cosignatures the caller required.
        required: usize,
        /// How many trusted-witness cosignatures actually verified.
        verified: usize,
    },
}

/// Outcome of a full replay audit ([`Wallet::verify_transparency_log_replay`])
/// — NUT-XX "Verification" steps 4–6: fetch every entry, recompute every
/// leaf hash, rebuild the tree, and compare against the signed checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayStatus {
    /// Every entry's recomputed leaf hash matched, and the rebuilt tree's
    /// root equals the checkpoint's root hash: the served entries are
    /// exactly, and only, the history the mint committed to.
    Verified {
        /// The verified checkpoint's tree size.
        size: u64,
        /// The verified (and independently recomputed) root hash.
        root: Hash,
    },
    /// An entry's recomputed leaf hash did not match the one the mint
    /// returned for it.
    LeafHashMismatch {
        /// The zero-based leaf index of the offending entry.
        seq: u64,
    },
    /// The tree rebuilt from the served entries does not match the signed
    /// checkpoint's root hash.
    RootMismatch {
        /// The checkpoint's tree size.
        size: u64,
    },
    /// The mint stopped serving entries (or served them out of order)
    /// before the checkpoint's tree size was reached.
    MissingEntries {
        /// The checkpoint's tree size.
        expected: u64,
        /// How many contiguous entries were actually served.
        got: u64,
    },
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
        self.verify_transparency_log_with_witnesses(&[], 0).await
    }

    /// Like [`Self::verify_transparency_log`], but additionally verifies
    /// witness cosignatures on the checkpoint and requires at least
    /// `min_cosignatures` of the given trusted witnesses to have cosigned
    /// it before the pin is advanced — the "largest verified,
    /// witness-cosigned checkpoint" behavior NUT-XX asks of wallets.
    /// Rollback/rewrite verdicts still take precedence: a hostile status is
    /// more informative than a missing cosignature.
    pub async fn verify_transparency_log_with_witnesses(
        &self,
        witnesses: &[TrustedWitness],
        min_cosignatures: usize,
    ) -> Result<TransparencyStatus, Error> {
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

        // Count which trusted witnesses verifiably cosigned this note.
        let cosigned_by: Vec<String> = witnesses
            .iter()
            .filter(|witness| {
                signed.signatures.iter().any(|line| {
                    verify_cosignature(
                        &signed.checkpoint,
                        &witness.name,
                        &witness.public_key,
                        line,
                    )
                    .is_ok()
                })
            })
            .map(|witness| witness.name.clone())
            .collect();

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
                    if cosigned_by.len() < min_cosignatures {
                        return Ok(TransparencyStatus::InsufficientCosignatures {
                            size: presented_size,
                            required: min_cosignatures,
                            verified: cosigned_by.len(),
                        });
                    }
                    // Nothing new; identical checkpoint re-verified.
                    return Ok(TransparencyStatus::Verified {
                        size: presented_size,
                        root: presented_root,
                        previous_size: Some(pinned_size),
                        cosigned_by,
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

        // Consistency verdicts above take precedence; only a checkpoint
        // that would otherwise verify is held back for lack of witnesses.
        if cosigned_by.len() < min_cosignatures {
            return Ok(TransparencyStatus::InsufficientCosignatures {
                size: presented_size,
                required: min_cosignatures,
                verified: cosigned_by.len(),
            });
        }

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
            cosigned_by,
        })
    }

    /// Full replay audit — NUT-XX "Verification" steps 4–6: fetches the
    /// latest signed checkpoint, downloads every log entry in
    /// `[0, tree_size)`, recomputes each entry's leaf hash from its own
    /// fields (rejecting any mismatch), rebuilds the RFC 6962 tree, and
    /// compares the resulting root against the checkpoint.
    ///
    /// This is deliberately independent of the pin: it audits what the
    /// mint *serves*, and can be run by any third party. Bandwidth scales
    /// with the log size; wallets are expected to run it occasionally (or
    /// leave it to dedicated auditors), not on every interaction.
    pub async fn verify_transparency_log_replay(&self) -> Result<ReplayStatus, Error> {
        let pubkey_response = self.client.get_audit_pubkey().await?;
        let checkpoint_response = self.client.get_audit_checkpoint().await?;

        let presented_key = decode_pubkey(&pubkey_response.pubkey)?;
        let origin = pubkey_response.origin;

        let signed = SignedCheckpoint::parse(&checkpoint_response.checkpoint)
            .map_err(|e| Error::Custom(format!("malformed checkpoint note: {e}")))?;
        if signed.checkpoint.origin != origin {
            return Err(Error::Custom(
                "checkpoint origin does not match the mint's declared origin".to_string(),
            ));
        }
        if !signed.signatures.iter().any(|line| {
            verify_checkpoint_signature(&signed.checkpoint, &origin, &presented_key, line).is_ok()
        }) {
            return Err(Error::Custom(
                "checkpoint carries no valid signature from the mint's log key".to_string(),
            ));
        }

        let size = signed.checkpoint.size;
        let mut tree = TreeHead::empty();
        let mut next = 0u64;

        while next < size {
            let response = self.client.get_audit_entries(next, size).await?;
            if response.entries.is_empty() {
                return Ok(ReplayStatus::MissingEntries {
                    expected: size,
                    got: next,
                });
            }
            for entry in response.entries {
                if entry.seq != next {
                    return Ok(ReplayStatus::MissingEntries {
                        expected: size,
                        got: next,
                    });
                }

                // NUT-XX: verifiers MUST recompute the leaf hash and
                // reject entries whose returned hash doesn't match. The
                // payload bytes are the canonical (compact, sorted-key)
                // JSON re-serialization of the served payload object.
                let payload_bytes = serde_json::to_vec(&entry.payload)
                    .map_err(|e| Error::Custom(format!("unserializable payload: {e}")))?;
                let op = match entry.op.as_str() {
                    "insert" => 0u8,
                    "update" => 1u8,
                    "delete" => 2u8,
                    other => {
                        return Err(Error::Custom(format!("unknown log entry op: {other}")));
                    }
                };
                let preimage = raw_leaf_preimage(
                    &entry.entity_type,
                    &entry.entity_id,
                    op,
                    entry.created_time,
                    &payload_bytes,
                );
                let hash = cdk_tlog::merkle::leaf_hash(&preimage);
                if hex::encode(hash) != entry.leaf_hash.to_lowercase() {
                    return Ok(ReplayStatus::LeafHashMismatch { seq: entry.seq });
                }

                tree.append(hash);
                next += 1;
            }
        }

        if tree.root() != signed.checkpoint.root_hash {
            return Ok(ReplayStatus::RootMismatch { size });
        }

        Ok(ReplayStatus::Verified {
            size,
            root: signed.checkpoint.root_hash,
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

/// NUT-XX leaf preimage over the entry's *served* fields, kept
/// string-typed (rather than going through `LoggedEntity`) so entries with
/// implementation-specific event kinds — which verifiers MUST preserve —
/// still hash correctly.
fn raw_leaf_preimage(
    entity_type: &str,
    entity_id: &str,
    op: u8,
    created_time: u64,
    payload: &[u8],
) -> Vec<u8> {
    let mut buf =
        Vec::with_capacity(entity_type.len() + entity_id.len() + payload.len() + 10);
    buf.extend_from_slice(entity_type.as_bytes());
    buf.push(0);
    buf.extend_from_slice(entity_id.as_bytes());
    buf.push(0);
    buf.push(op);
    buf.extend_from_slice(&created_time.to_be_bytes());
    buf.extend_from_slice(payload);
    buf
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
        AuditCheckpointResponse, AuditConsistencyResponse, AuditLogEntry, AuditPubkeyResponse,
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
    async fn witness_cosignature_gates_pin_advancement() {
        use cdk_tlog::checkpoint::cosign;

        let db = create_test_db().await;
        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;
        let key = SigningKey::generate(&mut OsRng);
        let witness_key = SigningKey::generate(&mut OsRng);
        let witness = TrustedWitness {
            name: "witness.example/w1".to_string(),
            public_key: witness_key.verifying_key(),
        };

        // Cosigned checkpoint at size 3: verifies, pin advances.
        let all = leaves(3);
        let checkpoint = Checkpoint::new(ORIGIN, 3, root_from_leaves(&all));
        let mut signed = sign_checkpoint(checkpoint, ORIGIN, &key);
        let cosig = cosign(
            &signed.checkpoint,
            1_700_000_000,
            &witness.name,
            &witness_key,
        );
        signed.signatures.push(cosig);
        *mock.audit_pubkey_response.lock().unwrap() = Some(Ok(AuditPubkeyResponse {
            origin: ORIGIN.to_string(),
            pubkey: BASE64.encode(key.verifying_key().as_bytes()),
            signature_scheme: "ed25519".to_string(),
        }));
        *mock.audit_checkpoint_response.lock().unwrap() = Some(Ok(AuditCheckpointResponse {
            checkpoint: signed.to_note(),
            sigsum_proof: None,
        }));

        let status = wallet
            .verify_transparency_log_with_witnesses(std::slice::from_ref(&witness), 1)
            .await
            .unwrap();
        match status {
            TransparencyStatus::Verified {
                size, cosigned_by, ..
            } => {
                assert_eq!(size, 3);
                assert_eq!(cosigned_by, vec![witness.name.clone()]);
            }
            other => panic!("expected Verified, got {other:?}"),
        }

        // Grown checkpoint without any cosignature: held back, pin stays.
        let all = leaves(7);
        stage_mint_view(&mock, &key, &all, Some((3, &all)));
        assert_eq!(
            wallet
                .verify_transparency_log_with_witnesses(std::slice::from_ref(&witness), 1)
                .await
                .unwrap(),
            TransparencyStatus::InsufficientCosignatures {
                size: 7,
                required: 1,
                verified: 0,
            }
        );
        assert_eq!(
            wallet
                .pinned_transparency_checkpoint()
                .await
                .unwrap()
                .map(|(size, _)| size),
            Some(3),
            "pin must not advance on a checkpoint lacking required cosignatures"
        );
    }

    fn make_entry(seq: u64, payload: serde_json::Value) -> AuditLogEntry {
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let preimage = raw_leaf_preimage("proof", "02aa", 1, 100 + seq, &payload_bytes);
        let hash = cdk_tlog::merkle::leaf_hash(&preimage);
        AuditLogEntry {
            seq,
            entity_type: "proof".to_string(),
            op: "update".to_string(),
            entity_id: "02aa".to_string(),
            payload,
            created_time: 100 + seq,
            leaf_hash: hex::encode(hash),
        }
    }

    fn stage_replay_view(mock: &MockMintConnector, key: &SigningKey, entries: &[AuditLogEntry]) {
        let leaf_hashes: Vec<Hash> = entries
            .iter()
            .map(|e| hex::decode(&e.leaf_hash).unwrap().try_into().unwrap())
            .collect();
        let checkpoint = Checkpoint::new(
            ORIGIN,
            leaf_hashes.len() as u64,
            root_from_leaves(&leaf_hashes),
        );
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
        *mock.audit_entries.lock().unwrap() = Some(entries.to_vec());
    }

    #[tokio::test]
    async fn full_replay_verifies_and_detects_tampering() {
        let db = create_test_db().await;
        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;
        let key = SigningKey::generate(&mut OsRng);

        let entries: Vec<AuditLogEntry> = (0..5)
            .map(|seq| make_entry(seq, serde_json::json!({ "state": "SPENT" })))
            .collect();

        // Honest view: replay verifies end to end.
        stage_replay_view(&mock, &key, &entries);
        let status = wallet.verify_transparency_log_replay().await.unwrap();
        assert!(
            matches!(status, ReplayStatus::Verified { size: 5, .. }),
            "expected Verified, got {status:?}"
        );

        // Tampered payload on one entry: its recomputed leaf hash no
        // longer matches the served one.
        let mut tampered = entries.clone();
        tampered[2].payload = serde_json::json!({ "state": "UNSPENT" });
        stage_replay_view(&mock, &key, &entries);
        *mock.audit_entries.lock().unwrap() = Some(tampered);
        assert_eq!(
            wallet.verify_transparency_log_replay().await.unwrap(),
            ReplayStatus::LeafHashMismatch { seq: 2 }
        );

        // Entries served with internally consistent hashes, but not the
        // ones the checkpoint committed to: the rebuilt root differs.
        let other_entries: Vec<AuditLogEntry> = (0..5)
            .map(|seq| make_entry(seq, serde_json::json!({ "state": "PENDING" })))
            .collect();
        stage_replay_view(&mock, &key, &entries);
        *mock.audit_entries.lock().unwrap() = Some(other_entries);
        assert_eq!(
            wallet.verify_transparency_log_replay().await.unwrap(),
            ReplayStatus::RootMismatch { size: 5 }
        );

        // Mint withholds the tail of the log.
        stage_replay_view(&mock, &key, &entries);
        *mock.audit_entries.lock().unwrap() = Some(entries[..3].to_vec());
        assert_eq!(
            wallet.verify_transparency_log_replay().await.unwrap(),
            ReplayStatus::MissingEntries {
                expected: 5,
                got: 3,
            }
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
