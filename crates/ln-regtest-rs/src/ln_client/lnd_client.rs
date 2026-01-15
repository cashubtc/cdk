//! LND Client

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use fedimint_tonic_lnd::lnrpc::{
    ConnectPeerRequest, GetInfoRequest, GetInfoResponse, LightningAddress, ListChannelsRequest,
    NewAddressRequest, OpenChannelRequest, WalletBalanceRequest,
};
use fedimint_tonic_lnd::Client;
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::sleep;

use super::types::{Balance, ConnectInfo};
use super::LightningClient;
use crate::{hex, InvoiceStatus};

/// Lnd
#[derive(Clone)]
pub struct LndClient {
    pub address: String,
    pub cert_file: PathBuf,
    pub macaroon_file: PathBuf,
    client: Arc<Mutex<Client>>,
}

impl LndClient {
    /// Create rpc client
    pub async fn new(addr: String, cert_file: PathBuf, macaroon_file: PathBuf) -> Result<Self> {
        let client =
            fedimint_tonic_lnd::connect(addr.clone(), cert_file.clone(), macaroon_file.clone())
                .await
                .map_err(|_err| anyhow!("Could not connect to lnd rpc"))?;

        Ok(LndClient {
            address: addr,
            cert_file,
            macaroon_file,
            client: Arc::new(Mutex::new(client)),
        })
    }

    /// Get node info
    pub async fn get_info(&self) -> Result<GetInfoResponse> {
        let client = &self.client;

        let get_info_request = GetInfoRequest {};

        let info = client
            .lock()
            .await
            .lightning()
            .get_info(get_info_request)
            .await?
            .into_inner();

        Ok(info)
    }

    pub async fn list_channels(&self) -> Result<()> {
        let channels = self
            .client
            .lock()
            .await
            .lightning()
            .list_channels(ListChannelsRequest {
                active_only: false,
                inactive_only: false,
                public_only: false,
                private_only: false,
                peer: vec![],
            })
            .await?
            .into_inner();

        for channel in channels.channels {
            println!("{:?}", channel);
        }

        Ok(())
    }

    pub async fn channels_balance(&self) -> Result<u64> {
        let channels = self
            .client
            .lock()
            .await
            .lightning()
            .list_channels(ListChannelsRequest {
                active_only: false,
                inactive_only: false,
                public_only: false,
                private_only: false,
                peer: vec![],
            })
            .await?
            .into_inner();

        let balance = channels
            .channels
            .iter()
            .map(|c| c.local_balance)
            .sum::<i64>();

        Ok(balance as u64)
    }

    pub async fn add_hold_invoice(&self, payment_hash: &str, amount_sat: u64) -> Result<String> {
        let rpc_server = self.address.trim_start_matches("https://");

        let output = Command::new("lncli")
            .arg(format!("--rpcserver={}", rpc_server))
            .arg(format!("--tlscertpath={}", self.cert_file.display()))
            .arg(format!("--macaroonpath={}", self.macaroon_file.display()))
            .arg("addholdinvoice")
            .arg(payment_hash)
            .arg(amount_sat.to_string())
            .output()
            .await?;

        if !output.status.success() {
            bail!(
                "Failed to create hold invoice: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8(output.stdout)?;
        let json: Value = serde_json::from_str(&stdout)?;

        json["payment_request"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("No payment_request in response"))
    }

    pub async fn cancel_invoice(&self, payment_hash: &str) -> Result<()> {
        let rpc_server = self.address.trim_start_matches("https://");

        let output = Command::new("lncli")
            .arg(format!("--rpcserver={}", rpc_server))
            .arg(format!("--tlscertpath={}", self.cert_file.display()))
            .arg(format!("--macaroonpath={}", self.macaroon_file.display()))
            .arg("cancelinvoice")
            .arg(payment_hash)
            .output()
            .await?;

        if !output.status.success() {
            bail!(
                "Failed to cancel invoice: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    pub async fn settle_invoice(&self, preimage: &str) -> Result<()> {
        let rpc_server = self.address.trim_start_matches("https://");

        let output = Command::new("lncli")
            .arg(format!("--rpcserver={}", rpc_server))
            .arg(format!("--tlscertpath={}", self.cert_file.display()))
            .arg(format!("--macaroonpath={}", self.macaroon_file.display()))
            .arg("settleinvoice")
            .arg(preimage)
            .output()
            .await?;

        if !output.status.success() {
            bail!(
                "Failed to settle invoice: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    pub async fn wait_for_hold_invoice_accepted(&self, payment_hash: &str) -> Result<()> {
        let rpc_server = self.address.trim_start_matches("https://");
        let start = std::time::Instant::now();
        let mut last_state: Option<String> = None;

        loop {
            if start.elapsed() > Duration::from_secs(120) {
                bail!(
                    "Timeout waiting for hold invoice to be accepted (last state: {})",
                    last_state.as_deref().unwrap_or("unknown")
                );
            }

            let output = Command::new("lncli")
                .arg(format!("--rpcserver={}", rpc_server))
                .arg(format!("--tlscertpath={}", self.cert_file.display()))
                .arg(format!("--macaroonpath={}", self.macaroon_file.display()))
                .arg("lookupinvoice")
                .arg(payment_hash)
                .output()
                .await?;

            if output.status.success() {
                let stdout = String::from_utf8(output.stdout)?;
                let json: Value = serde_json::from_str(&stdout)?;

                if let Some(state) = json["state"].as_str() {
                    last_state = Some(state.to_string());
                    match state {
                        "ACCEPTED" | "SETTLED" => return Ok(()),
                        "CANCELED" => bail!("Hold invoice was cancelled"),
                        _ => {}
                    }
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    }
}

#[async_trait]
impl LightningClient for LndClient {
    async fn get_connect_info(&self) -> Result<ConnectInfo> {
        let info = self.get_info().await?;
        let uri = info.uris.first().unwrap();

        let parsed = parse_uri(uri);

        Ok(parsed.unwrap())
    }

    async fn get_new_onchain_address(&self) -> Result<String> {
        let client = &self.client;

        let new_address_request = NewAddressRequest {
            r#type: 0,
            account: "".to_string(),
        };

        let new_address_response = client
            .lock()
            .await
            .lightning()
            .new_address(new_address_request)
            .await?
            .into_inner();

        Ok(new_address_response.address.to_string())
    }

    async fn connect_peer(&self, pubkey: String, addr: String, port: u16) -> Result<()> {
        let client = &self.client;

        let host = format!("{}:{}", addr, port);

        let lightning_addr = LightningAddress { pubkey, host };

        let connect_peer_request = ConnectPeerRequest {
            addr: Some(lightning_addr),
            perm: false,
            timeout: 60,
        };

        let _connect_peer = client
            .lock()
            .await
            .lightning()
            .connect_peer(connect_peer_request)
            .await?
            .into_inner();

        tracing::info!("LND connected to peer");

        Ok(())
    }

    async fn open_channel(
        &self,
        amount_sat: u64,
        peer_id: &str,
        push_amount: Option<u64>,
    ) -> Result<()> {
        let client = &self.client;

        let open_channel_request = OpenChannelRequest {
            node_pubkey: hex::decode(peer_id)?,
            push_sat: push_amount.unwrap_or_default() as i64,
            local_funding_amount: amount_sat as i64,
            ..Default::default()
        };

        let _connect_peer = client
            .lock()
            .await
            .lightning()
            .open_channel_sync(open_channel_request)
            .await?
            .into_inner();

        tracing::info!("LND opened channel");

        Ok(())
    }

    async fn balance(&self) -> Result<Balance> {
        let client = &self.client;

        let response = client
            .lock()
            .await
            .lightning()
            .wallet_balance(WalletBalanceRequest {})
            .await?
            .into_inner();

        let ln = self.channels_balance().await?;

        Ok(Balance {
            on_chain_spendable: response.confirmed_balance as u64,
            on_chain_total: response.total_balance as u64,
            ln,
        })
    }

    async fn pay_invoice(&self, bolt11: String) -> Result<String> {
        let pay_req = fedimint_tonic_lnd::lnrpc::SendRequest {
            payment_request: bolt11,
            ..Default::default()
        };

        let payment_response = self
            .client
            .lock()
            .await
            .lightning()
            .send_payment_sync(fedimint_tonic_lnd::tonic::Request::new(pay_req))
            .await?
            .into_inner();

        if !payment_response.payment_error.is_empty() {
            bail!("Lnd payment error: {}", payment_response.payment_error);
        }

        Ok(hex::encode(payment_response.payment_preimage))
    }

    async fn create_invoice(&self, amount_sat: Option<u64>) -> Result<String> {
        let value_msat = amount_sat.map(|a| (a * 1_000) as i64).unwrap_or(0);

        let invoice_request = fedimint_tonic_lnd::lnrpc::Invoice {
            value_msat,
            ..Default::default()
        };

        let invoice = self
            .client
            .lock()
            .await
            .lightning()
            .add_invoice(fedimint_tonic_lnd::tonic::Request::new(invoice_request))
            .await
            .unwrap()
            .into_inner();

        Ok(invoice.payment_request)
    }

    async fn wait_channels_active(&self) -> Result<()> {
        let mut count = 0;
        while count < 100 {
            let pending = self
                .client
                .lock()
                .await
                .lightning()
                .list_channels(ListChannelsRequest {
                    inactive_only: true,
                    active_only: false,
                    public_only: false,
                    private_only: false,
                    peer: vec![],
                })
                .await?
                .into_inner();

            if pending.channels.is_empty() {
                tracing::info!("All LND channels active");
                return Ok(());
            }

            count += 1;

            sleep(Duration::from_secs(2)).await;
        }

        bail!("Timeout waiting for pending")
    }

    async fn wait_chain_sync(&self) -> Result<()> {
        let mut count = 0;
        while count < 100 {
            let info = self.get_info().await?;

            if info.synced_to_chain {
                tracing::info!("LND completed chain sync");
                return Ok(());
            }
            count += 1;

            sleep(Duration::from_secs(2)).await;
        }

        bail!("Time out exceeded")
    }

    async fn check_incoming_payment_status(&self, payment_hash: &str) -> Result<InvoiceStatus> {
        let invoice_request = fedimint_tonic_lnd::lnrpc::PaymentHash {
            r_hash: hex::decode(payment_hash)?,
            ..Default::default()
        };

        let invoice = self
            .client
            .lock()
            .await
            .lightning()
            .lookup_invoice(fedimint_tonic_lnd::tonic::Request::new(invoice_request))
            .await
            .unwrap()
            .into_inner();

        match invoice.state {
            // Open
            0 => Ok(InvoiceStatus::Unpaid),
            // Settled
            1 => Ok(InvoiceStatus::Paid),
            // Canceled
            2 => Ok(InvoiceStatus::Unpaid),
            // Accepted
            3 => Ok(InvoiceStatus::Unpaid),
            _ => bail!("Unknown state"),
        }
    }

    async fn check_outgoing_payment_status(&self, payment_hash: &str) -> Result<InvoiceStatus> {
        let invoice_request = fedimint_tonic_lnd::lnrpc::ListPaymentsRequest {
            include_incomplete: true,
            index_offset: 0,
            max_payments: 1000,
            reversed: false,
            count_total_payments: false,
        };

        let invoices = self
            .client
            .lock()
            .await
            .lightning()
            .list_payments(invoice_request)
            .await
            .unwrap()
            .into_inner();

        let invoice: Vec<&fedimint_tonic_lnd::lnrpc::Payment> = invoices
            .payments
            .iter()
            .filter(|p| p.payment_hash == payment_hash)
            .collect();

        if invoice.len() != 1 {
            bail!("Could not find invoice");
        }

        let invoice = invoice.first().expect("Checked len is one");

        match invoice.status {
            // Open
            0 => Ok(InvoiceStatus::Unpaid),
            // Settled
            1 => Ok(InvoiceStatus::Paid),
            // Canceled
            2 => Ok(InvoiceStatus::Unpaid),
            // Accepted
            3 => Ok(InvoiceStatus::Unpaid),
            _ => bail!("Unknown state"),
        }
    }

    async fn pay_bolt12_offer(&self, _offer: &str, _amount_msats: Option<u64>) -> Result<String> {
        todo!()
    }
}

fn parse_uri(uri: &str) -> Option<ConnectInfo> {
    // Split at the '@' symbol to separate the node_id and the rest (addr and port)
    let parts: Vec<&str> = uri.split('@').collect();

    if parts.len() != 2 {
        return None; // If the format is invalid
    }

    let node_id = parts[0].to_string();
    let address_parts: Vec<&str> = parts[1].split(':').collect();

    if address_parts.len() != 2 {
        return None; // If the address and port format is invalid
    }

    let addr = address_parts[0].to_string();
    let port: u16 = address_parts[1].parse().ok()?; // Parse the port as u16

    Some(ConnectInfo {
        pubkey: node_id,
        address: addr,
        port,
    })
}
