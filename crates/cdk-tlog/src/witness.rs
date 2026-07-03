//! Witness side of the [C2SP tlog-witness protocol][spec]'s `add-checkpoint`
//! call: pure decision logic for whether to cosign a submitted checkpoint,
//! decoupled from HTTP and storage so it can be unit tested directly and
//! reused by both a mint's built-in witness (cosigning *other* logs'
//! checkpoints) and, in principle, a standalone witness binary.
//!
//! [spec]: https://github.com/C2SP/C2SP/blob/main/tlog-witness.md

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{SigningKey, VerifyingKey};

use crate::checkpoint::{cosign, verify_checkpoint_signature, SignatureLine, SignedCheckpoint};
use crate::merkle::{verify_consistency, Hash};

/// Errors from parsing an `add-checkpoint` request body.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The request body didn't start with an `old <size>` line.
    #[error("missing or malformed `old` size line")]
    MissingOldLine,
    /// A consistency-proof line wasn't valid base64/32 bytes.
    #[error("invalid consistency proof line: {0}")]
    InvalidProofLine(String),
    /// The request body had no blank line separating the proof from the checkpoint.
    #[error("missing blank line before the checkpoint")]
    MissingSeparator,
    /// The trailing checkpoint note failed to parse.
    #[error(transparent)]
    Checkpoint(#[from] crate::checkpoint::Error),
}

/// A parsed `add-checkpoint` request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddCheckpointRequest {
    /// The submitter's claimed size of the checkpoint it last received a
    /// cosignature for (`0` if it has none yet).
    pub old_size: u64,
    /// Consistency proof from `old_size` to `checkpoint.checkpoint.size`.
    pub consistency_proof: Vec<Hash>,
    /// The checkpoint being submitted for cosigning, plus at least its
    /// origin log's own signature.
    pub checkpoint: SignedCheckpoint,
}

impl AddCheckpointRequest {
    /// Parses a request body of the form specified by
    /// [tlog-witness §add-checkpoint](https://github.com/C2SP/C2SP/blob/main/tlog-witness.md#add-checkpoint):
    /// an `old <size>` line, zero or more base64 consistency-proof lines,
    /// a blank line, then the checkpoint.
    pub fn parse(body: &str) -> Result<Self, ParseError> {
        let (proof_block, checkpoint_block) = body
            .split_once("\n\n")
            .ok_or(ParseError::MissingSeparator)?;

        let mut lines = proof_block.lines();
        let old_line = lines.next().ok_or(ParseError::MissingOldLine)?;
        let old_size = old_line
            .strip_prefix("old ")
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or(ParseError::MissingOldLine)?;

        let consistency_proof = lines
            .map(|line| {
                BASE64
                    .decode(line)
                    .ok()
                    .and_then(|bytes| bytes.try_into().ok())
                    .ok_or_else(|| ParseError::InvalidProofLine(line.to_string()))
            })
            .collect::<Result<Vec<Hash>, _>>()?;

        let checkpoint = SignedCheckpoint::parse(checkpoint_block)?;

        Ok(Self {
            old_size,
            consistency_proof,
            checkpoint,
        })
    }

    /// Renders this request back to the wire format, e.g. for a mint's own
    /// outbound witness client requesting a cosignature.
    pub fn to_body(&self) -> String {
        let mut body = format!("old {}\n", self.old_size);
        for hash in &self.consistency_proof {
            body.push_str(&BASE64.encode(hash));
            body.push('\n');
        }
        body.push('\n');
        body.push_str(&self.checkpoint.to_note());
        body
    }
}

/// Reasons a witness declines to cosign a submitted checkpoint, mapped to
/// the HTTP status codes tlog-witness specifies for each.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WitnessError {
    /// The checkpoint's origin is not one this witness tracks (HTTP 404).
    #[error("unknown checkpoint origin")]
    UnknownOrigin,
    /// No valid signature from a trusted key was found for the origin (HTTP 403).
    #[error("no valid signature from a trusted key")]
    NoTrustedSignature,
    /// `old_size` exceeds the checkpoint's own size (HTTP 400).
    #[error("old size exceeds checkpoint size")]
    OldSizeExceedsCheckpoint,
    /// `old_size` didn't match the size this witness last cosigned for
    /// this origin (HTTP 409); `stored_size` is what it actually has.
    #[error("old size {claimed} does not match last cosigned size {stored}")]
    SizeConflict {
        /// The `old_size` the submitter claimed.
        claimed: u64,
        /// The size this witness actually last cosigned for this origin.
        stored: u64,
    },
    /// The consistency proof did not verify (HTTP 422).
    #[error("consistency proof failed to verify")]
    InvalidConsistencyProof,
    /// `old_size == checkpoint.size` but the root hashes differ (HTTP 409).
    #[error("root hash mismatch at equal tree size")]
    RootHashConflict,
}

/// Considers whether to cosign `request.checkpoint`, given the witness's
/// trust policy and its own record of the last checkpoint it cosigned for
/// this origin (`None` if it has never cosigned for this origin before).
///
/// On success, returns the cosignature line to return to the submitter,
/// plus the `(size, root_hash)` the caller must durably persist as this
/// origin's new last-cosigned checkpoint *before* responding — the caller
/// is responsible for doing that persist-then-respond step atomically
/// (see the race condition called out in the spec), since this function
/// has no storage of its own.
#[allow(clippy::too_many_arguments)]
pub fn consider_checkpoint(
    request: &AddCheckpointRequest,
    trusted_origin: &str,
    trusted_key: &VerifyingKey,
    last_cosigned: Option<(u64, Hash)>,
    witness_name: &str,
    witness_key: &SigningKey,
    now: u64,
) -> Result<(SignatureLine, u64, Hash), WitnessError> {
    if request.checkpoint.checkpoint.origin != trusted_origin {
        return Err(WitnessError::UnknownOrigin);
    }

    let has_trusted_signature = request.checkpoint.signatures.iter().any(|line| {
        verify_checkpoint_signature(
            &request.checkpoint.checkpoint,
            trusted_origin,
            trusted_key,
            line,
        )
        .is_ok()
    });
    if !has_trusted_signature {
        return Err(WitnessError::NoTrustedSignature);
    }

    let new_size = request.checkpoint.checkpoint.size;
    let new_root = request.checkpoint.checkpoint.root_hash;

    if request.old_size > new_size {
        return Err(WitnessError::OldSizeExceedsCheckpoint);
    }

    let stored_size = last_cosigned.map(|(size, _)| size).unwrap_or(0);
    if request.old_size != stored_size {
        return Err(WitnessError::SizeConflict {
            claimed: request.old_size,
            stored: stored_size,
        });
    }

    if request.old_size == new_size {
        let stored_root = last_cosigned.map(|(_, root)| root);
        if stored_root.is_some_and(|root| root != new_root) {
            return Err(WitnessError::RootHashConflict);
        }
    } else {
        let old_root = last_cosigned.map(|(_, root)| root).unwrap_or([0u8; 32]);
        let verified = verify_consistency(
            request.old_size,
            old_root,
            new_size,
            new_root,
            &request.consistency_proof,
        )
        .unwrap_or(false);
        if !verified {
            return Err(WitnessError::InvalidConsistencyProof);
        }
    }

    let cosignature = cosign(
        &request.checkpoint.checkpoint,
        now,
        witness_name,
        witness_key,
    );
    Ok((cosignature, new_size, new_root))
}

#[cfg(test)]
mod tests {
    use rand_core::OsRng;

    use super::*;
    use crate::checkpoint::{sign_checkpoint, Checkpoint};
    use crate::merkle::TreeHead;

    fn build_checkpoint(log_key: &SigningKey, size: u64, leaves: &[Hash]) -> SignedCheckpoint {
        let mut tree = TreeHead::empty();
        for &leaf in leaves {
            tree.append(leaf);
        }
        assert_eq!(tree.size, size);
        let checkpoint = Checkpoint::new("example.com/log", size, tree.root());
        sign_checkpoint(checkpoint, "example.com/log", log_key)
    }

    #[test]
    fn first_submission_for_a_log_is_cosigned() {
        let log_key = SigningKey::generate(&mut OsRng);
        let witness_key = SigningKey::generate(&mut OsRng);
        let leaves: Vec<Hash> = (0u8..3).map(|i| crate::merkle::leaf_hash(&[i])).collect();
        let checkpoint = build_checkpoint(&log_key, 3, &leaves);

        let request = AddCheckpointRequest {
            old_size: 0,
            consistency_proof: vec![],
            checkpoint,
        };

        let (cosig, size, root) = consider_checkpoint(
            &request,
            "example.com/log",
            &log_key.verifying_key(),
            None,
            "witness.example/w1",
            &witness_key,
            1_700_000_000,
        )
        .expect("first submission should be cosigned");

        assert_eq!(size, 3);
        assert_eq!(root, request.checkpoint.checkpoint.root_hash);
        assert_eq!(cosig.name, "witness.example/w1");
    }

    #[test]
    fn consistent_growth_is_cosigned() {
        let log_key = SigningKey::generate(&mut OsRng);
        let witness_key = SigningKey::generate(&mut OsRng);
        let leaves: Vec<Hash> = (0u8..7).map(|i| crate::merkle::leaf_hash(&[i])).collect();

        let first = build_checkpoint(&log_key, 3, &leaves[..3]);
        let second = build_checkpoint(&log_key, 7, &leaves);
        let proof = crate::merkle::consistency_proof(3, &leaves).unwrap();

        let request = AddCheckpointRequest {
            old_size: 3,
            consistency_proof: proof,
            checkpoint: second,
        };

        let (_, size, _) = consider_checkpoint(
            &request,
            "example.com/log",
            &log_key.verifying_key(),
            Some((3, first.checkpoint.root_hash)),
            "witness.example/w1",
            &witness_key,
            1_700_000_001,
        )
        .expect("consistent growth should be cosigned");
        assert_eq!(size, 7);
    }

    #[test]
    fn unknown_origin_is_rejected() {
        let log_key = SigningKey::generate(&mut OsRng);
        let witness_key = SigningKey::generate(&mut OsRng);
        let leaves: Vec<Hash> = (0u8..1).map(|i| crate::merkle::leaf_hash(&[i])).collect();
        let checkpoint = build_checkpoint(&log_key, 1, &leaves);
        let request = AddCheckpointRequest {
            old_size: 0,
            consistency_proof: vec![],
            checkpoint,
        };

        let err = consider_checkpoint(
            &request,
            "example.com/a-different-log",
            &log_key.verifying_key(),
            None,
            "witness.example/w1",
            &witness_key,
            1_700_000_000,
        )
        .unwrap_err();
        assert_eq!(err, WitnessError::UnknownOrigin);
    }

    #[test]
    fn signature_from_untrusted_key_is_rejected() {
        let log_key = SigningKey::generate(&mut OsRng);
        let other_key = SigningKey::generate(&mut OsRng);
        let witness_key = SigningKey::generate(&mut OsRng);
        let leaves: Vec<Hash> = (0u8..1).map(|i| crate::merkle::leaf_hash(&[i])).collect();
        let checkpoint = build_checkpoint(&log_key, 1, &leaves);
        let request = AddCheckpointRequest {
            old_size: 0,
            consistency_proof: vec![],
            checkpoint,
        };

        let err = consider_checkpoint(
            &request,
            "example.com/log",
            &other_key.verifying_key(),
            None,
            "witness.example/w1",
            &witness_key,
            1_700_000_000,
        )
        .unwrap_err();
        assert_eq!(err, WitnessError::NoTrustedSignature);
    }

    #[test]
    fn stale_old_size_is_a_conflict() {
        let log_key = SigningKey::generate(&mut OsRng);
        let witness_key = SigningKey::generate(&mut OsRng);
        let leaves: Vec<Hash> = (0u8..7).map(|i| crate::merkle::leaf_hash(&[i])).collect();
        let checkpoint = build_checkpoint(&log_key, 7, &leaves);
        let proof = crate::merkle::consistency_proof(3, &leaves).unwrap();
        let request = AddCheckpointRequest {
            old_size: 3,
            consistency_proof: proof,
            checkpoint,
        };

        // Witness actually already cosigned size 5, not 3.
        let stale = build_checkpoint(&log_key, 5, &leaves[..5]);
        let err = consider_checkpoint(
            &request,
            "example.com/log",
            &log_key.verifying_key(),
            Some((5, stale.checkpoint.root_hash)),
            "witness.example/w1",
            &witness_key,
            1_700_000_000,
        )
        .unwrap_err();
        assert_eq!(
            err,
            WitnessError::SizeConflict {
                claimed: 3,
                stored: 5
            }
        );
    }

    #[test]
    fn tampered_consistency_proof_is_rejected() {
        let log_key = SigningKey::generate(&mut OsRng);
        let witness_key = SigningKey::generate(&mut OsRng);
        let leaves: Vec<Hash> = (0u8..7).map(|i| crate::merkle::leaf_hash(&[i])).collect();
        let first = build_checkpoint(&log_key, 3, &leaves[..3]);
        let second = build_checkpoint(&log_key, 7, &leaves);
        let mut proof = crate::merkle::consistency_proof(3, &leaves).unwrap();
        if let Some(first_hash) = proof.first_mut() {
            first_hash[0] ^= 0xff;
        }

        let request = AddCheckpointRequest {
            old_size: 3,
            consistency_proof: proof,
            checkpoint: second,
        };

        let err = consider_checkpoint(
            &request,
            "example.com/log",
            &log_key.verifying_key(),
            Some((3, first.checkpoint.root_hash)),
            "witness.example/w1",
            &witness_key,
            1_700_000_000,
        )
        .unwrap_err();
        assert_eq!(err, WitnessError::InvalidConsistencyProof);
    }

    #[test]
    fn request_round_trips_through_parse_and_to_body() {
        let log_key = SigningKey::generate(&mut OsRng);
        let leaves: Vec<Hash> = (0u8..3).map(|i| crate::merkle::leaf_hash(&[i])).collect();
        let checkpoint = build_checkpoint(&log_key, 3, &leaves);
        let request = AddCheckpointRequest {
            old_size: 0,
            consistency_proof: vec![],
            checkpoint,
        };

        let body = request.to_body();
        let parsed = AddCheckpointRequest::parse(&body).expect("parses");
        assert_eq!(parsed, request);
    }
}
