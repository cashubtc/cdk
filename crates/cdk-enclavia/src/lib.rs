#![doc = include_str!("../README.md")]

mod client;
mod error;
mod transport;

pub use enclavia::Pcrs;

pub use self::client::{connect, EnclaviaClient, EnclaviaClientBuilder};
pub use self::error::{Error, Result};
pub use self::transport::EnclaviaTransport;
