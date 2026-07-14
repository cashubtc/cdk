//! Reusable persisted BDK wallet engine.
//!
//! This module owns descriptor validation, the BDK transaction graph, PSBT
//! construction/finalization, and chain submission. It deliberately does not
//! own Cashu payment intents or federation consensus state.

use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use bdk_wallet::bitcoin::key::TapTweak;
use bdk_wallet::bitcoin::psbt::{Psbt, PsbtSighashType};
use bdk_wallet::bitcoin::secp256k1::{schnorr, Message, Secp256k1};
use bdk_wallet::bitcoin::sighash::{Prevouts, SighashCache};
use bdk_wallet::bitcoin::{
    Amount, FeeRate, Network, OutPoint, ScriptBuf, Sequence, TapSighashType, Transaction, TxOut,
};
use bdk_wallet::descriptor::{ExtendedDescriptor, IntoWalletDescriptor};
use bdk_wallet::rusqlite::{params, Connection, OptionalExtension};
use bdk_wallet::{KeychainKind, PersistedWallet, SignOptions, Utxo, Wallet, WeightedUtxo};
use tokio::sync::Mutex;

use crate::chain::{BroadcastFailure, BroadcastOutcome};
use crate::{ChainSource, Error, WalletWithDb};

/// Public external and internal descriptors for a watch-only BDK wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorConfig {
    external: ExtendedDescriptor,
    internal: ExtendedDescriptor,
    network: Network,
}

/// A confirmed output observed in the local BDK transaction graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedWalletOutput {
    /// Descriptor keychain that owns the output.
    pub keychain: KeychainKind,
    /// Derived address for the output script.
    pub address: String,
    /// Transaction identifier.
    pub txid: String,
    /// Serialized outpoint identifier.
    pub outpoint: String,
    /// Output value in satoshis.
    pub amount_sat: u64,
    /// Block height anchoring the transaction.
    pub block_height: u32,
}

impl DescriptorConfig {
    /// Parse a pair of public descriptors for `network`.
    ///
    /// Secret descriptor keys are rejected by construction because both
    /// descriptors must parse as [`ExtendedDescriptor`].
    pub fn new(
        external: impl AsRef<str>,
        internal: impl AsRef<str>,
        network: Network,
    ) -> Result<Self, Error> {
        let external = ExtendedDescriptor::from_str(external.as_ref())
            .map_err(|err| Error::InvalidDescriptor(err.to_string()))?;
        let internal = ExtendedDescriptor::from_str(internal.as_ref())
            .map_err(|err| Error::InvalidDescriptor(err.to_string()))?;

        if external == internal {
            return Err(Error::InvalidDescriptor(
                "external and internal descriptors must differ".to_string(),
            ));
        }

        Ok(Self {
            external,
            internal,
            network,
        })
    }

    /// External receive descriptor.
    pub fn external(&self) -> &ExtendedDescriptor {
        &self.external
    }

    /// Internal change descriptor.
    pub fn internal(&self) -> &ExtendedDescriptor {
        &self.internal
    }

    /// Bitcoin network bound to both descriptors.
    pub fn network(&self) -> Network {
        self.network
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SigningMode {
    Local,
    ExternalOnly,
}

const WALLET_NAMESPACE_TABLE: &str = "cdk_bdk_wallet_namespace";
const LOCAL_SIGNING_NAMESPACE: &str = "cdk-bdk/local-signing/v1";
const WATCH_ONLY_NAMESPACE: &str = "cdk-bdk/watch-only/v1";

/// Reusable persisted BDK engine with an explicit signing boundary.
#[derive(Clone)]
pub struct BdkWalletEngine {
    pub(crate) wallet_with_db: Arc<Mutex<WalletWithDb>>,
    pub(crate) chain_source: ChainSource,
    network: Network,
    signing_mode: SigningMode,
}

impl BdkWalletEngine {
    pub(crate) fn open_local_signing<D>(
        external: D,
        internal: D,
        chain_source: ChainSource,
        database_dir: impl AsRef<Path>,
        network: Network,
    ) -> Result<Self, Error>
    where
        D: IntoWalletDescriptor + Clone + Send + 'static,
    {
        chain_source.validate()?;
        fs::create_dir_all(database_dir.as_ref())?;
        let mut db = Connection::open(database_dir.as_ref().join("bdk_wallet.sqlite"))?;
        ensure_wallet_namespace(&mut db, LOCAL_SIGNING_NAMESPACE, false)?;
        let wallet = Wallet::load()
            .descriptor(KeychainKind::External, Some(external.clone()))
            .descriptor(KeychainKind::Internal, Some(internal.clone()))
            .extract_keys()
            .check_network(network)
            .load_wallet(&mut db)
            .map_err(|err| Error::Wallet(err.to_string()))?;
        let initial_checkpoint = match wallet.is_none() {
            true => chain_source.initial_checkpoint()?,
            false => None,
        };
        let mut wallet = match wallet {
            Some(wallet) => wallet,
            None => Wallet::create(external, internal)
                .network(network)
                .create_wallet(&mut db)
                .map_err(|err| Error::Wallet(err.to_string()))?,
        };
        if let Some(block_id) = initial_checkpoint {
            let checkpoint = wallet.latest_checkpoint().insert(block_id);
            wallet.apply_update(bdk_wallet::Update {
                chain: Some(checkpoint),
                ..Default::default()
            }).map_err(|err| Error::Wallet(err.to_string()))?;
        }
        wallet.persist(&mut db)?;

        Ok(Self {
            wallet_with_db: Arc::new(Mutex::new(WalletWithDb::new(wallet, db))),
            chain_source,
            network,
            signing_mode: SigningMode::Local,
        })
    }

    /// Create or load a watch-only wallet in `database_dir`.
    pub fn open_watch_only(
        config: DescriptorConfig,
        chain_source: ChainSource,
        database_dir: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        chain_source.validate()?;
        fs::create_dir_all(database_dir.as_ref())?;
        let mut db = Connection::open(database_dir.as_ref().join("bdk_wallet.sqlite"))?;
        ensure_wallet_namespace(&mut db, WATCH_ONLY_NAMESPACE, false)?;
        let wallet = Self::load_or_create_watch_only(&config, &mut db)?;

        Ok(Self {
            wallet_with_db: Arc::new(Mutex::new(WalletWithDb::new(wallet, db))),
            chain_source,
            network: config.network,
            signing_mode: SigningMode::ExternalOnly,
        })
    }

    /// Create or load a watch-only wallet in a mandatory federation namespace.
    ///
    /// Existing unmarked databases are rejected so a standalone or generic BDK
    /// database cannot be claimed by a federation process accidentally.
    pub fn open_federated_watch_only(
        config: DescriptorConfig,
        chain_source: ChainSource,
        database_dir: impl AsRef<Path>,
        namespace: &str,
    ) -> Result<Self, Error> {
        fs::create_dir_all(database_dir.as_ref())?;
        let mut db = Connection::open(database_dir.as_ref().join("bdk_wallet.sqlite"))?;
        ensure_wallet_namespace(&mut db, namespace, true)?;
        let wallet = Self::load_or_create_watch_only(&config, &mut db)?;

        Ok(Self {
            wallet_with_db: Arc::new(Mutex::new(WalletWithDb::new(wallet, db))),
            chain_source,
            network: config.network,
            signing_mode: SigningMode::ExternalOnly,
        })
    }

    fn load_or_create_watch_only(
        config: &DescriptorConfig,
        db: &mut Connection,
    ) -> Result<PersistedWallet<Connection>, Error> {
        let wallet = Wallet::load()
            .descriptor(KeychainKind::External, Some(config.external.clone()))
            .descriptor(KeychainKind::Internal, Some(config.internal.clone()))
            .check_network(config.network)
            .load_wallet(db)
            .map_err(|err| Error::Wallet(err.to_string()))?;

        let mut wallet = match wallet {
            Some(wallet) => wallet,
            None => Wallet::create(config.external.clone(), config.internal.clone())
                .network(config.network)
                .create_wallet(db)
                .map_err(|err| Error::Wallet(err.to_string()))?,
        };
        wallet.persist(db)?;
        Ok(wallet)
    }

    /// Network enforced by this wallet.
    pub fn network(&self) -> Network {
        self.network
    }

    /// Return the latest locally persisted BDK checkpoint.
    pub async fn local_chain_tip(&self) -> bdk_wallet::chain::BlockId {
        self.wallet_with_db
            .lock()
            .await
            .wallet
            .latest_checkpoint()
            .block_id()
    }

    /// Perform one wallet synchronization pass using the configured local
    /// chain source.
    pub async fn sync_once(&self) -> Result<(), Error> {
        self.chain_source.sync_engine_once(self).await
    }

    /// Reveal every address through `target_index` and persist atomically.
    ///
    /// Repeating an already-applied target is idempotent and returns no newly
    /// revealed addresses.
    pub async fn reveal_addresses_to(
        &self,
        keychain: KeychainKind,
        target_index: u32,
    ) -> Result<Vec<bdk_wallet::AddressInfo>, Error> {
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        let addresses = wallet_with_db
            .wallet
            .reveal_addresses_to(keychain, target_index)
            .collect::<Vec<_>>();
        wallet_with_db.persist()?;
        Ok(addresses)
    }

    /// Build and persist an unsigned PSBT without invoking a signer.
    pub async fn build_psbt(
        &self,
        recipients: &[(ScriptBuf, u64)],
        fee_rate: FeeRate,
    ) -> Result<Psbt, Error> {
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        let mut builder = wallet_with_db.wallet.build_tx();
        for (script, amount_sat) in recipients {
            builder.add_recipient(script.clone(), Amount::from_sat(*amount_sat));
        }
        builder.fee_rate(fee_rate);
        let psbt = builder
            .finish()
            .map_err(|err| Error::Wallet(err.to_string()))?;
        wallet_with_db.persist()?;
        Ok(psbt)
    }

    /// Build a watch-only federation PSBT from only consensus-spendable inputs.
    ///
    /// `change_script` must already have a consensus allocation. The builder is
    /// forbidden from selecting any other local UTXO or revealing a change
    /// address, and every input opts into the first-release RBF policy.
    pub async fn build_federated_psbt(
        &self,
        recipients: &[(ScriptBuf, u64)],
        fee_rate: FeeRate,
        change_script: ScriptBuf,
        allowed_outpoints: &[OutPoint],
    ) -> Result<Psbt, Error> {
        if recipients.is_empty() || allowed_outpoints.is_empty() || change_script.is_empty() {
            return Err(Error::Wallet(
                "federated PSBT requires recipients, inputs, and allocated change".to_string(),
            ));
        }
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        let mut builder = wallet_with_db.wallet.build_tx();
        for (script, amount_sat) in recipients {
            builder.add_recipient(script.clone(), Amount::from_sat(*amount_sat));
        }
        builder
            .add_utxos(allowed_outpoints)
            .map_err(|error| Error::Wallet(error.to_string()))?;
        builder
            .manually_selected_only()
            .drain_to(change_script)
            .fee_rate(fee_rate)
            .version(2)
            .set_exact_sequence(Sequence::ENABLE_RBF_NO_LOCKTIME);
        let psbt = builder
            .finish()
            .map_err(|error| Error::Wallet(error.to_string()))?;
        wallet_with_db.persist()?;
        Ok(psbt)
    }

    /// Return the wallet's currently unspent outputs.
    pub async fn list_unspent(&self) -> Vec<bdk_wallet::LocalOutput> {
        let wallet_with_db = self.wallet_with_db.lock().await;
        wallet_with_db.wallet.list_unspent().collect()
    }

    pub(crate) async fn fee_estimation_snapshot(
        &self,
        max_inputs: usize,
    ) -> Result<(Vec<WeightedUtxo>, ScriptBuf), Error> {
        let wallet_with_db = self.wallet_with_db.lock().await;
        let weighted_utxos = wallet_with_db
            .wallet
            .list_unspent()
            .take(max_inputs.max(1))
            .map(|utxo| {
                Ok(WeightedUtxo {
                    satisfaction_weight: wallet_with_db
                        .wallet
                        .public_descriptor(utxo.keychain)
                        .max_weight_to_satisfy()
                        .map_err(|err| Error::Wallet(err.to_string()))?,
                    utxo: Utxo::Local(utxo),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let change_script = wallet_with_db
            .wallet
            .peek_address(KeychainKind::Internal, 0)
            .address
            .script_pubkey();
        Ok((weighted_utxos, change_script))
    }

    /// Return address-form outputs confirmed in the local BDK graph.
    pub async fn confirmed_outputs(&self) -> Vec<ConfirmedWalletOutput> {
        let wallet_with_db = self.wallet_with_db.lock().await;
        wallet_with_db
            .wallet
            .list_output()
            .filter_map(|output| {
                let bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. } =
                    &output.chain_position
                else {
                    return None;
                };
                let address = bdk_wallet::bitcoin::Address::from_script(
                    output.txout.script_pubkey.as_script(),
                    self.network,
                )
                .ok()?;
                Some(ConfirmedWalletOutput {
                    keychain: output.keychain,
                    address: address.to_string(),
                    txid: output.outpoint.txid.to_string(),
                    outpoint: output.outpoint.to_string(),
                    amount_sat: output.txout.value.to_sat(),
                    block_height: anchor.block_id.height,
                })
            })
            .collect()
    }

    /// Return whether `txid` has at least `required` confirmations in the
    /// local BDK graph.
    pub async fn transaction_has_confirmations(
        &self,
        txid: bdk_wallet::bitcoin::Txid,
        required: u32,
    ) -> bool {
        let wallet_with_db = self.wallet_with_db.lock().await;
        let Some(transaction) = wallet_with_db.wallet.get_tx(txid) else {
            return false;
        };
        let bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. } = transaction.chain_position
        else {
            return false;
        };
        let tip_height = wallet_with_db.wallet.latest_checkpoint().height();
        tip_height >= anchor.block_id.height && tip_height - anchor.block_id.height + 1 >= required
    }

    pub(crate) async fn sign_psbt_locally(&self, psbt: &mut Psbt) -> Result<(), Error> {
        if self.signing_mode != SigningMode::Local {
            return Err(Error::LocalSigningDisabled);
        }

        let mut wallet_with_db = self.wallet_with_db.lock().await;
        let signed = wallet_with_db
            .wallet
            .sign(psbt, SignOptions::default())
            .map_err(|err| Error::Wallet(err.to_string()))?;
        if !signed {
            return Err(Error::CouldNotSign);
        }
        wallet_with_db.persist()?;
        Ok(())
    }

    /// Verify externally supplied Taproot key-path signatures and finalize.
    ///
    /// Only `SIGHASH_DEFAULT` key-path signatures are accepted by this narrow
    /// first-release boundary. The signature, tweaked output key, and spent
    /// script are checked before BDK is allowed to finalize the PSBT.
    pub async fn finalize_external_psbt(&self, psbt: &mut Psbt) -> Result<Transaction, Error> {
        let secp = Secp256k1::verification_only();
        let prevouts = psbt
            .inputs
            .iter()
            .enumerate()
            .map(|(index, _)| {
                psbt.spend_utxo(index)
                    .cloned()
                    .map_err(|err| Error::ExternalSignature(err.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut sighash_cache = SighashCache::new(&psbt.unsigned_tx);

        for (index, input) in psbt.inputs.iter().enumerate() {
            if !input.tap_scripts.is_empty() || !input.tap_script_sigs.is_empty() {
                return Err(Error::ExternalSignature(
                    "Taproot script-path signatures are unsupported".to_string(),
                ));
            }

            let signature = input.tap_key_sig.as_ref().ok_or_else(|| {
                Error::ExternalSignature(format!("missing Taproot signature for input {index}"))
            })?;
            if signature.sighash_type != TapSighashType::Default {
                return Err(Error::ExternalSignature(format!(
                    "input {index} must use SIGHASH_DEFAULT"
                )));
            }

            let internal_key = input.tap_internal_key.ok_or_else(|| {
                Error::ExternalSignature(format!("missing Taproot internal key for input {index}"))
            })?;
            let (output_key, _) = internal_key.tap_tweak(&secp, input.tap_merkle_root);
            let expected_script = ScriptBuf::new_p2tr_tweaked(output_key);
            if prevouts[index].script_pubkey != expected_script {
                return Err(Error::ExternalSignature(format!(
                    "Taproot key does not match spent output for input {index}"
                )));
            }

            let sighash = sighash_cache
                .taproot_key_spend_signature_hash(
                    index,
                    &Prevouts::All(&prevouts),
                    signature.sighash_type,
                )
                .map_err(|err| Error::ExternalSignature(err.to_string()))?;
            let message = Message::from(sighash);
            secp.verify_schnorr(
                &signature.signature,
                &message,
                output_key.as_x_only_public_key(),
            )
            .map_err(|err| Error::ExternalSignature(err.to_string()))?;
        }

        let wallet_with_db = self.wallet_with_db.lock().await;
        let finalized = wallet_with_db
            .wallet
            .finalize_psbt(psbt, SignOptions::default())
            .map_err(|err| Error::ExternalSignature(err.to_string()))?;
        if !finalized {
            return Err(Error::ExternalSignature(
                "BDK could not finalize externally signed PSBT".to_string(),
            ));
        }

        psbt.clone()
            .extract_tx()
            .map_err(|err| Error::ExternalSignature(err.to_string()))
    }

    /// Reconstruct the exact accepted watch-only PSBT and finalize FROST signatures.
    pub async fn finalize_federated_transaction(
        &self,
        unsigned_transaction: &Transaction,
        selected_inputs: &[(OutPoint, TxOut)],
        signatures: &[[u8; 64]],
    ) -> Result<Transaction, Error> {
        if selected_inputs.is_empty()
            || selected_inputs.len() != unsigned_transaction.input.len()
            || signatures.len() != selected_inputs.len()
        {
            return Err(Error::ExternalSignature(
                "accepted federation transaction is not eligible for finalization".to_string(),
            ));
        }
        let mut psbt = Psbt::from_unsigned_tx(unsigned_transaction.clone())
            .map_err(|error| Error::ExternalSignature(error.to_string()))?;
        {
            let wallet_with_db = self.wallet_with_db.lock().await;
            for (index, (outpoint, previous_output)) in selected_inputs.iter().enumerate() {
                if unsigned_transaction.input[index].previous_output != *outpoint {
                    return Err(Error::ExternalSignature(format!(
                        "accepted input {index} is out of transaction order"
                    )));
                }
                let local = wallet_with_db
                    .wallet
                    .list_unspent()
                    .find(|output| output.outpoint == *outpoint)
                    .ok_or_else(|| {
                        Error::ExternalSignature(format!(
                            "accepted input {index} is absent from the local BDK wallet"
                        ))
                    })?;
                let input = wallet_with_db
                    .wallet
                    .get_psbt_input(
                        local,
                        Some(PsbtSighashType::from(TapSighashType::Default)),
                        true,
                    )
                    .map_err(|error| Error::ExternalSignature(error.to_string()))?;
                if input.witness_utxo.as_ref() != Some(previous_output) {
                    return Err(Error::ExternalSignature(format!(
                        "accepted input {index} differs from the local BDK prevout"
                    )));
                }
                psbt.inputs[index] = input;
            }
        }
        for (index, signature) in signatures.iter().enumerate() {
            let signature = schnorr::Signature::from_slice(signature)
                .map_err(|error| Error::ExternalSignature(error.to_string()))?;
            psbt.inputs[index].tap_key_sig = Some(bdk_wallet::bitcoin::taproot::Signature {
                signature,
                sighash_type: TapSighashType::Default,
            });
        }
        let transaction = self.finalize_external_psbt(&mut psbt).await?;
        let mut unsigned = transaction.clone();
        for input in &mut unsigned.input {
            input.script_sig = ScriptBuf::new();
            input.witness.clear();
        }
        if unsigned != *unsigned_transaction {
            return Err(Error::ExternalSignature(
                "final transaction differs from accepted non-witness bytes".to_string(),
            ));
        }
        Ok(transaction)
    }

    /// Apply an accepted signed transaction to the local BDK graph.
    pub async fn apply_accepted_transaction(
        &self,
        transaction: Transaction,
        last_seen: u64,
    ) -> Result<(), Error> {
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        wallet_with_db
            .wallet
            .apply_unconfirmed_txs([(transaction, last_seen)]);
        wallet_with_db.persist()?;
        Ok(())
    }

    /// Broadcast an already finalized transaction through the configured
    /// local chain source.
    pub async fn broadcast_transaction(&self, transaction: Transaction) -> Result<(), Error> {
        self.broadcast_classified(transaction)
            .await
            .map(|_| ())
            .map_err(|failure| Error::Wallet(failure.message))
    }

    /// Broadcast an exact finalized transaction while preserving backend result classification.
    pub async fn broadcast_classified(
        &self,
        transaction: Transaction,
    ) -> Result<BroadcastOutcome, BroadcastFailure> {
        self.chain_source.broadcast(transaction).await
    }
}

fn ensure_wallet_namespace(
    db: &mut Connection,
    expected: &str,
    reject_unmarked_existing_wallet: bool,
) -> Result<(), Error> {
    let wallet_exists = db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'bdk_wallet'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    db.execute(
        &format!(
            "CREATE TABLE IF NOT EXISTS {WALLET_NAMESPACE_TABLE} (id INTEGER PRIMARY KEY CHECK (id = 0), namespace TEXT NOT NULL)"
        ),
        [],
    )?;
    let stored = db
        .query_row(
            &format!("SELECT namespace FROM {WALLET_NAMESPACE_TABLE} WHERE id = 0"),
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match stored {
        Some(stored) if stored == expected => Ok(()),
        Some(_) => Err(Error::WalletNamespaceMismatch),
        None if wallet_exists && reject_unmarked_existing_wallet => {
            Err(Error::WalletNamespaceMismatch)
        }
        None => {
            db.execute(
                &format!("INSERT INTO {WALLET_NAMESPACE_TABLE} (id, namespace) VALUES (0, ?1)"),
                params![expected],
            )?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bdk_wallet::bitcoin::bip32::{DerivationPath, Xpriv, Xpub};
    use bdk_wallet::bitcoin::hashes::Hash as _;
    use bdk_wallet::bitcoin::key::TapTweak;
    use bdk_wallet::bitcoin::psbt::Psbt;
    use bdk_wallet::bitcoin::secp256k1::{Keypair, Message, Secp256k1};
    use bdk_wallet::bitcoin::sighash::{Prevouts, SighashCache};
    use bdk_wallet::bitcoin::taproot;
    use bdk_wallet::bitcoin::{
        absolute, transaction, Amount, FeeRate, Network, OutPoint, Sequence, TapSighashType,
        Transaction, TxIn, TxOut, Txid, Witness,
    };

    use super::*;
    use crate::EsploraConfig;

    fn descriptor_config(seed: u8, network: Network) -> (DescriptorConfig, Xpriv) {
        let secp = Secp256k1::new();
        let xpriv = Xpriv::new_master(Network::Regtest, &[seed; 32]).expect("master key");
        let xpub = Xpub::from_priv(&secp, &xpriv);
        let config = DescriptorConfig::new(
            format!("tr({xpub}/0/*)"),
            format!("tr({xpub}/1/*)"),
            network,
        )
        .expect("descriptor config");
        (config, xpriv)
    }

    fn chain_source() -> ChainSource {
        ChainSource::Esplora(EsploraConfig {
            url: "http://127.0.0.1:1".to_string(),
            parallel_requests: 1,
        })
    }

    async fn funded_psbt(engine: &BdkWalletEngine, amount_sat: u64) -> (Psbt, Transaction) {
        let funding_address = engine
            .reveal_addresses_to(KeychainKind::External, 0)
            .await
            .expect("reveal funding address")
            .into_iter()
            .last()
            .expect("funding address");
        let funding_tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::all_zeros(), 0),
                script_sig: Default::default(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(amount_sat),
                script_pubkey: funding_address.address.script_pubkey(),
            }],
        };
        engine
            .apply_accepted_transaction(funding_tx.clone(), 1)
            .await
            .expect("apply funding transaction");

        let recipient = engine
            .reveal_addresses_to(KeychainKind::Internal, 0)
            .await
            .expect("reveal recipient")
            .into_iter()
            .last()
            .expect("recipient");
        let fee_rate = FeeRate::from_sat_per_vb(1).expect("fee rate");
        let psbt = engine
            .build_psbt(&[(recipient.address.script_pubkey(), 10_000)], fee_rate)
            .await
            .expect("build PSBT");
        (psbt, funding_tx)
    }

    fn sign_input(
        psbt: &mut Psbt,
        input_index: usize,
        xpriv: &Xpriv,
        path: &str,
        sighash_type: TapSighashType,
    ) {
        let secp = Secp256k1::new();
        let path = DerivationPath::from_str(path).expect("derivation path");
        let child = xpriv.derive_priv(&secp, &path).expect("derive signing key");
        let keypair = Keypair::from_secret_key(&secp, &child.private_key);
        let tweaked_keypair = keypair.tap_tweak(&secp, None);
        let prevouts = psbt
            .inputs
            .iter()
            .enumerate()
            .map(|(index, _)| psbt.spend_utxo(index).expect("spend UTXO").clone())
            .collect::<Vec<_>>();
        let sighash = SighashCache::new(&psbt.unsigned_tx)
            .taproot_key_spend_signature_hash(input_index, &Prevouts::All(&prevouts), sighash_type)
            .expect("Taproot sighash");
        let signature =
            secp.sign_schnorr_no_aux_rand(&Message::from(sighash), &tweaked_keypair.to_keypair());
        psbt.inputs[input_index].tap_key_sig = Some(taproot::Signature {
            signature,
            sighash_type,
        });
    }

    #[test]
    fn descriptor_config_rejects_secrets_and_duplicate_keychains() {
        let (config, xpriv) = descriptor_config(1, Network::Regtest);
        assert_eq!(config.network(), Network::Regtest);
        assert_ne!(config.external(), config.internal());

        let secret = format!("tr({xpriv}/0/*)");
        assert!(matches!(
            DescriptorConfig::new(&secret, config.internal().to_string(), Network::Regtest),
            Err(Error::InvalidDescriptor(_))
        ));
        assert!(matches!(
            DescriptorConfig::new(
                config.external().to_string(),
                config.external().to_string(),
                Network::Regtest,
            ),
            Err(Error::InvalidDescriptor(_))
        ));
    }

    #[tokio::test]
    async fn watch_only_wallet_reveals_idempotently_and_restarts() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (config, _) = descriptor_config(2, Network::Regtest);
        let engine =
            BdkWalletEngine::open_watch_only(config.clone(), chain_source(), tempdir.path())
                .expect("create watch-only wallet");

        let addresses = engine
            .reveal_addresses_to(KeychainKind::External, 3)
            .await
            .expect("reveal addresses");
        assert_eq!(addresses.len(), 4);
        assert_eq!(addresses[0].index, 0);
        assert_eq!(addresses[3].index, 3);
        drop(engine);

        let database = Connection::open(tempdir.path().join("bdk_wallet.sqlite"))
            .expect("open persisted watch-only database");
        let (external, internal): (String, String) = database
            .query_row(
                "SELECT descriptor, change_descriptor FROM bdk_wallet WHERE id = 0",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read descriptors");
        assert!(!external.contains("prv"));
        assert!(!internal.contains("prv"));
        drop(database);

        let reopened = BdkWalletEngine::open_watch_only(config, chain_source(), tempdir.path())
            .expect("reopen watch-only wallet");
        assert!(reopened
            .reveal_addresses_to(KeychainKind::External, 3)
            .await
            .expect("repeat reveal")
            .is_empty());
        let next = reopened
            .reveal_addresses_to(KeychainKind::External, 4)
            .await
            .expect("reveal next");
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].index, 4);
    }

    #[test]
    fn watch_only_wallet_rejects_network_and_descriptor_mismatch() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (config, _) = descriptor_config(6, Network::Regtest);
        BdkWalletEngine::open_watch_only(config, chain_source(), tempdir.path())
            .expect("create watch-only wallet");

        let (wrong_descriptor, _) = descriptor_config(7, Network::Regtest);
        assert!(
            BdkWalletEngine::open_watch_only(wrong_descriptor, chain_source(), tempdir.path(),)
                .is_err()
        );

        let other_dir = tempfile::tempdir().expect("other tempdir");
        let (wrong_network, _) = descriptor_config(8, Network::Bitcoin);
        assert!(
            BdkWalletEngine::open_watch_only(wrong_network, chain_source(), other_dir.path(),)
                .is_err()
        );
    }

    #[tokio::test]
    async fn watch_only_wallet_builds_but_cannot_sign_locally() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (config, _) = descriptor_config(3, Network::Regtest);
        let engine = BdkWalletEngine::open_watch_only(config, chain_source(), tempdir.path())
            .expect("watch-only wallet");
        let (mut psbt, _) = funded_psbt(&engine, 50_000).await;

        assert_eq!(engine.list_unspent().await.len(), 1);
        assert!(psbt.inputs.iter().all(|input| input.tap_key_sig.is_none()));
        assert!(matches!(
            engine.sign_psbt_locally(&mut psbt).await,
            Err(Error::LocalSigningDisabled)
        ));
    }

    #[tokio::test]
    async fn federated_builder_uses_only_consensus_inputs_and_allocated_change() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (config, _) = descriptor_config(12, Network::Regtest);
        let engine = BdkWalletEngine::open_watch_only(config, chain_source(), tempdir.path())
            .expect("watch-only wallet");
        let funding_addresses = engine
            .reveal_addresses_to(KeychainKind::External, 1)
            .await
            .expect("funding addresses");
        let funding = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::all_zeros(), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: funding_addresses
                .iter()
                .map(|address| TxOut {
                    value: Amount::from_sat(30_000),
                    script_pubkey: address.address.script_pubkey(),
                })
                .collect(),
        };
        engine
            .apply_accepted_transaction(funding.clone(), 1)
            .await
            .expect("apply funding");
        let recipient = engine
            .reveal_addresses_to(KeychainKind::Internal, 0)
            .await
            .expect("recipient")
            .pop()
            .expect("recipient address")
            .address
            .script_pubkey();
        let allocated_change = engine
            .reveal_addresses_to(KeychainKind::Internal, 1)
            .await
            .expect("allocated change")
            .pop()
            .expect("change address")
            .address
            .script_pubkey();
        let allowed = OutPoint::new(funding.compute_txid(), 1);

        let psbt = engine
            .build_federated_psbt(
                &[(recipient.clone(), 10_000)],
                FeeRate::from_sat_per_vb(1).expect("fee rate"),
                allocated_change.clone(),
                &[allowed],
            )
            .await
            .expect("federated PSBT");

        assert_eq!(psbt.unsigned_tx.version, transaction::Version::TWO);
        assert_eq!(psbt.unsigned_tx.input.len(), 1);
        assert_eq!(psbt.unsigned_tx.input[0].previous_output, allowed);
        assert_eq!(
            psbt.unsigned_tx.input[0].sequence,
            Sequence::ENABLE_RBF_NO_LOCKTIME
        );
        assert!(psbt
            .unsigned_tx
            .output
            .iter()
            .any(|output| output.script_pubkey == recipient));
        assert!(psbt
            .unsigned_tx
            .output
            .iter()
            .any(|output| output.script_pubkey == allocated_change));
        assert!(!psbt
            .unsigned_tx
            .input
            .iter()
            .any(|input| input.previous_output == OutPoint::new(funding.compute_txid(), 0)));

        let unknown = OutPoint::new(Txid::all_zeros(), 42);
        assert!(engine
            .build_federated_psbt(
                &[(recipient, 10_000)],
                FeeRate::from_sat_per_vb(1).expect("fee rate"),
                allocated_change,
                &[unknown],
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn external_taproot_signature_is_verified_finalized_and_applied() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (config, xpriv) = descriptor_config(4, Network::Regtest);
        let engine = BdkWalletEngine::open_watch_only(config, chain_source(), tempdir.path())
            .expect("watch-only wallet");
        let (unsigned, _) = funded_psbt(&engine, 50_000).await;

        let mut missing = unsigned.clone();
        assert!(matches!(
            engine.finalize_external_psbt(&mut missing).await,
            Err(Error::ExternalSignature(_))
        ));

        let (_, wrong_xpriv) = descriptor_config(5, Network::Regtest);
        let mut wrong_key = unsigned.clone();
        sign_input(
            &mut wrong_key,
            0,
            &wrong_xpriv,
            "m/0/0",
            TapSighashType::Default,
        );
        assert!(matches!(
            engine.finalize_external_psbt(&mut wrong_key).await,
            Err(Error::ExternalSignature(_))
        ));

        let mut invalid_signature = unsigned.clone();
        sign_input(
            &mut invalid_signature,
            0,
            &xpriv,
            "m/0/0",
            TapSighashType::Default,
        );
        let mut signature_bytes = invalid_signature.inputs[0]
            .tap_key_sig
            .as_ref()
            .expect("Taproot signature")
            .signature
            .serialize();
        signature_bytes[0] ^= 1;
        invalid_signature.inputs[0]
            .tap_key_sig
            .as_mut()
            .expect("Taproot signature")
            .signature =
            bdk_wallet::bitcoin::secp256k1::schnorr::Signature::from_slice(&signature_bytes)
                .expect("structurally valid signature");
        assert!(matches!(
            engine.finalize_external_psbt(&mut invalid_signature).await,
            Err(Error::ExternalSignature(_))
        ));

        let mut wrong_sighash = unsigned.clone();
        sign_input(&mut wrong_sighash, 0, &xpriv, "m/0/0", TapSighashType::All);
        assert!(matches!(
            engine.finalize_external_psbt(&mut wrong_sighash).await,
            Err(Error::ExternalSignature(_))
        ));

        let selected_inputs = vec![(
            unsigned.unsigned_tx.input[0].previous_output,
            unsigned.spend_utxo(0).expect("accepted prevout").clone(),
        )];
        let mut threshold_signed = unsigned.clone();
        sign_input(
            &mut threshold_signed,
            0,
            &xpriv,
            "m/0/0",
            TapSighashType::Default,
        );
        let threshold_signature = threshold_signed.inputs[0]
            .tap_key_sig
            .as_ref()
            .expect("threshold signature")
            .signature
            .serialize();
        let threshold_transaction = engine
            .finalize_federated_transaction(
                &unsigned.unsigned_tx,
                &selected_inputs,
                &[threshold_signature],
            )
            .await
            .expect("reconstruct and finalize accepted transaction");
        assert_eq!(
            threshold_transaction.compute_txid(),
            unsigned.unsigned_tx.compute_txid()
        );
        let mut mutated_unsigned = unsigned.unsigned_tx.clone();
        mutated_unsigned.output[0].value = Amount::from_sat(9_999);
        assert!(matches!(
            engine
                .finalize_federated_transaction(
                    &mutated_unsigned,
                    &selected_inputs,
                    &[threshold_signature],
                )
                .await,
            Err(Error::ExternalSignature(_))
        ));

        let mut valid = unsigned;
        sign_input(&mut valid, 0, &xpriv, "m/0/0", TapSighashType::Default);
        let transaction = engine
            .finalize_external_psbt(&mut valid)
            .await
            .expect("finalize valid external signature");
        assert!(!transaction.input[0].witness.is_empty());

        let txid = transaction.compute_txid();
        engine
            .apply_accepted_transaction(transaction, 2)
            .await
            .expect("apply accepted transaction");
        let wallet_with_db = engine.wallet_with_db.lock().await;
        assert!(wallet_with_db.wallet.get_tx(txid).is_some());
    }

    #[tokio::test]
    async fn federated_finalization_verifies_every_input_and_rejects_one_bad_signature() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (config, xpriv) = descriptor_config(9, Network::Regtest);
        let engine = BdkWalletEngine::open_watch_only(config, chain_source(), tempdir.path())
            .expect("watch-only wallet");
        let addresses = engine
            .reveal_addresses_to(KeychainKind::External, 1)
            .await
            .expect("reveal funding addresses");
        let funding = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::all_zeros(), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: addresses
                .iter()
                .map(|address| TxOut {
                    value: Amount::from_sat(30_000),
                    script_pubkey: address.address.script_pubkey(),
                })
                .collect(),
        };
        engine
            .apply_accepted_transaction(funding, 1)
            .await
            .expect("apply funding");
        let recipient = engine
            .reveal_addresses_to(KeychainKind::Internal, 0)
            .await
            .expect("reveal recipient")
            .pop()
            .expect("recipient");
        let psbt = engine
            .build_psbt(
                &[(recipient.address.script_pubkey(), 50_000)],
                FeeRate::from_sat_per_vb(1).expect("fee rate"),
            )
            .await
            .expect("two-input PSBT");
        assert_eq!(psbt.inputs.len(), 2);
        let selected_inputs = psbt
            .unsigned_tx
            .input
            .iter()
            .enumerate()
            .map(|(index, input)| {
                (
                    input.previous_output,
                    psbt.spend_utxo(index).expect("accepted prevout").clone(),
                )
            })
            .collect::<Vec<_>>();
        let mut signed = psbt.clone();
        for (index, input) in signed.unsigned_tx.input.clone().iter().enumerate() {
            sign_input(
                &mut signed,
                index,
                &xpriv,
                &format!("m/0/{}", input.previous_output.vout),
                TapSighashType::Default,
            );
        }
        let signatures = signed
            .inputs
            .iter()
            .map(|input| {
                input
                    .tap_key_sig
                    .as_ref()
                    .expect("input signature")
                    .signature
                    .serialize()
            })
            .collect::<Vec<_>>();
        let transaction = engine
            .finalize_federated_transaction(&psbt.unsigned_tx, &selected_inputs, &signatures)
            .await
            .expect("finalize every input");
        assert!(transaction
            .input
            .iter()
            .all(|input| input.witness.len() == 1));

        let mut bad_signatures = signatures;
        bad_signatures[1][0] ^= 1;
        assert!(matches!(
            engine
                .finalize_federated_transaction(
                    &psbt.unsigned_tx,
                    &selected_inputs,
                    &bad_signatures,
                )
                .await,
            Err(Error::ExternalSignature(_))
        ));
    }
}
