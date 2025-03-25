#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]
// Allow missing documentation in generated code
#![allow(rustdoc::missing_doc_code_examples)]
// Allow missing docs in the generated protobuf code
#![allow(missing_docs)]

pub mod proto;

pub mod mint_rpc_cli;

pub use proto::*;
