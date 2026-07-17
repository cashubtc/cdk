//! Wallet-side federation validation and aggregation helpers.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdk_common::error::ErrorResponse;
use cdk_common::federation::{
    FederationConfig, FederationConfigDigest, FederationError, FederationInvite, MemberId,
};
use cdk_common::{
    MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse, MintQuoteRequest,
    MintQuoteResponse, QuoteId,
};
use futures::future::join_all;
use futures::stream::{FuturesUnordered, StreamExt};
use futures::FutureExt;

use super::{AuthWallet, HttpClient, MintConnector};
use crate::nuts::nut01::{
    aggregate_blind_signature_shares, batch_verify_blind_signature_shares,
    verify_blind_signature_share, BlsBlindSignatureShare,
};
use crate::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, BlindSignature, BlindedMessage,
    CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeySet, KeySetInfo, KeysetResponse,
    MeltQuoteState, MeltRequest, MintInfo, MintRequest, MintResponse, PaymentMethod,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use crate::{Amount, Error};

mod validation;

pub use self::validation::FederationWalletConnector;
use self::validation::{
    federated_keyset_to_keyset, federated_keyset_to_keyset_info, validate_member_active_keysets,
    validate_member_advertised_keyset,
};

const FEDERATION_CATCH_UP_MAX_RETRIES: u16 = 100;
const FEDERATION_CATCH_UP_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Wallet mint connector that fans out signing requests to federation members.
///
/// The connector preserves the existing wallet-to-mint protocol surface: member
/// mints return ordinary Cashu responses whose `C_` values are threshold BLS
/// signature shares, and the connector verifies and aggregates those shares
/// before returning a normal aggregate Cashu response to the wallet.
#[derive(Clone)]
pub struct FederatedMintConnector {
    public_config: FederationConfig,
    config_digest: FederationConfigDigest,
    member_connectors: BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    quote_coordinator_member_id: MemberId,
    config: FederatedMintConnectorConfig,
    valid_member_connectors_cache:
        Arc<tokio::sync::RwLock<Option<BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>>>>,
}

/// Runtime policy for wallet-side federation member calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederatedMintConnectorConfig {
    /// Preferred member for the first quote-creation attempt.
    pub quote_coordinator_member_id: Option<MemberId>,
    /// Timeout for sequential first-healthy-member calls such as quote creation.
    pub member_fallback_timeout: Duration,
    /// Per-attempt timeout for fanout calls to individual federation members.
    pub member_request_timeout: Duration,
    /// Additional retry attempts for retryable member fanout failures.
    pub member_request_max_retries: u16,
    /// Per-member timeout for quote-status polling calls.
    pub quote_status_member_timeout: Duration,
    /// Maximum number of concurrent member calls started by fanout collectors.
    pub fanout_concurrency: usize,
    /// Number of attempts to wait for a newly-created mint quote to become threshold-visible.
    pub mint_quote_visibility_attempts: u16,
    /// Delay between mint quote threshold-visibility attempts.
    pub mint_quote_visibility_retry_delay: Duration,
}

impl Default for FederatedMintConnectorConfig {
    fn default() -> Self {
        Self {
            quote_coordinator_member_id: None,
            member_fallback_timeout: Duration::from_secs(10),
            member_request_timeout: Duration::from_secs(10),
            member_request_max_retries: 0,
            quote_status_member_timeout: Duration::from_secs(10),
            fanout_concurrency: usize::MAX,
            mint_quote_visibility_attempts: 100,
            mint_quote_visibility_retry_delay: Duration::from_millis(50),
        }
    }
}

impl FederatedMintConnectorConfig {
    /// Validate connector policy values.
    pub fn validate(&self) -> Result<(), Error> {
        if self.member_fallback_timeout.is_zero() {
            return Err(Error::InvalidMintResponse(
                "federation member fallback timeout must be greater than zero".to_string(),
            ));
        }
        if self.member_request_timeout.is_zero() {
            return Err(Error::InvalidMintResponse(
                "federation member request timeout must be greater than zero".to_string(),
            ));
        }
        if self.quote_status_member_timeout.is_zero() {
            return Err(Error::InvalidMintResponse(
                "federation quote-status member timeout must be greater than zero".to_string(),
            ));
        }
        if self.fanout_concurrency == 0 {
            return Err(Error::InvalidMintResponse(
                "federation fanout concurrency must be greater than zero".to_string(),
            ));
        }
        if self.mint_quote_visibility_attempts == 0 {
            return Err(Error::InvalidMintResponse(
                "federation mint quote visibility attempts must be greater than zero".to_string(),
            ));
        }
        if self.mint_quote_visibility_retry_delay.is_zero() {
            return Err(Error::InvalidMintResponse(
                "federation mint quote visibility retry delay must be greater than zero"
                    .to_string(),
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShareAmountValidation {
    Exact,
    AllowZeroAmountPlaceholder,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MeltChangeSignatureView {
    amount: Amount,
    keyset_id: Id,
    has_dleq: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MeltQuoteStatusView {
    method: PaymentMethod,
    quote: String,
    amount: Amount,
    fee_reserve: Amount,
    state: MeltQuoteState,
    payment_proof: Option<String>,
    request: Option<String>,
    unit: Option<CurrencyUnit>,
    change: Option<Vec<MeltChangeSignatureView>>,
}

#[derive(Debug)]
struct FederationMemberFailure {
    member_id: MemberId,
    error: Error,
    rejection: Option<ErrorResponse>,
}

fn into_wallet_error(error: Error) -> Error {
    match error {
        Error::MintErrorResponse(response) => Error::from(response),
        error => error,
    }
}

impl FederationMemberFailure {
    fn new(member_id: MemberId, error: Error) -> Self {
        if let Error::MintErrorResponse(rejection) = error {
            let error = Error::from(rejection.clone());
            let rejection = error.is_definitive_failure().then_some(rejection);
            return Self {
                member_id,
                error,
                rejection,
            };
        }

        if error.is_definitive_failure() {
            let rejection = ErrorResponse::from(error);
            return Self {
                member_id,
                error: Error::from(rejection.clone()),
                rejection: Some(rejection),
            };
        }

        Self {
            member_id,
            error,
            rejection: None,
        }
    }
}

#[derive(Debug)]
struct FederationMemberResults<R> {
    responses: Vec<(MemberId, R)>,
    failures: Vec<FederationMemberFailure>,
}

impl<R> FederationMemberResults<R> {
    fn failure_summary(&self) -> String {
        self.failures
            .iter()
            .map(|failure| format!("member {}: {}", failure.member_id, failure.error))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn threshold_rejection(&self, threshold: usize) -> Option<ErrorResponse> {
        threshold_rejection(&self.failures, threshold).cloned()
    }

    fn catch_up_error(&self) -> Option<Error> {
        (!self.failures.is_empty()
            && self
                .failures
                .iter()
                .all(|failure| failure.error.is_federation_catch_up()))
        .then(|| Error::FederationCatchUp {
            detail: format!(
                "federation members did not finish catching up before the wallet retry deadline: {}",
                self.failure_summary()
            ),
        })
    }
}

fn threshold_rejection(
    failures: &[FederationMemberFailure],
    threshold: usize,
) -> Option<&ErrorResponse> {
    failures.iter().find_map(|candidate| {
        let rejection = candidate.rejection.as_ref()?;
        (failures
            .iter()
            .filter(|failure| failure.rejection.as_ref() == Some(rejection))
            .count()
            >= threshold)
            .then_some(rejection)
    })
}

impl fmt::Debug for FederatedMintConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FederatedMintConnector")
            .field("federation_id", &self.public_config.federation_id)
            .field("member_ids", &self.member_connectors.keys())
            .field(
                "quote_coordinator_member_id",
                &self.quote_coordinator_member_id,
            )
            .finish_non_exhaustive()
    }
}

impl FederatedMintConnector {
    /// Create a wallet connector for a static federation roster.
    pub fn new(
        public_config: FederationConfig,
        member_connectors: BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    ) -> Result<Self, Error> {
        Self::new_with_config(
            public_config,
            member_connectors,
            FederatedMintConnectorConfig::default(),
        )
    }

    /// Create a wallet connector from a federation invite code.
    ///
    /// The invite carries the public federation config. This constructor creates
    /// a normal HTTP connector for every member's public Cashu mint URL.
    pub fn from_invite_code(invite_code: &str) -> Result<Self, Error> {
        Self::from_invite_code_with_config(invite_code, FederatedMintConnectorConfig::default())
    }

    /// Create a wallet connector from a federation invite code and call policy.
    pub fn from_invite_code_with_config(
        invite_code: &str,
        config: FederatedMintConnectorConfig,
    ) -> Result<Self, Error> {
        let invite = FederationInvite::from_str(invite_code)?;
        Self::from_public_config_urls_with_config(invite.config, config)
    }

    /// Create a wallet connector from public member URLs in a federation config.
    pub fn from_public_config_urls(public_config: FederationConfig) -> Result<Self, Error> {
        Self::from_public_config_urls_with_config(
            public_config,
            FederatedMintConnectorConfig::default(),
        )
    }

    /// Create a wallet connector from public member URLs and an explicit policy.
    pub fn from_public_config_urls_with_config(
        public_config: FederationConfig,
        config: FederatedMintConnectorConfig,
    ) -> Result<Self, Error> {
        public_config.validate()?;
        Self::from_validated_public_config_urls_with_config(public_config, config)
    }

    /// Create a wallet connector from member URLs in an already-validated config.
    ///
    /// This is intended for trusted local registries that fully validated the
    /// federation config when it was imported. New external configs should use
    /// [`Self::from_public_config_urls_with_config`] so threshold aggregate keys
    /// and setup proofs are checked before the connector is constructed.
    pub fn from_validated_public_config_urls_with_config(
        public_config: FederationConfig,
        config: FederatedMintConnectorConfig,
    ) -> Result<Self, Error> {
        let member_connectors = public_config
            .members
            .iter()
            .map(|(member_id, member)| {
                (
                    *member_id,
                    Arc::new(
                        HttpClient::new(member.public_mint_url.clone(), None)
                            .with_preserved_error_responses(),
                    ) as Arc<dyn MintConnector + Send + Sync>,
                )
            })
            .collect();
        Self::new_with_validated_config(public_config, member_connectors, config)
    }

    /// Create a wallet connector with an explicit member-call policy.
    pub fn new_with_config(
        public_config: FederationConfig,
        member_connectors: BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
        config: FederatedMintConnectorConfig,
    ) -> Result<Self, Error> {
        public_config.validate()?;
        Self::new_with_validated_config(public_config, member_connectors, config)
    }

    /// Create a wallet connector with an already-validated federation config.
    ///
    /// This path avoids repeating expensive threshold aggregate-key validation
    /// after a config has been fully checked at import time. New external
    /// configs should use [`Self::new_with_config`].
    pub fn new_with_validated_config(
        public_config: FederationConfig,
        member_connectors: BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
        config: FederatedMintConnectorConfig,
    ) -> Result<Self, Error> {
        public_config.validate_wallet_runtime_metadata()?;
        let config_digest = public_config.config_digest_for_validated_config()?;
        config.validate()?;

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

        let default_quote_coordinator_member_id = public_config
            .members
            .keys()
            .next()
            .copied()
            .ok_or_else(|| Error::InvalidMintResponse("empty federation roster".to_string()))?;
        let quote_coordinator_member_id = match config.quote_coordinator_member_id {
            Some(quote_coordinator_member_id) => {
                if !public_config
                    .members
                    .contains_key(&quote_coordinator_member_id)
                {
                    return Err(FederationError::UnknownMember {
                        member_id: quote_coordinator_member_id,
                    }
                    .into());
                }
                quote_coordinator_member_id
            }
            None => default_quote_coordinator_member_id,
        };

        Ok(Self {
            public_config,
            config_digest,
            member_connectors,
            quote_coordinator_member_id,
            config,
            valid_member_connectors_cache: Arc::new(tokio::sync::RwLock::new(None)),
        })
    }

    /// Return the public federation config used by this connector.
    #[must_use]
    pub fn public_config(&self) -> &FederationConfig {
        &self.public_config
    }

    /// Return the member-call policy used by this connector.
    #[must_use]
    pub fn connector_config(&self) -> FederatedMintConnectorConfig {
        self.config
    }

    fn effective_fanout_concurrency(&self) -> usize {
        self.config
            .fanout_concurrency
            .min(self.member_connectors.len())
            .max(1)
    }

    #[cfg(feature = "prometheus")]
    fn record_aggregation_failure(&self, operation: &str) {
        cdk_prometheus::METRICS.record_federation_aggregation_failure("wallet", operation);
    }

    async fn call_first_healthy_member<R, F, Fut>(
        &self,
        operation: &str,
        call_member: F,
    ) -> Result<R, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: std::future::Future<Output = Result<R, Error>>,
    {
        self.call_first_healthy_member_from(&self.member_connectors, operation, call_member)
            .await
    }

    async fn call_first_healthy_member_from<R, F, Fut>(
        &self,
        member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
        operation: &str,
        mut call_member: F,
    ) -> Result<R, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: std::future::Future<Output = Result<R, Error>>,
    {
        let mut failures = Vec::new();

        for (member_id, connector) in member_connectors {
            match tokio::time::timeout(
                self.config.member_fallback_timeout,
                call_member(connector.clone()),
            )
            .await
            {
                Err(_) => failures.push(format!(
                    "member {}: timed out after {}s",
                    member_id,
                    self.config.member_fallback_timeout.as_secs()
                )),
                Ok(result) => match result {
                    Ok(response) => return Ok(response),
                    Err(err) if err.is_definitive_failure() => {
                        return Err(into_wallet_error(err));
                    }
                    Err(err) => failures.push(format!("member {}: {}", member_id, err)),
                },
            }
        }

        Err(Error::InvalidMintResponse(format!(
            "no healthy federation member accepted {}: {}",
            operation,
            failures.join("; ")
        )))
    }

    fn ordered_quote_creation_connectors(
        &self,
        member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    ) -> Vec<(MemberId, Arc<dyn MintConnector + Send + Sync>)> {
        let mut ordered_connectors = Vec::with_capacity(member_connectors.len());
        if let Some(connector) = member_connectors.get(&self.quote_coordinator_member_id) {
            ordered_connectors.push((self.quote_coordinator_member_id, connector.clone()));
        }
        for (member_id, connector) in member_connectors {
            if *member_id != self.quote_coordinator_member_id {
                ordered_connectors.push((*member_id, connector.clone()));
            }
        }
        ordered_connectors
    }

    async fn collect_member_results<R, F, Fut, Stop>(
        &self,
        call_member: F,
        per_member_timeout: Option<Duration>,
        max_member_retries: u16,
        timeout_operation: &'static str,
        stop_after: Stop,
    ) -> FederationMemberResults<R>
    where
        F: FnMut(MemberId, Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: Future<Output = Result<R, Error>>,
        Stop: FnMut(&[(MemberId, R)], &[FederationMemberFailure]) -> bool,
    {
        self.collect_member_results_from(
            &self.member_connectors,
            call_member,
            per_member_timeout,
            max_member_retries,
            timeout_operation,
            stop_after,
        )
        .await
    }

    async fn collect_member_results_from<R, F, Fut, Stop>(
        &self,
        member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
        mut call_member: F,
        per_member_timeout: Option<Duration>,
        max_member_retries: u16,
        timeout_operation: &'static str,
        mut stop_after: Stop,
    ) -> FederationMemberResults<R>
    where
        F: FnMut(MemberId, Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: Future<Output = Result<R, Error>>,
        Stop: FnMut(&[(MemberId, R)], &[FederationMemberFailure]) -> bool,
    {
        let mut responses = Vec::new();
        let mut failures = Vec::new();
        let mut members = member_connectors.iter();
        let mut pending = FuturesUnordered::new();
        let concurrency = self
            .config
            .fanout_concurrency
            .min(member_connectors.len())
            .max(1);
        let start_call = |member_id: MemberId,
                          attempt: u16,
                          retry_delay: Option<Duration>,
                          fut: Fut| async move {
            if let Some(retry_delay) = retry_delay {
                tokio::time::sleep(retry_delay).await;
            }
            let result = match per_member_timeout {
                Some(timeout) => match tokio::time::timeout(timeout, fut).await {
                    Ok(result) => result,
                    Err(_) => Err(Error::InvalidMintResponse(format!(
                        "member {} {} timed out after {}s",
                        member_id,
                        timeout_operation,
                        timeout.as_secs()
                    ))),
                },
                None => fut.await,
            };
            (member_id, attempt, result)
        };

        while pending.len() < concurrency {
            let Some((member_id, connector)) = members.next() else {
                break;
            };
            pending.push(start_call(
                *member_id,
                0,
                None,
                call_member(*member_id, connector.clone()),
            ));
        }

        while let Some((member_id, attempt, result)) = pending.next().await {
            match result {
                Ok(response) => responses.push((member_id, response)),
                Err(err) => {
                    let catch_up_retry = err.is_federation_catch_up();
                    let retry_limit = if catch_up_retry {
                        max_member_retries.max(FEDERATION_CATCH_UP_MAX_RETRIES)
                    } else {
                        max_member_retries
                    };
                    if !err.is_definitive_failure() && attempt < retry_limit {
                        if let Some(connector) = member_connectors.get(&member_id).cloned() {
                            pending.push(start_call(
                                member_id,
                                attempt.saturating_add(1),
                                catch_up_retry.then_some(FEDERATION_CATCH_UP_RETRY_DELAY),
                                call_member(member_id, connector),
                            ));
                            continue;
                        }
                    }

                    failures.push(FederationMemberFailure::new(member_id, err));
                }
            }

            while pending.len() < concurrency {
                let Some((member_id, connector)) = members.next() else {
                    break;
                };
                pending.push(start_call(
                    *member_id,
                    0,
                    None,
                    call_member(*member_id, connector.clone()),
                ));
            }

            if members.len() == 0 && stop_after(&responses, &failures) {
                // Poll every already-scheduled member once before returning. This starts
                // the full default fanout without making threshold liveness wait for a
                // slow minority.
                while let Some((member_id, _, result)) = pending.next().now_or_never().flatten() {
                    match result {
                        Ok(response) => responses.push((member_id, response)),
                        Err(err) => failures.push(FederationMemberFailure::new(member_id, err)),
                    }
                }
                return FederationMemberResults {
                    responses,
                    failures,
                };
            }
        }

        FederationMemberResults {
            responses,
            failures,
        }
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

    async fn validate_member_active_keysets(
        &self,
    ) -> Result<BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>, Error> {
        Ok(self
            .validate_member_active_keysets_with_mint_info()
            .await?
            .0)
    }

    async fn validate_member_active_keysets_with_mint_info(
        &self,
    ) -> Result<
        (
            BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
            Option<MintInfo>,
        ),
        Error,
    > {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        if let Some(cached) = self.valid_member_connectors_cache.read().await.clone() {
            if cached.len() >= threshold {
                return Ok((cached, None));
            }
        }

        let mut valid_member_connectors = BTreeMap::new();
        let mut valid_members = 0usize;
        let mut first_valid_mint_info = None;
        let mut failures = Vec::new();
        let mut members = self.member_connectors.iter();
        let mut pending = FuturesUnordered::new();
        let concurrency = self.effective_fanout_concurrency();
        let start_call = |member_id: MemberId, connector: Arc<dyn MintConnector + Send + Sync>| async move {
            let result = async {
                let mint_info = connector.get_mint_info().await?;
                let keysets = connector.get_mint_keys().await?;
                Ok::<_, Error>((mint_info, keysets))
            }
            .await;
            (member_id, result)
        };

        while pending.len() < concurrency {
            let Some((member_id, connector)) = members.next() else {
                break;
            };
            pending.push(start_call(*member_id, connector.clone()));
        }

        while let Some((member_id, result)) = pending.next().await {
            match result {
                Ok((mint_info, advertised)) => {
                    match FederationWalletConnector::validate_member_mint_info_with_config_digest(
                        &self.public_config,
                        member_id,
                        &mint_info,
                        self.config_digest,
                    )
                    .and_then(|_| {
                        validate_member_active_keysets(&self.public_config, member_id, &advertised)
                    }) {
                        Ok(()) => {
                            if let Some(connector) = self.member_connectors.get(&member_id) {
                                valid_member_connectors.insert(member_id, connector.clone());
                            }
                            if first_valid_mint_info.is_none() {
                                first_valid_mint_info = Some(mint_info);
                            }
                            valid_members = valid_members.saturating_add(1);
                        }
                        Err(err) => failures.push(format!("member {}: {}", member_id, err)),
                    }
                }
                Err(err) => failures.push(format!("member {}: {}", member_id, err)),
            }

            if valid_members >= threshold {
                while let Some((member_id, result)) = pending.next().now_or_never().flatten() {
                    match result {
                        Ok((mint_info, advertised)) => {
                            match FederationWalletConnector::validate_member_mint_info_with_config_digest(
                                &self.public_config,
                                member_id,
                                &mint_info,
                                self.config_digest,
                            )
                            .and_then(|_| {
                                validate_member_active_keysets(
                                    &self.public_config,
                                    member_id,
                                    &advertised,
                                )
                            }) {
                                Ok(()) => {
                                    if let Some(connector) = self.member_connectors.get(&member_id)
                                    {
                                        valid_member_connectors
                                            .insert(member_id, connector.clone());
                                    }
                                    if first_valid_mint_info.is_none() {
                                        first_valid_mint_info = Some(mint_info);
                                    }
                                }
                                Err(err) => {
                                    failures.push(format!("member {}: {}", member_id, err));
                                }
                            }
                        }
                        Err(err) => failures.push(format!("member {}: {}", member_id, err)),
                    }
                }
                *self.valid_member_connectors_cache.write().await =
                    Some(valid_member_connectors.clone());
                return Ok((valid_member_connectors, first_valid_mint_info));
            }

            while pending.len() < concurrency {
                let Some((member_id, connector)) = members.next() else {
                    break;
                };
                pending.push(start_call(*member_id, connector.clone()));
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

        *self.valid_member_connectors_cache.write().await = Some(valid_member_connectors.clone());
        Ok((valid_member_connectors, first_valid_mint_info))
    }

    async fn collect_mint_responses<F, Fut>(
        &self,
        member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
        outputs: &[BlindedMessage],
        amount_validation: ShareAmountValidation,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, MintResponse)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: Future<Output = Result<MintResponse, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let results = self
            .collect_member_results_from(
                member_connectors,
                |_member_id, connector| call_member(connector),
                Some(self.config.member_request_timeout),
                self.config.member_request_max_retries,
                "mint request",
                |responses, failures| {
                    (responses.len() >= threshold
                        && self
                            .aggregate_member_responses(outputs, responses, amount_validation)
                            .is_ok())
                        || threshold_rejection(failures, threshold).is_some()
                },
            )
            .await;

        if results.responses.len() >= threshold
            && self
                .aggregate_member_responses(outputs, &results.responses, amount_validation)
                .is_ok()
        {
            return Ok(results.responses);
        }

        if let Some(rejection) = results.threshold_rejection(threshold) {
            return Err(Error::from(rejection));
        }

        if let Some(catch_up_error) = results.catch_up_error() {
            return Err(catch_up_error);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation signature shares: required {}, received {}, failures: {}",
            threshold,
            results.responses.len(),
            results.failure_summary()
        )))
    }

    async fn collect_melt_responses<F, Fut>(
        &self,
        member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
        outputs: Option<&[BlindedMessage]>,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, MeltQuoteResponse<String>)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: Future<Output = Result<MeltQuoteResponse<String>, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let results = self
            .collect_member_results_from(
                member_connectors,
                |_member_id, connector| call_member(connector),
                Some(self.config.member_request_timeout),
                self.config.member_request_max_retries,
                "melt request",
                |responses, failures| {
                    let Some(group) = owned_matching_melt_response_group(responses, threshold)
                    else {
                        return threshold_rejection(failures, threshold).is_some();
                    };
                    (self.aggregate_melt_response(outputs, &group).is_ok())
                        || threshold_rejection(failures, threshold).is_some()
                },
            )
            .await;

        let aggregation_failure =
            match owned_matching_melt_response_group(&results.responses, threshold) {
                Some(group) => match self.aggregate_melt_response(outputs, &group) {
                    Ok(_) => return Ok(group),
                    Err(err) => Some(err.to_string()),
                },
                None => None,
            };

        if let Some(rejection) = results.threshold_rejection(threshold) {
            return Err(Error::from(rejection));
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation melt responses: required {}, received {}, failures: {}, aggregation: {}",
            threshold,
            results.responses.len(),
            results.failure_summary(),
            aggregation_failure.as_deref().unwrap_or("no matching threshold")
        )))
    }

    async fn collect_restore_responses<F, Fut>(
        &self,
        member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
        requested_outputs: &[BlindedMessage],
        mut call_member: F,
    ) -> Result<Vec<(MemberId, RestoreResponse)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: Future<Output = Result<RestoreResponse, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let results = self
            .collect_member_results_from(
                member_connectors,
                |_member_id, connector| call_member(connector),
                Some(self.config.member_request_timeout),
                self.config.member_request_max_retries,
                "restore request",
                |responses, _failures| {
                    responses.len() >= threshold
                        && self
                            .aggregate_restore_response(requested_outputs, responses)
                            .is_ok()
                },
            )
            .await;

        if results.responses.len() >= threshold {
            self.aggregate_restore_response(requested_outputs, &results.responses)?;
            return Ok(results.responses);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation restore responses: required {}, received {}, failures: {}",
            threshold,
            results.responses.len(),
            results.failure_summary()
        )))
    }

    async fn collect_melt_quote_status_responses<F, Fut>(
        &self,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, MeltQuoteResponse<String>)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: Future<Output = Result<MeltQuoteResponse<String>, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let results = self
            .collect_member_results(
                |_member_id, connector| call_member(connector),
                Some(self.config.quote_status_member_timeout),
                self.config.member_request_max_retries,
                "melt quote status",
                |responses, _failures| {
                    has_threshold_matching_melt_quote_status(responses, threshold)
                },
            )
            .await;

        if results.responses.len() >= threshold {
            return Ok(results.responses);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation melt quote status responses: required {}, received {}, failures: {}",
            threshold,
            results.responses.len(),
            results.failure_summary()
        )))
    }

    async fn collect_mint_quote_status_responses<F, Fut>(
        &self,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, MintQuoteResponse<String>)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: Future<Output = Result<MintQuoteResponse<String>, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let results = self
            .collect_member_results(
                |_member_id, connector| call_member(connector),
                Some(self.config.quote_status_member_timeout),
                self.config.member_request_max_retries,
                "mint quote status",
                |responses, _failures| {
                    has_threshold_matching_mint_quote_status(responses, threshold)
                },
            )
            .await;

        if results.responses.len() >= threshold {
            return Ok(results.responses);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation mint quote status responses: required {}, received {}, failures: {}",
            threshold,
            results.responses.len(),
            results.failure_summary()
        )))
    }

    async fn collect_batch_mint_quote_status_responses<F, Fut>(
        &self,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, Vec<MintQuoteResponse<String>>)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: Future<Output = Result<Vec<MintQuoteResponse<String>>, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let results = self
            .collect_member_results(
                |_member_id, connector| call_member(connector),
                Some(self.config.quote_status_member_timeout),
                self.config.member_request_max_retries,
                "batch mint quote status",
                |responses, _failures| responses.len() >= threshold,
            )
            .await;

        if results.responses.len() >= threshold {
            return Ok(results.responses);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation batch mint quote status responses: required {}, received {}, failures: {}",
            threshold,
            results.responses.len(),
            results.failure_summary()
        )))
    }

    async fn collect_check_state_responses<F, Fut>(
        &self,
        member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
        request: &CheckStateRequest,
        mut call_member: F,
    ) -> Result<Vec<(MemberId, CheckStateResponse)>, Error>
    where
        F: FnMut(Arc<dyn MintConnector + Send + Sync>) -> Fut,
        Fut: Future<Output = Result<CheckStateResponse, Error>>,
    {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let results = self
            .collect_member_results_from(
                member_connectors,
                |_member_id, connector| call_member(connector),
                Some(self.config.member_request_timeout),
                self.config.member_request_max_retries,
                "check-state request",
                |responses, _failures| {
                    responses.len() >= threshold
                        && self
                            .aggregate_check_state_response(request, responses)
                            .is_ok()
                },
            )
            .await;

        if results.responses.len() >= threshold {
            self.aggregate_check_state_response(request, &results.responses)?;
            return Ok(results.responses);
        }

        Err(Error::InvalidMintResponse(format!(
            "insufficient federation check-state responses: required {}, received {}, failures: {}",
            threshold,
            results.responses.len(),
            results.failure_summary()
        )))
    }

    async fn verify_mint_quote_visible_to_threshold(
        &self,
        response: &MintQuoteResponse<String>,
    ) -> Result<(), Error> {
        let mut last_error = None;
        for attempt in 1..=self.config.mint_quote_visibility_attempts {
            match self.mint_quote_visibility_check(response).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_error = Some(err);
                    if attempt < self.config.mint_quote_visibility_attempts {
                        tokio::time::sleep(self.config.mint_quote_visibility_retry_delay).await;
                    }
                }
            }
        }

        Err(Error::InvalidMintResponse(format!(
            "federation mint quote was not threshold-visible after {} attempts: {}",
            self.config.mint_quote_visibility_attempts,
            last_error
                .map(|err| err.to_string())
                .unwrap_or_else(|| "no status response".to_string())
        )))
    }

    async fn threshold_visible_mint_quote_after_ambiguous_creation(
        &self,
        request: &MintQuoteRequest,
        quote_id: &QuoteId,
    ) -> Result<Option<MintQuoteResponse<String>>, Error> {
        let method = request.method();
        let quote = quote_id.to_string();
        let member_responses = match self
            .collect_mint_quote_status_responses(|connector| {
                let method = method.clone();
                let quote = quote.clone();
                async move { connector.get_mint_quote_status(method, &quote).await }
            })
            .await
        {
            Ok(responses) => responses,
            Err(_) => return Ok(None),
        };
        let response = match self.aggregate_mint_quote_status_response(&member_responses) {
            Ok(response) => response,
            Err(_) => return Ok(None),
        };
        validate_assigned_mint_quote_response(request, quote_id, &response)?;
        Ok(Some(response))
    }

    async fn threshold_visible_melt_quote_after_ambiguous_creation(
        &self,
        request: &MeltQuoteRequest,
        quote_id: &QuoteId,
    ) -> Result<Option<MeltQuoteCreateResponse<String>>, Error> {
        let method = request.method();
        let quote = quote_id.to_string();
        let member_responses = match self
            .collect_melt_quote_status_responses(|connector| {
                let method = method.clone();
                let quote = quote.clone();
                async move { connector.get_melt_quote_status(method, &quote).await }
            })
            .await
        {
            Ok(responses) => responses,
            Err(_) => return Ok(None),
        };
        let response = match self.aggregate_melt_quote_status_response(&member_responses) {
            Ok(response) => melt_quote_create_response_from_status(response),
            Err(_) => return Ok(None),
        };
        validate_assigned_melt_quote_response(request, quote_id, &response)?;
        Ok(Some(response))
    }

    async fn create_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, Error> {
        let quote_id = QuoteId::new();
        let mut failures = Vec::new();

        for (member_id, connector) in
            self.ordered_quote_creation_connectors(&self.member_connectors)
        {
            let result = tokio::time::timeout(
                self.config.member_fallback_timeout,
                connector.post_mint_quote_with_id(request.clone(), &quote_id),
            )
            .await;
            match result {
                Ok(Ok(response)) => {
                    match validate_assigned_mint_quote_response(&request, &quote_id, &response) {
                        Ok(()) => {
                            self.verify_mint_quote_visible_to_threshold(&response)
                                .await?;
                            return Ok(response);
                        }
                        Err(err) => failures.push(format!("member {}: {}", member_id, err)),
                    }
                }
                Ok(Err(err)) if err.is_definitive_failure() => {
                    return Err(into_wallet_error(err));
                }
                Ok(Err(err)) => {
                    // Quote IDs are assigned once and reused across members, so
                    // retrying after one visibility check remains idempotent.
                    if let Some(response) = self
                        .threshold_visible_mint_quote_after_ambiguous_creation(&request, &quote_id)
                        .await?
                    {
                        return Ok(response);
                    }
                    failures.push(format!("member {}: {}", member_id, err));
                }
                Err(_) => {
                    if let Some(response) = self
                        .threshold_visible_mint_quote_after_ambiguous_creation(&request, &quote_id)
                        .await?
                    {
                        return Ok(response);
                    }
                    failures.push(format!(
                        "member {}: timed out after {}s",
                        member_id,
                        self.config.member_fallback_timeout.as_secs()
                    ));
                }
            }
        }

        Err(Error::InvalidMintResponse(format!(
            "no healthy federation member accepted mint quote creation: {}",
            failures.join("; ")
        )))
    }

    async fn create_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error> {
        let quote_id = QuoteId::new();
        let mut failures = Vec::new();

        for (member_id, connector) in
            self.ordered_quote_creation_connectors(&self.member_connectors)
        {
            let result = tokio::time::timeout(
                self.config.member_fallback_timeout,
                connector.post_melt_quote_with_id(request.clone(), &quote_id),
            )
            .await;
            match result {
                Ok(Ok(response)) => {
                    match validate_assigned_melt_quote_response(&request, &quote_id, &response) {
                        Ok(()) => {
                            self.verify_melt_quote_visible_to_threshold(&response)
                                .await?;
                            return Ok(response);
                        }
                        Err(err) => failures.push(format!("member {}: {}", member_id, err)),
                    }
                }
                Ok(Err(err)) if err.is_definitive_failure() => {
                    return Err(into_wallet_error(err));
                }
                Ok(Err(err)) => {
                    // Quote IDs are assigned once and reused across members, so
                    // retrying after one visibility check remains idempotent.
                    if let Some(response) = self
                        .threshold_visible_melt_quote_after_ambiguous_creation(&request, &quote_id)
                        .await?
                    {
                        return Ok(response);
                    }
                    failures.push(format!("member {}: {}", member_id, err));
                }
                Err(_) => {
                    if let Some(response) = self
                        .threshold_visible_melt_quote_after_ambiguous_creation(&request, &quote_id)
                        .await?
                    {
                        return Ok(response);
                    }
                    failures.push(format!(
                        "member {}: timed out after {}s",
                        member_id,
                        self.config.member_fallback_timeout.as_secs()
                    ));
                }
            }
        }

        Err(Error::InvalidMintResponse(format!(
            "no healthy federation member accepted melt quote creation: {}",
            failures.join("; ")
        )))
    }

    async fn mint_quote_visibility_check(
        &self,
        response: &MintQuoteResponse<String>,
    ) -> Result<(), Error> {
        let method = response.method();
        let quote_id = response.quote().to_string();
        let member_responses = self
            .collect_mint_quote_status_responses(|connector| {
                let method = method.clone();
                let quote_id = quote_id.clone();
                async move { connector.get_mint_quote_status(method, &quote_id).await }
            })
            .await?;
        let visible_response = self.aggregate_mint_quote_status_response(&member_responses)?;
        if visible_response.method() != response.method()
            || visible_response.quote() != response.quote()
        {
            return Err(Error::InvalidMintResponse(
                "federation mint quote visibility returned a different quote".to_string(),
            ));
        }

        Ok(())
    }

    async fn verify_melt_quote_visible_to_threshold(
        &self,
        response: &MeltQuoteCreateResponse<String>,
    ) -> Result<(), Error> {
        let mut last_error = None;
        for attempt in 1..=self.config.mint_quote_visibility_attempts {
            match self.melt_quote_visibility_check(response).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_error = Some(err);
                    if attempt < self.config.mint_quote_visibility_attempts {
                        tokio::time::sleep(self.config.mint_quote_visibility_retry_delay).await;
                    }
                }
            }
        }

        Err(Error::InvalidMintResponse(format!(
            "federation melt quote was not threshold-visible after {} attempts: {}",
            self.config.mint_quote_visibility_attempts,
            last_error
                .map(|err| err.to_string())
                .unwrap_or_else(|| "no status response".to_string())
        )))
    }

    async fn melt_quote_visibility_check(
        &self,
        response: &MeltQuoteCreateResponse<String>,
    ) -> Result<(), Error> {
        let method = response.method();
        let quote_id = response.quote().ok_or_else(|| {
            Error::InvalidMintResponse(
                "federation melt quote creation response omitted its quote id".to_string(),
            )
        })?;
        let member_responses = self
            .collect_melt_quote_status_responses(|connector| {
                let method = method.clone();
                let quote_id = quote_id.clone();
                async move { connector.get_melt_quote_status(method, &quote_id).await }
            })
            .await?;
        let visible_response = self.aggregate_melt_quote_status_response(&member_responses)?;
        if visible_response.method() != response.method() || visible_response.quote() != quote_id {
            return Err(Error::InvalidMintResponse(
                "federation melt quote visibility returned a different quote".to_string(),
            ));
        }

        Ok(())
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

        let mut matching_share_sets = Vec::<(
            Vec<(Amount, Id)>,
            Vec<(MemberId, Vec<BlsBlindSignatureShare>)>,
        )>::new();
        let mut invalid_members = Vec::new();

        for (member_id, response) in member_responses {
            match self.valid_member_share_set(outputs, *member_id, response, amount_validation) {
                Ok((metadata, shares)) => match matching_share_sets
                    .iter_mut()
                    .find(|(candidate, _)| candidate == &metadata)
                {
                    Some((_, members)) => members.push((*member_id, shares)),
                    None => matching_share_sets.push((metadata, vec![(*member_id, shares)])),
                },
                Err(err) => invalid_members.push(format!("member {} ignored: {}", member_id, err)),
            }
        }

        let max_valid_group = matching_share_sets
            .iter()
            .map(|(_, members)| members.len())
            .max()
            .unwrap_or(0);
        let (metadata, selected_members) = matching_share_sets
            .into_iter()
            .find_map(|(metadata, members)| {
                if members.len() >= threshold {
                    Some((metadata, members.into_iter().take(threshold).collect::<Vec<_>>()))
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                let invalid = if invalid_members.is_empty() {
                    "none".to_string()
                } else {
                    invalid_members.join("; ")
                };
                Error::InvalidMintResponse(format!(
                    "insufficient valid federation signature shares: required {}, largest compatible set {}, invalid responses: {}",
                    threshold, max_valid_group, invalid
                ))
            })?;

        let mut aggregate_signatures = Vec::with_capacity(outputs.len());
        for (index, (amount, keyset_id)) in metadata.into_iter().enumerate() {
            let shares = selected_members
                .iter()
                .map(|(_, shares)| shares[index])
                .collect::<Vec<_>>();
            let aggregate_signature = aggregate_blind_signature_shares(threshold, &shares)?;
            aggregate_signatures.push(BlindSignature {
                amount,
                keyset_id,
                c: aggregate_signature.into(),
                dleq: None,
            });
        }

        Ok(MintResponse {
            signatures: aggregate_signatures,
        })
    }

    fn valid_member_share_set(
        &self,
        outputs: &[BlindedMessage],
        member_id: MemberId,
        response: &MintResponse,
        amount_validation: ShareAmountValidation,
    ) -> Result<(Vec<(Amount, Id)>, Vec<BlsBlindSignatureShare>), Error> {
        if response.signatures.len() != outputs.len() {
            return Err(Error::InvalidMintResponse(format!(
                "member {} returned {} signature shares for {} outputs",
                member_id,
                response.signatures.len(),
                outputs.len()
            )));
        }

        let mut metadata = Vec::with_capacity(outputs.len());
        let mut shares = Vec::with_capacity(outputs.len());
        let mut blinded_messages = Vec::with_capacity(outputs.len());
        let mut public_shares = Vec::with_capacity(outputs.len());
        for (output, signature) in outputs.iter().zip(response.signatures.iter()) {
            validate_signature_metadata(signature, output, member_id, amount_validation)?;

            let mut signed_output = output.clone();
            signed_output.amount = signature.amount;
            signed_output.keyset_id = signature.keyset_id;
            let keyset = match amount_validation {
                ShareAmountValidation::Exact
                | ShareAmountValidation::AllowZeroAmountPlaceholder => {
                    self.public_config.validate_blinded_output(&signed_output)?
                }
                ShareAmountValidation::Restore => self
                    .public_config
                    .validate_blinded_restore_output(&signed_output)?,
            };
            let public_share = keyset
                .public_shares
                .get(&member_id)
                .ok_or(FederationError::MissingPublicShares {
                    keyset_id: keyset.id,
                    member_id,
                })?
                .get(&signature.amount)
                .ok_or(FederationError::MissingPublicShareAmount {
                    keyset_id: keyset.id,
                    member_id,
                    amount: signature.amount,
                })?;
            let blinded_message = output.blinded_secret.as_bls_g1()?;
            let share =
                BlsBlindSignatureShare::new(member_id.to_bls_signer_id(), signature.c.as_bls_g1()?);

            metadata.push((signature.amount, signature.keyset_id));
            shares.push(share);
            blinded_messages.push(blinded_message);
            public_shares.push(*public_share);
        }

        if !batch_verify_blind_signature_shares(&shares, &blinded_messages, &public_shares)? {
            for (((share, blinded_message), public_share), (amount, keyset_id)) in shares
                .iter()
                .zip(&blinded_messages)
                .zip(&public_shares)
                .zip(&metadata)
            {
                if !verify_blind_signature_share(share, blinded_message, public_share)? {
                    return Err(Error::InvalidMintResponse(format!(
                        "member {} returned an invalid signature share for keyset {} amount {}",
                        member_id, keyset_id, amount
                    )));
                }
            }

            return Err(Error::InvalidMintResponse(format!(
                "member {} signature-share batch verification failed",
                member_id
            )));
        }

        Ok((metadata, shares))
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

        let mut matching_views =
            Vec::<(serde_json::Value, MintQuoteResponse<String>, Vec<MemberId>)>::new();

        for (member_id, response) in member_responses {
            let view = mint_quote_status_consensus_view(*member_id, response)?;

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
            let view = response
                .iter()
                .map(|response| mint_quote_status_consensus_view(*member_id, response))
                .collect::<Result<Vec<_>, _>>()?;
            let view = serde_json::to_value(view).map_err(|err| {
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
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let matching_responses = matching_melt_quote_status_group(member_responses, threshold)
            .ok_or_else(|| {
                Error::InvalidMintResponse(format!(
                    "insufficient matching federation melt quote status responses: required {}",
                    threshold
                ))
            })?;
        let first_response = matching_responses
            .first()
            .map(|(_, response)| *response)
            .ok_or_else(|| {
                Error::InvalidMintResponse(
                    "missing federation melt quote status responses".to_string(),
                )
            })?;
        for (member_id, response) in matching_responses.iter().skip(1) {
            validate_melt_response_metadata(first_response, response, *member_id)?;
        }

        let Some(first_change) = first_response.change() else {
            for (member_id, response) in &matching_responses {
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
            for (member_id, response) in &matching_responses {
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

        let mut aggregate_change = Vec::with_capacity(first_change.len());
        for (index, first_signature) in first_change.iter().enumerate() {
            let mut shares = Vec::with_capacity(matching_responses.len());

            for (member_id, response) in &matching_responses {
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

        for (member_id, response) in &matching_responses {
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
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let matching_responses =
            matching_restore_response_group(requested_outputs, member_responses, threshold)?;
        let first_response = matching_responses
            .first()
            .map(|(_, response)| *response)
            .ok_or_else(|| {
                Error::InvalidMintResponse("missing federation restore responses".to_string())
            })?;

        let restore_share_responses = matching_responses
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

fn mint_quote_status_consensus_view(
    member_id: MemberId,
    response: &MintQuoteResponse<String>,
) -> Result<serde_json::Value, Error> {
    let mut response = response.clone();
    match &mut response {
        MintQuoteResponse::Bolt11(response) => response.updated_at = 0,
        MintQuoteResponse::Bolt12(response) => response.updated_at = 0,
        MintQuoteResponse::Onchain(response) => response.updated_at = 0,
        MintQuoteResponse::Custom { response, .. } => response.updated_at = 0,
    }
    serde_json::to_value(response).map_err(|err| {
        Error::InvalidMintResponse(format!(
            "member {} returned unserializable mint quote status response: {}",
            member_id, err
        ))
    })
}

fn validate_assigned_mint_quote_response(
    request: &MintQuoteRequest,
    quote_id: &QuoteId,
    response: &MintQuoteResponse<String>,
) -> Result<(), Error> {
    if response.quote() != &quote_id.to_string() || response.method() != request.method() {
        return Err(Error::InvalidMintResponse(
            "federation member returned a mint quote for a different assigned ID or method"
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_assigned_melt_quote_response(
    request: &MeltQuoteRequest,
    quote_id: &QuoteId,
    response: &MeltQuoteCreateResponse<String>,
) -> Result<(), Error> {
    if response.quote() != Some(&quote_id.to_string()) || response.method() != request.method() {
        return Err(Error::InvalidMintResponse(
            "federation member returned a melt quote for a different assigned ID or method"
                .to_string(),
        ));
    }

    Ok(())
}

fn melt_quote_create_response_from_status(
    response: MeltQuoteResponse<String>,
) -> MeltQuoteCreateResponse<String> {
    match response {
        MeltQuoteResponse::Bolt11(response) => MeltQuoteCreateResponse::Bolt11(response),
        MeltQuoteResponse::Bolt12(response) => MeltQuoteCreateResponse::Bolt12(response),
        MeltQuoteResponse::Onchain(response) => MeltQuoteCreateResponse::Onchain(response),
        MeltQuoteResponse::Custom(response) => MeltQuoteCreateResponse::Custom(response),
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MintConnector for FederatedMintConnector {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, Error> {
        let domain = domain.to_string();
        self.call_first_healthy_member("DNS TXT resolution", |connector| {
            let domain = domain.clone();
            async move { connector.resolve_dns_txt(&domain).await }
        })
        .await
    }

    async fn fetch_lnurl_pay_request(
        &self,
        url: &str,
    ) -> Result<crate::lightning_address::LnurlPayResponse, Error> {
        let url = url.to_string();
        self.call_first_healthy_member("LNURL pay request", |connector| {
            let url = url.clone();
            async move { connector.fetch_lnurl_pay_request(&url).await }
        })
        .await
    }

    async fn fetch_lnurl_invoice(
        &self,
        url: &str,
    ) -> Result<crate::lightning_address::LnurlPayInvoiceResponse, Error> {
        let url = url.to_string();
        self.call_first_healthy_member("LNURL invoice request", |connector| {
            let url = url.clone();
            async move { connector.fetch_lnurl_invoice(&url).await }
        })
        .await
    }

    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        self.validate_member_active_keysets().await?;
        Ok(self.active_aggregate_keysets())
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        let threshold = usize::from(self.public_config.threshold_params.signature_threshold);
        let mut valid_members = 0usize;
        let mut failures = Vec::new();

        let mut pending = FuturesUnordered::new();
        for (member_id, connector) in &self.member_connectors {
            let member_id = *member_id;
            let connector = connector.clone();
            pending.push(async move {
                let result = async {
                    let mint_info = connector.get_mint_info().await?;
                    let keyset = connector.get_mint_keyset(keyset_id).await?;
                    Ok::<_, Error>((mint_info, keyset))
                }
                .await;
                (member_id, result)
            });
        }

        while let Some((member_id, result)) = pending.next().await {
            match result {
                Ok((mint_info, advertised)) => {
                    FederationWalletConnector::validate_member_mint_info_with_config_digest(
                        &self.public_config,
                        member_id,
                        &mint_info,
                        self.config_digest,
                    )?;
                    validate_member_advertised_keyset(&self.public_config, member_id, &advertised)?;
                    valid_members = valid_members.saturating_add(1);
                    if valid_members >= threshold {
                        return self.aggregate_keyset(keyset_id);
                    }
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
        self.validate_member_active_keysets().await?;
        Ok(KeysetResponse {
            keysets: self.aggregate_keyset_infos(),
        })
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, Error> {
        self.create_mint_quote(request).await
    }

    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<String>,
    ) -> Result<MintResponse, Error> {
        self.public_config
            .validate_blinded_outputs(&request.outputs)
            .map_err(map_federation_validation_error)?;
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_mint_responses(
                &self.member_connectors,
                &request.outputs,
                ShareAmountValidation::Exact,
                |connector| {
                    let method = method.clone();
                    let request = request.clone();
                    async move { connector.post_mint(&method, request).await }
                },
            )
            .await?;
        let result = self.aggregate_member_responses(
            &request.outputs,
            &member_responses,
            ShareAmountValidation::Exact,
        );
        #[cfg(feature = "prometheus")]
        if result.is_err() {
            self.record_aggregation_failure("mint");
        }
        result
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
            .validate_blinded_outputs(&request.outputs)
            .map_err(map_federation_validation_error)?;
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_mint_responses(
                &self.member_connectors,
                &request.outputs,
                ShareAmountValidation::Exact,
                |connector| {
                    let method = method.clone();
                    let request = request.clone();
                    async move { connector.post_batch_mint(&method, request).await }
                },
            )
            .await?;
        let result = self.aggregate_member_responses(
            &request.outputs,
            &member_responses,
            ShareAmountValidation::Exact,
        );
        #[cfg(feature = "prometheus")]
        if result.is_err() {
            self.record_aggregation_failure("mint");
        }
        result
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error> {
        self.create_melt_quote(request).await
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
                .validate_blinded_change_outputs(outputs)
                .map_err(map_federation_validation_error)?;
        }
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_melt_responses(&self.member_connectors, outputs.as_deref(), |connector| {
                let method = method.clone();
                let request = request.clone();
                async move { connector.post_melt(&method, request).await }
            })
            .await?;
        let result = self.aggregate_melt_response(outputs.as_deref(), &member_responses);
        #[cfg(feature = "prometheus")]
        if result.is_err() {
            self.record_aggregation_failure("melt");
        }
        result
    }

    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, Error> {
        self.public_config
            .validate_blinded_outputs(request.outputs())
            .map_err(map_federation_validation_error)?;
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_mint_responses(
                &self.member_connectors,
                request.outputs(),
                ShareAmountValidation::Exact,
                |connector| {
                    let request = request.clone();
                    async move {
                        connector
                            .post_swap(request)
                            .await
                            .map(|response| MintResponse {
                                signatures: response.signatures,
                            })
                    }
                },
            )
            .await?;
        let response = self.aggregate_member_responses(
            request.outputs(),
            &member_responses,
            ShareAmountValidation::Exact,
        );
        #[cfg(feature = "prometheus")]
        if response.is_err() {
            self.record_aggregation_failure("swap");
        }
        let response = response?;

        Ok(SwapResponse {
            signatures: response.signatures,
        })
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        let (member_connectors, mint_info) =
            self.validate_member_active_keysets_with_mint_info().await?;
        if let Some(mint_info) = mint_info {
            return Ok(mint_info);
        }

        self.call_first_healthy_member_from(
            &member_connectors,
            "mint info",
            |connector| async move { connector.get_mint_info().await },
        )
        .await
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_check_state_responses(&self.member_connectors, &request, |connector| {
                let request = request.clone();
                async move { connector.post_check_state(request).await }
            })
            .await?;

        self.aggregate_check_state_response(&request, &member_responses)
    }

    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        self.public_config
            .validate_blinded_restore_outputs(&request.outputs)
            .map_err(map_federation_validation_error)?;
        self.validate_member_active_keysets().await?;

        let member_responses = self
            .collect_restore_responses(&self.member_connectors, &request.outputs, |connector| {
                let request = request.clone();
                async move { connector.post_restore(request).await }
            })
            .await?;

        let result = self.aggregate_restore_response(&request.outputs, &member_responses);
        #[cfg(feature = "prometheus")]
        if result.is_err() {
            self.record_aggregation_failure("restore");
        }
        result
    }

    async fn get_auth_wallet(&self) -> Option<AuthWallet> {
        for connector in self.member_connectors.values() {
            if let Some(wallet) = connector.get_auth_wallet().await {
                return Some(wallet);
            }
        }

        None
    }

    async fn set_auth_wallet(&self, wallet: Option<AuthWallet>) {
        join_all(self.member_connectors.values().map(|connector| {
            let connector = connector.clone();
            let wallet = wallet.clone();
            async move {
                connector.set_auth_wallet(wallet).await;
            }
        }))
        .await;
    }
}

fn map_federation_validation_error(err: FederationError) -> Error {
    match err {
        FederationError::InactiveFederatedKeyset { .. } => Error::InactiveKeyset,
        err => Error::Federation(err),
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

fn melt_quote_status_view(response: &MeltQuoteResponse<String>) -> MeltQuoteStatusView {
    let change = response.change().map(|signatures| {
        signatures
            .iter()
            .map(|signature| MeltChangeSignatureView {
                amount: signature.amount,
                keyset_id: signature.keyset_id,
                has_dleq: signature.dleq.is_some(),
            })
            .collect()
    });

    MeltQuoteStatusView {
        method: response.method(),
        quote: response.quote().clone(),
        amount: response.amount(),
        fee_reserve: response.fee_reserve(),
        state: response.state(),
        payment_proof: response.payment_proof().map(ToOwned::to_owned),
        request: response.request().map(ToOwned::to_owned),
        unit: response.unit(),
        change,
    }
}

fn has_threshold_matching_melt_quote_status(
    responses: &[(MemberId, MeltQuoteResponse<String>)],
    threshold: usize,
) -> bool {
    matching_melt_quote_status_group(responses, threshold).is_some()
}

fn has_threshold_matching_mint_quote_status(
    responses: &[(MemberId, MintQuoteResponse<String>)],
    threshold: usize,
) -> bool {
    let mut matching_views = Vec::<(serde_json::Value, usize)>::new();

    for (_, response) in responses {
        let Ok(view) = serde_json::to_value(response) else {
            return false;
        };

        match matching_views
            .iter_mut()
            .find(|(candidate, _)| *candidate == view)
        {
            Some((_, count)) => {
                *count = count.saturating_add(1);
                if *count >= threshold {
                    return true;
                }
            }
            None => matching_views.push((view, 1)),
        }
    }

    false
}

fn matching_melt_quote_status_group(
    responses: &[(MemberId, MeltQuoteResponse<String>)],
    threshold: usize,
) -> Option<Vec<(MemberId, &MeltQuoteResponse<String>)>> {
    let mut matching_views = Vec::<(
        MeltQuoteStatusView,
        Vec<(MemberId, &MeltQuoteResponse<String>)>,
    )>::new();

    for (member_id, response) in responses {
        let view = melt_quote_status_view(response);
        match matching_views
            .iter()
            .position(|(candidate, _)| candidate == &view)
        {
            Some(index) => matching_views[index].1.push((*member_id, response)),
            None => matching_views.push((view, vec![(*member_id, response)])),
        }
    }

    matching_views
        .into_iter()
        .filter(|(_, members)| members.len() >= threshold)
        .max_by_key(|(view, _)| match view.state {
            MeltQuoteState::Paid => 4,
            MeltQuoteState::Failed => 3,
            MeltQuoteState::Unpaid => 2,
            MeltQuoteState::Pending => 1,
            MeltQuoteState::Unknown => 0,
        })
        .map(|(_, members)| members)
}

fn owned_matching_melt_response_group(
    responses: &[(MemberId, MeltQuoteResponse<String>)],
    threshold: usize,
) -> Option<Vec<(MemberId, MeltQuoteResponse<String>)>> {
    matching_melt_quote_status_group(responses, threshold).map(|group| {
        group
            .into_iter()
            .map(|(member_id, response)| (member_id, response.clone()))
            .collect()
    })
}

fn matching_restore_response_group<'a>(
    requested_outputs: &[BlindedMessage],
    responses: &'a [(MemberId, RestoreResponse)],
    threshold: usize,
) -> Result<Vec<(MemberId, &'a RestoreResponse)>, Error> {
    let mut matching_views = Vec::<(Vec<BlindedMessage>, Vec<(MemberId, &RestoreResponse)>)>::new();

    for (member_id, response) in responses {
        validate_restore_response(requested_outputs, response, *member_id)?;
        match matching_views
            .iter()
            .position(|(outputs, _)| outputs == &response.outputs)
        {
            Some(index) => matching_views[index].1.push((*member_id, response)),
            None => matching_views.push((response.outputs.clone(), vec![(*member_id, response)])),
        }
    }

    matching_views
        .into_iter()
        .find_map(|(_, members)| {
            if members.len() >= threshold {
                Some(members)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            Error::InvalidMintResponse(format!(
                "insufficient matching federation restore responses: required {}",
                threshold
            ))
        })
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
        MeltQuoteResponse::Bolt11(response) => {
            response.change = change;
        }
        MeltQuoteResponse::Bolt12(response) => {
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

#[cfg(test)]
mod tests;
