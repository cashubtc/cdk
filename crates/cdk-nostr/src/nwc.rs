//! Nostr Wallet Connect [NIP-47](https://github.com/nostr-protocol/nips/blob/main/nip-47.md) implementation for a [`MultiMintWallet`].

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::{collections::HashSet, str::FromStr, sync::Arc, time::Duration};

use cdk::{
    amount::{to_unit, Amount},
    nuts::CurrencyUnit,
    wallet::{MultiMintWallet, Wallet},
};
use lightning_invoice::Bolt11Invoice;
use nostr_database::{MemoryDatabase, MemoryDatabaseOptions};
use nostr_sdk::{
    nips::{
        nip04,
        nip47::{self, NostrWalletConnectURI},
    },
    Alphabet, Client, EventBuilder, EventId, EventSource, Filter, FilterOptions, JsonUtil, Keys,
    Kind, PublicKey, SecretKey, SingleLetterTag, Tag, TagStandard, Timestamp,
};
use tokio::sync::{Mutex, RwLock};
use url::Url;

/// Nostr Wallet Connect implementation for a [`MultiMintWallet`].
///
/// This struct is used to create a Wallet Connect service that can be used to pay invoices and check balances.
/// The [`WalletConnection`]s must be stored externally and passed to the [`NostrWalletConnect`] instance.
#[derive(Clone)]
pub struct NostrWalletConnect {
    connections: Arc<RwLock<Vec<WalletConnection>>>,
    wallet: MultiMintWallet,

    client: Client,
    keys: Keys,
    last_check: Arc<Mutex<Timestamp>>,
    processed_events: Arc<Mutex<HashSet<EventId>>>,
}

impl NostrWalletConnect {
    /// Creates a new instance of [`NostrWalletConnect`].
    pub fn new(
        connections: Vec<WalletConnection>,
        wallet: MultiMintWallet,
        service_key: SecretKey,
    ) -> Self {
        let keys = Keys::new(service_key);
        let client = Client::builder()
            .signer(&keys)
            .database(MemoryDatabase::with_opts(MemoryDatabaseOptions {
                events: true,
                ..Default::default()
            }))
            .build();
        let mut connections = connections;
        connections.iter_mut().for_each(|conn| {
            conn.check_and_update_remaining_budget();
        });
        NostrWalletConnect {
            connections: Arc::new(RwLock::new(connections)),
            wallet,
            client,
            keys,
            last_check: Arc::new(Mutex::new(Timestamp::now())),
            processed_events: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Adds a new connection to the list of connections.
    pub async fn add_connection(&self, connection: WalletConnection) {
        let mut connections = self.connections.write().await;
        if connections
            .iter()
            .any(|conn| conn.keys.public_key() == connection.keys.public_key())
        {
            return;
        }
        connections.push(connection);
    }

    /// Waits until 1 event is received from the relays and processes it.
    pub async fn background_check_for_requests(
        &self,
        timeout: Duration,
    ) -> Result<Vec<PaymentDetails>, Error> {
        let connections = self.connections.read().await;
        if connections.is_empty() {
            tracing::debug!("No connections found");
            return Ok(Vec::new());
        }
        drop(connections);

        self.ensure_relays_connected().await?;
        let filters = self.filters().await;
        let events = self
            .client
            .pool()
            .get_events_of(filters, timeout, FilterOptions::WaitForEventsAfterEOSE(1))
            .await
            .map_err(|e| nostr_sdk::client::Error::RelayPool(e))?;
        self.handle_events(events).await
    }

    /// Checks for new requests from the relays.
    pub async fn check_for_requests(&self) -> Result<Vec<PaymentDetails>, Error> {
        let connections = self.connections.read().await;
        if connections.is_empty() {
            tracing::debug!("No connections found");
            return Ok(Vec::new());
        }
        drop(connections);

        self.ensure_relays_connected().await?;
        let filters = self.filters().await;
        let events = self
            .client
            .get_events_of(filters, EventSource::relays(None))
            .await?;
        self.handle_events(events).await
    }

    /// Gets the connection with the given secret key.
    pub async fn get_connection(&self, secret: SecretKey) -> Result<WalletConnection, Error> {
        let conn = self
            .find_connection(secret)
            .await
            .ok_or(Error::ConnectionNotFound)?;
        Ok(conn.clone())
    }

    /// Gets all the connections.
    pub async fn get_connections(&self) -> Vec<WalletConnection> {
        self.connections.read().await.clone()
    }

    /// Publishes a Wallet Connect info event.
    pub async fn publish_info(&self) -> Result<(), Error> {
        let connections = self.connections.read().await;
        if connections.is_empty() {
            return Ok(());
        }
        let event = EventBuilder::new(Kind::WalletConnectInfo, "pay_invoice get_balance", vec![])
            .to_event(&self.keys)?;
        self.client.send_event(event).await?;
        Ok(())
    }

    /// Removes a connection from the list of connections.
    pub async fn remove_connection(&self, secret: SecretKey) -> Result<(), Error> {
        let mut connections = self.connections.write().await;
        let index = connections
            .iter()
            .position(|conn| conn.keys.secret_key() == &secret)
            .ok_or(Error::ConnectionNotFound)?;
        connections.remove(index);
        Ok(())
    }

    /// Updates the budget of a connection.
    pub async fn update_budget(
        &self,
        secret: SecretKey,
        amount_msats: Amount,
        period: BudgetRenewalPeriod,
    ) -> Result<(), Error> {
        let mut connections = self.connections.write().await;
        let conn = connections
            .iter_mut()
            .find(|conn| conn.keys.secret_key() == &secret)
            .ok_or(Error::ConnectionNotFound)?;
        conn.budget.renewal_period = period;
        conn.budget.total_budget_msats = amount_msats;
        Ok(())
    }

    async fn ensure_relays_connected(&self) -> Result<(), Error> {
        let connections = self.connections.read().await;
        let urls = connections
            .iter()
            .map(|conn| conn.relay.clone())
            .collect::<HashSet<_>>();
        tracing::debug!("Relays: {:?}", urls);
        for url in &urls {
            if let Ok(relay) = self.client.relay(url).await {
                if !relay.is_connected().await {
                    tracing::debug!("Reconnecting to relay: {}", url);
                    relay.connect(Some(Duration::from_secs(5))).await;
                }
            } else {
                tracing::debug!("Adding relay: {}", url);
                self.client.add_relay(url).await?;
                self.client
                    .relay(url)
                    .await?
                    .connect(Some(Duration::from_secs(5)))
                    .await;
            }
        }
        Ok(())
    }

    async fn filters(&self) -> Vec<Filter> {
        let last_check = *self.last_check.lock().await;
        let connections = self.connections.read().await;
        connections
            .iter()
            .map(|conn| conn.filter(self.keys.public_key(), last_check))
            .collect()
    }

    async fn find_connection(&self, secret: SecretKey) -> Option<WalletConnection> {
        let connections = self.connections.read().await;
        connections
            .iter()
            .find(|conn| conn.keys.secret_key() == &secret)
            .cloned()
    }

    async fn get_wallet_to_send(&self, amount_msats: Amount) -> Result<Wallet, Error> {
        let wallets = self.wallet.get_wallets().await;
        for wallet in wallets {
            let balance = match to_unit(
                wallet.total_balance().await?,
                &wallet.unit,
                &CurrencyUnit::Msat,
            ) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if balance >= amount_msats {
                return Ok(wallet);
            }
        }
        Err(Error::InsufficientFunds)
    }

    async fn handle_events(
        &self,
        events: Vec<nostr_sdk::Event>,
    ) -> Result<Vec<PaymentDetails>, Error> {
        let mut payments = Vec::new();
        for event in events {
            if let Some(details) = self.handle_event(event).await? {
                payments.push(details);
            }
        }
        Ok(payments)
    }

    async fn handle_event(&self, event: nostr_sdk::Event) -> Result<Option<PaymentDetails>, Error> {
        let event_id = event.id;
        let mut processed_events = self.processed_events.lock().await;
        if processed_events.contains(&event_id) {
            tracing::debug!("NWC event already processed: {}", event_id);
            return Ok(None);
        }
        tracing::debug!("Processing NWC event: {}", event_id);

        let mut connections = self.connections.write().await;
        let connection = connections
            .iter_mut()
            .find(|conn| conn.keys.public_key() == event.pubkey)
            .ok_or(Error::ConnectionNotFound)?;
        let request = nip47::Request::from_json(nip04::decrypt(
            connection.keys.secret_key(),
            &self.keys.public_key(),
            &event.content,
        )?)?;
        let remaining_budget_msats = connection.check_and_update_remaining_budget();
        let (response, payment) = self.handle_request(request, remaining_budget_msats).await;
        processed_events.insert(event_id);
        if let Some(payment) = &payment {
            connection.budget.used_budget_msats += payment.total_amount_msats;
        }
        let encrypted_response = nip04::encrypt(
            connection.keys.secret_key(),
            &self.keys.public_key(),
            response.as_json(),
        )?;
        let res_event = EventBuilder::new(
            Kind::WalletConnectResponse,
            encrypted_response,
            vec![
                Tag::from_standardized(TagStandard::public_key(event.pubkey)),
                Tag::from_standardized(TagStandard::event(event_id)),
            ],
        )
        .to_event(&self.keys)?;
        self.client.send_event(res_event).await?;

        let mut last_check = self.last_check.lock().await;
        *last_check = event.created_at;
        Ok(payment)
    }

    async fn handle_request(
        &self,
        request: nip47::Request,
        remaining_budget_msats: Amount,
    ) -> (nip47::Response, Option<PaymentDetails>) {
        match request.params {
            nip47::RequestParams::PayInvoice(params) => {
                match self
                    .pay_invoice(params.invoice, remaining_budget_msats)
                    .await
                {
                    Ok(details) => (
                        nip47::Response {
                            result_type: nip47::Method::PayInvoice,
                            error: None,
                            result: Some(nip47::ResponseResult::PayInvoice(
                                nip47::PayInvoiceResponseResult {
                                    preimage: details.preimage.clone(),
                                },
                            )),
                        },
                        Some(details),
                    ),
                    Err(e) => (
                        nip47::Response {
                            result_type: nip47::Method::PayInvoice,
                            error: Some(e.into()),
                            result: None,
                        },
                        None,
                    ),
                }
            }
            nip47::RequestParams::GetBalance => match self.get_balance().await {
                Ok(balance) => (
                    nip47::Response {
                        result_type: nip47::Method::GetBalance,
                        error: None,
                        result: Some(nip47::ResponseResult::GetBalance(
                            nip47::GetBalanceResponseResult {
                                balance: balance.into(),
                            },
                        )),
                    },
                    None,
                ),
                Err(e) => (
                    nip47::Response {
                        result_type: nip47::Method::GetBalance,
                        error: Some(e.into()),
                        result: None,
                    },
                    None,
                ),
            },
            _ => (
                nip47::Response {
                    result_type: request.method,
                    error: Some(nip47::NIP47Error {
                        code: nip47::ErrorCode::NotImplemented,
                        message: "Method not implemented".to_string(),
                    }),
                    result: None,
                },
                None,
            ),
        }
    }

    async fn get_balance(&self) -> Result<Amount, Error> {
        let msat_balance = Amount::try_sum(
            self.wallet
                .get_balances(&CurrencyUnit::Msat)
                .await
                .unwrap_or_default()
                .into_values(),
        )?;
        let sat_balance = Amount::try_sum(
            self.wallet
                .get_balances(&CurrencyUnit::Sat)
                .await
                .unwrap_or_default()
                .into_values(),
        )?;
        Ok(msat_balance + (sat_balance * 1000.into()))
    }

    async fn pay_invoice(
        &self,
        invoice: String,
        remaining_budget_msats: Amount,
    ) -> Result<PaymentDetails, Error> {
        tracing::debug!("Paying invoice: {}", invoice);
        let invoice = Bolt11Invoice::from_str(&invoice)?;
        let amount_msats = Amount::from(
            invoice
                .amount_milli_satoshis()
                .ok_or(Error::InvalidInvoice)?,
        );
        tracing::debug!(
            "amount={}, remaining_budget_msats={}",
            amount_msats,
            remaining_budget_msats
        );
        if amount_msats > remaining_budget_msats {
            return Err(Error::BudgetExceeded);
        }
        let wallet = self.get_wallet_to_send(amount_msats).await?;
        let quote = wallet.melt_quote(invoice.to_string(), None).await?;
        let melted = wallet.melt(&quote.id).await?;
        Ok(PaymentDetails {
            preimage: melted.preimage.clone().unwrap_or_default(),
            payment_hash: invoice.payment_hash().to_string(),
            total_amount_msats: to_unit(melted.total_amount(), &wallet.unit, &CurrencyUnit::Msat)?,
        })
    }
}

/// A Wallet Connection.
#[derive(Debug, Clone)]
pub struct WalletConnection {
    /// The connection keys.
    pub keys: Keys,
    /// The relay to use.
    pub relay: Url,
    /// The budget of the connection.
    pub budget: ConnectionBudget,
}

impl WalletConnection {
    /// Creates a new instance of [`WalletConnection`].
    pub fn new(secret: SecretKey, relay: Url, budget: ConnectionBudget) -> Self {
        WalletConnection {
            keys: Keys::new(secret),
            relay,
            budget,
        }
    }

    /// Creates a new instance of [`WalletConnection`] from a Wallet Connect URI.
    pub fn from_uri(uri: NostrWalletConnectURI, budget: ConnectionBudget) -> Self {
        WalletConnection {
            keys: Keys::new(uri.secret),
            relay: uri.relay_url,
            budget,
        }
    }

    fn check_and_update_remaining_budget(&mut self) -> Amount {
        if let Some(renews_at) = self.budget_renews_at() {
            if renews_at < Timestamp::now() {
                self.budget.used_budget_msats = Amount::ZERO;
                self.budget.renews_at = Some(renews_at);
            }
        }
        if self.budget.used_budget_msats >= self.budget.total_budget_msats {
            return Amount::ZERO;
        }
        self.budget.total_budget_msats - self.budget.used_budget_msats
    }

    fn filter(&self, service_pubkey: PublicKey, since: Timestamp) -> Filter {
        Filter::new()
            .kind(Kind::WalletConnectRequest)
            .since(since)
            .custom_tag(
                SingleLetterTag::lowercase(Alphabet::P),
                vec![service_pubkey],
            )
    }

    /// Gets the next budget renewal timestamp.
    pub fn budget_renews_at(&self) -> Option<Timestamp> {
        let now = Timestamp::now();
        let period = match self.budget.renewal_period {
            BudgetRenewalPeriod::Daily => Duration::from_secs(24 * 60 * 60),
            BudgetRenewalPeriod::Weekly => Duration::from_secs(7 * 24 * 60 * 60),
            BudgetRenewalPeriod::Monthly => Duration::from_secs(30 * 24 * 60 * 60),
            BudgetRenewalPeriod::Yearly => Duration::from_secs(365 * 24 * 60 * 60),
            _ => return None,
        };
        let mut renews_at = match self.budget.renews_at {
            Some(t) => t,
            None => now,
        };

        loop {
            if renews_at > now {
                return Some(renews_at);
            }
            renews_at = renews_at + period;
        }
    }

    /// Gets the Wallet Connect URI for the given service public key.
    pub fn uri(&self, service_pubkey: PublicKey, lud16: Option<String>) -> Result<String, Error> {
        let uri = NostrWalletConnectURI::new(
            service_pubkey,
            self.relay.clone(),
            self.keys.secret_key().clone(),
            lud16,
        );
        Ok(uri.to_string())
    }
}

/// A Wallet Connection Budget.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionBudget {
    /// The renewal period of the budget.
    pub renewal_period: BudgetRenewalPeriod,
    /// When the budget renews next.
    pub renews_at: Option<Timestamp>,
    /// The total budget in millisatoshis.
    pub total_budget_msats: Amount,
    /// The used budget in millisatoshis.
    pub used_budget_msats: Amount,
}

impl Default for ConnectionBudget {
    fn default() -> Self {
        ConnectionBudget {
            renewal_period: BudgetRenewalPeriod::Never,
            renews_at: None,
            total_budget_msats: Amount::from(1_000_000), // 1_000_000 msats
            used_budget_msats: Amount::ZERO,
        }
    }
}

/// A Budget Renewal Period.
#[derive(Debug, Clone, Copy)]
pub enum BudgetRenewalPeriod {
    /// Daily (24 hours).
    Daily,
    /// Weekly (7 days).
    Weekly,
    /// Monthly (30 days).
    Monthly,
    /// Yearly (365 days).
    Yearly,
    /// Never.
    Never,
}

/// Payment Details for a `pay_invoice` request.
#[derive(Debug, Clone)]
pub struct PaymentDetails {
    /// The preimage of the payment.
    pub preimage: String,
    /// The payment hash.
    pub payment_hash: String,
    /// The total amount paid in millisatoshis.
    pub total_amount_msats: Amount,
}

/// Errors that can occur when using the Nostr Wallet Connect service.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// CDK Amount error.
    #[error(transparent)]
    Amount(#[from] cdk::amount::Error),
    /// Budget exceeded error.
    #[error("Budget exceeded")]
    BudgetExceeded,
    /// Client error.
    #[error(transparent)]
    Client(#[from] nostr_sdk::client::Error),
    /// Connection not found error.
    #[error("Connection not found")]
    ConnectionNotFound,
    /// Error creating an event.
    #[error(transparent)]
    EventBuilder(#[from] nostr_sdk::event::builder::Error),
    /// Insufficient funds error.
    #[error("Insufficient funds")]
    InsufficientFunds,
    /// Invalid invoice error.
    #[error("Invalid invoice")]
    InvalidInvoice,
    /// Error parsing an invoice.
    #[error(transparent)]
    InvoiceParse(#[from] lightning_invoice::ParseOrSemanticError),
    /// Nostr key error.
    #[error(transparent)]
    Key(#[from] nostr_sdk::key::Error),
    /// CDK Lightning error.
    #[error(transparent)]
    Lightning(#[from] cdk::cdk_lightning::Error),
    /// NIP-04 error.
    #[error(transparent)]
    Nip04(#[from] nip04::Error),
    /// NIP-47 error.
    #[error(transparent)]
    Nip47(#[from] nip47::Error),
    /// CDK Wallet error.
    #[error(transparent)]
    Wallet(#[from] cdk::Error),
}

impl Into<nip47::NIP47Error> for Error {
    fn into(self) -> nip47::NIP47Error {
        match self {
            Error::BudgetExceeded => nip47::NIP47Error {
                code: nip47::ErrorCode::QuotaExceeded,
                message: "Budget exceeded".to_string(),
            },
            Error::InsufficientFunds => nip47::NIP47Error {
                code: nip47::ErrorCode::InsufficientBalance,
                message: "Insufficient funds".to_string(),
            },
            e => nip47::NIP47Error {
                code: nip47::ErrorCode::Internal,
                message: e.to_string(),
            },
        }
    }
}
