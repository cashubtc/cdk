//! Main Signatory implementation
//!
//! It is named db_signatory because it uses a database to persist state. The
//! database is a persistence layer only: it is read once on boot to hydrate the
//! in-memory keysets and written only when rotating keys. Every other operation
//! is served from and mutates in-memory state. See ADR-0003.
//!
//! Boot is strict: `new` attempts the initial keyset load from the database
//! once and bubbles up any error, so a failed load fails construction rather
//! than leaving a signatory without keys. On success the returned signatory is
//! loaded and serving.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::secp256k1::{self, Secp256k1};
use cdk_common::database::MintKeyDatabaseTransaction;
use cdk_common::dhke::{sign_message, verify_message};
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::{BlindSignature, BlindedMessage, CurrencyUnit, Id, MintKeySet, Proof};
use cdk_common::{database, Error, PublicKey};
use tokio::sync::{watch, Mutex};
use tracing::instrument;

use crate::common::{
    check_unit_string_collision, create_new_keyset, derivation_path_from_unit, init_keysets,
};
use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets};

/// Immutable in-memory view of the keysets, swapped atomically on every change.
///
/// Readers load it lock-free; the periodic refresh and local rotations build a
/// fresh one off to the side and store it in a single atomic swap, so signing
/// never waits on a reload.
#[derive(Default)]
struct KeysetSnapshot {
    /// All keysets keyed by id, with their derived keys.
    by_id: HashMap<Id, (MintKeySetInfo, MintKeySet)>,
    /// Active keyset id per unit.
    active_by_unit: HashMap<CurrencyUnit, Id>,
    /// Storage keyset epoch this snapshot was built from; `None` before the
    /// first load. The refresh compares it against the storage token to decide
    /// whether a reload is needed.
    epoch: Option<u64>,
}

/// In-memory Signatory
///
/// This is the default signatory implementation for the mint.
///
/// The private keys and the all key-related data is stored in memory, in the same process, but it
/// is not accessible from the outside.
#[allow(missing_debug_implementations)]
pub struct DbSignatory {
    /// Lock-free in-memory keyset view. Reads load it without blocking;
    /// reloads and rotations replace it wholesale. The database is the global
    /// coordination point (advisory lock and epoch token); this only mirrors
    /// it.
    keysets: ArcSwap<KeysetSnapshot>,
    /// Serializes local keyset rotations. The standalone signatory gRPC server
    /// calls rotate_keyset directly (no embedded single-runner), so two
    /// concurrent local rotations of the same unit could otherwise open nested
    /// transactions on a single-connection backend. Cross-process
    /// serialization is the database's job.
    rotation_lock: Mutex<()>,
    localstore: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
    secp_ctx: Secp256k1<secp256k1::All>,
    custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    /// Units to initialize on boot, as `init_keysets` expects them.
    supported_units: HashMap<CurrencyUnit, (u64, Vec<u64>)>,
    xpriv: Xpriv,
    xpub: PublicKey,
    /// Latest keyset snapshot, published on every reload (initial load and each
    /// rotation).
    keyset_updates: watch::Sender<SignatoryKeysets>,
}

impl DbSignatory {
    /// Creates a new signatory, loading its keysets from the database.
    ///
    /// The load is attempted once and any error is bubbled up: a failed load
    /// fails construction rather than returning a signatory without keys. On
    /// success the returned signatory is loaded and serving.
    ///
    /// # Panics
    ///
    /// Panics if the seed produces an invalid master key (should never happen with valid entropy).
    pub async fn new(
        localstore: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
        seed: &[u8],
        supported_units: HashMap<CurrencyUnit, (u64, Vec<u64>)>,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<Self, Error> {
        let secp_ctx = Secp256k1::new();
        let xpriv = Xpriv::new_master(bitcoin::Network::Bitcoin, seed).expect("RNG busted");

        let xpub: PublicKey = xpriv.to_keypair(&secp_ctx).public_key().into();
        let (keyset_updates, _) = watch::channel(SignatoryKeysets {
            pubkey: xpub,
            keysets: vec![],
        });

        let signatory = Self {
            keysets: ArcSwap::from_pointee(KeysetSnapshot::default()),
            rotation_lock: Default::default(),
            localstore,
            custom_paths,
            supported_units,
            xpub,
            secp_ctx,
            xpriv,
            keyset_updates,
        };

        signatory.boot_load().await?;

        Ok(signatory)
    }

    /// Periodically reload keysets from the shared database, so a rotation
    /// performed by any instance is picked up without a restart.
    ///
    /// This is off by default: a single signatory process owns its database and
    /// needs no polling. Enabling it (passing an interval) is what lets several
    /// signatory processes run against one shared database (active/active): each
    /// process picks up peers' rotations within one interval. Rotation itself is
    /// always safe to share, its derivation index is allocated authoritatively
    /// in the database; this reload only closes the gap where a peer's change
    /// would otherwise be invisible until the next boot.
    ///
    /// `interval` is `None` (or zero) to disable: no task is spawned and nothing
    /// polls. A few seconds is a reasonable cadence. The refresh is aligned to a
    /// shared wall-clock epoch (Unix time in
    /// milliseconds floored to the interval) so every process reloads at about
    /// the same moment, keeping the fleet's view close to consistent without any
    /// inter-node messaging. The task holds a `Weak`, so it stops on its own once
    /// the signatory is dropped.
    pub fn spawn_keyset_refresh(self: &Arc<Self>, interval: Option<Duration>) {
        let interval_ms = match interval.map(|d| d.as_millis() as u64) {
            Some(ms) if ms > 0 => ms,
            _ => return,
        };

        let weak = Arc::downgrade(self);

        tokio::spawn(async move {
            loop {
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let next = (now_ms / interval_ms + 1) * interval_ms;
                tokio::time::sleep(Duration::from_millis(next.saturating_sub(now_ms))).await;

                let Some(signatory) = weak.upgrade() else {
                    break;
                };
                // No rotation lock: the reload loads authoritative database
                // state, and `load_keys_from_db` gates its swap on the database
                // epoch, so it cannot publish out of order with a rotation's
                // reload. It never blocks signing. It is also cheap when nothing
                // changed (epoch gate, no row fetch, no key derivation), so a
                // quiet refresh costs only a metadata read.
                if let Err(err) = signatory.load_keys_from_db().await {
                    tracing::warn!("periodic keyset reload from database failed: {err}");
                }
            }
        });
    }

    /// Load keysets from the database into memory.
    ///
    /// This runs the boot-time database reactivation (`init_keysets`) and then
    /// hydrates memory from the database.
    async fn boot_load(&self) -> Result<(), Error> {
        init_keysets(
            self.xpriv,
            &self.secp_ctx,
            &self.localstore,
            &self.supported_units,
        )
        .await?;
        self.load_keys_from_db().await?;
        Ok(())
    }

    /// Hydrate the in-memory keysets from the database and swap them in.
    ///
    /// A cheap, lock-free epoch pre-check skips the whole reload when nothing
    /// changed: one atomic read, no lock, no row fetch, no derivation. Only when
    /// the epoch moved does it open a keyset transaction (which takes the global
    /// keyset advisory lock) and reload a consistent snapshot through it, so a
    /// quiet periodic tick stays a single query. The rebuild reuses the keys
    /// already derived in the current snapshot, deriving only new keysets, and
    /// swaps the result in atomically so readers never block.
    async fn load_keys_from_db(&self) -> Result<(), Error> {
        // Steady-state gate: a single atomic epoch read, lock-free (one
        // statement is consistent on its own). Skip when it matches the loaded
        // snapshot.
        let epoch = self.localstore.keysets_epoch().await?;
        if self.keysets.load().epoch == Some(epoch) {
            return Ok(());
        }

        // Something changed: reload under the global keyset lock so the
        // multi-statement read is a consistent snapshot.
        let mut tx = self.localstore.begin_transaction().await?;
        self.reload_from_tx(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Reload the in-memory keysets from committed state read through an open
    /// keyset transaction.
    ///
    /// The transaction holds the global keyset advisory lock (taken in
    /// `begin_transaction`), so no other keyset transaction can commit between
    /// the reads: they are a consistent snapshot and need no epoch bracketing.
    /// `rotate_keyset` calls this on its own write transaction so the collision
    /// check and default amounts see peers' committed rotations;
    /// `load_keys_from_db` calls it on a dedicated read transaction.
    async fn reload_from_tx(
        &self,
        tx: &mut (dyn MintKeyDatabaseTransaction<'_, database::Error> + Send + Sync),
    ) -> Result<(), Error> {
        let epoch = tx.keysets_epoch().await?;
        if self.keysets.load().epoch == Some(epoch) {
            return Ok(());
        }

        let db_active_keysets = tx.get_active_keysets().await?;
        let db_infos = tx.get_keyset_infos().await?;

        self.publish_snapshot(epoch, db_active_keysets, db_infos);
        Ok(())
    }

    /// Build a keyset snapshot for `epoch` and publish it, reusing keys already
    /// derived in the current snapshot (a memory copy, no crypto); only a
    /// genuinely new keyset needs derivation.
    ///
    /// Publishing is conditional on the database epoch rather than a process
    /// lock. `keysets_epoch` is a counter bumped in every keyset-writing
    /// transaction, so it only ever increases and totally orders changes. Two
    /// reloads can race here (a periodic refresh against a rotation's
    /// post-commit reload, or an under-lock reload against the following
    /// post-commit reload); the compare-and-swap lets the newer database view
    /// win regardless of arrival order, so an older snapshot can never overwrite
    /// a newer one. Signing only reads the pointer, so it stays lock-free.
    fn publish_snapshot(
        &self,
        epoch: u64,
        db_active_keysets: HashMap<CurrencyUnit, Id>,
        db_infos: Vec<MintKeySetInfo>,
    ) {
        let current = self.keysets.load();
        let mut by_id = HashMap::with_capacity(db_infos.len());
        let mut active_by_unit = HashMap::new();
        for mut info in db_infos {
            let id = info.id;
            info.active = db_active_keysets.get(&info.unit) == Some(&info.id);
            if info.active {
                active_by_unit.insert(info.unit.clone(), id);
            }
            let keyset = match current.by_id.get(&id) {
                Some((_, keyset)) => keyset.clone(),
                None => self.generate_keyset(&info),
            };
            by_id.insert(id, (info, keyset));
        }

        let snapshot = Arc::new(KeysetSnapshot {
            by_id,
            active_by_unit,
            epoch: Some(epoch),
        });

        loop {
            let current = self.keysets.load();
            if current.epoch.is_some_and(|cur| epoch <= cur) {
                // A concurrent reload already published an equal-or-newer view.
                return;
            }
            let prev = self.keysets.compare_and_swap(&current, snapshot.clone());
            if Arc::ptr_eq(&prev, &current) {
                break;
            }
            // Lost the swap: another reload replaced the pointer between our load
            // and CAS. Retry to re-check its epoch against ours.
        }
        self.publish_latest();
    }

    /// Publish the latest stored keyset snapshot to watch subscribers.
    ///
    /// Reads the current `ArcSwap` snapshot inside the watch send closure, which
    /// holds the channel's send lock, so concurrent reloads publish in the
    /// pointer's epoch order and the subscriber view never regresses. This
    /// mirrors the compare-and-swap that orders the stored snapshot; without it
    /// two swap winners could still send stale-then-fresh out of order.
    fn publish_latest(&self) {
        self.keyset_updates.send_modify(|out| {
            let latest = self.keysets.load();
            out.pubkey = self.xpub;
            out.keysets = latest.by_id.values().map(|k| k.into()).collect();
        });
    }

    fn generate_keyset(&self, keyset_info: &MintKeySetInfo) -> MintKeySet {
        MintKeySet::generate_from_xpriv(
            &self.secp_ctx,
            self.xpriv,
            &keyset_info.amounts,
            keyset_info.unit.clone(),
            keyset_info.derivation_path.clone(),
            keyset_info.input_fee_ppk,
            keyset_info.final_expiry,
            keyset_info.id.get_version(),
        )
    }

    /// Snapshot the current keysets from memory (lock-free).
    fn keysets_snapshot(&self) -> SignatoryKeysets {
        SignatoryKeysets {
            pubkey: self.xpub,
            keysets: self
                .keysets
                .load()
                .by_id
                .values()
                .map(|k| k.into())
                .collect(),
        }
    }
}

#[async_trait::async_trait]
impl Signatory for DbSignatory {
    fn name(&self) -> String {
        format!("Signatory {}", env!("CARGO_PKG_VERSION"))
    }

    #[instrument(skip_all)]
    async fn blind_sign(
        &self,
        blinded_messages: Vec<BlindedMessage>,
    ) -> Result<Vec<BlindSignature>, Error> {
        let keysets = self.keysets.load();

        blinded_messages
            .into_iter()
            .map(|blinded_message| {
                let BlindedMessage {
                    amount,
                    blinded_secret,
                    keyset_id,
                    ..
                } = blinded_message;

                let (info, key) = keysets.by_id.get(&keyset_id).ok_or(Error::UnknownKeySet)?;
                if !info.active {
                    return Err(Error::InactiveKeyset);
                }
                if info.is_expired() {
                    return Err(Error::ExpiredKeyset);
                }

                let key_pair = key.keys.get(&amount).ok_or(Error::UnknownKeySet)?;
                let c = sign_message(&key_pair.secret_key, &blinded_secret)?;

                let blinded_signature = BlindSignature::new(
                    amount,
                    c,
                    keyset_id,
                    &blinded_message.blinded_secret,
                    &key_pair.secret_key,
                )?;

                Ok(blinded_signature)
            })
            .collect::<Result<Vec<_>, _>>()
    }

    #[tracing::instrument(skip_all)]
    async fn verify_proofs(&self, proofs: Vec<Proof>) -> Result<(), Error> {
        let keysets = self.keysets.load();

        proofs.into_iter().try_for_each(|proof| {
            let (_, key) = keysets
                .by_id
                .get(&proof.keyset_id)
                .ok_or(Error::UnknownKeySet)?;
            let key_pair = key.keys.get(&proof.amount).ok_or(Error::UnknownKeySet)?;
            verify_message(&key_pair.secret_key, proof.c, proof.secret.as_bytes())?;
            Ok(())
        })
    }

    #[tracing::instrument(skip_all)]
    async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
        Ok(self.keysets_snapshot())
    }

    #[tracing::instrument(skip_all)]
    async fn subscribe_keysets(&self) -> Result<watch::Receiver<SignatoryKeysets>, Error> {
        Ok(self.keyset_updates.subscribe())
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[tracing::instrument(skip(self))]
    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
        // Serialize local rotations. The standalone signatory gRPC server
        // invokes this directly (no embedded single-runner), so without this two
        // concurrent local rotations could open nested transactions on a
        // single-connection backend. Held for the whole method. Cross-process
        // rotations are serialized by the global keyset lock in the database.
        let _rotation = self.rotation_lock.lock().await;

        // Persist the rotation. This is the only path that writes to the
        // database. Opening the transaction takes the global keyset advisory
        // lock, held to commit, so all keyset transactions serialize across
        // processes: index allocation is authoritative and two rotations cannot
        // interleave.
        let mut tx = self.localstore.begin_transaction().await?;
        let path_index = tx.next_derivation_index(&args.unit).await?;

        // Reload in-memory keysets from committed state through this
        // transaction, under the global lock just taken. Both the default
        // amounts below and the collision check further down then see peers'
        // committed rotations rather than a possibly-stale in-memory snapshot.
        self.reload_from_tx(&mut *tx).await?;

        // Default amounts come from the in-memory active keyset and are only
        // used when the caller does not specify any. The authoritative
        // derivation index is allocated from the database above, not memory, so
        // concurrent rotations across instances cannot pick the same index.
        let default_amounts = {
            let keysets = self.keysets.load();
            keysets
                .active_by_unit
                .get(&args.unit)
                .and_then(|id| keysets.by_id.get(id))
                .map(|(info, _)| info.amounts.clone())
                .unwrap_or_default()
        };

        let derivation_path = match self.custom_paths.get(&args.unit) {
            Some(path) => path.clone(),
            None => derivation_path_from_unit(args.unit.clone(), path_index)
                .ok_or(Error::UnsupportedUnit)?,
        };

        let amounts = if args.amounts.is_empty() {
            if default_amounts.is_empty() {
                return Err(Error::Custom("Amounts cannot be empty".to_string()));
            }
            default_amounts
        } else {
            args.amounts
        };

        let (keyset, info) = create_new_keyset(
            &self.secp_ctx,
            self.xpriv,
            derivation_path,
            Some(path_index),
            args.unit.clone(),
            &amounts,
            args.input_fee_ppk,
            args.final_expiry.map(|expiry| expiry.to_u64()),
            args.keyset_id_type,
        );

        let keysets = self.keysets_snapshot();
        check_unit_string_collision(keysets.keysets, &info)?;

        let id = info.id;

        tx.add_keyset_info(info.clone()).await?;
        tx.set_active_keyset(args.unit.clone(), id).await?;
        tx.commit().await?;

        let mut info = info;
        info.active = true;
        let signatory_keyset: SignatoryKeySet = (&(info, keyset)).into();

        // Refresh from the full database state rather than patching memory by
        // hand. This picks up any concurrent peer rotation too, and records the
        // keyset epoch we loaded, so the periodic refresh does not reload
        // again on its next tick. (Recording the epoch from a bare post-commit
        // read would be unsafe: it could already include a peer change this
        // instance has not loaded.)
        self.load_keys_from_db().await?;

        Ok(signatory_keyset)
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use bitcoin::key::Secp256k1;
    use bitcoin::Network;
    use cdk_common::database::MintKeysDatabase;
    use cdk_common::nuts::SecretKey;
    use cdk_common::util::{hex, unix_time};
    use cdk_common::{Amount, MintKeySet, PublicKey};

    use super::*;
    use crate::signatory::KeysetExpiry;

    #[tokio::test]
    async fn keysets_epoch_moves_only_on_change() {
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let signatory = DbSignatory::new(
            store.clone(),
            b"test-seed-for-version",
            Default::default(),
            Default::default(),
        )
        .await
        .expect("DbSignatory::new");

        let rotate = |unit| RotateKeyArguments {
            unit,
            amounts: vec![1, 2, 4, 8],
            input_fee_ppk: 0,
            keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
            final_expiry: None,
        };

        let before = store.keysets_epoch().await.expect("epoch");
        assert_eq!(
            before,
            store.keysets_epoch().await.expect("epoch"),
            "epoch is stable when nothing changed"
        );

        // First keyset (becomes active), then a second (the first goes inactive).
        let first = signatory
            .rotate_keyset(rotate(CurrencyUnit::Sat))
            .await
            .expect("rotate_keyset");
        let after_insert = store.keysets_epoch().await.expect("epoch");
        assert_ne!(
            before, after_insert,
            "epoch changes after a rotation adds a keyset"
        );

        signatory
            .rotate_keyset(rotate(CurrencyUnit::Sat))
            .await
            .expect("rotate_keyset");
        let two_keysets = store.keysets_epoch().await.expect("epoch");

        // Reactivate the first keyset: a pure active-pointer change, no insert.
        let mut tx = MintKeysDatabase::begin_transaction(&*store)
            .await
            .expect("begin tx");
        tx.set_active_keyset(CurrencyUnit::Sat, first.id)
            .await
            .expect("set active");
        tx.commit().await.expect("commit");

        assert_ne!(
            two_keysets,
            store.keysets_epoch().await.expect("epoch"),
            "epoch changes on an active-pointer reassignment with no insert"
        );
    }

    #[tokio::test]
    async fn rotation_reloads_peer_keyset_under_lock() {
        // Two signatories sharing one database, the shape ADR-0004 enables.
        // Neither has the periodic refresh enabled, so the only way instance A
        // learns of instance B's rotation before a restart is the under-lock
        // reload inside its own rotate_keyset.
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let seed = b"test-seed-cross-instance-reload";
        let instance_a =
            DbSignatory::new(store.clone(), seed, Default::default(), Default::default())
                .await
                .expect("DbSignatory::new a");
        let instance_b =
            DbSignatory::new(store.clone(), seed, Default::default(), Default::default())
                .await
                .expect("DbSignatory::new b");

        let rotate = |unit| RotateKeyArguments {
            unit,
            amounts: vec![1, 2, 4, 8],
            input_fee_ppk: 0,
            keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
            final_expiry: None,
        };

        // B rotates Sat and commits. A's in-memory view is untouched.
        let b_keyset = instance_b
            .rotate_keyset(rotate(CurrencyUnit::Sat))
            .await
            .expect("rotate_keyset b");

        let a_before = instance_a.keysets().await.expect("keysets a");
        assert!(
            !a_before.keysets.iter().any(|k| k.id == b_keyset.id),
            "instance A should not see B's keyset before it reloads"
        );

        // A rotates a different, non-colliding unit. The under-lock reload must
        // pull B's committed keyset into A's snapshot.
        instance_a
            .rotate_keyset(rotate(CurrencyUnit::Usd))
            .await
            .expect("rotate_keyset a");

        let a_after = instance_a.keysets().await.expect("keysets a after");
        assert!(
            a_after.keysets.iter().any(|k| k.id == b_keyset.id),
            "instance A's under-lock reload did not pick up B's committed keyset"
        );
    }

    #[tokio::test]
    async fn empty_amount_rotation_adopts_peer_denominations() {
        // A rotation with empty amounts must reuse the currently active
        // keyset's denomination set. In an active/active fleet that active
        // keyset may have been rotated by a peer, so the amounts must come from
        // committed state read under the global keyset lock, not from this
        // instance's possibly-stale in-memory snapshot. Without the under-lock
        // reload in rotate_keyset this test reuses A's older [1, 2, 4, 8].
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let seed = b"test-seed-empty-amount-peer-denoms";
        let instance_a =
            DbSignatory::new(store.clone(), seed, Default::default(), Default::default())
                .await
                .expect("DbSignatory::new a");
        let instance_b =
            DbSignatory::new(store.clone(), seed, Default::default(), Default::default())
                .await
                .expect("DbSignatory::new b");

        let rotate = |amounts: Vec<u64>| RotateKeyArguments {
            unit: CurrencyUnit::Sat,
            amounts,
            input_fee_ppk: 0,
            keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
            final_expiry: None,
        };

        // A establishes an active Sat keyset, then B rotates it to a distinct
        // denomination set. A never reloads (no periodic refresh), so its
        // snapshot still shows the [1, 2, 4, 8] keyset as active.
        instance_a
            .rotate_keyset(rotate(vec![1, 2, 4, 8]))
            .await
            .expect("rotate_keyset a");
        instance_b
            .rotate_keyset(rotate(vec![1, 2, 4, 8, 16, 32]))
            .await
            .expect("rotate_keyset b");

        // A rotates with empty amounts: it must adopt B's committed [1, 2, 4,
        // 8, 16, 32], not its stale local [1, 2, 4, 8].
        let rotated = instance_a
            .rotate_keyset(rotate(vec![]))
            .await
            .expect("rotate_keyset a empty");

        assert_eq!(
            rotated.amounts,
            vec![1, 2, 4, 8, 16, 32],
            "empty-amount rotation must reuse the peer's committed denomination set"
        );
    }

    #[tokio::test]
    async fn blind_sign_rejects_expired_keyset() {
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let signatory = DbSignatory::new(
            store.clone(),
            b"test-seed-for-unit-tests",
            Default::default(),
            Default::default(),
        )
        .await
        .expect("DbSignatory::new");

        let expired_keyset = signatory
            .rotate_keyset(RotateKeyArguments {
                unit: CurrencyUnit::Sat,
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: 0,
                keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                final_expiry: Some(KeysetExpiry::new(unix_time() + 60).expect("future expiry")),
            })
            .await
            .expect("rotate_keyset");

        // Simulate a keyset that expired after it was created.
        let mut keyset_info = store
            .get_keyset_info(&expired_keyset.id)
            .await
            .expect("get keyset info")
            .expect("stored keyset info");
        keyset_info.final_expiry = Some(unix_time() - 1);
        let mut tx = store.begin_transaction().await.expect("begin transaction");
        tx.add_keyset_info(keyset_info)
            .await
            .expect("update keyset info");
        tx.commit().await.expect("commit keyset update");
        signatory
            .reload_keys_from_db()
            .await
            .expect("reload keysets");

        // Expiry check runs before crypto, so an unsigned blinded secret is fine here.
        let blinded_secret = SecretKey::generate().public_key();
        let msg = BlindedMessage::new(Amount::from(1), expired_keyset.id, blinded_secret);

        let result = signatory.blind_sign(vec![msg]).await;

        assert!(
            matches!(result, Err(Error::ExpiredKeyset)),
            "expected ExpiredKeyset error, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn subscribe_keysets_pushes_rotation() {
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let signatory = DbSignatory::new(
            store,
            b"test-seed-for-subscribe",
            Default::default(),
            Default::default(),
        )
        .await
        .expect("DbSignatory::new");

        let mut updates = signatory.subscribe_keysets().await.expect("subscribe");

        // The current snapshot is available immediately.
        let initial = updates.borrow_and_update().keysets.len();

        let rotated = signatory
            .rotate_keyset(RotateKeyArguments {
                unit: CurrencyUnit::Sat,
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: 0,
                keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                final_expiry: None,
            })
            .await
            .expect("rotate_keyset");

        // The rotation is pushed to the subscriber.
        updates.changed().await.expect("keyset update");
        let after = updates.borrow_and_update();

        assert!(
            after.keysets.iter().any(|k| k.id == rotated.id),
            "new keyset should be present after rotation"
        );
        assert!(
            after.keysets.len() > initial,
            "keyset count should grow after rotation"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn peer_rotation_is_reconciled_without_restart() {
        // One shared store, two signatory instances with the same seed. A
        // rotation on one must become visible on the other through the change
        // subscription, not only after a restart.
        let store: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync> =
            Arc::new(
                cdk_sqlite::mint::memory::empty()
                    .await
                    .expect("in-memory db"),
            );

        let seed = b"test-seed-for-multi-instance";
        let instance_a = Arc::new(
            DbSignatory::new(store.clone(), seed, Default::default(), Default::default())
                .await
                .expect("DbSignatory::new a"),
        );
        let instance_b = Arc::new(
            DbSignatory::new(store.clone(), seed, Default::default(), Default::default())
                .await
                .expect("DbSignatory::new b"),
        );

        // Only B reloads from the database; A drives the rotation. A short
        // interval keeps the test quick.
        instance_b.spawn_keyset_refresh(Some(Duration::from_millis(500)));

        let rotated = instance_a
            .rotate_keyset(RotateKeyArguments {
                unit: CurrencyUnit::Sat,
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: 0,
                keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                final_expiry: None,
            })
            .await
            .expect("rotate_keyset");

        // B learns about it on its next periodic reload from the database.
        let mut seen = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let keysets = instance_b.keysets().await.expect("keysets");
            if keysets
                .keysets
                .iter()
                .any(|k| k.id == rotated.id && k.active)
            {
                seen = true;
                break;
            }
        }

        assert!(
            seen,
            "instance B should reconcile the peer rotation without a restart"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_rotations_do_not_collide() {
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let signatory = Arc::new(
            DbSignatory::new(
                store,
                b"test-seed-concurrent-rotations",
                Default::default(),
                Default::default(),
            )
            .await
            .expect("DbSignatory::new"),
        );

        // Fire several rotations of the same unit concurrently. Without the
        // rotation lock they could read the same path index and derive
        // duplicate keysets.
        const ROTATIONS: usize = 8;
        let mut handles = Vec::with_capacity(ROTATIONS);
        for _ in 0..ROTATIONS {
            let signatory = signatory.clone();
            handles.push(tokio::spawn(async move {
                signatory
                    .rotate_keyset(RotateKeyArguments {
                        unit: CurrencyUnit::Sat,
                        amounts: vec![1, 2, 4, 8],
                        input_fee_ppk: 0,
                        keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                        final_expiry: None,
                    })
                    .await
                    .expect("rotate_keyset")
            }));
        }

        let mut ids = HashSet::new();
        let mut versions = HashSet::new();
        for handle in handles {
            let keyset = handle.await.expect("join");
            assert!(
                ids.insert(keyset.id),
                "duplicate keyset id from concurrent rotation"
            );
            assert!(
                versions.insert(keyset.version),
                "duplicate path index from concurrent rotation"
            );
        }
        assert_eq!(ids.len(), ROTATIONS);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_rotations_across_instances_do_not_collide() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use cdk_sqlite::mint::MintSqliteDatabase;

        // Two independent instances, each with its own rotation_lock, sharing one
        // on-disk SQLite file. The in-process lock cannot coordinate them; only
        // the database can (BEGIN IMMEDIATE plus busy_timeout serialize the write
        // transactions). A shared file is required because ":memory:" is always a
        // private database per connection pool.
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "cdk-rot-{}-{}.sqlite",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let path_str = path.to_str().expect("utf-8 temp path");

        let store_a = Arc::new(
            MintSqliteDatabase::new(path_str)
                .await
                .expect("shared-file db a"),
        );
        let store_b = Arc::new(
            MintSqliteDatabase::new(path_str)
                .await
                .expect("shared-file db b"),
        );

        let seed = b"test-seed-cross-instance-rotations";
        let instance_a = Arc::new(
            DbSignatory::new(store_a, seed, Default::default(), Default::default())
                .await
                .expect("DbSignatory::new a"),
        );
        let instance_b = Arc::new(
            DbSignatory::new(store_b, seed, Default::default(), Default::default())
                .await
                .expect("DbSignatory::new b"),
        );

        // Alternate rotations between the two instances so no single rotation_lock
        // serializes them all.
        const ROTATIONS: usize = 8;
        let mut handles = Vec::with_capacity(ROTATIONS);
        for i in 0..ROTATIONS {
            let signatory = if i % 2 == 0 {
                instance_a.clone()
            } else {
                instance_b.clone()
            };
            handles.push(tokio::spawn(async move {
                signatory
                    .rotate_keyset(RotateKeyArguments {
                        unit: CurrencyUnit::Sat,
                        amounts: vec![1, 2, 4, 8],
                        input_fee_ppk: 0,
                        keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                        final_expiry: None,
                    })
                    .await
                    .expect("rotate_keyset")
            }));
        }

        let mut ids = HashSet::new();
        let mut versions = HashSet::new();
        for handle in handles {
            let keyset = handle.await.expect("join");
            assert!(
                ids.insert(keyset.id),
                "duplicate keyset id from concurrent cross-instance rotation"
            );
            assert!(
                versions.insert(keyset.version),
                "duplicate path index from concurrent cross-instance rotation"
            );
        }
        assert_eq!(ids.len(), ROTATIONS);

        // Best-effort cleanup of the file and its WAL sidecars.
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{path_str}-wal"));
        let _ = std::fs::remove_file(format!("{path_str}-shm"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn boot_does_not_downgrade_active_below_highest_index() {
        // Boot reactivation and a peer rotation race on a shared store. Boot
        // selects the highest-index keyset and reassigns the active pointer; if
        // that selection reads outside the global keyset lock, a rotation
        // committing a higher keyset in the gap gets clobbered back to the older
        // one. The active pointer must always end at the highest index present.
        let store: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync> =
            Arc::new(
                cdk_sqlite::mint::memory::empty()
                    .await
                    .expect("in-memory db"),
            );
        let seed = b"test-seed-boot-no-downgrade";
        let supported: HashMap<CurrencyUnit, (u64, Vec<u64>)> =
            HashMap::from([(CurrencyUnit::Sat, (0u64, vec![1, 2, 4, 8]))]);

        let seeder = Arc::new(
            DbSignatory::new(store.clone(), seed, supported.clone(), Default::default())
                .await
                .expect("DbSignatory::new seeder"),
        );
        // Establish an initial active keyset so later rotations advance the index.
        seeder
            .rotate_keyset(RotateKeyArguments {
                unit: CurrencyUnit::Sat,
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: 0,
                keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                final_expiry: None,
            })
            .await
            .expect("seed rotation");

        for _ in 0..16 {
            let rotate = {
                let seeder = seeder.clone();
                tokio::spawn(async move {
                    seeder
                        .rotate_keyset(RotateKeyArguments {
                            unit: CurrencyUnit::Sat,
                            amounts: vec![1, 2, 4, 8],
                            input_fee_ppk: 0,
                            keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                            final_expiry: None,
                        })
                        .await
                        .expect("peer rotation")
                })
            };
            let boot = {
                let store = store.clone();
                let supported = supported.clone();
                tokio::spawn(async move {
                    DbSignatory::new(store, seed, supported, Default::default())
                        .await
                        .expect("DbSignatory::new boot")
                })
            };
            rotate.await.expect("join rotate");
            boot.await.expect("join boot");

            let mut tx = store.begin_transaction().await.expect("begin tx");
            let infos = tx.get_keyset_infos().await.expect("keyset infos");
            let active = tx
                .get_active_keysets()
                .await
                .expect("active keysets")
                .get(&CurrencyUnit::Sat)
                .copied();
            tx.commit().await.expect("commit");
            let highest = infos
                .iter()
                .max_by_key(|k| k.derivation_path_index)
                .expect("at least one keyset");
            assert_eq!(
                active,
                Some(highest.id),
                "boot must not reactivate a keyset below the highest index"
            );
        }
    }

    #[test]
    fn mint_mod_generate_keyset_from_seed() {
        let seed = hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        let keyset = MintKeySet::generate_from_seed(
            &Secp256k1::new(),
            &seed,
            &[1, 2],
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0).unwrap(),
            0,
            None,
            cdk_common::nut02::KeySetVersion::Version01,
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 2);

        let expected_amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = vec![
            (
                Amount::from(1),
                PublicKey::from_hex(
                    "0380a4bb98d9bc5d5b11c7cf2b705dbc894b62ac99cf67e0ef1a3d47ea6dc54706",
                )
                .unwrap(),
            ),
            (
                Amount::from(2),
                PublicKey::from_hex(
                    "022fe5e50a15d721014b538ca6a3ff20ee049b195ba0b1705f64829da8779b6940",
                )
                .unwrap(),
            ),
        ]
        .into_iter()
        .collect();

        let amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = keyset
            .keys
            .iter()
            .map(|(amount, pair)| (*amount, pair.public_key))
            .collect();

        assert_eq!(amounts_and_pubkeys, expected_amounts_and_pubkeys);
    }

    #[test]
    fn mint_mod_generate_keyset_from_xpriv() {
        let seed = hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        let network = Network::Bitcoin;
        let xpriv = Xpriv::new_master(network, &seed).expect("Failed to create xpriv");
        let keyset = MintKeySet::generate_from_xpriv(
            &Secp256k1::new(),
            xpriv,
            &[1, 2],
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0).unwrap(),
            0,
            None,
            cdk_common::nut02::KeySetVersion::Version00,
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 2);

        let expected_amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = vec![
            (
                Amount::from(1),
                PublicKey::from_hex(
                    "0380a4bb98d9bc5d5b11c7cf2b705dbc894b62ac99cf67e0ef1a3d47ea6dc54706",
                )
                .unwrap(),
            ),
            (
                Amount::from(2),
                PublicKey::from_hex(
                    "022fe5e50a15d721014b538ca6a3ff20ee049b195ba0b1705f64829da8779b6940",
                )
                .unwrap(),
            ),
        ]
        .into_iter()
        .collect();

        let amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = keyset
            .keys
            .iter()
            .map(|(amount, pair)| (*amount, pair.public_key))
            .collect();

        assert_eq!(amounts_and_pubkeys, expected_amounts_and_pubkeys);
    }

    #[test]
    fn mint_make_btc_remote_signer_keyset() {
        let seed = hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        let network = Network::Bitcoin;
        let xpriv = Xpriv::new_master(network, &seed).expect("Failed to create xpriv");
        let keyset = MintKeySet::generate_from_xpriv(
            &Secp256k1::new(),
            xpriv,
            &[
                1,
                2,
                4,
                8,
                16,
                32,
                64,
                128,
                256,
                512,
                1024,
                2048,
                4096,
                8192,
                16384,
                32768,
                65536,
                131072,
                262144,
                524288,
                1_048_576,
                2_097_152,
                4_194_304,
                8_388_608,
                16_777_216,
                33_554_432,
                67_108_864,
                134_217_728,
                268_435_456,
                536_870_912,
                1_073_741_824,
                2_147_483_648,
                4_294_967_296,
                8_589_934_592,
                17_179_869_184,
                34_359_738_368,
                68_719_476_736,
                137_438_953_472,
                274_877_906_944,
                549_755_813_888,
                1_099_511_627_776,
                2_199_023_255_552,
                4_398_046_511_104,
                8_796_093_022_208,
                17_592_186_044_416,
                35_184_372_088_832,
                70_368_744_177_664,
                140_737_488_355_328,
                281_474_976_710_656,
                562_949_953_421_312,
                1_125_899_906_842_624,
                2_251_799_813_685_248,
                4_503_599_627_370_496,
                9_007_199_254_740_992,
                18_014_398_509_481_984,
                36_028_797_018_963_968,
                72_057_594_037_927_936,
                144_115_188_075_855_872,
                288_230_376_151_711_744,
                576_460_752_303_423_488,
                1_152_921_504_606_846_976,
                2_305_843_009_213_693_952,
                4_611_686_018_427_387_904,
                9_223_372_036_854_775_808,
            ],
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 1).unwrap(),
            0,
            None,
            cdk_common::nut02::KeySetVersion::Version00,
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 64);

        let expected_results: HashMap<u64, &str> = [
            (
                1,
                "0233501d047ff4058007722d5d24e10a8ff5c723a677be411fff46a3cee9a92cc0",
            ),
            (
                2,
                "03a09803ce40118b8917fafa08409dbe6e8bb36d76c55f4c58400cd720abaf54cb",
            ),
            (
                4,
                "02dac058df2e8611098286ef87ee9698f555548784ab4b1a860c79338073ad8c49",
            ),
            (
                8,
                "025b66b937d65544981817aa9a053a762a7d72a7543c66a54370ea68aa53170a10",
            ),
            (
                16,
                "027cf2ad5fa02b99ea37b305048562828453d89dfa7defcda1c10f6746f25f7541",
            ),
            (
                32,
                "0336033cbbc044737bced1fd40b7f0cb0ce08a83aedaa882ed1ced875a1f517879",
            ),
            (
                64,
                "035be95ecaadbfe67b14f07205d13bbcab5da58bb595c57dfb9b61c5e3e7e4de0e",
            ),
            (
                128,
                "0232c757957a8f5a14e93a9bbe8852c273b985ad238ce9b4d5a16885d8a761462b",
            ),
            (
                256,
                "02cbd889df7d38e95dca2ee0e09bc22e3ae57e95975043854a5560a464f970ac1f",
            ),
            (
                512,
                "02c99a0b72ba8f01c5da765c534e75ae3e5f51e4931bfced18a91df4b9233b168f",
            ),
            (
                1024,
                "0320527abb6ae3dd6db9da5041ca941be679e953b446614843af7a4393e9ac96bc",
            ),
            (
                2048,
                "033f9276b0c5f73fbeb0130eab5705a8e878f4191fe251a18cbd918cda3c9e2d5e",
            ),
            (
                4096,
                "03cf69ed2939be4ac35308560d4423e1a0d96cacf9fe33267c7e6a047bf438e53e",
            ),
            (
                8192,
                "027c8bfff71352766c3870e9f5f577830bbb44eadfb757fdff9a8cd209c4b22d76",
            ),
            (
                16384,
                "02ea21bd310828b9e46746eba2ae985626b3a2efc2468db66ae480715dc6deec8a",
            ),
            (
                32768,
                "027ae7179192282d5b44ac55bff82c13e1ea916ae1edefa33ea64100be7408e015",
            ),
            (
                65536,
                "028f333c1beada3445cb62108e35d72199925a055c1e7c102c742e1761770f6c62",
            ),
            (
                131072,
                "03de95cae3614499a3df2d412e91aa09ddef8b8d49e8d652e3798419da86958139",
            ),
            (
                262144,
                "03c7817c19b4b107eb2ccf2f32b60f9c22a59a1d4a93e492ad01f1505097a654b7",
            ),
            (
                524288,
                "028aad03886b6ec6b9f628090e9c151a73f025aa949a9686dac1f0b32995a4e8df",
            ),
            (
                1048576,
                "034bf50a5916d9f112b8fbfe82a5ac914b5bec792b107cf25922c9866f002473e8",
            ),
            (
                2097152,
                "03d2894e1b1b7ab7497ff69e16d280b630f60ba34fe00edd7c748ae5ee73bc0d1a",
            ),
            (
                4194304,
                "0285ba0ee2960927de958610b13d63fc29019407eb32c477d9a2d016fda3062a37",
            ),
            (
                8388608,
                "03d7a4b4b1b8d6b9f2b5966e380a62f8efd53f79d1965e076a716d2fb75e9774a1",
            ),
            (
                16777216,
                "037a033e2f1df992523df83bcb9aa02cefdadd59882d7949f4500f5493d89fa2fd",
            ),
            (
                33554432,
                "03014de7af4809599cabc6d6b30e5121b4a88153eb38a7b66dd8e50e3166215ab0",
            ),
            (
                67108864,
                "0240162a1d2eb1841450de53a6244a625922b14006153d5219dad0fcf0c369c497",
            ),
            (
                134217728,
                "03f8c6f7b0ee71f66940a33c746c3bf8b1cba793a498dd2fdeb6857552415a4d5d",
            ),
            (
                268435456,
                "02dc9de15fa1332f5a2c8f85045ea127cbc3407fb8a844b453f38e1c9cdce9ef87",
            ),
            (
                536870912,
                "0291bdcb1719b5bf447b2885efc84061d1de30b9d1f583d25034059457a2fd739e",
            ),
            (
                1073741824,
                "02f8a96485e3fa791f57d7f4ef279dd3617b873efbdf673815c49dbf9ce7422b0d",
            ),
            (
                2147483648,
                "02ff8cf3e3de985bb2f286c98e335a175b2b53a0e0d7fa1f53d642c95a372329a2",
            ),
            (
                4294967296,
                "02d96196cc54e7506bfe9fdb4a0d691eed2948ecb9b8e81d28d27225287ad5debc",
            ),
            (
                8589934592,
                "03e64e5664f7ab843f41aaf4c0534d698b3318d140c23cbd2fcc33eece53400dac",
            ),
            (
                17179869184,
                "034c9a4bf7b4cb8fac6ace994624e5250ddac5ac84541b6c8bd12b71d22719bb2d",
            ),
            (
                34359738368,
                "0313027c2b106c7dcdee0d806c3343026260276c6793d4d1dfdf79aae30875be31",
            ),
            (
                68719476736,
                "03081adca96d42cb2ac4ac94e0ea2aac4d9412265ae55ed377e3c0357aa1157253",
            ),
            (
                137438953472,
                "02fdc4118761739425220ba87dee5ea9fdc1d581abfcb506fb5afabf76e172b798",
            ),
            (
                274877906944,
                "031dd7cd25f761c8f80828b487bab1cef730f68e8d6f2026b443cc7223862f6c73",
            ),
            (
                549755813888,
                "02da505eab15744a6fd3fa6b3257bced520d4d294ea94444528fd30d7f90948629",
            ),
            (
                1099511627776,
                "02bfc54369099958275376ab030f2a085532c8a00ae4d1bbfa5031c64b42d58a47",
            ),
            (
                2199023255552,
                "032241a5d4d1e988b8ae85f68a381df0e40065ae8c81b1c4f7ea31c87eab2c0d81",
            ),
            (
                4398046511104,
                "03a681e41990d350cdedd30840f26ad970b4015dd6e6b5c03f7cc99b384bee8762",
            ),
            (
                8796093022208,
                "033d5293a33cda29d65058d6d3a4b821472574e92414fa052c79f8bdc1cd72faba",
            ),
            (
                17592186044416,
                "033ddfec40622aaf62d672f43fd05ddb396afd7ad9f00daede45102c890d3a012b",
            ),
            (
                35184372088832,
                "02564bbdcbed18a8e2d79b2fdad6e5e8a9fe92e853ab23170934d84015cc4b96b0",
            ),
            (
                70368744177664,
                "02170950642b94d0ed232370d5dd3630b5eb7e73791447fb961b12d8139de975de",
            ),
            (
                140737488355328,
                "02b2add5a6eb5dc06f706e9dba190ba412c2c7ba240284b336b66ef38a39e51f1c",
            ),
            (
                281474976710656,
                "03e3e584a4bc1d0a6399f5b6b9355bd67a10ad9f46c8a4283de96854e47eb4357c",
            ),
            (
                562949953421312,
                "033821262e6a78f29dad81d3133845883a7632a47f51ab1d99a0eae4a5354eef45",
            ),
            (
                1125899906842624,
                "038db672a61c70dc66b504152ea39b607527f2f59e8ebfdf8d955c38e914661534",
            ),
            (
                2251799813685248,
                "03dafb9683eac036a422266ddc85b675bf13aeafe0658cad2ec1555c28f4049b28",
            ),
            (
                4503599627370496,
                "0351733345d4bb491e27bdb221e382d00f2248f2ee7f04dc6f3faab2692fbd296c",
            ),
            (
                9007199254740992,
                "03f930c1e6c154ca169370adbec7691fd9c11245867a37ae086f7547f5c9e8386f",
            ),
            (
                18014398509481984,
                "02d700dc30d3cd6be292bddbd5f74c09df784862c785cd763ad6c829be59c21bed",
            ),
            (
                36028797018963968,
                "03444b9c312900fffbd478e390aa6fdf9d3ffe230239141ecadf0bcee25e379512",
            ),
            (
                72057594037927936,
                "03af7acedfcfcaf83cfdb7d171ef64723286bd6e0ab90f3629e627e77955917776",
            ),
            (
                144115188075855872,
                "02e35aef647a881e8c318879fb81b6261df73e385dfbc5ff3fc0ab40f13f5ed560",
            ),
            (
                288230376151711744,
                "024558ed8e986901e05839c34d17c261c8d93b8cabb5dee83ab805bb5028e5e463",
            ),
            (
                576460752303423488,
                "024f60a89ba055e009d84a90a13a7860a909fb486a8ffb4315c2f59aff6fbfd929",
            ),
            (
                1152921504606846976,
                "0311b2a5b91dfaebab4fb125338fd38dab72ec5671e6db5f468cb1477970ea3876",
            ),
            (
                2305843009213693952,
                "02aeaa116d930767b5143cac922511c0e093beee5a2850f67490f5a5bb44a8af76",
            ),
            (
                4611686018427387904,
                "02bf7003847bc8e7ad35ea5c8975e3fdde8d1c43ef540d250cf2dc75792c733647",
            ),
            (
                9223372036854775808,
                "0376b06a13092fbb679f6e7a90ce877c37d5a20714a65567177a91a0479b3e86a9",
            ),
        ]
        .into_iter()
        .collect();

        assert_eq!(keyset.id.to_string(), "00b5a0580f75cc2f".to_string());

        for key in expected_results {
            let amount = Amount::from(key.0);
            let pubkey = keyset
                .keys
                .get(&amount)
                .unwrap()
                .public_key
                .clone()
                .to_hex();

            assert_eq!(pubkey, key.1.to_string());
        }
    }

    #[test]
    fn mint_make_auth_remote_signer_keyset() {
        let seed = hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        let network = Network::Bitcoin;
        let xpriv = Xpriv::new_master(network, &seed).expect("Failed to create xpriv");
        let keyset = MintKeySet::generate_from_xpriv(
            &Secp256k1::new(),
            xpriv,
            &[1],
            CurrencyUnit::Auth,
            derivation_path_from_unit(CurrencyUnit::Auth, 1).unwrap(),
            0,
            None,
            cdk_common::nut02::KeySetVersion::Version00,
        );

        assert_eq!(keyset.unit, CurrencyUnit::Auth);
        assert_eq!(keyset.keys.len(), 1);

        assert_eq!(keyset.id.to_string(), "00e1cf6079abb988".to_string());

        let amount = Amount::from(1);
        let pubkey = keyset
            .keys
            .get(&amount)
            .unwrap()
            .public_key
            .clone()
            .to_hex();
        assert_eq!(
            pubkey,
            "025b6c1ca8bb741a6f2321c953266df7bf3f3f2c3be8c54c0a6e41bb00976046a4".to_string()
        );
    }
}
