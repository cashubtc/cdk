//! Protected receive-secret bindings and private transport payloads.

use core::fmt;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::Network;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroize;

use super::operation::canonical_json_bytes;
use super::{
    FederationBarkActionId, FederationBarkDigest, FederationBarkReceiveIntent,
    FederationBarkReceiveProof, FederationBarkReceiveSecretAcknowledgement, FederationBarkUnit,
    FederationBarkVtxoLocator, FederationConfig, FederationError, FederationId,
    MemberConfigPrivate, MemberId, Result,
};
use crate::Amount;

/// Current protected receive-secret record and acknowledgement format.
pub const FEDERATION_BARK_RECEIVE_SECRET_VERSION: u16 = 1;
/// Maximum canonical private prepare-secret request size.
pub const FEDERATION_MAX_BARK_RECEIVE_SECRET_REQUEST_BYTES: usize = 4 * 1024;

const RECEIVE_SECRET_RECORD_DOMAIN: &[u8] = b"cdk/federation/bark-receive-secret-record/v1";
const RECEIVE_SECRET_PREIMAGE_DOMAIN: &[u8] = b"cdk/federation/bark-receive-preimage/v1";
const RECEIVE_SECRET_ACK_DOMAIN: &[u8] = b"cdk/federation/bark-receive-secret-ack/v1";
const ARK_SERVER_DOMAIN: &[u8] = b"cdk/federation/bark-ark-server/v1";

fn invalid_secret(reason: impl Into<String>) -> FederationError {
    FederationError::BarkOperationInvalid {
        reason: reason.into(),
    }
}

/// Complete public binding for one protected incoming-payment preimage.
///
/// This deliberately contains every receive-intent field that a member must
/// verify before persisting a secret. It contains only commitments, never the
/// preimage itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkReceiveSecretBinding {
    /// Protected-record format.
    pub version: u16,
    /// Owning federation.
    pub federation_id: FederationId,
    /// Stable receive action.
    pub action_id: FederationBarkActionId,
    /// Cashu mint quote identifier.
    pub quote: String,
    /// Member intended to create or recover the invoice.
    pub actor: MemberId,
    /// Cashu liability in satoshis.
    pub cashu_amount: Amount,
    /// Gross BOLT11 amount in satoshis.
    pub gross_amount: Amount,
    /// Accepted receive fee.
    pub receive_fee: Amount,
    /// Minimum value that must become federation custody.
    pub minimum_net_amount: Amount,
    /// Accepted currency unit.
    pub unit: FederationBarkUnit,
    /// Commitment to the user-visible description.
    pub description_hash: FederationBarkDigest,
    /// SHA-256 payment hash of the protected preimage.
    pub payment_hash: FederationBarkDigest,
    /// Domain-separated commitment to the protected preimage.
    pub preimage_commitment: FederationBarkDigest,
    /// Consensus-owned receive locator.
    pub destination: FederationBarkVtxoLocator,
    /// Member-independent mailbox scope.
    pub mailbox_scope: FederationBarkDigest,
    /// Pinned Ark server identity commitment.
    pub ark_server: FederationBarkDigest,
    /// Immutable Bitcoin network.
    pub network: Network,
    /// Accepted minimum final-CLTV delta.
    pub minimum_cltv_delta: u32,
    /// Accepted receive-fee policy version.
    pub fee_policy_version: u16,
    /// Accepted anti-DoS policy version.
    pub anti_dos_policy_version: u16,
    /// Exact anti-DoS proof mode.
    pub receive_proof: FederationBarkReceiveProof,
    /// Consensus-visible intent expiry.
    pub expires_at: u64,
}

impl FederationBarkReceiveSecretBinding {
    /// Reconstruct the exact protected-secret binding of a receive intent.
    #[must_use]
    pub fn from_intent(federation_id: FederationId, intent: &FederationBarkReceiveIntent) -> Self {
        Self {
            version: FEDERATION_BARK_RECEIVE_SECRET_VERSION,
            federation_id,
            action_id: intent.action_id,
            quote: intent.quote.clone(),
            actor: intent.actor,
            cashu_amount: intent.cashu_amount,
            gross_amount: intent.gross_amount,
            receive_fee: intent.receive_fee,
            minimum_net_amount: intent.minimum_net_amount,
            unit: intent.unit,
            description_hash: intent.description_hash,
            payment_hash: intent.payment_hash,
            preimage_commitment: intent.preimage_commitment,
            destination: intent.destination,
            mailbox_scope: intent.mailbox_scope,
            ark_server: intent.ark_server,
            network: intent.network,
            minimum_cltv_delta: intent.minimum_cltv_delta,
            fee_policy_version: intent.fee_policy_version,
            anti_dos_policy_version: intent.anti_dos_policy_version,
            receive_proof: intent.receive_proof,
            expires_at: intent.expires_at,
        }
    }

    /// Validate public binding fields against the accepted federation config.
    pub fn validate(&self, config: &FederationConfig) -> Result<()> {
        if self.version != FEDERATION_BARK_RECEIVE_SECRET_VERSION {
            return Err(invalid_secret("unsupported Bark receive-secret version"));
        }
        if self.federation_id != config.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: config.federation_id,
                found: self.federation_id,
            });
        }
        if self.quote.is_empty() || self.quote.len() > 256 {
            return Err(invalid_secret(
                "Bark receive-secret quote is empty or too long",
            ));
        }
        if !config.members.contains_key(&self.actor) {
            return Err(FederationError::UnknownMember {
                member_id: self.actor,
            });
        }
        if self.cashu_amount == Amount::ZERO
            || self.gross_amount != self.cashu_amount
            || self.receive_fee != Amount::ZERO
            || self.minimum_net_amount < self.cashu_amount
            || self.unit != FederationBarkUnit::Sat
        {
            return Err(invalid_secret(
                "Bark receive-secret amounts violate the v1 collateral invariant",
            ));
        }
        if self.destination.role != 1
            || self.destination.version != 1
            || self.minimum_cltv_delta == 0
            || self.fee_policy_version != 1
            || self.anti_dos_policy_version != 1
            || self.expires_at == 0
        {
            return Err(invalid_secret(
                "Bark receive-secret locator, policy, or expiry is unsupported",
            ));
        }
        let application = config
            .frost_wallet
            .as_ref()
            .and_then(|wallet| wallet.bark_application.as_ref())
            .ok_or_else(|| invalid_secret("accepted Bark application is missing"))?;
        if self.network != application.network
            || self.minimum_cltv_delta < application.policy.minimum_incoming_cltv_delta
            || self.ark_server != ark_server_commitment(&application.ark_server_public_key)?
        {
            return Err(invalid_secret(
                "Bark receive-secret network, CLTV, or Ark server differs from accepted config",
            ));
        }
        Ok(())
    }

    /// Return sorted-key canonical bytes for durable hashing and backup.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self)
    }

    /// Commitment shared by every member acknowledgement and local record.
    pub fn record_hash(&self) -> Result<FederationBarkDigest> {
        domain_hash(RECEIVE_SECRET_RECORD_DOMAIN, &self.canonical_bytes()?)
    }
}

/// Authenticated private request to durably protect one receive preimage.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkPrepareReceiveSecretRequest {
    /// Private request format.
    pub version: u16,
    /// Exact public record binding.
    pub binding: FederationBarkReceiveSecretBinding,
    /// Intended storing member, also bound by the outer signed REST envelope.
    pub recipient: MemberId,
    /// Plaintext preimage carried only over authenticated private transport.
    #[serde(with = "hex_array_32")]
    preimage: [u8; 32],
}

impl fmt::Debug for FederationBarkPrepareReceiveSecretRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederationBarkPrepareReceiveSecretRequest")
            .field("version", &self.version)
            .field("binding", &self.binding)
            .field("recipient", &self.recipient)
            .field("preimage", &"<redacted>")
            .finish()
    }
}

impl Drop for FederationBarkPrepareReceiveSecretRequest {
    fn drop(&mut self) {
        self.preimage.zeroize();
    }
}

impl FederationBarkPrepareReceiveSecretRequest {
    /// Construct a private request from explicit secret material.
    #[must_use]
    pub fn new(
        binding: FederationBarkReceiveSecretBinding,
        recipient: MemberId,
        preimage: [u8; 32],
    ) -> Self {
        Self {
            version: FEDERATION_BARK_RECEIVE_SECRET_VERSION,
            binding,
            recipient,
            preimage,
        }
    }

    /// Validate transport identities, public bindings, and both preimage commitments.
    pub fn validate(
        &self,
        config: &FederationConfig,
        authenticated_sender: MemberId,
        expected_recipient: MemberId,
    ) -> Result<()> {
        if self.version != FEDERATION_BARK_RECEIVE_SECRET_VERSION {
            return Err(invalid_secret(
                "unsupported Bark prepare-secret request version",
            ));
        }
        self.binding.validate(config)?;
        if authenticated_sender != self.binding.actor {
            return Err(invalid_secret(
                "authenticated Bark secret sender is not the bound receive actor",
            ));
        }
        if self.recipient != expected_recipient || !config.members.contains_key(&expected_recipient)
        {
            return Err(invalid_secret(
                "Bark secret request is bound to another recipient",
            ));
        }
        if self.binding.payment_hash != payment_hash_for_preimage(&self.preimage)?
            || self.binding.preimage_commitment != preimage_commitment(&self.preimage)?
        {
            return Err(invalid_secret(
                "Bark receive preimage does not match its payment commitments",
            ));
        }
        let actual = canonical_json_bytes(self)?.len();
        if actual > FEDERATION_MAX_BARK_RECEIVE_SECRET_REQUEST_BYTES {
            return Err(invalid_secret(format!(
                "Bark prepare-secret request is {actual} bytes, maximum is {FEDERATION_MAX_BARK_RECEIVE_SECRET_REQUEST_BYTES}"
            )));
        }
        Ok(())
    }

    /// Copy the fixed-size secret into an immediately zeroizable owner.
    #[must_use]
    pub fn secret(&self) -> FederationBarkReceiveSecret {
        FederationBarkReceiveSecret(self.preimage)
    }
}

/// Zeroizing in-memory owner for one receive preimage.
#[derive(PartialEq, Eq)]
pub struct FederationBarkReceiveSecret([u8; 32]);

impl FederationBarkReceiveSecret {
    /// Construct a zeroizing secret owner from validated bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the preimage without formatting it.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for FederationBarkReceiveSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FederationBarkReceiveSecret(<redacted>)")
    }
}

impl Drop for FederationBarkReceiveSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl FederationBarkReceiveSecretAcknowledgement {
    /// Sign the canonical protected-record commitment after durable persistence.
    pub fn new_signed(
        binding: &FederationBarkReceiveSecretBinding,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        if private_config
            .federation_id
            .is_some_and(|federation_id| federation_id != binding.federation_id)
        {
            return Err(invalid_secret(
                "receive-secret signer belongs to another federation",
            ));
        }
        let member = private_config.member_id;
        let record_hash = binding.record_hash()?;
        let signature =
            private_config
                .identity_secret_key
                .sign_no_aux_rand(&acknowledgement_transcript(
                    binding.federation_id,
                    member,
                    record_hash,
                ))?;
        Ok(Self {
            member,
            record_hash,
            signature: super::FederationOperationSignature::from_slice(&signature.serialize())?,
        })
    }

    /// Verify one durable acknowledgement against the roster and exact binding.
    pub fn verify(
        &self,
        config: &FederationConfig,
        binding: &FederationBarkReceiveSecretBinding,
    ) -> Result<()> {
        binding.validate(config)?;
        let expected_record_hash = binding.record_hash()?;
        if self.record_hash != expected_record_hash {
            return Err(invalid_secret(
                "receive-secret acknowledgement binds another record",
            ));
        }
        let member = config
            .member(self.member)
            .ok_or(FederationError::UnknownMember {
                member_id: self.member,
            })?;
        let signature = Signature::from_slice(&self.signature.to_bytes())
            .map_err(|_| invalid_secret("invalid receive-secret acknowledgement signature"))?;
        member
            .identity_public_key
            .verify(
                &acknowledgement_transcript(binding.federation_id, self.member, self.record_hash),
                &signature,
            )
            .map_err(|_| {
                invalid_secret("receive-secret acknowledgement signature verification failed")
            })
    }
}

impl FederationBarkReceiveIntent {
    /// Verify that a signature threshold durably acknowledged this exact intent binding.
    pub fn validate_secret_acknowledgements(&self, config: &FederationConfig) -> Result<()> {
        let binding = FederationBarkReceiveSecretBinding::from_intent(config.federation_id, self);
        binding.validate(config)?;
        if self.secret_acknowledgements.len()
            < usize::from(config.threshold_params.signature_threshold)
        {
            return Err(invalid_secret(
                "receive-secret acknowledgements do not satisfy the signature threshold",
            ));
        }
        let mut previous = None;
        let mut actor_acknowledged = false;
        for acknowledgement in &self.secret_acknowledgements {
            if previous.is_some_and(|member| member >= acknowledgement.member) {
                return Err(invalid_secret(
                    "receive-secret acknowledgements are not sorted and unique",
                ));
            }
            acknowledgement.verify(config, &binding)?;
            actor_acknowledged |= acknowledgement.member == self.actor;
            previous = Some(acknowledgement.member);
        }
        if !actor_acknowledged {
            return Err(invalid_secret(
                "receive actor did not acknowledge the protected secret",
            ));
        }
        Ok(())
    }
}

/// Compute the BOLT11 payment hash for a preimage.
pub fn payment_hash_for_preimage(preimage: &[u8; 32]) -> Result<FederationBarkDigest> {
    FederationBarkDigest::from_bytes(Sha256Hash::hash(preimage).to_byte_array())
}

/// Compute the domain-separated federation commitment to a preimage.
pub fn preimage_commitment(preimage: &[u8; 32]) -> Result<FederationBarkDigest> {
    domain_hash(RECEIVE_SECRET_PREIMAGE_DOMAIN, preimage)
}

/// Commit to the accepted Ark server public key.
pub fn ark_server_commitment(public_key: &[u8; 33]) -> Result<FederationBarkDigest> {
    bitcoin::secp256k1::PublicKey::from_slice(public_key)
        .map_err(|_| invalid_secret("invalid Bark Ark server public key"))?;
    domain_hash(ARK_SERVER_DOMAIN, public_key)
}

fn acknowledgement_transcript(
    federation_id: FederationId,
    member: MemberId,
    record_hash: FederationBarkDigest,
) -> Vec<u8> {
    let mut transcript = Vec::new();
    append_bytes(&mut transcript, RECEIVE_SECRET_ACK_DOMAIN);
    append_bytes(&mut transcript, &federation_id.to_bytes());
    append_bytes(&mut transcript, &member.get().to_be_bytes());
    append_bytes(&mut transcript, &record_hash.to_bytes());
    transcript
}

fn domain_hash(domain: &[u8], value: &[u8]) -> Result<FederationBarkDigest> {
    let mut transcript = Vec::new();
    append_bytes(&mut transcript, domain);
    append_bytes(&mut transcript, value);
    FederationBarkDigest::from_bytes(Sha256Hash::hash(&transcript).to_byte_array())
}

fn append_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u32).to_be_bytes());
    transcript.extend_from_slice(value);
}

mod hex_array_32 {
    use super::*;

    pub fn serialize<S>(bytes: &[u8; 32], serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&crate::util::hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes = crate::util::hex::decode(value).map_err(serde::de::Error::custom)?;
        bytes.try_into().map_err(|bytes: Vec<u8>| {
            serde::de::Error::custom(format!("expected 32 secret bytes, found {}", bytes.len()))
        })
    }
}
