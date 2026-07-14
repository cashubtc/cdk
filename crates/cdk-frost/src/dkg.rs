//! Dealer-free, application-neutral FROST root-key ceremony.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use frost_secp256k1_tr as frost;
use frost_secp256k1_tr::rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::canonical::CanonicalRootKeyPackage;
use crate::sealed;
use crate::types::MAX_PARTICIPANTS;
use crate::{EncryptedSecret, Error, MemberId, RootKeyEpoch, SecretEncryptionKey};

const DKG_PACKAGE_MAX: usize = 64 * 1024;
const ROUND1_MAGIC: &[u8; 8] = b"CDKFD101";
const ROUND2_MAGIC: &[u8; 8] = b"CDKFD201";

type DecodedDkgPackage = ([u8; 32], MemberId, Option<MemberId>, Vec<u8>);

/// Immutable domain and roster for one FROST root-key epoch ceremony.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DkgContext {
    federation_id: [u8; 32],
    ceremony_id: [u8; 32],
    root_epoch: RootKeyEpoch,
    threshold: u16,
    participants: BTreeSet<MemberId>,
    binding: [u8; 32],
}

impl DkgContext {
    /// Construct a bounded ceremony context using the federation's signing threshold.
    pub fn new(
        federation_id: [u8; 32],
        ceremony_id: [u8; 32],
        root_epoch: RootKeyEpoch,
        threshold: u16,
        participants: BTreeSet<MemberId>,
    ) -> Result<Self, Error> {
        if federation_id == [0; 32] || ceremony_id == [0; 32] {
            return Err(Error::malformed(
                "FROST DKG context",
                "federation and ceremony identifiers must be non-zero",
            ));
        }
        if !(2..=MAX_PARTICIPANTS).contains(&participants.len()) {
            return Err(Error::malformed(
                "FROST DKG context",
                "participant count is outside protocol bounds",
            ));
        }
        if threshold < 2 || usize::from(threshold) > participants.len() {
            return Err(Error::InsufficientThreshold {
                required: threshold,
                provided: participants.len(),
            });
        }
        let mut hasher = Sha256::new();
        hasher.update(b"cdk/frost/root-dkg-context/v1");
        hasher.update(federation_id);
        hasher.update(ceremony_id);
        hasher.update(root_epoch.get().to_be_bytes());
        hasher.update(threshold.to_be_bytes());
        hasher.update((participants.len() as u16).to_be_bytes());
        for participant in &participants {
            hasher.update(participant.get().to_be_bytes());
        }
        let binding = hasher.finalize().into();
        Ok(Self {
            federation_id,
            ceremony_id,
            root_epoch,
            threshold,
            participants,
            binding,
        })
    }

    /// Federation identifier committed by every DKG message.
    pub const fn federation_id(&self) -> [u8; 32] {
        self.federation_id
    }

    /// Ceremony identifier committed by every DKG message.
    pub const fn ceremony_id(&self) -> [u8; 32] {
        self.ceremony_id
    }

    /// Root-key epoch produced by this ceremony.
    pub const fn root_epoch(&self) -> RootKeyEpoch {
        self.root_epoch
    }

    /// FROST signing threshold, which must equal the federation threshold.
    pub const fn threshold(&self) -> u16 {
        self.threshold
    }

    /// Canonically ordered participant roster.
    pub fn participants(&self) -> &BTreeSet<MemberId> {
        &self.participants
    }

    /// Stable binding included in every public round message.
    pub const fn binding(&self) -> [u8; 32] {
        self.binding
    }

    fn ensure_participant(&self, participant: MemberId) -> Result<(), Error> {
        if !self.participants.contains(&participant) {
            return Err(Error::InvalidParticipant {
                participant: participant.get(),
                reason: "participant is absent from the DKG roster".to_string(),
            });
        }
        Ok(())
    }
}

/// Participant-local round-one secret state. Debug output is redacted.
pub struct DkgRound1Secret {
    context: DkgContext,
    participant: MemberId,
    package: Zeroizing<Vec<u8>>,
    own_public_package: Vec<u8>,
}

impl fmt::Debug for DkgRound1Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DkgRound1Secret")
            .field("context_binding", &self.context.binding)
            .field("participant", &self.participant)
            .field("package", &"<redacted>")
            .finish()
    }
}

impl DkgRound1Secret {
    /// Recreate this participant's public round-one broadcast after restart.
    pub fn public_package(&self) -> DkgRound1Package {
        DkgRound1Package {
            context_binding: self.context.binding,
            sender: self.participant,
            package: self.own_public_package.clone(),
        }
    }

    /// Seal restartable round-one state without exposing a plaintext encoding.
    pub fn seal<R>(&self, key: &SecretEncryptionKey, rng: &mut R) -> Result<EncryptedSecret, Error>
    where
        R: CryptoRng + RngCore,
    {
        sealed::seal(
            key,
            &secret_aad(b"round1", &self.context, self.participant),
            &encode_secret_parts(&self.package, &self.own_public_package),
            rng,
        )
    }

    /// Open and validate participant-bound round-one state.
    pub fn open(
        context: DkgContext,
        participant: MemberId,
        key: &SecretEncryptionKey,
        encrypted: &EncryptedSecret,
    ) -> Result<Self, Error> {
        context.ensure_participant(participant)?;
        let plaintext = sealed::open(
            key,
            &secret_aad(b"round1", &context, participant),
            encrypted,
        )?;
        let (package, own_public_package) = decode_secret_parts(&plaintext)?;
        frost::keys::dkg::round1::SecretPackage::deserialize(&package)?;
        frost::keys::dkg::round1::Package::deserialize(&own_public_package)?;
        Ok(Self {
            context,
            participant,
            package: Zeroizing::new(package),
            own_public_package,
        })
    }
}

/// Participant-local round-two secret state. Debug output is redacted.
pub struct DkgRound2Secret {
    context: DkgContext,
    participant: MemberId,
    package: Zeroizing<Vec<u8>>,
    own_round1_package: Vec<u8>,
}

impl fmt::Debug for DkgRound2Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DkgRound2Secret")
            .field("context_binding", &self.context.binding)
            .field("participant", &self.participant)
            .field("package", &"<redacted>")
            .finish()
    }
}

impl DkgRound2Secret {
    /// Seal restartable round-two state without exposing a plaintext encoding.
    pub fn seal<R>(&self, key: &SecretEncryptionKey, rng: &mut R) -> Result<EncryptedSecret, Error>
    where
        R: CryptoRng + RngCore,
    {
        sealed::seal(
            key,
            &secret_aad(b"round2", &self.context, self.participant),
            &encode_secret_parts(&self.package, &self.own_round1_package),
            rng,
        )
    }

    /// Open and validate participant-bound round-two state.
    pub fn open(
        context: DkgContext,
        participant: MemberId,
        key: &SecretEncryptionKey,
        encrypted: &EncryptedSecret,
    ) -> Result<Self, Error> {
        context.ensure_participant(participant)?;
        let plaintext = sealed::open(
            key,
            &secret_aad(b"round2", &context, participant),
            encrypted,
        )?;
        let (package, own_round1_package) = decode_secret_parts(&plaintext)?;
        frost::keys::dkg::round2::SecretPackage::deserialize(&package)?;
        frost::keys::dkg::round1::Package::deserialize(&own_round1_package)?;
        Ok(Self {
            context,
            participant,
            package: Zeroizing::new(package),
            own_round1_package,
        })
    }
}

/// Authenticated broadcast package for FROST DKG round one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DkgRound1Package {
    context_binding: [u8; 32],
    sender: MemberId,
    package: Vec<u8>,
}

/// Confidential authenticated per-recipient package for FROST DKG round two.
#[derive(Clone, PartialEq, Eq)]
pub struct DkgRound2Package {
    context_binding: [u8; 32],
    sender: MemberId,
    receiver: MemberId,
    package: Vec<u8>,
}

impl fmt::Debug for DkgRound2Package {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DkgRound2Package")
            .field("context_binding", &self.context_binding)
            .field("sender", &self.sender)
            .field("receiver", &self.receiver)
            .field("package", &"<redacted>")
            .finish()
    }
}

impl DkgRound1Package {
    /// Sender authenticated by the federation transport.
    pub const fn sender(&self) -> MemberId {
        self.sender
    }

    /// Canonical bounded wire representation.
    pub fn encode(&self) -> Vec<u8> {
        encode_package(
            ROUND1_MAGIC,
            self.context_binding,
            self.sender,
            None,
            &self.package,
        )
    }

    /// Decode and bind a round-one package to the expected ceremony.
    pub fn decode(context: &DkgContext, bytes: &[u8]) -> Result<Self, Error> {
        let (binding, sender, receiver, package) = decode_package(ROUND1_MAGIC, bytes)?;
        if receiver.is_some() || binding != context.binding {
            return Err(Error::WrongSession);
        }
        context.ensure_participant(sender)?;
        frost::keys::dkg::round1::Package::deserialize(&package)
            .map_err(|error| Error::Frost(error.to_string()))?;
        Ok(Self {
            context_binding: binding,
            sender,
            package,
        })
    }
}

impl DkgRound2Package {
    /// Sending participant.
    pub const fn sender(&self) -> MemberId {
        self.sender
    }

    /// Intended recipient.
    pub const fn receiver(&self) -> MemberId {
        self.receiver
    }

    /// Canonical bounded wire representation. This contains secret contribution material.
    pub fn encode(&self) -> Vec<u8> {
        encode_package(
            ROUND2_MAGIC,
            self.context_binding,
            self.sender,
            Some(self.receiver),
            &self.package,
        )
    }

    /// Seal this confidential contribution for participant-local durable retry.
    pub fn seal<R>(&self, key: &SecretEncryptionKey, rng: &mut R) -> Result<EncryptedSecret, Error>
    where
        R: CryptoRng + RngCore,
    {
        sealed::seal(
            key,
            &round2_package_aad(self.context_binding, self.sender, self.receiver),
            &self.encode(),
            rng,
        )
    }

    /// Open a contribution bound to the expected ceremony, sender, and receiver.
    pub fn open(
        context: &DkgContext,
        expected_sender: MemberId,
        expected_receiver: MemberId,
        key: &SecretEncryptionKey,
        encrypted: &EncryptedSecret,
    ) -> Result<Self, Error> {
        let plaintext = sealed::open(
            key,
            &round2_package_aad(context.binding, expected_sender, expected_receiver),
            encrypted,
        )?;
        let package = Self::decode(context, expected_receiver, &plaintext)?;
        if package.sender != expected_sender {
            return Err(Error::WrongSession);
        }
        Ok(package)
    }

    /// Decode a round-two package for one expected recipient and ceremony.
    pub fn decode(
        context: &DkgContext,
        expected_receiver: MemberId,
        bytes: &[u8],
    ) -> Result<Self, Error> {
        let (binding, sender, receiver, package) = decode_package(ROUND2_MAGIC, bytes)?;
        let receiver = receiver.ok_or_else(|| {
            Error::malformed("FROST DKG round two", "missing receiver identifier")
        })?;
        if binding != context.binding || receiver != expected_receiver {
            return Err(Error::WrongSession);
        }
        context.ensure_participant(sender)?;
        context.ensure_participant(receiver)?;
        frost::keys::dkg::round2::Package::deserialize(&package)
            .map_err(|error| Error::Frost(error.to_string()))?;
        Ok(Self {
            context_binding: binding,
            sender,
            receiver,
            package,
        })
    }
}

/// Start FROST DKG and return durable local secret state plus a public broadcast.
pub fn start_dkg<R>(
    context: DkgContext,
    participant: MemberId,
    rng: R,
) -> Result<(DkgRound1Secret, DkgRound1Package), Error>
where
    R: CryptoRng + RngCore,
{
    context.ensure_participant(participant)?;
    let (secret, package) = frost::keys::dkg::part1(
        frost_identifier(participant)?,
        context.participants.len() as u16,
        context.threshold,
        rng,
    )?;
    let secret = secret.serialize()?;
    let package = package.serialize()?;
    ensure_package_bound(&secret)?;
    ensure_package_bound(&package)?;
    Ok((
        DkgRound1Secret {
            context: context.clone(),
            participant,
            package: Zeroizing::new(secret),
            own_public_package: package.clone(),
        },
        DkgRound1Package {
            context_binding: context.binding,
            sender: participant,
            package,
        },
    ))
}

/// Verify every other round-one broadcast and produce per-recipient round-two packages.
pub fn advance_dkg(
    secret: DkgRound1Secret,
    packages: impl IntoIterator<Item = DkgRound1Package>,
) -> Result<(DkgRound2Secret, BTreeMap<MemberId, DkgRound2Package>), Error> {
    let received = collect_round1(&secret.context, secret.participant, packages)?;
    let library = received
        .iter()
        .map(|(member, package)| {
            Ok((
                frost_identifier(*member)?,
                frost::keys::dkg::round1::Package::deserialize(&package.package)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, Error>>()?;
    let secret_package = frost::keys::dkg::round1::SecretPackage::deserialize(&secret.package)?;
    let (round2_secret, outgoing) = frost::keys::dkg::part2(secret_package, &library)?;
    let encoded_secret = round2_secret.serialize()?;
    ensure_package_bound(&encoded_secret)?;
    let mut packages = BTreeMap::new();
    for receiver in secret.context.participants.iter().copied() {
        if receiver == secret.participant {
            continue;
        }
        let package = outgoing
            .get(&frost_identifier(receiver)?)
            .ok_or_else(|| Error::InvalidParticipant {
                participant: receiver.get(),
                reason: "FROST DKG omitted a round-two receiver".to_string(),
            })?
            .serialize()?;
        ensure_package_bound(&package)?;
        packages.insert(
            receiver,
            DkgRound2Package {
                context_binding: secret.context.binding,
                sender: secret.participant,
                receiver,
                package,
            },
        );
    }
    Ok((
        DkgRound2Secret {
            context: secret.context,
            participant: secret.participant,
            package: Zeroizing::new(encoded_secret),
            own_round1_package: secret.own_public_package,
        },
        packages,
    ))
}

/// Finish FROST DKG after independently verifying the full round-one roster and all incoming contributions.
pub fn finish_dkg(
    secret: DkgRound2Secret,
    round1_packages: impl IntoIterator<Item = DkgRound1Package>,
    round2_packages: impl IntoIterator<Item = DkgRound2Package>,
) -> Result<CanonicalRootKeyPackage, Error> {
    let mut round1 = collect_round1(&secret.context, secret.participant, round1_packages)?;
    round1.insert(
        secret.participant,
        DkgRound1Package {
            context_binding: secret.context.binding,
            sender: secret.participant,
            package: secret.own_round1_package.clone(),
        },
    );
    let round2 = collect_round2(&secret.context, secret.participant, round2_packages)?;
    let library_round1 = round1
        .iter()
        .filter(|(member, _)| **member != secret.participant)
        .map(|(member, package)| {
            Ok((
                frost_identifier(*member)?,
                frost::keys::dkg::round1::Package::deserialize(&package.package)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, Error>>()?;
    let library_round2 = round2
        .iter()
        .map(|(member, package)| {
            Ok((
                frost_identifier(*member)?,
                frost::keys::dkg::round2::Package::deserialize(&package.package)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, Error>>()?;
    let round2_secret = frost::keys::dkg::round2::SecretPackage::deserialize(&secret.package)?;
    let (key, public) = frost::keys::dkg::part3(&round2_secret, &library_round1, &library_round2)?;
    let transcript = transcript_binding(&secret.context, &round1, &public)?;
    let roster = secret
        .context
        .participants
        .iter()
        .copied()
        .map(|member| Ok((member, frost_identifier(member)?)))
        .collect::<Result<BTreeMap<_, _>, Error>>()?;
    CanonicalRootKeyPackage::from_library(
        secret.participant,
        &roster,
        &key,
        &public,
        secret.context.root_epoch,
        transcript,
    )
}

fn collect_round1(
    context: &DkgContext,
    local: MemberId,
    packages: impl IntoIterator<Item = DkgRound1Package>,
) -> Result<BTreeMap<MemberId, DkgRound1Package>, Error> {
    let mut received = BTreeMap::new();
    for package in packages {
        if package.context_binding != context.binding || package.sender == local {
            return Err(Error::WrongSession);
        }
        context.ensure_participant(package.sender)?;
        frost::keys::dkg::round1::Package::deserialize(&package.package)?;
        if received.insert(package.sender, package).is_some() {
            return Err(Error::InvalidParticipant {
                participant: local.get(),
                reason: "duplicate FROST DKG round-one sender".to_string(),
            });
        }
    }
    ensure_other_participants(context, local, received.keys().copied())?;
    Ok(received)
}

fn collect_round2(
    context: &DkgContext,
    local: MemberId,
    packages: impl IntoIterator<Item = DkgRound2Package>,
) -> Result<BTreeMap<MemberId, DkgRound2Package>, Error> {
    let mut received = BTreeMap::new();
    for package in packages {
        if package.context_binding != context.binding
            || package.sender == local
            || package.receiver != local
        {
            return Err(Error::WrongSession);
        }
        context.ensure_participant(package.sender)?;
        frost::keys::dkg::round2::Package::deserialize(&package.package)?;
        let sender = package.sender;
        if received.insert(sender, package).is_some() {
            return Err(Error::InvalidParticipant {
                participant: sender.get(),
                reason: "duplicate FROST DKG round-two sender".to_string(),
            });
        }
    }
    ensure_other_participants(context, local, received.keys().copied())?;
    Ok(received)
}

fn ensure_other_participants(
    context: &DkgContext,
    local: MemberId,
    actual: impl IntoIterator<Item = MemberId>,
) -> Result<(), Error> {
    let expected = context
        .participants
        .iter()
        .copied()
        .filter(|participant| *participant != local)
        .collect::<BTreeSet<_>>();
    if actual.into_iter().collect::<BTreeSet<_>>() != expected {
        return Err(Error::InvalidParticipant {
            participant: local.get(),
            reason: "FROST DKG participant set is incomplete or conflicting".to_string(),
        });
    }
    Ok(())
}

fn transcript_binding(
    context: &DkgContext,
    round1: &BTreeMap<MemberId, DkgRound1Package>,
    public: &frost::keys::PublicKeyPackage,
) -> Result<[u8; 32], Error> {
    let mut hasher = Sha256::new();
    hasher.update(b"cdk/frost/root-dkg-transcript/v1");
    hasher.update(context.binding);
    for (member, package) in round1 {
        hasher.update(member.get().to_be_bytes());
        hasher.update((package.package.len() as u32).to_be_bytes());
        hasher.update(&package.package);
    }
    hasher.update(public.verifying_key().serialize()?);
    for (identifier, share) in public.verifying_shares() {
        hasher.update(identifier.serialize());
        hasher.update(share.serialize()?);
    }
    Ok(hasher.finalize().into())
}

fn frost_identifier(member: MemberId) -> Result<frost::Identifier, Error> {
    frost::Identifier::try_from(member.get()).map_err(|error| Error::InvalidParticipant {
        participant: member.get(),
        reason: error.to_string(),
    })
}

fn ensure_package_bound(package: &[u8]) -> Result<(), Error> {
    if package.len() > DKG_PACKAGE_MAX {
        return Err(Error::OversizedInput {
            kind: "FROST DKG package",
            max: DKG_PACKAGE_MAX,
        });
    }
    Ok(())
}

fn secret_aad(kind: &[u8], context: &DkgContext, participant: MemberId) -> Vec<u8> {
    let mut aad = b"cdk/frost/sealed-dkg-state/v1".to_vec();
    aad.extend_from_slice(&(kind.len() as u16).to_be_bytes());
    aad.extend_from_slice(kind);
    aad.extend_from_slice(&context.binding);
    aad.extend_from_slice(&participant.get().to_be_bytes());
    aad
}

fn round2_package_aad(context_binding: [u8; 32], sender: MemberId, receiver: MemberId) -> Vec<u8> {
    let mut aad = b"cdk/frost/sealed-dkg-round2-package/v1".to_vec();
    aad.extend_from_slice(&context_binding);
    aad.extend_from_slice(&sender.get().to_be_bytes());
    aad.extend_from_slice(&receiver.get().to_be_bytes());
    aad
}

fn encode_secret_parts(secret: &[u8], public: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(8 + secret.len() + public.len());
    encoded.extend_from_slice(&(secret.len() as u32).to_be_bytes());
    encoded.extend_from_slice(secret);
    encoded.extend_from_slice(&(public.len() as u32).to_be_bytes());
    encoded.extend_from_slice(public);
    encoded
}

fn decode_secret_parts(encoded: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Error> {
    if encoded.len() < 8 {
        return Err(Error::malformed(
            "sealed FROST DKG state",
            "truncated state",
        ));
    }
    let secret_len = u32::from_be_bytes(
        encoded[..4]
            .try_into()
            .map_err(|_| Error::malformed("sealed FROST DKG state", "invalid secret length"))?,
    ) as usize;
    let public_len_offset = 4usize
        .checked_add(secret_len)
        .ok_or_else(|| Error::malformed("sealed FROST DKG state", "length overflow"))?;
    let public_start = public_len_offset
        .checked_add(4)
        .ok_or_else(|| Error::malformed("sealed FROST DKG state", "length overflow"))?;
    if public_start > encoded.len() {
        return Err(Error::malformed(
            "sealed FROST DKG state",
            "truncated secret",
        ));
    }
    let public_len = u32::from_be_bytes(
        encoded[public_len_offset..public_start]
            .try_into()
            .map_err(|_| Error::malformed("sealed FROST DKG state", "invalid public length"))?,
    ) as usize;
    if public_start.checked_add(public_len) != Some(encoded.len()) {
        return Err(Error::malformed(
            "sealed FROST DKG state",
            "payload length mismatch",
        ));
    }
    ensure_package_bound(&encoded[4..public_len_offset])?;
    ensure_package_bound(&encoded[public_start..])?;
    Ok((
        encoded[4..public_len_offset].to_vec(),
        encoded[public_start..].to_vec(),
    ))
}

fn encode_package(
    magic: &[u8; 8],
    binding: [u8; 32],
    sender: MemberId,
    receiver: Option<MemberId>,
    package: &[u8],
) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(48 + package.len());
    encoded.extend_from_slice(magic);
    encoded.extend_from_slice(&binding);
    encoded.extend_from_slice(&sender.get().to_be_bytes());
    encoded.extend_from_slice(&receiver.map_or(0, MemberId::get).to_be_bytes());
    encoded.extend_from_slice(&(package.len() as u32).to_be_bytes());
    encoded.extend_from_slice(package);
    encoded
}

fn decode_package(magic: &[u8; 8], encoded: &[u8]) -> Result<DecodedDkgPackage, Error> {
    if encoded.len() > DKG_PACKAGE_MAX + 48 {
        return Err(Error::OversizedInput {
            kind: "FROST DKG wire package",
            max: DKG_PACKAGE_MAX + 48,
        });
    }
    if encoded.len() < 48 || &encoded[..8] != magic {
        return Err(Error::malformed(
            "FROST DKG wire package",
            "truncated or wrong version",
        ));
    }
    let binding = encoded[8..40]
        .try_into()
        .map_err(|_| Error::malformed("FROST DKG wire package", "invalid binding"))?;
    let sender =
        MemberId::new(u16::from_be_bytes(encoded[40..42].try_into().map_err(
            |_| Error::malformed("FROST DKG wire package", "invalid sender"),
        )?))?;
    let receiver = u16::from_be_bytes(
        encoded[42..44]
            .try_into()
            .map_err(|_| Error::malformed("FROST DKG wire package", "invalid receiver"))?,
    );
    let length = u32::from_be_bytes(
        encoded[44..48]
            .try_into()
            .map_err(|_| Error::malformed("FROST DKG wire package", "invalid length"))?,
    ) as usize;
    if length > DKG_PACKAGE_MAX || encoded.len() != 48 + length {
        return Err(Error::malformed(
            "FROST DKG wire package",
            "payload length mismatch",
        ));
    }
    let receiver = match receiver {
        0 => None,
        value => Some(MemberId::new(value)?),
    };
    Ok((binding, sender, receiver, encoded[48..].to_vec()))
}

#[cfg(test)]
mod tests {
    use frost_secp256k1_tr::rand_core::SeedableRng;
    use rand_chacha::ChaCha20Rng;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::{
        ApplicationAuthorization, ApplicationDerivationVersion, ApplicationId, ApplicationKeyEpoch,
        ApplicationRegistration, AuthorizationRequest, AuthorizedSigningRequest,
        NonceCommitmentSet, ParticipantKeyManager, SecretEncryptionKey, SigningAuthorizationScope,
        SigningDerivation, SigningSessionContext, SigningSessionId,
    };

    #[derive(Debug, Clone, Copy)]
    struct TestAdapter(ApplicationRegistration);

    impl ApplicationAuthorization for TestAdapter {
        fn registration(&self) -> ApplicationRegistration {
            self.0
        }

        fn authorize(
            &self,
            request: &AuthorizationRequest,
        ) -> Result<AuthorizedSigningRequest, Error> {
            if request.application_id() != self.0.id() || request.payload().len() != 32 {
                return Err(Error::Application("invalid DKG test request".to_string()));
            }
            let message = request
                .payload()
                .try_into()
                .map_err(|_| Error::Application("invalid DKG test message".to_string()))?;
            Ok(AuthorizedSigningRequest::new(
                message,
                SigningDerivation::application_key(),
            ))
        }
    }

    fn context(max_signers: u16, threshold: u16, profile: u8) -> DkgContext {
        DkgContext::new(
            Sha256::digest([b"test-federation".as_slice(), &[profile]].concat()).into(),
            Sha256::digest([b"test-ceremony".as_slice(), &[profile]].concat()).into(),
            RootKeyEpoch::new(u64::from(profile)).expect("epoch"),
            threshold,
            (1..=max_signers)
                .map(|member| MemberId::new(member).expect("member"))
                .collect(),
        )
        .expect("DKG context")
    }

    fn run_dkg(max_signers: u16, threshold: u16, profile: u8) -> Vec<CanonicalRootKeyPackage> {
        let context = context(max_signers, threshold, profile);
        let storage_key =
            SecretEncryptionKey::new([profile.wrapping_add(100); 32]).expect("storage key");
        let mut round1_secrets = BTreeMap::new();
        let mut round1_packages = BTreeMap::new();
        for member in context.participants().iter().copied() {
            let mut seed = [profile; 32];
            seed[0] = member.get() as u8;
            let rng = ChaCha20Rng::from_seed(seed);
            let (secret, package) = start_dkg(context.clone(), member, rng).expect("round one");
            let sealed = secret
                .seal(
                    &storage_key,
                    &mut ChaCha20Rng::from_seed([member.get() as u8; 32]),
                )
                .expect("seal round one");
            let secret = DkgRound1Secret::open(context.clone(), member, &storage_key, &sealed)
                .expect("restart round one");
            assert_eq!(
                DkgRound1Package::decode(&context, &package.encode()).expect("round-one wire"),
                package
            );
            round1_secrets.insert(member, secret);
            round1_packages.insert(member, package);
        }

        let mut round2_secrets = BTreeMap::new();
        let mut outgoing = BTreeMap::new();
        for member in context.participants().iter().copied() {
            let packages = round1_packages
                .iter()
                .filter_map(|(sender, package)| (*sender != member).then_some(package.clone()));
            let (secret, packages) = advance_dkg(
                round1_secrets.remove(&member).expect("round-one secret"),
                packages,
            )
            .expect("round two");
            for package in packages.values() {
                assert_eq!(
                    DkgRound2Package::decode(&context, package.receiver(), &package.encode())
                        .expect("round-two wire"),
                    *package
                );
                let sealed = package
                    .seal(
                        &storage_key,
                        &mut ChaCha20Rng::from_seed(
                            [package
                                .sender()
                                .get()
                                .wrapping_add(package.receiver().get())
                                as u8; 32],
                        ),
                    )
                    .expect("seal round-two wire package");
                assert_eq!(
                    DkgRound2Package::open(
                        &context,
                        package.sender(),
                        package.receiver(),
                        &storage_key,
                        &sealed,
                    )
                    .expect("open round-two wire package"),
                    *package
                );
            }
            let sealed = secret
                .seal(
                    &storage_key,
                    &mut ChaCha20Rng::from_seed([member.get().wrapping_add(40) as u8; 32]),
                )
                .expect("seal round two");
            let secret = DkgRound2Secret::open(context.clone(), member, &storage_key, &sealed)
                .expect("restart round two");
            round2_secrets.insert(member, secret);
            outgoing.insert(member, packages);
        }

        let mut roots = Vec::new();
        for member in context.participants().iter().copied() {
            let round1 = round1_packages
                .iter()
                .filter_map(|(sender, package)| (*sender != member).then_some(package.clone()));
            let round2 = outgoing
                .iter()
                .filter(|(sender, _)| **sender != member)
                .map(|(_, packages)| packages.get(&member).expect("recipient package").clone());
            let root = finish_dkg(
                round2_secrets.remove(&member).expect("round-two secret"),
                round1,
                round2,
            )
            .expect("finish DKG");
            let public = root.public().clone();
            let sealed = root
                .seal(
                    &storage_key,
                    &mut ChaCha20Rng::from_seed([member.get().wrapping_add(80) as u8; 32]),
                )
                .expect("seal root share");
            roots.push(
                CanonicalRootKeyPackage::open(member, public, &storage_key, &sealed)
                    .expect("restart root share"),
            );
        }
        let expected = roots[0].public().encode();
        assert!(roots.iter().all(|root| root.public().encode() == expected));
        assert_eq!(roots[0].public().threshold(), threshold);
        assert_eq!(
            roots[0]
                .public()
                .verifying_shares()
                .values()
                .collect::<BTreeSet<_>>()
                .len(),
            usize::from(max_signers),
            "every participant must receive a distinct verification share"
        );
        roots
    }

    #[test]
    fn dealer_free_dkg_matrix_produces_signing_roots() {
        for (max_signers, threshold, profile) in [(3, 2, 41), (5, 3, 42), (11, 8, 43), (20, 14, 44)]
        {
            let roots = run_dkg(max_signers, threshold, profile);
            let registration = ApplicationRegistration::new(
                ApplicationId::from_label("org.cashubtc.cdk.dkg-test").expect("application"),
                ApplicationKeyEpoch::new(1).expect("key epoch"),
                ApplicationDerivationVersion::new(1).expect("version"),
                [profile; 32],
            )
            .expect("registration");
            let mut managers = roots
                .into_iter()
                .map(|root| {
                    let participant = root.participant();
                    let nonce_key = SecretEncryptionKey::new(
                        [profile
                            .wrapping_add(participant.get() as u8)
                            .wrapping_add(1); 32],
                    )
                    .expect("nonce storage key");
                    let mut manager = ParticipantKeyManager::new_in_memory(root, nonce_key)
                        .expect("participant key manager");
                    manager
                        .register_application(TestAdapter(registration))
                        .expect("register application");
                    manager
                })
                .collect::<Vec<_>>();
            let root_before = managers[0].root_public_package().encode();
            let second_registration = ApplicationRegistration::new(
                ApplicationId::from_label("org.cashubtc.cdk.dkg-second-test")
                    .expect("second application"),
                ApplicationKeyEpoch::new(1).expect("key epoch"),
                ApplicationDerivationVersion::new(1).expect("version"),
                [profile.wrapping_add(20); 32],
            )
            .expect("second registration");
            for manager in &mut managers {
                manager
                    .register_application(TestAdapter(second_registration))
                    .expect("register second application");
            }
            assert_ne!(
                managers[0]
                    .public_package(registration)
                    .expect("first application")
                    .group_key(),
                managers[0]
                    .public_package(second_registration)
                    .expect("second application")
                    .group_key()
            );
            assert_eq!(managers[0].root_public_package().encode(), root_before);
            let session = SigningSessionId::from_bytes([profile; 32]).expect("session");
            let message = [profile.wrapping_add(1); 32];
            let signing_scope = SigningAuthorizationScope::new(
                [profile.wrapping_add(30); 32],
                managers[0].root_public_package().epoch(),
                registration,
                SigningDerivation::application_key().binding(),
                session.to_bytes(),
                message,
            )
            .expect("signing scope");
            let signing_context = SigningSessionContext::new(
                signing_scope,
                0,
                managers
                    .iter()
                    .take(usize::from(threshold))
                    .map(ParticipantKeyManager::participant),
            )
            .expect("signing context");
            let session = signing_context.session_id();
            let mut rng = ChaCha20Rng::from_seed([profile.wrapping_add(2); 32]);
            let commitments = managers
                .iter_mut()
                .take(threshold.into())
                .map(|manager| {
                    manager
                        .commit_nonce(&signing_context, &mut rng)
                        .expect("commit nonce")
                })
                .collect::<Vec<_>>();
            let set = NonceCommitmentSet::new(commitments.clone()).expect("commitment set");
            let request = AuthorizationRequest::new(
                registration.id(),
                registration.key_epoch(),
                registration.derivation_version(),
                session,
                message.to_vec(),
            )
            .expect("request");
            let shares = managers
                .iter_mut()
                .zip(&commitments)
                .map(|(manager, commitment)| {
                    manager
                        .sign_share(commitment.nonce_id(), &request, &set)
                        .expect("share")
                })
                .collect::<Vec<_>>();
            ParticipantKeyManager::aggregate(
                managers[0]
                    .public_package(registration)
                    .expect("public package"),
                session,
                &message,
                &set,
                shares,
            )
            .expect("aggregate");
        }
    }

    #[test]
    fn dkg_rejects_wrong_context_missing_members_and_mutated_wire() {
        let expected_context = context(3, 2, 51);
        let member = MemberId::new(1).expect("member");
        let (secret, package) = start_dkg(
            expected_context.clone(),
            member,
            ChaCha20Rng::from_seed([51; 32]),
        )
        .expect("round one");
        let wrong = context(3, 2, 52);
        assert!(matches!(
            DkgRound1Package::decode(&wrong, &package.encode()),
            Err(Error::WrongSession)
        ));
        assert!(advance_dkg(secret, Vec::new()).is_err());
        let mut mutated = package.encode();
        mutated.push(0);
        assert!(DkgRound1Package::decode(&expected_context, &mutated).is_err());
        assert!(format!("{package:?}").contains("DkgRound1Package"));
    }

    #[test]
    fn dkg_messages_reject_cross_domain_replay_wrong_recipient_and_equivocation() {
        let expected = context(3, 2, 61);
        let members = expected.participants().iter().copied().collect::<Vec<_>>();
        let mut secrets = BTreeMap::new();
        let mut packages = BTreeMap::new();
        for member in &members {
            let (secret, package) = start_dkg(
                expected.clone(),
                *member,
                ChaCha20Rng::from_seed([member.get() as u8; 32]),
            )
            .expect("round one");
            secrets.insert(*member, secret);
            packages.insert(*member, package);
        }

        let original = packages.get(&members[1]).expect("round-one package");
        for wrong in [
            DkgContext::new(
                [62; 32],
                expected.ceremony_id(),
                expected.root_epoch(),
                expected.threshold(),
                expected.participants().clone(),
            )
            .expect("wrong federation"),
            DkgContext::new(
                expected.federation_id(),
                [63; 32],
                expected.root_epoch(),
                expected.threshold(),
                expected.participants().clone(),
            )
            .expect("wrong ceremony"),
            DkgContext::new(
                expected.federation_id(),
                expected.ceremony_id(),
                RootKeyEpoch::new(99).expect("wrong epoch"),
                expected.threshold(),
                expected.participants().clone(),
            )
            .expect("wrong epoch context"),
            DkgContext::new(
                expected.federation_id(),
                expected.ceremony_id(),
                expected.root_epoch(),
                3,
                expected.participants().clone(),
            )
            .expect("wrong threshold"),
            context(4, 2, 61),
        ] {
            assert!(matches!(
                DkgRound1Package::decode(&wrong, &original.encode()),
                Err(Error::WrongSession)
            ));
        }

        let local = members[0];
        let other_packages = packages
            .iter()
            .filter_map(|(member, package)| (*member != local).then_some(package.clone()))
            .collect::<Vec<_>>();
        let duplicate = vec![other_packages[0].clone(), other_packages[0].clone()];
        assert!(advance_dkg(secrets.remove(&local).expect("local secret"), duplicate).is_err());

        let (proof_test_secret, _) =
            start_dkg(expected.clone(), local, ChaCha20Rng::from_seed([90; 32]))
                .expect("proof-test local round one");
        let mut corrupted_round1 = other_packages[0].encode();
        *corrupted_round1.last_mut().expect("round-one proof byte") ^= 1;
        let corrupted_round1 = DkgRound1Package::decode(&expected, &corrupted_round1)
            .expect("structurally valid corrupted proof");
        assert!(advance_dkg(
            proof_test_secret,
            [corrupted_round1, other_packages[1].clone()]
        )
        .is_err());

        let (local_secret, replacement_package) =
            start_dkg(expected.clone(), local, ChaCha20Rng::from_seed([91; 32]))
                .expect("replacement local round one");
        packages.insert(local, replacement_package);
        let (round2_secret, outgoing) = advance_dkg(local_secret, other_packages.into_iter().rev())
            .expect("reordered round one is deterministic");
        let receiver = members[1];
        let round2 = outgoing.get(&receiver).expect("receiver package");
        assert!(matches!(
            DkgRound2Package::decode(&expected, members[2], &round2.encode()),
            Err(Error::WrongSession)
        ));
        let mut mutated = round2.encode();
        let last = mutated.last_mut().expect("round-two byte");
        *last ^= 1;
        let mutated = DkgRound2Package::decode(&expected, receiver, &mutated)
            .expect("structurally valid mutated share");
        let receiver_round1 = packages
            .iter()
            .filter_map(|(sender, package)| (*sender != receiver).then_some(package.clone()));
        let (receiver_secret, _) = advance_dkg(
            secrets.remove(&receiver).expect("receiver secret"),
            receiver_round1,
        )
        .expect("receiver round two");
        let third = members[2];
        let third_round1 = packages
            .iter()
            .filter_map(|(sender, package)| (*sender != third).then_some(package.clone()));
        let (_, third_outgoing) =
            advance_dkg(secrets.remove(&third).expect("third secret"), third_round1)
                .expect("third round two");
        let receiver_round1 = packages
            .iter()
            .filter_map(|(sender, package)| (*sender != receiver).then_some(package.clone()));
        assert!(finish_dkg(
            receiver_secret,
            receiver_round1,
            [
                mutated,
                third_outgoing
                    .get(&receiver)
                    .expect("third-to-receiver package")
                    .clone(),
            ],
        )
        .is_err());
        assert!(finish_dkg(round2_secret, Vec::new(), Vec::new()).is_err());
    }

    #[test]
    fn sealed_dkg_state_is_bound_to_key_context_phase_and_participant() {
        let expected = context(3, 2, 71);
        let member = MemberId::new(1).expect("member");
        let key = SecretEncryptionKey::new([71; 32]).expect("storage key");
        let wrong_key = SecretEncryptionKey::new([72; 32]).expect("wrong storage key");
        let (secret, _) = start_dkg(expected.clone(), member, ChaCha20Rng::from_seed([73; 32]))
            .expect("round one");
        let sealed = secret
            .seal(&key, &mut ChaCha20Rng::from_seed([74; 32]))
            .expect("seal round one");
        assert!(DkgRound1Secret::open(expected.clone(), member, &wrong_key, &sealed).is_err());
        assert!(DkgRound1Secret::open(
            expected.clone(),
            MemberId::new(2).expect("other member"),
            &key,
            &sealed
        )
        .is_err());
        assert!(DkgRound1Secret::open(context(3, 2, 72), member, &key, &sealed).is_err());
        assert!(DkgRound2Secret::open(expected, member, &key, &sealed).is_err());
    }
}
