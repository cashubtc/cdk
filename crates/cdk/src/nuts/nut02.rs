//! NUT-02: Keysets and keyset ID
//!
//! <https://github.com/cashubtc/nuts/blob/main/02.md>

use core::fmt;
use core::str::FromStr;
use std::array::TryFromSliceError;
#[cfg(feature = "mint")]
use std::collections::BTreeMap;

#[cfg(feature = "mint")]
use bitcoin::bip32::DerivationPath;
#[cfg(feature = "mint")]
use bitcoin::bip32::{ChildNumber, Xpriv};
use bitcoin::hashes::sha256::Hash as Sha256;
use bitcoin::hashes::Hash;
#[cfg(feature = "mint")]
use bitcoin::key::Secp256k1;
#[cfg(feature = "mint")]
use bitcoin::secp256k1;
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::{serde_as, VecSkipError};
use thiserror::Error;

use super::nut01::Keys;
#[cfg(feature = "mint")]
use super::nut01::{MintKeyPair, MintKeys};
use crate::amount::AmountStr;
use crate::nuts::nut00::CurrencyUnit;
use crate::util::hex;
#[cfg(feature = "mint")]
use crate::Amount;

/// NUT02 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Hex Error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    /// Keyset length error
    #[error("NUT02: ID length invalid")]
    Length,
    /// Unknown version
    #[error("NUT02: Unknown Version")]
    UnknownVersion,
    /// Slice Error
    #[error(transparent)]
    Slice(#[from] TryFromSliceError),
}

/// Keyset version
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeySetVersion {
    /// Current Version 00
    Version00,
}

impl KeySetVersion {
    /// [`KeySetVersion`] to byte
    pub fn to_byte(&self) -> u8 {
        match self {
            Self::Version00 => 0,
        }
    }

    /// [`KeySetVersion`] from byte
    pub fn from_byte(byte: &u8) -> Result<Self, Error> {
        match byte {
            0 => Ok(Self::Version00),
            _ => Err(Error::UnknownVersion),
        }
    }
}

impl fmt::Display for KeySetVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeySetVersion::Version00 => f.write_str("00"),
        }
    }
}

/// A keyset ID is an identifier for a specific keyset. It can be derived by
/// anyone who knows the set of public keys of a mint. The keyset ID **CAN**
/// be stored in a Cashu token such that the token can be used to identify
/// which mint or keyset it was generated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id {
    version: KeySetVersion,
    id: [u8; Self::BYTELEN],
}

impl Id {
    const STRLEN: usize = 14;
    const BYTELEN: usize = 7;

    /// [`Id`] to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        [vec![self.version.to_byte()], self.id.to_vec()].concat()
    }

    /// [`Id`] from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self {
            version: KeySetVersion::from_byte(&bytes[0])?,
            id: bytes[1..].try_into()?,
        })
    }
}

impl TryFrom<Id> for u64 {
    type Error = Error;
    fn try_from(value: Id) -> Result<Self, Self::Error> {
        let hex_bytes: [u8; 8] = value.to_bytes().try_into().map_err(|_| Error::Length)?;

        let int = u64::from_be_bytes(hex_bytes);

        Ok(int % (2_u64.pow(31) - 1))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format!("{}{}", self.version, hex::encode(self.id)))
    }
}

impl FromStr for Id {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Check if the string length is valid
        if s.len() != 16 {
            return Err(Error::Length);
        }

        Ok(Self {
            version: KeySetVersion::Version00,
            id: hex::decode(&s[2..])?
                .try_into()
                .map_err(|_| Error::Length)?,
        })
    }
}

impl Serialize for Id {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdVisitor;

        impl<'de> serde::de::Visitor<'de> for IdVisitor {
            type Value = Id;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("Expecting a 14 char hex string")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Id::from_str(v).map_err(|e| match e {
                    Error::Length => E::custom(format!(
                        "Invalid Length: Expected {}, got {}:
                        {}",
                        Id::STRLEN,
                        v.len(),
                        v
                    )),
                    _ => E::custom(e),
                })
            }
        }

        deserializer.deserialize_str(IdVisitor)
    }
}

impl From<&Keys> for Id {
    fn from(map: &Keys) -> Self {
        // REVIEW: Is it 16 or 14 bytes
        /* NUT-02
            1 - sort public keys by their amount in ascending order
            2 - concatenate all public keys to one string
            3 - HASH_SHA256 the concatenated public keys
            4 - take the first 14 characters of the hex-encoded hash
            5 - prefix it with a keyset ID version byte
        */

        let mut keys: Vec<(&AmountStr, &super::PublicKey)> = map.iter().collect();

        keys.sort_by_key(|(amt, _v)| *amt);

        let pubkeys_concat: Vec<u8> = keys
            .iter()
            .map(|(_, pubkey)| pubkey.to_bytes())
            .collect::<Vec<[u8; 33]>>()
            .concat();

        let hash = Sha256::hash(&pubkeys_concat);
        let hex_of_hash = hex::encode(hash.to_byte_array());

        Self {
            version: KeySetVersion::Version00,
            id: hex::decode(&hex_of_hash[0..Self::STRLEN])
                .expect("Keys hash could not be hex decoded")
                .try_into()
                .expect("Invalid length of hex id"),
        }
    }
}

/// Mint Keysets [NUT-02]
/// Ids of mints keyset ids
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeysetResponse {
    /// set of public key ids that the mint generates
    #[serde_as(as = "VecSkipError<_>")]
    pub keysets: Vec<KeySetInfo>,
}

/// Keyset
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct KeySet {
    /// Keyset [`Id`]
    pub id: Id,
    /// Keyset [`CurrencyUnit`]
    pub unit: CurrencyUnit,
    /// Keyset [`Keys`]
    pub keys: Keys,
}

#[cfg(feature = "mint")]
impl From<MintKeySet> for KeySet {
    fn from(keyset: MintKeySet) -> Self {
        Self {
            id: keyset.id,
            unit: keyset.unit,
            keys: Keys::from(keyset.keys),
        }
    }
}

/// KeySetInfo
#[derive(Debug, Clone, Hash, PartialEq, Eq, Deserialize, Serialize)]
pub struct KeySetInfo {
    /// Keyset [`Id`]
    pub id: Id,
    /// Keyset [`CurrencyUnit`]
    pub unit: CurrencyUnit,
    /// Keyset state
    /// Mint will only sign from an active keyset
    pub active: bool,
    /// Input Fee PPK
    #[serde(default = "default_input_fee_ppk")]
    pub input_fee_ppk: u64,
}

fn default_input_fee_ppk() -> u64 {
    0
}

/// MintKeyset
#[cfg(feature = "mint")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeySet {
    /// Keyset [`Id`]
    pub id: Id,
    /// Keyset [`CurrencyUnit`]
    pub unit: CurrencyUnit,
    /// Keyset [`MintKeys`]
    pub keys: MintKeys,
}

#[cfg(feature = "mint")]
impl MintKeySet {
    /// Generate new [`MintKeySet`]
    pub fn generate<C: secp256k1::Signing>(
        secp: &Secp256k1<C>,
        xpriv: Xpriv,
        unit: CurrencyUnit,
        max_order: u8,
    ) -> Self {
        let mut map = BTreeMap::new();
        for i in 0..max_order {
            let amount = Amount::from(2_u64.pow(i as u32));
            let secret_key = xpriv
                .derive_priv(
                    secp,
                    &[ChildNumber::from_hardened_idx(i as u32).expect("order is valid index")],
                )
                .expect("RNG busted")
                .private_key;
            let public_key = secret_key.public_key(secp);
            map.insert(
                amount,
                MintKeyPair {
                    secret_key: secret_key.into(),
                    public_key: public_key.into(),
                },
            );
        }

        let keys = MintKeys::new(map);
        Self {
            id: (&keys).into(),
            unit,
            keys,
        }
    }

    /// Generate new [`MintKeySet`] from seed
    pub fn generate_from_seed<C: secp256k1::Signing>(
        secp: &Secp256k1<C>,
        seed: &[u8],
        max_order: u8,
        currency_unit: CurrencyUnit,
        derivation_path: DerivationPath,
    ) -> Self {
        let xpriv = Xpriv::new_master(bitcoin::Network::Bitcoin, seed).expect("RNG busted");
        Self::generate(
            secp,
            xpriv
                .derive_priv(secp, &derivation_path)
                .expect("RNG busted"),
            currency_unit,
            max_order,
        )
    }

    /// Generate new [`MintKeySet`] from xpriv
    pub fn generate_from_xpriv<C: secp256k1::Signing>(
        secp: &Secp256k1<C>,
        xpriv: Xpriv,
        max_order: u8,
        currency_unit: CurrencyUnit,
        derivation_path: DerivationPath,
    ) -> Self {
        Self::generate(
            secp,
            xpriv
                .derive_priv(secp, &derivation_path)
                .expect("RNG busted"),
            currency_unit,
            max_order,
        )
    }
}

#[cfg(feature = "mint")]
impl From<MintKeySet> for Id {
    fn from(keyset: MintKeySet) -> Id {
        let keys: super::KeySet = keyset.into();

        Id::from(&keys.keys)
    }
}

#[cfg(feature = "mint")]
impl From<&MintKeys> for Id {
    fn from(map: &MintKeys) -> Self {
        let keys: super::Keys = map.clone().into();

        Id::from(&keys)
    }
}

#[cfg(test)]
mod test {

    use std::str::FromStr;

    use super::{KeySetInfo, Keys, KeysetResponse};
    use crate::nuts::nut02::Id;
    use crate::nuts::KeysResponse;

    const SHORT_KEYSET_ID: &str = "00456a94ab4e1c46";
    const SHORT_KEYSET: &str = r#"
        {
            "1":"03a40f20667ed53513075dc51e715ff2046cad64eb68960632269ba7f0210e38bc",
            "2":"03fd4ce5a16b65576145949e6f99f445f8249fee17c606b688b504a849cdc452de",
            "4":"02648eccfa4c026960966276fa5a4cae46ce0fd432211a4f449bf84f13aa5f8303",
            "8":"02fdfd6796bfeac490cbee12f778f867f0a2c68f6508d17c649759ea0dc3547528"
        }
    "#;

    const KEYSET_ID: &str = "000f01df73ea149a";
    const KEYSET: &str = r#"
        {
            "1":"03ba786a2c0745f8c30e490288acd7a72dd53d65afd292ddefa326a4a3fa14c566",
            "2":"03361cd8bd1329fea797a6add1cf1990ffcf2270ceb9fc81eeee0e8e9c1bd0cdf5",
            "4":"036e378bcf78738ddf68859293c69778035740e41138ab183c94f8fee7572214c7",
            "8":"03909d73beaf28edfb283dbeb8da321afd40651e8902fcf5454ecc7d69788626c0",
            "16":"028a36f0e6638ea7466665fe174d958212723019ec08f9ce6898d897f88e68aa5d",
            "32":"03a97a40e146adee2687ac60c2ba2586a90f970de92a9d0e6cae5a4b9965f54612",
            "64":"03ce86f0c197aab181ddba0cfc5c5576e11dfd5164d9f3d4a3fc3ffbbf2e069664",
            "128":"0284f2c06d938a6f78794814c687560a0aabab19fe5e6f30ede38e113b132a3cb9",
            "256":"03b99f475b68e5b4c0ba809cdecaae64eade2d9787aa123206f91cd61f76c01459",
            "512":"03d4db82ea19a44d35274de51f78af0a710925fe7d9e03620b84e3e9976e3ac2eb",
            "1024":"031fbd4ba801870871d46cf62228a1b748905ebc07d3b210daf48de229e683f2dc",
            "2048":"0276cedb9a3b160db6a158ad4e468d2437f021293204b3cd4bf6247970d8aff54b",
            "4096":"02fc6b89b403ee9eb8a7ed457cd3973638080d6e04ca8af7307c965c166b555ea2",
            "8192":"0320265583e916d3a305f0d2687fcf2cd4e3cd03a16ea8261fda309c3ec5721e21",
            "16384":"036e41de58fdff3cb1d8d713f48c63bc61fa3b3e1631495a444d178363c0d2ed50",
            "32768":"0365438f613f19696264300b069d1dad93f0c60a37536b72a8ab7c7366a5ee6c04",
            "65536":"02408426cfb6fc86341bac79624ba8708a4376b2d92debdf4134813f866eb57a8d",
            "131072":"031063e9f11c94dc778c473e968966eac0e70b7145213fbaff5f7a007e71c65f41",
            "262144":"02f2a3e808f9cd168ec71b7f328258d0c1dda250659c1aced14c7f5cf05aab4328",
            "524288":"038ac10de9f1ff9395903bb73077e94dbf91e9ef98fd77d9a2debc5f74c575bc86",
            "1048576":"0203eaee4db749b0fc7c49870d082024b2c31d889f9bc3b32473d4f1dfa3625788",
            "2097152":"033cdb9d36e1e82ae652b7b6a08e0204569ec7ff9ebf85d80a02786dc7fe00b04c",
            "4194304":"02c8b73f4e3a470ae05e5f2fe39984d41e9f6ae7be9f3b09c9ac31292e403ac512",
            "8388608":"025bbe0cfce8a1f4fbd7f3a0d4a09cb6badd73ef61829dc827aa8a98c270bc25b0",
            "16777216":"037eec3d1651a30a90182d9287a5c51386fe35d4a96839cf7969c6e2a03db1fc21",
            "33554432":"03280576b81a04e6abd7197f305506476f5751356b7643988495ca5c3e14e5c262",
            "67108864":"03268bfb05be1dbb33ab6e7e00e438373ca2c9b9abc018fdb452d0e1a0935e10d3",
            "134217728":"02573b68784ceba9617bbcc7c9487836d296aa7c628c3199173a841e7a19798020",
            "268435456":"0234076b6e70f7fbf755d2227ecc8d8169d662518ee3a1401f729e2a12ccb2b276",
            "536870912":"03015bd88961e2a466a2163bd4248d1d2b42c7c58a157e594785e7eb34d880efc9",
            "1073741824":"02c9b076d08f9020ebee49ac8ba2610b404d4e553a4f800150ceb539e9421aaeee",
            "2147483648":"034d592f4c366afddc919a509600af81b489a03caf4f7517c2b3f4f2b558f9a41a",
            "4294967296":"037c09ecb66da082981e4cbdb1ac65c0eb631fc75d85bed13efb2c6364148879b5",
            "8589934592":"02b4ebb0dda3b9ad83b39e2e31024b777cc0ac205a96b9a6cfab3edea2912ed1b3",
            "17179869184":"026cc4dacdced45e63f6e4f62edbc5779ccd802e7fabb82d5123db879b636176e9",
            "34359738368":"02b2cee01b7d8e90180254459b8f09bbea9aad34c3a2fd98c85517ecfc9805af75",
            "68719476736":"037a0c0d564540fc574b8bfa0253cca987b75466e44b295ed59f6f8bd41aace754",
            "137438953472":"021df6585cae9b9ca431318a713fd73dbb76b3ef5667957e8633bca8aaa7214fb6",
            "274877906944":"02b8f53dde126f8c85fa5bb6061c0be5aca90984ce9b902966941caf963648d53a",
            "549755813888":"029cc8af2840d59f1d8761779b2496623c82c64be8e15f9ab577c657c6dd453785",
            "1099511627776":"03e446fdb84fad492ff3a25fc1046fb9a93a5b262ebcd0151caa442ea28959a38a",
            "2199023255552":"02d6b25bd4ab599dd0818c55f75702fde603c93f259222001246569018842d3258",
            "4398046511104":"03397b522bb4e156ec3952d3f048e5a986c20a00718e5e52cd5718466bf494156a",
            "8796093022208":"02d1fb9e78262b5d7d74028073075b80bb5ab281edcfc3191061962c1346340f1e",
            "17592186044416":"030d3f2ad7a4ca115712ff7f140434f802b19a4c9b2dd1c76f3e8e80c05c6a9310",
            "35184372088832":"03e325b691f292e1dfb151c3fb7cad440b225795583c32e24e10635a80e4221c06",
            "70368744177664":"03bee8f64d88de3dee21d61f89efa32933da51152ddbd67466bef815e9f93f8fd1",
            "140737488355328":"0327244c9019a4892e1f04ba3bf95fe43b327479e2d57c25979446cc508cd379ed",
            "281474976710656":"02fb58522cd662f2f8b042f8161caae6e45de98283f74d4e99f19b0ea85e08a56d",
            "562949953421312":"02adde4b466a9d7e59386b6a701a39717c53f30c4810613c1b55e6b6da43b7bc9a",
            "1125899906842624":"038eeda11f78ce05c774f30e393cda075192b890d68590813ff46362548528dca9",
            "2251799813685248":"02ec13e0058b196db80f7079d329333b330dc30c000dbdd7397cbbc5a37a664c4f",
            "4503599627370496":"02d2d162db63675bd04f7d56df04508840f41e2ad87312a3c93041b494efe80a73",
            "9007199254740992":"0356969d6aef2bb40121dbd07c68b6102339f4ea8e674a9008bb69506795998f49",
            "18014398509481984":"02f4e667567ebb9f4e6e180a4113bb071c48855f657766bb5e9c776a880335d1d6",
            "36028797018963968":"0385b4fe35e41703d7a657d957c67bb536629de57b7e6ee6fe2130728ef0fc90b0",
            "72057594037927936":"02b2bc1968a6fddbcc78fb9903940524824b5f5bed329c6ad48a19b56068c144fd",
            "144115188075855872":"02e0dbb24f1d288a693e8a49bc14264d1276be16972131520cf9e055ae92fba19a",
            "288230376151711744":"03efe75c106f931a525dc2d653ebedddc413a2c7d8cb9da410893ae7d2fa7d19cc",
            "576460752303423488":"02c7ec2bd9508a7fc03f73c7565dc600b30fd86f3d305f8f139c45c404a52d958a",
            "1152921504606846976":"035a6679c6b25e68ff4e29d1c7ef87f21e0a8fc574f6a08c1aa45ff352c1d59f06",
            "2305843009213693952":"033cdc225962c052d485f7cfbf55a5b2367d200fe1fe4373a347deb4cc99e9a099",
            "4611686018427387904":"024a4b806cf413d14b294719090a9da36ba75209c7657135ad09bc65328fba9e6f",
            "9223372036854775808":"0377a6fe114e291a8d8e991627c38001c8305b23b9e98b1c7b1893f5cd0dda6cad"
        }
    "#;

    #[test]
    fn test_deserialization_and_id_generation() {
        let _id = Id::from_str("009a1f293253e41e").unwrap();

        let keys: Keys = serde_json::from_str(SHORT_KEYSET).unwrap();

        let id: Id = (&keys).into();

        assert_eq!(id, Id::from_str(SHORT_KEYSET_ID).unwrap());

        let keys: Keys = serde_json::from_str(KEYSET).unwrap();

        let id: Id = (&keys).into();

        assert_eq!(id, Id::from_str(KEYSET_ID).unwrap());
    }

    #[test]
    fn test_deserialization_keyset_info() {
        let h = r#"{"id":"009a1f293253e41e","unit":"sat","active":true}"#;

        let _keyset_response: KeySetInfo = serde_json::from_str(h).unwrap();
    }

    #[test]
    fn test_deserialization_of_keyset_response() {
        let h = r#"{"keysets":[{"id":"009a1f293253e41e","unit":"sat","active":true, "input_fee_ppk": 100},{"id":"eGnEWtdJ0PIM","unit":"sat","active":true},{"id":"003dfdf4e5e35487","unit":"sat","active":true},{"id":"0066ad1a4b6fc57c","unit":"sat","active":true},{"id":"00f7ca24d44c3e5e","unit":"sat","active":true},{"id":"001fcea2931f2d85","unit":"sat","active":true},{"id":"00d095959d940edb","unit":"sat","active":true},{"id":"000d7f730d657125","unit":"sat","active":true},{"id":"0007208d861d7295","unit":"sat","active":true},{"id":"00bfdf8889b719dd","unit":"sat","active":true},{"id":"00ca9b17da045f21","unit":"sat","active":true}]}"#;

        let _keyset_response: KeysetResponse = serde_json::from_str(h).unwrap();
    }

    #[test]
    fn test_to_int() {
        let id = Id::from_str("009a1f293253e41e").unwrap();

        let id_int = u64::try_from(id).unwrap();
        assert_eq!(864559728, id_int)
    }

    #[test]
    fn test_keyset_bytes() {
        let id = Id::from_str("009a1f293253e41e").unwrap();

        let id_bytes = id.to_bytes();

        assert_eq!(id_bytes.len(), 8);

        let id_from_bytes = Id::from_bytes(&id_bytes).unwrap();

        assert_eq!(id_from_bytes, id);
    }

    #[test]
    fn test_deserialization_keys_response() {
        let keys = r#"{"keysets":[{"id":"I2yN+iRYfkzT","unit":"sat","keys":{"1":"03ba786a2c0745f8c30e490288acd7a72dd53d65afd292ddefa326a4a3fa14c566","2":"03361cd8bd1329fea797a6add1cf1990ffcf2270ceb9fc81eeee0e8e9c1bd0cdf5","4":"036e378bcf78738ddf68859293c69778035740e41138ab183c94f8fee7572214c7","8":"03909d73beaf28edfb283dbeb8da321afd40651e8902fcf5454ecc7d69788626c0","16":"028a36f0e6638ea7466665fe174d958212723019ec08f9ce6898d897f88e68aa5d","32":"03a97a40e146adee2687ac60c2ba2586a90f970de92a9d0e6cae5a4b9965f54612","64":"03ce86f0c197aab181ddba0cfc5c5576e11dfd5164d9f3d4a3fc3ffbbf2e069664","128":"0284f2c06d938a6f78794814c687560a0aabab19fe5e6f30ede38e113b132a3cb9","256":"03b99f475b68e5b4c0ba809cdecaae64eade2d9787aa123206f91cd61f76c01459","512":"03d4db82ea19a44d35274de51f78af0a710925fe7d9e03620b84e3e9976e3ac2eb","1024":"031fbd4ba801870871d46cf62228a1b748905ebc07d3b210daf48de229e683f2dc","2048":"0276cedb9a3b160db6a158ad4e468d2437f021293204b3cd4bf6247970d8aff54b","4096":"02fc6b89b403ee9eb8a7ed457cd3973638080d6e04ca8af7307c965c166b555ea2","8192":"0320265583e916d3a305f0d2687fcf2cd4e3cd03a16ea8261fda309c3ec5721e21","16384":"036e41de58fdff3cb1d8d713f48c63bc61fa3b3e1631495a444d178363c0d2ed50","32768":"0365438f613f19696264300b069d1dad93f0c60a37536b72a8ab7c7366a5ee6c04","65536":"02408426cfb6fc86341bac79624ba8708a4376b2d92debdf4134813f866eb57a8d","131072":"031063e9f11c94dc778c473e968966eac0e70b7145213fbaff5f7a007e71c65f41","262144":"02f2a3e808f9cd168ec71b7f328258d0c1dda250659c1aced14c7f5cf05aab4328","524288":"038ac10de9f1ff9395903bb73077e94dbf91e9ef98fd77d9a2debc5f74c575bc86","1048576":"0203eaee4db749b0fc7c49870d082024b2c31d889f9bc3b32473d4f1dfa3625788","2097152":"033cdb9d36e1e82ae652b7b6a08e0204569ec7ff9ebf85d80a02786dc7fe00b04c","4194304":"02c8b73f4e3a470ae05e5f2fe39984d41e9f6ae7be9f3b09c9ac31292e403ac512","8388608":"025bbe0cfce8a1f4fbd7f3a0d4a09cb6badd73ef61829dc827aa8a98c270bc25b0","16777216":"037eec3d1651a30a90182d9287a5c51386fe35d4a96839cf7969c6e2a03db1fc21","33554432":"03280576b81a04e6abd7197f305506476f5751356b7643988495ca5c3e14e5c262","67108864":"03268bfb05be1dbb33ab6e7e00e438373ca2c9b9abc018fdb452d0e1a0935e10d3","134217728":"02573b68784ceba9617bbcc7c9487836d296aa7c628c3199173a841e7a19798020","268435456":"0234076b6e70f7fbf755d2227ecc8d8169d662518ee3a1401f729e2a12ccb2b276","536870912":"03015bd88961e2a466a2163bd4248d1d2b42c7c58a157e594785e7eb34d880efc9","1073741824":"02c9b076d08f9020ebee49ac8ba2610b404d4e553a4f800150ceb539e9421aaeee","2147483648":"034d592f4c366afddc919a509600af81b489a03caf4f7517c2b3f4f2b558f9a41a","4294967296":"037c09ecb66da082981e4cbdb1ac65c0eb631fc75d85bed13efb2c6364148879b5","8589934592":"02b4ebb0dda3b9ad83b39e2e31024b777cc0ac205a96b9a6cfab3edea2912ed1b3","17179869184":"026cc4dacdced45e63f6e4f62edbc5779ccd802e7fabb82d5123db879b636176e9","34359738368":"02b2cee01b7d8e90180254459b8f09bbea9aad34c3a2fd98c85517ecfc9805af75","68719476736":"037a0c0d564540fc574b8bfa0253cca987b75466e44b295ed59f6f8bd41aace754","137438953472":"021df6585cae9b9ca431318a713fd73dbb76b3ef5667957e8633bca8aaa7214fb6","274877906944":"02b8f53dde126f8c85fa5bb6061c0be5aca90984ce9b902966941caf963648d53a","549755813888":"029cc8af2840d59f1d8761779b2496623c82c64be8e15f9ab577c657c6dd453785","1099511627776":"03e446fdb84fad492ff3a25fc1046fb9a93a5b262ebcd0151caa442ea28959a38a","2199023255552":"02d6b25bd4ab599dd0818c55f75702fde603c93f259222001246569018842d3258","4398046511104":"03397b522bb4e156ec3952d3f048e5a986c20a00718e5e52cd5718466bf494156a","8796093022208":"02d1fb9e78262b5d7d74028073075b80bb5ab281edcfc3191061962c1346340f1e","17592186044416":"030d3f2ad7a4ca115712ff7f140434f802b19a4c9b2dd1c76f3e8e80c05c6a9310","35184372088832":"03e325b691f292e1dfb151c3fb7cad440b225795583c32e24e10635a80e4221c06","70368744177664":"03bee8f64d88de3dee21d61f89efa32933da51152ddbd67466bef815e9f93f8fd1","140737488355328":"0327244c9019a4892e1f04ba3bf95fe43b327479e2d57c25979446cc508cd379ed","281474976710656":"02fb58522cd662f2f8b042f8161caae6e45de98283f74d4e99f19b0ea85e08a56d","562949953421312":"02adde4b466a9d7e59386b6a701a39717c53f30c4810613c1b55e6b6da43b7bc9a","1125899906842624":"038eeda11f78ce05c774f30e393cda075192b890d68590813ff46362548528dca9","2251799813685248":"02ec13e0058b196db80f7079d329333b330dc30c000dbdd7397cbbc5a37a664c4f","4503599627370496":"02d2d162db63675bd04f7d56df04508840f41e2ad87312a3c93041b494efe80a73","9007199254740992":"0356969d6aef2bb40121dbd07c68b6102339f4ea8e674a9008bb69506795998f49","18014398509481984":"02f4e667567ebb9f4e6e180a4113bb071c48855f657766bb5e9c776a880335d1d6","36028797018963968":"0385b4fe35e41703d7a657d957c67bb536629de57b7e6ee6fe2130728ef0fc90b0","72057594037927936":"02b2bc1968a6fddbcc78fb9903940524824b5f5bed329c6ad48a19b56068c144fd","144115188075855872":"02e0dbb24f1d288a693e8a49bc14264d1276be16972131520cf9e055ae92fba19a","288230376151711744":"03efe75c106f931a525dc2d653ebedddc413a2c7d8cb9da410893ae7d2fa7d19cc","576460752303423488":"02c7ec2bd9508a7fc03f73c7565dc600b30fd86f3d305f8f139c45c404a52d958a","1152921504606846976":"035a6679c6b25e68ff4e29d1c7ef87f21e0a8fc574f6a08c1aa45ff352c1d59f06","2305843009213693952":"033cdc225962c052d485f7cfbf55a5b2367d200fe1fe4373a347deb4cc99e9a099","4611686018427387904":"024a4b806cf413d14b294719090a9da36ba75209c7657135ad09bc65328fba9e6f","9223372036854775808":"0377a6fe114e291a8d8e991627c38001c8305b23b9e98b1c7b1893f5cd0dda6cad"}},{"id":"00759e3f8b06b36f","unit":"sat","keys":{"1":"038a935c51c76c780ff9731cfbe9ab477f38346775809fa4c514340feabbec4b3a","2":"038288b12ebf2db3645e5d58835bd100398b6b19dfef338c698b55c05d0d41fb0a","4":"02fc8201cf4ea29abac0495d1304064f0e698762b8c0db145c1737b38a9d61c7e2","8":"02274243e03ca19f969acc7072812405b38adc672d1d753e65c63746b3f31cc6eb","16":"025f07cb2493351e7d5202f05eaf3934d5c9d17e73385e9de5bfab802f7d8caf92","32":"03afce0a897c858d7c88c1454d492eac43011e3396dda5b778ba1fcab381c748b1","64":"037b2178f42507f0c95e09d9b435a127df4b3e23ccd20af8075817d3abe90947ad","128":"02ebce8457b48407d4d248dba5a31b3eabf08a6285d09d08e40681c4adaf77bd40","256":"03c89713d27d6f8e328597b43dd87623efdcb251a484932f9e095ebfb6dbf4bdf2","512":"02df10f3ebba69916d03ab1754488770498f2e5466224d6df6d12811a13e46776c","1024":"02f5d9cba0502c21c6b39938a09dcb0390f124a2fd65e45dfeccd153cc1864273d","2048":"039de1dad91761b194e7674fb6ba212241aaf7f49dcb578a8fe093196ad1b20d1c","4096":"03cc694ba22e455f1c22b2cee4a40ecdd4f3bb4da0745411adb456158372d3efbb","8192":"029d66c24450fc315e046010df6870d61daa90c5c486c5ec4d7d3b99c5c2bce923","16384":"0387d063821010c7bd5cf79441870182f70cd432d13d3fc255e7b6ffd82c9d3c5a","32768":"021a94c6c03f7de8feb25b8a8b8d1f1c6f56af4bc533eb97c9e8b89c76b616ff11","65536":"038989c6ed91a7c577953115b465ee400a270a64e95eda8f7ee9d6bf30b8fe4908","131072":"03c3d3cd2523f004ee479a170b0ec5c74c060edb8356fc1b0a9ed8087cf6345172","262144":"02e54a7546f1a9194f30baa593a13d4e2949eb866593445d89675d7d394ef6320b","524288":"034e91037b3f1d3258d1e871dede80e98ef83e307c2e5ff589f38bd046f97546f8","1048576":"03306d42752a1adcfa394af2a690961ac9b80b1ac0f5fdc0890f66f8dc7d25ac6e","2097152":"03ec114332fe798c3e36675566c4748fda7d881000a01864ec48486512d7901e76","4194304":"02095e3e443d98ca3dfabcebc2f9154f3656b889783f7edb8290cfb01f497e63cf","8388608":"03c90f31525a4f9ab6562ec3edbf2bafc6662256ea6ce82ab19a45d2aee80b2f15","16777216":"03c0ae897a45724465c713c1379671ac5ff0a81c32e5f2dd27ea7e5530c7af484c","33554432":"034bcf793b70ba511e9c84cd07fc0c73c061e912bc02df4cac7871d048bad653b6","67108864":"021c6826c23a181d14962f43121943569a54f9d5af556eb839aee42d3f62debee6","134217728":"030e1bc651b6496922978d6cd3ed923cbf12b4332c496f841f506f5abf9d186d35","268435456":"03e3219e50cf389a75794f82ab4f880f5ffe9ca227b992c3e93cb4bb659d8e3353","536870912":"03879ad42536c410511ac6956b9da2d0da59ce7fbb6068bd9b25dd7cccddcc8096","1073741824":"03c4d3755a17904c0cfa7d7a21cc5b4e85fca8ac85369fcb12a6e2177525117dee","2147483648":"02e7a5d5cd3ea24f05f741dddad3dc8c5e24db60eb9bf9ad888b1c5dfbd792665e","4294967296":"03c783d24d8c9e51207eb3d6199bf48d6eb81a4b34103b422724be15501ff921bd","8589934592":"03200234495725455f4c4e6b6cb7b7936eb7cd1d1c9bb73d2ce032bae7d728b3ca","17179869184":"02eafa50ac67de2c206d1a67245b72ec20fac081c2a550294cc0a711246ed65a41","34359738368":"024c153c2a56de05860006aff9dc35ec9cafd7ac68708442a3a326c858b0c1a146","68719476736":"035a890c2d5c8bf259b98ac67d0d813b87778bcb0c0ea1ee9717ac804b0be3f563","137438953472":"025184ca832f08b105fdb471e2caf14025a1daa6f44ce90b4c7703878ccb6b26e8","274877906944":"039d19a41abdd49949c60672430018c63f27c5a28991f9fbb760499daccc63146c","549755813888":"03a138ac626dd3e6753459903aa128a13c052ed0058f2ead707c203bd4a7565237","1099511627776":"0298c8ef2eab728613103481167102efaf2d4b7a303cb94b9393da37a034a95c53","2199023255552":"02d88f8fc93cd2edf303fdebfecb70e59b5373cb8f746a1d075a9c86bc9382ac07","4398046511104":"02afd89ee23eee7d5fe6687fee898f64e9b01913ec71b5c596762b215e040c701f","8796093022208":"02196b461f3c804259e597c50e514920427aab4beaef0c666185fb2ff4399813db","17592186044416":"037b33746a6fd7a71d4cf17c85d13a64b98620614c0028d4995163f1b8484ee337","35184372088832":"036cce0a1878bbc63b3108c379ef4e6529fbf20ed675d80d91ca3ccc55fde4bdbd","70368744177664":"039c81dccb319ba70597cdf9db33b459164a1515c27366c8f667b01d988874e554","140737488355328":"036b2dd85a3c44c4458f0b246ce19a1524a191f1716834cfb452c6e1f946172c19","281474976710656":"022c84722c31a2b3d8cfd9b6a9e6199515fd97d6a9c390fc3d82f123bfc501ad04","562949953421312":"0355e2be85ee599b8fa7e6e68a9954573d032e89aa9e65c2e1231991664c200bf3","1125899906842624":"024b10818cd27f3eec6c9daf82b9dfa53928ab0711b711070bd39892ac10dee765","2251799813685248":"02a6d726432bb18c3145eba4fc0b587bf64f3be8617c0070dda33944474b3f8740","4503599627370496":"0248304be3cbaf31ec320bc636bb936c5984caf773df950fc44c6237ec09c557a1","9007199254740992":"03a3c0e9da7ece7d7b132c53662c0389bd87db801dff5ac9edd9f46699cb1dc065","18014398509481984":"03b6c4c874e2392072e17fbfd181afbd40d6766a8ca4cf932264ba98d98de1328c","36028797018963968":"0370dca4416ec6e30ff02f8e9db7804348b42e3f5c22099dfc896fa1b2ccbe7a69","72057594037927936":"0226250140aedb79de91cb4cc7350884bde229063f34ee0849081bb391a37c273e","144115188075855872":"02baef3a94d241aee9d6057c7a7ee7424f8a0bcb910daf6c49ddcabf70ffbc77d8","288230376151711744":"030f95a12369f1867ce0dbf2a6322c27d70c61b743064d76cfc81dd43f1a052ae6","576460752303423488":"021bc89118ab6eb1fbebe0fa6cc76da8236a7991163475a73a22d8efd016a45800","1152921504606846976":"03b0c1e658d7ca12830a0b590ea5a4d6db51084ae80b6d8abf27ad2d762209acd1","2305843009213693952":"0266926ce658a0bdae934071f22e09dbb6ecaff2a4dc4b1f8e23626570d993b48e","4611686018427387904":"03ac17f10f9bb745ebd8ee9cdca1b6981f5a356147d431196c21c6d4869402bde0","9223372036854775808":"037ab5b88c8ce34c4a3970be5c6f75b8a7a5493a12ef56a1c9ba9ff5f90de46fcc"}},{"id":"000f01df73ea149a","unit":"sat","keys":{"1":"03ba786a2c0745f8c30e490288acd7a72dd53d65afd292ddefa326a4a3fa14c566","2":"03361cd8bd1329fea797a6add1cf1990ffcf2270ceb9fc81eeee0e8e9c1bd0cdf5","4":"036e378bcf78738ddf68859293c69778035740e41138ab183c94f8fee7572214c7","8":"03909d73beaf28edfb283dbeb8da321afd40651e8902fcf5454ecc7d69788626c0","16":"028a36f0e6638ea7466665fe174d958212723019ec08f9ce6898d897f88e68aa5d","32":"03a97a40e146adee2687ac60c2ba2586a90f970de92a9d0e6cae5a4b9965f54612","64":"03ce86f0c197aab181ddba0cfc5c5576e11dfd5164d9f3d4a3fc3ffbbf2e069664","128":"0284f2c06d938a6f78794814c687560a0aabab19fe5e6f30ede38e113b132a3cb9","256":"03b99f475b68e5b4c0ba809cdecaae64eade2d9787aa123206f91cd61f76c01459","512":"03d4db82ea19a44d35274de51f78af0a710925fe7d9e03620b84e3e9976e3ac2eb","1024":"031fbd4ba801870871d46cf62228a1b748905ebc07d3b210daf48de229e683f2dc","2048":"0276cedb9a3b160db6a158ad4e468d2437f021293204b3cd4bf6247970d8aff54b","4096":"02fc6b89b403ee9eb8a7ed457cd3973638080d6e04ca8af7307c965c166b555ea2","8192":"0320265583e916d3a305f0d2687fcf2cd4e3cd03a16ea8261fda309c3ec5721e21","16384":"036e41de58fdff3cb1d8d713f48c63bc61fa3b3e1631495a444d178363c0d2ed50","32768":"0365438f613f19696264300b069d1dad93f0c60a37536b72a8ab7c7366a5ee6c04","65536":"02408426cfb6fc86341bac79624ba8708a4376b2d92debdf4134813f866eb57a8d","131072":"031063e9f11c94dc778c473e968966eac0e70b7145213fbaff5f7a007e71c65f41","262144":"02f2a3e808f9cd168ec71b7f328258d0c1dda250659c1aced14c7f5cf05aab4328","524288":"038ac10de9f1ff9395903bb73077e94dbf91e9ef98fd77d9a2debc5f74c575bc86","1048576":"0203eaee4db749b0fc7c49870d082024b2c31d889f9bc3b32473d4f1dfa3625788","2097152":"033cdb9d36e1e82ae652b7b6a08e0204569ec7ff9ebf85d80a02786dc7fe00b04c","4194304":"02c8b73f4e3a470ae05e5f2fe39984d41e9f6ae7be9f3b09c9ac31292e403ac512","8388608":"025bbe0cfce8a1f4fbd7f3a0d4a09cb6badd73ef61829dc827aa8a98c270bc25b0","16777216":"037eec3d1651a30a90182d9287a5c51386fe35d4a96839cf7969c6e2a03db1fc21","33554432":"03280576b81a04e6abd7197f305506476f5751356b7643988495ca5c3e14e5c262","67108864":"03268bfb05be1dbb33ab6e7e00e438373ca2c9b9abc018fdb452d0e1a0935e10d3","134217728":"02573b68784ceba9617bbcc7c9487836d296aa7c628c3199173a841e7a19798020","268435456":"0234076b6e70f7fbf755d2227ecc8d8169d662518ee3a1401f729e2a12ccb2b276","536870912":"03015bd88961e2a466a2163bd4248d1d2b42c7c58a157e594785e7eb34d880efc9","1073741824":"02c9b076d08f9020ebee49ac8ba2610b404d4e553a4f800150ceb539e9421aaeee","2147483648":"034d592f4c366afddc919a509600af81b489a03caf4f7517c2b3f4f2b558f9a41a","4294967296":"037c09ecb66da082981e4cbdb1ac65c0eb631fc75d85bed13efb2c6364148879b5","8589934592":"02b4ebb0dda3b9ad83b39e2e31024b777cc0ac205a96b9a6cfab3edea2912ed1b3","17179869184":"026cc4dacdced45e63f6e4f62edbc5779ccd802e7fabb82d5123db879b636176e9","34359738368":"02b2cee01b7d8e90180254459b8f09bbea9aad34c3a2fd98c85517ecfc9805af75","68719476736":"037a0c0d564540fc574b8bfa0253cca987b75466e44b295ed59f6f8bd41aace754","137438953472":"021df6585cae9b9ca431318a713fd73dbb76b3ef5667957e8633bca8aaa7214fb6","274877906944":"02b8f53dde126f8c85fa5bb6061c0be5aca90984ce9b902966941caf963648d53a","549755813888":"029cc8af2840d59f1d8761779b2496623c82c64be8e15f9ab577c657c6dd453785","1099511627776":"03e446fdb84fad492ff3a25fc1046fb9a93a5b262ebcd0151caa442ea28959a38a","2199023255552":"02d6b25bd4ab599dd0818c55f75702fde603c93f259222001246569018842d3258","4398046511104":"03397b522bb4e156ec3952d3f048e5a986c20a00718e5e52cd5718466bf494156a","8796093022208":"02d1fb9e78262b5d7d74028073075b80bb5ab281edcfc3191061962c1346340f1e","17592186044416":"030d3f2ad7a4ca115712ff7f140434f802b19a4c9b2dd1c76f3e8e80c05c6a9310","35184372088832":"03e325b691f292e1dfb151c3fb7cad440b225795583c32e24e10635a80e4221c06","70368744177664":"03bee8f64d88de3dee21d61f89efa32933da51152ddbd67466bef815e9f93f8fd1","140737488355328":"0327244c9019a4892e1f04ba3bf95fe43b327479e2d57c25979446cc508cd379ed","281474976710656":"02fb58522cd662f2f8b042f8161caae6e45de98283f74d4e99f19b0ea85e08a56d","562949953421312":"02adde4b466a9d7e59386b6a701a39717c53f30c4810613c1b55e6b6da43b7bc9a","1125899906842624":"038eeda11f78ce05c774f30e393cda075192b890d68590813ff46362548528dca9","2251799813685248":"02ec13e0058b196db80f7079d329333b330dc30c000dbdd7397cbbc5a37a664c4f","4503599627370496":"02d2d162db63675bd04f7d56df04508840f41e2ad87312a3c93041b494efe80a73","9007199254740992":"0356969d6aef2bb40121dbd07c68b6102339f4ea8e674a9008bb69506795998f49","18014398509481984":"02f4e667567ebb9f4e6e180a4113bb071c48855f657766bb5e9c776a880335d1d6","36028797018963968":"0385b4fe35e41703d7a657d957c67bb536629de57b7e6ee6fe2130728ef0fc90b0","72057594037927936":"02b2bc1968a6fddbcc78fb9903940524824b5f5bed329c6ad48a19b56068c144fd","144115188075855872":"02e0dbb24f1d288a693e8a49bc14264d1276be16972131520cf9e055ae92fba19a","288230376151711744":"03efe75c106f931a525dc2d653ebedddc413a2c7d8cb9da410893ae7d2fa7d19cc","576460752303423488":"02c7ec2bd9508a7fc03f73c7565dc600b30fd86f3d305f8f139c45c404a52d958a","1152921504606846976":"035a6679c6b25e68ff4e29d1c7ef87f21e0a8fc574f6a08c1aa45ff352c1d59f06","2305843009213693952":"033cdc225962c052d485f7cfbf55a5b2367d200fe1fe4373a347deb4cc99e9a099","4611686018427387904":"024a4b806cf413d14b294719090a9da36ba75209c7657135ad09bc65328fba9e6f","9223372036854775808":"0377a6fe114e291a8d8e991627c38001c8305b23b9e98b1c7b1893f5cd0dda6cad"}},{"id":"yjzQhxghPdrr","unit":"sat","keys":{"1":"038a935c51c76c780ff9731cfbe9ab477f38346775809fa4c514340feabbec4b3a","2":"038288b12ebf2db3645e5d58835bd100398b6b19dfef338c698b55c05d0d41fb0a","4":"02fc8201cf4ea29abac0495d1304064f0e698762b8c0db145c1737b38a9d61c7e2","8":"02274243e03ca19f969acc7072812405b38adc672d1d753e65c63746b3f31cc6eb","16":"025f07cb2493351e7d5202f05eaf3934d5c9d17e73385e9de5bfab802f7d8caf92","32":"03afce0a897c858d7c88c1454d492eac43011e3396dda5b778ba1fcab381c748b1","64":"037b2178f42507f0c95e09d9b435a127df4b3e23ccd20af8075817d3abe90947ad","128":"02ebce8457b48407d4d248dba5a31b3eabf08a6285d09d08e40681c4adaf77bd40","256":"03c89713d27d6f8e328597b43dd87623efdcb251a484932f9e095ebfb6dbf4bdf2","512":"02df10f3ebba69916d03ab1754488770498f2e5466224d6df6d12811a13e46776c","1024":"02f5d9cba0502c21c6b39938a09dcb0390f124a2fd65e45dfeccd153cc1864273d","2048":"039de1dad91761b194e7674fb6ba212241aaf7f49dcb578a8fe093196ad1b20d1c","4096":"03cc694ba22e455f1c22b2cee4a40ecdd4f3bb4da0745411adb456158372d3efbb","8192":"029d66c24450fc315e046010df6870d61daa90c5c486c5ec4d7d3b99c5c2bce923","16384":"0387d063821010c7bd5cf79441870182f70cd432d13d3fc255e7b6ffd82c9d3c5a","32768":"021a94c6c03f7de8feb25b8a8b8d1f1c6f56af4bc533eb97c9e8b89c76b616ff11","65536":"038989c6ed91a7c577953115b465ee400a270a64e95eda8f7ee9d6bf30b8fe4908","131072":"03c3d3cd2523f004ee479a170b0ec5c74c060edb8356fc1b0a9ed8087cf6345172","262144":"02e54a7546f1a9194f30baa593a13d4e2949eb866593445d89675d7d394ef6320b","524288":"034e91037b3f1d3258d1e871dede80e98ef83e307c2e5ff589f38bd046f97546f8","1048576":"03306d42752a1adcfa394af2a690961ac9b80b1ac0f5fdc0890f66f8dc7d25ac6e","2097152":"03ec114332fe798c3e36675566c4748fda7d881000a01864ec48486512d7901e76","4194304":"02095e3e443d98ca3dfabcebc2f9154f3656b889783f7edb8290cfb01f497e63cf","8388608":"03c90f31525a4f9ab6562ec3edbf2bafc6662256ea6ce82ab19a45d2aee80b2f15","16777216":"03c0ae897a45724465c713c1379671ac5ff0a81c32e5f2dd27ea7e5530c7af484c","33554432":"034bcf793b70ba511e9c84cd07fc0c73c061e912bc02df4cac7871d048bad653b6","67108864":"021c6826c23a181d14962f43121943569a54f9d5af556eb839aee42d3f62debee6","134217728":"030e1bc651b6496922978d6cd3ed923cbf12b4332c496f841f506f5abf9d186d35","268435456":"03e3219e50cf389a75794f82ab4f880f5ffe9ca227b992c3e93cb4bb659d8e3353","536870912":"03879ad42536c410511ac6956b9da2d0da59ce7fbb6068bd9b25dd7cccddcc8096","1073741824":"03c4d3755a17904c0cfa7d7a21cc5b4e85fca8ac85369fcb12a6e2177525117dee","2147483648":"02e7a5d5cd3ea24f05f741dddad3dc8c5e24db60eb9bf9ad888b1c5dfbd792665e","4294967296":"03c783d24d8c9e51207eb3d6199bf48d6eb81a4b34103b422724be15501ff921bd","8589934592":"03200234495725455f4c4e6b6cb7b7936eb7cd1d1c9bb73d2ce032bae7d728b3ca","17179869184":"02eafa50ac67de2c206d1a67245b72ec20fac081c2a550294cc0a711246ed65a41","34359738368":"024c153c2a56de05860006aff9dc35ec9cafd7ac68708442a3a326c858b0c1a146","68719476736":"035a890c2d5c8bf259b98ac67d0d813b87778bcb0c0ea1ee9717ac804b0be3f563","137438953472":"025184ca832f08b105fdb471e2caf14025a1daa6f44ce90b4c7703878ccb6b26e8","274877906944":"039d19a41abdd49949c60672430018c63f27c5a28991f9fbb760499daccc63146c","549755813888":"03a138ac626dd3e6753459903aa128a13c052ed0058f2ead707c203bd4a7565237","1099511627776":"0298c8ef2eab728613103481167102efaf2d4b7a303cb94b9393da37a034a95c53","2199023255552":"02d88f8fc93cd2edf303fdebfecb70e59b5373cb8f746a1d075a9c86bc9382ac07","4398046511104":"02afd89ee23eee7d5fe6687fee898f64e9b01913ec71b5c596762b215e040c701f","8796093022208":"02196b461f3c804259e597c50e514920427aab4beaef0c666185fb2ff4399813db","17592186044416":"037b33746a6fd7a71d4cf17c85d13a64b98620614c0028d4995163f1b8484ee337","35184372088832":"036cce0a1878bbc63b3108c379ef4e6529fbf20ed675d80d91ca3ccc55fde4bdbd","70368744177664":"039c81dccb319ba70597cdf9db33b459164a1515c27366c8f667b01d988874e554","140737488355328":"036b2dd85a3c44c4458f0b246ce19a1524a191f1716834cfb452c6e1f946172c19","281474976710656":"022c84722c31a2b3d8cfd9b6a9e6199515fd97d6a9c390fc3d82f123bfc501ad04","562949953421312":"0355e2be85ee599b8fa7e6e68a9954573d032e89aa9e65c2e1231991664c200bf3","1125899906842624":"024b10818cd27f3eec6c9daf82b9dfa53928ab0711b711070bd39892ac10dee765","2251799813685248":"02a6d726432bb18c3145eba4fc0b587bf64f3be8617c0070dda33944474b3f8740","4503599627370496":"0248304be3cbaf31ec320bc636bb936c5984caf773df950fc44c6237ec09c557a1","9007199254740992":"03a3c0e9da7ece7d7b132c53662c0389bd87db801dff5ac9edd9f46699cb1dc065","18014398509481984":"03b6c4c874e2392072e17fbfd181afbd40d6766a8ca4cf932264ba98d98de1328c","36028797018963968":"0370dca4416ec6e30ff02f8e9db7804348b42e3f5c22099dfc896fa1b2ccbe7a69","72057594037927936":"0226250140aedb79de91cb4cc7350884bde229063f34ee0849081bb391a37c273e","144115188075855872":"02baef3a94d241aee9d6057c7a7ee7424f8a0bcb910daf6c49ddcabf70ffbc77d8","288230376151711744":"030f95a12369f1867ce0dbf2a6322c27d70c61b743064d76cfc81dd43f1a052ae6","576460752303423488":"021bc89118ab6eb1fbebe0fa6cc76da8236a7991163475a73a22d8efd016a45800","1152921504606846976":"03b0c1e658d7ca12830a0b590ea5a4d6db51084ae80b6d8abf27ad2d762209acd1","2305843009213693952":"0266926ce658a0bdae934071f22e09dbb6ecaff2a4dc4b1f8e23626570d993b48e","4611686018427387904":"03ac17f10f9bb745ebd8ee9cdca1b6981f5a356147d431196c21c6d4869402bde0","9223372036854775808":"037ab5b88c8ce34c4a3970be5c6f75b8a7a5493a12ef56a1c9ba9ff5f90de46fcc"}}]}"#;

        let keys_response: KeysResponse = serde_json::from_str(keys).unwrap();

        assert_eq!(keys_response.keysets.len(), 2);
    }
}
