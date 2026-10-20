use cdk_common::database::mint::DynMintTransaction;
use cdk_common::nuts::unit_metadata;
use cdk_signatory::signatory::SignatoryKeySet;

use super::{
    CDK_MINT_CONFIG_KV_KEY, CDK_MINT_CONFIG_SECONDARY_NAMESPACE, CDK_MINT_PRIMARY_NAMESPACE,
};
use crate::nuts::MintInfo;
use crate::Error;

pub(super) fn validate_units(info: &MintInfo, keysets: &[SignatoryKeySet]) -> Result<(), Error> {
    if let Some(entries) = &info.nuts.unit_metadata {
        unit_metadata::validate(entries)?;
        for entry in entries {
            if !keysets
                .iter()
                .map(|keyset| &keyset.unit)
                .chain(info.nuts.nut04.methods.iter().map(|method| &method.unit))
                .chain(info.nuts.nut05.methods.iter().map(|method| &method.unit))
                .any(|unit| unit.to_string() == entry.unit)
            {
                return Err(unit_metadata::Error::UnknownUnit {
                    unit: entry.unit.clone(),
                }
                .into());
            }
        }
    }
    Ok(())
}

/// Claim each unit's precision in the same transaction that publishes mint info.
pub(super) async fn persist_history(
    tx: &mut DynMintTransaction,
    info: &MintInfo,
) -> Result<(), Error> {
    if !cfg!(feature = "experimental-unit-metadata") {
        return Ok(());
    }
    let stored = tx
        .kv_read(
            CDK_MINT_PRIMARY_NAMESPACE,
            CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
            CDK_MINT_CONFIG_KV_KEY,
        )
        .await?;
    let previous = stored
        .as_deref()
        .map(serde_json::from_slice::<MintInfo>)
        .transpose()?;
    for entries in previous
        .as_ref()
        .and_then(|info| info.nuts.unit_metadata.as_ref())
        .into_iter()
        .chain(info.nuts.unit_metadata.as_ref())
    {
        for entry in entries {
            use bitcoin::hashes::{sha256, Hash};
            let key = sha256::Hash::hash(entry.unit.as_bytes()).to_string();
            if !tx
                .kv_write_if_absent(
                    CDK_MINT_PRIMARY_NAMESPACE,
                    "unit_precision",
                    &key,
                    &[entry.precision],
                )
                .await?
            {
                let stored = tx
                    .kv_read(CDK_MINT_PRIMARY_NAMESPACE, "unit_precision", &key)
                    .await?;
                let previous = match stored.as_deref() {
                    Some([precision]) if *precision <= 18 => *precision,
                    _ => {
                        return Err(unit_metadata::Error::InvalidStoredPrecision {
                            unit: entry.unit.clone(),
                        }
                        .into())
                    }
                };
                if previous != entry.precision {
                    return Err(unit_metadata::Error::PrecisionChanged {
                        unit: entry.unit.clone(),
                        previous,
                        proposed: entry.precision,
                    }
                    .into());
                }
            }
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "experimental-unit-metadata"))]
mod tests {
    use std::sync::Arc;

    use crate::mint::{MintBuilder, UnitConfig};
    use crate::nuts::{CurrencyUnit, UnitMetadata};

    use super::*;

    fn entry(precision: u8) -> UnitMetadata {
        UnitMetadata {
            unit: "ora".to_owned(),
            precision,
            name: None,
            symbol: None,
        }
    }

    #[tokio::test]
    async fn unit_metadata_precision_survives_removal_and_restart() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(db.clone()).with_unit_metadata(vec![entry(2)]);
        builder
            .configure_unit(CurrencyUnit::custom("ora"), UnitConfig::default())
            .unwrap();
        let mint = builder
            .build_with_seed(db.clone(), &[42; 64])
            .await
            .unwrap();
        let keysets = mint.keysets();
        let mut info = mint.mint_info().await.unwrap();
        assert_eq!(info.nuts.unit_metadata.as_ref().unwrap()[0].precision, 2);
        info.nuts.unit_metadata = None;
        mint.set_mint_info(info.clone()).await.unwrap();
        drop(mint);

        let mut builder = MintBuilder::new(db.clone());
        builder
            .configure_unit(CurrencyUnit::custom("ora"), UnitConfig::default())
            .unwrap();
        let mint = builder.build_with_seed(db, &[42; 64]).await.unwrap();
        info.nuts.unit_metadata = Some(vec![entry(3)]);
        assert!(matches!(
            mint.set_mint_info(info.clone()).await,
            Err(Error::UnitMetadata(
                unit_metadata::Error::PrecisionChanged { .. }
            ))
        ));
        assert!(mint.mint_info().await.unwrap().nuts.unit_metadata.is_none());
        info.nuts.unit_metadata = Some(vec![entry(2)]);
        mint.set_mint_info_and_quote_ttl(info, crate::types::QuoteTTL::new(100, 100))
            .await
            .unwrap();
        assert_eq!(mint.keysets(), keysets);
    }

    #[tokio::test]
    async fn unit_metadata_concurrent_first_publication_has_one_precision() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(db.clone());
        builder
            .configure_unit(CurrencyUnit::custom("ora"), UnitConfig::default())
            .unwrap();
        let mint = builder.build_with_seed(db, &[42; 64]).await.unwrap();
        let mut first = mint.mint_info().await.unwrap();
        first.nuts.unit_metadata = Some(vec![entry(2)]);
        let mut second = first.clone();
        second.nuts.unit_metadata = Some(vec![entry(3)]);
        let (a, b) = tokio::join!(mint.set_mint_info(first), mint.set_mint_info(second));
        assert_ne!(a.is_ok(), b.is_ok());
        assert_eq!(
            mint.mint_info().await.unwrap().nuts.unit_metadata.unwrap()[0].precision,
            if a.is_ok() { 2 } else { 3 }
        );
    }
}
