use std::collections::HashMap;

use cdk_common::{CheckStateRequest, ProofsMethods};
use tracing::instrument;

use crate::nuts::Proofs;
use crate::{Error, Wallet};

impl Wallet {
    /// Synchronizes the states with the mint
    #[instrument(skip(self, proofs))]
    pub async fn sync_proofs_state(&self, proofs: Proofs) -> Result<(), Error> {
        let proof_ys = proofs.ys()?;

        let statuses = self
            .client
            .post_check_state(CheckStateRequest { ys: proof_ys })
            .await?
            .states;

        for (state, unspent) in proofs
            .into_iter()
            .zip(statuses)
            .map(|(p, s)| (s.state, p))
            .fold(HashMap::<_, Vec<_>>::new(), |mut acc, (cat, item)| {
                acc.entry(cat).or_default().push(item);
                acc
            })
        {
            self.localstore
                .update_proofs_state(
                    unspent
                        .iter()
                        .map(|x| x.y())
                        .collect::<Result<Vec<_>, _>>()?,
                    state,
                )
                .await?;
        }

        Ok(())
    }
}
