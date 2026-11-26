use cdk_common::common::QuoteTTL;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::instrument;

use super::{
    Mint, CDK_MINT_CONFIG_KV_KEY, CDK_MINT_CONFIG_SECONDARY_NAMESPACE, CDK_MINT_PRIMARY_NAMESPACE,
    CDK_MINT_QUOTE_TTL_KV_KEY,
};
use crate::nuts::MintInfo;
use crate::Error;

impl Mint {
    /// Update stored mint info while preserving concurrent changes by other replicas.
    /// The update may be called again with a newer value and must be free of side effects.
    #[instrument(skip_all)]
    pub async fn update_mint_info<F, R>(&self, update: F) -> Result<R, Error>
    where
        F: Fn(&mut MintInfo) -> R + Send + Sync,
        R: Send,
    {
        self.update_config_value(
            CDK_MINT_CONFIG_KV_KEY,
            || Err(Error::CouldNotGetMintInfo),
            update,
        )
        .await
    }

    /// Update the supplied quote time-to-live values, preserving other settings.
    #[instrument(skip_all)]
    pub async fn update_quote_ttl(
        &self,
        mint_ttl: Option<u64>,
        melt_ttl: Option<u64>,
    ) -> Result<QuoteTTL, Error> {
        if mint_ttl.is_none() && melt_ttl.is_none() {
            return self.quote_ttl().await;
        }

        self.update_config_value(
            CDK_MINT_QUOTE_TTL_KV_KEY,
            || Ok(QuoteTTL::default()),
            |ttl| {
                ttl.mint_ttl = mint_ttl.unwrap_or(ttl.mint_ttl);
                ttl.melt_ttl = melt_ttl.unwrap_or(ttl.melt_ttl);
                *ttl
            },
        )
        .await
    }

    async fn update_config_value<T, R, D, F>(
        &self,
        key: &str,
        default: D,
        update: F,
    ) -> Result<R, Error>
    where
        T: DeserializeOwned + Serialize + Send,
        R: Send,
        D: Fn() -> Result<T, Error> + Send + Sync,
        F: Fn(&mut T) -> R + Send + Sync,
    {
        const MAX_ATTEMPTS: usize = 8;
        let namespace = CDK_MINT_PRIMARY_NAMESPACE;
        let section = CDK_MINT_CONFIG_SECONDARY_NAMESPACE;

        for _ in 0..MAX_ATTEMPTS {
            let previous = self.localstore.kv_read(namespace, section, key).await?;
            let mut value = match previous.as_deref() {
                Some(bytes) => serde_json::from_slice(bytes)?,
                None => default()?,
            };
            let result = update(&mut value);
            let replacement = serde_json::to_vec(&value)?;
            let mut tx = self.localstore.begin_transaction().await?;
            let updated = match previous.as_deref() {
                Some(bytes) => {
                    tx.kv_write_if_equals(namespace, section, key, bytes, &replacement)
                        .await?
                }
                None => {
                    tx.kv_write_if_absent(namespace, section, key, &replacement)
                        .await?
                }
            };
            if updated {
                tx.commit().await?;
                return Ok(result);
            }
            tx.rollback().await?;
        }

        Err(Error::ConcurrentUpdate)
    }
}
