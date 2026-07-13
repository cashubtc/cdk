use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::federation::FederationJournalAppendOutcome;
use cdk_http_client::{HttpError, RawResponse};

use super::*;
use crate::nuts::AuthToken;
use crate::wallet::mint_connector::transport::Transport;

#[derive(Clone)]
pub(super) struct LocalFederatedMemberConnector {
    mint: Arc<Mint>,
    federation_mints: Option<Arc<Vec<Arc<Mint>>>>,
    federation_journal: Option<Arc<tokio::sync::Mutex<InMemoryFederationJournal>>>,
}

impl fmt::Debug for LocalFederatedMemberConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let member_id = self
            .mint
            .federation_config()
            .map(|config| config.member_id());
        f.debug_struct("LocalFederatedMemberConnector")
            .field("member_id", &member_id)
            .finish_non_exhaustive()
    }
}

impl LocalFederatedMemberConnector {
    pub(super) fn new_with_federation_mints(
        mint: Arc<Mint>,
        federation_mints: Arc<Vec<Arc<Mint>>>,
        federation_journal: Arc<tokio::sync::Mutex<InMemoryFederationJournal>>,
    ) -> Self {
        Self {
            mint,
            federation_mints: Some(federation_mints),
            federation_journal: Some(federation_journal),
        }
    }

    async fn append_and_apply_to_federation(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<Option<FederationJournalEntry>, crate::Error> {
        if let (Some(federation_mints), Some(federation_journal)) =
            (&self.federation_mints, &self.federation_journal)
        {
            let mut journal = federation_journal.lock().await;
            let entry = match journal.append(envelope)? {
                FederationJournalAppendOutcome::Appended(entry) => {
                    for mint in federation_mints.iter() {
                        try_persist_and_apply_entry(mint, &entry).await?;
                    }
                    entry
                }
                FederationJournalAppendOutcome::Duplicate(entry) => entry,
            };

            return Ok(Some(entry));
        }

        Ok(None)
    }

    async fn effective_federation_config(&self) -> Result<FederationRuntimeConfig, crate::Error> {
        self.mint
            .effective_federation_config()
            .await?
            .ok_or_else(|| {
                crate::Error::InvalidMintResponse(
                    "local test mint is not configured for federation".to_string(),
                )
            })
    }

    fn quote_id(quote: &str) -> Result<QuoteId, crate::Error> {
        QuoteId::from_str(quote).map_err(crate::Error::from)
    }

    fn paid_amount(response: &MintQuoteResponse<QuoteId>) -> Option<Amount> {
        match response {
            MintQuoteResponse::Bolt11(response) => response.amount,
            MintQuoteResponse::Bolt12(response) => Some(response.amount_paid),
            MintQuoteResponse::Onchain(response) => Some(response.amount_paid),
            MintQuoteResponse::Custom { response, .. } => Some(response.amount_paid),
        }
    }

    async fn mint_quote_payment_proof(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<String>, crate::Error> {
        Ok(self
            .mint
            .localstore()
            .get_mint_quote(quote_id)
            .await?
            .map(|quote| quote.request_lookup_id.to_string()))
    }

    async fn append_paid_mint_quote_observation(
        &self,
        quote_id: &QuoteId,
        method: PaymentMethod,
        amount_paid: Amount,
        payment_proof: Option<String>,
    ) -> Result<(), crate::Error> {
        let config = self.effective_federation_config().await?;
        let envelope = config.mint_quote_payment_observation_with_metadata(
            quote_id.clone(),
            method,
            MintQuoteState::Paid,
            amount_paid,
            Amount::ZERO,
            payment_proof,
            None,
        );
        self.append_and_apply_to_federation(envelope).await?;

        Ok(())
    }

    async fn sync_paid_mint_quote_observation(
        &self,
        quote_id: &QuoteId,
        response: &MintQuoteResponse<QuoteId>,
    ) -> Result<(), crate::Error> {
        if response.state() != Some(MintQuoteState::Paid) {
            return Ok(());
        }

        let amount_paid = Self::paid_amount(response).ok_or_else(|| {
            crate::Error::InvalidMintResponse(
                "paid federation mint quote status did not include an amount".to_string(),
            )
        })?;
        let payment_proof = self.mint_quote_payment_proof(quote_id).await?;
        self.append_paid_mint_quote_observation(
            quote_id,
            response.method(),
            amount_paid,
            payment_proof,
        )
        .await?;

        Ok(())
    }

    async fn validate_mint_quote_method(
        &self,
        quote: &QuoteId,
        method: &PaymentMethod,
    ) -> Result<(), crate::Error> {
        if self.mint.get_mint_quote_method(quote).await? != *method {
            return Err(crate::Error::InvalidPaymentMethod);
        }

        Ok(())
    }

    async fn validate_melt_quote_method(
        &self,
        quote: &QuoteId,
        method: &PaymentMethod,
    ) -> Result<(), crate::Error> {
        if self.mint.get_melt_quote_method(quote).await? != *method {
            return Err(crate::Error::InvalidPaymentMethod);
        }

        Ok(())
    }

    fn unsupported(operation: &str) -> crate::Error {
        crate::Error::Custom(format!(
            "local federated member connector does not implement {operation}"
        ))
    }
}

async fn try_persist_and_apply_entry(
    mint: &Mint,
    entry: &FederationJournalEntry,
) -> Result<(), crate::Error> {
    let localstore = mint.localstore();
    let mut tx = localstore.begin_transaction().await?;
    tx.add_federation_journal_entry(entry).await?;
    tx.commit().await?;
    mint.apply_federation_journal_entries(std::slice::from_ref(entry))
        .await?;

    Ok(())
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MintConnector for LocalFederatedMemberConnector {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, crate::Error> {
        Err(Self::unsupported("resolve_dns_txt"))
    }

    async fn fetch_lnurl_pay_request(
        &self,
        _url: &str,
    ) -> Result<crate::lightning_address::LnurlPayResponse, crate::Error> {
        Err(Self::unsupported("fetch_lnurl_pay_request"))
    }

    async fn fetch_lnurl_invoice(
        &self,
        _url: &str,
    ) -> Result<crate::lightning_address::LnurlPayInvoiceResponse, crate::Error> {
        Err(Self::unsupported("fetch_lnurl_invoice"))
    }

    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, crate::Error> {
        if let Some(pubkeys) = self.mint.effective_federation_pubkeys().await? {
            return Ok(pubkeys.keysets);
        }

        Ok(self.mint.pubkeys().keysets)
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, crate::Error> {
        let pubkeys = match self
            .mint
            .effective_federation_keyset_pubkeys(&keyset_id)
            .await?
        {
            Some(pubkeys) => pubkeys,
            None => self.mint.keyset_pubkeys(&keyset_id)?,
        };

        pubkeys
            .keysets
            .into_iter()
            .next()
            .ok_or(crate::Error::UnknownKeySet)
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, crate::Error> {
        if let Some(keysets) = self.mint.effective_federation_keysets().await? {
            return Ok(keysets);
        }

        Ok(self.mint.keysets())
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, crate::Error> {
        let response: MintQuoteResponse<String> =
            self.mint.get_mint_quote(request.clone()).await?.into();
        let method = request.method();
        let amount = request.amount();
        let config = self.effective_federation_config().await?;
        let quote = Self::quote_id(response.quote())?;
        let envelope = config.mint_quote_operation(quote.clone(), request, response.clone());
        self.append_and_apply_to_federation(envelope).await?;
        if self.federation_mints.is_some() {
            if let Some(amount_paid) = amount.filter(|amount| *amount > Amount::ZERO) {
                let payment_proof = self.mint_quote_payment_proof(&quote).await?;
                self.append_paid_mint_quote_observation(&quote, method, amount_paid, payment_proof)
                    .await?;
            }
        }

        Ok(response)
    }

    async fn post_mint_quote_with_id(
        &self,
        request: MintQuoteRequest,
        quote_id: &QuoteId,
    ) -> Result<MintQuoteResponse<String>, crate::Error> {
        if let Some((response, _)) = self
            .mint
            .ordered_federation_mint_quote_response(quote_id, &request)
            .await?
        {
            return Ok(response.into());
        }

        let (response, request_lookup_id) = self
            .mint
            .prepare_federation_mint_quote(quote_id.clone(), request.clone())
            .await?;
        let response: MintQuoteResponse<String> = response.into();
        let method = request.method();
        let amount = request.amount();
        let config = self.effective_federation_config().await?;
        let envelope = config.mint_quote_operation_with_lookup_id(
            quote_id.clone(),
            request_lookup_id,
            request,
            response.clone(),
        );
        self.append_and_apply_to_federation(envelope).await?;
        if self.federation_mints.is_some() {
            if let Some(amount_paid) = amount.filter(|amount| *amount > Amount::ZERO) {
                let payment_proof = self.mint_quote_payment_proof(quote_id).await?;
                self.append_paid_mint_quote_observation(
                    quote_id,
                    method,
                    amount_paid,
                    payment_proof,
                )
                .await?;
            }
        }

        Ok(response)
    }

    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<String>,
    ) -> Result<MintResponse, crate::Error> {
        let quote = Self::quote_id(&request.quote)?;
        self.validate_mint_quote_method(&quote, method).await?;
        let input = MintInput::Single(MintRequest {
            quote,
            outputs: request.outputs,
            signature: request.signature,
        });
        let config = self.effective_federation_config().await?;
        let envelope = config.mint_operation(input.clone())?;
        let operation_id = envelope.operation_id()?;
        if self
            .append_and_apply_to_federation(envelope)
            .await?
            .is_some()
        {
            return self
                .mint
                .persisted_mint_share_response(operation_id, &input)
                .await;
        }

        config.mint_share_response(operation_id, &input)
    }

    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteResponse<String>>, crate::Error> {
        let quote_ids = request
            .quotes
            .iter()
            .map(|quote| Self::quote_id(quote))
            .collect::<Result<Vec<_>, _>>()?;
        for quote in &quote_ids {
            self.validate_mint_quote_method(quote, method).await?;
        }

        Ok(self
            .mint
            .check_mint_quotes(&quote_ids)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<String>,
    ) -> Result<MintResponse, crate::Error> {
        let quotes = request
            .quotes
            .iter()
            .map(|quote| Self::quote_id(quote))
            .collect::<Result<Vec<_>, _>>()?;
        for quote in &quotes {
            self.validate_mint_quote_method(quote, method).await?;
        }

        let input = MintInput::Batch(BatchMintRequest {
            quotes,
            quote_amounts: request.quote_amounts,
            outputs: request.outputs,
            signatures: request.signatures,
        });
        let config = self.effective_federation_config().await?;
        let envelope = config.mint_operation(input.clone())?;
        let operation_id = envelope.operation_id()?;
        if self
            .append_and_apply_to_federation(envelope)
            .await?
            .is_some()
        {
            return self
                .mint
                .persisted_mint_share_response(operation_id, &input)
                .await;
        }

        config.mint_share_response(operation_id, &input)
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, crate::Error> {
        Ok(self.mint.get_melt_quote(request).await?.into())
    }

    async fn post_melt_quote_with_id(
        &self,
        request: MeltQuoteRequest,
        quote_id: &QuoteId,
    ) -> Result<MeltQuoteCreateResponse<String>, crate::Error> {
        if let Some(response) = self
            .mint
            .existing_federation_melt_quote(quote_id, &request)
            .await?
        {
            return Ok(response.into());
        }

        let (response, metadata) = self
            .mint
            .prepare_federation_melt_quote(quote_id.clone(), request.clone())
            .await?;
        let config = self.effective_federation_config().await?;
        let envelope =
            config.melt_quote_operation(quote_id.clone(), request, response.clone(), metadata);
        self.append_and_apply_to_federation(envelope).await?;
        Ok(response.into())
    }

    async fn get_mint_quote_status(
        &self,
        _method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, crate::Error> {
        let quote_id = Self::quote_id(quote_id)?;
        let was_paid = self
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await?
            .is_some_and(|quote| quote.state() == MintQuoteState::Paid);
        let response = self
            .mint
            .check_mint_quotes(std::slice::from_ref(&quote_id))
            .await?
            .into_iter()
            .next()
            .ok_or(crate::Error::UnknownQuote)?;

        if !was_paid {
            self.sync_paid_mint_quote_observation(&quote_id, &response)
                .await?;
        }

        Ok(response.into())
    }

    async fn get_melt_quote_status(
        &self,
        _method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, crate::Error> {
        let quote_id = Self::quote_id(quote_id)?;
        Ok(self.mint.check_melt_quote(&quote_id).await?.into())
    }

    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, crate::Error> {
        let request: MeltRequest<QuoteId> = request.try_into()?;
        self.validate_melt_quote_method(request.quote_id(), method)
            .await?;
        let config = self.effective_federation_config().await?;
        let envelope = config.melt_operation(request.clone())?;
        self.append_and_apply_to_federation(envelope).await?;
        if let Some(observation) = self
            .mint
            .probe_federation_melt_quote_payment(request.quote_id())
            .await?
        {
            let envelope = FederationOperationEnvelope::new(
                config.federation_id(),
                FederationOperation::MeltQuotePayment(observation),
            );
            self.append_and_apply_to_federation(envelope).await?;
        }

        Ok(self.mint.check_melt_quote(request.quote_id()).await?.into())
    }

    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, crate::Error> {
        let config = self.effective_federation_config().await?;
        let envelope = config.swap_operation(request.clone())?;
        let operation_id = envelope.operation_id()?;
        if self
            .append_and_apply_to_federation(envelope)
            .await?
            .is_some()
        {
            return self
                .mint
                .persisted_swap_share_response(operation_id, &request)
                .await;
        }

        config.swap_share_response(operation_id, &request)
    }

    async fn get_mint_info(&self) -> Result<MintInfo, crate::Error> {
        self.mint.mint_info().await
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, crate::Error> {
        self.mint.check_state(&request).await
    }

    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, crate::Error> {
        self.mint.restore(request).await
    }

    async fn get_auth_wallet(&self) -> Option<crate::wallet::AuthWallet> {
        None
    }

    async fn set_auth_wallet(&self, _wallet: Option<crate::wallet::AuthWallet>) {}
}

#[derive(Clone, Default)]
struct LocalMemberHttpTransport {
    connector: Option<Arc<dyn MintConnector + Send + Sync>>,
}

impl fmt::Debug for LocalMemberHttpTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalMemberHttpTransport")
            .field("has_connector", &self.connector.is_some())
            .finish()
    }
}

impl LocalMemberHttpTransport {
    fn new(connector: Arc<dyn MintConnector + Send + Sync>) -> Self {
        Self {
            connector: Some(connector),
        }
    }

    fn connector(&self) -> Result<Arc<dyn MintConnector + Send + Sync>, HttpError> {
        self.connector
            .clone()
            .ok_or_else(|| HttpError::Other("missing local member connector".to_string()))
    }

    fn decode_payload<T, P>(payload: &P) -> Result<T, HttpError>
    where
        T: serde::de::DeserializeOwned,
        P: serde::Serialize + Send + Sync,
    {
        serde_json::from_value(
            serde_json::to_value(payload)
                .map_err(|err| HttpError::Serialization(err.to_string()))?,
        )
        .map_err(|err| HttpError::Serialization(err.to_string()))
    }

    fn encode_response<T, R>(response: T) -> Result<R, HttpError>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        serde_json::from_value(
            serde_json::to_value(response)
                .map_err(|err| HttpError::Serialization(err.to_string()))?,
        )
        .map_err(|err| HttpError::Serialization(err.to_string()))
    }

    fn map_error(err: crate::Error) -> HttpError {
        HttpError::Other(err.to_string())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl Transport for LocalMemberHttpTransport {
    fn with_proxy(
        &mut self,
        _proxy: url::Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Ok(Vec::new())
    }

    async fn http_get<R>(&self, url: url::Url, _auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: serde::de::DeserializeOwned,
    {
        let connector = self.connector()?;
        let path = url.path();

        if path == "/v1/keys" {
            let keysets = connector.get_mint_keys().await.map_err(Self::map_error)?;
            return Self::encode_response(cdk_common::KeysResponse { keysets });
        }

        if let Some(keyset_id) = path.strip_prefix("/v1/keys/") {
            let keyset_id =
                Id::from_str(keyset_id).map_err(|err| HttpError::Serialization(err.to_string()))?;
            let keyset = connector
                .get_mint_keyset(keyset_id)
                .await
                .map_err(Self::map_error)?;
            return Self::encode_response(cdk_common::KeysResponse {
                keysets: vec![keyset],
            });
        }

        if path == "/v1/keysets" {
            return Self::encode_response(
                connector
                    .get_mint_keysets()
                    .await
                    .map_err(Self::map_error)?,
            );
        }

        if path == "/v1/info" {
            return Self::encode_response(
                connector.get_mint_info().await.map_err(Self::map_error)?,
            );
        }

        if let Some(quote_id) = path.strip_prefix("/v1/mint/quote/bolt11/") {
            let response = connector
                .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), quote_id)
                .await
                .map_err(Self::map_error)?;
            return match response {
                MintQuoteResponse::Bolt11(response) => Self::encode_response(response),
                _ => Err(HttpError::Other(
                    "local member returned non-bolt11 mint quote response".to_string(),
                )),
            };
        }

        if let Some(quote_id) = path.strip_prefix("/v1/melt/quote/bolt11/") {
            let response = connector
                .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), quote_id)
                .await
                .map_err(Self::map_error)?;
            return match response {
                MeltQuoteResponse::Bolt11(response) => Self::encode_response(response),
                _ => Err(HttpError::Other(
                    "local member returned non-bolt11 melt quote response".to_string(),
                )),
            };
        }

        Err(HttpError::Other(format!(
            "unsupported local member GET path {path}"
        )))
    }

    async fn http_get_raw(
        &self,
        _url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other(
            "raw GET is unsupported by the local member transport".to_string(),
        ))
    }

    async fn http_post<P, R>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: serde::Serialize + Send + Sync,
        R: serde::de::DeserializeOwned,
    {
        let connector = self.connector()?;
        match url.path() {
            "/v1/mint/bolt11" => {
                let request = Self::decode_payload::<MintRequest<String>, _>(payload)?;
                Self::encode_response(
                    connector
                        .post_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request)
                        .await
                        .map_err(Self::map_error)?,
                )
            }
            "/v1/mint/bolt11/batch" => {
                let request = Self::decode_payload::<BatchMintRequest<String>, _>(payload)?;
                Self::encode_response(
                    connector
                        .post_batch_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request)
                        .await
                        .map_err(Self::map_error)?,
                )
            }
            "/v1/swap" => {
                let request = Self::decode_payload::<SwapRequest, _>(payload)?;
                Self::encode_response(
                    connector
                        .post_swap(request)
                        .await
                        .map_err(Self::map_error)?,
                )
            }
            "/v1/melt/bolt11" => {
                let request = Self::decode_payload::<MeltRequest<String>, _>(payload)?;
                let response = connector
                    .post_melt(&PaymentMethod::Known(KnownMethod::Bolt11), request)
                    .await
                    .map_err(Self::map_error)?;
                match response {
                    MeltQuoteResponse::Bolt11(response) => Self::encode_response(response),
                    _ => Err(HttpError::Other(
                        "local member returned non-bolt11 melt response".to_string(),
                    )),
                }
            }
            "/v1/restore" => {
                let request = Self::decode_payload::<RestoreRequest, _>(payload)?;
                Self::encode_response(
                    connector
                        .post_restore(request)
                        .await
                        .map_err(Self::map_error)?,
                )
            }
            "/v1/checkstate" => {
                let request = Self::decode_payload::<CheckStateRequest, _>(payload)?;
                Self::encode_response(
                    connector
                        .post_check_state(request)
                        .await
                        .map_err(Self::map_error)?,
                )
            }
            path => Err(HttpError::Other(format!(
                "unsupported local member POST path {path}"
            ))),
        }
    }

    async fn http_post_form_raw<P>(
        &self,
        _url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: serde::Serialize + Send + Sync,
    {
        Err(HttpError::Other(
            "raw form POST is unsupported by the local member transport".to_string(),
        ))
    }
}

pub(super) fn http_member_connectors(
    connectors: BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
    connectors
        .into_iter()
        .map(|(member_id, connector)| {
            let mint_url =
                MintUrl::from_str(&format!("https://member{}.example.com", member_id.get()))
                    .expect("member mint url");
            let client = crate::wallet::mint_connector::http_client::HttpClient::with_transport(
                mint_url,
                LocalMemberHttpTransport::new(connector),
                None,
            );

            (
                member_id,
                Arc::new(client) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect()
}
