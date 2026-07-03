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

    /// Get the active keyset with the lowest fees.
    ///
    /// Filters the output of [`keysets()`](Self::keysets) for active keysets
    /// and returns the one with the minimum `input_fee_ppk`.
    #[instrument(skip(self))]
    pub async fn active_keyset(&self) -> Result<KeySet, Error> {
        self.active_keyset_with_policy(Default::default()).await
    }

    /// Get the active keyset using a specific [`KeysetLoadPolicy`].
    ///
    /// Same as [`active_keyset()`](Self::active_keyset) but lets callers
    /// control whether the network may be contacted.
    #[instrument(skip(self))]
    pub async fn active_keyset_with_policy(
        &self,
        policy: KeysetLoadPolicy,
    ) -> Result<KeySet, Error> {
        self.keysets(policy)
            .await?
            .into_iter()
            .filter(|k| k.active.unwrap_or(false))
            .min_by_key(|k| k.input_fee_ppk)
            .ok_or(Error::NoActiveKeyset)
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
                    .keysets(KeysetLoadPolicy::CacheOnly)
                    .await
                    .ok()
                    .and_then(|ks| {
                        ks.into_iter()
                            .filter(|k| k.active.unwrap_or(false))
                            .min_by_key(|k| k.input_fee_ppk)
                            .map(|k| k.id)
                    });

                let new_active =
                    self.keysets(KeysetLoadPolicy::Refresh)
                        .await
                        .ok()
                        .and_then(|ks| {
                            ks.into_iter()
                                .filter(|k| k.active.unwrap_or(false))
                                .min_by_key(|k| k.input_fee_ppk)
                                .map(|k| k.id)
                        });

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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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
