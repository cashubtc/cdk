//! Bitcoind RPC Client

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use bitcoincore_rpc::bitcoin::{consensus, Address, Amount, Transaction, Txid};
use bitcoincore_rpc::json::WalletCreateFundedPsbtOptions;
use bitcoincore_rpc::{Auth, Client, RpcApi};

/// Bitcoin client
#[derive(Clone)]
pub struct BitcoinClient {
    wallet: String,
    client: Arc<Client>,
}

impl BitcoinClient {
    /// Create bitcoind rpc client
    pub fn new(
        wallet: String,
        addr: PathBuf,
        cookie_file: Option<PathBuf>,
        user_name: Option<String>,
        password: Option<String>,
    ) -> Result<Self> {
        let addr = addr.join(format!("wallet/{}", wallet));

        let auth = match cookie_file {
            Some(path) => Auth::CookieFile(path),
            None => Auth::UserPass(user_name.unwrap(), password.unwrap()),
        };

        println!("{}", addr.display());

        let client = Client::new(&addr.display().to_string(), auth).unwrap();

        Ok(Self {
            client: Arc::new(client),
            wallet,
        })
    }

    /// Create wallet
    pub fn create_wallet(&self) -> Result<()> {
        let client = &self.client;

        client.create_wallet(&self.wallet, None, None, None, None)?;

        Ok(())
    }

    /// Load wallet
    pub fn load_wallet(&self) -> Result<()> {
        let client = &self.client;

        match client.load_wallet(&self.wallet) {
            Ok(_res) => Ok(()),
            Err(err) => {
                println!("{}", err);
                Ok(())
            }
        }
    }

    /// Get new address
    pub fn get_new_address(&self) -> Result<String> {
        let client = &self.client;

        let address = client.get_new_address(None, None)?.assume_checked();

        Ok(address.to_string())
    }

    /// Generate blocks
    pub fn generate_blocks(&self, address: &str, block_count: u64) -> Result<()> {
        let client = &self.client;

        client.generate_to_address(
            block_count,
            Address::from_str(address)?.assume_checked_ref(),
        )?;

        Ok(())
    }

    /// Send to address
    pub fn send_to_address(&self, address: &str, amount: u64) -> Result<()> {
        let client = &self.client;

        let address = Address::from_str(address)?.assume_checked();
        let amount = Amount::from_sat(amount);

        client.send_to_address(&address, amount, None, None, None, None, None, None)?;

        Ok(())
    }

    /// Create and fund a PSBT paying `amount` sats to `address`.
    pub fn create_funded_psbt(
        &self,
        address: &str,
        amount: u64,
        fee_rate_sat_vb: u64,
    ) -> Result<String> {
        let client = &self.client;

        let mut outputs = std::collections::HashMap::new();
        outputs.insert(address.to_string(), Amount::from_sat(amount));
        let options = WalletCreateFundedPsbtOptions {
            fee_rate: Some(Amount::from_sat(fee_rate_sat_vb.saturating_mul(1000))),
            replaceable: Some(false),
            ..Default::default()
        };

        Ok(client
            .wallet_create_funded_psbt(&[], &outputs, None, Some(options), Some(false))?
            .psbt)
    }

    /// Sign a PSBT with the regtest Bitcoin Core wallet.
    pub fn sign_psbt(&self, psbt: &str) -> Result<String> {
        let client = &self.client;
        let processed = client.wallet_process_psbt(psbt, Some(true), None, Some(false))?;
        Ok(processed.psbt)
    }

    /// Finalize and broadcast a PSBT with the regtest Bitcoin Core wallet.
    pub fn finalize_and_broadcast_psbt(&self, psbt: &str) -> Result<Txid> {
        let client = &self.client;
        let finalized = client.finalize_psbt(psbt, Some(true))?;
        let tx_hex = finalized
            .hex
            .ok_or_else(|| anyhow::anyhow!("finalizepsbt did not return transaction hex"))?;
        let tx: Transaction = consensus::deserialize(&tx_hex)?;
        Ok(client.send_raw_transaction(&tx)?)
    }

    /// Return the input count of the first mempool transaction that pays to
    /// `address`, if any.
    ///
    /// Only the mempool is inspected, so this works without `-txindex` even for
    /// transactions that do not belong to bitcoind's own wallet (e.g. a Payjoin
    /// transaction built and broadcast by the mints' BDK wallets). Used by the
    /// Payjoin tests to prove the combined transaction batches sender and
    /// receiver inputs before it is mined.
    pub fn mempool_tx_input_count_to_address(&self, address: &str) -> Result<Option<usize>> {
        let client = &self.client;
        let target = Address::from_str(address)?.assume_checked().script_pubkey();
        for txid in client.get_raw_mempool()? {
            let tx = match client.get_raw_transaction(&txid, None) {
                Ok(tx) => tx,
                Err(_) => continue,
            };
            if tx
                .output
                .iter()
                .any(|output| output.script_pubkey == target)
            {
                return Ok(Some(tx.input.len()));
            }
        }
        Ok(None)
    }

    pub fn get_balance(&self) -> Result<u64> {
        let client = &self.client;

        let balance = client.get_balance(None, None)?;
        let balances = client.get_balances()?;

        println!("{:?}", balances);

        Ok(balance.to_sat())
    }

    pub fn list_fund(&self) -> Result<()> {
        let client = &self.client;

        let balance = client.list_transactions(None, None, None, None)?;

        println!("{:#?}", balance);
        Ok(())
    }
}
