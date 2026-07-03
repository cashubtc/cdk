//! Built-in witness: cosigns *other* transparency logs' checkpoints,
//! speaking the [C2SP tlog-witness protocol][spec]'s `add-checkpoint` call
//! (see the NUT-XX draft's [Witnessing](../../../../docs/adr/nut-xx.md)
//! section). Any two mints running `cdk-mintd` can point at each other's
//! witness endpoint and get mutual cosigning for free, with no shared
//! infrastructure and no central authority — the same mechanism already
//! used by Sigsum's independent witness operators.
//!
//! This is deliberately a separate identity from a mint's own
//! transparency log: a mint witnessing other logs uses its own dedicated
//! Ed25519 witness key, distinct from the log-signing key it uses for its
//! own checkpoints (see [`super::transparency::TransparencyLogService`]).
//!
//! [spec]: https://github.com/C2SP/C2SP/blob/main/tlog-witness.md

use std::fmt;

use cdk_common::database::DynMintDatabase;
use cdk_tlog::witness::{consider_checkpoint, AddCheckpointRequest, ParseError, WitnessError};
use cdk_tlog::SignatureLine;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand_core::OsRng;

use crate::error::Error;

const KV_NAMESPACE: &str = "cdk_witness";
const KV_SIGNING_KEY: &str = "signing_key";

/// A trusted (origin, public key) pair this witness is willing to cosign
/// checkpoints for.
#[derive(Debug, Clone)]
pub struct TrustedLog {
    /// The checkpoint origin line this entry applies to.
    pub origin: String,
    /// The log's Ed25519 public key.
    pub public_key: VerifyingKey,
}

/// Errors returned when considering an `add-checkpoint` submission,
/// carrying enough information for an HTTP layer to pick the exact status
/// code the spec requires for each case.
#[derive(Debug, thiserror::Error)]
pub enum AddCheckpointError {
    /// The request body was malformed (HTTP 400).
    #[error("malformed request: {0}")]
    Malformed(#[from] ParseError),
    /// The witness declined to cosign (see [`WitnessError`] for the
    /// specific reason and its corresponding status code).
    #[error(transparent)]
    Declined(#[from] WitnessError),
    /// Persisting the new cosigned state failed.
    #[error(transparent)]
    Storage(#[from] Error),
}

/// Built-in witness service: cosigns other transparency logs' checkpoints
/// per a configured, static trust policy.
pub struct Witness {
    kv_db: DynMintDatabase,
    name: String,
    signing_key: SigningKey,
    trusted: Vec<TrustedLog>,
    /// Serializes the read-check-persist path of `handle_add_checkpoint`.
    /// Without it, two concurrent submissions for the same origin could
    /// both read the same last-cosigned state and each get a cosignature
    /// on a *different* root at the same size — the exact equivocation a
    /// witness exists to prevent (tlog-witness spec calls this race out
    /// explicitly; persistence must happen atomically before responding).
    submission_lock: tokio::sync::Mutex<()>,
}

impl fmt::Debug for Witness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Witness")
            .field("name", &self.name)
            .field(
                "trusted",
                &self.trusted.iter().map(|t| &t.origin).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl Witness {
    /// Loads (or, on first run, generates and persists) this witness's
    /// dedicated Ed25519 key, and configures which logs it's willing to
    /// cosign for.
    pub async fn load_or_create(
        kv_db: DynMintDatabase,
        name: String,
        trusted: Vec<TrustedLog>,
    ) -> Result<Self, Error> {
        let signing_key = load_or_create_witness_key(&kv_db).await?;
        Ok(Self {
            kv_db,
            name,
            signing_key,
            trusted,
            submission_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// This witness's name, as it will appear in cosignature lines.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// This witness's Ed25519 public key, base64-encoded — what another
    /// mint puts in its `[[transparency_log.witnesses]]` config to have
    /// its checkpoints cosigned here.
    pub fn public_key_base64(&self) -> String {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        BASE64.encode(self.signing_key.verifying_key().as_bytes())
    }

    /// Handles one `add-checkpoint` submission: parses it, checks it
    /// against the trust policy and this witness's own last-cosigned
    /// state for that origin, and — if everything checks out — persists
    /// the new state and returns the cosignature line to send back.
    pub async fn handle_add_checkpoint(
        &self,
        body: &str,
    ) -> Result<SignatureLine, AddCheckpointError> {
        let request = AddCheckpointRequest::parse(body)?;
        let origin = request.checkpoint.checkpoint.origin.clone();

        let trusted = self
            .trusted
            .iter()
            .find(|log| log.origin == origin)
            .ok_or(WitnessError::UnknownOrigin)?;

        let _guard = self.submission_lock.lock().await;
        let last_cosigned = self.read_last_cosigned(&origin).await?;
        let now = cdk_common::util::unix_time();

        let (cosignature, size, root) = consider_checkpoint(
            &request,
            &trusted.origin,
            &trusted.public_key,
            last_cosigned,
            &self.name,
            &self.signing_key,
            now,
        )?;

        self.write_last_cosigned(&origin, size, root).await?;
        Ok(cosignature)
    }

    async fn read_last_cosigned(&self, origin: &str) -> Result<Option<(u64, [u8; 32])>, Error> {
        let bytes = self
            .kv_db
            .kv_read(KV_NAMESPACE, "state", &origin_key(origin))
            .await?;
        Ok(match bytes {
            Some(bytes) if bytes.len() == 40 => {
                let size = u64::from_be_bytes(bytes[..8].try_into().expect("checked len"));
                let root: [u8; 32] = bytes[8..].try_into().expect("checked len");
                Some((size, root))
            }
            _ => None,
        })
    }

    async fn write_last_cosigned(
        &self,
        origin: &str,
        size: u64,
        root: [u8; 32],
    ) -> Result<(), Error> {
        let mut bytes = Vec::with_capacity(40);
        bytes.extend_from_slice(&size.to_be_bytes());
        bytes.extend_from_slice(&root);

        let mut tx = self.kv_db.begin_transaction().await?;
        tx.kv_write(KV_NAMESPACE, "state", &origin_key(origin), &bytes)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Namespaces persisted per-origin state under a hash of the origin
/// string, since raw origins (schema-less URLs) can contain characters
/// the KV store's key alphabet doesn't allow (see
/// `KVSTORE_NAMESPACE_KEY_ALPHABET`).
fn origin_key(origin: &str) -> String {
    use bitcoin::hashes::sha256;
    use bitcoin::hashes::Hash;

    hex::encode(sha256::Hash::hash(origin.as_bytes()).to_byte_array())
}

async fn load_or_create_witness_key(db: &DynMintDatabase) -> Result<SigningKey, Error> {
    if let Some(bytes) = db.kv_read(KV_NAMESPACE, "keys", KV_SIGNING_KEY).await? {
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Custom("stored witness key was not 32 bytes".into()))?;
        return Ok(SigningKey::from_bytes(&seed));
    }

    let key = SigningKey::generate(&mut OsRng);
    let mut tx = db.begin_transaction().await?;
    tx.kv_write(
        KV_NAMESPACE,
        "keys",
        KV_SIGNING_KEY,
        key.to_bytes().as_slice(),
    )
    .await?;
    tx.commit().await?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use cdk_tlog::checkpoint::{sign_checkpoint, Checkpoint};
    use cdk_tlog::merkle::{leaf_hash, Hash};
    use cdk_tlog::TreeHead;

    use super::*;

    async fn memory_db() -> DynMintDatabase {
        std::sync::Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap())
    }

    fn build_checkpoint(log_key: &SigningKey, origin: &str, leaves: &[Hash]) -> String {
        let mut tree = TreeHead::empty();
        for &leaf in leaves {
            tree.append(leaf);
        }
        let checkpoint = Checkpoint::new(origin, tree.size, tree.root());
        sign_checkpoint(checkpoint, origin, log_key).to_note()
    }

    #[tokio::test]
    async fn cosigns_a_trusted_logs_first_checkpoint() {
        let db = memory_db().await;
        let log_key = SigningKey::generate(&mut OsRng);
        let witness = Witness::load_or_create(
            db,
            "witness.example/w1".to_string(),
            vec![TrustedLog {
                origin: "mint-a.example/transparency-log".to_string(),
                public_key: log_key.verifying_key(),
            }],
        )
        .await
        .unwrap();

        let leaves: Vec<Hash> = (0u8..3).map(|i| leaf_hash(&[i])).collect();
        let checkpoint_note =
            build_checkpoint(&log_key, "mint-a.example/transparency-log", &leaves);
        let body = format!("old 0\n\n{checkpoint_note}");

        let cosig = witness.handle_add_checkpoint(&body).await.unwrap();
        assert_eq!(cosig.name, "witness.example/w1");
    }

    #[tokio::test]
    async fn rejects_checkpoints_from_untrusted_origins() {
        let db = memory_db().await;
        let log_key = SigningKey::generate(&mut OsRng);
        let witness = Witness::load_or_create(db, "witness.example/w1".to_string(), vec![])
            .await
            .unwrap();

        let leaves: Vec<Hash> = (0u8..1).map(|i| leaf_hash(&[i])).collect();
        let checkpoint_note =
            build_checkpoint(&log_key, "mint-a.example/transparency-log", &leaves);
        let body = format!("old 0\n\n{checkpoint_note}");

        let err = witness.handle_add_checkpoint(&body).await.unwrap_err();
        assert!(matches!(
            err,
            AddCheckpointError::Declined(WitnessError::UnknownOrigin)
        ));
    }

    #[tokio::test]
    async fn remembers_state_across_submissions() {
        let db = memory_db().await;
        let log_key = SigningKey::generate(&mut OsRng);
        let witness = Witness::load_or_create(
            db,
            "witness.example/w1".to_string(),
            vec![TrustedLog {
                origin: "mint-a.example/transparency-log".to_string(),
                public_key: log_key.verifying_key(),
            }],
        )
        .await
        .unwrap();

        let leaves: Vec<Hash> = (0u8..5).map(|i| leaf_hash(&[i])).collect();
        let first_note =
            build_checkpoint(&log_key, "mint-a.example/transparency-log", &leaves[..3]);
        witness
            .handle_add_checkpoint(&format!("old 0\n\n{first_note}"))
            .await
            .unwrap();

        // Resubmitting the same old size (0) again must now conflict,
        // since this witness has already moved on to size 3.
        let err = witness
            .handle_add_checkpoint(&format!("old 0\n\n{first_note}"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AddCheckpointError::Declined(WitnessError::SizeConflict {
                claimed: 0,
                stored: 3
            })
        ));
    }
}
