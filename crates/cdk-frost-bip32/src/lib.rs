//! Bitcoin BIP32 and BIP341 derivation for application-neutral CDK FROST keys.

use std::fmt;

use bitcoin::bip32::{ChainCode, ChildNumber, Fingerprint, Xpub};
use bitcoin::key::TweakedPublicKey;
use bitcoin::secp256k1::{PublicKey, Secp256k1};
use bitcoin::{Address, Network};
use cdk_frost::{
    derive_signing_public, ApplicationDerivationVersion, ApplicationId, ApplicationRegistration,
    DerivedPublicPackage, DerivedSigningPublicPackage, Error as FrostError, SignatureMode,
    SigningDerivation,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Stable CDK federated on-chain wallet application label.
pub const APPLICATION_LABEL: &str = "org.cashubtc.cdk.federated-onchain";
/// Current application derivation contract version.
pub const APPLICATION_DERIVATION_VERSION: u16 = 1;
const MAX_NORMAL_INDEX: u32 = (1 << 31) - 1;

/// BIP32 branch used by the watch-only descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WalletBranch {
    /// Receive/change=false descriptor branch `0`.
    External,
    /// Change descriptor branch `1`.
    Internal,
}

impl WalletBranch {
    /// Return the canonical unhardened BIP32 branch index.
    pub const fn index(self) -> u32 {
        match self {
            Self::External => 0,
            Self::Internal => 1,
        }
    }
}

/// Valid unhardened BIP32 child index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WalletIndex(u32);

impl WalletIndex {
    /// Validate an unhardened wallet index.
    pub fn new(index: u32) -> Result<Self, Error> {
        if index > MAX_NORMAL_INDEX {
            return Err(Error::HardenedIndex { index });
        }
        Ok(Self(index))
    }

    /// Return the numeric child index.
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Return the deterministic BIP32 retry index after an invalid child.
    pub fn next(self) -> Option<Self> {
        self.0
            .checked_add(1)
            .and_then(|index| Self::new(index).ok())
    }
}

/// External and internal Taproot descriptor strings for a watch-only BDK wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchOnlyDescriptors {
    /// Receive descriptor.
    pub external: String,
    /// Change descriptor.
    pub internal: String,
}

/// One fully derived BIP32/Taproot signing target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedWalletKey {
    branch: WalletBranch,
    index: WalletIndex,
    child_xpub: Xpub,
    derivation: SigningDerivation,
    public: DerivedSigningPublicPackage,
}

impl DerivedWalletKey {
    /// Descriptor branch.
    pub const fn branch(&self) -> WalletBranch {
        self.branch
    }

    /// Descriptor index.
    pub const fn index(&self) -> WalletIndex {
        self.index
    }

    /// BIP32 child extended public key before the BIP341 output-key tweak.
    pub const fn child_xpub(&self) -> Xpub {
        self.child_xpub
    }

    /// Exact derivation plan that signers must authorize in round two.
    pub fn signing_derivation(&self) -> &SigningDerivation {
        &self.derivation
    }

    /// Public FROST target corresponding to the BDK descriptor key.
    pub fn public_package(&self) -> &DerivedSigningPublicPackage {
        &self.public
    }

    /// P2TR key-path address for this target.
    pub fn address(&self, network: Network) -> Result<Address, Error> {
        let output = PublicKey::from_slice(&self.public.output_group_key())?;
        let (x_only, _) = output.x_only_public_key();
        Ok(Address::p2tr_tweaked(
            TweakedPublicKey::dangerous_assume_tweaked(x_only),
            network,
        ))
    }
}

/// Public BIP32 adapter rooted at an application-derived FROST package.
#[derive(Clone, PartialEq, Eq)]
pub struct FrostBip32 {
    application: DerivedPublicPackage,
    root_xpub: Xpub,
    network: Network,
}

impl fmt::Debug for FrostBip32 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrostBip32")
            .field("root_epoch", &self.application.root_epoch())
            .field("registration", &self.application.registration())
            .field("root_xpub", &self.root_xpub)
            .field("network", &self.network)
            .finish()
    }
}

impl FrostBip32 {
    /// Construct the public adapter and its deterministic application chain code.
    pub fn new(application: DerivedPublicPackage, network: Network) -> Result<Self, Error> {
        let public_key = PublicKey::from_slice(&application.group_key())?;
        let chain_code = application_chain_code(&application);
        let root_xpub = Xpub {
            network: network.into(),
            depth: 0,
            parent_fingerprint: Fingerprint::default(),
            child_number: ChildNumber::Normal { index: 0 },
            public_key,
            chain_code: ChainCode::from(chain_code),
        };
        Ok(Self {
            application,
            root_xpub,
            network,
        })
    }

    /// Application extended public key used by both descriptors.
    pub const fn root_xpub(&self) -> Xpub {
        self.root_xpub
    }

    /// Produce standard external and internal BIP86-style Taproot descriptors.
    pub fn descriptors(&self) -> WatchOnlyDescriptors {
        WatchOnlyDescriptors {
            external: format!("tr({}/0/*)", self.root_xpub),
            internal: format!("tr({}/1/*)", self.root_xpub),
        }
    }

    /// Derive the exact public child and signing plan used by BDK.
    pub fn derive(
        &self,
        branch: WalletBranch,
        index: WalletIndex,
    ) -> Result<DerivedWalletKey, Error> {
        let secp = Secp256k1::verification_only();
        let branch_number = ChildNumber::Normal {
            index: branch.index(),
        };
        let index_number = ChildNumber::Normal { index: index.get() };
        let (branch_tweak, _) = self.root_xpub.ckd_pub_tweak(branch_number)?;
        let branch_xpub = self.root_xpub.ckd_pub(&secp, branch_number)?;
        let (index_tweak, _) = branch_xpub
            .ckd_pub_tweak(index_number)
            .map_err(|error| invalid_child_error(index, error))?;
        let child_xpub = branch_xpub
            .ckd_pub(&secp, index_number)
            .map_err(|error| invalid_child_error(index, error))?;
        let context = derivation_context(
            &self.application,
            self.network,
            branch,
            index,
            self.root_xpub,
        );
        let derivation = SigningDerivation::new(
            context,
            vec![branch_tweak.secret_bytes(), index_tweak.secret_bytes()],
            SignatureMode::TaprootKeyPath { merkle_root: None },
        )?;
        let public = derive_signing_public(&self.application, &derivation)?;
        if public.internal_group_key() != child_xpub.public_key.serialize() {
            return Err(Error::PublicDerivationMismatch);
        }
        Ok(DerivedWalletKey {
            branch,
            index,
            child_xpub,
            derivation,
            public,
        })
    }
}

/// Stable registration for the CDK federated on-chain wallet adapter.
pub fn application_registration() -> Result<ApplicationRegistration, Error> {
    let id = ApplicationId::from_label(APPLICATION_LABEL)?;
    let version = ApplicationDerivationVersion::new(APPLICATION_DERIVATION_VERSION)?;
    let policy: [u8; 32] = Sha256::digest(b"cdk/frost/federated-onchain-authorization/v1").into();
    Ok(ApplicationRegistration::new(id, version, policy)?)
}

fn application_chain_code(application: &DerivedPublicPackage) -> [u8; 32] {
    let registration = application.registration();
    let mut hasher = Sha256::new();
    hasher.update(b"cdk/frost/bip32-chain-code/v1");
    hasher.update(application.root_epoch().get().to_be_bytes());
    hasher.update(registration.id().to_bytes());
    hasher.update(registration.derivation_version().get().to_be_bytes());
    hasher.update(registration.authorization_policy());
    hasher.update(application.group_key());
    hasher.finalize().into()
}

fn derivation_context(
    application: &DerivedPublicPackage,
    network: Network,
    branch: WalletBranch,
    index: WalletIndex,
    root_xpub: Xpub,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"cdk/frost/bip32-signing-target/v1");
    hasher.update(application.root_epoch().get().to_be_bytes());
    hasher.update(application.registration().id().to_bytes());
    hasher.update([network_tag(network)]);
    hasher.update([branch.index() as u8]);
    hasher.update(index.get().to_be_bytes());
    hasher.update(root_xpub.encode());
    hasher.finalize().into()
}

const fn network_tag(network: Network) -> u8 {
    match network {
        Network::Bitcoin => 0,
        Network::Testnet => 1,
        Network::Testnet4 => 2,
        Network::Signet => 3,
        Network::Regtest => 4,
    }
}

fn invalid_child_error(index: WalletIndex, error: bitcoin::bip32::Error) -> Error {
    match index.next() {
        Some(next) => Error::InvalidChild {
            index: index.get(),
            next_index: next.get(),
            reason: error.to_string(),
        },
        None => Error::TerminalInvalidChild {
            index: index.get(),
            reason: error.to_string(),
        },
    }
}

/// BIP32/Taproot adapter error.
#[derive(Debug, Error)]
pub enum Error {
    /// A hardened index cannot be represented by a public descriptor.
    #[error("wallet index {index} is hardened")]
    HardenedIndex {
        /// Rejected BIP32 index.
        index: u32,
    },
    /// Bitcoin public-key or BIP32 derivation failed.
    #[error("Bitcoin key derivation failed: {0}")]
    Bitcoin(#[from] bitcoin::bip32::Error),
    /// BIP32 specified an invalid child and every member must retry this index.
    #[error("BIP32 child {index} is invalid ({reason}); retry index {next_index}")]
    InvalidChild {
        /// Rejected child index.
        index: u32,
        /// Next consensus index all members must try.
        next_index: u32,
        /// Underlying BIP32 failure.
        reason: String,
    },
    /// The last unhardened child was invalid, so no automatic retry exists.
    #[error("final BIP32 child {index} is invalid: {reason}")]
    TerminalInvalidChild {
        /// Rejected final child index.
        index: u32,
        /// Underlying BIP32 failure.
        reason: String,
    },
    /// A compressed secp256k1 public key was malformed.
    #[error("secp256k1 public key is malformed: {0}")]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// The generic FROST boundary rejected the derivation.
    #[error("FROST derivation failed: {0}")]
    Frost(#[from] FrostError),
    /// BIP32 and additive FROST public derivation did not agree.
    #[error("BIP32 and FROST public derivation disagree")]
    PublicDerivationMismatch,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use cdk_bdk::DescriptorConfig;
    use cdk_frost::{DerivedPublicPackage, MemberId, RootKeyEpoch};

    use super::*;

    fn application() -> DerivedPublicPackage {
        application_with(
            application_registration().expect("registration"),
            RootKeyEpoch::new(7).expect("epoch"),
            7,
        )
    }

    fn application_with(
        registration: ApplicationRegistration,
        epoch: RootKeyEpoch,
        group_seed: u8,
    ) -> DerivedPublicPackage {
        let secp = Secp256k1::new();
        let point = |value: u8| {
            let secret = SecretKey::from_slice(&[value; 32]).expect("test secret");
            PublicKey::from_secret_key(&secp, &secret).serialize()
        };
        let shares = BTreeMap::from([
            (MemberId::new(1).expect("member"), point(11)),
            (MemberId::new(2).expect("member"), point(12)),
            (MemberId::new(3).expect("member"), point(13)),
        ]);
        DerivedPublicPackage::new(epoch, registration, 2, shares, point(group_seed))
            .expect("application package")
    }

    #[test]
    fn descriptors_and_frost_derivation_match_bdk() {
        let network = Network::Regtest;
        let adapter = FrostBip32::new(application(), network).expect("adapter");
        let descriptors = adapter.descriptors();
        let config = DescriptorConfig::new(&descriptors.external, &descriptors.internal, network)
            .expect("BDK descriptors");

        for (branch, descriptor) in [
            (WalletBranch::External, config.external()),
            (WalletBranch::Internal, config.internal()),
        ] {
            for raw_index in [0, 1, 17, 1_000_003, MAX_NORMAL_INDEX] {
                let index = WalletIndex::new(raw_index).expect("normal index");
                let target = adapter.derive(branch, index).expect("derived target");
                let script = descriptor
                    .at_derivation_index(raw_index)
                    .expect("descriptor child")
                    .script_pubkey();
                let bdk_address = Address::from_script(&script, network).expect("P2TR address");
                assert_eq!(target.address(network).expect("FROST address"), bdk_address);
                assert_eq!(
                    target.public_package().internal_group_key(),
                    target.child_xpub().public_key.serialize()
                );
            }
        }
    }

    #[test]
    fn branches_networks_versions_and_hardened_indexes_are_separated() {
        let application = application();
        let regtest = FrostBip32::new(application.clone(), Network::Regtest).expect("regtest");
        let mainnet = FrostBip32::new(application, Network::Bitcoin).expect("mainnet");
        let index = WalletIndex::new(42).expect("index");
        let external = regtest
            .derive(WalletBranch::External, index)
            .expect("external");
        let internal = regtest
            .derive(WalletBranch::Internal, index)
            .expect("internal");
        let other_network = mainnet
            .derive(WalletBranch::External, index)
            .expect("mainnet");
        assert_ne!(
            external.public_package().internal_group_key(),
            internal.public_package().internal_group_key()
        );
        assert_ne!(
            external.signing_derivation().binding(),
            internal.signing_derivation().binding()
        );
        assert_eq!(
            external.public_package().internal_group_key(),
            other_network.public_package().internal_group_key()
        );
        assert_ne!(
            external.signing_derivation().binding(),
            other_network.signing_derivation().binding()
        );
        assert!(matches!(
            WalletIndex::new(1 << 31),
            Err(Error::HardenedIndex { .. })
        ));
        assert_eq!(WalletIndex::new(41).expect("index").next(), Some(index));
        assert_eq!(
            WalletIndex::new(MAX_NORMAL_INDEX).expect("index").next(),
            None
        );
    }

    #[test]
    fn stable_vector() {
        let adapter = FrostBip32::new(application(), Network::Regtest).expect("adapter");
        let target = adapter
            .derive(WalletBranch::External, WalletIndex::new(0).expect("index"))
            .expect("target");
        assert_eq!(adapter.root_xpub().to_string(), "tpubD6NzVbkrYhZ4XzHFcgP6FEE3aDGdGsiCHVKwSEHX8ykmuUekLF2m4CVAJDvJ48pRVeoLXsmtvGN3ZkW4fUt3FNn4sumG6kN6ah7Va18EPFN");
        assert_eq!(
            target.public_package().internal_group_key(),
            [
                0x02, 0x62, 0x4f, 0x78, 0x99, 0x7b, 0x3e, 0x6c, 0xfc, 0x37, 0x4f, 0xf2, 0x32, 0x72,
                0x5d, 0x46, 0xbe, 0x41, 0xf1, 0xbc, 0x62, 0xd5, 0xf3, 0x3c, 0x6a, 0x7b, 0xd1, 0xb7,
                0x70, 0x41, 0x36, 0x82, 0xa4,
            ]
        );
        assert_eq!(
            target.public_package().output_group_key(),
            [
                0x03, 0xf0, 0x42, 0xb2, 0xf6, 0xf9, 0x07, 0xf5, 0x6d, 0xec, 0xab, 0x3a, 0x54, 0x8d,
                0x72, 0x39, 0x48, 0x61, 0xa4, 0x46, 0x2a, 0x91, 0x5a, 0xbc, 0x0d, 0x12, 0x1d, 0xde,
                0x7b, 0x65, 0xbc, 0xcb, 0xae,
            ]
        );
        assert_eq!(
            target.signing_derivation().binding(),
            [
                0xe8, 0xea, 0x43, 0x17, 0x64, 0xf7, 0x9b, 0x1c, 0x94, 0x5d, 0x92, 0x64, 0x59, 0x72,
                0xb0, 0x91, 0xda, 0x87, 0xd4, 0x77, 0x6e, 0xc7, 0x34, 0xd1, 0xf4, 0xc0, 0x87, 0x89,
                0x82, 0x36, 0x2e, 0xf0,
            ]
        );
        assert_eq!(
            target
                .address(Network::Regtest)
                .expect("address")
                .to_string(),
            "bcrt1p7ppt9aheql6kmm9t8f2g6u3efps6g332j9dtcrgjrh08keduewhquz8jwf"
        );
    }

    #[test]
    fn public_derivation_property_and_domain_separation() {
        let network = Network::Signet;
        let adapter = FrostBip32::new(application(), network).expect("adapter");
        let descriptors = adapter.descriptors();
        let config = DescriptorConfig::new(&descriptors.external, &descriptors.internal, network)
            .expect("BDK descriptors");
        let mut candidate = 0x5a17_91d3_u32;
        for _ in 0..256 {
            candidate = candidate
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223)
                & MAX_NORMAL_INDEX;
            let index = WalletIndex::new(candidate).expect("normal index");
            for (branch, descriptor) in [
                (WalletBranch::External, config.external()),
                (WalletBranch::Internal, config.internal()),
            ] {
                let target = adapter.derive(branch, index).expect("FROST child");
                let script = descriptor
                    .at_derivation_index(candidate)
                    .expect("descriptor child")
                    .script_pubkey();
                assert_eq!(
                    target.address(network).expect("FROST address"),
                    Address::from_script(&script, network).expect("BDK address")
                );
            }
        }

        let base = application_registration().expect("base registration");
        let version_two = ApplicationRegistration::new(
            base.id(),
            ApplicationDerivationVersion::new(2).expect("version"),
            base.authorization_policy(),
        )
        .expect("version-two registration");
        let other_application = ApplicationRegistration::new(
            ApplicationId::from_label("org.cashubtc.cdk.another-onchain-test")
                .expect("application ID"),
            base.derivation_version(),
            base.authorization_policy(),
        )
        .expect("other registration");
        let epoch_two = RootKeyEpoch::new(8).expect("epoch");
        let index = WalletIndex::new(9).expect("index");
        let base_key = adapter
            .derive(WalletBranch::External, index)
            .expect("base key");
        for package in [
            application_with(version_two, RootKeyEpoch::new(7).expect("epoch"), 7),
            application_with(other_application, RootKeyEpoch::new(7).expect("epoch"), 8),
            application_with(base, epoch_two, 7),
        ] {
            let separated = FrostBip32::new(package, network)
                .expect("separated adapter")
                .derive(WalletBranch::External, index)
                .expect("separated key");
            assert_ne!(
                base_key.public_package().internal_group_key(),
                separated.public_package().internal_group_key()
            );
            assert_ne!(
                base_key.signing_derivation().binding(),
                separated.signing_derivation().binding()
            );
        }
    }
}
