//! `cdk-mintd federation setup`: argument parsing, serving and printing over
//! the setup ceremony in `cdk_federation::setup`.

#[cfg(feature = "sqlite")]
use std::collections::BTreeSet;
use std::path::Path;
#[cfg(feature = "sqlite")]
use std::str::FromStr;
#[cfg(feature = "sqlite")]
use std::sync::Arc;

#[cfg(feature = "sqlite")]
use anyhow::{anyhow, bail, Context, Result};
#[cfg(not(feature = "sqlite"))]
use anyhow::{bail, Result};
#[cfg(all(feature = "sqlite", feature = "federation-bark"))]
use cdk_common::bitcoin::secp256k1::PublicKey;
#[cfg(feature = "sqlite")]
use cdk_common::database::{DynMintDatabase, MintFederationSetupDatabase};
#[cfg(all(feature = "sqlite", feature = "federation-bdk"))]
use cdk_common::federation::FederationSetupFrostApplication;
#[cfg(feature = "sqlite")]
use cdk_common::federation::{
    FederationCheckpointPolicy, FederationDkgKeysetSpec, FederationPaymentObservationPolicy,
    FederationSetupBitcoinNetwork, FederationSetupFrostPolicy, FederationSetupInvitation,
    FederationSetupLocalRole, FederationSetupProposalHash, FederationSetupUrlPolicy, MemberId,
    ThresholdParams,
};
#[cfg(feature = "sqlite")]
use cdk_common::mint_url::MintUrl;
#[cfg(feature = "sqlite")]
use cdk_common::{Amount, CurrencyUnit};
#[cfg(feature = "sqlite")]
use cdk_federation::setup::ceremony::{
    activate_setup, approve_setup, build_certified_dkg_mint, coordinator_service, create_setup,
    current_unix_time, fetch_and_review_proposal, issue_invitation, join_setup,
    load_certified_dkg_handoff, propose_setup, read_local_config, review_local_proposal,
    setup_status, start_certified_dkg, FederationSetupActivationResult, FederationSetupCreateSpec,
    FederationSetupLocalStatus,
};
#[cfg(feature = "sqlite")]
use cdk_federation::setup::files::read_private_text_file;
#[cfg(feature = "sqlite")]
use cdk_federation::setup::FederationSetupHttpClient;

use crate::cli;

#[cfg(feature = "sqlite")]
pub(crate) async fn run_setup_command(
    cli_args: &cli::CLIArgs,
    args: &cli::FederationSetupArgs,
) -> Result<()> {
    let config_path = cli_args
        .command_config()
        .ok_or_else(|| anyhow!("`federation setup` commands require --config <setup config>"))?;
    match &args.command {
        cli::FederationSetupCommand::Create(args) => {
            let thresholds = ThresholdParams::with_bft_consensus(args.members, args.threshold)
                .context("invalid federation threshold profile")?;
            let payment_threshold = args
                .payment_observation_threshold
                .unwrap_or(thresholds.consensus_threshold);
            let network = setup_network(args.network);
            let spec = FederationSetupCreateSpec {
                threshold_params: thresholds,
                public_mint_url: parse_mint_url(&args.public_mint_url, "public mint URL")?,
                federation_api_url: parse_mint_url(&args.federation_api_url, "federation API URL")?,
                coordinator_url: parse_mint_url(&args.coordinator_url, "coordinator URL")?,
                network,
                payment_observation_policy: FederationPaymentObservationPolicy::member_quorum(
                    payment_threshold,
                ),
                checkpoint_policy: FederationCheckpointPolicy {
                    checkpoint_interval_journal_operations: args
                        .checkpoint_interval_journal_operations,
                    checkpoint_interval_finalized_items: args.checkpoint_interval_finalized_items,
                    checkpoint_interval_canonical_bytes: args.checkpoint_interval_canonical_bytes,
                },
                keysets: vec![FederationDkgKeysetSpec::new(
                    CurrencyUnit::from_str(&args.unit).context("invalid setup currency unit")?,
                    true,
                    args.input_fee_ppk,
                    args.final_expiry,
                    args.amounts
                        .iter()
                        .copied()
                        .map(Amount::from)
                        .collect::<BTreeSet<_>>(),
                )
                .context("invalid setup keyset")?],
                frost: setup_frost_policy(args, network)?,
                invitation_lifetime_secs: args.invitation_lifetime_seconds,
                url_policy: setup_url_policy(args.allow_insecure_localhost),
            };
            let db = cdk_sqlite::MintSqliteDatabase::new(args.database.clone())
                .await
                .context("failed to open federation setup database")?;
            let created = create_setup(
                &db,
                config_path,
                &args.state_dir,
                &args.database,
                &spec,
                current_unix_time()?,
            )
            .await?;
            println!("setup_id: {}", created.setup_id);
            println!("config: {}", created.config_path.display());
            for (member_id, path) in created.invitation_paths {
                println!("member_{}_invitation: {}", member_id.get(), path.display());
            }
        }
        cli::FederationSetupCommand::Invite(args) => {
            let config = read_local_config(config_path)?;
            let db = cdk_sqlite::MintSqliteDatabase::new(config.database_path().to_path_buf())
                .await
                .context("failed to open federation setup database")?;
            let invitation = issue_invitation(
                &db,
                config_path,
                MemberId::new(args.member).context("invalid setup member ID")?,
                args.replace,
                &args.output,
                current_unix_time()?,
            )
            .await?;
            println!("setup_id: {}", invitation.setup_id);
            println!("member_id: {}", invitation.member_id.get());
            println!("invitation_file: {}", args.output.display());
        }
        cli::FederationSetupCommand::Join(args) => {
            let coordinator = setup_client(crate::federation_setup_transport(cli_args).await?);
            let invitation =
                read_private_text_file(&args.invitation_file, "federation setup invitation")?
                    .trim()
                    .parse::<FederationSetupInvitation>()
                    .context("invalid federation setup invitation")?;
            let db = cdk_sqlite::MintSqliteDatabase::new(args.database.clone())
                .await
                .context("failed to open federation setup database")?;
            let enrollment = join_setup(
                &db,
                &coordinator,
                config_path,
                &args.state_dir,
                &args.database,
                invitation,
                parse_mint_url(&args.public_mint_url, "public mint URL")?,
                parse_mint_url(&args.federation_api_url, "federation API URL")?,
                setup_url_policy(args.allow_insecure_localhost),
                current_unix_time()?,
            )
            .await?;
            println!("setup_id: {}", enrollment.setup_id);
            println!("member_id: {}", enrollment.member_id.get());
        }
        cli::FederationSetupCommand::Propose => {
            let config = read_local_config(config_path)?;
            let db = cdk_sqlite::MintSqliteDatabase::new(config.database_path().to_path_buf())
                .await
                .context("failed to open federation setup database")?;
            let proposal = propose_setup(&db, config_path, current_unix_time()?).await?;
            println!("proposal_hash: {}", proposal.proposal_hash);
        }
        cli::FederationSetupCommand::Review(args) => {
            let config = read_local_config(config_path)?;
            let db = cdk_sqlite::MintSqliteDatabase::new(config.database_path().to_path_buf())
                .await
                .context("failed to open federation setup database")?;
            let review = match config.local_role() {
                FederationSetupLocalRole::Coordinator { .. } => {
                    let proposal = db
                        .get_federation_setup_proposal(config.setup_id())
                        .await
                        .context("failed to load the local setup proposal")?
                        .ok_or_else(|| anyhow!("no local setup proposal is available to review"))?;
                    review_local_proposal(&db, config_path, proposal, current_unix_time()?).await?
                }
                FederationSetupLocalRole::Member { .. } => {
                    let coordinator =
                        setup_client(crate::federation_setup_transport(cli_args).await?);
                    fetch_and_review_proposal(&db, &coordinator, config_path, current_unix_time()?)
                        .await?
                }
            };
            match args.json {
                true => println!("{}", review.to_pretty_json()?),
                false => println!("{review}"),
            }
        }
        cli::FederationSetupCommand::Approve(args) => {
            let coordinator = setup_client(crate::federation_setup_transport(cli_args).await?);
            let config = read_local_config(config_path)?;
            let db = cdk_sqlite::MintSqliteDatabase::new(config.database_path().to_path_buf())
                .await
                .context("failed to open federation setup database")?;
            let hash = FederationSetupProposalHash::from_str(&args.proposal_hash)
                .context("invalid exact proposal hash")?;
            let approval =
                approve_setup(&db, &coordinator, config_path, hash, current_unix_time()?).await?;
            println!("approved_proposal_hash: {}", approval.proposal_hash);
        }
        cli::FederationSetupCommand::Serve(args) => {
            if args.dkg_max_ticks == 0
                || args.dkg_retry_delay_ms == 0
                || args.dkg_ceremony_timeout_seconds == 0
            {
                bail!("setup DKG tick, retry-delay, and ceremony-timeout bounds must be positive");
            }
            let driver = cdk_axum::FederationDkgDriverRunConfig {
                max_ticks: args.dkg_max_ticks,
                retry_delay: std::time::Duration::from_millis(args.dkg_retry_delay_ms),
                ceremony_timeout: std::time::Duration::from_secs(args.dkg_ceremony_timeout_seconds),
                ..cdk_axum::FederationDkgDriverRunConfig::default()
            };
            let (transport, http_enabled) =
                crate::federation_setup_server_transport(cli_args).await?;
            serve_setup_coordinator(config_path, args.listen, driver, transport, http_enabled)
                .await?;
        }
        cli::FederationSetupCommand::Status(args) => {
            let coordinator = setup_client(crate::federation_setup_transport(cli_args).await?);
            let database: DynMintDatabase =
                open_setup_database(read_local_config(config_path)?.database_path()).await?;
            let status = setup_status(&database, config_path, &coordinator).await?;
            match args.json {
                true => println!("{}", serde_json::to_string_pretty(&status)?),
                false => print_setup_status(&status),
            }
        }
        cli::FederationSetupCommand::Activate => {
            eprintln!(
                "Before using real funds, create and verify an encrypted backup of this member's setup state, database, and activated private config."
            );
            let database: DynMintDatabase =
                open_setup_database(read_local_config(config_path)?.database_path()).await?;
            let result = activate_setup(&database, config_path, current_unix_time()?).await?;
            print_activation_result(&result);
        }
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
fn setup_frost_policy(
    args: &cli::FederationSetupCreateArgs,
    network: FederationSetupBitcoinNetwork,
) -> Result<FederationSetupFrostPolicy> {
    let applications = vec![
        #[cfg(feature = "federation-bdk")]
        FederationSetupFrostApplication::CdkOnchain {
            application_key_epoch: args.frost_application_key_epoch,
            confirmation_policy_version: 1,
            minimum_confirmations: args.frost_minimum_confirmations,
            fee_policy_version: args.frost_fee_policy_version,
        },
    ];
    #[cfg(feature = "federation-bark")]
    let mut applications = applications;
    let bark = match (
        args.frost_bark_ark_server_public_key.as_deref(),
        args.frost_bark_application_key_epoch,
        args.frost_bark_policy_config.as_deref(),
    ) {
        (None, None, None) => None,
        (Some(ark_server_public_key), Some(application_key_epoch), Some(policy_path)) => {
            Some((ark_server_public_key, application_key_epoch, policy_path))
        }
        _ => bail!(
            "setup Bark policy requires --frost-bark-ark-server-public-key, \
             --frost-bark-application-key-epoch, and --frost-bark-policy-config together"
        ),
    };
    #[cfg(feature = "federation-bark")]
    if let Some((ark_server_public_key, application_key_epoch, policy_path)) = bark {
        let ark_server_public_key = PublicKey::from_str(ark_server_public_key)
            .context("invalid compressed Ark server public key")?;
        let policy: cdk_federation_bark::FederationBarkPolicyConfig =
            crate::read_federation_json(policy_path, "federation setup Bark public policy config")?;
        applications.push(cdk_federation_bark::federation_bark_setup_application(
            cdk_federation_bark::FederationBarkSetupConfig {
                application_key_epoch,
                network: network.bitcoin_network(),
                ark_server_public_key,
                policy,
            },
        )?);
        applications.push(
            cdk_federation_bark::federation_bark_recovery_setup_application(
                cdk_federation_bark::FederationBarkRecoverySetupConfig {
                    application_key_epoch,
                    network: network.bitcoin_network(),
                    minimum_confirmations: args.frost_minimum_confirmations,
                    fee_policy_version: args.frost_fee_policy_version,
                },
            )?,
        );
    }
    #[cfg(not(feature = "federation-bark"))]
    if bark.is_some() {
        bail!("setup Bark policy requires the `federation-bark` feature");
    }
    FederationSetupFrostPolicy::new(args.frost_root_key_epoch, applications, network)
        .context("invalid setup FROST policy")
}

#[cfg(not(feature = "sqlite"))]
pub(crate) async fn run_setup_command(
    _cli_args: &cli::CLIArgs,
    _args: &cli::FederationSetupArgs,
) -> Result<()> {
    bail!("federation setup commands currently require the `sqlite` feature")
}

#[cfg(feature = "sqlite")]
async fn serve_setup_coordinator(
    config_path: &Path,
    listen: std::net::SocketAddr,
    driver: cdk_axum::FederationDkgDriverRunConfig,
    transport: cdk_axum::FederationTransport,
    http_enabled: bool,
) -> Result<()> {
    let config = read_local_config(config_path)?;
    if config.manifest_path().exists() {
        return serve_certified_dkg(config_path, listen, driver, transport, http_enabled).await;
    }
    let database = open_setup_database(config.database_path()).await?;
    let service = Arc::new(coordinator_service(database, config_path)?);
    let router = cdk_axum::create_federation_setup_router(service.clone());
    let listener = if http_enabled {
        Some(
            tokio::net::TcpListener::bind(listen)
                .await
                .context("failed to bind setup-only coordinator listener")?,
        )
    } else {
        None
    };
    println!("setup_id: {}", config.setup_id());
    match http_enabled {
        true => println!("setup_only_listen: {listen}"),
        false => println!("setup_only_listen: disabled"),
    }
    let result =
        serve_setup_router(listener, router, transport, "setup-only coordinator server").await;
    service.shutdown().await;
    result
}

#[cfg(feature = "sqlite")]
async fn serve_certified_dkg(
    config_path: &Path,
    listen: std::net::SocketAddr,
    driver: cdk_axum::FederationDkgDriverRunConfig,
    transport: cdk_axum::FederationTransport,
    http_enabled: bool,
) -> Result<()> {
    let handoff = load_certified_dkg_handoff(config_path)?;
    let database = open_setup_database(read_local_config(config_path)?.database_path()).await?;
    let isolated_keystore = Arc::new(
        cdk_sqlite::mint::memory::empty()
            .await
            .context("failed to create isolated setup-only keystore")?,
    );
    let mint_database: DynMintDatabase = database.clone();
    let mint = build_certified_dkg_mint(
        mint_database.clone(),
        #[cfg(feature = "federation-bark")]
        database,
        isolated_keystore,
        &handoff,
    )
    .await?;
    let auto_start = start_certified_dkg(&mint_database, &handoff, driver).await?;
    let member_id = handoff.private_config.member_id;
    let ceremony_id = auto_start.ceremony_id();
    let router = cdk_axum::create_federation_dkg_setup_router_with_transport(
        Arc::new(mint),
        cdk_axum::FederationPeerSyncConfig::default(),
        auto_start,
        transport.clone(),
    )
    .await
    .context("failed to create setup-only DKG router")?;
    let listener = if http_enabled {
        Some(
            tokio::net::TcpListener::bind(listen)
                .await
                .context("failed to bind setup-only DKG listener")?,
        )
    } else {
        None
    };
    println!("setup_id: {}", handoff.manifest.proposal.body.setup_id);
    println!("member_id: {member_id}");
    println!("ceremony_id: {ceremony_id}");
    match http_enabled {
        true => println!("dkg_only_listen: {listen}"),
        false => println!("dkg_only_listen: disabled"),
    }
    serve_setup_router(listener, router, transport, "setup-only DKG server").await
}

#[cfg(feature = "sqlite")]
async fn serve_setup_router(
    listener: Option<tokio::net::TcpListener>,
    router: axum::Router,
    transport: cdk_axum::FederationTransport,
    context: &'static str,
) -> Result<()> {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
    let mut http_task = listener.map(|listener| {
        let http_router = router.clone();
        tokio::spawn(async move {
            axum::serve(
                listener,
                http_router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.recv().await;
            })
            .await
        })
    });

    #[cfg(feature = "iroh")]
    let iroh_server = transport
        .runtime_node()
        .map(|node| cdk_iroh::IrohServer::start(node, router));
    #[cfg(not(feature = "iroh"))]
    let _ = (transport, router);

    #[cfg(feature = "iroh")]
    let early_exit = match (http_task.as_mut(), iroh_server.as_ref()) {
        (Some(http_task), Some(server)) => {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal.context("setup listener shutdown signal failed")?;
                    None
                }
                result = http_task => Some(result.context("setup HTTP listener task failed")?.context(context)),
                () = server.finished() => Some(Err(anyhow!("{context} Iroh listener exited unexpectedly"))),
            }
        }
        (Some(http_task), None) => {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal.context("setup listener shutdown signal failed")?;
                    None
                }
                result = http_task => Some(result.context("setup HTTP listener task failed")?.context(context)),
            }
        }
        (None, Some(server)) => {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal.context("setup listener shutdown signal failed")?;
                    None
                }
                () = server.finished() => Some(Err(anyhow!("{context} Iroh listener exited unexpectedly"))),
            }
        }
        (None, None) => Some(Err(anyhow!("{context} has no enabled listener"))),
    };
    #[cfg(not(feature = "iroh"))]
    let early_exit = match http_task.as_mut() {
        Some(http_task) => tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.context("setup listener shutdown signal failed")?;
                None
            }
            result = http_task => Some(result.context("setup HTTP listener task failed")?.context(context)),
        },
        None => Some(Err(anyhow!("{context} has no enabled listener"))),
    };

    let _ = shutdown_tx.send(());
    if let Some(http_task) = http_task {
        if !http_task.is_finished() {
            http_task
                .await
                .context("setup HTTP listener task failed")?
                .context(context)?;
        }
    }
    #[cfg(feature = "iroh")]
    if let Some(server) = iroh_server {
        server
            .shutdown()
            .await
            .context("setup Iroh shutdown failed")?;
    }
    match early_exit {
        Some(result) => result,
        None => Ok(()),
    }
}

#[cfg(feature = "sqlite")]
fn print_setup_status(status: &FederationSetupLocalStatus) {
    println!("setup_id: {}", status.setup_id);
    println!("member_id: {}", status.member_id);
    println!("participant_phase: {}", status.participant_phase);
    if let Some(phase) = status.coordinator_phase.as_deref() {
        println!("coordinator_phase: {phase}");
    }
    println!(
        "enrollments: {}/{} missing={}",
        status.enrolled_members.len(),
        status.member_count,
        display_setup_members(&status.missing_enrollments)
    );
    println!(
        "approvals: {}/{} missing={}",
        status.approved_members.len(),
        status.member_count,
        display_setup_members(&status.missing_approvals)
    );
    if let Some(dkg) = status.dkg.as_ref() {
        println!(
            "dkg_phase: {} commitments={} reveals={} shares={} transcript_signatures={} final_confirmations={}/{} frost_roots={} finalized_ready={}",
            dkg.phase,
            dkg.commitments,
            dkg.reveals,
            dkg.secret_share_contributions,
            dkg.transcript_signatures,
            dkg.final_config_confirmations,
            status.member_count,
            dkg.frost_root_public_configs,
            dkg.finalized_ready
        );
        if let Some(digest) = dkg.final_config_digest.as_deref() {
            println!("final_config_digest: {digest}");
        }
    }
    println!("activated: {}", status.activation.is_some());
}

#[cfg(feature = "sqlite")]
fn display_setup_members(members: &[MemberId]) -> String {
    match members {
        [] => "none".to_string(),
        members => members
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
    }
}

#[cfg(feature = "sqlite")]
fn print_activation_result(result: &FederationSetupActivationResult) {
    print!("{}", activation_service_manager_output(result));
}

#[cfg(feature = "sqlite")]
fn activation_service_manager_output(result: &FederationSetupActivationResult) -> String {
    format!(
        "activated=true\nsetup_id={}\nmember_id={}\nfederation_id={}\nfinal_config_digest={}\nfederation_public_config_path={}\nfederation_private_config_path={}\nfederation_setup_config_path=unset\nfederation_dkg_enabled=false\nservice_restart_required=true\n",
        result.setup_id,
        result.member_id,
        result.federation_id,
        result.final_config_digest,
        result.public_config_path.display(),
        result.private_config_path.display(),
    )
}

#[cfg(feature = "sqlite")]
fn setup_network(network: cli::FederationSetupNetworkArg) -> FederationSetupBitcoinNetwork {
    match network {
        cli::FederationSetupNetworkArg::Bitcoin => FederationSetupBitcoinNetwork::Bitcoin,
        cli::FederationSetupNetworkArg::Testnet => FederationSetupBitcoinNetwork::Testnet,
        cli::FederationSetupNetworkArg::Signet => FederationSetupBitcoinNetwork::Signet,
        cli::FederationSetupNetworkArg::Regtest => FederationSetupBitcoinNetwork::Regtest,
    }
}

#[cfg(feature = "sqlite")]
fn setup_url_policy(allow_insecure_localhost: bool) -> FederationSetupUrlPolicy {
    match allow_insecure_localhost {
        true => FederationSetupUrlPolicy::AllowInsecureLocalhost,
        false => FederationSetupUrlPolicy::RequireSecureTransport,
    }
}

#[cfg(feature = "sqlite")]
fn parse_mint_url(value: &str, label: &str) -> Result<MintUrl> {
    MintUrl::from_str(value).with_context(|| format!("invalid {label}"))
}

/// The typed setup client over the daemon's transport.
#[cfg(feature = "sqlite")]
fn setup_client(transport: cdk_axum::FederationTransport) -> FederationSetupHttpClient {
    #[cfg(feature = "iroh")]
    if let Some(node) = transport.runtime_node() {
        return FederationSetupHttpClient::with_iroh_node(node);
    }
    let _ = transport;
    FederationSetupHttpClient::default()
}

/// Open the setup database a local setup config names.
#[cfg(feature = "sqlite")]
async fn open_setup_database(database_path: &Path) -> Result<Arc<cdk_sqlite::MintSqliteDatabase>> {
    Ok(Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(database_path.to_path_buf())
            .await
            .context("failed to open federation setup database")?,
    ))
}

#[cfg(all(test, feature = "sqlite"))]
mod tests;
