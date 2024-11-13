use std::{
    any::type_name, cmp::Ordering, collections::HashMap, fmt::Debug, net::SocketAddr,
    path::PathBuf, str::FromStr, sync::Arc, time::SystemTime,
};

use bitcoin::secp256k1::PublicKey;
use cdk::{amount::Amount, Bolt11Invoice};
use chrono::{DateTime, Utc};
use lightning::{
    events::Event,
    ln::{types::ChannelId, PaymentHash, PaymentPreimage},
    sign::SpendableOutputDescriptor,
    util::ser::{MaybeReadable, Writeable},
};
use redb::{Database, Key, ReadableTable, TableDefinition, TypeName, Value};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::Error;

// property key -> value
const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
// channeld id -> channel
const CHANNELS_TABLE: TableDefinition<[u8; 32], Bincode<Channel>> =
    TableDefinition::new("channels");
// timestamp -> event
const EVENTS_TABLE: TableDefinition<u128, Vec<u8>> = TableDefinition::new("events");
// payment hash -> invoice
const INVOICES_TABLE: TableDefinition<[u8; 32], Bincode<Invoice>> =
    TableDefinition::new("invoices");
// payment hash -> payment
const PAYMENTS_TABLE: TableDefinition<[u8; 32], Bincode<Payment>> =
    TableDefinition::new("payments");
// node id -> socket address
const PEERS_TABLE: TableDefinition<Bincode<PublicKey>, Bincode<SocketAddr>> =
    TableDefinition::new("peers");
// channel id -> spendable outputs
const SPENDABLE_OUTPUTS_TABLE: TableDefinition<[u8; 32], Bincode<Vec<Vec<u8>>>> =
    TableDefinition::new("spendable_outputs");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub node_id: PublicKey,
    pub amount: Amount,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invoice {
    pub bolt_11: String,
    pub expiry: u64,
    pub paid: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Payment {
    pub bolt_11: String,
    pub amount: Amount,
    pub paid: bool,
    pub spent: Amount,
    pub pre_image: Option<[u8; 32]>,
}

const DATABASE_VERSION: u64 = 0;

#[derive(Clone)]
pub struct NodeDatabase {
    db: Arc<RwLock<Database>>,
}

impl NodeDatabase {
    pub fn open(path: PathBuf) -> Result<Self, Error> {
        let db = Database::create(path)?;

        let write_txn = db.begin_write()?;
        // Check database version
        {
            let _ = write_txn.open_table(CONFIG_TABLE)?;
            let mut table = write_txn.open_table(CONFIG_TABLE)?;

            let db_version = table.get("db_version")?;
            let db_version = db_version.map(|v| v.value().to_owned());

            match db_version {
                Some(db_version) => {
                    let current_file_version = u64::from_str(&db_version)?;
                    if current_file_version.ne(&DATABASE_VERSION) {
                        // Database needs to be upgraded
                        todo!()
                    }
                }
                None => {
                    // Open all tables to init a new db
                    let _ = write_txn.open_table(CHANNELS_TABLE)?;
                    let _ = write_txn.open_table(EVENTS_TABLE)?;
                    let _ = write_txn.open_table(INVOICES_TABLE)?;
                    let _ = write_txn.open_table(PAYMENTS_TABLE)?;
                    let _ = write_txn.open_table(PEERS_TABLE)?;

                    table.insert("db_version", "0")?;
                }
            }
        }

        write_txn.commit()?;
        Ok(Self {
            db: Arc::new(RwLock::new(db)),
        })
    }

    pub async fn save_event(&self, event: Event) -> Result<(), Error> {
        let data = event.encode();
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos();
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(EVENTS_TABLE)?;
            if !data.is_empty() {
                table.insert(timestamp, data)?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    pub async fn get_events(
        &self,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    ) -> Result<Vec<(DateTime<Utc>, Event)>, Error> {
        let start = start
            .map(|s| s.timestamp_nanos_opt().map(|s| s as u128))
            .flatten()
            .unwrap_or(0);
        let end = end
            .map(|e| e.timestamp_nanos_opt().map(|e| e as u128))
            .flatten()
            .unwrap_or(u128::MAX);
        let mut events = Vec::new();
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(EVENTS_TABLE)?;
        let entries = table.range(start..end)?;
        for entry in entries {
            let (timestamp, data) = entry?;
            if let Some(event) = Event::read(&mut &data.value()[..])? {
                events.push((
                    DateTime::from_timestamp_nanos(timestamp.value() as i64),
                    event,
                ));
            }
        }
        Ok(events)
    }

    pub async fn insert_temp_channel(
        &self,
        channel_id: ChannelId,
        channel: Channel,
    ) -> Result<(), Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(CHANNELS_TABLE)?;
            table.insert(channel_id.0, &channel)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub async fn update_channel_id(
        &self,
        temp_channel_id: ChannelId,
        channel_id: ChannelId,
    ) -> Result<Option<Channel>, Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        let mut channel = {
            let table = write_txn.open_table(CHANNELS_TABLE)?;
            let entry = table.get(temp_channel_id.0)?;
            entry.map(|e| e.value())
        };
        if let Some(channel) = channel.as_mut() {
            let mut table = write_txn.open_table(CHANNELS_TABLE)?;
            table.insert(channel_id.0, channel)?;
            table.remove(temp_channel_id.0)?;
        }
        write_txn.commit()?;
        Ok(channel)
    }

    pub async fn get_channel(&self, channel_id: ChannelId) -> Result<Option<Channel>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(CHANNELS_TABLE)?;
        let entry = table.get(channel_id.0)?;
        Ok(entry.map(|e| e.value()))
    }

    pub async fn insert_invoice(&self, invoice: &Bolt11Invoice) -> Result<(), Error> {
        let payment_hash = invoice.payment_hash();
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(INVOICES_TABLE)?;
            table.insert(
                payment_hash.as_ref(),
                &Invoice {
                    bolt_11: invoice.to_string(),
                    expiry: invoice.expires_at().unwrap_or_default().as_secs(),
                    paid: false,
                },
            )?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub async fn update_paid_invoice(
        &self,
        payment_hash: PaymentHash,
    ) -> Result<Option<Invoice>, Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        let mut invoice = {
            let table = write_txn.open_table(INVOICES_TABLE)?;
            let entry = table.get(payment_hash.0)?;
            entry.map(|e| e.value())
        };
        if let Some(invoice) = invoice.as_mut() {
            let mut table = write_txn.open_table(INVOICES_TABLE)?;
            invoice.paid = true;
            table.insert(payment_hash.0, invoice)?;
        }
        write_txn.commit()?;
        Ok(invoice)
    }

    pub async fn get_invoice(&self, payment_hash: PaymentHash) -> Result<Option<Invoice>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(INVOICES_TABLE)?;
        let entry = table.get(payment_hash.0)?;
        Ok(entry.map(|e| e.value()))
    }

    pub async fn insert_payment(
        &self,
        invoice: &Bolt11Invoice,
        amount: Amount,
    ) -> Result<(), Error> {
        let payment_hash = invoice.payment_hash();
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(PAYMENTS_TABLE)?;
            table.insert(
                payment_hash.as_ref(),
                Payment {
                    bolt_11: invoice.to_string(),
                    amount,
                    paid: false,
                    spent: Amount::ZERO,
                    pre_image: None,
                },
            )?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub async fn update_payment(
        &self,
        payment_hash: PaymentHash,
        pre_image: PaymentPreimage,
        fee_paid: Amount,
    ) -> Result<Option<Payment>, Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        let mut payment = {
            let table = write_txn.open_table(PAYMENTS_TABLE)?;
            let entry = table.get(payment_hash.0)?;
            entry.map(|e| e.value())
        };
        if let Some(payment) = payment.as_mut() {
            let mut table = write_txn.open_table(PAYMENTS_TABLE)?;
            payment.paid = true;
            payment.spent = payment.amount + fee_paid;
            payment.pre_image = Some(pre_image.0);
            table.insert(payment_hash.0, payment)?;
        }
        write_txn.commit()?;
        Ok(payment)
    }

    pub async fn get_payment(&self, payment_hash: PaymentHash) -> Result<Option<Payment>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(PAYMENTS_TABLE)?;
        let entry = table.get(payment_hash.0)?;
        Ok(entry.map(|e| e.value()))
    }

    pub async fn insert_peer_address(
        &self,
        node_id: PublicKey,
        addr: SocketAddr,
    ) -> Result<(), Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(PEERS_TABLE)?;
            table.insert(node_id, addr)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub async fn get_peer_address(&self, node_id: PublicKey) -> Result<Option<SocketAddr>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(PEERS_TABLE)?;
        let entry = table.get(node_id)?;
        Ok(entry.map(|e| e.value()))
    }

    pub async fn insert_spendable_outputs(
        &self,
        channel_id: ChannelId,
        outputs: Vec<SpendableOutputDescriptor>,
    ) -> Result<(), Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(SPENDABLE_OUTPUTS_TABLE)?;
            table.insert(
                channel_id.0,
                &outputs.into_iter().map(|o| o.encode()).collect(),
            )?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub async fn get_all_spendable_outputs(
        &self,
    ) -> Result<HashMap<ChannelId, Vec<SpendableOutputDescriptor>>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(SPENDABLE_OUTPUTS_TABLE)?;
        Ok(table
            .iter()?
            .filter_map(|e| {
                let (channel_id, outputs) = e.ok()?;
                let outputs = outputs.value();
                let outputs = outputs
                    .into_iter()
                    .filter_map(|o| SpendableOutputDescriptor::read(&mut &o[..]).ok().flatten())
                    .collect();
                Some((ChannelId(channel_id.value()), outputs))
            })
            .collect())
    }

    pub async fn clear_spendable_outputs(&self, channel_ids: Vec<ChannelId>) -> Result<(), Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(SPENDABLE_OUTPUTS_TABLE)?;
            for channel_id in channel_ids {
                table.remove(channel_id.0)?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }
}

// https://github.com/cberner/redb/blob/master/examples/bincode_keys.rs
#[derive(Debug)]
struct Bincode<T>(pub T);

impl<T> Value for Bincode<T>
where
    T: Debug + Serialize + for<'a> Deserialize<'a>,
{
    type SelfType<'a> = T
    where
        Self: 'a;

    type AsBytes<'a> = Vec<u8>
    where
        Self: 'a;

    fn fixed_width() -> Option<usize> {
        None
    }

    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        bincode::deserialize(data).unwrap()
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'a,
        Self: 'b,
    {
        bincode::serialize(value).unwrap()
    }

    fn type_name() -> TypeName {
        // Backwards compatibility hack
        let name = type_name::<T>().replace("cdk_ldk::db", "cdk_ldk::ln");
        TypeName::new(&format!("Bincode<{}>", name))
    }
}

impl<T> Key for Bincode<T>
where
    T: Debug + Serialize + DeserializeOwned + Ord,
{
    fn compare(data1: &[u8], data2: &[u8]) -> Ordering {
        Self::from_bytes(data1).cmp(&Self::from_bytes(data2))
    }
}
