//! NUT-10: Spending conditions
//!
//! <https://github.com/cashubtc/nuts/blob/main/10.md>

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Serialize, Serializer};
use thiserror::Error;

use super::nut01::PublicKey;
use super::Conditions;

/// Spending requirements for P2PK or HTLC verification
///
/// Returned by `get_pubkeys_and_required_sigs` to indicate what conditions
/// must be met to spend a proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpendingRequirements {
    /// Whether a preimage is required (HTLC only, before locktime)
    pub preimage_needed: bool,
    /// Public keys that can provide valid signatures
    pub pubkeys: Vec<PublicKey>,
    /// Minimum number of signatures required from the pubkeys
    pub required_sigs: u64,
}

/// NUT13 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Secret error
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
}

///  NUT10 Secret Kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    /// NUT-11 P2PK
    P2PK,
    /// NUT-14 HTLC
    HTLC,
}

/// Secret Date
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretData {
    /// Unique random string
    nonce: String,
    /// Expresses the spending condition specific to each kind
    data: String,
    /// Additional data committed to and can be used for feature extensions
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<Vec<String>>>,
}

impl SecretData {
    /// Create new [`SecretData`]
    pub fn new<S, V>(data: S, tags: Option<V>) -> Self
    where
        S: Into<String>,
        V: Into<Vec<Vec<String>>>,
    {
        let nonce = crate::secret::Secret::generate().to_string();

        Self {
            nonce,
            data: data.into(),
            tags: tags.map(|v| v.into()),
        }
    }

    /// Get the nonce
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Get the data
    pub fn data(&self) -> &str {
        &self.data
    }

    /// Get the tags
    pub fn tags(&self) -> Option<&Vec<Vec<String>>> {
        self.tags.as_ref()
    }
}

/// NUT10 Secret
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Secret {
    ///  Kind of the spending condition
    kind: Kind,
    /// Secret Data
    secret_data: SecretData,
}

impl Secret {
    /// Create new [`Secret`]
    pub fn new<S, V>(kind: Kind, data: S, tags: Option<V>) -> Self
    where
        S: Into<String>,
        V: Into<Vec<Vec<String>>>,
    {
        let secret_data = SecretData::new(data, tags);
        Self { kind, secret_data }
    }

    /// Get the kind
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Get the secret data
    pub fn secret_data(&self) -> &SecretData {
        &self.secret_data
    }
}

/// Get the relevant public keys and required signature count for P2PK or HTLC verification
/// This is for NUT-11(P2PK) and NUT-14(HTLC)
///
/// Takes into account locktime - if locktime has passed, returns refund keys,
/// otherwise returns primary pubkeys/hash path.
/// From NUT-11: "If the tag locktime is the unix time and the mint's local clock is greater than
/// locktime, the Proof becomes spendable by anyone, except [... if refund keys are specified]"
///
/// Returns `SpendingRequirements` containing:
/// - `preimage_needed`: For P2PK, always false. For HTLC, true before locktime.
/// - `pubkeys`: The public keys that can provide valid signatures
/// - `required_sigs`: The minimum number of signatures required
///
/// From NUT-14: "if the current system time is later than Secret.tag.locktime, the Proof can
/// be spent if Proof.witness includes a signature from the key in Secret.tags.refund."
pub(crate) fn get_pubkeys_and_required_sigs(
    secret: &Secret,
    current_time: u64,
) -> Result<SpendingRequirements, super::nut11::Error> {
    debug_assert!(
        secret.kind() == Kind::P2PK || secret.kind() == Kind::HTLC,
        "get_pubkeys_and_required_sigs called with invalid kind - this is a bug"
    );

    let conditions: Conditions = secret
        .secret_data()
        .tags()
        .cloned()
        .unwrap_or_default()
        .try_into()?;

    // Check if locktime has passed
    let locktime_passed = conditions
        .locktime
        .map(|locktime| locktime < current_time)
        .unwrap_or(false);

    // Determine which keys and signature count to use
    if locktime_passed {
        // After locktime: use refund path (no preimage needed)
        if let Some(refund_keys) = &conditions.refund_keys {
            // Locktime has passed and refund keys exist - use refund keys
            let refund_sigs = conditions.num_sigs_refund.unwrap_or(1);
            Ok(SpendingRequirements {
                preimage_needed: false,
                pubkeys: refund_keys.clone(),
                required_sigs: refund_sigs,
            })
        } else {
            // Locktime has passed with no refund keys - anyone can spend
            Ok(SpendingRequirements {
                preimage_needed: false,
                pubkeys: vec![],
                required_sigs: 0,
            })
        }
    } else {
        // Before locktime: logic differs between P2PK and HTLC
        match secret.kind() {
            Kind::P2PK => {
                // P2PK: never needs preimage, use primary pubkeys
                let mut primary_keys = vec![];

                // Add the pubkey from secret.data
                let data_pubkey = PublicKey::from_str(secret.secret_data().data())?;
                primary_keys.push(data_pubkey);

                // Add any additional pubkeys from conditions
                if let Some(additional_keys) = &conditions.pubkeys {
                    primary_keys.extend(additional_keys.clone());
                }

                let primary_num_sigs_required = conditions.num_sigs.unwrap_or(1);
                Ok(SpendingRequirements {
                    preimage_needed: false,
                    pubkeys: primary_keys,
                    required_sigs: primary_num_sigs_required,
                })
            }
            Kind::HTLC => {
                // HTLC: needs preimage before locktime, pubkeys from conditions
                // (data contains hash, not pubkey)
                let pubkeys = conditions.pubkeys.clone().unwrap_or_default();
                // If no pubkeys are specified, require 0 signatures (only preimage needed)
                // Otherwise, default to requiring 1 signature
                let required_sigs = if pubkeys.is_empty() {
                    0
                } else {
                    conditions.num_sigs.unwrap_or(1)
                };
                Ok(SpendingRequirements {
                    preimage_needed: true,
                    pubkeys,
                    required_sigs,
                })
            }
        }
    }
}

use super::Proofs;

/// Verify that a preimage matches the hash in the secret data
///
/// The preimage should be a 64-character hex string representing 32 bytes.
/// We decode it from hex, hash it with SHA256, and compare to the hash in secret.data
pub fn verify_htlc_preimage(
    witness: &super::nut14::HTLCWitness,
    secret: &Secret,
) -> Result<(), super::nut14::Error> {
    use bitcoin::hashes::sha256::Hash as Sha256Hash;
    use bitcoin::hashes::Hash;

    // Get the hash lock from the secret data
    let hash_lock = Sha256Hash::from_str(secret.secret_data().data())
        .map_err(|_| super::nut14::Error::InvalidHash)?;

    // Decode and validate the preimage (returns [u8; 32])
    let preimage_bytes = witness.preimage_data()?;

    // Hash the 32-byte preimage
    let preimage_hash = Sha256Hash::hash(&preimage_bytes);

    // Compare with the hash lock
    if hash_lock.ne(&preimage_hash) {
        return Err(super::nut14::Error::Preimage);
    }

    Ok(())
}

/// Trait for requests that spend proofs (SwapRequest, MeltRequest)
pub trait SpendingConditionVerification {
    /// Get the input proofs
    fn inputs(&self) -> &Proofs;

    /// Construct the message to sign for SIG_ALL verification
    ///
    /// This concatenates all relevant transaction data that must be signed.
    /// For swap: input secrets + output blinded messages
    /// For melt: input secrets + quote/payment request
    fn sig_all_msg_to_sign(&self) -> String;

    /// Check if at least one proof in the set has SIG_ALL flag set
    ///
    /// SIG_ALL requires all proofs in the transaction to be signed.
    /// If any proof has this flag, we need to verify signatures on all proofs.
    fn has_at_least_one_sig_all(&self) -> Result<bool, super::nut11::Error> {
        for proof in self.inputs() {
            // Try to extract spending conditions from the proof's secret
            if let Ok(spending_conditions) = super::SpendingConditions::try_from(&proof.secret) {
                // Check for SIG_ALL flag in either P2PK or HTLC conditions
                let has_sig_all = match spending_conditions {
                    super::SpendingConditions::P2PKConditions { conditions, .. } => conditions
                        .map(|c| c.sig_flag == super::SigFlag::SigAll)
                        .unwrap_or(false),
                    super::SpendingConditions::HTLCConditions { conditions, .. } => conditions
                        .map(|c| c.sig_flag == super::SigFlag::SigAll)
                        .unwrap_or(false),
                };

                if has_sig_all {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Verify all inputs meet SIG_ALL requirements per NUT-11
    ///
    /// When any input has SIG_ALL, all inputs must have:
    /// 1. Same kind (P2PK or HTLC)
    /// 2. SIG_ALL flag set
    /// 3. Same Secret.data
    /// 4. Same Secret.tags
    fn verify_all_inputs_match_for_sig_all(&self) -> Result<(), super::nut11::Error> {
        let inputs = self.inputs();

        // Get first input's properties
        let first_input = inputs
            .first()
            .ok_or(super::nut11::Error::SpendConditionsNotMet)?;
        let first_secret = Secret::try_from(&first_input.secret)
            .map_err(|_| super::nut11::Error::IncorrectSecretKind)?;
        let first_kind = first_secret.kind();
        let first_data = first_secret.secret_data().data();
        let first_tags = first_secret.secret_data().tags();

        // Get first input's conditions to check SIG_ALL flag
        let first_conditions =
            super::Conditions::try_from(first_tags.cloned().unwrap_or_default())?;

        // Verify first input has SIG_ALL (it should, since we only call this function when SIG_ALL is detected)
        if first_conditions.sig_flag != super::SigFlag::SigAll {
            return Err(super::nut11::Error::SpendConditionsNotMet);
        }

        // Verify all remaining inputs match
        for proof in inputs.iter().skip(1) {
            let secret = Secret::try_from(&proof.secret)
                .map_err(|_| super::nut11::Error::IncorrectSecretKind)?;

            // Check kind matches
            if secret.kind() != first_kind {
                return Err(super::nut11::Error::SpendConditionsNotMet);
            }

            // Check data matches
            if secret.secret_data().data() != first_data {
                return Err(super::nut11::Error::SpendConditionsNotMet);
            }

            // Check tags match (this also ensures SIG_ALL flag matches, since sig_flag is part of tags)
            if secret.secret_data().tags() != first_tags {
                return Err(super::nut11::Error::SpendConditionsNotMet);
            }
        }

        Ok(())
    }

    /// Verify spending conditions for this transaction
    ///
    /// This is the main entry point for spending condition verification.
    /// It checks if any input has SIG_ALL and dispatches to the appropriate verification path.
    fn verify_spending_conditions(&self) -> Result<(), super::nut11::Error> {
        // Check if any input has SIG_ALL flag
        if self.has_at_least_one_sig_all()? {
            // at least one input has SIG_ALL
            self.verify_full_sig_all_check()
        } else {
            // none of the inputs are SIG_ALL, so we can simply check
            // each independently and verify any spending conditions
            // that may - or may not - be there.
            self.verify_inputs_individually().map_err(|e| match e {
                super::nut14::Error::NUT11(nut11_err) => nut11_err,
                _ => super::nut11::Error::SpendConditionsNotMet,
            })
        }
    }

    /// Verify spending conditions when SIG_ALL is present
    ///
    /// When SIG_ALL is set, all proofs in the transaction must be signed together.
    fn verify_full_sig_all_check(&self) -> Result<(), super::nut11::Error> {
        debug_assert!(
            self.has_at_least_one_sig_all()?,
            "verify_full_sig_all_check() called on proofs without SIG_ALL. This shouldn't happen"
        );
        // Verify all inputs meet SIG_ALL requirements per NUT-11:
        // All inputs must have: (1) same kind, (2) SIG_ALL flag, (3) same data, (4) same tags
        self.verify_all_inputs_match_for_sig_all()?;

        // Get the first input to determine the kind
        let first_input = self
            .inputs()
            .first()
            .ok_or(super::nut11::Error::SpendConditionsNotMet)?;
        let first_secret = Secret::try_from(&first_input.secret)
            .map_err(|_| super::nut11::Error::IncorrectSecretKind)?;

        // Dispatch based on secret kind
        match first_secret.kind() {
            Kind::P2PK => {
                self.verify_sig_all_p2pk()?;
            }
            Kind::HTLC => {
                self.verify_sig_all_htlc()?;
            }
        }

        Ok(())
    }

    /// Verify spending conditions for each input individually
    ///
    /// Handles SIG_INPUTS mode, non-NUT-10 secrets, and any other case where inputs
    /// are verified independently rather than as a group.
    /// This function will NOT be called if any input has SIG_ALL.
    fn verify_inputs_individually(&self) -> Result<(), super::nut14::Error> {
        debug_assert!(
            !(self.has_at_least_one_sig_all()?),
            "verify_inputs_individually() called on SIG_ALL. This shouldn't happen"
        );
        for proof in self.inputs() {
            // Check if secret is a nut10 secret with conditions
            if let Ok(secret) = Secret::try_from(&proof.secret) {
                // Verify this function isn't being called with SIG_ALL proofs (development check)
                if let Ok(conditions) = super::Conditions::try_from(
                    secret.secret_data().tags().cloned().unwrap_or_default(),
                ) {
                    debug_assert!(
                        conditions.sig_flag != super::SigFlag::SigAll,
                        "verify_inputs_individually called with SIG_ALL proof - this is a bug"
                    );
                }

                match secret.kind() {
                    Kind::P2PK => {
                        proof.verify_p2pk()?;
                    }
                    Kind::HTLC => {
                        proof.verify_htlc()?;
                    }
                }
            }
            // If not a nut10 secret, skip verification (plain secret)
        }
        Ok(())
    }

    /// Verify P2PK SIG_ALL signatures
    ///
    /// Do NOT call this directly. This is called only from 'verify_full_sig_all_check',
    /// which has already done many important SIG_ALL checks. This performs the final
    /// signature verification for SIG_ALL+P2PK transactions.
    fn verify_sig_all_p2pk(&self) -> Result<(), super::nut11::Error> {
        // Get the first input, as it's the one with the signatures
        let first_input = self
            .inputs()
            .first()
            .ok_or(super::nut11::Error::SpendConditionsNotMet)?;
        let first_secret = Secret::try_from(&first_input.secret)
            .map_err(|_| super::nut11::Error::IncorrectSecretKind)?;

        // Record current time for locktime evaluation
        let current_time = crate::util::unix_time();

        // Get the relevant public keys and required signature count based on locktime
        let requirements = get_pubkeys_and_required_sigs(&first_secret, current_time)?;

        debug_assert!(
            !requirements.preimage_needed,
            "P2PK should never require preimage"
        );

        // Handle "anyone can spend" case (locktime passed with no refund keys)
        if requirements.required_sigs == 0 {
            return Ok(());
        }

        // Construct the message that should be signed
        let msg_to_sign = self.sig_all_msg_to_sign();

        // Extract signatures from the first input's witness
        let first_witness = first_input
            .witness
            .as_ref()
            .ok_or(super::nut11::Error::SignaturesNotProvided)?;

        let witness_sigs = first_witness
            .signatures()
            .ok_or(super::nut11::Error::SignaturesNotProvided)?;

        // Convert witness strings to Signature objects
        use std::str::FromStr;
        let signatures: Vec<bitcoin::secp256k1::schnorr::Signature> = witness_sigs
            .iter()
            .map(|s| bitcoin::secp256k1::schnorr::Signature::from_str(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| super::nut11::Error::InvalidSignature)?;

        // Verify signatures using the existing valid_signatures function
        let valid_sig_count = super::nut11::valid_signatures(
            msg_to_sign.as_bytes(),
            &requirements.pubkeys,
            &signatures,
        )?;

        // Check if we have enough valid signatures
        if valid_sig_count < requirements.required_sigs {
            return Err(super::nut11::Error::SpendConditionsNotMet);
        }

        Ok(())
    }

    /// Verify HTLC SIG_ALL signatures
    ///
    /// Do NOT call this directly. This is called only from 'verify_full_sig_all_check',
    /// which has already done many important SIG_ALL checks. This performs the final
    /// signature verification for SIG_ALL+HTLC transactions.
    fn verify_sig_all_htlc(&self) -> Result<(), super::nut11::Error> {
        // Get the first input, as it's the one with the signatures
        let first_input = self
            .inputs()
            .first()
            .ok_or(super::nut11::Error::SpendConditionsNotMet)?;
        let first_secret = Secret::try_from(&first_input.secret)
            .map_err(|_| super::nut11::Error::IncorrectSecretKind)?;

        // Record current time for locktime evaluation
        let current_time = crate::util::unix_time();

        // Get the relevant public keys, required signature count, and whether preimage is needed
        let requirements = get_pubkeys_and_required_sigs(&first_secret, current_time)?;

        // If preimage is needed (before locktime), verify it
        if requirements.preimage_needed {
            // Extract HTLC witness
            let htlc_witness = match first_input.witness.as_ref() {
                Some(super::Witness::HTLCWitness(witness)) => witness,
                _ => return Err(super::nut11::Error::SignaturesNotProvided),
            };

            // Verify the preimage matches the hash in the secret
            verify_htlc_preimage(htlc_witness, &first_secret)
                .map_err(|_| super::nut11::Error::SpendConditionsNotMet)?;
        }

        // Handle "anyone can spend" case (locktime passed with no refund keys)
        if requirements.required_sigs == 0 {
            return Ok(());
        }

        // Construct the message that should be signed
        let msg_to_sign = self.sig_all_msg_to_sign();

        // Extract signatures from the first input's witness
        let first_witness = first_input
            .witness
            .as_ref()
            .ok_or(super::nut11::Error::SignaturesNotProvided)?;

        let witness_sigs = first_witness
            .signatures()
            .ok_or(super::nut11::Error::SignaturesNotProvided)?;

        // Convert witness strings to Signature objects
        use std::str::FromStr;
        let signatures: Vec<bitcoin::secp256k1::schnorr::Signature> = witness_sigs
            .iter()
            .map(|s| bitcoin::secp256k1::schnorr::Signature::from_str(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| super::nut11::Error::InvalidSignature)?;

        // Verify signatures using the existing valid_signatures function
        let valid_sig_count = super::nut11::valid_signatures(
            msg_to_sign.as_bytes(),
            &requirements.pubkeys,
            &signatures,
        )?;

        // Check if we have enough valid signatures
        if valid_sig_count < requirements.required_sigs {
            return Err(super::nut11::Error::SpendConditionsNotMet);
        }

        Ok(())
    }
}

impl Serialize for Secret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Create a tuple representing the struct fields
        let secret_tuple = (&self.kind, &self.secret_data);

        // Serialize the tuple as a JSON array
        let mut s = serializer.serialize_tuple(2)?;

        s.serialize_element(&secret_tuple.0)?;
        s.serialize_element(&secret_tuple.1)?;
        s.end()
    }
}

impl TryFrom<Secret> for crate::secret::Secret {
    type Error = Error;
    fn try_from(secret: Secret) -> Result<crate::secret::Secret, Self::Error> {
        Ok(crate::secret::Secret::from_str(&serde_json::to_string(
            &secret,
        )?)?)
    }
}

// Custom visitor for deserializing Secret
struct SecretVisitor;

impl<'de> Visitor<'de> for SecretVisitor {
    type Value = Secret;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a tuple with two elements: [Kind, SecretData]")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        // Deserialize the kind (first element)
        let kind = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;

        // Deserialize the secret_data (second element)
        let secret_data = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(1, &self))?;

        // Make sure there are no additional elements
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(de::Error::invalid_length(3, &self));
        }

        Ok(Secret { kind, secret_data })
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(SecretVisitor)
    }
}

#[cfg(test)]
mod tests {
    use std::assert_eq;
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_secret_serialize() {
        let secret = Secret {
            kind: Kind::P2PK,
            secret_data: SecretData {
                nonce: "5d11913ee0f92fefdc82a6764fd2457a".to_string(),
                data: "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"
                    .to_string(),
                tags: Some(vec![vec![
                    "key".to_string(),
                    "value1".to_string(),
                    "value2".to_string(),
                ]]),
            },
        };

        let secret_str = r#"["P2PK",{"nonce":"5d11913ee0f92fefdc82a6764fd2457a","data":"026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198","tags":[["key","value1","value2"]]}]"#;

        assert_eq!(serde_json::to_string(&secret).unwrap(), secret_str);
    }

    #[test]
    fn test_secret_round_trip_serialization() {
        // Create a Secret instance
        let original_secret = Secret {
            kind: Kind::P2PK,
            secret_data: SecretData {
                nonce: "5d11913ee0f92fefdc82a6764fd2457a".to_string(),
                data: "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"
                    .to_string(),
                tags: None,
            },
        };

        // Serialize the Secret to JSON string
        let serialized = serde_json::to_string(&original_secret).unwrap();

        // Deserialize directly back to Secret using serde
        let deserialized_secret: Secret = serde_json::from_str(&serialized).unwrap();

        // Verify the direct serde serialization/deserialization round trip works
        assert_eq!(original_secret, deserialized_secret);

        // Also verify that the conversion to crate::secret::Secret works
        let cashu_secret = crate::secret::Secret::from_str(&serialized).unwrap();
        let deserialized_from_cashu: Secret = TryFrom::try_from(&cashu_secret).unwrap();
        assert_eq!(original_secret, deserialized_from_cashu);
    }

    #[test]
    fn test_htlc_secret_round_trip() {
        // The reference BOLT11 invoice is:
        // lnbc100n1p5z3a63pp56854ytysg7e5z9fl3w5mgvrlqjfcytnjv8ff5hm5qt6gl6alxesqdqqcqzzsxqyz5vqsp5p0x0dlhn27s63j4emxnk26p7f94u0lyarnfp5yqmac9gzy4ngdss9qxpqysgqne3v0hnzt2lp0hc69xpzckk0cdcar7glvjhq60lsrfe8gejdm8c564prrnsft6ctxxyrewp4jtezrq3gxxqnfjj0f9tw2qs9y0lslmqpfu7et9

        // Payment hash (typical 32 byte hash in hex format)
        let payment_hash = "5c23fc3aec9d985bd5fc88ca8bceaccc52cf892715dd94b42b84f1b43350751e";

        // Create a Secret instance with HTLC kind
        let original_secret = Secret {
            kind: Kind::HTLC,
            secret_data: SecretData {
                nonce: "7a9128b3f9612549f9278958337a5d7f".to_string(),
                data: payment_hash.to_string(),
                tags: None,
            },
        };

        // Serialize the Secret to JSON string
        let serialized = serde_json::to_string(&original_secret).unwrap();

        // Validate serialized format
        let expected_json = format!(
            r#"["HTLC",{{"nonce":"7a9128b3f9612549f9278958337a5d7f","data":"{}"}}]"#,
            payment_hash
        );
        assert_eq!(serialized, expected_json);

        // Deserialize directly back to Secret using serde
        let deserialized_secret: Secret = serde_json::from_str(&serialized).unwrap();

        // Verify the direct serde serialization/deserialization round trip works
        assert_eq!(original_secret, deserialized_secret);
        assert_eq!(deserialized_secret.kind, Kind::HTLC);
        assert_eq!(deserialized_secret.secret_data.data, payment_hash);
    }
}
