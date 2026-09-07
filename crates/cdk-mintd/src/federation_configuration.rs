//! Federation configuration loading and startup validation.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::cli::CLIArgs;
use crate::config::PaymentBackendType;
use anyhow::{anyhow, bail, Context, Result};
use cdk::mint::FederationRuntimeConfig;
use cdk::mint::MintBuilder;
use cdk_common::database::DynMintDatabase;
use cdk_common::federation::{
    FederationConfig, FederationDkgCeremonyRecord, FederationDkgCeremonyState,
    FederationDkgSetupMethod, FederationPaymentObservationContext,
    FederationPaymentVerificationPolicy, MemberConfigPrivate,
};
#[cfg(feature = "federation-bark")]
use cdk_federation_bark::FederationBarkReceiveAntiDosPolicy;
use serde::de::DeserializeOwned;

use crate::{config, federation_setup_command, get_work_directory, load_stored_settings};

pub(crate) fn configure_federation(
    settings: &config::Settings,
    mint_builder: MintBuilder,
) -> Result<MintBuilder> {
    let Some(federation) = settings.federation.as_ref() else {
        return Ok(mint_builder);
    };

    if !federation.enabled {
        return Ok(mint_builder);
    }

    let runtime_config = configured_federation_runtime(settings)?;
    let public_config = runtime_config.public_config();
    validate_federation_threshold_startup_policy(
        public_config,
        federation.allow_development_payment_observation_policy,
    )?;
    #[cfg(feature = "federation-bdk")]
    validate_federated_bdk_startup(settings, public_config)?;
    #[cfg(feature = "federation-bark")]
    validate_federated_bark_startup(settings, public_config)?;
    warn_federated_wallet_startup_risks(federation, public_config);
    validate_federation_setup_method_startup_policy(public_config)?;
    validate_federation_payment_observation_startup_policy(
        public_config,
        federation.allow_development_payment_observation_policy,
    )?;

    tracing::info!(
        "Loaded federation {} as member {} using AlephBFT consensus",
        runtime_config.federation_id(),
        runtime_config.member_id()
    );

    Ok(mint_builder.with_federation_config(runtime_config))
}

pub(crate) fn configured_federation_runtime(
    settings: &config::Settings,
) -> Result<FederationRuntimeConfig> {
    let federation = settings
        .federation
        .as_ref()
        .ok_or_else(|| anyhow!("federation settings are missing"))?;
    if federation.dkg.enabled {
        if federation.public_config_path.is_some() || federation.private_config_path.is_some() {
            bail!(
                "certified DKG startup uses federation.setup_config_path, not raw public/private federation config paths"
            );
        }
        let setup_config_path = federation.setup_config_path.as_deref().ok_or_else(|| {
            anyhow!("federation.setup_config_path must be set when DKG is enabled")
        })?;
        let handoff = federation_setup_command::load_certified_dkg_handoff(setup_config_path)?;
        return FederationRuntimeConfig::new(handoff.public_config, handoff.private_config)
            .context("invalid manifest-derived federation runtime config");
    }
    if federation.setup_config_path.is_some() {
        bail!("federation.setup_config_path is only valid while certified DKG is enabled");
    }
    let public_config_path = federation.public_config_path.as_deref().ok_or_else(|| {
        anyhow!("federation.public_config_path must be set when federation is enabled")
    })?;
    let private_config_path = federation.private_config_path.as_deref().ok_or_else(|| {
        anyhow!("federation.private_config_path must be set when federation is enabled")
    })?;
    let public_config = read_public_federation_config(public_config_path)?;
    validate_private_federation_config_file(private_config_path)?;
    let private_config: MemberConfigPrivate =
        read_federation_json(private_config_path, "private federation config")?;
    FederationRuntimeConfig::new(public_config, private_config)
        .context("invalid federation runtime config")
}

#[cfg(all(feature = "federation", not(feature = "iroh")))]
pub(crate) fn validate_federation_transport_build(settings: &config::Settings) -> Result<()> {
    let Some(federation) = settings.federation.as_ref().filter(|value| value.enabled) else {
        return Ok(());
    };
    let _ = federation;
    let runtime = configured_federation_runtime(settings)?;
    for member in runtime.public_config().members.values() {
        if [&member.public_mint_url, &member.federation_api_url]
            .iter()
            .any(|url| url.to_string().starts_with("iroh://"))
        {
            bail!(
                "federation member {} advertises Iroh but this mintd build has no Iroh support",
                member.member_id
            );
        }
    }
    Ok(())
}

#[cfg(feature = "federation-bark")]
pub(crate) fn validate_federated_bark_startup(
    settings: &config::Settings,
    public_config: &FederationConfig,
) -> Result<()> {
    let federation = settings
        .federation
        .as_ref()
        .ok_or_else(|| anyhow!("federated Bark validation requires federation settings"))?;
    let selected = settings
        .payment_backend
        .iter()
        .filter(|entry| entry.backend == PaymentBackendType::FederatedBark)
        .collect::<Vec<_>>();
    if !federation.bark.enabled {
        if !selected.is_empty() {
            bail!("ln_backend = \"federated-bark\" requires federation.bark.enabled=true");
        }
        return Ok(());
    }
    if selected.len() != 1 || selected[0].unit != cdk::nuts::CurrencyUnit::Sat {
        bail!(
            "federation.bark.enabled=true requires exactly one satoshi ln_backend = \"federated-bark\" entry"
        );
    }
    let application = cdk_federation_bark::federation_bark_application(public_config)?
        .ok_or_else(|| anyhow!("federated Bark requires an accepted Bark FROST application"))?;
    let network = federation
        .bark
        .network
        .as_deref()
        .ok_or_else(|| anyhow!("federation.bark.network must be explicitly configured"))?;
    if parse_federation_bitcoin_network(network)? != application.network {
        bail!("federation.bark.network differs from the accepted Bark application");
    }
    let ark_server = federation
        .bark
        .ark_server_public_key
        .as_deref()
        .ok_or_else(|| {
            anyhow!("federation.bark.ark_server_public_key must be explicitly configured")
        })?;
    let ark_server = cdk::secp256k1::PublicKey::from_str(ark_server)
        .context("invalid federation.bark.ark_server_public_key")?;
    if ark_server.serialize() != application.ark_server_public_key {
        bail!("federation.bark.ark_server_public_key differs from the accepted Bark application");
    }
    if application.policy.receive_base_fee_sat != 0
        || application.policy.receive_fee_ppm != 0
        || application.policy.receive_anti_dos != FederationBarkReceiveAntiDosPolicy::NoProof
    {
        bail!("federated Bark production v1 requires zero receive fees and no anti-DoS proof");
    }
    if federation
        .bark
        .server_address
        .as_deref()
        .is_none_or(str::is_empty)
        || federation.bark.esplora_url.is_some() == federation.bark.bitcoind_address.is_some()
        || federation.bark.operational_seed_hex.is_none()
        || federation.bark.receive_secret_key_hex.is_none()
        || federation.bark.frost_protocol_storage_root.is_none()
    {
        bail!(
            "federation.bark requires an Ark endpoint, exactly one chain source, both local secret keys, and durable FROST storage"
        );
    }
    Ok(())
}

pub(crate) fn warn_federated_wallet_startup_risks(
    federation: &config::Federation,
    public_config: &FederationConfig,
) {
    #[cfg(feature = "federation-bdk")]
    if federation.onchain_wallet.enabled && federation.onchain_wallet.chain_source_scope == "shared"
    {
        tracing::warn!(
            source_id = federation
                .onchain_wallet
                .chain_source_id
                .as_deref()
                .unwrap_or("shared-source"),
            "Federated wallet chain observations use correlated shared infrastructure; independent member sources are recommended"
        );
    }
    if federation.dkg.enabled && public_config.setup_transcript.is_none() {
        tracing::warn!(
            required = public_config.threshold_params.consensus_threshold,
            members = public_config.threshold_params.member_count,
            "Federation DKG startup is incomplete until a threshold quorum completes and confirms both the BLS and FROST results"
        );
    }
}

#[cfg(feature = "federation-bdk")]
pub(crate) fn validate_federated_bdk_startup(
    settings: &config::Settings,
    public_config: &FederationConfig,
) -> Result<()> {
    let federation = settings
        .federation
        .as_ref()
        .ok_or_else(|| anyhow!("federated BDK validation requires federation settings"))?;
    let selected = settings
        .onchain
        .as_ref()
        .map(|onchain| &onchain.onchain_backend)
        .unwrap_or(&config::OnchainBackend::None);
    let wallet = &federation.onchain_wallet;

    if !wallet.enabled {
        if matches!(selected, config::OnchainBackend::FederatedBdk) {
            bail!(
                "onchain_backend = \"federated-bdk\" requires federation.onchain_wallet.enabled=true"
            );
        }
        if public_config.frost_wallet.is_some() && matches!(selected, config::OnchainBackend::Bdk) {
            bail!(
                "a federation with an active FROST wallet must use onchain_backend = \"federated-bdk\"; standalone BDK is not a custody backend"
            );
        }
        return Ok(());
    }

    if !matches!(selected, config::OnchainBackend::FederatedBdk) {
        bail!(
            "federation.onchain_wallet.enabled=true requires onchain_backend = \"federated-bdk\""
        );
    }
    if settings.bdk.is_some() {
        bail!(
            "the standalone [bdk] section, including local mnemonic configuration, is forbidden with federated-bdk"
        );
    }
    let frost_wallet = public_config.frost_wallet.as_ref().ok_or_else(|| {
        anyhow!("federated-bdk requires an activated public FROST wallet configuration")
    })?;
    frost_wallet.validate(public_config)?;
    let application = frost_wallet
        .cdk_onchain_application
        .as_ref()
        .ok_or_else(|| anyhow!("federated-bdk requires the CDK on-chain FROST application"))?;
    let network = wallet.network.as_deref().ok_or_else(|| {
        anyhow!("federation.onchain_wallet.network must be explicitly configured")
    })?;
    let network = parse_federation_bitcoin_network(network)?;
    let minimum_confirmations = wallet.minimum_confirmations.ok_or_else(|| {
        anyhow!("federation.onchain_wallet.minimum_confirmations must be explicitly configured")
    })?;
    let fee_policy_version = wallet.fee_policy_version.ok_or_else(|| {
        anyhow!("federation.onchain_wallet.fee_policy_version must be explicitly configured")
    })?;
    if network != application.network
        || minimum_confirmations != application.confirmation_policy.minimum_confirmations
        || fee_policy_version != application.fee_policy_version
    {
        bail!(
            "federation.onchain_wallet network, confirmation policy, or fee policy differs from the active consensus application"
        );
    }
    cdk::mint::FederationOnchainTransactionPolicy::for_version(fee_policy_version)?;
    federation_chain_source(wallet)?;
    Ok(())
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
pub(crate) fn parse_federation_bitcoin_network(value: &str) -> Result<bitcoin::Network> {
    match value.to_ascii_lowercase().as_str() {
        "mainnet" | "bitcoin" => Ok(bitcoin::Network::Bitcoin),
        "testnet" => Ok(bitcoin::Network::Testnet),
        "signet" => Ok(bitcoin::Network::Signet),
        "regtest" => Ok(bitcoin::Network::Regtest),
        network => bail!("unknown federation.onchain_wallet.network {network}"),
    }
}

pub(crate) fn validate_federation_setup_method_startup_policy(
    public_config: &FederationConfig,
) -> Result<()> {
    if public_config
        .setup_transcript
        .as_ref()
        .is_some_and(|transcript| {
            transcript.payload.setup_method == FederationDkgSetupMethod::TrustedDealer
        })
    {
        #[cfg(not(feature = "federation-test-utils"))]
        bail!(
            "trusted-dealer federation configs are test-only; run dealer-free Pedersen DKG for production"
        );
    }

    Ok(())
}

pub(crate) fn validate_private_federation_config_file(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path).with_context(|| {
        format!(
            "failed to inspect private federation config {}",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        bail!(
            "private federation config must be a regular file: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            bail!(
                "private federation config permissions must not grant group or other access: path={}, mode={mode:03o}",
                path.display()
            );
        }
    }
    Ok(())
}

pub(crate) fn read_public_federation_config(path: &Path) -> Result<FederationConfig> {
    let contents = std::fs::read_to_string(path).with_context(|| {
        format!(
            "failed to read public federation config from {}",
            path.display()
        )
    })?;
    let raw: serde_json::Value = serde_json::from_str(&contents).with_context(|| {
        format!(
            "failed to parse public federation config JSON from {}",
            path.display()
        )
    })?;
    let has_payment_observation_policy = raw
        .as_object()
        .and_then(|object| object.get("payment_observation_policy"))
        .is_some();
    if !has_payment_observation_policy {
        bail!(
            "federation public config must explicitly set payment_observation_policy; missing payment policies are not accepted by mintd startup"
        );
    }
    let config = serde_json::from_value(raw).with_context(|| {
        format!(
            "failed to parse public federation config JSON from {}",
            path.display()
        )
    })?;

    Ok(config)
}

pub(crate) fn validate_federation_payment_observation_startup_policy(
    public_config: &FederationConfig,
    allow_development_payment_observation_policy: bool,
) -> Result<()> {
    let development_contexts = [
        FederationPaymentObservationContext::MintQuote,
        FederationPaymentObservationContext::MeltQuote,
    ]
    .into_iter()
    .filter(|context| {
        matches!(
            *public_config.payment_verification_policy(*context),
            FederationPaymentVerificationPolicy::DevelopmentSingleObservation
        )
    })
    .map(|context| context.to_string())
    .collect::<Vec<_>>();

    if !development_contexts.is_empty() && !allow_development_payment_observation_policy {
        bail!(
            "federation payment_observation_policy uses development-only single-observation finality for {}; set federation.allow_development_payment_observation_policy=true only for local development or regtest",
            development_contexts.join(", ")
        );
    }

    Ok(())
}

pub(crate) fn validate_federation_threshold_startup_policy(
    public_config: &FederationConfig,
    allow_development_policy: bool,
) -> Result<()> {
    match public_config.threshold_params.validate_bft_safety() {
        Ok(()) => Ok(()),
        Err(error) if allow_development_policy => {
            tracing::warn!(
                "Allowing development-only unsafe federation threshold profile: {}",
                error
            );
            Ok(())
        }
        Err(error) => Err(error).context(
            "unsafe production federation threshold profile; set federation.allow_development_payment_observation_policy=true only for a local development or regtest configuration",
        ),
    }
}

pub(crate) fn federation_peer_sync_config(
    settings: &config::Settings,
) -> cdk_axum::FederationPeerSyncConfig {
    let peer_sync = settings
        .federation
        .as_ref()
        .map(|federation| &federation.peer_sync)
        .cloned()
        .unwrap_or_default();

    cdk_axum::FederationPeerSyncConfig {
        peer_sync_interval: std::time::Duration::from_secs(peer_sync.peer_sync_interval_secs),
        peer_request_timeout: std::time::Duration::from_secs(peer_sync.peer_request_timeout_secs),
        operation_submission_timeout: std::time::Duration::from_secs(
            peer_sync.operation_submission_timeout_secs,
        ),
        catch_up_limit: peer_sync.catch_up_limit,
        reconnect_initial_backoff: std::time::Duration::from_secs(
            peer_sync.reconnect_initial_backoff_secs,
        ),
        reconnect_max_backoff: std::time::Duration::from_secs(peer_sync.reconnect_max_backoff_secs),
        private_rest_inbound_concurrency_limit: peer_sync.private_rest_inbound_concurrency_limit,
        private_rest_outbound_concurrency_limit: peer_sync.private_rest_outbound_concurrency_limit,
        pending_mempool_item_limit: peer_sync.pending_mempool_item_limit,
        response_waiter_limit: peer_sync.response_waiter_limit,
        response_cache_limit: peer_sync.response_cache_limit,
        aleph_bft_active_unit_interval: std::time::Duration::from_millis(
            peer_sync.aleph_bft_active_unit_interval_ms,
        ),
        aleph_bft_network_pump_max_messages_per_tick: peer_sync
            .aleph_bft_network_pump_max_messages_per_tick,
        aleph_bft_live_session_max_round: peer_sync.aleph_bft_live_session_max_round,
        aleph_bft_live_session_rotation_limit: peer_sync.aleph_bft_live_session_rotation_limit,
    }
}

#[cfg(feature = "federation-bdk")]
pub(crate) fn federation_bdk_local_config(
    settings: &config::Settings,
    work_dir: &Path,
) -> Result<Option<cdk_bdk::FederatedBdkLocalConfig>> {
    let Some(federation) = settings.federation.as_ref() else {
        return Ok(None);
    };
    let wallet = &federation.onchain_wallet;
    if !wallet.enabled {
        return Ok(None);
    }
    if !federation.enabled {
        bail!("federation.onchain_wallet.enabled requires federation.enabled=true");
    }
    if wallet.sync_interval_secs == 0 {
        bail!("federation.onchain_wallet.sync_interval_secs must be greater than zero");
    }
    let chain_source = federation_chain_source(wallet)?;
    let source_scope = match wallet.chain_source_scope.to_ascii_lowercase().as_str() {
        "independent" => cdk_bdk::FederatedChainSourceScope::Independent,
        "shared" => cdk_bdk::FederatedChainSourceScope::Shared,
        scope => bail!(
            "unknown federation.onchain_wallet.chain_source_scope {scope}; expected independent or shared"
        ),
    };
    let source_id = wallet
        .chain_source_id
        .clone()
        .unwrap_or_else(|| chain_source.opaque_source_id());
    let observation_source = cdk_bdk::FederatedChainSourceDescriptor::new(source_scope, source_id)?;
    Ok(Some(cdk_bdk::FederatedBdkLocalConfig {
        chain_source,
        storage_root: work_dir.join("federation-wallet"),
        sync_interval: std::time::Duration::from_secs(wallet.sync_interval_secs),
        observation_source,
    }))
}

#[cfg(feature = "federation-bark")]
pub(crate) fn federation_bark_local_config(
    settings: &config::Settings,
    work_dir: &Path,
    runtime: &FederationRuntimeConfig,
) -> Result<Option<cdk_bark::FederatedBarkLocalConfig>> {
    let Some(federation) = settings.federation.as_ref() else {
        return Ok(None);
    };
    let wallet = &federation.bark;
    if !wallet.enabled {
        return Ok(None);
    }
    let application = cdk_federation_bark::federation_bark_application(runtime.public_config())?
        .ok_or_else(|| anyhow!("accepted Bark application is missing"))?;
    let selected = settings
        .payment_backend
        .iter()
        .find(|entry| entry.backend == PaymentBackendType::FederatedBark)
        .ok_or_else(|| anyhow!("federated Bark Lightning backend is missing"))?;
    let operational_seed = decode_fixed_hex::<64>(
        wallet
            .operational_seed_hex
            .as_deref()
            .ok_or_else(|| anyhow!("federation.bark.operational_seed_hex is missing"))?,
        "federation.bark.operational_seed_hex",
    )?;
    let receive_secret_key = decode_fixed_hex::<32>(
        wallet
            .receive_secret_key_hex
            .as_deref()
            .ok_or_else(|| anyhow!("federation.bark.receive_secret_key_hex is missing"))?,
        "federation.bark.receive_secret_key_hex",
    )?;
    let frost_storage = wallet
        .frost_protocol_storage_root
        .as_deref()
        .ok_or_else(|| anyhow!("federation.bark.frost_protocol_storage_root is missing"))?;
    if !frost_storage.is_dir() {
        bail!("federation.bark FROST protocol storage root is unavailable");
    }
    let data_directory = wallet
        .data_directory
        .clone()
        .unwrap_or_else(|| work_dir.join("federation-bark"));
    if wallet.create_if_missing {
        std::fs::create_dir_all(&data_directory)
            .context("failed to create federated Bark data directory")?;
    }
    let receive_secret_database_path = wallet
        .receive_secret_database_path
        .clone()
        .unwrap_or_else(|| data_directory.join("receive-secrets.redb"));
    let recovery_chain_source = if let Some(esplora_url) = wallet.esplora_url.as_ref() {
        cdk_bark::ChainSource::Esplora(cdk_bark::EsploraConfig {
            url: esplora_url.clone(),
            parallel_requests: 1,
            request_timeout_secs: 15,
            max_retries: 3,
        })
    } else {
        let address = wallet
            .bitcoind_address
            .as_deref()
            .ok_or_else(|| anyhow!("federation.bark recovery requires a chain source"))?;
        let address = url::Url::parse(address)
            .context("federation.bark.bitcoind_address must be an HTTP URL")?;
        let host = address
            .host_str()
            .ok_or_else(|| anyhow!("federation.bark.bitcoind_address has no host"))?
            .to_string();
        let port = address
            .port_or_known_default()
            .ok_or_else(|| anyhow!("federation.bark.bitcoind_address has no port"))?;
        let (user, password) = match (
            wallet.bitcoind_user.as_ref(),
            wallet.bitcoind_password.as_ref(),
            wallet.bitcoind_cookiefile.as_ref(),
        ) {
            (Some(user), Some(password), _) => (user.clone(), password.clone()),
            (None, None, Some(cookie_path)) => {
                let cookie = std::fs::read_to_string(cookie_path)
                    .context("failed to read federation.bark bitcoind cookie")?;
                let (user, password) = cookie.trim().split_once(':').ok_or_else(|| {
                    anyhow!("federation.bark bitcoind cookie has invalid contents")
                })?;
                (user.to_string(), password.to_string())
            }
            _ => bail!("federation.bark bitcoind recovery requires user/password or cookie auth"),
        };
        cdk_bark::ChainSource::BitcoinRpc(cdk_bark::BitcoinRpcConfig {
            host,
            port,
            user,
            password,
            request_timeout_secs: 15,
            wallet_rescan_from_height: Some(0),
        })
    };
    recovery_chain_source.validate()?;
    let recovery_observation_source = cdk_bark::FederatedChainSourceDescriptor::new(
        cdk_bark::FederatedChainSourceScope::Independent,
        recovery_chain_source.opaque_source_id(),
    )?;
    let recovery = cdk_bark::FederatedBarkRecoveryLocalConfig {
        chain_source: recovery_chain_source,
        storage_root: data_directory.join("recovery-wallet"),
        sync_interval: std::time::Duration::from_secs(30),
        observation_source: recovery_observation_source,
    };
    let bindings = cdk_bark::FederatedBarkLocalBindings {
        federation_id: runtime.federation_id(),
        network: application.network,
        ark_server_public_key: cdk::secp256k1::PublicKey::from_slice(
            &application.ark_server_public_key,
        )
        .context("invalid accepted Ark server key")?,
        root_key_epoch: application.root_key_epoch,
        application_id: application.application_id,
        application_key_epoch: application.application_key_epoch,
        threshold: application.threshold,
        members: runtime.public_config().members.keys().copied().collect(),
        policy: application.policy.clone(),
        unit: cdk::nuts::CurrencyUnit::Sat,
    };
    let connection = cdk_bark::FederatedBarkConnectionConfig {
        server_address: wallet
            .server_address
            .clone()
            .ok_or_else(|| anyhow!("federation.bark.server_address is missing"))?,
        server_access_token: wallet.server_access_token.clone(),
        esplora_address: wallet.esplora_url.clone(),
        bitcoind_address: wallet.bitcoind_address.clone(),
        bitcoind_cookiefile: wallet.bitcoind_cookiefile.clone(),
        bitcoind_user: wallet.bitcoind_user.clone(),
        bitcoind_pass: wallet.bitcoind_password.clone(),
    };
    Ok(Some(cdk_bark::FederatedBarkLocalConfig::from_seed_bytes(
        bindings,
        connection,
        data_directory,
        receive_secret_database_path,
        operational_seed,
        receive_secret_key,
        wallet.create_if_missing,
        cdk_bark::FederatedBarkStoreAvailability {
            frost_protocol_store: true,
            receive_secret_store: true,
        },
        cdk_bark::FederatedBarkLimits::new(
            u64::from(selected.min_mint),
            u64::from(selected.max_mint),
        )?,
        cdk_bark::FederatedBarkMaintenanceControls {
            automatic_refresh: wallet.automatic_refresh,
            automatic_emergency_exit: wallet.automatic_emergency_exit,
        },
        recovery,
    )?))
}

#[cfg(feature = "federation-bark")]
pub(crate) fn decode_fixed_hex<const N: usize>(
    value: &str,
    field: &'static str,
) -> Result<[u8; N]> {
    let bytes = cdk::util::hex::decode(value)
        .with_context(|| format!("{field} must be valid hexadecimal"))?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| anyhow!("{field} must contain {N} bytes, found {}", bytes.len()))
}

#[cfg(feature = "federation-bdk")]
pub(crate) fn federation_chain_source(
    wallet: &config::FederationOnchainWallet,
) -> Result<cdk_bdk::ChainSource> {
    let chain_source = match wallet.chain_source_type.to_ascii_lowercase().as_str() {
        "esplora" => {
            let url = wallet
                .esplora_url
                .as_ref()
                .filter(|url| !url.trim().is_empty())
                .ok_or_else(|| {
                    anyhow!(
                        "federation.onchain_wallet.esplora_url is required for Esplora"
                    )
                })?;
            if wallet.esplora_parallel_requests == 0 {
                bail!(
                    "federation.onchain_wallet.esplora_parallel_requests must be greater than zero"
                );
            }
            cdk_bdk::ChainSource::Esplora(cdk_bdk::EsploraConfig {
                url: url.clone(),
                parallel_requests: wallet.esplora_parallel_requests,
                request_timeout_secs: wallet.request_timeout_secs,
                max_retries: wallet.esplora_max_retries,
            })
        }
        "bitcoinrpc" => {
            let host = wallet
                .bitcoind_rpc_host
                .as_ref()
                .filter(|host| !host.trim().is_empty())
                .ok_or_else(|| {
                    anyhow!(
                        "federation.onchain_wallet.bitcoind_rpc_host is required for Bitcoin RPC"
                    )
                })?;
            let port = wallet.bitcoind_rpc_port.ok_or_else(|| {
                anyhow!("federation.onchain_wallet.bitcoind_rpc_port is required for Bitcoin RPC")
            })?;
            let user = wallet
                .bitcoind_rpc_user
                .as_ref()
                .filter(|user| !user.is_empty())
                .ok_or_else(|| {
                    anyhow!(
                        "federation.onchain_wallet.bitcoind_rpc_user is required for Bitcoin RPC"
                    )
                })?;
            let password = wallet
                .bitcoind_rpc_password
                .as_ref()
                .filter(|password| !password.is_empty())
                .ok_or_else(|| {
                    anyhow!(
                        "federation.onchain_wallet.bitcoind_rpc_password is required for Bitcoin RPC"
                    )
                })?;
            cdk_bdk::ChainSource::BitcoinRpc(cdk_bdk::BitcoinRpcConfig {
                host: host.clone(),
                port,
                user: user.clone(),
                password: password.clone(),
                request_timeout_secs: wallet.request_timeout_secs,
            wallet_rescan_from_height: Some(0),
            })
        }
        source => bail!(
            "unknown federation.onchain_wallet.chain_source_type {source}; expected bitcoinrpc or esplora"
        ),
    };
    chain_source.validate()?;
    Ok(chain_source)
}

pub(crate) fn federation_dkg_auto_start_config(
    settings: &config::Settings,
) -> Result<Option<cdk_axum::FederationDkgAutoStartConfig>> {
    let Some(federation) = settings.federation.as_ref() else {
        return Ok(None);
    };
    if !federation.dkg.enabled {
        return Ok(None);
    }
    if !federation.enabled {
        bail!("federation.dkg.enabled requires federation.enabled=true");
    }
    if federation.dkg.max_ticks == 0 {
        bail!("federation.dkg.max_ticks must be greater than zero");
    }
    if federation.dkg.retry_delay_ms == 0 {
        bail!("federation.dkg.retry_delay_ms must be greater than zero");
    }
    if federation.dkg.ceremony_timeout_secs == 0 {
        bail!("federation.dkg.ceremony_timeout_secs must be greater than zero");
    }
    if !(1..=64).contains(&federation.dkg.member_request_burst) {
        bail!("federation.dkg.member_request_burst must be between 1 and 64");
    }
    if federation.dkg.member_request_refill_ms < 10 {
        bail!("federation.dkg.member_request_refill_ms must be at least 10");
    }
    let setup_config_path = federation
        .setup_config_path
        .as_deref()
        .ok_or_else(|| anyhow!("federation.setup_config_path must be set when DKG is enabled"))?;
    let handoff = federation_setup_command::load_certified_dkg_handoff(setup_config_path)?;
    let driver = cdk_axum::FederationDkgDriverRunConfig {
        max_ticks: federation.dkg.max_ticks,
        retry_delay: std::time::Duration::from_millis(federation.dkg.retry_delay_ms),
        ceremony_timeout: std::time::Duration::from_secs(federation.dkg.ceremony_timeout_secs),
        secret_share_transport:
            cdk_axum::FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
        member_request_burst: federation.dkg.member_request_burst,
        member_request_refill_interval: std::time::Duration::from_millis(
            federation.dkg.member_request_refill_ms,
        ),
    };
    cdk_axum::FederationDkgAutoStartConfig::from_manifest(
        &handoff.manifest,
        handoff.url_policy,
        driver,
    )
    .map(Some)
    .map_err(anyhow::Error::from)
}

pub(crate) async fn reconcile_federation_dkg_ceremony(
    settings: &config::Settings,
    localstore: &DynMintDatabase,
) -> Result<()> {
    let Some(start_config) = federation_dkg_auto_start_config(settings)? else {
        return Ok(());
    };
    let federation = settings
        .federation
        .as_ref()
        .expect("enabled DKG config includes federation settings");
    let runtime_config = configured_federation_runtime(settings)?;
    validate_federation_threshold_startup_policy(
        runtime_config.public_config(),
        federation.allow_development_payment_observation_policy,
    )?;
    let desired = FederationDkgCeremonyRecord::active(
        runtime_config.federation_id(),
        runtime_config.public_config().setup_authorization,
        start_config.ceremony_id(),
        runtime_config.member_id(),
        start_config.keyset_spec().clone(),
        None,
    );
    let records = localstore
        .get_federation_dkg_ceremonies()
        .await
        .context("failed to read federation DKG ceremony lifecycle")?;
    if let Some(existing) = records
        .iter()
        .find(|record| record.ceremony_id == desired.ceremony_id)
    {
        let expected = match existing.state {
            FederationDkgCeremonyState::Active => desired,
            FederationDkgCeremonyState::Complete => desired.completed()?,
            FederationDkgCeremonyState::Aborted => {
                bail!(
                    "federation DKG ceremony {} was durably aborted and cannot be resumed",
                    existing.ceremony_id
                )
            }
        };
        if existing != &expected {
            bail!(
                "federation DKG ceremony {} lifecycle does not match configured federation, member, keyset policy, or predecessor",
                existing.ceremony_id
            );
        }
        return Ok(());
    }

    if !records.is_empty() {
        bail!(
            "local DKG lifecycle belongs to a different certified manifest; initial setup replacement requires a new setup ID and independent state"
        );
    }

    let mut tx = localstore.begin_transaction().await?;
    tx.add_federation_dkg_ceremony(&desired).await?;
    tx.commit().await?;

    Ok(())
}

pub(crate) fn read_federation_json<T>(path: &Path, label: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {label} from {}", path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {label} JSON from {}", path.display()))
}

pub(crate) async fn federation_config_paths(
    cli_args: &CLIArgs,
    public_config: &Option<PathBuf>,
    private_config: &Option<PathBuf>,
    command_name: &str,
) -> Result<(PathBuf, PathBuf)> {
    match (public_config, private_config) {
        (Some(public_config), Some(private_config)) => {
            Ok((public_config.clone(), private_config.clone()))
        }
        (None, None) => {
            let work_dir = get_work_directory(cli_args).await?;
            let settings = load_stored_settings(&work_dir, cli_args).await?;
            let federation = settings
                .federation
                .ok_or_else(|| anyhow!("mint config has no [federation] section"))?;
            let public_config = federation
                .public_config_path
                .ok_or_else(|| anyhow!("mint config [federation] has no public_config_path"))?;
            let private_config = federation
                .private_config_path
                .ok_or_else(|| anyhow!("mint config [federation] has no private_config_path"))?;
            Ok((public_config, private_config))
        }
        _ => bail!(
            "{command_name} requires --public-config and --private-config to be provided together"
        ),
    }
}
