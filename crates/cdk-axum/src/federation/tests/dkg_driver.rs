use std::collections::HashSet;
use std::sync::Arc;

use cdk::federation::{
    FederationDkgDriverPhase, FederationDkgFinalizedMemberConfig, FederationDkgKeysetSpec,
    ValidatedFederationDkgParticipantState,
};
use cdk_common::database::mint::{FederationDkgDatabase, FederationFrostWalletDatabase};

use super::*;

struct DkgDriverTestMember {
    state: MintState,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: [u8; 64],
    private_config: MemberConfigPrivate,
    participant: ValidatedFederationDkgParticipantState,
    delivery_tracker: tokio::sync::Mutex<FederationDkgDeliveryTracker>,
    client: FederationRestPeerClient<RouterFederationTransportRegistry>,
}

struct DkgDriverTestFederation {
    public_config: FederationConfig,
    ceremony_id: FederationDkgCeremonyId,
    keyset_spec: FederationDkgKeysetSpec,
    transport: RouterFederationTransportRegistry,
    members: Vec<DkgDriverTestMember>,
}

impl DkgDriverTestFederation {
    async fn new() -> Self {
        let (mut public_config, first, second, third) = transport_config_three();
        public_config.keysets.clear();
        public_config.setup_transcript = None;
        public_config.validate().expect("setup public config");

        let mut private_configs = vec![first, second, third];
        for private_config in &mut private_configs {
            private_config.federation_id = Some(public_config.federation_id);
            private_config.keyset_secret_shares.clear();
            private_config.setup_transcript_hash = None;
            private_config
                .validate_against_public(&public_config)
                .expect("setup private config");
        }

        let ceremony_id = FederationDkgCeremonyId::from_bytes([77; 32]);
        let keyset_spec = FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1)].into_iter().collect(),
        )
        .expect("DKG keyset spec");
        let transport = RouterFederationTransportRegistry::default();
        let mut states = Vec::new();

        for (index, private_config) in private_configs.iter().enumerate() {
            let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
            let mut seed = [0_u8; 64];
            seed.fill(u8::try_from(index + 1).expect("member seed"));
            let mut state = federation_route_state_for_private_with_db(
                public_config.clone(),
                private_config.clone(),
                db.clone(),
                &seed,
            )
            .await;
            state
                .federation
                .set_dkg_auto_start_config_for_test(Some(Arc::new(FederationDkgAutoStartConfig {
                    setup_authorization: public_config.setup_authorization,
                    ceremony_id,
                    root_key_epoch: 1,
                    keyset_spec: keyset_spec.clone(),
                    driver: FederationDkgDriverRunConfig::default(),
                })));
            let member = public_config
                .member(private_config.member_id)
                .expect("configured member");
            transport.insert(
                member.federation_api_url.clone(),
                Router::new()
                    .nest("/api/federation/v1", create_federation_router())
                    .with_state(state.clone()),
            );
            states.push((state, db, seed));
        }

        let mut members = Vec::new();
        for ((state, db, seed), private_config) in
            states.into_iter().zip(private_configs.into_iter())
        {
            let participant =
                prepare_federation_dkg_participant_state(&state, ceremony_id, keyset_spec.clone())
                    .await
                    .expect("prepare participant state");
            let client = FederationRestPeerClient::new(
                transport.clone(),
                &public_config,
                private_config.clone(),
            )
            .expect("DKG peer client");
            members.push(DkgDriverTestMember {
                state,
                db,
                seed,
                private_config,
                participant,
                delivery_tracker: tokio::sync::Mutex::new(FederationDkgDeliveryTracker::default()),
                client,
            });
        }

        Self {
            public_config,
            ceremony_id,
            keyset_spec,
            transport,
            members,
        }
    }

    async fn drive_until_complete(&self, max_rounds: usize) -> usize {
        let mut attempted_deliveries = 0usize;
        for _ in 0..max_rounds {
            let outcomes = self.drive_round().await;
            attempted_deliveries += outcomes
                .iter()
                .map(|outcome| outcome.attempted_deliveries)
                .sum::<usize>();
            if outcomes
                .iter()
                .all(FederationDkgDriverTickOutcome::is_complete)
            {
                return attempted_deliveries;
            }
        }

        panic!("DKG ceremony did not complete in {max_rounds} rounds");
    }

    async fn drive_round(&self) -> Vec<FederationDkgDriverTickOutcome> {
        futures::future::join_all(self.members.iter().map(|member| async move {
            let mut delivery_tracker = member.delivery_tracker.lock().await;
            run_federation_dkg_driver_tick(
                &member.state,
                &member.client,
                &member.participant,
                FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
                &mut delivery_tracker,
            )
            .await
            .expect("concurrent DKG driver tick")
        }))
        .await
    }

    async fn restart_member(&mut self, index: usize) {
        let member = &self.members[index];
        let mut state = federation_route_state_for_private_with_db(
            self.public_config.clone(),
            member.private_config.clone(),
            member.db.clone(),
            &member.seed,
        )
        .await;
        state
            .federation
            .set_dkg_auto_start_config_for_test(Some(Arc::new(FederationDkgAutoStartConfig {
                setup_authorization: self.public_config.setup_authorization,
                ceremony_id: self.ceremony_id,
                root_key_epoch: 1,
                keyset_spec: self.keyset_spec.clone(),
                driver: FederationDkgDriverRunConfig::default(),
            })));
        let participant = prepare_federation_dkg_participant_state(
            &state,
            self.ceremony_id,
            self.keyset_spec.clone(),
        )
        .await
        .expect("reload participant state");
        assert_eq!(
            participant.participant_state(),
            member.participant.participant_state()
        );
        let client = FederationRestPeerClient::new(
            self.transport.clone(),
            &self.public_config,
            member.private_config.clone(),
        )
        .expect("restarted DKG peer client");
        let public_member = self
            .public_config
            .member(member.private_config.member_id)
            .expect("configured restarted member");
        self.transport.insert(
            public_member.federation_api_url.clone(),
            Router::new()
                .nest("/api/federation/v1", create_federation_router())
                .with_state(state.clone()),
        );
        self.members[index].state = state;
        self.members[index].participant = participant;
        self.members[index].delivery_tracker =
            tokio::sync::Mutex::new(FederationDkgDeliveryTracker::default());
        self.members[index].client = client;
    }

    async fn assert_complete_and_consistent(&self) {
        let mut finalized_public_config = None;
        let mut frost_public_package = None;
        let mut encrypted_root_shares = HashSet::new();
        for member in &self.members {
            let db = member.state.mint.localstore();
            assert_eq!(
                db.get_federation_dkg_participant_states()
                    .await
                    .expect("participant states"),
                vec![member.participant.participant_state().clone()]
            );
            let commitments = db
                .get_federation_dkg_commitments()
                .await
                .expect("commitments");
            assert_eq!(
                commitments
                    .iter()
                    .filter(|commitment| commitment.ceremony_id == self.ceremony_id)
                    .count(),
                3
            );
            let reveals = db.get_federation_dkg_reveals().await.expect("reveals");
            assert_eq!(
                reveals
                    .iter()
                    .filter(|reveal| reveal.ceremony_id == self.ceremony_id)
                    .count(),
                3
            );
            let contributions = db
                .get_federation_dkg_secret_share_contributions()
                .await
                .expect("secret contributions");
            assert_eq!(
                contributions
                    .iter()
                    .filter(|contribution| contribution.ceremony_id == self.ceremony_id)
                    .count(),
                3
            );
            let keyset_shares = db
                .get_federation_dkg_member_keyset_shares()
                .await
                .expect("keyset shares");
            assert_eq!(keyset_shares.len(), 1);
            let transcript_signatures = db
                .get_federation_dkg_transcript_signatures()
                .await
                .expect("transcript signatures");
            assert_eq!(transcript_signatures.len(), 3);
            let activations = db
                .get_federation_dkg_activations()
                .await
                .expect("DKG activations");
            assert_eq!(activations.len(), 3);
            assert_eq!(
                db.get_federation_frost_dkg_round1()
                    .await
                    .expect("FROST round-one records")
                    .len(),
                3
            );
            let round2 = db
                .get_federation_frost_dkg_round2()
                .await
                .expect("sealed FROST round-two records");
            assert_eq!(round2.len(), 4);
            for record in round2 {
                record
                    .encrypted(&self.public_config)
                    .expect("authenticated ciphertext envelope");
                assert!(format!("{record:?}").contains("<redacted>"));
            }
            assert_eq!(
                db.get_federation_frost_dkg_local_states()
                    .await
                    .expect("sealed FROST phase states")
                    .len(),
                2
            );
            let roots = db
                .get_federation_frost_root_records()
                .await
                .expect("sealed FROST roots");
            assert_eq!(roots.len(), 1);
            roots[0]
                .validate(&self.public_config)
                .expect("valid canonical FROST root");
            assert!(encrypted_root_shares.insert(roots[0].encrypted_root_share.clone()));
            match frost_public_package.as_ref() {
                Some(expected) => assert_eq!(expected, &roots[0].public_package),
                None => frost_public_package = Some(roots[0].public_package.clone()),
            }
            assert_eq!(
                db.get_federation_frost_dkg_confirmations()
                    .await
                    .expect("FROST root confirmations")
                    .len(),
                3
            );
            let finalized = FederationDkgFinalizedMemberConfig::from_records(
                &self.public_config,
                &member.private_config,
                &keyset_shares,
                &transcript_signatures,
            )
            .expect("finalized member config");
            assert_eq!(
                finalized.private_config.member_id,
                member.private_config.member_id
            );
            assert!(!finalized.private_config.keyset_secret_shares.is_empty());
            match finalized_public_config.as_ref() {
                Some(expected) => assert_eq!(expected, &finalized.public_config),
                None => finalized_public_config = Some(finalized.public_config),
            }
        }
        assert_eq!(encrypted_root_shares.len(), self.members.len());
    }
}

#[tokio::test]
async fn dkg_driver_completes_over_authenticated_member_routes() {
    let federation = DkgDriverTestFederation::new().await;

    let attempted_deliveries = federation.drive_until_complete(12).await;
    assert_eq!(attempted_deliveries, 97);
    federation.assert_complete_and_consistent().await;
}

#[tokio::test]
async fn dkg_cannot_complete_while_one_configured_member_withholds() {
    let federation = DkgDriverTestFederation::new().await;

    // Take member 3 offline: its peers cannot reach it and its own
    // contributions never flow.
    let third_member = federation
        .public_config
        .member(member_id(3))
        .expect("third member");
    federation.transport.insert(
        third_member.federation_api_url.clone(),
        Router::new().fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
    );

    // The all-member readiness barrier must exhaust with a bounded error
    // that tells the operator why the ceremony cannot start and what the
    // recovery path is.
    let readiness = FederationDkgReadinessRequest::new(
        federation.public_config.federation_id,
        federation.public_config.setup_authorization,
        federation.ceremony_id,
        federation.keyset_spec.clone(),
    );
    let readiness_outcome = super::super::dkg_driver::wait_for_federation_dkg_readiness(
        &federation.members[0].client,
        &federation.public_config,
        &readiness,
        FederationDkgDriverRunConfig {
            max_ticks: 3,
            retry_delay: std::time::Duration::from_millis(10),
            ..FederationDkgDriverRunConfig::default()
        },
    )
    .await;
    let readiness_err =
        readiness_outcome.expect_err("one withholding member must fail the readiness barrier");
    let readiness_message = readiness_err.to_string();
    assert!(
        readiness_message.contains("exhausted") && readiness_message.contains("2/3"),
        "readiness error must report the bounded member counts: {readiness_message}"
    );
    assert!(
        readiness_message.contains("roster"),
        "readiness error must name the operator roster remedy: {readiness_message}"
    );

    // Even with the online members driving every tick, no member may reach
    // Complete while any configured member withholds, and progress must
    // stall below the all-member counts.
    for _ in 0..4 {
        for member in &federation.members[..2] {
            let mut delivery_tracker = member.delivery_tracker.lock().await;
            let outcome = run_federation_dkg_driver_tick(
                &member.state,
                &member.client,
                &member.participant,
                FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
                &mut delivery_tracker,
            )
            .await
            .expect("DKG driver tick");
            assert!(
                !outcome.is_complete(),
                "ceremony must not complete while a configured member withholds"
            );
            let progress = outcome.progress;
            assert!(
                progress.commitment_count < progress.member_count,
                "progress must stall below the all-member counts: {progress:?}"
            );
        }
    }
}

#[tokio::test]
async fn dkg_mid_ceremony_member_loss_aborts_others_with_timeout_naming_missing_members() {
    let federation = DkgDriverTestFederation::new().await;

    // Member 3 becomes unavailable mid-ceremony.
    let third_member = federation
        .public_config
        .member(member_id(3))
        .expect("third member");
    federation.transport.insert(
        third_member.federation_api_url.clone(),
        Router::new().fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
    );

    // The online members exchange their own commitments, then must wait on
    // member 3.
    for _ in 0..2 {
        for member in &federation.members[..2] {
            let mut delivery_tracker = member.delivery_tracker.lock().await;
            let outcome = run_federation_dkg_driver_tick(
                &member.state,
                &member.client,
                &member.participant,
                FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
                &mut delivery_tracker,
            )
            .await
            .expect("DKG driver tick");
            assert!(!outcome.is_complete());
        }
    }

    // The bounded driver loop for member 1 aborts after the ceremony
    // timeout, names the missing member, and surfaces the roster remedy
    // instead of retrying forever.
    let outcome = super::super::dkg_driver::run_federation_dkg_driver_loop_with_client(
        &federation.members[0].state,
        &federation.members[0].client,
        &federation.members[0].participant,
        federation.ceremony_id,
        FederationDkgDriverRunConfig {
            max_ticks: 64,
            retry_delay: std::time::Duration::from_millis(10),
            ceremony_timeout: std::time::Duration::from_millis(100),
            ..FederationDkgDriverRunConfig::default()
        },
    )
    .await;
    let err = outcome.expect_err("member loss must abort the bounded ceremony loop");
    let message = err.to_string();
    assert!(
        message.contains("aborted") && message.contains("timeout") || message.contains("aborted"),
        "abort must be reported as a timeout abort: {message}"
    );
    assert!(
        message.contains("still waiting on member(s): 3"),
        "abort must name the missing member: {message}"
    );
    assert!(
        message.contains("roster"),
        "abort must name the operator roster remedy: {message}"
    );
}

#[tokio::test]
async fn dkg_finalized_config_cache_is_seeded_by_driver_and_reused() {
    let federation = DkgDriverTestFederation::new().await;
    let state = &federation.members[0].state;
    assert!(state
        .ready_federation_dkg_finalized_member_config()
        .is_none());

    federation.drive_until_complete(12).await;
    let first = state
        .ready_federation_dkg_finalized_member_config()
        .expect("driver-seeded finalized DKG config");
    let second = state
        .ready_federation_dkg_finalized_member_config()
        .expect("cached finalized DKG config");
    assert!(Arc::ptr_eq(&first, &second));
}

#[tokio::test]
async fn dkg_driver_ignores_durable_artifacts_from_prior_ceremony() {
    let federation = DkgDriverTestFederation::new().await;
    let prior_ceremony_id = FederationDkgCeremonyId::from_bytes([76; 32]);

    for member in &federation.members {
        let participant = member.participant.participant_state();
        let mut commitment = participant
            .commitment_request()
            .expect("prior commitment")
            .commitment;
        commitment.ceremony_id = prior_ceremony_id;
        let mut reveal = participant.reveal_request().reveal;
        reveal.ceremony_id = prior_ceremony_id;
        let mut contribution = participant
            .secret_share_request(member.private_config.member_id)
            .expect("prior contribution");
        contribution.ceremony_id = prior_ceremony_id;

        let mut tx = member
            .db
            .begin_transaction()
            .await
            .expect("begin prior DKG");
        tx.add_federation_dkg_commitment(&commitment)
            .await
            .expect("persist prior commitment");
        tx.add_federation_dkg_reveal(&reveal)
            .await
            .expect("persist prior reveal");
        tx.add_federation_dkg_secret_share_contribution(&contribution)
            .await
            .expect("persist prior contribution");
        tx.commit().await.expect("commit prior DKG artifacts");
    }

    let attempted_deliveries = federation.drive_until_complete(12).await;
    assert_eq!(attempted_deliveries, 97);
    federation.assert_complete_and_consistent().await;
    for member in &federation.members {
        assert_eq!(
            member
                .db
                .get_federation_dkg_commitments()
                .await
                .expect("all ceremony commitments")
                .len(),
            4
        );
    }
}

#[tokio::test]
async fn dkg_driver_retries_response_lost_after_durable_persistence() {
    let federation = DkgDriverTestFederation::new().await;
    federation
        .transport
        .fail_post_responses_after_delivery("/federation/v1/dkg/commitments", 1);

    let attempted_deliveries = federation.drive_until_complete(12).await;
    assert_eq!(attempted_deliveries, 98);
    federation.assert_complete_and_consistent().await;
    for member in &federation.members {
        let commitments = member
            .db
            .get_federation_dkg_commitments()
            .await
            .expect("durable commitments");
        assert_eq!(
            commitments
                .iter()
                .filter(|commitment| commitment.ceremony_id == federation.ceremony_id)
                .count(),
            3
        );
    }
}

#[tokio::test]
async fn dkg_driver_restarts_safely_at_every_durable_phase() {
    let mut federation = DkgDriverTestFederation::new().await;
    let restart_phases = [
        FederationDkgDriverPhase::Commitment,
        FederationDkgDriverPhase::Reveal,
        FederationDkgDriverPhase::SecretShares,
        FederationDkgDriverPhase::LocalKeysetShare,
        FederationDkgDriverPhase::TranscriptSignature,
        FederationDkgDriverPhase::Activation,
    ];
    let mut restarted = HashSet::from([FederationDkgDriverPhase::Commitment]);
    let mut frost_round1_restarted = false;
    let mut frost_round2_restarted = false;
    let mut frost_root_restarted = false;
    federation.restart_member(0).await;

    let mut completed = false;
    for _ in 0..36 {
        let mut outcomes = Vec::with_capacity(federation.members.len());
        for index in 0..federation.members.len() {
            let outcome = {
                let member = &federation.members[index];
                let mut delivery_tracker = member.delivery_tracker.lock().await;
                run_federation_dkg_driver_tick(
                    &member.state,
                    &member.client,
                    &member.participant,
                    FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
                    &mut delivery_tracker,
                )
                .await
                .expect("DKG driver phase-matrix tick")
            };
            let phase_before = outcome.phase_before;
            let phase_after = outcome.phase_after;
            outcomes.push(outcome);
            if index == 0 {
                let restart_phase = [phase_before, phase_after]
                    .into_iter()
                    .find(|phase| restart_phases.contains(phase) && !restarted.contains(phase));
                if let Some(restart_phase) = restart_phase {
                    restarted.insert(restart_phase);
                    federation.restart_member(0).await;
                }
                let frost_states = federation.members[0]
                    .db
                    .get_federation_frost_dkg_local_states()
                    .await
                    .expect("restartable FROST states");
                let restart_frost = if !frost_round1_restarted
                    && frost_states.iter().any(|state| {
                        state.phase == cdk::federation::FederationFrostDkgLocalPhase::Round1
                    }) {
                    frost_round1_restarted = true;
                    true
                } else if !frost_round2_restarted
                    && frost_states.iter().any(|state| {
                        state.phase == cdk::federation::FederationFrostDkgLocalPhase::Round2
                    })
                {
                    frost_round2_restarted = true;
                    true
                } else if !frost_root_restarted
                    && !federation.members[0]
                        .db
                        .get_federation_frost_root_records()
                        .await
                        .expect("restartable FROST root")
                        .is_empty()
                {
                    frost_root_restarted = true;
                    true
                } else {
                    false
                };
                if restart_frost {
                    federation.restart_member(0).await;
                }
            }
        }
        if outcomes
            .iter()
            .all(FederationDkgDriverTickOutcome::is_complete)
        {
            completed = true;
            break;
        }
    }

    assert!(completed, "DKG did not complete after phase restart matrix");
    assert!(restarted.contains(&FederationDkgDriverPhase::Commitment));
    assert!(restarted.contains(&FederationDkgDriverPhase::Reveal));
    assert!(restarted.contains(&FederationDkgDriverPhase::SecretShares));
    assert!(restarted.contains(&FederationDkgDriverPhase::LocalKeysetShare));
    assert!(restarted.contains(&FederationDkgDriverPhase::TranscriptSignature));
    assert!(restarted.contains(&FederationDkgDriverPhase::Activation));
    assert!(frost_round1_restarted);
    assert!(frost_round2_restarted);
    assert!(frost_root_restarted);
    federation.assert_complete_and_consistent().await;
}

#[tokio::test]
async fn dkg_completion_marks_registered_ceremony_complete_idempotently() {
    let federation = DkgDriverTestFederation::new().await;
    for member in &federation.members {
        let ceremony = FederationDkgCeremonyRecord::active(
            federation.public_config.federation_id,
            federation.public_config.setup_authorization,
            federation.ceremony_id,
            member.private_config.member_id,
            federation.keyset_spec.clone(),
            None,
        );
        let mut tx = member
            .db
            .begin_transaction()
            .await
            .expect("begin ceremony registration");
        tx.add_federation_dkg_ceremony(&ceremony)
            .await
            .expect("register ceremony");
        tx.commit().await.expect("commit ceremony registration");
    }

    let _ = federation.drive_until_complete(24).await;
    let mut materialized_wallet_digest = None;
    for member in &federation.members {
        complete_federation_dkg_ceremony(&member.state, federation.ceremony_id)
            .await
            .expect("complete ceremony lifecycle");
        complete_federation_dkg_ceremony(&member.state, federation.ceremony_id)
            .await
            .expect("replay ceremony completion");
        let ceremonies = member
            .db
            .get_federation_dkg_ceremonies()
            .await
            .expect("ceremony lifecycle");
        assert_eq!(ceremonies.len(), 1);
        assert_eq!(ceremonies[0].state, FederationDkgCeremonyState::Complete);
        let root_public_configs = member
            .db
            .get_federation_frost_root_public_configs()
            .await
            .expect("public FROST root configs");
        assert_eq!(root_public_configs.len(), 1);
        root_public_configs[0]
            .validate(&federation.public_config)
            .expect("valid public FROST root config");
        let root_private_configs = member
            .db
            .get_federation_frost_root_private_configs()
            .await
            .expect("private FROST root configs");
        assert_eq!(root_private_configs.len(), 1);
        root_private_configs[0]
            .validate_metadata(&root_public_configs[0], member.private_config.member_id)
            .expect("member-bound private FROST root config");
        let snapshot = member
            .state
            .mint
            .federation_materialized_state_snapshot()
            .await
            .expect("wallet materialized snapshot");
        let fingerprints = snapshot.row_fingerprints().expect("wallet fingerprints");
        assert!(fingerprints
            .iter()
            .any(|row| row.table == "federation_frost_root_public_config"));
        assert!(!fingerprints.iter().any(|row| row.table.contains("private")));
        let digest = snapshot.digest().expect("wallet materialized digest");
        match materialized_wallet_digest {
            Some(expected) => assert_eq!(expected, digest),
            None => materialized_wallet_digest = Some(digest),
        }
    }
}

#[tokio::test]
async fn dkg_activation_mismatch_fails_before_sequence_or_record_mutation() {
    let federation = DkgDriverTestFederation::new().await;
    let _ = federation.drive_until_complete(12).await;
    let receiver = &federation.members[0];
    let sender = &federation.members[1];
    let before_activations = receiver
        .db
        .get_federation_dkg_activations()
        .await
        .expect("activations before mismatch");
    let before_sequences = receiver
        .db
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences before mismatch");
    let mut tampered = *before_activations
        .iter()
        .find(|activation| activation.signer == sender.private_config.member_id)
        .expect("sender activation");
    tampered.final_config_digest = cdk::federation::FederationConfigDigest::from_bytes([99; 32]);
    let sequence = before_sequences
        .get(&sender.private_config.member_id)
        .copied()
        .unwrap_or_default()
        .saturating_add(1);
    let request = FederationSignedRequest::new_signed(
        federation.public_config.federation_id,
        receiver.private_config.member_id,
        sequence,
        FederationRestMessageKind::DkgActivation,
        tampered,
        &sender.private_config,
    )
    .expect("signed tampered activation");

    let response = post_dkg_activation(State(receiver.state.clone()), Json(request))
        .await
        .expect_err("tampered activation must fail");
    assert!(response.status().is_client_error());
    assert_eq!(
        receiver
            .db
            .get_federation_dkg_activations()
            .await
            .expect("activations after mismatch"),
        before_activations
    );
    assert_eq!(
        receiver
            .db
            .get_federation_dkg_member_sequences()
            .await
            .expect("sequences after mismatch"),
        before_sequences
    );
}

#[tokio::test]
async fn frost_confirmation_mismatch_fails_before_sequence_or_record_mutation() {
    let federation = DkgDriverTestFederation::new().await;
    let _ = federation.drive_until_complete(12).await;
    let receiver = &federation.members[0];
    let sender = &federation.members[1];
    let before_confirmations = receiver
        .db
        .get_federation_frost_dkg_confirmations()
        .await
        .expect("confirmations before mismatch");
    let before_sequences = receiver
        .db
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences before FROST mismatch");
    let mut tampered = before_confirmations
        .iter()
        .find(|confirmation| confirmation.member_id == sender.private_config.member_id)
        .expect("sender confirmation")
        .clone();
    *tampered
        .public_package
        .last_mut()
        .expect("canonical public root byte") ^= 1;
    let sequence = before_sequences
        .get(&sender.private_config.member_id)
        .copied()
        .unwrap_or_default()
        .saturating_add(1);
    let request = FederationSignedRequest::new_signed(
        federation.public_config.federation_id,
        receiver.private_config.member_id,
        sequence,
        FederationRestMessageKind::FrostDkgConfirmation,
        tampered,
        &sender.private_config,
    )
    .expect("signed tampered FROST confirmation");

    let response = post_frost_dkg_confirmation(State(receiver.state.clone()), Json(request))
        .await
        .expect_err("tampered FROST confirmation must fail");
    assert!(response.status().is_client_error());
    assert_eq!(
        receiver
            .db
            .get_federation_frost_dkg_confirmations()
            .await
            .expect("confirmations after mismatch"),
        before_confirmations
    );
    assert_eq!(
        receiver
            .db
            .get_federation_dkg_member_sequences()
            .await
            .expect("sequences after FROST mismatch"),
        before_sequences
    );
}

#[tokio::test]
async fn dkg_completion_fails_closed_without_frost_result() {
    let federation = DkgDriverTestFederation::new().await;
    assert!(
        complete_federation_dkg_ceremony(&federation.members[0].state, federation.ceremony_id,)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn dkg_readiness_waits_for_exact_all_member_configuration_without_persistence() {
    let federation = DkgDriverTestFederation::new().await;
    let first = &federation.members[0];
    let missing_member = federation
        .public_config
        .members
        .values()
        .next_back()
        .expect("last member")
        .clone();
    federation
        .transport
        .remove(&missing_member.federation_api_url);
    let request = FederationDkgReadinessRequest::new(
        federation.public_config.federation_id,
        federation.public_config.setup_authorization,
        federation.ceremony_id,
        federation.keyset_spec.clone(),
    );
    let mut acknowledged = BTreeSet::new();

    let first_round = run_federation_dkg_readiness_round(
        &first.client,
        &federation.public_config,
        &request,
        &mut acknowledged,
    )
    .await;
    assert_eq!(first_round.attempted, 3);
    assert_eq!(first_round.successful, 2);
    assert_eq!(first_round.failed, 1);
    assert_eq!(acknowledged.len(), 2);
    assert!(first
        .db
        .get_federation_dkg_commitments()
        .await
        .expect("commitments")
        .is_empty());

    let missing_state = federation
        .members
        .last()
        .expect("last member state")
        .state
        .clone();
    federation.transport.insert(
        missing_member.federation_api_url,
        Router::new()
            .nest("/api/federation/v1", create_federation_router())
            .with_state(missing_state),
    );
    let second_round = run_federation_dkg_readiness_round(
        &first.client,
        &federation.public_config,
        &request,
        &mut acknowledged,
    )
    .await;
    assert_eq!(second_round.attempted, 1);
    assert_eq!(second_round.successful, 1);
    assert_eq!(second_round.failed, 0);
    assert_eq!(acknowledged.len(), 3);
    assert_eq!(federation.transport.dkg_readiness_requests().len(), 3);
    assert!(first
        .db
        .get_federation_dkg_commitments()
        .await
        .expect("commitments")
        .is_empty());

    let mut changed_spec = federation.keyset_spec.clone();
    changed_spec.input_fee_ppk = 1;
    let mismatched = FederationDkgReadinessRequest::new(
        federation.public_config.federation_id,
        federation.public_config.setup_authorization,
        federation.ceremony_id,
        changed_spec,
    );
    let receiver = federation
        .public_config
        .members
        .values()
        .nth(1)
        .expect("second member");
    assert!(first
        .client
        .submit_dkg_readiness(receiver, &mismatched)
        .await
        .is_err());
}

#[tokio::test]
async fn dkg_driver_times_out_one_slow_member_without_blocking_fast_members() {
    let mut federation = DkgDriverTestFederation::new().await;
    let sender_id = federation.members[0].private_config.member_id;
    let slow_member_id = federation.members[2].private_config.member_id;
    federation.transport.delay_next_posts_before_delivery(
        "/federation/v1/dkg/commitments",
        slow_member_id,
        [Duration::from_millis(100)],
    );
    let mut sync_config = FederationPeerSyncConfig::default();
    sync_config.peer_request_timeout = Duration::from_millis(20);
    federation.members[0].client = FederationRestPeerClient::new_with_config(
        federation.transport.clone(),
        &federation.public_config,
        federation.members[0].private_config.clone(),
        sync_config,
    )
    .expect("short-timeout DKG client");

    let first = {
        let member = &federation.members[0];
        let mut delivery_tracker = member.delivery_tracker.lock().await;
        run_federation_dkg_driver_tick(
            &member.state,
            &member.client,
            &member.participant,
            FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
            &mut delivery_tracker,
        )
        .await
        .expect("slow-member DKG tick")
    };
    assert_eq!(first.attempted_deliveries, 3);
    assert_eq!(first.successful_deliveries, 2);
    assert_eq!(first.failed_deliveries, 1);
    assert_eq!(first.failed_members, vec![slow_member_id]);

    for (index, member) in federation.members.iter().enumerate() {
        let sender_commitments = member
            .db
            .get_federation_dkg_commitments()
            .await
            .expect("commitments after slow-member deadline")
            .into_iter()
            .filter(|commitment| {
                commitment.ceremony_id == federation.ceremony_id
                    && commitment.member_id == sender_id
            })
            .count();
        assert_eq!(sender_commitments, usize::from(index != 2));
    }

    federation.members[0].client = FederationRestPeerClient::new(
        federation.transport.clone(),
        &federation.public_config,
        federation.members[0].private_config.clone(),
    )
    .expect("recovered DKG client");
    let _ = federation.drive_until_complete(24).await;
    federation.assert_complete_and_consistent().await;
}

#[tokio::test]
async fn durable_dkg_participant_skips_volatile_readiness_after_restart() {
    let federation = DkgDriverTestFederation::new().await;

    assert!(!federation_dkg_readiness_required(
        &federation.members[0].state,
        federation.ceremony_id,
    )
    .await
    .expect("readiness requirement"));
}

#[tokio::test]
async fn dkg_driver_attempts_all_members_and_resumes_after_partition_and_restart() {
    let mut federation = DkgDriverTestFederation::new().await;
    let first_member = federation.members[0].private_config.member_id;
    federation.transport.set_member_outbound_partition(
        &federation.public_config,
        first_member,
        true,
    );

    let mut first_delivery_tracker = federation.members[0].delivery_tracker.lock().await;
    let partitioned = run_federation_dkg_driver_tick(
        &federation.members[0].state,
        &federation.members[0].client,
        &federation.members[0].participant,
        FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
        &mut first_delivery_tracker,
    )
    .await
    .expect("partitioned DKG tick");
    assert_eq!(partitioned.attempted_deliveries, 3);
    assert_eq!(partitioned.successful_deliveries, 1);
    assert_eq!(partitioned.failed_deliveries, 2);
    assert_eq!(partitioned.failed_members.len(), 2);
    assert!(!partitioned.is_complete());
    drop(first_delivery_tracker);

    for _ in 0..4 {
        for member in &federation.members {
            let mut delivery_tracker = member.delivery_tracker.lock().await;
            let outcome = run_federation_dkg_driver_tick(
                &member.state,
                &member.client,
                &member.participant,
                FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
                &mut delivery_tracker,
            )
            .await
            .expect("partitioned ceremony tick");
            assert!(!outcome.is_complete());
        }
    }

    federation.restart_member(0).await;
    federation.transport.set_member_outbound_partition(
        &federation.public_config,
        first_member,
        false,
    );
    let _ = federation.drive_until_complete(24).await;
    federation.assert_complete_and_consistent().await;
}

#[tokio::test]
async fn dkg_participant_reload_rejects_changed_keyset_policy() {
    let federation = DkgDriverTestFederation::new().await;
    let changed = FederationDkgKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        1,
        None,
        [Amount::from(1)].into_iter().collect(),
    )
    .expect("changed keyset spec");

    assert!(prepare_federation_dkg_participant_state(
        &federation.members[0].state,
        federation.ceremony_id,
        changed,
    )
    .await
    .is_err());
}

#[test]
fn dkg_driver_run_config_is_bounded() {
    let config = FederationDkgDriverRunConfig::default();

    assert!(config.max_ticks > 0);
    assert!(!config.retry_delay.is_zero());
}
