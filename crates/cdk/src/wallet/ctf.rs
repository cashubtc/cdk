//! Wallet-side CTF (Conditional Token Framework) operations

use cdk_common::nuts::nut_ctf::{
    ConditionInfo, ConditionalKeysetsResponse, CtfConvertRequest, CtfConvertResponse,
    GetConditionsResponse, RedeemOutcomeRequest, RedeemOutcomeResponse, RegisterConditionRequest,
    RegisterConditionResponse,
};
use tracing::instrument;

use super::Wallet;
use crate::error::Error;

impl Wallet {
    /// Get all conditions from the mint
    ///
    /// Supports cursor-based pagination via `since`+`limit` and repeatable `status` filter.
    #[instrument(skip(self))]
    pub async fn get_conditions(
        &self,
        since: Option<u64>,
        limit: Option<u64>,
        status: &[String],
    ) -> Result<GetConditionsResponse, Error> {
        self.client.get_conditions(since, limit, status).await
    }

    /// Get a specific condition from the mint
    #[instrument(skip(self))]
    pub async fn get_condition(&self, condition_id: &str) -> Result<ConditionInfo, Error> {
        self.client.get_condition(condition_id).await
    }

    /// Register a new condition on the mint
    #[instrument(skip(self, request))]
    pub async fn register_condition(
        &self,
        request: RegisterConditionRequest,
    ) -> Result<RegisterConditionResponse, Error> {
        self.client.post_register_condition(request).await
    }

    /// Get all conditional keysets from the mint
    ///
    /// Supports cursor-based pagination via `since`+`limit` and `active` filter.
    #[instrument(skip(self))]
    pub async fn get_conditional_keysets(
        &self,
        since: Option<u64>,
        limit: Option<u64>,
        active: Option<bool>,
    ) -> Result<ConditionalKeysetsResponse, Error> {
        self.client
            .get_conditional_keysets(since, limit, active)
            .await
    }

    /// Convert conditional/collateral positions.
    #[instrument(skip(self, request))]
    pub async fn ctf_convert(
        &self,
        request: CtfConvertRequest,
    ) -> Result<CtfConvertResponse, Error> {
        self.client.post_ctf_convert(request).await
    }

    /// Redeem winning conditional tokens for regular tokens
    #[instrument(skip(self, request))]
    pub async fn redeem_outcome(
        &self,
        request: RedeemOutcomeRequest,
    ) -> Result<RedeemOutcomeResponse, Error> {
        self.client.post_redeem_outcome(request).await
    }
}
