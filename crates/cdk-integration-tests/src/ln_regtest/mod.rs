pub mod bitcoin_client;
pub mod bitcoind;
pub mod cln;
pub mod hex;
pub mod ln_client;
pub mod lnd;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum InvoiceStatus {
    Paid,
    Pending,
    Unpaid,
    Expired,
    Failed,
}
