use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::cdk_database::{self, WalletDatabase};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MeltQuoteState, MintInfo, PaymentMethod};
use cdk::wallet::types::SendKind;
use cdk::wallet::{
    FederatedMintConnector, FederatedMintConnectorConfig, HttpTransport, ReceiveOptions,
    SendOptions, Wallet, WalletBuilder,
};
use cdk::{Amount, StreamExt};
use cdk_common::federation::{FederationConfig, FederationInvite};
use cdk_http_client::Async;
use cdk_iroh::IrohTransport;
use cdk_sqlite::WalletSqliteDatabase;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::app::{AppEvent, PendingQuote, QuoteKind};

const CONFIG_VERSION: u16 = 1;
const CONFIG_FILE: &str = "federation.json";
const SEED_FILE: &str = "seed";
const DATABASE_FILE: &str = "wallet.sqlite";

pub type WalletTransport = IrohTransport<Async>;

#[derive(Debug, Clone)]
pub enum JoinSource {
    ConfigPath(PathBuf),
    Invite(String),
    MintUrl(MintUrl),
}

impl JoinSource {
    pub fn parse(value: String) -> Self {
        let value = value.trim();
        if value.starts_with("cashufed") {
            return Self::Invite(value.to_string());
        }
        match MintUrl::from_str(value) {
            Ok(mint_url) => Self::MintUrl(mint_url),
            Err(_) => Self::ConfigPath(PathBuf::from(value)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredFederation {
    version: u16,
    local_mint_url: MintUrl,
    public_config: FederationConfig,
}

#[derive(Debug, Clone)]
pub struct WalletRuntime {
    wallet: Wallet,
    stored: StoredFederation,
    payment_capabilities: PaymentCapabilities,
    data_dir: PathBuf,
    transport: WalletTransport,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PaymentCapabilities {
    pub mint_bolt11: bool,
    pub mint_onchain: bool,
    pub melt_bolt11: bool,
    pub melt_onchain: bool,
}

impl PaymentCapabilities {
    fn from_mint_info(mint_info: &MintInfo) -> Self {
        let supports_mint = |method: &PaymentMethod| {
            !mint_info.nuts.nut04.disabled
                && mint_info.nuts.nut04.methods.iter().any(|settings| {
                    &settings.method == method && settings.unit == CurrencyUnit::Sat
                })
        };
        let supports_melt = |method: &PaymentMethod| {
            !mint_info.nuts.nut05.disabled
                && mint_info.nuts.nut05.methods.iter().any(|settings| {
                    &settings.method == method && settings.unit == CurrencyUnit::Sat
                })
        };
        let onchain = PaymentMethod::Known(KnownMethod::Onchain);
        Self {
            mint_bolt11: supports_mint(&PaymentMethod::BOLT11),
            mint_onchain: supports_mint(&onchain),
            melt_bolt11: supports_melt(&PaymentMethod::BOLT11),
            melt_onchain: supports_melt(&onchain),
        }
    }
}

impl WalletRuntime {
    pub async fn load(data_dir: &Path) -> Result<Option<Self>> {
        let config_path = data_dir.join(CONFIG_FILE);
        if !config_path.exists() {
            return Ok(None);
        }

        let encoded = fs::read_to_string(&config_path)
            .with_context(|| format!("could not read {}", config_path.display()))?;
        let stored: StoredFederation = serde_json::from_str(&encoded)
            .with_context(|| format!("could not parse {}", config_path.display()))?;
        stored.validate()?;
        Self::build(data_dir, stored, WalletTransport::default())
            .await
            .map(Some)
    }

    pub async fn join(data_dir: &Path, source: JoinSource) -> Result<Self> {
        fs::create_dir_all(data_dir)?;
        secure_directory(data_dir)?;
        let transport = WalletTransport::default();

        let (public_config, discovery_mint_url) = match source {
            JoinSource::ConfigPath(path) => {
                let encoded = fs::read_to_string(&path)
                    .with_context(|| format!("could not read {}", path.display()))?;
                let public_config = serde_json::from_str::<FederationConfig>(&encoded)
                    .with_context(|| format!("could not parse {}", path.display()))?;
                (public_config, None)
            }
            JoinSource::Invite(invite) => (FederationInvite::from_str(&invite)?.config, None),
            JoinSource::MintUrl(mint_url) => {
                let public_config = discover_public_config(&mint_url, &transport).await?;
                (public_config, Some(mint_url))
            }
        };

        public_config.validate()?;
        public_config.validate_wallet_runtime_metadata()?;
        reject_conflicting_stored_federation(data_dir, &public_config)?;
        let local_mint_url = match discovery_mint_url {
            Some(mint_url) => mint_url,
            None => public_config
                .members
                .values()
                .next()
                .map(|member| member.public_mint_url.clone())
                .ok_or_else(|| anyhow!("federation roster is empty"))?,
        };
        let stored = StoredFederation {
            version: CONFIG_VERSION,
            local_mint_url,
            public_config,
        };
        stored.validate()?;

        let runtime = Self::build(data_dir, stored.clone(), transport).await?;
        persist_config(data_dir, &stored)?;
        Ok(runtime)
    }

    async fn build(
        data_dir: &Path,
        stored: StoredFederation,
        transport: WalletTransport,
    ) -> Result<Self> {
        let seed = load_or_create_seed(data_dir)?;
        let database = WalletSqliteDatabase::new(data_dir.join(DATABASE_FILE)).await?;
        let localstore: Arc<dyn WalletDatabase<cdk_database::Error> + Send + Sync> =
            Arc::new(database);

        let quote_coordinator_member_id = stored
            .public_config
            .members
            .iter()
            .find(|(_, member)| member.public_mint_url == stored.local_mint_url)
            .map(|(member_id, _)| *member_id);
        let connector_config = FederatedMintConnectorConfig {
            quote_coordinator_member_id,
            member_fallback_timeout: Duration::from_secs(90),
            member_request_timeout: Duration::from_secs(90),
            quote_status_member_timeout: Duration::from_secs(90),
            fanout_concurrency: stored.public_config.members.len(),
            mint_quote_visibility_attempts: 120,
            mint_quote_visibility_retry_delay: Duration::from_millis(100),
            ..FederatedMintConnectorConfig::default()
        };
        let connector =
            FederatedMintConnector::from_validated_public_config_urls_with_transport_and_config(
                stored.public_config.clone(),
                transport.clone(),
                connector_config,
            )?;
        let wallet = WalletBuilder::new()
            .mint_url(stored.local_mint_url.clone())
            .unit(CurrencyUnit::Sat)
            .localstore(localstore)
            .seed(seed)
            .shared_client(Arc::new(connector))
            .build()?;

        wallet.recover_incomplete_sagas().await?;
        let mint_info = wallet.load_mint_info().await?;
        let payment_capabilities = PaymentCapabilities::from_mint_info(&mint_info);
        Ok(Self {
            wallet,
            stored,
            payment_capabilities,
            data_dir: data_dir.to_path_buf(),
            transport,
        })
    }

    pub async fn switch_primary(&self, new_local_mint_url: MintUrl) -> Result<Self> {
        let stored = self.stored.with_primary(new_local_mint_url)?;
        let runtime = Self::build(&self.data_dir, stored.clone(), self.transport.clone()).await?;
        persist_config(&self.data_dir, &stored)?;
        Ok(runtime)
    }

    pub fn public_config(&self) -> FederationConfig {
        self.stored.public_config.clone()
    }

    pub fn federation_id(&self) -> String {
        self.stored.public_config.federation_id.to_string()
    }

    pub fn local_mint_url(&self) -> &MintUrl {
        &self.stored.local_mint_url
    }

    pub fn wallet(&self) -> Wallet {
        self.wallet.clone()
    }

    pub fn payment_capabilities(&self) -> PaymentCapabilities {
        self.payment_capabilities
    }

    pub fn transport(&self) -> WalletTransport {
        self.transport.clone()
    }
}

async fn discover_public_config(
    mint_url: &MintUrl,
    transport: &WalletTransport,
) -> Result<FederationConfig> {
    let config_url = mint_url.join_paths(&["federation", "v1", "config"])?;
    let public_config: FederationConfig = fetch_json(transport, config_url).await?;
    let info_url = mint_url.join_paths(&["v1", "info"])?;
    let mint_info: MintInfo = fetch_json(transport, info_url).await?;
    validate_discovered_public_config(mint_url, public_config, &mint_info)
}

async fn fetch_json<T>(transport: &WalletTransport, url: url::Url) -> Result<T>
where
    T: DeserializeOwned,
{
    let response = transport
        .http_get_raw(url.clone(), None)
        .await
        .with_context(|| format!("could not fetch {url}"))?;
    let status = response.status();
    if !response.is_success() {
        bail!("GET {url} returned HTTP {status}");
    }
    response
        .json()
        .await
        .with_context(|| format!("could not decode JSON from {url}"))
}

fn validate_discovered_public_config(
    mint_url: &MintUrl,
    public_config: FederationConfig,
    mint_info: &MintInfo,
) -> Result<FederationConfig> {
    public_config.validate()?;
    if !public_config.keysets.iter().any(|keyset| keyset.active) {
        bail!("federation public config does not contain an active keyset");
    }

    let mut matching_members = public_config
        .members
        .iter()
        .filter(|(_, member)| &member.public_mint_url == mint_url);
    let Some((member_id, _)) = matching_members.next() else {
        bail!("federation public config does not contain discovery mint URL {mint_url}");
    };
    if matching_members.next().is_some() {
        bail!("federation public config contains discovery mint URL {mint_url} more than once");
    }

    let federation_info = mint_info.federation.as_ref().ok_or_else(|| {
        anyhow!("discovery member mint info does not advertise federation metadata")
    })?;
    let config_digest = public_config.config_digest_for_validated_config()?;
    public_config.validate_member_mint_info_with_config_digest(
        *member_id,
        federation_info,
        config_digest,
    )?;
    Ok(public_config)
}

impl StoredFederation {
    fn validate(&self) -> Result<()> {
        if self.version != CONFIG_VERSION {
            bail!(
                "unsupported federation wallet config version {}",
                self.version
            );
        }
        self.public_config.validate()?;
        self.public_config.validate_wallet_runtime_metadata()?;
        if !self
            .public_config
            .members
            .values()
            .any(|member| member.public_mint_url == self.local_mint_url)
        {
            bail!("local mint URL is not in the federation roster");
        }
        Ok(())
    }

    fn with_primary(&self, new_local_mint_url: MintUrl) -> Result<Self> {
        if !self
            .public_config
            .members
            .values()
            .any(|member| member.public_mint_url == new_local_mint_url)
        {
            bail!("mint URL {new_local_mint_url} is not in the federation roster");
        }
        let stored = Self {
            local_mint_url: new_local_mint_url,
            ..self.clone()
        };
        stored.validate()?;
        Ok(stored)
    }
}

fn reject_conflicting_stored_federation(
    data_dir: &Path,
    public_config: &FederationConfig,
) -> Result<()> {
    let config_path = data_dir.join(CONFIG_FILE);
    if !config_path.exists() {
        return Ok(());
    }
    let encoded = fs::read_to_string(&config_path)
        .with_context(|| format!("could not read {}", config_path.display()))?;
    let existing: StoredFederation = serde_json::from_str(&encoded)
        .with_context(|| format!("could not parse {}", config_path.display()))?;
    if existing.public_config.federation_id != public_config.federation_id {
        bail!(
            "wallet at {} already belongs to federation {}; refusing to join federation {} over it. Use --data-dir for a separate wallet or `reset` to delete this one",
            data_dir.display(),
            existing.public_config.federation_id,
            public_config.federation_id
        );
    }
    Ok(())
}

fn persist_config(data_dir: &Path, stored: &StoredFederation) -> Result<()> {
    let path = data_dir.join(CONFIG_FILE);
    let temporary = data_dir.join(format!("{CONFIG_FILE}.tmp"));
    let encoded = serde_json::to_vec_pretty(stored)?;
    write_private_file(&temporary, &encoded)?;
    fs::rename(&temporary, &path)?;
    Ok(())
}

fn load_or_create_seed(data_dir: &Path) -> Result<[u8; 64]> {
    let path = data_dir.join(SEED_FILE);
    let mnemonic = if path.exists() {
        let encoded = fs::read_to_string(&path)?;
        Mnemonic::from_str(encoded.trim())?
    } else {
        let entropy: [u8; 32] = rand::random();
        let mnemonic = Mnemonic::from_entropy(&entropy)?;
        write_private_file(&path, mnemonic.to_string().as_bytes())?;
        mnemonic
    };
    Ok(mnemonic.to_seed_normalized(""))
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn secure_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn spawn_balance_refresh(
    runtime: Arc<WalletRuntime>,
    tx: mpsc::UnboundedSender<AppEvent>,
    generation: u64,
    current_generation: Arc<AtomicU64>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(4));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if current_generation.load(Ordering::Relaxed) != generation {
                return;
            }
            if !send_wallet_snapshot(&runtime, &tx).await {
                return;
            }
        }
    });
}

async fn send_wallet_snapshot(
    runtime: &WalletRuntime,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> bool {
    let wallet = runtime.wallet();
    match tokio::try_join!(
        wallet.total_balance(),
        wallet.total_pending_balance(),
        wallet.total_reserved_balance()
    ) {
        Ok((available, pending, reserved)) => {
            if tx
                .send(AppEvent::Balance {
                    available: u64::from(available),
                    pending: u64::from(pending),
                    reserved: u64::from(reserved),
                })
                .is_err()
            {
                return false;
            }
        }
        Err(err) => {
            if tx
                .send(AppEvent::BackgroundError(format!(
                    "balance refresh failed: {err}"
                )))
                .is_err()
            {
                return false;
            }
        }
    }
    match wallet.list_transactions(None).await {
        Ok(transactions) => {
            if tx.send(AppEvent::Transactions(transactions)).is_err() {
                return false;
            }
        }
        Err(err) => {
            if tx
                .send(AppEvent::BackgroundError(format!(
                    "transaction refresh failed: {err}"
                )))
                .is_err()
            {
                return false;
            }
        }
    }
    tx.send(AppEvent::Quotes(fetch_pending_quotes(&wallet).await))
        .is_ok()
}

async fn fetch_pending_quotes(wallet: &cdk::wallet::Wallet) -> Vec<PendingQuote> {
    let mut quotes = Vec::new();
    if let Ok(mint_quotes) = wallet.get_unissued_mint_quotes().await {
        quotes.extend(mint_quotes.into_iter().map(|quote| {
            let amount = quote
                .amount
                .map(u64::from)
                .unwrap_or_else(|| u64::from(quote.amount_paid));
            PendingQuote {
                id: quote.id,
                kind: QuoteKind::Mint,
                state: quote.state.to_string(),
                in_flight: quote.used_by_operation.is_some(),
                method: quote.payment_method.to_string(),
                amount,
                fee_reserve: 0,
                request: quote.request,
                expiry: quote.expiry,
                updated_at: quote.updated_at,
            }
        }));
    }
    if let Ok(melt_quotes) = wallet.get_active_melt_quotes().await {
        quotes.extend(melt_quotes.into_iter().map(|quote| PendingQuote {
            id: quote.id,
            kind: QuoteKind::Melt,
            state: quote.state.to_string(),
            in_flight: quote.used_by_operation.is_some(),
            method: quote.payment_method.to_string(),
            amount: u64::from(quote.amount),
            fee_reserve: u64::from(quote.fee_reserve),
            request: quote.request,
            expiry: quote.expiry,
            updated_at: 0,
        }));
    }
    quotes
}

pub fn spawn_quote_action(
    runtime: Arc<WalletRuntime>,
    kind: QuoteKind,
    quote_id: String,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let member_url = runtime.local_mint_url().to_string();
        let result = async {
            let wallet = runtime.wallet();
            match kind {
                QuoteKind::Mint => {
                    let quote = wallet
                        .check_mint_quote_status(&quote_id)
                        .await
                        .map_err(|err| {
                            anyhow!("could not check mint quote {}: {err}", short_id(&quote_id))
                        })?;
                    if quote.amount_mintable() > Amount::ZERO {
                        if quote.used_by_operation.is_some() {
                            bail!(
                                "mint quote {} is reserved by an in-flight operation",
                                short_id(&quote_id)
                            );
                        }
                        let proofs = wallet
                            .mint(&quote_id, SplitTarget::default(), None)
                            .await
                            .map_err(|err| {
                                anyhow!("mint failed for quote {}: {err}", short_id(&quote_id))
                            })?;
                        let minted = proofs.total_amount()?;
                        Ok(format!(
                            "minted {minted} sat from quote {} via {member_url}",
                            short_id(&quote_id)
                        ))
                    } else {
                        Ok(format!(
                            "mint quote {} is {} (paid {} sat, issued {} sat)",
                            short_id(&quote_id),
                            quote.state,
                            quote.amount_paid,
                            quote.amount_issued
                        ))
                    }
                }
                QuoteKind::Melt => {
                    let quote = wallet
                        .check_melt_quote_status(&quote_id)
                        .await
                        .map_err(|err| {
                            anyhow!("could not check melt quote {}: {err}", short_id(&quote_id))
                        })?;
                    match quote.state {
                        MeltQuoteState::Unpaid => {
                            if quote.used_by_operation.is_some() {
                                bail!(
                                    "melt quote {} is reserved by an in-flight operation",
                                    short_id(&quote_id)
                                );
                            }
                            let outcome = wallet
                                .prepare_melt(&quote_id, HashMap::new())
                                .await?
                                .confirm()
                                .await?;
                            Ok(format!(
                                "Paid {} sat with {} sat fee ({}) via {member_url}",
                                outcome.amount(),
                                outcome.fee_paid(),
                                outcome.state()
                            ))
                        }
                        MeltQuoteState::Paid => Ok(format!(
                            "melt quote {} already paid (preimage {})",
                            short_id(&quote_id),
                            quote
                                .payment_proof
                                .as_deref()
                                .map(short_request)
                                .unwrap_or_else(|| "not recorded".to_string())
                        )),
                        state => Ok(format!("melt quote {} is {state}", short_id(&quote_id))),
                    }
                }
            }
        }
        .await;
        finish_operation(kind.operation(), result, runtime, tx).await;
    });
}

pub fn spawn_onchain_mint(
    runtime: Arc<WalletRuntime>,
    amount: u64,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let member_url = runtime.local_mint_url().to_string();
        let result = async {
            let wallet = runtime.wallet();
            tx.send(AppEvent::OperationProgress {
                operation: "mint-onchain",
                message: format!(
                    "POST /v1/mint/quote/onchain amount={amount} via {member_url} (quote coordinator; may fall back)"
                ),
                member_url: Some(member_url.clone()),
            })
            .ok();
            let quote = wallet
                .mint_quote(
                    PaymentMethod::Known(KnownMethod::Onchain),
                    Some(Amount::from(amount)),
                    None,
                    None,
                )
                .await?;
            tx.send(AppEvent::OperationPayload {
                operation: "mint-onchain",
                title: "Fund this on-chain address".to_string(),
                content: quote.request.clone(),
                message: format!(
                    "mint_quote ok id={} amount={} request={} via {member_url}; waiting for confirmation then mint",
                    short_id(&quote.id),
                    amount,
                    short_request(&quote.request)
                ),
                member_url: Some(member_url.clone()),
            })
            .ok();

            let quote_id = quote.id.clone();
            tx.send(AppEvent::OperationProgress {
                operation: "mint-onchain",
                message: format!(
                    "POST /v1/mint/onchain poll start quote={} via {member_url}",
                    short_id(&quote_id)
                ),
                member_url: Some(member_url.clone()),
            })
            .ok();
            let mut stream = wallet.proof_stream(quote, SplitTarget::default(), None);
            let mut minted = Amount::ZERO;
            loop {
                match stream.next().await {
                    Some(Ok(proofs)) => {
                        let batch = proofs.total_amount()?;
                        minted += batch;
                        tx.send(AppEvent::OperationProgress {
                            operation: "mint-onchain",
                            message: format!(
                                "mint responses aggregated {} sat from quote {} via {member_url}",
                                batch,
                                short_id(&quote_id)
                            ),
                            member_url: Some(member_url.clone()),
                        })
                        .ok();
                    }
                    Some(Err(err)) => return Err(anyhow!(err)),
                    None => break,
                }
            }
            if minted == Amount::ZERO {
                bail!("mint completed without issuing proofs");
            }
            Ok(format!(
                "mint complete:{minted} sat quote={} via {member_url}",
                short_id(&quote_id)
            ))
        }
        .await;
        finish_operation("mint-onchain", result, runtime, tx).await;
    });
}

pub fn spawn_bolt11_mint(
    runtime: Arc<WalletRuntime>,
    amount: u64,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let member_url = runtime.local_mint_url().to_string();
        let result = async {
            let wallet = runtime.wallet();
            tx.send(AppEvent::OperationProgress {
                operation: "mint-bolt11",
                message: format!(
                    "POST /v1/mint/quote/bolt11 amount={amount} via {member_url} (quote coordinator; may fall back)"
                ),
                member_url: Some(member_url.clone()),
            })
            .ok();
            let quote = wallet
                .mint_quote(
                    PaymentMethod::BOLT11,
                    Some(Amount::from(amount)),
                    None,
                    None,
                )
                .await?;
            tx.send(AppEvent::OperationPayload {
                operation: "mint-bolt11",
                title: "Pay mint invoice".to_string(),
                content: quote.request.clone(),
                message: format!(
                    "mint_quote ok id={} amount={} invoice={} via {member_url}; waiting for payment then mint",
                    short_id(&quote.id),
                    amount,
                    short_request(&quote.request)
                ),
                member_url: Some(member_url.clone()),
            })
            .ok();

            let quote_id = quote.id.clone();
            tx.send(AppEvent::OperationProgress {
                operation: "mint-bolt11",
                message: format!(
                    "POST /v1/mint/bolt11 poll start quote={} via {member_url}",
                    short_id(&quote_id)
                ),
                member_url: Some(member_url.clone()),
            })
            .ok();
            let mut stream = wallet.proof_stream(quote, SplitTarget::default(), None);
            let mut minted = Amount::ZERO;
            loop {
                match tokio::time::timeout(Duration::from_secs(180), stream.next()).await {
                    Ok(Some(Ok(proofs))) => {
                        let batch = proofs.total_amount()?;
                        minted += batch;
                        tx.send(AppEvent::OperationProgress {
                            operation: "mint-bolt11",
                            message: format!(
                                "mint responses aggregated {} sat from quote {} via {member_url}",
                                batch,
                                short_id(&quote_id)
                            ),
                            member_url: Some(member_url.clone()),
                        })
                        .ok();
                    }
                    Ok(Some(Err(err))) => return Err(anyhow!(err)),
                    Ok(None) => break,
                    Err(_) => bail!("timed out waiting for mint quote payment"),
                }
            }
            if minted == Amount::ZERO {
                bail!("mint completed without issuing proofs");
            }
            Ok(format!(
                "mint complete:{minted} sat quote={} via {member_url}",
                short_id(&quote_id)
            ))
        }
        .await;
        finish_operation("mint-bolt11", result, runtime, tx).await;
    });
}

pub fn spawn_send(runtime: Arc<WalletRuntime>, amount: u64, tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let member_url = runtime.local_mint_url().to_string();
        let result = async {
            let wallet = runtime.wallet();
            let options = SendOptions {
                send_kind: SendKind::OnlineExact,
                ..SendOptions::default()
            };
            let prepared = wallet.prepare_send(Amount::from(amount), options).await?;
            let token = prepared.confirm(None).await?.to_string();
            tx.send(AppEvent::OperationPayload {
                operation: "send",
                title: format!("{amount} sat Cashu token"),
                content: token,
                message: format!(
                    "Federated swap complete; copy the token for the receiver via {member_url}"
                ),
                member_url: Some(member_url.clone()),
            })
            .ok();
            Ok(format!("Prepared {amount} sat token via {member_url}"))
        }
        .await;
        finish_operation("send", result, runtime, tx).await;
    });
}

pub fn spawn_receive(
    runtime: Arc<WalletRuntime>,
    token: String,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let result = runtime
            .wallet()
            .receive(&token, ReceiveOptions::default())
            .await
            .map(|amount| format!("Received {amount} sat"))
            .map_err(anyhow::Error::from);
        finish_operation("receive", result, runtime, tx).await;
    });
}

pub fn spawn_onchain_melt(
    runtime: Arc<WalletRuntime>,
    address: String,
    amount: u64,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let member_url = runtime.local_mint_url().to_string();
        let result = async {
            let wallet = runtime.wallet();
            let quotes = wallet
                .quote_onchain_melt_options(&address, Amount::from(amount), None)
                .await?;
            let selected = quotes
                .into_iter()
                .min_by_key(|quote| quote.fee_reserve)
                .ok_or_else(|| anyhow!("the federation returned no on-chain fee options"))?;
            let quote = wallet.select_onchain_melt_quote(selected).await?;
            tx.send(AppEvent::OperationProgress {
                operation: "melt-onchain",
                message: format!(
                    "melt_quote ok id={}: {} sat + {} sat reserve, {} block estimate via {member_url}",
                    short_id(&quote.id),
                    quote.amount,
                    quote.fee_reserve,
                    quote.estimated_blocks.unwrap_or_default()
                ),
                member_url: Some(member_url.clone()),
            })
            .ok();
            let outcome = wallet
                .prepare_melt(&quote.id, HashMap::new())
                .await?
                .confirm()
                .await?;
            Ok(format!(
                "Paid {} sat with {} sat fee ({}) via {member_url}",
                outcome.amount(),
                outcome.fee_paid(),
                outcome.state()
            ))
        }
        .await;
        finish_operation("melt-onchain", result, runtime, tx).await;
    });
}

pub fn spawn_bolt11_melt(
    runtime: Arc<WalletRuntime>,
    invoice: String,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let member_url = runtime.local_mint_url().to_string();
        let result = async {
            cdk::Bolt11Invoice::from_str(&invoice)
                .map_err(|err| anyhow!("invalid BOLT11 invoice: {err}"))?;
            let wallet = runtime.wallet();
            let quote = wallet
                .melt_quote(PaymentMethod::BOLT11, invoice, None, None)
                .await?;
            tx.send(AppEvent::OperationProgress {
                operation: "melt-bolt11",
                message: format!(
                    "melt_quote ok id={}: {} sat + {} sat reserve via {member_url}",
                    short_id(&quote.id),
                    quote.amount,
                    quote.fee_reserve
                ),
                member_url: Some(member_url.clone()),
            })
            .ok();
            let outcome = wallet
                .prepare_melt(&quote.id, HashMap::new())
                .await?
                .confirm()
                .await?;
            Ok(format!(
                "Paid {} sat with {} sat fee ({}) via {member_url}",
                outcome.amount(),
                outcome.fee_paid(),
                outcome.state()
            ))
        }
        .await;
        finish_operation("melt-bolt11", result, runtime, tx).await;
    });
}

async fn finish_operation(
    operation: &'static str,
    result: Result<String>,
    runtime: Arc<WalletRuntime>,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    let member_url = Some(runtime.local_mint_url().to_string());
    match result {
        Ok(message) => {
            tx.send(AppEvent::OperationFinished {
                operation,
                message,
                member_url,
            })
            .ok();
        }
        Err(err) => {
            tx.send(AppEvent::OperationFailed {
                operation,
                error: err.to_string(),
                member_url,
            })
            .ok();
        }
    }

    send_wallet_snapshot(&runtime, &tx).await;
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn short_request(request: &str) -> String {
    if request.chars().count() <= 28 {
        return request.to_string();
    }
    let prefix: String = request.chars().take(14).collect();
    let suffix: String = request
        .chars()
        .rev()
        .take(10)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}…{suffix}")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cdk::nuts::nut01::BlsSecretKey;
    use cdk::nuts::nut04::{MintMethodOptions, MintMethodSettings};
    use cdk::nuts::nut05::{MeltMethodOptions, MeltMethodSettings};
    use cdk::Amount;
    use cdk_common::federation::{
        generate_trusted_dealer_configs, FederationConfigDigest, MemberId, ThresholdParams,
        TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    use cdk_common::SecretKey;

    use super::*;

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("valid member id")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("valid secp256k1 secret")
    }

    #[test]
    fn payment_capabilities_follow_advertised_methods() {
        let onchain = PaymentMethod::Known(KnownMethod::Onchain);
        let mut mint_info = MintInfo::new();
        mint_info.nuts.nut04 = cdk::nuts::nut04::Settings::new(
            vec![MintMethodSettings {
                method: onchain.clone(),
                unit: CurrencyUnit::Sat,
                method_name: Some("Onchain".to_string()),
                min_amount: Some(Amount::from(1)),
                max_amount: Some(Amount::from(50_000_000)),
                options: Some(MintMethodOptions::Onchain { confirmations: 1 }),
            }],
            false,
        );
        mint_info.nuts.nut05 = cdk::nuts::nut05::Settings {
            methods: vec![MeltMethodSettings {
                method: onchain,
                unit: CurrencyUnit::Sat,
                method_name: Some("Onchain".to_string()),
                min_amount: Some(Amount::from(1)),
                max_amount: Some(Amount::from(50_000_000)),
                options: None::<MeltMethodOptions>,
            }],
            disabled: false,
        };

        let methods = PaymentCapabilities::from_mint_info(&mint_info);
        assert!(methods.mint_onchain);
        assert!(methods.melt_onchain);
        assert!(!methods.mint_bolt11);
        assert!(!methods.melt_bolt11);
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

    fn valid_public_config() -> FederationConfig {
        valid_public_config_seeded(0)
    }

    fn valid_public_config_seeded(seed: u8) -> FederationConfig {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("threshold params");
        generate_trusted_dealer_configs(
            params,
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
                    (
                        Amount::from(1),
                        vec![bls_key(seed.wrapping_add(1)), bls_key(seed.wrapping_add(2))],
                    ),
                    (
                        Amount::from(2),
                        vec![bls_key(seed.wrapping_add(3)), bls_key(seed.wrapping_add(4))],
                    ),
                ]),
            )],
        )
        .expect("trusted dealer config")
        .public_config
    }

    fn distinct_public_config() -> FederationConfig {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("threshold params");
        let distinct_member = |id: u16| {
            TrustedDealerMember::new(
                member_id(id),
                MintUrl::from_str(&format!("https://mint{}.other-example.com", id))
                    .expect("public mint url"),
                MintUrl::from_str(&format!("https://fed{}.other-example.com", id))
                    .expect("federation api url"),
                identity_secret(member_id(id)),
            )
        };
        generate_trusted_dealer_configs(
            params,
            vec![distinct_member(1), distinct_member(2), distinct_member(3)],
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([
                    (Amount::from(1), vec![bls_key(1), bls_key(2)]),
                    (Amount::from(2), vec![bls_key(3), bls_key(4)]),
                ]),
            )],
        )
        .expect("trusted dealer config")
        .public_config
    }

    #[test]
    fn join_source_detects_invites_and_paths() {
        assert!(matches!(
            JoinSource::parse(" cashufed1example ".to_string()),
            JoinSource::Invite(value) if value == "cashufed1example"
        ));
        assert!(matches!(
            JoinSource::parse(" federation-public.json ".to_string()),
            JoinSource::ConfigPath(path) if path.as_path() == Path::new("federation-public.json")
        ));
        assert!(matches!(
            JoinSource::parse(" https://mint.example.com ".to_string()),
            JoinSource::MintUrl(url) if url.to_string() == "https://mint.example.com"
        ));
    }

    #[test]
    fn discovered_public_config_accepts_matching_source_member() {
        let public_config = valid_public_config();
        let source_url = public_config
            .member(member_id(2))
            .expect("source member")
            .public_mint_url
            .clone();
        let mint_info = MintInfo::new().federation(
            public_config
                .mint_info_for_member(member_id(2))
                .expect("member federation info"),
        );

        let validated =
            validate_discovered_public_config(&source_url, public_config.clone(), &mint_info)
                .expect("matching discovery metadata");

        assert_eq!(validated, public_config);
    }

    #[test]
    fn discovered_public_config_rejects_unlisted_source_url() {
        let public_config = valid_public_config();
        let source_url = MintUrl::from_str("https://unknown.example.com").expect("source URL");
        let mint_info = MintInfo::new().federation(
            public_config
                .mint_info_for_member(member_id(1))
                .expect("member federation info"),
        );

        let err = validate_discovered_public_config(&source_url, public_config, &mint_info)
            .expect_err("unlisted source rejected");

        assert!(err
            .to_string()
            .contains("does not contain discovery mint URL"));
    }

    #[test]
    fn discovered_public_config_rejects_mismatched_mint_info_digest() {
        let public_config = valid_public_config();
        let source_url = public_config
            .member(member_id(1))
            .expect("source member")
            .public_mint_url
            .clone();
        let mut federation_info = public_config
            .mint_info_for_member(member_id(1))
            .expect("member federation info");
        federation_info.config_digest = FederationConfigDigest::from_bytes([42; 32]).to_string();
        let mint_info = MintInfo::new().federation(federation_info);

        let err = validate_discovered_public_config(&source_url, public_config, &mint_info)
            .expect_err("mismatched digest rejected");

        assert!(err.to_string().contains("config digest"));
    }

    #[test]
    fn join_rejects_a_different_federation_over_an_existing_wallet() {
        let data_dir =
            std::env::temp_dir().join(format!("fed-cdk-tui-test-{}", rand::random::<u64>()));
        fs::create_dir_all(&data_dir).expect("create test data dir");
        let stored = StoredFederation {
            version: CONFIG_VERSION,
            local_mint_url: valid_public_config()
                .member(member_id(1))
                .expect("member")
                .public_mint_url
                .clone(),
            public_config: valid_public_config(),
        };
        persist_config(&data_dir, &stored).expect("persist stored federation");

        reject_conflicting_stored_federation(&data_dir, &stored.public_config)
            .expect("same federation is accepted");

        let other_config = distinct_public_config();
        assert_ne!(
            stored.public_config.federation_id, other_config.federation_id,
            "trusted dealer configs must have distinct federation ids"
        );
        let err = reject_conflicting_stored_federation(&data_dir, &other_config)
            .expect_err("different federation rejected");

        assert!(err.to_string().contains("already belongs to federation"));

        fs::remove_dir_all(&data_dir).expect("remove test data dir");
    }

    #[test]
    fn with_primary_accepts_roster_member_and_rejects_strangers() {
        let public_config = valid_public_config();
        let stored = StoredFederation {
            version: CONFIG_VERSION,
            local_mint_url: public_config
                .member(member_id(1))
                .expect("member")
                .public_mint_url
                .clone(),
            public_config: public_config.clone(),
        };

        let member_two_url = public_config
            .member(member_id(2))
            .expect("member")
            .public_mint_url
            .clone();
        let switched = stored
            .with_primary(member_two_url.clone())
            .expect("roster member accepted");
        assert_eq!(switched.local_mint_url, member_two_url);
        assert_eq!(switched.public_config, public_config);

        let stranger = MintUrl::from_str("https://stranger.example.com").expect("mint url");
        let err = stored
            .with_primary(stranger)
            .expect_err("non-roster member rejected");
        assert!(err.to_string().contains("not in the federation roster"));
    }
}
