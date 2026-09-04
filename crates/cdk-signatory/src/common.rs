use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::secp256k1::{self, All, Secp256k1};
use cdk_common::common::IssuerVersion;
use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::{CurrencyUnit, MintKeySet};
use cdk_common::util::unix_time;
use cdk_common::{database, nut02};

/// Initialize keysets
pub async fn init_keysets(
    xpriv: Xpriv,
    secp_ctx: &Secp256k1<All>,
    localstore: &Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
    supported_units: &HashMap<CurrencyUnit, (u64, Vec<u64>)>,
) -> Result<(), Error> {
    let mut tx = localstore.begin_transaction().await?;

    // The transaction holds the global keyset advisory lock, so reading each
    // unit's keysets and reassigning the active pointer is atomic against any
    // concurrent rotation. A pre-transaction read would reopen that race.
    //
    // Iterate in a deterministic order for reproducibility; HashMap iteration
    // order is randomized per process.
    let mut units: Vec<_> = supported_units.iter().collect();
    units.sort_by(|a, b| a.0.cmp(b.0));

    for (unit, (input_fee_ppk, amounts)) in units {
        let mut keysets = tx.get_keyset_infos_by_unit(unit).await?;
        keysets.sort_by_key(|b| std::cmp::Reverse(b.derivation_path_index));

        let Some(highest_index_keyset) = keysets.first() else {
            continue;
        };

        if highest_index_keyset.is_expired() {
            tracing::info!(
                "Highest index keyset for unit {} has expired, skipping reactivation",
                unit
            );
            continue;
        }

        // Check if it matches our criteria
        if highest_index_keyset.input_fee_ppk == *input_fee_ppk
            && highest_index_keyset.amounts == *amounts
        {
            tracing::debug!(
                "Current highest index keyset matches expect fee and amounts. Setting active"
            );
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
    keyset_id_version: nut02::KeySetVersion,
) -> (MintKeySet, MintKeySetInfo) {
    let keyset = MintKeySet::generate(
        secp,
        xpriv
            .derive_priv(secp, &derivation_path)
            .expect("RNG busted"),
        unit,
        amounts,
        input_fee_ppk,
        final_expiry,
        keyset_id_version,
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
        issuer_version: IssuerVersion::from_str(&format!("cdk/{}", env!("CARGO_PKG_VERSION"))).ok(),
    };
    (keyset, keyset_info)
}

pub fn derivation_path_from_unit(unit: CurrencyUnit, index: u32) -> Option<DerivationPath> {
    let unit_index = unit.hashed_derivation_index();

    Some(DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(129372).expect("129372 is a valid index"),
        ChildNumber::from_hardened_idx(unit_index).expect("unit index should be valid"),
        ChildNumber::from_hardened_idx(index).expect("0 is a valid index"),
    ]))
}

/// take all the keyset units and if te new keyset is a new unit we check
///
/// Takes the existing units by reference rather than whole keysets: the check
/// only needs the units, and a sweep calls it once per rotating unit, where
/// materializing every keyset with its derived keys would be wasted work.
pub fn check_unit_string_collision<'a>(
    existing_units: impl Iterator<Item = &'a CurrencyUnit>,
    new_keyset: &MintKeySetInfo,
) -> Result<(), Error> {
    let mut unit_hash: HashSet<&CurrencyUnit> = HashSet::new();

    for unit in existing_units {
        unit_hash.insert(unit);
    }

    if unit_hash.contains(&new_keyset.unit) {
        // the currency unit already exists so we don't have to check it
        return Ok(());
    }

    let new_unit_int = new_keyset.unit.hashed_derivation_index();
    for unit in unit_hash {
        let existing_unit_string = unit.hashed_derivation_index();
        if existing_unit_string == new_unit_int {
            return Err(Error::UnitStringCollision(new_keyset.unit.clone()));
        }
    }

    Ok(())
}
