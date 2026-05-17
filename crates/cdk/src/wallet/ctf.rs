//! Wallet-side CTF (Conditional Token Framework) operations

use cdk_common::nuts::nut_ctf::{
    ConditionInfo, ConditionalKeysetsResponse, CtfMergeRequest, CtfMergeResponse, CtfSplitRequest,
    CtfSplitResponse, GetConditionsResponse, RedeemOutcomeRequest, RedeemOutcomeResponse,
    RegisterConditionRequest, RegisterConditionResponse, RegisterPartitionRequest,
    RegisterPartitionResponse,
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

    /// Register a partition for a condition on the mint
    #[instrument(skip(self, request))]
    pub async fn register_partition(
        &self,
        condition_id: &str,
        request: RegisterPartitionRequest,
    ) -> Result<RegisterPartitionResponse, Error> {
        self.client
            .post_register_partition(condition_id, request)
            .await
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
        self.client.get_conditional_keysets(since, limit, active).await
    }

    /// Split regular tokens into conditional tokens
    #[instrument(skip(self, request))]
    pub async fn ctf_split(
        &self,
        request: CtfSplitRequest,
    ) -> Result<CtfSplitResponse, Error> {
        self.client.post_ctf_split(request).await
    }

    /// Merge conditional tokens back into regular tokens
    #[instrument(skip(self, request))]
    pub async fn ctf_merge(
        &self,
        request: CtfMergeRequest,
    ) -> Result<CtfMergeResponse, Error> {
        self.client.post_ctf_merge(request).await
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
