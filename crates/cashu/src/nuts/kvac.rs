use super::nut02::KeySetVersion;
use super::CurrencyUnit;
use super::Id;
use super::KeySetInfo;
use super::State;
use crate::util::hex;
use crate::Amount;
use crate::SECP256K1;
use bitcoin::bip32::ChildNumber;
use bitcoin::bip32::DerivationPath;
use bitcoin::bip32::Xpriv;
use bitcoin::hashes::sha256::Hash as Sha256;
use bitcoin::hashes::Hash;
use bitcoin::key::Secp256k1;
use bitcoin::secp256k1;
use cashu_kvac::models::{
    AmountAttribute, Coin, MintPrivateKey, MintPublicKey, RandomizedCoin, RangeZKP,
    ScriptAttribute, MAC, ZKP,
};
use cashu_kvac::secp::{GroupElement, Scalar};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::VecSkipError;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Incorrect KVAC KeySet ID")]
    IncorrectKeySetId,
    /// Bip32 Error
    #[error(transparent)]
    Bip32(#[from] bitcoin::bip32::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKvacKeys {
    pub private_key: MintPrivateKey,
    pub public_key: MintPublicKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvacKeys(pub MintPublicKey);

impl From<MintKvacKeys> for KvacKeys {
    fn from(keys: MintKvacKeys) -> Self {
        Self(keys.public_key)
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
        xpriv = xpriv
            .derive_priv(secp, &derivation_path)
            .expect("RNG busted");
        xpriv = xpriv
            .derive_priv(
                secp,
                &[ChildNumber::from_hardened_idx(derivation_path_index)
                    .expect("derivation_path_index is a valid index")],
            )
            .expect("RNG busted");
        let scalars: Vec<Scalar> = (0..6)
            .map(|i| {
                Scalar::new(
                    &xpriv
                        .derive_priv(
                            secp,
                            &[ChildNumber::from_hardened_idx(i as u32)
                                .expect("order is valid index")],
                        )
                        .expect("RNG busted")
                        .private_key
                        .secret_bytes(),
                )
            })
            .collect();
        let private_key =
            MintPrivateKey::from_scalars(&scalars).expect("couldn't generate KVAC privkey");
        let kvac_keys = MintKvacKeys {
            private_key: private_key.clone(),
            public_key: private_key.public_key,
        };
        let pub_kvac_keys: KvacKeys = kvac_keys.clone().into();
        Self {
            id: (&pub_kvac_keys).into(),
            unit,
            kvac_keys,
        }
    }
}

/// Keyset
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacKeySet {
    /// Keyset [`Id`]
    pub id: Id,
    /// Keyset [`CurrencyUnit`]
    pub unit: CurrencyUnit,
    /// Keyset [`KvacKeys`]
    pub kvac_keys: KvacKeys,
}

impl KvacKeySet {
    /// Verify the keyset is matches keys
    pub fn verify_id(&self) -> Result<(), Error> {
        let keys_id: Id = (&self.kvac_keys).into();

        if keys_id != self.id {
            return Err(Error::IncorrectKeySetId);
        }

        Ok(())
    }
}

#[cfg(feature = "mint")]
impl From<MintKvacKeySet> for KvacKeySet {
    fn from(keyset: MintKvacKeySet) -> Self {
        Self {
            id: keyset.id,
            unit: keyset.unit,
            kvac_keys: KvacKeys::from(keyset.kvac_keys),
        }
    }
}

/// Kvac Coin Message
///
/// A kvac coin as intended to be seen by the Mint.
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
    /// Unique identifier used by the Mint to create the algebraic MAC
    /// and for recovery purporses
    #[serde(rename = "t")]
    pub t_tag: Scalar,
    /// Pair of commitments
    ///
    /// Pair ([`GroupElement`], [`GroupElement`]) that represent:
    /// 1) Value: encoding value 0
    /// 2) Script: encoding a custom script (Mint doesn't care)
    #[serde(rename = "c")]
    pub commitments: (GroupElement, GroupElement),
}

impl From<&KvacPreCoin> for KvacCoinMessage {
    fn from(c: &KvacPreCoin) -> Self {
        Self {
            keyset_id: c.keyset_id,
            t_tag: c.t_tag.clone(),
            commitments: (c.attributes.0.commitment(), c.attributes.1.commitment()),
        }
    }
}

/// Coin without a MAC
///
/// A kvac coin as intended to be seen by the Mint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacPreCoin {
    /// Keyset ID
    ///
    /// ID from which we expect a signature.
    pub keyset_id: Id,
    /// Amount
    ///
    /// Amount encoded in [`AmountAttribute`]
    /// (for easier retrieval)
    pub amount: Amount,
    /// Script
    ///
    /// Script encoded in [`ScriptAttribute`]
    pub script: Option<String>,
    /// CurrencyUnit
    ///
    /// Unit of the coin
    pub unit: CurrencyUnit,
    /// Tag
    ///
    /// Unique identifier used to create the algebraic MAC from
    /// and for recovery purporses
    pub t_tag: Scalar,
    /// Pair of attributes
    ///
    /// Pair ([`AmountAttribute`], [`ScriptAttribute`]) that represent:
    /// 1) Value: encoding value 0
    /// 2) Script: encoding a custom script
    pub attributes: (AmountAttribute, ScriptAttribute),
}

impl KvacPreCoin {
    pub fn from_xpriv(
        keyset_id: Id,
        amount: Amount,
        unit: CurrencyUnit,
        script: Option<String>,
        counter: u32,
        xpriv: Xpriv,
    ) -> Result<Self, Error> {
        let t_path = derive_path_from_kvac_keyset_id(keyset_id)?
            .child(ChildNumber::from_hardened_idx(counter)?)
            .child(ChildNumber::from_normal_idx(0)?);
        let r_a_path = derive_path_from_kvac_keyset_id(keyset_id)?
            .child(ChildNumber::from_hardened_idx(counter)?)
            .child(ChildNumber::from_normal_idx(1)?);
        let r_s_path = derive_path_from_kvac_keyset_id(keyset_id)?
            .child(ChildNumber::from_hardened_idx(counter)?)
            .child(ChildNumber::from_normal_idx(2)?);

        let t_xpriv = xpriv.derive_priv(&SECP256K1, &t_path)?;
        let r_a_priv = xpriv.derive_priv(&SECP256K1, &r_a_path)?;
        let r_s_priv = xpriv.derive_priv(&SECP256K1, &r_s_path)?;

        let t_tag = Scalar::new(&t_xpriv.private_key.secret_bytes());
        let a = AmountAttribute::new(amount.0, Some(&r_a_priv.private_key.secret_bytes()));
        let s = match script.clone() {
            Some(script_vec) => ScriptAttribute::new(
                script_vec.as_bytes(),
                Some(&r_s_priv.private_key.secret_bytes()),
            ),
            None => ScriptAttribute::new(b"", Some(&r_s_priv.private_key.secret_bytes())),
        };

        Ok(Self {
            keyset_id,
            amount,
            script,
            unit,
            t_tag,
            attributes: (a, s),
        })
    }

    pub fn new(keyset_id: Id, amount: Amount, unit: CurrencyUnit, script: Option<String>) -> Self {
        let t_tag = Scalar::random();
        let a = AmountAttribute::new(amount.0, None);
        let s = match script.clone() {
            Some(script_vec) => ScriptAttribute::new(script_vec.as_bytes(), None),
            None => ScriptAttribute::new(b"", None),
        };

        Self {
            keyset_id,
            amount,
            script,
            unit,
            t_tag,
            attributes: (a, s),
        }
    }
}

/// Kvac Coin
///
/// A KVAC coin as intended to be saved in the wallet.
#[cfg(feature = "wallet")]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacCoin {
    /// Keyset ID
    ///
    /// [`ID`] from which we expect a signature.
    pub keyset_id: Id,
    /// Amount
    ///
    /// Amount encoded in AmountAttribute
    /// (for easier retrieval)
    pub amount: Amount,
    /// Script
    ///
    /// Script encoded in ScriptAttribute
    pub script: Option<String>,
    /// CurrencyUnit
    ///
    /// Unit of the coin
    pub unit: CurrencyUnit,
    /// Coin
    ///
    /// [`Coin`] containing [`MAC`], [`AmountAttribute`] and [`ScriptAttribute`]
    pub coin: Coin,
}

/// Kvac Coin
///
/// A KVAC coin as intended to be saved in the wallet.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacRandomizedCoin {
    /// Keyset ID
    ///
    /// [`ID`] from which we expect a signature.
    pub keyset_id: Id,
    /// Script
    ///
    /// Script encoded in ScriptAttribute **IF** the client intends to reveal it
    pub script: Option<String>,
    /// Unit
    ///
    /// Unit of the coin
    pub unit: CurrencyUnit,
    /// Randomized Coin
    ///
    /// [`RandomizedCoin`] version of a [`Coin`]
    pub randomized_coin: RandomizedCoin,
}

impl From<&KvacCoin> for KvacRandomizedCoin {
    fn from(coin: &KvacCoin) -> Self {
        Self {
            randomized_coin: RandomizedCoin::from_coin(&coin.coin, true).expect(""),
            keyset_id: coin.keyset_id,
            script: coin.script.clone(),
            unit: coin.unit.clone(),
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvacNullifier {
    pub nullifier: GroupElement,
    pub keyset_id: Id,
    pub quote_id: Option<Uuid>,
    pub state: State,
}

impl KvacNullifier {
    pub fn set_quote_id(self, quote_id: Uuid) -> Self {
        Self {
            quote_id: Some(quote_id),
            ..self
        }
    }

    pub fn set_state(self, state: State) -> Self {
        Self { state, ..self }
    }
}

impl From<&KvacRandomizedCoin> for KvacNullifier {
    fn from(coin: &KvacRandomizedCoin) -> Self {
        Self {
            keyset_id: coin.keyset_id.clone(),
            nullifier: coin.randomized_coin.Ca.clone(),
            quote_id: None,
            state: State::Unspent,
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacIssuedMac {
    pub mac: MAC,
    pub commitments: (GroupElement, GroupElement),
    pub keyset_id: Id,
    pub quote_id: Option<Uuid>,
}

// --- Helpers ---

fn derive_path_from_kvac_keyset_id(id: Id) -> Result<DerivationPath, Error> {
    let index = u32::from(id);

    let keyset_child_number = ChildNumber::from_hardened_idx(index)?;
    Ok(DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(129372)?,
        ChildNumber::from_hardened_idx(1)?,
        keyset_child_number,
    ]))
}

// --- Requests ---

/// Bootstrap Request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BootstrapRequest {
    /// Outputs
    ///
    /// [`Vec<KvacCoinMessage>`] Where each element is a coin encoding 0 as an amount.
    #[cfg_attr(feature = "swagger", schema(max_items = 1_000, min_items = 2))]
    pub outputs: Vec<KvacCoinMessage>,
    /// Bootstrap Proofs
    ///
    /// [`Vec<ZKP>`] proving that each coin is worth 0
    #[cfg_attr(feature = "swagger", schema(max_items = 1_000, min_items = 2))]
    pub proofs: Vec<ZKP>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacSwapRequest {
    /// Inputs
    ///
    /// [`Vec<KvacRandomizedCoin>`] Where each element is the randomized version of a [`KvacCoin`] for
    /// which a [`MAC`] was issued. In other words, the outputs of a previous request but randomized.
    pub inputs: Vec<KvacRandomizedCoin>,
    /// Outputs
    ///
    /// [`Vec<KvacCoinMessage>`] Where elements are new coins awaiting their [`MAC`]
    pub outputs: Vec<KvacCoinMessage>,
    /// Balance Proofs
    ///
    /// [`ZKP`] Proving that inputs - outputs == delta_amount
    pub balance_proof: ZKP,
    /// MAC Proofs
    ///
    /// [`Vec<ZKP>`] Proofs that inputs where issued a MAC previously
    pub mac_proofs: Vec<ZKP>,
    /// Script
    ///
    /// [`String`] revealing the script to unlock the inputs
    pub script: Option<String>,
    /// Range Proof
    ///
    /// A single [`RangeProof`] proving the outputs are all within range
    pub range_proof: RangeZKP,
}

// --- Responses ---
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacKeysResponse {
    pub kvac_keysets: Vec<KvacKeySet>,
}

/// Ids of mints keyset ids
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacKeysetResponse {
    /// set of public key ids that the mint generates
    #[serde_as(as = "VecSkipError<_>")]
    pub kvac_keysets: Vec<KeySetInfo>,
}

/// Bootstrap Response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BootstrapResponse {
    /// MACs
    ///
    /// [`Vec<MAC>`] Approval stamp of the Mint
    pub macs: Vec<MAC>,
    /// IParams Proofs
    ///
    /// [`Vec<ZKP>`] Proving that a certain [`MintPrivateKey`] was used to issue each [`MAC`]
    pub proofs: Vec<ZKP>,
}

/// Swap Response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacSwapResponse {
    /// MACs
    ///
    /// [`Vec<MAC>`] Approval stamp of the Mint
    pub macs: Vec<MAC>,
    /// IParams Proofs
    ///
    /// [`Vec<ZKP>`] Proving that a certain [`MintPrivateKey`] was used to issue each [`MAC`]
    pub proofs: Vec<ZKP>,
}
