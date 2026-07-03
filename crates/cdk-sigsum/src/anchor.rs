//! High-level orchestration for anchoring a single checksum to a Sigsum
//! log: submit, poll until committed, then assemble a [`SigsumProof`].
//!
//! This is what a mint's checkpoint-publishing background task (see
//! `docs/adr/0001-append-only-transparency-log.md`, section 7) is expected
//! to call once per published checkpoint.

use std::time::Duration;

use ed25519_dalek::{SigningKey, VerifyingKey};
use tokio::time::sleep;

use crate::client::{AddLeafOutcome, SigsumClient};
use crate::hashing::{checksum_of, leaf_hash, sha256, sign_leaf};
use crate::proof::SigsumProof;
use crate::rate_limit::SubmitToken;
use crate::Error;

/// Anchors `data` (e.g. the canonical bytes of a mint transparency
/// checkpoint) to `log`, retrying `add-leaf` until the log commits to
/// publishing it, then assembles an offline-verifiable [`SigsumProof`].
///
/// `data` itself is never sent to the log — only `H(H(data))` is, per the
/// log's recommended usage. Callers that already have a 32-byte digest
/// they want logged directly should sign the checksum with
/// [`crate::hashing::sign_leaf`] and call [`SigsumClient::add_leaf`]
/// themselves instead of going through this helper.
///
/// ```no_run
/// use cdk_sigsum::{anchor, SigsumClient};
/// use ed25519_dalek::{SigningKey, VerifyingKey};
/// use url::Url;
///
/// # async fn example(
/// #     log_public_key: VerifyingKey,
/// #     submit_key: SigningKey,
/// #     checkpoint_bytes: &[u8],
/// # ) -> Result<(), cdk_sigsum::Error> {
/// let client = SigsumClient::new(Url::parse("https://seasalp.glasklar.is/")?);
/// let proof = anchor(&client, &log_public_key, &submit_key, None, checkpoint_bytes).await?;
/// println!("{}", proof.to_ascii());
/// # Ok(())
/// # }
/// ```
pub async fn anchor(
    log: &SigsumClient,
    log_public_key: &VerifyingKey,
    submit_key: &SigningKey,
    token: Option<&SubmitToken>,
    data: &[u8],
) -> Result<SigsumProof, Error> {
    // Per spec §2.2.4: `message = H(data)` is what's sent on the wire and
    // what the log recomputes `checksum` from server-side; `checksum =
    // H(message) = H(H(data))` is what actually gets signed and stored in
    // the leaf. These are deliberately two different 32-byte values.
    let message = sha256(data);
    let checksum = checksum_of(data);
    debug_assert_eq!(checksum, sha256(&message));
    let leaf = sign_leaf(submit_key, checksum);
    let submit_public_key = submit_key.verifying_key();

    // `add-leaf` may need to be retried until the log moves the
    // submission from "accepted" to "committed" (spec section 3.5).
    let mut attempt = 0;
    loop {
        match log.add_leaf(message, &leaf, &submit_public_key, token).await? {
            AddLeafOutcome::Committed => break,
            AddLeafOutcome::Accepted => {
                attempt += 1;
                tracing::debug!(attempt, "sigsum add-leaf accepted, not yet committed");
                sleep(Duration::from_secs(2)).await;
            }
        }
    }

    let target_leaf_hash = leaf_hash(&leaf);
    let log_key_hash = sha256(log_public_key.as_bytes());

    // Wait for a published tree head that actually covers our leaf: a
    // "committed" response only means the log intends to include it in
    // its *next* signed tree head, not that one exists yet.
    let (tree_head, inclusion_proof) = loop {
        let tree_head = log.get_tree_head().await?;
        if tree_head.size == 0 {
            sleep(Duration::from_secs(5)).await;
            continue;
        }
        if tree_head.size == 1 {
            if tree_head.root_hash == target_leaf_hash {
                break (tree_head, None);
            }
            sleep(Duration::from_secs(5)).await;
            continue;
        }
        match log
            .get_inclusion_proof(tree_head.size, target_leaf_hash)
            .await
        {
            Ok(proof) => break (tree_head, Some(proof)),
            Err(Error::UnexpectedStatus { status: 404, .. }) => {
                sleep(Duration::from_secs(5)).await;
            }
            Err(err) => return Err(err),
        }
    };

    Ok(SigsumProof {
        log_key_hash,
        leaf,
        tree_head,
        inclusion_proof,
    })
}
