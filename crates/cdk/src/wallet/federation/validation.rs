use cdk_common::federation::{
    FederatedKeyset, FederationConfig, FederationConfigDigest, FederationError, MemberId,
};

use crate::nuts::{KeySet, KeySetInfo, MintInfo};
use crate::Error;

/// Validate one member's federation metadata with a precomputed config digest.
pub(super) fn validate_member_mint_info_with_config_digest(
    public_config: &FederationConfig,
    member_id: MemberId,
    mint_info: &MintInfo,
    config_digest: FederationConfigDigest,
) -> Result<(), Error> {
    let federation_info = mint_info
        .federation
        .as_ref()
        .ok_or(FederationError::MissingFederationMintInfo { member_id })?;
    Ok(public_config.validate_member_mint_info_with_config_digest(
        member_id,
        federation_info,
        config_digest,
    )?)
}

pub(super) fn federated_keyset_to_keyset(keyset: &FederatedKeyset) -> KeySet {
    KeySet {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: Some(keyset.active),
        keys: keyset.keys(),
        input_fee_ppk: keyset.input_fee_ppk,
        final_expiry: keyset.final_expiry,
    }
}

pub(super) fn federated_keyset_to_keyset_info(keyset: &FederatedKeyset) -> KeySetInfo {
    KeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: keyset.active,
        input_fee_ppk: keyset.input_fee_ppk,
        final_expiry: keyset.final_expiry,
    }
}

pub(super) fn validate_member_active_keysets(
    public_config: &FederationConfig,
    member_id: MemberId,
    advertised_keysets: &[KeySet],
) -> Result<(), Error> {
    if !public_config.members.contains_key(&member_id) {
        return Err(FederationError::UnknownMember { member_id }.into());
    }

    for advertised in advertised_keysets {
        validate_member_advertised_keyset(public_config, member_id, advertised)?;
    }

    for keyset in public_config.keysets.iter().filter(|keyset| keyset.active) {
        if !advertised_keysets
            .iter()
            .any(|advertised| advertised.id == keyset.id)
        {
            return Err(FederationError::MissingAdvertisedFederatedKeyset {
                member_id,
                keyset_id: keyset.id,
            }
            .into());
        }
    }

    Ok(())
}

pub(super) fn validate_member_advertised_keyset(
    public_config: &FederationConfig,
    member_id: MemberId,
    advertised: &KeySet,
) -> Result<(), Error> {
    if advertised.id.get_version() != crate::nuts::KeySetVersion::Version02 {
        return Err(FederationError::InvalidFederatedKeysetVersion {
            keyset_id: advertised.id,
        }
        .into());
    }

    let expected = public_config
        .keysets
        .iter()
        .find(|keyset| keyset.id == advertised.id)
        .ok_or(FederationError::UnknownFederatedKeyset {
            keyset_id: advertised.id,
        })?;
    let expected_keyset = federated_keyset_to_keyset(expected);

    if advertised.id != expected_keyset.id
        || advertised.unit != expected_keyset.unit
        || advertised.input_fee_ppk != expected_keyset.input_fee_ppk
        || advertised.final_expiry != expected_keyset.final_expiry
        || advertised.keys != expected_keyset.keys
    {
        return Err(FederationError::AdvertisedFederatedKeysetMismatch {
            member_id,
            keyset_id: expected.id,
        }
        .into());
    }
    if let Some(active) = advertised.active {
        if active != expected.active {
            return Err(FederationError::AdvertisedFederatedKeysetMismatch {
                member_id,
                keyset_id: expected.id,
            }
            .into());
        }
    }

    Ok(())
}
