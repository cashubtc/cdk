//! Main Signatory implementation
//!
//! It is named db_signatory because it uses a database to maintain state.
use std::collections::HashMap;
use std::sync::Arc;

use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::secp256k1::{self, Secp256k1};
use cdk_common::dhke::{sign_message, verify_message};
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::{BlindSignature, BlindedMessage, CurrencyUnit, Id, MintKeySet, Proof};
use cdk_common::{database, Error, PublicKey};
use tokio::sync::RwLock;
use tracing::instrument;

use crate::common::{
    check_unit_string_collision, create_new_keyset, derivation_path_from_unit, init_keysets,
};
use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets};

/// In-memory Signatory
///
/// This is the default signatory implementation for the mint.
///
/// The private keys and the all key-related data is stored in memory, in the same process, but it
/// is not accessible from the outside.
pub struct DbSignatory {
    keysets: RwLock<HashMap<Id, (MintKeySetInfo, MintKeySet)>>,
    active_keysets: RwLock<HashMap<CurrencyUnit, Id>>,
    localstore: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
    secp_ctx: Secp256k1<secp256k1::All>,
    custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    xpriv: Xpriv,
    xpub: PublicKey,
}

impl DbSignatory {
    /// Creates a new MemorySignatory instance
    pub async fn new(
        localstore: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
        seed: &[u8],
        mut supported_units: HashMap<CurrencyUnit, (u64, u8)>,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<Self, Error> {
        let secp_ctx = Secp256k1::new();
        let xpriv = Xpriv::new_master(bitcoin::Network::Bitcoin, seed).expect("RNG busted");
        supported_units.entry(CurrencyUnit::Auth).or_insert((0, 1));

        let (mut active_keysets, active_keyset_units) = init_keysets(
            xpriv,
            &secp_ctx,
            &localstore,
            &supported_units,
            &custom_paths,
        )
        .await?;

        let keysets_infos = localstore.get_keyset_infos().await?;
        let mut tx = localstore.begin_transaction().await?;

        // Create new keysets for supported units that aren't covered by the current keysets
        for (unit, (fee, max_order)) in supported_units {
            if !active_keyset_units.contains(&unit) {
                let derivation_path = match custom_paths.get(&unit) {
                    Some(path) => path.clone(),
                    None => {
                        derivation_path_from_unit(unit.clone(), 0).ok_or(Error::UnsupportedUnit)?
                    }
                };

                let amounts = (0..max_order)
                    .map(|i| 2_u64.pow(i as u32))
                    .collect::<Vec<_>>();

                let (keyset, keyset_info) = create_new_keyset(
                    &secp_ctx,
                    xpriv,
                    derivation_path,
                    Some(0),
                    unit.clone(),
                    &amounts,
                    fee,
                    // TODO: add and connect settings for this
                    None,
                );

                check_unit_string_collision(keysets_infos.clone(), &keyset_info).await?;

                let id = keyset_info.id;
                tx.add_keyset_info(keyset_info).await?;
                tx.set_active_keyset(unit, id).await?;
                active_keysets.insert(id, keyset);
            }
        }

        tx.commit().await?;

        let keys = Self {
            keysets: Default::default(),
            active_keysets: Default::default(),
            localstore,
            custom_paths,
            xpub: xpriv.to_keypair(&secp_ctx).public_key().into(),
            secp_ctx,
            xpriv,
        };
        keys.reload_keys_from_db().await?;

        Ok(keys)
    }

    /// Load all the keysets from the database, even if they are not active.
    ///
    /// Since the database is owned by this process, we can load all the keysets in memory, and use
    /// it as the primary source, and the database as the persistence layer.
    ///
    /// Any operation performed with keysets, are done through this trait and never to the database
    /// directly.
    async fn reload_keys_from_db(&self) -> Result<(), Error> {
        let mut keysets = self.keysets.write().await;
        let mut active_keysets = self.active_keysets.write().await;
        keysets.clear();
        active_keysets.clear();

        let db_active_keysets = self.localstore.get_active_keysets().await?;

        for mut info in self.localstore.get_keyset_infos().await? {
            let id = info.id;
            let keyset = self.generate_keyset(&info);
            info.active = db_active_keysets.get(&info.unit) == Some(&info.id);
            if info.active {
                active_keysets.insert(info.unit.clone(), id);
            }
            keysets.insert(id, (info, keyset));
        }

        Ok(())
    }

    fn generate_keyset(&self, keyset_info: &MintKeySetInfo) -> MintKeySet {
        MintKeySet::generate_from_xpriv(
            &self.secp_ctx,
            self.xpriv,
            &keyset_info.amounts,
            keyset_info.unit.clone(),
            keyset_info.derivation_path.clone(),
            keyset_info.final_expiry,
            keyset_info.id.get_version(),
        )
    }
}

#[async_trait::async_trait]
impl Signatory for DbSignatory {
    fn name(&self) -> String {
        format!("Signatory {}", env!("CARGO_PKG_VERSION"))
    }

    #[instrument(skip_all)]
    async fn blind_sign(
        &self,
        blinded_messages: Vec<BlindedMessage>,
    ) -> Result<Vec<BlindSignature>, Error> {
        let keysets = self.keysets.read().await;

        blinded_messages
            .into_iter()
            .map(|blinded_message| {
                let BlindedMessage {
                    amount,
                    blinded_secret,
                    keyset_id,
                    ..
                } = blinded_message;

                let (info, key) = keysets.get(&keyset_id).ok_or(Error::UnknownKeySet)?;
                if !info.active {
                    return Err(Error::InactiveKeyset);
                }

                let key_pair = key.keys.get(&amount).ok_or(Error::UnknownKeySet)?;
                let c = sign_message(&key_pair.secret_key, &blinded_secret)?;

                let blinded_signature = BlindSignature::new(
                    amount,
                    c,
                    keyset_id,
                    &blinded_message.blinded_secret,
                    key_pair.secret_key.clone(),
                )?;

                Ok(blinded_signature)
            })
            .collect::<Result<Vec<_>, _>>()
    }

    #[tracing::instrument(skip_all)]
    async fn verify_proofs(&self, proofs: Vec<Proof>) -> Result<(), Error> {
        let keysets = self.keysets.read().await;

        proofs.into_iter().try_for_each(|proof| {
            let (_, key) = keysets.get(&proof.keyset_id).ok_or(Error::UnknownKeySet)?;
            let key_pair = key.keys.get(&proof.amount).ok_or(Error::UnknownKeySet)?;
            verify_message(&key_pair.secret_key, proof.c, proof.secret.as_bytes())?;
            Ok(())
        })
    }

    #[tracing::instrument(skip_all)]
    async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
        Ok(SignatoryKeysets {
            pubkey: self.xpub,
            keysets: self
                .keysets
                .read()
                .await
                .values()
                .map(|k| k.into())
                .collect::<Vec<_>>(),
        })
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[tracing::instrument(skip(self))]
    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
        let path_index = if let Some(current_keyset_id) =
            self.localstore.get_active_keyset_id(&args.unit).await?
        {
            let keyset_info = self
                .localstore
                .get_keyset_info(&current_keyset_id)
                .await?
                .ok_or(Error::UnknownKeySet)?;

            keyset_info.derivation_path_index.unwrap_or(1) + 1
        } else {
            1
        };

        let derivation_path = match self.custom_paths.get(&args.unit) {
            Some(path) => path.clone(),
            None => derivation_path_from_unit(args.unit.clone(), path_index)
                .ok_or(Error::UnsupportedUnit)?,
        };

        let (keyset, info) = create_new_keyset(
            &self.secp_ctx,
            self.xpriv,
            derivation_path,
            Some(path_index),
            args.unit.clone(),
            &args.amounts,
            args.input_fee_ppk,
            // TODO: add and connect settings for this
            None,
        );

        let keysets_infos = self.localstore.get_keyset_infos().await?;
        check_unit_string_collision(keysets_infos, &info).await?;

        let id = info.id;
        let mut tx = self.localstore.begin_transaction().await?;
        tx.add_keyset_info(info.clone()).await?;
        tx.set_active_keyset(args.unit, id).await?;
        tx.commit().await?;

        self.reload_keys_from_db().await?;

        Ok((&(info, keyset)).into())
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use bitcoin::key::Secp256k1;
    use bitcoin::Network;
    use cdk_common::util::hex;
    use cdk_common::{Amount, MintKeySet, PublicKey};

    use super::*;

    #[test]
    fn mint_mod_generate_keyset_from_seed() {
        let seed = hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        let keyset = MintKeySet::generate_from_seed(
            &Secp256k1::new(),
            &seed,
            &[1, 2],
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0).unwrap(),
            None,
            cdk_common::nut02::KeySetVersion::Version00,
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 2);

        let expected_amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = vec![
            (
                Amount::from(1),
                PublicKey::from_hex(
                    "0380a4bb98d9bc5d5b11c7cf2b705dbc894b62ac99cf67e0ef1a3d47ea6dc54706",
                )
                .unwrap(),
            ),
            (
                Amount::from(2),
                PublicKey::from_hex(
                    "022fe5e50a15d721014b538ca6a3ff20ee049b195ba0b1705f64829da8779b6940",
                )
                .unwrap(),
            ),
        ]
        .into_iter()
        .collect();

        let amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = keyset
            .keys
            .iter()
            .map(|(amount, pair)| (*amount, pair.public_key))
            .collect();

        assert_eq!(amounts_and_pubkeys, expected_amounts_and_pubkeys);
    }

    #[test]
    fn mint_mod_generate_keyset_from_xpriv() {
        let seed = hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        let network = Network::Bitcoin;
        let xpriv = Xpriv::new_master(network, &seed).expect("Failed to create xpriv");
        let keyset = MintKeySet::generate_from_xpriv(
            &Secp256k1::new(),
            xpriv,
            &[1, 2],
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0).unwrap(),
            None,
            cdk_common::nut02::KeySetVersion::Version00,
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 2);

        let expected_amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = vec![
            (
                Amount::from(1),
                PublicKey::from_hex(
                    "0380a4bb98d9bc5d5b11c7cf2b705dbc894b62ac99cf67e0ef1a3d47ea6dc54706",
                )
                .unwrap(),
            ),
            (
                Amount::from(2),
                PublicKey::from_hex(
                    "022fe5e50a15d721014b538ca6a3ff20ee049b195ba0b1705f64829da8779b6940",
                )
                .unwrap(),
            ),
        ]
        .into_iter()
        .collect();

        let amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = keyset
            .keys
            .iter()
            .map(|(amount, pair)| (*amount, pair.public_key))
            .collect();

        assert_eq!(amounts_and_pubkeys, expected_amounts_and_pubkeys);
    }

    #[test]
    fn mint_make_btc_remote_signer_keyset() {
        let seed = hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        let network = Network::Bitcoin;
        let xpriv = Xpriv::new_master(network, &seed).expect("Failed to create xpriv");
        let keyset = MintKeySet::generate_from_xpriv(
            &Secp256k1::new(),
            xpriv,
            &[
                1,
                2,
                4,
                8,
                16,
                32,
                64,
                128,
                256,
                512,
                1024,
                2048,
                4096,
                8192,
                16384,
                32768,
                65536,
                131072,
                262144,
                524288,
                1_048_576,
                2_097_152,
                4_194_304,
                8_388_608,
                16_777_216,
                33_554_432,
                67_108_864,
                134_217_728,
                268_435_456,
                536_870_912,
                1_073_741_824,
                2_147_483_648,
                4_294_967_296,
                8_589_934_592,
                17_179_869_184,
                34_359_738_368,
                68_719_476_736,
                137_438_953_472,
                274_877_906_944,
                549_755_813_888,
                1_099_511_627_776,
                2_199_023_255_552,
                4_398_046_511_104,
                8_796_093_022_208,
                17_592_186_044_416,
                35_184_372_088_832,
                70_368_744_177_664,
                140_737_488_355_328,
                281_474_976_710_656,
                562_949_953_421_312,
                1_125_899_906_842_624,
                2_251_799_813_685_248,
                4_503_599_627_370_496,
                9_007_199_254_740_992,
                18_014_398_509_481_984,
                36_028_797_018_963_968,
                72_057_594_037_927_936,
                144_115_188_075_855_872,
                288_230_376_151_711_744,
                576_460_752_303_423_488,
                1_152_921_504_606_846_976,
                2_305_843_009_213_693_952,
                4_611_686_018_427_387_904,
                9_223_372_036_854_775_808,
            ],
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 1).unwrap(),
            None,
            cdk_common::nut02::KeySetVersion::Version00,
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 64);

        let expected_results: HashMap<u64, &str> = [
            (
                1,
                "0233501d047ff4058007722d5d24e10a8ff5c723a677be411fff46a3cee9a92cc0",
            ),
            (
                2,
                "03a09803ce40118b8917fafa08409dbe6e8bb36d76c55f4c58400cd720abaf54cb",
            ),
            (
                4,
                "02dac058df2e8611098286ef87ee9698f555548784ab4b1a860c79338073ad8c49",
            ),
            (
                8,
                "025b66b937d65544981817aa9a053a762a7d72a7543c66a54370ea68aa53170a10",
            ),
            (
                16,
                "027cf2ad5fa02b99ea37b305048562828453d89dfa7defcda1c10f6746f25f7541",
            ),
            (
                32,
                "0336033cbbc044737bced1fd40b7f0cb0ce08a83aedaa882ed1ced875a1f517879",
            ),
            (
                64,
                "035be95ecaadbfe67b14f07205d13bbcab5da58bb595c57dfb9b61c5e3e7e4de0e",
            ),
            (
                128,
                "0232c757957a8f5a14e93a9bbe8852c273b985ad238ce9b4d5a16885d8a761462b",
            ),
            (
                256,
                "02cbd889df7d38e95dca2ee0e09bc22e3ae57e95975043854a5560a464f970ac1f",
            ),
            (
                512,
                "02c99a0b72ba8f01c5da765c534e75ae3e5f51e4931bfced18a91df4b9233b168f",
            ),
            (
                1024,
                "0320527abb6ae3dd6db9da5041ca941be679e953b446614843af7a4393e9ac96bc",
            ),
            (
                2048,
                "033f9276b0c5f73fbeb0130eab5705a8e878f4191fe251a18cbd918cda3c9e2d5e",
            ),
            (
                4096,
                "03cf69ed2939be4ac35308560d4423e1a0d96cacf9fe33267c7e6a047bf438e53e",
            ),
            (
                8192,
                "027c8bfff71352766c3870e9f5f577830bbb44eadfb757fdff9a8cd209c4b22d76",
            ),
            (
                16384,
                "02ea21bd310828b9e46746eba2ae985626b3a2efc2468db66ae480715dc6deec8a",
            ),
            (
                32768,
                "027ae7179192282d5b44ac55bff82c13e1ea916ae1edefa33ea64100be7408e015",
            ),
            (
                65536,
                "028f333c1beada3445cb62108e35d72199925a055c1e7c102c742e1761770f6c62",
            ),
            (
                131072,
                "03de95cae3614499a3df2d412e91aa09ddef8b8d49e8d652e3798419da86958139",
            ),
            (
                262144,
                "03c7817c19b4b107eb2ccf2f32b60f9c22a59a1d4a93e492ad01f1505097a654b7",
            ),
            (
                524288,
                "028aad03886b6ec6b9f628090e9c151a73f025aa949a9686dac1f0b32995a4e8df",
            ),
            (
                1048576,
                "034bf50a5916d9f112b8fbfe82a5ac914b5bec792b107cf25922c9866f002473e8",
            ),
            (
                2097152,
                "03d2894e1b1b7ab7497ff69e16d280b630f60ba34fe00edd7c748ae5ee73bc0d1a",
            ),
            (
                4194304,
                "0285ba0ee2960927de958610b13d63fc29019407eb32c477d9a2d016fda3062a37",
            ),
            (
                8388608,
                "03d7a4b4b1b8d6b9f2b5966e380a62f8efd53f79d1965e076a716d2fb75e9774a1",
            ),
            (
                16777216,
                "037a033e2f1df992523df83bcb9aa02cefdadd59882d7949f4500f5493d89fa2fd",
            ),
            (
                33554432,
                "03014de7af4809599cabc6d6b30e5121b4a88153eb38a7b66dd8e50e3166215ab0",
            ),
            (
                67108864,
                "0240162a1d2eb1841450de53a6244a625922b14006153d5219dad0fcf0c369c497",
            ),
            (
                134217728,
                "03f8c6f7b0ee71f66940a33c746c3bf8b1cba793a498dd2fdeb6857552415a4d5d",
            ),
            (
                268435456,
                "02dc9de15fa1332f5a2c8f85045ea127cbc3407fb8a844b453f38e1c9cdce9ef87",
            ),
            (
                536870912,
                "0291bdcb1719b5bf447b2885efc84061d1de30b9d1f583d25034059457a2fd739e",
            ),
            (
                1073741824,
                "02f8a96485e3fa791f57d7f4ef279dd3617b873efbdf673815c49dbf9ce7422b0d",
            ),
            (
                2147483648,
                "02ff8cf3e3de985bb2f286c98e335a175b2b53a0e0d7fa1f53d642c95a372329a2",
            ),
            (
                4294967296,
                "02d96196cc54e7506bfe9fdb4a0d691eed2948ecb9b8e81d28d27225287ad5debc",
            ),
            (
                8589934592,
                "03e64e5664f7ab843f41aaf4c0534d698b3318d140c23cbd2fcc33eece53400dac",
            ),
            (
                17179869184,
                "034c9a4bf7b4cb8fac6ace994624e5250ddac5ac84541b6c8bd12b71d22719bb2d",
            ),
            (
                34359738368,
                "0313027c2b106c7dcdee0d806c3343026260276c6793d4d1dfdf79aae30875be31",
            ),
            (
                68719476736,
                "03081adca96d42cb2ac4ac94e0ea2aac4d9412265ae55ed377e3c0357aa1157253",
            ),
            (
                137438953472,
                "02fdc4118761739425220ba87dee5ea9fdc1d581abfcb506fb5afabf76e172b798",
            ),
            (
                274877906944,
                "031dd7cd25f761c8f80828b487bab1cef730f68e8d6f2026b443cc7223862f6c73",
            ),
            (
                549755813888,
                "02da505eab15744a6fd3fa6b3257bced520d4d294ea94444528fd30d7f90948629",
            ),
            (
                1099511627776,
                "02bfc54369099958275376ab030f2a085532c8a00ae4d1bbfa5031c64b42d58a47",
            ),
            (
                2199023255552,
                "032241a5d4d1e988b8ae85f68a381df0e40065ae8c81b1c4f7ea31c87eab2c0d81",
            ),
            (
                4398046511104,
                "03a681e41990d350cdedd30840f26ad970b4015dd6e6b5c03f7cc99b384bee8762",
            ),
            (
                8796093022208,
                "033d5293a33cda29d65058d6d3a4b821472574e92414fa052c79f8bdc1cd72faba",
            ),
            (
                17592186044416,
                "033ddfec40622aaf62d672f43fd05ddb396afd7ad9f00daede45102c890d3a012b",
            ),
            (
                35184372088832,
                "02564bbdcbed18a8e2d79b2fdad6e5e8a9fe92e853ab23170934d84015cc4b96b0",
            ),
            (
                70368744177664,
                "02170950642b94d0ed232370d5dd3630b5eb7e73791447fb961b12d8139de975de",
            ),
            (
                140737488355328,
                "02b2add5a6eb5dc06f706e9dba190ba412c2c7ba240284b336b66ef38a39e51f1c",
            ),
            (
                281474976710656,
                "03e3e584a4bc1d0a6399f5b6b9355bd67a10ad9f46c8a4283de96854e47eb4357c",
            ),
            (
                562949953421312,
                "033821262e6a78f29dad81d3133845883a7632a47f51ab1d99a0eae4a5354eef45",
            ),
            (
                1125899906842624,
                "038db672a61c70dc66b504152ea39b607527f2f59e8ebfdf8d955c38e914661534",
            ),
            (
                2251799813685248,
                "03dafb9683eac036a422266ddc85b675bf13aeafe0658cad2ec1555c28f4049b28",
            ),
            (
                4503599627370496,
                "0351733345d4bb491e27bdb221e382d00f2248f2ee7f04dc6f3faab2692fbd296c",
            ),
            (
                9007199254740992,
                "03f930c1e6c154ca169370adbec7691fd9c11245867a37ae086f7547f5c9e8386f",
            ),
            (
                18014398509481984,
                "02d700dc30d3cd6be292bddbd5f74c09df784862c785cd763ad6c829be59c21bed",
            ),
            (
                36028797018963968,
                "03444b9c312900fffbd478e390aa6fdf9d3ffe230239141ecadf0bcee25e379512",
            ),
            (
                72057594037927936,
                "03af7acedfcfcaf83cfdb7d171ef64723286bd6e0ab90f3629e627e77955917776",
            ),
            (
                144115188075855872,
                "02e35aef647a881e8c318879fb81b6261df73e385dfbc5ff3fc0ab40f13f5ed560",
            ),
            (
                288230376151711744,
                "024558ed8e986901e05839c34d17c261c8d93b8cabb5dee83ab805bb5028e5e463",
            ),
            (
                576460752303423488,
                "024f60a89ba055e009d84a90a13a7860a909fb486a8ffb4315c2f59aff6fbfd929",
            ),
            (
                1152921504606846976,
                "0311b2a5b91dfaebab4fb125338fd38dab72ec5671e6db5f468cb1477970ea3876",
            ),
            (
                2305843009213693952,
                "02aeaa116d930767b5143cac922511c0e093beee5a2850f67490f5a5bb44a8af76",
            ),
            (
                4611686018427387904,
                "02bf7003847bc8e7ad35ea5c8975e3fdde8d1c43ef540d250cf2dc75792c733647",
            ),
            (
                9223372036854775808,
                "0376b06a13092fbb679f6e7a90ce877c37d5a20714a65567177a91a0479b3e86a9",
            ),
        ]
        .into_iter()
        .collect();

        assert_eq!(keyset.id.to_string(), "00b5a0580f75cc2f".to_string());

        for key in expected_results {
            let amount = Amount::from(key.0);
            let pubkey = keyset
                .keys
                .get(&amount)
                .unwrap()
                .public_key
                .clone()
                .to_hex();

            assert_eq!(pubkey, key.1.to_string());
        }
    }

    #[test]
    fn mint_make_auth_remote_signer_keyset() {
        let seed = hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        let network = Network::Bitcoin;
        let xpriv = Xpriv::new_master(network, &seed).expect("Failed to create xpriv");
        let keyset = MintKeySet::generate_from_xpriv(
            &Secp256k1::new(),
            xpriv,
            &[1],
            CurrencyUnit::Auth,
            derivation_path_from_unit(CurrencyUnit::Auth, 1).unwrap(),
            None,
            cdk_common::nut02::KeySetVersion::Version00,
        );

        assert_eq!(keyset.unit, CurrencyUnit::Auth);
        assert_eq!(keyset.keys.len(), 1);

        assert_eq!(keyset.id.to_string(), "00e1cf6079abb988".to_string());

        let amount = Amount::from(1);
        let pubkey = keyset
            .keys
            .get(&amount)
            .unwrap()
            .public_key
            .clone()
            .to_hex();
        assert_eq!(
            pubkey,
            "025b6c1ca8bb741a6f2321c953266df7bf3f3f2c3be8c54c0a6e41bb00976046a4".to_string()
        );
    }
}
