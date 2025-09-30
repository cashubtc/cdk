//! Proof-related FFI types

use std::str::FromStr;

use cdk::nuts::State as CdkState;
use serde::{Deserialize, Serialize};

use super::amount::{Amount, CurrencyUnit};
use super::mint::MintUrl;
use crate::error::FfiError;

/// FFI-compatible Proof state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum ProofState {
    Unspent,
    Pending,
    Spent,
    Reserved,
    PendingSpent,
}

impl From<CdkState> for ProofState {
    fn from(state: CdkState) -> Self {
        match state {
            CdkState::Unspent => ProofState::Unspent,
            CdkState::Pending => ProofState::Pending,
            CdkState::Spent => ProofState::Spent,
            CdkState::Reserved => ProofState::Reserved,
            CdkState::PendingSpent => ProofState::PendingSpent,
        }
    }
}

impl From<ProofState> for CdkState {
    fn from(state: ProofState) -> Self {
        match state {
            ProofState::Unspent => CdkState::Unspent,
            ProofState::Pending => CdkState::Pending,
            ProofState::Spent => CdkState::Spent,
            ProofState::Reserved => CdkState::Reserved,
            ProofState::PendingSpent => CdkState::PendingSpent,
        }
    }
}

/// FFI-compatible Proof
#[derive(Debug, uniffi::Object)]
pub struct Proof {
    pub(crate) inner: cdk::nuts::Proof,
}

impl From<cdk::nuts::Proof> for Proof {
    fn from(proof: cdk::nuts::Proof) -> Self {
        Self { inner: proof }
    }
}

impl From<Proof> for cdk::nuts::Proof {
    fn from(proof: Proof) -> Self {
        proof.inner
    }
}

#[uniffi::export]
impl Proof {
    /// Get the amount
    pub fn amount(&self) -> Amount {
        self.inner.amount.into()
    }

    /// Get the secret as string
    pub fn secret(&self) -> String {
        self.inner.secret.to_string()
    }

    /// Get the unblinded signature (C) as string
    pub fn c(&self) -> String {
        self.inner.c.to_string()
    }

    /// Get the keyset ID as string
    pub fn keyset_id(&self) -> String {
        self.inner.keyset_id.to_string()
    }

    /// Get the witness
    pub fn witness(&self) -> Option<Witness> {
        self.inner.witness.as_ref().map(|w| w.clone().into())
    }

    /// Check if proof is active with given keyset IDs
    pub fn is_active(&self, active_keyset_ids: Vec<String>) -> bool {
        use cdk::nuts::Id;
        let ids: Vec<Id> = active_keyset_ids
            .into_iter()
            .filter_map(|id| Id::from_str(&id).ok())
            .collect();
        self.inner.is_active(&ids)
    }

    /// Get the Y value (hash_to_curve of secret)
    pub fn y(&self) -> Result<String, FfiError> {
        Ok(self.inner.y()?.to_string())
    }

    /// Get the DLEQ proof if present
    pub fn dleq(&self) -> Option<ProofDleq> {
        self.inner.dleq.as_ref().map(|d| d.clone().into())
    }

    /// Check if proof has DLEQ proof
    pub fn has_dleq(&self) -> bool {
        self.inner.dleq.is_some()
    }
}

/// FFI-compatible Proofs (vector of Proof)
pub type Proofs = Vec<std::sync::Arc<Proof>>;

/// FFI-compatible DLEQ proof for proofs
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ProofDleq {
    /// e value (hex-encoded SecretKey)
    pub e: String,
    /// s value (hex-encoded SecretKey)
    pub s: String,
    /// r value - blinding factor (hex-encoded SecretKey)
    pub r: String,
}

/// FFI-compatible DLEQ proof for blind signatures
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct BlindSignatureDleq {
    /// e value (hex-encoded SecretKey)
    pub e: String,
    /// s value (hex-encoded SecretKey)
    pub s: String,
}

impl From<cdk::nuts::ProofDleq> for ProofDleq {
    fn from(dleq: cdk::nuts::ProofDleq) -> Self {
        Self {
            e: dleq.e.to_secret_hex(),
            s: dleq.s.to_secret_hex(),
            r: dleq.r.to_secret_hex(),
        }
    }
}

impl From<ProofDleq> for cdk::nuts::ProofDleq {
    fn from(dleq: ProofDleq) -> Self {
        Self {
            e: cdk::nuts::SecretKey::from_hex(&dleq.e).expect("Invalid e hex"),
            s: cdk::nuts::SecretKey::from_hex(&dleq.s).expect("Invalid s hex"),
            r: cdk::nuts::SecretKey::from_hex(&dleq.r).expect("Invalid r hex"),
        }
    }
}

impl From<cdk::nuts::BlindSignatureDleq> for BlindSignatureDleq {
    fn from(dleq: cdk::nuts::BlindSignatureDleq) -> Self {
        Self {
            e: dleq.e.to_secret_hex(),
            s: dleq.s.to_secret_hex(),
        }
    }
}

impl From<BlindSignatureDleq> for cdk::nuts::BlindSignatureDleq {
    fn from(dleq: BlindSignatureDleq) -> Self {
        Self {
            e: cdk::nuts::SecretKey::from_hex(&dleq.e).expect("Invalid e hex"),
            s: cdk::nuts::SecretKey::from_hex(&dleq.s).expect("Invalid s hex"),
        }
    }
}

/// Helper functions for Proofs
pub fn proofs_total_amount(proofs: &Proofs) -> Result<Amount, FfiError> {
    let cdk_proofs: Vec<cdk::nuts::Proof> = proofs.iter().map(|p| p.inner.clone()).collect();
    use cdk::nuts::ProofsMethods;
    Ok(cdk_proofs.total_amount()?.into())
}

/// FFI-compatible Conditions (for spending conditions)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct Conditions {
    /// Unix locktime after which refund keys can be used
    pub locktime: Option<u64>,
    /// Additional Public keys (as hex strings)
    pub pubkeys: Vec<String>,
    /// Refund keys (as hex strings)
    pub refund_keys: Vec<String>,
    /// Number of signatures required (default 1)
    pub num_sigs: Option<u64>,
    /// Signature flag (0 = SigInputs, 1 = SigAll)
    pub sig_flag: u8,
    /// Number of refund signatures required (default 1)
    pub num_sigs_refund: Option<u64>,
}

impl From<cdk::nuts::nut11::Conditions> for Conditions {
    fn from(conditions: cdk::nuts::nut11::Conditions) -> Self {
        Self {
            locktime: conditions.locktime,
            pubkeys: conditions
                .pubkeys
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.to_string())
                .collect(),
            refund_keys: conditions
                .refund_keys
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.to_string())
                .collect(),
            num_sigs: conditions.num_sigs,
            sig_flag: match conditions.sig_flag {
                cdk::nuts::nut11::SigFlag::SigInputs => 0,
                cdk::nuts::nut11::SigFlag::SigAll => 1,
            },
            num_sigs_refund: conditions.num_sigs_refund,
        }
    }
}

impl TryFrom<Conditions> for cdk::nuts::nut11::Conditions {
    type Error = FfiError;

    fn try_from(conditions: Conditions) -> Result<Self, Self::Error> {
        let pubkeys = if conditions.pubkeys.is_empty() {
            None
        } else {
            Some(
                conditions
                    .pubkeys
                    .into_iter()
                    .map(|s| {
                        s.parse().map_err(|e| FfiError::InvalidCryptographicKey {
                            msg: format!("Invalid pubkey: {}", e),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )
        };

        let refund_keys = if conditions.refund_keys.is_empty() {
            None
        } else {
            Some(
                conditions
                    .refund_keys
                    .into_iter()
                    .map(|s| {
                        s.parse().map_err(|e| FfiError::InvalidCryptographicKey {
                            msg: format!("Invalid refund key: {}", e),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )
        };

        let sig_flag = match conditions.sig_flag {
            0 => cdk::nuts::nut11::SigFlag::SigInputs,
            1 => cdk::nuts::nut11::SigFlag::SigAll,
            _ => {
                return Err(FfiError::Generic {
                    msg: "Invalid sig_flag value".to_string(),
                })
            }
        };

        Ok(Self {
            locktime: conditions.locktime,
            pubkeys,
            refund_keys,
            num_sigs: conditions.num_sigs,
            sig_flag,
            num_sigs_refund: conditions.num_sigs_refund,
        })
    }
}

impl Conditions {
    /// Convert Conditions to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode Conditions from JSON string
#[uniffi::export]
pub fn decode_conditions(json: String) -> Result<Conditions, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode Conditions to JSON string
#[uniffi::export]
pub fn encode_conditions(conditions: Conditions) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&conditions)?)
}

/// FFI-compatible Witness
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
pub enum Witness {
    /// P2PK Witness
    P2PK {
        /// Signatures
        signatures: Vec<String>,
    },
    /// HTLC Witness
    HTLC {
        /// Preimage
        preimage: String,
        /// Optional signatures
        signatures: Option<Vec<String>>,
    },
}

impl From<cdk::nuts::Witness> for Witness {
    fn from(witness: cdk::nuts::Witness) -> Self {
        match witness {
            cdk::nuts::Witness::P2PKWitness(p2pk) => Self::P2PK {
                signatures: p2pk.signatures,
            },
            cdk::nuts::Witness::HTLCWitness(htlc) => Self::HTLC {
                preimage: htlc.preimage,
                signatures: htlc.signatures,
            },
        }
    }
}

impl From<Witness> for cdk::nuts::Witness {
    fn from(witness: Witness) -> Self {
        match witness {
            Witness::P2PK { signatures } => {
                Self::P2PKWitness(cdk::nuts::nut11::P2PKWitness { signatures })
            }
            Witness::HTLC {
                preimage,
                signatures,
            } => Self::HTLCWitness(cdk::nuts::nut14::HTLCWitness {
                preimage,
                signatures,
            }),
        }
    }
}

/// FFI-compatible SpendingConditions
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
pub enum SpendingConditions {
    /// P2PK (Pay to Public Key) conditions
    P2PK {
        /// The public key (as hex string)
        pubkey: String,
        /// Additional conditions
        conditions: Option<Conditions>,
    },
    /// HTLC (Hash Time Locked Contract) conditions
    HTLC {
        /// Hash of the preimage (as hex string)
        hash: String,
        /// Additional conditions
        conditions: Option<Conditions>,
    },
}

impl From<cdk::nuts::SpendingConditions> for SpendingConditions {
    fn from(spending_conditions: cdk::nuts::SpendingConditions) -> Self {
        match spending_conditions {
            cdk::nuts::SpendingConditions::P2PKConditions { data, conditions } => Self::P2PK {
                pubkey: data.to_string(),
                conditions: conditions.map(Into::into),
            },
            cdk::nuts::SpendingConditions::HTLCConditions { data, conditions } => Self::HTLC {
                hash: data.to_string(),
                conditions: conditions.map(Into::into),
            },
        }
    }
}

impl TryFrom<SpendingConditions> for cdk::nuts::SpendingConditions {
    type Error = FfiError;

    fn try_from(spending_conditions: SpendingConditions) -> Result<Self, Self::Error> {
        match spending_conditions {
            SpendingConditions::P2PK { pubkey, conditions } => {
                let pubkey = pubkey
                    .parse()
                    .map_err(|e| FfiError::InvalidCryptographicKey {
                        msg: format!("Invalid pubkey: {}", e),
                    })?;
                let conditions = conditions.map(|c| c.try_into()).transpose()?;
                Ok(Self::P2PKConditions {
                    data: pubkey,
                    conditions,
                })
            }
            SpendingConditions::HTLC { hash, conditions } => {
                let hash = hash
                    .parse()
                    .map_err(|e| FfiError::InvalidCryptographicKey {
                        msg: format!("Invalid hash: {}", e),
                    })?;
                let conditions = conditions.map(|c| c.try_into()).transpose()?;
                Ok(Self::HTLCConditions {
                    data: hash,
                    conditions,
                })
            }
        }
    }
}

/// FFI-compatible ProofInfo
#[derive(Debug, Clone, uniffi::Record)]
pub struct ProofInfo {
    /// Proof
    pub proof: std::sync::Arc<Proof>,
    /// Y value (hash_to_curve of secret)
    pub y: super::keys::PublicKey,
    /// Mint URL
    pub mint_url: MintUrl,
    /// Proof state
    pub state: ProofState,
    /// Proof Spending Conditions
    pub spending_condition: Option<SpendingConditions>,
    /// Currency unit
    pub unit: CurrencyUnit,
}

impl From<cdk::types::ProofInfo> for ProofInfo {
    fn from(info: cdk::types::ProofInfo) -> Self {
        Self {
            proof: std::sync::Arc::new(info.proof.into()),
            y: info.y.into(),
            mint_url: info.mint_url.into(),
            state: info.state.into(),
            spending_condition: info.spending_condition.map(Into::into),
            unit: info.unit.into(),
        }
    }
}

/// Decode ProofInfo from JSON string
#[uniffi::export]
pub fn decode_proof_info(json: String) -> Result<ProofInfo, FfiError> {
    let info: cdk::types::ProofInfo = serde_json::from_str(&json)?;
    Ok(info.into())
}

/// Encode ProofInfo to JSON string
#[uniffi::export]
pub fn encode_proof_info(info: ProofInfo) -> Result<String, FfiError> {
    // Convert to cdk::types::ProofInfo for serialization
    let cdk_info = cdk::types::ProofInfo {
        proof: info.proof.inner.clone(),
        y: info.y.try_into()?,
        mint_url: info.mint_url.try_into()?,
        state: info.state.into(),
        spending_condition: info.spending_condition.and_then(|c| c.try_into().ok()),
        unit: info.unit.into(),
    };
    Ok(serde_json::to_string(&cdk_info)?)
}

/// FFI-compatible ProofStateUpdate
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ProofStateUpdate {
    /// Y value (hash_to_curve of secret)
    pub y: String,
    /// Current state
    pub state: ProofState,
    /// Optional witness data
    pub witness: Option<String>,
}

impl From<cdk::nuts::nut07::ProofState> for ProofStateUpdate {
    fn from(proof_state: cdk::nuts::nut07::ProofState) -> Self {
        Self {
            y: proof_state.y.to_string(),
            state: proof_state.state.into(),
            witness: proof_state.witness.map(|w| format!("{:?}", w)),
        }
    }
}

impl ProofStateUpdate {
    /// Convert ProofStateUpdate to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode ProofStateUpdate from JSON string
#[uniffi::export]
pub fn decode_proof_state_update(json: String) -> Result<ProofStateUpdate, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode ProofStateUpdate to JSON string
#[uniffi::export]
pub fn encode_proof_state_update(update: ProofStateUpdate) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&update)?)
}
