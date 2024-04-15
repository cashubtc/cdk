//! NUT-01: Mint public key exchange
//!
//! <https://github.com/cashubtc/nuts/blob/main/01.md>

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use bitcoin::secp256k1;
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

mod public_key;
mod secret_key;

pub use self::public_key::PublicKey;
pub use self::secret_key::SecretKey;
use super::nut02::KeySet;
use crate::amount::Amount;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Secp256k1(#[from] secp256k1::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Invalid public key size: expected={expected}, found={found}")]
    InvalidPublicKeySize { expected: usize, found: usize },
}

/// Mint Keys [NUT-01]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Keys(BTreeMap<Amount, PublicKey>);

impl From<MintKeys> for Keys {
    fn from(keys: MintKeys) -> Self {
        Self(
            keys.0
                .iter()
                .map(|(amount, keypair)| (*amount, keypair.public_key))
                .collect(),
        )
    }
}

impl Keys {
    #[inline]
    pub fn new(keys: BTreeMap<Amount, PublicKey>) -> Self {
        Self(keys)
    }

    #[inline]
    pub fn keys(&self) -> &BTreeMap<Amount, PublicKey> {
        &self.0
    }

    #[inline]
    pub fn amount_key(&self, amount: Amount) -> Option<PublicKey> {
        self.0.get(&amount).copied()
    }

    /// Iterate through the (`Amount`, `PublicKey`) entries in the Map
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&Amount, &PublicKey)> {
        self.0.iter()
    }
}

/// Mint Public Keys [NUT-01]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeysResponse {
    pub keysets: Vec<KeySet>,
}

impl<'de> Deserialize<'de> for KeysResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let keys_response: Value = Value::deserialize(deserializer)?;

        let keysets = keys_response
            .get("keysets")
            .ok_or(de::Error::custom("Keysets not found"))?
            .as_array()
            .ok_or(de::Error::custom("Keysets not found"))?;

        let keysets = keysets
            .iter()
            .flat_map(|keyset| serde_json::from_value(keyset.clone()))
            .collect();

        Ok(KeysResponse { keysets })
    }
}

/// Mint keys
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeys(BTreeMap<Amount, MintKeyPair>);

impl Deref for MintKeys {
    type Target = BTreeMap<Amount, MintKeyPair>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MintKeys {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl MintKeys {
    #[inline]
    pub fn new(map: BTreeMap<Amount, MintKeyPair>) -> Self {
        Self(map)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeyPair {
    pub public_key: PublicKey,
    pub secret_key: SecretKey,
}

impl MintKeyPair {
    #[inline]
    pub fn from_secret_key(secret_key: SecretKey) -> Self {
        Self {
            public_key: secret_key.public_key(),
            secret_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::nuts::nut11::VerifyingKey;

    #[test]
    fn pubkey() {
        let pubkey_str = "02c020067db727d586bc3183aecf97fcb800c3f4cc4759f69c626c9db5d8f5b5d4";
        let pubkey = PublicKey::from_str(pubkey_str).unwrap();
        assert_eq!(pubkey_str, pubkey.to_string());
        /*
        let pubkey_str = "04918dfc36c93e7db6cc0d60f37e1522f1c36b64d3f4b424c532d7c595febbc5";
        let pubkey = PublicKey::from_hex(pubkey_str.to_string()).unwrap();
        assert_eq!(pubkey_str, pubkey.to_hex())*/
    }

    #[test]
    fn verying_key() {
        let key_str = "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198";

        let pubkey = PublicKey::from_str(key_str).unwrap();
        let v_key: VerifyingKey = pubkey.try_into().unwrap();

        let p: PublicKey = v_key.to_normalized_public_key();

        assert_eq!(p, pubkey);
    }

    #[test]
    fn key_response() {
        let res: String = r#"{"1":"02f71e2d93aa95fc52b938735a24774ad926406c81e9dc9d2aa699fb89281548fd","2":"03b28dd9c19aaf1ec847be31b60c6a5e1a6cb6f87434afcdb0d9348ba0e2bdb150","4":"03ede0e704e223e764a82f73984b0fec0fdbde15ef57b4de95b527f7182af7487e","8":"020fd24fbd552445df70c244be2af77da2b2f634ccfda9e9620b347b5cd50dbdd8","16":"03ef9ef2515df5c0d0851ed9419a24a571ef5e03206d9d2fc6572ac050c5afe1aa","32":"02dbd455474176b30234c178573e874cc79d0c2fc1920cf0e9f133204cf43299c1","64":"0237c1eb11b8a214cca3e0104684227952188039a05cd55c1ad3896a572c70a7c3","128":"02655041771766b94a269f9f1ec1860f2eade55bb472c4db74ac1257ef54aac52b","256":"02b9e0be7b423bded7d60ff7114549de8d2d5b9c099edf8887aff474037e4bc0cf","512":"0320454cc41e646f49e1ac0a62b9667c80dee45545b045575f2a26f01770dc2521","1024":"0267fc1dabac016f46b3d1a650d97b56f3e56540106720f3d24ff7a6e9cd7183e9","2048":"035a9a25251a4da56f49667ca50677470fc6d8e186a875ab7b32aa064eb9e9e948","4096":"02f607a9eed310825c2d2e66d6e64fb237fe21b640b9a66cc7646b2a6480d91457","8192":"033346f7dce2ef71a80c5d657a8930bdd19c7c1708d03829daf43f20eaeda76768","16384":"024fad3b0b60c6b71d848deac173183fae8ddde31bbde531f18ab23473ddff541d","32768":"020d195466819d96d8c7eee9150565b7bd37196c7d12d0e96e389f56be8aebb44b","65536":"038c9bf295a745726c38d14988851d68d201296a802c296faa838000c2f44d25e0","131072":"032ff6491cdeff0bf9b34acd5deceef3cca7682b5f94dbe3068af8bb3b5aa34b81","262144":"02570090f5b6900955fd794d8f22c23fb35fc87fa03069b9b16bea63ea7cda419a","524288":"029d3c751c7d1c3e1d3e4b7791e1e809f6dedf2c28e172a82967d49a14b7c26ce2","1048576":"03b4a41d39cc6f2a8925f694c514e107b87d7ddb8f5ac55c9e4b7895139d0decd8","2097152":"02d4abbce491f87656eb0d2e66ef18eb009f6320169ef12e66703298d5395f2b91","4194304":"0359023fb85f6e6ede0141ab8f4a1277c19ed62b49b8ef5c5e2c8ca7fefe9b2f91","8388608":"0353d3ae1dad05e1b46ab85a366bfcdb7a645e3457f7714003e0fb06f4d75f4d87","16777216":"032d0847606465b97f15aca30c69f5baeeb43bf6188b4679f723119ce6fb9708c5","33554432":"028a673a53e78aa8c992128e21efb3b33fbd54de20afcf81a67e69eaf2bab7e0e9","67108864":"0278b66e140559352bb5aeca854a6466bc439ee206a9f349ed7926aae4335269b7","134217728":"023834651da0737f484a77204c2d06543fb65ad2dd8d095a2be48ca12ebf2664ec","268435456":"032cba9068638965ccc3870c140c72a1b028a820851f36fe59639e7ab3093a8ffd","536870912":"03eae5e4b22dfa5ad77476c925717dc4e005da78142e75b47fb28569d745483af3","1073741824":"02d17d61027602432a8484b65e6d6063ed9157c51ce92099d61ac2820411c59f9f","2147483648":"0236870e39b3a739d5caa04988dce432e3d7988420f04d9b415125af22672e2726"}"#.to_string();

        let response: Keys = serde_json::from_str(&res).unwrap();

        assert_eq!(&serde_json::to_string(&response).unwrap(), &res)
    }

    #[test]
    fn test_ser_der_secret() {
        let secret = SecretKey::generate();

        let json = serde_json::to_string(&secret).unwrap();

        let sec: SecretKey = serde_json::from_str(&json).unwrap();

        assert_eq!(sec, secret);
    }

    #[test]
    fn test_vectors_01() {
        let incorrect_1 = r#"{
  "1":"03a40f20667ed53513075dc51e715ff2046cad64eb68960632269ba7f0210e38","2":"03fd4ce5a16b65576145949e6f99f445f8249fee17c606b688b504a849cdc452de","4":"02648eccfa4c026960966276fa5a4cae46ce0fd432211a4f449bf84f13aa5f8303","8":"02fdfd6796bfeac490cbee12f778f867f0a2c68f6508d17c649759ea0dc3547528"
}"#;

        let response: Result<Keys, serde_json::Error> = serde_json::from_str(incorrect_1);

        assert!(response.is_err());
        let incorrect_1 = r#"{
  "1":"03a40f20667ed53513075dc51e715ff2046cad64eb68960632269ba7f0210e38bc","2":"04fd4ce5a16b65576145949e6f99f445f8249fee17c606b688b504a849cdc452de3625246cb2c27dac965cb7200a5986467eee92eb7d496bbf1453b074e223e481","4":"02648eccfa4c026960966276fa5a4cae46ce0fd432211a4f449bf84f13aa5f8303","8":"02fdfd6796bfeac490cbee12f778f867f0a2c68f6508d17c649759ea0dc3547528"
}"#;
        let response: Result<Keys, serde_json::Error> = serde_json::from_str(incorrect_1);
        assert!(response.is_err());

        let incorrect_1 = r#"{
         "1":"03a40f20667ed53513075dc51e715ff2046cad64eb68960632269ba7f0210e38bc","2":"03fd4ce5a16b65576145949e6f99f445f8249fee17c606b688b504a849cdc452de","4":"02648eccfa4c026960966276fa5a4cae46ce0fd432211a4f449bf84f13aa5f8303","8":"02fdfd6796bfeac490cbee12f778f867f0a2c68f6508d17c649759ea0dc3547528"
}"#;
        let response: Result<Keys, serde_json::Error> = serde_json::from_str(incorrect_1);
        assert!(response.is_ok());

        let incorrect_1 = r#"{
          "1":"03ba786a2c0745f8c30e490288acd7a72dd53d65afd292ddefa326a4a3fa14c566","2":"03361cd8bd1329fea797a6add1cf1990ffcf2270ceb9fc81eeee0e8e9c1bd0cdf5","4":"036e378bcf78738ddf68859293c69778035740e41138ab183c94f8fee7572214c7","8":"03909d73beaf28edfb283dbeb8da321afd40651e8902fcf5454ecc7d69788626c0","16":"028a36f0e6638ea7466665fe174d958212723019ec08f9ce6898d897f88e68aa5d","32":"03a97a40e146adee2687ac60c2ba2586a90f970de92a9d0e6cae5a4b9965f54612","64":"03ce86f0c197aab181ddba0cfc5c5576e11dfd5164d9f3d4a3fc3ffbbf2e069664","128":"0284f2c06d938a6f78794814c687560a0aabab19fe5e6f30ede38e113b132a3cb9","256":"03b99f475b68e5b4c0ba809cdecaae64eade2d9787aa123206f91cd61f76c01459","512":"03d4db82ea19a44d35274de51f78af0a710925fe7d9e03620b84e3e9976e3ac2eb","1024":"031fbd4ba801870871d46cf62228a1b748905ebc07d3b210daf48de229e683f2dc","2048":"0276cedb9a3b160db6a158ad4e468d2437f021293204b3cd4bf6247970d8aff54b","4096":"02fc6b89b403ee9eb8a7ed457cd3973638080d6e04ca8af7307c965c166b555ea2","8192":"0320265583e916d3a305f0d2687fcf2cd4e3cd03a16ea8261fda309c3ec5721e21","16384":"036e41de58fdff3cb1d8d713f48c63bc61fa3b3e1631495a444d178363c0d2ed50","32768":"0365438f613f19696264300b069d1dad93f0c60a37536b72a8ab7c7366a5ee6c04","65536":"02408426cfb6fc86341bac79624ba8708a4376b2d92debdf4134813f866eb57a8d","131072":"031063e9f11c94dc778c473e968966eac0e70b7145213fbaff5f7a007e71c65f41","262144":"02f2a3e808f9cd168ec71b7f328258d0c1dda250659c1aced14c7f5cf05aab4328","524288":"038ac10de9f1ff9395903bb73077e94dbf91e9ef98fd77d9a2debc5f74c575bc86","1048576":"0203eaee4db749b0fc7c49870d082024b2c31d889f9bc3b32473d4f1dfa3625788","2097152":"033cdb9d36e1e82ae652b7b6a08e0204569ec7ff9ebf85d80a02786dc7fe00b04c","4194304":"02c8b73f4e3a470ae05e5f2fe39984d41e9f6ae7be9f3b09c9ac31292e403ac512","8388608":"025bbe0cfce8a1f4fbd7f3a0d4a09cb6badd73ef61829dc827aa8a98c270bc25b0","16777216":"037eec3d1651a30a90182d9287a5c51386fe35d4a96839cf7969c6e2a03db1fc21","33554432":"03280576b81a04e6abd7197f305506476f5751356b7643988495ca5c3e14e5c262","67108864":"03268bfb05be1dbb33ab6e7e00e438373ca2c9b9abc018fdb452d0e1a0935e10d3","134217728":"02573b68784ceba9617bbcc7c9487836d296aa7c628c3199173a841e7a19798020","268435456":"0234076b6e70f7fbf755d2227ecc8d8169d662518ee3a1401f729e2a12ccb2b276","536870912":"03015bd88961e2a466a2163bd4248d1d2b42c7c58a157e594785e7eb34d880efc9","1073741824":"02c9b076d08f9020ebee49ac8ba2610b404d4e553a4f800150ceb539e9421aaeee","2147483648":"034d592f4c366afddc919a509600af81b489a03caf4f7517c2b3f4f2b558f9a41a","4294967296":"037c09ecb66da082981e4cbdb1ac65c0eb631fc75d85bed13efb2c6364148879b5","8589934592":"02b4ebb0dda3b9ad83b39e2e31024b777cc0ac205a96b9a6cfab3edea2912ed1b3","17179869184":"026cc4dacdced45e63f6e4f62edbc5779ccd802e7fabb82d5123db879b636176e9","34359738368":"02b2cee01b7d8e90180254459b8f09bbea9aad34c3a2fd98c85517ecfc9805af75","68719476736":"037a0c0d564540fc574b8bfa0253cca987b75466e44b295ed59f6f8bd41aace754","137438953472":"021df6585cae9b9ca431318a713fd73dbb76b3ef5667957e8633bca8aaa7214fb6","274877906944":"02b8f53dde126f8c85fa5bb6061c0be5aca90984ce9b902966941caf963648d53a","549755813888":"029cc8af2840d59f1d8761779b2496623c82c64be8e15f9ab577c657c6dd453785","1099511627776":"03e446fdb84fad492ff3a25fc1046fb9a93a5b262ebcd0151caa442ea28959a38a","2199023255552":"02d6b25bd4ab599dd0818c55f75702fde603c93f259222001246569018842d3258","4398046511104":"03397b522bb4e156ec3952d3f048e5a986c20a00718e5e52cd5718466bf494156a","8796093022208":"02d1fb9e78262b5d7d74028073075b80bb5ab281edcfc3191061962c1346340f1e","17592186044416":"030d3f2ad7a4ca115712ff7f140434f802b19a4c9b2dd1c76f3e8e80c05c6a9310","35184372088832":"03e325b691f292e1dfb151c3fb7cad440b225795583c32e24e10635a80e4221c06","70368744177664":"03bee8f64d88de3dee21d61f89efa32933da51152ddbd67466bef815e9f93f8fd1","140737488355328":"0327244c9019a4892e1f04ba3bf95fe43b327479e2d57c25979446cc508cd379ed","281474976710656":"02fb58522cd662f2f8b042f8161caae6e45de98283f74d4e99f19b0ea85e08a56d","562949953421312":"02adde4b466a9d7e59386b6a701a39717c53f30c4810613c1b55e6b6da43b7bc9a","1125899906842624":"038eeda11f78ce05c774f30e393cda075192b890d68590813ff46362548528dca9","2251799813685248":"02ec13e0058b196db80f7079d329333b330dc30c000dbdd7397cbbc5a37a664c4f","4503599627370496":"02d2d162db63675bd04f7d56df04508840f41e2ad87312a3c93041b494efe80a73","9007199254740992":"0356969d6aef2bb40121dbd07c68b6102339f4ea8e674a9008bb69506795998f49","18014398509481984":"02f4e667567ebb9f4e6e180a4113bb071c48855f657766bb5e9c776a880335d1d6","36028797018963968":"0385b4fe35e41703d7a657d957c67bb536629de57b7e6ee6fe2130728ef0fc90b0","72057594037927936":"02b2bc1968a6fddbcc78fb9903940524824b5f5bed329c6ad48a19b56068c144fd","144115188075855872":"02e0dbb24f1d288a693e8a49bc14264d1276be16972131520cf9e055ae92fba19a","288230376151711744":"03efe75c106f931a525dc2d653ebedddc413a2c7d8cb9da410893ae7d2fa7d19cc","576460752303423488":"02c7ec2bd9508a7fc03f73c7565dc600b30fd86f3d305f8f139c45c404a52d958a","1152921504606846976":"035a6679c6b25e68ff4e29d1c7ef87f21e0a8fc574f6a08c1aa45ff352c1d59f06","2305843009213693952":"033cdc225962c052d485f7cfbf55a5b2367d200fe1fe4373a347deb4cc99e9a099","4611686018427387904":"024a4b806cf413d14b294719090a9da36ba75209c7657135ad09bc65328fba9e6f","9223372036854775808":"0377a6fe114e291a8d8e991627c38001c8305b23b9e98b1c7b1893f5cd0dda6cad"
}"#;
        let response: Result<Keys, serde_json::Error> = serde_json::from_str(incorrect_1);
        assert!(response.is_ok());
    }
}
