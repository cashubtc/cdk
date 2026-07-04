//! Wallet-side federation validation and aggregation helpers.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::federation::{FederatedKeyset, FederationConfig, FederationError, MemberId};
use cdk_common::{
    MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse, MintQuoteRequest,
    MintQuoteResponse,
};

use super::{AuthWallet, MintConnector};
use crate::nuts::nut01::{
    aggregate_blind_signature_shares, verify_blind_signature_share, BlsBlindSignatureShare,
};
use crate::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, BlindSignature, BlindedMessage,
    CheckStateRequest, CheckStateResponse, Id, KeySet, KeySetInfo, KeysetResponse, MeltRequest,
    MintInfo, MintRequest, MintResponse, PaymentMethod, RestoreRequest, RestoreResponse,
    SwapRequest, SwapResponse,
};
use crate::{Amount, Error};

/// Minimal wallet federation connector validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FederationWalletConnector;

impl FederationWalletConnector {
    /// Validate the aggregate keysets advertised by one federation member.
    pub fn validate_member_keysets(
        public_config: &FederationConfig,
        member_id: MemberId,
        advertised_keysets: &[KeySet],
    ) -> Result<(), Error> {
        Ok(public_config.validate_member_advertised_keysets(member_id, advertised_keysets)?)
    }
}

/// Wallet mint connector that fans out signing requests to federation members.
///
/// The connector preserves the existing wallet-to-mint protocol surface: member
/// mints return ordinary Cashu responses whose `C_` values are threshold BLS
/// signature shares, and the connector verifies and aggregates those shares
/// before returning a normal aggregate Cashu response to the wallet.
#[derive(Clone)]
pub struct FederatedMintConnector {
    public_config: FederationConfig,
    member_connectors: BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    primary_member_id: MemberId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShareAmountValidation {
    Exact,
    AllowZeroAmountPlaceholder,
}

impl fmt::Debug for FederatedMintConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FederatedMintConnector")
            .field("federation_id", &self.public_config.federation_id)
            .field("member_ids", &self.member_connectors.keys())
            .field("primary_member_id", &self.primary_member_id)
            .finish_non_exhaustive()
    }
}

impl FederatedMintConnector {
    /// Create a wallet connector for a static federation roster.
    pub fn new(
        public_config: FederationConfig,
        member_connectors: BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    ) -> Result<Self, Error> {
        public_config.validate()?;

        for member_id in public_config.members.keys() {
            if !member_connectors.contains_key(member_id) {
                return Err(FederationError::MissingMember {
                    member_id: *member_id,
                }
                .into());
            }
        }

        for member_id in member_connectors.keys() {
            if !public_config.members.contains_key(member_id) {
                return Err(FederationError::UnknownMember {
                    member_id: *member_id,
                }
                .into());
            }
        }

        let primary_member_id = public_config
            .members
            .keys()
            .next()
            .copied()
            .ok_or_else(|| Error::InvalidMintResponse("empty federation roster".to_string()))?;

        Ok(Self {
            public_config,
            member_connectors,
            primary_member_id,
        })
    }

    /// Return the public federation config used by this connector.
    #[must_use]
    pub fn public_config(&self) -> &FederationConfig {
        &self.public_config
    }

    fn primary_connector(&self) -> Result<Arc<dyn MintConnector + Send + Sync>, Error> {
        self.member_connectors
            .get(&self.primary_member_id)
            .cloned()
            .ok_or_else(|| {
                Error::InvalidMintResponse("missing primary member connector".to_string())
            })
    }

    fn active_aggregate_keysets(&self) -> Vec<KeySet> {
        self.public_config
            .keysets
            .iter()
            .filter(|keyset| keyset.active)
            .map(federated_keyset_to_keyset)
            .collect()
    }

    fn aggregate_keyset_infos(&self) -> Vec<KeySetInfo> {
        self.public_config
            .keysets
            .iter()
            .map(federated_keyset_to_keyset_info)
            .collect()
    }

    fn aggregate_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        self.public_config
            .keysets
            .iter()
            .find(|keyset| keyset.id == keyset_id)
            .map(federated_keyset_to_keyset)
            .ok_or(Error::UnknownKeySet)
    }

    async fn validate_member_active_keysets(&self) -> Result<(), Error> {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut valid_members = 0usize;
        let mut failures = Vec::new();

        for (member_id, connector) in &self.member_connectors {
            match connector.get_mint_keys().await {
                Ok(advertised) => {
                    validate_member_active_keysets(&self.public_config, *member_id, &advertised)?;
                    valid_members = valid_members.saturating_add(1);
                }
                Err(err) => failures.push(format!("member {}: {}", member_id, err)),
            }
        }

        if valid_members < threshold {
            return Err(Error::InvalidMintResponse(format!(
                "insufficient federation keyset advertisements: required {}, received {}, failures: {}",
                threshold,
                valid_members,
                failures.join("; ")
            )));
        }

        Ok(())
    }

    async fn collect_mint_responses<F, Fut>(
        &self,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, MintResponse)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: std::future::Future<Output = Result<MintResponse, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut responses = Vec::new();
        let mut failures = Vec::new();

        for (member_id, connector) in &self.member_connectors {
            match call_member(connector.clone()).await {
                Ok(response) => {
                    responses.push((*member_id, response));
                    if responses.len() >= threshold {
                        break;
                    }
                }
                Err(err) => failures.push(format!("member {}: {}", member_id, err)),
            }
        }

        if responses.len() >= threshold {
            return Ok(responses);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation signature shares: required {}, received {}, failures: {}",
            threshold,
            responses.len(),
            failures.join("; ")
        )))
    }

    async fn collect_melt_responses<F, Fut>(
        &self,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, MeltQuoteResponse<String>)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: std::future::Future<Output = Result<MeltQuoteResponse<String>, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut responses = Vec::new();
        let mut failures = Vec::new();

        for (member_id, connector) in &self.member_connectors {
            match call_member(connector.clone()).await {
                Ok(response) => {
                    responses.push((*member_id, response));
                    if responses.len() >= threshold {
                        break;
                    }
                }
                Err(err) => failures.push(format!("member {}: {}", member_id, err)),
            }
        }

        if responses.len() >= threshold {
            return Ok(responses);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation melt responses: required {}, received {}, failures: {}",
            threshold,
            responses.len(),
            failures.join("; ")
        )))
    }

    async fn collect_restore_responses<F, Fut>(
        &self,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, RestoreResponse)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: std::future::Future<Output = Result<RestoreResponse, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut responses = Vec::new();
        let mut failures = Vec::new();

        for (member_id, connector) in &self.member_connectors {
            match call_member(connector.clone()).await {
                Ok(response) => {
                    responses.push((*member_id, response));
                    if responses.len() >= threshold {
                        break;
                    }
                }
                Err(err) => failures.push(format!("member {}: {}", member_id, err)),
            }
        }

        if responses.len() >= threshold {
            return Ok(responses);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation restore responses: required {}, received {}, failures: {}",
            threshold,
            responses.len(),
            failures.join("; ")
        )))
    }

    async fn collect_check_state_responses<F, Fut>(
        &self,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, CheckStateResponse)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: std::future::Future<Output = Result<CheckStateResponse, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut responses = Vec::new();
        let mut failures = Vec::new();

        for (member_id, connector) in &self.member_connectors {
            match call_member(connector.clone()).await {
                Ok(response) => {
                    responses.push((*member_id, response));
                    if responses.len() >= threshold {
                        break;
                    }
                }
                Err(err) => failures.push(format!("member {}: {}", member_id, err)),
            }
        }

        if responses.len() >= threshold {
            return Ok(responses);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation check-state responses: required {}, received {}, failures: {}",
            threshold,
            responses.len(),
            failures.join("; ")
        )))
    }

    fn aggregate_member_responses(
        &self,
        outputs: &[BlindedMessage],
        member_responses: &[(MemberId, MintResponse)],
        amount_validation: ShareAmountValidation,
    ) -> Result<MintResponse, Error> {
        match amount_validation {
            ShareAmountValidation::Exact => self.public_config.validate_blinded_outputs(outputs)?,
            ShareAmountValidation::AllowZeroAmountPlaceholder => self
                .public_config
                .validate_blinded_change_outputs(outputs)?,
        }

        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        if member_responses.len() < threshold {
            return Err(Error::InvalidMintResponse(format!(
                "insufficient federation signature shares: required {}, received {}",
                threshold,
                member_responses.len()
            )));
        }

        let mut aggregate_signatures = Vec::with_capacity(outputs.len());
        for (index, output) in outputs.iter().enumerate() {
            let first_signature = member_responses
                .first()
                .and_then(|(_, response)| response.signatures.get(index))
                .ok_or_else(|| {
                    Error::InvalidMintResponse(
                        "missing federation signature share for output".to_string(),
                    )
                })?;
            let mut signed_output = output.clone();
            signed_output.amount = first_signature.amount;
            signed_output.keyset_id = first_signature.keyset_id;
            let keyset = self.public_config.validate_blinded_output(&signed_output)?;
            let blinded_message = output.blinded_secret.as_bls_g1()?;
            let mut shares = Vec::with_capacity(member_responses.len());

            for (member_id, response) in member_responses {
                let signature = response.signatures.get(index).ok_or_else(|| {
                    Error::InvalidMintResponse(format!(
                        "member {} returned too few signature shares",
                        member_id
                    ))
                })?;
                validate_signature_metadata(signature, output, *member_id, amount_validation)?;
                if signature.amount != signed_output.amount
                    || signature.keyset_id != signed_output.keyset_id
                {
                    return Err(Error::InvalidMintResponse(format!(
                        "member {} returned a share that does not match the threshold set",
                        member_id
                    )));
                }

                let public_share = keyset
                    .public_shares
                    .get(member_id)
                    .ok_or(FederationError::MissingPublicShares {
                        keyset_id: keyset.id,
                        member_id: *member_id,
                    })?
                    .get(&signed_output.amount)
                    .ok_or(FederationError::MissingPublicShareAmount {
                        keyset_id: keyset.id,
                        member_id: *member_id,
                        amount: signed_output.amount,
                    })?;
                let share = BlsBlindSignatureShare::new(
                    member_id.to_bls_signer_id(),
                    signature.c.as_bls_g1()?,
                );

                if !verify_blind_signature_share(&share, &blinded_message, public_share)? {
                    return Err(Error::InvalidMintResponse(format!(
                        "member {} returned an invalid signature share for keyset {} amount {}",
                        member_id, output.keyset_id, output.amount
                    )));
                }
                shares.push(share);
            }

            let aggregate_signature = aggregate_blind_signature_shares(threshold, &shares)?;
            aggregate_signatures.push(BlindSignature {
                amount: signed_output.amount,
                keyset_id: signed_output.keyset_id,
                c: aggregate_signature.into(),
                dleq: None,
            });
        }

        for (member_id, response) in member_responses {
            if response.signatures.len() != outputs.len() {
                return Err(Error::InvalidMintResponse(format!(
                    "member {} returned {} signature shares for {} outputs",
                    member_id,
                    response.signatures.len(),
                    outputs.len()
                )));
            }
        }

        Ok(MintResponse {
            signatures: aggregate_signatures,
        })
    }

    fn aggregate_melt_response(
        &self,
        outputs: Option<&[BlindedMessage]>,
        member_responses: &[(MemberId, MeltQuoteResponse<String>)],
    ) -> Result<MeltQuoteResponse<String>, Error> {
        let (_, first_response) = member_responses.first().ok_or_else(|| {
            Error::InvalidMintResponse("missing federation melt responses".to_string())
        })?;
        for (member_id, response) in member_responses.iter().skip(1) {
            validate_melt_response_metadata(first_response, response, *member_id)?;
        }

        let Some(first_change) = first_response.change() else {
            for (member_id, response) in member_responses {
                if response.change().is_some() {
                    return Err(Error::InvalidMintResponse(format!(
                        "member {} returned melt change when the threshold set did not",
                        member_id
                    )));
                }
            }
            return Ok(first_response.clone());
        };

        let outputs = outputs.ok_or_else(|| {
            Error::InvalidMintResponse(
                "federation member returned melt change without requested change outputs"
                    .to_string(),
            )
        })?;
        self.public_config
            .validate_blinded_change_outputs(outputs)?;

        if first_change.is_empty() {
            for (member_id, response) in member_responses {
                match response.change() {
                    Some(change) if change.is_empty() => (),
                    _ => {
                        return Err(Error::InvalidMintResponse(format!(
                            "member {} returned mismatched empty melt change",
                            member_id
                        )))
                    }
                }
            }
            return Ok(first_response.clone());
        }

        let mut change_responses = Vec::with_capacity(member_responses.len());
        for (member_id, response) in member_responses {
            let change = response.change().cloned().ok_or_else(|| {
                Error::InvalidMintResponse(format!(
                    "member {} did not return melt change signatures",
                    member_id
                ))
            })?;
            change_responses.push((*member_id, MintResponse { signatures: change }));
        }

        let aggregate_change = self.aggregate_member_responses(
            outputs,
            &change_responses,
            ShareAmountValidation::AllowZeroAmountPlaceholder,
        )?;
        let mut response = first_response.clone();
        set_melt_response_change(&mut response, Some(aggregate_change.signatures));

        Ok(response)
    }

    fn aggregate_restore_response(
        &self,
        requested_outputs: &[BlindedMessage],
        member_responses: &[(MemberId, RestoreResponse)],
    ) -> Result<RestoreResponse, Error> {
        self.public_config
            .validate_blinded_change_outputs(requested_outputs)?;

        let (first_member_id, first_response) = member_responses.first().ok_or_else(|| {
            Error::InvalidMintResponse("missing federation restore responses".to_string())
        })?;
        validate_restore_response(requested_outputs, first_response, *first_member_id)?;

        for (member_id, response) in member_responses.iter().skip(1) {
            validate_restore_response(requested_outputs, response, *member_id)?;
            if response.outputs != first_response.outputs {
                return Err(Error::InvalidMintResponse(format!(
                    "member {} returned a mismatched restore output set",
                    member_id
                )));
            }
        }

        let restore_share_responses = member_responses
            .iter()
            .map(|(member_id, response)| {
                (
                    *member_id,
                    MintResponse {
                        signatures: response.signatures.clone(),
                    },
                )
            })
            .collect::<Vec<_>>();
        let aggregate = self.aggregate_member_responses(
            &first_response.outputs,
            &restore_share_responses,
            ShareAmountValidation::AllowZeroAmountPlaceholder,
        )?;

        Ok(RestoreResponse {
            outputs: first_response.outputs.clone(),
            signatures: aggregate.signatures,
        })
    }

    fn aggregate_check_state_response(
        &self,
        request: &CheckStateRequest,
        member_responses: &[(MemberId, CheckStateResponse)],
    ) -> Result<CheckStateResponse, Error> {
        let (first_member_id, first_response) = member_responses.first().ok_or_else(|| {
            Error::InvalidMintResponse("missing federation check-state responses".to_string())
        })?;
        validate_check_state_response(request, first_response, *first_member_id)?;

        for (member_id, response) in member_responses.iter().skip(1) {
            validate_check_state_response(request, response, *member_id)?;
            if response.states != first_response.states {
                return Err(Error::InvalidMintResponse(format!(
                    "member {} returned a mismatched proof state view",
                    member_id
                )));
            }
        }

        Ok(first_response.clone())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MintConnector for FederatedMintConnector {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, Error> {
        self.primary_connector()?.resolve_dns_txt(domain).await
    }

    async fn fetch_lnurl_pay_request(
        &self,
        url: &str,
    ) -> Result<crate::lightning_address::LnurlPayResponse, Error> {
        self.primary_connector()?.fetch_lnurl_pay_request(url).await
    }

    async fn fetch_lnurl_invoice(
        &self,
        url: &str,
    ) -> Result<crate::lightning_address::LnurlPayInvoiceResponse, Error> {
        self.primary_connector()?.fetch_lnurl_invoice(url).await
    }

    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        self.validate_member_active_keysets().await?;
        Ok(self.active_aggregate_keysets())
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut valid_members = 0usize;
        let mut failures = Vec::new();

        for (member_id, connector) in &self.member_connectors {
            match connector.get_mint_keyset(keyset_id).await {
                Ok(advertised) => {
                    validate_member_advertised_keyset(
                        &self.public_config,
                        *member_id,
                        &advertised,
                    )?;
                    valid_members = valid_members.saturating_add(1);
                }
                Err(err) => failures.push(format!("member {}: {}", member_id, err)),
            }
        }

        if valid_members < threshold {
            return Err(Error::InvalidMintResponse(format!(
                "insufficient federation keyset advertisements: required {}, received {}, failures: {}",
                threshold,
                valid_members,
                failures.join("; ")
            )));
        }

        self.aggregate_keyset(keyset_id)
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        Ok(KeysetResponse {
            keysets: self.aggregate_keyset_infos(),
        })
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, Error> {
        self.primary_connector()?.post_mint_quote(request).await
    }

    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<String>,
    ) -> Result<MintResponse, Error> {
        self.public_config
            .validate_blinded_outputs(&request.outputs)?;
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_mint_responses(|connector| {
                let method = method.clone();
                let request = request.clone();
                async move { connector.post_mint(&method, request).await }
            })
            .await?;
        self.aggregate_member_responses(
            &request.outputs,
            &member_responses,
            ShareAmountValidation::Exact,
        )
    }

    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteResponse<String>>, Error> {
        self.primary_connector()?
            .post_batch_check_mint_quote_status(method, request)
            .await
    }

    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<String>,
    ) -> Result<MintResponse, Error> {
        self.public_config
            .validate_blinded_outputs(&request.outputs)?;
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_mint_responses(|connector| {
                let method = method.clone();
                let request = request.clone();
                async move { connector.post_batch_mint(&method, request).await }
            })
            .await?;
        self.aggregate_member_responses(
            &request.outputs,
            &member_responses,
            ShareAmountValidation::Exact,
        )
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error> {
        self.primary_connector()?.post_melt_quote(request).await
    }

    async fn get_mint_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, Error> {
        self.primary_connector()?
            .get_mint_quote_status(method, quote_id)
            .await
    }

    async fn get_melt_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        self.primary_connector()?
            .get_melt_quote_status(method, quote_id)
            .await
    }

    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        let outputs = request.outputs().clone();
        if let Some(outputs) = &outputs {
            self.public_config
                .validate_blinded_change_outputs(outputs)?;
        }
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_melt_responses(|connector| {
                let method = method.clone();
                let request = request.clone();
                async move { connector.post_melt(&method, request).await }
            })
            .await?;
        self.aggregate_melt_response(outputs.as_deref(), &member_responses)
    }

    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, Error> {
        self.public_config
            .validate_blinded_outputs(request.outputs())?;
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_mint_responses(|connector| {
                let request = request.clone();
                async move {
                    connector
                        .post_swap(request)
                        .await
                        .map(|response| MintResponse {
                            signatures: response.signatures,
                        })
                }
            })
            .await?;
        let response = self.aggregate_member_responses(
            request.outputs(),
            &member_responses,
            ShareAmountValidation::Exact,
        )?;

        Ok(SwapResponse {
            signatures: response.signatures,
        })
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        self.primary_connector()?.get_mint_info().await
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_check_state_responses(|connector| {
                let request = request.clone();
                async move { connector.post_check_state(request).await }
            })
            .await?;

        self.aggregate_check_state_response(&request, &member_responses)
    }

    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        self.public_config
            .validate_blinded_change_outputs(&request.outputs)?;
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_restore_responses(|connector| {
                let request = request.clone();
                async move { connector.post_restore(request).await }
            })
            .await?;

        self.aggregate_restore_response(&request.outputs, &member_responses)
    }

    async fn get_auth_wallet(&self) -> Option<AuthWallet> {
        match self.primary_connector() {
            Ok(connector) => connector.get_auth_wallet().await,
            Err(_) => None,
        }
    }

    async fn set_auth_wallet(&self, wallet: Option<AuthWallet>) {
        if let Ok(connector) = self.primary_connector() {
            connector.set_auth_wallet(wallet).await;
        }
    }
}

fn federated_keyset_to_keyset(keyset: &FederatedKeyset) -> KeySet {
    KeySet {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: Some(keyset.active),
        keys: keyset.keys(),
        input_fee_ppk: keyset.input_fee_ppk,
        final_expiry: keyset.final_expiry,
    }
}

fn federated_keyset_to_keyset_info(keyset: &FederatedKeyset) -> KeySetInfo {
    KeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: keyset.active,
        input_fee_ppk: keyset.input_fee_ppk,
        final_expiry: keyset.final_expiry,
    }
}

fn validate_signature_metadata(
    signature: &BlindSignature,
    output: &BlindedMessage,
    member_id: MemberId,
    amount_validation: ShareAmountValidation,
) -> Result<(), Error> {
    let amount_matches = match amount_validation {
        ShareAmountValidation::Exact => signature.amount == output.amount,
        ShareAmountValidation::AllowZeroAmountPlaceholder => {
            output.amount == Amount::ZERO || signature.amount == output.amount
        }
    };
    if !amount_matches {
        return Err(Error::InvalidMintResponse(format!(
            "member {} signature amount ({}) does not match requested amount ({})",
            member_id, signature.amount, output.amount
        )));
    }
    if signature.keyset_id != output.keyset_id {
        return Err(Error::InvalidMintResponse(format!(
            "member {} signature keyset ({}) does not match requested keyset ({})",
            member_id, signature.keyset_id, output.keyset_id
        )));
    }
    if signature.dleq.is_some() {
        return Err(Error::InvalidMintResponse(format!(
            "member {} returned DLEQ data for a BLS federation share",
            member_id
        )));
    }

    Ok(())
}

fn validate_melt_response_metadata(
    expected: &MeltQuoteResponse<String>,
    response: &MeltQuoteResponse<String>,
    member_id: MemberId,
) -> Result<(), Error> {
    if expected.method() != response.method()
        || expected.quote() != response.quote()
        || expected.amount() != response.amount()
        || expected.fee_reserve() != response.fee_reserve()
        || expected.state() != response.state()
        || expected.expiry() != response.expiry()
        || expected.payment_proof() != response.payment_proof()
        || expected.request() != response.request()
        || expected.unit() != response.unit()
    {
        return Err(Error::InvalidMintResponse(format!(
            "member {} returned mismatched melt response metadata",
            member_id
        )));
    }

    Ok(())
}

fn validate_restore_response(
    requested_outputs: &[BlindedMessage],
    response: &RestoreResponse,
    member_id: MemberId,
) -> Result<(), Error> {
    if response.outputs.len() != response.signatures.len() {
        return Err(Error::InvalidMintResponse(format!(
            "member {} returned {} restore outputs for {} signatures",
            member_id,
            response.outputs.len(),
            response.signatures.len()
        )));
    }

    let positions = requested_outputs
        .iter()
        .enumerate()
        .map(|(index, output)| (output.blinded_secret, (index, output)))
        .collect::<BTreeMap<_, _>>();
    let mut last_position = None;

    for output in &response.outputs {
        let (position, requested_output) =
            positions.get(&output.blinded_secret).ok_or_else(|| {
                Error::InvalidMintResponse(format!(
                    "member {} returned restore output not present in request",
                    member_id
                ))
            })?;
        if output != *requested_output {
            return Err(Error::InvalidMintResponse(format!(
                "member {} returned mismatched restore output metadata",
                member_id
            )));
        }
        if last_position.is_some_and(|last| *position <= last) {
            return Err(Error::InvalidMintResponse(format!(
                "member {} returned restore outputs out of request order",
                member_id
            )));
        }
        last_position = Some(*position);
    }

    Ok(())
}

fn validate_check_state_response(
    request: &CheckStateRequest,
    response: &CheckStateResponse,
    member_id: MemberId,
) -> Result<(), Error> {
    if response.states.len() != request.ys.len() {
        return Err(Error::InvalidMintResponse(format!(
            "member {} returned {} proof states for {} requested proofs",
            member_id,
            response.states.len(),
            request.ys.len()
        )));
    }

    for (proof_state, y) in response.states.iter().zip(&request.ys) {
        if proof_state.y != *y {
            return Err(Error::InvalidMintResponse(format!(
                "member {} returned proof states out of request order",
                member_id
            )));
        }
    }

    Ok(())
}

fn set_melt_response_change(
    response: &mut MeltQuoteResponse<String>,
    change: Option<Vec<BlindSignature>>,
) {
    match response {
        MeltQuoteResponse::Bolt11(response) | MeltQuoteResponse::Bolt12(response) => {
            response.change = change;
        }
        MeltQuoteResponse::Onchain(response) => {
            response.change = change;
        }
        MeltQuoteResponse::Custom((_, response)) => {
            response.change = change;
        }
    }
}

fn validate_member_active_keysets(
    public_config: &FederationConfig,
    member_id: MemberId,
    advertised_keysets: &[KeySet],
) -> Result<(), Error> {
    public_config.validate()?;
    if !public_config.members.contains_key(&member_id) {
        return Err(FederationError::UnknownMember { member_id }.into());
    }

    for advertised in advertised_keysets {
        validate_member_advertised_keyset(public_config, member_id, advertised)?;
    }

    for keyset in public_config.keysets.iter().filter(|keyset| keyset.active) {
        if !advertised_keysets
            .iter()
            .any(|advertised| advertised.id == keyset.id)
        {
            return Err(FederationError::MissingAdvertisedFederatedKeyset {
                member_id,
                keyset_id: keyset.id,
            }
            .into());
        }
    }

    Ok(())
}

fn validate_member_advertised_keyset(
    public_config: &FederationConfig,
    member_id: MemberId,
    advertised: &KeySet,
) -> Result<(), Error> {
    if advertised.id.get_version() != crate::nuts::KeySetVersion::Version02 {
        return Err(FederationError::InvalidFederatedKeysetVersion {
            keyset_id: advertised.id,
        }
        .into());
    }

    let expected = public_config
        .keysets
        .iter()
        .find(|keyset| keyset.id == advertised.id)
        .ok_or(FederationError::UnknownFederatedKeyset {
            keyset_id: advertised.id,
        })?;
    let expected_keyset = federated_keyset_to_keyset(expected);

    if advertised.id != expected_keyset.id
        || advertised.unit != expected_keyset.unit
        || advertised.input_fee_ppk != expected_keyset.input_fee_ppk
        || advertised.final_expiry != expected_keyset.final_expiry
        || advertised.keys != expected_keyset.keys
    {
        return Err(FederationError::AdvertisedFederatedKeysetMismatch {
            member_id,
            keyset_id: expected.id,
        }
        .into());
    }
    if let Some(active) = advertised.active {
        if active != expected.active {
            return Err(FederationError::AdvertisedFederatedKeysetMismatch {
                member_id,
                keyset_id: expected.id,
            }
            .into());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::fmt;
    use std::str::FromStr;
    use std::sync::Arc;

    use async_trait::async_trait;
    use bip39::Mnemonic;
    use cdk_common::federation::{
        generate_trusted_dealer_configs, FederationError, FederationOperationId, MemberId,
        ThresholdParams, TrustedDealerConfigBundle, TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    use cdk_common::mint::{MeltPaymentRequest, MeltQuote, MintQuote};
    use cdk_common::mint_url::MintUrl;
    use cdk_common::nuts::nut01::{BlsG1PublicKey, BlsSecretKey};
    use cdk_common::nuts::nut02::KeySet;
    use cdk_common::payment::PaymentIdentifier;
    use cdk_common::util::unix_time;
    use cdk_common::{
        Amount, BlindSignature, BlindedMessage, Bolt11Invoice, CurrencyUnit, Id,
        MeltQuoteBolt11Response, MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse,
        PublicKey, QuoteId,
    };
    use cdk_common::{MintQuoteRequest, MintQuoteResponse, MintResponse, SecretKey};
    use cdk_fake_wallet::{create_fake_invoice, FakeWallet};

    use super::{FederatedMintConnector, FederationWalletConnector};
    use crate::dhke::verify_bls_blind_signature;
    use crate::mint::{FederationRuntimeConfig, Mint, MintBuilder, MintInput, MintMeltLimits};
    use crate::nuts::nut00::{KnownMethod, PaymentMethod};
    use crate::nuts::{
        BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest, CheckStateResponse,
        KeysetResponse, MeltQuoteState, MeltRequest, MintInfo, MintQuoteBolt11Request,
        MintQuoteState, MintRequest, ProofState, RestoreRequest, RestoreResponse, State,
        SwapRequest, SwapResponse,
    };
    use crate::types::{FeeReserve, QuoteTTL};
    use crate::wallet::test_utils::MockMintConnector;
    use crate::wallet::MintConnector;

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("secret key")
    }

    fn member(member_id: MemberId) -> TrustedDealerMember {
        TrustedDealerMember::new(
            member_id,
            MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
                .expect("mint url"),
            MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get()))
                .expect("fed url"),
            identity_secret(member_id),
        )
    }

    fn bundle() -> cdk_common::federation::TrustedDealerConfigBundle {
        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(1, 1).expect("params"),
            vec![member(member_id(1))],
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([(
                    Amount::from(1),
                    vec![BlsSecretKey::from_reduced_bytes(&[2; 32])],
                )]),
            )],
        )
        .expect("trusted dealer bundle")
    }

    fn threshold_bundle() -> TrustedDealerConfigBundle {
        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("params"),
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([
                    (
                        Amount::from(1),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[2; 32]),
                            BlsSecretKey::from_reduced_bytes(&[3; 32]),
                        ],
                    ),
                    (
                        Amount::from(2),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[4; 32]),
                            BlsSecretKey::from_reduced_bytes(&[5; 32]),
                        ],
                    ),
                    (
                        Amount::from(4),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[6; 32]),
                            BlsSecretKey::from_reduced_bytes(&[7; 32]),
                        ],
                    ),
                ]),
            )],
        )
        .expect("trusted dealer bundle")
    }

    fn advertised_keyset(bundle: &cdk_common::federation::TrustedDealerConfigBundle) -> KeySet {
        let keyset = &bundle.public_config.keysets[0];
        KeySet {
            id: keyset.id,
            unit: keyset.unit.clone(),
            active: Some(keyset.active),
            keys: keyset.keys(),
            input_fee_ppk: keyset.input_fee_ppk,
            final_expiry: keyset.final_expiry,
        }
    }

    fn blinded_outputs(bundle: &TrustedDealerConfigBundle) -> Vec<BlindedMessage> {
        let keyset = &bundle.public_config.keysets[0];
        vec![
            BlindedMessage::new(
                Amount::from(1),
                keyset.id,
                PublicKey::from(BlsG1PublicKey::hash_to_curve(
                    b"federated wallet connector output 1",
                )),
            ),
            BlindedMessage::new(
                Amount::from(2),
                keyset.id,
                PublicKey::from(BlsG1PublicKey::hash_to_curve(
                    b"federated wallet connector output 2",
                )),
            ),
        ]
    }

    fn blinded_change_outputs(bundle: &TrustedDealerConfigBundle) -> Vec<BlindedMessage> {
        blinded_outputs(bundle)
            .into_iter()
            .map(|mut output| {
                output.amount = Amount::ZERO;
                output
            })
            .collect()
    }

    fn signed_change_outputs(change_outputs: &[BlindedMessage]) -> Vec<BlindedMessage> {
        [Amount::from(1), Amount::from(2)]
            .into_iter()
            .zip(change_outputs.iter().cloned())
            .map(|(amount, mut output)| {
                output.amount = amount;
                output
            })
            .collect()
    }

    fn aggregate_secret_for_amount(amount: u64) -> BlsSecretKey {
        let byte = match amount {
            1 => 2,
            2 => 4,
            4 => 6,
            _ => panic!("missing aggregate secret fixture for amount {amount}"),
        };

        BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    fn proof_for_amount(
        bundle: &TrustedDealerConfigBundle,
        amount: u64,
        label: &str,
    ) -> crate::nuts::Proof {
        let keyset = &bundle.public_config.keysets[0];
        let secret = crate::secret::Secret::new(format!("federated-wallet-proof-{label}"));
        let signature = BlsG1PublicKey::hash_to_curve(secret.as_bytes())
            .mul(&aggregate_secret_for_amount(amount));

        crate::nuts::Proof::new(
            Amount::from(amount),
            keyset.id,
            secret,
            PublicKey::from(signature),
        )
    }

    fn share_response(
        bundle: &TrustedDealerConfigBundle,
        member_id: MemberId,
        outputs: &[BlindedMessage],
    ) -> MintResponse {
        let private = bundle
            .private_configs
            .get(&member_id)
            .expect("member private config");
        let share_set = private
            .sign_blinded_messages(
                &bundle.public_config,
                FederationOperationId::from_bytes([9; 32]),
                outputs,
            )
            .expect("signature shares");

        MintResponse {
            signatures: share_set
                .shares
                .into_iter()
                .map(|share| BlindSignature {
                    amount: share.amount,
                    keyset_id: share.keyset_id,
                    c: share.share.signature().into(),
                    dleq: None,
                })
                .collect(),
        }
    }

    fn member_connectors(
        bundle: &TrustedDealerConfigBundle,
        outputs: &[BlindedMessage],
    ) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
        let advertised = advertised_keyset(bundle);
        [member_id(1), member_id(2), member_id(3)]
            .into_iter()
            .map(|member_id| {
                let connector = MockMintConnector::new();
                connector.set_active_keyset(advertised.clone());
                connector.set_post_mint_response(Ok(share_response(bundle, member_id, outputs)));
                connector
                    .set_post_batch_mint_response(Ok(share_response(bundle, member_id, outputs)));
                connector.set_post_swap_response(Ok(SwapResponse {
                    signatures: share_response(bundle, member_id, outputs).signatures,
                }));
                (
                    member_id,
                    Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
                )
            })
            .collect()
    }

    fn melt_response(
        bundle: &TrustedDealerConfigBundle,
        member_id: MemberId,
        change_outputs: &[BlindedMessage],
    ) -> MeltQuoteResponse<String> {
        let signed_outputs = signed_change_outputs(change_outputs);
        MeltQuoteResponse::Bolt11(MeltQuoteBolt11Response {
            quote: "melt-quote".to_string(),
            amount: Amount::from(3),
            fee_reserve: Amount::ZERO,
            state: MeltQuoteState::Paid,
            expiry: 42,
            payment_preimage: Some("payment-preimage".to_string()),
            change: Some(share_response(bundle, member_id, &signed_outputs).signatures),
            request: Some("bolt11-invoice".to_string()),
            unit: Some(CurrencyUnit::Sat),
        })
    }

    fn melt_response_without_change() -> MeltQuoteResponse<String> {
        MeltQuoteResponse::Bolt11(MeltQuoteBolt11Response {
            quote: "melt-quote".to_string(),
            amount: Amount::from(1),
            fee_reserve: Amount::ZERO,
            state: MeltQuoteState::Paid,
            expiry: 42,
            payment_preimage: Some("payment-preimage".to_string()),
            change: None,
            request: Some("bolt11-invoice".to_string()),
            unit: Some(CurrencyUnit::Sat),
        })
    }

    fn restore_response(
        bundle: &TrustedDealerConfigBundle,
        member_id: MemberId,
        restore_outputs: &[BlindedMessage],
    ) -> RestoreResponse {
        RestoreResponse {
            outputs: restore_outputs.to_vec(),
            signatures: share_response(bundle, member_id, &signed_change_outputs(restore_outputs))
                .signatures,
        }
    }

    fn proof_state_response(ys: &[PublicKey], states: &[State]) -> CheckStateResponse {
        CheckStateResponse {
            states: ys
                .iter()
                .copied()
                .zip(states.iter().copied())
                .map(|(y, state)| ProofState {
                    y,
                    state,
                    witness: None,
                })
                .collect(),
        }
    }

    fn member_connectors_with_melt_change(
        bundle: &TrustedDealerConfigBundle,
        change_outputs: &[BlindedMessage],
    ) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
        let advertised = advertised_keyset(bundle);
        [member_id(1), member_id(2), member_id(3)]
            .into_iter()
            .map(|member_id| {
                let connector = MockMintConnector::new();
                connector.set_active_keyset(advertised.clone());
                connector.set_post_melt_response(Ok(melt_response(
                    bundle,
                    member_id,
                    change_outputs,
                )));
                (
                    member_id,
                    Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
                )
            })
            .collect()
    }

    #[derive(Clone)]
    struct LocalFederatedMemberConnector {
        mint: Arc<Mint>,
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
        fn new(mint: Arc<Mint>) -> Self {
            Self { mint }
        }

        fn federation_config(&self) -> Result<&FederationRuntimeConfig, crate::Error> {
            self.mint.federation_config().ok_or_else(|| {
                crate::Error::InvalidMintResponse(
                    "local test mint is not configured for federation".to_string(),
                )
            })
        }

        fn quote_id(quote: &str) -> Result<QuoteId, crate::Error> {
            QuoteId::from_str(quote).map_err(crate::Error::from)
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
            Ok(self.mint.pubkeys().keysets)
        }

        async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, crate::Error> {
            self.mint
                .keyset_pubkeys(&keyset_id)?
                .keysets
                .into_iter()
                .next()
                .ok_or(crate::Error::UnknownKeySet)
        }

        async fn get_mint_keysets(&self) -> Result<KeysetResponse, crate::Error> {
            Ok(self.mint.keysets())
        }

        async fn post_mint_quote(
            &self,
            request: MintQuoteRequest,
        ) -> Result<MintQuoteResponse<String>, crate::Error> {
            Ok(self.mint.get_mint_quote(request).await?.into())
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
            let config = self.federation_config()?;
            let operation_id = config.mint_operation(input.clone())?.operation_id()?;
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
            let config = self.federation_config()?;
            let operation_id = config.mint_operation(input.clone())?.operation_id()?;
            config.mint_share_response(operation_id, &input)
        }

        async fn post_melt_quote(
            &self,
            request: MeltQuoteRequest,
        ) -> Result<MeltQuoteCreateResponse<String>, crate::Error> {
            Ok(self.mint.get_melt_quote(request).await?.into())
        }

        async fn get_mint_quote_status(
            &self,
            _method: PaymentMethod,
            quote_id: &str,
        ) -> Result<MintQuoteResponse<String>, crate::Error> {
            let quote_id = Self::quote_id(quote_id)?;
            let response = self
                .mint
                .check_mint_quotes(std::slice::from_ref(&quote_id))
                .await?
                .into_iter()
                .next()
                .ok_or(crate::Error::UnknownQuote)?;

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
            let config = self.federation_config()?;
            let operation_id = config.melt_operation(request.clone())?.operation_id()?;

            Ok(self
                .mint
                .melt_federation(&request, operation_id)
                .await?
                .into())
        }

        async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, crate::Error> {
            let config = self.federation_config()?;
            let operation_id = config.swap_operation(request.clone())?.operation_id()?;
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

        async fn post_restore(
            &self,
            request: RestoreRequest,
        ) -> Result<RestoreResponse, crate::Error> {
            self.mint.restore(request).await
        }

        async fn get_auth_wallet(&self) -> Option<crate::wallet::AuthWallet> {
            None
        }

        async fn set_auth_wallet(&self, _wallet: Option<crate::wallet::AuthWallet>) {}
    }

    async fn create_fake_wallet_member_mint(
        bundle: &TrustedDealerConfigBundle,
        member_id: MemberId,
    ) -> Mint {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let runtime_config = FederationRuntimeConfig::new(
            bundle.public_config.clone(),
            bundle
                .private_configs
                .get(&member_id)
                .expect("private member config")
                .clone(),
        )
        .expect("runtime config");
        let mut builder = MintBuilder::new(db.clone())
            .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
            .with_federation_config(runtime_config);
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: Amount::from(1),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(fake),
            )
            .await
            .expect("payment processor");

        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let mint = builder
            .build_with_seed(db, &mnemonic.to_seed_normalized(""))
            .await
            .expect("mint");
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
            .await
            .expect("quote ttl");
        mint.start().await.expect("mint start");
        mint
    }

    async fn add_paid_bolt11_quote(mint: &Mint, quote_id: &QuoteId, amount: u64) {
        let amount = Amount::new(amount, CurrencyUnit::Sat);
        let quote = MintQuote::new(
            Some(quote_id.clone()),
            format!("invoice-{quote_id}"),
            CurrencyUnit::Sat,
            Some(amount.clone()),
            unix_time() + 10_000,
            PaymentIdentifier::QuoteId(quote_id.clone()),
            None,
            amount,
            Amount::new(0, CurrencyUnit::Sat),
            PaymentMethod::Known(KnownMethod::Bolt11),
            unix_time(),
            Vec::new(),
            Vec::new(),
            None,
        );
        let localstore = mint.localstore();
        let mut tx = localstore.begin_transaction().await.expect("begin tx");
        tx.add_mint_quote(quote).await.expect("add quote");
        tx.commit().await.expect("commit quote");
    }

    async fn add_unpaid_bolt11_melt_quote(
        mint: &Mint,
        quote_id: &QuoteId,
        invoice: &Bolt11Invoice,
        amount: u64,
        expiry: u64,
    ) {
        let amount = Amount::new(amount, CurrencyUnit::Sat);
        let quote = MeltQuote::new(
            Some(quote_id.clone()),
            MeltPaymentRequest::Bolt11 {
                bolt11: invoice.clone(),
            },
            CurrencyUnit::Sat,
            amount,
            Amount::new(1, CurrencyUnit::Sat),
            expiry,
            Some(PaymentIdentifier::PaymentHash(
                *invoice.payment_hash().as_ref(),
            )),
            None,
            PaymentMethod::Known(KnownMethod::Bolt11),
            None,
            None,
        );
        let localstore = mint.localstore();
        let mut tx = localstore.begin_transaction().await.expect("begin tx");
        tx.add_melt_quote(quote).await.expect("add melt quote");
        tx.commit().await.expect("commit melt quote");
    }

    async fn fake_wallet_member_connectors_with_melt_quote(
        bundle: &TrustedDealerConfigBundle,
        mint_quote_id: &QuoteId,
        mint_quote_amount: u64,
        melt_quote_id: &QuoteId,
        melt_invoice: &Bolt11Invoice,
        melt_amount: u64,
    ) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
        let mut connectors = BTreeMap::new();
        let melt_expiry = unix_time() + 10_000;
        for member_id in [member_id(1), member_id(2), member_id(3)] {
            let mint = create_fake_wallet_member_mint(bundle, member_id).await;
            add_paid_bolt11_quote(&mint, mint_quote_id, mint_quote_amount).await;
            add_unpaid_bolt11_melt_quote(
                &mint,
                melt_quote_id,
                melt_invoice,
                melt_amount,
                melt_expiry,
            )
            .await;
            connectors.insert(
                member_id,
                Arc::new(LocalFederatedMemberConnector::new(Arc::new(mint)))
                    as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        connectors
    }

    fn assert_aggregate_response_valid(
        bundle: &TrustedDealerConfigBundle,
        outputs: &[BlindedMessage],
        response: &MintResponse,
    ) {
        let keyset = advertised_keyset(bundle);

        assert_eq!(response.signatures.len(), outputs.len());
        for (signature, output) in response.signatures.iter().zip(outputs) {
            let aggregate_key = keyset.keys.amount_key(output.amount).expect("amount key");
            verify_bls_blind_signature(aggregate_key, signature.c, output.blinded_secret)
                .expect("aggregate signature verifies");
        }
    }

    #[test]
    fn wallet_connector_accepts_matching_v3_aggregate_keysets() {
        let bundle = bundle();
        let advertised = advertised_keyset(&bundle);

        FederationWalletConnector::validate_member_keysets(
            &bundle.public_config,
            member_id(1),
            &[advertised],
        )
        .expect("matching member keysets");
    }

    #[test]
    fn wallet_connector_rejects_non_v3_aggregate_keysets() {
        let bundle = bundle();
        let mut advertised = advertised_keyset(&bundle);
        let expected = &bundle.public_config.keysets[0];
        advertised.id = Id::v2_from_data(
            &advertised.keys,
            &expected.unit,
            expected.input_fee_ppk,
            expected.final_expiry,
        );

        assert!(matches!(
            FederationWalletConnector::validate_member_keysets(
                &bundle.public_config,
                member_id(1),
                &[advertised],
            ),
            Err(crate::Error::Federation(
                FederationError::UnknownFederatedKeyset { .. }
                    | FederationError::InvalidFederatedKeysetVersion { .. }
            ))
        ));
    }

    #[test]
    fn wallet_connector_rejects_mismatched_aggregate_keysets() {
        let bundle = bundle();
        let mut advertised = advertised_keyset(&bundle);
        advertised.keys = cdk_common::Keys::new(BTreeMap::new());

        assert!(matches!(
            FederationWalletConnector::validate_member_keysets(
                &bundle.public_config,
                member_id(1),
                &[advertised],
            ),
            Err(crate::Error::Federation(
                FederationError::Nut02(_)
                    | FederationError::AdvertisedFederatedKeysetMismatch { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn federated_mint_connector_aggregates_mint_signature_shares() {
        let bundle = threshold_bundle();
        let outputs = blinded_outputs(&bundle);
        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            member_connectors(&bundle, &outputs),
        )
        .expect("federated mint connector");
        let response = connector
            .post_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MintRequest {
                    quote: "paid-quote".to_string(),
                    outputs: outputs.clone(),
                    signature: None,
                },
            )
            .await
            .expect("mint response");

        assert_aggregate_response_valid(&bundle, &outputs, &response);
    }

    #[tokio::test]
    async fn federated_mint_connector_tolerates_offline_member_key_discovery() {
        let bundle = threshold_bundle();
        let outputs = blinded_outputs(&bundle);
        let advertised = advertised_keyset(&bundle);
        let mut connectors = BTreeMap::new();

        let offline_member = MockMintConnector::new();
        offline_member.set_active_keyset(advertised.clone());
        offline_member.set_mint_keys_error("member unavailable");
        offline_member
            .set_post_mint_response(Err(crate::Error::Custom("member unavailable".to_string())));
        connectors.insert(
            member_id(1),
            Arc::new(offline_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        for member_id in [member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .post_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MintRequest {
                    quote: "paid-quote".to_string(),
                    outputs: outputs.clone(),
                    signature: None,
                },
            )
            .await
            .expect("mint response");

        assert_aggregate_response_valid(&bundle, &outputs, &response);
    }

    #[tokio::test]
    async fn federated_mint_connector_aggregates_batch_mint_signature_shares() {
        let bundle = threshold_bundle();
        let outputs = blinded_outputs(&bundle);
        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            member_connectors(&bundle, &outputs),
        )
        .expect("federated mint connector");
        let response = connector
            .post_batch_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                BatchMintRequest {
                    quotes: vec!["paid-quote-1".to_string(), "paid-quote-2".to_string()],
                    quote_amounts: None,
                    outputs: outputs.clone(),
                    signatures: None,
                },
            )
            .await
            .expect("batch mint response");

        assert_aggregate_response_valid(&bundle, &outputs, &response);
    }

    #[tokio::test]
    async fn federated_mint_connector_aggregates_fake_wallet_member_shares() {
        let bundle = threshold_bundle();
        let outputs = blinded_outputs(&bundle);
        let quote_id = QuoteId::new();
        let melt_quote_id = QuoteId::new();
        let melt_invoice = create_fake_invoice(1000, "".to_string());
        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            fake_wallet_member_connectors_with_melt_quote(
                &bundle,
                &quote_id,
                3,
                &melt_quote_id,
                &melt_invoice,
                1,
            )
            .await,
        )
        .expect("federated mint connector");

        let fake_quote = connector
            .post_mint_quote(MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(1),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }))
            .await
            .expect("fake wallet mint quote");
        let fake_quote_id = fake_quote.quote().to_string();
        let mut settled_state = fake_quote.state();
        for _ in 0..100 {
            let status = connector
                .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), &fake_quote_id)
                .await
                .expect("fake wallet quote status");
            settled_state = status.state();
            if settled_state == Some(MintQuoteState::Paid) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(settled_state, Some(MintQuoteState::Paid));

        let mint_response = connector
            .post_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MintRequest {
                    quote: quote_id.to_string(),
                    outputs: outputs.clone(),
                    signature: None,
                },
            )
            .await
            .expect("mint response");
        assert_aggregate_response_valid(&bundle, &outputs, &mint_response);

        let batch_response = connector
            .post_batch_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                BatchMintRequest {
                    quotes: vec![quote_id.to_string()],
                    quote_amounts: None,
                    outputs: outputs.clone(),
                    signatures: None,
                },
            )
            .await
            .expect("batch mint response");
        assert_aggregate_response_valid(&bundle, &outputs, &batch_response);

        let swap_response = connector
            .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
            .await
            .expect("swap response");
        assert_aggregate_response_valid(
            &bundle,
            &outputs,
            &MintResponse {
                signatures: swap_response.signatures,
            },
        );

        let change_outputs = vec![blinded_change_outputs(&bundle)
            .into_iter()
            .next()
            .expect("change output")];
        let melt_response = connector
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MeltRequest::new(
                    melt_quote_id.to_string(),
                    vec![proof_for_amount(&bundle, 4, "fake-wallet-melt")],
                    Some(change_outputs.clone()),
                ),
            )
            .await
            .expect("melt response");
        let change = melt_response.change().cloned().expect("change signatures");
        let mut signed_change_output = change_outputs[0].clone();
        signed_change_output.amount = Amount::from(2);
        assert_eq!(melt_response.state(), MeltQuoteState::Paid);
        assert_aggregate_response_valid(
            &bundle,
            &[signed_change_output],
            &MintResponse { signatures: change },
        );
    }

    #[tokio::test]
    async fn federated_mint_connector_aggregates_swap_signature_shares() {
        let bundle = threshold_bundle();
        let outputs = blinded_outputs(&bundle);
        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            member_connectors(&bundle, &outputs),
        )
        .expect("federated mint connector");
        let response = connector
            .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
            .await
            .expect("swap response");

        assert_aggregate_response_valid(
            &bundle,
            &outputs,
            &MintResponse {
                signatures: response.signatures,
            },
        );
    }

    #[tokio::test]
    async fn federated_mint_connector_aggregates_melt_change_signature_shares() {
        let bundle = threshold_bundle();
        let change_outputs = blinded_change_outputs(&bundle);
        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            member_connectors_with_melt_change(&bundle, &change_outputs),
        )
        .expect("federated mint connector");
        let response = connector
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MeltRequest::new(
                    "melt-quote".to_string(),
                    Vec::new(),
                    Some(change_outputs.clone()),
                ),
            )
            .await
            .expect("melt response");
        let change = response.change().cloned().expect("change signatures");

        assert_aggregate_response_valid(
            &bundle,
            &signed_change_outputs(&change_outputs),
            &MintResponse { signatures: change },
        );
    }

    #[tokio::test]
    async fn federated_mint_connector_fans_out_melt_without_change() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let mut connectors = BTreeMap::new();

        let failing_member = MockMintConnector::new();
        failing_member.set_active_keyset(advertised.clone());
        failing_member
            .set_post_melt_response(Err(crate::Error::Custom("member unavailable".to_string())));
        connectors.insert(
            member_id(1),
            Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        for member_id in [member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_post_melt_response(Ok(melt_response_without_change()));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MeltRequest::new("melt-quote".to_string(), Vec::new(), None),
            )
            .await
            .expect("melt response");

        assert_eq!(response.state(), MeltQuoteState::Paid);
        assert!(response.change().is_none());
    }

    #[tokio::test]
    async fn federated_mint_connector_aggregates_restore_signature_shares() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let restore_outputs = blinded_change_outputs(&bundle);
        let connectors = [member_id(1), member_id(2), member_id(3)]
            .into_iter()
            .map(|member_id| {
                let connector = MockMintConnector::new();
                connector.set_active_keyset(advertised.clone());
                connector._set_restore_response(Ok(restore_response(
                    &bundle,
                    member_id,
                    &restore_outputs,
                )));
                (
                    member_id,
                    Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
                )
            })
            .collect();
        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .post_restore(RestoreRequest {
                outputs: restore_outputs.clone(),
            })
            .await
            .expect("restore response");

        assert_eq!(response.outputs, restore_outputs);
        assert_aggregate_response_valid(
            &bundle,
            &signed_change_outputs(&restore_outputs),
            &MintResponse {
                signatures: response.signatures,
            },
        );
    }

    #[tokio::test]
    async fn federated_mint_connector_rejects_mismatched_restore_views() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let restore_outputs = blinded_change_outputs(&bundle);
        let mut connectors = BTreeMap::new();

        for member_id in [member_id(1), member_id(2)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            let member_outputs = if member_id.get() == 2 {
                restore_outputs[0..1].to_vec()
            } else {
                restore_outputs.clone()
            };
            connector._set_restore_response(Ok(restore_response(
                &bundle,
                member_id,
                &member_outputs,
            )));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised);
        connector._set_restore_response(Ok(restore_response(
            &bundle,
            member_id(3),
            &restore_outputs,
        )));
        connectors.insert(
            member_id(3),
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let result = connector
            .post_restore(RestoreRequest {
                outputs: restore_outputs,
            })
            .await;

        assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
    }

    #[tokio::test]
    async fn federated_mint_connector_collects_threshold_check_state_views() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let ys = vec![
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-y-1")),
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-y-2")),
        ];
        let expected = proof_state_response(&ys, &[State::Unspent, State::Pending]);
        let mut connectors = BTreeMap::new();

        let failing_member = MockMintConnector::new();
        failing_member.set_active_keyset(advertised.clone());
        failing_member
            .set_check_state_response(Err(crate::Error::Custom("member unavailable".to_string())));
        connectors.insert(
            member_id(1),
            Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        for member_id in [member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_check_state_response(Ok(expected.clone()));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .post_check_state(CheckStateRequest { ys })
            .await
            .expect("check-state response");

        assert_eq!(response, expected);
    }

    #[tokio::test]
    async fn federated_mint_connector_rejects_mismatched_check_state_views() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let ys = vec![
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-mismatch-y-1")),
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-mismatch-y-2")),
        ];
        let first = proof_state_response(&ys, &[State::Unspent, State::Pending]);
        let second = proof_state_response(&ys, &[State::Spent, State::Pending]);
        let mut connectors = BTreeMap::new();

        for (member_id, response) in [(member_id(1), first), (member_id(2), second)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_check_state_response(Ok(response));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised);
        connector.set_check_state_response(Ok(proof_state_response(
            &ys,
            &[State::Unspent, State::Pending],
        )));
        connectors.insert(
            member_id(3),
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let result = connector.post_check_state(CheckStateRequest { ys }).await;

        assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
    }

    #[tokio::test]
    async fn federated_mint_connector_rejects_non_v3_melt_change_outputs() {
        let bundle = threshold_bundle();
        let change_outputs = blinded_change_outputs(&bundle);
        let mut bad_change_outputs = change_outputs.clone();
        let keyset = &bundle.public_config.keysets[0];
        bad_change_outputs[0].keyset_id = Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        );
        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            member_connectors_with_melt_change(&bundle, &change_outputs),
        )
        .expect("federated mint connector");
        let result = connector
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MeltRequest::new(
                    "melt-quote".to_string(),
                    Vec::new(),
                    Some(bad_change_outputs),
                ),
            )
            .await;

        assert!(matches!(
            result,
            Err(crate::Error::Federation(
                FederationError::InvalidFederatedKeysetVersion { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn federated_mint_connector_rejects_non_v3_member_advertisement() {
        let bundle = threshold_bundle();
        let outputs = blinded_outputs(&bundle);
        let mut connectors = member_connectors(&bundle, &outputs);
        let mut advertised = advertised_keyset(&bundle);
        let expected = &bundle.public_config.keysets[0];
        advertised.id = Id::v2_from_data(
            &advertised.keys,
            &expected.unit,
            expected.input_fee_ppk,
            expected.final_expiry,
        );
        let bad_member = MockMintConnector::new();
        bad_member.set_active_keyset(advertised);
        connectors.insert(
            member_id(1),
            Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let result = connector
            .post_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MintRequest {
                    quote: "paid-quote".to_string(),
                    outputs,
                    signature: None,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(crate::Error::Federation(
                FederationError::InvalidFederatedKeysetVersion { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn federated_mint_connector_rejects_mismatched_member_advertisement() {
        let bundle = threshold_bundle();
        let outputs = blinded_outputs(&bundle);
        let mut connectors = member_connectors(&bundle, &outputs);
        let mut advertised = advertised_keyset(&bundle);
        advertised.keys = cdk_common::Keys::new(BTreeMap::new());
        let bad_member = MockMintConnector::new();
        bad_member.set_active_keyset(advertised);
        connectors.insert(
            member_id(1),
            Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let result = connector
            .post_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MintRequest {
                    quote: "paid-quote".to_string(),
                    outputs,
                    signature: None,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(crate::Error::Federation(
                FederationError::AdvertisedFederatedKeysetMismatch { .. }
            ))
        ));
    }
}
