//! PostgreSQL-backed swap coordination with an HTTP wallet client.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::{mpsc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use cdk::amount::FeeAndAmounts;
use cdk::mint::{MintBuilder, MintMeltLimits, UnitConfig};
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod, PreMintSecrets, State, SwapRequest};
use cdk::wallet::{ReceiveOptions, SendOptions};
use cdk::{Amount, Wallet};
use cdk_common::common::{FeeReserve, QuoteTTL};
use cdk_common::mint::OperationKind;
use cdk_common::nuts::{BlindSignature, BlindedMessage, Proofs};
use cdk_fake_wallet::FakeWallet;
use cdk_postgres::MintPgDatabase;
use cdk_signatory::db_signatory::DbSignatory;
use cdk_signatory::embedded;
use cdk_signatory::signatory::{
    ReconstructDleqArguments, RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets,
};
use cdk_sqlite::wallet::memory;
use tokio::sync::{oneshot, watch, Mutex};
use tokio::time::{sleep, timeout};

type Signer = Arc<dyn Signatory + Send + Sync>;
struct SigningGate {
    reached: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

struct PauseSigning {
    inner: Signer,
    next: Mutex<Option<SigningGate>>,
}

#[async_trait::async_trait]
impl Signatory for PauseSigning {
    fn name(&self) -> String {
        self.inner.name()
    }

    async fn blind_sign(
        &self,
        messages: Vec<BlindedMessage>,
    ) -> Result<Vec<BlindSignature>, cdk_common::Error> {
        let next = self.next.lock().await.take();
        if let Some(gate) = next {
            let _ = gate.reached.send(());
            let _ = gate.release.await;
        }
        self.inner.blind_sign(messages).await
    }

    async fn verify_proofs(&self, proofs: Proofs) -> Result<(), cdk_common::Error> {
        self.inner.verify_proofs(proofs).await
    }

    async fn keysets(&self) -> Result<SignatoryKeysets, cdk_common::Error> {
        self.inner.keysets().await
    }

    async fn subscribe_keysets(
        &self,
    ) -> Result<watch::Receiver<SignatoryKeysets>, cdk_common::Error> {
        self.inner.subscribe_keysets().await
    }

    async fn rotate_keyset(
        &self,
        args: RotateKeyArguments,
    ) -> Result<SignatoryKeySet, cdk_common::Error> {
        self.inner.rotate_keyset(args).await
    }

    async fn reconstruct_dleq(
        &self,
        args: Vec<ReconstructDleqArguments>,
    ) -> Result<Vec<BlindSignature>, cdk_common::Error> {
        self.inner.reconstruct_dleq(args).await
    }
}

async fn builder(db: Arc<MintPgDatabase>, url: &str) -> Result<MintBuilder> {
    let mut builder = MintBuilder::new(db).with_urls(vec![url.to_owned()]);
    let backend = FakeWallet::new(
        FeeReserve {
            min_fee_reserve: Amount::from(1),
            percent_fee_reserve: 1.0,
        },
        Default::default(),
        Default::default(),
        0,
        CurrencyUnit::Sat,
    );
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::BOLT11,
            MintMeltLimits::new(1, 50_000),
            Arc::new(backend),
        )
        .await?;
    Ok(builder)
}

async fn fund_wallet(wallet: &Wallet, amount: Amount) -> Result<Proofs> {
    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
        .await?;
    timeout(Duration::from_secs(10), async {
        loop {
            if wallet.check_mint_quote_status(&quote.id).await?.state == MintQuoteState::Paid {
                return Ok::<_, anyhow::Error>(());
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;
    Ok(wallet.mint(&quote.id, Default::default(), None).await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a PostgreSQL test database"]
async fn concurrent_replica_config_updates_preserve_both_changes() -> Result<()> {
    let url = std::env::var("CDK_MINTD_DATABASE_URL")
        .or_else(|_| std::env::var("PG_DB_URL"))
        .context("set CDK_MINTD_DATABASE_URL or PG_DB_URL")?;
    let config = format!(
        "{url} schema=replica_config_{}",
        uuid::Uuid::new_v4().simple()
    );
    let db_a = Arc::new(MintPgDatabase::new(config.as_str()).await?);
    let db_b = Arc::new(MintPgDatabase::new(config.as_str()).await?);
    let mint_url = "http://127.0.0.1:8085";
    let mint_a = builder(db_a.clone(), mint_url)
        .await?
        .build_with_seed(db_a, &[42; 64])
        .await?;
    let mint_b = builder(db_b.clone(), mint_url)
        .await?
        .build_with_seed(db_b, &[42; 64])
        .await?;

    let (arrived_a, await_a) = mpsc::channel();
    let (arrived_b, await_b) = mpsc::channel();
    let await_a = StdMutex::new(await_a);
    let await_b = StdMutex::new(await_b);
    let a = mint_a.clone();
    let b = mint_b.clone();
    let update_a = tokio::spawn(async move {
        let entered = AtomicBool::new(false);
        a.update_mint_info(|info| {
            if !entered.swap(true, Ordering::Relaxed) {
                arrived_a.send(()).expect("signal first read");
                await_b
                    .lock()
                    .expect("gate")
                    .recv_timeout(Duration::from_secs(10))
                    .expect("peer read");
            }
            info.name = Some("Replica mint".to_owned());
        })
        .await
    });
    let update_b = tokio::spawn(async move {
        let entered = AtomicBool::new(false);
        b.update_mint_info(|info| {
            if !entered.swap(true, Ordering::Relaxed) {
                arrived_b.send(()).expect("signal first read");
                await_a
                    .lock()
                    .expect("gate")
                    .recv_timeout(Duration::from_secs(10))
                    .expect("peer read");
            }
            info.motd = Some("Concurrent replica update".to_owned());
        })
        .await
    });
    let (left, right) = tokio::join!(update_a, update_b);
    left??;
    right??;

    for mint in [&mint_a, &mint_b] {
        let info = mint.mint_info().await?;
        ensure!(info.name.as_deref() == Some("Replica mint"));
        ensure!(info.motd.as_deref() == Some("Concurrent replica update"));
    }
    let (left, right) = tokio::join!(
        mint_a.update_quote_ttl(Some(61), None),
        mint_b.update_quote_ttl(None, Some(62)),
    );
    left?;
    right?;
    ensure!(mint_a.quote_ttl().await? == QuoteTTL::new(61, 62));
    ensure!(mint_b.quote_ttl().await? == QuoteTTL::new(61, 62));
    Ok(())
}

#[tokio::test]
#[ignore = "requires a PostgreSQL test database"]
async fn peer_startup_preserves_a_live_swap() -> Result<()> {
    let url = std::env::var("CDK_MINTD_DATABASE_URL")
        .or_else(|_| std::env::var("PG_DB_URL"))
        .context("set CDK_MINTD_DATABASE_URL or PG_DB_URL")?;
    let config = format!(
        "{url} schema=replica_swap_{}",
        uuid::Uuid::new_v4().simple()
    );
    let db_a = Arc::new(MintPgDatabase::new(config.as_str()).await?);
    let db_b = Arc::new(MintPgDatabase::new(config.as_str()).await?);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let mint_url = format!("http://{}", listener.local_addr()?);
    let builder_a = builder(db_a.clone(), &mint_url).await?;
    let build_b = builder(db_b, &mint_url).await?;
    let unit = UnitConfig::default();
    let signer = Arc::new(embedded::Service::new(Arc::new(
        DbSignatory::new(
            db_a,
            &[42; 64],
            [(CurrencyUnit::Sat, (unit.input_fee_ppk, unit.amounts))].into(),
            Default::default(),
        )
        .await?,
    )));
    let gate = Arc::new(PauseSigning {
        inner: signer.clone(),
        next: Mutex::new(None),
    });
    let mint_a = builder_a.build_with_signatory(gate.clone()).await?;
    let mint_b = build_b.build_with_signatory(signer).await?;
    mint_a.set_quote_ttl(QuoteTTL::new(60, 60)).await?;
    mint_a.start().await?;
    let router = cdk_axum::create_mint_router(mint_a.clone(), vec!["bolt11".to_owned()]).await?;
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let alice = Wallet::new(
        &mint_url,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        [43; 64],
        None,
    )?;
    let bob = Wallet::new(
        &mint_url,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        [44; 64],
        None,
    )?;

    let amount = Amount::from(1024);
    let proofs = fund_wallet(&alice, amount).await?;
    let keys = proofs.ys()?;
    let token = alice
        .prepare_send(amount, SendOptions::default())
        .await?
        .confirm(None)
        .await?
        .to_string();

    let (arrived, parked) = oneshot::channel();
    let (unpark, release) = oneshot::channel();
    *gate.next.lock().await = Some(SigningGate {
        reached: arrived,
        release,
    });
    let receive = tokio::spawn(async move { bob.receive(&token, ReceiveOptions::default()).await });
    timeout(Duration::from_secs(5), parked).await??;
    let storage_a = mint_a.localstore();
    ensure!(storage_a
        .get_proofs_states(&keys)
        .await?
        .iter()
        .all(|state| *state == Some(State::Pending)));
    let live_swap = storage_a
        .get_incomplete_sagas(OperationKind::Swap)
        .await?
        .pop()
        .context("live swap saga")?;
    mint_b.start().await?;
    ensure!(
        storage_a
            .get_proofs_states(&keys)
            .await?
            .iter()
            .all(|state| *state == Some(State::Pending)),
        "startup changed the live swap reservation"
    );
    timeout(Duration::from_secs(22), async {
        loop {
            let sagas = storage_a.get_incomplete_sagas(OperationKind::Swap).await?;
            if sagas.iter().any(|saga| {
                saga.operation_id == live_swap.operation_id
                    && saga.updated_at > live_swap.updated_at
            }) {
                return Ok::<_, anyhow::Error>(());
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("live swap did not renew its recovery lease")??;
    let _ = unpark.send(());
    let receive_result = timeout(Duration::from_secs(5), receive).await??;
    mint_b.stop().await?;
    mint_a.stop().await?;
    server.abort();
    ensure!(receive_result? == amount, "wallet receive did not finish");
    ensure!(storage_a
        .get_proofs_states(&keys)
        .await?
        .iter()
        .all(|state| *state == Some(State::Spent)));
    Ok(())
}

#[tokio::test]
#[ignore = "requires a PostgreSQL test database"]
async fn peer_accepts_rotated_keyset_before_background_refresh() -> Result<()> {
    let url = std::env::var("CDK_MINTD_DATABASE_URL")
        .or_else(|_| std::env::var("PG_DB_URL"))
        .context("set CDK_MINTD_DATABASE_URL or PG_DB_URL")?;
    let config = format!(
        "{url} schema=replica_keys_{}",
        uuid::Uuid::new_v4().simple()
    );
    let db_a = Arc::new(MintPgDatabase::new(config.as_str()).await?);
    let db_b = Arc::new(MintPgDatabase::new(config.as_str()).await?);
    let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url_a = format!("http://{}", listener_a.local_addr()?);
    let url_b = format!("http://{}", listener_b.local_addr()?);
    // The signatory aligns to Unix epoch boundaries. Schedule the first tick an hour away.
    let refresh = Duration::from_secs(cdk_common::util::unix_time() + 3_600);
    let mint_a = builder(db_a.clone(), &url_a)
        .await?
        .with_keyset_refresh_interval(Some(refresh))
        .build_with_seed(db_a, &[42; 64])
        .await?;
    let mint_b = builder(db_b.clone(), &url_a)
        .await?
        .with_keyset_refresh_interval(Some(refresh))
        .build_with_seed(db_b, &[42; 64])
        .await?;
    mint_a.set_quote_ttl(QuoteTTL::new(60, 60)).await?;
    mint_a.start().await?;
    mint_b.start().await?;
    let router_a = cdk_axum::create_mint_router(mint_a.clone(), vec!["bolt11".to_owned()]).await?;
    let router_b = cdk_axum::create_mint_router(mint_b.clone(), vec!["bolt11".to_owned()]).await?;
    let server_a = tokio::spawn(async move { axum::serve(listener_a, router_a).await });
    let server_b = tokio::spawn(async move { axum::serve(listener_b, router_b).await });
    let wallet_a = Wallet::new(
        &url_a,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        [43; 64],
        None,
    )?;
    let wallet_b = Wallet::new(
        &url_b,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        [44; 64],
        None,
    )?;

    let amount = Amount::from(1024);
    let proofs = fund_wallet(&wallet_a, amount).await?;
    let old_id = proofs.first().context("pre-rotation proof")?.keyset_id;
    let rotated = mint_a
        .rotate_keyset(CurrencyUnit::Sat, Vec::new(), 0, true, None)
        .await?;
    ensure!(mint_b.get_active_keysets().get(&CurrencyUnit::Sat) == Some(&old_id));
    let amounts = FeeAndAmounts::from((0, rotated.amounts));
    let outputs = PreMintSecrets::random(rotated.id, amount, &Default::default(), &amounts)?;
    let swapped = wallet_b
        .mint_connector()
        .post_swap(SwapRequest::new(proofs, outputs.blinded_messages()))
        .await?;
    ensure!(!swapped.signatures.is_empty());
    ensure!(swapped
        .signatures
        .iter()
        .all(|sig| sig.keyset_id == rotated.id));

    let rotated_again = mint_b
        .rotate_keyset(CurrencyUnit::Sat, Vec::new(), 0, true, None)
        .await?;
    ensure!(mint_a.get_active_keysets().get(&CurrencyUnit::Sat) == Some(&rotated.id));
    let connector = wallet_a.mint_connector();
    let active = connector.get_mint_keys().await?;
    ensure!(active.iter().any(|keys| keys.id == rotated_again.id));
    ensure!(connector.get_mint_keyset(old_id).await?.id == old_id);

    mint_a.stop().await?;
    mint_b.stop().await?;
    server_a.abort();
    server_b.abort();
    Ok(())
}
