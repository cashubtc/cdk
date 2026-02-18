use std::collections::HashMap;
use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::secp256k1::{self, All, Secp256k1};
use cdk_common::database;
use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::{CurrencyUnit, Id, MintKeySet};
use cdk_common::util::unix_time;

/// Initialize keysets and returns a [`Result`] with a tuple of the following:
/// * a [`HashMap`] mapping each active keyset `Id` to `MintKeySet`
/// * a [`Vec`] of `CurrencyUnit` containing active keysets units
pub async fn init_keysets(
    xpriv: Xpriv,
    secp_ctx: &Secp256k1<All>,
    localstore: &Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
    supported_units: &HashMap<CurrencyUnit, (u64, u8)>,
    custom_paths: &HashMap<CurrencyUnit, DerivationPath>,
) -> Result<(HashMap<Id, MintKeySet>, Vec<CurrencyUnit>), Error> {
    let mut active_keysets: HashMap<Id, MintKeySet> = HashMap::new();
    let mut active_keyset_units: Vec<CurrencyUnit> = vec![];

    // Get keysets info from DB
    let keysets_infos = localstore.get_keyset_infos().await?;

    let mut tx = localstore.begin_transaction().await?;
    if !keysets_infos.is_empty() {
        tracing::debug!("Setting all saved keysets to inactive");
        for keyset in keysets_infos.clone() {
            // Set all to in active
            let mut keyset = keyset;
            keyset.active = false;
            tx.add_keyset_info(keyset).await?;
        }

        let keysets_by_unit: HashMap<CurrencyUnit, Vec<MintKeySetInfo>> =
            keysets_infos.iter().fold(HashMap::new(), |mut acc, ks| {
                acc.entry(ks.unit.clone()).or_default().push(ks.clone());
                acc
            });

        for (unit, keysets) in keysets_by_unit {
            let mut keysets = keysets;
            keysets.sort_by(|a, b| b.derivation_path_index.cmp(&a.derivation_path_index));

            // Get the keyset with the highest counter
            let highest_index_keyset = keysets
                .first()
                .cloned()
                .expect("unit will not be added to hashmap if empty");

            let keysets: Vec<MintKeySetInfo> = keysets
                .into_iter()
                .filter(|ks| ks.derivation_path_index.is_some())
                .collect();

            if let Some((input_fee_ppk, max_order)) = supported_units.get(&unit) {
                if !keysets.is_empty()
                    && highest_index_keyset.input_fee_ppk == *input_fee_ppk
                    && highest_index_keyset.amounts.len() == (*max_order as usize)
                {
                    tracing::debug!("Current highest index keyset matches expect fee and max order. Setting active");
                    let id = highest_index_keyset.id;
                    let keyset = MintKeySet::generate_from_xpriv(
                        secp_ctx,
                        xpriv,
                        &highest_index_keyset.amounts,
                        highest_index_keyset.unit.clone(),
                        highest_index_keyset.derivation_path.clone(),
                        highest_index_keyset.input_fee_ppk,
                        highest_index_keyset.final_expiry,
                        cdk_common::nut02::KeySetVersion::Version00,
                    );
                    active_keysets.insert(id, keyset);
                    let mut keyset_info = highest_index_keyset;
                    keyset_info.active = true;
                    tx.add_keyset_info(keyset_info).await?;
                    active_keyset_units.push(unit.clone());
                    tx.set_active_keyset(unit, id).await?;
                } else {
                    // Check to see if there are not keysets by this unit
                    let derivation_path_index = if keysets.is_empty() {
                        1
                    } else {
                        highest_index_keyset.derivation_path_index.unwrap_or(0) + 1
                    };

                    let derivation_path = match custom_paths.get(&unit) {
                        Some(path) => path.clone(),
                        None => derivation_path_from_unit(unit.clone(), derivation_path_index)
                            .ok_or(Error::UnsupportedUnit)?,
                    };

                    let (keyset, keyset_info) = create_new_keyset(
                        secp_ctx,
                        xpriv,
                        derivation_path,
                        Some(derivation_path_index),
                        unit.clone(),
                        &highest_index_keyset.amounts,
                        *input_fee_ppk,
                        // TODO: add Mint settings for a final expiry of newly generated keysets
                        None,
                    );

                    let id = keyset_info.id;
                    tx.add_keyset_info(keyset_info).await?;
                    tx.set_active_keyset(unit.clone(), id).await?;
                    active_keysets.insert(id, keyset);
                    active_keyset_units.push(unit.clone());
                };
            }
        }
    }

    tx.commit().await?;

    Ok((active_keysets, active_keyset_units))
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
        // TODO: change this to Version01 to generate keysets v2
        cdk_common::nut02::KeySetVersion::Version00,
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
        #[cfg(feature = "conditional-tokens")]
        condition_id: None,
        #[cfg(feature = "conditional-tokens")]
        outcome_collection: None,
        #[cfg(feature = "conditional-tokens")]
        outcome_collection_id: None,
    };
    (keyset, keyset_info)
}

/// Create a new keyset for a conditional token (NUT-28).
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
        condition_id: Some(condition_id.to_string()),
        outcome_collection: None, // Set by the caller
        outcome_collection_id: Some(outcome_collection_id.to_string()),
    };
    Some((keyset, keyset_info))
}

pub fn derivation_path_from_unit(unit: CurrencyUnit, index: u32) -> Option<DerivationPath> {
    let unit_index = unit.derivation_index()?;

    Some(DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(0).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(unit_index).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(index).expect("0 is a valid index"),
    ]))
}
