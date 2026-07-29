//! Federation identity, threshold, payment-policy, keyset, and config types.

use core::fmt;
use core::str::FromStr;
use std::collections::BTreeMap;
#[cfg(feature = "prometheus")]
use std::time::Instant;

use bitcoin::base64::engine::{general_purpose, GeneralPurpose};
use bitcoin::base64::{alphabet, Engine as _};
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use super::dkg::{FederationDkgTranscript, FederationDkgTranscriptHash};
use super::frost_config::FederationFrostWalletConfig;
use super::operation::{FederationKeysetRotationOperation, FederationPaymentObservationQuorum};
use super::{
    append_bytes, append_str, FederationCheckpointPolicy, FederationError, Result,
    FEDERATION_CONFIG_DIGEST_VERSION, FEDERATION_INVITE_PREFIX, FEDERATION_INVITE_VERSION,
    FEDERATION_KEYSET_ROTATION_PROOF_VERSION, FEDERATION_MIN_MEMBERS,
    FEDERATION_ROSTER_HASH_VERSION, FEDERATION_WALLET_PROTOCOL_VERSION,
};
use crate::nuts::nut01::{
    aggregate_public_key_shares, verify_blind_signature_share, BlsAggregatePublicKey,
    BlsG1PublicKey, BlsPublicKeyShare, BlsSecretKeyShare, BlsSignerId,
};
use crate::nuts::nut02::{KeySet, KeySetVersion};
use crate::nuts::nut06::FederationInfo;
use crate::{
    mint_url::MintUrl, Amount, BlindedMessage, CurrencyUnit, Id, Keys, PublicKey, SecretKey,
};

/// Stable identifier for a federation roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationId([u8; 32]);

impl FederationId {
    /// Create a federation ID from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a federation ID from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidFederationIdSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a federation ID from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this federation ID as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this federation ID as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationId {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Digest of the complete public federation config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationConfigDigest([u8; 32]);

impl FederationConfigDigest {
    /// Create a federation config digest from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a federation config digest from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidFederationConfigDigestSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a federation config digest from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this federation config digest as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this federation config digest as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationConfigDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationConfigDigest {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationConfigDigest {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationConfigDigest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Non-zero federation member identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemberId(u16);

impl MemberId {
    /// Create a member ID.
    pub fn new(id: u16) -> Result<Self> {
        if id == 0 {
            return Err(FederationError::InvalidMemberId(id));
        }

        Ok(Self(id))
    }

    /// Return this member ID as a u16.
    #[must_use]
    pub fn get(self) -> u16 {
        self.0
    }

    /// Return this member ID as a threshold BLS signer ID.
    ///
    /// # Panics
    ///
    /// Panics only if the validated non-zero member ID invariant is violated.
    #[must_use]
    pub fn to_bls_signer_id(self) -> BlsSignerId {
        BlsSignerId::new(self.0).expect("member id is non-zero")
    }
}

impl fmt::Display for MemberId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for MemberId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16(self.0)
    }
}

impl<'de> Deserialize<'de> for MemberId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MemberIdVisitor;

        impl de::Visitor<'_> for MemberIdVisitor {
            type Value = MemberId;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a non-zero u16 federation member id")
            }

            fn visit_u16<E>(self, value: u16) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                MemberId::new(value).map_err(E::custom)
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                let id = u16::try_from(value).map_err(E::custom)?;
                MemberId::new(id).map_err(E::custom)
            }

            fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                let id = u16::try_from(value).map_err(E::custom)?;
                MemberId::new(id).map_err(E::custom)
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                let id = value.parse::<u16>().map_err(E::custom)?;
                MemberId::new(id).map_err(E::custom)
            }

            fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&value)
            }
        }

        deserializer.deserialize_any(MemberIdVisitor)
    }
}

/// Federation threshold settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThresholdParams {
    /// Number of federation members.
    pub member_count: u16,
    /// Number of BLS shares needed to produce a mint signature.
    pub signature_threshold: u16,
    /// Number of members needed to commit consensus items.
    pub consensus_threshold: u16,
}

impl ThresholdParams {
    /// Create validated threshold parameters.
    pub fn new(
        member_count: u16,
        signature_threshold: u16,
        consensus_threshold: u16,
    ) -> Result<Self> {
        let params = Self {
            member_count,
            signature_threshold,
            consensus_threshold,
        };
        params.validate()?;
        Ok(params)
    }

    /// Create threshold params with the default BFT consensus quorum.
    pub fn with_bft_consensus(member_count: u16, signature_threshold: u16) -> Result<Self> {
        let max_faulty = member_count.saturating_sub(1) / 3;
        let consensus_threshold = member_count.saturating_sub(max_faulty);
        Self::new(member_count, signature_threshold, consensus_threshold)
    }

    /// Return the maximum Byzantine members in the production BFT model.
    pub fn max_faulty_members(&self) -> u16 {
        self.member_count.saturating_sub(1) / 3
    }

    /// Validate the production BFT safety profile.
    ///
    /// Structural validation deliberately remains separate so deterministic
    /// test runtimes and specialist deployments can represent weaker quorums
    /// explicitly. Production members and wallet imports must call this check
    /// in addition to [`Self::validate`].
    pub fn validate_bft_safety(&self) -> Result<()> {
        self.validate()?;

        let max_faulty = self.max_faulty_members();
        let minimum_signature_threshold = max_faulty.saturating_add(1);
        let minimum_consensus_threshold = self.member_count.saturating_sub(max_faulty);
        if self.signature_threshold < minimum_signature_threshold
            || self.consensus_threshold < minimum_consensus_threshold
        {
            return Err(FederationError::UnsafeThresholdProfile {
                members: self.member_count,
                signature_threshold: self.signature_threshold,
                consensus_threshold: self.consensus_threshold,
                max_faulty,
                minimum_signature_threshold,
                minimum_consensus_threshold,
            });
        }

        Ok(())
    }

    /// Validate threshold parameters.
    pub fn validate(&self) -> Result<()> {
        if self.member_count < FEDERATION_MIN_MEMBERS {
            return Err(FederationError::InsufficientMembers {
                found: self.member_count,
                minimum: FEDERATION_MIN_MEMBERS,
            });
        }
        if self.signature_threshold == 0
            || self.consensus_threshold == 0
            || self.signature_threshold > self.member_count
            || self.consensus_threshold > self.member_count
            || self.consensus_threshold < self.signature_threshold
        {
            return Err(FederationError::InvalidThresholdParams {
                members: self.member_count,
                signature_threshold: self.signature_threshold,
                consensus_threshold: self.consensus_threshold,
            });
        }

        Ok(())
    }
}

fn validate_protocol_component(component: &'static str, expected: u16, found: u16) -> Result<()> {
    if expected == found {
        return Ok(());
    }

    Err(FederationError::ProtocolVersionMismatch {
        component,
        expected,
        found,
    })
}

/// Payment-observation context with separate mint and melt policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationPaymentObservationContext {
    /// Incoming payment observation for a mint quote.
    MintQuote,
    /// Outgoing payment observation for a melt quote.
    MeltQuote,
}

impl fmt::Display for FederationPaymentObservationContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MintQuote => f.write_str("mint_quote"),
            Self::MeltQuote => f.write_str("melt_quote"),
        }
    }
}

/// Public policy for deciding when payment observations can mutate quote state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationPaymentObservationPolicy {
    /// Incoming mint quote payment verification policy.
    pub mint_quotes: FederationPaymentVerificationPolicy,
    /// Outgoing melt quote payment verification policy.
    pub melt_quotes: FederationPaymentVerificationPolicy,
}

impl FederationPaymentObservationPolicy {
    /// Create the development/test policy where one accepted observation finalizes.
    #[must_use]
    pub fn development_single_observation_for_testing() -> Self {
        Self {
            mint_quotes:
                FederationPaymentVerificationPolicy::development_single_observation_for_testing(),
            melt_quotes:
                FederationPaymentVerificationPolicy::development_single_observation_for_testing(),
        }
    }

    /// Create a real-backend policy requiring matching member observations.
    #[must_use]
    pub fn member_quorum(required: u16) -> Self {
        let policy = FederationPaymentVerificationPolicy::member_quorum(required);
        Self {
            mint_quotes: policy,
            melt_quotes: policy,
        }
    }

    /// Return the policy for one payment observation context.
    #[must_use]
    pub fn policy_for(
        &self,
        context: FederationPaymentObservationContext,
    ) -> &FederationPaymentVerificationPolicy {
        match context {
            FederationPaymentObservationContext::MintQuote => &self.mint_quotes,
            FederationPaymentObservationContext::MeltQuote => &self.melt_quotes,
        }
    }

    /// Validate this policy against federation threshold parameters.
    pub fn validate(&self, threshold_params: ThresholdParams) -> Result<()> {
        self.mint_quotes.validate(
            FederationPaymentObservationContext::MintQuote,
            threshold_params,
        )?;
        self.melt_quotes.validate(
            FederationPaymentObservationContext::MeltQuote,
            threshold_params,
        )?;

        Ok(())
    }
}

/// Policy for one class of payment observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FederationPaymentVerificationPolicy {
    /// Test/development policy where one accepted observation finalizes.
    #[serde(rename = "development_single_observation")]
    DevelopmentSingleObservation,
    /// Real-backend policy requiring matching observations from a member quorum.
    MemberQuorum {
        /// Required number of matching member observations.
        required: u16,
    },
}

impl FederationPaymentVerificationPolicy {
    /// Create the development/test single-observation policy.
    #[must_use]
    pub fn development_single_observation_for_testing() -> Self {
        Self::DevelopmentSingleObservation
    }

    /// Create a real-backend member-quorum policy.
    #[must_use]
    pub fn member_quorum(required: u16) -> Self {
        Self::MemberQuorum { required }
    }

    /// Return true when one accepted observation may finalize quote state.
    #[must_use]
    pub fn permits_single_observation_finality(self) -> bool {
        matches!(self, Self::DevelopmentSingleObservation)
    }

    /// Number of matching observations required by this policy.
    #[must_use]
    pub fn required_matching_observations(self) -> u16 {
        match self {
            Self::DevelopmentSingleObservation => 1,
            Self::MemberQuorum { required, .. } => required,
        }
    }

    /// Validate an accumulated observation quorum against this policy.
    pub fn evaluate_quorum(self, quorum: &FederationPaymentObservationQuorum) -> Result<()> {
        let required = self.required_matching_observations();
        if quorum.matching_count() < required {
            return Err(FederationError::PaymentObservationQuorumNotSatisfied {
                context: quorum.context,
                required,
                found: quorum.matching_count(),
            });
        }

        Ok(())
    }

    fn validate(
        self,
        context: FederationPaymentObservationContext,
        threshold_params: ThresholdParams,
    ) -> Result<()> {
        match self {
            Self::DevelopmentSingleObservation => Ok(()),
            Self::MemberQuorum { required, .. } => {
                if required == 0
                    || required > threshold_params.member_count
                    || required < threshold_params.consensus_threshold
                {
                    return Err(FederationError::InvalidPaymentObservationQuorum {
                        context,
                        required,
                        members: threshold_params.member_count,
                        consensus_threshold: threshold_params.consensus_threshold,
                    });
                }

                Ok(())
            }
        }
    }
}

/// Public configuration for one federation member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberConfigPublic {
    /// Stable federation member ID.
    pub member_id: MemberId,
    /// Public Cashu mint REST URL exposed to wallets.
    pub public_mint_url: MintUrl,
    /// Private federation REST URL exposed to federation peers.
    pub federation_api_url: MintUrl,
    /// Public secp256k1 identity key used to authenticate peer traffic.
    pub identity_public_key: PublicKey,
}

impl MemberConfigPublic {
    /// Validate this member against its map key.
    pub fn validate(&self, expected_member_id: MemberId) -> Result<()> {
        if self.member_id != expected_member_id {
            return Err(FederationError::MemberIdMismatch {
                expected: expected_member_id,
                found: self.member_id,
            });
        }

        if !matches!(self.identity_public_key, PublicKey::Secp256k1(_)) {
            return Err(FederationError::InvalidIdentityKeyKind {
                member_id: self.member_id,
            });
        }

        Ok(())
    }
}

/// Public aggregate keyset plus every member's public BLS shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedKeyset {
    /// Cashu keyset ID derived from the aggregate public keys.
    pub id: Id,
    /// Keyset currency unit.
    pub unit: CurrencyUnit,
    /// Whether members should sign new outputs for this keyset.
    pub active: bool,
    /// Input fee in parts per thousand.
    pub input_fee_ppk: u64,
    /// Optional final expiry timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_expiry: Option<u64>,
    /// Aggregate BLS public key per amount, exposed to wallets as the mint keyset.
    pub aggregate_keys: BTreeMap<Amount, BlsAggregatePublicKey>,
    /// Public BLS share per member and amount.
    pub public_shares: BTreeMap<MemberId, BTreeMap<Amount, BlsPublicKeyShare>>,
}

impl FederatedKeyset {
    /// Create a federated keyset with a computed NUT-02 v3 keyset ID.
    pub fn new(
        unit: CurrencyUnit,
        active: bool,
        input_fee_ppk: u64,
        final_expiry: Option<u64>,
        aggregate_keys: BTreeMap<Amount, BlsAggregatePublicKey>,
        public_shares: BTreeMap<MemberId, BTreeMap<Amount, BlsPublicKeyShare>>,
    ) -> Self {
        let id = Self::derive_keyset_id(&unit, input_fee_ppk, final_expiry, &aggregate_keys);
        Self {
            id,
            unit,
            active,
            input_fee_ppk,
            final_expiry,
            aggregate_keys,
            public_shares,
        }
    }

    /// Derive a NUT-02 v3 keyset ID from aggregate BLS public keys.
    #[must_use]
    pub fn derive_keyset_id(
        unit: &CurrencyUnit,
        input_fee_ppk: u64,
        final_expiry: Option<u64>,
        aggregate_keys: &BTreeMap<Amount, BlsAggregatePublicKey>,
    ) -> Id {
        let keys = Self::aggregate_keys_as_cashu_keys(aggregate_keys);
        Id::v3_from_data(&keys, unit, input_fee_ppk, final_expiry)
    }

    /// Convert aggregate BLS public keys into a standard Cashu `Keys` map.
    #[must_use]
    pub fn aggregate_keys_as_cashu_keys(
        aggregate_keys: &BTreeMap<Amount, BlsAggregatePublicKey>,
    ) -> Keys {
        Keys::new(
            aggregate_keys
                .iter()
                .map(|(amount, key)| (*amount, PublicKey::from(*key)))
                .collect(),
        )
    }

    /// Return aggregate public keys as a standard Cashu `Keys` map.
    #[must_use]
    pub fn keys(&self) -> Keys {
        Self::aggregate_keys_as_cashu_keys(&self.aggregate_keys)
    }

    /// Validate aggregate keys, keyset ID, member shares, and interpolation.
    pub fn validate(
        &self,
        threshold_params: ThresholdParams,
        members: &BTreeMap<MemberId, MemberConfigPublic>,
    ) -> Result<()> {
        #[cfg(feature = "prometheus")]
        {
            cdk_prometheus::METRICS.record_federation_performance_work(
                cdk_prometheus::FederationPerformanceWork::KeysetValidationAttempts,
                1,
            );
        }
        #[cfg(feature = "prometheus")]
        let started_at = Instant::now();
        let result = self.validate_inner(threshold_params, members);
        #[cfg(feature = "prometheus")]
        {
            if result.is_err() {
                cdk_prometheus::METRICS.record_federation_performance_work(
                    cdk_prometheus::FederationPerformanceWork::KeysetValidationFailures,
                    1,
                );
            }
            cdk_prometheus::METRICS.record_federation_performance_operation(
                cdk_prometheus::FederationPerformanceOperation::KeysetValidation,
                cdk_prometheus::FederationPerformanceOutcome::from_success(result.is_ok()),
                started_at.elapsed().as_secs_f64(),
            );
        }
        result
    }

    fn validate_inner(
        &self,
        threshold_params: ThresholdParams,
        members: &BTreeMap<MemberId, MemberConfigPublic>,
    ) -> Result<()> {
        self.validate_metadata(members)?;

        for (amount, aggregate_key) in &self.aggregate_keys {
            let mut shares = Vec::with_capacity(members.len());
            for member_id in members.keys() {
                let member_shares = self.public_shares.get(member_id).ok_or(
                    FederationError::MissingPublicShares {
                        keyset_id: self.id,
                        member_id: *member_id,
                    },
                )?;
                let share =
                    member_shares
                        .get(amount)
                        .ok_or(FederationError::MissingPublicShareAmount {
                            keyset_id: self.id,
                            member_id: *member_id,
                            amount: *amount,
                        })?;
                shares.push(*share);
            }

            #[cfg(feature = "prometheus")]
            {
                cdk_prometheus::METRICS.record_federation_performance_work(
                    cdk_prometheus::FederationPerformanceWork::AggregateKeyInterpolationAttempts,
                    1,
                );
            }
            #[cfg(feature = "prometheus")]
            let interpolation_started_at = Instant::now();
            let derived = aggregate_public_key_shares(
                usize::from(threshold_params.signature_threshold),
                &shares,
            );
            #[cfg(feature = "prometheus")]
            {
                if derived.is_err() {
                    cdk_prometheus::METRICS.record_federation_performance_work(
                        cdk_prometheus::FederationPerformanceWork::AggregateKeyInterpolationFailures,
                        1,
                    );
                }
                cdk_prometheus::METRICS.record_federation_performance_operation(
                    cdk_prometheus::FederationPerformanceOperation::AggregateKeyInterpolation,
                    cdk_prometheus::FederationPerformanceOutcome::from_success(derived.is_ok()),
                    interpolation_started_at.elapsed().as_secs_f64(),
                );
            }
            let derived = derived?;
            if derived != *aggregate_key {
                return Err(FederationError::AggregateKeyMismatch {
                    keyset_id: self.id,
                    amount: *amount,
                });
            }
        }

        Ok(())
    }

    pub(super) fn validate_metadata(
        &self,
        members: &BTreeMap<MemberId, MemberConfigPublic>,
    ) -> Result<()> {
        if self.aggregate_keys.is_empty() {
            return Err(FederationError::EmptyFederatedKeyset { keyset_id: self.id });
        }
        if self.id.get_version() != KeySetVersion::Version02 {
            return Err(FederationError::InvalidFederatedKeysetVersion { keyset_id: self.id });
        }

        let expected_id = Self::derive_keyset_id(
            &self.unit,
            self.input_fee_ppk,
            self.final_expiry,
            &self.aggregate_keys,
        );
        if expected_id != self.id {
            return Err(FederationError::KeysetIdMismatch {
                expected: expected_id,
                found: self.id,
            });
        }

        for member_id in self.public_shares.keys() {
            if !members.contains_key(member_id) {
                return Err(FederationError::UnknownMember {
                    member_id: *member_id,
                });
            }
        }

        for member_id in members.keys() {
            let member_shares =
                self.public_shares
                    .get(member_id)
                    .ok_or(FederationError::MissingPublicShares {
                        keyset_id: self.id,
                        member_id: *member_id,
                    })?;

            for amount in self.aggregate_keys.keys() {
                let share =
                    member_shares
                        .get(amount)
                        .ok_or(FederationError::MissingPublicShareAmount {
                            keyset_id: self.id,
                            member_id: *member_id,
                            amount: *amount,
                        })?;
                if share.signer_id().get() != member_id.get() {
                    return Err(FederationError::PublicShareSignerMismatch {
                        keyset_id: self.id,
                        member_id: *member_id,
                        signer_id: share.signer_id().get(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Validate that one member controls the local secret shares for this rotation keyset.
    ///
    /// This is an activation-time proof gate: the member signs a deterministic,
    /// domain-separated challenge for each amount with its local BLS secret share
    /// and immediately verifies the proof against the published public share.
    pub fn validate_member_rotation_proof_of_possession(
        &self,
        federation_id: FederationId,
        member_id: MemberId,
        secret_shares: &BTreeMap<Amount, BlsSecretKeyShare>,
    ) -> Result<()> {
        let public_member_shares =
            self.public_shares
                .get(&member_id)
                .ok_or(FederationError::MissingPublicShares {
                    keyset_id: self.id,
                    member_id,
                })?;

        for (amount, aggregate_key) in &self.aggregate_keys {
            let secret_share =
                secret_shares
                    .get(amount)
                    .ok_or(FederationError::MissingSecretShare {
                        keyset_id: self.id,
                        amount: *amount,
                    })?;
            if secret_share.signer_id().get() != member_id.get() {
                return Err(FederationError::SecretShareSignerMismatch {
                    keyset_id: self.id,
                    member_id,
                    signer_id: secret_share.signer_id().get(),
                });
            }

            let public_share = public_member_shares.get(amount).ok_or(
                FederationError::MissingPublicShareAmount {
                    keyset_id: self.id,
                    member_id,
                    amount: *amount,
                },
            )?;
            if secret_share.public_key_share() != *public_share {
                return Err(FederationError::PrivateShareMismatch {
                    keyset_id: self.id,
                    member_id,
                    amount: *amount,
                });
            }

            let challenge = self.rotation_proof_challenge(
                federation_id,
                member_id,
                *amount,
                aggregate_key,
                public_share,
            );
            let proof = secret_share.blind_sign_share(&challenge);
            let proof_valid = verify_blind_signature_share(&proof, &challenge, public_share)
                .map_err(|_| FederationError::KeysetProofOfPossessionFailed {
                    keyset_id: self.id,
                    member_id,
                    amount: *amount,
                })?;
            if !proof_valid {
                return Err(FederationError::KeysetProofOfPossessionFailed {
                    keyset_id: self.id,
                    member_id,
                    amount: *amount,
                });
            }
        }

        Ok(())
    }

    fn rotation_proof_challenge(
        &self,
        federation_id: FederationId,
        member_id: MemberId,
        amount: Amount,
        aggregate_key: &BlsAggregatePublicKey,
        public_share: &BlsPublicKeyShare,
    ) -> BlsG1PublicKey {
        let mut transcript = Vec::new();
        append_bytes(&mut transcript, FEDERATION_KEYSET_ROTATION_PROOF_VERSION);
        append_bytes(&mut transcript, &federation_id.to_bytes());
        append_str(&mut transcript, &self.id.to_string());
        append_str(&mut transcript, &self.unit.to_string());
        append_bytes(&mut transcript, &self.input_fee_ppk.to_be_bytes());
        match self.final_expiry {
            Some(expiry) => {
                append_bytes(&mut transcript, &[1]);
                append_bytes(&mut transcript, &expiry.to_be_bytes());
            }
            None => append_bytes(&mut transcript, &[0]),
        }
        append_bytes(&mut transcript, &member_id.get().to_be_bytes());
        append_str(&mut transcript, &amount.to_string());
        append_bytes(&mut transcript, &aggregate_key.to_bytes());
        append_bytes(&mut transcript, &public_share.to_bytes());

        BlsG1PublicKey::hash_to_curve(&transcript)
    }

    fn validate_advertised_by_member(
        &self,
        member_id: MemberId,
        advertised: &KeySet,
    ) -> Result<()> {
        if advertised.id.get_version() != KeySetVersion::Version02 {
            return Err(FederationError::InvalidFederatedKeysetVersion {
                keyset_id: advertised.id,
            });
        }

        advertised.verify_id()?;
        if advertised.id != self.id
            || advertised.unit != self.unit
            || advertised.input_fee_ppk != self.input_fee_ppk
            || advertised.final_expiry != self.final_expiry
            || advertised.keys != self.keys()
        {
            return Err(FederationError::AdvertisedFederatedKeysetMismatch {
                member_id,
                keyset_id: self.id,
            });
        }
        if let Some(active) = advertised.active {
            if active != self.active {
                return Err(FederationError::AdvertisedFederatedKeysetMismatch {
                    member_id,
                    keyset_id: self.id,
                });
            }
        }

        Ok(())
    }
}

/// Public federation config shared by every member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationConfig {
    /// Stable federation roster ID.
    pub federation_id: FederationId,
    /// Wallet-facing federation protocol required by this public config.
    pub wallet_protocol_version: u16,
    /// Federation threshold settings.
    pub threshold_params: ThresholdParams,
    /// Public member roster keyed by member ID.
    pub members: BTreeMap<MemberId, MemberConfigPublic>,
    /// Federated aggregate keysets.
    pub keysets: Vec<FederatedKeyset>,
    /// Signed public setup transcript for audit and later DKG compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_transcript: Option<FederationDkgTranscript>,
    /// Policy for mint and melt payment observations.
    pub payment_observation_policy: FederationPaymentObservationPolicy,
    /// Consensus-configured scheduled-checkpoint intervals.
    #[serde(default)]
    pub checkpoint_policy: FederationCheckpointPolicy,
    /// Optional reusable FROST root and federated on-chain wallet configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frost_wallet: Option<FederationFrostWalletConfig>,
}

impl FederationConfig {
    /// Create a federation config with consensus-threshold payment quorum and compute its ID.
    ///
    /// Development setup must replace the payment policy explicitly before signing the config.
    pub fn new(
        threshold_params: ThresholdParams,
        members: BTreeMap<MemberId, MemberConfigPublic>,
        keysets: Vec<FederatedKeyset>,
    ) -> Result<Self> {
        let federation_id = Self::derive_federation_id(threshold_params, &members)?;
        let config = Self {
            federation_id,
            wallet_protocol_version: FEDERATION_WALLET_PROTOCOL_VERSION,
            threshold_params,
            members,
            keysets,
            setup_transcript: None,
            payment_observation_policy: FederationPaymentObservationPolicy::member_quorum(
                threshold_params.consensus_threshold,
            ),
            checkpoint_policy: FederationCheckpointPolicy::default(),
            frost_wallet: None,
        };
        config.validate()?;
        Ok(config)
    }

    /// Derive the deterministic federation ID from threshold params and roster.
    pub fn derive_federation_id(
        threshold_params: ThresholdParams,
        members: &BTreeMap<MemberId, MemberConfigPublic>,
    ) -> Result<FederationId> {
        threshold_params.validate()?;
        if usize::from(threshold_params.member_count) != members.len() {
            return Err(FederationError::MemberCountMismatch {
                expected: threshold_params.member_count,
                found: members.len(),
            });
        }

        let mut transcript = Vec::new();
        append_bytes(&mut transcript, FEDERATION_ROSTER_HASH_VERSION);
        transcript.extend_from_slice(&threshold_params.member_count.to_be_bytes());
        transcript.extend_from_slice(&threshold_params.signature_threshold.to_be_bytes());
        transcript.extend_from_slice(&threshold_params.consensus_threshold.to_be_bytes());

        for (member_id, member) in members {
            member.validate(*member_id)?;
            transcript.extend_from_slice(&member_id.get().to_be_bytes());
            append_str(&mut transcript, &member.public_mint_url.to_string());
            append_str(&mut transcript, &member.federation_api_url.to_string());
            append_bytes(&mut transcript, &member.identity_public_key.to_bytes());
        }

        Ok(FederationId::from_bytes(
            Sha256Hash::hash(&transcript).to_byte_array(),
        ))
    }

    /// Validate this config.
    pub fn validate(&self) -> Result<()> {
        #[cfg(feature = "prometheus")]
        {
            cdk_prometheus::METRICS.record_federation_performance_work(
                cdk_prometheus::FederationPerformanceWork::ConfigValidationAttempts,
                1,
            );
        }
        #[cfg(feature = "prometheus")]
        let started_at = Instant::now();
        let result = self.validate_inner();
        #[cfg(feature = "prometheus")]
        {
            if result.is_err() {
                cdk_prometheus::METRICS.record_federation_performance_work(
                    cdk_prometheus::FederationPerformanceWork::ConfigValidationFailures,
                    1,
                );
            }
            cdk_prometheus::METRICS.record_federation_performance_operation(
                cdk_prometheus::FederationPerformanceOperation::FullConfigValidation,
                cdk_prometheus::FederationPerformanceOutcome::from_success(result.is_ok()),
                started_at.elapsed().as_secs_f64(),
            );
        }
        result
    }

    fn validate_inner(&self) -> Result<()> {
        validate_protocol_component(
            "wallet",
            FEDERATION_WALLET_PROTOCOL_VERSION,
            self.wallet_protocol_version,
        )?;

        let expected_id = Self::derive_federation_id(self.threshold_params, &self.members)?;
        if expected_id != self.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: expected_id,
                found: self.federation_id,
            });
        }

        for keyset in &self.keysets {
            keyset.validate(self.threshold_params, &self.members)?;
        }

        if let Some(transcript) = &self.setup_transcript {
            transcript.validate_setup_lineage_against_config(self)?;
        }

        self.payment_observation_policy
            .validate(self.threshold_params)?;
        self.checkpoint_policy.validate()?;
        if let Some(wallet) = &self.frost_wallet {
            wallet.validate(self)?;
        }

        Ok(())
    }

    /// Validate config metadata needed by an already-imported wallet runtime.
    ///
    /// This keeps cheap structural, version, roster, keyset ID, member-share
    /// presence, and payment-policy checks, but skips expensive threshold BLS
    /// aggregate-key interpolation and setup-transcript proof validation. New
    /// external federation configs should still use [`Self::validate`] at
    /// import or mint startup.
    pub fn validate_wallet_runtime_metadata(&self) -> Result<()> {
        validate_protocol_component(
            "wallet",
            FEDERATION_WALLET_PROTOCOL_VERSION,
            self.wallet_protocol_version,
        )?;

        let expected_id = Self::derive_federation_id(self.threshold_params, &self.members)?;
        if expected_id != self.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: expected_id,
                found: self.federation_id,
            });
        }

        for keyset in &self.keysets {
            keyset.validate_metadata(&self.members)?;
        }

        self.payment_observation_policy
            .validate(self.threshold_params)?;
        self.checkpoint_policy.validate()?;
        if let Some(wallet) = &self.frost_wallet {
            wallet.validate(self)?;
        }

        Ok(())
    }

    /// Compute a digest of the complete public federation config.
    pub fn config_digest(&self) -> Result<FederationConfigDigest> {
        self.validate()?;
        self.config_digest_for_validated_config()
    }

    /// Compute a digest of an already-validated public federation config.
    ///
    /// This avoids repeating threshold BLS key interpolation on hot paths where
    /// the caller has already validated the config during construction or
    /// registry loading.
    pub fn config_digest_for_validated_config(&self) -> Result<FederationConfigDigest> {
        let config = serde_json::to_vec(self)?;
        let mut transcript = Vec::new();
        append_bytes(&mut transcript, FEDERATION_CONFIG_DIGEST_VERSION);
        append_bytes(&mut transcript, &config);
        Ok(FederationConfigDigest::from_bytes(
            Sha256Hash::hash(&transcript).to_byte_array(),
        ))
    }

    /// Build the mint-info federation advertisement expected from a member.
    pub fn mint_info_for_member(&self, member_id: MemberId) -> Result<FederationInfo> {
        let config_digest = self.config_digest()?;
        self.mint_info_for_member_with_config_digest(member_id, config_digest)
    }

    /// Build a member advertisement with the digest of an already-validated config.
    pub fn mint_info_for_member_with_config_digest(
        &self,
        member_id: MemberId,
        config_digest: FederationConfigDigest,
    ) -> Result<FederationInfo> {
        if !self.members.contains_key(&member_id) {
            return Err(FederationError::UnknownMember { member_id });
        }

        Ok(FederationInfo::new(
            self.federation_id.to_string(),
            member_id.get(),
            config_digest.to_string(),
            self.wallet_protocol_version,
        ))
    }

    /// Validate a member's mint-info federation advertisement.
    pub fn validate_member_mint_info(
        &self,
        expected_member_id: MemberId,
        info: &FederationInfo,
    ) -> Result<()> {
        let expected_digest = self.config_digest()?;
        self.validate_member_mint_info_with_config_digest(expected_member_id, info, expected_digest)
    }

    /// Validate a member's mint-info federation advertisement using a known config digest.
    ///
    /// Callers should use this after validating the config and computing its
    /// digest once for the surrounding operation.
    pub fn validate_member_mint_info_with_config_digest(
        &self,
        expected_member_id: MemberId,
        info: &FederationInfo,
        expected_digest: FederationConfigDigest,
    ) -> Result<()> {
        if !self.members.contains_key(&expected_member_id) {
            return Err(FederationError::UnknownMember {
                member_id: expected_member_id,
            });
        }

        let found_member_id = MemberId::new(info.member_id)?;
        if found_member_id != expected_member_id {
            return Err(FederationError::MemberIdMismatch {
                expected: expected_member_id,
                found: found_member_id,
            });
        }

        let found_federation_id = FederationId::from_str(&info.federation_id)?;
        if found_federation_id != self.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: self.federation_id,
                found: found_federation_id,
            });
        }

        let found_digest = FederationConfigDigest::from_str(&info.config_digest)?;
        if found_digest != expected_digest {
            return Err(FederationError::FederationConfigDigestMismatch {
                member_id: expected_member_id,
                expected: expected_digest,
                found: found_digest,
            });
        }

        validate_protocol_component(
            "wallet",
            self.wallet_protocol_version,
            info.wallet_protocol_version,
        )?;

        Ok(())
    }

    /// Return a validated config view after applying an accepted keyset rotation.
    pub fn apply_keyset_rotation(
        &self,
        operation: &FederationKeysetRotationOperation,
    ) -> Result<Self> {
        operation.validate_against_config(self)?;

        let mut rotated = self.clone();
        for keyset in &mut rotated.keysets {
            if keyset.unit == operation.keyset.unit {
                keyset.active = false;
            }
        }

        match rotated
            .keysets
            .iter()
            .position(|keyset| keyset.id == operation.keyset.id)
        {
            Some(index) => rotated.keysets[index] = operation.keyset.clone(),
            None => rotated.keysets.push(operation.keyset.clone()),
        }

        rotated.validate()?;
        Ok(rotated)
    }

    /// Validate that blinded outputs target active v3/BLS aggregate keysets.
    pub fn validate_blinded_outputs(&self, outputs: &[BlindedMessage]) -> Result<()> {
        for output in outputs {
            self.validate_blinded_output(output)?;
        }

        Ok(())
    }

    /// Validate melt change outputs, allowing zero-amount change outputs.
    pub fn validate_blinded_change_outputs(&self, outputs: &[BlindedMessage]) -> Result<()> {
        for output in outputs {
            self.validate_blinded_change_output(output)?;
        }

        Ok(())
    }

    /// Validate restore outputs without requiring the keyset to still be active.
    pub fn validate_blinded_restore_outputs(&self, outputs: &[BlindedMessage]) -> Result<()> {
        for output in outputs {
            self.validate_blinded_restore_output(output)?;
        }

        Ok(())
    }

    /// Validate one blinded output and return the aggregate keyset it targets.
    pub fn validate_blinded_output(&self, output: &BlindedMessage) -> Result<&FederatedKeyset> {
        self.validate_blinded_output_with_amount_policy(output, false, true)
    }

    /// Validate one melt change output and return the aggregate keyset it targets.
    pub fn validate_blinded_change_output(
        &self,
        output: &BlindedMessage,
    ) -> Result<&FederatedKeyset> {
        self.validate_blinded_output_with_amount_policy(output, true, true)
    }

    /// Validate one restore output and return the aggregate keyset it targets.
    pub fn validate_blinded_restore_output(
        &self,
        output: &BlindedMessage,
    ) -> Result<&FederatedKeyset> {
        self.validate_blinded_output_with_amount_policy(output, true, false)
    }

    fn validate_blinded_output_with_amount_policy(
        &self,
        output: &BlindedMessage,
        allow_zero_amount_change_output: bool,
        require_active_keyset: bool,
    ) -> Result<&FederatedKeyset> {
        if output.keyset_id.get_version() != KeySetVersion::Version02 {
            return Err(FederationError::InvalidFederatedKeysetVersion {
                keyset_id: output.keyset_id,
            });
        }

        let keyset = self
            .keysets
            .iter()
            .find(|keyset| keyset.id == output.keyset_id)
            .ok_or(FederationError::UnknownFederatedKeyset {
                keyset_id: output.keyset_id,
            })?;
        if require_active_keyset && !keyset.active {
            return Err(FederationError::InactiveFederatedKeyset {
                keyset_id: keyset.id,
            });
        }
        if (output.amount != Amount::ZERO || !allow_zero_amount_change_output)
            && !keyset.aggregate_keys.contains_key(&output.amount)
        {
            return Err(FederationError::UnknownFederatedKeysetAmount {
                keyset_id: keyset.id,
                amount: output.amount,
            });
        }
        if output.amount == Amount::ZERO
            && allow_zero_amount_change_output
            && keyset.aggregate_keys.is_empty()
        {
            return Err(FederationError::UnknownFederatedKeysetAmount {
                keyset_id: keyset.id,
                amount: output.amount,
            });
        }
        output.blinded_secret.as_bls_g1().map_err(|_| {
            FederationError::InvalidFederationBlindedSecret {
                keyset_id: output.keyset_id,
                amount: output.amount,
            }
        })?;

        Ok(keyset)
    }

    /// Validate keysets advertised by one federation member to a wallet.
    pub fn validate_member_advertised_keysets(
        &self,
        member_id: MemberId,
        advertised_keysets: &[KeySet],
    ) -> Result<()> {
        self.validate()?;
        if !self.members.contains_key(&member_id) {
            return Err(FederationError::UnknownMember { member_id });
        }

        for advertised in advertised_keysets {
            if advertised.id.get_version() != KeySetVersion::Version02 {
                return Err(FederationError::InvalidFederatedKeysetVersion {
                    keyset_id: advertised.id,
                });
            }
            let keyset = self
                .keysets
                .iter()
                .find(|keyset| keyset.id == advertised.id)
                .ok_or(FederationError::UnknownFederatedKeyset {
                    keyset_id: advertised.id,
                })?;
            keyset.validate_advertised_by_member(member_id, advertised)?;
        }

        for keyset in &self.keysets {
            if !advertised_keysets
                .iter()
                .any(|advertised| advertised.id == keyset.id)
            {
                return Err(FederationError::MissingAdvertisedFederatedKeyset {
                    member_id,
                    keyset_id: keyset.id,
                });
            }
        }

        Ok(())
    }

    /// Return the payment verification policy for one observation context.
    #[must_use]
    pub fn payment_verification_policy(
        &self,
        context: FederationPaymentObservationContext,
    ) -> &FederationPaymentVerificationPolicy {
        self.payment_observation_policy.policy_for(context)
    }

    /// Return a member by ID.
    #[must_use]
    pub fn member(&self, member_id: MemberId) -> Option<&MemberConfigPublic> {
        self.members.get(&member_id)
    }
}

/// Private federation config stored by one local member.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberConfigPrivate {
    /// Final federation roster hash this private config belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub federation_id: Option<FederationId>,
    /// Local member ID.
    pub member_id: MemberId,
    /// Private secp256k1 identity key used to authenticate peer traffic.
    pub identity_secret_key: SecretKey,
    /// Local secret BLS shares by keyset ID and amount.
    pub keyset_secret_shares: BTreeMap<Id, BTreeMap<Amount, BlsSecretKeyShare>>,
    /// Hash of the public setup transcript used to derive this member's shares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_transcript_hash: Option<FederationDkgTranscriptHash>,
}

impl fmt::Debug for MemberConfigPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemberConfigPrivate")
            .field("federation_id", &self.federation_id)
            .field("member_id", &self.member_id)
            .field("keyset_secret_shares", &self.keyset_secret_shares.keys())
            .field("setup_transcript_hash", &self.setup_transcript_hash)
            .finish_non_exhaustive()
    }
}

impl MemberConfigPrivate {
    /// Validate identity and transcript metadata against the public federation config.
    pub(crate) fn validate_metadata_against_public(
        &self,
        public_config: &FederationConfig,
    ) -> Result<()> {
        let member =
            public_config
                .member(self.member_id)
                .ok_or(FederationError::MissingMember {
                    member_id: self.member_id,
                })?;

        if !matches!(self.identity_secret_key, SecretKey::Secp256k1(_)) {
            return Err(FederationError::InvalidIdentityKeyKind {
                member_id: self.member_id,
            });
        }

        if self.identity_secret_key.public_key() != member.identity_public_key {
            return Err(FederationError::PrivateIdentityMismatch {
                member_id: self.member_id,
            });
        }

        match self.federation_id {
            Some(federation_id) if federation_id != public_config.federation_id => {
                return Err(FederationError::FederationIdMismatch {
                    expected: public_config.federation_id,
                    found: federation_id,
                });
            }
            None if public_config.setup_transcript.is_some() => {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: "private config missing federation id".to_string(),
                });
            }
            Some(_) | None => {}
        }

        match (&public_config.setup_transcript, self.setup_transcript_hash) {
            (Some(transcript), Some(hash)) if transcript.transcript_hash != hash => {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: "private setup transcript hash mismatch".to_string(),
                });
            }
            (Some(_), None) => {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: "private config missing setup transcript hash".to_string(),
                });
            }
            (None, Some(_)) => {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: "private config has setup transcript hash but public config has no transcript"
                        .to_string(),
                });
            }
            (Some(_), Some(_)) | (None, None) => {}
        }

        Ok(())
    }

    /// Validate this private config against the public federation config.
    pub fn validate_against_public(&self, public_config: &FederationConfig) -> Result<()> {
        self.validate_metadata_against_public(public_config)?;

        for keyset_id in self.keyset_secret_shares.keys() {
            if public_config
                .keysets
                .iter()
                .all(|keyset| keyset.id != *keyset_id)
            {
                return Err(FederationError::UnknownPrivateKeyset {
                    keyset_id: *keyset_id,
                });
            }
        }

        for keyset in &public_config.keysets {
            let secret_shares = self.keyset_secret_shares.get(&keyset.id).ok_or(
                FederationError::MissingPrivateKeyset {
                    keyset_id: keyset.id,
                },
            )?;
            let public_member_shares = keyset.public_shares.get(&self.member_id).ok_or(
                FederationError::MissingPublicShares {
                    keyset_id: keyset.id,
                    member_id: self.member_id,
                },
            )?;

            for amount in keyset.aggregate_keys.keys() {
                let secret_share =
                    secret_shares
                        .get(amount)
                        .ok_or(FederationError::MissingSecretShare {
                            keyset_id: keyset.id,
                            amount: *amount,
                        })?;
                if secret_share.signer_id().get() != self.member_id.get() {
                    return Err(FederationError::SecretShareSignerMismatch {
                        keyset_id: keyset.id,
                        member_id: self.member_id,
                        signer_id: secret_share.signer_id().get(),
                    });
                }

                let public_share = public_member_shares.get(amount).ok_or(
                    FederationError::MissingPublicShareAmount {
                        keyset_id: keyset.id,
                        member_id: self.member_id,
                        amount: *amount,
                    },
                )?;
                if secret_share.public_key_share() != *public_share {
                    return Err(FederationError::PrivateShareMismatch {
                        keyset_id: keyset.id,
                        member_id: self.member_id,
                        amount: *amount,
                    });
                }
            }
        }

        Ok(())
    }
}

/// Portable wallet bootstrap artifact for a static federation.
///
/// The invite intentionally contains only the public federation config. Wallets
/// derive member HTTP connectors from the public mint URLs already committed to
/// that config, then validate aggregate keysets against live member responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationInvite {
    /// Invite payload version.
    pub version: u16,
    /// Public federation config shared by every member.
    pub config: FederationConfig,
}

impl FederationInvite {
    /// Build a validated federation invite from a public config.
    pub fn new(config: FederationConfig) -> Result<Self> {
        let invite = Self {
            version: FEDERATION_INVITE_VERSION,
            config,
        };
        invite.validate()?;
        Ok(invite)
    }

    /// Return the public federation config carried by this invite.
    #[must_use]
    pub fn public_config(&self) -> &FederationConfig {
        &self.config
    }

    /// Validate the invite payload and its public config.
    pub fn validate(&self) -> Result<()> {
        if self.version != FEDERATION_INVITE_VERSION {
            return Err(FederationError::FederationInviteVersionMismatch {
                expected: FEDERATION_INVITE_VERSION,
                found: self.version,
            });
        }
        self.config.validate()
    }

    /// Encode this invite as a versioned, URL-safe invite code.
    pub fn to_invite_code(&self) -> Result<String> {
        self.validate()?;
        let json =
            serde_json::to_vec(self).map_err(|err| FederationError::InvalidFederationInvite {
                reason: err.to_string(),
            })?;
        let encoded = general_purpose::URL_SAFE_NO_PAD.encode(json);
        Ok(format!("{FEDERATION_INVITE_PREFIX}{encoded}"))
    }
}

impl fmt::Display for FederationInvite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let invite_code = self.to_invite_code().map_err(|_| fmt::Error)?;
        f.write_str(&invite_code)
    }
}

impl FromStr for FederationInvite {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        let payload = value
            .strip_prefix(FEDERATION_INVITE_PREFIX)
            .ok_or_else(|| FederationError::InvalidFederationInvite {
                reason: "unsupported invite prefix".to_string(),
            })?;

        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(bitcoin::base64::engine::DecodePaddingMode::Indifferent);
        let decoded = GeneralPurpose::new(&alphabet::URL_SAFE, decode_config)
            .decode(payload)
            .map_err(|err| FederationError::InvalidFederationInvite {
                reason: format!("payload is not URL-safe base64: {err}"),
            })?;
        let invite = serde_json::from_slice::<Self>(&decoded).map_err(|err| {
            FederationError::InvalidFederationInvite {
                reason: err.to_string(),
            }
        })?;
        invite.validate()?;
        Ok(invite)
    }
}

/// Durable public federation config snapshot.
///
/// This snapshot is local member startup state. It stores the public federation
/// config and a digest so a member does not accidentally reuse a database with
/// a different federation roster, keyset set, policy, or protocol version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationConfigSnapshot {
    /// Federation ID derived from the static roster.
    pub federation_id: FederationId,
    /// Digest over the complete public config.
    pub config_digest: FederationConfigDigest,
    /// Public config shared by federation members.
    pub config: FederationConfig,
}

impl FederationConfigSnapshot {
    /// Build a validated durable snapshot from a public federation config.
    pub fn new(config: FederationConfig) -> Result<Self> {
        let config_digest = config.config_digest()?;
        Ok(Self {
            federation_id: config.federation_id,
            config_digest,
            config,
        })
    }
}
