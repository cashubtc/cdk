//! Federation discovery and wallet configuration bindings.
//!
//! Discovery, validation and alias mapping live in the `cdk` wallet's
//! [`FederationRegistry`]; these bindings only convert its entries.

use cdk::federation::FederationConfig;
use cdk::wallet::{
    FederationImportOptions, FederationRegistry, FederationRegistryError, FederationSource,
    RegisteredFederation,
};
use cdk_http_client::Async;
use cdk_iroh::IrohTransport;

use crate::error::FfiError;
use crate::types::{CurrencyUnit, MintUrl};

/// Transport for federation discovery and member requests: HTTPS and Iroh.
pub(crate) type FederationTransport = IrohTransport<Async>;

/// Public data that identifies a federation to a wallet: its local mint URL
/// and its validated public config.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FederationWalletConfig {
    /// Mint URL used as the local wallet identifier.
    pub local_mint_url: MintUrl,
    /// Validated public federation config encoded as JSON.
    pub public_config_json: String,
}

/// Public, non-secret metadata for an imported federation.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FederationWalletInfo {
    /// Stable federation identifier.
    pub federation_id: String,
    /// Local mint URL and public config.
    pub wallet_config: FederationWalletConfig,
    /// Public mint URLs for every federation member.
    pub member_mint_urls: Vec<MintUrl>,
    /// Number of federation members.
    pub member_count: u16,
    /// Number of valid signature shares required for issuance.
    pub signature_threshold: u16,
    /// Number of members required for consensus finality.
    pub consensus_threshold: u16,
    /// Units with an active aggregate keyset.
    pub units: Vec<CurrencyUnit>,
}

impl FederationWalletConfig {
    pub(crate) fn public_config(&self) -> Result<FederationConfig, FfiError> {
        Ok(serde_json::from_str(&self.public_config_json)?)
    }
}

impl TryFrom<&RegisteredFederation> for FederationWalletInfo {
    type Error = FfiError;

    fn try_from(federation: &RegisteredFederation) -> Result<Self, Self::Error> {
        let config = federation.public_config();
        let member_mint_urls = config
            .members
            .values()
            .map(|member| member.public_mint_url.clone().into())
            .collect();
        let wallet_config = FederationWalletConfig {
            local_mint_url: federation.local_mint_url().clone().into(),
            public_config_json: serde_json::to_string(config)?,
        };

        Ok(Self {
            federation_id: federation.federation_id().to_string(),
            wallet_config,
            member_mint_urls,
            member_count: config.threshold_params.member_count,
            signature_threshold: config.threshold_params.signature_threshold,
            consensus_threshold: config.threshold_params.consensus_threshold,
            units: federation
                .active_units()
                .into_iter()
                .map(Into::into)
                .collect(),
        })
    }
}

impl From<FederationRegistryError> for FfiError {
    fn from(error: FederationRegistryError) -> Self {
        match error {
            FederationRegistryError::Connector(error) => error.into(),
            error => Self::internal(error.to_string()),
        }
    }
}

async fn inspect(source: FederationSource) -> Result<FederationWalletInfo, FfiError> {
    let federation = FederationRegistry::inspect_with_transport(
        source,
        FederationImportOptions::default(),
        &FederationTransport::default(),
    )
    .await?;
    FederationWalletInfo::try_from(&federation)
}

/// Decode and validate a federation invite without connecting to its members.
#[uniffi::export]
pub fn inspect_federation_invite(invite_code: String) -> Result<FederationWalletInfo, FfiError> {
    let rt = crate::runtime::RuntimeGuard::new().map_err(FfiError::internal)?;
    rt.block_on(inspect(FederationSource::Invite(invite_code)))
}

/// Discover and validate a federation through any member mint URL.
#[uniffi::export(async_runtime = "tokio")]
pub async fn discover_federation(mint_url: MintUrl) -> Result<FederationWalletInfo, FfiError> {
    inspect(FederationSource::MemberUrl(mint_url.try_into()?)).await
}
