#![cfg(test)]
#![allow(missing_docs)]

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use bip39::Mnemonic;
use cdk_common::database::WalletDatabase;
use cdk_common::mint_url::MintUrl;
use cdk_common::nut00::KnownMethod;
use cdk_common::nuts::{
    CheckStateResponse, CurrencyUnit, Id, KeysetResponse, MeltQuoteBolt11Response,
    MeltQuoteCustomRequest, MeltQuoteCustomResponse, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintQuoteCustomRequest, MintQuoteCustomResponse, MintRequest,
    MintResponse, Proof, RestoreResponse, SwapRequest, SwapResponse,
};
use cdk_common::secret::Secret;
use cdk_common::wallet::{MeltQuote, MintQuote};
use cdk_common::{
    Amount, MeltQuoteBolt12Request, MeltQuoteState, MintQuoteBolt12Request,
    MintQuoteBolt12Response, PaymentMethod, SecretKey, State,
};

use crate::nuts::{CheckStateRequest, MeltQuoteBolt11Request, MeltRequest, RestoreRequest};
use crate::wallet::{MintConnector, Wallet};
use crate::Error;

/// Create test database
pub async fn create_test_db() -> Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync>
{
    let db = cdk_sqlite::wallet::memory::empty().await.unwrap();
    Arc::new(db)
}

/// Create a test mint URL
pub fn test_mint_url() -> MintUrl {
    MintUrl::from_str("https://test-mint.example.com").unwrap()
}

/// Create a test keyset ID
pub fn test_keyset_id() -> Id {
    Id::from_str("00916bbf7ef91a36").unwrap()
}

/// Create a test proof
pub fn test_proof(keyset_id: Id, amount: u64) -> Proof {
    Proof {
        amount: Amount::from(amount),
        keyset_id,
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        witness: None,
        dleq: None,
    }
}

/// Create a test proof info in Unspent state
pub fn test_proof_info(keyset_id: Id, amount: u64, mint_url: MintUrl) -> cdk_common::wallet::ProofInfo {
    let proof = test_proof(keyset_id, amount);
    cdk_common::wallet::ProofInfo::new(proof, mint_url, State::Unspent, CurrencyUnit::Sat).unwrap()
}

/// Create a test melt quote
pub fn test_melt_quote() -> MeltQuote {
    MeltQuote {
        id: format!("test_melt_quote_{}", uuid::Uuid::new_v4()),
        unit: CurrencyUnit::Sat,
        amount: Amount::from(1000),
        request: "lnbc1000...".to_string(),
        fee_reserve: Amount::from(10),
        state: MeltQuoteState::Unpaid,
        expiry: 9999999999,
        payment_preimage: None,
        payment_method: PaymentMethod::Known(KnownMethod::Bolt11),
        used_by_operation: None,
        version: 0,
    }
}

/// Create a test mint quote
pub fn test_mint_quote(mint_url: MintUrl) -> MintQuote {
    MintQuote::new(
        format!("test_mint_quote_{}", uuid::Uuid::new_v4()),
        mint_url,
        PaymentMethod::Known(KnownMethod::Bolt11),
        Some(Amount::from(1000)),
        CurrencyUnit::Sat,
        "lnbc1000...".to_string(),
        9999999999,
        None,
    )
}

/// Create a test wallet
pub async fn create_test_wallet(
    db: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync>,
) -> Wallet {
    let mint_url = "https://test-mint.example.com";
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");

    Wallet::new(mint_url, CurrencyUnit::Sat, db, seed, None).unwrap()
}

/// Create a test wallet with a mock client
pub async fn create_test_wallet_with_mock(
    db: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync>,
    mock_client: Arc<MockMintConnector>,
) -> Wallet {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");

    crate::wallet::WalletBuilder::new()
        .mint_url(test_mint_url())
        .unit(CurrencyUnit::Sat)
        .localstore(db)
        .seed(seed)
        .shared_client(mock_client)
        .build()
        .unwrap()
}

/// Mock MintConnector for testing recovery scenarios
#[derive(Debug)]
pub struct MockMintConnector {
    /// Response for post_check_state calls
    pub check_state_response: Mutex<Option<Result<CheckStateResponse, Error>>>,
    /// Response for post_restore calls
    pub restore_response: Mutex<Option<Result<RestoreResponse, Error>>>,
    /// Response for get_melt_quote_status calls
    pub melt_quote_status_response: Mutex<Option<Result<MeltQuoteBolt11Response<String>, Error>>>,
}

impl MockMintConnector {
    pub fn new() -> Self {
        Self {
            check_state_response: Mutex::new(None),
            restore_response: Mutex::new(None),
            melt_quote_status_response: Mutex::new(None),
        }
    }

    pub fn set_check_state_response(&self, response: Result<CheckStateResponse, Error>) {
        *self.check_state_response.lock().unwrap() = Some(response);
    }

    pub fn _set_restore_response(&self, response: Result<RestoreResponse, Error>) {
        *self.restore_response.lock().unwrap() = Some(response);
    }

    pub fn set_melt_quote_status_response(
        &self,
        response: Result<MeltQuoteBolt11Response<String>, Error>,
    ) {
        *self.melt_quote_status_response.lock().unwrap() = Some(response);
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl MintConnector for MockMintConnector {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error> {
        unimplemented!()
    }

    async fn fetch_lnurl_pay_request(
        &self,
        _url: &str,
    ) -> Result<crate::lightning_address::LnurlPayResponse, Error> {
        unimplemented!()
    }

    async fn fetch_lnurl_invoice(
        &self,
        _url: &str,
    ) -> Result<crate::lightning_address::LnurlPayInvoiceResponse, Error> {
        unimplemented!()
    }

    async fn get_mint_keys(&self) -> Result<Vec<crate::nuts::KeySet>, Error> {
        unimplemented!()
    }

    async fn get_mint_keyset(&self, _keyset_id: Id) -> Result<crate::nuts::KeySet, Error> {
        unimplemented!()
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        unimplemented!()
    }

    async fn post_mint_quote(
        &self,
        _request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        unimplemented!()
    }

    async fn get_mint_quote_status(
        &self,
        _quote_id: &str,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        unimplemented!()
    }

    async fn post_mint(
        &self,
        _method: &PaymentMethod,
        _request: MintRequest<String>,
    ) -> Result<MintResponse, Error> {
        unimplemented!()
    }

    async fn post_melt_quote(
        &self,
        _request: MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        unimplemented!()
    }

    async fn get_melt_quote_status(
        &self,
        _quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.melt_quote_status_response
            .lock()
            .unwrap()
            .take()
            .expect("MockMintConnector: get_melt_quote_status called without configured response")
    }

    async fn post_melt(
        &self,
        _method: &PaymentMethod,
        _request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        unimplemented!()
    }

    async fn post_swap(&self, _request: SwapRequest) -> Result<SwapResponse, Error> {
        unimplemented!()
    }

    async fn get_mint_info(&self) -> Result<crate::nuts::MintInfo, Error> {
        unimplemented!()
    }

    async fn post_check_state(
        &self,
        _request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        self.check_state_response
            .lock()
            .unwrap()
            .take()
            .expect("MockMintConnector: post_check_state called without configured response")
    }

    async fn post_restore(&self, _request: RestoreRequest) -> Result<RestoreResponse, Error> {
        self.restore_response
            .lock()
            .unwrap()
            .take()
            .expect("MockMintConnector: post_restore called without configured response")
    }

    #[cfg(feature = "auth")]
    async fn get_auth_wallet(&self) -> Option<crate::wallet::AuthWallet> {
        None
    }

    #[cfg(feature = "auth")]
    async fn set_auth_wallet(&self, _wallet: Option<crate::wallet::AuthWallet>) {}

    async fn get_mint_quote_custom_status(
        &self,
        _method: &str,
        _quote_id: &str,
    ) -> Result<MintQuoteCustomResponse<String>, Error> {
        unimplemented!()
    }

    async fn get_melt_quote_custom_status(
        &self,
        _method: &str,
        _quote_id: &str,
    ) -> Result<MeltQuoteCustomResponse<String>, Error> {
        unimplemented!()
    }

    async fn post_mint_bolt12_quote(
        &self,
        _request: MintQuoteBolt12Request,
    ) -> Result<MintQuoteBolt12Response<String>, Error> {
        unimplemented!()
    }

    async fn get_mint_quote_bolt12_status(
        &self,
        _quote_id: &str,
    ) -> Result<MintQuoteBolt12Response<String>, Error> {
        unimplemented!()
    }

    async fn post_melt_bolt12_quote(
        &self,
        _request: MeltQuoteBolt12Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        unimplemented!()
    }

    async fn get_melt_bolt12_quote_status(
        &self,
        _quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        unimplemented!()
    }

    async fn post_mint_custom_quote(
        &self,
        _method: &PaymentMethod,
        _request: MintQuoteCustomRequest,
    ) -> Result<MintQuoteCustomResponse<String>, Error> {
        unimplemented!()
    }

    async fn post_melt_custom_quote(
        &self,
        _request: MeltQuoteCustomRequest,
    ) -> Result<MeltQuoteCustomResponse<String>, Error> {
        unimplemented!()
    }
}
