use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk::nuts::nutxx1::MintAuthRequest;
use cdk::nuts::{
    AuthToken, CheckStateRequest, CheckStateResponse, Id, KeySet, KeysetResponse,
    MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintBolt11Request,
    MintBolt11Response, MintInfo, MintQuoteBolt11Request, MintQuoteBolt11Response, RestoreRequest,
    RestoreResponse, SwapRequest, SwapResponse,
};
use cdk::util::unix_time;
use cdk::wallet::MintConnector;
use cdk::{Error, Mint};
use uuid::Uuid;

pub struct DirectMintConnection {
    pub mint: Arc<Mint>,
}

impl Debug for DirectMintConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DirectMintConnection {{ mint_info: {:?} }}",
            self.mint.config.mint_info()
        )
    }
}

/// Implements the generic [MintConnector] (i.e. use the interface that expects to communicate
/// to a generic mint, where we don't know that quote ID's are [Uuid]s) for [DirectMintConnection],
/// where we know we're dealing with a mint that uses [Uuid]s for quotes.
/// Convert the requests and responses between the [String] and [Uuid] variants as necessary.
#[async_trait]
impl MintConnector for DirectMintConnection {
    async fn get_mint_keys(&self, _auth_token: Option<AuthToken>) -> Result<Vec<KeySet>, Error> {
        self.mint.pubkeys().await.map(|pks| pks.keysets)
    }

    async fn get_mint_keyset(
        &self,
        keyset_id: Id,
        _auth_token: Option<AuthToken>,
    ) -> Result<KeySet, Error> {
        self.mint
            .keyset(&keyset_id)
            .await
            .and_then(|res| res.ok_or(Error::UnknownKeySet))
    }

    async fn get_mint_keysets(
        &self,
        _auth_token: Option<AuthToken>,
    ) -> Result<KeysetResponse, Error> {
        self.mint.keysets().await
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteBolt11Request,
        _auth_token: Option<AuthToken>,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        self.mint
            .get_mint_bolt11_quote(None, request)
            .await
            .map(Into::into)
    }

    async fn get_mint_quote_status(
        &self,
        quote_id: &str,
        _auth_token: Option<AuthToken>,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        let quote_id_uuid = Uuid::from_str(quote_id).unwrap();
        self.mint
            .check_mint_quote(None, &quote_id_uuid)
            .await
            .map(Into::into)
    }

    async fn post_mint(
        &self,
        request: MintBolt11Request<String>,
        _auth_token: Option<AuthToken>,
    ) -> Result<MintBolt11Response, Error> {
        let request_uuid = request.try_into().unwrap();
        self.mint.process_mint_request(None, request_uuid).await
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteBolt11Request,
        _auth_token: Option<AuthToken>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .get_melt_bolt11_quote(None, &request)
            .await
            .map(Into::into)
    }

    async fn get_melt_quote_status(
        &self,
        quote_id: &str,
        _auth_token: Option<AuthToken>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let quote_id_uuid = Uuid::from_str(quote_id).unwrap();
        self.mint
            .check_melt_quote(None, &quote_id_uuid)
            .await
            .map(Into::into)
    }

    async fn post_melt(
        &self,
        request: MeltBolt11Request<String>,
        _auth_token: Option<AuthToken>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let request_uuid = request.try_into().unwrap();
        self.mint
            .melt_bolt11(None, &request_uuid)
            .await
            .map(Into::into)
    }

    async fn post_swap(
        &self,
        swap_request: SwapRequest,
        _auth_token: Option<AuthToken>,
    ) -> Result<SwapResponse, Error> {
        self.mint.process_swap_request(None, swap_request).await
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        Ok(self.mint.mint_info().clone().time(unix_time()))
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
        _auth_token: Option<AuthToken>,
    ) -> Result<CheckStateResponse, Error> {
        self.mint.check_state(&request).await
    }

    async fn post_restore(
        &self,
        request: RestoreRequest,
        _auth_token: Option<AuthToken>,
    ) -> Result<RestoreResponse, Error> {
        self.mint.restore(request).await
    }

    /// Get Blind Auth keys
    async fn get_mint_blind_auth_keys(&self) -> Result<Vec<KeySet>, Error> {
        todo!();
    }
    /// Get Blind Auth Keyset
    async fn get_mint_blind_auth_keyset(&self, _keyset_id: Id) -> Result<KeySet, Error> {
        todo!();
    }
    /// Get Blind Auth keysets
    async fn get_mint_blind_auth_keysets(&self) -> Result<KeysetResponse, Error> {
        todo!();
    }
    /// Post mint blind auth
    async fn post_mint_blind_auth(
        &self,
        _request: MintAuthRequest,
    ) -> Result<MintBolt11Response, Error> {
        todo!();
    }
}
