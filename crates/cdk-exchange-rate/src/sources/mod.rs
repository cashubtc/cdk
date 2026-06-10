//! Optional exchange-rate source implementations.

#[cfg(feature = "sources-http")]
pub mod bitstamp;
#[cfg(feature = "sources-http")]
pub mod coinbase;
#[cfg(feature = "sources-http")]
pub mod kraken;

#[cfg(feature = "sources-http")]
pub use bitstamp::BitstampRateSource;
#[cfg(feature = "sources-http")]
pub use coinbase::CoinbaseRateSource;
#[cfg(feature = "sources-http")]
pub use kraken::KrakenRateSource;
