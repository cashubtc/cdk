use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use bitcoin::Network;
use cdk_bdk::{BatchConfig, BitcoinRpcConfig, CdkBdk, ChainSource};
use cdk_common::common::FeeReserve;
use cdk_sqlite::MintSqliteDatabase;

const PREFIX: &str = "CDK_PAYMENT_PROCESSOR_BDK_";

fn required(key: &str) -> Result<String> {
    env::var(format!("{PREFIX}{key}")).with_context(|| format!("set {PREFIX}{key}"))
}

fn number<T>(key: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
{
    match env::var(format!("{PREFIX}{key}")) {
        Ok(value) => value
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid {PREFIX}{key}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(err) => Err(err).with_context(|| format!("read {PREFIX}{key}")),
    }
}

pub(super) async fn from_env() -> Result<CdkBdk> {
    let work_dir = PathBuf::from(required("WORK_DIR")?);
    let mnemonic = bip39::Mnemonic::parse(required("MNEMONIC")?)
        .map_err(|_| anyhow::anyhow!("invalid {PREFIX}MNEMONIC"))?;
    let network = match required("NETWORK")?.as_str() {
        "mainnet" => Network::Bitcoin,
        name => name
            .parse()
            .with_context(|| format!("invalid {PREFIX}NETWORK"))?,
    };

    let intents_path = work_dir.join("intents.sqlite");
    match env::var(format!("{PREFIX}INIT_MODE")).as_deref() {
        Err(env::VarError::NotPresent) | Ok("existing") => {
            cdk_bdk::validate_existing_wallet(mnemonic.clone(), network, &work_dir)?;
            if !intents_path.is_file() {
                bail!("existing BDK processor intent database is missing");
            }
        }
        Ok("new") => std::fs::create_dir_all(&work_dir)?,
        _ => bail!("{PREFIX}INIT_MODE must be new or existing"),
    }

    let num_confs = number("NUM_CONFS", 6)?;
    if num_confs == 0 {
        bail!("{PREFIX}NUM_CONFS must be at least one");
    }
    let chain_source = ChainSource::BitcoinRpc(BitcoinRpcConfig {
        host: required("BITCOIN_RPC_HOST")?,
        port: required("BITCOIN_RPC_PORT")?
            .parse()
            .with_context(|| format!("invalid {PREFIX}BITCOIN_RPC_PORT"))?,
        user: required("BITCOIN_RPC_USER")?,
        password: required("BITCOIN_RPC_PASSWORD")?,
        wallet_rescan_from_height: None,
    });
    let fee_reserve = FeeReserve {
        percent_fee_reserve: env::var(super::ENV_FEE_PERCENT)
            .unwrap_or_else(|_| "0.02".to_owned())
            .parse()
            .context("invalid payment processor fee percent")?,
        min_fee_reserve: env::var(super::ENV_RESERVE_FEE_MIN)
            .unwrap_or_else(|_| "1".to_owned())
            .parse::<u64>()
            .context("invalid payment processor minimum reserve fee")?
            .into(),
    };
    let batch = BatchConfig {
        poll_interval: Duration::from_secs(number("POLL_INTERVAL_SECS", 30)?),
        ..BatchConfig::default()
    };
    let sync_interval = number("SYNC_INTERVAL_SECS", 30)?;
    let intents = Arc::new(MintSqliteDatabase::new(intents_path).await?);

    Ok(CdkBdk::new(
        mnemonic,
        network,
        chain_source,
        work_dir
            .to_str()
            .context("BDK work directory must be valid UTF-8")?
            .to_owned(),
        fee_reserve,
        intents,
        Some(batch),
        num_confs,
        1_000,
        546,
        sync_interval,
        None,
        None,
    )?)
}
