use std::collections::HashMap;

use cdk_common::amount::{FeeAndAmounts, KeysetFeeAndAmounts};
use cdk_common::wallet::KeysetLoadPolicy;
use tracing::instrument;

use crate::nuts::{Id, KeySet, KeySetInfo, Proofs, Token};
use crate::{Error, Wallet};

impl Wallet {
    /// Get all keysets for this wallet's unit.
    ///
    /// Tries a fresh fetch from the mint. On failure, falls back to any
    /// cached/persisted data. Only hard-fails when the cache is empty
    /// (first-ever fetch).
    #[instrument(skip(self))]
    pub async fn keysets(&self, policy: KeysetLoadPolicy) -> Result<Vec<KeySet>, Error> {
        let metadata = match policy {
            KeysetLoadPolicy::CacheOnly => {
                self.metadata_cache.load_cached(&self.localstore).await?
            }
            KeysetLoadPolicy::CacheThenNetwork => {
                self.metadata_cache
                    .load(&self.localstore, &self.client)
                    .await?
            }
            KeysetLoadPolicy::Refresh => {
                match self
                    .metadata_cache
                    .load_from_mint(&self.localstore, &self.client)
                    .await
                {
                    Ok(m) => m,
                    Err(e) => match self.metadata_cache.get_cached() {
                        Some(m) => {
                            tracing::warn!(
                                "Failed to refresh keysets from mint: {}. Using cached data.",
                                e
                            );
                            m
                        }
                        None => return Err(e),
                    },
                }
            }
        };

        let keysets: Vec<_> = metadata
            .keysets
            .values()
            .filter(|ks| ks.unit == self.unit)
            .filter_map(|ks| {
                let keys = metadata.keys.get(&ks.id)?;
                Some(KeySet {
                    id: ks.id,
                    unit: ks.unit.clone(),
                    active: Some(ks.active),
                    keys: (**keys).clone(),
                    input_fee_ppk: ks.input_fee_ppk,
                    final_expiry: ks.final_expiry,
                })
            })
            .collect();

        if keysets.is_empty() {
            Err(Error::UnknownKeySet)
        } else {
            Ok(keysets)
        }
    }

    /// Get all keysets for this wallet's unit, refreshing once from the mint
    /// when none of them can still carry new outputs.
    ///
    /// A cached active keyset that has reached its final expiry leaves nothing
    /// to build outputs from, and by then the mint has usually rotated to a
    /// replacement. Only [`KeysetLoadPolicy::CacheThenNetwork`] refreshes:
    /// `CacheOnly` must stay offline, and `Refresh` already asked the mint.
    ///
    /// A mint with no replacement yet would otherwise be polled on every
    /// operation, so the refresh is capped at one request per minimum refetch
    /// interval. The load above counts as that request when it found the cache
    /// stale and went to the mint itself.
    pub(crate) async fn keysets_for_outputs(
        &self,
        policy: KeysetLoadPolicy,
    ) -> Result<Vec<KeySet>, Error> {
        let keysets = self.keysets(policy).await?;

        if policy != KeysetLoadPolicy::CacheThenNetwork || keysets.iter().any(usable_for_outputs) {
            return Ok(keysets);
        }

        if self.metadata_cache.asked_mint_recently() {
            tracing::debug!(
                "No unexpired active keyset, but the mint was just asked; not refreshing"
            );
            return Ok(keysets);
        }

        tracing::info!("No unexpired active keyset cached, refreshing from the mint");
        self.keysets(KeysetLoadPolicy::Refresh).await
    }

    /// Get the cheapest keyset the mint will still sign with.
    ///
    /// Filters the output of [`keysets()`](Self::keysets) for active, unexpired
    /// keysets and returns the one with the minimum `input_fee_ppk`.
    #[instrument(skip(self))]
    pub async fn active_keyset(&self) -> Result<KeySet, Error> {
        self.active_keyset_with_policy(Default::default()).await
    }

    /// Get the active keyset using a specific [`KeysetLoadPolicy`].
    ///
    /// Same as [`active_keyset()`](Self::active_keyset) but lets callers
    /// control whether the network may be contacted. When the cached keysets
    /// hold no candidate and the policy allows it, the mint is asked once for
    /// a rotation before giving up.
    #[instrument(skip(self))]
    pub async fn active_keyset_with_policy(
        &self,
        policy: KeysetLoadPolicy,
    ) -> Result<KeySet, Error> {
        self.keysets_for_outputs(policy)
            .await?
            .into_iter()
            .filter(usable_for_outputs)
            .min_by_key(|k| k.input_fee_ppk)
            .ok_or(Error::NoActiveKeyset)
    }

    /// The id [`active_keyset_with_policy()`](Self::active_keyset_with_policy)
    /// would pick under `policy`, without the refresh fallback.
    ///
    /// Used to compare the keyset before and after a refresh, so a load
    /// failure is just "no candidate" rather than an error to propagate.
    async fn cheapest_usable_keyset_id(&self, policy: KeysetLoadPolicy) -> Option<Id> {
        match self.keysets(policy).await {
            Ok(keysets) => keysets
                .into_iter()
                .filter(usable_for_outputs)
                .min_by_key(|k| k.input_fee_ppk)
                .map(|k| k.id),
            Err(e) => {
                tracing::debug!("Could not load keysets with {:?}: {}", policy, e);
                None
            }
        }
    }

    /// Run an operation and retry once if the mint rejects it with
    /// [`Error::InactiveKeyset`], refreshing the keyset cache before the
    /// second attempt.
    ///
    /// Use this around any code path that selects the active keyset, builds
    /// outputs, and posts them to the mint.
    pub(crate) async fn retry_on_inactive_keyset<F, Fut, T>(&self, f: F) -> Result<T, Error>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, Error>>,
    {
        match f().await {
            Err(Error::InactiveKeyset) => {
                tracing::info!(
                    "Mint rejected outputs with inactive keyset, refreshing keysets and retrying"
                );

                let old_active = self
                    .cheapest_usable_keyset_id(KeysetLoadPolicy::CacheOnly)
                    .await;

                let new_active = self
                    .cheapest_usable_keyset_id(KeysetLoadPolicy::Refresh)
                    .await;

                if new_active.is_some() && new_active != old_active {
                    tracing::info!(
                        "Active keyset changed from {:?} to {:?}, retrying operation",
                        old_active,
                        new_active
                    );
                    f().await
                } else {
                    tracing::warn!("No new active keyset found after refresh, not retrying");
                    Err(Error::InactiveKeyset)
                }
            }
            other => other,
        }
    }

    /// Get a single keyset by ID.
    #[instrument(skip(self))]
    pub async fn keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        self.keyset_with_policy(keyset_id, Default::default()).await
    }

    /// Get a single keyset by ID using a specific [`KeysetLoadPolicy`].
    #[instrument(skip(self))]
    pub async fn keyset_with_policy(
        &self,
        keyset_id: Id,
        policy: KeysetLoadPolicy,
    ) -> Result<KeySet, Error> {
        self.keysets(policy)
            .await?
            .into_iter()
            .find(|k| k.id == keyset_id)
            .ok_or(Error::UnknownKeySet)
    }

    /// Decode proofs from a token using all keysets for this mint.
    ///
    /// Loads both active and inactive keysets so that proofs minted under
    /// any keyset can be decoded and redeemed. Uses keyset info directly
    /// from metadata (not full key material), so keysets whose keys were
    /// not persisted can still be resolved.
    #[instrument(skip(self, token))]
    pub(crate) async fn token_proofs(&self, token: &Token) -> Result<Proofs, Error> {
        let metadata = self
            .metadata_cache
            .load(&self.localstore, &self.client)
            .await?;

        let keysets: Vec<KeySetInfo> = metadata
            .keysets
            .values()
            .filter(|ks| ks.unit == self.unit)
            .map(|ks| (**ks).clone())
            .collect();

        if keysets.is_empty() {
            return Err(Error::UnknownKeySet);
        }

        Ok(token.proofs(&keysets)?)
    }

    /// Get keyset fees and amounts for all keysets
    pub async fn get_keyset_fees_and_amounts(&self) -> Result<KeysetFeeAndAmounts, Error> {
        self.get_keyset_fees_and_amounts_with_policy(Default::default())
            .await
    }

    /// Same as [`get_keyset_fees_and_amounts()`](Self::get_keyset_fees_and_amounts)
    /// but lets callers control the [`KeysetLoadPolicy`].
    pub async fn get_keyset_fees_and_amounts_with_policy(
        &self,
        policy: KeysetLoadPolicy,
    ) -> Result<KeysetFeeAndAmounts, Error> {
        let all = self.keysets(policy).await?;
        let mut fees = HashMap::new();
        for ks in &all {
            fees.insert(
                ks.id,
                (
                    ks.input_fee_ppk,
                    ks.keys
                        .iter()
                        .map(|(amount, _)| amount.to_u64())
                        .collect::<Vec<_>>(),
                )
                    .into(),
            );
        }
        Ok(fees)
    }

    /// Get the input fee rate for a specific keyset ID
    pub async fn get_keyset_fees_by_id(&self, keyset_id: Id) -> Result<u64, Error> {
        Ok(self
            .get_keyset_fees_and_amounts_by_id(keyset_id)
            .await?
            .fee())
    }

    /// Get keyset fees and amounts for a specific keyset ID
    pub async fn get_keyset_fees_and_amounts_by_id(
        &self,
        keyset_id: Id,
    ) -> Result<FeeAndAmounts, Error> {
        self.get_keyset_fees_and_amounts_by_id_with_policy(keyset_id, Default::default())
            .await
    }

    /// Get keyset fees and amounts for a specific keyset ID using a specific
    /// [`KeysetLoadPolicy`].
    pub async fn get_keyset_fees_and_amounts_by_id_with_policy(
        &self,
        keyset_id: Id,
        policy: KeysetLoadPolicy,
    ) -> Result<FeeAndAmounts, Error> {
        self.get_keyset_fees_and_amounts_with_policy(policy)
            .await?
            .get(&keyset_id)
            .cloned()
            .ok_or(Error::UnknownKeySet)
    }
}

/// Whether the mint would still sign new outputs under this keyset.
pub(crate) fn usable_for_outputs(keyset: &KeySet) -> bool {
    keyset.active.unwrap_or(false) && !keyset.is_expired()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use cdk_common::util::unix_time;

    use super::*;
    use crate::nuts::{CurrencyUnit, Token};
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock, make_inactive_keyset, test_keyset,
        test_mint_url, test_proof, MockMintConnector,
    };

    #[tokio::test]
    async fn token_proofs_decodes_inactive_keyset_proofs() {
        let active_ks = test_keyset();
        let active_id = active_ks.id;

        let inactive_ks = make_inactive_keyset();
        let inactive_id = inactive_ks.id;

        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None)
            .await
            .expect("mint should be stored");

        let mock = MockMintConnector::new();
        mock.set_mint_keys_response(Ok(vec![active_ks.clone(), inactive_ks.clone()]));
        let mock = Arc::new(mock);

        let wallet = create_test_wallet_with_mock(db, mock).await;

        let token = Token::new(
            mint_url,
            vec![test_proof(inactive_id, 1)],
            None,
            CurrencyUnit::Sat,
        );

        let active_keysets: Vec<KeySetInfo> = wallet
            .keysets(Default::default())
            .await
            .expect("keysets should load")
            .into_iter()
            .filter(|k| k.active.unwrap_or(false))
            .map(|ks| KeySetInfo {
                id: ks.id,
                unit: ks.unit,
                active: ks.active.unwrap_or(true),
                input_fee_ppk: ks.input_fee_ppk,
                final_expiry: ks.final_expiry,
            })
            .collect();
        assert_eq!(active_keysets.len(), 1);
        assert_eq!(active_keysets[0].id, active_id);
        assert!(
            token.proofs(&active_keysets).is_err(),
            "active-only keysets should not decode an inactive keyset proof"
        );

        let proofs = wallet
            .token_proofs(&token)
            .await
            .expect("all keysets should decode an inactive keyset proof");
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].keyset_id, inactive_id);
    }

    /// Verify that receiving a token with proofs from an inactive keyset
    /// succeeds through the full receive preparation phase: token decoding,
    /// fee calculation, and active keyset selection for swap outputs.
    #[tokio::test]
    async fn receive_prepares_swap_for_inactive_keyset_proofs() {
        use crate::wallet::receive::saga::ReceiveSaga;
        use crate::wallet::ReceiveOptions;

        let inactive_ks = make_inactive_keyset();
        let inactive_id = inactive_ks.id;

        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None)
            .await
            .expect("mint should be stored");

        let mock = MockMintConnector::new();
        mock.set_mint_keys_response(Ok(vec![test_keyset(), inactive_ks]));
        let mock = Arc::new(mock);

        let wallet = create_test_wallet_with_mock(db, mock).await;

        // Build proofs as if they came from the inactive keyset
        let proofs = vec![test_proof(inactive_id, 2), test_proof(inactive_id, 1)];

        let saga = ReceiveSaga::new(&wallet);
        // Prepare decodes proofs, looks up the inactive keyset for DLEQ
        // verification, calculates fees, and selects the active keyset for
        // swap outputs. If inactive keysets are not properly loaded, this
        // call would fail with UnknownKeySet.
        let _prepared = saga
            .prepare(proofs, ReceiveOptions::default(), None, None)
            .await
            .expect("prepare should succeed with inactive keyset proofs");
    }

    /// When the active keyset changes after refresh, retry_on_inactive_keyset
    /// should retry and return the second attempt's result.
    #[tokio::test]
    async fn retry_on_inactive_keyset_retries_when_keyset_changes() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None).await.unwrap();

        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;

        // Prime the cache with the original keyset
        wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

        // Build a new keyset that will become the active one after rotation
        let new_keyset = make_inactive_keyset();
        let mut rotated = new_keyset;
        rotated.active = Some(true);

        // Deactivate the old keyset
        let mut old = test_keyset();
        old.active = Some(false);

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();
        let mock_clone = mock.clone();

        let result = wallet
            .retry_on_inactive_keyset(|| {
                let call_count = call_count_clone.clone();
                let mock = mock_clone.clone();
                let old = old.clone();
                let rotated = rotated.clone();
                async move {
                    let n = call_count.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        // Before the retry, rotate the keysets on the mock
                        // so the refresh picks up the new active keyset.
                        mock.set_mint_keys_response(Ok(vec![old, rotated]));
                        Err(Error::InactiveKeyset)
                    } else {
                        Ok(42u32)
                    }
                }
            })
            .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    /// When the active keyset does NOT change after refresh,
    /// retry_on_inactive_keyset should not retry.
    #[tokio::test]
    async fn retry_on_inactive_keyset_no_retry_when_keyset_unchanged() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None).await.unwrap();

        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock).await;

        // Prime the cache
        wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let result: Result<u32, Error> = wallet
            .retry_on_inactive_keyset(|| {
                let call_count = call_count_clone.clone();
                async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    Err(Error::InactiveKeyset)
                }
            })
            .await;

        assert!(matches!(result, Err(Error::InactiveKeyset)));
        // Should only be called once — no retry since keyset didn't change
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    /// When the operation succeeds on the first try, no retry logic runs.
    #[tokio::test]
    async fn retry_on_inactive_keyset_no_retry_on_success() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None).await.unwrap();

        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock).await;

        // Prime the cache
        wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let result = wallet
            .retry_on_inactive_keyset(|| {
                let call_count = call_count_clone.clone();
                async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    Ok(99u32)
                }
            })
            .await;

        assert_eq!(result.unwrap(), 99);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    /// A cached active keyset that has reached its final expiry must not leave
    /// the wallet stuck: the mint has rotated to a replacement, and
    /// `CacheThenNetwork` should go and find it even with the TTL disabled.
    #[tokio::test]
    async fn active_keyset_refreshes_when_cached_active_keyset_expired() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None).await.unwrap();

        let mut expired = test_keyset();
        expired.final_expiry = Some(unix_time() - 60);

        let mock = Arc::new(MockMintConnector::new());
        mock.set_mint_keys_response(Ok(vec![expired.clone()]));
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;

        wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();
        assert!(
            matches!(wallet.active_keyset().await, Err(Error::NoActiveKeyset)),
            "the mint has no replacement yet, so the refresh finds nothing either"
        );

        wallet.set_metadata_cache_ttl(None);
        wallet
            .metadata_cache
            .set_min_refetch_interval(Duration::ZERO);

        let mut rotated = make_inactive_keyset();
        rotated.active = Some(true);
        let rotated_id = rotated.id;
        expired.active = Some(false);
        mock.set_mint_keys_response(Ok(vec![expired, rotated]));

        let active = wallet
            .active_keyset()
            .await
            .expect("the rotated keyset should be picked up");
        assert_eq!(active.id, rotated_id);
    }

    /// `CacheOnly` must stay offline, so an expired cached keyset still fails
    /// locally even once the mint has rotated.
    #[tokio::test]
    async fn active_keyset_cache_only_does_not_refresh_when_expired() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None).await.unwrap();

        let mut expired = test_keyset();
        expired.final_expiry = Some(unix_time() - 60);

        let mock = Arc::new(MockMintConnector::new());
        mock.set_mint_keys_response(Ok(vec![expired.clone()]));
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;

        wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();
        wallet.set_metadata_cache_ttl(None);

        let mut rotated = make_inactive_keyset();
        rotated.active = Some(true);
        expired.active = Some(false);
        mock.set_mint_keys_response(Ok(vec![expired, rotated]));

        let result = wallet
            .active_keyset_with_policy(KeysetLoadPolicy::CacheOnly)
            .await;
        assert!(matches!(result, Err(Error::NoActiveKeyset)));
    }

    /// A mint that has no replacement keyset yet must not be polled on every
    /// call: once it has just been asked, the expiry fallback waits for the
    /// minimum refetch interval before asking again.
    #[tokio::test]
    async fn active_keyset_does_not_poll_mint_while_refetch_is_throttled() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None).await.unwrap();

        let mut expired = test_keyset();
        expired.final_expiry = Some(unix_time() - 60);

        let mock = Arc::new(MockMintConnector::new());
        mock.set_mint_keys_response(Ok(vec![expired.clone()]));
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;

        wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();
        wallet.set_metadata_cache_ttl(None);

        let mut rotated = make_inactive_keyset();
        rotated.active = Some(true);
        let rotated_id = rotated.id;
        expired.active = Some(false);
        mock.set_mint_keys_response(Ok(vec![expired, rotated]));

        assert!(
            matches!(wallet.active_keyset().await, Err(Error::NoActiveKeyset)),
            "the mint was just asked, so the rotation should not be fetched yet"
        );

        wallet
            .metadata_cache
            .set_min_refetch_interval(Duration::ZERO);

        let active = wallet
            .active_keyset()
            .await
            .expect("once the interval has passed the rotation is picked up");
        assert_eq!(active.id, rotated_id);
    }

    /// When the in-memory cache's TTL has expired, `CacheThenNetwork` should
    /// fetch fresh data from the mint — not return stale data from the database.
    #[tokio::test]
    async fn load_fetches_from_mint_when_ttl_expired() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None).await.unwrap();

        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;

        // Prime cache + DB with the default keyset (fee = 101)
        let initial = wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();
        let initial_fee = initial[0].input_fee_ppk;
        assert_eq!(initial_fee, 101);

        // Expire the cache immediately
        wallet.set_metadata_cache_ttl(Some(Duration::ZERO));

        // Update the mock to return a keyset with a different fee
        let mut updated_keyset = test_keyset();
        updated_keyset.input_fee_ppk = 999;
        mock.set_mint_keys_response(Ok(vec![updated_keyset]));

        // CacheThenNetwork should fetch from mint since TTL expired
        let refreshed = wallet
            .keysets(KeysetLoadPolicy::CacheThenNetwork)
            .await
            .unwrap();
        assert_eq!(
            refreshed[0].input_fee_ppk, 999,
            "expected fresh data from mint, got stale cache/db data"
        );
    }
}
