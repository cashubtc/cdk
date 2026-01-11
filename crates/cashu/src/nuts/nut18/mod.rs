//! NUT-18: Payment Requests
//!
//! This module provides JSON-based payment request functionality (CREQ-A format).
//! For bech32m encoding (CREQ-B format), see NUT-26.
//!
//! <https://github.com/cashubtc/nuts/blob/main/18.md>

pub mod error;
pub mod payment_request;
pub mod secret;
pub mod transport;

pub use error::Error;
pub use payment_request::{PaymentRequest, PaymentRequestBuilder, PaymentRequestPayload};
pub use secret::Nut10SecretRequest;
pub use transport::{Transport, TransportBuilder, TransportType};
