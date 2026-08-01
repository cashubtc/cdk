//! Trusted-dealer federation config generation.
//!
//! This is the mocked DKG path used while the federation protocol is built out:
//! one process creates BLS threshold shares locally and writes a public
//! federation config plus one private member config for each mint.

use core::fmt;
use std::collections::BTreeMap;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;

use crate::nuts::nut01::{trusted_dealer_keygen, BlsSecretKey};
use crate::{mint_url::MintUrl, Amount, CurrencyUnit, SecretKey};

use super::{
    FederatedKeyset, FederationCheckpointPolicy, FederationConfig, FederationDkgTranscript,
    FederationError, FederationPaymentObservationPolicy, FederationSetupAuthorizationHash,
    MemberConfigPrivate, MemberConfigPublic, MemberId, Result, ThresholdParams,
};

/// Member input for trusted-dealer config generation.
#[derive(Clone, PartialEq, Eq)]
pub struct TrustedDealerMember {
    /// Stable federation member ID.
    pub member_id: MemberId,
    /// Public Cashu mint REST URL exposed to wallets.
    pub public_mint_url: MintUrl,
    /// Private federation REST URL exposed to federation peers.
    pub federation_api_url: MintUrl,
    /// Private secp256k1 identity key for this member.
    pub identity_secret_key: SecretKey,
}

impl fmt::Debug for TrustedDealerMember {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrustedDealerMember")
            .field("member_id", &self.member_id)
            .field("public_mint_url", &self.public_mint_url)
            .field("federation_api_url", &self.federation_api_url)
            .finish_non_exhaustive()
    }
}

impl TrustedDealerMember {
    /// Create a trusted-dealer member input.
    #[must_use]
    pub fn new(
        member_id: MemberId,
        public_mint_url: MintUrl,
        federation_api_url: MintUrl,
        identity_secret_key: SecretKey,
    ) -> Self {
        Self {
            member_id,
            public_mint_url,
            federation_api_url,
            identity_secret_key,
        }
    }
}

/// Keyset input for trusted-dealer config generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedDealerKeysetSpec {
    /// Keyset currency unit.
    pub unit: CurrencyUnit,
    /// Whether generated members should sign new outputs for this keyset.
    pub active: bool,
    /// Input fee in parts per thousand.
    pub input_fee_ppk: u64,
    /// Optional final expiry timestamp.
    pub final_expiry: Option<u64>,
    /// Polynomial coefficients per amount.
    ///
    /// For each amount, `coefficients[0]` is the aggregate mint secret and the
    /// vector length must equal the signature threshold.
    pub amount_coefficients: BTreeMap<Amount, Vec<BlsSecretKey>>,
}

impl TrustedDealerKeysetSpec {
    /// Create a trusted-dealer keyset input.
    #[must_use]
    pub fn new(
        unit: CurrencyUnit,
        active: bool,
        input_fee_ppk: u64,
        final_expiry: Option<u64>,
        amount_coefficients: BTreeMap<Amount, Vec<BlsSecretKey>>,
    ) -> Self {
        Self {
            unit,
            active,
            input_fee_ppk,
            final_expiry,
            amount_coefficients,
        }
    }
}

/// Generated public federation config and private config for every member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedDealerConfigBundle {
    /// Public federation config shared by every member.
    pub public_config: FederationConfig,
    /// Private federation config per member.
    pub private_configs: BTreeMap<MemberId, MemberConfigPrivate>,
}

/// Options for trusted-dealer config generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrustedDealerGenerationOptions {
    /// Use no-aux-rand Schnorr signatures for reproducible fixture output.
    pub no_aux_rand_signatures: bool,
    /// Scheduled-checkpoint cadence embedded in the generated public config.
    pub checkpoint_policy: FederationCheckpointPolicy,
}

/// Generate a federation config bundle with locally computed threshold shares.
pub fn generate_trusted_dealer_configs(
    threshold_params: ThresholdParams,
    members: Vec<TrustedDealerMember>,
    keysets: Vec<TrustedDealerKeysetSpec>,
) -> Result<TrustedDealerConfigBundle> {
    generate_trusted_dealer_configs_with_options(
        threshold_params,
        members,
        keysets,
        TrustedDealerGenerationOptions::default(),
    )
}

/// Generate a federation config bundle with locally computed threshold shares and options.
pub fn generate_trusted_dealer_configs_with_options(
    threshold_params: ThresholdParams,
    members: Vec<TrustedDealerMember>,
    keysets: Vec<TrustedDealerKeysetSpec>,
    options: TrustedDealerGenerationOptions,
) -> Result<TrustedDealerConfigBundle> {
    threshold_params.validate()?;
    if usize::from(threshold_params.member_count) != members.len() {
        return Err(FederationError::MemberCountMismatch {
            expected: threshold_params.member_count,
            found: members.len(),
        });
    }

    let mut public_members = BTreeMap::new();
    let mut private_configs = BTreeMap::new();
    for member in members {
        if !matches!(member.identity_secret_key, SecretKey::Secp256k1(_)) {
            return Err(FederationError::InvalidIdentityKeyKind {
                member_id: member.member_id,
            });
        }

        let public_member = MemberConfigPublic {
            member_id: member.member_id,
            public_mint_url: member.public_mint_url,
            federation_api_url: member.federation_api_url,
            identity_public_key: member.identity_secret_key.public_key(),
        };
        if public_members
            .insert(member.member_id, public_member)
            .is_some()
        {
            return Err(FederationError::DuplicateMember {
                member_id: member.member_id,
            });
        }

        private_configs.insert(
            member.member_id,
            MemberConfigPrivate {
                federation_id: None,
                member_id: member.member_id,
                identity_secret_key: member.identity_secret_key,
                keyset_secret_shares: BTreeMap::new(),
                setup_transcript_hash: None,
            },
        );
    }

    let signer_ids = public_members
        .keys()
        .map(|member_id| member_id.to_bls_signer_id())
        .collect::<Vec<_>>();
    let mut federated_keysets = Vec::with_capacity(keysets.len());
    for keyset_spec in keysets {
        if keyset_spec.amount_coefficients.is_empty() {
            return Err(FederationError::EmptyTrustedDealerKeyset {
                unit: keyset_spec.unit,
            });
        }

        let mut aggregate_keys = BTreeMap::new();
        let mut public_shares = public_members
            .keys()
            .map(|member_id| (*member_id, BTreeMap::new()))
            .collect::<BTreeMap<_, _>>();
        let mut private_keyset_shares = public_members
            .keys()
            .map(|member_id| (*member_id, BTreeMap::new()))
            .collect::<BTreeMap<_, _>>();

        for (amount, coefficients) in keyset_spec.amount_coefficients {
            validate_coefficient_count(
                amount,
                threshold_params.signature_threshold,
                coefficients.len(),
            )?;
            let threshold_keyset = trusted_dealer_keygen(&coefficients, &signer_ids)?;
            aggregate_keys.insert(amount, threshold_keyset.aggregate_public_key());

            for share in threshold_keyset.public_shares() {
                let member_id = MemberId::new(share.signer_id().get())?;
                public_shares
                    .get_mut(&member_id)
                    .ok_or(FederationError::MissingMember { member_id })?
                    .insert(amount, *share);
            }

            for share in threshold_keyset.secret_shares() {
                let member_id = MemberId::new(share.signer_id().get())?;
                private_keyset_shares
                    .get_mut(&member_id)
                    .ok_or(FederationError::MissingMember { member_id })?
                    .insert(amount, share.clone());
            }
        }

        let federated_keyset = FederatedKeyset::new(
            keyset_spec.unit,
            keyset_spec.active,
            keyset_spec.input_fee_ppk,
            keyset_spec.final_expiry,
            aggregate_keys,
            public_shares,
        );

        for (member_id, secret_shares) in private_keyset_shares {
            private_configs
                .get_mut(&member_id)
                .ok_or(FederationError::MissingMember { member_id })?
                .keyset_secret_shares
                .insert(federated_keyset.id, secret_shares);
        }

        federated_keysets.push(federated_keyset);
    }

    let setup_authorization =
        trusted_dealer_setup_authorization(threshold_params, &public_members, &federated_keysets)?;
    let mut public_config = FederationConfig::new(
        setup_authorization,
        threshold_params,
        public_members,
        federated_keysets,
    )?;
    public_config.checkpoint_policy = options.checkpoint_policy;
    public_config.payment_observation_policy =
        FederationPaymentObservationPolicy::development_single_observation_for_testing();
    let federation_id = public_config.federation_id;
    for private_config in private_configs.values_mut() {
        private_config.federation_id = Some(federation_id);
    }
    let setup_transcript = match options.no_aux_rand_signatures {
        true => {
            FederationDkgTranscript::trusted_dealer_no_aux_rand(&public_config, &private_configs)?
        }
        false => FederationDkgTranscript::trusted_dealer(&public_config, &private_configs)?,
    };
    let setup_transcript_hash = setup_transcript.transcript_hash;
    public_config.setup_transcript = Some(setup_transcript);
    public_config.validate()?;
    for private_config in private_configs.values_mut() {
        private_config.setup_transcript_hash = Some(setup_transcript_hash);
        private_config.validate_against_public(&public_config)?;
    }

    Ok(TrustedDealerConfigBundle {
        public_config,
        private_configs,
    })
}

fn trusted_dealer_setup_authorization(
    threshold_params: ThresholdParams,
    members: &BTreeMap<MemberId, MemberConfigPublic>,
    keysets: &[FederatedKeyset],
) -> Result<FederationSetupAuthorizationHash> {
    let mut transcript = Vec::new();
    append_trusted_dealer_field(
        &mut transcript,
        b"cdk/federation/setup/test-trusted-dealer/v1",
    )?;
    transcript.extend_from_slice(&threshold_params.member_count.to_be_bytes());
    transcript.extend_from_slice(&threshold_params.signature_threshold.to_be_bytes());
    transcript.extend_from_slice(&threshold_params.consensus_threshold.to_be_bytes());
    for (member_id, member) in members {
        transcript.extend_from_slice(&member_id.get().to_be_bytes());
        append_trusted_dealer_field(
            &mut transcript,
            member.public_mint_url.to_string().as_bytes(),
        )?;
        append_trusted_dealer_field(
            &mut transcript,
            member.federation_api_url.to_string().as_bytes(),
        )?;
        append_trusted_dealer_field(&mut transcript, &member.identity_public_key.to_bytes())?;
    }
    for keyset in keysets {
        append_trusted_dealer_field(&mut transcript, &keyset.id.to_bytes())?;
        append_trusted_dealer_field(&mut transcript, keyset.unit.to_string().as_bytes())?;
        transcript.push(u8::from(keyset.active));
        transcript.extend_from_slice(&keyset.input_fee_ppk.to_be_bytes());
        match keyset.final_expiry {
            Some(expiry) => {
                transcript.push(1);
                transcript.extend_from_slice(&expiry.to_be_bytes());
            }
            None => transcript.push(0),
        }
        for (amount, aggregate_key) in &keyset.aggregate_keys {
            transcript.extend_from_slice(&u64::from(*amount).to_be_bytes());
            append_trusted_dealer_field(&mut transcript, &aggregate_key.to_bytes())?;
        }
    }
    Ok(FederationSetupAuthorizationHash::from_bytes(
        Sha256Hash::hash(&transcript).to_byte_array(),
    ))
}

fn append_trusted_dealer_field(output: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let length = u32::try_from(value.len()).map_err(|_| FederationError::InvalidSetup {
        reason: "test trusted-dealer field length does not fit u32".to_string(),
    })?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

fn validate_coefficient_count(
    amount: Amount,
    signature_threshold: u16,
    found: usize,
) -> Result<()> {
    let expected = usize::from(signature_threshold);
    if found != expected {
        return Err(FederationError::InvalidTrustedDealerCoefficientCount {
            amount,
            expected,
            found,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::federation::{
        FederationDkgSetupMethod, FederationDkgTranscriptHash, FederationId,
        FederationKeysetRotationOperation,
    };

    use super::*;

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("valid member id")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("valid secp256k1 secret")
    }

    fn member(member_id: MemberId) -> TrustedDealerMember {
        TrustedDealerMember::new(
            member_id,
            MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
                .expect("public mint url"),
            MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get()))
                .expect("federation api url"),
            identity_secret(member_id),
        )
    }

    fn bls_key(byte: u8) -> BlsSecretKey {
        BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    fn keyset_spec() -> TrustedDealerKeysetSpec {
        TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [
                (Amount::from(1), vec![bls_key(1), bls_key(2)]),
                (Amount::from(2), vec![bls_key(3), bls_key(4)]),
            ]
            .into_iter()
            .collect(),
        )
    }

    fn rotation_keyset_spec() -> TrustedDealerKeysetSpec {
        TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            false,
            0,
            None,
            [
                (Amount::from(1), vec![bls_key(5), bls_key(6)]),
                (Amount::from(2), vec![bls_key(7), bls_key(8)]),
            ]
            .into_iter()
            .collect(),
        )
    }

    #[cfg(feature = "prometheus")]
    fn performance_metric_value(metric: &str, labels: &[(&str, &str)]) -> f64 {
        let scrape = cdk_prometheus::prometheus::TextEncoder::new()
            .encode_to_string(&cdk_prometheus::METRICS.registry().gather())
            .expect("encode prometheus metrics");
        scrape
            .lines()
            .find_map(|line| {
                if !line.starts_with(metric)
                    || labels
                        .iter()
                        .any(|(key, value)| !line.contains(&format!("{key}=\"{value}\"")))
                {
                    return None;
                }
                line.rsplit_once(' ')
                    .and_then(|(_, value)| value.parse::<f64>().ok())
            })
            .unwrap_or(0.0)
    }

    #[test]
    fn trusted_dealer_generates_public_and_private_configs() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let bundle = generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![keyset_spec()],
        )
        .expect("trusted dealer bundle");

        assert_eq!(bundle.public_config.members.len(), 3);
        assert_eq!(bundle.public_config.keysets.len(), 1);
        assert_eq!(bundle.private_configs.len(), 3);
        assert_eq!(
            bundle.public_config.payment_observation_policy,
            FederationPaymentObservationPolicy::development_single_observation_for_testing()
        );
        let setup_transcript = bundle
            .public_config
            .setup_transcript
            .as_ref()
            .expect("setup transcript");
        assert_eq!(
            setup_transcript.payload.setup_method,
            FederationDkgSetupMethod::TrustedDealer
        );
        assert_eq!(
            setup_transcript.payload.federation_id,
            bundle.public_config.federation_id
        );
        assert_eq!(
            setup_transcript.payload.participant_order,
            vec![member_id(1), member_id(2), member_id(3)]
        );
        assert_eq!(setup_transcript.signatures.len(), 3);
        setup_transcript
            .validate_against_config(&bundle.public_config)
            .expect("setup transcript validates");
        for private_config in bundle.private_configs.values() {
            assert_eq!(
                private_config.federation_id,
                Some(bundle.public_config.federation_id)
            );
            assert_eq!(
                private_config.setup_transcript_hash,
                Some(setup_transcript.transcript_hash)
            );
            private_config
                .validate_against_public(&bundle.public_config)
                .expect("private config");
        }

        let public_json = serde_json::to_string(&bundle.public_config).expect("public config json");
        assert_eq!(
            serde_json::from_str::<FederationConfig>(&public_json)
                .expect("public config from json"),
            bundle.public_config
        );
        let private_json =
            serde_json::to_string(bundle.private_configs.get(&member_id(1)).expect("private"))
                .expect("private config json");
        assert_eq!(
            serde_json::from_str::<MemberConfigPrivate>(&private_json)
                .expect("private config from json"),
            *bundle.private_configs.get(&member_id(1)).expect("private")
        );
    }

    #[cfg(feature = "prometheus")]
    #[test]
    fn config_validation_performance_metrics_cover_success_and_failure_branches() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let bundle = generate_trusted_dealer_configs_with_options(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![keyset_spec()],
            TrustedDealerGenerationOptions {
                no_aux_rand_signatures: true,
                ..TrustedDealerGenerationOptions::default()
            },
        )
        .expect("trusted dealer bundle");

        let config_attempts_before = performance_metric_value(
            "cdk_federation_performance_work_total",
            &[("work", "config_validation_attempts")],
        );
        let keyset_attempts_before = performance_metric_value(
            "cdk_federation_performance_work_total",
            &[("work", "keyset_validation_attempts")],
        );
        let interpolation_attempts_before = performance_metric_value(
            "cdk_federation_performance_work_total",
            &[("work", "aggregate_key_interpolation_attempts")],
        );
        let lineage_attempts_before = performance_metric_value(
            "cdk_federation_performance_work_total",
            &[("work", "setup_lineage_validation_attempts")],
        );
        let full_config_success_before = performance_metric_value(
            "cdk_federation_performance_operation_duration_seconds_count",
            &[
                ("operation", "full_config_validation"),
                ("outcome", "success"),
            ],
        );

        bundle
            .public_config
            .validate()
            .expect("valid config remains valid");

        assert!(
            performance_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "config_validation_attempts")],
            ) >= config_attempts_before + 2.0,
            "setup lineage performs one nested setup-config validation"
        );
        assert!(
            performance_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "keyset_validation_attempts")],
            ) >= keyset_attempts_before + 2.0
        );
        assert!(
            performance_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "aggregate_key_interpolation_attempts")],
            ) >= interpolation_attempts_before + 4.0
        );
        assert!(
            performance_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "setup_lineage_validation_attempts")],
            ) >= lineage_attempts_before + 1.0
        );
        assert!(
            performance_metric_value(
                "cdk_federation_performance_operation_duration_seconds_count",
                &[
                    ("operation", "full_config_validation"),
                    ("outcome", "success")
                ],
            ) >= full_config_success_before + 2.0
        );

        let keyset_failures_before = performance_metric_value(
            "cdk_federation_performance_work_total",
            &[("work", "keyset_validation_failures")],
        );
        let interpolation_failures_before = performance_metric_value(
            "cdk_federation_performance_work_total",
            &[("work", "aggregate_key_interpolation_failures")],
        );
        let invalid_thresholds = ThresholdParams {
            member_count: 3,
            signature_threshold: 4,
            consensus_threshold: 4,
        };
        bundle.public_config.keysets[0]
            .validate(invalid_thresholds, &bundle.public_config.members)
            .expect_err("interpolation threshold above share count must fail");
        assert!(
            performance_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "keyset_validation_failures")],
            ) >= keyset_failures_before + 1.0
        );
        assert!(
            performance_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "aggregate_key_interpolation_failures")],
            ) >= interpolation_failures_before + 1.0
        );

        let config_failures_before = performance_metric_value(
            "cdk_federation_performance_work_total",
            &[("work", "config_validation_failures")],
        );
        let lineage_failures_before = performance_metric_value(
            "cdk_federation_performance_work_total",
            &[("work", "setup_lineage_validation_failures")],
        );
        let mut invalid_lineage = bundle.public_config;
        invalid_lineage
            .setup_transcript
            .as_mut()
            .expect("setup transcript")
            .signatures
            .remove(&member_id(1));
        invalid_lineage
            .validate()
            .expect_err("missing setup signature must fail lineage validation");
        assert!(
            performance_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "config_validation_failures")],
            ) >= config_failures_before + 1.0
        );
        assert!(
            performance_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "setup_lineage_validation_failures")],
            ) >= lineage_failures_before + 1.0
        );
    }

    #[test]
    fn trusted_dealer_setup_transcript_survives_consensus_keyset_rotation() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let bundle = generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![keyset_spec(), rotation_keyset_spec()],
        )
        .expect("trusted dealer bundle");
        let setup_transcript = bundle
            .public_config
            .setup_transcript
            .clone()
            .expect("setup transcript");
        let original_active_keyset = bundle
            .public_config
            .keysets
            .iter()
            .find(|keyset| keyset.active)
            .expect("original active keyset")
            .id;
        let mut rotation_keyset = bundle
            .public_config
            .keysets
            .iter()
            .find(|keyset| !keyset.active)
            .expect("rotation keyset")
            .clone();
        rotation_keyset.active = true;

        let rotated = bundle
            .public_config
            .apply_keyset_rotation(&FederationKeysetRotationOperation {
                keyset: rotation_keyset,
            })
            .expect("consensus keyset rotation");

        assert_eq!(rotated.setup_transcript, Some(setup_transcript));
        assert!(
            !rotated
                .keysets
                .iter()
                .find(|keyset| keyset.id == original_active_keyset)
                .expect("original keyset retained")
                .active
        );
        rotated.validate().expect("rotated config validates");
        for private_config in bundle.private_configs.values() {
            private_config
                .validate_against_public(&rotated)
                .expect("rotated private config validates");
        }

        let mut missing_setup_keyset = rotated;
        missing_setup_keyset
            .keysets
            .retain(|keyset| keyset.id != original_active_keyset);
        assert!(matches!(
            missing_setup_keyset.validate(),
            Err(FederationError::DkgTranscriptMismatch { reason })
                if reason.contains("is missing from current config")
        ));
    }

    #[test]
    fn trusted_dealer_rejects_private_federation_id_mismatch() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let bundle = generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![keyset_spec()],
        )
        .expect("trusted dealer bundle");

        let mut private_config = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config")
            .clone();
        private_config.federation_id = Some(FederationId::from_bytes([7; 32]));

        assert!(matches!(
            private_config.validate_against_public(&bundle.public_config),
            Err(FederationError::FederationIdMismatch { expected, found })
                if expected == bundle.public_config.federation_id
                    && found != bundle.public_config.federation_id
        ));
    }

    #[test]
    fn trusted_dealer_rejects_private_config_missing_federation_id() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let bundle = generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![keyset_spec()],
        )
        .expect("trusted dealer bundle");

        let mut private_config = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config")
            .clone();
        private_config.federation_id = None;

        assert!(matches!(
            private_config.validate_against_public(&bundle.public_config),
            Err(FederationError::DkgTranscriptMismatch { reason })
                if reason == "private config missing federation id"
        ));
    }

    #[test]
    fn trusted_dealer_rejects_private_transcript_hash_mismatch() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let bundle = generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![keyset_spec()],
        )
        .expect("trusted dealer bundle");

        let mut private_config = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config")
            .clone();
        private_config.setup_transcript_hash =
            Some(FederationDkgTranscriptHash::from_bytes([9; 32]));

        assert!(matches!(
            private_config.validate_against_public(&bundle.public_config),
            Err(FederationError::DkgTranscriptMismatch { reason })
                if reason == "private setup transcript hash mismatch"
        ));
    }

    #[test]
    fn trusted_dealer_rejects_private_config_missing_transcript_hash() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let bundle = generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![keyset_spec()],
        )
        .expect("trusted dealer bundle");

        let mut private_config = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config")
            .clone();
        private_config.setup_transcript_hash = None;

        assert!(matches!(
            private_config.validate_against_public(&bundle.public_config),
            Err(FederationError::DkgTranscriptMismatch { reason })
                if reason == "private config missing setup transcript hash"
        ));
    }

    #[test]
    fn trusted_dealer_rejects_tampered_setup_transcript_payload() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let bundle = generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![keyset_spec()],
        )
        .expect("trusted dealer bundle");

        let mut public_config = bundle.public_config;
        public_config
            .setup_transcript
            .as_mut()
            .expect("setup transcript")
            .payload
            .participant_order
            .swap(0, 1);

        assert!(matches!(
            public_config.validate(),
            Err(FederationError::DkgTranscriptMismatch { reason })
                if reason == "transcript hash does not match payload"
        ));
    }

    #[test]
    fn trusted_dealer_rejects_tampered_setup_transcript_signature() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let bundle = generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![keyset_spec()],
        )
        .expect("trusted dealer bundle");

        let mut public_config = bundle.public_config;
        let replacement_signature = *public_config
            .setup_transcript
            .as_ref()
            .expect("setup transcript")
            .signatures
            .get(&member_id(2))
            .expect("replacement signature");
        public_config
            .setup_transcript
            .as_mut()
            .expect("setup transcript")
            .signatures
            .insert(member_id(1), replacement_signature);

        assert!(matches!(
            public_config.validate(),
            Err(FederationError::DkgTranscriptSignatureVerificationFailed { member_id: id })
                if id == member_id(1)
        ));
    }

    #[test]
    fn trusted_dealer_rejects_unknown_setup_transcript_signer() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let bundle = generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![keyset_spec()],
        )
        .expect("trusted dealer bundle");

        let mut public_config = bundle.public_config;
        let extra_signature = *public_config
            .setup_transcript
            .as_ref()
            .expect("setup transcript")
            .signatures
            .get(&member_id(1))
            .expect("extra signature");
        public_config
            .setup_transcript
            .as_mut()
            .expect("setup transcript")
            .signatures
            .insert(member_id(99), extra_signature);

        assert!(matches!(
            public_config.validate(),
            Err(FederationError::UnknownMember { member_id: id }) if id == member_id(99)
        ));
    }

    #[test]
    fn trusted_dealer_rejects_duplicate_members() {
        let params = ThresholdParams::with_bft_consensus(2, 2).expect("params");
        assert!(matches!(
            generate_trusted_dealer_configs(
                params,
                vec![member(member_id(1)), member(member_id(1))],
                vec![keyset_spec()],
            ),
            Err(FederationError::DuplicateMember { member_id: id }) if id == member_id(1)
        ));
    }

    #[test]
    fn trusted_dealer_rejects_wrong_coefficient_count() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let mut keyset = keyset_spec();
        keyset
            .amount_coefficients
            .insert(Amount::from(1), vec![bls_key(1)]);

        assert!(matches!(
            generate_trusted_dealer_configs(
                params,
                vec![member(member_id(1)), member(member_id(2)), member(member_id(3))],
                vec![keyset],
            ),
            Err(FederationError::InvalidTrustedDealerCoefficientCount {
                amount,
                expected: 2,
                found: 1,
            }) if amount == Amount::from(1)
        ));
    }

    #[test]
    fn trusted_dealer_rejects_bls_identity_key() {
        let params = ThresholdParams::with_bft_consensus(2, 1).expect("params");
        let invalid_member = TrustedDealerMember::new(
            member_id(1),
            MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed1.example.com").expect("federation api url"),
            SecretKey::bls_from_reduced_bytes(&[9; 32]),
        );

        assert!(matches!(
            generate_trusted_dealer_configs(
                params,
                vec![invalid_member, member(member_id(2))],
                vec![keyset_spec()],
            ),
            Err(FederationError::InvalidIdentityKeyKind { member_id: id }) if id == member_id(1)
        ));
    }
}
