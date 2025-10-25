//! Bitcoind RPC Client

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use bitcoincore_rpc::bitcoin::{Address, Amount};
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
