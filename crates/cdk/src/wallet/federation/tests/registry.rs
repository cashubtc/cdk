//! Federation registry: discovery, validation, member URL aliases, persistence and refresh.

use std::path::PathBuf;
use std::sync::Mutex;

use cdk_common::database::{self, WalletDatabase};
use cdk_http_client::{HttpError, RawResponse};

use super::*;
use crate::wallet::federation::{
    FederationConnectorPolicy, FederationImportOptions, FederationRegistry,
    FederationRegistryError, FederationSource, FEDERATION_KV_NAMESPACE,
    FEDERATION_REGISTRY_KV_NAMESPACE,
};
use crate::wallet::HttpTransport;

/// Status and body served for one URL.
type StaticResponse = (u16, Vec<u8>);

/// Transport that serves fixed JSON bodies by URL and counts requests.
#[derive(Debug, Clone, Default)]
struct StaticTransport {
    responses: Arc<Mutex<HashMap<String, StaticResponse>>>,
    requests: Arc<AtomicUsize>,
}

impl StaticTransport {
    fn serve<T>(&self, mint_url: &MintUrl, path: &[&str], body: &T)
    where
        T: serde::Serialize,
    {
        let url = mint_url.join_paths(path).expect("url");
        self.responses.lock().expect("responses").insert(
            url.to_string(),
            (200, serde_json::to_vec(body).expect("json body")),
        );
    }

    /// Serve `public_config` from every member, each with its own mint-info advertisement.
    fn serve_federation(&self, public_config: &FederationConfig) {
        for (member_id, member) in &public_config.members {
            self.serve(
                &member.public_mint_url,
                &["federation", "v1", "config"],
                public_config,
            );
            let info = MintInfo::new().federation(
                public_config
                    .mint_info_for_member(*member_id)
                    .expect("member advertisement"),
            );
            self.serve(&member.public_mint_url, &["v1", "info"], &info);
        }
    }
}

#[async_trait]
impl HttpTransport for StaticTransport {
    fn with_proxy(
        &mut self,
        _proxy: url::Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    async fn http_get(
        &self,
        url: url::Url,
        _auth: Option<cdk_common::AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let response = self
            .responses
            .lock()
            .expect("responses")
            .get(url.as_str())
            .cloned();
        Ok(match response {
            Some((status, body)) => RawResponse::new(status, body),
            None => RawResponse::new(404, b"not found".to_vec()),
        })
    }

    async fn http_post<P>(
        &self,
        _url: url::Url,
        _auth_token: Option<cdk_common::AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: serde::Serialize + Send + Sync,
    {
        Err(HttpError::Other("POST is not served".to_string()))
    }
}

type WalletStore = Arc<dyn WalletDatabase<database::Error> + Send + Sync>;

async fn memory_store() -> WalletStore {
    Arc::new(
        cdk_sqlite::wallet::memory::empty()
            .await
            .expect("wallet db"),
    )
}

fn temp_database_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "cdk-federation-registry-{}-{}.sqlite",
        std::process::id(),
        unix_time_nanos()
    ))
}

fn unix_time_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}

fn member_url(config: &FederationConfig, id: u16) -> MintUrl {
    config
        .member(member_id(id))
        .expect("member")
        .public_mint_url
        .clone()
}

#[tokio::test]
async fn registry_discovers_a_federation_through_a_member_url() {
    let public_config = threshold_rotation_bundle().public_config;
    let transport = StaticTransport::default();
    transport.serve_federation(&public_config);
    let registry = FederationRegistry::new(memory_store().await);
    let discovery_url = member_url(&public_config, 2);

    let entry = registry
        .import_with_transport(
            FederationSource::MemberUrl(discovery_url.clone()),
            FederationImportOptions::default(),
            &transport,
        )
        .await
        .expect("discover and import");

    assert_eq!(entry.federation_id(), public_config.federation_id);
    assert_eq!(entry.public_config(), &public_config);
    assert_eq!(entry.local_mint_url(), &discovery_url);
    assert_eq!(entry.active_units(), vec![CurrencyUnit::Sat]);
    assert_eq!(
        entry.config_digest().expect("digest"),
        public_config.config_digest().expect("digest")
    );
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
    assert_eq!(registry.list().await.expect("list"), vec![entry.clone()]);
    entry.connector().expect("connector from registry entry");
}

#[tokio::test]
async fn registry_rejects_invalid_federations_without_storing_them() {
    let bundle = threshold_rotation_bundle();
    let public_config = bundle.public_config.clone();
    let store = memory_store().await;
    let registry = FederationRegistry::new(store.clone());

    // The discovery member advertises another federation's digest.
    let transport = StaticTransport::default();
    transport.serve_federation(&public_config);
    let discovery_url = member_url(&public_config, 1);
    let mut mismatched = federation_mint_info(&bundle, member_id(1));
    mismatched
        .federation
        .as_mut()
        .expect("federation info")
        .config_digest =
        cdk_common::federation::FederationConfigDigest::from_bytes([42; 32]).to_string();
    transport.serve(&discovery_url, &["v1", "info"], &mismatched);
    let err = registry
        .import_with_transport(
            FederationSource::MemberUrl(discovery_url),
            FederationImportOptions::default(),
            &transport,
        )
        .await
        .expect_err("mismatched digest rejected");
    assert!(matches!(
        err,
        FederationRegistryError::Federation(FederationError::FederationConfigDigestMismatch { .. })
    ));

    // A URL that serves the config but is not on its roster.
    let stranger = MintUrl::from_str("https://stranger.example.com").expect("url");
    transport.serve(&stranger, &["federation", "v1", "config"], &public_config);
    transport.serve(
        &stranger,
        &["v1", "info"],
        &federation_mint_info(&bundle, member_id(1)),
    );
    let err = registry
        .import_with_transport(
            FederationSource::MemberUrl(stranger),
            FederationImportOptions::default(),
            &transport,
        )
        .await
        .expect_err("unlisted discovery URL rejected");
    assert!(matches!(
        err,
        FederationRegistryError::DiscoveryUrlNotInRoster { .. }
    ));

    // A member that does not answer.
    let silent = MintUrl::from_str("https://silent.example.com").expect("url");
    let err = registry
        .import_with_transport(
            FederationSource::MemberUrl(silent),
            FederationImportOptions::default(),
            &transport,
        )
        .await
        .expect_err("unreachable member rejected");
    assert!(matches!(err, FederationRegistryError::Fetch { .. }));

    // A config whose roster no longer derives its federation ID.
    let mut tampered = public_config.clone();
    tampered.federation_id = FederationId::from_bytes([7; 32]);
    let err = registry
        .import(
            FederationSource::PublicConfig(Box::new(tampered)),
            FederationImportOptions::default(),
        )
        .await
        .expect_err("tampered federation id rejected");
    assert!(matches!(
        err,
        FederationRegistryError::Federation(FederationError::FederationIdMismatch { .. })
    ));

    // A coordinator preference outside the roster.
    let err = registry
        .import(
            FederationSource::PublicConfig(Box::new(public_config.clone())),
            FederationImportOptions {
                connector_policy: Some(FederationConnectorPolicy {
                    quote_coordinator_member_id: Some(member_id(9)),
                    ..FederationConnectorPolicy::default()
                }),
                ..FederationImportOptions::default()
            },
        )
        .await
        .expect_err("unknown coordinator rejected");
    assert!(matches!(
        err,
        FederationRegistryError::UnknownQuoteCoordinator { .. }
    ));

    // An invite that does not decode.
    let err = registry
        .import(
            FederationSource::Invite("cashufedAnot-an-invite".to_string()),
            FederationImportOptions::default(),
        )
        .await
        .expect_err("bad invite rejected");
    assert!(matches!(err, FederationRegistryError::Federation(_)));

    assert!(registry.list().await.expect("list").is_empty());
    assert!(store
        .kv_list(FEDERATION_KV_NAMESPACE, FEDERATION_REGISTRY_KV_NAMESPACE)
        .await
        .expect("kv list")
        .is_empty());
}

/// The checks the CLI's former registry file made, now made once by the registry.
#[tokio::test]
async fn registry_rejects_configs_the_wallet_cannot_use() {
    let registry = FederationRegistry::new(memory_store().await);
    let import = |public_config: FederationConfig| {
        let registry = registry.clone();
        async move {
            registry
                .import(
                    FederationSource::PublicConfig(Box::new(public_config)),
                    FederationImportOptions::default(),
                )
                .await
                .expect_err("config rejected")
        }
    };
    let base = threshold_rotation_bundle().public_config;

    let mut inactive = base.clone();
    for keyset in &mut inactive.keysets {
        keyset.active = false;
    }
    assert!(matches!(
        import(inactive).await,
        FederationRegistryError::NoActiveKeyset { .. }
    ));

    let unsafe_thresholds = generate_trusted_dealer_configs(
        ThresholdParams::new(3, 2, 2).expect("structurally valid thresholds"),
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
            BTreeMap::from([(
                Amount::from(1),
                vec![
                    BlsSecretKey::from_reduced_bytes(&[31; 32]),
                    BlsSecretKey::from_reduced_bytes(&[32; 32]),
                ],
            )]),
        )],
    )
    .expect("trusted dealer configs")
    .public_config;
    assert!(matches!(
        import(unsafe_thresholds).await,
        FederationRegistryError::UnsafeThresholdProfile(_)
    ));

    let mut non_v3 = base.clone();
    let keyset = non_v3.keysets.first_mut().expect("keyset");
    keyset.id = Id::v2_from_data(
        &keyset.keys(),
        &keyset.unit,
        keyset.input_fee_ppk,
        keyset.final_expiry,
    );
    assert!(matches!(
        import(non_v3).await,
        FederationRegistryError::Federation(_)
    ));

    let mut stale_protocol = base.clone();
    stale_protocol.wallet_protocol_version += 1;
    assert!(matches!(
        import(stale_protocol).await,
        FederationRegistryError::Federation(_)
    ));

    let mut mismatched_roster = base;
    mismatched_roster.threshold_params = ThresholdParams::with_bft_consensus(4, 3).expect("params");
    assert!(matches!(
        import(mismatched_roster).await,
        FederationRegistryError::Federation(_)
    ));

    assert!(registry.list().await.expect("list").is_empty());
}

#[tokio::test]
async fn registry_maps_every_member_url_to_one_federation() {
    let public_config = threshold_rotation_bundle().public_config;
    let registry = FederationRegistry::new(memory_store().await);
    let local = MintUrl::from_str("https://wallet-alias.example.com").expect("url");
    let entry = registry
        .import(
            FederationSource::PublicConfig(Box::new(public_config.clone())),
            FederationImportOptions {
                local_mint_url: Some(local.clone()),
                ..FederationImportOptions::default()
            },
        )
        .await
        .expect("import");

    for alias in
        std::iter::once(local.clone()).chain((1..=3).map(|id| member_url(&public_config, id)))
    {
        let found = registry
            .find_by_mint_url(&alias)
            .await
            .expect("lookup")
            .expect("alias resolves");
        assert_eq!(found.federation_id(), public_config.federation_id);
        assert_eq!(found.local_mint_url(), &local);
    }
    assert_eq!(entry.aliases().len(), 4);
    assert_eq!(
        entry.member_id_for_mint_url(&member_url(&public_config, 3)),
        Some(member_id(3))
    );
    assert_eq!(entry.member_id_for_mint_url(&local), None);
    let unknown = MintUrl::from_str("https://unknown.example.com").expect("url");
    assert!(registry
        .find_by_mint_url(&unknown)
        .await
        .expect("lookup")
        .is_none());

    // Importing again through a member keeps the local identity.
    let invite = FederationInvite::new(public_config.clone())
        .expect("invite")
        .to_invite_code()
        .expect("invite code");
    let again = registry
        .import(
            FederationSource::Invite(invite),
            FederationImportOptions::default(),
        )
        .await
        .expect("re-import");
    assert_eq!(again.local_mint_url(), &local);
    assert_eq!(registry.list().await.expect("list").len(), 1);

    // ...and refuses to move it.
    let err = registry
        .import(
            FederationSource::PublicConfig(Box::new(public_config.clone())),
            FederationImportOptions {
                local_mint_url: Some(member_url(&public_config, 1)),
                ..FederationImportOptions::default()
            },
        )
        .await
        .expect_err("local mint URL change rejected");
    assert!(matches!(
        err,
        FederationRegistryError::LocalMintUrlConflict { .. }
    ));

    // Another federation that shares a member URL is refused.
    let other = bundle().public_config;
    assert_ne!(other.federation_id, public_config.federation_id);
    let err = registry
        .import(
            FederationSource::PublicConfig(Box::new(other)),
            FederationImportOptions::default(),
        )
        .await
        .expect_err("shared alias rejected");
    match err {
        FederationRegistryError::AliasConflict {
            mint_url,
            registered,
            ..
        } => {
            assert_eq!(mint_url, member_url(&public_config, 1));
            assert_eq!(registered, public_config.federation_id);
        }
        err => panic!("unexpected error: {err}"),
    }

    // Once removed, the URLs are free again.
    assert!(registry
        .remove(public_config.federation_id)
        .await
        .expect("remove"));
    assert!(registry
        .find_by_mint_url(&local)
        .await
        .expect("lookup")
        .is_none());
    registry
        .import(
            FederationSource::PublicConfig(Box::new(bundle().public_config)),
            FederationImportOptions::default(),
        )
        .await
        .expect("other federation after removal");
}

/// A repository wallet built from a federation's wallet config is keyed by
/// the local mint URL and owns every member URL, so a lookup, token or
/// payment request through any member reaches it.
#[tokio::test]
async fn wallet_config_makes_every_member_url_an_alias_of_the_local_wallet() {
    let public_config = threshold_rotation_bundle().public_config;
    let store = memory_store().await;
    let registry = FederationRegistry::new(store.clone());
    let local = member_url(&public_config, 1);
    let entry = registry
        .import(
            FederationSource::PublicConfig(Box::new(public_config.clone())),
            FederationImportOptions {
                local_mint_url: Some(local.clone()),
                ..FederationImportOptions::default()
            },
        )
        .await
        .expect("import");
    let repository = crate::wallet::WalletRepositoryBuilder::new()
        .localstore(store)
        .seed([7u8; 64])
        .build()
        .await
        .expect("repository");

    let wallet = repository
        .create_wallet(
            local.clone(),
            CurrencyUnit::Sat,
            Some(entry.wallet_config().expect("wallet config")),
        )
        .await
        .expect("federation wallet");

    assert!(!wallet.mint_url_aliases().contains(&local));
    assert_eq!(wallet.mint_url_aliases().len(), 2);
    for id in 1..=3 {
        let member = member_url(&public_config, id);
        assert!(wallet.accepts_mint_url(&member));
        let found = repository
            .get_wallet(&member, &CurrencyUnit::Sat)
            .await
            .expect("member URL resolves");
        assert_eq!(found.mint_url, local);
    }
    assert!(format!("{:?}", wallet.mint_connector()).contains("FederatedMintConnector"));
}

#[tokio::test]
async fn registry_survives_a_wallet_restart() {
    let public_config = threshold_rotation_bundle().public_config;
    let path = temp_database_path();
    let policy = FederationConnectorPolicy {
        quote_coordinator_member_id: Some(member_id(2)),
        member_request_timeout_secs: 90,
        ..FederationConnectorPolicy::default()
    };

    let imported = {
        let store: WalletStore = Arc::new(
            cdk_sqlite::WalletSqliteDatabase::new(path.clone())
                .await
                .expect("open wallet db"),
        );
        let registry = FederationRegistry::new(store.clone());
        let imported = registry
            .import(
                FederationSource::PublicConfig(Box::new(public_config.clone())),
                FederationImportOptions {
                    local_mint_url: None,
                    connector_policy: Some(policy),
                },
            )
            .await
            .expect("import");
        let stored = store
            .kv_read(
                FEDERATION_KV_NAMESPACE,
                FEDERATION_REGISTRY_KV_NAMESPACE,
                &public_config.federation_id.to_string(),
            )
            .await
            .expect("kv read")
            .expect("entry stored under the federation id");
        let stored = String::from_utf8(stored).expect("utf8");
        assert!(stored.contains("\"public_config\""));
        assert!(!stored.contains("secret_share"));
        imported
    };

    let store: WalletStore = Arc::new(
        cdk_sqlite::WalletSqliteDatabase::new(path.clone())
            .await
            .expect("reopen wallet db"),
    );
    let registry = FederationRegistry::new(store);
    let reloaded = registry
        .get(&public_config.federation_id)
        .await
        .expect("get")
        .expect("entry survives restart");
    assert_eq!(reloaded, imported);
    assert_eq!(reloaded.local_mint_url(), &member_url(&public_config, 1));
    assert_eq!(reloaded.connector_policy(), &policy);
    assert_eq!(
        reloaded
            .connector()
            .expect("connector")
            .connector_config()
            .member_request_timeout,
        Duration::from_secs(90)
    );

    let updated = registry
        .set_connector_policy(
            public_config.federation_id,
            FederationConnectorPolicy::default(),
        )
        .await
        .expect("policy update");
    let reopened = FederationRegistry::new(Arc::new(
        cdk_sqlite::WalletSqliteDatabase::new(path.clone())
            .await
            .expect("reopen wallet db"),
    ));
    assert_eq!(
        reopened.list().await.expect("list"),
        vec![updated],
        "policy update persisted"
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn registry_rejects_a_stored_entry_under_another_key() {
    let public_config = threshold_rotation_bundle().public_config;
    let store = memory_store().await;
    let registry = FederationRegistry::new(store.clone());
    let entry = registry
        .import(
            FederationSource::PublicConfig(Box::new(public_config)),
            FederationImportOptions::default(),
        )
        .await
        .expect("import");
    store
        .kv_write(
            FEDERATION_KV_NAMESPACE,
            FEDERATION_REGISTRY_KV_NAMESPACE,
            "not-the-federation-id",
            &serde_json::to_vec(&entry).expect("json"),
        )
        .await
        .expect("kv write");

    let err = FederationRegistry::new(store)
        .list()
        .await
        .expect_err("misplaced entry rejected");
    assert!(matches!(err, FederationRegistryError::KeyMismatch { .. }));
}

#[tokio::test]
async fn rediscovery_refreshes_the_stored_config() {
    let bundle = threshold_rotation_bundle();
    let original = bundle.public_config.clone();
    let rotated = rotated_public_config(&bundle);
    assert_eq!(rotated.federation_id, original.federation_id);
    assert_ne!(rotated, original);

    let store = memory_store().await;
    let registry = FederationRegistry::new(store.clone());
    let local = member_url(&original, 3);
    registry
        .import(
            FederationSource::PublicConfig(Box::new(original.clone())),
            FederationImportOptions {
                local_mint_url: Some(local.clone()),
                ..FederationImportOptions::default()
            },
        )
        .await
        .expect("import");

    // The local member is down; another member serves the rotated config.
    let transport = StaticTransport::default();
    transport.serve_federation(&rotated);
    transport
        .responses
        .lock()
        .expect("responses")
        .retain(|url, _| !url.starts_with(local.to_string().as_str()));

    let refreshed = registry
        .refresh_with_transport(original.federation_id, &transport)
        .await
        .expect("refresh");
    assert_eq!(refreshed.public_config(), &rotated);
    assert_eq!(refreshed.local_mint_url(), &local);
    assert_eq!(
        refreshed
            .public_config()
            .keysets
            .iter()
            .filter(|keyset| keyset.active)
            .count(),
        rotated
            .keysets
            .iter()
            .filter(|keyset| keyset.active)
            .count()
    );

    let reloaded = FederationRegistry::new(store)
        .get(&original.federation_id)
        .await
        .expect("get")
        .expect("entry");
    assert_eq!(reloaded, refreshed, "refreshed config persisted");

    // Rediscovery by importing through a member URL refreshes the same entry.
    let transport = StaticTransport::default();
    transport.serve_federation(&original);
    let reimported = registry
        .import_with_transport(
            FederationSource::MemberUrl(member_url(&original, 1)),
            FederationImportOptions::default(),
            &transport,
        )
        .await
        .expect("rediscover");
    assert_eq!(reimported.public_config(), &original);
    assert_eq!(reimported.local_mint_url(), &local);
    assert_eq!(registry.list().await.expect("list").len(), 1);

    // A member that now serves a different federation does not replace it.
    let other = bundle_with_member_urls_of(&original);
    let transport = StaticTransport::default();
    transport.serve_federation(&other);
    let err = registry
        .refresh_with_transport(original.federation_id, &transport)
        .await
        .expect_err("changed federation rejected");
    assert!(matches!(
        err,
        FederationRegistryError::FederationChanged { .. }
    ));
    assert_eq!(
        registry
            .get(&original.federation_id)
            .await
            .expect("get")
            .expect("entry")
            .public_config(),
        &original
    );
}

/// A different federation (other thresholds) on the same three member URLs.
fn bundle_with_member_urls_of(config: &FederationConfig) -> FederationConfig {
    let other = generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(3, 3).expect("params"),
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
            BTreeMap::from([(
                Amount::from(1),
                vec![
                    BlsSecretKey::from_reduced_bytes(&[21; 32]),
                    BlsSecretKey::from_reduced_bytes(&[22; 32]),
                    BlsSecretKey::from_reduced_bytes(&[23; 32]),
                ],
            )]),
        )],
    )
    .expect("other federation")
    .public_config;
    assert_ne!(other.federation_id, config.federation_id);
    other
}
