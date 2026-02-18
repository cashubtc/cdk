//! NUT-28 Conditional token condition registration and query logic

use std::time::{SystemTime, UNIX_EPOCH};

use cdk_common::mint::{StoredCondition, StoredPartition};
use cdk_common::nuts::nut28::{
    compute_condition_id, compute_outcome_collection_id, dlc, from_hex, parse_outcome_collection,
    to_hex, validate_partition, AttestationState, AttestationStatus, ConditionInfo,
    ConditionalKeysetsResponse, GetConditionsResponse, PartitionInfoEntry,
    RegisterConditionRequest, RegisterConditionResponse, RegisterPartitionRequest,
    RegisterPartitionResponse, ZERO_COLLECTION_ID,
};
use tracing::instrument;

use super::Mint;
use crate::Error;

impl Mint {
    /// Register a new condition (POST /v1/conditions)
    ///
    /// This only registers the condition itself (oracle announcements, threshold, etc.)
    /// without creating any keysets. Keysets are created via `register_partition`.
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

        // 3. Compute condition_id (no partition in the hash)
        let condition_id_bytes =
            compute_condition_id(&oracle_pubkeys, &event_id, outcome_count);
        let condition_id = to_hex(&condition_id_bytes);

        // 4. Check for existing condition (idempotency)
        if self.localstore.get_condition(&condition_id).await?.is_some() {
            // Condition already exists — idempotent return
            return Ok(RegisterConditionResponse { condition_id });
        }

        // 5. Store the condition
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let stored = StoredCondition {
            condition_id: condition_id.clone(),
            threshold: request.threshold,
            description: request.description.clone(),
            announcements_json: serde_json::to_string(&request.announcements)
                .unwrap_or_default(),
            attestation_status: "pending".to_string(),
            winning_outcome: None,
            attested_at: None,
            created_at: now,
        };

        self.localstore.add_condition(stored).await?;

        Ok(RegisterConditionResponse { condition_id })
    }

    /// Register a partition for a condition (POST /v1/conditions/{condition_id}/partitions)
    ///
    /// Creates conditional keysets for each outcome collection in the partition.
    #[instrument(skip_all)]
    pub async fn register_partition(
        &self,
        condition_id: &str,
        request: RegisterPartitionRequest,
    ) -> Result<RegisterPartitionResponse, Error> {
        // 1. Look up the condition
        let condition = self
            .localstore
            .get_condition(condition_id)
            .await?
            .ok_or(Error::ConditionNotFound)?;

        // 2. Extract outcomes from stored announcements
        let announcements: Vec<String> =
            serde_json::from_str(&condition.announcements_json).unwrap_or_default();
        let first_ann = dlc::parse_oracle_announcement(&announcements[0])?;
        let outcomes = dlc::extract_outcomes(&first_ann)?;

        // 3. Determine partition keys
        let partition: Vec<String> = if let Some(ref p) = request.partition {
            p.clone()
        } else {
            outcomes.clone()
        };

        // 4. Validate partition (disjoint + complete)
        validate_partition(&outcomes, &partition)?;

        // 5. Parse parent_collection_id
        let parent_collection_id_hex = request
            .parent_collection_id
            .clone()
            .unwrap_or_else(|| ZERO_COLLECTION_ID.to_string());
        if parent_collection_id_hex.len() != 64 {
            return Err(Error::InvalidConditionId);
        }
        let parent_collection_id_bytes: [u8; 32] = from_hex(&parent_collection_id_hex)
            .map_err(|_| Error::InvalidConditionId)?
            .try_into()
            .map_err(|_| Error::InvalidConditionId)?;

        let condition_id_bytes: [u8; 32] = from_hex(condition_id)
            .map_err(|_| Error::InvalidConditionId)?
            .try_into()
            .map_err(|_| Error::InvalidConditionId)?;

        // 6. Store the partition
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let stored_partition = StoredPartition {
            condition_id: condition_id.to_string(),
            partition_json: serde_json::to_string(&partition).unwrap_or_default(),
            collateral: request.collateral.clone(),
            parent_collection_id: parent_collection_id_hex,
            created_at: now,
        };

        self.localstore.add_partition(stored_partition).await?;

        // 7. Create conditional keysets via signatory
        let mut keysets = std::collections::HashMap::new();
        let amounts = (0..32).map(|n| 2u64.pow(n)).collect::<Vec<u64>>();
        let mut new_signatory_keysets = Vec::new();

        for partition_key in &partition {
            // Normalize: parse outcomes from the key, sort, rejoin
            let mut elements = parse_outcome_collection(partition_key);
            elements.sort();
            let outcome_collection_string = elements.join("|");

            let outcome_collection_id_bytes = compute_outcome_collection_id(
                &parent_collection_id_bytes,
                &condition_id_bytes,
                &outcome_collection_string,
            )?;
            let outcome_collection_id = to_hex(&outcome_collection_id_bytes);

            let mut keyset = self
                .signatory
                .create_conditional_keyset(
                    cdk_common::CurrencyUnit::Sat,
                    condition_id,
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
                    condition_id,
                    &outcome_collection_string,
                    &outcome_collection_id,
                    &keyset.id,
                )
                .await?;

            // Conditional keysets must be active for swaps
            keyset.active = true;
            new_signatory_keysets.push(keyset.clone());

            keysets.insert(outcome_collection_string, keyset.id);
        }

        // Append new conditional keysets to the existing in-memory store
        let mut current = self.keysets.load().as_ref().clone();
        current.extend(new_signatory_keysets);
        self.keysets.store(current.into());

        Ok(RegisterPartitionResponse { keysets })
    }

    /// Get all conditions (GET /v1/conditions)
    #[instrument(skip_all)]
    pub async fn get_conditions(&self) -> Result<GetConditionsResponse, Error> {
        let conditions = self.localstore.get_conditions().await?;
        let mut infos = Vec::new();

        for condition in conditions {
            let info = self.build_condition_info(condition).await?;
            infos.push(info);
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

        self.build_condition_info(condition).await
    }

    /// Build a ConditionInfo from a StoredCondition, including partitions and keysets
    async fn build_condition_info(
        &self,
        condition: StoredCondition,
    ) -> Result<ConditionInfo, Error> {
        let announcements: Vec<String> =
            serde_json::from_str(&condition.announcements_json).unwrap_or_default();

        // Load partitions for this condition
        let stored_partitions = self
            .localstore
            .get_partitions_for_condition(&condition.condition_id)
            .await?;

        // Load all keyset mappings for this condition
        let all_keysets = self
            .localstore
            .get_conditional_keysets_for_condition(&condition.condition_id)
            .await?;

        // Build partition info entries
        let mut partitions = Vec::new();
        for sp in stored_partitions {
            let partition_keys: Vec<String> =
                serde_json::from_str(&sp.partition_json).unwrap_or_default();

            // Filter keysets that belong to this partition
            let mut partition_keysets = std::collections::HashMap::new();
            for key in &partition_keys {
                let mut elements = parse_outcome_collection(key);
                elements.sort();
                let oc_string = elements.join("|");
                if let Some(kid) = all_keysets.get(&oc_string) {
                    partition_keysets.insert(oc_string, *kid);
                }
            }

            partitions.push(PartitionInfoEntry {
                partition: partition_keys,
                collateral: sp.collateral,
                parent_collection_id: sp.parent_collection_id,
                keysets: partition_keysets,
            });
        }

        Ok(ConditionInfo {
            condition_id: condition.condition_id,
            threshold: condition.threshold,
            description: condition.description,
            announcements,
            partitions,
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
