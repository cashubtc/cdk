//! Bark custody adapter rooted in the accepted CDK FROST application.

use core::fmt;

use async_trait::async_trait;
use bark::ark::mailbox::BlindedMailboxIdentifier;
use bark::ark::musig::PartialSignature;
use bark::custody::{
    AbortReason, CustodySessionId, DerivedVtxoPublicKey, KeySourceDescriptor,
    MailboxBlindingRequest, Musig2Commitment, Musig2CommitmentRequest, Musig2SigningRequest,
    SchnorrSigningRequest, VtxoCustody, VtxoKeyLocator,
};
use bitcoin::secp256k1::schnorr;
use cdk_frost_bark::BarkKeyLocator;

use crate::{Error, ValidatedFederatedBarkConfig};

/// Bark [`VtxoCustody`] implementation backed by the registered CDK FROST root.
///
/// Milestone 6 exposes public derivation only. Every custody operation that can
/// release a signature or DH-derived value remains fail-closed until accepted
/// Bark operations install the typed authorization adapter in Milestone 7.
#[derive(Clone)]
pub struct CdkFrostVtxoCustody {
    config: ValidatedFederatedBarkConfig,
}

impl CdkFrostVtxoCustody {
    /// Construct from fully validated accepted and local configuration.
    #[must_use]
    pub const fn new(config: ValidatedFederatedBarkConfig) -> Self {
        Self { config }
    }
}

impl fmt::Debug for CdkFrostVtxoCustody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CdkFrostVtxoCustody")
            .field("scope", &self.config.scope())
            .field("descriptor", &self.config.descriptor())
            .finish()
    }
}

#[async_trait]
impl VtxoCustody for CdkFrostVtxoCustody {
    fn descriptor(&self) -> KeySourceDescriptor {
        self.config.descriptor()
    }

    fn derive_public_key(&self, locator: VtxoKeyLocator) -> anyhow::Result<DerivedVtxoPublicKey> {
        let derived = self
            .config
            .public_adapter()
            .derive(BarkKeyLocator::vtxo(locator.index))
            .map_err(|_| Error::PublicKeyDerivationFailed)?;
        Ok(DerivedVtxoPublicKey {
            locator,
            public_key: derived.public_key(),
        })
    }

    async fn sign_schnorr(
        &self,
        _request: SchnorrSigningRequest,
    ) -> anyhow::Result<schnorr::Signature> {
        Err(Error::ConsensusOnly.into())
    }

    async fn begin_musig2(
        &self,
        _request: Musig2CommitmentRequest,
    ) -> anyhow::Result<Musig2Commitment> {
        Err(Error::ConsensusOnly.into())
    }

    async fn finish_musig2(
        &self,
        _request: Musig2SigningRequest,
    ) -> anyhow::Result<PartialSignature> {
        Err(Error::ConsensusOnly.into())
    }

    async fn blind_mailbox(
        &self,
        _request: MailboxBlindingRequest,
    ) -> anyhow::Result<BlindedMailboxIdentifier> {
        Err(Error::ConsensusOnly.into())
    }

    async fn abort_session(
        &self,
        _session: CustodySessionId,
        _reason: AbortReason,
    ) -> anyhow::Result<()> {
        // No protocol state can exist while every mutating custody operation
        // above is disabled. This remains idempotent for Bark cleanup paths.
        Ok(())
    }
}
