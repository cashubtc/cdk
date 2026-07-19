//! Member-local Bark wallet runtime and bounded readiness reporting.

use core::fmt;
use std::sync::Arc;

use bark::actions::lightning::receive::LightningReceiveAntiDosMode;
use bark::ark::lightning::{PaymentHash, Preimage};
use bark::ark::mailbox::MailboxIdentifier;
use bark::custody::{KeySourceDescriptor, KeySourceKind, VtxoCustody, VtxoKeyLocator};
use bark::federation::{
    FederationActionId, FederationLightningReceiveIntent, FederationLightningReceivePlan,
    FederationRequestCommitment, FederationWallet,
};
use bark::{OpenWalletArgs, Wallet, WalletProperties};
use bitcoin::secp256k1::PublicKey;
use cdk_common::federation::{
    FederationBarkOperation, FederationBarkReceiveIntent, FederationBarkReceivePrepared,
    FederationBarkReceiveSecretBinding, FederationConfig, FederationOperationId,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{
    CdkFrostVtxoCustody, Error, FederatedBarkLimits, FederatedBarkLocalConfig,
    FederatedBarkReceiveSecretStore, FederatedBarkScope, ValidatedFederatedBarkConfig,
};

/// Whether member-local Bark state can serve an accepted frontier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedBarkReadiness {
    /// Local projection is valid and at or beyond the requested frontier.
    Ready,
    /// Local projection is valid but has not replayed the requested frontier.
    BehindConsensus,
    /// Local state or a required dependency is unavailable.
    Degraded,
}

/// Bounded reason for degraded operation; never an external error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedBarkDegradedReason {
    /// Bark's member-local execution wallet is unavailable.
    WalletUnavailable,
    /// Deterministic accepted-state projection is invalid.
    ProjectionInvalid,
    /// The selected participant set cannot satisfy the operation.
    ParticipantsUnavailable,
}

/// Public, redacted runtime status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedBarkRuntimeStatus {
    /// Current readiness for the requested frontier.
    pub readiness: FederatedBarkReadiness,
    /// Highest locally replayed accepted frontier.
    pub local_frontier: u64,
    /// Bounded degraded reason, if any.
    pub degraded_reason: Option<FederatedBarkDegradedReason>,
}

#[derive(Debug, Clone, Copy)]
struct RuntimeHealth {
    local_frontier: u64,
    degraded_reason: Option<FederatedBarkDegradedReason>,
}

/// Restricted member-local Bark execution runtime.
pub struct FederatedBarkRuntime {
    scope: FederatedBarkScope,
    limits: FederatedBarkLimits,
    wallet: FederationWallet,
    receive_secrets: Arc<FederatedBarkReceiveSecretStore>,
    health: RwLock<RuntimeHealth>,
}

impl fmt::Debug for FederatedBarkRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBarkRuntime")
            .field("scope", &self.scope)
            .field("limits", &self.limits)
            .field("wallet", &"<member-local Bark projection>")
            .field("receive_secrets", &"<encrypted member-local store>")
            .field("health", &"<bounded runtime status>")
            .finish()
    }
}

impl FederatedBarkRuntime {
    /// Validate all bindings and open a fresh or existing FROST-custodied Bark wallet.
    pub async fn open(
        federation_enabled: bool,
        federation: &FederationConfig,
        local: FederatedBarkLocalConfig,
    ) -> Result<Arc<Self>, Error> {
        let validated =
            ValidatedFederatedBarkConfig::validate(federation_enabled, federation, &local)?;
        let receive_secrets = Arc::new(FederatedBarkReceiveSecretStore::open(
            &local.receive_secret_store.database_path,
            Arc::clone(&local.receive_secret_store.encryption_key),
        )?);
        let custody = Arc::new(CdkFrostVtxoCustody::new(validated.clone()));
        validated.validate_descriptor(&custody.descriptor())?;
        let expected_operational_fingerprint = local.operational_seed.fingerprint();
        let expected_descriptor = custody.descriptor();
        let expected_server = validated.ark_server_public_key();
        let limits = local.limits;
        let wallet = Wallet::open_with_custody(
            validated.network(),
            local.operational_seed,
            custody,
            local.bark,
            OpenWalletArgs {
                run_daemon: false,
                datadir: Some(local.data_directory),
                create_if_not_exists: local.create_if_missing,
                create_without_server: false,
                ..OpenWalletArgs::default()
            },
        )
        .await
        .map_err(|_| Error::WalletOpenFailed)?;
        let properties = wallet
            .properties()
            .await
            .map_err(|_| Error::WalletPropertiesUnavailable)?;
        validate_wallet_properties(
            &properties,
            validated.network(),
            expected_operational_fingerprint,
            &expected_descriptor,
            expected_server,
        )?;
        let ark_info = wallet
            .require_ark_info()
            .await
            .map_err(|_| Error::ArkCapabilityMismatch)?;
        if ark_info.network != validated.network()
            || ark_info.server_pubkey != expected_server
            || ark_info.ln_receive_anti_dos_required
            || ark_info.fees.lightning_receive.base_fee != bitcoin::Amount::ZERO
            || ark_info.fees.lightning_receive.ppm.0 != 0
            || u32::from(ark_info.max_user_invoice_cltv_delta)
                < validated.policy().minimum_incoming_cltv_delta
            || ark_info
                .max_vtxo_amount
                .is_some_and(|maximum| maximum.to_sat() < limits.maximum_sat())
        {
            return Err(Error::ArkCapabilityMismatch);
        }
        let wallet = FederationWallet::new(&wallet)
            .await
            .map_err(|_| Error::FederationFacadeUnavailable)?;
        Ok(Arc::new(Self {
            scope: validated.scope(),
            limits,
            wallet,
            receive_secrets,
            health: RwLock::new(RuntimeHealth {
                local_frontier: 0,
                degraded_reason: None,
            }),
        }))
    }

    /// Immutable accepted runtime scope.
    #[must_use]
    pub const fn scope(&self) -> FederatedBarkScope {
        self.scope
    }

    /// Startup-validated satoshi payment limits.
    #[must_use]
    pub const fn limits(&self) -> FederatedBarkLimits {
        self.limits
    }

    /// Encrypted member-local receive-secret store.
    #[must_use]
    pub fn receive_secret_store(&self) -> Arc<FederatedBarkReceiveSecretStore> {
        Arc::clone(&self.receive_secrets)
    }

    /// Purely derive one accepted locator without database or network I/O.
    pub fn derive_public_key(&self, index: u32) -> Result<PublicKey, Error> {
        self.wallet
            .derive_locator(VtxoKeyLocator { index })
            .map(|derived| derived.public_key)
            .map_err(|_| Error::PublicKeyDerivationFailed)
    }

    /// Perform the sole Ark invoice RPC for an already accepted receive intent.
    ///
    /// The protected preimage is loaded only after the caller supplies the
    /// accepted intent operation ID. Bark receives no locally generated
    /// identity, secret, locator, mailbox, amount, or policy value.
    pub async fn prepare_receive_invoice(
        &self,
        intent_operation_id: FederationOperationId,
        intent: &FederationBarkReceiveIntent,
    ) -> Result<FederationBarkReceivePrepared, Error> {
        let binding =
            FederationBarkReceiveSecretBinding::from_intent(self.scope.federation_id(), intent);
        let secret = self.receive_secrets.load_secret(&binding)?;
        let bark_intent = self.bark_receive_intent(intent, *secret.as_bytes())?;
        let plan = self
            .wallet
            .start_lightning_receive(&bark_intent)
            .await
            .map_err(|_| Error::InvoicePreparationFailed)?;
        FederationBarkReceivePrepared::from_invoice(
            intent.actor,
            intent_operation_id,
            intent,
            plan.invoice,
        )
        .map_err(|_| Error::InvoicePreparationFailed)
    }

    /// Materialize an accepted receive plan without another Ark RPC.
    pub async fn apply_receive_prepared(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
    ) -> Result<(), Error> {
        prepared
            .validate_against_intent(intent)
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        let binding =
            FederationBarkReceiveSecretBinding::from_intent(self.scope.federation_id(), intent);
        let secret = self.receive_secrets.load_secret(&binding)?;
        let bark_intent = self.bark_receive_intent(intent, *secret.as_bytes())?;
        let plan = FederationLightningReceivePlan {
            action_id: bark_intent.action_id,
            request_commitment: bark_intent.request_commitment,
            payment_hash: bark_intent.payment_hash,
            cashu_amount: bark_intent.cashu_amount,
            gross_amount: bark_intent.gross_amount,
            receive_fee_bound: bark_intent.receive_fee_bound,
            minimum_net_amount: bark_intent.minimum_net_amount,
            description: bark_intent.description,
            invoice: prepared
                .invoice
                .parse()
                .map_err(|_| Error::ReceiveProjectionFailed)?,
            payment_preimage: bark_intent.payment_preimage,
            locator: bark_intent.locator,
            mailbox_id: bark_intent.mailbox_id,
            htlc_recv_cltv_delta: bark_intent.htlc_recv_cltv_delta,
            anti_dos: bark_intent.anti_dos,
            policy_version: bark_intent.policy_version,
            custody_attempt: bark_intent.custody_attempt,
        };
        self.wallet
            .validate_lightning_receive_plan(&plan)
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        self.wallet
            .apply_lightning_receive(&plan)
            .await
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        Ok(())
    }

    fn bark_receive_intent(
        &self,
        intent: &FederationBarkReceiveIntent,
        preimage: [u8; 32],
    ) -> Result<FederationLightningReceiveIntent, Error> {
        let action_id = FederationActionId::from_bytes(intent.action_id.to_bytes())
            .map_err(|_| Error::InvoicePreparationFailed)?;
        let request_commitment = FederationRequestCommitment::from_bytes(
            FederationBarkOperation::ReceiveIntent(intent.clone())
                .commitment()
                .map_err(|_| Error::InvoicePreparationFailed)?
                .to_bytes(),
        )
        .map_err(|_| Error::InvoicePreparationFailed)?;
        let mailbox_public_key = PublicKey::from_slice(&intent.mailbox_id)
            .map_err(|_| Error::InvoicePreparationFailed)?;
        let htlc_recv_cltv_delta = u16::try_from(intent.minimum_cltv_delta)
            .map_err(|_| Error::InvoicePreparationFailed)?;
        Ok(FederationLightningReceiveIntent {
            action_id,
            request_commitment,
            payment_hash: PaymentHash::from(intent.payment_hash.to_bytes()),
            cashu_amount: bitcoin::Amount::from_sat(u64::from(intent.cashu_amount)),
            gross_amount: bitcoin::Amount::from_sat(u64::from(intent.gross_amount)),
            receive_fee_bound: bitcoin::Amount::from_sat(u64::from(intent.receive_fee)),
            minimum_net_amount: bitcoin::Amount::from_sat(u64::from(intent.minimum_net_amount)),
            description: intent.description.clone(),
            payment_preimage: Preimage::from(preimage),
            locator: VtxoKeyLocator {
                index: intent.destination.index,
            },
            mailbox_id: MailboxIdentifier::from_pubkey(mailbox_public_key),
            htlc_recv_cltv_delta,
            anti_dos: LightningReceiveAntiDosMode::NoProof,
            policy_version: intent.fee_policy_version,
            custody_attempt: 1,
        })
    }

    /// Return readiness relative to one accepted frontier.
    pub async fn status(&self, required_frontier: u64) -> FederatedBarkRuntimeStatus {
        let health = *self.health.read().await;
        let readiness = match health.degraded_reason {
            Some(_) => FederatedBarkReadiness::Degraded,
            None if health.local_frontier < required_frontier => {
                FederatedBarkReadiness::BehindConsensus
            }
            None => FederatedBarkReadiness::Ready,
        };
        FederatedBarkRuntimeStatus {
            readiness,
            local_frontier: health.local_frontier,
            degraded_reason: health.degraded_reason,
        }
    }

    /// Fail closed unless this member can serve the required accepted frontier.
    pub async fn ensure_ready(&self, required_frontier: u64) -> Result<(), Error> {
        match self.status(required_frontier).await.readiness {
            FederatedBarkReadiness::Ready => Ok(()),
            FederatedBarkReadiness::BehindConsensus | FederatedBarkReadiness::Degraded => {
                Err(Error::NotReady)
            }
        }
    }

    /// Return the current bounded degradation reason.
    pub async fn degraded_reason(&self) -> Option<FederatedBarkDegradedReason> {
        self.health.read().await.degraded_reason
    }

    /// Advance the member-local replay frontier monotonically.
    pub async fn record_projection_frontier(&self, frontier: u64) {
        let mut health = self.health.write().await;
        health.local_frontier = health.local_frontier.max(frontier);
    }

    /// Mark the runtime degraded with a bounded, non-sensitive reason.
    pub async fn mark_degraded(&self, reason: FederatedBarkDegradedReason) {
        self.health.write().await.degraded_reason = Some(reason);
    }
}

pub(crate) fn validate_wallet_properties(
    properties: &WalletProperties,
    expected_network: bitcoin::Network,
    expected_operational_fingerprint: bitcoin::bip32::Fingerprint,
    expected_descriptor: &KeySourceDescriptor,
    expected_server: PublicKey,
) -> Result<(), Error> {
    if properties.network != expected_network {
        return Err(Error::ConfigMismatch {
            field: "wallet_network",
        });
    }
    if properties.fingerprint != expected_operational_fingerprint {
        return Err(Error::ConfigMismatch {
            field: "operational_seed",
        });
    }
    let descriptor = properties
        .key_source
        .as_ref()
        .ok_or(Error::StandaloneCustodyForbidden)?;
    if descriptor.kind() != KeySourceKind::Frost {
        return Err(Error::StandaloneCustodyForbidden);
    }
    if descriptor != expected_descriptor {
        return Err(Error::CustodyDescriptorMismatch);
    }
    if properties.server_pubkey != Some(expected_server) {
        return Err(Error::ConfigMismatch {
            field: "ark_server_public_key",
        });
    }
    if properties.server_mailbox_pubkey.is_none() {
        return Err(Error::ConfigMismatch {
            field: "ark_server_mailbox_public_key",
        });
    }
    Ok(())
}
