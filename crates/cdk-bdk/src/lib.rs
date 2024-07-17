//! CDK lightning backend for CLN

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, Error};
use async_trait::async_trait;
use bdk_chain::{miniscript, ConfirmationTime};
use bdk_esplora::esplora_client;
use bdk_file_store::Store;
use bdk_wallet::bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bdk_wallet::bitcoin::{Address, FeeRate, Network};
use bdk_wallet::keys::bip39::Mnemonic;
use bdk_wallet::keys::{DerivableKey, ExtendedKey};
use bdk_wallet::template::DescriptorTemplateOut;
use bdk_wallet::{KeychainKind, SignOptions, Wallet};
use cdk::amount::Amount;
use cdk::cdk_onchain::{self, AddressPaidResponse, MintOnChain, Settings};
use cdk::mint;
use cdk::nuts::CurrencyUnit;
use cdk::util::unix_time;
use tokio::sync::Mutex;

pub mod error;

const DB_MAGIC: &str = "bdk_wallet_electrum_example";

pub struct BdkWallet {
    wallet: Arc<Mutex<Wallet>>,
    client: esplora_client::AsyncClient,
    min_melt_amount: u64,
    max_melt_amount: u64,
    min_mint_amount: u64,
    max_mint_amount: u64,
    mint_enabled: bool,
    melt_enabled: bool,
}

impl BdkWallet {
    pub async fn new(
        min_melt_amount: u64,
        max_melt_amount: u64,
        min_mint_amount: u64,
        max_mint_amount: u64,
        // REVIEW: I think it maybe best if we force a Mnemonic here as it will hole onchain funds.
        // But maybe should be a byte seed like we do for mint and wallet?
        mnemonic: Mnemonic,
        work_dir: &PathBuf,
        network: Network,
    ) -> Result<Self, Error> {
        let db_path = work_dir.join("bdk-mint");
        let mut db = Store::<bdk_wallet::wallet::ChangeSet>::open_or_create_new(
            DB_MAGIC.as_bytes(),
            db_path,
        )
        .map_err(|e| anyhow!("Could not open bdk change store: {}", e))?;

        let xkey: ExtendedKey = mnemonic.into_extended_key().unwrap();
        // Get xprv from the extended key
        let xprv = xkey.into_xprv(network).unwrap();

        let changeset = db
            .aggregate_changesets()
            .map_err(|e| anyhow!("load changes error: {}", e))
            .unwrap();

        let (internal_descriptor, external_descriptor) =
            get_wpkh_descriptors_for_extended_key(xprv, network, 0)?;

        let wallet = Wallet::new_or_load(
            internal_descriptor,
            external_descriptor,
            changeset,
            Network::Signet,
        )
        .map_err(|_| anyhow!("Could not create cdk wallet"))?;

        let client =
            esplora_client::Builder::new("http://signet.bitcoindevkit.net").build_async()?;

        Ok(Self {
            wallet: Arc::new(Mutex::new(wallet)),
            client,
            min_mint_amount,
            max_mint_amount,
            min_melt_amount,
            max_melt_amount,
            mint_enabled: true,
            melt_enabled: true,
        })
    }
}

#[async_trait]
impl MintOnChain for BdkWallet {
    type Err = cdk_onchain::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: true,
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
    async fn new_address(&self) -> Result<String, Self::Err> {
        let mut wallet = self.wallet.lock().await;
        Ok(wallet
            .reveal_next_address(KeychainKind::External)
            .to_string())
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
<<<<<<< Updated upstream
    async fn check_address_paid(
        &self,
        address: cdk::bitcoin::Address,
    ) -> Result<AddressPaidResponse, Self::Err> {
        let address: Address = Address::from_str(&address.to_string())
            .unwrap()
            .assume_checked();
=======
    async fn check_address_paid(&self, address: &str) -> Result<AddressPaidResponse, Self::Err> {
        let address: Address = Address::from_str(address).unwrap().assume_checked();
>>>>>>> Stashed changes

        let script = address.script_pubkey();

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

                let confirmation_time = tx
                    .confirmation_time()
                    .map(|c| ConfirmationTime::Confirmed {
                        height: c.height,
                        time: c.timestamp,
                    })
                    .unwrap_or(ConfirmationTime::Unconfirmed {
                        last_seen: unix_time(),
                    });

                if let Err(err) = wallet.insert_tx(tx.to_tx(), confirmation_time) {
                    tracing::warn!(
                        "Could not insert transaction: {}, {}",
                        tx.to_tx().compute_txid(),
                        err
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

// Copyright (c) 2022-2024 Mutiny Wallet Inc. (MIT)
fn get_wpkh_descriptors_for_extended_key(
    master_xprv: Xpriv,
    network: Network,
    account_number: u32,
) -> anyhow::Result<(DescriptorTemplateOut, DescriptorTemplateOut)> {
    let coin_type = coin_type_from_network(network);

    let base_path = DerivationPath::from_str("m/86'")?;
    let derivation_path = base_path.extend([
        ChildNumber::from_hardened_idx(coin_type)?,
        ChildNumber::from_hardened_idx(account_number)?,
    ]);

    let receive_descriptor_template = bdk_wallet::descriptor!(wpkh((
        master_xprv,
        derivation_path.extend([ChildNumber::Normal { index: 0 }])
    )))
    .map_err(|_| anyhow!("Could not generate derivation path"))?;

    let change_descriptor_template = bdk_wallet::descriptor!(wpkh((
        master_xprv,
        derivation_path.extend([ChildNumber::Normal { index: 1 }])
    )))
    .map_err(|_| anyhow!("Could not generate derivation path"))?;

    Ok((receive_descriptor_template, change_descriptor_template))
}

pub(crate) fn coin_type_from_network(network: Network) -> u32 {
    match network {
        Network::Bitcoin => 0,
        Network::Testnet => 1,
        Network::Signet => 1,
        Network::Regtest => 1,
        net => panic!("Got unknown network: {net}!"),
    }
}
