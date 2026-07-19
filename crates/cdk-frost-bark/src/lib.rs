//! Bark VTXO key derivation rooted at a registered CDK FROST application.

use bitcoin::secp256k1::PublicKey;
use cdk_frost::{
    derive_signing_public, ApplicationDerivationVersion, ApplicationId, ApplicationKeyEpoch,
    ApplicationRegistration, DerivedPublicPackage, DerivedSigningPublicPackage,
    Error as FrostError, SignatureMode, SigningDerivation,
};
use k256::elliptic_curve::bigint::U256;
use k256::elliptic_curve::ops::Reduce;
use k256::Scalar;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Stable Bark VTXO custody application label.
pub const APPLICATION_LABEL: &str = "org.cashubtc.cdk.federated-bark";
/// Current Bark application derivation contract version.
pub const APPLICATION_DERIVATION_VERSION: u16 = 1;
/// First Bark application key epoch.
pub const INITIAL_APPLICATION_KEY_EPOCH: u64 = 1;
/// Current Bark VTXO locator derivation version.
pub const LOCATOR_DERIVATION_VERSION: u16 = 1;
/// Commitment domain for the accepted Bark authorization policy.
pub const AUTHORIZATION_POLICY_DOMAIN: &[u8] = b"cdk/frost/federated-bark-authorization/v1";
const ADDITIVE_DERIVATION_DOMAIN: &[u8] = b"cdk/frost/federated-bark-additive-derivation/v1";
const SIGNING_TARGET_DOMAIN: &[u8] = b"cdk/frost/federated-bark-signing-target/v1";

/// Supported application-key role in a Bark locator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum BarkKeyRole {
    /// Key controlling one Bark VTXO.
    VtxoCustody = 1,
}

/// Versioned consensus locator for one Bark application key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BarkKeyLocator {
    role: BarkKeyRole,
    index: u32,
    version: u16,
}

impl BarkKeyLocator {
    /// Construct a v1 VTXO custody locator.
    #[must_use]
    pub const fn vtxo(index: u32) -> Self {
        Self {
            role: BarkKeyRole::VtxoCustody,
            index,
            version: LOCATOR_DERIVATION_VERSION,
        }
    }

    /// Construct and validate an explicit locator.
    pub fn new(role: BarkKeyRole, index: u32, version: u16) -> Result<Self, Error> {
        if version != LOCATOR_DERIVATION_VERSION {
            return Err(Error::UnsupportedLocatorVersion { version });
        }
        Ok(Self {
            role,
            index,
            version,
        })
    }

    /// Application-key role.
    #[must_use]
    pub const fn role(self) -> BarkKeyRole {
        self.role
    }

    /// Consensus-allocated locator index.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Locator derivation version.
    #[must_use]
    pub const fn version(self) -> u16 {
        self.version
    }
}

/// Public result and exact no-secret signing derivation for one locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedBarkKey {
    locator: BarkKeyLocator,
    public_key: PublicKey,
    derivation: SigningDerivation,
    public: DerivedSigningPublicPackage,
}

impl DerivedBarkKey {
    /// Locator used for this key.
    #[must_use]
    pub const fn locator(&self) -> BarkKeyLocator {
        self.locator
    }

    /// Compressed public key expected by Bark's custody interface.
    #[must_use]
    pub const fn public_key(&self) -> PublicKey {
        self.public_key
    }

    /// Exact derivation that an authorized participant must use.
    #[must_use]
    pub fn signing_derivation(&self) -> &SigningDerivation {
        &self.derivation
    }

    /// Complete public FROST signing target.
    #[must_use]
    pub fn public_package(&self) -> &DerivedSigningPublicPackage {
        &self.public
    }
}

/// Public-only Bark locator adapter for one derived FROST application package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrostBark {
    application: DerivedPublicPackage,
}

impl FrostBark {
    /// Construct from the exact registered Bark application package.
    pub fn new(application: DerivedPublicPackage) -> Result<Self, Error> {
        if application.registration()
            != application_registration_for_epoch(application.registration().key_epoch().get())?
        {
            return Err(Error::UnregisteredApplication);
        }
        Ok(Self { application })
    }

    /// Return the immutable public application package.
    #[must_use]
    pub const fn application(&self) -> &DerivedPublicPackage {
        &self.application
    }

    /// Derive the public Bark key and exact plain-signature FROST target.
    pub fn derive(&self, locator: BarkKeyLocator) -> Result<DerivedBarkKey, Error> {
        if locator.version() != LOCATOR_DERIVATION_VERSION {
            return Err(Error::UnsupportedLocatorVersion {
                version: locator.version(),
            });
        }
        let transcript = locator_transcript(&self.application, locator);
        let context: [u8; 32] = hash_transcript(SIGNING_TARGET_DOMAIN, &transcript, None);
        let tweak = derive_non_zero_tweak(&transcript)?;
        let derivation = SigningDerivation::new(context, vec![tweak], SignatureMode::Plain)?;
        let public = derive_signing_public(&self.application, &derivation)?;
        let public_key = PublicKey::from_slice(&public.output_group_key())?;
        Ok(DerivedBarkKey {
            locator,
            public_key,
            derivation,
            public,
        })
    }
}

/// Stable registration for the first Bark application key epoch.
pub fn application_registration() -> Result<ApplicationRegistration, Error> {
    application_registration_for_epoch(INITIAL_APPLICATION_KEY_EPOCH)
}

/// Stable registration for one Bark application key epoch.
pub fn application_registration_for_epoch(
    application_key_epoch: u64,
) -> Result<ApplicationRegistration, Error> {
    let id = ApplicationId::from_label(APPLICATION_LABEL)?;
    let key_epoch = ApplicationKeyEpoch::new(application_key_epoch)?;
    let version = ApplicationDerivationVersion::new(APPLICATION_DERIVATION_VERSION)?;
    let policy: [u8; 32] = Sha256::digest(AUTHORIZATION_POLICY_DOMAIN).into();
    Ok(ApplicationRegistration::new(
        id, key_epoch, version, policy,
    )?)
}

fn locator_transcript(application: &DerivedPublicPackage, locator: BarkKeyLocator) -> Vec<u8> {
    let registration = application.registration();
    let mut transcript = Vec::with_capacity(120);
    transcript.extend_from_slice(&application.root_epoch().get().to_be_bytes());
    transcript.extend_from_slice(&registration.id().to_bytes());
    transcript.extend_from_slice(&registration.key_epoch().get().to_be_bytes());
    transcript.extend_from_slice(&registration.derivation_version().get().to_be_bytes());
    transcript.extend_from_slice(&registration.authorization_policy());
    transcript.extend_from_slice(&application.group_key());
    transcript.push(locator.role() as u8);
    transcript.extend_from_slice(&locator.index().to_be_bytes());
    transcript.extend_from_slice(&locator.version().to_be_bytes());
    transcript
}

fn derive_non_zero_tweak(transcript: &[u8]) -> Result<[u8; 32], Error> {
    for counter in 0..=u32::MAX {
        let digest = hash_transcript(ADDITIVE_DERIVATION_DOMAIN, transcript, Some(counter));
        let scalar = Scalar::reduce(U256::from_be_slice(&digest));
        if !bool::from(scalar.is_zero()) {
            return Ok(scalar.to_bytes().into());
        }
    }
    Err(Error::TweakExhausted)
}

fn hash_transcript(domain: &[u8], transcript: &[u8], counter: Option<u32>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(transcript);
    if let Some(counter) = counter {
        hasher.update(counter.to_be_bytes());
    }
    hasher.finalize().into()
}

/// Bark application derivation error.
#[derive(Debug, Error)]
pub enum Error {
    /// The generic FROST boundary rejected registration or derivation.
    #[error("FROST derivation failed: {0}")]
    Frost(#[from] FrostError),
    /// The derived compressed public key was malformed.
    #[error("derived Bark public key is malformed: {0}")]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// The application package does not carry the frozen Bark registration.
    #[error("FROST package is not registered as the CDK Bark application")]
    UnregisteredApplication,
    /// The locator version is not accepted by this implementation.
    #[error("unsupported Bark locator derivation version {version}")]
    UnsupportedLocatorVersion {
        /// Rejected version.
        version: u16,
    },
    /// All scalar counters were exhausted without a non-zero result.
    #[error("Bark locator derivation exhausted every scalar retry")]
    TweakExhausted,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cdk_frost::{DerivedPublicPackage, MemberId, RootKeyEpoch};

    use super::*;

    fn application() -> DerivedPublicPackage {
        let registration = application_registration().expect("registration");
        let shares = [
            (
                MemberId::new(1).expect("member"),
                hex_point("024d4b6cd1361032ca9bd2aeb9d900aa4d45d9ead80ac9423374c451a7254d0766"),
            ),
            (
                MemberId::new(2).expect("member"),
                hex_point("02531fe6068134503d2723133227c867ac8fa6c83c537e9a44c3c5bdbdcb1fe337"),
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        DerivedPublicPackage::new(
            RootKeyEpoch::new(1).expect("epoch"),
            registration,
            2,
            shares,
            hex_point("026a04ab98d9e4774ad806e302dddeb63bea16b5cb5f223ee77478e861bb583eb3"),
        )
        .expect("application")
    }

    fn hex_point(value: &str) -> [u8; 33] {
        let bytes = value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII"), 16).expect("hex")
            })
            .collect::<Vec<_>>();
        bytes.try_into().expect("point")
    }

    #[test]
    fn registration_matches_frozen_values() {
        let registration = application_registration().expect("registration");
        assert_eq!(
            registration.id().to_bytes(),
            [
                0x59, 0x2b, 0x03, 0x60, 0xe9, 0x08, 0x67, 0xb3, 0x1b, 0xa5, 0x8f, 0x02, 0x77, 0xe1,
                0xd7, 0x84, 0x85, 0xee, 0x2c, 0xa5, 0xfa, 0x20, 0x94, 0x55, 0x12, 0xf4, 0xba, 0x01,
                0xf4, 0xb6, 0x15, 0x86,
            ]
        );
        assert_eq!(
            registration.authorization_policy(),
            [
                0x8a, 0x01, 0x2e, 0x3c, 0x91, 0xd8, 0xdb, 0xb6, 0x25, 0xc2, 0x10, 0xa0, 0xc2, 0x54,
                0xe7, 0xb9, 0x30, 0x7f, 0x76, 0x62, 0x9b, 0xbd, 0x83, 0x25, 0x05, 0x7b, 0xcb, 0xf0,
                0x5b, 0x06, 0xfc, 0xc6,
            ]
        );
    }

    #[test]
    fn locator_derivation_is_plain_stable_and_versioned() {
        let adapter = FrostBark::new(application()).expect("adapter");
        let first = adapter.derive(BarkKeyLocator::vtxo(0)).expect("first key");
        let second = adapter.derive(BarkKeyLocator::vtxo(1)).expect("second key");
        assert_ne!(first.public_key(), second.public_key());
        assert_eq!(first.signing_derivation().tweaks().len(), 1);
        assert_eq!(
            first.signing_derivation().signature_mode(),
            SignatureMode::Plain
        );
        assert!(BarkKeyLocator::new(BarkKeyRole::VtxoCustody, 0, 2).is_err());
    }
}
