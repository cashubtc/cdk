//! v3 (nutroot) inputs on the wallet
//!
//! Every v3 input signs its input digest (NUT-00). The wallet's own proofs
//! have seed-derived secrets, `k*G` for NUT-13 type `0x00` at the proof's
//! counter. Received proofs carry spend info, from which receive check 1
//! rebuilds the key.

use std::collections::HashMap;

use cdk_common::nut00::spend_info::Reconstruction;
use cdk_common::nut00::transaction::Transaction;
use cdk_common::nuts::nut02::KeySetVersion;
use cdk_common::nuts::nut10::nutroot::{sign_proofs_key_path, Leaf};
use cdk_common::nuts::nut13::derive_v3_secret_key;
use cdk_common::util::unix_time;
use cdk_common::{Proof, Proofs, ProofsMethods, PublicKey, SecretKey};

use crate::{Error, Wallet};

/// Spend keys of v3 inputs, by secret point
pub(crate) type V3Keys = HashMap<PublicKey, SecretKey>;

/// Default minimum time, in seconds, before an `after` leaf's refund opens
/// for a received v3 payment to count as received
pub const DEFAULT_MIN_REFUND_HORIZON: u64 = 3600;

fn is_v3(proof: &Proof) -> bool {
    proof.keyset_id.get_version() == KeySetVersion::Version02
}

impl Wallet {
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
        received: &V3Keys,
    ) -> Result<(), Error> {
        if !request.inputs().iter().any(is_v3) {
            return Ok(());
        }
        // Received proofs are keyed by receive check 1; the request has
        // dropped their spend info, so only the rest are the wallet's own.
        let own: Proofs = request
            .inputs()
            .iter()
            .filter(|proof| {
                proof
                    .secret
                    .v3_point()
                    .map_or(true, |point| !received.contains_key(&point))
            })
            .cloned()
            .collect();
        let mut keys = self.own_v3_keys(&own).await?;
        keys.extend(received.iter().map(|(k, v)| (*k, v.clone())));
        let transaction = request.transaction()?;
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
) -> Result<SecretKey, Error> {
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
        .ok_or(Error::V3ProofNotSpendable)
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
}
