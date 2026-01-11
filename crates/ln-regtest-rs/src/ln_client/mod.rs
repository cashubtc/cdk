use anyhow::Result;
use async_trait::async_trait;

use self::types::{Balance, ConnectInfo};
use crate::InvoiceStatus;

pub mod cln_client;
pub mod lnd_client;
pub mod types;

pub use cln_client::ClnClient;
pub use lnd_client::LndClient;

#[async_trait]
pub trait LightningClient {
    /// Get info required to connect to the node
    async fn get_connect_info(&self) -> Result<ConnectInfo>;

    /// Get new onchain address
    async fn get_new_onchain_address(&self) -> Result<String>;

    /// Connect to a peer
    async fn connect_peer(&self, pubkey: String, addr: String, port: u16) -> Result<()>;

    /// Open channel to peer
    async fn open_channel(
        &self,
        amount_sat: u64,
        peer_id: &str,
        push_amount: Option<u64>,
    ) -> Result<()>;

    /// Balance
    async fn balance(&self) -> Result<Balance>;

    /// Pa bolt11 invoice
    async fn pay_invoice(&self, bolt11: String) -> Result<String>;

    /// Create bolt11 invoice    
    async fn create_invoice(&self, amount_sat: Option<u64>) -> Result<String>;

    /// Wait for all channel to be active
    async fn wait_channels_active(&self) -> Result<()>;

    /// Wait for chain sync
    async fn wait_chain_sync(&self) -> Result<()>;

    /// Check incoming invoice status
    async fn check_incoming_payment_status(&self, payment_hash: &str) -> Result<InvoiceStatus>;

    /// Check outgoing invoice status
    async fn check_outgoing_payment_status(&self, payment_hash: &str) -> Result<InvoiceStatus>;

    /// Pay bolt12
    async fn pay_bolt12_offer(&self, offer: &str, amount_msats: Option<u64>) -> Result<String>;
}
