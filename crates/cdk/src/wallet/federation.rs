//! Wallet-side federation validation helpers.

use cdk_common::federation::{FederationConfig, MemberId};
use cdk_common::nuts::KeySet;

use crate::Error;

/// Minimal wallet federation connector validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FederationWalletConnector;

impl FederationWalletConnector {
    /// Validate the aggregate keysets advertised by one federation member.
    pub fn validate_member_keysets(
        public_config: &FederationConfig,
        member_id: MemberId,
        advertised_keysets: &[KeySet],
    ) -> Result<(), Error> {
        Ok(public_config.validate_member_advertised_keysets(member_id, advertised_keysets)?)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use cdk_common::federation::{
        generate_trusted_dealer_configs, FederationError, MemberId, ThresholdParams,
        TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    use cdk_common::mint_url::MintUrl;
    use cdk_common::nuts::nut01::BlsSecretKey;
    use cdk_common::nuts::nut02::KeySet;
    use cdk_common::{Amount, CurrencyUnit, Id, SecretKey};

    use super::FederationWalletConnector;

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("secret key")
    }

    fn member(member_id: MemberId) -> TrustedDealerMember {
        TrustedDealerMember::new(
            member_id,
            MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
                .expect("mint url"),
            MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get()))
                .expect("fed url"),
            identity_secret(member_id),
        )
    }

    fn bundle() -> cdk_common::federation::TrustedDealerConfigBundle {
        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(1, 1).expect("params"),
            vec![member(member_id(1))],
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([(
                    Amount::from(1),
                    vec![BlsSecretKey::from_reduced_bytes(&[2; 32])],
                )]),
            )],
        )
        .expect("trusted dealer bundle")
    }

    fn advertised_keyset(bundle: &cdk_common::federation::TrustedDealerConfigBundle) -> KeySet {
        let keyset = &bundle.public_config.keysets[0];
        KeySet {
            id: keyset.id,
            unit: keyset.unit.clone(),
            active: Some(keyset.active),
            keys: keyset.keys(),
            input_fee_ppk: keyset.input_fee_ppk,
            final_expiry: keyset.final_expiry,
        }
    }

    #[test]
    fn wallet_connector_accepts_matching_v3_aggregate_keysets() {
        let bundle = bundle();
        let advertised = advertised_keyset(&bundle);

        FederationWalletConnector::validate_member_keysets(
            &bundle.public_config,
            member_id(1),
            &[advertised],
        )
        .expect("matching member keysets");
    }

    #[test]
    fn wallet_connector_rejects_non_v3_aggregate_keysets() {
        let bundle = bundle();
        let mut advertised = advertised_keyset(&bundle);
        let expected = &bundle.public_config.keysets[0];
        advertised.id = Id::v2_from_data(
            &advertised.keys,
            &expected.unit,
            expected.input_fee_ppk,
            expected.final_expiry,
        );

        assert!(matches!(
            FederationWalletConnector::validate_member_keysets(
                &bundle.public_config,
                member_id(1),
                &[advertised],
            ),
            Err(crate::Error::Federation(
                FederationError::UnknownFederatedKeyset { .. }
                    | FederationError::InvalidFederatedKeysetVersion { .. }
            ))
        ));
    }

    #[test]
    fn wallet_connector_rejects_mismatched_aggregate_keysets() {
        let bundle = bundle();
        let mut advertised = advertised_keyset(&bundle);
        advertised.keys = cdk_common::Keys::new(BTreeMap::new());

        assert!(matches!(
            FederationWalletConnector::validate_member_keysets(
                &bundle.public_config,
                member_id(1),
                &[advertised],
            ),
            Err(crate::Error::Federation(
                FederationError::Nut02(_)
                    | FederationError::AdvertisedFederatedKeysetMismatch { .. }
            ))
        ));
    }
}
