//! Wallet client

use std::fmt::Debug;

use async_trait::async_trait;
use cdk_common::{
    MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse, MintQuoteRequest,
    MintQuoteResponse,
};

use super::Error;
// Re-export Lightning address types for trait implementers
pub use crate::lightning_address::{LnurlPayInvoiceResponse, LnurlPayResponse};
use crate::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest, CheckStateResponse, Id,
    KeySet, KeysetResponse, MeltRequest, MintInfo, MintRequest, MintResponse, PaymentMethod,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use crate::wallet::AuthWallet;

pub mod http_client;
pub mod transport;

/// Auth HTTP Client with async transport
pub type AuthHttpClient = http_client::AuthHttpClient<transport::Async>;
/// Default Http Client with async transport (non-Tor)
pub type HttpClient = http_client::HttpClient<transport::Async>;
/// Tor Http Client with async transport (only when `tor` feature is enabled and not on wasm32)
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub type TorHttpClient = http_client::HttpClient<transport::TorAsync>;

/// Interface that connects a wallet to a mint. Typically represents an [HttpClient].
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MintConnector: Debug {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    /// Resolve the DNS record getting the TXT value
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error>;

    /// Fetch Lightning address pay request data
    async fn fetch_lnurl_pay_request(
        &self,
        url: &str,
    ) -> Result<crate::lightning_address::LnurlPayResponse, Error>;

    /// Fetch invoice from Lightning address callback
    async fn fetch_lnurl_invoice(
        &self,
        url: &str,
    ) -> Result<crate::lightning_address::LnurlPayInvoiceResponse, Error>;

    /// Get Active Mint Keys [NUT-01]
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error>;
    /// Get Keyset Keys [NUT-01]
    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error>;
    /// Get Keysets [NUT-02]
    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error>;
    /// Mint Quote [NUT-04, NUT-23, NUT-25]
    async fn post_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, Error>;
    /// Mint Tokens [NUT-04]
    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<String>,
    ) -> Result<MintResponse, Error>;

    /// Batch check mint quote status [NUT-29]
    ///
    /// Checks the status of multiple mint quotes in a single request.
    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteResponse<String>>, Error>;

    /// Batch mint tokens [NUT-29]
    ///
    /// Mints tokens for multiple quotes in a single atomic request.
    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<String>,
    ) -> Result<MintResponse, Error>;

    /// Melt Quote [NUT-05]
    async fn post_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error>;

    /// Mint Quote status with payment method
    async fn get_mint_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, Error>;

    /// Melt [NUT-05]
    /// Melt Quote Status
    async fn get_melt_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error>;

    /// [Nut-08] Lightning fee return if outputs defined
    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, Error>;

    /// Split Token [NUT-06]
    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, Error>;
    /// Get Mint Info [NUT-06]
    async fn get_mint_info(&self) -> Result<MintInfo, Error>;
    /// Spendable check [NUT-07]
    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error>;
    /// Restore request [NUT-13]
    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error>;

    /// Get the auth wallet for the client
    async fn get_auth_wallet(&self) -> Option<AuthWallet>;

    /// Set auth wallet on client
    async fn set_auth_wallet(&self, wallet: Option<AuthWallet>);

    /// Get the mint's transparency-log signing key and origin (NUT-XX,
    /// `GET /v1/audit/pubkey`). Default implementation reports the
    /// endpoint as unsupported, so custom connectors keep compiling.
    #[cfg(feature = "transparency-log")]
    async fn get_audit_pubkey(&self) -> Result<AuditPubkeyResponse, Error> {
        Err(Error::Custom(
            "transparency log endpoints not supported by this connector".to_string(),
        ))
    }

    /// Get the mint's latest signed checkpoint (NUT-XX,
    /// `GET /v1/audit/checkpoint`).
    #[cfg(feature = "transparency-log")]
    async fn get_audit_checkpoint(&self) -> Result<AuditCheckpointResponse, Error> {
        Err(Error::Custom(
            "transparency log endpoints not supported by this connector".to_string(),
        ))
    }

    /// Get an RFC 6962 consistency proof between two checkpoint sizes
    /// (NUT-XX, `GET /v1/audit/proof/consistency`).
    #[cfg(feature = "transparency-log")]
    async fn get_audit_consistency_proof(
        &self,
        _first: u64,
        _second: u64,
    ) -> Result<AuditConsistencyResponse, Error> {
        Err(Error::Custom(
            "transparency log endpoints not supported by this connector".to_string(),
        ))
    }
}

/// Response of `GET /v1/audit/pubkey` (NUT-XX).
#[cfg(feature = "transparency-log")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditPubkeyResponse {
    /// The checkpoint origin line this mint signs (its log's name).
    pub origin: String,
    /// Base64-encoded 32-byte Ed25519 log-signing public key.
    pub pubkey: String,
    /// Signature scheme identifier; `"ed25519"` for this NUT revision.
    pub signature_scheme: String,
}

/// Response of `GET /v1/audit/checkpoint` (NUT-XX).
#[cfg(feature = "transparency-log")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditCheckpointResponse {
    /// Full C2SP signed note (checkpoint plus signature lines).
    pub checkpoint: String,
    /// Ascii Sigsum proof-of-logging for this checkpoint, if anchored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigsum_proof: Option<String>,
}

/// Response of `GET /v1/audit/proof/consistency` (NUT-XX).
#[cfg(feature = "transparency-log")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditConsistencyResponse {
    /// The smaller tree size the proof starts from.
    pub first: u64,
    /// The larger tree size the proof extends to.
    pub second: u64,
    /// Hex-encoded RFC 6962 consistency proof nodes.
    pub proof: Vec<String>,
}
