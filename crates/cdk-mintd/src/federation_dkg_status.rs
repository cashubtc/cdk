use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{bail, Result};
use cdk_common::federation::{
    redact_federation_operator_message, FederationConfig, FederationDkgFinalizedMemberConfig,
    FederationDkgMemberKeysetShare, FederationDkgSetupMethod,
    FederationDkgTranscriptSignatureRequest, MemberConfigPrivate, MemberId,
};

use crate::cli::{CLIArgs, FederationDkgStatusArgs};

pub(crate) fn run_dkg_status_command(
    cli_args: &CLIArgs,
    args: &FederationDkgStatusArgs,
) -> Result<()> {
    let (public_config_path, private_config_path) = crate::federation_config_paths(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation dkg-status",
    )?;
    let public_config: FederationConfig =
        crate::read_federation_json(&public_config_path, "public federation config")?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(&private_config_path, "private federation config")?;
    public_config.validate()?;
    private_config.validate_against_public(&public_config)?;

    let keyset_shares = read_record_files(&args.keyset_shares, "DKG keyset-share record")?;
    let transcript_signatures = read_record_files(
        &args.transcript_signatures,
        "DKG transcript-signature record",
    )?;
    let report = dkg_status_report(
        &public_config,
        &private_config,
        &keyset_shares,
        &transcript_signatures,
    );
    print!("{}", report.text);

    if args.require_ready && !report.ready {
        bail!("federation DKG finalization is not ready");
    }

    Ok(())
}

fn read_record_files<T>(paths: &[PathBuf], label: &str) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    paths
        .iter()
        .map(|path| crate::read_federation_json(path, label))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DkgStatusReport {
    text: String,
    ready: bool,
}

fn dkg_status_report(
    setup_public_config: &FederationConfig,
    local_private_config: &MemberConfigPrivate,
    keyset_shares: &[FederationDkgMemberKeysetShare],
    transcript_signatures: &[FederationDkgTranscriptSignatureRequest],
) -> DkgStatusReport {
    let keyset_ids = keyset_shares
        .iter()
        .map(|share| share.keyset.id.to_string())
        .collect::<BTreeSet<_>>();
    let transcript_hashes = transcript_signatures
        .iter()
        .map(|signature| signature.transcript_hash.to_string())
        .collect::<BTreeSet<_>>();
    let transcript_signers = transcript_signatures
        .iter()
        .map(|signature| signature.signer)
        .collect::<BTreeSet<_>>();
    let setup_transcript = setup_public_config
        .setup_transcript
        .as_ref()
        .map(|transcript| setup_method_label(transcript.payload.setup_method))
        .unwrap_or("none");

    let mut text = String::new();
    text.push_str(&format!(
        "Federation {} DKG status for member {}\n",
        setup_public_config.federation_id, local_private_config.member_id
    ));
    text.push_str(&format!(
        "setup_transcript={} setup_keysets={} keyset_share_records={} transcript_signature_records={}\n",
        setup_transcript,
        setup_public_config.keysets.len(),
        keyset_shares.len(),
        transcript_signatures.len()
    ));
    text.push_str(&format!(
        "keyset_share_ids={} transcript_hashes={} transcript_signers={}\n",
        display_string_set(&keyset_ids),
        display_string_set(&transcript_hashes),
        display_member_set(&transcript_signers)
    ));

    match FederationDkgFinalizedMemberConfig::from_records(
        setup_public_config,
        local_private_config,
        keyset_shares,
        transcript_signatures,
    ) {
        Ok(finalized) => {
            let transcript = finalized.public_config.setup_transcript.as_ref();
            let transcript_hash = transcript
                .map(|transcript| transcript.transcript_hash.to_string())
                .unwrap_or_else(|| "none".to_string());
            let transcript_signers = transcript
                .map(|transcript| transcript.signatures.keys().copied().collect())
                .unwrap_or_default();
            let local_amounts = finalized
                .private_config
                .keyset_secret_shares
                .values()
                .map(|shares| shares.len())
                .sum::<usize>();
            text.push_str(&format!(
                "finalized_ready=true final_keysets={} transcript_hash={} transcript_signers={} local_keysets={} local_amounts={}\n",
                finalized.public_config.keysets.len(),
                transcript_hash,
                display_member_set(&transcript_signers),
                finalized.private_config.keyset_secret_shares.len(),
                local_amounts
            ));
            DkgStatusReport { text, ready: true }
        }
        Err(err) => {
            text.push_str(&format!(
                "finalized_ready=false reason={}\n",
                redact_federation_operator_message(&err.to_string())
            ));
            DkgStatusReport { text, ready: false }
        }
    }
}

fn display_string_set(values: &BTreeSet<String>) -> String {
    if values.is_empty() {
        return "none".to_string();
    }

    values.iter().cloned().collect::<Vec<_>>().join(",")
}

fn display_member_set(values: &BTreeSet<MemberId>) -> String {
    if values.is_empty() {
        return "none".to_string();
    }

    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn setup_method_label(setup_method: FederationDkgSetupMethod) -> &'static str {
    match setup_method {
        FederationDkgSetupMethod::TrustedDealer => "trusted_dealer",
        FederationDkgSetupMethod::PedersenDkg => "pedersen_dkg",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use cdk_common::federation::{
        generate_trusted_dealer_configs, FederatedKeyset, FederationDkgTranscript,
        FederationDkgTranscriptSignature, MemberId, ThresholdParams, TrustedDealerKeysetSpec,
        TrustedDealerMember,
    };
    use cdk_common::mint_url::MintUrl;
    use cdk_common::nuts::nut01::BlsSecretKey;
    use cdk_common::{Amount, CurrencyUnit, SecretKey};

    use super::*;

    struct DkgCompletionFixture {
        setup_public_config: FederationConfig,
        setup_private_config: MemberConfigPrivate,
        keyset_share: FederationDkgMemberKeysetShare,
        transcript_signatures: Vec<FederationDkgTranscriptSignatureRequest>,
    }

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("identity secret")
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

    fn dkg_completion_fixture(local_member: MemberId) -> DkgCompletionFixture {
        let bundle = generate_trusted_dealer_configs(
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
                BTreeMap::from([
                    (Amount::from(1), vec![bls_key(11), bls_key(12)]),
                    (Amount::from(2), vec![bls_key(21), bls_key(22)]),
                ]),
            )],
        )
        .expect("trusted dealer bundle");

        let mut final_public_config = bundle.public_config.clone();
        final_public_config.setup_transcript = None;
        let transcript_hash = FederationDkgTranscript::hash_for_config(
            FederationDkgSetupMethod::PedersenDkg,
            &final_public_config,
        )
        .expect("pedersen transcript hash");
        let transcript_signatures = bundle
            .private_configs
            .values()
            .map(|private_config| {
                FederationDkgTranscriptSignatureRequest::new(
                    final_public_config.federation_id,
                    private_config.member_id,
                    transcript_hash,
                    FederationDkgTranscriptSignature::sign(transcript_hash, private_config)
                        .expect("transcript signature"),
                )
            })
            .collect::<Vec<_>>();

        let final_keyset = final_public_config
            .keysets
            .first()
            .expect("federated keyset")
            .clone();
        let local_private_config = bundle
            .private_configs
            .get(&local_member)
            .expect("local private config");
        let keyset_share = keyset_share_from_private(
            final_public_config.federation_id,
            local_member,
            &final_keyset,
            local_private_config,
        );

        let mut setup_public_config = final_public_config;
        setup_public_config.keysets.clear();
        setup_public_config.validate().expect("setup public config");

        let mut setup_private_config = local_private_config.clone();
        setup_private_config.keyset_secret_shares.clear();
        setup_private_config.setup_transcript_hash = None;
        setup_private_config
            .validate_against_public(&setup_public_config)
            .expect("setup private config");

        DkgCompletionFixture {
            setup_public_config,
            setup_private_config,
            keyset_share,
            transcript_signatures,
        }
    }

    fn keyset_share_from_private(
        federation_id: cdk_common::federation::FederationId,
        local_member: MemberId,
        keyset: &FederatedKeyset,
        private_config: &MemberConfigPrivate,
    ) -> FederationDkgMemberKeysetShare {
        FederationDkgMemberKeysetShare {
            federation_id,
            member_id: local_member,
            keyset: keyset.clone(),
            secret_shares: private_config
                .keyset_secret_shares
                .get(&keyset.id)
                .expect("local keyset secret shares")
                .clone(),
        }
    }

    #[test]
    fn dkg_status_report_marks_finalized_records_ready_without_leaking_private_shares() {
        let fixture = dkg_completion_fixture(member_id(2));

        let report = dkg_status_report(
            &fixture.setup_public_config,
            &fixture.setup_private_config,
            std::slice::from_ref(&fixture.keyset_share),
            &fixture.transcript_signatures,
        );

        assert!(report.ready);
        assert!(report.text.contains("DKG status for member 2"));
        assert!(report.text.contains("setup_keysets=0"));
        assert!(report.text.contains("keyset_share_records=1"));
        assert!(report.text.contains("transcript_signature_records=3"));
        assert!(report.text.contains("transcript_signers=1,2,3"));
        assert!(report.text.contains("finalized_ready=true"));
        assert!(report.text.contains("final_keysets=1"));
        assert!(report.text.contains("local_keysets=1 local_amounts=2"));
        for private_fragment in [
            "identity_secret_key",
            "keyset_secret_shares",
            "BlsSecretKeyShare",
        ] {
            assert!(!report.text.contains(private_fragment));
        }
    }

    #[test]
    fn dkg_status_report_marks_missing_transcript_signature_not_ready() {
        let mut fixture = dkg_completion_fixture(member_id(2));
        fixture
            .transcript_signatures
            .retain(|signature| signature.signer != member_id(3));

        let report = dkg_status_report(
            &fixture.setup_public_config,
            &fixture.setup_private_config,
            std::slice::from_ref(&fixture.keyset_share),
            &fixture.transcript_signatures,
        );

        assert!(!report.ready);
        assert!(report.text.contains("transcript_signature_records=2"));
        assert!(report.text.contains("finalized_ready=false"));
        assert!(report
            .text
            .contains("missing transcript signature for member 3"));
        assert!(!report.text.contains("keyset_secret_shares"));
    }
}
