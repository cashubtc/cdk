use bitcoin::bip32::ChildNumber;
use bitcoin::bip32::DerivationPath;
use bitcoin::bip32::Xpriv;
use bitcoin::hashes::sha256::Hash as Sha256;
use bitcoin::hashes::Hash;
use bitcoin::key::Secp256k1;
use bitcoin::secp256k1;
use cashu_kvac::models::MintPrivateKey;
use cashu_kvac::models::MintPublicKey;
use cashu_kvac::models::ZKP;
use cashu_kvac::models::MAC;
use cashu_kvac::secp::GroupElement;
use cashu_kvac::secp::Scalar;
use serde::{Deserialize, Serialize};
use crate::util::hex;
use super::nut02::KeySetVersion;
use super::CurrencyUnit;
use super::Id;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKvacKeys {
    pub private_key: MintPrivateKey,
    pub public_key: MintPublicKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvacKeys(MintPublicKey);

impl From<MintKvacKeys> for KvacKeys {
    fn from(keys: MintKvacKeys) -> Self {
        Self(
            keys.public_key
        )
    }
}

#[cfg(feature = "mint")]
/// MintKeyset
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKvacKeySet {
    /// Keyset [`Id`]
    pub id: Id,
    /// Keyset [`CurrencyUnit`]
    pub unit: CurrencyUnit,
    /// Kvac public keys [`MintKvacKeys`]
    pub kvac_keys: MintKvacKeys,
}

impl From<&KvacKeys> for Id {
    fn from(kvac_keys: &KvacKeys) -> Self {
        let mut data = kvac_keys.0.Cw.to_bytes();
        data.extend(kvac_keys.0.I.to_bytes());
        let hash = Sha256::hash(&data);
        let hex_of_hash = hex::encode(hash.to_byte_array());
        Id {
            version: KeySetVersion::Version00,
            id: hex::decode(&hex_of_hash[0..14])
                .expect("Keys hash could not be hex decoded")
                .try_into()
                .expect("Invalid length of hex id"),
        }
    }
}

#[cfg(feature = "mint")]
impl MintKvacKeySet {
    /// Generate new [`MintKeySet`]
    pub fn generate<C: secp256k1::Signing>(
        secp: &Secp256k1<C>,
        mut xpriv: Xpriv,
        unit: CurrencyUnit,
        derivation_path: DerivationPath,
        derivation_path_index: u32,
    ) -> Self {
        xpriv = xpriv.derive_priv(secp, &derivation_path).expect("RNG busted");
        let mut scalars = Vec::with_capacity(6);
        for i in 0..6 {
            let secret_key = xpriv
                .derive_priv(
                    secp,
                    &[ChildNumber::from_hardened_idx(i as u32).expect("order is valid index")],
                )
                .expect("RNG busted")
                .private_key
                .secret_bytes();
            scalars[i] = Scalar::new(&secret_key);
        }
        let private_key = cashu_kvac::models::MintPrivateKey::from_scalars(&scalars)
            .expect("couldn't generate KVAC privkey")
            .tweak_epoch(derivation_path_index as u64);
        let kvac_keys = MintKvacKeys {
            private_key: private_key.clone(),
            public_key: private_key.public_key
        };
        let pub_kvac_keys: KvacKeys = kvac_keys.clone().into();
        Self { id: (&pub_kvac_keys).into(), unit, kvac_keys }
    }
}

/// Kvac Coin Message
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacCoinMessage {
    /// Keyset ID
    ///
    /// ID from which we expect a signature.
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Tag
    /// 
    /// Unique identifier used to create the algebraic MAC from
    /// and for recovery purporses
    #[serde(rename = "t")]
    pub t_tag: Scalar,
    /// Coin
    /// 
    /// Pair ([`GroupElement`], [`GroupElement`]) that represent:
    /// 1) Value: encoding value 0
    /// 2) Script: encoding a custom script (Mint doesn't care)
    #[serde(rename = "c")]
    pub coin: (GroupElement, GroupElement)
}

/// Bootstrap Request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BootstrapRequest {
    /// Inputs
    /// 
    /// [`Vec<KvacCoinMessage>`] Where each element is a coin encoding 0 as an amount.
    #[cfg_attr(feature = "swagger", schema(max_items = 1_000, min_items = 2))]
    pub inputs: Vec<KvacCoinMessage>,
    /// Bootstrap Proofs
    /// 
    /// [`Vec<ZKP>`] proving that each coin is worth 0
    #[cfg_attr(feature = "swagger", schema(max_items = 1_000, min_items = 2))]
    pub proofs: Vec<ZKP>,
}

/// Bootstrap Response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BootstrapResponse {
    /// MACs
    /// 
    /// Approval stamp of the Mint
    pub macs: Vec<MAC>,
    /// IParams Proofs
    /// 
    /// [`Vec<ZKP>`] Proving that [`MintPrivateKey`] was used to issue each [`MAC`]
    pub proofs: Vec<ZKP>,
}