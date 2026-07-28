#![doc = include_str!("../README.md")]

mod client;
mod error;
mod transport;

const DEFAULT_OPERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub use enclavia::Pcrs;

pub use self::client::{connect, EnclaviaClient, EnclaviaClientBuilder};
pub use self::error::{Error, Result};
pub use self::transport::EnclaviaTransport;
