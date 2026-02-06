use std::collections::HashMap;
use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::secp256k1::{self, All, Secp256k1};
use cdk_common::database;
use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::{CurrencyUnit, MintKeySet};
use cdk_common::util::unix_time;

/// Initialize keysets
pub async fn init_keysets(
    xpriv: Xpriv,
    secp_ctx: &Secp256k1<All>,
    localstore: &Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
    supported_units: &HashMap<CurrencyUnit, (u64, u8)>,
) -> Result<(), Error> {
    let keysets_infos = localstore.get_keyset_infos().await?;
    let mut tx = localstore.begin_transaction().await?;

    let keysets_by_unit: HashMap<CurrencyUnit, Vec<MintKeySetInfo>> =
        keysets_infos.iter().fold(HashMap::new(), |mut acc, ks| {
            acc.entry(ks.unit.clone()).or_default().push(ks.clone());
            acc
        });

    for (unit, keysets) in keysets_by_unit {
        // We only care about units that are supported
        if let Some((input_fee_ppk, max_order)) = supported_units.get(&unit) {
            let mut keysets = keysets;
            keysets.sort_by(|a, b| b.derivation_path_index.cmp(&a.derivation_path_index));

            if let Some(highest_index_keyset) = keysets.first() {
                // Check if it matches our criteria
                if highest_index_keyset.input_fee_ppk == *input_fee_ppk
                    && highest_index_keyset.amounts.len() == (*max_order as usize)
                {
                    tracing::debug!("Current highest index keyset matches expect fee and max order. Setting active");
                    let id = highest_index_keyset.id;

                    // Validate we can generate it (sanity check)
                    let _ = MintKeySet::generate_from_xpriv(
                        secp_ctx,
                        xpriv,
                        &highest_index_keyset.amounts,
                        highest_index_keyset.unit.clone(),
                        highest_index_keyset.derivation_path.clone(),
                        highest_index_keyset.input_fee_ppk,
                        highest_index_keyset.final_expiry,
                        highest_index_keyset.id.get_version(),
                    );

                    let mut keyset_info = highest_index_keyset.clone();
                    keyset_info.active = true;
                    tx.add_keyset_info(keyset_info).await?;
                    tx.set_active_keyset(unit.clone(), id).await?;
                }
            }
        }
    }

    tx.commit().await?;

    Ok(())
}

/// Generate new [`MintKeySetInfo`] from path
#[tracing::instrument(skip_all)]
#[allow(clippy::too_many_arguments)]
pub fn create_new_keyset<C: secp256k1::Signing>(
    secp: &secp256k1::Secp256k1<C>,
    xpriv: Xpriv,
    derivation_path: DerivationPath,
    derivation_path_index: Option<u32>,
    unit: CurrencyUnit,
    amounts: &[u64],
    input_fee_ppk: u64,
    final_expiry: Option<u64>,
    use_keyset_v2: bool,
) -> (MintKeySet, MintKeySetInfo) {
    let version = if use_keyset_v2 {
        cdk_common::nut02::KeySetVersion::Version01
    } else {
        cdk_common::nut02::KeySetVersion::Version00
    };

    let keyset = MintKeySet::generate(
        secp,
        xpriv
            .derive_priv(secp, &derivation_path)
            .expect("RNG busted"),
        unit,
        amounts,
        input_fee_ppk,
        final_expiry,
        version,
    );
    let keyset_info = MintKeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: true,
        valid_from: unix_time(),
        final_expiry: keyset.final_expiry,
        derivation_path,
        derivation_path_index,
        amounts: amounts.to_owned(),
        input_fee_ppk,
    };
    (keyset, keyset_info)
}

pub fn derivation_path_from_unit(unit: CurrencyUnit, index: u32) -> Option<DerivationPath> {
    let unit_index = unit.derivation_index()?;

    Some(DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(0).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(unit_index).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(index).expect("0 is a valid index"),
    ]))
}
