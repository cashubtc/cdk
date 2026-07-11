use std::sync::Arc;

use cdk::federation::{
    FederationDkgFinalizedMemberConfig, FederationDkgKeysetSpec,
    ValidatedFederationDkgParticipantState,
};

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
            let state = federation_route_state_for_private_with_db(
                public_config.clone(),
                private_config.clone(),
                db.clone(),
                &seed,
            )
            .await;
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
            let mut all_complete = true;
            for member in &self.members {
                let mut delivery_tracker = member.delivery_tracker.lock().await;
                let outcome = run_federation_dkg_driver_tick(
                    &member.state,
                    &member.client,
                    &member.participant,
                    FederationDkgSecretShareTransportPolicy::RequireHttps,
                    &mut delivery_tracker,
                )
                .await
                .expect("DKG driver tick");
                attempted_deliveries += outcome.attempted_deliveries;
                all_complete &= outcome.is_complete();
            }
            if all_complete {
                return attempted_deliveries;
            }
        }

        panic!("DKG ceremony did not complete in {max_rounds} rounds");
    }

    async fn restart_member(&mut self, index: usize) {
        let member = &self.members[index];
        let state = federation_route_state_for_private_with_db(
            self.public_config.clone(),
            member.private_config.clone(),
            member.db.clone(),
            &member.seed,
        )
        .await;
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
        for member in &self.members {
            let db = member.state.mint.localstore();
            assert_eq!(
                db.get_federation_dkg_participant_states()
                    .await
                    .expect("participant states"),
                vec![member.participant.participant_state().clone()]
            );
            assert_eq!(
                db.get_federation_dkg_commitments()
                    .await
                    .expect("commitments")
                    .len(),
                3
            );
            assert_eq!(
                db.get_federation_dkg_reveals()
                    .await
                    .expect("reveals")
                    .len(),
                3
            );
            assert_eq!(
                db.get_federation_dkg_secret_share_contributions()
                    .await
                    .expect("secret contributions")
                    .len(),
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
    }
}

#[tokio::test]
async fn dkg_driver_completes_over_authenticated_member_routes() {
    let federation = DkgDriverTestFederation::new().await;

    let attempted_deliveries = federation.drive_until_complete(12).await;
    assert_eq!(attempted_deliveries, 36);
    federation.assert_complete_and_consistent().await;
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
        FederationDkgSecretShareTransportPolicy::RequireHttps,
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
                FederationDkgSecretShareTransportPolicy::RequireHttps,
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
    let _ = federation.drive_until_complete(12).await;
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
