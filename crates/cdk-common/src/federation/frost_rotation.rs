//! Consensus-safe lifecycle transitions for reusable FROST roots and applications.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    FederationConfig, FederationError, FederationFrostApplicationConfig,
    FederationFrostApplicationId, FederationFrostConsumerRegistration, FederationFrostEpochState,
    FederationFrostRootPublicConfig, FederationFrostWalletConfig, Result,
};

const ROTATION_OPERATION_VERSION: u16 = 1;

/// One consensus transition in the reusable-root/application lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostRotationOperation {
    /// Rotation operation schema version.
    pub version: u16,
    /// Exact lifecycle transition requested after consensus acceptance.
    pub transition: FederationFrostRotationTransition,
}

impl FederationFrostRotationOperation {
    /// Propose a root public package produced by a completed DKG ceremony.
    #[must_use]
    pub fn propose_root(root: FederationFrostRootPublicConfig) -> Self {
        Self {
            version: ROTATION_OPERATION_VERSION,
            transition: FederationFrostRotationTransition::ProposeRoot { root },
        }
    }

    /// Propose a derived application epoch and register its root usage.
    #[must_use]
    pub fn propose_application(
        application: FederationFrostApplicationConfig,
        consumer: FederationFrostConsumerRegistration,
    ) -> Self {
        Self {
            version: ROTATION_OPERATION_VERSION,
            transition: FederationFrostRotationTransition::ProposeApplication {
                application,
                consumer,
            },
        }
    }

    /// Register a non-CDK consumer of a reusable root epoch.
    #[must_use]
    pub fn register_consumer(registration: FederationFrostConsumerRegistration) -> Self {
        Self {
            version: ROTATION_OPERATION_VERSION,
            transition: FederationFrostRotationTransition::RegisterConsumer { registration },
        }
    }

    /// Activate a proposed root for subsequently registered applications.
    #[must_use]
    pub fn activate_root(root_key_epoch: u64) -> Self {
        Self {
            version: ROTATION_OPERATION_VERSION,
            transition: FederationFrostRotationTransition::ActivateRoot { root_key_epoch },
        }
    }

    /// Activate a proposed application for new addresses and payment intents.
    #[must_use]
    pub fn activate_application(
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
    ) -> Self {
        Self {
            version: ROTATION_OPERATION_VERSION,
            transition: FederationFrostRotationTransition::ActivateApplication {
                application_id,
                application_key_epoch,
            },
        }
    }

    /// Complete a non-CDK consumer's reviewed root-retirement contract.
    #[must_use]
    pub fn complete_consumer_retirement(registration: FederationFrostConsumerRegistration) -> Self {
        Self {
            version: ROTATION_OPERATION_VERSION,
            transition: FederationFrostRotationTransition::CompleteConsumerRetirement {
                registration,
            },
        }
    }

    /// Retire a drained CDK application epoch.
    #[must_use]
    pub fn retire_application(
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
    ) -> Self {
        Self {
            version: ROTATION_OPERATION_VERSION,
            transition: FederationFrostRotationTransition::RetireApplication {
                application_id,
                application_key_epoch,
            },
        }
    }

    /// Retire a root after every registered consumer has completed retirement.
    #[must_use]
    pub fn retire_root(root_key_epoch: u64) -> Self {
        Self {
            version: ROTATION_OPERATION_VERSION,
            transition: FederationFrostRotationTransition::RetireRoot { root_key_epoch },
        }
    }

    /// Validate only bounded operation-local structure.
    pub fn validate(&self) -> Result<()> {
        if self.version != ROTATION_OPERATION_VERSION {
            return Err(invalid("unsupported FROST rotation operation version"));
        }
        match &self.transition {
            FederationFrostRotationTransition::ProposeRoot { root } => {
                if root.state != FederationFrostEpochState::Proposed {
                    return Err(invalid("proposed root must carry proposed lifecycle state"));
                }
            }
            FederationFrostRotationTransition::ProposeApplication {
                application,
                consumer,
            } => {
                if application.state != FederationFrostEpochState::Proposed
                    || application.root_key_epoch != consumer.root_key_epoch
                    || application.application_id != consumer.application_id
                    || application.application_key_epoch != consumer.application_key_epoch
                {
                    return Err(invalid(
                        "proposed application and consumer registration do not match",
                    ));
                }
            }
            FederationFrostRotationTransition::RegisterConsumer { registration }
            | FederationFrostRotationTransition::CompleteConsumerRetirement { registration } => {
                if registration.root_key_epoch == 0 || registration.application_key_epoch == 0 {
                    return Err(invalid("consumer registration contains a zero epoch"));
                }
            }
            FederationFrostRotationTransition::ActivateRoot { root_key_epoch }
            | FederationFrostRotationTransition::RetireRoot { root_key_epoch } => {
                if *root_key_epoch == 0 {
                    return Err(invalid("root rotation epoch is zero"));
                }
            }
            FederationFrostRotationTransition::ActivateApplication {
                application_key_epoch,
                ..
            }
            | FederationFrostRotationTransition::RetireApplication {
                application_key_epoch,
                ..
            } => {
                if *application_key_epoch == 0 {
                    return Err(invalid("application rotation epoch is zero"));
                }
            }
        }
        Ok(())
    }
}

/// Versioned root/application registry mutation accepted through consensus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum FederationFrostRotationTransition {
    /// Add a validated DKG root without activating it.
    ProposeRoot {
        /// Exact proposed root package.
        root: FederationFrostRootPublicConfig,
    },
    /// Add a derived application epoch without activating it.
    ProposeApplication {
        /// Exact public application package.
        application: FederationFrostApplicationConfig,
        /// Explicit root-consumer registration.
        consumer: FederationFrostConsumerRegistration,
    },
    /// Register another reviewed application or backend against a root.
    RegisterConsumer {
        /// Exact usage-registry entry.
        registration: FederationFrostConsumerRegistration,
    },
    /// Select a proposed root for subsequently registered applications.
    ActivateRoot {
        /// Proposed root epoch.
        root_key_epoch: u64,
    },
    /// Select a proposed CDK application for new addresses and intents.
    ActivateApplication {
        /// Stable application identity.
        application_id: FederationFrostApplicationId,
        /// Proposed application key epoch.
        application_key_epoch: u64,
    },
    /// Remove a non-CDK consumer after its reviewed retirement contract completes.
    CompleteConsumerRetirement {
        /// Exact registry entry being completed.
        registration: FederationFrostConsumerRegistration,
    },
    /// Retire a drained CDK application and complete its consumer registration.
    RetireApplication {
        /// Stable application identity.
        application_id: FederationFrostApplicationId,
        /// Retiring application key epoch.
        application_key_epoch: u64,
    },
    /// Retire a root with no remaining consumers.
    RetireRoot {
        /// Retiring root epoch.
        root_key_epoch: u64,
    },
}

/// Consensus-derived live usage that must drain before application retirement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FederationFrostApplicationUsage {
    /// Wallet outputs not yet terminally spent or reorged away.
    pub unspent_utxos: u64,
    /// Sticky reservations held by unresolved transactions.
    pub reserved_utxos: u64,
    /// Pending or accepted payment intents for this epoch.
    pub unresolved_intents: u64,
    /// Accepted transactions without a terminal outcome.
    pub unresolved_transactions: u64,
    /// Unresolved recipient-paying transactions, excluding migration sweeps.
    pub unresolved_payment_transactions: u64,
}

impl FederationFrostApplicationUsage {
    /// Return true only when deleting the application signing path is safe.
    #[must_use]
    pub fn is_clear(self) -> bool {
        self.unspent_utxos == 0
            && self.reserved_utxos == 0
            && self.unresolved_intents == 0
            && self.unresolved_transactions == 0
    }
}

/// Materialized retirement guard keyed by stable application and key epoch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FederationFrostRotationGuard {
    applications: BTreeMap<(FederationFrostApplicationId, u64), FederationFrostApplicationUsage>,
}

impl FederationFrostRotationGuard {
    /// Construct a guard from consensus-derived application usage.
    #[must_use]
    pub fn new(
        applications: BTreeMap<
            (FederationFrostApplicationId, u64),
            FederationFrostApplicationUsage,
        >,
    ) -> Self {
        Self { applications }
    }

    /// Usage for one epoch, defaulting to a clear state when no state exists.
    #[must_use]
    pub fn usage(
        &self,
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
    ) -> FederationFrostApplicationUsage {
        self.applications
            .get(&(application_id, application_key_epoch))
            .copied()
            .unwrap_or_default()
    }
}

/// Safe public lifecycle status for one reusable root epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostRootEpochStatus {
    /// Root epoch.
    pub root_key_epoch: u64,
    /// Public lifecycle state.
    pub state: FederationFrostEpochState,
    /// Whether this root is selected for newly registered applications.
    pub active: bool,
    /// Number of explicit consumer registrations still using this root.
    pub registered_consumers: u64,
    /// DKG transcript commitment, containing no private share.
    pub transcript_binding: [u8; 32],
}

/// Safe public lifecycle status for one application key epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostApplicationEpochStatus {
    /// Stable application identity.
    pub application_id: FederationFrostApplicationId,
    /// Application key epoch.
    pub application_key_epoch: u64,
    /// Reusable root epoch used for derivation.
    pub root_key_epoch: u64,
    /// Public lifecycle state.
    pub state: FederationFrostEpochState,
    /// Whether this epoch receives new addresses and payment intents.
    pub active: bool,
    /// Public descriptor commitment.
    pub descriptor_commitment: [u8; 32],
}

/// Safe public root/application rotation view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostRotationStatus {
    /// Root epochs in canonical ascending order.
    pub roots: Vec<FederationFrostRootEpochStatus>,
    /// Application epochs ordered by application ID and epoch.
    pub applications: Vec<FederationFrostApplicationEpochStatus>,
}

impl FederationFrostWalletConfig {
    /// Apply one accepted lifecycle transition to a validated public snapshot.
    pub fn apply_rotation(
        &self,
        federation: &FederationConfig,
        operation: &FederationFrostRotationOperation,
        guard: &FederationFrostRotationGuard,
    ) -> Result<Self> {
        self.validate(federation)?;
        operation.validate()?;
        let mut next = self.clone();
        match &operation.transition {
            FederationFrostRotationTransition::ProposeRoot { root } => {
                root.validate(federation)?;
                if next.root(root.root_key_epoch).is_some()
                    || next
                        .roots
                        .iter()
                        .map(|existing| existing.root_key_epoch)
                        .max()
                        .is_some_and(|maximum| root.root_key_epoch <= maximum)
                {
                    return Err(invalid("proposed root epoch is reused or non-monotonic"));
                }
                next.roots.push(root.clone());
            }
            FederationFrostRotationTransition::ProposeApplication {
                application,
                consumer,
            } => {
                let root = next
                    .root(application.root_key_epoch)
                    .ok_or_else(|| invalid("proposed application root is unknown"))?;
                if root.state != FederationFrostEpochState::Active
                    || next.active_epochs.root_key_epoch != root.root_key_epoch
                {
                    return Err(invalid("proposed application must use the active root"));
                }
                application.validate(federation, root)?;
                let maximum = next
                    .cdk_onchain_applications
                    .iter()
                    .filter(|existing| existing.application_id == application.application_id)
                    .map(|existing| existing.application_key_epoch)
                    .max()
                    .unwrap_or(0);
                if application.application_key_epoch <= maximum
                    || next.registered_consumers.contains(consumer)
                {
                    return Err(invalid(
                        "proposed application epoch is reused or already registered",
                    ));
                }
                next.cdk_onchain_applications.push(application.clone());
                next.registered_consumers.push(consumer.clone());
            }
            FederationFrostRotationTransition::RegisterConsumer { registration } => {
                let root = next
                    .root(registration.root_key_epoch)
                    .ok_or_else(|| invalid("consumer root is unknown"))?;
                if root.state == FederationFrostEpochState::Retired
                    || next.registered_consumers.contains(registration)
                {
                    return Err(invalid("consumer root is retired or registration exists"));
                }
                next.registered_consumers.push(registration.clone());
            }
            FederationFrostRotationTransition::ActivateRoot { root_key_epoch } => {
                let target = next
                    .roots
                    .iter()
                    .position(|root| root.root_key_epoch == *root_key_epoch)
                    .ok_or_else(|| invalid("activation root is unknown"))?;
                if next.roots[target].state != FederationFrostEpochState::Proposed {
                    return Err(invalid("root activation requires a proposed root"));
                }
                let active = next
                    .roots
                    .iter()
                    .position(|root| root.state == FederationFrostEpochState::Active)
                    .ok_or_else(|| invalid("current active root is missing"))?;
                next.roots[active].state = FederationFrostEpochState::Retiring;
                next.roots[target].state = FederationFrostEpochState::Active;
                next.active_epochs.root_key_epoch = *root_key_epoch;
            }
            FederationFrostRotationTransition::ActivateApplication {
                application_id,
                application_key_epoch,
            } => {
                let target = next
                    .cdk_onchain_applications
                    .iter()
                    .position(|application| {
                        application.application_id == *application_id
                            && application.application_key_epoch == *application_key_epoch
                    })
                    .ok_or_else(|| invalid("activation application is unknown"))?;
                let target_root = next.cdk_onchain_applications[target].root_key_epoch;
                if next.cdk_onchain_applications[target].state
                    != FederationFrostEpochState::Proposed
                    || target_root != next.active_epochs.root_key_epoch
                {
                    return Err(invalid(
                        "application activation requires a proposed epoch on the active root",
                    ));
                }
                if let Some(active_epoch) = next
                    .active_epochs
                    .application_key_epochs
                    .get(application_id)
                    .copied()
                {
                    let usage = guard.usage(*application_id, active_epoch);
                    if usage.unresolved_intents != 0 || usage.unresolved_payment_transactions != 0 {
                        return Err(invalid(
                            "application activation requires the current epoch to have no unresolved payments",
                        ));
                    }
                    let active = next
                        .cdk_onchain_applications
                        .iter()
                        .position(|application| {
                            application.application_id == *application_id
                                && application.application_key_epoch == active_epoch
                        })
                        .ok_or_else(|| invalid("current active application is missing"))?;
                    next.cdk_onchain_applications[active].state =
                        FederationFrostEpochState::Retiring;
                }
                next.cdk_onchain_applications[target].state = FederationFrostEpochState::Active;
                next.active_epochs
                    .application_key_epochs
                    .insert(*application_id, *application_key_epoch);
            }
            FederationFrostRotationTransition::CompleteConsumerRetirement { registration } => {
                if next.cdk_onchain_applications.iter().any(|application| {
                    application.root_key_epoch == registration.root_key_epoch
                        && application.application_id == registration.application_id
                        && application.application_key_epoch == registration.application_key_epoch
                        && application.state != FederationFrostEpochState::Retired
                }) {
                    return Err(invalid(
                        "CDK consumer retirement must use application retirement guards",
                    ));
                }
                remove_consumer(&mut next, registration)?;
            }
            FederationFrostRotationTransition::RetireApplication {
                application_id,
                application_key_epoch,
            } => {
                let index = next
                    .cdk_onchain_applications
                    .iter()
                    .position(|application| {
                        application.application_id == *application_id
                            && application.application_key_epoch == *application_key_epoch
                    })
                    .ok_or_else(|| invalid("retirement application is unknown"))?;
                let application = &next.cdk_onchain_applications[index];
                if application.state != FederationFrostEpochState::Retiring
                    || next
                        .active_epochs
                        .application_key_epochs
                        .get(application_id)
                        == Some(application_key_epoch)
                    || !guard
                        .usage(*application_id, *application_key_epoch)
                        .is_clear()
                {
                    return Err(invalid(
                        "application retirement requires a drained retiring epoch",
                    ));
                }
                let registration = FederationFrostConsumerRegistration::new(
                    application.root_key_epoch,
                    *application_id,
                    *application_key_epoch,
                )?;
                next.cdk_onchain_applications[index].state = FederationFrostEpochState::Retired;
                remove_consumer(&mut next, &registration)?;
            }
            FederationFrostRotationTransition::RetireRoot { root_key_epoch } => {
                let index = next
                    .roots
                    .iter()
                    .position(|root| root.root_key_epoch == *root_key_epoch)
                    .ok_or_else(|| invalid("retirement root is unknown"))?;
                if next.roots[index].state != FederationFrostEpochState::Retiring
                    || next
                        .registered_consumers
                        .iter()
                        .any(|consumer| consumer.root_key_epoch == *root_key_epoch)
                    || next.cdk_onchain_applications.iter().any(|application| {
                        application.root_key_epoch == *root_key_epoch
                            && application.state != FederationFrostEpochState::Retired
                    })
                {
                    return Err(invalid(
                        "root retirement requires every consumer and application to retire",
                    ));
                }
                next.roots[index].state = FederationFrostEpochState::Retired;
            }
        }
        next.roots.sort_by_key(|root| root.root_key_epoch);
        next.cdk_onchain_applications.sort_by_key(|application| {
            (
                application.application_id,
                application.application_key_epoch,
            )
        });
        next.registered_consumers.sort();
        next.validate(federation)?;
        Ok(next)
    }

    /// Refuse local root-share deletion until the accepted lifecycle is terminal.
    pub fn ensure_root_share_deletable(
        &self,
        root_key_epoch: u64,
        guard: &FederationFrostRotationGuard,
    ) -> Result<()> {
        let root = self
            .root(root_key_epoch)
            .ok_or_else(|| invalid("root share deletion references an unknown epoch"))?;
        if root.state != FederationFrostEpochState::Retired
            || self
                .registered_consumers
                .iter()
                .any(|consumer| consumer.root_key_epoch == root_key_epoch)
            || self.cdk_onchain_applications.iter().any(|application| {
                application.root_key_epoch == root_key_epoch
                    && (!guard
                        .usage(
                            application.application_id,
                            application.application_key_epoch,
                        )
                        .is_clear()
                        || application.state != FederationFrostEpochState::Retired)
            })
        {
            return Err(invalid(
                "root share deletion is blocked by lifecycle or live usage",
            ));
        }
        Ok(())
    }

    /// Build public root/application lifecycle metadata without private shares.
    #[must_use]
    pub fn rotation_status(&self) -> FederationFrostRotationStatus {
        let mut roots = self
            .roots
            .iter()
            .map(|root| FederationFrostRootEpochStatus {
                root_key_epoch: root.root_key_epoch,
                state: root.state,
                active: self.active_epochs.root_key_epoch == root.root_key_epoch,
                registered_consumers: u64::try_from(
                    self.registered_consumers
                        .iter()
                        .filter(|consumer| consumer.root_key_epoch == root.root_key_epoch)
                        .count(),
                )
                .unwrap_or(u64::MAX),
                transcript_binding: root.transcript_binding,
            })
            .collect::<Vec<_>>();
        roots.sort_by_key(|root| root.root_key_epoch);
        let mut applications = self
            .cdk_onchain_applications
            .iter()
            .map(|application| FederationFrostApplicationEpochStatus {
                application_id: application.application_id,
                application_key_epoch: application.application_key_epoch,
                root_key_epoch: application.root_key_epoch,
                state: application.state,
                active: self
                    .active_epochs
                    .application_key_epochs
                    .get(&application.application_id)
                    == Some(&application.application_key_epoch),
                descriptor_commitment: application.descriptor_commitment,
            })
            .collect::<Vec<_>>();
        applications.sort_by_key(|application| {
            (
                application.application_id,
                application.application_key_epoch,
            )
        });
        FederationFrostRotationStatus {
            roots,
            applications,
        }
    }
}

fn remove_consumer(
    wallet: &mut FederationFrostWalletConfig,
    registration: &FederationFrostConsumerRegistration,
) -> Result<()> {
    let index = wallet
        .registered_consumers
        .iter()
        .position(|existing| existing == registration)
        .ok_or_else(|| invalid("consumer retirement registration is unknown"))?;
    wallet.registered_consumers.remove(index);
    Ok(())
}

fn invalid(reason: impl Into<String>) -> FederationError {
    FederationError::FrostConfigurationInvalid {
        reason: reason.into(),
    }
}
