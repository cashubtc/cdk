//! Consensus materialization for federated wallet address allocation.
//!
//! The ordered operation contains an allocation intent, never a member-local
//! cursor. The accepted journal order assigns the next index so concurrent
//! proposers cannot advance a local BDK wallet before consensus.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    decode_row, digest_row_key, insert_row, row_kind, unknown_row_kind, FederationError,
    FederationFrostApplicationId, FederationOperationId, FederationStateRows, FederationStateTable,
    Result,
};

const MAX_QUOTE_ID_BYTES: usize = 256;

/// Largest non-hardened BIP32 child index accepted for descriptor allocation.
pub const FEDERATION_MAX_DERIVATION_INDEX: u32 = (1 << 31) - 1;

/// Descriptor branch controlled by one allocation counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationAddressKeychain {
    /// Incoming/deposit address branch.
    External,
    /// Change/internal address branch.
    Internal,
}

impl fmt::Display for FederationAddressKeychain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::External => formatter.write_str("external"),
            Self::Internal => formatter.write_str("internal"),
        }
    }
}

/// Canonical consensus intent to allocate one deterministic address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAddressAllocationOperation {
    /// Mint quote or other durable application allocation key.
    pub quote: String,
    /// Registered CDK on-chain application.
    pub application_id: FederationFrostApplicationId,
    /// Exact application key epoch whose descriptors derive the address.
    pub application_key_epoch: u64,
    /// Independent descriptor branch to advance after consensus.
    pub keychain: FederationAddressKeychain,
}

impl FederationAddressAllocationOperation {
    /// Construct a bounded allocation intent.
    pub fn new(
        quote: impl Into<String>,
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
        keychain: FederationAddressKeychain,
    ) -> Result<Self> {
        let operation = Self {
            quote: quote.into(),
            application_id,
            application_key_epoch,
            keychain,
        };
        operation.validate()?;
        Ok(operation)
    }

    /// Validate the canonical allocation key and epoch bounds.
    pub fn validate(&self) -> Result<()> {
        if self.quote.is_empty()
            || self.quote.len() > MAX_QUOTE_ID_BYTES
            || self.application_key_epoch == 0
        {
            return Err(FederationError::AddressAllocationInvalid {
                reason:
                    "quote must be non-empty and bounded and application epoch must be non-zero",
            });
        }
        Ok(())
    }

    /// Immutable state scope selected by this operation.
    #[must_use]
    pub fn scope(&self) -> FederationAddressAllocationScope {
        FederationAddressAllocationScope {
            application_id: self.application_id,
            application_key_epoch: self.application_key_epoch,
        }
    }
}

/// Application/epoch namespace for independent allocation counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationAddressAllocationScope {
    /// Registered CDK on-chain application.
    pub application_id: FederationFrostApplicationId,
    /// Descriptor application key epoch.
    pub application_key_epoch: u64,
}

impl fmt::Display for FederationAddressAllocationScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}",
            self.application_id, self.application_key_epoch
        )
    }
}

impl FromStr for FederationAddressAllocationScope {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        let (application_id, application_key_epoch) = value.split_once(':').ok_or_else(|| {
            invalid_state("address-allocation scope must contain application id and epoch")
        })?;
        let application_id = FederationFrostApplicationId::from_str(application_id)?;
        let application_key_epoch = application_key_epoch
            .parse::<u64>()
            .map_err(|_| invalid_state("address-allocation scope epoch is invalid"))?;
        if application_key_epoch == 0 {
            return Err(invalid_state(
                "address-allocation scope epoch must be non-zero",
            ));
        }
        Ok(Self {
            application_id,
            application_key_epoch,
        })
    }
}

impl Serialize for FederationAddressAllocationScope {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for FederationAddressAllocationScope {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(serde::de::Error::custom)
    }
}

/// One accepted quote-to-index mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAddressAllocation {
    /// Quote or durable application allocation key.
    pub quote: String,
    /// Descriptor branch containing the assigned index.
    pub keychain: FederationAddressKeychain,
    /// Deterministically assigned non-hardened child index.
    pub derivation_index: u32,
    /// First accepted operation that established this mapping.
    pub operation_id: FederationOperationId,
    /// Consensus journal position of the establishing operation.
    pub operation_index: u64,
}

/// Result of applying an accepted allocation intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederationAddressAllocationOutcome {
    /// Consensus created a new quote mapping.
    Allocated(FederationAddressAllocation),
    /// A duplicate or replay returned the immutable existing mapping.
    Existing(FederationAddressAllocation),
}

impl FederationAddressAllocationOutcome {
    /// Borrow the accepted allocation for either idempotent outcome.
    #[must_use]
    pub fn allocation(&self) -> &FederationAddressAllocation {
        match self {
            Self::Allocated(allocation) | Self::Existing(allocation) => allocation,
        }
    }
}

/// Per-application materialized counters and mappings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAddressWalletState {
    /// Next external child index, or max-plus-one after exhaustion.
    pub external_next_index: u64,
    /// Next internal child index, or max-plus-one after exhaustion.
    pub internal_next_index: u64,
    /// Immutable mappings keyed by quote/allocation key.
    pub allocations: BTreeMap<String, FederationAddressAllocation>,
}

/// Complete canonical address-allocation projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationAddressAllocationState {
    wallets: BTreeMap<FederationAddressAllocationScope, FederationAddressWalletState>,
}

impl Default for FederationAddressAllocationState {
    fn default() -> Self {
        Self::new()
    }
}

impl FederationAddressAllocationState {
    /// Construct an empty current-version projection.
    #[must_use]
    pub fn new() -> Self {
        Self {
            wallets: BTreeMap::new(),
        }
    }

    /// Apply one accepted operation in journal order.
    pub fn apply(
        &mut self,
        operation: &FederationAddressAllocationOperation,
        operation_id: FederationOperationId,
        operation_index: u64,
    ) -> Result<FederationAddressAllocationOutcome> {
        operation.validate()?;
        self.validate()?;
        let wallet = self.wallets.entry(operation.scope()).or_default();
        if let Some(existing) = wallet.allocations.get(&operation.quote) {
            if existing.keychain != operation.keychain {
                return Err(FederationError::AddressAllocationConflict {
                    quote: operation.quote.clone(),
                });
            }
            return Ok(FederationAddressAllocationOutcome::Existing(
                existing.clone(),
            ));
        }

        let next_index = match operation.keychain {
            FederationAddressKeychain::External => wallet.external_next_index,
            FederationAddressKeychain::Internal => wallet.internal_next_index,
        };
        let derivation_index = checked_derivation_index(next_index, operation.keychain)?;
        let allocation = FederationAddressAllocation {
            quote: operation.quote.clone(),
            keychain: operation.keychain,
            derivation_index,
            operation_id,
            operation_index,
        };
        wallet
            .allocations
            .insert(operation.quote.clone(), allocation.clone());
        match operation.keychain {
            FederationAddressKeychain::External => {
                wallet.external_next_index = next_index + 1;
            }
            FederationAddressKeychain::Internal => {
                wallet.internal_next_index = next_index + 1;
            }
        }
        self.validate()?;
        Ok(FederationAddressAllocationOutcome::Allocated(allocation))
    }

    /// Look up one accepted mapping by application epoch and quote.
    #[must_use]
    pub fn allocation(
        &self,
        scope: FederationAddressAllocationScope,
        quote: &str,
    ) -> Option<&FederationAddressAllocation> {
        self.wallets
            .get(&scope)
            .and_then(|wallet| wallet.allocations.get(quote))
    }

    /// Return one application/epoch state when materialized.
    #[must_use]
    pub fn wallet(
        &self,
        scope: FederationAddressAllocationScope,
    ) -> Option<&FederationAddressWalletState> {
        self.wallets.get(&scope)
    }

    /// Iterate over materialized application wallets in canonical scope order.
    pub fn wallets(
        &self,
    ) -> impl Iterator<
        Item = (
            FederationAddressAllocationScope,
            &FederationAddressWalletState,
        ),
    > {
        self.wallets.iter().map(|(scope, wallet)| (*scope, wallet))
    }

    /// Validate counters, uniqueness, and contiguous allocation invariants.
    pub fn validate(&self) -> Result<()> {
        let max_next = u64::from(FEDERATION_MAX_DERIVATION_INDEX) + 1;
        for (scope, wallet) in &self.wallets {
            if scope.application_key_epoch == 0
                || wallet.external_next_index > max_next
                || wallet.internal_next_index > max_next
            {
                return Err(invalid_state(
                    "invalid application scope or next-index counter",
                ));
            }
            let mut external = BTreeSet::new();
            let mut internal = BTreeSet::new();
            for (quote, allocation) in &wallet.allocations {
                if quote != &allocation.quote
                    || quote.is_empty()
                    || quote.len() > MAX_QUOTE_ID_BYTES
                    || allocation.derivation_index > FEDERATION_MAX_DERIVATION_INDEX
                {
                    return Err(invalid_state("invalid quote allocation record"));
                }
                let indexes = match allocation.keychain {
                    FederationAddressKeychain::External => &mut external,
                    FederationAddressKeychain::Internal => &mut internal,
                };
                if !indexes.insert(allocation.derivation_index) {
                    return Err(invalid_state("derivation index is assigned more than once"));
                }
            }
            validate_contiguous_indexes(&external, wallet.external_next_index)?;
            validate_contiguous_indexes(&internal, wallet.internal_next_index)?;
        }
        Ok(())
    }
}

/// Stored form of one allocation: its scope and the allocation itself. The
/// per-scope counters are not stored; they are the allocation counts.
#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct AllocationRowRef<'a> {
    scope: FederationAddressAllocationScope,
    allocation: &'a FederationAddressAllocation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AllocationRow {
    scope: FederationAddressAllocationScope,
    allocation: FederationAddressAllocation,
}

const ALLOCATION_ROW: &str = "allocation";

fn allocation_row_key(scope: FederationAddressAllocationScope, quote: &str) -> String {
    digest_row_key(
        ALLOCATION_ROW,
        &[scope.to_string().as_bytes(), quote.as_bytes()],
    )
}

impl FederationStateTable for FederationAddressAllocationState {
    const SECONDARY_NAMESPACE: &'static str = "address_allocation";

    fn to_rows(&self) -> Result<FederationStateRows> {
        self.validate()?;
        let mut rows = FederationStateRows::new();
        for (scope, wallet) in &self.wallets {
            for allocation in wallet.allocations.values() {
                insert_row(
                    &mut rows,
                    allocation_row_key(*scope, &allocation.quote),
                    &AllocationRowRef {
                        scope: *scope,
                        allocation,
                    },
                )?;
            }
        }
        Ok(rows)
    }

    fn from_rows(rows: &FederationStateRows) -> Result<Self> {
        let mut state = Self::new();
        for (key, bytes) in rows {
            if row_kind(key)? != ALLOCATION_ROW {
                return Err(unknown_row_kind());
            }
            let row: AllocationRow = decode_row(key, bytes, |row: &AllocationRow| {
                Ok(allocation_row_key(row.scope, &row.allocation.quote))
            })?;
            let wallet = state.wallets.entry(row.scope).or_default();
            match row.allocation.keychain {
                FederationAddressKeychain::External => wallet.external_next_index += 1,
                FederationAddressKeychain::Internal => wallet.internal_next_index += 1,
            }
            wallet
                .allocations
                .insert(row.allocation.quote.clone(), row.allocation);
        }
        state.validate()?;
        Ok(state)
    }
}

fn checked_derivation_index(next_index: u64, keychain: FederationAddressKeychain) -> Result<u32> {
    if next_index > u64::from(FEDERATION_MAX_DERIVATION_INDEX) {
        return Err(FederationError::AddressDerivationExhausted { keychain });
    }
    u32::try_from(next_index).map_err(|_| FederationError::AddressDerivationExhausted { keychain })
}

fn validate_contiguous_indexes(indexes: &BTreeSet<u32>, next_index: u64) -> Result<()> {
    if indexes.len() as u64 != next_index
        || indexes
            .iter()
            .copied()
            .zip(0_u32..)
            .any(|(found, expected)| found != expected)
    {
        return Err(invalid_state(
            "allocation indexes must be unique and contiguous below the next counter",
        ));
    }
    Ok(())
}

fn invalid_state(reason: &'static str) -> FederationError {
    FederationError::AddressAllocationInvalid { reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::{
        FederationId, FederationOperation, FederationOperationConflictKey,
        FederationOperationEnvelope, FEDERATION_OPERATION_VERSION,
    };

    fn application() -> FederationFrostApplicationId {
        FederationFrostApplicationId::from_bytes([7; 32]).expect("application")
    }

    fn operation(
        quote: &str,
        keychain: FederationAddressKeychain,
    ) -> FederationAddressAllocationOperation {
        FederationAddressAllocationOperation::new(quote, application(), 3, keychain)
            .expect("allocation operation")
    }

    #[test]
    fn sequential_duplicate_and_keychain_separated_allocations() {
        let mut state = FederationAddressAllocationState::new();
        let first = state
            .apply(
                &operation("quote-a", FederationAddressKeychain::External),
                FederationOperationId::from_bytes([1; 32]),
                4,
            )
            .expect("first allocation");
        let second = state
            .apply(
                &operation("quote-b", FederationAddressKeychain::External),
                FederationOperationId::from_bytes([2; 32]),
                5,
            )
            .expect("second allocation");
        let internal = state
            .apply(
                &operation("change-a", FederationAddressKeychain::Internal),
                FederationOperationId::from_bytes([3; 32]),
                6,
            )
            .expect("internal allocation");
        let duplicate = state
            .apply(
                &operation("quote-a", FederationAddressKeychain::External),
                FederationOperationId::from_bytes([9; 32]),
                9,
            )
            .expect("duplicate allocation");

        assert_eq!(first.allocation().derivation_index, 0);
        assert_eq!(second.allocation().derivation_index, 1);
        assert_eq!(internal.allocation().derivation_index, 0);
        assert!(matches!(
            duplicate,
            FederationAddressAllocationOutcome::Existing(_)
        ));
        let wallet = state
            .wallet(operation("quote-a", FederationAddressKeychain::External).scope())
            .expect("wallet state");
        assert_eq!(wallet.external_next_index, 2);
        assert_eq!(wallet.internal_next_index, 1);
    }

    #[test]
    fn remapping_quote_to_other_keychain_is_rejected() {
        let mut state = FederationAddressAllocationState::new();
        state
            .apply(
                &operation("quote", FederationAddressKeychain::External),
                FederationOperationId::from_bytes([1; 32]),
                0,
            )
            .expect("allocation");
        assert!(matches!(
            state.apply(
                &operation("quote", FederationAddressKeychain::Internal),
                FederationOperationId::from_bytes([2; 32]),
                1
            ),
            Err(FederationError::AddressAllocationConflict { .. })
        ));
    }

    #[test]
    fn corrupted_reused_or_skipped_indexes_fail_closed() {
        let mut state = FederationAddressAllocationState::new();
        for (index, quote) in ["a", "b"].into_iter().enumerate() {
            state
                .apply(
                    &operation(quote, FederationAddressKeychain::External),
                    FederationOperationId::from_bytes([index as u8 + 1; 32]),
                    index as u64,
                )
                .expect("allocation");
        }
        let scope = operation("a", FederationAddressKeychain::External).scope();
        let wallet = state.wallets.get_mut(&scope).expect("wallet");
        wallet.allocations.get_mut("b").expect("b").derivation_index = 0;
        assert!(state.validate().is_err());

        let mut skipped = FederationAddressAllocationState::new();
        skipped.wallets.insert(
            scope,
            FederationAddressWalletState {
                external_next_index: 1,
                internal_next_index: 0,
                allocations: BTreeMap::new(),
            },
        );
        assert!(skipped.validate().is_err());
    }

    #[test]
    fn maximum_index_and_exhaustion_are_explicit() {
        assert!(FederationAddressAllocationOperation::new(
            "",
            application(),
            3,
            FederationAddressKeychain::External,
        )
        .is_err());
        assert_eq!(
            checked_derivation_index(
                u64::from(FEDERATION_MAX_DERIVATION_INDEX),
                FederationAddressKeychain::External,
            )
            .expect("maximum index"),
            FEDERATION_MAX_DERIVATION_INDEX
        );
        assert!(matches!(
            checked_derivation_index(
                u64::from(FEDERATION_MAX_DERIVATION_INDEX) + 1,
                FederationAddressKeychain::Internal,
            ),
            Err(FederationError::AddressDerivationExhausted {
                keychain: FederationAddressKeychain::Internal
            })
        ));
    }

    #[test]
    fn serialization_and_materialized_rows_preserve_all_state() {
        let mut state = FederationAddressAllocationState::new();
        state
            .apply(
                &operation("quote", FederationAddressKeychain::External),
                FederationOperationId::from_bytes([1; 32]),
                11,
            )
            .expect("allocation");
        let restored =
            FederationAddressAllocationState::from_rows(&state.to_rows().expect("serialize"))
                .expect("restore");
        assert_eq!(restored, state);
        // One row per allocation; the next-index counters are rebuilt on load.
        assert_eq!(restored.materialized_rows().expect("rows").len(), 1);
    }

    #[test]
    fn operation_envelope_is_versioned_canonical_and_quote_scoped() {
        let envelope = FederationOperationEnvelope::new(
            FederationId::from_bytes([4; 32]),
            FederationOperation::AddressAllocation(operation(
                "quote",
                FederationAddressKeychain::External,
            )),
        );
        assert_eq!(envelope.version, FEDERATION_OPERATION_VERSION);
        let bytes = envelope.canonical_bytes().expect("canonical envelope");
        let decoded = FederationOperationEnvelope::from_json_slice(&bytes).expect("round trip");
        assert_eq!(
            decoded.operation_id().expect("operation id"),
            envelope.operation_id().expect("operation id")
        );
        assert!(matches!(
            decoded.operation.conflict_keys().as_slice(),
            [FederationOperationConflictKey::AddressAllocation {
                application_key_epoch: 3,
                quote,
                ..
            }] if quote == "quote"
        ));
    }
}
