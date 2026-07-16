//! Bounded opaque AlephBFT live-session backup transport.

use serde::{Deserialize, Serialize};

use super::FEDERATION_REST_API_VERSION;
use crate::federation::{FederationError, FederationId, MemberId, Result};

/// Maximum number of opaque backup chunks returned by one page.
pub const FEDERATION_MAX_ALEPH_BFT_BACKUP_CHUNKS_PER_PAGE: usize = 512;
/// Maximum total raw chunk bytes returned by one backup page.
pub const FEDERATION_MAX_ALEPH_BFT_BACKUP_RAW_BYTES_PER_PAGE: usize = 1024 * 1024;
/// Maximum encoded private REST backup-page response size.
pub const FEDERATION_MAX_ALEPH_BFT_BACKUP_PAGE_BYTES: usize = 8 * 1024 * 1024;
/// Maximum encoded signed backup-page request size.
pub const FEDERATION_MAX_ALEPH_BFT_BACKUP_REQUEST_BYTES: usize = 16 * 1024;

/// One page request for an append-only AlephBFT session backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAlephBftBackupRequest {
    /// Private REST payload version.
    pub version: u16,
    /// AlephBFT session whose signed DAG backup is requested.
    pub session: u64,
    /// First missing zero-based backup chunk.
    pub next_chunk: u64,
}

impl FederationAlephBftBackupRequest {
    /// Construct a backup page request for one exact session and chunk cursor.
    #[must_use]
    pub const fn new(session: u64, next_chunk: u64) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            session,
            next_chunk,
        }
    }

    /// Validate the private REST payload version.
    pub fn validate(&self) -> Result<()> {
        if self.version != FEDERATION_REST_API_VERSION {
            return Err(FederationError::RestVersionMismatch {
                expected: FEDERATION_REST_API_VERSION,
                found: self.version,
            });
        }
        Ok(())
    }
}

/// One bounded page of an append-only AlephBFT session backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAlephBftBackupResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this member.
    pub federation_id: FederationId,
    /// Member serving this backup page.
    pub member_id: MemberId,
    /// AlephBFT session represented by these chunks.
    pub session: u64,
    /// Zero-based index of the first returned chunk.
    pub from_chunk: u64,
    /// Consecutive opaque upstream AlephBFT backup chunks.
    pub chunks: Vec<Vec<u8>>,
    /// Whether this page reaches the peer's current append-only backup end.
    pub complete: bool,
}

impl FederationAlephBftBackupResponse {
    /// Construct one bounded backup page response.
    #[must_use]
    pub fn new(
        federation_id: FederationId,
        member_id: MemberId,
        request: FederationAlephBftBackupRequest,
        chunks: Vec<Vec<u8>>,
        complete: bool,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            member_id,
            session: request.session,
            from_chunk: request.next_chunk,
            chunks,
            complete,
        }
    }

    /// Return the exact encoded JSON size used by the private REST transport.
    pub fn encoded_size(&self) -> Result<usize> {
        Ok(serde_json::to_vec(self)?.len())
    }

    /// Validate response identity, cursor, progress, and independent page bounds.
    pub fn validate(
        &self,
        expected_federation_id: FederationId,
        expected_member_id: MemberId,
        request: FederationAlephBftBackupRequest,
    ) -> Result<()> {
        if self.version != FEDERATION_REST_API_VERSION {
            return Err(FederationError::RestVersionMismatch {
                expected: FEDERATION_REST_API_VERSION,
                found: self.version,
            });
        }
        if self.federation_id != expected_federation_id {
            return Err(FederationError::RestFederationMismatch {
                expected: expected_federation_id,
                found: self.federation_id,
            });
        }
        if self.member_id != expected_member_id {
            return Err(FederationError::CheckpointInvalid {
                reason: "AlephBFT backup response member mismatch".to_string(),
            });
        }
        if self.session != request.session || self.from_chunk != request.next_chunk {
            return Err(FederationError::CheckpointInvalid {
                reason: "AlephBFT backup response session or chunk cursor mismatch".to_string(),
            });
        }
        if self.chunks.len() > FEDERATION_MAX_ALEPH_BFT_BACKUP_CHUNKS_PER_PAGE {
            return Err(FederationError::CheckpointCatchUpTooLarge {
                resource: "AlephBFT backup chunks",
                actual: u64::try_from(self.chunks.len()).unwrap_or(u64::MAX),
                max: FEDERATION_MAX_ALEPH_BFT_BACKUP_CHUNKS_PER_PAGE as u64,
            });
        }
        let mut raw_bytes = 0usize;
        for chunk in &self.chunks {
            if chunk.is_empty() {
                return Err(FederationError::CheckpointInvalid {
                    reason: "AlephBFT backup response contains an empty chunk".to_string(),
                });
            }
            raw_bytes = raw_bytes.checked_add(chunk.len()).ok_or({
                FederationError::CheckpointCatchUpTooLarge {
                    resource: "AlephBFT backup raw bytes",
                    actual: u64::MAX,
                    max: FEDERATION_MAX_ALEPH_BFT_BACKUP_RAW_BYTES_PER_PAGE as u64,
                }
            })?;
        }
        if raw_bytes > FEDERATION_MAX_ALEPH_BFT_BACKUP_RAW_BYTES_PER_PAGE {
            return Err(FederationError::CheckpointCatchUpTooLarge {
                resource: "AlephBFT backup raw bytes",
                actual: u64::try_from(raw_bytes).unwrap_or(u64::MAX),
                max: FEDERATION_MAX_ALEPH_BFT_BACKUP_RAW_BYTES_PER_PAGE as u64,
            });
        }
        if self.chunks.is_empty() && !self.complete {
            return Err(FederationError::CheckpointInvalid {
                reason: "AlephBFT backup response made no progress".to_string(),
            });
        }
        let encoded_size = self.encoded_size()?;
        if encoded_size > FEDERATION_MAX_ALEPH_BFT_BACKUP_PAGE_BYTES {
            return Err(FederationError::CheckpointCatchUpTooLarge {
                resource: "AlephBFT backup page",
                actual: u64::try_from(encoded_size).unwrap_or(u64::MAX),
                max: FEDERATION_MAX_ALEPH_BFT_BACKUP_PAGE_BYTES as u64,
            });
        }
        Ok(())
    }
}
