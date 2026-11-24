//! Federation configs, payment backends and route-state builders.

use super::*;

pub(super) fn private_member(value: u16) -> MemberConfigPrivate {
    MemberConfigPrivate {
        federation_id: None,
        member_id: member_id(value),
        identity_secret_key: SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
        keyset_secret_shares: BTreeMap::new(),
        setup_transcript_hash: None,
    }
}

pub(super) fn transport_config() -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
    transport_config_with_federation_api_urls(
        MintUrl::from_str("https://fed1.example.com/api").expect("federation api url"),
        MintUrl::from_str("https://fed2.example.com/api").expect("federation api url"),
    )
}

pub(super) fn transport_config_with_federation_api_urls(
    first_federation_api_url: MintUrl,
    second_federation_api_url: MintUrl,
) -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
    transport_config_with_members_and_federation_api_urls(
        member_id(1),
        member_id(2),
        first_federation_api_url,
        second_federation_api_url,
    )
}

pub(super) fn transport_config_with_members_and_federation_api_urls(
    first_member_id: MemberId,
    second_member_id: MemberId,
    first_federation_api_url: MintUrl,
    second_federation_api_url: MintUrl,
) -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
    let members = vec![
        TrustedDealerMember::new(
            first_member_id,
            MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
            first_federation_api_url,
            SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        ),
        TrustedDealerMember::new(
            second_member_id,
            MintUrl::from_str("https://mint2.example.com").expect("public mint url"),
            second_federation_api_url,
            SecretKey::from_slice(&[2; 32]).expect("identity secret"),
        ),
    ];
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[11; 32]),
                BlsSecretKey::from_reduced_bytes(&[12; 32]),
            ],
        )]),
    );
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::new(2, 2, 2).expect("threshold params"),
        members,
        vec![keyset],
    )
    .expect("trusted dealer bundle");
    let first_private = bundle
        .private_configs
        .get(&first_member_id)
        .expect("first private config")
        .clone();
    let second_private = bundle
        .private_configs
        .get(&second_member_id)
        .expect("second private config")
        .clone();
    (bundle.public_config, first_private, second_private)
}

pub(super) fn transport_config_three() -> (
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    transport_config_three_with_threshold(ThresholdParams::new(3, 2, 2).expect("threshold params"))
}

pub(super) fn transport_config_three_with_threshold(
    threshold_params: ThresholdParams,
) -> (
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    let members = vec![
        TrustedDealerMember::new(
            member_id(1),
            MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed1.example.com/api").expect("federation api url"),
            SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        ),
        TrustedDealerMember::new(
            member_id(2),
            MintUrl::from_str("https://mint2.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed2.example.com/api").expect("federation api url"),
            SecretKey::from_slice(&[2; 32]).expect("identity secret"),
        ),
        TrustedDealerMember::new(
            member_id(3),
            MintUrl::from_str("https://mint3.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed3.example.com/api").expect("federation api url"),
            SecretKey::from_slice(&[3; 32]).expect("identity secret"),
        ),
    ];
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[11; 32]),
                BlsSecretKey::from_reduced_bytes(&[12; 32]),
            ],
        )]),
    );
    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset])
        .expect("trusted dealer bundle");
    let first_private = bundle
        .private_configs
        .get(&member_id(1))
        .expect("first private config")
        .clone();
    let second_private = bundle
        .private_configs
        .get(&member_id(2))
        .expect("second private config")
        .clone();
    let third_private = bundle
        .private_configs
        .get(&member_id(3))
        .expect("third private config")
        .clone();
    (
        bundle.public_config,
        first_private,
        second_private,
        third_private,
    )
}

pub(super) fn transport_config_with_member_federation_api_urls(
    threshold_params: ThresholdParams,
    federation_api_urls: Vec<MintUrl>,
) -> (FederationConfig, Vec<MemberConfigPrivate>) {
    let member_count = threshold_params.member_count;
    assert_eq!(usize::from(member_count), federation_api_urls.len());
    let members = federation_api_urls
        .into_iter()
        .enumerate()
        .map(|(index, federation_api_url)| {
            let value = u16::try_from(index + 1).expect("member index");
            TrustedDealerMember::new(
                member_id(value),
                MintUrl::from_str(&format!("https://mint{value}.example.com"))
                    .expect("public mint url"),
                federation_api_url,
                SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
            )
        })
        .collect::<Vec<_>>();
    let amount_coefficients = (0..threshold_params.signature_threshold)
        .map(|index| BlsSecretKey::from_reduced_bytes(&[31 + index as u8; 32]))
        .collect::<Vec<_>>();
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(Amount::from(1), amount_coefficients)]),
    );
    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset])
        .expect("trusted dealer bundle");
    let private_configs = (1u16..=member_count)
        .map(|value| {
            bundle
                .private_configs
                .get(&member_id(value))
                .expect("private config")
                .clone()
        })
        .collect::<Vec<_>>();

    (bundle.public_config, private_configs)
}

pub(super) fn transport_config_with_member_federation_api_urls_and_keysets(
    threshold_params: ThresholdParams,
    federation_api_urls: Vec<MintUrl>,
    keysets: Vec<TrustedDealerKeysetSpec>,
) -> (FederationConfig, Vec<MemberConfigPrivate>) {
    let member_count = threshold_params.member_count;
    assert_eq!(usize::from(member_count), federation_api_urls.len());
    let members = federation_api_urls
        .into_iter()
        .enumerate()
        .map(|(index, federation_api_url)| {
            let value = u16::try_from(index + 1).expect("member index");
            TrustedDealerMember::new(
                member_id(value),
                MintUrl::from_str(&format!("https://mint{value}.example.com"))
                    .expect("public mint url"),
                federation_api_url,
                SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
            )
        })
        .collect::<Vec<_>>();
    let bundle = generate_trusted_dealer_configs(threshold_params, members, keysets)
        .expect("trusted dealer bundle");
    let private_configs = (1u16..=member_count)
        .map(|value| {
            bundle
                .private_configs
                .get(&member_id(value))
                .expect("private config")
                .clone()
        })
        .collect::<Vec<_>>();

    (bundle.public_config, private_configs)
}

pub(super) fn transport_config_five_with_threshold(
    threshold_params: ThresholdParams,
) -> (FederationConfig, Vec<MemberConfigPrivate>) {
    let members = (1u16..=5)
        .map(|value| {
            TrustedDealerMember::new(
                member_id(value),
                MintUrl::from_str(&format!("https://mint{value}.example.com"))
                    .expect("public mint url"),
                MintUrl::from_str(&format!("https://fed{value}.example.com/api"))
                    .expect("federation api url"),
                SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
            )
        })
        .collect::<Vec<_>>();
    let amount_coefficients = (0..threshold_params.signature_threshold)
        .map(|index| BlsSecretKey::from_reduced_bytes(&[21 + index as u8; 32]))
        .collect::<Vec<_>>();
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(Amount::from(1), amount_coefficients)]),
    );
    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset])
        .expect("trusted dealer bundle");
    let private_configs = (1u16..=5)
        .map(|value| {
            bundle
                .private_configs
                .get(&member_id(value))
                .expect("private config")
                .clone()
        })
        .collect::<Vec<_>>();

    (bundle.public_config, private_configs)
}

#[derive(Debug)]
pub(super) struct CountingFakeWallet {
    pub(super) inner: FakeWallet,
    pub(super) make_payment_calls: Arc<AtomicUsize>,
}

impl CountingFakeWallet {
    pub(super) fn with_call_counter() -> (DynMintPayment, Arc<AtomicUsize>) {
        let make_payment_calls = Arc::new(AtomicUsize::new(0));
        let wallet = Self {
            inner: FakeWallet::new(
                FeeReserve {
                    min_fee_reserve: 1.into(),
                    percent_fee_reserve: 0.0,
                },
                HashMap::default(),
                HashSet::default(),
                0,
                CurrencyUnit::Sat,
            ),
            make_payment_calls: Arc::clone(&make_payment_calls),
        };

        (Arc::new(wallet), make_payment_calls)
    }
}

#[async_trait]
impl MintPayment for CountingFakeWallet {
    type Err = cdk_payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        self.inner.get_settings().await
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        self.inner.create_incoming_payment_request(options).await
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        self.inner.get_payment_quote(unit, options).await
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.make_payment_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.make_payment(unit, options).await
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        self.inner.wait_payment_event().await
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.inner.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        self.inner
            .check_incoming_payment_status(payment_identifier)
            .await
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.inner.check_outgoing_payment(payment_identifier).await
    }
}

pub(super) async fn federation_route_state_with_local_private() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    let (public_config, local_private, peer_private) = transport_config();
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;

    (state, public_config, local_private, peer_private)
}

pub(super) async fn federation_route_state_three_with_local_private() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    let (public_config, first_private, second_private, third_private) = transport_config_three();
    let state =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;

    (
        state,
        public_config,
        first_private,
        second_private,
        third_private,
    )
}

pub(super) async fn federation_route_state_for_private(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
) -> MintState {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    federation_route_state_for_private_with_db(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await
}

pub(super) async fn recovery_checkpoint_quorum_for_state(
    state: &MintState,
    public_config: &FederationConfig,
    scheduled_checkpoint: Option<&FederationCheckpoint>,
    signers: &[&MemberConfigPrivate],
) -> (FederationCheckpoint, FederationCheckpointQuorum) {
    let checkpoint =
        recovery_checkpoint_for_state(state, public_config, scheduled_checkpoint).await;
    let signatures = signers
        .iter()
        .map(|signer| {
            FederationCheckpointSignature::new_signed(&checkpoint, signer)
                .expect("checkpoint signature")
        })
        .collect();
    let quorum = FederationCheckpointQuorum::new(checkpoint.clone(), signatures);
    quorum
        .verify(public_config)
        .expect("recovery checkpoint quorum");
    (checkpoint, quorum)
}

pub(super) async fn recovery_checkpoint_for_state(
    state: &MintState,
    public_config: &FederationConfig,
    scheduled_checkpoint: Option<&FederationCheckpoint>,
) -> FederationCheckpoint {
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("checkpoint frontier");
    let consensus = state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await;
    let status = consensus.status();
    let boundary = FederationCheckpointBoundary::from_complete_unit(
        &frontier,
        status.session,
        status.next_unit_finalization_index,
        consensus.next_unit_index_by_member().clone(),
    )
    .expect("complete checkpoint boundary");
    drop(consensus);
    let materialized_state_digest = state
        .mint
        .federation_materialized_state_snapshot()
        .await
        .expect("materialized state snapshot")
        .digest()
        .expect("materialized state digest");

    boundary
        .recovery_checkpoint(
            public_config,
            scheduled_checkpoint,
            materialized_state_digest,
        )
        .expect("recovery checkpoint")
}

pub(super) fn one_operation_unit_checkpoint_pages(
    runtime: &FederationConsensusRuntime,
    checkpoint: &FederationCheckpoint,
    limit: u16,
) -> Vec<FederationJournalCatchUpResponse> {
    let checkpoint_id = checkpoint.id().expect("checkpoint page target id");
    let mut pages = Vec::new();
    let mut next = 0_usize;
    let target = usize::try_from(checkpoint.journal_next_index)
        .expect("checkpoint journal cursor fits usize");
    assert_eq!(
        checkpoint.journal_next_index, checkpoint.finalized_next_item_index,
        "test helper requires one accepted operation per finalized item"
    );
    while next < target {
        let request = FederationJournalCatchUpRequest::new(next as u64)
            .with_target_checkpoint(checkpoint_id)
            .with_limit(limit)
            .with_aleph_bft_next_item_index(next as u64);
        let response = FederationJournalCatchUpResponse::from_journal(
            checkpoint.federation_id,
            runtime.journal(),
            checkpoint.materialized_state_digest,
            &request,
            limit,
            limit,
        )
        .bound_to_checkpoint(checkpoint)
        .expect("bound one-operation-unit checkpoint page");
        let end = next.saturating_add(response.entries.len());
        pages.push(response.with_aleph_bft_finalized_items(
            runtime.finalized_items()[next..end].to_vec(),
            end == target,
        ));
        next = end;
    }
    pages
}

pub(super) async fn federation_route_state_for_private_with_db(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
) -> MintState {
    let fake: DynMintPayment = Arc::new(FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 0.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    ));
    Box::pin(
        federation_route_state_for_private_with_db_payment_and_finality(
            public_config,
            local_private,
            db,
            seed,
            fake,
            true,
        ),
    )
    .await
}

pub(super) async fn federation_route_state_for_private_with_db_and_aleph_bft(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
) -> MintState {
    let fake: DynMintPayment = Arc::new(FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 0.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    ));
    Box::pin(
        federation_route_state_for_private_with_db_payment_and_finality(
            public_config,
            local_private,
            db,
            seed,
            fake,
            false,
        ),
    )
    .await
}

pub(super) async fn federation_route_state_for_private_with_db_and_aleph_bft_payment_processor(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
    payment_processor: DynMintPayment,
) -> MintState {
    Box::pin(
        federation_route_state_for_private_with_db_payment_and_finality(
            public_config,
            local_private,
            db,
            seed,
            payment_processor,
            false,
        ),
    )
    .await
}

pub(super) async fn federation_route_state_for_private_with_db_payment_and_finality(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
    payment_processor: DynMintPayment,
    local_finality_test: bool,
) -> MintState {
    let runtime_config =
        cdk::mint::FederationRuntimeConfig::new(public_config.clone(), local_private.clone())
            .expect("runtime config");
    let mut builder = MintBuilder::new(db.clone())
        .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
        .with_federation_config(runtime_config.clone());
    #[cfg(feature = "federation-bark")]
    builder.set_federation_bark_store(db.clone());
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            payment_processor,
        )
        .await
        .expect("payment processor");
    let mint = builder.build_with_seed(db, seed).await.expect("mint");
    let consensus = FederationConsensusRuntime::authenticated_aleph_bft(
        runtime_config.public_config(),
        runtime_config.member_id(),
        0,
    )
    .expect("AlephBFT consensus runtime");
    let mint = Arc::new(mint);
    MintState {
        mint: Arc::clone(&mint),
        cache: Arc::new(crate::cache::HttpCache::default()),
        node: cdk_federation::FederationNode {
            mint: Arc::clone(&mint),
            federation: crate::federation::FederationNodeRuntime {
                transport: crate::default_federation_transport(),
                membership: crate::federation::FederationMembership::configured(
                    crate::federation::FederationMemberContext::new(
                        mint,
                        Arc::new(tokio::sync::Mutex::new(consensus)),
                        Arc::new(tokio::sync::Mutex::new(
                            cdk::federation::FederationRestSequenceTracker::new(),
                        )),
                        crate::FederationPeerSyncConfig::default(),
                    ),
                ),
                tasks: crate::federation::services::FederationTaskGroup::default(),
                payment_observer: crate::federation::FederationPaymentObserver::default(),
                #[cfg(feature = "federation-bdk")]
                transaction_broadcaster:
                    crate::federation::FederationTransactionBroadcaster::default(),
                #[cfg(feature = "federation-bdk")]
                wallet_liveness: crate::federation::FederationWalletLivenessWorker::default(),
                #[cfg(feature = "federation-bdk")]
                onchain_sync_interval: None,
                components: crate::federation::runtime_components::FederationRuntimeComponents::new(
                    Some(Arc::new(tokio::sync::Mutex::new(if local_finality_test {
                        crate::FederationSyncState::default()
                    } else {
                        crate::FederationSyncState::for_aleph_bft()
                    }))),
                    Some(Arc::new(FederationDkgAutoStartConfig {
                        setup_authorization: public_config.setup_authorization,
                        ceremony_id: FederationDkgCeremonyId::from_bytes([55; 32]),
                        root_key_epoch: 1,
                        keyset_spec: cdk::federation::FederationDkgKeysetSpec::new(
                            CurrencyUnit::Sat,
                            true,
                            0,
                            None,
                            [Amount::from(1)].into_iter().collect(),
                        )
                        .expect("test DKG keyset spec"),
                        driver: FederationDkgDriverRunConfig::default(),
                    })),
                    Some(crate::federation_dkg_member_admission(
                        &public_config,
                        FederationDkgDriverRunConfig::default().member_request_burst,
                        FederationDkgDriverRunConfig::default().member_request_refill_interval,
                    )),
                    #[cfg(feature = "federation-bdk")]
                    Vec::new(),
                    None,
                    #[cfg(feature = "federation-bark")]
                    None,
                ),
                aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
                aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
                aleph_bft_halt_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                finality: if local_finality_test {
                    cdk_federation::FederationFinalityMode::Local
                } else {
                    cdk_federation::FederationFinalityMode::Live
                },
            },
        },
    }
}

pub(super) async fn federation_route_state() -> (MintState, FederationConfig, MemberConfigPrivate) {
    let (state, public_config, _, peer_private) = federation_route_state_with_local_private().await;
    (state, public_config, peer_private)
}
