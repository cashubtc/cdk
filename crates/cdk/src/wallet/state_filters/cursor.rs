//! Where a wallet resumes fetching filters.
//!
//! Only the cursor is persisted. Filters are tested as they arrive and the
//! bytes are never needed again, so the wallet stores none of them.

use bitcoin::hashes::{sha256, Hash};
use cdk_common::nuts::state_filters::GetFiltersInfoResponse;
use cdk_common::Error;

use crate::Wallet;

const NAMESPACE: &str = "state_filters";
const CURSOR_KEY: &str = "cursor";

/// The point a wallet resumes from.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// First page still worth fetching
    pub next_page: u64,
    /// Start of the first epoch not yet tested
    pub next_start: u64,
}

impl Cursor {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(&self.next_page.to_be_bytes());
        bytes.extend_from_slice(&self.next_start.to_be_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != 16 {
            return Err(Error::Custom("Malformed state filter cursor".to_string()));
        }

        let mut page = [0u8; 8];
        let mut start = [0u8; 8];
        page.copy_from_slice(&bytes[..8]);
        start.copy_from_slice(&bytes[8..]);

        Ok(Self {
            next_page: u64::from_be_bytes(page),
            next_start: u64::from_be_bytes(start),
        })
    }
}

impl Wallet {
    /// The KV namespace for this mint's cursor.
    ///
    /// A mint URL contains characters the KV store rejects, so it is hashed
    /// rather than escaped: the value is opaque and only has to be stable.
    fn cursor_namespace(&self) -> String {
        let digest = sha256::Hash::hash(self.mint_url.to_string().as_bytes());
        digest.to_string()
    }

    /// Where the next sweep should resume.
    pub async fn state_filter_cursor(&self) -> Result<Cursor, Error> {
        let stored = self
            .localstore
            .kv_read(NAMESPACE, &self.cursor_namespace(), CURSOR_KEY)
            .await?;

        match stored {
            Some(bytes) => Cursor::decode(&bytes),
            None => Ok(Cursor::default()),
        }
    }

    /// Record how far a completed sweep got.
    ///
    /// The current page is still filling, so the cursor stops at it rather than
    /// past it, and `next_start` skips the epochs already tested when that page
    /// is fetched again.
    pub(crate) async fn advance_state_filter_cursor(
        &self,
        info: &GetFiltersInfoResponse,
    ) -> Result<(), Error> {
        let cursor = Cursor {
            next_page: info.current_page,
            next_start: info.latest_end,
        };

        self.localstore
            .kv_write(
                NAMESPACE,
                &self.cursor_namespace(),
                CURSOR_KEY,
                &cursor.encode(),
            )
            .await?;

        Ok(())
    }

    /// Forget the cursor, so the next sweep reads the whole retained history.
    pub async fn reset_state_filter_cursor(&self) -> Result<(), Error> {
        self.localstore
            .kv_remove(NAMESPACE, &self.cursor_namespace(), CURSOR_KEY)
            .await?;
        Ok(())
    }
}
