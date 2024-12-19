use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::secp256k1;
use tracing::instrument;

use crate::mint::{CurrencyUnit, MintKeySet, MintKeySetInfo};
use crate::util::unix_time;

/// Generate new [`MintKeySetInfo`] from path
#[instrument(skip_all)]
pub fn create_new_keyset<C: secp256k1::Signing>(
    secp: &secp256k1::Secp256k1<C>,
    xpriv: Xpriv,
    derivation_path: DerivationPath,
    derivation_path_index: Option<u32>,
    unit: CurrencyUnit,
    max_order: u8,
    input_fee_ppk: u64,
) -> (MintKeySet, MintKeySetInfo) {
    let keyset = MintKeySet::generate(
        secp,
        xpriv
            .derive_priv(secp, &derivation_path)
            .expect("RNG busted"),
        unit,
        max_order,
    );
    let keyset_info = MintKeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: true,
        valid_from: unix_time(),
        valid_to: None,
        derivation_path,
        derivation_path_index,
        max_order,
        input_fee_ppk,
    };
    (keyset, keyset_info)
}
