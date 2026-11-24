//! Download a peer's snapshot of a scheduled checkpoint and install it.
//!
//! A member that cannot replay the journal from its own frontier installs the
//! checkpoint's materialized state instead. Pages may come from any peer that
//! stored the snapshot: the installer accepts the rows only if they hash to
//! the checkpoint's quorum-signed digest. After the install the caller
//! restarts the federation runtime, which starts from the checkpoint, and
//! catches up the journal suffix with the ordinary protocol.

use cdk::federation::{
    FederationCheckpointError, FederationCheckpointKind, FederationCheckpointQuorum,
    FederationError, FederationStateSnapshotRow, FederationStateTransferRequest,
    FederationStateTransferWindow, FederationStateTransferWindowRequest, MemberConfigPublic,
    FEDERATION_MAX_STATE_TRANSFER_BYTES,
};
use cdk::Mint;
use cdk_http_client::Transport;

use super::peer_client::FederationRestPeerClient;
use super::sync_loop::checked_checkpoint_transfer_bytes;

fn transfer_invalid(reason: &'static str) -> FederationError {
    FederationCheckpointError::StateTransferInvalid { reason }.into()
}

impl<T> FederationRestPeerClient<T>
where
    T: Transport,
{
    /// Download every page of `member`'s snapshot of the quorum's scheduled
    /// checkpoint.
    pub async fn download_state_snapshot_from_member(
        &self,
        member: &MemberConfigPublic,
        quorum: &FederationCheckpointQuorum,
    ) -> anyhow::Result<Vec<FederationStateSnapshotRow>> {
        if quorum.checkpoint.kind != FederationCheckpointKind::Scheduled {
            return Err(
                transfer_invalid("state transfers install scheduled checkpoints only").into(),
            );
        }
        let checkpoint_id = quorum.checkpoint.id()?;
        let mut rows = Vec::new();
        let mut row_count = None;
        let mut transferred_bytes = 0_u64;
        loop {
            let next_row_index = u64::try_from(rows.len())
                .map_err(|_| transfer_invalid("snapshot row count exceeds u64"))?;
            let request = FederationStateTransferRequest::new(checkpoint_id, next_row_index)
                .with_limit(self.sync_config.catch_up_limit);
            let page = self.fetch_state_transfer_page(member, &request).await?;
            let page_bytes = u64::try_from(page.encoded_size()?).unwrap_or(u64::MAX);
            transferred_bytes = checked_checkpoint_transfer_bytes(
                transferred_bytes,
                page_bytes,
                FEDERATION_MAX_STATE_TRANSFER_BYTES,
            )?;
            match row_count {
                Some(count) if count != page.row_count => {
                    return Err(transfer_invalid("snapshot row count changed between pages").into());
                }
                Some(_) => {}
                None => row_count = Some(page.row_count),
            }
            let complete = page.is_complete()?;
            if let (Some(last), Some(first)) = (rows.last(), page.rows.first()) {
                let last: &FederationStateSnapshotRow = last;
                if last.row >= first.row {
                    return Err(transfer_invalid("snapshot pages are not in row order").into());
                }
            }
            rows.extend(page.rows);
            if complete {
                return Ok(rows);
            }
        }
    }

    /// Download every page of `member`'s validity window before the quorum's
    /// checkpoint: its finalized items and journal entries.
    pub async fn download_state_transfer_window_from_member(
        &self,
        member: &MemberConfigPublic,
        quorum: &FederationCheckpointQuorum,
    ) -> anyhow::Result<FederationStateTransferWindow> {
        let checkpoint = &quorum.checkpoint;
        let checkpoint_id = checkpoint.id()?;
        let mut bounds = None;
        let mut items = Vec::new();
        let mut entries = Vec::new();
        let mut next_item_index = None;
        let mut next_journal_index = None;
        let mut transferred_bytes = 0_u64;
        loop {
            let request = FederationStateTransferWindowRequest {
                checkpoint_id,
                next_item_index,
                next_journal_index,
                limit: Some(self.sync_config.catch_up_limit),
            };
            let page = self
                .fetch_state_transfer_window_page(member, checkpoint, &request)
                .await?;
            let page_bytes = u64::try_from(page.encoded_size()?).unwrap_or(u64::MAX);
            transferred_bytes = checked_checkpoint_transfer_bytes(
                transferred_bytes,
                page_bytes,
                FEDERATION_MAX_STATE_TRANSFER_BYTES,
            )?;
            match bounds {
                Some(bounds) if bounds != page.bounds => {
                    return Err(transfer_invalid("window bounds changed between pages").into());
                }
                Some(_) => {}
                None => bounds = Some(page.bounds),
            }
            let item_cursor = page
                .from_item_index
                .checked_add(page.items.len() as u64)
                .ok_or_else(|| transfer_invalid("window item cursor overflow"))?;
            let journal_cursor = page
                .from_journal_index
                .checked_add(page.entries.len() as u64)
                .ok_or_else(|| transfer_invalid("window journal cursor overflow"))?;
            items.extend(page.items);
            entries.extend(page.entries);
            if item_cursor == checkpoint.finalized_next_item_index
                && journal_cursor == checkpoint.journal_next_index
            {
                let bounds = bounds.ok_or_else(|| transfer_invalid("window has no bounds"))?;
                return Ok(FederationStateTransferWindow {
                    bounds,
                    items,
                    entries,
                });
            }
            next_item_index = Some(item_cursor);
            next_journal_index = Some(journal_cursor);
        }
    }

    /// Download `member`'s snapshot of the quorum's scheduled checkpoint and
    /// its validity window, and install them into `mint`, which must hold no
    /// federation state yet.
    ///
    /// The federation runtime must be (re)started afterwards: it starts from
    /// the installed checkpoint.
    pub async fn transfer_state_from_member(
        &self,
        mint: &Mint,
        member: &MemberConfigPublic,
        quorum: &FederationCheckpointQuorum,
    ) -> anyhow::Result<()> {
        let rows = self
            .download_state_snapshot_from_member(member, quorum)
            .await?;
        let window = self
            .download_state_transfer_window_from_member(member, quorum)
            .await?;
        mint.install_federation_state_transfer(quorum, &rows, window)
            .await?;
        Ok(())
    }
}
