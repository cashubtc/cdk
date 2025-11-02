use std::future::Future;

use futures::future::BoxFuture;

use crate::amount::SplitTarget;
use crate::nuts::{Proofs, State};
use crate::{Error, Wallet};

/// Size of proofs to send to avoid hitting the mint limit.
const BATCH_PROOF_SIZE: usize = 100;

impl Wallet {
    /// Perform an async task, which is assumed to be a foreign mint call that can fail. If fails,
    /// the proofs used in the request are set as unspent, then they are swapped, as they are
    /// believed to be already shown to the mint
    #[inline(always)]
    pub(crate) fn try_proof_operation<'a, F, R>(
        &'a self,
        inputs: Proofs,
        f: F,
    ) -> BoxFuture<'a, F::Output>
    where
        F: Future<Output = Result<R, Error>> + Send + 'a,
        R: Send + Sync,
    {
        Box::pin(async move {
            match f.await {
                Ok(r) => Ok(r),
                Err(err) => {
                    tracing::error!(
                        "Http operation failed, revering  {} proofs states to UNSPENT",
                        inputs.len()
                    );

                    // Although the proofs has been leaked already, we cannot swap them internally to
                    // recover them, at least we flag it as unspent.
                    self.localstore
                        .update_proofs_state(
                            inputs
                                .iter()
                                .map(|x| x.y())
                                .collect::<Result<Vec<_>, _>>()?,
                            State::Unspent,
                        )
                        .await?;

                    tracing::error!(
                        "Attempting to swap exposed {} proofs to new proofs",
                        inputs.len()
                    );

                    for proofs in inputs.chunks(BATCH_PROOF_SIZE) {
                        let _ = self
                            .swap(None, SplitTarget::None, proofs.to_owned(), None, true)
                            .await?;
                    }

                    Err(err)
                }
            }
        })
    }
}
