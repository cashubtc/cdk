//! NUT-CTF-split-merge CTF Split/Merge operations
//!
//! Split: Convert regular tokens into conditional tokens across a partition.
//! Merge: Convert conditional tokens from a complete partition back into regular tokens.

use std::collections::{HashMap, HashSet};

use cdk_common::nuts::nut_ctf::{
    parse_outcome_collection, validate_partition, CtfMergeRequest, CtfMergeResponse,
    CtfSplitRequest, CtfSplitResponse, ZERO_COLLECTION_ID,
};
use tracing::instrument;

use super::swap::swap_saga::SwapSaga;
use super::Mint;
use crate::Error;

impl Mint {
    /// Process a CTF split request (POST /v1/ctf/split)
    ///
    /// Splits regular (or parent-collection) tokens into conditional tokens
    /// across a registered partition. Each outcome collection in the outputs
    /// receives the same total amount.
    #[instrument(skip_all)]
    pub async fn process_ctf_split(
        &self,
        request: CtfSplitRequest,
    ) -> Result<CtfSplitResponse, Error> {
        let inputs = &request.inputs;
        let outputs = &request.outputs;

        if inputs.is_empty() || outputs.is_empty() {
            return Err(Error::TransactionUnbalanced(0, 0, 0));
        }

        // 1. Look up the condition
        let condition = self
            .localstore
            .get_condition(&request.condition_id)
            .await?
            .ok_or(Error::ConditionNotFound)?;

        // Check condition is still pending (not yet attested)
        if condition.attestation_status != "pending" {
            return Err(Error::ConditionNotActive);
        }

        // 2. Extract outcomes from stored announcements
        let announcements: Vec<String> =
            serde_json::from_str(&condition.announcements_json)?;
        if announcements.is_empty() {
            return Err(Error::ConditionNotFound);
        }
        let first_ann =
            cdk_common::nuts::nut_ctf::dlc::parse_oracle_announcement(&announcements[0])?;
        let all_outcomes =
            cdk_common::nuts::nut_ctf::dlc::extract_outcomes(&first_ann)?;

        // 3. Validate output keys form a registered partition
        let output_keys: Vec<String> = outputs.keys().cloned().collect();
        validate_partition(&all_outcomes, &output_keys)?;

        // 4. Validate each output uses the correct conditional keyset
        let all_keysets = self
            .localstore
            .get_conditional_keysets_for_condition(&request.condition_id)
            .await?;

        for (oc_key, blinded_msgs) in outputs {
            // Normalize the outcome collection key
            let mut elements = parse_outcome_collection(oc_key);
            elements.sort();
            let oc_string = elements.join("|");

            let expected_keyset_id = all_keysets
                .get(&oc_string)
                .ok_or(Error::UnknownKeySet)?;

            for bm in blinded_msgs {
                if bm.keyset_id != *expected_keyset_id {
                    return Err(Error::UnknownKeySet);
                }
            }
        }

        // 5. Verify all outcome collection totals are equal
        let mut per_oc_total: Option<u64> = None;
        for blinded_msgs in outputs.values() {
            let total: u64 = blinded_msgs.iter().map(|bm| u64::from(bm.amount)).sum();
            match per_oc_total {
                Some(prev) if prev != total => return Err(Error::SplitAmountMismatch),
                None => per_oc_total = Some(total),
                _ => {}
            }
        }
        let per_oc_total = per_oc_total.ok_or(Error::SplitAmountMismatch)?;

        // 6. Verify inputs use a regular keyset (for root) or parent collection keyset (for nested)
        let input_keyset_ids: HashSet<_> = inputs.iter().map(|p| p.keyset_id).collect();
        // Determine if this is a root or nested split by checking parent_collection_id
        let stored_partitions = self
            .localstore
            .get_partitions_for_condition(&request.condition_id)
            .await?;

        let is_root = stored_partitions
            .iter()
            .any(|sp| sp.parent_collection_id == ZERO_COLLECTION_ID);

        if is_root {
            // For root conditions: inputs must use regular (non-conditional) keysets
            for kid in &input_keyset_ids {
                if self
                    .localstore
                    .get_condition_for_keyset(kid)
                    .await?
                    .is_some()
                {
                    return Err(Error::Custom(
                        "Root split inputs must use regular keysets".into(),
                    ));
                }
            }
        }

        // 7. Balance check: input_amount - fees = per_oc_total
        let input_amount: u64 = inputs.iter().map(|p| u64::from(p.amount)).sum();
        let fee_breakdown = self.get_proofs_fee(inputs).await?;
        let fee: u64 = fee_breakdown.total.into();

        if input_amount < fee || (input_amount - fee) != per_oc_total {
            return Err(Error::SplitAmountMismatch);
        }

        // 8. Flatten all blinded messages into a single list for the swap saga
        let mut all_blinded_messages = Vec::new();
        let mut partition_ranges: HashMap<String, (usize, usize)> = HashMap::new();

        for (oc_key, blinded_msgs) in outputs {
            let start = all_blinded_messages.len();
            all_blinded_messages.extend(blinded_msgs.iter().cloned());
            let end = all_blinded_messages.len();
            partition_ranges.insert(oc_key.clone(), (start, end));
        }

        // 9. Verify inputs cryptographically
        let input_verification = self.verify_inputs(inputs).await?;

        // 10. Execute via swap saga atomically
        let init_saga = SwapSaga::new(self, self.localstore.clone(), self.pubsub_manager.clone());

        let setup_saga = init_saga
            .setup_swap_unbalanced(inputs, &all_blinded_messages, None, input_verification)
            .await?;

        let signed_saga = setup_saga.sign_outputs().await?;
        let swap_response = signed_saga.finalize().await?;

        // 11. Partition signatures back by outcome collection
        let all_sigs = swap_response.signatures;
        let mut result_sigs: HashMap<String, Vec<_>> = HashMap::new();

        for (oc_key, (start, end)) in &partition_ranges {
            result_sigs.insert(oc_key.clone(), all_sigs[*start..*end].to_vec());
        }

        Ok(CtfSplitResponse {
            signatures: result_sigs,
        })
    }

    /// Process a CTF merge request (POST /v1/ctf/merge)
    ///
    /// Merges conditional tokens from a complete partition back into
    /// regular (or parent-collection) tokens.
    #[instrument(skip_all)]
    pub async fn process_ctf_merge(
        &self,
        request: CtfMergeRequest,
    ) -> Result<CtfMergeResponse, Error> {
        let inputs = &request.inputs;
        let outputs = &request.outputs;

        if inputs.is_empty() || outputs.is_empty() {
            return Err(Error::TransactionUnbalanced(0, 0, 0));
        }

        // 1. Look up the condition
        let condition = self
            .localstore
            .get_condition(&request.condition_id)
            .await?
            .ok_or(Error::ConditionNotFound)?;

        if condition.attestation_status != "pending" {
            return Err(Error::ConditionNotActive);
        }

        // 2. Extract outcomes
        let announcements: Vec<String> =
            serde_json::from_str(&condition.announcements_json)?;
        if announcements.is_empty() {
            return Err(Error::ConditionNotFound);
        }
        let first_ann =
            cdk_common::nuts::nut_ctf::dlc::parse_oracle_announcement(&announcements[0])?;
        let all_outcomes =
            cdk_common::nuts::nut_ctf::dlc::extract_outcomes(&first_ann)?;

        // 3. Validate input keys form a complete partition
        let input_keys: Vec<String> = inputs.keys().cloned().collect();
        validate_partition(&all_outcomes, &input_keys)?;

        // 4. Validate each input proof uses the correct conditional keyset
        let all_keysets = self
            .localstore
            .get_conditional_keysets_for_condition(&request.condition_id)
            .await?;

        for (oc_key, proofs) in inputs {
            let mut elements = parse_outcome_collection(oc_key);
            elements.sort();
            let oc_string = elements.join("|");

            let expected_keyset_id = all_keysets
                .get(&oc_string)
                .ok_or(Error::UnknownKeySet)?;

            for proof in proofs {
                if proof.keyset_id != *expected_keyset_id {
                    return Err(Error::UnknownKeySet);
                }
            }
        }

        // 5. Verify all per-outcome input amounts are equal
        let mut per_oc_total: Option<u64> = None;
        for proofs in inputs.values() {
            let total: u64 = proofs.iter().map(|p| u64::from(p.amount)).sum();
            match per_oc_total {
                Some(prev) if prev != total => return Err(Error::MergeAmountMismatch),
                None => per_oc_total = Some(total),
                _ => {}
            }
        }
        let per_oc_total = per_oc_total.ok_or(Error::MergeAmountMismatch)?;

        // 6. Validate outputs use regular keyset (for root) or parent keyset (for nested)
        let stored_partitions = self
            .localstore
            .get_partitions_for_condition(&request.condition_id)
            .await?;

        let is_root = stored_partitions
            .iter()
            .any(|sp| sp.parent_collection_id == ZERO_COLLECTION_ID);

        if is_root {
            let output_keyset_ids: HashSet<_> = outputs.iter().map(|o| o.keyset_id).collect();
            for kid in &output_keyset_ids {
                if self
                    .localstore
                    .get_condition_for_keyset(kid)
                    .await?
                    .is_some()
                {
                    return Err(Error::OutputsMustUseRegularKeyset);
                }
            }
        }

        // 7. Balance check: per_oc_total - fees(all_inputs) = output_amount
        let output_amount: u64 = outputs.iter().map(|o| u64::from(o.amount)).sum();

        // Flatten all inputs for fee calculation
        let mut all_input_proofs = Vec::new();
        for proofs in inputs.values() {
            all_input_proofs.extend(proofs.iter().cloned());
        }

        let fee_breakdown = self.get_proofs_fee(&all_input_proofs).await?;
        let fee: u64 = fee_breakdown.total.into();

        if per_oc_total < fee || (per_oc_total - fee) != output_amount {
            return Err(Error::MergeAmountMismatch);
        }

        // 8. Verify all inputs cryptographically
        let input_verification = self.verify_inputs(&all_input_proofs).await?;

        // 9. Execute via swap saga atomically
        let init_saga = SwapSaga::new(self, self.localstore.clone(), self.pubsub_manager.clone());

        let setup_saga = init_saga
            .setup_swap_unbalanced(&all_input_proofs, outputs, None, input_verification)
            .await?;

        let signed_saga = setup_saga.sign_outputs().await?;
        let swap_response = signed_saga.finalize().await?;

        Ok(CtfMergeResponse {
            signatures: swap_response.signatures,
        })
    }
}
