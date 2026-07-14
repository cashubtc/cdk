//! Isolated watch-only BDK runtime for a federated wallet member.

use core::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bdk_wallet::bitcoin::hashes::{sha256, Hash};
use bdk_wallet::bitcoin::{BlockHash, Network, OutPoint, ScriptBuf, TxOut, Txid};
use bdk_wallet::KeychainKind;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{BdkWalletEngine, ChainSource, ChainSourceKind, DescriptorConfig, Error};

const FEDERATED_WALLET_NAMESPACE_VERSION: u16 = 1;
const FEDERATED_WALLET_NAMESPACE_DOMAIN: &[u8] = b"cdk/bdk/federated-wallet-namespace/v1";
const FEDERATED_WALLET_DIRECTORY: &str = "federation-bdk-v1";
const MAX_SYNC_FAILURES: u32 = 1_000_000;

/// Operational independence classification for a local chain source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederatedChainSourceScope {
    /// Member-operated infrastructure.
    Independent,
    /// Infrastructure intentionally shared by multiple members.
    Shared,
}

/// Opaque source identity safe to attach to consensus observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederatedChainSourceDescriptor {
    /// Operational independence classification.
    pub scope: FederatedChainSourceScope,
    /// Bounded opaque identifier; never an endpoint or credential.
    pub source_id: String,
}

impl FederatedChainSourceDescriptor {
    /// Construct a bounded, redacted descriptor.
    pub fn new(
        scope: FederatedChainSourceScope,
        source_id: impl Into<String>,
    ) -> Result<Self, Error> {
        let source = Self {
            scope,
            source_id: source_id.into(),
        };
        if source.source_id.is_empty()
            || source.source_id.len() > 128
            || !source
                .source_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(Error::InvalidConfig(
                "federated chain source id must be bounded and opaque".to_string(),
            ));
        }
        Ok(source)
    }
}

/// Public descriptor branch used after a consensus allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederatedWalletKeychain {
    /// Incoming/deposit branch.
    External,
    /// Change/internal branch.
    Internal,
}

impl From<FederatedWalletKeychain> for KeychainKind {
    fn from(keychain: FederatedWalletKeychain) -> Self {
        match keychain {
            FederatedWalletKeychain::External => Self::External,
            FederatedWalletKeychain::Internal => Self::Internal,
        }
    }
}

/// Member-local storage and chain-source configuration.
#[derive(Clone)]
pub struct FederatedBdkLocalConfig {
    /// Local chain source. Its endpoint and credentials never enter consensus.
    pub chain_source: ChainSource,
    /// Parent directory for federation-only wallet namespaces.
    pub storage_root: PathBuf,
    /// Delay between independent local synchronization attempts.
    pub sync_interval: Duration,
    /// Opaque local source identity and operational independence classification.
    pub observation_source: FederatedChainSourceDescriptor,
}

impl fmt::Debug for FederatedBdkLocalConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBdkLocalConfig")
            .field("source", &self.chain_source.kind())
            .field("storage_root", &"<redacted>")
            .field("sync_interval", &self.sync_interval)
            .field("source_scope", &self.observation_source.scope)
            .finish()
    }
}

/// Public consensus identity of one federated descriptor wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederatedWalletScope {
    federation_id: [u8; 32],
    application_id: [u8; 32],
    root_key_epoch: u64,
    application_key_epoch: u64,
    descriptor_commitment: [u8; 32],
}

impl FederatedWalletScope {
    /// Construct the immutable namespace scope accepted from federation config.
    pub fn new(
        federation_id: [u8; 32],
        application_id: [u8; 32],
        root_key_epoch: u64,
        application_key_epoch: u64,
        descriptor_commitment: [u8; 32],
    ) -> Result<Self, Error> {
        if federation_id == [0; 32]
            || application_id == [0; 32]
            || root_key_epoch == 0
            || application_key_epoch == 0
            || descriptor_commitment == [0; 32]
        {
            return Err(Error::InvalidConfig(
                "federated wallet scope contains an empty identity or epoch".to_string(),
            ));
        }
        Ok(Self {
            federation_id,
            application_id,
            root_key_epoch,
            application_key_epoch,
            descriptor_commitment,
        })
    }

    fn namespace(self) -> String {
        let mut bytes = Vec::with_capacity(2 + 32 + 32 + 8 + 8 + 32);
        bytes.extend_from_slice(&FEDERATED_WALLET_NAMESPACE_VERSION.to_be_bytes());
        bytes.extend_from_slice(&self.federation_id);
        bytes.extend_from_slice(&self.application_id);
        bytes.extend_from_slice(&self.root_key_epoch.to_be_bytes());
        bytes.extend_from_slice(&self.application_key_epoch.to_be_bytes());
        bytes.extend_from_slice(&self.descriptor_commitment);
        let mut domain_bytes =
            Vec::with_capacity(FEDERATED_WALLET_NAMESPACE_DOMAIN.len() + bytes.len());
        domain_bytes.extend_from_slice(FEDERATED_WALLET_NAMESPACE_DOMAIN);
        domain_bytes.extend_from_slice(&bytes);
        format!(
            "cdk-bdk/federation/v1/{}",
            sha256::Hash::hash(&domain_bytes)
        )
    }

    fn directory_name(self) -> String {
        self.namespace()
            .rsplit('/')
            .next()
            .expect("namespace always has a digest segment")
            .to_string()
    }

    /// Registered application identifier used to select consensus state.
    #[must_use]
    pub fn application_id(self) -> [u8; 32] {
        self.application_id
    }

    /// FROST root epoch used by this wallet namespace.
    #[must_use]
    pub fn root_key_epoch(self) -> u64 {
        self.root_key_epoch
    }

    /// Descriptor application epoch used to select consensus state.
    #[must_use]
    pub fn application_key_epoch(self) -> u64 {
        self.application_key_epoch
    }
}

/// Accepted public descriptors and network for one federated BDK instance.
#[derive(Clone)]
pub struct FederatedWalletConfig {
    scope: FederatedWalletScope,
    descriptors: DescriptorConfig,
}

impl fmt::Debug for FederatedWalletConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedWalletConfig")
            .field("scope", &self.scope)
            .field("network", &self.descriptors.network())
            .field("descriptors", &"<public descriptors redacted>")
            .finish()
    }
}

impl FederatedWalletConfig {
    /// Parse the accepted public descriptor pair and bind it to its scope.
    pub fn new(
        scope: FederatedWalletScope,
        external_descriptor: impl AsRef<str>,
        internal_descriptor: impl AsRef<str>,
        network: Network,
    ) -> Result<Self, Error> {
        Ok(Self {
            scope,
            descriptors: DescriptorConfig::new(external_descriptor, internal_descriptor, network)?,
        })
    }

    /// Consensus Bitcoin network.
    #[must_use]
    pub fn network(&self) -> Network {
        self.descriptors.network()
    }

    /// Immutable consensus scope for this public descriptor pair.
    #[must_use]
    pub fn scope(&self) -> FederatedWalletScope {
        self.scope
    }
}

/// Whether the local chain view may be used for a consensus-dependent action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedWalletReadiness {
    /// No successful synchronization has completed.
    Unsynced,
    /// The latest synchronization failed, so the local view is not actionable.
    Degraded,
    /// The local checkpoint is below the caller's required consensus height.
    BehindConsensus,
    /// The local checkpoint satisfies the caller's required height.
    Ready,
}

/// Bounded, non-sensitive synchronization health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedWalletSyncHealth {
    /// A sync has not succeeded yet.
    NeverSynced,
    /// The most recent attempt succeeded.
    Healthy,
    /// The most recent attempt failed; persisted wallet state remains intact.
    Degraded,
}

/// Public operational snapshot without endpoints, credentials, or scripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedWalletSyncStatus {
    /// Chain-source implementation.
    pub source: ChainSourceKind,
    /// Current local health.
    pub health: FederatedWalletSyncHealth,
    /// Last locally persisted checkpoint height.
    pub local_tip_height: u32,
    /// Last locally persisted checkpoint hash.
    pub local_tip_hash: String,
    /// Unix timestamp of the last successful pass, when available.
    pub last_success_unix: Option<u64>,
    /// Saturating number of consecutive failures.
    pub consecutive_failures: u32,
}

/// Confirmed block evidence from one member's local BDK graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederatedWalletOutputConfirmation {
    /// Block containing the output transaction.
    pub block_hash: BlockHash,
    /// Height of the containing block.
    pub block_height: u32,
    /// Confirmation depth relative to the local persisted tip.
    pub confirmation_depth: u32,
}

/// Confirmed location of one exact federated wallet transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederatedTransactionConfirmation {
    /// Block containing the transaction.
    pub block_hash: BlockHash,
    /// Height of the containing block.
    pub block_height: u32,
    /// Depth relative to the locally persisted tip.
    pub confirmation_depth: u32,
}

/// One descriptor-owned output observed in the member-local BDK graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederatedWalletOutput {
    /// Descriptor branch that recognized the output.
    pub keychain: FederatedWalletKeychain,
    /// Non-hardened descriptor child index.
    pub derivation_index: u32,
    /// Transaction output identifier.
    pub outpoint: OutPoint,
    /// Exact value and script.
    pub txout: TxOut,
    /// Block evidence, or `None` while unconfirmed.
    pub confirmation: Option<FederatedWalletOutputConfirmation>,
}

/// One member's isolated watch-only BDK wallet.
#[derive(Clone)]
pub struct FederatedBdkWallet {
    engine: BdkWalletEngine,
    scope: FederatedWalletScope,
    status: Arc<RwLock<FederatedWalletSyncStatus>>,
    observation_source: FederatedChainSourceDescriptor,
    #[allow(dead_code)]
    database_dir: Arc<PathBuf>,
}

impl fmt::Debug for FederatedBdkWallet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBdkWallet")
            .field("network", &self.engine.network())
            .field("database_dir", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl FederatedBdkWallet {
    /// Open the accepted public wallet in a federation-only database namespace.
    pub fn open(
        config: FederatedWalletConfig,
        chain_source: ChainSource,
        storage_root: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        let source_id = chain_source.opaque_source_id();
        let observation_source =
            FederatedChainSourceDescriptor::new(FederatedChainSourceScope::Independent, source_id)?;
        Self::open_with_observation_source(config, chain_source, storage_root, observation_source)
    }

    /// Open with an explicit redacted chain-source diagnostic classification.
    pub fn open_with_observation_source(
        config: FederatedWalletConfig,
        chain_source: ChainSource,
        storage_root: impl AsRef<Path>,
        observation_source: FederatedChainSourceDescriptor,
    ) -> Result<Self, Error> {
        let scope = config.scope;
        let namespace = scope.namespace();
        let database_dir = storage_root
            .as_ref()
            .join(FEDERATED_WALLET_DIRECTORY)
            .join(config.scope.directory_name());
        let source = chain_source.kind();
        let engine = BdkWalletEngine::open_federated_watch_only(
            config.descriptors,
            chain_source,
            &database_dir,
            &namespace,
        )?;
        Ok(Self {
            engine,
            scope,
            status: Arc::new(RwLock::new(FederatedWalletSyncStatus {
                source,
                health: FederatedWalletSyncHealth::NeverSynced,
                local_tip_height: 0,
                local_tip_hash: String::new(),
                last_success_unix: None,
                consecutive_failures: 0,
            })),
            observation_source,
            database_dir: Arc::new(database_dir),
        })
    }

    /// Underlying watch-only engine for bounded wallet operations.
    #[must_use]
    pub fn engine(&self) -> &BdkWalletEngine {
        &self.engine
    }

    /// Immutable consensus scope selecting this wallet.
    #[must_use]
    pub fn scope(&self) -> FederatedWalletScope {
        self.scope
    }

    /// Redacted source identity attached to this member's observations.
    #[must_use]
    pub fn observation_source(&self) -> &FederatedChainSourceDescriptor {
        &self.observation_source
    }

    /// Persist every accepted external and internal script through the supplied
    /// max-plus-one counters. Repeating the same counters is idempotent.
    pub async fn reconcile_revealed_counters(
        &self,
        external_next_index: u64,
        internal_next_index: u64,
    ) -> Result<(), Error> {
        for (keychain, next_index) in [
            (KeychainKind::External, external_next_index),
            (KeychainKind::Internal, internal_next_index),
        ] {
            if next_index == 0 {
                continue;
            }
            let target_index = u32::try_from(next_index - 1).map_err(|_| {
                Error::InvalidConfig("federated reveal counter exceeds u32".to_string())
            })?;
            self.engine
                .reveal_addresses_to(keychain, target_index)
                .await?;
        }
        Ok(())
    }

    /// Reveal and return one accepted address. Consensus must assign the index
    /// before this method is called.
    pub async fn reveal_address(
        &self,
        keychain: FederatedWalletKeychain,
        index: u32,
    ) -> Result<String, Error> {
        let keychain = KeychainKind::from(keychain);
        self.engine.reveal_addresses_to(keychain, index).await?;
        let wallet = self.engine.wallet_with_db.lock().await;
        Ok(wallet
            .wallet
            .peek_address(keychain, index)
            .address
            .to_string())
    }

    /// Synchronize once. Failure leaves the last persisted chain graph intact.
    pub async fn sync_once(&self) -> Result<(), Error> {
        match self.engine.sync_once().await {
            Ok(()) => {
                let tip = self.engine.local_chain_tip().await;
                let last_success_unix = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs());
                let mut status = self.status.write().await;
                status.health = FederatedWalletSyncHealth::Healthy;
                status.local_tip_height = tip.height;
                status.local_tip_hash = tip.hash.to_string();
                status.last_success_unix = last_success_unix;
                status.consecutive_failures = 0;
                Ok(())
            }
            Err(error) => {
                let mut status = self.status.write().await;
                status.health = FederatedWalletSyncHealth::Degraded;
                status.consecutive_failures = status
                    .consecutive_failures
                    .saturating_add(1)
                    .min(MAX_SYNC_FAILURES);
                Err(error)
            }
        }
    }

    /// Return a bounded public health snapshot.
    pub async fn sync_status(&self) -> FederatedWalletSyncStatus {
        self.status.read().await.clone()
    }

    /// Evaluate readiness against a consensus-required block height.
    pub async fn readiness(&self, required_height: u32) -> FederatedWalletReadiness {
        let status = self.status.read().await;
        match status.health {
            FederatedWalletSyncHealth::NeverSynced => FederatedWalletReadiness::Unsynced,
            FederatedWalletSyncHealth::Degraded => FederatedWalletReadiness::Degraded,
            _ if status.local_tip_height < required_height => {
                FederatedWalletReadiness::BehindConsensus
            }
            _ => FederatedWalletReadiness::Ready,
        }
    }

    /// Fail closed when the local chain view is behind a required height.
    pub async fn ensure_ready(&self, required_height: u32) -> Result<(), Error> {
        match self.readiness(required_height).await {
            FederatedWalletReadiness::Ready => Ok(()),
            FederatedWalletReadiness::Unsynced
            | FederatedWalletReadiness::Degraded
            | FederatedWalletReadiness::BehindConsensus => Err(Error::FederatedWalletNotReady),
        }
    }

    /// Derive one script without advancing the persisted reveal index.
    pub async fn peek_script(&self, keychain: KeychainKind, index: u32) -> ScriptBuf {
        let wallet = self.engine.wallet_with_db.lock().await;
        wallet
            .wallet
            .peek_address(keychain, index)
            .address
            .script_pubkey()
    }

    /// Derive one script through the federation-specific keychain boundary.
    pub async fn peek_federated_script(
        &self,
        keychain: FederatedWalletKeychain,
        index: u32,
    ) -> ScriptBuf {
        self.peek_script(keychain.into(), index).await
    }

    /// Return all descriptor-owned outputs with bounded local chain evidence.
    pub async fn observed_outputs(&self) -> Vec<FederatedWalletOutput> {
        let wallet = self.engine.wallet_with_db.lock().await;
        let tip_height = wallet.wallet.latest_checkpoint().height();
        wallet
            .wallet
            .list_output()
            .map(|output| {
                let confirmation = match output.chain_position {
                    bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. } => {
                        Some(FederatedWalletOutputConfirmation {
                            block_hash: anchor.block_id.hash,
                            block_height: anchor.block_id.height,
                            confirmation_depth: tip_height
                                .saturating_sub(anchor.block_id.height)
                                .saturating_add(1),
                        })
                    }
                    bdk_wallet::chain::ChainPosition::Unconfirmed { .. } => None,
                };
                FederatedWalletOutput {
                    keychain: match output.keychain {
                        KeychainKind::External => FederatedWalletKeychain::External,
                        KeychainKind::Internal => FederatedWalletKeychain::Internal,
                    },
                    derivation_index: output.derivation_index,
                    outpoint: output.outpoint,
                    txout: output.txout,
                    confirmation,
                }
            })
            .collect()
    }

    /// Return local confirmed-chain evidence for an exact transaction.
    pub async fn transaction_confirmation(
        &self,
        txid: Txid,
    ) -> Option<FederatedTransactionConfirmation> {
        let wallet = self.engine.wallet_with_db.lock().await;
        let transaction = wallet.wallet.get_tx(txid)?;
        let bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. } = transaction.chain_position
        else {
            return None;
        };
        let tip_height = wallet.wallet.latest_checkpoint().height();
        Some(FederatedTransactionConfirmation {
            block_hash: anchor.block_id.hash,
            block_height: anchor.block_id.height,
            confirmation_depth: tip_height
                .saturating_sub(anchor.block_id.height)
                .saturating_add(1),
        })
    }

    #[cfg(test)]
    fn database_dir(&self) -> &Path {
        self.database_dir.as_path()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bdk_wallet::bitcoin::bip32::{Xpriv, Xpub};
    use bdk_wallet::bitcoin::secp256k1::Secp256k1;
    use bdk_wallet::bitcoin::{
        absolute, transaction, Amount, Network, OutPoint, Sequence, Transaction, TxIn, TxOut,
        Witness,
    };
    use bdk_wallet::rusqlite::Connection;
    use bdk_wallet::template::Bip84;

    use super::*;
    use crate::{BitcoinRpcConfig, EsploraConfig};

    fn scope(seed: u8) -> FederatedWalletScope {
        FederatedWalletScope::new([seed; 32], [seed + 1; 32], 1, 1, [seed + 2; 32])
            .expect("valid scope")
    }

    fn descriptor_strings(seed: u8) -> (String, String) {
        let secp = Secp256k1::new();
        let xpriv = Xpriv::new_master(Network::Regtest, &[seed; 32]).expect("master key");
        let xpub = Xpub::from_priv(&secp, &xpriv);
        (format!("tr({xpub}/0/*)"), format!("tr({xpub}/1/*)"))
    }

    fn config(seed: u8) -> FederatedWalletConfig {
        let (external, internal) = descriptor_strings(seed);
        FederatedWalletConfig::new(scope(seed), external, internal, Network::Regtest)
            .expect("federated wallet config")
    }

    fn esplora_source() -> ChainSource {
        ChainSource::Esplora(EsploraConfig {
            url: "http://127.0.0.1:1/private-token".to_string(),
            parallel_requests: 1,
        })
    }

    fn bitcoin_rpc_source() -> ChainSource {
        ChainSource::BitcoinRpc(BitcoinRpcConfig {
            host: "127.0.0.1".to_string(),
            port: 1,
            user: "private-user".to_string(),
            password: "private-password".to_string(),
        })
    }

    #[tokio::test]
    async fn five_members_with_mixed_sources_converge_on_the_same_chain() {
        let mut roots = Vec::new();
        let mut wallets = Vec::new();
        for member in 0..5 {
            let root = tempfile::tempdir().expect("tempdir");
            let source = if member % 2 == 0 {
                esplora_source()
            } else {
                bitcoin_rpc_source()
            };
            wallets.push(
                FederatedBdkWallet::open(config(10), source, root.path()).expect("member wallet"),
            );
            roots.push(root);
        }

        for keychain in [KeychainKind::External, KeychainKind::Internal] {
            for index in [0, 1, 17, 1_000] {
                let expected = wallets[0].peek_script(keychain, index).await;
                for wallet in &wallets[1..] {
                    assert_eq!(wallet.peek_script(keychain, index).await, expected);
                }
            }
        }
        let funding_script = wallets[0].peek_script(KeychainKind::External, 0).await;
        let funding = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: Vec::new(),
            output: vec![TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: funding_script,
            }],
        };
        for wallet in &wallets {
            wallet
                .engine()
                .reveal_addresses_to(KeychainKind::External, 0)
                .await
                .expect("reveal funding script");
            wallet
                .engine()
                .apply_accepted_transaction(funding.clone(), 1)
                .await
                .expect("apply common chain transaction");
            let unspent = wallet.engine().list_unspent().await;
            assert_eq!(unspent.len(), 1);
            assert_eq!(unspent[0].txout.value.to_sat(), 50_000);
            let observed = wallet.observed_outputs().await;
            assert_eq!(observed.len(), 1);
            assert_eq!(observed[0].keychain, FederatedWalletKeychain::External);
            assert_eq!(observed[0].derivation_index, 0);
            assert_eq!(observed[0].txout.value.to_sat(), 50_000);
            assert_eq!(observed[0].confirmation, None);
            assert_eq!(
                wallet
                    .transaction_confirmation(funding.compute_txid())
                    .await,
                None,
                "locally injected unconfirmed bytes are not confirmation evidence"
            );
        }
        assert_eq!(
            wallets[0].sync_status().await.source,
            ChainSourceKind::Esplora
        );
        assert_eq!(
            wallets[1].sync_status().await.source,
            ChainSourceKind::BitcoinRpc
        );
        assert!(wallets[0].sync_once().await.is_err());
        assert!(wallets[1].sync_once().await.is_err());
        assert_eq!(
            wallets[0].sync_status().await.health,
            FederatedWalletSyncHealth::Degraded
        );
        assert_eq!(
            wallets[1].sync_status().await.health,
            FederatedWalletSyncHealth::Degraded
        );
        assert_eq!(wallets[0].engine().list_unspent().await.len(), 1);
        assert_eq!(wallets[1].engine().list_unspent().await.len(), 1);
        drop(roots);
    }

    #[tokio::test]
    async fn offline_member_replays_address_gaps_and_multiple_spends_idempotently() {
        let roots = (0..5)
            .map(|_| tempfile::tempdir().expect("tempdir"))
            .collect::<Vec<_>>();
        let wallets = roots
            .iter()
            .map(|root| {
                FederatedBdkWallet::open(config(11), esplora_source(), root.path())
                    .expect("member wallet")
            })
            .collect::<Vec<_>>();
        let external = wallets[0].peek_script(KeychainKind::External, 7).await;
        let internal = wallets[0].peek_script(KeychainKind::Internal, 5).await;
        let final_script = wallets[0].peek_script(KeychainKind::External, 6).await;
        let funding = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: Vec::new(),
            output: vec![TxOut {
                value: Amount::from_sat(60_000),
                script_pubkey: external,
            }],
        };
        let first_spend = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(funding.compute_txid(), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: internal,
            }],
        };
        let second_spend = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(first_spend.compute_txid(), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(40_000),
                script_pubkey: final_script.clone(),
            }],
        };
        let accepted = [funding, first_spend, second_spend];

        for wallet in &wallets[..4] {
            wallet
                .reconcile_revealed_counters(8, 6)
                .await
                .expect("online counter reconciliation");
            for (offset, transaction) in accepted.iter().enumerate() {
                wallet
                    .engine()
                    .apply_accepted_transaction(
                        transaction.clone(),
                        u64::try_from(offset + 1).expect("accepted index"),
                    )
                    .await
                    .expect("online accepted transaction");
            }
        }
        assert!(wallets[4].engine().list_unspent().await.is_empty());

        wallets[4]
            .reconcile_revealed_counters(8, 6)
            .await
            .expect("offline counter catch-up");
        for _ in 0..2 {
            for (offset, transaction) in accepted.iter().enumerate() {
                wallets[4]
                    .engine()
                    .apply_accepted_transaction(
                        transaction.clone(),
                        u64::try_from(offset + 1).expect("accepted index"),
                    )
                    .await
                    .expect("idempotent transaction catch-up");
            }
        }
        for wallet in &wallets {
            let unspent = wallet.engine().list_unspent().await;
            assert_eq!(unspent.len(), 1);
            assert_eq!(
                unspent[0].outpoint,
                OutPoint::new(accepted[2].compute_txid(), 0)
            );
            assert_eq!(unspent[0].txout.value.to_sat(), 40_000);
            assert_eq!(unspent[0].txout.script_pubkey, final_script);
        }

        drop(wallets);
        let restarted = FederatedBdkWallet::open(config(11), bitcoin_rpc_source(), roots[4].path())
            .expect("restarted caught-up member");
        restarted
            .reconcile_revealed_counters(8, 6)
            .await
            .expect("restart counter reconciliation");
        for (offset, transaction) in accepted.iter().enumerate() {
            restarted
                .engine()
                .apply_accepted_transaction(
                    transaction.clone(),
                    u64::try_from(offset + 1).expect("accepted index"),
                )
                .await
                .expect("restart transaction reconciliation");
        }
        let unspent = restarted.engine().list_unspent().await;
        assert_eq!(unspent.len(), 1);
        assert_eq!(
            unspent[0].outpoint,
            OutPoint::new(accepted[2].compute_txid(), 0)
        );
    }

    #[tokio::test]
    async fn restart_restore_and_descriptor_mismatch_are_fail_closed() {
        let root = tempfile::tempdir().expect("tempdir");
        let wallet = FederatedBdkWallet::open(config(20), esplora_source(), root.path())
            .expect("initial wallet");
        let expected = wallet.peek_script(KeychainKind::External, 42).await;
        wallet
            .engine()
            .reveal_addresses_to(KeychainKind::External, 42)
            .await
            .expect("reveal addresses");
        drop(wallet);

        let restarted = FederatedBdkWallet::open(config(20), bitcoin_rpc_source(), root.path())
            .expect("restart with another local source");
        assert_eq!(
            restarted.peek_script(KeychainKind::External, 42).await,
            expected
        );
        assert!(restarted
            .engine()
            .reveal_addresses_to(KeychainKind::External, 42)
            .await
            .expect("idempotent reveal")
            .is_empty());
        drop(restarted);

        let (wrong_external, wrong_internal) = descriptor_strings(21);
        let wrong =
            FederatedWalletConfig::new(scope(20), wrong_external, wrong_internal, Network::Regtest)
                .expect("parse wrong descriptors");
        assert!(FederatedBdkWallet::open(wrong, esplora_source(), root.path()).is_err());

        let restored_root = tempfile::tempdir().expect("restored tempdir");
        let restored = FederatedBdkWallet::open(config(20), esplora_source(), restored_root.path())
            .expect("empty database restore");
        assert_eq!(
            restored.peek_script(KeychainKind::External, 42).await,
            expected
        );
    }

    #[tokio::test]
    async fn consensus_counters_reveal_gaps_idempotently_across_restart() {
        let root = tempfile::tempdir().expect("tempdir");
        let wallet = FederatedBdkWallet::open(config(25), esplora_source(), root.path())
            .expect("initial wallet");
        let expected_external = wallet.peek_script(KeychainKind::External, 6).await;
        let expected_internal = wallet.peek_script(KeychainKind::Internal, 3).await;
        wallet
            .reconcile_revealed_counters(7, 4)
            .await
            .expect("reconcile accepted counters");
        assert_eq!(
            wallet
                .reveal_address(FederatedWalletKeychain::External, 6)
                .await
                .expect("accepted external address"),
            bdk_wallet::bitcoin::Address::from_script(
                expected_external.as_script(),
                Network::Regtest,
            )
            .expect("external address")
            .to_string()
        );
        drop(wallet);

        let restarted = FederatedBdkWallet::open(config(25), bitcoin_rpc_source(), root.path())
            .expect("restarted wallet");
        restarted
            .reconcile_revealed_counters(7, 4)
            .await
            .expect("idempotent restart reconciliation");
        assert_eq!(
            restarted.peek_script(KeychainKind::External, 6).await,
            expected_external
        );
        assert_eq!(
            restarted.peek_script(KeychainKind::Internal, 3).await,
            expected_internal
        );
        assert!(restarted
            .engine()
            .reveal_addresses_to(KeychainKind::External, 6)
            .await
            .expect("already revealed external range")
            .is_empty());
        assert!(restarted
            .engine()
            .reveal_addresses_to(KeychainKind::Internal, 3)
            .await
            .expect("already revealed internal range")
            .is_empty());
    }

    #[tokio::test]
    async fn unavailable_source_degrades_health_without_corrupting_wallet() {
        let root = tempfile::tempdir().expect("tempdir");
        let wallet = FederatedBdkWallet::open(config(30), esplora_source(), root.path())
            .expect("federated wallet");
        let expected = wallet.peek_script(KeychainKind::External, 7).await;
        assert_eq!(
            wallet.readiness(0).await,
            FederatedWalletReadiness::Unsynced
        );
        assert!(wallet.sync_once().await.is_err());
        let status = wallet.sync_status().await;
        assert_eq!(status.health, FederatedWalletSyncHealth::Degraded);
        assert_eq!(status.consecutive_failures, 1);
        assert_eq!(status.last_success_unix, None);
        assert_eq!(
            wallet.peek_script(KeychainKind::External, 7).await,
            expected
        );
        assert!(matches!(
            wallet.ensure_ready(0).await,
            Err(Error::FederatedWalletNotReady)
        ));
        drop(wallet);

        let restarted = FederatedBdkWallet::open(config(30), esplora_source(), root.path())
            .expect("uncorrupted restart");
        assert_eq!(
            restarted.peek_script(KeychainKind::External, 7).await,
            expected
        );
    }

    #[test]
    fn federation_database_is_namespaced_and_contains_no_signing_material() {
        let root = tempfile::tempdir().expect("tempdir");
        let wallet = FederatedBdkWallet::open(config(40), esplora_source(), root.path())
            .expect("federated wallet");
        let database_path = wallet.database_dir().join("bdk_wallet.sqlite");
        let database = Connection::open(database_path).expect("open database");
        let (external, internal): (String, String) = database
            .query_row(
                "SELECT descriptor, change_descriptor FROM bdk_wallet WHERE id = 0",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("wallet descriptors");
        let persisted = format!("{external}\n{internal}").to_ascii_lowercase();
        assert!(!persisted.contains("prv"));
        assert!(!persisted.contains("mnemonic"));
        assert!(!persisted.contains("private-password"));
        drop(database);

        let generic_root = tempfile::tempdir().expect("generic tempdir");
        let generic_config = config(40).descriptors;
        BdkWalletEngine::open_watch_only(
            generic_config.clone(),
            esplora_source(),
            generic_root.path(),
        )
        .expect("generic watch-only wallet");
        assert!(matches!(
            BdkWalletEngine::open_federated_watch_only(
                generic_config,
                esplora_source(),
                generic_root.path(),
                &scope(40).namespace(),
            ),
            Err(Error::WalletNamespaceMismatch)
        ));

        let local_root = tempfile::tempdir().expect("local tempdir");
        let xpriv = Xpriv::from_str(
            "tprv8ZgxMBicQKsPdZVA7mmJ25Z7v4weN6qmGa7t6cRBdY8icQnJvQz4aXcJ3k37d41Yw52fLQW31A8XEv9vCHm9dJfXoWQG4GduS9rNQGCSkDq",
        )
        .unwrap_or_else(|_| Xpriv::new_master(Network::Regtest, &[41; 32]).expect("master key"));
        BdkWalletEngine::open_local_signing(
            Bip84(xpriv, KeychainKind::External),
            Bip84(xpriv, KeychainKind::Internal),
            esplora_source(),
            local_root.path(),
            Network::Regtest,
        )
        .expect("local wallet");
        let public = config(41).descriptors;
        assert!(matches!(
            BdkWalletEngine::open_federated_watch_only(
                public,
                esplora_source(),
                local_root.path(),
                &scope(41).namespace(),
            ),
            Err(Error::WalletNamespaceMismatch)
        ));
    }

    #[test]
    fn debug_and_status_redact_local_configuration() {
        let root = tempfile::tempdir().expect("tempdir");
        let wallet = FederatedBdkWallet::open(config(50), bitcoin_rpc_source(), root.path())
            .expect("federated wallet");
        let debug = format!("{wallet:?}");
        assert!(!debug.contains("private-user"));
        assert!(!debug.contains("private-password"));
        assert!(!debug.contains(&root.path().to_string_lossy().to_string()));
        assert!(!format!("{:?}", bitcoin_rpc_source()).contains("private-password"));
        assert!(!format!("{:?}", esplora_source()).contains("private-token"));
    }
}
