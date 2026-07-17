//! Federation threshold BLS signing helpers.

#[cfg(feature = "prometheus")]
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::{
    FederationConfig, FederationError, FederationJournalConsensusEvidence, FederationJournalEntry,
    FederationOperationId, MemberConfigPrivate, MemberId, Result,
};
use crate::nuts::nut01::{verify_blind_signature_share, BlsBlindSignatureShare};
use crate::{Amount, BlindedMessage, Id, PublicKey};

/// Blind signature share for one blinded output and one federation member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationBlindSignatureShare {
    /// Operation that authorized this signature share.
    pub operation_id: FederationOperationId,
    /// Federation member that produced the share.
    pub member_id: MemberId,
    /// Keyset used to sign the output.
    pub keyset_id: Id,
    /// Output amount signed by the share.
    pub amount: Amount,
    /// Wallet-provided blinded secret.
    pub blinded_secret: PublicKey,
    /// BLS blind signature share.
    pub share: BlsBlindSignatureShare,
}

/// Signature shares produced by one member for one accepted operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationBlindSignatureShareSet {
    /// Operation that authorized these signature shares.
    pub operation_id: FederationOperationId,
    /// Federation member that produced the shares.
    pub member_id: MemberId,
    /// Shares in the same order as the signed outputs.
    pub shares: Vec<FederationBlindSignatureShare>,
}

/// Durable federation signature-share row for one operation output and member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationSignatureShareRecord {
    /// Operation that authorized this signature share.
    pub operation_id: FederationOperationId,
    /// Zero-based accepted operation index in the federation journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_operation_index: Option<u64>,
    /// Ordering-backend evidence for the accepted operation, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consensus_evidence: Option<FederationJournalConsensusEvidence>,
    /// Zero-based output index inside the accepted operation.
    pub output_index: u32,
    /// Federation member that produced the share.
    pub member_id: MemberId,
    /// Keyset used to sign the output.
    pub keyset_id: Id,
    /// Output amount signed by the share.
    pub amount: Amount,
    /// Wallet-provided blinded secret.
    pub blinded_secret: PublicKey,
    /// BLS blind signature share.
    pub share: BlsBlindSignatureShare,
}

impl FederationSignatureShareRecord {
    /// Build a durable row from a signed output share.
    pub fn new(output_index: u32, share: FederationBlindSignatureShare) -> Self {
        Self {
            operation_id: share.operation_id,
            accepted_operation_index: None,
            consensus_evidence: None,
            output_index,
            member_id: share.member_id,
            keyset_id: share.keyset_id,
            amount: share.amount,
            blinded_secret: share.blinded_secret,
            share: share.share,
        }
    }

    /// Build durable rows for all shares in a share set.
    pub fn from_share_set(share_set: &FederationBlindSignatureShareSet) -> Vec<Self> {
        share_set
            .shares
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, share)| Self::new(index as u32, share))
            .collect()
    }

    /// Build durable rows for all shares in a share set with accepted-order provenance.
    ///
    /// Prefer [`Self::from_accepted_operation`] or
    /// [`Self::from_accepted_journal_entry`] when the accepted operation ID is
    /// available, so provenance cannot be attached to the wrong share set.
    pub fn from_accepted_share_set(
        share_set: &FederationBlindSignatureShareSet,
        accepted_operation_index: u64,
        consensus_evidence: Option<FederationJournalConsensusEvidence>,
    ) -> Vec<Self> {
        let mut records = Self::from_share_set(share_set);
        for record in &mut records {
            record.accepted_operation_index = Some(accepted_operation_index);
            record.consensus_evidence = consensus_evidence;
        }
        records
    }

    /// Build durable rows with accepted-order provenance after checking the operation ID.
    pub fn from_accepted_operation(
        share_set: &FederationBlindSignatureShareSet,
        operation_id: FederationOperationId,
        accepted_operation_index: u64,
        consensus_evidence: Option<FederationJournalConsensusEvidence>,
    ) -> Result<Vec<Self>> {
        if share_set.operation_id != operation_id {
            return Err(FederationError::SignatureShareOperationMismatch {
                expected: operation_id,
                found: share_set.operation_id,
            });
        }

        Ok(Self::from_accepted_share_set(
            share_set,
            accepted_operation_index,
            consensus_evidence,
        ))
    }

    /// Build durable rows using provenance from the accepted journal entry.
    pub fn from_accepted_journal_entry(
        share_set: &FederationBlindSignatureShareSet,
        entry: &FederationJournalEntry,
    ) -> Result<Vec<Self>> {
        Self::from_accepted_operation(
            share_set,
            entry.operation_id,
            entry.index,
            entry.consensus_evidence,
        )
    }
}

impl MemberConfigPrivate {
    /// Sign blinded outputs with this member's threshold BLS secret shares.
    pub fn sign_blinded_messages(
        &self,
        public_config: &FederationConfig,
        operation_id: FederationOperationId,
        outputs: &[BlindedMessage],
    ) -> Result<FederationBlindSignatureShareSet> {
        self.validate_metadata_against_public(public_config)?;

        let mut shares = Vec::with_capacity(outputs.len());
        for output in outputs {
            let keyset = public_config.validate_blinded_output(output)?;
            let blinded_secret = output.blinded_secret.as_bls_g1().map_err(|_| {
                FederationError::InvalidFederationBlindedSecret {
                    keyset_id: output.keyset_id,
                    amount: output.amount,
                }
            })?;
            let secret_share = self
                .keyset_secret_shares
                .get(&keyset.id)
                .ok_or(FederationError::MissingPrivateKeyset {
                    keyset_id: keyset.id,
                })?
                .get(&output.amount)
                .ok_or(FederationError::MissingSecretShare {
                    keyset_id: keyset.id,
                    amount: output.amount,
                })?;
            if secret_share.signer_id().get() != self.member_id.get() {
                return Err(FederationError::SecretShareSignerMismatch {
                    keyset_id: keyset.id,
                    member_id: self.member_id,
                    signer_id: secret_share.signer_id().get(),
                });
            }
            let public_share = keyset
                .public_shares
                .get(&self.member_id)
                .ok_or(FederationError::MissingPublicShares {
                    keyset_id: keyset.id,
                    member_id: self.member_id,
                })?
                .get(&output.amount)
                .ok_or(FederationError::MissingPublicShareAmount {
                    keyset_id: keyset.id,
                    member_id: self.member_id,
                    amount: output.amount,
                })?;
            #[cfg(feature = "prometheus")]
            cdk_prometheus::METRICS.record_federation_performance_work(
                cdk_prometheus::FederationPerformanceWork::MemberBlindedShareOutputs,
                1,
            );
            #[cfg(feature = "prometheus")]
            let signing_started_at = Instant::now();
            let share = secret_share.blind_sign_share(&blinded_secret);
            #[cfg(feature = "prometheus")]
            cdk_prometheus::METRICS.record_federation_performance_operation(
                cdk_prometheus::FederationPerformanceOperation::MemberBlindedShareSigning,
                cdk_prometheus::FederationPerformanceOutcome::Success,
                signing_started_at.elapsed().as_secs_f64(),
            );

            #[cfg(feature = "prometheus")]
            let verification_started_at = Instant::now();
            let verification = verify_blind_signature_share(&share, &blinded_secret, public_share);
            #[cfg(feature = "prometheus")]
            cdk_prometheus::METRICS.record_federation_performance_operation(
                cdk_prometheus::FederationPerformanceOperation::MemberBlindedShareSelfVerification,
                cdk_prometheus::FederationPerformanceOutcome::from_success(
                    verification.as_ref().is_ok_and(|verified| *verified),
                ),
                verification_started_at.elapsed().as_secs_f64(),
            );
            if !verification? {
                return Err(FederationError::SignatureShareVerificationFailed {
                    keyset_id: keyset.id,
                    member_id: self.member_id,
                    amount: output.amount,
                });
            }

            shares.push(FederationBlindSignatureShare {
                operation_id,
                member_id: self.member_id,
                keyset_id: keyset.id,
                amount: output.amount,
                blinded_secret: output.blinded_secret,
                share,
            });
        }

        Ok(FederationBlindSignatureShareSet {
            operation_id,
            member_id: self.member_id,
            shares,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use crate::nuts::nut01::{aggregate_blind_signature_shares, BlsG1PublicKey, BlsSecretKey};
    use crate::{
        mint_url::MintUrl, CurrencyUnit, MintQuoteBolt11Request, MintQuoteBolt11Response,
        MintQuoteRequest, MintQuoteResponse, MintQuoteState, SecretKey,
    };

    use super::*;
    use crate::federation::{
        generate_trusted_dealer_configs, FederationId, FederationMintQuoteOperation,
        FederationOperation, FederationOperationEnvelope, InMemoryFederationJournal,
        ThresholdParams, TrustedDealerKeysetSpec, TrustedDealerMember,
    };

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("secret key")
    }

    fn member(member_id: MemberId) -> TrustedDealerMember {
        TrustedDealerMember::new(
            member_id,
            MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
                .expect("mint url"),
            MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get()))
                .expect("fed url"),
            identity_secret(member_id),
        )
    }

    fn bls_key(byte: u8) -> BlsSecretKey {
        BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    fn mint_quote_envelope(
        federation_id: FederationId,
        quote: &str,
    ) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            FederationOperation::MintQuote(FederationMintQuoteOperation {
                quote: quote.to_string(),
                request_lookup_id: crate::payment::PaymentIdentifier::CustomId(quote.to_string()),
                request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                    amount: Amount::from(1),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }),
                response: MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                    quote: quote.to_string(),
                    request: format!("invoice-{quote}"),
                    amount: Some(Amount::from(1)),
                    unit: Some(CurrencyUnit::Sat),
                    method: crate::PaymentMethod::BOLT11,
                    amount_paid: Amount::ZERO,
                    amount_issued: Amount::ZERO,
                    updated_at: 0,
                    state: MintQuoteState::Unpaid,
                    expiry: None,
                    pubkey: None,
                }),
            }),
        )
    }

    fn bundle() -> super::super::TrustedDealerConfigBundle {
        let mut amounts = BTreeMap::new();
        amounts.insert(Amount::from(1), vec![bls_key(1), bls_key(2)]);
        amounts.insert(Amount::from(2), vec![bls_key(3), bls_key(4)]);
        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("params"),
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                amounts,
            )],
        )
        .expect("trusted dealer bundle")
    }

    fn sliding_window_bundle() -> super::super::TrustedDealerConfigBundle {
        let mut amounts = BTreeMap::new();
        amounts.insert(Amount::from(1), vec![bls_key(11), bls_key(12), bls_key(13)]);
        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(4, 3).expect("params"),
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
                member(member_id(4)),
            ],
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                amounts,
            )],
        )
        .expect("trusted dealer bundle")
    }

    #[test]
    fn member_private_config_signs_bls_outputs() {
        let bundle = bundle();
        let keyset = &bundle.public_config.keysets[0];
        let operation_id = FederationOperationId::from_bytes([1; 32]);
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"federation signing output");
        let output =
            BlindedMessage::new(Amount::from(1), keyset.id, PublicKey::from(blinded_message));
        let private = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config");

        let share_set = private
            .sign_blinded_messages(&bundle.public_config, operation_id, &[output])
            .expect("signature share set");
        let share = &share_set.shares[0];
        let public_share = keyset
            .public_shares
            .get(&member_id(1))
            .expect("member shares")
            .get(&Amount::from(1))
            .expect("amount share");

        assert_eq!(share_set.member_id, member_id(1));
        assert_eq!(share.operation_id, operation_id);
        assert!(
            verify_blind_signature_share(&share.share, &blinded_message, public_share)
                .expect("share verification")
        );
        serde_json::to_string(&share_set).expect("share set json");
    }

    #[test]
    fn threshold_member_shares_can_be_aggregated() {
        let bundle = bundle();
        let keyset = &bundle.public_config.keysets[0];
        let operation_id = FederationOperationId::from_bytes([2; 32]);
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"aggregate federation shares");
        let output =
            BlindedMessage::new(Amount::from(1), keyset.id, PublicKey::from(blinded_message));
        let mut shares = Vec::new();
        for member_id in [member_id(1), member_id(2)] {
            let private = bundle
                .private_configs
                .get(&member_id)
                .expect("private config");
            let share_set = private
                .sign_blinded_messages(
                    &bundle.public_config,
                    operation_id,
                    std::slice::from_ref(&output),
                )
                .expect("share set");
            shares.push(share_set.shares[0].share);
        }

        aggregate_blind_signature_shares(
            usize::from(bundle.public_config.threshold_params.signature_threshold),
            &shares,
        )
        .expect("aggregate signature");
    }

    #[test]
    fn accepted_share_records_bind_to_journal_entry() {
        let bundle = bundle();
        let keyset = &bundle.public_config.keysets[0];
        let mut journal = InMemoryFederationJournal::new();
        let entry = journal
            .append(mint_quote_envelope(
                bundle.public_config.federation_id,
                "share-provenance",
            ))
            .expect("append accepted operation")
            .entry()
            .clone();
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"accepted share provenance");
        let output =
            BlindedMessage::new(Amount::from(1), keyset.id, PublicKey::from(blinded_message));
        let private = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config");
        let share_set = private
            .sign_blinded_messages(
                &bundle.public_config,
                entry.operation_id,
                std::slice::from_ref(&output),
            )
            .expect("signature share set");

        let records =
            FederationSignatureShareRecord::from_accepted_journal_entry(&share_set, &entry)
                .expect("accepted records");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].operation_id, entry.operation_id);
        assert_eq!(records[0].accepted_operation_index, Some(entry.index));
        assert_eq!(records[0].consensus_evidence, entry.consensus_evidence);
        assert_eq!(records[0].output_index, 0);
        assert_eq!(records[0].member_id, member_id(1));
        assert_eq!(records[0].keyset_id, output.keyset_id);
        assert_eq!(records[0].amount, output.amount);
        assert_eq!(records[0].blinded_secret, output.blinded_secret);
    }

    #[test]
    fn accepted_share_records_reject_wrong_operation_provenance() {
        let bundle = bundle();
        let keyset = &bundle.public_config.keysets[0];
        let mut journal = InMemoryFederationJournal::new();
        let entry = journal
            .append(mint_quote_envelope(
                bundle.public_config.federation_id,
                "share-provenance-mismatch",
            ))
            .expect("append accepted operation")
            .entry()
            .clone();
        let wrong_operation_id = FederationOperationId::from_bytes([99; 32]);
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"wrong accepted share provenance");
        let output =
            BlindedMessage::new(Amount::from(1), keyset.id, PublicKey::from(blinded_message));
        let private = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config");
        let share_set = private
            .sign_blinded_messages(&bundle.public_config, wrong_operation_id, &[output])
            .expect("signature share set");

        assert!(matches!(
            FederationSignatureShareRecord::from_accepted_journal_entry(&share_set, &entry),
            Err(FederationError::SignatureShareOperationMismatch { expected, found })
                if expected == entry.operation_id && found == wrong_operation_id
        ));
    }

    #[test]
    fn naive_local_signing_allows_sliding_window_quorum_cover() {
        let bundle = sliding_window_bundle();
        let keyset = &bundle.public_config.keysets[0];
        let output_a = BlindedMessage::new(
            Amount::from(1),
            keyset.id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"naive-window-output-a")),
        );
        let output_b = BlindedMessage::new(
            Amount::from(1),
            keyset.id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"naive-window-output-b")),
        );
        let output_c = BlindedMessage::new(
            Amount::from(1),
            keyset.id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"naive-window-output-c")),
        );
        let output_d = BlindedMessage::new(
            Amount::from(1),
            keyset.id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"naive-window-output-d")),
        );
        let views = [
            (
                member_id(1),
                FederationOperationId::from_bytes([1; 32]),
                vec![
                    ("A", output_a.clone()),
                    ("B", output_b.clone()),
                    ("C", output_c.clone()),
                ],
            ),
            (
                member_id(2),
                FederationOperationId::from_bytes([2; 32]),
                vec![
                    ("B", output_b.clone()),
                    ("C", output_c.clone()),
                    ("D", output_d.clone()),
                ],
            ),
            (
                member_id(3),
                FederationOperationId::from_bytes([3; 32]),
                vec![
                    ("C", output_c),
                    ("D", output_d.clone()),
                    ("A", output_a.clone()),
                ],
            ),
            (
                member_id(4),
                FederationOperationId::from_bytes([4; 32]),
                vec![("D", output_d), ("A", output_a), ("B", output_b)],
            ),
        ];

        let mut shares_by_output = BTreeMap::from([
            ("A", Vec::new()),
            ("B", Vec::new()),
            ("C", Vec::new()),
            ("D", Vec::new()),
        ]);
        for (member_id, operation_id, outputs) in views {
            let private = bundle
                .private_configs
                .get(&member_id)
                .expect("private config");
            let blinded_outputs = outputs
                .iter()
                .map(|(_, output)| output.clone())
                .collect::<Vec<_>>();
            let share_set = private
                .sign_blinded_messages(&bundle.public_config, operation_id, &blinded_outputs)
                .expect("naive pre-consensus signature shares");

            assert_eq!(share_set.operation_id, operation_id);
            assert_eq!(share_set.member_id, member_id);
            assert_eq!(share_set.shares.len(), blinded_outputs.len());

            for ((label, _), share) in outputs.iter().zip(share_set.shares) {
                assert_eq!(share.operation_id, operation_id);
                shares_by_output
                    .get_mut(label)
                    .expect("output bucket")
                    .push(share.share);
            }
        }

        let threshold = usize::from(bundle.public_config.threshold_params.signature_threshold);
        let expected_signers = BTreeMap::from([
            ("A", vec![1, 3, 4]),
            ("B", vec![1, 2, 4]),
            ("C", vec![1, 2, 3]),
            ("D", vec![2, 3, 4]),
        ]);
        for (label, shares) in shares_by_output {
            assert_eq!(shares.len(), threshold);
            let signer_ids = shares
                .iter()
                .map(|share| share.signer_id().get())
                .collect::<Vec<_>>();
            assert_eq!(
                signer_ids,
                *expected_signers.get(label).expect("expected signer set")
            );

            aggregate_blind_signature_shares(threshold, &shares)
                .expect("naive regrouped shares aggregate");
        }
    }

    #[test]
    fn signing_rejects_non_bls_outputs() {
        let bundle = bundle();
        let keyset = &bundle.public_config.keysets[0];
        let operation_id = FederationOperationId::from_bytes([3; 32]);
        let output = BlindedMessage::new(
            Amount::from(1),
            keyset.id,
            PublicKey::from_hex(
                "02a8cda4cf448bfce9a9e46e588c06ea1780fcb94e3bbdf3277f42995d403a8b0c",
            )
            .expect("secp key"),
        );
        let private = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config");

        assert!(matches!(
            private.sign_blinded_messages(&bundle.public_config, operation_id, &[output]),
            Err(FederationError::InvalidFederationBlindedSecret { .. })
        ));
    }

    #[test]
    fn signing_rejects_non_v3_outputs() {
        let bundle = bundle();
        let keyset = &bundle.public_config.keysets[0];
        let operation_id = FederationOperationId::from_bytes([5; 32]);
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"non-v3 federation output");
        let non_v3_keyset_id = Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        );
        let output = BlindedMessage::new(
            Amount::from(1),
            non_v3_keyset_id,
            PublicKey::from(blinded_message),
        );
        let private = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config");

        assert!(matches!(
            private.sign_blinded_messages(&bundle.public_config, operation_id, &[output]),
            Err(FederationError::InvalidFederatedKeysetVersion { .. })
        ));
    }

    #[test]
    fn signing_rejects_missing_keyset_amount() {
        let bundle = bundle();
        let keyset = &bundle.public_config.keysets[0];
        let operation_id = FederationOperationId::from_bytes([4; 32]);
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"missing amount");
        let output =
            BlindedMessage::new(Amount::from(4), keyset.id, PublicKey::from(blinded_message));
        let private = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config");

        assert!(matches!(
            private.sign_blinded_messages(&bundle.public_config, operation_id, &[output]),
            Err(FederationError::UnknownFederatedKeysetAmount { .. })
        ));
    }

    #[test]
    fn signing_rejects_wrong_member_secret_share() {
        let bundle = bundle();
        let keyset = &bundle.public_config.keysets[0];
        let operation_id = FederationOperationId::from_bytes([6; 32]);
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"wrong member secret share");
        let output =
            BlindedMessage::new(Amount::from(1), keyset.id, PublicKey::from(blinded_message));

        let wrong_secret_share = bundle
            .private_configs
            .get(&member_id(2))
            .expect("second private config")
            .keyset_secret_shares
            .get(&keyset.id)
            .expect("second keyset shares")
            .get(&Amount::from(1))
            .expect("second amount share")
            .clone();
        let mut private = bundle
            .private_configs
            .get(&member_id(1))
            .expect("first private config")
            .clone();
        private
            .keyset_secret_shares
            .get_mut(&keyset.id)
            .expect("first keyset shares")
            .insert(Amount::from(1), wrong_secret_share);

        assert!(matches!(
            private.sign_blinded_messages(&bundle.public_config, operation_id, &[output]),
            Err(FederationError::SecretShareSignerMismatch {
                member_id: local_member_id,
                signer_id: 2,
                ..
            }) if local_member_id == member_id(1)
        ));
    }
}
