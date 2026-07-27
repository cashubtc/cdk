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

/// Purpose component of every keyset derivation path, per the remote signer
/// spec.
const KEYSET_PURPOSE: u32 = 129372;

pub fn derivation_path_from_unit(unit: CurrencyUnit, index: u32) -> Option<DerivationPath> {
    let unit_index = unit.hashed_derivation_index();

    Some(DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(KEYSET_PURPOSE).expect("129372 is a valid index"),
        ChildNumber::from_hardened_idx(unit_index).expect("unit index should be valid"),
        ChildNumber::from_hardened_idx(index).expect("0 is a valid index"),
    ]))
}

/// Reject a custom derivation path claimed by more than one unit.
///
/// Keys derive from the xpriv and the derivation path alone, so two units
/// pinned to one path sign with identical keys, and a proof minted under one
/// unit verifies under the other.
pub fn check_custom_path_uniqueness(
    custom_paths: &HashMap<CurrencyUnit, DerivationPath>,
) -> Result<(), Error> {
    let mut entries: Vec<_> = custom_paths.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let mut by_path: HashMap<&DerivationPath, &CurrencyUnit> = HashMap::new();
    for (unit, path) in entries {
        if let Some(existing) = by_path.insert(path, unit) {
            return Err(Error::CustomPathCollision {
                first: existing.clone(),
                second: unit.clone(),
                path: path.clone(),
            });
        }
    }

    Ok(())
}

/// Reject a custom derivation path pinned inside another unit's branch.
///
/// [`derivation_path_from_unit`] builds exactly
/// `m/129372'/<unit.hashed_derivation_index()>'/<index>'`, so any other path of
/// that shape belongs to some other unit's index derivation, which will
/// eventually reach it and re-derive the pinned unit's keys. A path outside the
/// `m/129372'` subtree can never collide with an index-derived one and stays
/// allowed, which is also what a pre-NUT-XX `m/0'/<ordinal>'/<index>'` migration
/// pin needs.
pub fn check_custom_path_branch(
    custom_paths: &HashMap<CurrencyUnit, DerivationPath>,
) -> Result<(), Error> {
    let mut entries: Vec<_> = custom_paths.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    for (unit, path) in entries {
        let components: &[ChildNumber] = path.as_ref();
        let [ChildNumber::Hardened { index: purpose }, ChildNumber::Hardened { index: branch }, ChildNumber::Hardened { .. }] =
            components
        else {
            continue;
        };

        if *purpose == KEYSET_PURPOSE && *branch != unit.hashed_derivation_index() {
            return Err(Error::CustomPathOutsideUnitBranch {
                unit: unit.clone(),
                path: path.clone(),
            });
        }
    }

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> DerivationPath {
        s.parse().expect("derivation path")
    }

    #[test]
    fn distinct_custom_paths_are_accepted() {
        let paths = HashMap::from([
            (CurrencyUnit::Sat, path("m/8'/0'/9'")),
            (CurrencyUnit::Usd, path("m/8'/2'/9'")),
        ]);

        assert!(check_custom_path_uniqueness(&paths).is_ok());
        assert!(check_custom_path_uniqueness(&HashMap::new()).is_ok());
    }

    #[test]
    fn two_units_on_one_custom_path_are_rejected() {
        let shared = path("m/8'/0'/9'");
        let paths = HashMap::from([
            (CurrencyUnit::Sat, shared.clone()),
            (CurrencyUnit::Usd, shared.clone()),
        ]);

        let err = check_custom_path_uniqueness(&paths).expect_err("duplicate path is rejected");
        let message = err.to_string();
        assert!(message.contains("sat"), "{message}");
        assert!(message.contains("usd"), "{message}");
        assert!(message.contains(&shared.to_string()), "{message}");
    }

    #[test]
    fn a_pin_inside_the_unit_own_branch_is_accepted() {
        let own = derivation_path_from_unit(CurrencyUnit::Sat, 9).expect("derivation path");
        let paths = HashMap::from([(CurrencyUnit::Sat, own)]);

        assert!(check_custom_path_branch(&paths).is_ok());
    }

    #[test]
    fn a_pin_outside_the_protocol_subtree_is_accepted() {
        let paths = HashMap::from([
            (CurrencyUnit::Sat, path("m/0'/0'/9'")),
            (CurrencyUnit::Usd, path("m/8'/2'/9'")),
        ]);

        assert!(check_custom_path_branch(&paths).is_ok());
        assert!(check_custom_path_branch(&HashMap::new()).is_ok());
    }

    #[test]
    fn a_pin_inside_another_unit_branch_is_rejected() {
        let contested = derivation_path_from_unit(CurrencyUnit::Sat, 2).expect("derivation path");
        let paths = HashMap::from([(CurrencyUnit::Usd, contested.clone())]);

        let err = check_custom_path_branch(&paths).expect_err("foreign branch is rejected");
        let message = err.to_string();
        assert!(message.contains("usd"), "{message}");
        assert!(message.contains(&contested.to_string()), "{message}");
    }
}
