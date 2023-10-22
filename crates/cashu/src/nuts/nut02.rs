//! Keysets and keyset ID
// https://github.com/cashubtc/nuts/blob/main/02.md

use std::collections::HashSet;
use std::fmt;

use base64::engine::general_purpose;
use base64::Engine as _;
use bitcoin::hashes::{sha256, Hash};
use itertools::Itertools;
use serde::{Deserialize, Serialize};

use super::nut01::Keys;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Base64(base64::DecodeError),
    Length,
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length => write!(f, "NUT02: Id invalid length"),
            Self::Base64(err) => write!(f, "NUT02: {:?}", err),
        }
    }
}

/// A keyset ID is an identifier for a specific keyset. It can be derived by
/// anyone who knows the set of public keys of a mint. The keyset ID **CAN**
/// be stored in a Cashu token such that the token can be used to identify
/// which mint or keyset it was generated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id([u8; Id::BYTES]);

impl Id {
    const BYTES: usize = 9;
    const STRLEN: usize = 12;

    pub fn try_from_base64(b64: &str) -> Result<Self, Error> {
        use base64::engine::general_purpose::{STANDARD, URL_SAFE};
        use base64::Engine as _;

        if b64.len() != Self::STRLEN {
            return Err(Error::Length);
        }

        if let Ok(bytes) = URL_SAFE.decode(b64) {
            if bytes.len() == Self::BYTES {
                return Ok(Self(
                    <[u8; Self::BYTES]>::try_from(bytes.as_slice()).unwrap(),
                ));
            }
        }

        match STANDARD.decode(b64) {
            Ok(bytes) if bytes.len() == Self::BYTES => Ok(Self(
                <[u8; Self::BYTES]>::try_from(bytes.as_slice()).unwrap(),
            )),
            Ok(_) => Err(Error::Length),
            Err(e) => Err(Error::Base64(e)),
        }
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut output = String::with_capacity(Self::STRLEN);
        general_purpose::STANDARD.encode_string(self.0.as_slice(), &mut output);
        f.write_str(&output)
    }
}

impl std::convert::TryFrom<String> for Id {
    type Error = Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Id::try_from_base64(&value)
    }
}

impl serde::ser::Serialize for Id {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::de::Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct IdVisitor;

        impl<'de> serde::de::Visitor<'de> for IdVisitor {
            type Value = Id;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a 12-character Base64 string")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Id::try_from_base64(v).map_err(|e| match e {
                    Error::Length => E::custom(format!(
                        "Invalid Length: Expected {}, got {}",
                        Id::STRLEN,
                        v.len()
                    )),
                    Error::Base64(e) => E::custom(e),
                })
            }
        }

        deserializer.deserialize_str(IdVisitor)
    }
}

impl From<&Keys> for Id {
    fn from(map: &Keys) -> Self {
        /* NUT-02 § 2.2.2
            1 - sort keyset by amount
            2 - concatenate all (sorted) public keys to one string
            3 - HASH_SHA256 the concatenated public keys
            4 - take the first 12 characters of the base64-encoded hash
        */

        let pubkeys_concat = map
            .iter()
            .sorted_by(|(amt_a, _), (amt_b, _)| amt_a.cmp(amt_b))
            .map(|(_, pubkey)| pubkey)
            .join("");

        let hash = sha256::Hash::hash(pubkeys_concat.as_bytes());
        let bytes = hash.to_byte_array();
        // First 9 bytes of hash will encode as the first 12 Base64 characters later
        Self(<[u8; Self::BYTES]>::try_from(&bytes[0..Self::BYTES]).unwrap())
    }
}

/// Mint Keysets [NUT-02]
/// Ids of mints keyset ids
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// set of public key ids that the mint generates
    pub keysets: HashSet<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct KeySet {
    pub id: Id,
    pub keys: Keys,
}

impl From<mint::KeySet> for KeySet {
    fn from(keyset: mint::KeySet) -> Self {
        Self {
            id: keyset.id,
            keys: Keys::from(keyset.keys),
        }
    }
}

pub mod mint {
    use std::collections::BTreeMap;

    use bitcoin::hashes::sha256::Hash as Sha256;
    use bitcoin::hashes::{Hash, HashEngine};
    use itertools::Itertools;
    use k256::SecretKey;
    use serde::Serialize;

    use super::Id;
    use crate::nuts::nut01::mint::{KeyPair, Keys};
    use crate::Amount;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct KeySet {
        pub id: Id,
        pub keys: Keys,
    }

    impl KeySet {
        pub fn generate(
            secret: impl Into<String>,
            derivation_path: impl Into<String>,
            max_order: u8,
        ) -> Self {
            // Elliptic curve math context

            /* NUT-02 § 2.1
                for i in range(MAX_ORDER):
                    k_i = HASH_SHA256(s + D + i)[:32]
            */

            let mut map = BTreeMap::new();

            // SHA-256 midstate, for quicker hashing
            let mut engine = Sha256::engine();
            engine.input(secret.into().as_bytes());
            engine.input(derivation_path.into().as_bytes());

            for i in 0..max_order {
                let amount = Amount::from_sat(2_u64.pow(i as u32));

                // Reuse midstate
                let mut e = engine.clone();
                e.input(i.to_string().as_bytes());
                let hash = Sha256::from_engine(e);
                let secret_key = SecretKey::from_slice(&hash.to_byte_array()).unwrap();
                let keypair = KeyPair::from_secret_key(secret_key.into());
                map.insert(amount, keypair);
            }

            let keys = Keys(map);

            Self {
                id: (&keys).into(),
                keys,
            }
        }
    }

    impl From<KeySet> for Id {
        fn from(keyset: KeySet) -> Id {
            let keys: super::KeySet = keyset.into();

            Id::from(&keys.keys)
        }
    }

    impl From<&Keys> for Id {
        fn from(map: &Keys) -> Self {
            /* NUT-02 § 2.2.2
                1 - sort keyset by amount
                2 - concatenate all (sorted) public keys to one string
                3 - HASH_SHA256 the concatenated public keys
                4 - take the first 12 characters of the base64-encoded hash
            */

            let keys: super::Keys = map.clone().into();

            let pubkeys_concat = keys
                .iter()
                .sorted_by(|(amt_a, _), (amt_b, _)| amt_a.cmp(amt_b))
                .map(|(_, pubkey)| pubkey)
                .join("");

            let hash = Sha256::hash(pubkeys_concat.as_bytes());
            let bytes = hash.to_byte_array();
            // First 9 bytes of hash will encode as the first 12 Base64 characters later
            Self(<[u8; Self::BYTES]>::try_from(&bytes[0..Self::BYTES]).unwrap())
        }
    }
}

#[cfg(test)]
mod test {

    use super::Keys;
    use crate::nuts::nut02::Id;

    const SHORT_KEYSET_ID: &str = "esom3oyNLLit";
    const SHORT_KEYSET: &str = r#"
        {
            "1":"03a40f20667ed53513075dc51e715ff2046cad64eb68960632269ba7f0210e38bc",
            "2":"03fd4ce5a16b65576145949e6f99f445f8249fee17c606b688b504a849cdc452de",
            "4":"02648eccfa4c026960966276fa5a4cae46ce0fd432211a4f449bf84f13aa5f8303",
            "8":"02fdfd6796bfeac490cbee12f778f867f0a2c68f6508d17c649759ea0dc3547528"
        }
    "#;

    const KEYSET_ID: &str = "I2yN+iRYfkzT";
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
    fn deserialization_and_id_generation() {
        let keys: Keys = serde_json::from_str(SHORT_KEYSET).unwrap();

        let id: Id = (&keys).into();

        assert_eq!(id, Id::try_from_base64(SHORT_KEYSET_ID).unwrap());

        let keys: Keys = serde_json::from_str(KEYSET).unwrap();

        let id: Id = (&keys).into();

        assert_eq!(id, Id::try_from_base64(KEYSET_ID).unwrap());
    }
}
