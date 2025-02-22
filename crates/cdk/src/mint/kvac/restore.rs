use cdk_common::kvac::{KvacRestoreRequest, KvacRestoreResponse};
use tracing::instrument;

use crate::{Error, Mint};

/// Restore

impl Mint {
    /// Restore KVAC coins from tags
    #[instrument(skip_all)]
    pub async fn kvac_restore(
        &self,
        request: KvacRestoreRequest,
    ) -> Result<KvacRestoreResponse, Error> {
        let tags = request.tags;

        let issued_macs = self.localstore.get_kvac_issued_macs_by_tags(&tags).await?;

        Ok(KvacRestoreResponse { issued_macs })
    }
}
