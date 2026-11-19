//! v3 (nutroot) inputs on the wallet
//!
//! Every v3 input signs its input digest (NUT-00). The wallet's own proofs
//! have seed-derived secrets, `k*G` for NUT-13 type `0x00` at the proof's
//! counter. Received proofs carry spend info, from which receive check 1
//! rebuilds the key.

use std::collections::{HashMap, HashSet};

use bitcoin::hashes::{sha256, Hash};
use cdk_common::nut00::spend_info::Reconstruction;
use cdk_common::nut00::transaction::Transaction;
use cdk_common::nuts::nut02::KeySetVersion;
use cdk_common::nuts::nut10::nutroot::{
    merkle_path, sign_digest, sign_proofs_key_path, verify_proof_input, Control, Leaf,
    NutrootWitness,
};
use cdk_common::nuts::nut13::derive_v3_secret_key;
use cdk_common::nuts::nut28::slot_map::match_leaf_slots;
use cdk_common::util::unix_time;
use cdk_common::{Proof, Proofs, ProofsMethods, PublicKey, SecretKey};

use crate::{Error, Wallet};

/// Spend keys of v3 inputs, by secret point
pub(crate) type V3Keys = HashMap<PublicKey, SecretKey>;

/// Signing material selected after verifying a received proof's spend info.
pub(crate) enum V3Spend {
    KeyPath(SecretKey),
    ScriptPath {
        witness: NutrootWitness,
        keys: Vec<SecretKey>,
    },
}

pub(crate) type V3Spends = HashMap<PublicKey, V3Spend>;

/// Default minimum time, in seconds, before an `after` leaf's refund opens
/// for a received v3 payment to count as received
pub const DEFAULT_MIN_REFUND_HORIZON: u64 = 3600;

fn is_v3(proof: &Proof) -> bool {
    proof.keyset_id.get_version() == KeySetVersion::Version02
}

impl Wallet {
    /// Check lock construction requirements before reserving any inputs.
    pub(crate) async fn validate_nutroot_lock(
        &self,
        keyset_id: cdk_common::Id,
        request: &cdk_common::nuts::nut18::NutrootOption,
    ) -> Result<(), Error> {
        if keyset_id.get_version() != KeySetVersion::Version02 {
            return Err(Error::NutrootRequiresV3Keyset);
        }
        if request.is_nums() {
            let info = self.load_mint_info().await?;
            for leaf in request.l.iter().flatten() {
                let signal = leaf.parse_known()?.leaf_type().signal();
                if !info.nuts.nut10.supports_leaf(&signal) {
                    return Err(Error::UnsupportedNutrootLeaf { leaf: signal });
                }
            }
        }
        Ok(())
    }

    /// Spend keys for the wallet's own v3 proofs (no spend info)
    ///
    /// Each secret is re-derived from the seed at the counter it was minted
    /// with.
    pub(crate) async fn own_v3_keys(&self, proofs: &Proofs) -> Result<V3Keys, Error> {
        let own: Proofs = proofs
            .iter()
            .filter(|proof| is_v3(proof) && proof.spend_info.is_none())
            .cloned()
            .collect();
        if own.is_empty() {
            return Ok(V3Keys::new());
        }
        let stored = self.localstore.get_proofs_by_ys(own.ys()?).await?;
        let mut keys = V3Keys::new();
        for proof in &own {
            let point = proof.secret.v3_point()?;
            let index = stored
                .iter()
                .find(|info| info.proof.secret == proof.secret)
                .and_then(|info| info.derivation_index)
                .ok_or(Error::V3ProofNotSpendable)?;
            let key = derive_v3_secret_key(&self.seed, proof.keyset_id, index)?;
            if key.public_key() != point {
                return Err(Error::V3ProofNotSpendable);
            }
            keys.insert(point, key);
        }
        Ok(keys)
    }

    /// Attach bearer spend info to the wallet's own v3 proofs for a token
    ///
    /// The key handed over is the proof's seed-derived `k`; the receiver
    /// should sweep promptly, as the sender still holds it.
    pub(crate) async fn with_bearer_spend_info(&self, proofs: &Proofs) -> Result<Proofs, Error> {
        let keys = self.own_v3_keys(proofs).await?;
        proofs
            .iter()
            .map(|proof| {
                let mut proof = proof.clone();
                if is_v3(&proof) {
                    if proof.spend_info.is_none() {
                        let key = keys
                            .get(&proof.secret.v3_point()?)
                            .ok_or(Error::V3ProofNotSpendable)?;
                        proof.spend_info = Some(cdk_common::nut00::SpendInfo::bearer(key.clone()));
                    }
                    proof.witness = None;
                }
                Ok(proof)
            })
            .collect()
    }

    /// Sign the v3 inputs of a swap request
    ///
    /// `received` holds keys for received proofs (from receive check 1);
    /// the wallet's own proofs are keyed from the seed.
    pub(crate) async fn sign_v3_swap(
        &self,
        request: &mut cdk_common::SwapRequest,
        received: &V3Spends,
    ) -> Result<(), Error> {
        if !request.inputs().iter().any(is_v3) {
            return Ok(());
        }
        let transaction = request.transaction()?;
        // Recovery replays the same outputs. A persisted witness can be reused
        // only if it verifies against this exact transaction.
        let replayed: HashSet<PublicKey> = request
            .inputs()
            .iter()
            .enumerate()
            .filter(|(index, proof)| {
                is_v3(proof) && verify_proof_input(&transaction, *index, proof, unix_time()).is_ok()
            })
            .map(|(_, proof)| proof.secret.v3_point())
            .collect::<Result<_, _>>()?;
        // Received proofs are keyed by receive check 1; the request has
        // dropped their spend info, so only the rest are the wallet's own.
        let own: Proofs = request
            .inputs()
            .iter()
            .filter(|proof| {
                proof.secret.v3_point().map_or(true, |point| {
                    !received.contains_key(&point) && !replayed.contains(&point)
                })
            })
            .cloned()
            .collect();
        let mut keys = self.own_v3_keys(&own).await?;
        keys.extend(received.iter().filter_map(|(point, spend)| match spend {
            V3Spend::KeyPath(key) => Some((*point, key.clone())),
            V3Spend::ScriptPath { .. } => None,
        }));
        for (index, proof) in request.inputs_mut().iter_mut().enumerate() {
            if !is_v3(proof) {
                continue;
            }
            if let Some(V3Spend::ScriptPath { witness, keys }) =
                received.get(&proof.secret.v3_point()?)
            {
                let mut witness = witness.clone();
                let digest = transaction.input_digest(index)?;
                witness.signatures = keys.iter().map(|key| sign_digest(key, &digest)).collect();
                proof.witness = Some(witness.into());
            }
        }
        Self::sign_v3_inputs(&transaction, request.inputs_mut(), &keys)
    }

    /// Sign every v3 input of `transaction` by the key path
    pub(crate) fn sign_v3_inputs(
        transaction: &Transaction,
        proofs: &mut Proofs,
        keys: &V3Keys,
    ) -> Result<(), Error> {
        sign_proofs_key_path(transaction, proofs, |point| keys.get(point).cloned())?;
        if proofs
            .iter()
            .any(|proof| is_v3(proof) && proof.witness.is_none())
        {
            return Err(Error::V3ProofNotSpendable);
        }
        for (index, proof) in proofs.iter().enumerate().filter(|(_, proof)| is_v3(proof)) {
            verify_proof_input(transaction, index, proof, unix_time())?;
        }
        Ok(())
    }
}

/// Receive checks 1 and 2 for a received v3 proof
///
/// Rebuilds the secret from the spend info (trying the wallet's receiver
/// keys for a receiver-keyed proof), requires a key-path key the wallet
/// holds, and evaluates every disclosed leaf against the acceptance policy.
pub(crate) fn check_received_v3_proof(
    proof: &Proof,
    receiver_keys: &[SecretKey],
    min_refund_horizon: u64,
) -> Result<V3Spend, Error> {
    let secret = proof.secret.v3_point()?;
    let spend_info = proof
        .spend_info
        .as_ref()
        .ok_or(Error::V3ProofNotSpendable)?;

    let reconstruction: Reconstruction = match spend_info.e {
        None => spend_info.reconstruct(&secret, None)?,
        Some(_) => {
            let mut last_err = Error::V3ProofNotSpendable;
            let mut found = None;
            for receiver in receiver_keys {
                match spend_info.reconstruct(&secret, Some(receiver)) {
                    Ok(reconstruction) => {
                        found = Some(reconstruction);
                        break;
                    }
                    Err(err) => last_err = err.into(),
                }
            }
            found.ok_or(last_err)?
        }
    };

    check_leaf_policy(&reconstruction.leaves, unix_time(), min_refund_horizon)?;
    reconstruction
        .key_path_key
        .map(V3Spend::KeyPath)
        .ok_or(Error::V3ProofNotSpendable)
}

/// The NUT-18 payee checks for a v3 proof paying `request`
///
/// The spend info must reconstruct the secret, carry `E` exactly when the
/// request blinds something and `u` exactly for a NUMS `k`, and disclose
/// exactly the requested tree. A script-only payment requires a satisfiable
/// leaf, using the supplied signing keys and hashlock preimages.
pub(crate) fn check_requested_v3_proof(
    proof: &Proof,
    request: &cdk_common::nuts::nut18::NutrootOption,
    receiver_keys: &[SecretKey],
    preimages: &[String],
) -> Result<V3Spend, Error> {
    let secret = proof.secret.v3_point()?;
    let spend_info = proof
        .spend_info
        .as_ref()
        .ok_or(Error::V3ProofNotSpendable)?;
    let mut last_err = Error::V3ProofNotSpendable;
    let candidates: Vec<Option<&SecretKey>> = match spend_info.e {
        Some(_) => receiver_keys.iter().map(Some).collect(),
        None => vec![None],
    };
    for receiver in candidates {
        match request.verify_payment(&secret, spend_info, receiver) {
            Ok(reconstruction) => {
                return match reconstruction.key_path_key {
                    Some(key) => Ok(V3Spend::KeyPath(key)),
                    None => {
                        script_path_spend(spend_info, &reconstruction, receiver_keys, preimages)
                    }
                }
            }
            Err(err) => last_err = Error::NUT18(err),
        }
    }
    Err(last_err)
}

/// Select a satisfiable disclosed leaf after the request's exact-tree check.
fn script_path_spend(
    spend_info: &cdk_common::nut00::SpendInfo,
    reconstruction: &Reconstruction,
    receiver_keys: &[SecretKey],
    preimages: &[String],
) -> Result<V3Spend, Error> {
    let tree = spend_info.tree.as_ref().ok_or(Error::V3ProofNotSpendable)?;
    let mut available = receiver_keys.to_vec();
    if let Some(ephemeral) = &spend_info.e {
        for key in receiver_keys {
            available.extend(
                match_leaf_slots(key, ephemeral, tree)
                    .map_err(cdk_common::nut00::spend_info::Error::from)?
                    .into_iter()
                    .map(|(_, key)| key),
            );
        }
    }
    let preimages = preimages
        .iter()
        .map(cdk_common::util::hex::decode)
        .collect::<Result<Vec<_>, _>>()?;
    for (index, leaf) in reconstruction.leaves.iter().enumerate() {
        let preimage = match leaf {
            Leaf::Commit { .. } => continue,
            Leaf::After { time, .. } if unix_time() < *time => continue,
            Leaf::Hashlock { hash, .. } => match preimages.iter().find(|preimage| {
                preimage.len() == 32 && sha256::Hash::hash(preimage).to_byte_array() == *hash
            }) {
                Some(preimage) => Some(preimage.clone()),
                None => continue,
            },
            _ => None,
        };
        let required = usize::from(leaf.threshold().ok_or(Error::V3ProofNotSpendable)?);
        let keys: Vec<SecretKey> = leaf
            .keys()
            .iter()
            .filter_map(|point| {
                available
                    .iter()
                    .find(|key| key.public_key().x_only_public_key() == point.x_only_public_key())
                    .cloned()
            })
            .take(required)
            .collect();
        if keys.len() == required {
            return Ok(V3Spend::ScriptPath {
                witness: NutrootWitness {
                    signatures: vec![],
                    leaf: Some(tree[index].clone()),
                    control: Some(Control {
                        internal_key: reconstruction.internal_key,
                        path: merkle_path(tree, index)?,
                    }),
                    preimage,
                },
                keys,
            });
        }
    }
    Err(Error::V3ProofNotSpendable)
}

/// Every disclosed leaf against the wallet's acceptance policy
///
/// A `commit` leaf adds no spend path. An `after` leaf (eg the sender's
/// refund) is accepted only if it opens at least `min_refund_horizon`
/// seconds from now. `threshold` and `hashlock` leaves let their keys spend
/// at once, so they are refused: the wallet cannot treat such a proof as
/// received value.
pub(crate) fn check_leaf_policy(
    leaves: &[Leaf],
    now: u64,
    min_refund_horizon: u64,
) -> Result<(), Error> {
    for leaf in leaves {
        match leaf {
            Leaf::Commit { .. } => (),
            Leaf::After { time, .. } if *time >= now.saturating_add(min_refund_horizon) => (),
            _ => return Err(Error::V3LeafPolicy),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn after(time: u64) -> Leaf {
        Leaf::After {
            n: 1,
            keys: vec![SecretKey::generate().public_key()],
            time,
        }
    }

    #[test]
    fn leaf_policy() {
        let now = 1_000_000;
        check_leaf_policy(&[], now, 3600).unwrap();
        check_leaf_policy(&[after(now + 3600)], now, 3600).unwrap();
        check_leaf_policy(&[Leaf::Commit { hash: [0; 32] }], now, 3600).unwrap();
        assert!(matches!(
            check_leaf_policy(&[after(now + 3599)], now, 3600),
            Err(Error::V3LeafPolicy)
        ));
        let threshold = Leaf::Threshold {
            n: 1,
            keys: vec![SecretKey::generate().public_key()],
        };
        assert!(matches!(
            check_leaf_policy(&[threshold], now, 3600),
            Err(Error::V3LeafPolicy)
        ));
    }

    #[test]
    fn requested_script_paths_require_satisfied_time_and_threshold() {
        use cdk_common::nuts::nut10::nutroot::internal_key::nums_h;
        use cdk_common::nuts::nut18::NutrootOption;

        let alice = SecretKey::generate();
        let bob = SecretKey::generate();
        let spend = |leaf: Leaf, keys: &[SecretKey]| {
            let request = NutrootOption {
                k: nums_h(),
                l: Some(vec![leaf.to_leaf_bytes().unwrap()]),
                b: None,
            };
            let payment = request
                .pay(&SecretKey::generate(), &SecretKey::generate())
                .unwrap();
            let reconstruction = request
                .verify_payment(&payment.secret, &payment.spend_info, None)
                .unwrap();
            script_path_spend(&payment.spend_info, &reconstruction, keys, &[])
        };
        let threshold = Leaf::Threshold {
            n: 2,
            keys: vec![alice.public_key(), bob.public_key()],
        };
        assert!(spend(threshold.clone(), std::slice::from_ref(&alice)).is_err());
        assert!(spend(threshold, &[alice.clone(), bob]).is_ok());
        assert!(spend(
            Leaf::After {
                n: 1,
                keys: vec![alice.public_key()],
                time: unix_time() + 3600,
            },
            std::slice::from_ref(&alice),
        )
        .is_err());
        assert!(spend(
            Leaf::After {
                n: 1,
                keys: vec![alice.public_key()],
                time: 0,
            },
            &[alice],
        )
        .is_ok());
    }

    #[tokio::test]
    async fn persisted_script_witness_replays_only_its_original_outputs() {
        use std::sync::Arc;

        use cdk_common::nuts::nut01::BlsG1PublicKey;
        use cdk_common::nuts::nut10::nutroot::internal_key::nums_h;
        use cdk_common::nuts::nut18::NutrootOption;
        use cdk_common::nuts::{Id, PreMintSecrets};
        use cdk_common::secret::Secret;

        use crate::wallet::test_utils::{
            create_test_db, create_test_wallet_with_mock, MockMintConnector,
        };

        let wallet = create_test_wallet_with_mock(
            create_test_db().await,
            Arc::new(MockMintConnector::new()),
        )
        .await;
        let key = SecretKey::generate();
        let request = NutrootOption {
            k: nums_h(),
            l: Some(vec![Leaf::Threshold {
                n: 1,
                keys: vec![key.public_key()],
            }
            .to_leaf_bytes()
            .unwrap()]),
            b: None,
        };
        let payment = request
            .pay(&SecretKey::generate(), &SecretKey::generate())
            .unwrap();
        let reconstruction = request
            .verify_payment(&payment.secret, &payment.spend_info, None)
            .unwrap();
        let spend = script_path_spend(&payment.spend_info, &reconstruction, &[key], &[]).unwrap();
        let id: Id = "022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701"
            .parse()
            .unwrap();
        let proof = Proof::new(
            crate::Amount::from(1),
            id,
            Secret::from_point(&payment.secret),
            BlsG1PublicKey::hash_to_curve(b"proof"),
        );
        let outputs = || {
            PreMintSecrets::random(
                id,
                crate::Amount::from(1),
                &Default::default(),
                &(0, vec![1]).into(),
            )
            .unwrap()
            .blinded_messages()
        };
        let mut swap = cdk_common::SwapRequest::new(vec![proof], outputs());
        wallet
            .sign_v3_swap(&mut swap, &HashMap::from([(payment.secret, spend)]))
            .await
            .unwrap();
        let witness = swap.inputs()[0].witness.clone();
        wallet
            .sign_v3_swap(&mut swap, &Default::default())
            .await
            .unwrap();
        assert_eq!(swap.inputs()[0].witness, witness);
        *swap.outputs_mut() = outputs();
        assert!(wallet
            .sign_v3_swap(&mut swap, &Default::default())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn nums_lock_requires_advertised_leaf_support() {
        use std::sync::Arc;

        use cdk_common::nuts::nut10::nutroot::internal_key::nums_h;
        use cdk_common::nuts::nut18::NutrootOption;
        use cdk_common::nuts::Id;

        use crate::wallet::swap::ProofReservation;
        use crate::wallet::test_utils::{
            create_test_db, create_test_wallet_with_mock, MockMintConnector,
        };

        let client = Arc::new(MockMintConnector::new());
        client.mint_info.lock().unwrap().nuts.nut10.leaves.clear();
        let wallet = create_test_wallet_with_mock(create_test_db().await, client.clone()).await;
        let id: Id = "022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701"
            .parse()
            .unwrap();
        let request = NutrootOption {
            k: nums_h(),
            l: Some(vec![Leaf::Threshold {
                n: 1,
                keys: vec![SecretKey::generate().public_key()],
            }
            .to_leaf_bytes()
            .unwrap()]),
            b: None,
        };
        let result = wallet
            .create_swap(
                &uuid::Uuid::new_v4(),
                id,
                &(0, vec![1]).into(),
                Some(crate::Amount::from(1)),
                Default::default(),
                vec![],
                None,
                false,
                false,
                &crate::fees::ProofsFeeBreakdown {
                    total: crate::Amount::ZERO,
                    per_keyset: HashMap::new(),
                },
                ProofReservation::Skip,
                Some(&request),
            )
            .await;
        assert!(matches!(result, Err(Error::UnsupportedNutrootLeaf { leaf }) if leaf == "0001"));
        assert!(client.captured_swap_requests().is_empty());
    }
}
