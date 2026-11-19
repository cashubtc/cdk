//! The NUT-00 signing rule for v3 inputs, on the mint
//!
//! Every v3 proof input carries a nutroot witness satisfying its secret over
//! its input digest. Pre-v3 inputs keep the legacy NUT-10/11/14 rules.

use cdk_common::nut00::transaction::{Digest, Transaction, TransactionInput};
use cdk_common::nuts::nut02::KeySetVersion;
use cdk_common::nuts::nut10::nutroot::verify_proof_input;
use cdk_common::nuts::{ProofId, Proofs};
use cdk_common::util::unix_time;

use crate::Error;

/// Most characters a v3 witness may have (NUT-10 lets mints reject longer)
pub(crate) const MAX_V3_WITNESS_LEN: usize = 4096;

/// Whether any input is on a v3 keyset
pub(crate) fn has_v3_inputs(proofs: &Proofs) -> bool {
    proofs
        .iter()
        .any(|proof| proof.keyset_id.get_version() == KeySetVersion::Version02)
}

/// Verify every v3 proof input of `transaction`, built from `proofs` in
/// order, and return each one's `(Y, input_digest)`
///
/// A v3 input with a DLEQ proof is malformed and rejected (NUT-00).
pub(crate) fn verify_v3_proof_inputs(
    transaction: &Transaction,
    proofs: &Proofs,
) -> Result<Vec<(ProofId, Digest)>, Error> {
    let now = unix_time();
    let mut digests = Vec::new();
    for (index, proof) in proofs.iter().enumerate() {
        if proof.keyset_id.get_version() != KeySetVersion::Version02 {
            continue;
        }
        if proof.dleq.is_some() {
            return Err(Error::NUT12(
                cdk_common::nuts::nut12::Error::UnexpectedDleqProof,
            ));
        }
        if let Some(witness) = proof.witness.as_ref().and_then(|w| w.as_nutroot()) {
            if witness.as_str().len() > MAX_V3_WITNESS_LEN {
                return Err(Error::ProofContentTooLarge {
                    actual: witness.as_str().len(),
                    max: MAX_V3_WITNESS_LEN,
                });
            }
        }
        verify_proof_input(transaction, index, proof, now).map_err(|err| {
            tracing::debug!("v3 input {index} failed the signing rule: {err}");
            Error::SignatureMissingOrInvalid
        })?;
        let Some(TransactionInput::Proof { y, .. }) = transaction.inputs().get(index) else {
            return Err(Error::SignatureMissingOrInvalid);
        };
        digests.push((*y, transaction.input_digest(index)?));
    }
    Ok(digests)
}
