//! NUT-28 Conditional token condition registration and query logic

use std::time::{SystemTime, UNIX_EPOCH};

use cdk_common::mint::StoredCondition;
use cdk_common::nuts::nut28::{
    compute_condition_id, compute_outcome_collection_id, dlc, parse_outcome_collection, to_hex,
    validate_partition, AttestationState, AttestationStatus, ConditionInfo,
    ConditionalKeysetsResponse, GetConditionsResponse, RegisterConditionRequest,
    RegisterConditionResponse, ZERO_COLLECTION_ID,
};
use tracing::instrument;

use super::Mint;
use crate::Error;

impl Mint {
    /// Register a new condition (POST /v1/conditions)
    #[instrument(skip_all)]
    pub async fn register_condition(
        &self,
        request: RegisterConditionRequest,
    ) -> Result<RegisterConditionResponse, Error> {
        // 1. Parse and verify announcements
        let announcements: Vec<_> = request
            .announcements
            .iter()
            .map(|hex| dlc::parse_oracle_announcement(hex))
            .collect::<Result<Vec<_>, _>>()?;

        for ann in &announcements {
            dlc::verify_announcement_signature(ann)?;
        }

        // 2. Extract info from announcements
        let oracle_pubkeys: Vec<Vec<u8>> = announcements
            .iter()
            .map(|a| dlc::extract_oracle_pubkey(a).to_vec())
            .collect();
        let event_id = dlc::extract_event_id(&announcements[0]);
        let outcomes = dlc::extract_outcomes(&announcements[0])?;
        let outcome_count = outcomes.len() as u8;

        // 3. Determine partition keys
        // Each partition key is a pipe-separated string of outcomes, e.g. "A|B"
        let partition: Vec<String> = if let Some(ref p) = request.partition {
            p.clone()
        } else {
            // Default: each outcome is its own collection key
            outcomes.clone()
        };

        // 4. Validate partition (disjoint + complete)
        validate_partition(&outcomes, &partition)?;

        // 5. Compute condition_id
        let condition_id_bytes = compute_condition_id(
            &oracle_pubkeys,
            &event_id,
            outcome_count,
            &partition,
        );
        let condition_id = to_hex(&condition_id_bytes);

        // 6. Check for existing condition (idempotency or conflict)
        if let Some(existing) = self.localstore.get_condition(&condition_id).await? {
            // Condition already exists — check if it's the same configuration
            let existing_partition: Vec<String> =
                serde_json::from_str(&existing.partition_json).unwrap_or_default();
            if existing_partition == partition {
                // Idempotent: return existing keysets
                let keysets = self
                    .localstore
                    .get_conditional_keysets_for_condition(&condition_id)
                    .await?;
                return Ok(RegisterConditionResponse {
                    condition_id,
                    keysets,
                });
            }
            return Err(Error::ConditionAlreadyExists);
        }

        // 7. Validate parent_collection_id if set
        let parent_collection_id = request
            .parent_collection_id
            .clone()
            .unwrap_or_else(|| ZERO_COLLECTION_ID.to_string());
        if parent_collection_id != ZERO_COLLECTION_ID {
            if parent_collection_id.len() != 64 {
                return Err(Error::InvalidConditionId);
            }
        }

        let threshold = request.threshold;
        let depth = 0u32; // Root conditions have depth 0

        // 8. Create conditional keysets via signatory
        let mut keysets = std::collections::HashMap::new();
        let amounts = (0..32).map(|n| 2u64.pow(n)).collect::<Vec<u64>>();

        for partition_key in &partition {
            // Normalize: parse outcomes from the key, sort, rejoin
            let mut elements = parse_outcome_collection(partition_key);
            elements.sort();
            let outcome_collection_string = elements.join("|");

            let outcome_collection_id_bytes =
                compute_outcome_collection_id(&outcome_collection_string, &condition_id_bytes);
            let outcome_collection_id = to_hex(&outcome_collection_id_bytes);

            let keyset = self
                .signatory
                .create_conditional_keyset(
                    cdk_common::CurrencyUnit::Sat,
                    &condition_id,
                    &outcome_collection_string,
                    &outcome_collection_id,
                    amounts.clone(),
                    0,
                    None,
                )
                .await?;

            // Store the keyset mapping
            self.localstore
                .add_conditional_keyset_info(
                    &condition_id,
                    &outcome_collection_string,
                    &outcome_collection_id,
                    &keyset.id,
                )
                .await?;

            keysets.insert(outcome_collection_string, keyset.id);
        }

        // 9. Store the condition
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let stored = StoredCondition {
            condition_id: condition_id.clone(),
            collateral: request.collateral,
            parent_collection_id: request
                .parent_collection_id
                .unwrap_or_else(|| ZERO_COLLECTION_ID.to_string()),
            depth,
            threshold,
            description: request.description,
            announcements_json: serde_json::to_string(&request.announcements)
                .unwrap_or_default(),
            partition_json: serde_json::to_string(&partition).unwrap_or_default(),
            attestation_status: "pending".to_string(),
            winning_outcome: None,
            attested_at: None,
            created_at: now,
        };

        self.localstore.add_condition(stored).await?;

        // Reload keys from signatory so new conditional keysets are available
        let updated_keysets = self.signatory.keysets().await?;
        self.keysets.store(updated_keysets.keysets.into());

        Ok(RegisterConditionResponse {
            condition_id,
            keysets,
        })
    }

    /// Get all conditions (GET /v1/conditions)
    #[instrument(skip_all)]
    pub async fn get_conditions(&self) -> Result<GetConditionsResponse, Error> {
        let conditions = self.localstore.get_conditions().await?;
        let mut infos = Vec::new();

        for condition in conditions {
            let keysets = self
                .localstore
                .get_conditional_keysets_for_condition(&condition.condition_id)
                .await?;

            let announcements: Vec<String> =
                serde_json::from_str(&condition.announcements_json).unwrap_or_default();

            infos.push(ConditionInfo {
                condition_id: condition.condition_id,
                collateral: condition.collateral,
                parent_collection_id: condition.parent_collection_id,
                depth: condition.depth,
                threshold: condition.threshold,
                description: condition.description,
                announcements,
                keysets,
                attestation: Some(AttestationState {
                    status: match condition.attestation_status.as_str() {
                        "attested" => AttestationStatus::Attested,
                        "expired" => AttestationStatus::Expired,
                        "violation" => AttestationStatus::Violation,
                        _ => AttestationStatus::Pending,
                    },
                    winning_outcome: condition.winning_outcome,
                    attested_at: condition.attested_at,
                }),
            });
        }

        Ok(GetConditionsResponse { conditions: infos })
    }

    /// Get a specific condition (GET /v1/conditions/{condition_id})
    #[instrument(skip_all)]
    pub async fn get_condition(&self, condition_id: &str) -> Result<ConditionInfo, Error> {
        let condition = self
            .localstore
            .get_condition(condition_id)
            .await?
            .ok_or(Error::ConditionNotFound)?;

        let keysets = self
            .localstore
            .get_conditional_keysets_for_condition(condition_id)
            .await?;

        let announcements: Vec<String> =
            serde_json::from_str(&condition.announcements_json).unwrap_or_default();

        Ok(ConditionInfo {
            condition_id: condition.condition_id,
            collateral: condition.collateral,
            parent_collection_id: condition.parent_collection_id,
            depth: condition.depth,
            threshold: condition.threshold,
            description: condition.description,
            announcements,
            keysets,
            attestation: Some(AttestationState {
                status: match condition.attestation_status.as_str() {
                    "attested" => AttestationStatus::Attested,
                    "expired" => AttestationStatus::Expired,
                    "violation" => AttestationStatus::Violation,
                    _ => AttestationStatus::Pending,
                },
                winning_outcome: condition.winning_outcome,
                attested_at: condition.attested_at,
            }),
        })
    }

    /// Get all conditional keysets (GET /v1/conditional_keysets)
    #[instrument(skip_all)]
    pub async fn get_conditional_keysets(
        &self,
    ) -> Result<ConditionalKeysetsResponse, Error> {
        let keysets = self
            .localstore
            .get_all_conditional_keyset_infos()
            .await?;

        Ok(ConditionalKeysetsResponse { keysets })
    }
}
