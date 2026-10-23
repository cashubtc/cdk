use std::collections::HashMap;

use bitcoin::hashes::{sha256, Hash};
use cdk_common::database::{self, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::unit_metadata::{defined_precision, Error as MetadataError};
use cdk_common::nuts::{MintInfo, UnitDisplayInfo};
use tracing::instrument;

use crate::{Error, Wallet};

const NAMESPACE: &str = "unit_metadata";

fn unit_namespace(mint_url: &MintUrl, unit: &str) -> String {
    sha256::Hash::hash(format!("{mint_url}\0{unit}").as_bytes()).to_string()
}

pub(super) fn mint_namespace(mint_url: &MintUrl) -> String {
    sha256::Hash::hash(mint_url.to_string().as_bytes()).to_string()
}

pub(super) async fn copy_history(
    storage: &(dyn WalletDatabase<database::Error> + Send + Sync),
    old_url: &MintUrl,
    new_url: &MintUrl,
) -> Result<(), Error> {
    if !cfg!(feature = "experimental-unit-metadata") || old_url == new_url {
        return Ok(());
    }
    let mut units = std::collections::HashSet::new();
    if let Some(bytes) = storage
        .kv_read(NAMESPACE, &mint_namespace(old_url), "unit_names")
        .await?
    {
        let names: HashMap<crate::nuts::Id, String> = serde_json::from_slice(&bytes)?;
        units.extend(names.into_values());
    }
    if let Some(info) = storage.get_mint(old_url.clone()).await? {
        observe(storage, old_url, &info).await?;
        units.extend(
            info.nuts
                .unit_metadata
                .into_iter()
                .flatten()
                .map(|entry| entry.unit),
        );
    }
    for keyset in storage
        .get_mint_keysets(old_url.clone())
        .await?
        .into_iter()
        .flatten()
    {
        units.insert(keyset.unit.to_string());
    }
    for unit in units {
        let old_namespace = unit_namespace(old_url, &unit);
        let new_namespace = unit_namespace(new_url, &unit);
        for key in storage.kv_list(NAMESPACE, &old_namespace).await? {
            if let Some(value) = storage.kv_read(NAMESPACE, &old_namespace, &key).await? {
                storage
                    .kv_write(NAMESPACE, &new_namespace, &key, &value)
                    .await?;
            }
        }
    }
    Ok(())
}

pub(super) async fn observe(
    storage: &(dyn WalletDatabase<database::Error> + Send + Sync),
    mint_url: &MintUrl,
    info: &MintInfo,
) -> Result<(), Error> {
    let mut counts = HashMap::new();
    for entry in info.nuts.unit_metadata.iter().flatten() {
        *counts.entry(&entry.unit).or_insert(0) += 1;
    }
    for entry in info.nuts.unit_metadata.iter().flatten() {
        if defined_precision(&entry.unit).is_none()
            && entry.precision <= 18
            && counts[&entry.unit] == 1
        {
            let namespace = unit_namespace(mint_url, &entry.unit);
            let key = format!("seen_{}", entry.precision);
            if storage
                .kv_read(NAMESPACE, &namespace, &key)
                .await?
                .is_none()
            {
                // Immutable keys avoid lost updates between wallets sharing a database.
                storage.kv_write(NAMESPACE, &namespace, &key, &[]).await?;
            }
        }
    }
    Ok(())
}

async fn observations(
    storage: &(dyn WalletDatabase<database::Error> + Send + Sync),
    namespace: &str,
) -> Result<u32, Error> {
    let mut mask = 0;
    for key in storage.kv_list(NAMESPACE, namespace).await? {
        if let Some(precision) = key
            .strip_prefix("seen_")
            .and_then(|p| p.parse::<u8>().ok())
            .filter(|p| *p <= 18)
        {
            mask |= 1 << precision;
        }
    }
    Ok(mask)
}

impl Wallet {
    /// Resolve display metadata for this mint and unit.
    ///
    /// Changed custom precision returns raw display and a pending precision.
    /// Storage failures are returned to the caller, which should display raw
    /// integer amounts instead. This API never changes spendable amounts.
    #[instrument(skip_all)]
    pub async fn unit_display_info(&self) -> Result<UnitDisplayInfo, Error> {
        let metadata = self
            .metadata_cache
            .load(&self.localstore, &self.client)
            .await?;
        let mut names = metadata
            .keysets
            .values()
            .filter(|keyset| keyset.unit == self.unit)
            .map(|keyset| metadata.unit_names.get(&keyset.id));
        let canonical = self.unit.to_string();
        let first = names.next().flatten();
        let exact = first.filter(|first| names.all(|name| name == Some(first)));
        let unit = exact.map(String::as_str).unwrap_or(&canonical);
        let entries = exact.and(metadata.mint_info.nuts.unit_metadata.as_deref());
        let mut display = UnitDisplayInfo::resolve(unit, entries);
        let precision = match (defined_precision(unit), display.precision) {
            (None, Some(precision)) => precision,
            _ => return Ok(display),
        };
        if let Some(previous) = self.localstore.get_mint(self.mint_url.clone()).await? {
            observe(self.localstore.as_ref(), &self.mint_url, &previous).await?;
        }
        observe(
            self.localstore.as_ref(),
            &self.mint_url,
            &metadata.mint_info,
        )
        .await?;
        let namespace = unit_namespace(&self.mint_url, unit);
        let mask = observations(self.localstore.as_ref(), &namespace).await?;
        if mask & (1 << precision) == 0 {
            return Err(MetadataError::InvalidStoredPrecision {
                unit: unit.to_owned(),
            }
            .into());
        }
        if mask.count_ones() == 1 {
            return Ok(display);
        }
        let accepted = self
            .localstore
            .kv_read(NAMESPACE, &namespace, "accepted")
            .await?;
        let expected = format!("{mask}_{precision}");
        if accepted.as_deref() != Some(expected.as_bytes()) {
            display.previous_precision = accepted
                .as_deref()
                .and_then(|value| std::str::from_utf8(value).ok())
                .and_then(|value| value.split_once('_'))
                .and_then(|(_, previous)| previous.parse::<u8>().ok())
                .filter(|previous| {
                    *previous <= 18 && *previous != precision && mask & (1 << previous) != 0
                })
                .or(Some((mask & !(1 << precision)).trailing_zeros() as u8));
            display.pending_precision = Some(precision);
            display.precision = None;
        }
        Ok(display)
    }

    /// Acknowledge a precision change after notifying the user.
    ///
    /// Only the currently displayed pending value can be acknowledged.
    /// New observations invalidate this acknowledgment automatically.
    #[instrument(skip_all)]
    pub async fn acknowledge_unit_precision(&self, precision: u8) -> Result<(), Error> {
        let display = self.unit_display_info().await?;
        if display.pending_precision != Some(precision) {
            return Err(MetadataError::PrecisionNotPending {
                proposed: precision,
            }
            .into());
        }
        let namespace = unit_namespace(&self.mint_url, &display.unit);
        let mask = observations(self.localstore.as_ref(), &namespace).await?;
        // Keep only the current acceptance so returning to an older precision
        // requires acknowledgment again.
        self.localstore
            .kv_write(
                NAMESPACE,
                &namespace,
                "accepted",
                format!("{mask}_{precision}").as_bytes(),
            )
            .await?;
        Ok(())
    }
}

#[cfg(all(test, feature = "experimental-unit-metadata"))]
mod tests {
    use std::sync::Arc;

    use crate::nuts::{CurrencyUnit, Id, UnitMetadata};
    use crate::wallet::test_utils::{
        create_test_db, test_keyset, test_mint_url, MockMintConnector,
    };
    use crate::wallet::WalletBuilder;

    use super::*;

    fn info(precision: u8) -> MintInfo {
        let mut info = MintInfo::default();
        info.nuts.unit_metadata = Some(vec![UnitMetadata {
            unit: "ora".to_owned(),
            precision,
            name: Some("Ora".to_owned()),
            symbol: None,
        }]);
        info
    }

    fn wallet(
        db: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        client: Arc<MockMintConnector>,
    ) -> Wallet {
        let mut keyset = test_keyset();
        keyset.unit = CurrencyUnit::custom("ora");
        keyset.id = Id::v2_from_data(
            &keyset.keys,
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        );
        client.set_mint_keys_response(Ok(vec![keyset]));
        WalletBuilder::new()
            .mint_url(test_mint_url())
            .unit(CurrencyUnit::custom("ora"))
            .localstore(db)
            .seed([42; 64])
            .shared_client(client)
            .build()
            .expect("test wallet configuration is valid")
    }

    #[tokio::test]
    async fn unit_metadata_change_requires_acknowledgment_after_restart() {
        let db = create_test_db().await;
        let client = Arc::new(MockMintConnector::new());
        client.set_mint_info_response(Ok(info(2)));
        let first = wallet(db.clone(), client.clone());
        first.fetch_mint_info().await.unwrap();
        assert_eq!(
            first.unit_display_info().await.unwrap().format_amount(500),
            "5.00 ora"
        );
        drop(first);

        let reopened = wallet(db, client.clone());
        assert_eq!(
            reopened.unit_display_info().await.unwrap().precision,
            Some(2)
        );
        let mut rotated = test_keyset();
        rotated.unit = CurrencyUnit::custom("ora");
        rotated.input_fee_ppk = 1;
        rotated.id = Id::v2_from_data(
            &rotated.keys,
            &rotated.unit,
            rotated.input_fee_ppk,
            rotated.final_expiry,
        );
        client.set_mint_keys_response(Ok(vec![rotated]));
        client.set_mint_info_response(Ok(info(3)));
        reopened.fetch_mint_info().await.unwrap();
        let display = reopened.unit_display_info().await.unwrap();
        assert_eq!(display.previous_precision, Some(2));
        assert_eq!(display.pending_precision, Some(3));
        assert_eq!(display.format_amount(500), "500 ora");
        assert!(reopened.acknowledge_unit_precision(4).await.is_err());
        reopened.acknowledge_unit_precision(3).await.unwrap();
        assert_eq!(
            reopened
                .unit_display_info()
                .await
                .unwrap()
                .format_amount(500),
            "0.500 ora"
        );

        for precision in [2, 3] {
            client.set_mint_info_response(Ok(info(precision)));
            reopened.fetch_mint_info().await.unwrap();
            assert_eq!(
                reopened
                    .unit_display_info()
                    .await
                    .unwrap()
                    .pending_precision,
                Some(precision)
            );
            reopened
                .acknowledge_unit_precision(precision)
                .await
                .unwrap();
        }

        client.set_mint_info_response(Ok(MintInfo::default()));
        reopened.fetch_mint_info().await.unwrap();
        assert_eq!(reopened.unit_display_info().await.unwrap().precision, None);
        client.set_mint_info_response(Ok(info(4)));
        reopened.fetch_mint_info().await.unwrap();
        let display = reopened.unit_display_info().await.unwrap();
        assert_eq!(display.previous_precision, Some(3));
        assert_eq!(display.pending_precision, Some(4));
    }

    #[tokio::test]
    async fn unit_metadata_acknowledgment_is_scoped_to_storage() {
        let client = Arc::new(MockMintConnector::new());
        client.set_mint_info_response(Ok(info(2)));
        let first = wallet(create_test_db().await, client.clone());
        let mut second = wallet(create_test_db().await, client.clone());
        second.metadata_cache = first.metadata_cache.clone();
        assert_eq!(first.unit_display_info().await.unwrap().precision, Some(2));
        assert_eq!(second.unit_display_info().await.unwrap().precision, Some(2));
        client.set_mint_info_response(Ok(info(3)));
        first.fetch_mint_info().await.unwrap();
        first.acknowledge_unit_precision(3).await.unwrap();
        assert_eq!(first.unit_display_info().await.unwrap().precision, Some(3));
        assert_eq!(
            second.unit_display_info().await.unwrap().pending_precision,
            Some(3)
        );
    }

    #[tokio::test]
    async fn unit_metadata_url_move_keeps_observations_after_metadata_disappears() {
        let db = create_test_db().await;
        let client = Arc::new(MockMintConnector::new());
        client.set_mint_info_response(Ok(info(2)));
        let mut wallet = wallet(db.clone(), client.clone());
        wallet.fetch_mint_info().await.unwrap();
        client.set_mint_info_response(Ok(MintInfo::default()));
        wallet.fetch_mint_info().await.unwrap();
        let new_url: MintUrl = "https://new-mint.example.com".parse().unwrap();
        wallet.update_mint_url(new_url.clone()).await.unwrap();
        assert_eq!(
            observations(db.as_ref(), &unit_namespace(&new_url, "ora"))
                .await
                .unwrap(),
            1 << 2
        );
        observe(db.as_ref(), &new_url, &info(3)).await.unwrap();
        assert_eq!(
            observations(db.as_ref(), &unit_namespace(&new_url, "ora"))
                .await
                .unwrap()
                .count_ones(),
            2
        );
    }
}
