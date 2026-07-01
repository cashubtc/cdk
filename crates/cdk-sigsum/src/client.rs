//! HTTP client for the five endpoints a Sigsum log server exposes.
//!
//! See <https://git.glasklar.is/sigsum/project/documentation/-/blob/main/log.md>
//! section 3 for the protocol this mirrors.

use reqwest::StatusCode;
use tracing::instrument;
use url::Url;

use crate::error::Error;
use crate::hashing::Hash;
use crate::rate_limit::SubmitToken;
use crate::types::{ConsistencyProof, Cosignature, InclusionProof, SignedTreeHead, TreeLeaf};

/// A client for one Sigsum log, identified by its base URL (e.g.
/// `https://seasalp.glasklar.is/`).
#[derive(Debug, Clone)]
pub struct SigsumClient {
    http: reqwest::Client,
    base_url: Url,
}

/// The outcome of an `add-leaf` submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddLeafOutcome {
    /// HTTP 202: the log has accepted the request but is not yet
    /// committed to publishing it. Per the spec, the submitter should
    /// resend the request until it observes [`AddLeafOutcome::Committed`].
    Accepted,
    /// HTTP 200: the log is committed to including the leaf in its next
    /// signed tree head.
    Committed,
}

impl SigsumClient {
    /// Creates a client for the log at `base_url`.
    pub fn new(base_url: Url) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
        }
    }

    /// Builds a client from an existing [`reqwest::Client`] (e.g. to share
    /// connection pools / TLS config with the rest of the mint).
    pub fn from_reqwest(http: reqwest::Client, base_url: Url) -> Self {
        Self { http, base_url }
    }

    /// `GET <base>/get-tree-head`.
    #[instrument(skip(self))]
    pub async fn get_tree_head(&self) -> Result<SignedTreeHead, Error> {
        let url = self.base_url.join("get-tree-head")?;
        let body = self.get_ok(url).await?;
        let lines = parse_kv_lines(&body);

        let size = find_one(&lines, "size")?
            .parse::<u64>()
            .map_err(|e| Error::MalformedResponse(format!("invalid `size`: {e}")))?;
        let root_hash = decode_hash(find_one(&lines, "root_hash")?, "root_hash")?;
        let signature = decode_signature(find_one(&lines, "signature")?, "signature")?;

        let mut cosignatures = Vec::new();
        for value in find_all(&lines, "cosignature") {
            let mut fields = value.split(' ');
            let key_hash = decode_hash(
                fields
                    .next()
                    .ok_or(Error::MissingField("cosignature.key_hash"))?,
                "cosignature.key_hash",
            )?;
            let timestamp = fields
                .next()
                .ok_or(Error::MissingField("cosignature.timestamp"))?
                .parse::<u64>()
                .map_err(|e| {
                    Error::MalformedResponse(format!("invalid cosignature timestamp: {e}"))
                })?;
            let signature = decode_signature(
                fields
                    .next()
                    .ok_or(Error::MissingField("cosignature.signature"))?,
                "cosignature.signature",
            )?;
            cosignatures.push(Cosignature {
                witness_key_hash: key_hash,
                timestamp,
                signature,
            });
        }

        Ok(SignedTreeHead {
            size,
            root_hash,
            signature,
            cosignatures,
        })
    }

    /// `GET <base>/get-inclusion-proof/<size>/<leaf_hash>`.
    #[instrument(skip(self))]
    pub async fn get_inclusion_proof(
        &self,
        size: u64,
        leaf_hash: Hash,
    ) -> Result<InclusionProof, Error> {
        let path = format!("get-inclusion-proof/{size}/{}", hex::encode(leaf_hash));
        let url = self.base_url.join(&path)?;
        let body = self.get_ok(url).await?;
        let lines = parse_kv_lines(&body);

        let leaf_index = find_one(&lines, "leaf_index")?
            .parse::<u64>()
            .map_err(|e| Error::MalformedResponse(format!("invalid `leaf_index`: {e}")))?;
        let mut node_hashes = Vec::new();
        for value in find_all(&lines, "node_hash") {
            node_hashes.push(decode_hash(value, "node_hash")?);
        }

        Ok(InclusionProof {
            leaf_index,
            node_hashes,
        })
    }

    /// `GET <base>/get-consistency-proof/<old_size>/<new_size>`.
    #[instrument(skip(self))]
    pub async fn get_consistency_proof(
        &self,
        old_size: u64,
        new_size: u64,
    ) -> Result<ConsistencyProof, Error> {
        let path = format!("get-consistency-proof/{old_size}/{new_size}");
        let url = self.base_url.join(&path)?;
        let body = self.get_ok(url).await?;
        let lines = parse_kv_lines(&body);

        let mut node_hashes = Vec::new();
        for value in find_all(&lines, "node_hash") {
            node_hashes.push(decode_hash(value, "node_hash")?);
        }
        Ok(ConsistencyProof { node_hashes })
    }

    /// `GET <base>/get-leaves/<start_index>/<end_index>`. The log may
    /// return fewer leaves than requested; it never returns more.
    #[instrument(skip(self))]
    pub async fn get_leaves(&self, start: u64, end: u64) -> Result<Vec<TreeLeaf>, Error> {
        let path = format!("get-leaves/{start}/{end}");
        let url = self.base_url.join(&path)?;
        let body = self.get_ok(url).await?;
        let lines = parse_kv_lines(&body);

        let mut leaves = Vec::new();
        for value in find_all(&lines, "leaf") {
            let mut fields = value.split(' ');
            let checksum = decode_hash(
                fields.next().ok_or(Error::MissingField("leaf.checksum"))?,
                "leaf.checksum",
            )?;
            let signature = decode_signature(
                fields.next().ok_or(Error::MissingField("leaf.signature"))?,
                "leaf.signature",
            )?;
            let key_hash = decode_hash(
                fields.next().ok_or(Error::MissingField("leaf.key_hash"))?,
                "leaf.key_hash",
            )?;
            leaves.push(TreeLeaf {
                checksum,
                signature,
                key_hash,
            });
        }
        Ok(leaves)
    }

    /// `POST <base>/add-leaf`. `token` is required by public logs that
    /// enforce domain-based rate limiting (see [`crate::RateLimitKeyPair`]);
    /// it is omitted entirely for private/self-hosted logs that don't
    /// require it.
    ///
    /// `message` is the wire-format `message` field — *not* the same value
    /// as `leaf.checksum`. Per spec §2.2.4, `checksum = H(message)`; when
    /// following the recommended `message = H(data)` convention (as
    /// [`crate::anchor`] does), `checksum` ends up as `H(H(data))`, one
    /// hash deeper than `message`. The log recomputes `checksum` from
    /// `message` itself and rejects the signature (over the namespaced
    /// `checksum`) if it doesn't match — so sending `leaf.checksum` here
    /// instead of the true `message` fails with "invalid signature" even
    /// though the signature itself was computed correctly.
    ///
    /// `public_key` is the submitter's raw Ed25519 public key — required by
    /// the spec's `add-leaf` input so the log can verify `leaf.signature`
    /// itself. This is deliberately a separate parameter rather than
    /// derived from `leaf.key_hash`: `TreeLeaf` only ever carries the
    /// *hash* of the submitter's key (that's what's serialized on the
    /// wire per §2.2.4), so the raw key has to come from the caller, who
    /// still has it (it's `submit_key.verifying_key()` in [`crate::anchor`]).
    #[instrument(skip(self, message, leaf, public_key))]
    pub async fn add_leaf(
        &self,
        message: Hash,
        leaf: &TreeLeaf,
        public_key: &ed25519_dalek::VerifyingKey,
        token: Option<&SubmitToken>,
    ) -> Result<AddLeafOutcome, Error> {
        let url = self.base_url.join("add-leaf")?;
        let body = format!(
            "message={}\nsignature={}\npublic_key={}\n",
            hex::encode(message),
            hex::encode(leaf.signature),
            hex::encode(public_key.as_bytes()),
        );

        let mut request = self.http.post(url).body(body);
        if let Some(token) = token {
            request = request.header("sigsum-token", token.header_value());
        }

        let response = request.send().await?;
        match response.status() {
            StatusCode::OK => Ok(AddLeafOutcome::Committed),
            StatusCode::ACCEPTED => Ok(AddLeafOutcome::Accepted),
            status => {
                let body = response.text().await.unwrap_or_default();
                Err(Error::UnexpectedStatus {
                    status: status.as_u16(),
                    body,
                })
            }
        }
    }

    async fn get_ok(&self, url: Url) -> Result<String, Error> {
        let response = self.http.get(url).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if status != StatusCode::OK {
            return Err(Error::UnexpectedStatus {
                status: status.as_u16(),
                body,
            });
        }
        Ok(body)
    }
}

/// Splits a Sigsum `Key=Value\n` response body into ordered `(key, value)`
/// pairs, preserving repeated keys in order.
fn parse_kv_lines(body: &str) -> Vec<(&str, &str)> {
    body.lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.split_once('='))
        .collect()
}

fn find_one<'a>(lines: &'a [(&'a str, &'a str)], key: &'static str) -> Result<&'a str, Error> {
    lines
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .ok_or(Error::MissingField(key))
}

fn find_all<'a>(lines: &'a [(&'a str, &'a str)], key: &'static str) -> Vec<&'a str> {
    lines
        .iter()
        .filter(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .collect()
}

fn decode_hash(value: &str, field: &'static str) -> Result<Hash, Error> {
    let bytes = hex::decode(value).map_err(|e| Error::InvalidHex {
        field,
        reason: e.to_string(),
    })?;
    bytes.try_into().map_err(|_| Error::InvalidHex {
        field,
        reason: "expected 32 bytes".to_string(),
    })
}

fn decode_signature(value: &str, field: &'static str) -> Result<[u8; 64], Error> {
    let bytes = hex::decode(value).map_err(|e| Error::InvalidHex {
        field,
        reason: e.to_string(),
    })?;
    bytes.try_into().map_err(|_| Error::InvalidHex {
        field,
        reason: "expected 64 bytes".to_string(),
    })
}
