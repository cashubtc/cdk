//! Mint transparency log: background checkpoint publisher and query
//! surface for the audit HTTP endpoints (see
//! `docs/adr/0001-append-only-transparency-log.md` and the accompanying
//! NUT-XX draft).
//!
//! This service owns:
//!
//! 1. Advancing a [`cdk_tlog::TreeHead`] from newly appended
//!    `mint_event_log` rows (via [`cdk_common::database::mint::TransparencyLogDatabase`]).
//! 2. Periodically signing a [`cdk_tlog::checkpoint::SignedCheckpoint`]
//!    over the current tree head.
//! 3. Optionally anchoring that checkpoint's hash to a public Sigsum log
//!    via `cdk-sigsum`.
//!
//! Tree state, checkpoints, and the log-signing key are persisted through
//! the mint's existing generic key-value store rather than a bespoke
//! schema, since they're small, single-valued records with no need for
//! relational range queries (only the event log itself needs that, and
//! already has it via `mint_event_log`).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use cdk_common::database::mint::DynTransparencyLogDatabase;
use cdk_common::database::DynMintDatabase;
use cdk_tlog::checkpoint::{sign_checkpoint, Checkpoint, SignedCheckpoint};
use cdk_tlog::merkle::{consistency_proof, inclusion_proof, Hash};
use cdk_tlog::TreeHead;
use ed25519_dalek::SigningKey;
#[cfg(feature = "sigsum-anchor")]
use ed25519_dalek::VerifyingKey;
use rand_core::OsRng;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::error::Error;

const KV_NAMESPACE: &str = "cdk_transparency";
const KV_SIGNING_KEY: &str = "signing_key";
const KV_TREE_STATE: &str = "tree_state";
const KV_LATEST_CHECKPOINT_SIZE: &str = "latest_checkpoint_size";

/// How many `mint_event_log` rows the publisher folds into the tree per
/// batch, at most.
const BATCH_SIZE: u64 = 1_000;

/// Upper bound on one external Sigsum anchoring attempt. `cdk_sigsum::anchor`
/// polls the log until the leaf is committed and covered by a published tree
/// head, which can take arbitrarily long against a slow or wedged log; the
/// publisher tick (and, transitively, mint shutdown) must not be held hostage
/// by that.
#[cfg(feature = "sigsum-anchor")]
const SIGSUM_ANCHOR_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on one outbound witness `add-checkpoint` HTTP exchange.
const WITNESS_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A witness (speaking C2SP tlog-witness) this mint asks to cosign every
/// checkpoint it publishes — e.g. another cdk mint's
/// `/witness/add-checkpoint` endpoint, or a standalone witness.
#[derive(Debug, Clone)]
pub struct CheckpointWitness {
    /// Full URL of the witness's `add-checkpoint` endpoint.
    pub url: String,
    /// The name the witness signs cosignature lines with.
    pub name: String,
    /// The witness's Ed25519 public key, used to verify returned
    /// cosignatures before attaching them to the stored checkpoint.
    pub public_key: ed25519_dalek::VerifyingKey,
}

/// Configuration for anchoring checkpoints to an external Sigsum log.
#[cfg(feature = "sigsum-anchor")]
#[derive(Debug, Clone)]
pub struct SigsumAnchorConfig {
    /// Base URL of the Sigsum log to submit to (e.g. `https://seasalp.glasklar.is/`).
    pub log_url: url::Url,
    /// The Sigsum log's own Ed25519 public key.
    pub log_public_key: VerifyingKey,
    /// The Ed25519 keypair this mint uses to submit leaves.
    pub submit_key: SigningKey,
    /// Optional domain-based rate-limit token, if the log requires one.
    pub token: Option<cdk_sigsum::SubmitToken>,
}

/// Background service that advances the mint's transparency-log Merkle
/// tree and periodically signs checkpoints over it.
pub struct TransparencyLogService {
    log_db: DynTransparencyLogDatabase,
    kv_db: DynMintDatabase,
    origin: String,
    signing_key: SigningKey,
    #[cfg(feature = "sigsum-anchor")]
    sigsum: Option<SigsumAnchorConfig>,
    /// Witnesses asked to cosign every published checkpoint.
    witnesses: Vec<CheckpointWitness>,
    /// How often the background publisher ticks (see [`Self::spawn`]).
    publish_interval: Duration,
    /// Serializes `run_once` so a slow tick can't overlap the next one.
    run_lock: Mutex<()>,
}

impl fmt::Debug for TransparencyLogService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransparencyLogService")
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

impl TransparencyLogService {
    /// Loads (or, on first run, generates and persists) the service's
    /// state: its dedicated Ed25519 log-signing key and its tree
    /// advancement cursor. `origin` is the checkpoint origin line this
    /// mint will sign (see NUT-XX, e.g. `"<mint-domain>/transparency-log"`).
    pub async fn load_or_create(
        log_db: DynTransparencyLogDatabase,
        kv_db: DynMintDatabase,
        origin: String,
    ) -> Result<Self, Error> {
        let signing_key = load_or_create_signing_key(&kv_db).await?;
        Ok(Self {
            log_db,
            kv_db,
            origin,
            signing_key,
            #[cfg(feature = "sigsum-anchor")]
            sigsum: None,
            witnesses: Vec::new(),
            publish_interval: Duration::from_secs(30),
            run_lock: Mutex::new(()),
        })
    }

    /// Configures witnesses this service asks to cosign each published
    /// checkpoint. Best-effort, like external anchoring: an unreachable or
    /// declining witness costs that checkpoint its cosignature line, never
    /// the publishing tick.
    pub fn with_witnesses(mut self, witnesses: Vec<CheckpointWitness>) -> Self {
        self.witnesses = witnesses;
        self
    }

    /// Overrides how often the background publisher ticks (default 30s).
    pub fn with_publish_interval(mut self, interval: Duration) -> Self {
        self.publish_interval = interval;
        self
    }

    /// The configured publishing interval, used by [`crate::mint::Mint::start`]
    /// when spawning the background task.
    pub fn publish_interval(&self) -> Duration {
        self.publish_interval
    }

    /// Configures external anchoring of every future checkpoint to a
    /// Sigsum log (see `crates/cdk-sigsum`). Best-effort: a failed anchor
    /// attempt is logged and does not fail the publishing tick, since the
    /// mint's own `/v1/audit/*` surface must keep working regardless of an
    /// external service's availability.
    #[cfg(feature = "sigsum-anchor")]
    pub fn with_sigsum_anchor(mut self, config: SigsumAnchorConfig) -> Self {
        self.sigsum = Some(config);
        self
    }

    /// The checkpoint origin line this service signs.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The log-signing public key, base64-encoded (32 raw bytes).
    pub fn public_key_base64(&self) -> String {
        BASE64.encode(self.signing_key.verifying_key().as_bytes())
    }

    /// Runs one advancement + (maybe) checkpoint-signing cycle. Returns
    /// the newly signed checkpoint, if one was published this cycle.
    #[tracing::instrument(skip(self))]
    pub async fn run_once(&self) -> Result<Option<SignedCheckpoint>, Error> {
        let _guard = self.run_lock.lock().await;

        let mut tree = read_tree_state(&self.kv_db).await?;

        let mut advanced = false;
        loop {
            // First drain any rows already assigned a leaf index but not
            // yet folded — this is the crash-recovery path: leaf indices
            // are durable in the event log table itself, so a crash
            // between assignment and the tree-state write below just
            // replays the same leaves into the same positions.
            let mut rows = self
                .log_db
                .get_event_log_range(tree.size, tree.size + BATCH_SIZE)
                .await?;
            if rows.is_empty() {
                // Then have the DB assign indices to newly committed rows.
                // Row-id gaps (permanent on Postgres after a rollback)
                // don't exist in leaf-index space: the appender numbers
                // exactly the committed rows it observes, densely.
                rows = self.log_db.assign_leaf_indices(BATCH_SIZE).await?;
                rows.retain(|row| row.seq >= tree.size);
            }
            if rows.is_empty() {
                break;
            }

            for row in rows {
                if row.seq != tree.size {
                    return Err(Error::Custom(format!(
                        "transparency log leaf indices are not contiguous: expected {}, got {}",
                        tree.size, row.seq
                    )));
                }
                tree.append(row.leaf_hash);
                advanced = true;
            }
        }

        if tree.size == 0 {
            return Ok(None);
        }

        let latest_signed_size = read_u64(&self.kv_db, KV_LATEST_CHECKPOINT_SIZE)
            .await?
            .unwrap_or(0);
        if !advanced && latest_signed_size == tree.size {
            return Ok(None);
        }

        let checkpoint = Checkpoint::new(self.origin.clone(), tree.size, tree.root());
        let signed = sign_checkpoint(checkpoint, &self.origin, &self.signing_key);

        // Tree state, the checkpoint note, and the latest-checkpoint
        // pointer land in one KV transaction: a crash can never leave the
        // persisted tree ahead of or behind the leaves it was built from,
        // which would silently skip (or double-fold) leaves forever.
        persist_tick(&self.kv_db, &tree, &signed).await?;

        // Best-effort extras, after the checkpoint itself is durable.
        let signed = self.collect_witness_cosignatures(signed).await;

        #[cfg(feature = "sigsum-anchor")]
        self.anchor_to_sigsum(&signed).await;

        Ok(Some(signed))
    }

    /// Asks every configured witness to cosign `signed` (C2SP tlog-witness
    /// `add-checkpoint`), verifies each returned cosignature, appends the
    /// valid ones to the note, and re-persists it so `/v1/audit/checkpoint`
    /// serves the cosigned version. Entirely best-effort.
    async fn collect_witness_cosignatures(&self, mut signed: SignedCheckpoint) -> SignedCheckpoint {
        if self.witnesses.is_empty() {
            return signed;
        }

        let new_size = signed.checkpoint.size;
        let leaves = match self.leaf_hashes_up_to(new_size).await {
            Ok(leaves) => leaves,
            Err(err) => {
                tracing::warn!("cannot build witness consistency proofs: {err}");
                return signed;
            }
        };

        let client = match reqwest::Client::builder()
            .timeout(WITNESS_REQUEST_TIMEOUT)
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!("failed to build witness HTTP client: {err}");
                return signed;
            }
        };

        let mut added_any = false;
        for witness in &self.witnesses {
            match self
                .request_cosignature(&client, witness, &signed, &leaves)
                .await
            {
                Ok(line) => {
                    tracing::info!(
                        witness = witness.name,
                        tree_size = new_size,
                        "checkpoint cosigned by witness"
                    );
                    signed.signatures.push(line);
                    added_any = true;
                }
                Err(err) => {
                    tracing::warn!(
                        witness = witness.name,
                        "failed to obtain witness cosignature: {err}"
                    );
                }
            }
        }

        if added_any {
            if let Err(err) = persist_checkpoint_note(&self.kv_db, new_size, &signed).await {
                tracing::warn!("failed to persist cosigned checkpoint note: {err}");
            }
        }
        signed
    }

    /// One witness exchange: submit with our recorded `old_size` for this
    /// witness, follow the spec's 409 recovery (response body carries the
    /// witness's actual last-cosigned size) once, verify the returned
    /// cosignature line before trusting it, and remember the new size.
    async fn request_cosignature(
        &self,
        client: &reqwest::Client,
        witness: &CheckpointWitness,
        signed: &SignedCheckpoint,
        leaves: &[Hash],
    ) -> Result<cdk_tlog::SignatureLine, Error> {
        use cdk_tlog::witness::AddCheckpointRequest;

        let ack_key = witness_ack_key(&witness.name);
        let mut old_size = read_kv_u64(&self.kv_db, "witness_ack", &ack_key)
            .await?
            .unwrap_or(0);

        for attempt in 0..2 {
            let proof = consistency_proof(old_size, leaves)
                .map_err(|e| Error::Custom(e.to_string()))?;
            let body = AddCheckpointRequest {
                old_size,
                consistency_proof: proof,
                checkpoint: signed.clone(),
            }
            .to_body();

            let response = client
                .post(&witness.url)
                .header("content-type", "text/plain")
                .body(body)
                .send()
                .await
                .map_err(|e| Error::Custom(format!("witness request failed: {e}")))?;

            let status = response.status();
            let text = response
                .text()
                .await
                .map_err(|e| Error::Custom(format!("witness response unreadable: {e}")))?;

            if status == reqwest::StatusCode::CONFLICT && attempt == 0 {
                // 409: the witness's last-cosigned size differs from our
                // record; the body is its actual size. Resync and retry.
                if let Ok(stored) = text.trim().parse::<u64>() {
                    old_size = stored;
                    continue;
                }
                return Err(Error::Custom(format!(
                    "witness 409 with unparseable size: {text:?}"
                )));
            }
            if !status.is_success() {
                return Err(Error::Custom(format!(
                    "witness declined ({status}): {}",
                    text.trim()
                )));
            }

            // 200: one or more cosignature lines. Take the first one that
            // actually verifies against this witness's key.
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                let Ok(parsed) = cdk_tlog::SignatureLine::parse_line(line) else {
                    continue;
                };
                if cdk_tlog::checkpoint::verify_cosignature(
                    &signed.checkpoint,
                    &witness.name,
                    &witness.public_key,
                    &parsed,
                )
                .is_ok()
                {
                    write_kv_u64(&self.kv_db, "witness_ack", &ack_key, signed.checkpoint.size)
                        .await?;
                    return Ok(parsed);
                }
            }
            return Err(Error::Custom(
                "witness returned no verifiable cosignature".to_string(),
            ));
        }
        Err(Error::Custom(
            "witness old-size conflict persisted after resync".to_string(),
        ))
    }

    /// Best-effort external anchor of `checkpoint`'s note text to the
    /// configured Sigsum log, if any. Never fails the calling tick.
    #[cfg(feature = "sigsum-anchor")]
    async fn anchor_to_sigsum(&self, checkpoint: &SignedCheckpoint) {
        let Some(config) = &self.sigsum else {
            return;
        };
        let client = cdk_sigsum::SigsumClient::new(config.log_url.clone());
        // Hard deadline: `anchor` polls the log until the leaf is covered
        // by a published tree head, which is unbounded against a slow or
        // wedged log. Anchoring is best-effort, so give up and move on —
        // the next published checkpoint gets its own attempt.
        let outcome = tokio::time::timeout(
            SIGSUM_ANCHOR_TIMEOUT,
            cdk_sigsum::anchor(
                &client,
                &config.log_public_key,
                &config.submit_key,
                config.token.as_ref(),
                checkpoint.to_note().as_bytes(),
            ),
        )
        .await;
        match outcome {
            Ok(Ok(proof)) => {
                let size = checkpoint.checkpoint.size;
                // Persist the offline-verifiable proof next to the
                // checkpoint it anchors (ADR §7.1), so `/v1/audit/checkpoint`
                // can serve it to auditors. Best-effort like the anchoring
                // itself: a failed write only costs this proof, not the tick.
                if let Err(err) = persist_sigsum_proof(&self.kv_db, size, &proof.to_ascii()).await
                {
                    tracing::warn!("failed to persist Sigsum proof: {err}");
                } else {
                    tracing::info!(
                        tree_size = size,
                        "anchored transparency log checkpoint to Sigsum log"
                    );
                }
            }
            Ok(Err(err)) => {
                tracing::warn!("failed to anchor checkpoint to Sigsum log: {err}");
            }
            Err(_) => {
                tracing::warn!(
                    timeout_secs = SIGSUM_ANCHOR_TIMEOUT.as_secs(),
                    "timed out anchoring checkpoint to Sigsum log"
                );
            }
        }
    }

    /// Spawns a background loop calling [`Self::run_once`] every
    /// `interval`, until `shutdown` is notified.
    pub fn spawn(self: Arc<Self>, shutdown: Arc<Notify>, interval: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            // Created once and pinned across iterations: `notify_waiters`
            // only wakes already-registered waiters, so a fresh
            // `notified()` per loop iteration would miss a shutdown signal
            // that fires while a tick is running, leaving this task (and
            // `Mint::stop`, which joins it) hanging forever.
            let shutdown_signal = shutdown.notified();
            tokio::pin!(shutdown_signal);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {
                        if let Err(err) = self.run_once().await {
                            tracing::error!("transparency log publisher tick failed: {err}");
                        }
                    }
                    _ = &mut shutdown_signal => {
                        tracing::info!("transparency log publisher shutting down");
                        break;
                    }
                }
            }
        })
    }

    /// The latest signed checkpoint, as a full C2SP signed note, if any
    /// entries have ever been logged.
    pub async fn latest_checkpoint(&self) -> Result<Option<String>, Error> {
        match read_u64(&self.kv_db, KV_LATEST_CHECKPOINT_SIZE).await? {
            Some(size) => self.checkpoint_at(size).await,
            None => Ok(None),
        }
    }

    /// The signed checkpoint for exactly `tree_size`, if one was ever
    /// published at that size.
    pub async fn checkpoint_at(&self, tree_size: u64) -> Result<Option<String>, Error> {
        let key = checkpoint_key(tree_size);
        Ok(self
            .kv_db
            .kv_read(KV_NAMESPACE, "checkpoint", &key)
            .await?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// The ascii Sigsum proof-of-logging for the checkpoint at exactly
    /// `tree_size`, if that checkpoint was successfully anchored to a
    /// Sigsum log (see the `sigsum-anchor` feature). External anchors are
    /// best-effort, so a checkpoint can exist without one.
    pub async fn sigsum_proof_at(&self, tree_size: u64) -> Result<Option<String>, Error> {
        Ok(self
            .kv_db
            .kv_read(KV_NAMESPACE, "anchor", &sigsum_proof_key(tree_size))
            .await?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// The ascii Sigsum proof for the latest published checkpoint, if any.
    pub async fn latest_sigsum_proof(&self) -> Result<Option<String>, Error> {
        match read_u64(&self.kv_db, KV_LATEST_CHECKPOINT_SIZE).await? {
            Some(size) => self.sigsum_proof_at(size).await,
            None => Ok(None),
        }
    }

    /// Log entries with zero-based leaf index in `[start, end)`, ordered by
    /// index, for the `/v1/audit/entries` endpoint.
    ///
    /// NUT-XX defines `seq` as a zero-based leaf index (`0 <= seq <
    /// tree_size`), matching RFC 6962 / Sigsum / CT convention — which is
    /// exactly what the appender assigns as `leaf_index`, so no numbering
    /// translation is needed anywhere.
    pub async fn entries(
        &self,
        start: u64,
        end: u64,
    ) -> Result<Vec<cdk_common::database::mint::MintEventLogEntry>, Error> {
        Ok(self.log_db.get_event_log_range(start, end).await?)
    }

    /// Builds an RFC 6962 inclusion proof for the zero-based leaf `index`
    /// under a tree of `tree_size` leaves.
    ///
    /// Requires loading every leaf hash in the tree — proof generation,
    /// unlike incremental root advancement, needs the actual leaves. This
    /// is the scaling limitation flagged in the ADR as follow-up work for
    /// high-volume mints.
    pub async fn inclusion_proof(
        &self,
        index: u64,
        tree_size: u64,
    ) -> Result<(Hash, Vec<Hash>), Error> {
        if index >= tree_size {
            return Err(Error::Internal);
        }
        let leaves = self.leaf_hashes_up_to(tree_size).await?;
        let leaf = leaves[index as usize];
        let proof = inclusion_proof(&leaves, index).map_err(|e| Error::Custom(e.to_string()))?;
        Ok((leaf, proof))
    }

    /// Builds an RFC 6962 consistency proof between tree sizes `first`
    /// and `second`.
    pub async fn consistency_proof(&self, first: u64, second: u64) -> Result<Vec<Hash>, Error> {
        let leaves = self.leaf_hashes_up_to(second).await?;
        consistency_proof(first, &leaves).map_err(|e| Error::Custom(e.to_string()))
    }

    async fn leaf_hashes_up_to(&self, tree_size: u64) -> Result<Vec<Hash>, Error> {
        let rows = self.log_db.get_event_log_range(0, tree_size).await?;
        if rows.len() as u64 != tree_size {
            return Err(Error::Custom(format!(
                "expected {tree_size} contiguous log entries, found {}",
                rows.len()
            )));
        }
        Ok(rows.into_iter().map(|row| row.leaf_hash).collect())
    }
}

fn checkpoint_key(tree_size: u64) -> String {
    format!("size_{tree_size:020}")
}

fn sigsum_proof_key(tree_size: u64) -> String {
    format!("sigsum_{tree_size:020}")
}

/// Per-witness "largest size this witness has cosigned for us" KV key.
/// Hashed because witness names (schema-less URLs) can contain characters
/// outside the KV key alphabet.
fn witness_ack_key(witness_name: &str) -> String {
    use bitcoin::hashes::{sha256, Hash as BitcoinHash};
    hex::encode(sha256::Hash::hash(witness_name.as_bytes()).to_byte_array())
}

async fn read_kv_u64(
    db: &DynMintDatabase,
    secondary: &str,
    key: &str,
) -> Result<Option<u64>, Error> {
    let bytes = db.kv_read(KV_NAMESPACE, secondary, key).await?;
    Ok(match bytes {
        Some(bytes) => {
            let arr: [u8; 8] = bytes
                .try_into()
                .map_err(|_| Error::Custom(format!("stored `{secondary}/{key}` was not 8 bytes")))?;
            Some(u64::from_be_bytes(arr))
        }
        None => None,
    })
}

async fn write_kv_u64(
    db: &DynMintDatabase,
    secondary: &str,
    key: &str,
    value: u64,
) -> Result<(), Error> {
    let mut tx = db.begin_transaction().await?;
    tx.kv_write(KV_NAMESPACE, secondary, key, &value.to_be_bytes())
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Overwrites the stored note for `tree_size` — used to re-persist a
/// checkpoint after witness cosignature lines were appended to it. The
/// checkpoint body (and the mint's own signature) never change; only
/// signature lines accumulate.
async fn persist_checkpoint_note(
    db: &DynMintDatabase,
    tree_size: u64,
    checkpoint: &SignedCheckpoint,
) -> Result<(), Error> {
    let mut tx = db.begin_transaction().await?;
    tx.kv_write(
        KV_NAMESPACE,
        "checkpoint",
        &checkpoint_key(tree_size),
        checkpoint.to_note().as_bytes(),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Stores the ascii Sigsum proof for the checkpoint at `tree_size`.
/// Written after (and separately from) the checkpoint's own atomic
/// persist — anchoring is best-effort and asynchronous relative to
/// checkpoint publication (ADR §7.1 item 3).
#[cfg(feature = "sigsum-anchor")]
async fn persist_sigsum_proof(
    db: &DynMintDatabase,
    tree_size: u64,
    proof_ascii: &str,
) -> Result<(), Error> {
    let mut tx = db.begin_transaction().await?;
    tx.kv_write(
        KV_NAMESPACE,
        "anchor",
        &sigsum_proof_key(tree_size),
        proof_ascii.as_bytes(),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn load_or_create_signing_key(db: &DynMintDatabase) -> Result<SigningKey, Error> {
    if let Some(bytes) = db.kv_read(KV_NAMESPACE, "keys", KV_SIGNING_KEY).await? {
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Custom("stored transparency log key was not 32 bytes".into()))?;
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

async fn read_u64(db: &DynMintDatabase, key: &str) -> Result<Option<u64>, Error> {
    let bytes = db.kv_read(KV_NAMESPACE, "state", key).await?;
    Ok(match bytes {
        Some(bytes) => {
            let arr: [u8; 8] = bytes
                .try_into()
                .map_err(|_| Error::Custom(format!("stored `{key}` was not 8 bytes")))?;
            Some(u64::from_be_bytes(arr))
        }
        None => None,
    })
}

async fn read_tree_state(db: &DynMintDatabase) -> Result<TreeHead, Error> {
    let bytes = db.kv_read(KV_NAMESPACE, "state", KV_TREE_STATE).await?;
    let Some(bytes) = bytes else {
        return Ok(TreeHead::empty());
    };
    if bytes.len() < 8 || (bytes.len() - 8) % 32 != 0 {
        return Err(Error::Custom("corrupt persisted tree state".into()));
    }
    let size = u64::from_be_bytes(bytes[..8].try_into().expect("checked len"));
    let peaks = bytes[8..]
        .chunks_exact(32)
        .map(|chunk| chunk.try_into().expect("chunks_exact(32)"))
        .collect();
    Ok(TreeHead::from_parts(size, peaks))
}

/// Persists everything one publishing tick produced — the advanced tree
/// state, the signed checkpoint note, and the latest-checkpoint pointer —
/// in a single KV transaction, so a crash can never persist one without
/// the others.
async fn persist_tick(
    db: &DynMintDatabase,
    tree: &TreeHead,
    checkpoint: &SignedCheckpoint,
) -> Result<(), Error> {
    let mut tree_bytes = Vec::with_capacity(8 + tree.peaks().len() * 32);
    tree_bytes.extend_from_slice(&tree.size.to_be_bytes());
    for peak in tree.peaks() {
        tree_bytes.extend_from_slice(peak);
    }

    let mut tx = db.begin_transaction().await?;
    tx.kv_write(KV_NAMESPACE, "state", KV_TREE_STATE, &tree_bytes)
        .await?;
    tx.kv_write(
        KV_NAMESPACE,
        "checkpoint",
        &checkpoint_key(tree.size),
        checkpoint.to_note().as_bytes(),
    )
    .await?;
    tx.kv_write(
        KV_NAMESPACE,
        "state",
        KV_LATEST_CHECKPOINT_SIZE,
        &tree.size.to_be_bytes(),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;

    use bitcoin::bip32::DerivationPath;
    use cdk_common::common::IssuerVersion;
    use cdk_common::database::mint::KeysDatabase;
    use cdk_common::mint::MintKeySetInfo;
    use cdk_common::{Amount, CurrencyUnit, Id, Proof, SecretKey};
    use cdk_sqlite::mint::MintSqliteDatabase;
    use cdk_tlog::checkpoint::verify_checkpoint_signature;
    use cdk_tlog::merkle::{root_from_leaves, verify_consistency, verify_inclusion};

    use super::*;

    /// Creates a fresh in-memory mint DB with one active keyset, returning
    /// both the erased handles a [`TransparencyLogService`] needs and the
    /// keyset id, so callers can add more proofs later without needing
    /// [`KeysDatabase`] (which isn't part of the erased [`DynMintDatabase`]
    /// trait object).
    async fn setup() -> (DynMintDatabase, DynTransparencyLogDatabase, Id) {
        let sql_db = MintSqliteDatabase::new(":memory:").await.unwrap();

        let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
        let keyset_info = MintKeySetInfo {
            id: keyset_id,
            unit: CurrencyUnit::Sat,
            active: false,
            valid_from: 0,
            final_expiry: None,
            derivation_path: DerivationPath::from_str("m/0'/0'/0'").unwrap(),
            derivation_path_index: Some(0),
            input_fee_ppk: 0,
            amounts: vec![1, 2, 4, 8],
            issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
        };
        let mut tx = KeysDatabase::begin_transaction(&sql_db).await.unwrap();
        tx.add_keyset_info(keyset_info).await.unwrap();
        tx.set_active_keyset(CurrencyUnit::Sat, keyset_id)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let sql_db = Arc::new(sql_db);
        let dyn_db: DynMintDatabase = sql_db.clone();
        let log_db: DynTransparencyLogDatabase = sql_db;
        (dyn_db, log_db, keyset_id)
    }

    /// Adds `count` proofs against `keyset_id` and immediately moves each
    /// to `Pending`, so `update_proofs_state` (one of the six real
    /// mutation sites wired to the event log) actually fires.
    async fn add_some_proofs(db: &DynMintDatabase, keyset_id: Id, count: usize) {
        for _ in 0..count {
            let proof = Proof {
                amount: Amount::from(1),
                keyset_id,
                secret: cdk_common::secret::Secret::generate(),
                c: SecretKey::generate().public_key(),
                witness: None,
                dleq: None,
                p2pk_e: None,
            };
            let y = proof.y().unwrap();

            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(
                vec![proof],
                None,
                &cdk_common::mint::Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();

            let mut tx = db.begin_transaction().await.unwrap();
            let mut proofs = tx.get_proofs(&[y]).await.unwrap();
            tx.update_proofs_state(&mut proofs, cdk_common::State::Pending)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    }

    #[tokio::test]
    async fn run_once_advances_tree_and_signs_a_verifiable_checkpoint() {
        let (dyn_db, log_db, keyset_id) = setup().await;
        add_some_proofs(&dyn_db, keyset_id, 5).await;

        let service = TransparencyLogService::load_or_create(
            log_db,
            dyn_db,
            "mint.example.com/transparency-log".to_string(),
        )
        .await
        .unwrap();

        let signed = service
            .run_once()
            .await
            .unwrap()
            .expect("a checkpoint should be published: the log is non-empty");

        // At least the keyset activation plus one Update per proof.
        assert!(
            signed.checkpoint.size >= 6,
            "size={}",
            signed.checkpoint.size
        );
        assert_eq!(
            signed.checkpoint.origin,
            "mint.example.com/transparency-log"
        );

        // The checkpoint's own signature must verify against the
        // service's public key.
        let pubkey_bytes: [u8; 32] = base64::engine::general_purpose::STANDARD
            .decode(service.public_key_base64())
            .unwrap()
            .try_into()
            .unwrap();
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&pubkey_bytes).unwrap();
        verify_checkpoint_signature(
            &signed.checkpoint,
            &signed.checkpoint.origin,
            &verifying_key,
            &signed.signatures[0],
        )
        .expect("checkpoint signature must verify");

        // The root must be independently reproducible from the raw log
        // entries — this is the actual "playback + verify" property the
        // whole feature exists for.
        let entries = service.entries(0, signed.checkpoint.size).await.unwrap();
        assert_eq!(entries.len() as u64, signed.checkpoint.size);
        let leaves: Vec<_> = entries.iter().map(|e| e.leaf_hash).collect();
        assert_eq!(root_from_leaves(&leaves), signed.checkpoint.root_hash);

        // A second tick with no new events must not republish.
        assert!(service.run_once().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn inclusion_and_consistency_proofs_verify_against_the_service() {
        let (dyn_db, log_db, keyset_id) = setup().await;
        add_some_proofs(&dyn_db, keyset_id, 3).await;

        let service = TransparencyLogService::load_or_create(
            log_db,
            dyn_db.clone(),
            "mint.example.com/transparency-log".to_string(),
        )
        .await
        .unwrap();
        let first = service.run_once().await.unwrap().unwrap();

        // Log a few more events, then take a second checkpoint.
        add_some_proofs(&dyn_db, keyset_id, 2).await;
        let second = service.run_once().await.unwrap().unwrap();
        assert!(second.checkpoint.size > first.checkpoint.size);

        // Inclusion: the very first logged entry (index=0) under the final tree.
        let (leaf, proof) = service
            .inclusion_proof(0, second.checkpoint.size)
            .await
            .unwrap();
        assert!(verify_inclusion(
            leaf,
            0,
            second.checkpoint.size,
            &proof,
            second.checkpoint.root_hash
        )
        .unwrap());

        // Consistency: the first checkpoint's tree must be a genuine
        // prefix of the second's.
        let proof = service
            .consistency_proof(first.checkpoint.size, second.checkpoint.size)
            .await
            .unwrap();
        assert!(verify_consistency(
            first.checkpoint.size,
            first.checkpoint.root_hash,
            second.checkpoint.size,
            second.checkpoint.root_hash,
            &proof,
        )
        .unwrap());
    }
}
