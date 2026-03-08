//! NUT-CTF Redeem outcome processing

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use cdk_common::nuts::nut_ctf::dlc;
use cdk_common::nuts::nut_ctf::{
    compute_numeric_payout, from_hex, parse_outcome_collection, to_hex, OracleWitness,
    RedeemOutcomeRequest, RedeemOutcomeResponse,
};
use cdk_common::nuts::Witness;
use tracing::instrument;

use super::conditions::STATUS_ATTESTED;
use super::Mint;
use crate::Error;

/// Parse announcements JSON and build a pubkey-to-hex-string lookup map.
/// Returns (parsed announcement hex strings, pubkey->index map).
fn parse_announcements_with_index(
    announcements_json: &str,
) -> Result<(Vec<String>, HashMap<String, usize>), Error> {
    let hex_strings: Vec<String> = serde_json::from_str(announcements_json)?;
    let mut pubkey_index = HashMap::with_capacity(hex_strings.len());
    for (i, hex) in hex_strings.iter().enumerate() {
        let ann = dlc::parse_oracle_announcement(hex)?;
        let pubkey_hex = to_hex(&dlc::extract_oracle_pubkey(&ann));
        pubkey_index.insert(pubkey_hex, i);
    }
    Ok((hex_strings, pubkey_index))
}

/// Verify enum oracle signatures and return the attested outcome.
fn verify_enum_threshold(
    ann_hex_strings: &[String],
    pubkey_index: &HashMap<String, usize>,
    witness: &OracleWitness,
    threshold: u32,
) -> Result<String, Error> {
    let mut verified_oracle_pubkeys: HashSet<String> = HashSet::new();
    let mut attested_outcome: Option<String> = None;

    for sig in &witness.oracle_sigs {
        if let Some(&idx) = pubkey_index.get(&sig.oracle_pubkey) {
            let ann = dlc::parse_oracle_announcement(&ann_hex_strings[idx])?;
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
                        match &attested_outcome {
                            Some(prev) if *prev != sig.outcome => {
                                return Err(Error::OracleNotAttestedOutcome);
                            }
                            None => {
                                attested_outcome = Some(sig.outcome.clone());
                            }
                            _ => {}
                        }
                        verified_oracle_pubkeys.insert(sig.oracle_pubkey.clone());
                    }
                }
            }
        }
    }

    if (verified_oracle_pubkeys.len() as u32) < threshold {
        return Err(Error::OracleThresholdNotMet);
    }

    attested_outcome.ok_or(Error::ConditionalKeysetRequiresWitness)
}

/// Verify numeric (digit decomposition) oracle signatures and return the attested value.
fn verify_numeric_threshold(
    ann_hex_strings: &[String],
    pubkey_index: &HashMap<String, usize>,
    witness: &OracleWitness,
    threshold: u32,
) -> Result<i64, Error> {
    let mut verified_oracle_pubkeys: HashSet<String> = HashSet::new();
    let mut attested_value: Option<i64> = None;

    for sig_entry in &witness.oracle_sigs {
        if let Some(&idx) = pubkey_index.get(&sig_entry.oracle_pubkey) {
            let ann = dlc::parse_oracle_announcement(&ann_hex_strings[idx])?;

            let digit_sigs_hex = sig_entry
                .digit_sigs
                .as_ref()
                .ok_or(Error::ConditionalKeysetRequiresWitness)?;

            let digit_sigs_bytes: Vec<Vec<u8>> = digit_sigs_hex
                .iter()
                .map(|h| from_hex(h))
                .collect::<Result<Vec<_>, _>>()?;

            let nonce_points = dlc::extract_nonce_points(&ann.oracle_event);
            let dd_info = dlc::extract_digit_decomposition(&ann)?;

            let pk_bytes = from_hex(&sig_entry.oracle_pubkey)?;
            let value = dlc::verify_digit_attestation(
                &pk_bytes,
                &digit_sigs_bytes,
                &nonce_points,
                dd_info.base,
                dd_info.is_signed,
            )?;

            match attested_value {
                Some(prev) if prev != value => {
                    return Err(Error::OracleNotAttestedOutcome);
                }
                None => {
                    attested_value = Some(value);
                }
                _ => {}
            }
            verified_oracle_pubkeys.insert(sig_entry.oracle_pubkey.clone());
        }
    }

    if (verified_oracle_pubkeys.len() as u32) < threshold {
        return Err(Error::OracleThresholdNotMet);
    }

    attested_value.ok_or(Error::ConditionalKeysetRequiresWitness)
}

/// Record the attestation result and handle concurrent attestation races.
async fn record_attestation(
    localstore: &dyn cdk_common::database::MintDatabase<cdk_common::database::Error>,
    condition_id: &str,
    winning_value: &str,
) -> Result<(), Error> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let updated = localstore
        .update_condition_attestation(condition_id, STATUS_ATTESTED, Some(winning_value), Some(now))
        .await
        .map_err(Error::Database)?;

    if !updated {
        let refreshed = localstore
            .get_condition(condition_id)
            .await
            .map_err(Error::Database)?
            .ok_or(Error::ConditionNotFound)?;
        if refreshed.winning_outcome.as_deref() != Some(winning_value) {
            return Err(Error::OracleNotAttestedOutcome);
        }
    }

    Ok(())
}

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

        // 3. Verify all outputs use a regular (non-conditional), active keyset
        let output_keyset_ids: HashSet<_> = outputs.iter().map(|o| o.keyset_id).collect();
        for oid in &output_keyset_ids {
            // Check keyset exists and is active
            let keyset_info = self.get_keyset_info(oid).ok_or(Error::UnknownKeySet)?;
            if !keyset_info.active {
                return Err(Error::InactiveKeyset);
            }
            // Check it's not a conditional keyset
            let is_conditional = self.localstore.get_condition_for_keyset(oid).await?;
            if is_conditional.is_some() {
                return Err(Error::OutputsMustUseRegularKeyset);
            }
        }

        // Branch on condition type
        let is_numeric = condition.condition_type == "numeric";

        if is_numeric {
            // --- NUT-CTF-numeric: Numeric proportional redemption ---
            self.process_numeric_redemption(
                &condition,
                &condition_id,
                &outcome_collection,
                inputs,
                outputs,
            )
            .await
        } else {
            // --- NUT-CTF: Enum winner-take-all redemption ---
            self.process_enum_redemption(
                &condition,
                &condition_id,
                &outcome_collection,
                inputs,
                outputs,
            )
            .await
        }
    }

    /// NUT-CTF: Enum winner-take-all redemption
    async fn process_enum_redemption(
        &self,
        condition: &cdk_common::mint::StoredCondition,
        condition_id: &str,
        outcome_collection: &str,
        inputs: &cdk_common::Proofs,
        outputs: &[cdk_common::nuts::nut00::BlindedMessage],
    ) -> Result<RedeemOutcomeResponse, Error> {
        // 4. Check attestation state
        if condition.attestation_status == STATUS_ATTESTED {
            // Already attested — verify inputs match the winning outcome
            if let Some(ref winner) = condition.winning_outcome {
                if outcome_collection != *winner {
                    return Err(Error::OracleNotAttestedOutcome);
                }
            }
        } else {
            // 5. Not yet attested — verify the oracle witness
            let witness = Self::extract_oracle_witness(inputs)?;
            let (ann_hex, pubkey_index) =
                parse_announcements_with_index(&condition.announcements_json)?;
            let attested_outcome =
                verify_enum_threshold(&ann_hex, &pubkey_index, &witness, condition.threshold)?;

            // Find winning collection
            let stored_partitions = self
                .localstore
                .get_partitions_for_condition(condition_id)
                .await?;

            let mut winning_collection = None;
            for sp in &stored_partitions {
                let partition_keys: Vec<String> =
                    serde_json::from_str(&sp.partition_json)?;
                for key in &partition_keys {
                    let outcomes = parse_outcome_collection(key);
                    if outcomes.iter().any(|o| o.as_str() == attested_outcome) {
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

            if outcome_collection != winning_collection {
                return Err(Error::OracleNotAttestedOutcome);
            }

            record_attestation(&*self.localstore, condition_id, &winning_collection).await?;
        }

        // 6. Balance check
        let input_amount: u64 = inputs.iter().map(|p| u64::from(p.amount)).sum();
        let output_amount: u64 = outputs.iter().map(|o| u64::from(o.amount)).sum();

        if input_amount < output_amount {
            return Err(Error::TransactionUnbalanced(
                input_amount,
                output_amount,
                0,
            ));
        }

        // 7. Verify inputs and execute swap saga
        let input_verification = self.verify_inputs(inputs).await?;

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

    /// NUT-CTF-numeric: Numeric proportional redemption
    async fn process_numeric_redemption(
        &self,
        condition: &cdk_common::mint::StoredCondition,
        condition_id: &str,
        outcome_collection: &str,
        inputs: &cdk_common::Proofs,
        outputs: &[cdk_common::nuts::nut00::BlindedMessage],
    ) -> Result<RedeemOutcomeResponse, Error> {
        // Validate outcome collection is HI or LO
        if outcome_collection != "HI" && outcome_collection != "LO" {
            return Err(Error::OracleNotAttestedOutcome);
        }

        let lo_bound = condition.lo_bound.ok_or_else(|| {
            Error::Custom("Numeric condition missing lo_bound".into())
        })?;
        let hi_bound = condition.hi_bound.ok_or_else(|| {
            Error::Custom("Numeric condition missing hi_bound".into())
        })?;

        // Determine the attested value
        let attested_value: i64 = if condition.attestation_status == STATUS_ATTESTED {
            // Already attested — parse stored value
            condition
                .winning_outcome
                .as_deref()
                .ok_or(Error::OracleNotAttestedOutcome)?
                .parse()
                .map_err(|_| Error::Custom("Invalid stored attested value".into()))?
        } else {
            // Not yet attested — verify digit signatures
            let witness = Self::extract_oracle_witness(inputs)?;
            let (ann_hex, pubkey_index) =
                parse_announcements_with_index(&condition.announcements_json)?;
            let value =
                verify_numeric_threshold(&ann_hex, &pubkey_index, &witness, condition.threshold)?;

            // Record attestation (store attested value as string)
            let value_str = value.to_string();
            record_attestation(&*self.localstore, condition_id, &value_str).await?;

            value
        };

        // Compute proportional payout
        let input_amount: u64 = inputs.iter().map(|p| u64::from(p.amount)).sum();
        let (hi_payout, lo_payout) =
            compute_numeric_payout(input_amount, attested_value, lo_bound, hi_bound)?;

        let my_payout = if outcome_collection == "HI" {
            hi_payout
        } else {
            lo_payout
        };

        // Balance check: output_amount <= my_payout (fees handled by swap saga)
        let output_amount: u64 = outputs.iter().map(|o| u64::from(o.amount)).sum();
        let fee_breakdown = self.get_proofs_fee(inputs).await?;
        let fee: u64 = fee_breakdown.total.into();

        if my_payout < fee || output_amount > (my_payout - fee) {
            return Err(Error::TransactionUnbalanced(
                my_payout,
                output_amount,
                fee,
            ));
        }

        // Verify inputs and execute via unbalanced swap saga
        let input_verification = self.verify_inputs(inputs).await?;

        let init_saga = crate::mint::swap::swap_saga::SwapSaga::new(
            self,
            self.localstore.clone(),
            self.pubsub_manager.clone(),
        );

        let setup_saga = init_saga
            .setup_swap_unbalanced(inputs, outputs, None, input_verification)
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
            if let Some(Witness::OracleWitness(ref ow)) = proof.witness {
                return Ok(ow.clone());
            }
        }
        Err(Error::ConditionalKeysetRequiresWitness)
    }
}
