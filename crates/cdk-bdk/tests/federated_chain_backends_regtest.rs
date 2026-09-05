use std::collections::HashMap;
use std::env;
use std::str::FromStr;
use std::time::Duration;

use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, RpcApi};
use bdk_wallet::bitcoin::bip32::{Xpriv, Xpub};
use bdk_wallet::bitcoin::secp256k1::Secp256k1;
use bdk_wallet::bitcoin::{Address, Amount, Network, Transaction, Txid};
use cdk_bdk::{
    BitcoinRpcConfig, BroadcastOutcome, ChainSource, EsploraConfig, FederatedBdkWallet,
    FederatedWalletConfig, FederatedWalletKeychain, FederatedWalletOutput, FederatedWalletScope,
};

static REGTEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn env_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn rpc_config() -> BitcoinRpcConfig {
    BitcoinRpcConfig {
        wallet_rescan_from_height: Some(0),
        host: env_or("CDK_BDK_REGTEST_RPC_HOST", "127.0.0.1"),
        port: env_or("CDK_BDK_REGTEST_RPC_PORT", "28443")
            .parse()
            .expect("RPC port"),
        user: env_or("CDK_BDK_REGTEST_RPC_USER", "cdk-regtest"),
        password: env_or("CDK_BDK_REGTEST_RPC_PASSWORD", "cdk-regtest-pass"),
        request_timeout_secs: 15,
    }
}

fn esplora_config() -> EsploraConfig {
    EsploraConfig {
        url: env_or("CDK_BDK_REGTEST_ESPLORA_URL", "http://127.0.0.1:23002"),
        parallel_requests: 4,
        request_timeout_secs: 15,
        max_retries: 3,
    }
}

fn wallet_config(seed: u8) -> FederatedWalletConfig {
    let secp = Secp256k1::new();
    let xpriv = Xpriv::new_master(Network::Regtest, &[seed; 32]).expect("master key");
    let xpub = Xpub::from_priv(&secp, &xpriv);
    let scope =
        FederatedWalletScope::new([1; 32], [2; 32], 1, 1, [3; 32]).expect("federated wallet scope");
    FederatedWalletConfig::new(
        scope,
        format!("tr({xpub}/0/*)"),
        format!("tr({xpub}/1/*)"),
        Network::Regtest,
    )
    .expect("federated wallet config")
}

fn checked_address(address: &str) -> Address {
    Address::from_str(address)
        .expect("regtest address")
        .require_network(Network::Regtest)
        .expect("regtest network")
}

fn canonical_outputs(outputs: Vec<FederatedWalletOutput>) -> Vec<(String, u64, bool)> {
    let mut outputs = outputs
        .into_iter()
        .map(|output| {
            (
                output.outpoint.to_string(),
                output.txout.value.to_sat(),
                output.confirmation.is_some(),
            )
        })
        .collect::<Vec<_>>();
    outputs.sort();
    outputs
}

async fn wait_for_matching_output(
    stage: &str,
    esplora: &FederatedBdkWallet,
    rpc: &FederatedBdkWallet,
    txid: Txid,
    confirmed: bool,
) {
    let mut last_rpc_outputs = Vec::new();
    let mut last_esplora_outputs = Vec::new();
    let mut last_rpc_sync_ok = false;
    let mut last_esplora_sync_ok = false;
    let mut last_rpc_error = None;
    let mut last_esplora_error = None;
    for _ in 0..80 {
        let rpc_sync = rpc.sync_once().await;
        let esplora_sync = esplora.sync_once().await;
        last_rpc_sync_ok = rpc_sync.is_ok();
        last_esplora_sync_ok = esplora_sync.is_ok();
        last_rpc_error = rpc_sync.err().map(|error| error.to_string());
        last_esplora_error = esplora_sync.err().map(|error| error.to_string());
        let rpc_outputs = rpc.observed_outputs().await;
        let esplora_outputs = esplora.observed_outputs().await;
        last_rpc_outputs = canonical_outputs(rpc_outputs.clone());
        last_esplora_outputs = canonical_outputs(esplora_outputs.clone());
        let rpc_match = rpc_outputs.iter().any(|output| {
            output.outpoint.txid == txid && output.confirmation.is_some() == confirmed
        });
        let esplora_match = esplora_outputs.iter().any(|output| {
            output.outpoint.txid == txid && output.confirmation.is_some() == confirmed
        });
        if last_rpc_sync_ok
            && last_esplora_sync_ok
            && rpc_match
            && esplora_match
            && last_rpc_outputs == last_esplora_outputs
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!(
        "RPC and Esplora wallets did not converge during {stage}: rpc_sync_ok={last_rpc_sync_ok}, esplora_sync_ok={last_esplora_sync_ok}, rpc_error={last_rpc_error:?}, esplora_error={last_esplora_error:?}, rpc={last_rpc_outputs:?}, esplora={last_esplora_outputs:?}"
    );
}

fn unsigned_payment(wallet: &Client, destination: &Address, amount: Amount) -> Transaction {
    let outputs = HashMap::from([(destination.to_string(), amount)]);
    let raw = wallet
        .create_raw_transaction_hex(&[], &outputs, None, Some(true))
        .expect("create raw transaction");
    let funded = wallet
        .fund_raw_transaction(raw, None, None)
        .expect("fund raw transaction");
    let signed = wallet
        .sign_raw_transaction_with_wallet(funded.hex.as_slice(), None, None)
        .expect("sign raw transaction");
    assert!(signed.complete);
    signed.transaction().expect("signed transaction")
}

async fn wait_for_rpc_output(
    stage: &str,
    wallet: &FederatedBdkWallet,
    txid: Txid,
    confirmed: bool,
) {
    let mut last_error = None;
    let mut last_outputs = Vec::new();
    for _ in 0..40 {
        let sync = wallet.sync_once().await;
        last_error = sync.err().map(|error| error.to_string());
        if last_error.is_none() {
            let outputs = wallet.observed_outputs().await;
            last_outputs = canonical_outputs(outputs.clone());
            let output = outputs.iter().find(|output| output.outpoint.txid == txid);
            if output
                .map(|output| output.confirmation.is_some())
                .unwrap_or(false)
                == confirmed
            {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "Bitcoin RPC wallet did not reach {stage}: error={last_error:?}, outputs={last_outputs:?}"
    );
}

#[tokio::test]
#[ignore = "requires isolated Bitcoin Core and Esplora regtest services"]
async fn heterogeneous_federated_wallets_converge_and_broadcast() {
    let _regtest_guard = REGTEST_LOCK.lock().await;
    let rpc_config = rpc_config();
    let rpc_url = format!("http://{}:{}", rpc_config.host, rpc_config.port);
    let auth = Auth::UserPass(rpc_config.user.clone(), rpc_config.password.clone());
    let admin = Client::new(&rpc_url, auth.clone()).expect("Bitcoin RPC admin client");
    let wallet_name = format!("cdk-m21-{}", std::process::id());
    let descriptor_seed = ((std::process::id() % 200) + 20) as u8;
    admin
        .create_wallet(&wallet_name, None, None, None, None)
        .expect("create mining wallet");
    let wallet = Client::new(&format!("{rpc_url}/wallet/{wallet_name}"), auth)
        .expect("Bitcoin RPC wallet client");
    let miner_address = wallet
        .get_new_address(None, None)
        .expect("miner address")
        .require_network(Network::Regtest)
        .expect("miner address network");
    wallet
        .generate_to_address(101, &miner_address)
        .expect("mature mining balance");

    let esplora_root = tempfile::tempdir().expect("Esplora wallet directory");
    let rpc_root = tempfile::tempdir().expect("RPC wallet directory");
    let wallet_config = wallet_config(descriptor_seed);
    let esplora_wallet = FederatedBdkWallet::open(
        wallet_config.clone(),
        ChainSource::Esplora(esplora_config()),
        esplora_root.path(),
    )
    .expect("Esplora federated wallet");
    let rpc_wallet = FederatedBdkWallet::open(
        wallet_config,
        ChainSource::BitcoinRpc(rpc_config),
        rpc_root.path(),
    )
    .expect("RPC federated wallet");

    let deposit_address = checked_address(
        &esplora_wallet
            .reveal_address(FederatedWalletKeychain::External, 0)
            .await
            .expect("Esplora deposit address"),
    );
    assert_eq!(
        rpc_wallet
            .reveal_address(FederatedWalletKeychain::External, 0)
            .await
            .expect("RPC deposit address"),
        deposit_address.to_string()
    );
    let deposit_txid = wallet
        .send_to_address(
            &deposit_address,
            Amount::from_sat(50_000),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("send descriptor deposit");
    wallet
        .generate_to_address(1, &miner_address)
        .expect("confirm descriptor deposit");
    wait_for_matching_output(
        "initial confirmation",
        &esplora_wallet,
        &rpc_wallet,
        deposit_txid,
        true,
    )
    .await;

    let second_address = checked_address(
        &esplora_wallet
            .reveal_address(FederatedWalletKeychain::External, 1)
            .await
            .expect("second Esplora address"),
    );
    assert_eq!(
        rpc_wallet
            .reveal_address(FederatedWalletKeychain::External, 1)
            .await
            .expect("second RPC address"),
        second_address.to_string()
    );
    let transaction = unsigned_payment(&wallet, &second_address, Amount::from_sat(25_000));
    let transaction_id = transaction.compute_txid();
    assert_eq!(
        esplora_wallet
            .engine()
            .broadcast_classified(transaction.clone())
            .await
            .expect("Esplora accepts transaction"),
        BroadcastOutcome::Accepted
    );
    assert!(matches!(
        rpc_wallet
            .engine()
            .broadcast_classified(transaction)
            .await
            .expect("RPC recognizes or accepts transaction"),
        BroadcastOutcome::Accepted | BroadcastOutcome::AlreadyKnown
    ));
    wallet
        .generate_to_address(1, &miner_address)
        .expect("confirm second descriptor deposit");
    wait_for_matching_output(
        "broadcast confirmation",
        &esplora_wallet,
        &rpc_wallet,
        transaction_id,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires isolated Bitcoin Core regtest service"]
async fn bitcoin_rpc_wallet_reorgs_and_recovers() {
    let _regtest_guard = REGTEST_LOCK.lock().await;
    let rpc_config = rpc_config();
    let rpc_url = format!("http://{}:{}", rpc_config.host, rpc_config.port);
    let auth = Auth::UserPass(rpc_config.user.clone(), rpc_config.password.clone());
    let admin = Client::new(&rpc_url, auth.clone()).expect("Bitcoin RPC admin client");
    let wallet_name = format!("cdk-m21-reorg-{}", std::process::id());
    admin
        .create_wallet(&wallet_name, None, None, None, None)
        .expect("create reorg wallet");
    let core_wallet = Client::new(&format!("{rpc_url}/wallet/{wallet_name}"), auth)
        .expect("Bitcoin RPC wallet client");
    let miner_address = core_wallet
        .get_new_address(None, None)
        .expect("miner address")
        .require_network(Network::Regtest)
        .expect("miner address network");
    core_wallet
        .generate_to_address(101, &miner_address)
        .expect("mature mining balance");

    let root = tempfile::tempdir().expect("RPC BDK wallet directory");
    let descriptor_seed = ((std::process::id() % 200) + 30) as u8;
    let bdk_wallet = FederatedBdkWallet::open(
        wallet_config(descriptor_seed),
        ChainSource::BitcoinRpc(rpc_config),
        root.path(),
    )
    .expect("RPC federated wallet");
    let deposit_address = checked_address(
        &bdk_wallet
            .reveal_address(FederatedWalletKeychain::External, 0)
            .await
            .expect("deposit address"),
    );
    let txid = core_wallet
        .send_to_address(
            &deposit_address,
            Amount::from_sat(40_000),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("send reorg deposit");
    let mined = core_wallet
        .generate_to_address(1, &miner_address)
        .expect("confirm reorg deposit");
    let block = *mined.first().expect("mined block");
    wait_for_rpc_output("initial confirmation", &bdk_wallet, txid, true).await;

    admin
        .invalidate_block(&block)
        .expect("invalidate deposit block");
    let _: serde_json::Value = admin
        .call(
            "generateblock",
            &[
                miner_address.to_string().into(),
                serde_json::Value::Array(Vec::new()),
            ],
        )
        .expect("mine competing block without the deposit");
    wait_for_rpc_output("reorged state", &bdk_wallet, txid, false).await;
    core_wallet
        .generate_to_address(1, &miner_address)
        .expect("reconfirm deposit");
    wait_for_rpc_output("recovered confirmation", &bdk_wallet, txid, true).await;
}
