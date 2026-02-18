//! NUT-28 Redeem outcome processing

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use cdk_common::nuts::nut28::dlc;
use cdk_common::nuts::nut28::{
    from_hex, parse_outcome_collection, OracleWitness, RedeemOutcomeRequest,
    RedeemOutcomeResponse,
};
use cdk_common::nuts::Witness;
use tracing::instrument;

use super::Mint;
use crate::Error;

impl Mint {
    /// Process a redeem outcome request (POST /v1/redeem_outcome)
    #[instrument(skip_all)]
    pub async fn process_redeem_outcome(
        &self,
        request: RedeemOutcomeRequest,
    ) -> Result<RedeemOutcomeResponse, Error> {
        let inputs = &request.inputs;
        let outputs = &request.outputs;

        if inputs.is_empty() {
            return Err(Error::TransactionUnbalanced(0, 0, 0));
        }

        // 1. Verify all inputs use the same conditional keyset
        let input_keyset_ids: HashSet<_> = inputs.iter().map(|p| p.keyset_id).collect();
        if input_keyset_ids.len() != 1 {
            return Err(Error::InputsMustUseSameConditionalKeyset);
        }
        let input_keyset_id = inputs[0].keyset_id;

        // 2. Look up the condition for this keyset
        let (condition_id, outcome_collection, _outcome_collection_id) = self
            .localstore
            .get_condition_for_keyset(&input_keyset_id)
            .await?
            .ok_or(Error::ConditionNotFound)?;

        let condition = self
            .localstore
            .get_condition(&condition_id)
            .await?
            .ok_or(Error::ConditionNotFound)?;

        // 3. Verify all outputs use a regular (non-conditional) keyset of the same unit
        let output_keyset_ids: HashSet<_> = outputs.iter().map(|o| o.keyset_id).collect();
        for oid in &output_keyset_ids {
            let is_conditional = self.localstore.get_condition_for_keyset(oid).await?;
            if is_conditional.is_some() {
                return Err(Error::OutputsMustUseRegularKeyset);
            }
        }

        // 4. Check attestation state
        if condition.attestation_status == "attested" {
            // Already attested — verify inputs match the winning outcome
            if let Some(ref winner) = condition.winning_outcome {
                if outcome_collection != *winner {
                    return Err(Error::OracleNotAttestedOutcome);
                }
            }
        } else {
            // 5. Not yet attested — verify the oracle witness
            let witness = Self::extract_oracle_witness(inputs)?;

            // Parse announcements to get oracle info
            let announcements: Vec<String> =
                serde_json::from_str(&condition.announcements_json).unwrap_or_default();

            let parsed_announcements: Vec<_> = announcements
                .iter()
                .map(|hex| dlc::parse_oracle_announcement(hex))
                .collect::<Result<Vec<_>, _>>()?;

            // Verify threshold oracle signatures
            let mut valid_sigs = 0u32;
            for sig in &witness.oracle_sigs {
                // Find matching announcement by oracle pubkey
                for ann in &parsed_announcements {
                    let pubkey_bytes = dlc::extract_oracle_pubkey(ann);
                    let pubkey_hex =
                        pubkey_bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>();
                    if pubkey_hex == sig.oracle_pubkey {
                        let nonce_points = dlc::extract_nonce_points(&ann.oracle_event);
                        if !nonce_points.is_empty() {
                            if let Some(ref oracle_sig_hex) = sig.oracle_sig {
                                let pk_bytes = from_hex(&sig.oracle_pubkey)?;
                                let sig_bytes = from_hex(oracle_sig_hex)?;
                                if dlc::verify_oracle_attestation(
                                    &pk_bytes,
                                    &sig_bytes,
                                    &sig.outcome,
                                    &nonce_points[0],
                                )
                                .is_ok()
                                {
                                    valid_sigs += 1;
                                }
                            }
                        }
                    }
                }
            }

            if valid_sigs < condition.threshold {
                return Err(Error::OracleThresholdNotMet);
            }

            // Determine winning outcome from the oracle's attested outcome
            let attested_outcome = &witness.oracle_sigs[0].outcome;

            // Check which partition key contains the attested outcome
            // Load all partitions for this condition and search through them
            let stored_partitions = self
                .localstore
                .get_partitions_for_condition(&condition_id)
                .await?;

            let mut winning_collection = None;
            for sp in &stored_partitions {
                let partition_keys: Vec<String> =
                    serde_json::from_str(&sp.partition_json).unwrap_or_default();
                for key in &partition_keys {
                    let outcomes = parse_outcome_collection(key);
                    if outcomes.contains(attested_outcome) {
                        let mut elements = parse_outcome_collection(key);
                        elements.sort();
                        winning_collection = Some(elements.join("|"));
                        break;
                    }
                }
                if winning_collection.is_some() {
                    break;
                }
            }

            let winning_collection =
                winning_collection.ok_or(Error::OracleNotAttestedOutcome)?;

            // Verify the inputs match the winning collection
            if outcome_collection != winning_collection {
                return Err(Error::OracleNotAttestedOutcome);
            }

            // Record attestation (first-write-wins)
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            self.localstore
                .update_condition_attestation(
                    &condition_id,
                    "attested",
                    Some(&winning_collection),
                    Some(now),
                )
                .await?;
        }

        // 6. Balance check (inputs >= outputs + fees)
        let input_amount: u64 = inputs.iter().map(|p| u64::from(p.amount)).sum();
        let output_amount: u64 = outputs.iter().map(|o| u64::from(o.amount)).sum();

        if input_amount < output_amount {
            return Err(Error::TransactionUnbalanced(
                input_amount,
                output_amount,
                0,
            ));
        }

        // 7. Verify inputs cryptographically
        let input_verification = self.verify_inputs(inputs).await?;

        // 8. Execute via swap saga (reserve inputs -> sign outputs -> mark spent)
        let init_saga = crate::mint::swap::swap_saga::SwapSaga::new(
            self,
            self.localstore.clone(),
            self.pubsub_manager.clone(),
        );

        let setup_saga = init_saga
            .setup_swap(inputs, outputs, None, input_verification)
            .await?;

        let signed_saga = setup_saga.sign_outputs().await?;
        let swap_response = signed_saga.finalize().await?;

        Ok(RedeemOutcomeResponse {
            signatures: swap_response.signatures,
        })
    }

    /// Extract the OracleWitness from a set of proofs
    fn extract_oracle_witness(proofs: &cdk_common::Proofs) -> Result<OracleWitness, Error> {
        for proof in proofs {
            if let Some(ref witness) = proof.witness {
                if let Witness::OracleWitness(ref ow) = witness {
                    return Ok(ow.clone());
                }
            }
        }
        Err(Error::ConditionalKeysetRequiresWitness)
    }
}
