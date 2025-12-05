#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

pub mod proto;

pub mod mint_rpc_cli;

pub use proto::*;
