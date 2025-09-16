#[cfg(feature = "fakewallet")]
use std::collections::HashMap;
#[cfg(feature = "fakewallet")]
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

#[cfg(feature = "cln")]
use anyhow::anyhow;
use async_trait::async_trait;
#[cfg(feature = "fakewallet")]
use bip39::rand::{thread_rng, Rng};
use cdk::cdk_database::MintKVStore;
use cdk::cdk_payment::MintPayment;
use cdk::nuts::CurrencyUnit;
#[cfg(any(
    feature = "lnbits",
    feature = "cln",
    feature = "lnd",
    feature = "ldk-node",
    feature = "fakewallet"
))]
use cdk::types::FeeReserve;

use crate::config::{self, Settings};
#[cfg(feature = "cln")]
use crate::expand_path;

#[async_trait]
pub trait LnBackendSetup {
    async fn setup(
        &self,
        settings: &Settings,
        unit: CurrencyUnit,
        runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
        work_dir: &Path,
        kv_store: Option<Arc<dyn MintKVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
    ) -> anyhow::Result<impl MintPayment>;
}

#[cfg(feature = "cln")]
#[async_trait]
impl LnBackendSetup for config::Cln {
    async fn setup(
        &self,
        _settings: &Settings,
        _unit: CurrencyUnit,
        _runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
        _work_dir: &Path,
        kv_store: Option<Arc<dyn MintKVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
    ) -> anyhow::Result<cdk_cln::Cln> {
        let cln_socket = expand_path(
            self.rpc_path
                .to_str()
                .ok_or(anyhow!("cln socket not defined"))?,
        )
        .ok_or(anyhow!("cln socket not defined"))?;

        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        let cln = cdk_cln::Cln::new(
            cln_socket,
            fee_reserve,
            kv_store.expect("Cln needs kv store"),
        )
        .await?;

        Ok(cln)
    }
}

#[cfg(feature = "lnbits")]
#[async_trait]
impl LnBackendSetup for config::LNbits {
    async fn setup(
        &self,
        _settings: &Settings,
        _unit: CurrencyUnit,
        _runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
        _work_dir: &Path,
        _kv_store: Option<Arc<dyn MintKVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
    ) -> anyhow::Result<cdk_lnbits::LNbits> {
        let admin_api_key = &self.admin_api_key;
        let invoice_api_key = &self.invoice_api_key;

        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        let lnbits = cdk_lnbits::LNbits::new(
            admin_api_key.clone(),
            invoice_api_key.clone(),
            self.lnbits_api.clone(),
            fee_reserve,
        )
        .await?;

        // Use v1 websocket API
        lnbits.subscribe_ws().await?;

        Ok(lnbits)
    }
}

#[cfg(feature = "lnd")]
#[async_trait]
impl LnBackendSetup for config::Lnd {
    async fn setup(
        &self,
        _settings: &Settings,
        _unit: CurrencyUnit,
        _runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
        _work_dir: &Path,
        kv_store: Option<Arc<dyn MintKVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
    ) -> anyhow::Result<cdk_lnd::Lnd> {
        let address = &self.address;
        let cert_file = &self.cert_file;
        let macaroon_file = &self.macaroon_file;

        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        let lnd = cdk_lnd::Lnd::new(
            address.to_string(),
            cert_file.clone(),
            macaroon_file.clone(),
            fee_reserve,
            kv_store.expect("Lnd needs kv store"),
        )
        .await?;

        Ok(lnd)
    }
}

#[cfg(feature = "fakewallet")]
#[async_trait]
impl LnBackendSetup for config::FakeWallet {
    async fn setup(
        &self,
        _settings: &Settings,
        unit: CurrencyUnit,
        _runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
        _work_dir: &Path,
        _kv_store: Option<Arc<dyn MintKVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
    ) -> anyhow::Result<cdk_fake_wallet::FakeWallet> {
        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        // calculate random delay time
        let mut rng = thread_rng();
        let delay_time = rng.gen_range(self.min_delay_time..=self.max_delay_time);

        let fake_wallet = cdk_fake_wallet::FakeWallet::new(
            fee_reserve,
            HashMap::default(),
            HashSet::default(),
            delay_time,
            unit,
        );

        Ok(fake_wallet)
    }
}

#[cfg(feature = "grpc-processor")]
#[async_trait]
impl LnBackendSetup for config::GrpcProcessor {
    async fn setup(
        &self,
        _settings: &Settings,
        _unit: CurrencyUnit,
        _runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
        _work_dir: &Path,
        _kv_store: Option<Arc<dyn MintKVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
    ) -> anyhow::Result<cdk_payment_processor::PaymentProcessorClient> {
        let payment_processor = cdk_payment_processor::PaymentProcessorClient::new(
            &self.addr,
            self.port,
            self.tls_dir.clone(),
        )
        .await?;

        Ok(payment_processor)
    }
}

#[cfg(feature = "ldk-node")]
#[async_trait]
impl LnBackendSetup for config::LdkNode {
    async fn setup(
        &self,
        _settings: &Settings,
        _unit: CurrencyUnit,
        runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
        work_dir: &Path,
        _kv_store: Option<Arc<dyn MintKVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
    ) -> anyhow::Result<cdk_ldk_node::CdkLdkNode> {
        use std::net::SocketAddr;

        use bitcoin::Network;

        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        // Parse network from config
        let network = match self
            .bitcoin_network
            .as_ref()
            .map(|n| n.to_lowercase())
            .as_deref()
            .unwrap_or("regtest")
        {
            "mainnet" | "bitcoin" => Network::Bitcoin,
            "testnet" => Network::Testnet,
            "signet" => Network::Signet,
            _ => Network::Regtest,
        };

        // Parse chain source from config
        let chain_source = match self
            .chain_source_type
            .as_ref()
            .map(|s| s.to_lowercase())
            .as_deref()
            .unwrap_or("esplora")
        {
            "bitcoinrpc" => {
                let host = self
                    .bitcoind_rpc_host
                    .clone()
                    .unwrap_or_else(|| "127.0.0.1".to_string());
                let port = self.bitcoind_rpc_port.unwrap_or(18443);
                let user = self
                    .bitcoind_rpc_user
                    .clone()
                    .unwrap_or_else(|| "testuser".to_string());
                let password = self
                    .bitcoind_rpc_password
                    .clone()
                    .unwrap_or_else(|| "testpass".to_string());

                cdk_ldk_node::ChainSource::BitcoinRpc(cdk_ldk_node::BitcoinRpcConfig {
                    host,
                    port,
                    user,
                    password,
                })
            }
            _ => {
                let esplora_url = self
                    .esplora_url
                    .clone()
                    .unwrap_or_else(|| "https://mutinynet.com/api".to_string());
                cdk_ldk_node::ChainSource::Esplora(esplora_url)
            }
        };

        // Parse gossip source from config
        let gossip_source = match self.rgs_url.clone() {
            Some(rgs_url) => cdk_ldk_node::GossipSource::RapidGossipSync(rgs_url),
            None => cdk_ldk_node::GossipSource::P2P,
        };

        // Get storage directory path
        let storage_dir_path = if let Some(dir_path) = &self.storage_dir_path {
            dir_path.clone()
        } else {
            let mut work_dir = work_dir.to_path_buf();
            work_dir.push("ldk-node");
            work_dir.to_string_lossy().to_string()
        };

        // Get LDK node listen address
        let host = self
            .ldk_node_host
            .clone()
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let port = self.ldk_node_port.unwrap_or(8090);

        let socket_addr = SocketAddr::new(host.parse()?, port);

        // Parse socket address using ldk_node's SocketAddress
        // We need to get the actual socket address struct from ldk_node
        // For now, let's construct it manually based on the cdk-ldk-node implementation
        let listen_address = vec![socket_addr.into()];

        let mut ldk_node = cdk_ldk_node::CdkLdkNode::new(
            network,
            chain_source,
            gossip_source,
            storage_dir_path,
            fee_reserve,
            listen_address,
            runtime,
        )?;

        // Configure webserver address if specified
        let webserver_addr = if let Some(host) = &self.webserver_host {
            let port = self.webserver_port.unwrap_or(8091);
            let socket_addr: SocketAddr = format!("{host}:{port}").parse()?;
            Some(socket_addr)
        } else if self.webserver_port.is_some() {
            // If only port is specified, use default host
            let port = self.webserver_port.unwrap_or(8091);
            let socket_addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
            Some(socket_addr)
        } else {
            // Use default webserver address if nothing is configured
            Some(cdk_ldk_node::CdkLdkNode::default_web_addr())
        };

        println!("webserver: {:?}", webserver_addr);

        ldk_node.set_web_addr(webserver_addr);

        Ok(ldk_node)
    }
}
