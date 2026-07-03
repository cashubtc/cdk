//! Federation mode scaffolding for CDK mints.
//!
//! This module is intentionally small at first. Later federation milestones will
//! attach consensus, peer networking, and threshold signing behavior to member
//! mode. Until then, the default standalone mode preserves current mint
//! behavior.

use cdk_common::error::Error;
use cdk_common::federation::{FederationConfig, FederationId, MemberConfigPrivate, MemberId};

/// Defines whether a mint runs as a standalone Cashu mint or as a federation member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FederationMode {
    /// Current single-operator mint behavior.
    #[default]
    Standalone,
    /// Placeholder for consensus-backed federation member behavior.
    Member,
}

impl FederationMode {
    /// Returns true when this mode is the federation member placeholder.
    #[must_use]
    pub fn is_member(self) -> bool {
        matches!(self, Self::Member)
    }
}

/// Validated federation config held by a mint member at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationRuntimeConfig {
    public_config: FederationConfig,
    private_config: MemberConfigPrivate,
}

impl FederationRuntimeConfig {
    /// Create a runtime federation config from public and local private config.
    pub fn new(
        public_config: FederationConfig,
        private_config: MemberConfigPrivate,
    ) -> Result<Self, Error> {
        public_config.validate()?;
        private_config.validate_against_public(&public_config)?;

        Ok(Self {
            public_config,
            private_config,
        })
    }

    /// Public federation config shared by all members.
    #[must_use]
    pub fn public_config(&self) -> &FederationConfig {
        &self.public_config
    }

    /// Local member private federation config.
    #[must_use]
    pub fn private_config(&self) -> &MemberConfigPrivate {
        &self.private_config
    }

    /// Local member ID.
    #[must_use]
    pub fn member_id(&self) -> MemberId {
        self.private_config.member_id
    }

    /// Federation roster ID.
    #[must_use]
    pub fn federation_id(&self) -> FederationId {
        self.public_config.federation_id
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use cdk_common::federation::{
        generate_trusted_dealer_configs, ThresholdParams, TrustedDealerKeysetSpec,
        TrustedDealerMember,
    };
    use cdk_common::mint_url::MintUrl;
    use cdk_common::nuts::nut01::BlsSecretKey;
    use cdk_common::{Amount, CurrencyUnit, SecretKey};

    use super::{FederationMode, FederationRuntimeConfig};

    #[test]
    fn federation_mode_defaults_to_standalone() {
        assert_eq!(FederationMode::default(), FederationMode::Standalone);
        assert!(!FederationMode::default().is_member());
        assert!(FederationMode::Member.is_member());
    }

    #[test]
    fn runtime_config_validates_public_and_private_configs() {
        let member = TrustedDealerMember::new(
            cdk_common::federation::MemberId::new(1).expect("member id"),
            MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed1.example.com").expect("federation api url"),
            SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        );
        let keyset = TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            BTreeMap::from([(
                Amount::from(1),
                vec![BlsSecretKey::from_reduced_bytes(&[2; 32])],
            )]),
        );
        let bundle = generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(1, 1).expect("params"),
            vec![member],
            vec![keyset],
        )
        .expect("trusted dealer bundle");
        let private_config = bundle
            .private_configs
            .get(&cdk_common::federation::MemberId::new(1).expect("member id"))
            .expect("private config")
            .clone();
        let runtime_config =
            FederationRuntimeConfig::new(bundle.public_config.clone(), private_config)
                .expect("runtime config");

        assert_eq!(
            runtime_config.federation_id(),
            bundle.public_config.federation_id
        );
        assert_eq!(runtime_config.member_id().get(), 1);
    }
}
