//! Canonical untweaked root package and library conversion boundary.

use std::collections::BTreeMap;
use std::fmt;

use frost_secp256k1_tr as frost;
use frost_secp256k1_tr::keys::Tweak;
use frost_secp256k1_tr::rand_core::{CryptoRng, RngCore};
use k256::elliptic_curve::bigint::U256;
use k256::elliptic_curve::ops::Reduce;
use k256::elliptic_curve::sec1::{FromEncodedPoint, ToEncodedPoint};
use k256::elliptic_curve::Group;
use k256::elliptic_curve::PrimeField;
use k256::{AffinePoint, EncodedPoint, ProjectivePoint, Scalar};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::sealed;
use crate::types::{
    ApplicationRegistration, DerivedPublicPackage, DerivedSigningPublicPackage, FrostCiphersuite,
    MemberId, RootKeyEpoch, RootPublicPackage, SignatureMode, SigningDerivation, MAX_PARTICIPANTS,
};
use crate::{EncryptedSecret, Error, SecretEncryptionKey};

pub(crate) const ROOT_DKG_PROTOCOL_VERSION: u16 = 1;

/// Opaque participant-local canonical root package.
///
/// Its debug representation redacts the signing share and it exposes no
/// plaintext share serialization or reconstruction operation.
pub struct CanonicalRootKeyPackage {
    participant: MemberId,
    signing_share: Zeroizing<[u8; 32]>,
    public: RootPublicPackage,
}

impl fmt::Debug for CanonicalRootKeyPackage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalRootKeyPackage")
            .field("participant", &self.participant)
            .field("signing_share", &"<redacted>")
            .field("public", &self.public)
            .finish()
    }
}

pub(crate) struct DerivedLibraryPackage {
    pub(crate) key: frost::keys::KeyPackage,
    pub(crate) public_boundary: DerivedPublicPackage,
}

pub(crate) struct DerivedSigningLibraryPackage {
    pub(crate) key: frost::keys::KeyPackage,
}

impl CanonicalRootKeyPackage {
    pub(crate) fn from_library(
        participant: MemberId,
        roster: &BTreeMap<MemberId, frost::Identifier>,
        key_package: &frost::keys::KeyPackage,
        public_key_package: &frost::keys::PublicKeyPackage,
        epoch: RootKeyEpoch,
        transcript_binding: [u8; 32],
    ) -> Result<Self, Error> {
        if roster.len() > MAX_PARTICIPANTS {
            return Err(Error::OversizedInput {
                kind: "root roster",
                max: MAX_PARTICIPANTS,
            });
        }
        let expected_identifier =
            roster
                .get(&participant)
                .ok_or_else(|| Error::InvalidParticipant {
                    participant: participant.get(),
                    reason: "local participant missing from roster".to_string(),
                })?;
        if key_package.identifier() != expected_identifier {
            return Err(Error::InvalidParticipant {
                participant: participant.get(),
                reason: "library key package belongs to another participant".to_string(),
            });
        }
        let threshold = *key_package.min_signers();
        if public_key_package.min_signers() != Some(threshold) {
            return Err(Error::InsufficientThreshold {
                required: threshold,
                provided: public_key_package.min_signers().unwrap_or_default() as usize,
            });
        }
        if roster.len() != public_key_package.verifying_shares().len() {
            return Err(Error::InvalidParticipant {
                participant: participant.get(),
                reason: "roster and verification-share sets differ".to_string(),
            });
        }

        let mut verifying_shares = BTreeMap::new();
        for (member, identifier) in roster {
            let share = public_key_package
                .verifying_shares()
                .get(identifier)
                .ok_or_else(|| Error::InvalidParticipant {
                    participant: member.get(),
                    reason: "verification share missing".to_string(),
                })?;
            verifying_shares.insert(
                *member,
                to_fixed_33(share.serialize()?, "verification share")?,
            );
        }
        let group_key = to_fixed_33(public_key_package.verifying_key().serialize()?, "group key")?;
        let share = to_fixed_32(key_package.signing_share().serialize(), "signing share")?;
        let public = RootPublicPackage {
            epoch,
            dkg_protocol_version: ROOT_DKG_PROTOCOL_VERSION,
            ciphersuite: FrostCiphersuite::Secp256k1Sha256TrV1,
            threshold,
            verifying_shares,
            group_key,
            transcript_binding,
        };
        validate_root_public(&public)?;
        validate_local_share(participant, &share, &public)?;

        Ok(Self {
            participant,
            signing_share: Zeroizing::new(share),
            public,
        })
    }

    /// Local participant that owns this package.
    pub fn participant(&self) -> MemberId {
        self.participant
    }

    /// Application-neutral untweaked public root package.
    pub fn public(&self) -> &RootPublicPackage {
        &self.public
    }

    /// Seal the local canonical root share without exposing plaintext bytes.
    pub fn seal<R>(&self, key: &SecretEncryptionKey, rng: &mut R) -> Result<EncryptedSecret, Error>
    where
        R: CryptoRng + RngCore,
    {
        sealed::seal(
            key,
            &root_secret_aad(self.participant, &self.public),
            self.signing_share.as_ref(),
            rng,
        )
    }

    /// Open a participant-bound share and validate it against the public root.
    pub fn open(
        participant: MemberId,
        public: RootPublicPackage,
        key: &SecretEncryptionKey,
        encrypted: &EncryptedSecret,
    ) -> Result<Self, Error> {
        validate_root_public(&public)?;
        let plaintext = sealed::open(key, &root_secret_aad(participant, &public), encrypted)?;
        let signing_share: [u8; 32] = plaintext
            .as_slice()
            .try_into()
            .map_err(|_| Error::malformed("sealed FROST root share", "expected 32 bytes"))?;
        validate_local_share(participant, &signing_share, &public)?;
        Ok(Self {
            participant,
            signing_share: Zeroizing::new(signing_share),
            public,
        })
    }

    pub(crate) fn derive_application(
        &self,
        registration: ApplicationRegistration,
    ) -> Result<DerivedLibraryPackage, Error> {
        let tweak = application_tweak(&self.public, registration)?;
        let root_share = scalar_from_bytes(&self.signing_share, "root signing share")?;
        let derived_share = root_share + tweak;
        if bool::from(derived_share.is_zero()) {
            return Err(Error::Internal(
                "application derivation produced a zero signing share".to_string(),
            ));
        }
        let tweak_point = ProjectivePoint::GENERATOR * tweak;
        let group_key = point_from_bytes(&self.public.group_key, "root group key")? + tweak_point;

        let mut boundary_shares = BTreeMap::new();
        let mut library_shares = BTreeMap::new();
        for (member, encoded) in &self.public.verifying_shares {
            let point = point_from_bytes(encoded, "root verification share")? + tweak_point;
            let serialized = point_to_bytes(point);
            boundary_shares.insert(*member, serialized);
            library_shares.insert(
                identifier(*member)?,
                frost::keys::VerifyingShare::deserialize(&serialized)
                    .map_err(|err| Error::Frost(err.to_string()))?,
            );
        }
        let group_key = point_to_bytes(group_key);
        let library_group_key = frost::VerifyingKey::deserialize(&group_key)
            .map_err(|err| Error::Frost(err.to_string()))?;
        let library_signing_share =
            frost::keys::SigningShare::deserialize(derived_share.to_bytes().as_ref())
                .map_err(|err| Error::Frost(err.to_string()))?;
        let own_verifying_share = library_shares
            .get(&identifier(self.participant)?)
            .copied()
            .ok_or_else(|| Error::InvalidParticipant {
                participant: self.participant.get(),
                reason: "derived verification share missing".to_string(),
            })?;
        let key = frost::keys::KeyPackage::new(
            identifier(self.participant)?,
            library_signing_share,
            own_verifying_share,
            library_group_key,
            self.public.threshold,
        );
        let public_boundary = DerivedPublicPackage {
            root_epoch: self.public.epoch,
            registration,
            threshold: self.public.threshold,
            verifying_shares: boundary_shares,
            group_key,
        };
        Ok(DerivedLibraryPackage {
            key,
            public_boundary,
        })
    }

    pub(crate) fn derive_signing_target(
        &self,
        registration: ApplicationRegistration,
        derivation: &SigningDerivation,
    ) -> Result<DerivedSigningLibraryPackage, Error> {
        let application = self.derive_application(registration)?;
        let mut share = scalar_from_bytes(
            &application
                .key
                .signing_share()
                .serialize()
                .try_into()
                .map_err(|_| Error::Internal("FROST signing share is not 32 bytes".to_string()))?,
            "application signing share",
        )?;
        for encoded in derivation.tweaks() {
            share += scalar_from_bytes(encoded, "signing derivation tweak")?;
        }
        if bool::from(share.is_zero()) {
            return Err(Error::Application(
                "signing derivation produced a zero signing share".to_string(),
            ));
        }
        let public_boundary = derive_signing_public(&application.public_boundary, derivation)?;
        let public = library_signing_public_package(&public_boundary)?;
        let signing_share = frost::keys::SigningShare::deserialize(share.to_bytes().as_ref())
            .map_err(|err| Error::Frost(err.to_string()))?;
        let own_verifying_share = public
            .verifying_shares()
            .get(&identifier(self.participant)?)
            .copied()
            .ok_or_else(|| Error::InvalidParticipant {
                participant: self.participant.get(),
                reason: "derived verification share missing".to_string(),
            })?;
        let key = frost::keys::KeyPackage::new(
            identifier(self.participant)?,
            signing_share,
            own_verifying_share,
            *public.verifying_key(),
            self.public.threshold,
        );
        Ok(DerivedSigningLibraryPackage { key })
    }
}

fn root_secret_aad(participant: MemberId, public: &RootPublicPackage) -> Vec<u8> {
    let mut aad = b"cdk/frost/sealed-root-share/v1".to_vec();
    aad.extend_from_slice(&participant.get().to_be_bytes());
    aad.extend_from_slice(&public.encode());
    aad
}

pub(crate) fn derive_signing_public(
    application: &DerivedPublicPackage,
    derivation: &SigningDerivation,
) -> Result<DerivedSigningPublicPackage, Error> {
    let total_tweak = derivation
        .tweaks()
        .iter()
        .try_fold(Scalar::ZERO, |sum, encoded| {
            Ok::<_, Error>(sum + scalar_from_bytes(encoded, "signing derivation tweak")?)
        })?;
    let tweak_point = ProjectivePoint::GENERATOR * total_tweak;
    let mut shares = BTreeMap::new();
    for (member, encoded) in &application.verifying_shares {
        let point = point_from_bytes(encoded, "application verification share")? + tweak_point;
        if bool::from(point.is_identity()) {
            return Err(Error::Application(
                "signing derivation produced an identity verification share".to_string(),
            ));
        }
        shares.insert(*member, point_to_bytes(point));
    }
    let group = point_from_bytes(&application.group_key, "application group key")? + tweak_point;
    if bool::from(group.is_identity()) {
        return Err(Error::Application(
            "signing derivation produced an identity group key".to_string(),
        ));
    }
    let internal_group_key = point_to_bytes(group);
    let internal = DerivedPublicPackage {
        root_epoch: application.root_epoch,
        registration: application.registration,
        threshold: application.threshold,
        verifying_shares: shares.clone(),
        group_key: internal_group_key,
    };
    let library = library_public_package(&internal)?;
    let output = match derivation.signature_mode() {
        SignatureMode::Plain => library,
        SignatureMode::TaprootKeyPath { merkle_root } => {
            library.tweak(merkle_root.as_ref().map(|root| root.as_slice()))
        }
    };
    let output_group_key = output
        .verifying_key()
        .serialize()
        .map_err(|err| Error::Frost(err.to_string()))?
        .try_into()
        .map_err(|_| Error::Internal("FROST group key is not 33 bytes".to_string()))?;
    Ok(DerivedSigningPublicPackage {
        application: application.clone(),
        derivation_binding: derivation.binding(),
        internal_verifying_shares: shares,
        internal_group_key,
        output_group_key,
        signature_mode: derivation.signature_mode(),
    })
}

pub(crate) fn library_signing_public_package(
    package: &DerivedSigningPublicPackage,
) -> Result<frost::keys::PublicKeyPackage, Error> {
    let internal = DerivedPublicPackage {
        root_epoch: package.application.root_epoch,
        registration: package.application.registration,
        threshold: package.application.threshold,
        verifying_shares: package.internal_verifying_shares.clone(),
        group_key: package.internal_group_key,
    };
    library_public_package(&internal)
}

pub(crate) fn library_public_package(
    package: &DerivedPublicPackage,
) -> Result<frost::keys::PublicKeyPackage, Error> {
    validate_derived_public(package)?;
    let mut shares = BTreeMap::new();
    for (member, encoded) in &package.verifying_shares {
        shares.insert(
            identifier(*member)?,
            frost::keys::VerifyingShare::deserialize(encoded)
                .map_err(|err| Error::Frost(err.to_string()))?,
        );
    }
    let group_key = frost::VerifyingKey::deserialize(&package.group_key)
        .map_err(|err| Error::Frost(err.to_string()))?;
    Ok(frost::keys::PublicKeyPackage::new(
        shares,
        group_key,
        Some(package.threshold),
    ))
}

pub(crate) fn validate_derived_public(package: &DerivedPublicPackage) -> Result<(), Error> {
    let count = package.verifying_shares.len();
    if !(2..=MAX_PARTICIPANTS).contains(&count) {
        return Err(Error::malformed(
            "derived public package",
            "participant count is outside protocol bounds",
        ));
    }
    if package.threshold < 2 || usize::from(package.threshold) > count {
        return Err(Error::InsufficientThreshold {
            required: package.threshold,
            provided: count,
        });
    }
    point_from_bytes(&package.group_key, "application group key")?;
    for encoded in package.verifying_shares.values() {
        point_from_bytes(encoded, "application verification share")?;
    }
    Ok(())
}

pub(crate) fn identifier(member: MemberId) -> Result<frost::Identifier, Error> {
    frost::Identifier::try_from(member.get()).map_err(|err| Error::InvalidParticipant {
        participant: member.get(),
        reason: err.to_string(),
    })
}

pub(crate) fn validate_root_public(public: &RootPublicPackage) -> Result<(), Error> {
    if public.dkg_protocol_version != ROOT_DKG_PROTOCOL_VERSION {
        return Err(Error::malformed(
            "root public package",
            "unsupported DKG protocol version",
        ));
    }
    if public.ciphersuite != FrostCiphersuite::Secp256k1Sha256TrV1 {
        return Err(Error::malformed(
            "root public package",
            "unsupported ciphersuite",
        ));
    }
    let count = public.verifying_shares.len();
    if !(2..=MAX_PARTICIPANTS).contains(&count) {
        return Err(Error::malformed(
            "root public package",
            "participant count is outside protocol bounds",
        ));
    }
    if public.threshold < 2 || usize::from(public.threshold) > count {
        return Err(Error::InsufficientThreshold {
            required: public.threshold,
            provided: count,
        });
    }
    if public.transcript_binding == [0; 32] {
        return Err(Error::malformed(
            "root public package",
            "transcript binding cannot be all zeroes",
        ));
    }
    point_from_bytes(&public.group_key, "root group key")?;
    for encoded in public.verifying_shares.values() {
        point_from_bytes(encoded, "root verification share")?;
    }
    Ok(())
}

fn validate_local_share(
    participant: MemberId,
    share: &[u8; 32],
    public: &RootPublicPackage,
) -> Result<(), Error> {
    let scalar = scalar_from_bytes(share, "root signing share")?;
    let expected = point_to_bytes(ProjectivePoint::GENERATOR * scalar);
    let actual =
        public
            .verifying_shares
            .get(&participant)
            .ok_or_else(|| Error::InvalidParticipant {
                participant: participant.get(),
                reason: "local verification share missing".to_string(),
            })?;
    if &expected != actual {
        return Err(Error::InvalidParticipant {
            participant: participant.get(),
            reason: "signing share does not match verification share".to_string(),
        });
    }
    Ok(())
}

fn application_tweak(
    public: &RootPublicPackage,
    registration: ApplicationRegistration,
) -> Result<Scalar, Error> {
    let mut hasher = Sha256::new();
    hasher.update(b"cdk/frost/application-derivation/v1");
    hasher.update(public.epoch.get().to_be_bytes());
    hasher.update(public.transcript_binding);
    hasher.update(public.group_key);
    hasher.update(registration.id().to_bytes());
    hasher.update(registration.derivation_version().get().to_be_bytes());
    hasher.update(registration.authorization_policy());
    let tweak = Scalar::reduce(U256::from_be_slice(&hasher.finalize()));
    if bool::from(tweak.is_zero()) {
        return Err(Error::Internal(
            "application derivation produced a zero tweak".to_string(),
        ));
    }
    Ok(tweak)
}

fn scalar_from_bytes(bytes: &[u8; 32], kind: &'static str) -> Result<Scalar, Error> {
    let scalar = Option::<Scalar>::from(Scalar::from_repr((*bytes).into()))
        .ok_or_else(|| Error::malformed(kind, "invalid secp256k1 scalar"))?;
    if bool::from(scalar.is_zero()) {
        return Err(Error::malformed(kind, "scalar cannot be zero"));
    }
    Ok(scalar)
}

fn point_from_bytes(bytes: &[u8; 33], kind: &'static str) -> Result<ProjectivePoint, Error> {
    let encoded =
        EncodedPoint::from_bytes(bytes).map_err(|err| Error::malformed(kind, err.to_string()))?;
    let affine = Option::<AffinePoint>::from(AffinePoint::from_encoded_point(&encoded))
        .ok_or_else(|| Error::malformed(kind, "invalid secp256k1 point"))?;
    Ok(ProjectivePoint::from(affine))
}

fn point_to_bytes(point: ProjectivePoint) -> [u8; 33] {
    point
        .to_affine()
        .to_encoded_point(true)
        .as_bytes()
        .try_into()
        .expect("compressed secp256k1 points are always 33 bytes")
}

fn to_fixed_32(bytes: Vec<u8>, kind: &'static str) -> Result<[u8; 32], Error> {
    bytes
        .try_into()
        .map_err(|_| Error::malformed(kind, "expected 32 bytes"))
}

fn to_fixed_33(bytes: Vec<u8>, kind: &'static str) -> Result<[u8; 33], Error> {
    bytes
        .try_into()
        .map_err(|_| Error::malformed(kind, "expected 33 bytes"))
}

impl From<frost::Error> for Error {
    fn from(error: frost::Error) -> Self {
        Self::Frost(error.to_string())
    }
}
