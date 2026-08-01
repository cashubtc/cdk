use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use cdk::amount::SplitTarget;
use cdk::cdk_database::{Error as DatabaseError, WalletDatabase};
use cdk::mint::FederationRuntimeConfig;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, Id, MeltQuoteState, MintQuoteState, PaymentMethod, State};
use cdk::wallet::{
    FederatedMintConnector, FederatedMintConnectorConfig, HttpClient, MeltOutcome, MintConnector,
    NUT13Options, Wallet, WalletBuilder,
};
use cdk::{Amount, Error};
use cdk_common::federation::{
    FederationConfig, FederationJournalAppendOutcome, FederationKeysetRotationOperation,
    FederationOperation, FederationOperationEnvelope, FederationRestMessageKind,
    FederationSignedRequest, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    MemberConfigPrivate, MemberConfigPublic, MemberId,
};
use cdk_common::wallet::{
    MeltQuote as WalletMeltQuote, MeltSagaState, WalletSaga, WalletSagaState,
};
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
use cdk_http_client::Transport;
use futures::stream::{FuturesUnordered, StreamExt};

use crate::cli::{FederationSmokeOperationsArgs, FederationSmokeScenario};
use crate::federation_status::{query_local_statuses_prevalidated, FederationLocalStatusResponse};

const STANDARD_SMOKE_SEED: [u8; 64] = [7u8; 64];
const PENDING_PAID_SMOKE_SEED: [u8; 64] = [8u8; 64];
const PENDING_FAILED_SMOKE_SEED: [u8; 64] = [9u8; 64];
const OFFLINE_MEMBER_SMOKE_SEED: [u8; 64] = [10u8; 64];
const KEYSET_ROTATION_SMOKE_SEED: [u8; 64] = [11u8; 64];
const MEMBER_STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const FEDERATION_SMOKE_MEMBER_TIMEOUT: Duration = Duration::from_secs(60);
const FEDERATION_SMOKE_RESTORE_BATCH_SIZE: u32 = 8;
const FEDERATION_SMOKE_RESTORE_MAX_GAP: u32 = 2;

fn smoke_seed(base: [u8; 64], offset: u8) -> [u8; 64] {
    let mut seed = base;
    seed[63] = seed[63].wrapping_add(offset);
    seed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmokeMemberExpectation {
    AllMembers,
    ThresholdMembers,
}

pub(crate) async fn run_smoke_operations_command(
    args: &FederationSmokeOperationsArgs,
) -> Result<()> {
    validate_smoke_operation_args(args)?;

    let public_config: FederationConfig =
        crate::read_federation_json(&args.public_config, "public federation config")?;
    public_config
        .validate()
        .context("invalid public federation config")?;

    let unit = CurrencyUnit::from_str(&args.unit)
        .with_context(|| format!("invalid federation smoke unit {}", args.unit))?;
    let member_connectors = member_connectors(&public_config);
    let federation_connector = FederatedMintConnector::new_with_config(
        public_config.clone(),
        member_connectors.clone(),
        smoke_connector_config(args),
    )
    .context("federated wallet connector")?;
    let connector = Arc::new(federation_connector);

    for batch_index in 0..args.operation_batches {
        let batch_seed_offset = args
            .seed_offset
            .checked_add(batch_index)
            .context("smoke operation batch seed offset overflow")?;
        let run_scenario = async {
            match args.scenario {
                FederationSmokeScenario::Standard => {
                    run_standard_smoke(
                        args,
                        &public_config,
                        unit.clone(),
                        &member_connectors,
                        connector.clone(),
                        smoke_seed(STANDARD_SMOKE_SEED, batch_seed_offset),
                        SmokeMemberExpectation::AllMembers,
                    )
                    .await
                }
                FederationSmokeScenario::OfflineMemberOperations => {
                    run_standard_smoke(
                        args,
                        &public_config,
                        unit.clone(),
                        &member_connectors,
                        connector.clone(),
                        smoke_seed(OFFLINE_MEMBER_SMOKE_SEED, batch_seed_offset),
                        SmokeMemberExpectation::ThresholdMembers,
                    )
                    .await
                }
                FederationSmokeScenario::PendingMeltStart => {
                    let wallet_db = args
                        .wallet_db
                        .as_deref()
                        .context("--wallet-db is required for pending-melt-start")?;
                    run_pending_melt_start(
                        args,
                        &public_config,
                        unit.clone(),
                        &member_connectors,
                        connector.clone(),
                        PENDING_PAID_SMOKE_SEED,
                        wallet_db,
                        MeltQuoteState::Paid,
                    )
                    .await
                }
                FederationSmokeScenario::PendingMeltFinish => {
                    let wallet_db = args
                        .wallet_db
                        .as_deref()
                        .context("--wallet-db is required for pending-melt-finish")?;
                    run_pending_melt_finish(
                        args,
                        &public_config,
                        unit.clone(),
                        &member_connectors,
                        connector.clone(),
                        PENDING_PAID_SMOKE_SEED,
                        wallet_db,
                        MeltQuoteState::Paid,
                        SmokeMemberExpectation::ThresholdMembers,
                    )
                    .await
                }
                FederationSmokeScenario::PendingMeltFailStart => {
                    let wallet_db = args
                        .wallet_db
                        .as_deref()
                        .context("--wallet-db is required for pending-melt-fail-start")?;
                    run_pending_melt_start(
                        args,
                        &public_config,
                        unit.clone(),
                        &member_connectors,
                        connector.clone(),
                        PENDING_FAILED_SMOKE_SEED,
                        wallet_db,
                        MeltQuoteState::Failed,
                    )
                    .await
                }
                FederationSmokeScenario::PendingMeltFailFinish => {
                    let wallet_db = args
                        .wallet_db
                        .as_deref()
                        .context("--wallet-db is required for pending-melt-fail-finish")?;
                    run_pending_melt_finish(
                        args,
                        &public_config,
                        unit.clone(),
                        &member_connectors,
                        connector.clone(),
                        PENDING_FAILED_SMOKE_SEED,
                        wallet_db,
                        MeltQuoteState::Failed,
                        SmokeMemberExpectation::ThresholdMembers,
                    )
                    .await
                }
                FederationSmokeScenario::KeysetRotation => {
                    let private_config_path = args
                        .private_config
                        .as_deref()
                        .context("--private-config is required for keyset-rotation")?;
                    let private_config: MemberConfigPrivate = crate::read_federation_json(
                        private_config_path,
                        "private federation config",
                    )?;
                    run_keyset_rotation_smoke(
                        args,
                        &public_config,
                        unit.clone(),
                        &member_connectors,
                        connector.clone(),
                        smoke_seed(KEYSET_ROTATION_SMOKE_SEED, batch_seed_offset),
                        &private_config,
                        SmokeMemberExpectation::AllMembers,
                    )
                    .await
                }
                FederationSmokeScenario::OfflineMemberKeysetRotation => {
                    let private_config_path = args.private_config.as_deref().context(
                        "--private-config is required for offline-member-keyset-rotation",
                    )?;
                    let private_config: MemberConfigPrivate = crate::read_federation_json(
                        private_config_path,
                        "private federation config",
                    )?;
                    run_keyset_rotation_smoke(
                        args,
                        &public_config,
                        unit.clone(),
                        &member_connectors,
                        connector.clone(),
                        smoke_seed(KEYSET_ROTATION_SMOKE_SEED, batch_seed_offset),
                        &private_config,
                        SmokeMemberExpectation::ThresholdMembers,
                    )
                    .await
                }
            }
        };

        match args.operation_timeout_seconds {
            Some(timeout_seconds) => {
                match tokio::time::timeout(Duration::from_secs(timeout_seconds), run_scenario).await
                {
                    Ok(result) => result?,
                    Err(_) => bail!(
                        "federation smoke scenario {} batch {} did not complete within {} seconds",
                        smoke_scenario_name(args.scenario),
                        u16::from(batch_index).saturating_add(1),
                        timeout_seconds
                    ),
                }
            }
            None => run_scenario.await?,
        }
    }

    Ok(())
}

fn validate_smoke_operation_args(args: &FederationSmokeOperationsArgs) -> Result<()> {
    if args.mint_amount == 0 {
        bail!("--mint-amount must be greater than zero");
    }
    if args.quote_retries == 0 {
        bail!("--quote-retries must be greater than zero");
    }
    if args.quote_sleep_ms == 0 {
        bail!("--quote-sleep-ms must be greater than zero to avoid busy polling");
    }
    if args.operation_timeout_seconds == Some(0) {
        bail!("--operation-timeout-seconds must be greater than zero");
    }
    if args.operation_batches == 0 {
        bail!("--operation-batches must be greater than zero");
    }
    if args.operation_batches > 1 && !scenario_supports_operation_batches(args.scenario) {
        bail!(
            "--operation-batches is only supported for federation smoke scenarios standard and offline-member-operations"
        );
    }
    let last_seed_offset = u16::from(args.seed_offset)
        .saturating_add(u16::from(args.operation_batches))
        .saturating_sub(1);
    if last_seed_offset > u16::from(u8::MAX) {
        bail!(
            "last smoke seed offset from --seed-offset and --operation-batches must not exceed 255"
        );
    }
    if scenario_requires_wallet_db(args.scenario) && args.wallet_db.is_none() {
        bail!(
            "--wallet-db is required for federation smoke scenario {}",
            smoke_scenario_name(args.scenario)
        );
    }
    if scenario_requires_private_config(args.scenario) && args.private_config.is_none() {
        bail!(
            "--private-config is required for federation smoke scenario {}",
            smoke_scenario_name(args.scenario)
        );
    }
    Ok(())
}

fn smoke_connector_config(args: &FederationSmokeOperationsArgs) -> FederatedMintConnectorConfig {
    let mut config = FederatedMintConnectorConfig::default();
    config.member_fallback_timeout = FEDERATION_SMOKE_MEMBER_TIMEOUT;
    config.member_request_timeout = FEDERATION_SMOKE_MEMBER_TIMEOUT;
    config.quote_status_member_timeout = FEDERATION_SMOKE_MEMBER_TIMEOUT;
    config.mint_quote_visibility_attempts = args.quote_retries;
    config.mint_quote_visibility_retry_delay = Duration::from_millis(args.quote_sleep_ms);
    config
}

fn scenario_supports_operation_batches(scenario: FederationSmokeScenario) -> bool {
    matches!(
        scenario,
        FederationSmokeScenario::Standard | FederationSmokeScenario::OfflineMemberOperations
    )
}

fn scenario_requires_wallet_db(scenario: FederationSmokeScenario) -> bool {
    matches!(
        scenario,
        FederationSmokeScenario::PendingMeltStart
            | FederationSmokeScenario::PendingMeltFinish
            | FederationSmokeScenario::PendingMeltFailStart
            | FederationSmokeScenario::PendingMeltFailFinish
    )
}

fn scenario_requires_private_config(scenario: FederationSmokeScenario) -> bool {
    matches!(
        scenario,
        FederationSmokeScenario::KeysetRotation
            | FederationSmokeScenario::OfflineMemberKeysetRotation
    )
}

fn smoke_scenario_name(scenario: FederationSmokeScenario) -> &'static str {
    match scenario {
        FederationSmokeScenario::Standard => "standard",
        FederationSmokeScenario::OfflineMemberOperations => "offline-member-operations",
        FederationSmokeScenario::PendingMeltStart => "pending-melt-start",
        FederationSmokeScenario::PendingMeltFinish => "pending-melt-finish",
        FederationSmokeScenario::PendingMeltFailStart => "pending-melt-fail-start",
        FederationSmokeScenario::PendingMeltFailFinish => "pending-melt-fail-finish",
        FederationSmokeScenario::KeysetRotation => "keyset-rotation",
        FederationSmokeScenario::OfflineMemberKeysetRotation => "offline-member-keyset-rotation",
    }
}

fn print_smoke_stage(stage: &str, started_at: Instant) {
    println!(
        "federation smoke stage: stage={} elapsed_ms={}",
        stage,
        started_at.elapsed().as_millis()
    );
    let _ = io::stdout().flush();
}

fn print_smoke_substage(stage_prefix: &str, suffix: &str, started_at: Instant) {
    let stage = format!("{stage_prefix}.{suffix}");
    print_smoke_stage(&stage, started_at);
}

async fn run_standard_smoke(
    args: &FederationSmokeOperationsArgs,
    public_config: &FederationConfig,
    unit: CurrencyUnit,
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    connector: Arc<FederatedMintConnector>,
    seed: [u8; 64],
    member_expectation: SmokeMemberExpectation,
) -> Result<()> {
    let smoke_started_at = Instant::now();
    print_smoke_stage("standard.wallet.start", smoke_started_at);
    let wallet = smoke_wallet(public_config, unit.clone(), seed, connector.clone(), None).await?;
    print_smoke_stage("standard.wallet.done", smoke_started_at);

    print_smoke_stage("standard.mint.start", smoke_started_at);
    let minted_amount = mint_smoke_funds(
        &wallet,
        public_config,
        member_connectors,
        args,
        member_expectation,
        "standard.mint",
        smoke_started_at,
    )
    .await?;
    print_smoke_stage("standard.mint.done", smoke_started_at);

    print_smoke_stage("standard.swap.start", smoke_started_at);
    let proofs_before_swap = wallet
        .get_unspent_proofs()
        .await
        .context("load minted proofs")?;
    wallet
        .swap(
            None,
            SplitTarget::default(),
            proofs_before_swap.clone(),
            None,
            false,
            false,
        )
        .await
        .context("swap through federation connector")?;
    let proofs_after_swap = wallet
        .get_unspent_proofs()
        .await
        .context("load swapped proofs")?;
    print_smoke_stage("standard.swap.done", smoke_started_at);
    let swapped_amount = proofs_after_swap.total_amount()?;
    if swapped_amount != minted_amount {
        bail!(
            "federation smoke swapped {}, expected {}",
            swapped_amount,
            minted_amount
        );
    }

    print_smoke_stage("standard.check-state.start", smoke_started_at);
    let spent_states = wallet
        .check_proofs_spent(proofs_before_swap)
        .await
        .context("check spent state for swapped inputs")?;
    if spent_states
        .iter()
        .any(|proof_state| proof_state.state != State::Spent)
    {
        bail!("federation smoke check-state did not mark swapped inputs spent");
    }
    print_smoke_stage("standard.check-state.done", smoke_started_at);

    print_smoke_stage("standard.melt-quote.start", smoke_started_at);
    let melt_invoice = create_fake_invoice(1000, "federation smoke melt".to_string());
    let melt_quote = wallet
        .melt_quote(PaymentMethod::BOLT11, melt_invoice, None, None)
        .await
        .context("create federation melt quote")?;
    print_smoke_stage("standard.melt-quote.done", smoke_started_at);
    print_smoke_stage("standard.melt-visibility.start", smoke_started_at);
    wait_for_melt_quote_visible(
        member_connectors,
        public_config,
        &melt_quote.id,
        args.quote_retries,
        Duration::from_millis(args.quote_sleep_ms),
        member_expectation,
    )
    .await?;
    print_smoke_stage("standard.melt-visibility.done", smoke_started_at);

    print_smoke_stage("standard.melt-confirm.start", smoke_started_at);
    let finalized = wallet
        .prepare_melt(&melt_quote.id, HashMap::new())
        .await
        .context("prepare federation melt")?
        .confirm()
        .await
        .context("confirm federation melt")?;
    if finalized.state() != MeltQuoteState::Paid {
        bail!(
            "federation smoke melt ended in state {}, expected paid",
            finalized.state()
        );
    }
    print_smoke_stage("standard.melt-confirm.done", smoke_started_at);

    print_smoke_stage("standard.restore.start", smoke_started_at);
    let restored_wallet = smoke_wallet(public_config, unit, seed, connector, None).await?;
    let restored = restored_wallet.restore().await.context("restore wallet")?;
    if restored.unspent == Amount::ZERO
        && restored.spent == Amount::ZERO
        && restored.pending == Amount::ZERO
    {
        bail!("federation smoke restore recovered no proofs");
    }
    print_smoke_stage("standard.restore.done", smoke_started_at);

    println!(
        "federation operation smoke passed: minted={} swapped={} melted={} fee={} restored_unspent={} restored_spent={} restored_pending={}",
        minted_amount,
        swapped_amount,
        finalized.amount(),
        finalized.fee_paid(),
        restored.unspent,
        restored.spent,
        restored.pending
    );

    Ok(())
}

async fn run_keyset_rotation_smoke(
    args: &FederationSmokeOperationsArgs,
    public_config: &FederationConfig,
    unit: CurrencyUnit,
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    connector: Arc<FederatedMintConnector>,
    seed: [u8; 64],
    private_config: &MemberConfigPrivate,
    member_expectation: SmokeMemberExpectation,
) -> Result<()> {
    let smoke_started_at = Instant::now();
    let old_keyset = public_config
        .keysets
        .iter()
        .find(|keyset| keyset.active && keyset.unit == unit)
        .context("keyset rotation smoke needs an active keyset for the requested unit")?
        .clone();
    let mut next_keyset = public_config
        .keysets
        .iter()
        .find(|keyset| !keyset.active && keyset.unit == unit)
        .context("keyset rotation smoke needs an inactive rotation keyset for the requested unit")?
        .clone();
    next_keyset.active = true;

    let wallet = smoke_wallet(public_config, unit.clone(), seed, connector, None).await?;
    let pre_rotation_amount = mint_smoke_funds(
        &wallet,
        public_config,
        member_connectors,
        args,
        member_expectation,
        "keyset-rotation.pre-mint",
        smoke_started_at,
    )
    .await?;

    let runtime_config =
        FederationRuntimeConfig::new(public_config.clone(), private_config.clone())
            .context("private federation config does not match public federation config")?;
    let rotation = FederationKeysetRotationOperation {
        keyset: next_keyset.clone(),
    };
    let envelope =
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(rotation.clone()));
    let rotation_response = submit_smoke_operation_to_expected_members(
        public_config,
        private_config,
        envelope,
        member_expectation,
    )
    .await?;
    let rotated_public_config = public_config
        .apply_keyset_rotation(&rotation)
        .context("apply keyset rotation to smoke wallet config")?;

    wait_for_keyset_rotation_visible(
        member_connectors,
        public_config,
        old_keyset.id,
        next_keyset.id,
        args.quote_retries,
        Duration::from_millis(args.quote_sleep_ms),
        member_expectation,
    )
    .await?;

    let rotated_connector = Arc::new(
        FederatedMintConnector::new_with_config(
            rotated_public_config.clone(),
            member_connectors.clone(),
            smoke_connector_config(args),
        )
        .context("rotated federated wallet connector")?,
    );
    let restored_wallet = smoke_wallet(
        &rotated_public_config,
        unit.clone(),
        seed,
        rotated_connector.clone(),
        None,
    )
    .await?;
    print_smoke_stage("keyset-rotation.restore-old.start", smoke_started_at);
    let restored = restored_wallet
        .restore_with_opts(federation_smoke_restore_opts()?)
        .await
        .context("restore pre-rotation proofs after keyset rotation")?;
    print_smoke_stage("keyset-rotation.restore-old.done", smoke_started_at);
    if restored.unspent < pre_rotation_amount {
        bail!(
            "keyset rotation smoke restored {} old-keyset unspent, expected at least {}",
            restored.unspent,
            pre_rotation_amount
        );
    }

    let old_proofs = restored_wallet
        .get_unspent_proofs()
        .await
        .context("load restored old-keyset proofs")?;
    let old_proof_amount = old_proofs.total_amount()?;
    if old_proof_amount < pre_rotation_amount {
        bail!(
            "keyset rotation smoke loaded {} old-keyset proofs, expected at least {}",
            old_proof_amount,
            pre_rotation_amount
        );
    }
    restored_wallet
        .swap(
            None,
            SplitTarget::default(),
            old_proofs.clone(),
            None,
            false,
            false,
        )
        .await
        .context("swap restored old-keyset proofs after rotation")?;
    let spent_states = restored_wallet
        .check_proofs_spent(old_proofs)
        .await
        .context("check restored old-keyset proofs spent after rotation")?;
    if spent_states
        .iter()
        .any(|proof_state| proof_state.state != State::Spent)
    {
        bail!("keyset rotation smoke did not mark restored old-keyset inputs spent");
    }

    let post_rotation_amount = mint_smoke_funds(
        &restored_wallet,
        &rotated_public_config,
        member_connectors,
        args,
        member_expectation,
        "keyset-rotation.post-mint",
        smoke_started_at,
    )
    .await?;
    print_smoke_stage("keyset-rotation.melt-quote.start", smoke_started_at);
    let melt_invoice = create_fake_invoice(1000, "federation rotation smoke melt".to_string());
    let melt_quote = restored_wallet
        .melt_quote(PaymentMethod::BOLT11, melt_invoice, None, None)
        .await
        .context("create keyset rotation smoke melt quote")?;
    print_smoke_stage("keyset-rotation.melt-quote.done", smoke_started_at);
    print_smoke_stage("keyset-rotation.melt-visibility.start", smoke_started_at);
    wait_for_melt_quote_visible(
        member_connectors,
        &rotated_public_config,
        &melt_quote.id,
        args.quote_retries,
        Duration::from_millis(args.quote_sleep_ms),
        member_expectation,
    )
    .await?;
    print_smoke_stage("keyset-rotation.melt-visibility.done", smoke_started_at);
    print_smoke_stage("keyset-rotation.melt-confirm.start", smoke_started_at);
    let finalized = restored_wallet
        .prepare_melt(&melt_quote.id, HashMap::new())
        .await
        .context("prepare keyset rotation smoke melt")?
        .confirm()
        .await
        .context("confirm keyset rotation smoke melt")?;
    print_smoke_stage("keyset-rotation.melt-confirm.done", smoke_started_at);
    if finalized.state() != MeltQuoteState::Paid {
        bail!(
            "keyset rotation smoke melt ended in state {}, expected paid",
            finalized.state()
        );
    }

    print_smoke_stage("keyset-rotation.final-restore.start", smoke_started_at);
    let final_wallet =
        smoke_wallet(&rotated_public_config, unit, seed, rotated_connector, None).await?;
    let final_restored = final_wallet
        .restore_with_opts(federation_smoke_restore_opts()?)
        .await
        .context("restore rotated wallet after post-rotation operations")?;
    print_smoke_stage("keyset-rotation.final-restore.done", smoke_started_at);
    if final_restored.unspent == Amount::ZERO
        && final_restored.spent == Amount::ZERO
        && final_restored.pending == Amount::ZERO
    {
        bail!("keyset rotation smoke final restore recovered no proofs");
    }

    println!(
        "federation keyset rotation smoke passed: old_keyset={} new_keyset={} rotation={} restored_old={} post_rotation_minted={} melted={} final_unspent={} final_spent={} final_pending={}",
        old_keyset.id,
        next_keyset.id,
        rotation_response.operation_id,
        restored.unspent,
        post_rotation_amount,
        finalized.amount(),
        final_restored.unspent,
        final_restored.spent,
        final_restored.pending
    );

    Ok(())
}

fn federation_smoke_restore_opts() -> Result<NUT13Options> {
    NUT13Options::new(
        FEDERATION_SMOKE_RESTORE_BATCH_SIZE,
        FEDERATION_SMOKE_RESTORE_MAX_GAP,
    )
    .context("build federation smoke restore options")
}

async fn run_pending_melt_start(
    args: &FederationSmokeOperationsArgs,
    public_config: &FederationConfig,
    unit: CurrencyUnit,
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    connector: Arc<FederatedMintConnector>,
    seed: [u8; 64],
    wallet_db: &Path,
    terminal_state: MeltQuoteState,
) -> Result<()> {
    let smoke_started_at = Instant::now();
    let wallet = smoke_wallet(public_config, unit, seed, connector, Some(wallet_db)).await?;
    let minted_amount = mint_smoke_funds(
        &wallet,
        public_config,
        member_connectors,
        args,
        SmokeMemberExpectation::AllMembers,
        "pending-melt.mint",
        smoke_started_at,
    )
    .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let melt_invoice = create_fake_invoice(
        1000,
        serde_json::to_string(&fake_description).context("serialize fake invoice description")?,
    );
    let melt_quote = wallet
        .melt_quote(PaymentMethod::BOLT11, melt_invoice, None, None)
        .await
        .context("create pending federation melt quote")?;
    wait_for_melt_quote_visible(
        member_connectors,
        public_config,
        &melt_quote.id,
        args.quote_retries,
        Duration::from_millis(args.quote_sleep_ms),
        SmokeMemberExpectation::AllMembers,
    )
    .await?;

    let prepared = wallet
        .prepare_melt(&melt_quote.id, HashMap::new())
        .await
        .context("prepare pending federation melt")?;
    let operation_id = prepared.operation_id();
    match prepared
        .confirm_prefer_async()
        .await
        .context("start pending federation melt")?
    {
        MeltOutcome::Pending(_) => {}
        MeltOutcome::Paid(finalized) => {
            bail!(
                "pending federation melt finalized immediately in state {}, expected pending",
                finalized.state()
            );
        }
    }

    let stored_saga = wallet
        .localstore
        .get_saga(&operation_id)
        .await
        .context("load persisted pending melt saga")?;
    if stored_saga
        .as_ref()
        .and_then(|saga| saga.quote_id.as_deref())
        != Some(melt_quote.id.as_str())
    {
        bail!(
            "pending federation melt quote {} was not recoverable from wallet saga {}",
            melt_quote.id,
            operation_id
        );
    }

    println!(
        "federation pending melt smoke started: minted={} quote={} operation={} terminal={}",
        minted_amount, melt_quote.id, operation_id, terminal_state
    );

    Ok(())
}

async fn run_pending_melt_finish(
    args: &FederationSmokeOperationsArgs,
    public_config: &FederationConfig,
    unit: CurrencyUnit,
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    connector: Arc<FederatedMintConnector>,
    seed: [u8; 64],
    wallet_db: &Path,
    terminal_state: MeltQuoteState,
    member_expectation: SmokeMemberExpectation,
) -> Result<()> {
    let wallet = smoke_wallet(public_config, unit, seed, connector, Some(wallet_db)).await?;
    let quote = recoverable_pending_melt_quote(&wallet)
        .await
        .context("pending melt finish found no recoverable melt quote in wallet db")?;
    let quote_id = quote.id.clone();

    submit_terminal_melt_payment_observation(
        args,
        public_config,
        member_connectors,
        &quote,
        terminal_state,
        member_expectation,
    )
    .await?;

    let expected_wallet_state = wallet_melt_quote_state_for_terminal(terminal_state);
    let terminal = wait_for_pending_melt_finalization(
        &wallet,
        &quote_id,
        expected_wallet_state,
        args.quote_retries,
        Duration::from_millis(args.quote_sleep_ms),
    )
    .await
    .with_context(|| {
        format!(
            "pending federation melt did not finalize as {expected_wallet_state} \
             after terminal {terminal_state} observation"
        )
    })?;

    wait_for_melt_quote_state(
        member_connectors,
        public_config,
        &quote_id,
        member_melt_quote_state_for_terminal(terminal_state),
        args.quote_retries,
        Duration::from_millis(args.quote_sleep_ms),
        member_expectation,
    )
    .await?;

    let pending_quotes_after = wallet
        .get_pending_melt_quotes()
        .await
        .context("load pending melt quotes after recovery")?;
    if pending_quotes_after
        .iter()
        .any(|quote| quote.id == quote_id)
    {
        bail!(
            "pending federation melt quote {} remained pending after {} recovery",
            quote_id,
            terminal_state
        );
    }

    if terminal_state == MeltQuoteState::Failed {
        let unspent_amount = wallet
            .get_unspent_proofs()
            .await
            .context("load proofs after failed pending melt recovery")?
            .total_amount()?;
        let expected_amount = Amount::from(args.mint_amount);
        if unspent_amount != expected_amount {
            bail!(
                "failed pending federation melt left {} unspent, expected restored {}",
                unspent_amount,
                expected_amount
            );
        }
    }

    println!(
        "federation pending melt smoke finalized: quote={} state={} amount={} fee={} change={}",
        terminal.quote_id(),
        terminal.state(),
        terminal.amount(),
        terminal.fee_paid(),
        terminal
            .change()
            .and_then(|proofs| proofs.total_amount().ok())
            .unwrap_or(Amount::ZERO)
    );

    Ok(())
}

async fn recoverable_pending_melt_quote(wallet: &Wallet) -> Result<WalletMeltQuote> {
    let sagas = wallet
        .localstore
        .get_incomplete_sagas()
        .await
        .context("load incomplete wallet sagas")?;
    let quote_id = select_recoverable_pending_melt_quote_id(&sagas, &wallet.mint_url, &wallet.unit)
        .context("no incomplete melt saga references a pending melt quote")?;
    let quote = wallet
        .localstore
        .get_melt_quote(&quote_id)
        .await
        .context("load recoverable pending melt quote")?
        .with_context(|| {
            format!("recoverable pending melt saga referenced missing quote {quote_id}")
        })?;

    Ok(quote)
}

fn select_recoverable_pending_melt_quote_id(
    sagas: &[WalletSaga],
    wallet_mint_url: &MintUrl,
    unit: &CurrencyUnit,
) -> Option<String> {
    sagas
        .iter()
        .filter(|saga| {
            &saga.mint_url == wallet_mint_url
                && &saga.unit == unit
                && matches!(
                    saga.state,
                    WalletSagaState::Melt(
                        MeltSagaState::MeltRequested | MeltSagaState::PaymentPending
                    )
                )
        })
        .min_by_key(|saga| (saga.created_at, saga.updated_at, saga.id.to_string()))
        .and_then(|saga| saga.quote_id.clone())
}

async fn wait_for_pending_melt_finalization(
    wallet: &Wallet,
    quote_id: &str,
    expected_state: MeltQuoteState,
    retries: u16,
    sleep: Duration,
) -> Result<cdk_common::FinalizedMelt> {
    for attempt in 0..retries {
        let finalized = wallet
            .finalize_pending_melts()
            .await
            .context("finalize pending federation melt")?;
        if let Some(terminal) = finalized
            .into_iter()
            .find(|melt| melt.quote_id() == quote_id && melt.state() == expected_state)
        {
            return Ok(terminal);
        }

        if attempt.saturating_add(1) < retries {
            tokio::time::sleep(sleep).await;
        }
    }

    bail!(
        "pending federation melt quote {} did not finalize as {} after {} attempts",
        quote_id,
        expected_state,
        retries
    )
}

async fn submit_terminal_melt_payment_observation(
    args: &FederationSmokeOperationsArgs,
    public_config: &FederationConfig,
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    quote: &WalletMeltQuote,
    terminal_state: MeltQuoteState,
    member_expectation: SmokeMemberExpectation,
) -> Result<()> {
    let private_config_path = args.private_config.as_deref().with_context(|| {
        format!("--private-config is required to finalize pending melt as {terminal_state}")
    })?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(private_config_path, "private federation config")?;
    let runtime_config =
        FederationRuntimeConfig::new(public_config.clone(), private_config.clone())
            .context("private federation config does not match public federation config")?;
    let target_member = public_config
        .member(private_config.member_id)
        .context("private federation member is missing from the public federation config")?;
    let envelope = runtime_config.melt_quote_payment_observation(
        &quote.id,
        quote.payment_method.clone(),
        terminal_state,
        quote.amount,
        quote.fee_reserve,
        match terminal_state {
            MeltQuoteState::Paid => Some(
                quote
                    .amount
                    .checked_add(quote.fee_reserve)
                    .context("smoke melt amount plus fee reserve overflow")?,
            ),
            _ => None,
        },
        match terminal_state {
            MeltQuoteState::Paid => Some(format!("federation-smoke-paid-{}", quote.id)),
            _ => None,
        },
    );
    let sequence = initial_smoke_sequence().saturating_add(1);
    submit_smoke_operation(
        &cdk_axum::default_federation_transport(),
        public_config,
        &private_config,
        target_member,
        sequence,
        envelope,
    )
    .await?;
    wait_for_melt_quote_state(
        member_connectors,
        public_config,
        &quote.id,
        member_melt_quote_state_for_terminal(terminal_state),
        args.quote_retries,
        Duration::from_millis(args.quote_sleep_ms),
        member_expectation,
    )
    .await?;

    Ok(())
}

fn member_melt_quote_state_for_terminal(terminal_state: MeltQuoteState) -> MeltQuoteState {
    match terminal_state {
        MeltQuoteState::Failed => MeltQuoteState::Unpaid,
        state => state,
    }
}

fn wallet_melt_quote_state_for_terminal(terminal_state: MeltQuoteState) -> MeltQuoteState {
    match terminal_state {
        MeltQuoteState::Failed => MeltQuoteState::Unpaid,
        state => state,
    }
}

async fn mint_smoke_funds(
    wallet: &Wallet,
    public_config: &FederationConfig,
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    args: &FederationSmokeOperationsArgs,
    member_expectation: SmokeMemberExpectation,
    stage_prefix: &str,
    started_at: Instant,
) -> Result<Amount> {
    print_smoke_substage(stage_prefix, "quote.start", started_at);
    let quote = wallet
        .mint_quote(
            PaymentMethod::BOLT11,
            Some(Amount::from(args.mint_amount)),
            None,
            None,
        )
        .await
        .context("create federation mint quote")?;
    print_smoke_substage(stage_prefix, "quote.done", started_at);
    print_smoke_substage(stage_prefix, "quote-paid.start", started_at);
    wait_for_mint_quote_paid(
        member_connectors,
        public_config,
        &quote.id,
        args.quote_retries,
        Duration::from_millis(args.quote_sleep_ms),
        member_expectation,
    )
    .await?;
    print_smoke_substage(stage_prefix, "quote-paid.done", started_at);
    print_smoke_substage(stage_prefix, "ready-members.start", started_at);
    wait_for_ready_members_after_consensus(
        public_config,
        args,
        member_expectation,
        "mint quote payment",
    )
    .await?;
    print_smoke_substage(stage_prefix, "ready-members.done", started_at);

    print_smoke_substage(stage_prefix, "request.start", started_at);
    let minted = wallet
        .mint(&quote.id, SplitTarget::default(), None)
        .await
        .context("mint through federation connector")?;
    print_smoke_substage(stage_prefix, "request.done", started_at);
    let minted_amount = minted.total_amount()?;
    if minted_amount != Amount::from(args.mint_amount) {
        bail!(
            "federation smoke minted {}, expected {}",
            minted_amount,
            args.mint_amount
        );
    }

    Ok(minted_amount)
}

async fn wait_for_ready_members_after_consensus(
    public_config: &FederationConfig,
    args: &FederationSmokeOperationsArgs,
    member_expectation: SmokeMemberExpectation,
    context: &str,
) -> Result<()> {
    if member_expectation == SmokeMemberExpectation::ThresholdMembers {
        return Ok(());
    }
    let Some(private_config_path) = args.private_config.as_deref() else {
        return Ok(());
    };
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(private_config_path, "private federation config")?;
    private_config
        .validate_against_public(public_config)
        .context("private federation config does not match public federation config")?;
    let required = required_member_observations(public_config, member_expectation);
    let sleep = Duration::from_millis(args.quote_sleep_ms);
    let mut last_ready = 0usize;
    for _ in 0..args.quote_retries {
        match query_local_statuses_prevalidated(
            cdk_axum::default_federation_transport(),
            public_config,
            &private_config,
            &[],
            false,
        )
        .await
        {
            Ok(statuses) => {
                last_ready = statuses
                    .iter()
                    .filter(|status| status_ready_for_smoke(status))
                    .count();
            }
            Err(err) => {
                tracing::debug!(
                    "federation smoke readiness status unavailable after {context}: {}",
                    err
                );
                last_ready = 0;
            }
        }
        if last_ready >= required {
            return Ok(());
        }
        tokio::time::sleep(sleep).await;
    }

    bail!(
        "federation smoke saw only {last_ready} ready members after {context}, required {required}"
    )
}

fn status_ready_for_smoke(status: &FederationLocalStatusResponse) -> bool {
    status.sync_ready
}

async fn submit_smoke_operation<T>(
    transport: &T,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    target_member: &MemberConfigPublic,
    sequence: u64,
    envelope: FederationOperationEnvelope,
) -> Result<FederationSubmitOperationResponse>
where
    T: Transport,
{
    let request = FederationSubmitOperationRequest::new(envelope);
    request.ensure_admissible_size()?;
    let operation_id = request.operation_id()?;
    let signed_request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        target_member.member_id,
        sequence,
        FederationRestMessageKind::SubmitOperation,
        request,
        private_config,
    )?;
    let url = target_member
        .federation_api_url
        .join_paths(&["federation", "v1", "operations"])?;
    let response: FederationSubmitOperationResponse =
        transport.http_post(url, None, &signed_request).await?;
    if response.operation_id != operation_id {
        bail!(
            "federation member {} returned operation id {} for submitted operation {}",
            target_member.member_id,
            response.operation_id,
            operation_id
        );
    }
    let (outcome, index) = match &response.outcome {
        FederationJournalAppendOutcome::Appended(entry) => ("appended", entry.index),
        FederationJournalAppendOutcome::Duplicate(entry) => ("duplicate", entry.index),
    };
    tracing::debug!(
        member_id = %target_member.member_id,
        %operation_id,
        outcome,
        index,
        "federation smoke operation submitted"
    );

    Ok(response)
}

async fn submit_smoke_operation_to_expected_members(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    envelope: FederationOperationEnvelope,
    member_expectation: SmokeMemberExpectation,
) -> Result<FederationSubmitOperationResponse> {
    let operation_id = envelope.operation_id()?;
    let required = required_member_observations(public_config, member_expectation);
    let sequence_base = initial_smoke_sequence().saturating_add(1);
    let transport = cdk_axum::default_federation_transport();
    let mut submissions = FuturesUnordered::new();
    for (index, member) in public_config.members.values().enumerate() {
        let sequence_offset =
            u64::try_from(index).context("federation member index does not fit in u64")?;
        let sequence = sequence_base.saturating_add(sequence_offset);
        let transport = &transport;
        let member = member.clone();
        let envelope = envelope.clone();
        submissions.push(async move {
            (
                member.member_id,
                submit_smoke_operation(
                    transport,
                    public_config,
                    private_config,
                    &member,
                    sequence,
                    envelope,
                )
                .await,
            )
        });
    }

    let mut successful = Vec::new();
    let mut failures = Vec::new();
    let mut remaining = public_config.members.len();
    while let Some((member_id, result)) = submissions.next().await {
        remaining = remaining.saturating_sub(1);
        match result {
            Ok(response) => {
                successful.push(response);
                if successful.len() >= required {
                    return successful
                        .into_iter()
                        .next()
                        .context("successful member submissions disappeared");
                }
            }
            Err(err) => failures.push(format!("member {member_id}: {err}")),
        }

        if successful.len().saturating_add(remaining) < required {
            bail!(
                "federation smoke operation {} reached {} successful member submissions, required {}; failures: {}",
                operation_id,
                successful.len(),
                required,
                failures.join("; ")
            );
        }
    }

    bail!(
        "federation smoke operation {} reached {} successful member submissions, required {}; failures: {}",
        operation_id,
        successful.len(),
        required,
        failures.join("; ")
    )
}

fn initial_smoke_sequence() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| {
            duration
                .as_secs()
                .saturating_mul(1_000_000_000)
                .saturating_add(u64::from(duration.subsec_nanos()))
        })
        .unwrap_or_default()
}

fn member_connectors(
    public_config: &FederationConfig,
) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
    public_config
        .members
        .iter()
        .map(|(member_id, member)| {
            let client = HttpClient::new(member.public_mint_url.clone(), None);
            (
                *member_id,
                Arc::new(client) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect()
}

async fn smoke_wallet(
    public_config: &FederationConfig,
    unit: CurrencyUnit,
    seed: [u8; 64],
    connector: Arc<FederatedMintConnector>,
    wallet_db: Option<&Path>,
) -> Result<Wallet, Error> {
    let primary_member = public_config
        .members
        .values()
        .next()
        .ok_or_else(|| Error::Custom("empty federation roster".to_string()))?;
    let localstore: Arc<dyn WalletDatabase<DatabaseError> + Send + Sync> = match wallet_db {
        Some(path) => {
            let path = path.to_string_lossy().to_string();
            Arc::new(cdk_sqlite::WalletSqliteDatabase::new(path.as_str()).await?)
        }
        None => Arc::new(cdk_sqlite::wallet::memory::empty().await?),
    };

    WalletBuilder::new()
        .mint_url(primary_member.public_mint_url.clone())
        .unit(unit)
        .localstore(localstore)
        .seed(seed)
        .target_proof_count(3)
        .shared_client(connector)
        .build()
}

async fn wait_for_mint_quote_paid(
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    public_config: &FederationConfig,
    quote_id: &str,
    retries: u16,
    sleep: Duration,
    member_expectation: SmokeMemberExpectation,
) -> Result<()> {
    let required = required_member_observations(public_config, member_expectation);
    let quote_id = quote_id.to_string();
    for _ in 0..retries {
        let mut paid = 0usize;
        for (member_id, connector) in member_connectors {
            match tokio::time::timeout(
                MEMBER_STATUS_TIMEOUT,
                connector.get_mint_quote_status(PaymentMethod::BOLT11, &quote_id),
            )
            .await
            {
                Ok(Ok(status)) if status.state() == Some(MintQuoteState::Paid) => {
                    paid = paid.saturating_add(1);
                }
                Ok(Ok(status)) => {
                    tracing::debug!(
                        member_id = %member_id,
                        state = ?status.state(),
                        "federation smoke mint quote is not paid yet"
                    );
                }
                Ok(Err(err)) => {
                    tracing::debug!(
                        member_id = %member_id,
                        "federation smoke mint quote status unavailable: {}",
                        err
                    );
                }
                Err(_) => {
                    tracing::debug!(
                        member_id = %member_id,
                        timeout_seconds = MEMBER_STATUS_TIMEOUT.as_secs(),
                        "federation smoke mint quote status timed out"
                    );
                }
            }
            if paid >= required {
                return Ok(());
            }
        }

        tokio::time::sleep(sleep).await;
    }

    bail!(
        "federation smoke mint quote {} did not reach {} paid member observations after {} attempts",
        quote_id,
        required,
        retries
    )
}

async fn wait_for_melt_quote_visible(
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    public_config: &FederationConfig,
    quote_id: &str,
    retries: u16,
    sleep: Duration,
    member_expectation: SmokeMemberExpectation,
) -> Result<()> {
    let required = required_member_observations(public_config, member_expectation);
    let quote_id = quote_id.to_string();
    for _ in 0..retries {
        let mut visible = 0usize;
        for (member_id, connector) in member_connectors {
            match tokio::time::timeout(
                MEMBER_STATUS_TIMEOUT,
                connector.get_melt_quote_status(PaymentMethod::BOLT11, &quote_id),
            )
            .await
            {
                Ok(Ok(_)) => {
                    visible = visible.saturating_add(1);
                }
                Ok(Err(err)) => {
                    tracing::debug!(
                        member_id = %member_id,
                        "federation smoke melt quote status unavailable: {}",
                        err
                    );
                }
                Err(_) => {
                    tracing::debug!(
                        member_id = %member_id,
                        timeout_seconds = MEMBER_STATUS_TIMEOUT.as_secs(),
                        "federation smoke melt quote status timed out"
                    );
                }
            }
            if visible >= required {
                return Ok(());
            }
        }

        tokio::time::sleep(sleep).await;
    }

    bail!(
        "federation smoke melt quote {} did not reach {} visible member observations after {} attempts",
        quote_id,
        required,
        retries
    )
}

async fn wait_for_keyset_rotation_visible(
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    public_config: &FederationConfig,
    old_keyset_id: Id,
    next_keyset_id: Id,
    retries: u16,
    sleep: Duration,
    member_expectation: SmokeMemberExpectation,
) -> Result<()> {
    let required = required_member_observations(public_config, member_expectation);
    for _ in 0..retries {
        let mut visible = 0usize;
        for (member_id, connector) in member_connectors {
            let active_keys = match connector.get_mint_keys().await {
                Ok(keys) => keys,
                Err(err) => {
                    tracing::debug!(
                        member_id = %member_id,
                        "federation smoke active keyset view unavailable during rotation: {}",
                        err
                    );
                    continue;
                }
            };
            let keysets = match connector.get_mint_keysets().await {
                Ok(keysets) => keysets,
                Err(err) => {
                    tracing::debug!(
                        member_id = %member_id,
                        "federation smoke keyset list unavailable during rotation: {}",
                        err
                    );
                    continue;
                }
            };

            let active_view_rotated = active_keys.iter().any(|keyset| keyset.id == next_keyset_id)
                && !active_keys.iter().any(|keyset| keyset.id == old_keyset_id);
            let old_retired = keysets
                .keysets
                .iter()
                .any(|keyset| keyset.id == old_keyset_id && !keyset.active);
            let next_active = keysets
                .keysets
                .iter()
                .any(|keyset| keyset.id == next_keyset_id && keyset.active);
            if active_view_rotated && old_retired && next_active {
                visible = visible.saturating_add(1);
            } else {
                tracing::debug!(
                    member_id = %member_id,
                    %old_keyset_id,
                    %next_keyset_id,
                    active_view_rotated,
                    old_retired,
                    next_active,
                    "federation smoke keyset rotation is not visible yet"
                );
            }
        }
        if visible >= required {
            return Ok(());
        }
        tokio::time::sleep(sleep).await;
    }

    bail!(
        "federation smoke keyset rotation to {} did not reach {} member views after {} attempts",
        next_keyset_id,
        required,
        retries
    )
}

fn required_member_observations(
    public_config: &FederationConfig,
    member_expectation: SmokeMemberExpectation,
) -> usize {
    match member_expectation {
        SmokeMemberExpectation::AllMembers => public_config.members.len(),
        SmokeMemberExpectation::ThresholdMembers => {
            usize::from(public_config.threshold_params.signature_threshold)
        }
    }
}

async fn wait_for_melt_quote_state(
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    public_config: &FederationConfig,
    quote_id: &str,
    expected_state: MeltQuoteState,
    retries: u16,
    sleep: Duration,
    member_expectation: SmokeMemberExpectation,
) -> Result<()> {
    let required = required_member_observations(public_config, member_expectation);
    let quote_id = quote_id.to_string();
    for _ in 0..retries {
        let mut expected = 0usize;
        for (member_id, connector) in member_connectors {
            match tokio::time::timeout(
                MEMBER_STATUS_TIMEOUT,
                connector.get_melt_quote_status(PaymentMethod::BOLT11, &quote_id),
            )
            .await
            {
                Ok(Ok(status)) if status.state() == expected_state => {
                    expected = expected.saturating_add(1);
                }
                Ok(Ok(status)) => {
                    tracing::debug!(
                        member_id = %member_id,
                        state = ?status.state(),
                        expected_state = %expected_state,
                        "federation smoke melt quote is not in expected state"
                    );
                }
                Ok(Err(err)) => {
                    tracing::debug!(
                        member_id = %member_id,
                        "federation smoke melt quote status unavailable: {}",
                        err
                    );
                }
                Err(_) => {
                    tracing::debug!(
                        member_id = %member_id,
                        timeout_seconds = MEMBER_STATUS_TIMEOUT.as_secs(),
                        "federation smoke melt quote status timed out"
                    );
                }
            }
            if expected >= required {
                return Ok(());
            }
        }

        tokio::time::sleep(sleep).await;
    }

    bail!(
        "federation smoke melt quote {} did not reach {} member observations in state {} after {} attempts",
        quote_id,
        required,
        expected_state,
        retries
    )
}

#[cfg(test)]
mod tests;
