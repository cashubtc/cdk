//! NUT-CTF-split-merge CTF convert operation.
//!
//! Convert is the unified payoff-preserving operation for split, merge,
//! recombine, and collateral-crossing conversion.

use std::collections::{HashMap, HashSet};

use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::nut00::{BlindedMessage, Proof};
use cdk_common::nuts::nut02::Id;
use cdk_common::nuts::nut_ctf::{
    canonical_outcome_collection, compute_outcome_collection_id, from_hex,
    parse_outcome_collection, to_hex, CtfConvertRequest, CtfConvertResponse, ZERO_COLLECTION_ID,
};
use cdk_common::CurrencyUnit;
use tracing::instrument;

use super::conditions::STATUS_PENDING;
use super::swap::swap_saga::SwapSaga;
use super::Mint;
use crate::Error;

const COLLATERAL_KEY: &str = "*";

#[derive(Debug, Clone)]
struct EntryMeta {
    cover: Vec<String>,
    unit: CurrencyUnit,
}

impl Mint {
    /// Process a CTF convert request (POST /v1/ctf/convert).
    #[instrument(skip_all)]
    pub async fn process_ctf_convert(
        &self,
        request: CtfConvertRequest,
    ) -> Result<CtfConvertResponse, Error> {
        if request.inputs.is_empty() || request.outputs.is_empty() {
            return Err(Error::TransactionUnbalanced(0, 0, 0));
        }

        let condition = self
            .localstore
            .get_condition(&request.condition_id)
            .await?
            .ok_or(Error::ConditionNotFound)?;

        if condition.attestation_status != STATUS_PENDING {
            return Err(Error::ConvertNotPermitted);
        }

        let parent_collection_id = request
            .parent_collection_id
            .as_deref()
            .unwrap_or(ZERO_COLLECTION_ID);
        if parent_collection_id != ZERO_COLLECTION_ID {
            return Err(Error::ConvertPayoffFeeViolation);
        }
        let parent_bytes = hex_32(parent_collection_id)?;
        let condition_bytes = hex_32(&request.condition_id)?;
        let outcomes = self.condition_outcomes(&condition).await?;

        let mut expected_unit: Option<CurrencyUnit> = None;
        let mut input_vector = zero_vector(&outcomes);
        let mut output_vector = zero_vector(&outcomes);

        let mut all_input_proofs = Vec::new();
        let mut seen_secrets = HashSet::new();

        for (key, proofs) in &request.inputs {
            if proofs.is_empty() {
                return Err(Error::TransactionUnbalanced(0, 0, 0));
            }

            let meta = self
                .resolve_input_entry(
                    key,
                    proofs,
                    &request.condition_id,
                    &condition_bytes,
                    parent_collection_id,
                    &parent_bytes,
                    &outcomes,
                )
                .await?;
            check_unit(&mut expected_unit, &meta.unit)?;
            add_proof_amounts(&mut input_vector, &meta.cover, proofs)?;

            for proof in proofs {
                if !seen_secrets.insert(proof.secret.to_string()) {
                    return Err(Error::DuplicateInputs);
                }
                all_input_proofs.push(proof.clone());
            }
        }

        let mut all_blinded_messages = Vec::new();
        let mut output_ranges = HashMap::new();

        for (key, outputs) in &request.outputs {
            if outputs.is_empty() {
                return Err(Error::TransactionUnbalanced(0, 0, 0));
            }

            let meta = self
                .resolve_output_entry(
                    key,
                    outputs,
                    &request.condition_id,
                    &condition_bytes,
                    parent_collection_id,
                    &parent_bytes,
                    &outcomes,
                )
                .await?;
            check_unit(&mut expected_unit, &meta.unit)?;
            add_output_amounts(&mut output_vector, &meta.cover, outputs)?;

            let start = all_blinded_messages.len();
            all_blinded_messages.extend(outputs.iter().cloned());
            let end = all_blinded_messages.len();
            output_ranges.insert(key.clone(), (start, end));
        }

        let fee_breakdown = self.get_proofs_fee(&all_input_proofs).await?;
        let fee: u64 = fee_breakdown.total.into();
        if fee == 0 {
            return Err(Error::ConvertPayoffFeeViolation);
        }

        for outcome in &outcomes {
            let input_amount = *input_vector
                .get(outcome)
                .ok_or(Error::ConvertPayoffFeeViolation)?;
            let output_amount = *output_vector
                .get(outcome)
                .ok_or(Error::ConvertPayoffFeeViolation)?;
            if input_amount < fee || output_amount != input_amount - fee {
                return Err(Error::ConvertPayoffFeeViolation);
            }
        }

        let input_verification = self.verify_inputs(&all_input_proofs).await?;
        let init_saga = SwapSaga::new(self, self.localstore.clone(), self.pubsub_manager.clone());
        let setup_saga = init_saga
            .setup_swap_unbalanced(
                &all_input_proofs,
                &all_blinded_messages,
                None,
                input_verification,
            )
            .await?;
        let signed_saga = setup_saga.sign_outputs().await?;
        let swap_response = signed_saga.finalize().await?;

        let all_sigs = swap_response.signatures;
        let mut signatures = HashMap::new();
        for (key, (start, end)) in output_ranges {
            signatures.insert(key, all_sigs[start..end].to_vec());
        }

        Ok(CtfConvertResponse { signatures })
    }

    async fn resolve_input_entry(
        &self,
        key: &str,
        proofs: &[Proof],
        condition_id: &str,
        condition_id_bytes: &[u8; 32],
        parent_collection_id: &str,
        parent_collection_id_bytes: &[u8; 32],
        outcomes: &[String],
    ) -> Result<EntryMeta, Error> {
        if key == COLLATERAL_KEY {
            let unit = self
                .validate_collateral_proof_keysets(proofs, parent_collection_id)
                .await?;
            return Ok(EntryMeta {
                cover: outcomes.to_vec(),
                unit,
            });
        }

        let canonical = canonical_from_key(key, outcomes)?;
        if canonical != key {
            return Err(Error::ConvertPayoffFeeViolation);
        }

        let expected_collection_id =
            expected_collection_id(parent_collection_id_bytes, condition_id_bytes, &canonical)?;

        let mut unit = None;
        for proof in proofs {
            let keyset_info = self.active_keyset_info(&proof.keyset_id)?;
            check_unit(&mut unit, &keyset_info.unit)?;
            let (stored_condition_id, stored_collection, stored_collection_id) = self
                .localstore
                .get_condition_for_keyset(&proof.keyset_id)
                .await?
                .ok_or(Error::OutputsMustUseRegularKeyset)?;

            if stored_condition_id != condition_id
                || stored_collection != canonical
                || stored_collection_id != expected_collection_id
            {
                return Err(Error::OutputsMustUseRegularKeyset);
            }
        }

        Ok(EntryMeta {
            cover: parse_outcome_collection(&canonical),
            unit: unit.ok_or(Error::UnknownKeySet)?,
        })
    }

    async fn resolve_output_entry(
        &self,
        key: &str,
        outputs: &[BlindedMessage],
        condition_id: &str,
        condition_id_bytes: &[u8; 32],
        parent_collection_id: &str,
        parent_collection_id_bytes: &[u8; 32],
        outcomes: &[String],
    ) -> Result<EntryMeta, Error> {
        if key == COLLATERAL_KEY {
            let unit = self
                .validate_collateral_output_keysets(outputs, parent_collection_id)
                .await?;
            return Ok(EntryMeta {
                cover: outcomes.to_vec(),
                unit,
            });
        }

        let canonical = canonical_from_key(key, outcomes)?;
        if canonical != key {
            return Err(Error::ConvertPayoffFeeViolation);
        }

        let expected_collection_id =
            expected_collection_id(parent_collection_id_bytes, condition_id_bytes, &canonical)?;

        let mut unit = None;
        for output in outputs {
            let keyset_info = self.active_keyset_info(&output.keyset_id)?;
            check_unit(&mut unit, &keyset_info.unit)?;
            let (stored_condition_id, stored_collection, stored_collection_id) = self
                .localstore
                .get_condition_for_keyset(&output.keyset_id)
                .await?
                .ok_or(Error::OutputsMustUseRegularKeyset)?;

            if stored_condition_id != condition_id
                || stored_collection != canonical
                || stored_collection_id != expected_collection_id
            {
                return Err(Error::OutputsMustUseRegularKeyset);
            }
        }

        Ok(EntryMeta {
            cover: parse_outcome_collection(&canonical),
            unit: unit.ok_or(Error::UnknownKeySet)?,
        })
    }

    async fn validate_collateral_proof_keysets(
        &self,
        proofs: &[Proof],
        parent_collection_id: &str,
    ) -> Result<CurrencyUnit, Error> {
        let mut unit = None;
        let is_root = parent_collection_id == ZERO_COLLECTION_ID;
        for proof in proofs {
            let keyset_info = self.active_keyset_info(&proof.keyset_id)?;
            check_unit(&mut unit, &keyset_info.unit)?;
            let condition_keyset = self
                .localstore
                .get_condition_for_keyset(&proof.keyset_id)
                .await?;
            match (is_root, condition_keyset) {
                (true, None) => {}
                (false, Some((_, _, outcome_collection_id)))
                    if outcome_collection_id == parent_collection_id => {}
                _ => return Err(Error::OutputsMustUseRegularKeyset),
            }
        }
        unit.ok_or(Error::UnknownKeySet)
    }

    async fn validate_collateral_output_keysets(
        &self,
        outputs: &[BlindedMessage],
        parent_collection_id: &str,
    ) -> Result<CurrencyUnit, Error> {
        let mut unit = None;
        let is_root = parent_collection_id == ZERO_COLLECTION_ID;
        for output in outputs {
            let keyset_info = self.active_keyset_info(&output.keyset_id)?;
            check_unit(&mut unit, &keyset_info.unit)?;
            let condition_keyset = self
                .localstore
                .get_condition_for_keyset(&output.keyset_id)
                .await?;
            match (is_root, condition_keyset) {
                (true, None) => {}
                (false, Some((_, _, outcome_collection_id)))
                    if outcome_collection_id == parent_collection_id => {}
                _ => return Err(Error::OutputsMustUseRegularKeyset),
            }
        }
        unit.ok_or(Error::UnknownKeySet)
    }

    fn active_keyset_info(&self, keyset_id: &Id) -> Result<MintKeySetInfo, Error> {
        let keyset_info = self
            .get_keyset_info(keyset_id)
            .ok_or(Error::UnknownKeySet)?;
        if !keyset_info.active {
            return Err(Error::InactiveKeyset);
        }
        Ok(keyset_info)
    }

    async fn condition_outcomes(
        &self,
        condition: &cdk_common::mint::StoredCondition,
    ) -> Result<Vec<String>, Error> {
        if condition.condition_type == "numeric" {
            return Ok(vec!["HI".to_string(), "LO".to_string()]);
        }

        let announcements: Vec<String> = serde_json::from_str(&condition.announcements_json)?;
        let first_announcement = announcements.first().ok_or(Error::ConditionNotFound)?;
        let parsed = cdk_common::nuts::nut_ctf::dlc::parse_oracle_announcement(first_announcement)?;
        cdk_common::nuts::nut_ctf::dlc::extract_outcomes(&parsed).map_err(Error::from)
    }
}

fn hex_32(hex: &str) -> Result<[u8; 32], Error> {
    let bytes = from_hex(hex)?;
    bytes.try_into().map_err(|_| Error::InvalidConditionId)
}

fn canonical_from_key(key: &str, outcomes: &[String]) -> Result<String, Error> {
    let members = parse_outcome_collection(key);
    canonical_outcome_collection(outcomes, &members).map_err(Error::from)
}

fn expected_collection_id(
    parent_collection_id: &[u8; 32],
    condition_id: &[u8; 32],
    canonical: &str,
) -> Result<String, Error> {
    let id = compute_outcome_collection_id(parent_collection_id, condition_id, canonical)?;
    Ok(to_hex(&id))
}

fn zero_vector(outcomes: &[String]) -> HashMap<String, u64> {
    outcomes
        .iter()
        .map(|outcome| (outcome.clone(), 0u64))
        .collect()
}

fn check_unit(expected: &mut Option<CurrencyUnit>, actual: &CurrencyUnit) -> Result<(), Error> {
    match expected {
        Some(unit) if unit != actual => Err(Error::MultipleUnits),
        Some(_) => Ok(()),
        None => {
            *expected = Some(actual.clone());
            Ok(())
        }
    }
}

fn add_proof_amounts(
    vector: &mut HashMap<String, u64>,
    cover: &[String],
    proofs: &[Proof],
) -> Result<(), Error> {
    let total: u64 = proofs.iter().map(|proof| u64::from(proof.amount)).sum();
    add_amount_to_cover(vector, cover, total)
}

fn add_output_amounts(
    vector: &mut HashMap<String, u64>,
    cover: &[String],
    outputs: &[BlindedMessage],
) -> Result<(), Error> {
    let total: u64 = outputs.iter().map(|output| u64::from(output.amount)).sum();
    add_amount_to_cover(vector, cover, total)
}

fn add_amount_to_cover(
    vector: &mut HashMap<String, u64>,
    cover: &[String],
    amount: u64,
) -> Result<(), Error> {
    if amount == 0 {
        return Err(Error::ConvertPayoffFeeViolation);
    }
    for outcome in cover {
        let current = vector
            .get_mut(outcome)
            .ok_or(Error::ConvertPayoffFeeViolation)?;
        *current = current
            .checked_add(amount)
            .ok_or(Error::ConvertPayoffFeeViolation)?;
    }
    Ok(())
}
