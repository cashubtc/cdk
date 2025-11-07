use std::future::Future;

use crate::nuts::{Proofs, State};
use crate::{Error, Wallet};

#[cfg(not(target_arch = "wasm32"))]
type BoxFuture<'a, T> = futures::future::BoxFuture<'a, T>;

///
#[cfg(target_arch = "wasm32")]
type BoxFuture<'a, T> = futures::future::LocalBoxFuture<'a, T>;

/// MaybeSend
///
/// Which is Send for most platforms but WASM.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}

#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}

/// Autoimplement MaybeSend for T
#[cfg(not(target_arch = "wasm32"))]
impl<T: ?Sized + Send> MaybeSend for T {}

#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

/// Size of proofs to send to avoid hitting the mint limit.
const BATCH_PROOF_SIZE: usize = 100;

impl Wallet {
    /// Perform an async task, which is assumed to be a foreign mint call that can fail. If fails,
    /// the proofs used in the request are set as unspent, then they are swapped, as they are
    /// believed to be already shown to the mint
    #[inline(always)]
    pub(crate) fn try_proof_operation_or_reclaim<'a, F, R>(
        &'a self,
        inputs: Proofs,
        f: F,
    ) -> BoxFuture<'a, F::Output>
    where
        F: Future<Output = Result<R, Error>> + MaybeSend + 'a,
        R: MaybeSend + Sync + 'a,
    {
        Box::pin(async move {
            match f.await {
                Ok(r) => Ok(r),
                Err(err) => {
                    tracing::error!(
                        "Http operation failed with \"{}\", revering  {} proofs states to UNSPENT",
                        err,
                        inputs.len()
                    );

                    let swap_reverted_proofs = self
                        .in_error_swap_reverted_proofs
                        .compare_exchange(
                            false,
                            true,
                            std::sync::atomic::Ordering::SeqCst,
                            std::sync::atomic::Ordering::SeqCst,
                        )
                        .is_ok();

                    if swap_reverted_proofs {
                        tracing::error!(
                            "Attempting to swap exposed {} proofs to new proofs",
                            inputs.len()
                        );
                        for proofs in inputs.chunks(BATCH_PROOF_SIZE) {
                            if let Err(inner_err) = self.reclaim_unspent(proofs.to_owned()).await {
                                println!(
                                    "Failed to swap exposed proofs ({}), updating local database instead", inner_err
                                );
                                tracing::warn!(
                                    "Failed to swap exposed proofs ({}), updating local database instead", inner_err
                                );

                                let _ = self
                                    .localstore
                                    .update_proofs_state(
                                        proofs
                                            .iter()
                                            .map(|x| x.y())
                                            .collect::<Result<Vec<_>, _>>()?,
                                        State::Unspent,
                                    )
                                    .await
                                    .inspect_err(|err| {
                                        tracing::error!("Failed err update_proofs_state {}", err)
                                    });
                            }
                        }

                        self.in_error_swap_reverted_proofs
                            .store(false, std::sync::atomic::Ordering::SeqCst);
                    }

                    Err(err)
                }
            }
        })
    }
}
