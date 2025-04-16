use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::hashes::sha256::Hash as Sha256;
use bitcoin::hashes::Hash;
use bitcoin::key::Secp256k1;
use bitcoin::secp256k1;
use cashu_kvac::models::{
    AmountAttribute, Coin, MintPrivateKey, MintPublicKey, RandomizedCoin, RangeZKP,
    ScriptAttribute, MAC, ZKP,
};
use cashu_kvac::secp::{GroupElement, Scalar};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, VecSkipError};
use thiserror::Error;
use uuid::Uuid;

use super::nut02::KeySetVersion;
use super::{CurrencyUnit, Id, KeySetInfo, State};
use crate::nut05::QuoteState;
use crate::{Amount, SECP256K1};

#[derive(Debug, Error)]
pub enum Error {
    /// KVAC Request Invalid Length
    #[error("Invalid input length for this request")]
    RequestInvalidInputLength,
    /// KVAC Request Invalid Length
    #[error("Invalid output length for this request")]
    RequestInvalidOutputLength,
    /// KVAC Proofs and inputs mismatch
    #[error("Number of inputs does not match number of proofs provided")]
    InputsToProofsLengthMismatch,
    /// KVAC Bootstrap proofs failed to verify
    #[error("Failed to verify one of the provided proofs")]
    BootstrapVerificationError,
    /// KVAC IParams proofs failed to verify
    #[error("Failed to verify one of the provided proofs")]
    IParamsVerificationError,
    /// Out of bounds
    #[error("Out of bounds")]
    OutOfBounds,
    /// KVAC Mac was already issued for outputs
    #[error("MAC was already issued for these outputs")]
    MacAlreadyIssued,
    /// KVAC BalanceProof failed to verify
    #[error("Balance proof failed to verify with delta = `{0}` and fee `{1}`")]
    BalanceVerificationError(i64, i64),
    /// KVAC MacProof failed to verify
    #[error("Mac proof failed to verify")]
    MacVerificationError,
    /// KVAC RangeProof failed to verify
    #[error("Range proof failed to verify. One of the outputs is not within range")]
    RangeProofVerificationError,
    /// KVAC Script is not the same for all coins
    #[error("Script is not the same across all coins")]
    DifferentScriptsError,
    /// KVAC No zero-value coins available
    #[error("No zero valued coins available: mint some with a bootstrap request")]
    NoZeroValueCoins,
    /// KVAC Not enough coins available
    #[error("Not enough coins available")]
    NotEnoughCoins,
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
        let hash_bytes = hash.to_byte_array()[0..Self::BYTELEN]
            .to_vec()
            .try_into()
            .expect("Invalid length of hex id");
        Id {
            version: KeySetVersion::Version00,
            id: hash_bytes,
        }
    }
}

#[cfg(feature = "mint")]
impl MintKvacKeySet {
    /// Generate new [`MintKvacKeySet`]
    pub fn generate<C: secp256k1::Signing>(
        secp: &Secp256k1<C>,
        mut xpriv: Xpriv,
        unit: CurrencyUnit,
        derivation_path: DerivationPath,
    ) -> Self {
        xpriv = xpriv
            .derive_priv(secp, &derivation_path)
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
/// A kvac coin to be sent as an output:
///     * keyset ID
///     * commitments
///     * identifying tag
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
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
    /// 1) Value: holds the [`Scalar`] of the amount and its blinding factor
    /// 2) Script: holds the [`Scalar`] of the scripthash and its blinding factor
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
    /// Issuance proof
    /// 
    /// [`ZKP`] proving the issuance of this coin
    pub issuance_proof: ZKP,
}

/// Kvac Coin
///
/// A KVAC coin to be sent as input.
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

impl KvacRandomizedCoin {
    pub fn get_nullifier(&self) -> GroupElement {
        self.randomized_coin.Ca.clone()
    }
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
            keyset_id: coin.keyset_id,
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
    pub issuance_proof: ZKP,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacCoinState {
    pub nullifier: GroupElement,
    pub state: State,
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
pub struct KvacBootstrapRequest {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// Range Proof
    ///
    /// A single [`RangeProof`] proving the outputs are all within range
    pub range_proof: RangeZKP,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct KvacMintBolt11Request<Q> {
    /// Quote id
    #[cfg_attr(feature = "swagger", schema(max_length = 1_000))]
    pub quote: Q,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// Range Proof
    ///
    /// A single [`RangeProof`] proving the outputs are all within range
    pub range_proof: RangeZKP,
}

pub type KvacMeltBolt11Request<Q> = KvacMintBolt11Request<Q>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacRestoreRequest {
    /// Each tag should match an output for which [`MAC`]s were previously issued
    pub tags: Vec<Scalar>,
}

/// Check spendable request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacCheckStateRequest {
    /// nullifiers of the coins to check
    #[cfg_attr(feature = "swagger", schema(value_type = Vec<String>, max_items = 1_000))]
    pub nullifiers: Vec<GroupElement>,
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

/// Swap Response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacResponse {
    pub issued_macs: Vec<KvacIssuedMac>
}

/// Bootstrap Response
pub type KvacBootstrapResponse = KvacResponse;

/// Swap Response
pub type KvacSwapResponse = KvacResponse;

/// Mint Bolt11 Response
pub type KvacMintBolt11Response = KvacResponse;

/// Melt Bolt11 Response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacMeltBolt11Response {
    /// Status of the operation
    pub state: QuoteState,
    /// Lightning fee return
    ///
    /// [`Amount`] added to the first output as a lightning overpaid-fee return
    pub fee_return: Amount,
    /// Payment preimage
    ///
    /// [`Option<String>`] holding the pre-image to the payment
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preimage: Option<String>,
    /// Issued MACs
    ///
    /// Outputs of the response with the remaining balance + returned fees
    pub issued_macs: Vec<KvacIssuedMac>,
}

/// Restore Response
pub type KvacRestoreResponse = KvacResponse;

/// Check state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacCheckStateResponse {
    /// Proof states
    pub states: Vec<KvacCoinState>,
}
