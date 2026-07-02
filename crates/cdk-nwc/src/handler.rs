//! Handler trait implemented by wallet backends.
//!
//! A backend (e.g. a Cashu wallet) implements [`NwcRequestHandler`] to service
//! the NIP-47 commands. The handler is intentionally decoupled from any
//! relay/transport concerns: the [`crate::NwcService`] owns the Nostr relay
//! connection, decryption, authorization and response encoding, and only calls
//! into the handler with already-validated, decrypted requests.
//!
//! Every method returns either a typed NIP-47 response or a [`NIP47Error`],
//! which the service serializes into the encrypted response event. Handlers
//! must never panic: any internal failure should be surfaced as a
//! [`NIP47Error`] with an appropriate [`ErrorCode`](nostr_sdk::nips::nip47::ErrorCode).

use async_trait::async_trait;
use nostr_sdk::nips::nip47::{
    GetBalanceResponse, GetInfoResponse, ListTransactionsRequest, LookupInvoiceRequest,
    LookupInvoiceResponse, MakeInvoiceRequest, MakeInvoiceResponse, NIP47Error, PayInvoiceRequest,
    PayInvoiceResponse,
};

/// Backend that services NIP-47 Nostr Wallet Connect requests.
///
/// Implementations are expected to be cheap to clone/share (e.g. wrap an
/// `Arc`), since the service may dispatch requests concurrently.
#[async_trait]
pub trait NwcRequestHandler: Send + Sync {
    /// Handle a `get_info` request.
    async fn get_info(&self) -> Result<GetInfoResponse, NIP47Error>;

    /// Handle a `get_balance` request. The returned balance is in millisatoshis.
    async fn get_balance(&self) -> Result<GetBalanceResponse, NIP47Error>;

    /// Handle a `make_invoice` request.
    async fn make_invoice(
        &self,
        request: MakeInvoiceRequest,
    ) -> Result<MakeInvoiceResponse, NIP47Error>;

    /// Handle a `pay_invoice` request.
    async fn pay_invoice(
        &self,
        request: PayInvoiceRequest,
    ) -> Result<PayInvoiceResponse, NIP47Error>;

    /// Handle a `lookup_invoice` request.
    async fn lookup_invoice(
        &self,
        request: LookupInvoiceRequest,
    ) -> Result<LookupInvoiceResponse, NIP47Error>;

    /// Handle a `list_transactions` request.
    async fn list_transactions(
        &self,
        request: ListTransactionsRequest,
    ) -> Result<Vec<LookupInvoiceResponse>, NIP47Error>;
}
