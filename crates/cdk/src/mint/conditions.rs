//! NUT-CTF Conditional token condition registration and query logic

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use cdk_common::mint::{StoredCondition, StoredPartition};
use cdk_common::nuts::nut_ctf::{
    compute_condition_id, compute_condition_id_numeric, compute_outcome_collection_id, dlc,
    from_hex, parse_outcome_collection, to_hex, validate_partition, AttestationState,
    AttestationStatus, ConditionInfo, ConditionalKeysetsResponse, GetConditionsResponse,
    PartitionInfoEntry, RegisterConditionRequest, RegisterConditionResponse,
    RegisterPartitionRequest, RegisterPartitionResponse, MAX_ANNOUNCEMENT_HEX_LENGTH,
    MAX_ANNOUNCEMENTS, MAX_DESCRIPTION_LENGTH, MAX_PARTITION_KEYS, ZERO_COLLECTION_ID,
};
use tracing::instrument;

use super::Mint;
use crate::Error;

/// Maximum number of items returned per paginated request.
const MAX_PAGE_SIZE: u64 = 100;

/// Valid values for the `status` query parameter on conditions.
const VALID_CONDITION_STATUSES: &[&str] = &["pending", "attested", "expired", "violation"];

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
        // 0. Input size validation
        if request.announcements.is_empty() || request.announcements.len() > MAX_ANNOUNCEMENTS {
            return Err(Error::Custom(format!(
                "Number of announcements must be between 1 and {}",
                MAX_ANNOUNCEMENTS
            )));
        }
        if request.description.len() > MAX_DESCRIPTION_LENGTH {
            return Err(Error::Custom(format!(
                "Description exceeds maximum length of {}",
                MAX_DESCRIPTION_LENGTH
            )));
        }
        for ann_hex in &request.announcements {
            if ann_hex.len() > MAX_ANNOUNCEMENT_HEX_LENGTH {
                return Err(Error::Custom(format!(
                    "Announcement hex exceeds maximum length of {}",
                    MAX_ANNOUNCEMENT_HEX_LENGTH
                )));
            }
        }

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

        // 3. Branch on condition_type
        let is_numeric = request.condition_type == "numeric";
        let (outcomes, outcome_count, condition_id_bytes) = if is_numeric {
            // NUT-CTF-numeric: numeric condition
            let lo_bound = request.lo_bound.ok_or_else(|| {
                Error::Custom("lo_bound required for numeric conditions".into())
            })?;
            let hi_bound = request.hi_bound.ok_or_else(|| {
                Error::Custom("hi_bound required for numeric conditions".into())
            })?;
            if lo_bound >= hi_bound {
                return Err(Error::Custom(format!(
                    "lo_bound ({}) must be less than hi_bound ({})",
                    lo_bound, hi_bound
                )));
            }
            let precision = request.precision.unwrap_or(0);

            // Verify it's actually a digit decomposition announcement
            dlc::extract_digit_decomposition(&announcements[0])?;

            // Numeric conditions always have 2 outcome collections: HI, LO
            let outcomes = vec!["HI".to_string(), "LO".to_string()];
            let cid = compute_condition_id_numeric(
                &oracle_pubkeys,
                &event_id,
                2,
                lo_bound,
                hi_bound,
                precision,
            );
            (outcomes, 2u8, cid)
        } else {
            // NUT-CTF: enum condition
            let outcomes = dlc::extract_outcomes(&announcements[0])?;
            let outcome_count = u8::try_from(outcomes.len()).map_err(|_| {
                Error::Custom(format!(
                    "Outcome count {} exceeds maximum of 255",
                    outcomes.len()
                ))
            })?;
            let cid = compute_condition_id(&oracle_pubkeys, &event_id, outcome_count);
            (outcomes, outcome_count, cid)
        };
        let _ = (outcomes, outcome_count); // suppress unused warnings; used for validation above
        let condition_id = to_hex(&condition_id_bytes);

        // 4. Check for existing condition (idempotency or conflict)
        if let Some(existing) = self.localstore.get_condition(&condition_id).await? {
            // Validate parameters match for true idempotency
            let existing_announcements: Vec<String> =
                serde_json::from_str(&existing.announcements_json)?;
            if existing.threshold != request.threshold
                || existing.description != request.description
                || existing.condition_type != request.condition_type
                || existing_announcements != request.announcements
                || existing.lo_bound != request.lo_bound
                || existing.hi_bound != request.hi_bound
                || existing.precision != request.precision
            {
                return Err(Error::ConditionAlreadyExists);
            }
            // Parameters match — idempotent return
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
            announcements_json: serde_json::to_string(&request.announcements)?,
            attestation_status: "pending".to_string(),
            winning_outcome: None,
            attested_at: None,
            created_at: now,
            condition_type: request.condition_type.clone(),
            lo_bound: request.lo_bound,
            hi_bound: request.hi_bound,
            precision: request.precision,
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
        // 0. Validate partition size limits
        if let Some(ref p) = request.partition {
            if p.len() > MAX_PARTITION_KEYS {
                return Err(Error::Custom(format!(
                    "Partition keys exceed maximum of {}",
                    MAX_PARTITION_KEYS
                )));
            }
        }

        // 1. Look up the condition
        let condition = self
            .localstore
            .get_condition(condition_id)
            .await?
            .ok_or(Error::ConditionNotFound)?;

        // 2. Extract outcomes from stored announcements
        let announcements: Vec<String> =
            serde_json::from_str(&condition.announcements_json)?;
        if announcements.is_empty() {
            return Err(Error::ConditionNotFound);
        }
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

        // 6. Prepare partition (persisted after keysets succeed)
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let stored_partition = StoredPartition {
            condition_id: condition_id.to_string(),
            partition_json: serde_json::to_string(&partition)?,
            collateral: request.collateral.clone(),
            parent_collection_id: parent_collection_id_hex.clone(),
            created_at: now,
        };

        // 7. Create conditional keysets via signatory
        // For root partitions (parent is zero), collateral is a currency unit string.
        // For nested partitions, collateral is an outcome_collection_id; look up the
        // parent keyset's unit instead.
        let is_root = parent_collection_id_hex == ZERO_COLLECTION_ID;
        let unit = if is_root {
            cdk_common::CurrencyUnit::from_str(&request.collateral).map_err(|_| {
                Error::Custom(format!(
                    "Invalid collateral unit: {}",
                    request.collateral
                ))
            })?
        } else {
            // Nested partition: collateral is an outcome_collection_id.
            // Look up the parent keyset to determine the unit.
            let parent_keysets = self
                .localstore
                .get_conditional_keysets_for_condition(condition_id)
                .await?;
            let parent_keyset_id = parent_keysets.values().next().ok_or_else(|| {
                Error::Custom(
                    "No parent keysets found for nested partition".to_string(),
                )
            })?;
            let parent_info = self.keysets.load();
            let info = parent_info
                .iter()
                .find(|k| k.id == *parent_keyset_id)
                .ok_or_else(|| {
                    Error::Custom("Parent keyset not found in memory".to_string())
                })?;
            info.unit.clone()
        };

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
                    unit.clone(),
                    condition_id,
                    &outcome_collection_string,
                    &outcome_collection_id,
                    amounts.clone(),
                    0,
                    None,
                )
                .await?;

            // Store the keyset mapping
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            self.localstore
                .add_conditional_keyset_info(
                    condition_id,
                    &outcome_collection_string,
                    &outcome_collection_id,
                    &keyset.id,
                    now,
                )
                .await?;

            // Conditional keysets must be active for swaps
            keyset.active = true;
            new_signatory_keysets.push(keyset.clone());

            keysets.insert(outcome_collection_string, keyset.id);
        }

        // 8. Persist partition only after all keysets were created successfully
        self.localstore.add_partition(stored_partition).await?;

        // Append new conditional keysets to the existing in-memory store
        let mut current = self.keysets.load().as_ref().clone();
        current.extend(new_signatory_keysets);
        self.keysets.store(current.into());

        Ok(RegisterPartitionResponse { keysets })
    }

    /// Get all conditions (GET /v1/conditions)
    ///
    /// Supports cursor-based pagination via `since`+`limit` and repeatable `status` filter.
    #[instrument(skip_all)]
    pub async fn get_conditions(
        &self,
        since: Option<u64>,
        limit: Option<u64>,
        status: &[String],
    ) -> Result<GetConditionsResponse, Error> {
        // Validate status filter values
        for s in status {
            if !VALID_CONDITION_STATUSES.contains(&s.as_str()) {
                return Err(Error::Custom(format!(
                    "Invalid status filter value: '{}'. Valid values are: {}",
                    s,
                    VALID_CONDITION_STATUSES.join(", ")
                )));
            }
        }

        // Cap limit to MAX_PAGE_SIZE
        let limit = limit.map(|l| l.min(MAX_PAGE_SIZE));

        let conditions = self.localstore.get_conditions(since, limit, status).await?;
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
            serde_json::from_str(&condition.announcements_json)?;

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
                serde_json::from_str(&sp.partition_json)?;

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
                registered_at: sp.created_at,
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
            condition_type: condition.condition_type,
            lo_bound: condition.lo_bound,
            hi_bound: condition.hi_bound,
            precision: condition.precision,
            registered_at: condition.created_at,
        })
    }

    /// Get all conditional keysets (GET /v1/conditional_keysets)
    ///
    /// Supports cursor-based pagination via `since`+`limit` and `active` filter.
    #[instrument(skip_all)]
    pub async fn get_conditional_keysets(
        &self,
        since: Option<u64>,
        limit: Option<u64>,
        active: Option<bool>,
    ) -> Result<ConditionalKeysetsResponse, Error> {
        // Cap limit to MAX_PAGE_SIZE
        let limit = limit.map(|l| l.min(MAX_PAGE_SIZE));

        let keysets = self
            .localstore
            .get_all_conditional_keyset_infos(since, limit, active)
            .await?;

        Ok(ConditionalKeysetsResponse { keysets })
    }
}
