use cashu_kvac::models::ZKP;
use cashu_kvac::models::MAC;
use cashu_kvac::secp::GroupElement;
use cashu_kvac::secp::Scalar;
use serde::{Deserialize, Serialize};

use super::Id;


/// Kvac Coin Message
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct KvacCoinMessage {
    /// Keyset ID
    ///
    /// ID from which we expect a signature.
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Tag
    /// 
    /// Unique identifier used to create the algebraic MAC from
    /// and for recovery purporses
    #[serde(rename = "t")]
    pub t_tag: Scalar,
    /// Coin
    /// 
    /// Pair ([`GroupElement`], [`GroupElement`]) that represent:
    /// 1) Value: encoding value 0
    /// 2) Script: encoding a custom script (Mint doesn't care)
    #[serde(rename = "c")]
    pub coin: (GroupElement, GroupElement)
}

/// Bootstrap Request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BootstrapRequest {
    /// Inputs
    /// 
    /// [`Vec<KvacCoinMessage>`] Where each element is a coin encoding 0 as an amount.
    #[cfg_attr(feature = "swagger", schema(max_items = 1_000, min_items = 2))]
    pub inputs: Vec<KvacCoinMessage>,
    /// Bootstrap Proofs
    /// 
    /// [`Vec<ZKP>`] proving that each coin is worth 0
    #[cfg_attr(feature = "swagger", schema(max_items = 1_000, min_items = 2))]
    pub proofs: Vec<ZKP>,
}

/// Bootstrap Response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BootstrapResponse {
    /// MACs
    /// 
    /// Approval stamp of the Mint
    pub macs: Vec<MAC>,
    /// IParams Proofs
    /// 
    /// [`Vec<ZKP>`] Proving that [`MintPrivateKey`] was used to issue each [`MAC`]
    pub proofs: Vec<ZKP>,
}