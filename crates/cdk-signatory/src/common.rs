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
    let keysets_infos = localstore.get_keyset_infos().await?;
    let mut tx = localstore.begin_transaction().await?;

    let keysets_by_unit: HashMap<CurrencyUnit, Vec<MintKeySetInfo>> =
        keysets_infos.iter().fold(HashMap::new(), |mut acc, ks| {
            acc.entry(ks.unit.clone()).or_default().push(ks.clone());
            acc
        });

    for (unit, keysets) in keysets_by_unit {
        // We only care about units that are supported
        if let Some((input_fee_ppk, amounts)) = supported_units.get(&unit) {
            let mut keysets = keysets;
            keysets.sort_by_key(|b| std::cmp::Reverse(b.derivation_path_index));

            if let Some(highest_index_keyset) = keysets.first() {
                // Check if it matches our criteria
                if highest_index_keyset.input_fee_ppk == *input_fee_ppk
                    && highest_index_keyset.amounts == *amounts
                {
                    tracing::debug!("Current highest index keyset matches expect fee and amounts. Setting active");
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
        #[cfg(feature = "conditional-tokens")]
        condition_id: None,
        #[cfg(feature = "conditional-tokens")]
        outcome_collection: None,
        #[cfg(feature = "conditional-tokens")]
        outcome_collection_id: None,
    };
    (keyset, keyset_info)
}

/// Create a new keyset for a conditional token (NUT-CTF).
///
/// Uses a derivation path based on a SHA-256 hash of the condition_id and outcome_collection_id
/// to derive a unique child index: `m/0'/<unit_index>'/<hash_derived_index>'`
#[cfg(feature = "conditional-tokens")]
#[allow(clippy::too_many_arguments)]
pub fn create_conditional_keyset<C: secp256k1::Signing>(
    secp: &secp256k1::Secp256k1<C>,
    xpriv: Xpriv,
    unit: CurrencyUnit,
    condition_id: &str,
    outcome_collection_id: &str,
    amounts: &[u64],
    input_fee_ppk: u64,
    final_expiry: Option<u64>,
) -> Option<(MintKeySet, MintKeySetInfo)> {
    use bitcoin::hashes::{sha256, Hash};

    let unit_index = unit.derivation_index()?;

    // Derive a unique child index from SHA256(condition_id || outcome_collection_id)
    let mut hash_input = Vec::new();
    hash_input.extend_from_slice(condition_id.as_bytes());
    hash_input.extend_from_slice(outcome_collection_id.as_bytes());
    let hash = sha256::Hash::hash(&hash_input);
    let hash_bytes = hash.as_byte_array();
    // Use the first 4 bytes as a u32 index, masking to valid hardened range (max 2^31 - 1)
    let derived_index =
        u32::from_be_bytes([hash_bytes[0], hash_bytes[1], hash_bytes[2], hash_bytes[3]])
            & 0x7FFF_FFFF;

    let derivation_path = DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(0).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(unit_index).expect("valid unit index"),
        ChildNumber::from_hardened_idx(derived_index).expect("valid derived index"),
    ]);

    let mut keyset = MintKeySet::generate(
        secp,
        xpriv
            .derive_priv(secp, &derivation_path)
            .expect("RNG busted"),
        unit,
        amounts,
        input_fee_ppk,
        final_expiry,
        cdk_common::nut02::KeySetVersion::Version01,
    );

    // Override the keyset ID with V2 conditional derivation
    let pub_keys: cdk_common::nuts::Keys = keyset.keys.clone().into();
    keyset.id = cdk_common::nuts::Id::v2_from_data_conditional(
        &pub_keys,
        &keyset.unit,
        input_fee_ppk,
        final_expiry,
        condition_id,
        outcome_collection_id,
    );

    let keyset_info = MintKeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: true,
        valid_from: unix_time(),
        final_expiry: keyset.final_expiry,
        derivation_path,
        derivation_path_index: Some(derived_index),
        amounts: amounts.to_owned(),
        input_fee_ppk,
        issuer_version: None,
        condition_id: Some(condition_id.to_string()),
        outcome_collection: None, // Set by the caller
        outcome_collection_id: Some(outcome_collection_id.to_string()),
    };
    Some((keyset, keyset_info))
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
pub fn check_unit_string_collision(
    keysets: Vec<crate::signatory::SignatoryKeySet>,
    new_keyset: &MintKeySetInfo,
) -> Result<(), Error> {
    let mut unit_hash: HashSet<CurrencyUnit> = HashSet::new();

    for key in keysets {
        unit_hash.insert(key.unit);
    }

    if unit_hash.contains(&new_keyset.unit) {
        // the currency unit already exists so we don't have to check it
        return Ok(());
    }

    let new_unit_int = new_keyset.unit.hashed_derivation_index();
    for unit in unit_hash.iter() {
        let existing_unit_string = unit.hashed_derivation_index();
        if existing_unit_string == new_unit_int {
            return Err(Error::UnitStringCollision(new_keyset.unit.clone()));
        }
    }

    Ok(())
}
