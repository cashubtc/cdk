use std::collections::{HashMap, HashSet};

use cdk_common::amount::{FeeAndAmounts, KeysetFeeAndAmounts};
use cdk_common::wallet::KeysetLoadPolicy;
use tracing::instrument;

use crate::nuts::{Id, KeySet, KeySetInfo, Proofs, Token};
use crate::{ensure_cdk, Error, Wallet};

impl Wallet {
    /// Ids of this mint's keysets that have passed their `final_expiry`.
    ///
    /// Reads the cached metadata only, so proof listing never triggers network
    /// I/O. A wallet that has never cached keyset metadata reports no expired
    /// keysets: it cannot know that any keyset expired, and failing here would
    /// break balance queries on a fresh wallet.
    pub(crate) async fn expired_keyset_ids(&self) -> Result<HashSet<Id>, Error> {
        let metadata = match self.metadata_cache.load_cached(&self.localstore).await {
            Ok(metadata) => metadata,
            Err(Error::UnknownKeySet) => return Ok(HashSet::new()),
            Err(err) => return Err(err),
        };

        Ok(metadata
            .keysets
            .values()
            .filter(|keyset| keyset.is_expired())
            .map(|keyset| keyset.id)
            .collect())
    }

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

    /// Get the active keyset with the lowest fees.
    ///
    /// Filters the output of [`keysets()`](Self::keysets) for active keysets
    /// that have not expired and returns the one with the minimum
    /// `input_fee_ppk`. Expired keysets are skipped because the mint rejects
    /// outputs signed by them.
    ///
    /// Use this only where the mint has to sign something. A send that hands
    /// over proofs the wallet already holds falls back to the crate-private
    /// `split_basis_keyset` instead.
    #[instrument(skip(self))]
    pub async fn active_keyset(&self) -> Result<KeySet, Error> {
        self.active_keyset_with_policy(Default::default()).await
    }

    /// Get the active keyset using a specific [`KeysetLoadPolicy`].
    ///
    /// Same as [`active_keyset()`](Self::active_keyset) but lets callers
    /// control whether the network may be contacted. A cached verdict of no
    /// signable keyset is confirmed against the mint once, since the cache may
    /// predate a rotation.
    ///
    /// Reports [`Error::ExpiredKeyset`] rather than
    /// [`Error::NoActiveKeyset`] when the mint has an active keyset and has let
    /// it expire. The two call for different responses: the first means the mint
    /// stopped rotating, the second that this wallet holds no keys for it.
    #[instrument(skip(self))]
    pub async fn active_keyset_with_policy(
        &self,
        policy: KeysetLoadPolicy,
    ) -> Result<KeySet, Error> {
        self.select_keyset(policy, select_active_keyset).await
    }

    /// The fallback basis a send prices against when no keyset can sign
    /// outputs.
    ///
    /// Returns the cheapest unexpired keyset, preferring an active one, so a
    /// send that hands over proofs the wallet already holds does not fail just
    /// because the mint let its active keyset expire. A send that has to swap
    /// does need a signable keyset, and `SendSaga::internal_prepare` refuses
    /// this fallback once the split shows the swap, before any proof is
    /// reserved.
    #[instrument(skip(self))]
    pub(crate) async fn split_basis_keyset(
        &self,
        policy: KeysetLoadPolicy,
    ) -> Result<KeySet, Error> {
        self.select_keyset(policy, select_split_basis_keyset).await
    }

    /// Run a keyset selection again against freshly fetched keysets when the
    /// cached ones yield none the mint would sign on.
    ///
    /// The metadata cache can be configured never to go stale and a mint does
    /// not rotate an expired keyset on its own, so a verdict drawn from cached
    /// `active` flags and expiry timestamps is not final. Only
    /// [`KeysetLoadPolicy::CacheThenNetwork`] escalates, and once:
    /// `CacheOnly` has to stay offline and `Refresh` already fetched.
    async fn select_keyset(
        &self,
        policy: KeysetLoadPolicy,
        select: fn(Vec<KeySet>) -> Result<KeySet, Error>,
    ) -> Result<KeySet, Error> {
        let cached_verdict = match select(self.keysets(policy).await?) {
            Err(verdict @ (Error::ExpiredKeyset | Error::NoActiveKeyset))
                if policy == KeysetLoadPolicy::CacheThenNetwork =>
            {
                verdict
            }
            selected => return selected,
        };

        tracing::info!(
            "Cached keysets hold none the mint would sign on ({}), refreshing",
            cached_verdict
        );

        match self.keysets(KeysetLoadPolicy::Refresh).await {
            Ok(keysets) => select(keysets),
            Err(err) => {
                tracing::warn!(
                    "Failed to refresh keysets, reporting {}: {}",
                    cached_verdict,
                    err
                );
                Err(cached_verdict)
            }
        }
    }

    /// Run an operation and retry once if the mint rejects it with
    /// [`Error::InactiveKeyset`], refreshing the keyset cache before the
    /// second attempt.
    ///
    /// Use this around any code path that selects the active keyset, builds
    /// outputs, and posts them to the mint. Only the mint can tell that a keyset
    /// it still lists as active was deactivated; expiry and a cached inactive
    /// flag are caught by the keyset selection itself, which refreshes before
    /// any output is built.
    ///
    /// Whether to retry is decided by asking
    /// [`active_keyset_with_policy()`](Self::active_keyset_with_policy) before
    /// and after the refresh, so the comparison always names the keyset the
    /// retried operation would itself pick. Selecting it locally here instead
    /// would drift from that rule.
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

                let old_active = match self
                    .active_keyset_with_policy(KeysetLoadPolicy::CacheOnly)
                    .await
                {
                    Ok(keyset) => Some(keyset.id),
                    Err(err) => {
                        tracing::debug!("No usable active keyset cached before refresh: {}", err);
                        None
                    }
                };

                let new_active = match self
                    .active_keyset_with_policy(KeysetLoadPolicy::Refresh)
                    .await
                {
                    Ok(keyset) => Some(keyset.id),
                    Err(err) => {
                        tracing::debug!("No usable active keyset after refresh: {}", err);
                        None
                    }
                };

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

/// Pick the cheapest active keyset the mint would still sign outputs on.
fn select_active_keyset(keysets: Vec<KeySet>) -> Result<KeySet, Error> {
    let active: Vec<KeySet> = keysets
        .into_iter()
        .filter(|k| k.active.unwrap_or(false))
        .collect();

    ensure_cdk!(!active.is_empty(), Error::NoActiveKeyset);

    active
        .into_iter()
        .filter(|k| !k.is_expired())
        .min_by_key(|k| k.input_fee_ppk)
        .ok_or(Error::ExpiredKeyset)
}

/// Pick the keyset a send splits its denominations and input fee against,
/// preferring an unexpired active one over an unexpired inactive one.
fn select_split_basis_keyset(keysets: Vec<KeySet>) -> Result<KeySet, Error> {
    keysets
        .into_iter()
        .filter(|k| !k.is_expired())
        .min_by_key(|k| (!k.active.unwrap_or(false), k.input_fee_ppk))
        .ok_or(Error::ExpiredKeyset)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::nuts::{CurrencyUnit, Token};
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock, make_expired_keyset, make_inactive_keyset,
        test_keyset, test_mint_url, test_proof, MockMintConnector,
    };

    async fn wallet_with_keysets(keysets: Vec<KeySet>) -> Wallet {
        let db = create_test_db().await;
        db.add_mint(test_mint_url(), None)
            .await
            .expect("mint should be stored");

        let mock = MockMintConnector::new();
        mock.set_mint_keys_response(Ok(keysets));

        create_test_wallet_with_mock(db, Arc::new(mock)).await
    }

    /// A wallet whose keyset cache is primed and never goes stale, so a later
    /// change to the mock's answer is visible only to code that refreshes.
    async fn wallet_with_primed_cache(keysets: Vec<KeySet>) -> (Wallet, Arc<MockMintConnector>) {
        let db = create_test_db().await;
        db.add_mint(test_mint_url(), None)
            .await
            .expect("mint should be stored");

        let mock = Arc::new(MockMintConnector::new());
        mock.set_mint_keys_response(Ok(keysets));

        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;
        wallet.set_metadata_cache_ttl(None);
        wallet
            .keysets(KeysetLoadPolicy::Refresh)
            .await
            .expect("keysets should prime the cache");

        (wallet, mock)
    }

    /// The keysets a mint answers with after rotating away from `expired`.
    fn rotation_away_from(expired: &KeySet) -> (Vec<KeySet>, Id) {
        let mut deactivated = expired.clone();
        deactivated.active = Some(false);

        let mut rotated = make_inactive_keyset();
        rotated.active = Some(true);
        let rotated_id = rotated.id;

        (vec![deactivated, rotated], rotated_id)
    }

    #[tokio::test]
    async fn active_keyset_skips_an_expired_keyset() {
        let mut expired = make_expired_keyset();
        expired.active = Some(true);

        let mut unexpired = test_keyset();
        unexpired.input_fee_ppk = expired.input_fee_ppk + 100;

        let wallet = wallet_with_keysets(vec![expired.clone(), unexpired.clone()]).await;

        let keyset = wallet.active_keyset().await.expect("keyset should resolve");

        assert_eq!(
            keyset.id, unexpired.id,
            "the cheaper keyset is expired, so the mint would reject its outputs"
        );
    }

    /// A mint that has an active keyset and let it expire has stopped rotating,
    /// which a caller handles differently from a wallet that holds no keys.
    #[tokio::test]
    async fn active_keyset_reports_expiry_when_every_active_keyset_is_expired() {
        let mut expired = make_expired_keyset();
        expired.active = Some(true);

        let wallet = wallet_with_keysets(vec![expired]).await;

        assert!(matches!(
            wallet.active_keyset().await,
            Err(Error::ExpiredKeyset)
        ));
    }

    #[tokio::test]
    async fn active_keyset_reports_no_active_keyset_when_none_is_active() {
        let wallet = wallet_with_keysets(vec![make_inactive_keyset()]).await;

        assert!(matches!(
            wallet.active_keyset().await,
            Err(Error::NoActiveKeyset)
        ));
    }

    /// The send path splits against denominations, not against a keyset the mint
    /// will sign on, so an expired active keyset must not hide an unexpired
    /// inactive one.
    #[tokio::test]
    async fn split_basis_keyset_falls_back_to_an_unexpired_inactive_keyset() {
        let mut expired = make_expired_keyset();
        expired.active = Some(true);
        let inactive = make_inactive_keyset();

        let wallet = wallet_with_keysets(vec![expired, inactive.clone()]).await;

        let keyset = wallet
            .split_basis_keyset(Default::default())
            .await
            .expect("an unexpired keyset is available");

        assert_eq!(keyset.id, inactive.id);
    }

    /// A swap would use the active keyset's denominations, so the fallback must
    /// stay a fallback even when an inactive keyset is cheaper.
    #[tokio::test]
    async fn split_basis_keyset_prefers_an_active_keyset_over_a_cheaper_inactive_one() {
        let mut active = test_keyset();
        active.input_fee_ppk = 200;
        let mut inactive = make_inactive_keyset();
        inactive.input_fee_ppk = 1;
        // A v2 keyset id commits to input_fee_ppk, so the id has to follow.
        inactive.id = Id::v2_from_data(
            &inactive.keys,
            &inactive.unit,
            inactive.input_fee_ppk,
            inactive.final_expiry,
        );

        let wallet = wallet_with_keysets(vec![inactive, active.clone()]).await;

        let keyset = wallet
            .split_basis_keyset(Default::default())
            .await
            .expect("an unexpired keyset is available");

        assert_eq!(keyset.id, active.id);
    }

    /// The cached verdict that every active keyset expired cannot be final: the
    /// mint does not rotate an expired keyset itself, so an operator rotates
    /// behind it and the wallet has to ask before refusing.
    #[tokio::test]
    async fn active_keyset_refreshes_when_the_cached_active_keyset_expired() {
        let mut expired = make_expired_keyset();
        expired.active = Some(true);

        let (wallet, mock) = wallet_with_primed_cache(vec![expired.clone()]).await;

        let (rotation, rotated_id) = rotation_away_from(&expired);
        mock.set_mint_keys_response(Ok(rotation));

        let cached = wallet
            .keysets(KeysetLoadPolicy::CacheThenNetwork)
            .await
            .expect("the cache is primed");
        assert!(
            cached.iter().all(|k| k.id != rotated_id),
            "the cache has to still be stale for this to test anything"
        );

        let keyset = wallet
            .active_keyset()
            .await
            .expect("the rotation the mint published is reachable");

        assert_eq!(keyset.id, rotated_id);
    }

    /// A cache holding no active keyset at all is stale in the same way.
    #[tokio::test]
    async fn active_keyset_refreshes_when_no_cached_keyset_is_active() {
        let (wallet, mock) = wallet_with_primed_cache(vec![make_inactive_keyset()]).await;

        let rotated = test_keyset();
        mock.set_mint_keys_response(Ok(vec![make_inactive_keyset(), rotated.clone()]));

        let keyset = wallet
            .active_keyset()
            .await
            .expect("the mint has an active keyset");

        assert_eq!(keyset.id, rotated.id);
    }

    /// `CacheOnly` promises no network access, so it reports the cached verdict
    /// even when the mint is already answering with a usable keyset.
    #[tokio::test]
    async fn active_keyset_with_cache_only_policy_stays_offline() {
        let mut expired = make_expired_keyset();
        expired.active = Some(true);

        let (wallet, mock) = wallet_with_primed_cache(vec![expired.clone()]).await;

        let (rotation, _) = rotation_away_from(&expired);
        mock.set_mint_keys_response(Ok(rotation));

        assert!(matches!(
            wallet
                .active_keyset_with_policy(KeysetLoadPolicy::CacheOnly)
                .await,
            Err(Error::ExpiredKeyset)
        ));
    }

    /// A mint that really has only an expired active keyset still reports
    /// expiry, and the refusal is not latched: the next call asks again.
    #[tokio::test]
    async fn active_keyset_reports_expiry_when_the_mint_confirms_it() {
        let mut expired = make_expired_keyset();
        expired.active = Some(true);

        let (wallet, mock) = wallet_with_primed_cache(vec![expired.clone()]).await;

        assert!(matches!(
            wallet.active_keyset().await,
            Err(Error::ExpiredKeyset)
        ));

        let (rotation, rotated_id) = rotation_away_from(&expired);
        mock.set_mint_keys_response(Ok(rotation));

        let keyset = wallet
            .active_keyset()
            .await
            .expect("the refusal must not latch");

        assert_eq!(keyset.id, rotated_id);
    }

    /// The send path's basis is chosen from cached data too, so it needs the
    /// same confirmation once nothing unexpired is left.
    #[tokio::test]
    async fn split_basis_keyset_refreshes_when_every_cached_keyset_expired() {
        let mut expired = make_expired_keyset();
        expired.active = Some(true);

        let (wallet, mock) = wallet_with_primed_cache(vec![expired.clone()]).await;

        let (rotation, rotated_id) = rotation_away_from(&expired);
        mock.set_mint_keys_response(Ok(rotation));

        let keyset = wallet
            .split_basis_keyset(Default::default())
            .await
            .expect("the rotated keyset is an unexpired basis");

        assert_eq!(keyset.id, rotated_id);

        let mut every_keyset_expired = make_expired_keyset();
        every_keyset_expired.active = Some(true);
        let (offline, mock) = wallet_with_primed_cache(vec![every_keyset_expired.clone()]).await;
        let (rotation, _) = rotation_away_from(&every_keyset_expired);
        mock.set_mint_keys_response(Ok(rotation));

        assert!(matches!(
            offline
                .split_basis_keyset(KeysetLoadPolicy::CacheOnly)
                .await,
            Err(Error::ExpiredKeyset)
        ));
    }

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

    /// The retry probe has to name the keyset the retried operation would pick.
    /// Here an active keyset the mint let expire is the cheapest one, so a probe
    /// that ignored expiry would name it both before and after the refresh and
    /// refuse to retry across a real rotation.
    #[tokio::test]
    async fn retry_on_inactive_keyset_retries_when_only_the_unexpired_keyset_rotates() {
        let mut expired = make_expired_keyset();
        expired.active = Some(true);
        expired.input_fee_ppk = 1;
        expired.id = Id::v2_from_data(
            &expired.keys,
            &expired.unit,
            expired.input_fee_ppk,
            expired.final_expiry,
        );

        let old = test_keyset();
        assert!(old.input_fee_ppk > expired.input_fee_ppk);

        let db = create_test_db().await;
        db.add_mint(test_mint_url(), None)
            .await
            .expect("mint should be stored");

        let mock = Arc::new(MockMintConnector::new());
        mock.set_mint_keys_response(Ok(vec![expired.clone(), old.clone()]));
        let wallet = create_test_wallet_with_mock(db, mock.clone()).await;

        wallet
            .keysets(KeysetLoadPolicy::Refresh)
            .await
            .expect("keysets should prime the cache");

        let mut rotated = make_inactive_keyset();
        rotated.active = Some(true);

        let mut deactivated = old;
        deactivated.active = Some(false);

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();
        let mock_clone = mock.clone();

        let result = wallet
            .retry_on_inactive_keyset(|| {
                let call_count = call_count_clone.clone();
                let mock = mock_clone.clone();
                let rotation = vec![expired.clone(), deactivated.clone(), rotated.clone()];
                async move {
                    if call_count.fetch_add(1, Ordering::SeqCst) == 0 {
                        mock.set_mint_keys_response(Ok(rotation));
                        Err(Error::InactiveKeyset)
                    } else {
                        Ok(7u32)
                    }
                }
            })
            .await;

        assert_eq!(result.expect("the retry should succeed"), 7);
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "the unexpired active keyset rotated, so the operation must be retried"
        );
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
        wallet.set_metadata_cache_ttl(Some(std::time::Duration::ZERO));

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
