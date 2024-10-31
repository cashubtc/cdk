//! CDK implementation of Nostr Wallet Connect (NIP-47).

use std::{collections::HashMap, str::FromStr, sync::Arc, time::Duration};

use cdk::{
    amount::{to_unit, Amount},
    mint_url::MintUrl,
    nuts::CurrencyUnit,
    wallet::{multi_mint_wallet::WalletKey, MultiMintWallet, Wallet},
};
use lightning_invoice::Bolt11Invoice;
use nostr_relay_pool::Output;
use nostr_sdk::{
    nips::{
        nip04,
        nip47::{self, MakeInvoiceResponseResult, NostrWalletConnectURI},
    },
    Alphabet, Client, Event, EventBuilder, EventId, EventSource, Filter, JsonUtil, Keys, Kind,
    PublicKey, SecretKey, SingleLetterTag, Tag, TagKind, TagStandard, Timestamp, Url,
};
use tokio::sync::{Mutex, RwLock};

/// Nostr Wallet Connect implementation for a [`MultiMintWallet`].
///
/// This struct is used to create a Wallet Connect service that can be used to pay invoices and check balances.
/// The [`WalletConnection`]s must be stored externally and passed to the [`NostrWalletConnect`] instance.
/// The budget of each connection is updated automatically based on the [`ConnectionBudget`] settings.
/// To persist the budget, the user must store it externally and pass it to the [`WalletConnection`] constructor.
#[derive(Clone)]
pub struct NostrWalletConnect {
    connections: Arc<RwLock<Vec<WalletConnection>>>,
    default_mint_url: Option<MintUrl>,
    keys: Keys,
    last_check: Arc<Mutex<Timestamp>>,
    wallet: MultiMintWallet,

    response_event_cache: Arc<Mutex<HashMap<EventId, (Event, Option<PaymentDetails>)>>>,
}

impl NostrWalletConnect {
    /// Creates a new instance of [`NostrWalletConnect`].
    pub fn new(
        connections: Vec<WalletConnection>,
        service_key: SecretKey,
        wallet: MultiMintWallet,
        last_check: Option<Timestamp>,
        default_mint_url: Option<MintUrl>,
    ) -> Self {
        Self {
            connections: Arc::new(RwLock::new(connections)),
            default_mint_url,
            keys: Keys::new(service_key),
            last_check: Arc::new(Mutex::new(last_check.unwrap_or(Timestamp::now()))),
            wallet,

            response_event_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Adds a new connection to the list of connections.
    pub async fn add_connection(&self, connection: WalletConnection) -> Result<(), Error> {
        let mut connections = self.connections.write().await;
        if connections
            .iter()
            .any(|conn| conn.keys.public_key() == connection.keys.public_key())
        {
            return Ok(());
        }
        connections.push(connection);
        Ok(())
    }

    /// Gets the connection with the given secret key.
    pub async fn get_connection(&self, secret: SecretKey) -> Result<WalletConnection, Error> {
        let connections = self.connections.read().await;
        Ok(connections
            .iter()
            .find(|conn| conn.keys.secret_key() == &secret)
            .cloned()
            .ok_or(Error::ConnectionNotFound)?)
    }

    /// Gets all the connections.
    pub async fn get_connections(&self) -> Vec<WalletConnection> {
        self.connections.read().await.clone()
    }

    /// Creates a kind 13194 event for the NWC wallet info.
    pub fn info_event(&self) -> Result<Event, Error> {
        let event = EventBuilder::new(
            Kind::WalletConnectInfo,
            "get_balance make_invoice pay_invoice",
            vec![],
        )
        .to_event(&self.keys)?;
        Ok(event)
    }

    /// Nostr filter for the NWC service.
    pub async fn filters(&self) -> Vec<Filter> {
        let last_check = *self.last_check.lock().await;
        let connections = self.connections.read().await;
        connections
            .iter()
            .map(|conn| conn.filter(self.keys.public_key(), last_check))
            .collect()
    }

    /// Handle a NWC request event.
    pub async fn handle_event(
        &self,
        event: Event,
    ) -> Result<(Event, Option<PaymentDetails>), Error> {
        if event.kind != Kind::WalletConnectRequest {
            return Err(Error::InvalidKind);
        }
        let service_pubkey = PublicKey::from_str(
            event
                .get_tag_content(TagKind::SingleLetter(SingleLetterTag::lowercase(
                    Alphabet::P,
                )))
                .ok_or(Error::MissingServiceKey)?,
        )?;
        if service_pubkey != self.keys.public_key() {
            return Err(Error::InvalidServiceKey(service_pubkey));
        }

        let event_id = event.id;
        let mut response_events = self.response_event_cache.lock().await;
        if let Some(res) = response_events.get(&event_id) {
            return Ok(res.clone());
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
        response_events.insert(event_id, (res_event.clone(), payment.clone()));

        let mut last_check = self.last_check.lock().await;
        *last_check = event.created_at;
        Ok((res_event, payment))
    }

    /// Process NWC events using Nostr [`Client`].
    pub async fn process_events(
        &self,
        client: &Client,
        timeout: Option<Duration>,
    ) -> Result<Vec<PaymentDetails>, Error> {
        let mut payments = Vec::new();
        let events = self.query_events(client, timeout).await?;
        for event in events {
            let event_id = event.id;
            let (res_event, payment) = self.handle_event(event).await?;
            if let Some(payment) = payment {
                payments.push(payment);
            }
            match client.send_event(res_event).await {
                Ok(output) => tracing::debug!(
                    "Processed NWC event ({}): responded with {} (success={}, failure={})",
                    event_id,
                    output.val,
                    output.success.len(),
                    output.failed.len()
                ),
                Err(e) => tracing::error!("Error processing NWC event ({}): {}", event_id, e),
            }
        }
        Ok(payments)
    }

    /// Publishes a NWC info event.
    pub async fn publish_info_event(&self, client: &Client) -> Result<Output<EventId>, Error> {
        let event = self.info_event()?;
        Ok(client.send_event(event).await?)
    }

    /// Query NWC events.
    pub async fn query_events(
        &self,
        client: &Client,
        timeout: Option<Duration>,
    ) -> Result<Vec<Event>, Error> {
        let filters = self.filters().await;
        Ok(client
            .get_events_of(filters, EventSource::relays(timeout))
            .await?)
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
        Ok(msat_balance + to_unit(sat_balance, &CurrencyUnit::Sat, &CurrencyUnit::Msat)?)
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

    async fn handle_request(
        &self,
        request: nip47::Request,
        remaining_budget_msats: Amount,
    ) -> (nip47::Response, Option<PaymentDetails>) {
        match request.params {
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
            nip47::RequestParams::MakeInvoice(params) => {
                match self
                    .make_invoice(params.amount.into(), params.description)
                    .await
                {
                    Ok(invoice) => (
                        nip47::Response {
                            result_type: nip47::Method::MakeInvoice,
                            error: None,
                            result: Some(nip47::ResponseResult::MakeInvoice(
                                MakeInvoiceResponseResult {
                                    invoice: invoice.to_string(),
                                    payment_hash: invoice.payment_hash().to_string(),
                                },
                            )),
                        },
                        None,
                    ),
                    Err(e) => (
                        nip47::Response {
                            result_type: nip47::Method::MakeInvoice,
                            error: Some(e.into()),
                            result: None,
                        },
                        None,
                    ),
                }
            }
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

    async fn make_invoice(
        &self,
        amount_msats: Amount,
        description: Option<String>,
    ) -> Result<Bolt11Invoice, Error> {
        let first_wallet = self
            .wallet
            .get_wallets()
            .await
            .into_iter()
            .next()
            .map(|w| w.mint_url);
        let mint_url = self
            .default_mint_url
            .clone()
            .or(first_wallet)
            .ok_or(Error::NoWallet)?;

        let (wallet, amount) = match self
            .wallet
            .get_wallet(&WalletKey::new(mint_url.clone(), CurrencyUnit::Msat))
            .await
        {
            Some(wallet) => (wallet, amount_msats),
            None => match self
                .wallet
                .get_wallet(&WalletKey::new(mint_url, CurrencyUnit::Sat))
                .await
            {
                Some(wallet) => (
                    wallet,
                    to_unit(amount_msats, &CurrencyUnit::Msat, &CurrencyUnit::Sat)?,
                ),
                None => return Err(Error::NoWallet),
            },
        };

        let quote = wallet.mint_quote(amount, description).await?;
        let invoice = Bolt11Invoice::from_str(&quote.request)?;
        Ok(invoice)
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
        if let Some(renews_at) = self.budget.renews_at {
            if renews_at <= Timestamp::now() {
                self.budget.used_budget_msats = Amount::ZERO;
                self.budget.renews_at = self.budget_renews_at();
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
            .author(self.keys.public_key())
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
    /// Invalid kind error.
    #[error("Invalid kind")]
    InvalidKind,
    /// Invalid service key error.
    #[error("Invalid service key: {0}")]
    InvalidServiceKey(PublicKey),
    /// Error parsing an invoice.
    #[error(transparent)]
    InvoiceParse(#[from] lightning_invoice::ParseOrSemanticError),
    /// Nostr key error.
    #[error(transparent)]
    Key(#[from] nostr_sdk::key::Error),
    /// Missing service key error.
    #[error("Missing service key")]
    MissingServiceKey,
    /// NIP-04 error.
    #[error(transparent)]
    Nip04(#[from] nip04::Error),
    /// NIP-47 error.
    #[error(transparent)]
    Nip47(#[from] nip47::Error),
    /// No wallet error.
    #[error("No wallet")]
    NoWallet,
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use nostr_sdk::{SecretKey, Timestamp, Url};

    use super::{BudgetRenewalPeriod, ConnectionBudget, WalletConnection};

    #[test]
    fn test_connection_budget_update() {
        let now = Timestamp::now();
        let mut connection = WalletConnection::new(
            SecretKey::generate(),
            Url::from_str("ws://localhost:7777").unwrap(),
            ConnectionBudget {
                renewal_period: BudgetRenewalPeriod::Daily,
                renews_at: Some(now - (24 * 60 * 60 + 2)),
                total_budget_msats: 1_000_000.into(),
                used_budget_msats: 21_000.into(),
            },
        );
        let remaining_amount = connection.check_and_update_remaining_budget();
        assert_eq!(remaining_amount, 1_000_000.into());
        assert_eq!(connection.budget.used_budget_msats, 0.into());

        let new_renews_at = connection.budget.renews_at.unwrap();
        assert!(new_renews_at == now + (24 * 60 * 60 - 2));
    }
}
