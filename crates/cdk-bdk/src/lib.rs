//! CDK lightning backend for CLN

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, Error, Result};
use async_trait::async_trait;
use bdk_esplora::{esplora_client, EsploraAsyncExt};
use bdk_wallet::bitcoin::{Address, FeeRate, Network, Script};
use bdk_wallet::chain::BlockId;
use bdk_wallet::file_store::Store;
use bdk_wallet::keys::bip39::Mnemonic;
use bdk_wallet::keys::{DerivableKey, ExtendedKey};
use bdk_wallet::template::Bip84;
use bdk_wallet::{ChangeSet, KeychainKind, PersistedWallet, SignOptions, Wallet};
use cdk::amount::Amount;
use cdk::bitcoin::bech32::ToBase32;
use cdk::cdk_onchain::{
    self, AddressPaidResponse, MintOnChain, NewAddressResponse, PayjoinSettings, Settings,
};
use cdk::mint;
use cdk::nuts::CurrencyUnit;
use payjoin::receive::v2::{PayjoinProposal, ProvisionalProposal, UncheckedProposal};
use payjoin::Url;
use tokio::sync::Mutex;

pub mod error;

const STOP_GAP: usize = 50;
const PARALLEL_REQUESTS: usize = 5;

const NETWORK: Network = Network::Signet;

#[derive(Clone)]
pub struct BdkWallet {
    wallet: Arc<Mutex<PersistedWallet>>,
    client: esplora_client::AsyncClient,
    db: Arc<Mutex<Store<bdk_wallet::ChangeSet>>>,
    min_melt_amount: u64,
    max_melt_amount: u64,
    min_mint_amount: u64,
    max_mint_amount: u64,
    mint_enabled: bool,
    melt_enabled: bool,
    payjoin_settings: PayjoinSettings,
    sender: tokio::sync::mpsc::Sender<UncheckedProposal>,
    receiver: Arc<Mutex<tokio::sync::mpsc::Receiver<UncheckedProposal>>>,
    seen_inputs: Arc<Mutex<HashSet<payjoin::bitcoin::OutPoint>>>,
}

impl BdkWallet {
    pub async fn new(
        min_melt_amount: u64,
        max_melt_amount: u64,
        min_mint_amount: u64,
        max_mint_amount: u64,
        // REVIEW: I think it maybe best if we force a Mnemonic here as it will hold onchain funds.
        // But maybe should be a byte seed like we do for mint and wallet?
        mnemonic: Mnemonic,
        work_dir: &Path,
        payjoin_settings: PayjoinSettings,
    ) -> Result<Self, Error> {
        let db_path = work_dir.join("bdk-mint");
        // FIXME: Updates bytes
        let mut db =
            Store::<ChangeSet>::open_or_create_new(b"magic_bytes", db_path).expect("create store");

        let xkey: ExtendedKey = mnemonic.into_extended_key()?;
        // Get xprv from the extended key
        let xprv = xkey
            .into_xprv(NETWORK)
            .ok_or(anyhow!("Could not get expriv from network"))?;

        let descriptor = Bip84(xprv, KeychainKind::External);
        let change_descriptor = Bip84(xprv, KeychainKind::Internal);

        let wallet_opt = Wallet::load()
            .descriptors(descriptor.clone(), change_descriptor.clone())
            .network(NETWORK)
            .load_wallet(&mut db)?;

        let mut wallet = match wallet_opt {
            Some(wallet) => wallet,
            None => Wallet::create(descriptor, change_descriptor)
                .network(NETWORK)
                .create_wallet(&mut db)?,
        };

        let client = esplora_client::Builder::new("https://mutinynet.com/api").build_async()?;

        fn generate_inspect(
            kind: KeychainKind,
        ) -> impl FnMut(u32, &Script) + Send + Sync + 'static {
            let mut once = Some(());
            let mut stdout = std::io::stdout();
            move |spk_i, _| {
                match once.take() {
                    Some(_) => print!("\nScanning keychain [{:?}]", kind),
                    None => print!(" {:<3}", spk_i),
                };
                stdout.flush().expect("must flush");
            }
        }

        let request = wallet
            .start_full_scan()
            .inspect_spks_for_all_keychains({
                let mut once = BTreeSet::<KeychainKind>::new();
                move |keychain, spk_i, _| {
                    match once.insert(keychain) {
                        true => print!("\nScanning keychain [{:?}]", keychain),
                        false => print!(" {:<3}", spk_i),
                    }
                    std::io::stdout().flush().expect("must flush")
                }
            })
            .inspect_spks_for_keychain(
                KeychainKind::External,
                generate_inspect(KeychainKind::External),
            )
            .inspect_spks_for_keychain(
                KeychainKind::Internal,
                generate_inspect(KeychainKind::Internal),
            );

        tracing::debug!("Starting wallet full scan");

        let mut update = client
            .full_scan(request, STOP_GAP, PARALLEL_REQUESTS)
            .await?;
        let now = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
        let _ = update.graph_update.update_last_seen_unconfirmed(now);

        wallet.persist(&mut db)?;

        tracing::debug!("Completed wallet scan");

        let (sender, receiver) = tokio::sync::mpsc::channel(8);

        Ok(Self {
            wallet: Arc::new(Mutex::new(wallet)),
            db: Arc::new(Mutex::new(db)),
            client,
            min_mint_amount,
            max_mint_amount,
            min_melt_amount,
            max_melt_amount,
            mint_enabled: true,
            melt_enabled: true,
            payjoin_settings,
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
            seen_inputs: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    async fn update_chain_tip(&self) -> Result<(), Error> {
        let mut wallet = self.wallet.lock().await;
        let latest_checkpoint = wallet.latest_checkpoint();
        let latest_checkpoint_height = latest_checkpoint.height();

        tracing::info!("Current wallet known height: {}", latest_checkpoint_height);

        let mut fetched_blocks: Vec<BlockId> = vec![];

        let mut last_fetched_height = None;

        while last_fetched_height.is_none()
            || last_fetched_height.expect("Checked for none") > latest_checkpoint_height
        {
            let blocks = self.client.get_blocks(last_fetched_height).await?;

            for block in blocks {
                match last_fetched_height {
                    Some(height) if block.time.height < height => {
                        last_fetched_height = Some(block.time.height);
                    }
                    None => {
                        tracing::info!("Current block tip: {}", block.time.height);
                        last_fetched_height = Some(block.time.height);
                    }
                    _ => {}
                }
                let block_id = BlockId {
                    height: block.time.height,
                    hash: block.id,
                };

                match block.time.height > latest_checkpoint_height {
                    true => fetched_blocks.push(block_id),
                    false => break,
                }
            }
        }

        fetched_blocks.reverse();

        for block_id in fetched_blocks {
            tracing::trace!("Inserting wallet checkpoint: {}", block_id.height);
            wallet.insert_checkpoint(block_id)?;
        }

        if let Some(changeset) = wallet.take_staged() {
            let mut db = self.db.lock().await;
            db.append_changeset(&changeset)?;
        }

        Ok(())
    }
}

// TODO: Making this a payjoin trait
impl BdkWallet {
    async fn start_payjoin(&self, address: &str) -> Result<String, Error> {
        let ohttp_relay = self.payjoin_settings.ohttp_relay.clone();

        let payjoin_directory = self
            .payjoin_settings
            .payjoin_directory
            .clone()
            .ok_or(anyhow!("pajoin directory required"))?;

        let ohttp_relay: Url = ohttp_relay
            .ok_or(anyhow!("Payjoing ohttp relay must be defined"))?
            .parse()?;
        let payjoin_directory: Url = payjoin_directory.parse()?;

        // Fetch keys using HTTP CONNECT method
        let ohttp_keys =
            payjoin::io::fetch_ohttp_keys(ohttp_relay.clone(), payjoin_directory.clone()).await?;

        let mut session = payjoin::receive::v2::SessionInitializer::new(
            payjoin::bitcoin::Address::from_str(address)?.assume_checked(),
            payjoin_directory,
            ohttp_keys,
            ohttp_relay,
            Some(std::time::Duration::from_secs(600)),
        );
        let (req, ctx) = session.extract_req().unwrap();
        let http = reqwest::Client::new();

        let res = http
            .post(req.url)
            .body(req.body)
            .header("Content-Type", payjoin::V2_REQ_CONTENT_TYPE)
            .send()
            .await
            .unwrap();
        let mut session = session
            .process_res(res.bytes().await?.to_vec().as_slice(), ctx)
            .unwrap();

        let uri = session
            .pj_uri_builder()
            .amount(payjoin::bitcoin::Amount::from_sat(88888))
            .build();

        tracing::info!("PJ url: {}", session.pj_url());
        println!("Payjoin URI: {}", uri);
        tracing::debug!("{}", uri.to_string());
        let pj_url = session.pj_url().to_string();

        let sender = self.sender.clone();
        tokio::spawn(async move {
            let proposal = loop {
                tracing::debug!("Polling for proposal");
                let (req, ctx) = match session.extract_req() {
                    Ok((res, tx)) => (res, tx),
                    Err(err) => {
                        tracing::info!("Error extracting session: {}", err);
                        break None;
                    }
                };

                let res = match http
                    .post(req.url)
                    .body(req.body)
                    .header("Content-Type", payjoin::V2_REQ_CONTENT_TYPE)
                    .send()
                    .await
                {
                    Ok(res) => res,
                    Err(err) => {
                        tracing::error!("Error making payjoin polling request: {}", err);

                        continue;
                    }
                };

                match session.process_res(res.bytes().await.unwrap().to_vec().as_slice(), ctx) {
                    Ok(Some(proposal)) => {
                        break Some(proposal);
                    }
                    Ok(None) => {
                        continue;
                    }
                    Err(err) => {
                        tracing::error!("Error polling for payjoin proposal: {}", err);
                        continue;
                    }
                }
            };

            if let Some(proposal) = proposal {
                tracing::debug!("Received Proposal");
                if let Err(err) = sender.send(proposal).await {
                    tracing::error!("Could not send proposal on channel: {}", err);
                }
            }
        });

        Ok(pj_url)
    }

    pub async fn verify_proposal(
        wallet: Arc<Mutex<PersistedWallet>>,
        proposal: UncheckedProposal,
        seen_inputs: HashSet<payjoin::bitcoin::OutPoint>,
    ) -> Result<ProvisionalProposal, Error> {
        let wallet = wallet.lock().await;
        proposal
            // TODO: Check this can be broadcast
            .check_broadcast_suitability(None, |_tx| Ok(true))
            .map_err(|_| anyhow!("TX cannot be broadcast"))?
            .check_inputs_not_owned(|input| {
                let bytes = input.to_bytes();
                let script = Script::from_bytes(&bytes);
                Ok(wallet.is_mine(script.into()))
            })
            .map_err(|_| anyhow!("Receiver should not own any of the inputs"))?
            .check_no_mixed_input_scripts()
            .expect("No mixed input scripts")
            .check_no_inputs_seen_before(|outpoint| match seen_inputs.contains(outpoint) {
                true => Ok(true),
                false => Ok(false),
            })
            .expect("No inputs seen before")
            .identify_receiver_outputs(|output_script| {
                let bytes = output_script.to_bytes();
                let script = Script::from_bytes(&bytes);
                Ok(wallet.is_mine(script.into()))
            })
            .map_err(|_| anyhow!("Receiver outputs"))
    }

    pub async fn wait_handle_proposal(&self) -> Result<(), Error> {
        let mut receiver = self.receiver.lock().await;
        tokio::select! {
            Some(proposal) = receiver.recv() => {
                match self.handle_proposal(proposal).await {
                    Ok(()) => {
                        tracing::info!("Sent payjoin");
                    }
                    Err(err) => {
                        tracing::error!("Could not proceed with payjoin proposal: {:?}", err);
                    }
                }
            }
            else => ()
        }
        Ok(())
    }

    pub async fn handle_proposal(&self, proposal: UncheckedProposal) -> Result<(), Error> {
        let mut seen_inputs = self.seen_inputs.lock().await;

        let tx = proposal.extract_tx_to_schedule_broadcast();

        let their_inputs: HashSet<_> = tx.input.iter().map(|tx_in| tx_in.previous_output).collect();

        let mut payjoin =
            BdkWallet::verify_proposal(Arc::clone(&self.wallet), proposal, seen_inputs.clone())
                .await?;
        seen_inputs.extend(their_inputs);
        drop(seen_inputs);

        tracing::debug!("Verified proposal");

        let wallet = self.wallet.lock().await;

        // Augment the Proposal to Make a Batched Transaction
        let available_inputs: Vec<_> = wallet.list_unspent().collect();
        tracing::debug!("{} available inputs to contribute", available_inputs.len());
        let candidate_inputs: HashMap<payjoin::bitcoin::Amount, payjoin::bitcoin::OutPoint> =
            available_inputs
                .iter()
                .map(|i| {
                    (
                        payjoin::bitcoin::Amount::from_sat(i.txout.value.to_sat()),
                        payjoin::bitcoin::OutPoint {
                            txid: payjoin::bitcoin::Txid::from_str(
                                &i.outpoint.txid.to_raw_hash().to_string(),
                            )
                            .unwrap(),
                            vout: i.outpoint.vout,
                        },
                    )
                })
                .collect();

        let selected_outpoint = payjoin.try_preserving_privacy(candidate_inputs).unwrap();
        let selected_utxo = available_inputs
            .iter()
            .find(|i| {
                i.outpoint.txid.to_base32() == selected_outpoint.txid.to_base32()
                    && i.outpoint.vout == selected_outpoint.vout
            })
            .unwrap();

        let txo_to_contribute = payjoin::bitcoin::TxOut {
            value: selected_utxo.txout.value.to_sat(),
            script_pubkey: payjoin::bitcoin::ScriptBuf::from_bytes(
                selected_utxo.clone().txout.script_pubkey.into_bytes(),
            ),
        };
        let outpoint_to_contribute = payjoin::bitcoin::OutPoint {
            txid: payjoin::bitcoin::Txid::from_str(
                &selected_utxo.outpoint.txid.to_raw_hash().to_string(),
            )
            .unwrap(),
            vout: selected_utxo.outpoint.vout,
        };
        payjoin.contribute_witness_input(txo_to_contribute, outpoint_to_contribute);

        let payjoin = payjoin
            .finalize_proposal(
                |psbt| {
                    let psbt = psbt.to_string();
                    let mut psbt = bdk_wallet::bitcoin::psbt::Psbt::from_str(&psbt).unwrap();

                    let sign_options = SignOptions {
                        trust_witness_utxo: true,
                        ..Default::default()
                    };

                    if let Err(err) = wallet.sign(&mut psbt, sign_options.clone()) {
                        tracing::error!("Could not sign psbt: {}", err);
                    }

                    if let Err(err) = wallet.finalize_psbt(&mut psbt, sign_options) {
                        tracing::debug!("Could not finalize transactions: {}", err);
                    }

                    let psbt = payjoin::bitcoin::psbt::Psbt::from_str(&psbt.to_string()).unwrap();
                    Ok(psbt)
                },
                Some(payjoin::bitcoin::FeeRate::MIN),
            )
            .map_err(|_| anyhow!("Could not finalize proposal"))?;

        self.send_payjoin_proposal(payjoin).await?;

        tracing::debug!("finalized transaction");

        Ok(())
    }

    pub async fn send_payjoin_proposal(&self, payjoin: PayjoinProposal) -> Result<(), Error> {
        let mut payjoin = payjoin;
        let (req, ctx) = payjoin.extract_v2_req().unwrap();
        let http = reqwest::Client::new();
        let res = http
            .post(req.url)
            .body(req.body)
            .header("Content-Type", payjoin::V2_REQ_CONTENT_TYPE)
            .send()
            .await
            .unwrap();
        payjoin
            .process_res(res.bytes().await.unwrap().to_vec(), ctx)
            .unwrap();
        let payjoin_psbt = payjoin.psbt().clone();

        println!(
            "response successful. Watch mempool for successful payjoin. TXID: {}",
            payjoin_psbt.extract_tx().clone().txid()
        );

        Ok(())
    }
}

#[async_trait]
impl MintOnChain for BdkWallet {
    type Err = cdk_onchain::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            payjoin_settings: self.payjoin_settings.clone(),
            min_mint_amount: self.min_mint_amount,
            max_mint_amount: self.max_mint_amount,
            min_melt_amount: self.min_melt_amount,
            max_melt_amount: self.max_melt_amount,
            unit: CurrencyUnit::Sat,
            mint_enabled: self.mint_enabled,
            melt_enabled: self.melt_enabled,
        }
    }

    /// New onchain address
    async fn new_address(&self) -> Result<NewAddressResponse, Self::Err> {
        let mut wallet = self.wallet.lock().await;
        let address = wallet
            .reveal_next_address(KeychainKind::External)
            .address
            .to_string();

        if let Some(changeset) = wallet.take_staged() {
            let mut db = self.db.lock().await;
            if let Err(err) = db.append_changeset(&changeset) {
                tracing::error!("Could not update change set with new address: {}", err);
                return Err(anyhow!("Could not update used address index").into());
            }
        }

        let payjoin_url = match self.payjoin_settings.receive_enabled {
            true => Some(self.start_payjoin(&address).await?),
            false => None,
        };

        Ok(NewAddressResponse {
            address,
            payjoin_url,
            pjos: None,
        })
    }

    /// Pay Address
    async fn pay_address(
        &self,
        melt_quote: mint::MeltQuote,
        _max_fee_sat: Amount,
    ) -> Result<String, Self::Err> {
        let mut wallet = self.wallet.lock().await;
        let address = Address::from_str(&melt_quote.request)
            .map_err(|_| anyhow!("Could not parse address"))?;

        let mut psbt = {
            let mut builder = wallet.build_tx();
            builder
                .add_recipient(
                    address.assume_checked().into(),
                    bdk_wallet::bitcoin::Amount::from_sat(melt_quote.amount.into()),
                )
                .enable_rbf()
                // TODO: Fee rate
                .fee_rate(FeeRate::from_sat_per_vb(5).unwrap());
            builder
                .finish()
                .map_err(|_| anyhow!("Could not build transaction"))?
        };

        wallet
            .sign(&mut psbt, SignOptions::default())
            .map_err(|_| anyhow!("Could not sign transaction"))?;

        let tx = psbt
            .extract_tx()
            .map_err(|_| anyhow!("Could not extract transaction"))?;

        self.client.broadcast(&tx).await.map_err(Error::from)?;

        Ok(tx.compute_txid().to_string())
    }

    /// Check if an address has been paid
    async fn check_address_paid(&self, address: &str) -> Result<AddressPaidResponse, Self::Err> {
        let address: Address = Address::from_str(address).unwrap().assume_checked();

        let script = address.script_pubkey();

        self.update_chain_tip().await?;

        let transactions = self
            .client
            .scripthash_txs(script.as_script(), None)
            .await
            .unwrap();

        let mut amount: u64 = 0;
        let mut block_heights: Vec<u32> = Vec::with_capacity(transactions.capacity());

        for tx in transactions {
            let received: u64 = tx
                .vout
                .iter()
                .filter(|v| v.scriptpubkey == script)
                .map(|v| v.value)
                .sum();

            amount += received;

            if let Some(block_time) = tx.confirmation_time() {
                let height = block_time.height;
                block_heights.push(height);
            }

            let wallet_clone = Arc::clone(&self.wallet);

            tokio::spawn(async move {
                let mut wallet = wallet_clone.lock().await;

                if !wallet.insert_tx(tx.to_tx()) {
                    tracing::warn!(
                        "Could not insert transaction: {}",
                        tx.to_tx().compute_txid(),
                    );
                }
            });
        }

        let max_block_height = block_heights.iter().max().cloned();

        Ok(AddressPaidResponse {
            amount: Amount::from(amount),
            max_block_height,
        })
    }
}
