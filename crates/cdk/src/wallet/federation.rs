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
use futures::future::join_all;
use futures::stream::{FuturesUnordered, StreamExt};

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
    Restore,
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
        let responses = join_all(self.member_connectors.iter().map(|(member_id, connector)| {
            let member_id = *member_id;
            let connector = connector.clone();
            async move { (member_id, connector.get_mint_keys().await) }
        }))
        .await;

        for (member_id, result) in responses {
            match result {
                Ok(advertised) => {
                    validate_member_active_keysets(&self.public_config, member_id, &advertised)?;
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
        let mut pending = FuturesUnordered::new();

        for (member_id, connector) in &self.member_connectors {
            let member_id = *member_id;
            let fut = call_member(connector.clone());
            pending.push(async move { (member_id, fut.await) });
        }

        while let Some((member_id, result)) = pending.next().await {
            match result {
                Ok(response) => {
                    responses.push((member_id, response));
                    if responses.len() >= threshold {
                        return Ok(responses);
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
        let mut pending = FuturesUnordered::new();

        for (member_id, connector) in &self.member_connectors {
            let member_id = *member_id;
            let fut = call_member(connector.clone());
            pending.push(async move { (member_id, fut.await) });
        }

        while let Some((member_id, result)) = pending.next().await {
            match result {
                Ok(response) => {
                    responses.push((member_id, response));
                    if responses.len() >= threshold {
                        return Ok(responses);
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
        let mut pending = FuturesUnordered::new();

        for (member_id, connector) in &self.member_connectors {
            let member_id = *member_id;
            let fut = call_member(connector.clone());
            pending.push(async move { (member_id, fut.await) });
        }

        while let Some((member_id, result)) = pending.next().await {
            match result {
                Ok(response) => {
                    responses.push((member_id, response));
                    if responses.len() >= threshold {
                        return Ok(responses);
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

    async fn collect_melt_quote_status_responses<F, Fut>(
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
        let mut primary_response = None;
        let mut pending = FuturesUnordered::new();

        for (member_id, connector) in &self.member_connectors {
            let member_id = *member_id;
            let fut = call_member(connector.clone());
            pending.push(async move { (member_id, fut.await) });
        }

        while let Some((member_id, result)) = pending.next().await {
            match result {
                Ok(response) => {
                    if member_id == self.primary_member_id {
                        primary_response = Some(response.clone());
                    }
                    responses.push((member_id, response));
                    if responses.len() >= threshold {
                        return Ok(responses);
                    }
                }
                Err(err) => failures.push(format!("member {}: {}", member_id, err)),
            }
        }

        if responses.len() >= threshold {
            return Ok(responses);
        }

        if let Some(response) = primary_response {
            return Ok(vec![(self.primary_member_id, response)]);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation melt quote status responses: required {}, received {}, failures: {}",
            threshold,
            responses.len(),
            failures.join("; ")
        )))
    }

    async fn collect_mint_quote_status_responses<F, Fut>(
        &self,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, MintQuoteResponse<String>)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: std::future::Future<Output = Result<MintQuoteResponse<String>, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut responses = Vec::new();
        let mut failures = Vec::new();
        let mut primary_response = None;
        let mut pending = FuturesUnordered::new();

        for (member_id, connector) in &self.member_connectors {
            let member_id = *member_id;
            let fut = call_member(connector.clone());
            pending.push(async move { (member_id, fut.await) });
        }

        while let Some((member_id, result)) = pending.next().await {
            match result {
                Ok(response) => {
                    if member_id == self.primary_member_id {
                        primary_response = Some(response.clone());
                    }
                    responses.push((member_id, response));
                    if responses.len() >= threshold {
                        return Ok(responses);
                    }
                }
                Err(err) => failures.push(format!("member {}: {}", member_id, err)),
            }
        }

        if responses.len() >= threshold {
            return Ok(responses);
        }

        if let Some(response) = primary_response {
            return Ok(vec![(self.primary_member_id, response)]);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation mint quote status responses: required {}, received {}, failures: {}",
            threshold,
            responses.len(),
            failures.join("; ")
        )))
    }

    async fn collect_batch_mint_quote_status_responses<F, Fut>(
        &self,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, Vec<MintQuoteResponse<String>>)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<MintQuoteResponse<String>>, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut responses = Vec::new();
        let mut failures = Vec::new();
        let mut primary_response = None;
        let mut pending = FuturesUnordered::new();

        for (member_id, connector) in &self.member_connectors {
            let member_id = *member_id;
            let fut = call_member(connector.clone());
            pending.push(async move { (member_id, fut.await) });
        }

        while let Some((member_id, result)) = pending.next().await {
            match result {
                Ok(response) => {
                    if member_id == self.primary_member_id {
                        primary_response = Some(response.clone());
                    }
                    responses.push((member_id, response));
                    if responses.len() >= threshold {
                        return Ok(responses);
                    }
                }
                Err(err) => failures.push(format!("member {}: {}", member_id, err)),
            }
        }

        if responses.len() >= threshold {
            return Ok(responses);
        }

        if let Some(response) = primary_response {
            return Ok(vec![(self.primary_member_id, response)]);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation batch mint quote status responses: required {}, received {}, failures: {}",
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
        let member_responses =
            join_all(self.member_connectors.iter().map(|(member_id, connector)| {
                let member_id = *member_id;
                let connector = connector.clone();
                let fut = call_member(connector);
                async move { (member_id, fut.await) }
            }))
            .await;

        for (member_id, result) in member_responses {
            match result {
                Ok(response) => {
                    responses.push((member_id, response));
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
            ShareAmountValidation::Restore => self
                .public_config
                .validate_blinded_restore_outputs(outputs)?,
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
            let keyset = match amount_validation {
                ShareAmountValidation::Exact
                | ShareAmountValidation::AllowZeroAmountPlaceholder => {
                    self.public_config.validate_blinded_output(&signed_output)?
                }
                ShareAmountValidation::Restore => self
                    .public_config
                    .validate_blinded_restore_output(&signed_output)?,
            };
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

    fn aggregate_mint_quote_status_response(
        &self,
        member_responses: &[(MemberId, MintQuoteResponse<String>)],
    ) -> Result<MintQuoteResponse<String>, Error> {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);

        if member_responses.len() == 1 {
            return member_responses
                .first()
                .map(|(_, response)| response.clone())
                .ok_or_else(|| {
                    Error::InvalidMintResponse(
                        "missing federation mint quote status responses".to_string(),
                    )
                });
        }

        let mut matching_views =
            Vec::<(serde_json::Value, MintQuoteResponse<String>, Vec<MemberId>)>::new();

        for (member_id, response) in member_responses {
            let view = serde_json::to_value(response).map_err(|err| {
                Error::InvalidMintResponse(format!(
                    "member {} returned unserializable mint quote status response: {}",
                    member_id, err
                ))
            })?;

            match matching_views
                .iter_mut()
                .find(|(candidate, _, _)| *candidate == view)
            {
                Some((_, _, members)) => members.push(*member_id),
                None => matching_views.push((view, response.clone(), vec![*member_id])),
            }
        }

        matching_views
            .into_iter()
            .find_map(|(_, response, members)| {
                if members.len() >= threshold {
                    Some(response)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                Error::InvalidMintResponse(format!(
                    "insufficient matching federation mint quote status responses: required {}",
                    threshold
                ))
            })
    }

    fn aggregate_batch_mint_quote_status_response(
        &self,
        expected_count: usize,
        member_responses: &[(MemberId, Vec<MintQuoteResponse<String>>)],
    ) -> Result<Vec<MintQuoteResponse<String>>, Error> {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);

        if member_responses.len() == 1 {
            let response = member_responses
                .first()
                .map(|(_, response)| response.clone())
                .ok_or_else(|| {
                    Error::InvalidMintResponse(
                        "missing federation batch mint quote status responses".to_string(),
                    )
                })?;
            if response.len() != expected_count {
                return Err(Error::InvalidMintResponse(format!(
                    "primary member returned {} batch mint quote statuses for {} requested quotes",
                    response.len(),
                    expected_count
                )));
            }
            return Ok(response);
        }

        let mut matching_views = Vec::<(
            serde_json::Value,
            Vec<MintQuoteResponse<String>>,
            Vec<MemberId>,
        )>::new();

        for (member_id, response) in member_responses {
            if response.len() != expected_count {
                return Err(Error::InvalidMintResponse(format!(
                    "member {} returned {} batch mint quote statuses for {} requested quotes",
                    member_id,
                    response.len(),
                    expected_count
                )));
            }
            let view = serde_json::to_value(response).map_err(|err| {
                Error::InvalidMintResponse(format!(
                    "member {} returned unserializable batch mint quote status response: {}",
                    member_id, err
                ))
            })?;

            match matching_views
                .iter_mut()
                .find(|(candidate, _, _)| *candidate == view)
            {
                Some((_, _, members)) => members.push(*member_id),
                None => matching_views.push((view, response.clone(), vec![*member_id])),
            }
        }

        matching_views
            .into_iter()
            .find_map(|(_, response, members)| {
                if members.len() >= threshold {
                    Some(response)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                Error::InvalidMintResponse(format!(
                    "insufficient matching federation batch mint quote status responses: required {}",
                    threshold
                ))
            })
    }

    fn aggregate_melt_quote_status_response(
        &self,
        member_responses: &[(MemberId, MeltQuoteResponse<String>)],
    ) -> Result<MeltQuoteResponse<String>, Error> {
        let (_, first_response) = member_responses.first().ok_or_else(|| {
            Error::InvalidMintResponse("missing federation melt quote status responses".to_string())
        })?;
        for (member_id, response) in member_responses.iter().skip(1) {
            validate_melt_response_metadata(first_response, response, *member_id)?;
        }

        let Some(first_change) = first_response.change() else {
            for (member_id, response) in member_responses {
                if response.change().is_some() {
                    return Err(Error::InvalidMintResponse(format!(
                        "member {} returned melt status change when the threshold set did not",
                        member_id
                    )));
                }
            }
            return Ok(first_response.clone());
        };

        if first_change.is_empty() {
            for (member_id, response) in member_responses {
                match response.change() {
                    Some(change) if change.is_empty() => (),
                    _ => {
                        return Err(Error::InvalidMintResponse(format!(
                            "member {} returned mismatched empty melt status change",
                            member_id
                        )))
                    }
                }
            }
            return Ok(first_response.clone());
        }

        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut aggregate_change = Vec::with_capacity(first_change.len());
        for (index, first_signature) in first_change.iter().enumerate() {
            let mut shares = Vec::with_capacity(member_responses.len());

            for (member_id, response) in member_responses {
                let change = response.change().ok_or_else(|| {
                    Error::InvalidMintResponse(format!(
                        "member {} did not return melt status change signatures",
                        member_id
                    ))
                })?;
                let signature = change.get(index).ok_or_else(|| {
                    Error::InvalidMintResponse(format!(
                        "member {} returned too few melt status change signatures",
                        member_id
                    ))
                })?;
                if signature.amount != first_signature.amount
                    || signature.keyset_id != first_signature.keyset_id
                    || signature.dleq.is_some()
                {
                    return Err(Error::InvalidMintResponse(format!(
                        "member {} returned mismatched melt status change metadata",
                        member_id
                    )));
                }

                shares.push(BlsBlindSignatureShare::new(
                    member_id.to_bls_signer_id(),
                    signature.c.as_bls_g1()?,
                ));
            }

            let aggregate_signature = aggregate_blind_signature_shares(threshold, &shares)?;
            aggregate_change.push(BlindSignature {
                amount: first_signature.amount,
                keyset_id: first_signature.keyset_id,
                c: aggregate_signature.into(),
                dleq: None,
            });
        }

        for (member_id, response) in member_responses {
            let change = response.change().ok_or_else(|| {
                Error::InvalidMintResponse(format!(
                    "member {} did not return melt status change signatures",
                    member_id
                ))
            })?;
            if change.len() != first_change.len() {
                return Err(Error::InvalidMintResponse(format!(
                    "member {} returned {} melt status change signatures for {} expected signatures",
                    member_id,
                    change.len(),
                    first_change.len()
                )));
            }
        }

        let mut response = first_response.clone();
        set_melt_response_change(&mut response, Some(aggregate_change));

        Ok(response)
    }

    fn aggregate_restore_response(
        &self,
        requested_outputs: &[BlindedMessage],
        member_responses: &[(MemberId, RestoreResponse)],
    ) -> Result<RestoreResponse, Error> {
        self.public_config
            .validate_blinded_restore_outputs(requested_outputs)?;

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
            ShareAmountValidation::Restore,
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
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut matching_views = Vec::<(CheckStateResponse, Vec<MemberId>)>::new();

        for (member_id, response) in member_responses {
            validate_check_state_response(request, response, *member_id)?;

            match matching_views
                .iter_mut()
                .find(|(candidate, _)| candidate.states == response.states)
            {
                Some((_, members)) => members.push(*member_id),
                None => matching_views.push((response.clone(), vec![*member_id])),
            }
        }

        matching_views
            .into_iter()
            .find_map(|(response, members)| {
                if members.len() >= threshold {
                    Some(response)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                Error::InvalidMintResponse(format!(
                    "insufficient matching federation check-state responses: required {}",
                    threshold
                ))
            })
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
        let responses = join_all(self.member_connectors.iter().map(|(member_id, connector)| {
            let member_id = *member_id;
            let connector = connector.clone();
            async move { (member_id, connector.get_mint_keyset(keyset_id).await) }
        }))
        .await;

        for (member_id, result) in responses {
            match result {
                Ok(advertised) => {
                    validate_member_advertised_keyset(&self.public_config, member_id, &advertised)?;
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
        let expected_count = request.quotes.len();
        let member_responses = self
            .collect_batch_mint_quote_status_responses(|connector| {
                let method = method.clone();
                let request = request.clone();
                async move {
                    connector
                        .post_batch_check_mint_quote_status(&method, request)
                        .await
                }
            })
            .await?;

        self.aggregate_batch_mint_quote_status_response(expected_count, &member_responses)
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
        let member_responses = self
            .collect_mint_quote_status_responses(|connector| {
                let method = method.clone();
                let quote_id = quote_id.to_string();
                async move { connector.get_mint_quote_status(method, &quote_id).await }
            })
            .await?;

        self.aggregate_mint_quote_status_response(&member_responses)
    }

    async fn get_melt_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        let member_responses = self
            .collect_melt_quote_status_responses(|connector| {
                let method = method.clone();
                let quote_id = quote_id.to_string();
                async move { connector.get_melt_quote_status(method, &quote_id).await }
            })
            .await?;

        self.aggregate_melt_quote_status_response(&member_responses)
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
            .validate_blinded_restore_outputs(&request.outputs)?;
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
        ShareAmountValidation::AllowZeroAmountPlaceholder | ShareAmountValidation::Restore => {
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
        generate_trusted_dealer_configs, FederatedKeyset, FederationConfig, FederationError,
        FederationJournalEntry, FederationKeysetRotationOperation, FederationOperation,
        FederationOperationEnvelope, FederationOperationId, InMemoryFederationJournal, MemberId,
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
        MintQuoteBolt11Response, PublicKey, QuoteId,
    };
    use cdk_common::{MintQuoteRequest, MintQuoteResponse, MintResponse, SecretKey};
    use cdk_fake_wallet::{create_fake_invoice, FakeWallet};
    use cdk_http_client::HttpError;

    use super::{FederatedMintConnector, FederationWalletConnector};
    use crate::dhke::verify_bls_blind_signature;
    use crate::mint::{FederationRuntimeConfig, Mint, MintBuilder, MintInput, MintMeltLimits};
    use crate::nuts::nut00::{KnownMethod, PaymentMethod};
    use crate::nuts::{
        AuthToken, BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest,
        CheckStateResponse, KeysetResponse, MeltQuoteState, MeltRequest, MintInfo,
        MintQuoteBolt11Request, MintQuoteState, MintRequest, ProofState, RestoreRequest,
        RestoreResponse, State, SwapRequest, SwapResponse,
    };
    use crate::types::{FeeReserve, QuoteTTL};
    use crate::wallet::mint_connector::transport::Transport;
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

    fn threshold_rotation_bundle() -> TrustedDealerConfigBundle {
        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("params"),
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![
                TrustedDealerKeysetSpec::new(
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
                ),
                TrustedDealerKeysetSpec::new(
                    CurrencyUnit::Sat,
                    false,
                    0,
                    None,
                    BTreeMap::from([
                        (
                            Amount::from(1),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[8; 32]),
                                BlsSecretKey::from_reduced_bytes(&[9; 32]),
                            ],
                        ),
                        (
                            Amount::from(2),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[10; 32]),
                                BlsSecretKey::from_reduced_bytes(&[11; 32]),
                            ],
                        ),
                        (
                            Amount::from(4),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[12; 32]),
                                BlsSecretKey::from_reduced_bytes(&[13; 32]),
                            ],
                        ),
                    ]),
                ),
            ],
        )
        .expect("trusted dealer rotation bundle")
    }

    fn active_keyset(bundle: &TrustedDealerConfigBundle) -> FederatedKeyset {
        bundle
            .public_config
            .keysets
            .iter()
            .find(|keyset| keyset.active)
            .expect("active keyset")
            .clone()
    }

    fn inactive_rotation_keyset(bundle: &TrustedDealerConfigBundle) -> FederatedKeyset {
        bundle
            .public_config
            .keysets
            .iter()
            .find(|keyset| !keyset.active)
            .expect("inactive rotation keyset")
            .clone()
    }

    fn activated_rotation_keyset(bundle: &TrustedDealerConfigBundle) -> FederatedKeyset {
        let mut keyset = inactive_rotation_keyset(bundle);
        keyset.active = true;
        keyset
    }

    fn rotated_public_config(bundle: &TrustedDealerConfigBundle) -> FederationConfig {
        bundle
            .public_config
            .apply_keyset_rotation(&FederationKeysetRotationOperation {
                keyset: activated_rotation_keyset(bundle),
            })
            .expect("rotated public config")
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
        blinded_outputs_for_keyset(keyset, b"federated wallet connector output")
    }

    fn blinded_outputs_for_keyset(keyset: &FederatedKeyset, label: &[u8]) -> Vec<BlindedMessage> {
        let first_label = [label, b" 1".as_slice()].concat();
        let second_label = [label, b" 2".as_slice()].concat();

        vec![
            BlindedMessage::new(
                Amount::from(1),
                keyset.id,
                PublicKey::from(BlsG1PublicKey::hash_to_curve(&first_label)),
            ),
            BlindedMessage::new(
                Amount::from(2),
                keyset.id,
                PublicKey::from(BlsG1PublicKey::hash_to_curve(&second_label)),
            ),
        ]
    }

    fn blinded_change_outputs_for_keyset(
        keyset: &FederatedKeyset,
        label: &[u8],
    ) -> Vec<BlindedMessage> {
        blinded_outputs_for_keyset(keyset, label)
            .into_iter()
            .map(|mut output| {
                output.amount = Amount::ZERO;
                output
            })
            .collect()
    }

    fn blinded_change_outputs(bundle: &TrustedDealerConfigBundle) -> Vec<BlindedMessage> {
        let keyset = &bundle.public_config.keysets[0];
        blinded_change_outputs_for_keyset(keyset, b"federated wallet connector change")
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

    fn mint_quote_status_response(state: MintQuoteState) -> MintQuoteBolt11Response<String> {
        mint_quote_status_response_for("mint-quote", state)
    }

    fn mint_quote_status_response_for(
        quote: &str,
        state: MintQuoteState,
    ) -> MintQuoteBolt11Response<String> {
        MintQuoteBolt11Response {
            quote: quote.to_string(),
            request: "bolt11-invoice".to_string(),
            amount: Some(Amount::from(3)),
            unit: Some(CurrencyUnit::Sat),
            state,
            expiry: Some(42),
            pubkey: None,
        }
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

        async fn effective_federation_config(
            &self,
        ) -> Result<FederationRuntimeConfig, crate::Error> {
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
            let config = self.effective_federation_config().await?;
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
            let config = self.effective_federation_config().await?;
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
            let config = self.effective_federation_config().await?;
            let operation_id = config.melt_operation(request.clone())?.operation_id()?;

            Ok(self
                .mint
                .melt_federation(&request, operation_id)
                .await?
                .into())
        }

        async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, crate::Error> {
            let config = self.effective_federation_config().await?;
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
                let keyset_id = Id::from_str(keyset_id)
                    .map_err(|err| HttpError::Serialization(err.to_string()))?;
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
    }

    fn http_member_connectors(
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

    fn appended_entry(
        journal: &mut InMemoryFederationJournal,
        envelope: FederationOperationEnvelope,
    ) -> FederationJournalEntry {
        journal.append(envelope).expect("append").entry().clone()
    }

    async fn persist_and_apply_entry(mint: &Mint, entry: &FederationJournalEntry) {
        let localstore = mint.localstore();
        let mut tx = localstore.begin_transaction().await.expect("begin");
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist journal entry");
        tx.commit().await.expect("commit journal entry");

        mint.apply_federation_journal_entries(std::slice::from_ref(entry))
            .await
            .expect("apply journal entry");
    }

    async fn add_paid_bolt11_quote(mint: &Mint, quote_id: &QuoteId, amount: u64) {
        let amount = Amount::new(amount, CurrencyUnit::Sat);
        let mut quote = MintQuote::new(
            Some(quote_id.clone()),
            format!("invoice-{quote_id}"),
            CurrencyUnit::Sat,
            Some(amount.clone()),
            unix_time() + 10_000,
            PaymentIdentifier::QuoteId(quote_id.clone()),
            None,
            Amount::new(0, CurrencyUnit::Sat),
            Amount::new(0, CurrencyUnit::Sat),
            PaymentMethod::Known(KnownMethod::Bolt11),
            unix_time(),
            Vec::new(),
            Vec::new(),
            None,
        );
        quote
            .add_payment(amount, format!("payment-{quote_id}"), None)
            .expect("add quote payment");
        let localstore = mint.localstore();
        let mut tx = localstore.begin_transaction().await.expect("begin tx");
        tx.add_mint_quote(quote).await.expect("add quote");
        tx.commit().await.expect("commit quote");
    }

    fn bolt11_quote_request(amount: u64) -> MintQuoteRequest {
        MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(amount),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        })
    }

    fn bolt11_quote_response(quote: &QuoteId, amount: u64) -> MintQuoteResponse<String> {
        MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: quote.to_string(),
            request: format!("invoice-{quote}"),
            amount: Some(Amount::from(amount)),
            unit: Some(CurrencyUnit::Sat),
            state: MintQuoteState::Unpaid,
            expiry: None,
            pubkey: None,
        })
    }

    async fn persist_paid_mint_quote(
        mint: &Mint,
        config: &FederationRuntimeConfig,
        journal: &mut InMemoryFederationJournal,
        quote_id: &QuoteId,
        amount: u64,
    ) {
        let quote_entry = appended_entry(
            journal,
            config.mint_quote_operation(
                quote_id.clone(),
                bolt11_quote_request(amount),
                bolt11_quote_response(quote_id, amount),
            ),
        );
        persist_and_apply_entry(mint, &quote_entry).await;

        let paid_entry = appended_entry(
            journal,
            config.mint_quote_payment_observation(
                quote_id.clone(),
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintQuoteState::Paid,
                Amount::from(amount),
                Amount::ZERO,
                Some(format!("payment-{quote_id}")),
            ),
        );
        persist_and_apply_entry(mint, &paid_entry).await;
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

    async fn rotated_fake_wallet_member_connectors_with_state(
        bundle: &TrustedDealerConfigBundle,
        old_mint_quote_id: &QuoteId,
        new_mint_quote_id: &QuoteId,
        old_restore_output: &BlindedMessage,
        melt_quote_id: &QuoteId,
        melt_invoice: &Bolt11Invoice,
        melt_amount: u64,
    ) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
        let mut connectors = BTreeMap::new();
        let melt_expiry = unix_time() + 10_000;
        let rotation_keyset = activated_rotation_keyset(bundle);

        for member_id in [member_id(1), member_id(2), member_id(3)] {
            let mint = create_fake_wallet_member_mint(bundle, member_id).await;
            let config = mint.federation_config().expect("federation config").clone();
            let mut journal = InMemoryFederationJournal::new();
            persist_paid_mint_quote(&mint, &config, &mut journal, old_mint_quote_id, 1).await;
            persist_paid_mint_quote(&mint, &config, &mut journal, new_mint_quote_id, 3).await;
            add_unpaid_bolt11_melt_quote(
                &mint,
                melt_quote_id,
                melt_invoice,
                melt_amount,
                melt_expiry,
            )
            .await;

            let old_mint_input = MintInput::Single(MintRequest {
                quote: old_mint_quote_id.clone(),
                outputs: vec![old_restore_output.clone()],
                signature: None,
            });
            let old_mint_entry = appended_entry(
                &mut journal,
                config
                    .mint_operation(old_mint_input)
                    .expect("old mint operation"),
            );
            persist_and_apply_entry(&mint, &old_mint_entry).await;

            let rotation_entry = appended_entry(
                &mut journal,
                config.operation_envelope(FederationOperation::KeysetRotation(
                    FederationKeysetRotationOperation {
                        keyset: rotation_keyset.clone(),
                    },
                )),
            );
            persist_and_apply_entry(&mint, &rotation_entry).await;

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
        assert_eq!(response.signatures.len(), outputs.len());
        for (signature, output) in response.signatures.iter().zip(outputs) {
            let keyset = bundle
                .public_config
                .keysets
                .iter()
                .find(|keyset| keyset.id == signature.keyset_id)
                .expect("signature keyset");
            let aggregate_key = keyset
                .keys()
                .amount_key(signature.amount)
                .expect("amount key");
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
    async fn federated_mint_connector_polls_member_keysets_concurrently() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let mut connectors = BTreeMap::new();

        for member_id in [member_id(1), member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_mint_keys_barrier(barrier.clone());
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response =
            tokio::time::timeout(std::time::Duration::from_secs(1), connector.get_mint_keys())
                .await
                .expect("member keyset probes should run concurrently")
                .expect("aggregate keysets");

        assert_eq!(response.len(), 1);
        assert_eq!(response[0].id, advertised.id);
    }

    #[tokio::test]
    async fn federated_mint_connector_fans_out_mint_members_concurrently() {
        let bundle = threshold_bundle();
        let outputs = blinded_outputs(&bundle);
        let advertised = advertised_keyset(&bundle);
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut connectors = BTreeMap::new();

        for member_id in [member_id(1), member_id(2)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_post_mint_barrier(barrier.clone());
            connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let failing_member = MockMintConnector::new();
        failing_member.set_active_keyset(advertised);
        failing_member
            .set_post_mint_response(Err(crate::Error::Custom("member unavailable".to_string())));
        connectors.insert(
            member_id(3),
            Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            connector.post_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MintRequest {
                    quote: "paid-quote".to_string(),
                    outputs: outputs.clone(),
                    signature: None,
                },
            ),
        )
        .await
        .expect("mint member requests should run concurrently")
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
    async fn federated_mint_connector_aggregates_rotated_fake_wallet_member_shares() {
        let bundle = threshold_rotation_bundle();
        let old_keyset = active_keyset(&bundle);
        let next_keyset = activated_rotation_keyset(&bundle);
        let old_mint_quote_id = QuoteId::new();
        let new_mint_quote_id = QuoteId::new();
        let melt_quote_id = QuoteId::new();
        let melt_invoice = create_fake_invoice(1000, "".to_string());
        let old_restore_output =
            blinded_outputs_for_keyset(&old_keyset, b"rotated wallet old restore")
                .into_iter()
                .next()
                .expect("old restore output");
        let next_outputs = blinded_outputs_for_keyset(&next_keyset, b"rotated wallet output");
        let next_change_outputs =
            vec![
                blinded_change_outputs_for_keyset(&next_keyset, b"rotated wallet melt change")
                    .into_iter()
                    .next()
                    .expect("next change output"),
            ];
        let connector = FederatedMintConnector::new(
            rotated_public_config(&bundle),
            rotated_fake_wallet_member_connectors_with_state(
                &bundle,
                &old_mint_quote_id,
                &new_mint_quote_id,
                &old_restore_output,
                &melt_quote_id,
                &melt_invoice,
                1,
            )
            .await,
        )
        .expect("rotated federated mint connector");

        let advertised = connector.get_mint_keys().await.expect("active keysets");
        assert_eq!(advertised.len(), 1);
        assert_eq!(advertised[0].id, next_keyset.id);

        let mint_response = connector
            .post_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MintRequest {
                    quote: new_mint_quote_id.to_string(),
                    outputs: next_outputs.clone(),
                    signature: None,
                },
            )
            .await
            .expect("rotated mint response");
        assert_aggregate_response_valid(&bundle, &next_outputs, &mint_response);

        let swap_response = connector
            .post_swap(SwapRequest::new(
                vec![proof_for_amount(&bundle, 4, "rotated-wallet-swap")],
                next_outputs.clone(),
            ))
            .await
            .expect("rotated swap response");
        assert_aggregate_response_valid(
            &bundle,
            &next_outputs,
            &MintResponse {
                signatures: swap_response.signatures,
            },
        );

        let melt_response = connector
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MeltRequest::new(
                    melt_quote_id.to_string(),
                    vec![proof_for_amount(&bundle, 4, "rotated-wallet-melt")],
                    Some(next_change_outputs.clone()),
                ),
            )
            .await
            .expect("rotated melt response");
        let change = melt_response.change().cloned().expect("change signatures");
        let mut signed_change_output = next_change_outputs[0].clone();
        signed_change_output.amount = Amount::from(2);
        assert_eq!(melt_response.state(), MeltQuoteState::Paid);
        assert_aggregate_response_valid(
            &bundle,
            &[signed_change_output],
            &MintResponse { signatures: change },
        );

        let restore_response = connector
            .post_restore(RestoreRequest {
                outputs: vec![old_restore_output.clone()],
            })
            .await
            .expect("rotated restore response");
        assert_eq!(restore_response.outputs, vec![old_restore_output.clone()]);
        assert_aggregate_response_valid(
            &bundle,
            &[old_restore_output],
            &MintResponse {
                signatures: restore_response.signatures,
            },
        );
    }

    #[tokio::test]
    async fn federated_mint_connector_aggregates_rotated_http_member_shares() {
        let bundle = threshold_rotation_bundle();
        let old_keyset = active_keyset(&bundle);
        let next_keyset = activated_rotation_keyset(&bundle);
        let old_mint_quote_id = QuoteId::new();
        let new_mint_quote_id = QuoteId::new();
        let melt_quote_id = QuoteId::new();
        let melt_invoice = create_fake_invoice(1000, "".to_string());
        let old_restore_output =
            blinded_outputs_for_keyset(&old_keyset, b"rotated http old restore")
                .into_iter()
                .next()
                .expect("old restore output");
        let next_outputs = blinded_outputs_for_keyset(&next_keyset, b"rotated http output");
        let next_change_outputs =
            vec![
                blinded_change_outputs_for_keyset(&next_keyset, b"rotated http melt change")
                    .into_iter()
                    .next()
                    .expect("next change output"),
            ];
        let local_connectors = rotated_fake_wallet_member_connectors_with_state(
            &bundle,
            &old_mint_quote_id,
            &new_mint_quote_id,
            &old_restore_output,
            &melt_quote_id,
            &melt_invoice,
            1,
        )
        .await;
        let connector = FederatedMintConnector::new(
            rotated_public_config(&bundle),
            http_member_connectors(local_connectors),
        )
        .expect("rotated federated HTTP mint connector");

        let advertised = connector
            .get_mint_keyset(next_keyset.id)
            .await
            .expect("next keyset over HTTP");
        assert_eq!(advertised.id, next_keyset.id);

        let mint_response = connector
            .post_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MintRequest {
                    quote: new_mint_quote_id.to_string(),
                    outputs: next_outputs.clone(),
                    signature: None,
                },
            )
            .await
            .expect("rotated HTTP mint response");
        assert_aggregate_response_valid(&bundle, &next_outputs, &mint_response);

        let swap_response = connector
            .post_swap(SwapRequest::new(
                vec![proof_for_amount(&bundle, 4, "rotated-http-swap")],
                next_outputs.clone(),
            ))
            .await
            .expect("rotated HTTP swap response");
        assert_aggregate_response_valid(
            &bundle,
            &next_outputs,
            &MintResponse {
                signatures: swap_response.signatures,
            },
        );

        let melt_proof = proof_for_amount(&bundle, 4, "rotated-http-melt");
        let melt_y = melt_proof.y().expect("melt proof y");
        let melt_response = connector
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MeltRequest::new(
                    melt_quote_id.to_string(),
                    vec![melt_proof],
                    Some(next_change_outputs.clone()),
                ),
            )
            .await
            .expect("rotated HTTP melt response");
        let change = melt_response.change().cloned().expect("change signatures");
        let mut signed_change_output = next_change_outputs[0].clone();
        signed_change_output.amount = Amount::from(2);
        assert_eq!(melt_response.state(), MeltQuoteState::Paid);
        assert_aggregate_response_valid(
            &bundle,
            &[signed_change_output],
            &MintResponse { signatures: change },
        );
        let melt_status = connector
            .get_melt_quote_status(
                PaymentMethod::Known(KnownMethod::Bolt11),
                &melt_quote_id.to_string(),
            )
            .await
            .expect("rotated HTTP melt status");
        assert_eq!(melt_status.state(), MeltQuoteState::Paid);
        let spent_state = connector
            .post_check_state(CheckStateRequest { ys: vec![melt_y] })
            .await
            .expect("rotated HTTP check-state");
        assert_eq!(spent_state.states.len(), 1);
        assert_eq!(spent_state.states[0].state, State::Spent);

        let restore_response = connector
            .post_restore(RestoreRequest {
                outputs: vec![old_restore_output.clone()],
            })
            .await
            .expect("rotated HTTP restore response");
        assert_eq!(restore_response.outputs, vec![old_restore_output.clone()]);
        assert_aggregate_response_valid(
            &bundle,
            &[old_restore_output],
            &MintResponse {
                signatures: restore_response.signatures,
            },
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
    async fn federated_mint_connector_uses_threshold_mint_quote_status() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let status = mint_quote_status_response(MintQuoteState::Paid);
        let mut connectors = BTreeMap::new();

        let failing_member = MockMintConnector::new();
        failing_member.set_active_keyset(advertised.clone());
        failing_member.set_mint_quote_status_response(Err(crate::Error::Custom(
            "primary unavailable".to_string(),
        )));
        connectors.insert(
            member_id(1),
            Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        for member_id in [member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_mint_quote_status_response(Ok(status.clone()));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "mint-quote")
            .await
            .expect("mint quote status");

        assert_eq!(response.state(), Some(MintQuoteState::Paid));
    }

    #[tokio::test]
    async fn federated_mint_connector_falls_back_to_primary_mint_quote_status() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let mut connectors = BTreeMap::new();

        let primary_member = MockMintConnector::new();
        primary_member.set_active_keyset(advertised.clone());
        primary_member
            .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Paid)));
        connectors.insert(
            member_id(1),
            Arc::new(primary_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        for member_id in [member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_mint_quote_status_response(Err(crate::Error::UnknownQuote));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "mint-quote")
            .await
            .expect("mint quote status");

        assert_eq!(response.state(), Some(MintQuoteState::Paid));
    }

    #[tokio::test]
    async fn federated_mint_connector_uses_threshold_batch_mint_quote_status() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let statuses = vec![
            MintQuoteResponse::Bolt11(mint_quote_status_response_for(
                "mint-quote-1",
                MintQuoteState::Paid,
            )),
            MintQuoteResponse::Bolt11(mint_quote_status_response_for(
                "mint-quote-2",
                MintQuoteState::Unpaid,
            )),
        ];
        let mut connectors = BTreeMap::new();

        let failing_member = MockMintConnector::new();
        failing_member.set_active_keyset(advertised.clone());
        failing_member.set_batch_mint_quote_status_response(Err(crate::Error::Custom(
            "primary unavailable".to_string(),
        )));
        connectors.insert(
            member_id(1),
            Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        for member_id in [member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_batch_mint_quote_status_response(Ok(statuses.clone()));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .post_batch_check_mint_quote_status(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                BatchCheckMintQuoteRequest {
                    quotes: vec!["mint-quote-1".to_string(), "mint-quote-2".to_string()],
                },
            )
            .await
            .expect("batch mint quote status");

        assert_eq!(response[0].state(), Some(MintQuoteState::Paid));
        assert_eq!(response[1].state(), Some(MintQuoteState::Unpaid));
    }

    #[tokio::test]
    async fn federated_mint_connector_falls_back_to_primary_batch_mint_quote_status() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let statuses = vec![
            MintQuoteResponse::Bolt11(mint_quote_status_response_for(
                "mint-quote-1",
                MintQuoteState::Paid,
            )),
            MintQuoteResponse::Bolt11(mint_quote_status_response_for(
                "mint-quote-2",
                MintQuoteState::Unpaid,
            )),
        ];
        let mut connectors = BTreeMap::new();

        let primary_member = MockMintConnector::new();
        primary_member.set_active_keyset(advertised.clone());
        primary_member.set_batch_mint_quote_status_response(Ok(statuses));
        connectors.insert(
            member_id(1),
            Arc::new(primary_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        for member_id in [member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_batch_mint_quote_status_response(Err(crate::Error::UnknownQuote));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .post_batch_check_mint_quote_status(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                BatchCheckMintQuoteRequest {
                    quotes: vec!["mint-quote-1".to_string(), "mint-quote-2".to_string()],
                },
            )
            .await
            .expect("batch mint quote status");

        assert_eq!(response[0].state(), Some(MintQuoteState::Paid));
        assert_eq!(response[1].state(), Some(MintQuoteState::Unpaid));
    }

    #[tokio::test]
    async fn federated_mint_connector_uses_threshold_melt_quote_status() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let status = match melt_response_without_change() {
            MeltQuoteResponse::Bolt11(response) => response,
            _ => panic!("expected bolt11 melt response"),
        };
        let mut connectors = BTreeMap::new();

        let failing_member = MockMintConnector::new();
        failing_member.set_active_keyset(advertised.clone());
        failing_member.set_melt_quote_status_response(Err(crate::Error::Custom(
            "primary unavailable".to_string(),
        )));
        connectors.insert(
            member_id(1),
            Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        for member_id in [member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_melt_quote_status_response(Ok(status.clone()));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
            .await
            .expect("melt quote status");

        assert_eq!(response.state(), MeltQuoteState::Paid);
        assert!(response.change().is_none());
    }

    #[tokio::test]
    async fn federated_mint_connector_aggregates_melt_quote_status_change_shares() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let change_outputs = blinded_change_outputs(&bundle);
        let connectors = [member_id(1), member_id(2), member_id(3)]
            .into_iter()
            .map(|member_id| {
                let connector = MockMintConnector::new();
                connector.set_active_keyset(advertised.clone());
                let status = match melt_response(&bundle, member_id, &change_outputs) {
                    MeltQuoteResponse::Bolt11(response) => response,
                    _ => panic!("expected bolt11 melt response"),
                };
                connector.set_melt_quote_status_response(Ok(status));
                (
                    member_id,
                    Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
                )
            })
            .collect();

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
            .await
            .expect("melt quote status");

        let change = response.change().cloned().expect("change signatures");
        assert_eq!(response.state(), MeltQuoteState::Paid);
        assert_aggregate_response_valid(
            &bundle,
            &signed_change_outputs(&change_outputs),
            &MintResponse { signatures: change },
        );
    }

    #[tokio::test]
    async fn federated_mint_connector_falls_back_to_primary_melt_quote_status() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let status = match melt_response_without_change() {
            MeltQuoteResponse::Bolt11(response) => response,
            _ => panic!("expected bolt11 melt response"),
        };
        let mut connectors = BTreeMap::new();

        let primary_member = MockMintConnector::new();
        primary_member.set_active_keyset(advertised.clone());
        primary_member.set_melt_quote_status_response(Ok(status));
        connectors.insert(
            member_id(1),
            Arc::new(primary_member) as Arc<dyn MintConnector + Send + Sync>,
        );

        for member_id in [member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_melt_quote_status_response(Err(crate::Error::UnknownQuote));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = connector
            .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
            .await
            .expect("melt quote status");

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
    async fn federated_mint_connector_accepts_threshold_matching_check_state_views() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let ys = vec![
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-lag-y-1")),
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-lag-y-2")),
        ];
        let stale = proof_state_response(&ys, &[State::Unspent, State::Pending]);
        let expected = proof_state_response(&ys, &[State::Spent, State::Pending]);
        let mut connectors = BTreeMap::new();

        for (member_id, response) in [
            (member_id(1), expected.clone()),
            (member_id(2), stale),
            (member_id(3), expected.clone()),
        ] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_check_state_response(Ok(response));
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
    async fn federated_mint_connector_polls_check_state_members_concurrently() {
        let bundle = threshold_bundle();
        let advertised = advertised_keyset(&bundle);
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let ys = vec![
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-parallel-y-1")),
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-parallel-y-2")),
        ];
        let expected = proof_state_response(&ys, &[State::Unspent, State::Spent]);
        let mut connectors = BTreeMap::new();

        for member_id in [member_id(1), member_id(2), member_id(3)] {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            connector.set_check_state_barrier(barrier.clone());
            connector.set_check_state_response(Ok(expected.clone()));
            connectors.insert(
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            );
        }

        let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
            .expect("federated mint connector");
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            connector.post_check_state(CheckStateRequest { ys }),
        )
        .await
        .expect("check-state probes should run concurrently")
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
            &[State::Unspent, State::Spent],
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
