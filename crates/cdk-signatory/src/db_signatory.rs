//! Main Signatory implementation
//!
//! It is named db_signatory because it uses a database to persist state. The
//! database is a persistence layer only: it is read once on boot to hydrate the
//! in-memory keysets and written only when rotating keys. Every other operation
//! is served from and mutates in-memory state. See ADR-0003.
//!
//! Boot has two modes. The standalone server uses [`DbSignatory::new`], which
//! attempts the initial load once and, if the database is unavailable, returns
//! anyway and keeps retrying in the background; until that first load succeeds
//! every key-using operation returns [`Error::KeysetsNotLoaded`]. The embedded
//! mint uses [`DbSignatory::try_new`], which attempts the load once
//! and bubbles up any error so the mint fails to boot instead of serving
//! without keys.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::secp256k1::{self, Secp256k1};
use cdk_common::dhke::{sign_message, verify_message};
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::{BlindSignature, BlindedMessage, CurrencyUnit, Id, MintKeySet, Proof};
use cdk_common::util::unix_time;
use cdk_common::{database, Error, PublicKey};
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::instrument;

use crate::common::{
    check_unit_string_collision, create_new_keyset, derivation_path_from_unit, init_keysets,
};
use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets};

/// Initial delay before retrying a failed boot load.
const BOOT_RETRY_BACKOFF: Duration = Duration::from_secs(1);

/// Cap on the boot-load retry backoff.
const BOOT_RETRY_MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Longest gap between auto-rotation checks. The rotation loop polls at
/// `min(interval, DEFAULT_TICKET)` so a large interval (hours/days) still wakes
/// periodically, while a small interval is not checked slower than itself.
const DEFAULT_TICKET: Duration = Duration::from_secs(120);

/// In-memory Signatory
///
/// This is the default signatory implementation for the mint.
///
/// The private keys and the all key-related data is stored in memory, in the same process, but it
/// is not accessible from the outside.
#[allow(missing_debug_implementations)]
pub struct DbSignatory {
    inner: Arc<Inner>,
}

/// Shared state behind the signatory.
///
/// Held in an `Arc` so the background boot-load task can share it with the
/// signatory without changing `new`'s public signature.
struct Inner {
    keysets: RwLock<HashMap<Id, (MintKeySetInfo, MintKeySet)>>,
    active_keysets: RwLock<HashMap<CurrencyUnit, Id>>,
    /// Serializes keyset rotations. The standalone signatory gRPC server calls
    /// rotate_keyset directly (no embedded single-runner), so two concurrent
    /// rotations of the same unit could otherwise read the same path index and
    /// derive duplicate keysets.
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
    /// Set once the first successful load from the database completes. Operations
    /// that need keys error until then.
    loaded: AtomicBool,
}

impl DbSignatory {
    /// Build the shared inner state without loading keys.
    ///
    /// # Panics
    ///
    /// Panics if the seed produces an invalid master key (should never happen with valid entropy).
    fn build_inner(
        localstore: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
        seed: &[u8],
        supported_units: HashMap<CurrencyUnit, (u64, Vec<u64>)>,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Arc<Inner> {
        let secp_ctx = Secp256k1::new();
        let xpriv = Xpriv::new_master(bitcoin::Network::Bitcoin, seed).expect("RNG busted");

        let xpub: PublicKey = xpriv.to_keypair(&secp_ctx).public_key().into();
        let (keyset_updates, _) = watch::channel(SignatoryKeysets {
            pubkey: xpub,
            keysets: vec![],
        });

        Arc::new(Inner {
            keysets: Default::default(),
            active_keysets: Default::default(),
            rotation_lock: Default::default(),
            localstore,
            custom_paths,
            supported_units,
            xpub,
            secp_ctx,
            xpriv,
            keyset_updates,
            loaded: AtomicBool::new(false),
        })
    }

    /// Creates a new signatory with resilient boot, for the standalone server.
    ///
    /// The initial keyset load is attempted once. If the database is
    /// unavailable, construction still succeeds and a background task keeps
    /// retrying the load until it succeeds; operations return
    /// [`Error::KeysetsNotLoaded`] until then. This keeps a standalone signatory
    /// process up across a transient database outage instead of exiting.
    pub async fn new(
        localstore: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
        seed: &[u8],
        supported_units: HashMap<CurrencyUnit, (u64, Vec<u64>)>,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<Self, Error> {
        let inner = Self::build_inner(localstore, seed, supported_units, custom_paths);

        // Try once so a healthy database is loaded before returning. If it
        // fails, keep retrying in the background rather than taking the whole
        // process down.
        if let Err(err) = inner.boot_load().await {
            tracing::warn!("initial keyset load failed, retrying in background: {err}");
            Inner::spawn_boot_retry(Arc::downgrade(&inner));
        }

        Ok(Self { inner })
    }

    /// Creates a new signatory with strict boot, for the embedded mint.
    ///
    /// The keyset load is attempted once and any error is bubbled up. There is
    /// no background retry: an embedded signatory is part of the mint process,
    /// so a failed load should fail the mint boot rather than leave it serving
    /// [`Error::KeysetsNotLoaded`]. On success the returned signatory is loaded.
    pub async fn try_new(
        localstore: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
        seed: &[u8],
        supported_units: HashMap<CurrencyUnit, (u64, Vec<u64>)>,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<Self, Error> {
        let inner = Self::build_inner(localstore, seed, supported_units, custom_paths);
        inner.boot_load().await?;
        Ok(Self { inner })
    }
}

impl Inner {
    /// Load keysets from the database and mark the signatory ready.
    ///
    /// This runs the boot-time database reactivation (`init_keysets`) and then
    /// hydrates memory from the database. On success the signatory is marked
    /// loaded and operations start serving.
    async fn boot_load(&self) -> Result<(), Error> {
        init_keysets(
            self.xpriv,
            &self.secp_ctx,
            &self.localstore,
            &self.supported_units,
        )
        .await?;
        self.load_keys_from_db().await?;
        self.loaded.store(true, Ordering::Release);
        Ok(())
    }

    /// Retry [`Inner::boot_load`] with exponential backoff until it succeeds.
    ///
    /// Holds a [`Weak`] so the loop ends once the signatory is dropped: the
    /// strong reference is released before each sleep, so a dropped signatory
    /// stops the task within one backoff interval.
    fn spawn_boot_retry(weak: Weak<Inner>) {
        tokio::spawn(async move {
            let mut backoff = BOOT_RETRY_BACKOFF;
            loop {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                match inner.boot_load().await {
                    Ok(()) => {
                        tracing::info!("keysets loaded from database");
                        return;
                    }
                    Err(err) => tracing::warn!("keyset load retry failed: {err}"),
                }
                drop(inner);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BOOT_RETRY_MAX_BACKOFF);
            }
        });
    }

    /// Returns an error until the first successful load from the database.
    fn ensure_loaded(&self) -> Result<(), Error> {
        if self.loaded.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(Error::KeysetsNotLoaded)
        }
    }

    /// Hydrate the in-memory keysets from the database.
    ///
    /// This is the only path that reads keysets from the database. Since the
    /// database is owned by this process, all keysets are loaded into memory
    /// and memory is the primary source afterwards; the database is only the
    /// persistence layer. Any later operation reads from and mutates memory,
    /// never the database directly.
    async fn load_keys_from_db(&self) -> Result<(), Error> {
        let mut keysets = self.keysets.write().await;
        let mut active_keysets = self.active_keysets.write().await;
        keysets.clear();
        active_keysets.clear();

        let db_active_keysets = self.localstore.get_active_keysets().await?;

        for mut info in self.localstore.get_keyset_infos().await? {
            let id = info.id;
            let keyset = self.generate_keyset(&info);
            info.active = db_active_keysets.get(&info.unit) == Some(&info.id);
            if info.active {
                active_keysets.insert(info.unit.clone(), id);
            }
            keysets.insert(id, (info, keyset));
        }

        self.publish_snapshot(&keysets);

        Ok(())
    }

    /// Publish the current keyset set to any subscribers of the watch channel.
    ///
    /// Callers hold the `keysets` write lock while calling this so the published
    /// snapshot stays consistent with the in-memory state that produced it.
    fn publish_snapshot(&self, keysets: &HashMap<Id, (MintKeySetInfo, MintKeySet)>) {
        self.keyset_updates.send_replace(SignatoryKeysets {
            pubkey: self.xpub,
            keysets: keysets.values().map(|k| k.into()).collect(),
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

    /// Snapshot the current keysets from memory.
    async fn keysets_snapshot(&self) -> SignatoryKeysets {
        SignatoryKeysets {
            pubkey: self.xpub,
            keysets: self
                .keysets
                .read()
                .await
                .values()
                .map(|k| k.into())
                .collect(),
        }
    }
}

impl DbSignatory {
    /// Rotate every active keyset that has been valid for at least `max_age`.
    ///
    /// Each replacement keeps the previous keyset's amounts, input fee and id
    /// version. When the previous keyset had a `final_expiry`, the new one is
    /// pushed forward by the keyset's active age so it stays valid at least as
    /// long as the keyset it replaces.
    ///
    /// The age check needs each keyset's `valid_from`, which does not cross the
    /// `Signatory` trait, so this reads the in-memory keysets directly and then
    /// rotates each due unit through `rotate_keyset`, which serializes with any
    /// mint-initiated rotation and updates memory in place.
    async fn rotate_aged_keysets(&self, max_age: Duration) -> Result<(), Error> {
        let now = unix_time();
        let max_age = max_age.as_secs();

        if max_age == 0 {
            return Ok(());
        }

        // Snapshot the units due for rotation from short-lived read locks. The
        // locks are released before rotating so signing is never blocked, and
        // each unit is rechecked below in case a rotation landed in between.
        let due: Vec<MintKeySetInfo> = {
            let keysets = self.inner.keysets.read().await;
            let active_keysets = self.inner.active_keysets.read().await;
            active_keysets
                .values()
                .filter_map(|id| keysets.get(id).map(|(info, _)| info.clone()))
                .filter(|info| now.saturating_sub(info.valid_from) >= max_age)
                .collect()
        };

        for info in due {
            // A mint-initiated rotation may have advanced this unit between the
            // snapshot above and reaching it here. If its active keyset is no
            // longer the one judged due, skip it rather than issue a redundant
            // keyset.
            let still_active = self
                .inner
                .active_keysets
                .read()
                .await
                .get(&info.unit)
                .copied();
            if still_active != Some(info.id) {
                tracing::debug!(
                    "Skipping auto-rotation of keyset {} for unit {}: already rotated",
                    info.id,
                    info.unit
                );
                continue;
            }

            let active_age = now.saturating_sub(info.valid_from);
            let final_expiry = info
                .final_expiry
                .map(|expiry| expiry.saturating_add(active_age));

            tracing::info!(
                "Auto-rotating keyset {} for unit {} (active for {}s, interval {}s)",
                info.id,
                info.unit,
                active_age,
                max_age
            );

            self.rotate_keyset(RotateKeyArguments {
                unit: info.unit.clone(),
                amounts: info.amounts.clone(),
                input_fee_ppk: info.input_fee_ppk,
                keyset_id_type: info.id.get_version(),
                final_expiry,
            })
            .await?;
        }

        Ok(())
    }

    /// Spawn the background keyset auto-rotation task.
    ///
    /// Every `interval` the task rotates each active keyset that has been valid
    /// for at least `interval`. Rotations are published to keyset subscribers
    /// through the same path as manual rotations, so mints learn about them
    /// without a restart.
    ///
    /// The task holds a weak reference to the signatory, so it stops on its own
    /// once the signatory is dropped. Sending `true` on `shutdown` stops it
    /// cooperatively: a rotation already in flight runs to completion and the
    /// loop then exits, so shutdown never interrupts a rotation mid-flight.
    /// Aborting the returned handle also stops it, as a drop-time fallback.
    pub fn spawn_auto_rotation(
        self: &Arc<Self>,
        interval: Duration,
        shutdown: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            Self::auto_rotation_loop(weak, interval, shutdown).await;
        })
    }

    async fn auto_rotation_loop(
        weak: Weak<Self>,
        interval: Duration,
        mut shutdown: watch::Receiver<bool>,
    ) {
        // Poll at `min(interval, DEFAULT_TICKET)`. Each tick asks
        // `rotate_aged_keysets` to rotate any keyset older than `interval`. The
        // cadence is capped at `DEFAULT_TICKET` so large intervals (hours/days)
        // still check periodically, and floored at the interval so a small
        // interval is not checked slower than it. The first tick fires
        // immediately, so a freshly built keyset (age 0) is only rotated once it
        // has aged past `interval` on a later tick.
        let mut ticker = tokio::time::interval(interval.min(DEFAULT_TICKET));

        loop {
            // `biased` checks shutdown first, so once it is signalled the loop
            // exits promptly instead of running one more rotation. Shutdown is
            // only observed between rotations, so a rotation started below always
            // runs to completion. `wait_for` re-checks the latched value, so a
            // signal sent while a rotation was in flight is not missed.
            tokio::select! {
                biased;
                res = shutdown.wait_for(|stop| *stop) => {
                    // `Ok` means shutdown was signalled; `Err` means the sender
                    // was dropped (mint gone). Either way, stop rotating.
                    let _ = res;
                    break;
                }
                _ = ticker.tick() => {}
            }

            let Some(signatory) = weak.upgrade() else {
                break;
            };

            if let Err(err) = signatory.rotate_aged_keysets(interval).await {
                tracing::error!("Automatic keyset rotation failed: {}", err);
            }
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
        self.inner.ensure_loaded()?;
        let keysets = self.inner.keysets.read().await;

        blinded_messages
            .into_iter()
            .map(|blinded_message| {
                let BlindedMessage {
                    amount,
                    blinded_secret,
                    keyset_id,
                    ..
                } = blinded_message;

                let (info, key) = keysets.get(&keyset_id).ok_or(Error::UnknownKeySet)?;
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
        self.inner.ensure_loaded()?;
        let keysets = self.inner.keysets.read().await;

        proofs.into_iter().try_for_each(|proof| {
            let (_, key) = keysets.get(&proof.keyset_id).ok_or(Error::UnknownKeySet)?;
            let key_pair = key.keys.get(&proof.amount).ok_or(Error::UnknownKeySet)?;
            verify_message(&key_pair.secret_key, proof.c, proof.secret.as_bytes())?;
            Ok(())
        })
    }

    #[tracing::instrument(skip_all)]
    async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
        self.inner.ensure_loaded()?;
        Ok(self.inner.keysets_snapshot().await)
    }

    #[tracing::instrument(skip_all)]
    async fn subscribe_keysets(&self) -> Result<watch::Receiver<SignatoryKeysets>, Error> {
        Ok(self.inner.keyset_updates.subscribe())
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[tracing::instrument(skip(self))]
    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
        self.inner.ensure_loaded()?;

        // Serialize rotations. The standalone signatory gRPC server invokes this
        // directly (no embedded single-runner), so without this two concurrent
        // rotations of the same unit could read the same path index below and
        // derive duplicate keysets. Held for the whole method.
        let _rotation = self.inner.rotation_lock.lock().await;

        // Derive the next path index and default amounts from the in-memory
        // active keyset rather than the database. The rotation lock above keeps
        // this read stable across the DB write and the in-memory update.
        // Acquire keysets before active_keysets, the same order the write phase
        // below uses (and load_keys_from_db), keeping lock ordering consistent.
        let (path_index, amounts) = {
            let keysets = self.inner.keysets.read().await;
            let active_keysets = self.inner.active_keysets.read().await;
            if let Some(current_keyset_id) = active_keysets.get(&args.unit) {
                let (info, _) = keysets.get(current_keyset_id).ok_or(Error::UnknownKeySet)?;
                (
                    info.derivation_path_index.unwrap_or(1) + 1,
                    info.amounts.clone(),
                )
            } else {
                (1, vec![])
            }
        };

        let derivation_path = match self.inner.custom_paths.get(&args.unit) {
            Some(path) => path.clone(),
            None => derivation_path_from_unit(args.unit.clone(), path_index)
                .ok_or(Error::UnsupportedUnit)?,
        };

        let amounts = if args.amounts.is_empty() {
            if amounts.is_empty() {
                return Err(Error::Custom("Amounts cannot be empty".to_string()));
            }
            amounts
        } else {
            args.amounts
        };

        let (keyset, info) = create_new_keyset(
            &self.inner.secp_ctx,
            self.inner.xpriv,
            derivation_path,
            Some(path_index),
            args.unit.clone(),
            &amounts,
            args.input_fee_ppk,
            args.final_expiry,
            args.keyset_id_type,
        );

        let keysets = self.inner.keysets_snapshot().await;
        check_unit_string_collision(&keysets.keysets, &info)?;

        let id = info.id;

        // Persist the rotation. This is the only path that writes to the
        // database.
        let mut tx = self.inner.localstore.begin_transaction().await?;
        tx.add_keyset_info(info.clone()).await?;
        tx.set_active_keyset(args.unit.clone(), id).await?;
        tx.commit().await?;

        // Refresh the in-memory state to match what was just persisted, without
        // reading the database back. This mirrors a fresh boot: the active
        // pointer for the unit moves to the new keyset, so any previously active
        // keyset for the unit becomes inactive.
        let mut info = info;
        info.active = true;
        let signatory_keyset: SignatoryKeySet = (&(info.clone(), keyset.clone())).into();

        let mut keysets = self.inner.keysets.write().await;
        let mut active_keysets = self.inner.active_keysets.write().await;

        if let Some(prev_id) = active_keysets.insert(args.unit, id) {
            if let Some((prev_info, _)) = keysets.get_mut(&prev_id) {
                prev_info.active = false;
            }
        }
        keysets.insert(id, (info, keyset));

        self.inner.publish_snapshot(&keysets);

        Ok(signatory_keyset)
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use bitcoin::key::Secp256k1;
    use bitcoin::Network;
    use cdk_common::nuts::SecretKey;
    use cdk_common::util::{hex, unix_time};
    use cdk_common::{Amount, MintKeySet, PublicKey};

    use super::*;

    /// Wraps a real store but returns a transient error for every read while
    /// `fail` is set, to exercise the background boot-load retry.
    struct FailingStore {
        inner: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
        fail: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl database::MintKeysDatabase for FailingStore {
        type Err = database::Error;

        async fn begin_transaction<'a>(
            &'a self,
        ) -> Result<
            Box<dyn database::MintKeyDatabaseTransaction<'a, Self::Err> + Send + Sync + 'a>,
            database::Error,
        > {
            self.inner.begin_transaction().await
        }

        async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
            if self.fail.load(Ordering::Acquire) {
                return Err(database::Error::Locked);
            }
            self.inner.get_active_keyset_id(unit).await
        }

        async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
            if self.fail.load(Ordering::Acquire) {
                return Err(database::Error::Locked);
            }
            self.inner.get_active_keysets().await
        }

        async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
            if self.fail.load(Ordering::Acquire) {
                return Err(database::Error::Locked);
            }
            self.inner.get_keyset_info(id).await
        }

        async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
            if self.fail.load(Ordering::Acquire) {
                return Err(database::Error::Locked);
            }
            self.inner.get_keyset_infos().await
        }
    }

    #[tokio::test]
    async fn operations_error_until_background_load_succeeds() {
        let sqlite = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let fail = Arc::new(AtomicBool::new(true));
        let store = Arc::new(FailingStore {
            inner: sqlite,
            fail: fail.clone(),
        });

        // Construction succeeds even though the database is unavailable.
        let signatory = DbSignatory::new(
            store,
            b"test-seed-background-load",
            Default::default(),
            Default::default(),
        )
        .await
        .expect("DbSignatory::new returns while the db is unavailable");

        // Until the first load, operations report the keysets are not loaded.
        assert!(matches!(
            signatory.keysets().await,
            Err(Error::KeysetsNotLoaded)
        ));

        // Recover the database; the background retry loads and the signatory
        // starts serving.
        fail.store(false, Ordering::Release);

        let loaded = tokio::time::timeout(Duration::from_secs(5), async {
            while signatory.keysets().await.is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;

        assert!(
            loaded.is_ok(),
            "signatory should load once the database recovers"
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
            store,
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
                final_expiry: Some(unix_time() - 1),
            })
            .await
            .expect("rotate_keyset");

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

    #[tokio::test]
    async fn rotate_aged_keysets_respects_age() {
        let signatory = test_signatory(b"test-seed-for-aged-rotation").await;

        // Seed one keyset aged 120 seconds.
        let seeded = seed_aged_keyset(
            &signatory,
            CurrencyUnit::Sat,
            &[1, 2, 4, 8],
            0,
            cdk_common::nut02::KeySetVersion::Version00,
            None,
            120,
        )
        .await;

        // An interval larger than the keyset's age leaves it in place.
        signatory
            .rotate_aged_keysets(Duration::from_secs(600))
            .await
            .expect("rotate_aged_keysets");
        assert_eq!(
            *signatory
                .inner
                .active_keysets
                .read()
                .await
                .get(&CurrencyUnit::Sat)
                .expect("active sat keyset"),
            seeded,
            "keyset younger than the interval must not rotate"
        );

        // An interval below the keyset's age makes it due, so it rotates.
        signatory
            .rotate_aged_keysets(Duration::from_secs(60))
            .await
            .expect("rotate_aged_keysets");
        assert_ne!(
            *signatory
                .inner
                .active_keysets
                .read()
                .await
                .get(&CurrencyUnit::Sat)
                .expect("active sat keyset"),
            seeded,
            "keyset at or past the interval must rotate"
        );
    }

    #[tokio::test]
    async fn spawn_auto_rotation_pushes_new_keyset() {
        let signatory = test_signatory(b"test-seed-for-auto-rotation").await;

        // Seed an already-aged active Sat keyset for the task to rotate.
        seed_aged_keyset(
            &signatory,
            CurrencyUnit::Sat,
            &[1, 2, 4, 8],
            0,
            cdk_common::nut02::KeySetVersion::Version00,
            None,
            120,
        )
        .await;

        let mut updates = signatory.subscribe_keysets().await.expect("subscribe");
        let before = updates.borrow_and_update().keysets.len();

        // The seeded keyset is older than this one second interval, so the
        // first (immediate) tick rotates it. Keep the shutdown sender alive so
        // the loop is not asked to stop.
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _handle = signatory.spawn_auto_rotation(Duration::from_secs(1), shutdown_rx);

        tokio::time::timeout(Duration::from_secs(5), updates.changed())
            .await
            .expect("auto rotation should push within timeout")
            .expect("keyset update");

        let after = updates.borrow_and_update().keysets.len();
        assert!(
            after > before,
            "auto rotation should add a keyset ({after} > {before})"
        );
    }

    /// Build an in-memory signatory with no keysets.
    async fn test_signatory(seed: &[u8]) -> Arc<DbSignatory> {
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        Arc::new(
            DbSignatory::new(store, seed, Default::default(), Default::default())
                .await
                .expect("DbSignatory::new"),
        )
    }

    /// Return the single active keyset for `unit`, panicking if there isn't one.
    async fn active_keyset(sig: &DbSignatory, unit: &CurrencyUnit) -> SignatoryKeySet {
        sig.keysets()
            .await
            .expect("keysets")
            .keysets
            .into_iter()
            .find(|k| k.active && &k.unit == unit)
            .expect("active keyset for unit")
    }

    /// Seed an active keyset for `unit` whose `valid_from` is backdated by
    /// `age` seconds, so `rotate_aged_keysets` treats it as due for any
    /// interval below `age`. Returns the seeded keyset id.
    async fn seed_aged_keyset(
        sig: &DbSignatory,
        unit: CurrencyUnit,
        amounts: &[u64],
        input_fee_ppk: u64,
        version: cdk_common::nut02::KeySetVersion,
        final_expiry: Option<u64>,
        age: u64,
    ) -> Id {
        let derivation_path = derivation_path_from_unit(unit.clone(), 1).expect("derivation path");
        let (keyset, mut info) = create_new_keyset(
            &sig.inner.secp_ctx,
            sig.inner.xpriv,
            derivation_path,
            Some(1),
            unit.clone(),
            amounts,
            input_fee_ppk,
            final_expiry,
            version,
        );
        // Backdate the keyset so it reads as aged without waiting.
        info.valid_from = unix_time() - age;
        let id = keyset.id;

        let mut tx = sig
            .inner
            .localstore
            .begin_transaction()
            .await
            .expect("begin tx");
        tx.add_keyset_info(info).await.expect("add keyset info");
        tx.set_active_keyset(unit, id)
            .await
            .expect("set active keyset");
        tx.commit().await.expect("commit");
        sig.inner.load_keys_from_db().await.expect("reload");

        id
    }

    /// Backdate the active keyset for `unit` by `age` seconds so the next
    /// `rotate_aged_keysets` treats it as due, then reload. Used to trigger a
    /// second rotation after the first one produced a fresh (age zero) keyset.
    async fn age_active_keyset(sig: &DbSignatory, unit: &CurrencyUnit, age: u64) {
        let active_id = sig
            .inner
            .localstore
            .get_active_keyset_id(unit)
            .await
            .expect("active keyset id")
            .expect("active keyset for unit");
        let mut info = sig
            .inner
            .localstore
            .get_keyset_info(&active_id)
            .await
            .expect("keyset info")
            .expect("keyset info present");
        info.valid_from = unix_time() - age;

        let mut tx = sig
            .inner
            .localstore
            .begin_transaction()
            .await
            .expect("begin tx");
        tx.add_keyset_info(info).await.expect("update keyset info");
        tx.commit().await.expect("commit");
        sig.inner.load_keys_from_db().await.expect("reload");
    }

    async fn assert_rotation_preserves_metadata(version: cdk_common::nut02::KeySetVersion) {
        let sig = test_signatory(b"test-seed-preserve").await;
        let amounts = vec![1, 2, 4, 8, 16];
        let fee = 100;

        seed_aged_keyset(&sig, CurrencyUnit::Sat, &amounts, fee, version, None, 120).await;
        let original = active_keyset(&sig, &CurrencyUnit::Sat).await;

        sig.rotate_aged_keysets(Duration::from_secs(60))
            .await
            .expect("rotate_aged_keysets");

        let rotated = active_keyset(&sig, &CurrencyUnit::Sat).await;
        assert_ne!(rotated.id, original.id, "a new keyset must be created");
        assert_eq!(rotated.amounts, amounts, "amounts must be preserved");
        assert_eq!(rotated.input_fee_ppk, fee, "input fee must be preserved");
        assert_eq!(
            rotated.final_expiry, None,
            "a keyset without a final_expiry rotates into one without a final_expiry"
        );
        assert_eq!(
            rotated.id.get_version(),
            version,
            "keyset id version must be preserved"
        );
        assert_eq!(
            rotated.version,
            original.version + 1,
            "derivation index must increment on rotation"
        );
    }

    #[tokio::test]
    async fn auto_rotation_preserves_amounts_fee_and_version_v1() {
        assert_rotation_preserves_metadata(cdk_common::nut02::KeySetVersion::Version00).await;
    }

    #[tokio::test]
    async fn auto_rotation_preserves_amounts_fee_and_version_v2() {
        assert_rotation_preserves_metadata(cdk_common::nut02::KeySetVersion::Version01).await;
    }

    #[tokio::test]
    async fn rotate_aged_keysets_pushes_final_expiry_forward() {
        let sig = test_signatory(b"test-seed-final-expiry").await;
        let age = 100;
        // Far enough in the future that the keyset is not treated as expired.
        let expiry = unix_time() + 10_000;

        seed_aged_keyset(
            &sig,
            CurrencyUnit::Sat,
            &[1, 2, 4, 8],
            0,
            cdk_common::nut02::KeySetVersion::Version00,
            Some(expiry),
            age,
        )
        .await;

        sig.rotate_aged_keysets(Duration::from_secs(60))
            .await
            .expect("rotate_aged_keysets");

        let rotated = active_keyset(&sig, &CurrencyUnit::Sat).await;
        let bumped = rotated
            .final_expiry
            .expect("rotated keyset carries a final_expiry");
        // The new expiry is the old one pushed forward by the active age, which
        // is at least `age`. Use `>=` since the clock may tick during the test.
        assert!(
            bumped >= expiry + age,
            "final_expiry must be pushed forward by the active age (got {bumped}, expected >= {})",
            expiry + age
        );
    }

    #[tokio::test]
    async fn rotate_aged_keysets_noop_without_active_keysets() {
        let sig = test_signatory(b"test-seed-noop").await;

        let before = sig.keysets().await.expect("keysets").keysets.len();
        sig.rotate_aged_keysets(Duration::ZERO)
            .await
            .expect("rotate_aged_keysets");
        let after = sig.keysets().await.expect("keysets").keysets.len();

        assert_eq!(before, 0, "fresh signatory has no keysets");
        assert_eq!(after, before, "no active keysets means nothing to rotate");
    }

    #[tokio::test]
    async fn rotate_aged_keysets_ignores_inactive_keysets() {
        let sig = test_signatory(b"test-seed-inactive").await;

        seed_aged_keyset(
            &sig,
            CurrencyUnit::Sat,
            &[1, 2, 4, 8],
            0,
            cdk_common::nut02::KeySetVersion::Version00,
            None,
            120,
        )
        .await;

        let total = |ks: &SignatoryKeysets| ks.keysets.len();
        let active_sat = |ks: &SignatoryKeysets| {
            ks.keysets
                .iter()
                .filter(|k| k.active && k.unit == CurrencyUnit::Sat)
                .count()
        };

        let after_seed = sig.keysets().await.expect("keysets");
        assert_eq!(total(&after_seed), 1);
        assert_eq!(active_sat(&after_seed), 1);

        sig.rotate_aged_keysets(Duration::from_secs(60))
            .await
            .expect("rotate_aged_keysets");
        let after_first = sig.keysets().await.expect("keysets");
        assert_eq!(
            total(&after_first),
            2,
            "one rotation adds exactly one keyset"
        );
        assert_eq!(active_sat(&after_first), 1, "exactly one active Sat keyset");

        // The first rotation left a fresh active keyset (age zero). Age it so
        // the next pass finds it due. The keyset the first pass retired stays
        // inactive and aged, so if inactive keysets were re-rotated the count
        // would grow by more than one.
        age_active_keyset(&sig, &CurrencyUnit::Sat, 120).await;

        sig.rotate_aged_keysets(Duration::from_secs(60))
            .await
            .expect("rotate_aged_keysets");
        let after_second = sig.keysets().await.expect("keysets");
        assert_eq!(
            total(&after_second),
            3,
            "second rotation adds exactly one more; inactive keysets are not re-rotated"
        );
        assert_eq!(
            active_sat(&after_second),
            1,
            "still exactly one active Sat keyset"
        );
    }

    #[tokio::test]
    async fn rotate_aged_keysets_rotates_all_aged_units() {
        let sig = test_signatory(b"test-seed-multi-unit").await;

        let sat = seed_aged_keyset(
            &sig,
            CurrencyUnit::Sat,
            &[1, 2, 4, 8],
            0,
            cdk_common::nut02::KeySetVersion::Version00,
            None,
            120,
        )
        .await;
        let usd = seed_aged_keyset(
            &sig,
            CurrencyUnit::Usd,
            &[1, 2, 4, 8],
            0,
            cdk_common::nut02::KeySetVersion::Version00,
            None,
            120,
        )
        .await;

        sig.rotate_aged_keysets(Duration::from_secs(60))
            .await
            .expect("rotate_aged_keysets");

        let new_sat = active_keyset(&sig, &CurrencyUnit::Sat).await;
        let new_usd = active_keyset(&sig, &CurrencyUnit::Usd).await;
        assert_ne!(new_sat.id, sat, "Sat keyset should rotate");
        assert_ne!(new_usd.id, usd, "Usd keyset should rotate");
    }

    #[tokio::test]
    async fn spawn_auto_rotation_stops_when_signatory_dropped() {
        let sig = test_signatory(b"test-seed-drop").await;
        // Keep the shutdown sender alive so the loop can only exit through the
        // dropped-signatory path, not a shutdown signal.
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = sig.spawn_auto_rotation(Duration::from_millis(50), shutdown_rx);

        // Drop the only strong reference; the task's weak upgrade then fails and
        // the loop exits on its next tick.
        drop(sig);

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("auto rotation task should stop after the signatory is dropped")
            .expect("task should not panic");
    }

    #[tokio::test]
    async fn spawn_auto_rotation_stops_on_shutdown_signal() {
        // The signatory stays alive; only the shutdown signal ends the loop.
        let sig = test_signatory(b"test-seed-shutdown").await;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = sig.spawn_auto_rotation(Duration::from_millis(50), shutdown_rx);

        shutdown_tx.send(true).expect("receiver alive");

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("auto rotation task should stop after shutdown is signalled")
            .expect("task should not panic");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_rotations_all_land_in_memory() {
        // Consistency guard for concurrent rotations: several run at once on
        // distinct units (as auto-rotation and a mint-initiated rotate might).
        // `rotate_keyset` serializes them through `rotation_lock`, commits each
        // to the DB, and updates memory in place. After they settle, the
        // in-memory map and the published watch snapshot must both equal the
        // full DB set, with no keyset lost to a race.
        let sig = test_signatory(b"test-seed-concurrent-rotations").await;

        // Distinct units so each rotation creates its own keyset without
        // contending on a shared unit's derivation index.
        let units: Vec<CurrencyUnit> = (0..12)
            .map(|i| CurrencyUnit::Custom(format!("UNIT{i}")))
            .collect();

        let mut handles = Vec::new();
        for unit in units.iter().cloned() {
            let sig = Arc::clone(&sig);
            handles.push(tokio::spawn(async move {
                sig.rotate_keyset(RotateKeyArguments {
                    unit,
                    amounts: vec![1, 2, 4, 8],
                    input_fee_ppk: 0,
                    keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                    final_expiry: None,
                })
                .await
                .expect("rotate_keyset");
            }));
        }
        for handle in handles {
            handle.await.expect("rotation task should not panic");
        }

        let db_ids: HashSet<Id> = sig
            .inner
            .localstore
            .get_keyset_infos()
            .await
            .expect("keyset infos")
            .into_iter()
            .map(|info| info.id)
            .collect();
        assert_eq!(
            db_ids.len(),
            units.len(),
            "every rotation committed a keyset"
        );

        let memory_ids: HashSet<Id> = sig
            .keysets()
            .await
            .expect("keysets")
            .keysets
            .into_iter()
            .map(|k| k.id)
            .collect();
        assert_eq!(
            memory_ids, db_ids,
            "in-memory keysets must match the DB after concurrent rotations"
        );

        let watch_ids: HashSet<Id> = sig
            .subscribe_keysets()
            .await
            .expect("subscribe")
            .borrow()
            .keysets
            .iter()
            .map(|k| k.id)
            .collect();
        assert_eq!(
            watch_ids, db_ids,
            "published keyset snapshot must match the DB after concurrent rotations"
        );
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
