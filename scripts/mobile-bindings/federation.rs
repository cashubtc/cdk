//! Federation wallet bindings used by the local mobile packages.

use std::collections::BTreeSet;
use std::str::FromStr;

use cdk::federation::{FederationConfig, FederationInvite};
use cdk::nuts::MintInfo;
use cdk_http_client::HttpClient;

use crate::error::FfiError;
use crate::types::{CurrencyUnit, MintUrl};

/// Persistable public data required to reconstruct a federated wallet connector.
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
    /// Persistable connector bootstrap data.
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
    pub(crate) fn parse(&self) -> Result<FederationConfig, FfiError> {
        let config: FederationConfig = serde_json::from_str(&self.public_config_json)?;
        config.validate_wallet_runtime_metadata()?;
        Ok(config)
    }
}

impl FederationWalletInfo {
    pub(crate) fn from_config(
        config: &FederationConfig,
        local_mint_url: cdk::mint_url::MintUrl,
    ) -> Result<Self, FfiError> {
        config.validate_wallet_runtime_metadata()?;

        let member_mint_urls = config
            .members
            .values()
            .map(|member| member.public_mint_url.clone().into())
            .collect();
        let units = config
            .keysets
            .iter()
            .filter(|keyset| keyset.active)
            .map(|keyset| keyset.unit.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(Into::into)
            .collect();
        let wallet_config = FederationWalletConfig {
            local_mint_url: local_mint_url.into(),
            public_config_json: serde_json::to_string(config)?,
        };

        Ok(Self {
            federation_id: config.federation_id.to_string(),
            wallet_config,
            member_mint_urls,
            member_count: config.threshold_params.member_count,
            signature_threshold: config.threshold_params.signature_threshold,
            consensus_threshold: config.threshold_params.consensus_threshold,
            units,
        })
    }
}

pub(crate) fn parse_federation_invite(
    invite_code: &str,
) -> Result<(FederationConfig, cdk::mint_url::MintUrl), FfiError> {
    let invite = FederationInvite::from_str(invite_code)?;
    let local_mint_url = invite
        .config
        .members
        .values()
        .next()
        .map(|member| member.public_mint_url.clone())
        .ok_or_else(|| FfiError::internal("Federation does not contain any members"))?;
    Ok((invite.config, local_mint_url))
}

pub(crate) async fn discover_public_config(
    mint_url: &cdk::mint_url::MintUrl,
) -> Result<FederationConfig, FfiError> {
    let client = HttpClient::builder()
        .no_redirects()
        .build()
        .map_err(FfiError::internal)?;
    let config_url = mint_url
        .join_paths(&["federation", "v1", "config"])
        .map_err(FfiError::internal)?;
    let public_config: FederationConfig = client
        .fetch(config_url.as_str())
        .await
        .map_err(FfiError::internal)?;
    let info_url = mint_url
        .join_paths(&["v1", "info"])
        .map_err(FfiError::internal)?;
    let mint_info: MintInfo = client
        .fetch(info_url.as_str())
        .await
        .map_err(FfiError::internal)?;

    public_config.validate()?;
    if !public_config.keysets.iter().any(|keyset| keyset.active) {
        return Err(FfiError::internal(
            "Federation public config does not contain an active keyset",
        ));
    }

    let mut matching_members = public_config
        .members
        .iter()
        .filter(|(_, member)| &member.public_mint_url == mint_url);
    let Some((member_id, _)) = matching_members.next() else {
        return Err(FfiError::internal(format!(
            "Federation public config does not contain discovery mint URL {mint_url}"
        )));
    };
    if matching_members.next().is_some() {
        return Err(FfiError::internal(format!(
            "Federation public config contains discovery mint URL {mint_url} more than once"
        )));
    }

    let federation_info = mint_info.federation.as_ref().ok_or_else(|| {
        FfiError::internal("Discovery member mint info does not advertise federation metadata")
    })?;
    let config_digest = public_config.config_digest_for_validated_config()?;
    public_config.validate_member_mint_info_with_config_digest(
        *member_id,
        federation_info,
        config_digest,
    )?;
    Ok(public_config)
}

/// Decode and validate a federation invite without connecting to its members.
#[uniffi::export]
pub fn inspect_federation_invite(invite_code: String) -> Result<FederationWalletInfo, FfiError> {
    let (config, local_mint_url) = parse_federation_invite(&invite_code)?;
    FederationWalletInfo::from_config(&config, local_mint_url)
}

/// Discover and validate a federation through any member mint URL.
#[uniffi::export(async_runtime = "tokio")]
pub async fn discover_federation(mint_url: MintUrl) -> Result<FederationWalletInfo, FfiError> {
    let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
    let config = discover_public_config(&cdk_mint_url).await?;
    FederationWalletInfo::from_config(&config, cdk_mint_url)
}
