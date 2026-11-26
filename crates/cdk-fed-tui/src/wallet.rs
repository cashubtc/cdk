//! Wallet runtime: one federation per data directory, kept in the wallet
//! database's federation registry.

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
use cdk::nuts::nut00::{KnownMethod, ProofsMethods};
use cdk::nuts::{CurrencyUnit, MeltQuoteState, MintInfo, PaymentMethod};
use cdk::wallet::types::SendKind;
use cdk::wallet::{
    FederationConnectorPolicy, FederationImportOptions, FederationRegistry, FederationSource,
    ReceiveOptions, RegisteredFederation, SendOptions, Wallet, WalletBuilder,
};
use cdk::{Amount, StreamExt};
use cdk_common::federation::FederationConfig;
use cdk_http_client::Async;
use cdk_iroh::IrohTransport;
use cdk_sqlite::WalletSqliteDatabase;
use tokio::sync::mpsc;

use crate::app::{AppEvent, PendingQuote, QuoteKind};

const SEED_FILE: &str = "seed";
const DATABASE_FILE: &str = "wallet.sqlite";

/// Transport shared by discovery, wallet requests, WebSockets and probes.
pub type WalletTransport = IrohTransport<Async>;

type LocalStore = Arc<dyn WalletDatabase<cdk_database::Error> + Send + Sync>;

/// What the user asked to join.
#[derive(Debug, Clone)]
pub enum JoinSource {
    /// Path to a `federation-public.json` file.
    ConfigPath(PathBuf),
    /// A member URL or an invite.
    Federation(FederationSource),
}

impl JoinSource {
    /// Interpret pasted text as an invite, a member URL, or a config path.
    pub fn parse(value: String) -> Self {
        match FederationSource::parse_invite_or_url(&value) {
            Some(source) => Self::Federation(source),
            None => Self::ConfigPath(PathBuf::from(value.trim())),
        }
    }

    fn into_federation_source(self) -> Result<FederationSource> {
        match self {
            Self::Federation(source) => Ok(source),
            Self::ConfigPath(path) => {
                let encoded = fs::read_to_string(&path)
                    .with_context(|| format!("could not read {}", path.display()))?;
                let public_config = serde_json::from_str::<FederationConfig>(&encoded)
                    .with_context(|| format!("could not parse {}", path.display()))?;
                Ok(FederationSource::PublicConfig(Box::new(public_config)))
            }
        }
    }
}

/// A joined federation with its wallet.
#[derive(Debug, Clone)]
pub struct WalletRuntime {
    wallet: Wallet,
    federation: RegisteredFederation,
    registry: FederationRegistry,
    localstore: LocalStore,
    payment_capabilities: PaymentCapabilities,
    data_dir: PathBuf,
    transport: WalletTransport,
}

/// Payment methods the federation advertises for sat.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaymentCapabilities {
    /// BOLT11 minting.
    pub mint_bolt11: bool,
    /// On-chain minting.
    pub mint_onchain: bool,
    /// BOLT11 melting.
    pub melt_bolt11: bool,
    /// On-chain melting.
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

/// Member-call policy for the dashboard: deployment-tested 90-second
/// timeouts so a slow consensus round does not cause quote fallback churn.
fn dashboard_policy(
    quote_coordinator: Option<cdk_common::federation::MemberId>,
) -> FederationConnectorPolicy {
    FederationConnectorPolicy {
        quote_coordinator_member_id: quote_coordinator,
        member_fallback_timeout_secs: 90,
        member_request_timeout_secs: 90,
        quote_status_member_timeout_secs: 90,
        quote_visibility_attempts: 120,
        quote_visibility_retry_delay_ms: 100,
        ..FederationConnectorPolicy::default()
    }
}

/// Import a federation into a data directory's registry.
///
/// A directory holds one federation: joining another one is refused so eCash
/// from two federations never shares a database. A first join makes the
/// joined member the primary quote coordinator.
async fn register_join<T>(
    registry: &FederationRegistry,
    source: FederationSource,
    transport: &T,
) -> Result<RegisteredFederation>
where
    T: cdk::wallet::HttpTransport,
{
    let before = registry.list().await?;
    let federation = registry
        .import_with_transport(source, FederationImportOptions::default(), transport)
        .await?;
    let federation_id = federation.federation_id();
    let was_registered = before
        .iter()
        .any(|existing| existing.federation_id() == federation_id);
    if let Some(other) = before
        .iter()
        .find(|existing| existing.federation_id() != federation_id)
    {
        if !was_registered {
            registry.remove(federation_id).await?;
        }
        bail!(
            "wallet already belongs to federation {}; refusing to join federation {} over it. Use --data-dir for a separate wallet or `reset` to delete this one",
            other.federation_id(),
            federation_id
        );
    }
    if was_registered {
        return Ok(federation);
    }
    let coordinator = federation.member_id_for_mint_url(federation.local_mint_url());
    Ok(registry
        .set_connector_policy(federation_id, dashboard_policy(coordinator))
        .await?)
}

/// Policy that makes `mint_url`'s member the primary quote coordinator.
fn primary_policy(
    federation: &RegisteredFederation,
    mint_url: &MintUrl,
) -> Result<FederationConnectorPolicy> {
    let member_id = federation
        .member_id_for_mint_url(mint_url)
        .ok_or_else(|| anyhow!("mint URL {mint_url} is not in the federation roster"))?;
    Ok(FederationConnectorPolicy {
        quote_coordinator_member_id: Some(member_id),
        ..*federation.connector_policy()
    })
}

/// Public mint URL of the member that coordinates quotes.
fn primary_member_url(federation: &RegisteredFederation) -> Option<MintUrl> {
    let members = &federation.public_config().members;
    match federation.connector_policy().quote_coordinator_member_id {
        Some(member_id) => members.get(&member_id),
        None => members.values().next(),
    }
    .map(|member| member.public_mint_url.clone())
}

impl WalletRuntime {
    /// Open the federation stored in `data_dir`, if any.
    pub async fn load(data_dir: &Path) -> Result<Option<Self>> {
        if !data_dir.join(DATABASE_FILE).exists() {
            return Ok(None);
        }
        let (registry, localstore) = open_registry(data_dir).await?;
        let mut federations = registry.list().await?;
        let federation = match federations.len() {
            0 => return Ok(None),
            1 => federations.remove(0),
            count => bail!(
                "wallet at {} holds {count} federations; expected one",
                data_dir.display()
            ),
        };
        Self::build(
            data_dir,
            registry,
            localstore,
            federation,
            WalletTransport::default(),
        )
        .await
        .map(Some)
    }

    /// Join a federation and open its wallet.
    pub async fn join(data_dir: &Path, source: JoinSource) -> Result<Self> {
        fs::create_dir_all(data_dir)?;
        secure_directory(data_dir)?;
        let transport = WalletTransport::default();
        let (registry, localstore) = open_registry(data_dir).await?;
        let federation =
            register_join(&registry, source.into_federation_source()?, &transport).await?;
        Self::build(data_dir, registry, localstore, federation, transport).await
    }

    async fn build(
        data_dir: &Path,
        registry: FederationRegistry,
        localstore: LocalStore,
        federation: RegisteredFederation,
        transport: WalletTransport,
    ) -> Result<Self> {
        let seed = load_or_create_seed(data_dir)?;
        let connector = federation.connector_with_transport(transport.clone())?;
        let wallet = WalletBuilder::new()
            .mint_url(federation.local_mint_url().clone())
            .unit(CurrencyUnit::Sat)
            .localstore(localstore.clone())
            .seed(seed)
            .shared_client(Arc::new(connector))
            .with_mint_url_aliases(federation.aliases())
            .build()?;

        wallet.recover_incomplete_sagas().await?;
        let mint_info = wallet.load_mint_info().await?;
        let payment_capabilities = PaymentCapabilities::from_mint_info(&mint_info);
        Ok(Self {
            wallet,
            federation,
            registry,
            localstore,
            payment_capabilities,
            data_dir: data_dir.to_path_buf(),
            transport,
        })
    }

    /// Make another member the primary quote coordinator. The wallet keeps
    /// its local mint URL, so its proofs and quotes stay where they are.
    pub async fn switch_primary(&self, mint_url: MintUrl) -> Result<Self> {
        let policy = primary_policy(&self.federation, &mint_url)?;
        let federation = self
            .registry
            .set_connector_policy(self.federation.federation_id(), policy)
            .await?;
        Self::build(
            &self.data_dir,
            self.registry.clone(),
            self.localstore.clone(),
            federation,
            self.transport.clone(),
        )
        .await
    }

    /// Validated public federation config.
    pub fn public_config(&self) -> FederationConfig {
        self.federation.public_config().clone()
    }

    /// Federation ID.
    pub fn federation_id(&self) -> String {
        self.federation.federation_id().to_string()
    }

    /// Public mint URL of the primary quote coordinator.
    pub fn primary_mint_url(&self) -> MintUrl {
        primary_member_url(&self.federation)
            .unwrap_or_else(|| self.federation.local_mint_url().clone())
    }

    /// The federated wallet.
    pub fn wallet(&self) -> Wallet {
        self.wallet.clone()
    }

    /// Advertised payment methods.
    pub fn payment_capabilities(&self) -> PaymentCapabilities {
        self.payment_capabilities
    }

    /// Shared transport.
    pub fn transport(&self) -> WalletTransport {
        self.transport.clone()
    }
}

async fn open_registry(data_dir: &Path) -> Result<(FederationRegistry, LocalStore)> {
    let database = WalletSqliteDatabase::new(data_dir.join(DATABASE_FILE)).await?;
    let localstore: LocalStore = Arc::new(database);
    Ok((FederationRegistry::new(localstore.clone()), localstore))
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
        let member_url = runtime.primary_mint_url().to_string();
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
        let member_url = runtime.primary_mint_url().to_string();
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
        let member_url = runtime.primary_mint_url().to_string();
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
        let member_url = runtime.primary_mint_url().to_string();
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
        let member_url = runtime.primary_mint_url().to_string();
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
        let member_url = runtime.primary_mint_url().to_string();
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
    let member_url = Some(runtime.primary_mint_url().to_string());
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
        generate_trusted_dealer_configs, MemberId, ThresholdParams, TrustedDealerKeysetSpec,
        TrustedDealerMember,
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
            JoinSource::Federation(FederationSource::Invite(value)) if value == "cashufed1example"
        ));
        assert!(matches!(
            JoinSource::parse(" federation-public.json ".to_string()),
            JoinSource::ConfigPath(path) if path.as_path() == Path::new("federation-public.json")
        ));
        assert!(matches!(
            JoinSource::parse(" https://mint.example.com ".to_string()),
            JoinSource::Federation(FederationSource::MemberUrl(url))
                if url.to_string() == "https://mint.example.com"
        ));
    }

    async fn memory_registry() -> FederationRegistry {
        let localstore: LocalStore = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("wallet memory db"),
        );
        FederationRegistry::new(localstore)
    }

    fn config_source(public_config: FederationConfig) -> FederationSource {
        FederationSource::PublicConfig(Box::new(public_config))
    }

    #[tokio::test]
    async fn first_join_uses_the_dashboard_policy_and_rejoin_keeps_it() {
        let registry = memory_registry().await;
        let transport = Async::default();
        let public_config = valid_public_config();
        let joined = register_join(&registry, config_source(public_config.clone()), &transport)
            .await
            .expect("join");
        let policy = joined.connector_policy();
        assert_eq!(policy.member_request_timeout_secs, 90);
        assert_eq!(policy.quote_coordinator_member_id, Some(member_id(1)));

        let switched = registry
            .set_connector_policy(
                joined.federation_id(),
                primary_policy(&joined, &member_url(&joined, 2)).expect("roster member"),
            )
            .await
            .expect("switch primary");
        let rejoined = register_join(&registry, config_source(public_config), &transport)
            .await
            .expect("rejoin");
        assert_eq!(rejoined, switched);
        assert_eq!(primary_member_url(&rejoined), Some(member_url(&joined, 2)));
    }

    #[tokio::test]
    async fn join_rejects_a_different_federation_over_an_existing_wallet() {
        let registry = memory_registry().await;
        let transport = Async::default();
        let joined = register_join(&registry, config_source(valid_public_config()), &transport)
            .await
            .expect("join");

        let other_config = distinct_public_config();
        assert_ne!(
            joined.federation_id(),
            other_config.federation_id,
            "trusted dealer configs must have distinct federation ids"
        );
        let err = register_join(&registry, config_source(other_config), &transport)
            .await
            .expect_err("different federation rejected");

        assert!(err.to_string().contains("already belongs to federation"));
        assert_eq!(registry.list().await.expect("list"), vec![joined]);
    }

    #[tokio::test]
    async fn primary_policy_accepts_roster_members_and_rejects_strangers() {
        let registry = memory_registry().await;
        let joined = register_join(
            &registry,
            config_source(valid_public_config()),
            &Async::default(),
        )
        .await
        .expect("join");

        let policy = primary_policy(&joined, &member_url(&joined, 3)).expect("roster member");
        assert_eq!(policy.quote_coordinator_member_id, Some(member_id(3)));
        assert_eq!(
            policy.member_request_timeout_secs,
            joined.connector_policy().member_request_timeout_secs
        );

        let stranger = MintUrl::from_str("https://stranger.example.com").expect("mint url");
        let err = primary_policy(&joined, &stranger).expect_err("non-roster member rejected");
        assert!(err.to_string().contains("not in the federation roster"));
    }

    fn member_url(federation: &RegisteredFederation, id: u16) -> MintUrl {
        federation
            .public_config()
            .member(member_id(id))
            .expect("member")
            .public_mint_url
            .clone()
    }
}
