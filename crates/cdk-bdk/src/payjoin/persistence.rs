#[derive(Debug, Clone)]
pub(super) struct RecordingSessionPersister<E> {
    events: Arc<StdMutex<Vec<E>>>,
    closed: Arc<AtomicBool>,
    /// Whether the session changed since construction; clean sessions do not
    /// need their (potentially PSBT-sized) record rewritten after a poll.
    dirty: Arc<AtomicBool>,
}

impl<E> RecordingSessionPersister<E>
where
    E: Clone,
{
    pub(super) fn new(events: Vec<E>, closed: bool) -> Self {
        Self {
            events: Arc::new(StdMutex::new(events)),
            closed: Arc::new(AtomicBool::new(closed)),
            dirty: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn events(&self) -> Result<Vec<E>, Error> {
        self.events
            .lock()
            .map(|events| events.clone())
            .map_err(|err| Error::Payjoin(format!("Payjoin session lock poisoned: {}", err)))
    }

    pub(super) fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::SeqCst)
    }

    pub(super) fn replace(&self, events: Vec<E>, closed: bool) -> Result<(), Error> {
        *self
            .events
            .lock()
            .map_err(|err| Error::Payjoin(format!("Payjoin session lock poisoned: {}", err)))? =
            events;
        self.closed.store(closed, Ordering::SeqCst);
        self.dirty.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl<E> ::payjoin::persist::SessionPersister for RecordingSessionPersister<E>
where
    E: Clone + Send + Sync + 'static,
{
    type InternalStorageError = Error;
    type SessionEvent = E;

    fn save_event(&self, event: Self::SessionEvent) -> Result<(), Self::InternalStorageError> {
        self.events
            .lock()
            .map_err(|err| Error::Payjoin(format!("Payjoin session lock poisoned: {}", err)))?
            .push(event);
        self.dirty.store(true, Ordering::SeqCst);

        Ok(())
    }

    fn load(
        &self,
    ) -> Result<Box<dyn Iterator<Item = Self::SessionEvent>>, Self::InternalStorageError> {
        let events = self
            .events
            .lock()
            .map(|events| events.clone())
            .map_err(|err| Error::Payjoin(format!("Payjoin session lock poisoned: {}", err)))?;

        Ok(Box::new(events.into_iter()))
    }

    fn close(&self) -> Result<(), Self::InternalStorageError> {
        self.closed.store(true, Ordering::SeqCst);
        self.dirty.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Pre-exposure state for a Payjoin send: everything built and signed before
/// the original PSBT is shared with the receiver. The `Sender` itself is not
/// returned — it is saved into `persister`'s event log, from which the
/// background poller replays it; the persisted events plus the signed original
/// are all the poller needs to drive (and resume) the session.
use super::*;

impl CdkBdk {
    pub(super) fn apply_payjoin_receive_session_progress(
        &self,
        record: &mut crate::storage::PayjoinReceiveSessionRecord,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
        closed: bool,
    ) -> Result<(), Error> {
        record.events = persister.events()?;
        update_payjoin_receive_credit_cap(record);
        record.closed = closed || persister.closed();
        Ok(())
    }

    pub(super) async fn persist_payjoin_receive_session_progress(
        &self,
        record: &mut crate::storage::PayjoinReceiveSessionRecord,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
        closed: bool,
    ) -> Result<(), Error> {
        self.apply_payjoin_receive_session_progress(record, persister, closed)?;
        self.storage.put_payjoin_receive_session(record).await
    }

    /// Evict the cut-through proposal tx reservation, folding any eviction
    /// failure into `err` so neither error is lost.
    /// Persist the current Payjoin send session event log without changing the
    /// poller's terminal `closed` flag (which is set only after the resulting
    /// transaction is staged).
    pub(super) async fn persist_payjoin_send_progress(
        &self,
        intent: &mut SendIntent<intent_state::PayjoinNegotiating>,
        persister: &RecordingSessionPersister<::payjoin::send::v2::SessionEvent>,
    ) -> Result<(), Error> {
        // Skip the rewrite on no-progress polls: the intent record embeds the
        // signed original tx and the full event log.
        if !persister.is_dirty() {
            return Ok(());
        }
        intent
            .update_payjoin_events(&self.storage, persister.events()?)
            .await
    }

    pub(crate) fn payjoin_config(&self) -> Option<&PayjoinConfig> {
        self.payjoin_config.as_ref()
    }

    pub(super) async fn cached_ohttp_keys(
        &self,
        config: &PayjoinConfig,
    ) -> Result<::payjoin::OhttpKeys, Error> {
        if let Some((keys, true)) = self.lookup_ohttp_keys_cache(config).await {
            return Ok(keys);
        }

        // Single-flight: only one task refreshes; the rest re-check under the lock.
        let _fetch_guard = self.payjoin_ohttp_keys_fetch_lock.lock().await;
        let cached = self.lookup_ohttp_keys_cache(config).await;
        if let Some((keys, true)) = cached {
            return Ok(keys);
        }

        match fetch_ohttp_keys_with_timeout(config).await {
            Ok(keys) => {
                self.store_ohttp_keys(config, keys.clone(), crate::util::unix_now())
                    .await;
                Ok(keys)
            }
            // Stale fallback: a previously fetched key set beats failing outright.
            Err(err) => match cached {
                Some((stale_keys, false)) => {
                    tracing::warn!(
                        error = %err,
                        "Could not refresh Payjoin OHTTP keys; using stale cached keys"
                    );
                    Ok(stale_keys)
                }
                _ => Err(err),
            },
        }
    }

    /// The cached key set for `config`, paired with `true` while still fresh.
    async fn lookup_ohttp_keys_cache(
        &self,
        config: &PayjoinConfig,
    ) -> Option<(::payjoin::OhttpKeys, bool)> {
        let now = crate::util::unix_now();
        let cache = self.payjoin_ohttp_keys_cache.lock().await;
        cache.as_ref().and_then(|cached| {
            let config_matches = cached.directory_url == config.directory_url
                && cached.ohttp_relay_url == config.ohttp_relay_url;
            if !config_matches {
                return None;
            }
            let fresh = now.saturating_sub(cached.fetched_at) <= PAYJOIN_OHTTP_KEYS_CACHE_TTL_SECS;
            Some((cached.keys.clone(), fresh))
        })
    }

    async fn store_ohttp_keys(
        &self,
        config: &PayjoinConfig,
        keys: ::payjoin::OhttpKeys,
        fetched_at: u64,
    ) {
        let mut cache = self.payjoin_ohttp_keys_cache.lock().await;
        *cache = Some(crate::PayjoinOhttpKeysCache {
            keys,
            fetched_at,
            directory_url: config.directory_url.clone(),
            ohttp_relay_url: config.ohttp_relay_url.clone(),
        });
    }
}
