//! Canonical bounded wire encodings for public root and signing messages.

use std::collections::BTreeMap;

use frost_secp256k1_tr as frost;

use crate::canonical::validate_root_public;
use crate::types::{
    AggregateSignature, ApplicationDerivationVersion, ApplicationId, ApplicationKeyEpoch,
    ApplicationRegistration, AuthorizationRequest, FrostCiphersuite, MemberId, NonceCommitment,
    NonceCommitmentSet, NonceId, RootKeyEpoch, RootPublicPackage, SignatureMode, SignatureShare,
    SigningDerivation, SigningSessionId, MAX_AUTHORIZATION_BYTES, MAX_COMMITMENT_BYTES,
    MAX_DERIVATION_STEPS, MAX_PARTICIPANTS,
};
use crate::{Error, SigningAuthorizationScope, SigningSessionContext};

const ROOT_MAGIC: &[u8; 8] = b"CDKFRT01";
const COMMITMENT_MAGIC: &[u8; 8] = b"CDKFNC03";
const SHARE_MAGIC: &[u8; 8] = b"CDKFSH03";
const SESSION_MAGIC: &[u8; 8] = b"CDKFSC01";
const DERIVATION_MAGIC: &[u8; 8] = b"CDKFSD01";
const AUTHORIZATION_MAGIC: &[u8; 8] = b"CDKFAU01";
const COMMITMENT_SET_MAGIC: &[u8; 8] = b"CDKFCS01";
const ROOT_WIRE_MAX: usize = 128 + MAX_PARTICIPANTS * 35;
const COMMITMENT_WIRE_MAX: usize = 192 + MAX_COMMITMENT_BYTES;
const SHARE_WIRE_LEN: usize = 8 + 8 + 32 + 8 + 2 + 32 + 32 + 32 + 2 + 32;
const SESSION_WIRE_MAX: usize = 256 + MAX_PARTICIPANTS * 2;
const DERIVATION_WIRE_MAX: usize = 80 + MAX_DERIVATION_STEPS * 32;
const AUTHORIZATION_WIRE_MAX: usize = 96 + MAX_AUTHORIZATION_BYTES;
const COMMITMENT_SET_WIRE_MAX: usize = 16 + MAX_PARTICIPANTS * (4 + COMMITMENT_WIRE_MAX);

impl RootPublicPackage {
    /// Encode the public root package canonically.
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(96 + self.verifying_shares.len() * 35);
        output.extend_from_slice(ROOT_MAGIC);
        output.extend_from_slice(&self.epoch.get().to_be_bytes());
        output.extend_from_slice(&self.dkg_protocol_version.to_be_bytes());
        output.extend_from_slice(&1u16.to_be_bytes());
        output.extend_from_slice(&self.threshold.to_be_bytes());
        output.extend_from_slice(&(self.verifying_shares.len() as u16).to_be_bytes());
        for (member, share) in &self.verifying_shares {
            output.extend_from_slice(&member.get().to_be_bytes());
            output.extend_from_slice(share);
        }
        output.extend_from_slice(&self.group_key);
        output.extend_from_slice(&self.transcript_binding);
        output
    }

    /// Decode a canonical public root package, rejecting trailing and oversized
    /// input.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > ROOT_WIRE_MAX {
            return Err(Error::OversizedInput {
                kind: "root public package",
                max: ROOT_WIRE_MAX,
            });
        }
        let mut reader = Reader::new(bytes, "root public package");
        reader.expect_magic(ROOT_MAGIC)?;
        let epoch = RootKeyEpoch::new(reader.u64()?)?;
        let dkg_protocol_version = reader.u16()?;
        let ciphersuite = match reader.u16()? {
            1 => FrostCiphersuite::Secp256k1Sha256TrV1,
            _ => {
                return Err(Error::malformed(
                    "root public package",
                    "unsupported ciphersuite",
                ));
            }
        };
        let threshold = reader.u16()?;
        let count = usize::from(reader.u16()?);
        if count > MAX_PARTICIPANTS {
            return Err(Error::OversizedInput {
                kind: "root roster",
                max: MAX_PARTICIPANTS,
            });
        }
        let mut verifying_shares = BTreeMap::new();
        let mut previous = None;
        for _ in 0..count {
            let member = MemberId::new(reader.u16()?)?;
            if previous.is_some_and(|prior: MemberId| prior >= member) {
                return Err(Error::InvalidParticipant {
                    participant: member.get(),
                    reason: "members are duplicated or not canonically ordered".to_string(),
                });
            }
            let share = reader.array::<33>()?;
            verifying_shares.insert(member, share);
            previous = Some(member);
        }
        let group_key = reader.array::<33>()?;
        let transcript_binding = reader.array::<32>()?;
        reader.finish()?;
        let package = Self {
            epoch,
            dkg_protocol_version,
            ciphersuite,
            threshold,
            verifying_shares,
            group_key,
            transcript_binding,
        };
        validate_root_public(&package)?;
        if package.encode() != bytes {
            return Err(Error::malformed(
                "root public package",
                "encoding is not canonical",
            ));
        }
        Ok(package)
    }
}

impl NonceCommitment {
    /// Encode a public nonce commitment canonically.
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(192 + self.commitments.len());
        output.extend_from_slice(COMMITMENT_MAGIC);
        encode_scope(
            &mut output,
            self.root_epoch,
            self.registration,
            self.session_id,
        );
        output.extend_from_slice(&self.derivation_binding);
        output.extend_from_slice(&self.nonce_id.to_bytes());
        output.extend_from_slice(&self.participant.get().to_be_bytes());
        output.extend_from_slice(&(self.commitments.len() as u16).to_be_bytes());
        output.extend_from_slice(&self.commitments);
        output
    }

    /// Decode a canonical bounded public nonce commitment.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > COMMITMENT_WIRE_MAX {
            return Err(Error::OversizedInput {
                kind: "nonce commitment",
                max: COMMITMENT_WIRE_MAX,
            });
        }
        let mut reader = Reader::new(bytes, "nonce commitment");
        reader.expect_magic(COMMITMENT_MAGIC)?;
        let (root_epoch, registration, session_id) = decode_scope(&mut reader)?;
        let derivation_binding = reader.array::<32>()?;
        if derivation_binding == [0; 32] {
            return Err(Error::malformed(
                "nonce commitment",
                "derivation binding cannot be all zeroes",
            ));
        }
        let nonce_bytes = reader.array::<32>()?;
        if nonce_bytes == [0; 32] {
            return Err(Error::malformed(
                "nonce commitment",
                "nonce identifier cannot be all zeroes",
            ));
        }
        let nonce_id = NonceId::from_bytes_unchecked(nonce_bytes);
        let participant = MemberId::new(reader.u16()?)?;
        let commitment_len = usize::from(reader.u16()?);
        if commitment_len > MAX_COMMITMENT_BYTES {
            return Err(Error::OversizedInput {
                kind: "nonce commitment payload",
                max: MAX_COMMITMENT_BYTES,
            });
        }
        let commitments = reader.bytes(commitment_len)?.to_vec();
        reader.finish()?;
        frost::round1::SigningCommitments::deserialize(&commitments)
            .map_err(|err| Error::InvalidCommitment(err.to_string()))?;
        let value = Self {
            root_epoch,
            registration,
            session_id,
            derivation_binding,
            nonce_id,
            participant,
            commitments,
        };
        if !value.has_valid_nonce_id() {
            return Err(Error::InvalidCommitment(
                "nonce identifier does not match its commitment".to_string(),
            ));
        }
        if value.encode() != bytes {
            return Err(Error::malformed(
                "nonce commitment",
                "encoding is not canonical",
            ));
        }
        Ok(value)
    }
}

impl SignatureShare {
    /// Encode a signature share canonically.
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(SHARE_WIRE_LEN);
        output.extend_from_slice(SHARE_MAGIC);
        encode_scope(
            &mut output,
            self.root_epoch,
            self.registration,
            self.session_id,
        );
        output.extend_from_slice(&self.derivation_binding);
        output.extend_from_slice(&self.participant.get().to_be_bytes());
        output.extend_from_slice(&self.share);
        output
    }

    /// Decode a canonical fixed-size signature share.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > SHARE_WIRE_LEN {
            return Err(Error::OversizedInput {
                kind: "signature share",
                max: SHARE_WIRE_LEN,
            });
        }
        let mut reader = Reader::new(bytes, "signature share");
        reader.expect_magic(SHARE_MAGIC)?;
        let (root_epoch, registration, session_id) = decode_scope(&mut reader)?;
        let derivation_binding = reader.array::<32>()?;
        if derivation_binding == [0; 32] {
            return Err(Error::malformed(
                "signature share",
                "derivation binding cannot be all zeroes",
            ));
        }
        let participant = MemberId::new(reader.u16()?)?;
        let share = reader.array::<32>()?;
        reader.finish()?;
        frost::round2::SignatureShare::deserialize(&share)
            .map_err(|err| Error::InvalidShare(err.to_string()))?;
        let value = Self {
            root_epoch,
            registration,
            session_id,
            derivation_binding,
            participant,
            share,
        };
        if value.encode() != bytes {
            return Err(Error::malformed(
                "signature share",
                "encoding is not canonical",
            ));
        }
        Ok(value)
    }
}

impl SigningSessionContext {
    /// Encode every authorization and attempt-separation field canonically.
    pub fn encode(&self) -> Vec<u8> {
        let registration = self.registration();
        let mut output = Vec::with_capacity(224 + self.signer_set().len() * 2);
        output.extend_from_slice(SESSION_MAGIC);
        output.extend_from_slice(&self.federation_id());
        output.extend_from_slice(&self.root_epoch().get().to_be_bytes());
        output.extend_from_slice(&registration.id().to_bytes());
        output.extend_from_slice(&registration.key_epoch().get().to_be_bytes());
        output.extend_from_slice(&registration.derivation_version().get().to_be_bytes());
        output.extend_from_slice(&registration.authorization_policy());
        output.extend_from_slice(&self.derivation_binding());
        output.extend_from_slice(&self.authorization_reference());
        output.extend_from_slice(&self.message_commitment());
        output.extend_from_slice(&self.attempt().to_be_bytes());
        output.extend_from_slice(&(self.signer_set().len() as u16).to_be_bytes());
        for member in self.signer_set() {
            output.extend_from_slice(&member.get().to_be_bytes());
        }
        output
    }

    /// Decode a bounded canonical signing context and recompute its session ID.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > SESSION_WIRE_MAX {
            return Err(Error::OversizedInput {
                kind: "signing session context",
                max: SESSION_WIRE_MAX,
            });
        }
        let mut reader = Reader::new(bytes, "signing session context");
        reader.expect_magic(SESSION_MAGIC)?;
        let federation_id = reader.array::<32>()?;
        let root_epoch = RootKeyEpoch::new(reader.u64()?)?;
        let registration = ApplicationRegistration::new(
            ApplicationId::from_bytes(reader.array::<32>()?)?,
            ApplicationKeyEpoch::new(reader.u64()?)?,
            ApplicationDerivationVersion::new(reader.u16()?)?,
            reader.array::<32>()?,
        )?;
        let derivation_binding = reader.array::<32>()?;
        let authorization_reference = reader.array::<32>()?;
        let message_commitment = reader.array::<32>()?;
        let attempt = reader.u32()?;
        let count = usize::from(reader.u16()?);
        if count > MAX_PARTICIPANTS {
            return Err(Error::OversizedInput {
                kind: "signing session signer set",
                max: MAX_PARTICIPANTS,
            });
        }
        let signers = (0..count)
            .map(|_| MemberId::new(reader.u16()?))
            .collect::<Result<Vec<_>, _>>()?;
        reader.finish()?;
        let scope = SigningAuthorizationScope::new(
            federation_id,
            root_epoch,
            registration,
            derivation_binding,
            authorization_reference,
            message_commitment,
        )?;
        let value = Self::new(scope, attempt, signers)?;
        if value.encode() != bytes {
            return Err(Error::malformed(
                "signing session context",
                "encoding is not canonical",
            ));
        }
        Ok(value)
    }
}

impl SigningDerivation {
    /// Encode the bounded additive derivation and final signature mode.
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(80 + self.tweaks().len() * 32);
        output.extend_from_slice(DERIVATION_MAGIC);
        output.extend_from_slice(&self.context());
        output.extend_from_slice(&(self.tweaks().len() as u16).to_be_bytes());
        for tweak in self.tweaks() {
            output.extend_from_slice(tweak);
        }
        match self.signature_mode() {
            SignatureMode::Plain => output.push(0),
            SignatureMode::TaprootKeyPath { merkle_root: None } => output.push(1),
            SignatureMode::TaprootKeyPath {
                merkle_root: Some(root),
            } => {
                output.push(2);
                output.extend_from_slice(&root);
            }
        }
        output
    }

    /// Decode and validate a canonical derivation plan.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > DERIVATION_WIRE_MAX {
            return Err(Error::OversizedInput {
                kind: "signing derivation",
                max: DERIVATION_WIRE_MAX,
            });
        }
        let mut reader = Reader::new(bytes, "signing derivation");
        reader.expect_magic(DERIVATION_MAGIC)?;
        let context = reader.array::<32>()?;
        let count = usize::from(reader.u16()?);
        if count > MAX_DERIVATION_STEPS {
            return Err(Error::OversizedInput {
                kind: "signing derivation",
                max: MAX_DERIVATION_STEPS,
            });
        }
        let tweaks = (0..count)
            .map(|_| reader.array::<32>())
            .collect::<Result<Vec<_>, _>>()?;
        let mode = match reader.u8()? {
            0 => SignatureMode::Plain,
            1 => SignatureMode::TaprootKeyPath { merkle_root: None },
            2 => SignatureMode::TaprootKeyPath {
                merkle_root: Some(reader.array::<32>()?),
            },
            _ => {
                return Err(Error::malformed(
                    "signing derivation",
                    "unknown signature mode",
                ))
            }
        };
        reader.finish()?;
        let value = Self::new(context, tweaks, mode)?;
        if value.encode() != bytes {
            return Err(Error::malformed(
                "signing derivation",
                "encoding is not canonical",
            ));
        }
        Ok(value)
    }
}

impl AuthorizationRequest {
    /// Encode the registered-application authorization request canonically.
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(88 + self.payload().len());
        output.extend_from_slice(AUTHORIZATION_MAGIC);
        output.extend_from_slice(&self.application_id().to_bytes());
        output.extend_from_slice(&self.application_key_epoch().get().to_be_bytes());
        output.extend_from_slice(&self.derivation_version().get().to_be_bytes());
        output.extend_from_slice(&self.session_id().to_bytes());
        output.extend_from_slice(&(self.payload().len() as u32).to_be_bytes());
        output.extend_from_slice(self.payload());
        output
    }

    /// Decode a bounded canonical application authorization request.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > AUTHORIZATION_WIRE_MAX {
            return Err(Error::OversizedInput {
                kind: "authorization request",
                max: AUTHORIZATION_WIRE_MAX,
            });
        }
        let mut reader = Reader::new(bytes, "authorization request");
        reader.expect_magic(AUTHORIZATION_MAGIC)?;
        let application_id = ApplicationId::from_bytes(reader.array::<32>()?)?;
        let key_epoch = ApplicationKeyEpoch::new(reader.u64()?)?;
        let derivation_version = ApplicationDerivationVersion::new(reader.u16()?)?;
        let session_id = SigningSessionId::from_bytes(reader.array::<32>()?)?;
        let payload_len = usize::try_from(reader.u32()?)
            .map_err(|_| Error::malformed("authorization request", "length overflow"))?;
        if payload_len > MAX_AUTHORIZATION_BYTES {
            return Err(Error::OversizedInput {
                kind: "authorization request",
                max: MAX_AUTHORIZATION_BYTES,
            });
        }
        let payload = reader.bytes(payload_len)?.to_vec();
        reader.finish()?;
        let value = Self::new(
            application_id,
            key_epoch,
            derivation_version,
            session_id,
            payload,
        )?;
        if value.encode() != bytes {
            return Err(Error::malformed(
                "authorization request",
                "encoding is not canonical",
            ));
        }
        Ok(value)
    }
}

impl NonceCommitmentSet {
    /// Encode a canonically ordered bounded signer commitment set.
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(COMMITMENT_SET_MAGIC);
        output.extend_from_slice(&(self.commitments().len() as u16).to_be_bytes());
        for commitment in self.commitments().values() {
            let encoded = commitment.encode();
            output.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
            output.extend_from_slice(&encoded);
        }
        output
    }

    /// Decode a bounded canonical commitment set and reject duplicates.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > COMMITMENT_SET_WIRE_MAX {
            return Err(Error::OversizedInput {
                kind: "nonce commitment set",
                max: COMMITMENT_SET_WIRE_MAX,
            });
        }
        let mut reader = Reader::new(bytes, "nonce commitment set");
        reader.expect_magic(COMMITMENT_SET_MAGIC)?;
        let count = usize::from(reader.u16()?);
        if count > MAX_PARTICIPANTS {
            return Err(Error::OversizedInput {
                kind: "nonce commitment set",
                max: MAX_PARTICIPANTS,
            });
        }
        let mut commitments = Vec::with_capacity(count);
        for _ in 0..count {
            let length = usize::try_from(reader.u32()?)
                .map_err(|_| Error::malformed("nonce commitment set", "length overflow"))?;
            if length > COMMITMENT_WIRE_MAX {
                return Err(Error::OversizedInput {
                    kind: "nonce commitment",
                    max: COMMITMENT_WIRE_MAX,
                });
            }
            commitments.push(NonceCommitment::decode(reader.bytes(length)?)?);
        }
        reader.finish()?;
        let value = Self::new(commitments)?;
        if value.encode() != bytes {
            return Err(Error::malformed(
                "nonce commitment set",
                "encoding is not canonical",
            ));
        }
        Ok(value)
    }
}

impl AggregateSignature {
    /// Decode and validate a compact BIP340 aggregate signature.
    pub fn from_bytes(bytes: [u8; 64]) -> Result<Self, Error> {
        frost::Signature::deserialize(&bytes)
            .map_err(|error| Error::InvalidShare(error.to_string()))?;
        Ok(Self::from_bytes_unchecked(bytes))
    }
}

fn encode_scope(
    output: &mut Vec<u8>,
    epoch: RootKeyEpoch,
    registration: ApplicationRegistration,
    session: SigningSessionId,
) {
    output.extend_from_slice(&epoch.get().to_be_bytes());
    output.extend_from_slice(&registration.id().to_bytes());
    output.extend_from_slice(&registration.key_epoch().get().to_be_bytes());
    output.extend_from_slice(&registration.derivation_version().get().to_be_bytes());
    output.extend_from_slice(&registration.authorization_policy());
    output.extend_from_slice(&session.to_bytes());
}

fn decode_scope(
    reader: &mut Reader<'_>,
) -> Result<(RootKeyEpoch, ApplicationRegistration, SigningSessionId), Error> {
    let epoch = RootKeyEpoch::new(reader.u64()?)?;
    let application_id = ApplicationId::from_bytes(reader.array::<32>()?)?;
    let application_key_epoch = ApplicationKeyEpoch::new(reader.u64()?)?;
    let derivation_version = ApplicationDerivationVersion::new(reader.u16()?)?;
    let policy = reader.array::<32>()?;
    let registration = ApplicationRegistration::new(
        application_id,
        application_key_epoch,
        derivation_version,
        policy,
    )?;
    let session = SigningSessionId::from_bytes(reader.array::<32>()?)?;
    Ok((epoch, registration, session))
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    kind: &'static str,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], kind: &'static str) -> Self {
        Self {
            bytes,
            position: 0,
            kind,
        }
    }

    fn expect_magic(&mut self, expected: &[u8; 8]) -> Result<(), Error> {
        if self.bytes(8)? != expected {
            return Err(Error::malformed(self.kind, "wrong type or version header"));
        }
        Ok(())
    }

    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.bytes(N)?
            .try_into()
            .map_err(|_| Error::malformed(self.kind, "truncated value"))
    }

    fn bytes(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| Error::malformed(self.kind, "length overflow"))?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| Error::malformed(self.kind, "truncated value"))?;
        self.position = end;
        Ok(bytes)
    }

    fn finish(self) -> Result<(), Error> {
        if self.position != self.bytes.len() {
            return Err(Error::malformed(self.kind, "trailing bytes"));
        }
        Ok(())
    }
}
