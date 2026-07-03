//! Assembly of a self-contained, offline-verifiable "sigsum proof" (see
//! <https://git.glasklar.is/sigsum/core/sigsum-go/-/blob/main/doc/sigsum-proof.md>)
//! for one submitted leaf.
//!
//! This crate only assembles proofs on the submitter (mint) side. To
//! verify one, use the `sigsum` crate (<https://docs.rs/sigsum>), which
//! implements offline verification of exactly this ascii format, or port
//! the verification steps described in the spec linked above.

use crate::hashing::Hash;
use crate::types::{InclusionProof, SignedTreeHead, TreeLeaf};

/// A complete proof of logging for one leaf: enough for an offline
/// verifier to check, without contacting the log again, that the leaf was
/// included under a witness-cosigned tree head.
#[derive(Debug, Clone)]
pub struct SigsumProof {
    /// `H(log_public_key)`, identifying which log this proof is for.
    pub log_key_hash: Hash,
    /// The submitted leaf (its `checksum` is re-derived by the verifier
    /// from the data being verified, so it is not repeated here — see the
    /// v2 proof format).
    pub leaf: TreeLeaf,
    /// The cosigned tree head the inclusion proof is relative to.
    pub tree_head: SignedTreeHead,
    /// Inclusion proof of `leaf` under `tree_head`. `None` iff
    /// `tree_head.size == 1`, in which case inclusion is trivially
    /// `leaf_hash == tree_head.root_hash`.
    pub inclusion_proof: Option<InclusionProof>,
}

impl SigsumProof {
    /// Serializes the proof using the ascii format from the sigsum-proof
    /// spec (version 2), suitable for storing alongside the mint's own
    /// checkpoint record and for handing to third-party auditors.
    pub fn to_ascii(&self) -> String {
        let mut out = String::new();
        out.push_str("version=2\n");
        out.push_str(&format!("log={}\n", hex::encode(self.log_key_hash)));
        out.push_str(&format!(
            "leaf={} {}\n\n",
            hex::encode(self.leaf.key_hash),
            hex::encode(self.leaf.signature),
        ));
        out.push_str(&format!("size={}\n", self.tree_head.size));
        out.push_str(&format!(
            "root_hash={}\n",
            hex::encode(self.tree_head.root_hash)
        ));
        out.push_str(&format!(
            "signature={}\n",
            hex::encode(self.tree_head.signature)
        ));
        for cosig in &self.tree_head.cosignatures {
            out.push_str(&format!(
                "cosignature={} {} {}\n",
                hex::encode(cosig.witness_key_hash),
                cosig.timestamp,
                hex::encode(cosig.signature),
            ));
        }

        if let Some(proof) = &self.inclusion_proof {
            out.push('\n');
            out.push_str(&format!("leaf_index={}\n", proof.leaf_index));
            for node in &proof.node_hashes {
                out.push_str(&format!("node_hash={}\n", hex::encode(node)));
            }
        }
        out
    }
}
