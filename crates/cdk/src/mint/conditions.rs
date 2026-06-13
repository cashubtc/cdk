//! NUT-CTF Conditional token condition registration and query logic

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use cdk_common::mint::StoredCondition;
use cdk_common::nuts::nut_ctf::{
    canonical_outcome_collection, compute_condition_id, compute_condition_id_numeric,
    compute_outcome_collection_id, dlc, parse_outcome_collection, to_hex, AttestationState,
    AttestationStatus, ConditionInfo, ConditionalKeysetsResponse, GetConditionsResponse,
    RegisterConditionRequest, RegisterConditionResponse, MAX_ANNOUNCEMENTS,
    MAX_ANNOUNCEMENT_HEX_LENGTH, MAX_OUTCOMES, MAX_OUTCOME_COLLECTIONS, MAX_TAGS_JSON_LENGTH,
};
use cdk_common::nuts::{BlindSignature, BlindedMessage};
use cdk_common::CurrencyUnit;
use tracing::instrument;

use super::Mint;
use crate::Error;

/// Maximum number of items returned per paginated request.
const MAX_PAGE_SIZE: u64 = 100;

/// Valid values for the `status` query parameter on conditions.
const VALID_CONDITION_STATUSES: &[&str] = &["pending", "attested", "expired", "violation"];

/// Attestation status string constants matching DB storage values.
pub(super) const STATUS_PENDING: &str = "pending";
pub(super) const STATUS_ATTESTED: &str = "attested";

const CONDITION_TYPE_ENUM: &str = "enum";
const CONDITION_TYPE_NUMERIC: &str = "numeric";
const KEYSET_POLICY_NONE: &str = "none";
const KEYSET_POLICY_ONE_VS_REST: &str = "one-vs-rest";
const KEYSET_POLICY_ALL: &str = "all";

struct RegistrationFeeVerification {
    proofs: cdk_common::Proofs,
    amount: cdk_common::Amount<cdk_common::CurrencyUnit>,
    change_messages: Vec<BlindedMessage>,
    change_blinded_secrets: Vec<cdk_common::PublicKey>,
    change: Vec<BlindSignature>,
}

impl Mint {
    /// Register a new condition (POST /v1/conditions)
    ///
    /// Registers the condition and creates any requested outcome-collection keysets.
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
        let tags_json = serde_json::to_string(&request.tags)?;
        if tags_json.len() > MAX_TAGS_JSON_LENGTH {
            return Err(Error::Custom(format!(
                "Tags JSON exceeds maximum length of {}",
                MAX_TAGS_JSON_LENGTH
            )));
        }
        for tag in &request.tags {
            if tag.is_empty() {
                return Err(Error::Custom(
                    "Each tag must contain at least one element".to_string(),
                ));
            }
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
        if request.condition_type != CONDITION_TYPE_ENUM
            && request.condition_type != CONDITION_TYPE_NUMERIC
        {
            return Err(Error::Custom(format!(
                "Unsupported condition_type: {}",
                request.condition_type
            )));
        }
        let is_numeric = request.condition_type == CONDITION_TYPE_NUMERIC;
        let (outcomes, _outcome_count, condition_id_bytes) = if is_numeric {
            // NUT-CTF-numeric: numeric condition
            let lo_bound = request
                .lo_bound
                .ok_or_else(|| Error::Custom("lo_bound required for numeric conditions".into()))?;
            let hi_bound = request
                .hi_bound
                .ok_or_else(|| Error::Custom("hi_bound required for numeric conditions".into()))?;
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
            if outcomes.len() > self.max_outcomes_per_condition {
                return Err(Error::Custom(format!(
                    "Outcome count {} exceeds configured maximum of {}",
                    outcomes.len(),
                    self.max_outcomes_per_condition
                )));
            }
            let outcome_count = u8::try_from(outcomes.len()).map_err(|_| {
                Error::Custom(format!(
                    "Outcome count {} exceeds protocol maximum of {}",
                    outcomes.len(),
                    MAX_OUTCOMES
                ))
            })?;
            let cid = compute_condition_id(&oracle_pubkeys, &event_id, outcome_count);
            (outcomes, outcome_count, cid)
        };
        let condition_id = to_hex(&condition_id_bytes);
        let default_keyset_creation = self.default_keyset_creation_policy().await?;
        let requested_collections = self.requested_outcome_collections(
            &outcomes,
            &request,
            is_numeric,
            &default_keyset_creation,
        )?;
        let required_fee = self
            .required_registration_fee(requested_collections.len())
            .await?;
        let collateral_unit = request
            .collateral
            .as_deref()
            .map(CurrencyUnit::from_str)
            .transpose()
            .map_err(|_| {
                Error::Custom(format!(
                    "Invalid collateral unit: {}",
                    request.collateral.as_deref().unwrap_or_default()
                ))
            })?;

        // 4. Check for existing condition (idempotency or conflict)
        if let Some(existing) = self.localstore.get_condition(&condition_id).await? {
            // Validate parameters match for true idempotency. condition_id binds to
            // the *sorted* oracle pubkeys, so two requests with the same announcement
            // set in different submission orders produce the same condition_id —
            // compare announcement and tag arrays as multisets, not in submission order.
            let mut existing_announcements: Vec<String> =
                serde_json::from_str(&existing.announcements_json)?;
            let mut existing_tags: Vec<Vec<String>> =
                serde_json::from_str(&existing.tags_json).unwrap_or_default();
            existing_announcements.sort();
            existing_tags.sort();
            let mut request_announcements = request.announcements.clone();
            let mut request_tags = request.tags.clone();
            request_announcements.sort();
            request_tags.sort();
            if existing.threshold != request.threshold
                || existing_tags != request_tags
                || existing.condition_type != request.condition_type
                || existing_announcements != request_announcements
                || existing.lo_bound != request.lo_bound
                || existing.hi_bound != request.hi_bound
                || existing.precision != request.precision
                || existing.collateral != collateral_unit
            {
                return Err(Error::ConditionAlreadyExists);
            }

            let existing_keysets = self
                .localstore
                .get_conditional_keysets_for_condition(&condition_id)
                .await?;
            let existing_set: HashSet<String> = existing_keysets.keys().cloned().collect();
            let requested_set: HashSet<String> = requested_collections.iter().cloned().collect();
            if existing_set != requested_set {
                return Err(Error::ConditionAlreadyExists);
            }

            return Ok(RegisterConditionResponse {
                condition_id,
                keysets: existing_keysets,
                change: None,
            });
        }

        if !requested_collections.is_empty() || required_fee > 0 {
            let collateral = request.collateral.as_deref().ok_or_else(|| {
                Error::Custom(
                    "collateral is required when creating keysets or paying registration fees"
                        .to_string(),
                )
            })?;
            if collateral_unit.is_none() {
                return Err(Error::Custom(format!(
                    "Invalid collateral unit: {}",
                    collateral
                )));
            }
        }

        let fee_verification = if required_fee > 0 {
            let collateral = request
                .collateral
                .as_deref()
                .ok_or(Error::RegistrationFeeInsufficient)?;
            Some(
                self.verify_registration_fee(
                    request.fee.as_ref(),
                    request.outputs.as_deref(),
                    collateral,
                    required_fee,
                )
                .await?,
            )
        } else {
            None
        };

        // 5. Store the condition
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let stored = StoredCondition {
            condition_id: condition_id.clone(),
            threshold: request.threshold,
            tags_json,
            announcements_json: serde_json::to_string(&request.announcements)?,
            collateral: collateral_unit,
            attestation_status: STATUS_PENDING.to_string(),
            winning_outcome: None,
            attested_at: None,
            created_at: now,
            condition_type: request.condition_type.clone(),
            lo_bound: request.lo_bound,
            hi_bound: request.hi_bound,
            precision: request.precision,
        };

        let prepared_keysets = self
            .prepare_condition_keysets(
                &condition_id,
                &condition_id_bytes,
                &requested_collections,
                request.collateral.as_deref(),
            )
            .await?;
        let keysets = prepared_keysets
            .iter()
            .map(|(collection, prepared)| (collection.clone(), prepared.keyset.id))
            .collect::<HashMap<_, _>>();

        let mut tx = self.localstore.begin_transaction().await?;
        if let Some(fee_verification) = &fee_verification {
            let operation = cdk_common::mint::Operation::new(
                uuid::Uuid::new_v4(),
                cdk_common::mint::OperationKind::Swap,
                cdk_common::Amount::ZERO,
                fee_verification.amount.clone().into(),
                fee_verification.amount.clone().into(),
                None,
                None,
            );
            let mut fee_records = match tx
                .add_proofs(fee_verification.proofs.clone(), None, &operation)
                .await
            {
                Ok(records) => records,
                Err(err) => {
                    tx.rollback().await?;
                    return Err(err.into());
                }
            };
            if let Err(err) = crate::Mint::update_proofs_state(
                &mut tx,
                &mut fee_records,
                cdk_common::State::Spent,
            )
            .await
            {
                tx.rollback().await?;
                return Err(err);
            }
            if !fee_verification.change_messages.is_empty() {
                if let Err(err) = tx
                    .add_blinded_messages(None, &fee_verification.change_messages, &operation)
                    .await
                {
                    tx.rollback().await?;
                    return Err(err.into());
                }
                if let Err(err) = tx
                    .add_blind_signatures(
                        &fee_verification.change_blinded_secrets,
                        &fee_verification.change,
                        None,
                    )
                    .await
                {
                    tx.rollback().await?;
                    return Err(err.into());
                }
            }
        }
        if let Err(err) = tx.add_condition(stored).await {
            tx.rollback().await?;
            return Err(err.into());
        }
        for (_, prepared) in prepared_keysets {
            if let Err(err) = tx.add_conditional_keyset(prepared.info, now).await {
                tx.rollback().await?;
                return Err(err.into());
            }
        }
        tx.commit().await?;

        self.signatory.reload_keysets_from_storage().await?;
        let new_keysets = self.signatory.keysets().await?;
        self.keysets.store(new_keysets.keysets.into());

        Ok(RegisterConditionResponse {
            condition_id,
            keysets,
            change: fee_verification.and_then(|verification| {
                (!verification.change.is_empty()).then_some(verification.change)
            }),
        })
    }

    async fn required_registration_fee(&self, num_keysets: usize) -> Result<u64, Error> {
        let settings = self.mint_info().await?.nuts.nut_ctf.unwrap_or_default();
        let per_keyset = settings
            .registration_fee_per_keyset
            .checked_mul(num_keysets as u64)
            .ok_or(Error::AmountOverflow)?;
        settings
            .registration_fee_base
            .checked_add(per_keyset)
            .ok_or(Error::AmountOverflow)
    }

    async fn verify_registration_fee(
        &self,
        fee: Option<&cdk_common::Proofs>,
        outputs: Option<&[BlindedMessage]>,
        collateral: &str,
        required_fee: u64,
    ) -> Result<RegistrationFeeVerification, Error> {
        let fee = fee.ok_or(Error::RegistrationFeeInsufficient)?;
        if fee.is_empty() {
            return Err(Error::RegistrationFeeInsufficient);
        }

        let collateral_unit = cdk_common::CurrencyUnit::from_str(collateral)
            .map_err(|_| Error::Custom(format!("Invalid collateral unit: {}", collateral)))?;

        for proof in fee {
            if self
                .localstore
                .get_condition_for_keyset(&proof.keyset_id)
                .await?
                .is_some()
            {
                return Err(Error::OutputsMustUseRegularKeyset);
            }
            let keyset_info = self
                .get_keyset_info(&proof.keyset_id)
                .ok_or(Error::UnknownKeySet)?;
            if keyset_info.unit != collateral_unit {
                return Err(Error::OutputsMustUseRegularKeyset);
            }
        }

        let verification = self.verify_inputs(fee).await?;
        if verification.amount.value() < required_fee {
            return Err(Error::RegistrationFeeInsufficient);
        }

        let change_amount = verification
            .amount
            .value()
            .checked_sub(required_fee)
            .ok_or(Error::AmountOverflow)?;
        let (change_messages, change_blinded_secrets, change) = if change_amount > 0 {
            self.sign_registration_fee_change(outputs, collateral_unit, change_amount)
                .await?
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };

        Ok(RegistrationFeeVerification {
            proofs: fee.clone(),
            amount: verification.amount,
            change_messages,
            change_blinded_secrets,
            change,
        })
    }

    async fn sign_registration_fee_change(
        &self,
        outputs: Option<&[BlindedMessage]>,
        collateral_unit: cdk_common::CurrencyUnit,
        change_amount: u64,
    ) -> Result<
        (
            Vec<BlindedMessage>,
            Vec<cdk_common::PublicKey>,
            Vec<BlindSignature>,
        ),
        Error,
    > {
        let outputs = outputs.ok_or(Error::RegistrationFeeChangeOutputs)?;
        if outputs.is_empty() {
            return Err(Error::RegistrationFeeChangeOutputs);
        }
        if outputs.len() > self.max_outputs {
            return Err(Error::RegistrationFeeChangeOutputs);
        }
        Mint::check_outputs_unique(outputs).map_err(|_| Error::RegistrationFeeChangeOutputs)?;
        let output_unit = self.verify_outputs_keyset(outputs)?;
        if output_unit != collateral_unit {
            return Err(Error::OutputsMustUseRegularKeyset);
        }
        for output in outputs {
            if self
                .localstore
                .get_condition_for_keyset(&output.keyset_id)
                .await?
                .is_some()
            {
                return Err(Error::OutputsMustUseRegularKeyset);
            }
        }

        let fee_and_amounts =
            super::melt::shared::get_keyset_fee_and_amounts(&self.keysets, outputs);
        let amounts = cdk_common::Amount::from(change_amount)
            .split(&fee_and_amounts)
            .map_err(|_| Error::RegistrationFeeChangeOutputs)?;
        if outputs.len() < amounts.len() {
            return Err(Error::RegistrationFeeChangeOutputs);
        }

        let change_messages = amounts
            .iter()
            .zip(outputs.iter().cloned())
            .map(|(amount, mut output)| {
                output.amount = *amount;
                output
            })
            .collect::<Vec<_>>();
        let change_blinded_secrets = change_messages
            .iter()
            .map(|message| message.blinded_secret)
            .collect::<Vec<_>>();
        let change = self.blind_sign(change_messages.clone()).await?;

        Ok((change_messages, change_blinded_secrets, change))
    }

    fn requested_outcome_collections(
        &self,
        outcomes: &[String],
        request: &RegisterConditionRequest,
        is_numeric: bool,
        default_keyset_creation: &str,
    ) -> Result<Vec<String>, Error> {
        if request.outcome_collections.is_some()
            && matches!(
                default_keyset_creation,
                KEYSET_POLICY_ONE_VS_REST | KEYSET_POLICY_ALL
            )
        {
            return Err(Error::Custom(format!(
                "outcome_collections must be omitted when default_keyset_creation is {}",
                default_keyset_creation
            )));
        };

        let raw = match (is_numeric, request.outcome_collections.as_ref()) {
            (true, Some(collections)) => collections.clone(),
            (true, None) => vec!["HI".to_string(), "LO".to_string()],
            (false, Some(collections)) => collections.clone(),
            (false, None) => self.default_outcome_collections(outcomes, default_keyset_creation)?,
        };

        if raw.len() > MAX_OUTCOME_COLLECTIONS {
            return Err(Error::Custom(format!(
                "Outcome collections exceed maximum of {}",
                MAX_OUTCOME_COLLECTIONS
            )));
        }

        let mut canonical = Vec::with_capacity(raw.len());
        let mut seen = HashSet::with_capacity(raw.len());
        for key in raw {
            let members = parse_outcome_collection(&key);
            let collection =
                canonical_outcome_collection(outcomes, &members).map_err(Error::from)?;
            if !seen.insert(collection.clone()) {
                return Err(Error::OverlappingOutcomeCollections);
            }
            canonical.push(collection);
        }

        if is_numeric {
            let expected: HashSet<String> =
                ["HI".to_string(), "LO".to_string()].into_iter().collect();
            let actual: HashSet<String> = canonical.iter().cloned().collect();
            if actual != expected {
                return Err(Error::Custom(
                    "Numeric conditions only support HI and LO outcome collections".to_string(),
                ));
            }
        }

        Ok(canonical)
    }

    async fn default_keyset_creation_policy(&self) -> Result<String, Error> {
        let policy = self
            .mint_info()
            .await?
            .nuts
            .nut_ctf
            .map(|settings| settings.default_keyset_creation)
            .unwrap_or_else(|| KEYSET_POLICY_NONE.to_string());

        match policy.as_str() {
            KEYSET_POLICY_NONE | KEYSET_POLICY_ONE_VS_REST | KEYSET_POLICY_ALL => Ok(policy),
            _ => Err(Error::Custom(format!(
                "Unsupported default_keyset_creation policy: {}",
                policy
            ))),
        }
    }

    fn default_outcome_collections(
        &self,
        outcomes: &[String],
        policy: &str,
    ) -> Result<Vec<String>, Error> {
        match policy {
            KEYSET_POLICY_NONE => Ok(Vec::new()),
            KEYSET_POLICY_ONE_VS_REST => self.one_vs_rest_collections(outcomes),
            KEYSET_POLICY_ALL => self.all_non_full_collections(outcomes),
            _ => Err(Error::Custom(format!(
                "Unsupported default_keyset_creation policy: {}",
                policy
            ))),
        }
    }

    fn one_vs_rest_collections(&self, outcomes: &[String]) -> Result<Vec<String>, Error> {
        let mut collections = Vec::with_capacity(outcomes.len().saturating_mul(2));
        let mut seen = HashSet::new();

        for outcome in outcomes {
            let singleton = canonical_outcome_collection(outcomes, std::slice::from_ref(outcome))
                .map_err(Error::from)?;
            if seen.insert(singleton.clone()) {
                collections.push(singleton);
            }

            let complement_members = outcomes
                .iter()
                .filter(|candidate| *candidate != outcome)
                .cloned()
                .collect::<Vec<_>>();
            if complement_members.is_empty() {
                continue;
            }
            let complement =
                canonical_outcome_collection(outcomes, &complement_members).map_err(Error::from)?;
            if seen.insert(complement.clone()) {
                collections.push(complement);
            }
        }

        if collections.len() > MAX_OUTCOME_COLLECTIONS {
            return Err(Error::Custom(format!(
                "default_keyset_creation one-vs-rest expands to {} outcome collections, exceeding maximum of {}",
                collections.len(),
                MAX_OUTCOME_COLLECTIONS
            )));
        }

        Ok(collections)
    }

    fn all_non_full_collections(&self, outcomes: &[String]) -> Result<Vec<String>, Error> {
        if outcomes.len() >= usize::BITS as usize {
            return Err(Error::Custom(
                "default_keyset_creation all exceeds platform subset capacity".to_string(),
            ));
        }

        let count = (1usize << outcomes.len()).saturating_sub(2);
        if count > MAX_OUTCOME_COLLECTIONS {
            return Err(Error::Custom(format!(
                "default_keyset_creation all expands to {} outcome collections, exceeding maximum of {}",
                count,
                MAX_OUTCOME_COLLECTIONS
            )));
        }

        let mut collections = Vec::with_capacity(count);
        for mask in 1usize..((1usize << outcomes.len()) - 1) {
            let members = outcomes
                .iter()
                .enumerate()
                .filter_map(|(index, outcome)| {
                    if mask & (1usize << index) != 0 {
                        Some(outcome.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            collections
                .push(canonical_outcome_collection(outcomes, &members).map_err(Error::from)?);
        }

        Ok(collections)
    }

    async fn prepare_condition_keysets(
        &self,
        condition_id: &str,
        condition_id_bytes: &[u8; 32],
        outcome_collections: &[String],
        collateral: Option<&str>,
    ) -> Result<Vec<(String, cdk_signatory::signatory::PreparedConditionalKeySet)>, Error> {
        if outcome_collections.is_empty() {
            return Ok(Vec::new());
        }

        let collateral = collateral.ok_or_else(|| {
            Error::Custom(
                "collateral is required when creating outcome collection keysets".to_string(),
            )
        })?;
        let unit = cdk_common::CurrencyUnit::from_str(collateral)
            .map_err(|_| Error::Custom(format!("Invalid collateral unit: {}", collateral)))?;
        let parent_collection_id_bytes = [0u8; 32];
        let amounts = (0..32).map(|n| 2u64.pow(n)).collect::<Vec<u64>>();
        let mut keysets = Vec::with_capacity(outcome_collections.len());

        for outcome_collection_string in outcome_collections {
            let outcome_collection_id_bytes = compute_outcome_collection_id(
                &parent_collection_id_bytes,
                condition_id_bytes,
                outcome_collection_string,
            )?;
            let outcome_collection_id = to_hex(&outcome_collection_id_bytes);

            let keyset = self
                .signatory
                .prepare_conditional_keyset(
                    unit.clone(),
                    condition_id,
                    outcome_collection_string,
                    &outcome_collection_id,
                    amounts.clone(),
                    1,
                    None,
                )
                .await?;

            keysets.push((outcome_collection_string.clone(), keyset));
        }

        Ok(keysets)
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

        // TODO: N+1 query — build_condition_info loads keysets per condition.
        // Batch when condition count grows.
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

    /// Build a ConditionInfo from a StoredCondition, including keysets
    async fn build_condition_info(
        &self,
        condition: StoredCondition,
    ) -> Result<ConditionInfo, Error> {
        let announcements: Vec<String> = serde_json::from_str(&condition.announcements_json)?;

        let keysets = self
            .localstore
            .get_conditional_keysets_for_condition(&condition.condition_id)
            .await?;

        Ok(ConditionInfo {
            condition_id: condition.condition_id,
            threshold: condition.threshold,
            tags: serde_json::from_str(&condition.tags_json).unwrap_or_default(),
            announcements,
            collateral: condition.collateral,
            keysets,
            attestation: Some(AttestationState {
                status: match condition.attestation_status.as_str() {
                    STATUS_ATTESTED => AttestationStatus::Attested,
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
