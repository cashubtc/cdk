//! Renewable ownership of a melt execution or recovery pass.

use std::future::Future;
use std::time::Duration;

use cdk_common::database::{DynMintDatabase, DynMintTransaction};
use cdk_common::mint::MeltQuote;
use cdk_common::{Error, MeltQuoteState, QuoteId};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub(crate) const MELT_LEASE_SECONDS: u64 = 60;
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) async fn check_melt_lease(
    tx: &mut DynMintTransaction,
    quote: &MeltQuote,
    owner: &str,
) -> Result<(), Error> {
    let valid = if owner.is_empty() {
        !quote.is_locked()
    } else {
        quote.melt_lock == owner && quote.melt_lock_expires_at > tx.melt_lease_time().await?
    };
    if valid {
        Ok(())
    } else {
        Err(Error::MeltQuoteLocked)
    }
}

pub(crate) struct MeltLease {
    db: DynMintDatabase,
    quote_id: QuoteId,
    owner: String,
    heartbeat: JoinHandle<()>,
    lost: CancellationToken,
    deadline: Instant,
    released: bool,
}

impl MeltLease {
    pub(crate) fn owned(db: DynMintDatabase, quote: &MeltQuote) -> Self {
        Self::with_timing(
            db,
            quote,
            Duration::from_secs(MELT_LEASE_SECONDS / 4),
            EXECUTION_TIMEOUT,
        )
    }

    fn with_timing(
        db: DynMintDatabase,
        quote: &MeltQuote,
        interval: Duration,
        budget: Duration,
    ) -> Self {
        let quote_id = quote.id.clone();
        let owner = quote.melt_lock.clone();
        let lost = CancellationToken::new();
        let deadline = Instant::now() + budget;
        let heartbeat = tokio::spawn({
            let db = db.clone();
            let quote_id = quote_id.clone();
            let owner = owner.clone();
            let lost = lost.clone();
            async move {
                let _ = tokio::time::timeout_at(deadline, async {
                    loop {
                        tokio::time::sleep(interval).await;
                        let result = async {
                            #[cfg(test)]
                            if crate::test_helpers::mint::take_fail_for("MELT_LEASE_RENEW") {
                                return Err(Error::Internal);
                            }
                            let mut tx = db.begin_transaction().await?;
                            let renewed = tx
                                .renew_melt_quote_lease(&quote_id, &owner, MELT_LEASE_SECONDS)
                                .await?;
                            tx.commit().await?;
                            Ok::<_, Error>(renewed)
                        }
                        .await;
                        match result {
                            Ok(true) => {}
                            Ok(false) => {
                                // Finalization can clear ownership before returning.
                                if let Ok(Some(quote)) = db.get_melt_quote(&quote_id).await {
                                    if !quote.is_locked() && quote.state != MeltQuoteState::Pending
                                    {
                                        return;
                                    }
                                }
                                lost.cancel();
                                return;
                            }
                            Err(err) => {
                                tracing::warn!(
                                    "Could not renew melt lease for {}: {}",
                                    quote_id,
                                    err
                                );
                            }
                        }
                    }
                })
                .await;
            }
        });
        Self {
            db,
            quote_id,
            owner,
            heartbeat,
            lost,
            deadline,
            released: false,
        }
    }

    pub(crate) async fn claim(
        db: &DynMintDatabase,
        quote: &mut MeltQuote,
    ) -> Result<Option<Self>, Error> {
        #[cfg(test)]
        if crate::test_helpers::mint::take_fail_for("MELT_LEASE_CLAIM") {
            return Err(Error::Internal);
        }
        let mut tx = db.begin_transaction().await?;
        let Some(current) = tx
            .claim_melt_quote_lease(
                &quote.id,
                &uuid::Uuid::now_v7().to_string(),
                MELT_LEASE_SECONDS,
            )
            .await?
        else {
            *quote = tx
                .get_melt_quote(&quote.id)
                .await?
                .ok_or(Error::UnknownQuote)?
                .inner();
            tx.rollback().await?;
            return Ok(None);
        };
        tx.commit().await?;
        *quote = current;
        Ok(Some(Self::owned(db.clone(), quote)))
    }

    pub(crate) async fn run<T>(
        &self,
        work: impl Future<Output = Result<T, Error>>,
    ) -> Result<T, Error> {
        #[cfg(test)]
        {
            if crate::test_helpers::mint::take_fail_for("MELT_LEASE_LOST") {
                return Err(Error::MeltQuoteLocked);
            }
            if crate::test_helpers::mint::take_fail_for("MELT_LEASE_TIMEOUT") {
                return Err(Error::PendingMeltTimeout {
                    last_backend_error: None,
                });
            }
        }
        tokio::select! {
            biased;
            _ = self.lost.cancelled() => Err(Error::MeltQuoteLocked),
            _ = tokio::time::sleep_until(self.deadline) => Err(Error::PendingMeltTimeout { last_backend_error: None }),
            result = work => result,
        }
    }

    pub(crate) async fn release(&mut self) -> Result<(), Error> {
        if !self.released {
            self.heartbeat.abort();
            let _ = (&mut self.heartbeat).await;
            #[cfg(test)]
            if crate::test_helpers::mint::take_fail_for("MELT_LEASE_RELEASE") {
                return Err(Error::Internal);
            }
            release_owner(&self.db, &self.quote_id, &self.owner).await?;
            self.released = true;
        }
        Ok(())
    }
}

async fn release_owner(db: &DynMintDatabase, quote_id: &QuoteId, owner: &str) -> Result<(), Error> {
    let mut tx = db.begin_transaction().await?;
    tx.unlock_melt_quote(quote_id, owner).await?;
    tx.commit().await?;
    Ok(())
}

impl Drop for MeltLease {
    fn drop(&mut self) {
        self.heartbeat.abort();
        if !self.released {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let db = self.db.clone();
                let quote_id = self.quote_id.clone();
                let owner = self.owner.clone();
                runtime.spawn(async move {
                    if let Err(err) = release_owner(&db, &quote_id, &owner).await {
                        tracing::warn!("Could not release melt lease for {}: {}", quote_id, err);
                    }
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/lease_tests.rs"]
mod tests;
