use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use bip39::rand::{thread_rng, Rng};
use bip39::Mnemonic;
use cdk_sqlite::WalletSqliteDatabase;
use clap::Parser;
use tracing::Level;

const DEFAULT_WORK_DIR: &str = ".cdk-signatory";

/// Simple CLI application to interact with cashu
#[derive(Parser)]
#[command(name = "cashu-signatory")]
#[command(author = "thesimplekid <tsk@thesimplekid.com>")]
#[command(version = "0.1.0")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Database engine to use (sqlite/redb)
    #[arg(short, long, default_value = "sqlite")]
    engine: String,
    /// Database password for sqlcipher
    #[arg(long)]
    password: Option<String>,
    /// Path to working dir
    #[arg(short, long)]
    work_dir: Option<PathBuf>,
    /// Logging level
    #[arg(short, long, default_value = "error")]
    log_level: Level,
    /// NWS Proxy
    #[arg(short, long)]
    proxy: Option<Url>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Cli = Cli::parse();
    let default_filter = args.log_level;

    let sqlx_filter = "sqlx=warn,hyper_util=warn,reqwest=warn";

    let env_filter = EnvFilter::new(format!("{},{}", default_filter, sqlx_filter));

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let work_dir = match &args.work_dir {
        Some(work_dir) => work_dir.clone(),
        None => {
            let home_dir = home::home_dir().unwrap();
            home_dir.join(DEFAULT_WORK_DIR)
        }
    };

    fs::create_dir_all(&work_dir)?;

    let localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync> =
        match args.engine.as_str() {
            "sqlite" => {
                let sql_path = work_dir.join("cdk-cli.sqlite");
                #[cfg(not(feature = "sqlcipher"))]
                let sql = WalletSqliteDatabase::new(&sql_path).await?;
                #[cfg(feature = "sqlcipher")]
                let sql = {
                    match args.password {
                        Some(pass) => WalletSqliteDatabase::new(&sql_path, pass).await?,
                        None => bail!("Missing database password"),
                    }
                };

                Arc::new(sql)
            }
            "redb" => {
                #[cfg(feature = "redb")]
                {
                    let redb_path = work_dir.join("cdk-cli.redb");
                    Arc::new(WalletRedbDatabase::new(&redb_path)?)
                }
                #[cfg(not(feature = "redb"))]
                {
                    bail!("redb feature not enabled");
                }
            }
            _ => bail!("Unknown DB engine"),
        };

    let seed_path = work_dir.join("seed");

    let mnemonic = match fs::metadata(seed_path.clone()) {
        Ok(_) => {
            let contents = fs::read_to_string(seed_path.clone())?;
            Mnemonic::from_str(&contents)?
        }
        Err(_e) => {
            let mut rng = thread_rng();
            let random_bytes: [u8; 32] = rng.gen();

            let mnemonic = Mnemonic::from_entropy(&random_bytes)?;
            tracing::info!("Creating new seed");

            fs::write(seed_path, mnemonic.to_string())?;

            mnemonic
        }
    };
    let seed = mnemonic.to_seed_normalized("");

    let mut wallets: Vec<Wallet> = Vec::new();

    let mints = localstore.get_mints().await?;

    for (mint_url, _) in mints {
        let mut builder = WalletBuilder::new()
            .mint_url(mint_url.clone())
            .unit(cdk::nuts::CurrencyUnit::Sat)
            .localstore(localstore.clone())
            .seed(&mnemonic.to_seed_normalized(""));

        if let Some(proxy_url) = args.proxy.as_ref() {
            let http_client = HttpClient::with_proxy(mint_url, proxy_url.clone(), None, true)?;
            builder = builder.client(http_client);
        }

        let wallet = builder.build()?;

        let wallet_clone = wallet.clone();

        tokio::spawn(async move {
            if let Err(err) = wallet_clone.get_mint_info().await {
                tracing::error!(
                    "Could not get mint quote for {}, {}",
                    wallet_clone.mint_url,
                    err
                );
            }
        });

        wallets.push(wallet);
    }

    let multi_mint_wallet = MultiMintWallet::new(localstore, Arc::new(seed), wallets);
}
