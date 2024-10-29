//! NUT-DLC: Discrete Log Contracts
//!
//! https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/dlc.md

use std::{collections::HashMap, str::FromStr};

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;

use super::nut00::Witness;
use super::{nut00::token::TokenV3Token, nut01::PublicKey, Proof, Proofs};
use super::{nut10, BlindSignature, BlindedMessage, CurrencyUnit, Nut10Secret, SecretData};
use crate::util::hex;
use crate::Amount;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::json;
use thiserror::Error;

pub mod serde_dlc_witness;

#[derive(Debug, Error)]
/// Errors for DLC
pub enum Error {}

/// DLC Witness
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DLCWitness {
    /// DLC Secret
    pub dlc_secret: SecretData,
}

impl Proof {
    /// Add DLC witness to proof
    pub fn add_dlc_witness(&mut self, dlc_secret: Nut10Secret) {
        let secret_data = match dlc_secret.kind {
            nut10::Kind::DLC => dlc_secret.secret_data,
            _ => todo!("this should error"),
        };
        self.witness = Some(Witness::DLCWitness(DLCWitness {
            dlc_secret: secret_data,
        }));
    }
}

// Ti == SHA256(Ki_ || Pi)
#[derive(Clone, Debug)]
/// DLC leaf corresponding to a single outcome
pub struct DLCLeaf {
    /// Blinded locking point - Ki_ = Ki + b*G
    pub blinded_locking_point: PublicKey, // TODO: is this the right type to use?
    /// Payouts for this outcome
    pub payout: PayoutStructure, // JSON-encoded payout structure
}

impl DLCLeaf {
    /// SHA256(Ki_ || Pi)
    pub fn hash(&self) -> [u8; 32] {
        // Convert blinded_locking_point to bytes
        let point_bytes = self.blinded_locking_point.to_bytes().to_vec();

        // Concatenate point_bytes and payout string
        let mut input = point_bytes;
        input.extend(self.payout.as_bytes());

        // Compute SHA256 hash
        Sha256Hash::hash(&input).to_byte_array()
    }
}

// Tt = SHA256(hash_to_curve(t.to_bytes(8, 'big')) || Pt)
/// DLC leaf for the timeout condition
pub struct DLCTimeoutLeaf {
    /// H2C of timeout
    timeout_hash: PublicKey,
    /// Payout structure for the timeout
    payout: PayoutStructure,
}

impl DLCTimeoutLeaf {
    /// Create new [`DLCTimeoutLeaf`]
    pub fn new(timeout: &u64, payout: &PayoutStructure) -> Self {
        let timeout_hash = crate::dhke::hash_to_curve(&timeout.to_be_bytes())
            .expect("error calculating timeout hash");

        Self {
            timeout_hash,
            payout: payout.clone(),
        }
    }

    /// SHA256(hash_to_curve(timeout) || Pt)
    pub fn hash(&self) -> [u8; 32] {
        let mut input = self.timeout_hash.to_bytes().to_vec();
        input.extend(self.payout.as_bytes());
        Sha256Hash::hash(&input).to_byte_array()
    }
}

/// Hash of all spending conditions and blinded locking points
#[derive(Serialize, Deserialize, Debug)]
pub struct DLCRoot([u8; 32]);

impl DLCRoot {
    /// new [`DLCRoot`] from [`DLCLeaf`]s and optional [`DLCTimeoutLeaf`]
    pub fn compute(leaves: Vec<DLCLeaf>, timeout_leaf: Option<DLCTimeoutLeaf>) -> Self {
        let mut input: Vec<[u8; 32]> = Vec::new();
        for leaf in leaves {
            input.push(leaf.hash());
        }
        if let Some(timeout_leaf) = timeout_leaf {
            input.push(timeout_leaf.hash());
        }
        Self {
            0: crate::nuts::nutsct::merkle_root(&input),
        }
    }

    /// Convert to bytes
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl ToString for DLCRoot {
    fn to_string(&self) -> String {
        hex::encode(self.0)
    }
}

impl FromStr for DLCRoot {
    type Err = crate::nuts::nut11::Error;

    fn from_str(s: &str) -> Result<Self, crate::nuts::nut11::Error> {
        let bytes = hex::decode(s).map_err(|_| crate::nuts::nut11::Error::InvalidHash)?;
        if bytes.len() != 32 {
            return Err(crate::nuts::nut11::Error::InvalidHash);
        }
        let mut array = [0u8; 32];
        array.copy_from_slice(&bytes);
        Ok(DLCRoot(array))
    }
}

// struct DLCMerkleTree {
//     root: DLCRoot,
//     leaves: Vec<DLCLeaf>,
//     timeout_leaf: Option<DLCTimeoutLeaf>,
// }

// NOTE: copied from nut00/token.rs TokenV3, should it be V3 or V4?
/// DLC Funding Token
pub struct DLCFundingToken {
    /// Proofs in [`Token`] by mint
    pub token: Vec<TokenV3Token>,
    /// Memo for token
    // #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Token Unit
    // #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<CurrencyUnit>,
    /// DLC Root
    pub dlc_root: DLCRoot,
}

#[derive(Serialize, Deserialize, Debug)]
/// DLC
pub struct DLC {
    /// DLC Root
    pub dlc_root: String,

    /// Amount of funds locked in the contract
    pub funding_amount: Amount,

    /// unit of the contract
    pub unit: CurrencyUnit,

    /// Proofs funding the DLC
    pub inputs: Proofs, // locked with DLC secret - only spendable in this DLC
}

/// see https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/dlc.md#mint-registration
#[derive(Serialize, Deserialize, Debug)]
/// POST request body for /v1/dlc/fund
pub struct PostDLCRegistrationRequest {
    /// DLCs to register
    pub registrations: Vec<DLC>,
}

#[derive(Serialize, Deserialize, Debug)]
/// Successfully funded DLC
pub struct FundedDLC {
    /// DLC Root
    pub dlc_root: String,
    /// [`FundingProof`] from mint
    pub funding_proof: FundingProof,
}

//
#[derive(Serialize, Deserialize, Debug)]
/// Proof from the mint that the DLC was funded
///
/// see https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/dlc.md#funding-proofs
pub struct FundingProof {
    /// Keyset Id
    pub keyset: String,
    ///BIP-340 signature of DLC root and funding amount
    pub signature: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
/// [`DLCRegistrationResponse`] can be either a success or an error
pub enum DLCRegistrationResponse {
    /// Success [`DLCRegistrationResponse`]
    Success {
        /// successfully [`FundedDLC`]s
        funded: Vec<FundedDLC>,
    },
    /// Error [`DLCRegistrationResponse`]
    Error {
        /// successfully [`FundedDLC`]s
        funded: Vec<FundedDLC>,
        /// [`DLCError`]s for inputs that failed to register
        errors: Vec<DLCError>,
    },
}

#[derive(Serialize, Deserialize, Debug)]
/// Error for [`DLCRegistrationResponse`]
pub struct DLCError {
    /// DLC Root
    pub dlc_root: String,
    /// [`BadInput`]s
    pub bad_inputs: Vec<BadInput>,
}

#[derive(Serialize, Deserialize, Debug)]
/// [`BadInput`] for [`DLCError`]
pub struct BadInput {
    /// Index of the input that failed
    pub index: u32,
    /// Detail of the error
    pub detail: String,
}

#[derive(Clone, Debug)]
/// serialized dictionaries which map `xonly_pubkey -> weight`
///
/// see https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/dlc.md#payout-structures
pub struct PayoutStructure(HashMap<PublicKey, u64>);

impl PayoutStructure {
    /// Create new [`PayoutStructure`] with a single payout
    pub fn default(pubkey: String) -> Self {
        let pubkey = if pubkey.len() == 64 {
            // this way we can use nostr keys
            format!("02{}", pubkey)
        } else {
            pubkey
        };
        let pubkey = PublicKey::from_str(&pubkey).unwrap();
        Self(HashMap::from([(pubkey, 1)]))
    }

    /// Create new [`PayoutStructure`] with even weight to all pubkeys
    pub fn default_timeout(mut pubkeys: Vec<String>) -> Self {
        let mut payout = HashMap::new();
        pubkeys.sort(); // Sort pubkeys before creating hashmap
        for pubkey in pubkeys {
            let pubkey = if pubkey.len() == 64 {
                format!("02{}", pubkey)
            } else {
                pubkey
            };
            let pubkey = PublicKey::from_str(&pubkey).unwrap();
            payout.insert(pubkey, 1);
        }
        Self(payout)
    }

    /// Convert the PayoutStructure to a byte representation
    pub fn as_bytes(&self) -> Vec<u8> {
        // Create sorted vector of entries
        let mut entries: Vec<_> = self.0.iter().collect();
        entries.sort_by_key(|(pubkey, _)| pubkey.to_string());

        // Create ordered map and serialize
        let mut map = serde_json::Map::new();
        for (pubkey, amount) in entries {
            map.insert(pubkey.to_string(), json!(*amount));
        }

        // NOTE: using json so it matches what happens in python

        let json_string =
            serde_json::to_string(&map).expect("Failed to serialize PayoutStructure to JSON");

        json_string.into_bytes()
    }
}

impl Serialize for PayoutStructure {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serde_json::Map::new();
        for (pubkey, amount) in &self.0 {
            map.insert(pubkey.to_string(), json!(*amount));
        }
        let json_string = serde_json::to_string(&map).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&json_string)
    }
}

impl<'de> Deserialize<'de> for PayoutStructure {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let temp_map: HashMap<String, u64> =
            serde_json::from_str(&s).map_err(serde::de::Error::custom)?;

        let mut map = HashMap::new();
        for (key_str, value) in temp_map {
            let pubkey = PublicKey::from_str(&key_str)
                .map_err(|e| serde::de::Error::custom(format!("Invalid public key: {}", e)))?;
            map.insert(pubkey, value);
        }

        Ok(PayoutStructure(map))
    }
}

#[derive(Serialize, Deserialize, Debug)]
/// DLC outcome
pub struct DLCOutcome {
    #[serde(rename = "k")]
    /// see https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/dlc.md#payout-structures
    pub blinded_attestation_secret: String,
    #[serde(rename = "P")]
    /// [`PayoutStructure`] for this outcome
    pub payout_structure: PayoutStructure,
}

#[derive(Serialize, Deserialize, Debug)]
/// Settled DLC
pub struct DLCSettlement {
    /// DLC Root
    pub dlc_root: String,
    /// [`DLCOutcome`] for this settlement
    pub outcome: DLCOutcome,
    /// Mekrle proof that the outcome is in the root
    pub merkle_proof: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
/// POST request body for /v1/dlc/settle
pub struct PostSettleDLCRequest {
    /// [`DLCSettlement`]s to settle
    pub settlements: Vec<DLCSettlement>,
}

#[derive(Serialize, Deserialize, Debug)]
/// Error for [`DLCSettlement`]
pub struct DLCSettlementError {
    /// DLC Root
    dlc_root: String,
    /// Detail of the error
    detail: String,
}

#[derive(Serialize, Deserialize, Debug)]
/// Settled DLC
pub struct SettledDLC {
    /// DLC Root
    pub dlc_root: String,
}

#[derive(Serialize, Deserialize, Debug)]
/// Response for /v1/dlc/settle
pub struct SettleDLCResponse {
    /// Settled DLCs
    pub settled: Vec<SettledDLC>,
    /// Errors
    pub errors: Option<Vec<DLCSettlementError>>,
}

/// Response for /v1/dlc/status/{dlc_root}
#[derive(Serialize, Deserialize, Debug)]
pub struct DLCStatusResponse {
    /// Whether the DLC is settled
    pub settled: bool,
    /// If not settled
    pub funding_amount: Option<u64>,
    /// If settled
    pub debts: Option<HashMap<String, u64>>,
    /// Unit
    pub unit: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
/// Witness to prove ownership of pubkey in [`ClaimDLCPayout`]
pub struct DLCPayoutWitness {
    ///  discrete log (private key) of `Payout.pubkey` (either parity)
    pub secret: Option<String>,
    /// BIP-340 signature made by `Payout.pubkey` on `Payout.dlc_root`
    pub signature: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
/// ClaimDLCPayout
pub struct ClaimDLCPayout {
    /// DLC root hash
    pub dlc_root: String,
    /// Public key of the payout
    pub pubkey: String,
    /// Blinded outputs to be signed
    pub outputs: Vec<BlindedMessage>,
    /// [`DLCPayoutWitness`]
    pub witness: DLCPayoutWitness,
}

#[derive(Serialize, Deserialize, Debug)]
/// Request for /v1/dlc/payout
pub struct PostDLCPayoutRequest {
    /// Payouts being claimed
    pub payouts: Vec<ClaimDLCPayout>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
/// Successful payout for a DLC
pub struct DLCPayout {
    /// DLC root hash
    pub dlc_root: String,
    /// Blinded signatures on outputs
    pub outputs: Vec<BlindSignature>,
}

#[derive(Serialize, Deserialize, Debug)]
/// Error details for a failed DLC payout
pub struct DLCPayoutError {
    /// DLC root hash
    pub dlc_root: String,
    /// Error details
    pub detail: String,
}

#[derive(Serialize, Deserialize, Debug)]
/// Response for /v1/dlc/payout
pub struct PostDLCPayoutResponse {
    /// Successfully paid DLCs
    pub paid: Vec<DLCPayout>,
    /// Errors for failed payouts
    pub errors: Option<Vec<DLCPayoutError>>,
}

// Known Parameters
/*
- The number of possible outcomes `n`

- An outcome blinding secret scalar `b`

- A vector of `n` outcome locking points `[K1, K2, ... Kn]`

- A vector of `n` payout structures `[P1, P2, ... Pn]`

- A vector of `n` payout structures `[P1, P2, ... Pn]`

- An optional timeout timestamp `t` and timeout payout structure `Pt`
*/

// b = random secret scalar
// SecretKey::generate()

// blinding points
/*
Ki_ = Ki + b*G
*/
