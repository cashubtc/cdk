//! Checkpoint state transfer.
//!
//! A member that cannot replay the journal from its own frontier installs the
//! materialized state of a scheduled checkpoint instead. The exporter freezes
//! the state rows when it captures the checkpoint, because its live database
//! moves past the checkpoint at once. The installer downloads the rows in
//! bounded pages from any peer and accepts them only if they hash to the
//! checkpoint's quorum-signed `materialized_state_digest`, so one honest peer
//! suffices and the quorum signature is the trust anchor.

use serde::{Deserialize, Serialize};

use super::{
    FederationCheckpointError, FederationCheckpointId, FederationId,
    FederationMaterializedStateDigest, FederationMaterializedStateRow,
    FederationMaterializedStateSnapshot, Result, FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES,
    FEDERATION_REST_API_VERSION,
};

/// Scheduled-checkpoint snapshots a member keeps: the latest one and the one
/// before it, which is the retention floor.
pub const FEDERATION_STATE_SNAPSHOT_RETAINED: u64 = 2;
/// Rows returned by one state-transfer page when the request names no limit.
pub const DEFAULT_FEDERATION_STATE_TRANSFER_PAGE_ROWS: u16 = 500;
/// Most rows one state-transfer page may return.
pub const MAX_FEDERATION_STATE_TRANSFER_PAGE_ROWS: u16 = 5_000;
/// Most encoded page bytes one state transfer downloads. The installer holds
/// the whole snapshot in memory to write it in one transaction.
pub const FEDERATION_MAX_STATE_TRANSFER_BYTES: u64 = 1024 * 1024 * 1024;

/// One frozen row of a checkpoint's materialized state.
///
/// `row` is exactly the row hashed into the checkpoint's materialized-state
/// digest. `attachment` carries bytes a row covers only by hash (the full
/// payment observation behind an evidence hash); the installer checks it
/// against the row before using it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationStateSnapshotRow {
    /// Row hashed into the materialized-state digest.
    pub row: FederationMaterializedStateRow,
    /// Bytes the row covers by hash, when it does not carry them itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<String>,
}

impl FederationStateSnapshotRow {
    /// Wrap a digest row that carries all of its data.
    #[must_use]
    pub fn new(row: FederationMaterializedStateRow) -> Self {
        Self {
            row,
            attachment: None,
        }
    }

    /// Wrap a digest row together with the bytes it covers by hash.
    #[must_use]
    pub fn with_attachment(row: FederationMaterializedStateRow, attachment: String) -> Self {
        Self {
            row,
            attachment: Some(attachment),
        }
    }
}

/// Digest of frozen snapshot rows, as signed by a checkpoint.
pub fn federation_state_snapshot_digest(
    rows: &[FederationStateSnapshotRow],
) -> Result<FederationMaterializedStateDigest> {
    FederationMaterializedStateSnapshot::from_rows(rows.iter().map(|row| row.row.clone())).digest()
}

/// Sort snapshot rows into the order they are stored and served in.
///
/// The digest sorts rows itself, so the order does not change it; a fixed
/// order makes pages stable across requests and lets the installer reject
/// duplicate rows.
pub fn sort_federation_state_snapshot_rows(rows: &mut [FederationStateSnapshotRow]) {
    rows.sort_by(|left, right| left.row.cmp(&right.row));
}

/// Reject snapshot rows that are not strictly increasing.
pub fn ensure_federation_state_snapshot_rows_sorted(
    rows: &[FederationStateSnapshotRow],
) -> Result<()> {
    if rows.windows(2).any(|pair| pair[0].row >= pair[1].row) {
        return Err(FederationCheckpointError::StateTransferInvalid {
            reason: "snapshot rows are not strictly sorted",
        }
        .into());
    }
    Ok(())
}

/// Metadata of one stored checkpoint snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationStateSnapshotInfo {
    /// Scheduled checkpoint whose state the snapshot froze.
    pub checkpoint_id: FederationCheckpointId,
    /// Scheduled sequence of that checkpoint.
    pub scheduled_checkpoint_sequence: u64,
    /// Number of stored rows.
    pub row_count: u64,
    /// Digest of the stored rows, equal to the checkpoint's signed digest.
    pub materialized_state_digest: FederationMaterializedStateDigest,
}

/// Request one page of a stored checkpoint snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationStateTransferRequest {
    /// Scheduled checkpoint whose snapshot is requested.
    pub checkpoint_id: FederationCheckpointId,
    /// First row to return.
    pub next_row_index: u64,
    /// Optional maximum rows to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

impl FederationStateTransferRequest {
    /// Request rows of one checkpoint snapshot from a row cursor.
    #[must_use]
    pub fn new(checkpoint_id: FederationCheckpointId, next_row_index: u64) -> Self {
        Self {
            checkpoint_id,
            next_row_index,
            limit: None,
        }
    }

    /// Set an optional row limit.
    #[must_use]
    pub fn with_limit(mut self, limit: u16) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Return the bounded row limit to apply to a response.
    #[must_use]
    pub fn effective_limit(&self) -> usize {
        usize::from(
            self.limit
                .unwrap_or(DEFAULT_FEDERATION_STATE_TRANSFER_PAGE_ROWS)
                .clamp(1, MAX_FEDERATION_STATE_TRANSFER_PAGE_ROWS),
        )
    }
}

/// One page of a stored checkpoint snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationStateTransferResponse {
    /// REST API version.
    pub version: u16,
    /// Federation that owns the checkpoint.
    pub federation_id: FederationId,
    /// Checkpoint whose snapshot the rows belong to.
    pub checkpoint_id: FederationCheckpointId,
    /// Index of the first returned row.
    pub from_row_index: u64,
    /// Number of rows in the whole snapshot.
    pub row_count: u64,
    /// Returned rows, in stored order.
    pub rows: Vec<FederationStateSnapshotRow>,
}

impl FederationStateTransferResponse {
    /// Build a page.
    #[must_use]
    pub fn new(
        federation_id: FederationId,
        checkpoint_id: FederationCheckpointId,
        from_row_index: u64,
        row_count: u64,
        rows: Vec<FederationStateSnapshotRow>,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            checkpoint_id,
            from_row_index,
            row_count,
            rows,
        }
    }

    /// Encoded JSON size used for the protocol page limit.
    pub fn encoded_size(&self) -> Result<usize> {
        Ok(serde_json::to_vec(self)?.len())
    }

    /// Reject a page above the fixed protocol response limit.
    pub fn ensure_within_page_limit(&self) -> Result<usize> {
        let actual = self.encoded_size()?;
        if actual > FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES {
            return Err(FederationCheckpointError::CatchUpTooLarge {
                resource: "state transfer page",
                actual: actual as u64,
                max: FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES as u64,
            }
            .into());
        }
        Ok(actual)
    }

    /// Index after the last returned row.
    pub fn next_row_index(&self) -> Result<u64> {
        let returned =
            u64::try_from(self.rows.len()).map_err(|_| invalid("page length exceeds u64"))?;
        self.from_row_index
            .checked_add(returned)
            .ok_or_else(|| invalid("row cursor overflow"))
    }

    /// Whether this page ends the snapshot.
    pub fn is_complete(&self) -> Result<bool> {
        Ok(self.next_row_index()? == self.row_count)
    }

    /// Check that this page answers `request` for `federation_id` and makes
    /// progress unless the snapshot is already complete.
    pub fn validate_for_request(
        &self,
        federation_id: FederationId,
        request: &FederationStateTransferRequest,
    ) -> Result<()> {
        if self.version != FEDERATION_REST_API_VERSION {
            return Err(super::FederationError::RestVersionMismatch {
                expected: FEDERATION_REST_API_VERSION,
                found: self.version,
            });
        }
        if self.federation_id != federation_id {
            return Err(super::FederationError::CatchUpFederationMismatch {
                expected: federation_id,
                found: self.federation_id,
            });
        }
        if self.checkpoint_id != request.checkpoint_id {
            return Err(invalid("page belongs to another checkpoint"));
        }
        if self.from_row_index != request.next_row_index {
            return Err(invalid("page does not start at the requested row"));
        }
        if self.rows.len() > request.effective_limit() {
            return Err(invalid("page exceeds the requested row limit"));
        }
        if self.next_row_index()? > self.row_count {
            return Err(invalid("page extends past the snapshot"));
        }
        if self.rows.is_empty() && !self.is_complete()? {
            return Err(invalid("page makes no progress"));
        }
        ensure_federation_state_snapshot_rows_sorted(&self.rows)
    }
}

fn invalid(reason: &'static str) -> super::FederationError {
    FederationCheckpointError::StateTransferInvalid { reason }.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str) -> FederationStateSnapshotRow {
        FederationStateSnapshotRow::new(FederationMaterializedStateRow::new("table", key))
    }

    fn response(
        from: u64,
        count: u64,
        rows: Vec<FederationStateSnapshotRow>,
    ) -> FederationStateTransferResponse {
        FederationStateTransferResponse::new(
            FederationId::from_bytes([1; 32]),
            FederationCheckpointId::from_bytes([2; 32]),
            from,
            count,
            rows,
        )
    }

    #[test]
    fn digest_ignores_row_order_and_attachments() {
        let mut rows = vec![row("b"), row("a")];
        let digest = federation_state_snapshot_digest(&rows).expect("digest");
        sort_federation_state_snapshot_rows(&mut rows);
        rows[0].attachment = Some("evidence".to_string());
        assert_eq!(
            federation_state_snapshot_digest(&rows).expect("sorted digest"),
            digest
        );
        ensure_federation_state_snapshot_rows_sorted(&rows).expect("sorted rows");
        rows.push(row("b"));
        assert!(ensure_federation_state_snapshot_rows_sorted(&rows).is_err());
    }

    #[test]
    fn page_validation_binds_checkpoint_cursor_and_progress() {
        let request =
            FederationStateTransferRequest::new(FederationCheckpointId::from_bytes([2; 32]), 1)
                .with_limit(2);
        let federation_id = FederationId::from_bytes([1; 32]);
        response(1, 3, vec![row("a"), row("b")])
            .validate_for_request(federation_id, &request)
            .expect("valid page");
        assert!(response(0, 3, vec![row("a")])
            .validate_for_request(federation_id, &request)
            .is_err());
        assert!(response(1, 3, vec![row("a"), row("b"), row("c")])
            .validate_for_request(federation_id, &request)
            .is_err());
        assert!(response(1, 2, vec![row("a"), row("b")])
            .validate_for_request(federation_id, &request)
            .is_err());
        assert!(response(1, 3, Vec::new())
            .validate_for_request(federation_id, &request)
            .is_err());
        assert!(response(1, 3, vec![row("b"), row("a")])
            .validate_for_request(federation_id, &request)
            .is_err());
        let done =
            FederationStateTransferRequest::new(FederationCheckpointId::from_bytes([2; 32]), 3);
        response(3, 3, Vec::new())
            .validate_for_request(federation_id, &done)
            .expect("empty final page");
    }
}
