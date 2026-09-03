//! Building and serving compact state filters

use cdk_common::database::mint::StateFilterConfig;
use cdk_common::database::DynMintDatabase;
use cdk_common::nuts::state_filters::{
    encode, FilterKind, GetFiltersInfoResponse, GetFiltersResponse, PendingFilterResponse,
};
use cdk_common::util::unix_time;
use cdk_common::Error;

use super::{StateFilters, BUILD_GRACE_SECONDS};

/// Builds one filter per epoch and serves the published pages.
#[derive(Clone)]
pub struct StateFilterService {
    db: DynMintDatabase,
    config: StateFilterConfig,
    kinds: Vec<FilterKind>,
    pending: bool,
}

impl std::fmt::Debug for StateFilterService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateFilterService")
            .field("config", &self.config)
            .field("kinds", &self.kinds)
            .field("pending", &self.pending)
            .finish()
    }
}

impl StateFilterService {
    /// Load or establish the filter parameters.
    ///
    /// The stored parameters win, and a mismatch is refused rather than
    /// silently renumbering pages or changing the false positive rate under
    /// wallets that already hold history. `genesis` is only chosen the first
    /// time, aligned down to a multiple of the epoch so hourly epochs land on
    /// the hour.
    pub async fn new(
        db: DynMintDatabase,
        epoch_seconds: u64,
        p: u8,
        page_size: u64,
        kinds: Vec<FilterKind>,
        pending: bool,
    ) -> Result<Self, Error> {
        if epoch_seconds == 0 || page_size == 0 {
            return Err(Error::StateFilterConfig(
                "epoch and page size must be non-zero".to_string(),
            ));
        }

        let config = match db.get_state_filter_config().await? {
            Some(stored) => {
                if stored.epoch_seconds != epoch_seconds
                    || stored.p != p
                    || stored.page_size != page_size
                {
                    return Err(Error::StateFilterConfig(format!(
                        "stored parameters (epoch {}, p {}, page size {}) cannot be changed to (epoch {}, p {}, page size {})",
                        stored.epoch_seconds, stored.p, stored.page_size, epoch_seconds, p, page_size
                    )));
                }
                stored
            }
            None => {
                let now = unix_time();
                let config = StateFilterConfig {
                    genesis: now - (now % epoch_seconds),
                    epoch_seconds,
                    p,
                    page_size,
                };

                let mut tx = db.begin_transaction().await?;
                tx.set_state_filter_config(&config).await?;
                tx.commit().await?;

                config
            }
        };

        Ok(Self {
            db,
            config,
            kinds,
            pending,
        })
    }

    /// How often the builder should look for epochs to close.
    ///
    /// Capped so a mint with a long epoch still closes promptly after the
    /// grace period rather than a whole epoch late.
    pub fn build_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.config.epoch_seconds.clamp(1, 60))
    }

    /// The recorder that capture points write elements through.
    pub fn recorder(&self) -> StateFilters {
        StateFilters::new(self.config, self.kinds.clone())
    }

    fn epoch_start(&self, epoch: u64) -> u64 {
        self.config
            .genesis
            .saturating_add(epoch.saturating_mul(self.config.epoch_seconds))
    }

    fn epoch_end(&self, epoch: u64) -> u64 {
        self.epoch_start(epoch.saturating_add(1))
    }

    fn open_epoch(&self, now: u64) -> u64 {
        now.saturating_sub(self.config.genesis) / self.config.epoch_seconds
    }

    async fn built_count(&self) -> Result<u64, Error> {
        Ok(self
            .db
            .latest_built_epoch()
            .await?
            .map(|epoch| epoch.saturating_add(1))
            .unwrap_or(0))
    }

    /// Close every epoch whose grace period has passed.
    ///
    /// An epoch with no elements still gets a filter with empty data, which is
    /// what keeps the published history contiguous. Running this at startup is
    /// what closes the epochs that ended while the mint was down.
    pub async fn build_due_epochs(&self) -> Result<u64, Error> {
        let now = unix_time();
        let mut next = self.built_count().await?;
        let mut built = 0;

        while self.epoch_end(next).saturating_add(BUILD_GRACE_SECONDS) <= now {
            let mut tx = self.db.begin_transaction().await?;

            let elements = tx.take_filter_elements(next).await?;
            let data = encode(&elements, self.config.p)?;
            tx.add_filter(next, self.epoch_start(next), self.epoch_end(next), &data)
                .await?;

            tx.commit().await?;

            built += 1;
            next += 1;
        }

        Ok(built)
    }

    /// Parameters of the published filters.
    pub async fn info(&self) -> Result<GetFiltersInfoResponse, Error> {
        let built = self.built_count().await?;
        let current_page = built / self.config.page_size;

        let latest_end = match built {
            0 => self.config.genesis,
            built => self.epoch_end(built - 1),
        };

        Ok(GetFiltersInfoResponse {
            p: self.config.p,
            epoch: self.config.epoch_seconds,
            kinds: self.kinds.clone(),
            page_size: self.config.page_size,
            first_page: 0,
            current_page,
            current_page_count: built - current_page * self.config.page_size,
            earliest_start: self.config.genesis,
            latest_end,
            pending: self.pending,
        })
    }

    /// A page of filters, oldest first.
    ///
    /// Every page below `current_page` is complete and never changes again.
    pub async fn page(&self, page: u64) -> Result<GetFiltersResponse, Error> {
        let built = self.built_count().await?;
        let current_page = built / self.config.page_size;

        if page > current_page {
            return Err(Error::FilterPageOutOfRange);
        }

        let filters = self
            .db
            .get_filters(page * self.config.page_size, self.config.page_size)
            .await?;

        Ok(GetFiltersResponse { page, filters })
    }

    /// Whether this page is complete and safe to cache forever.
    pub async fn page_is_complete(&self, page: u64) -> Result<bool, Error> {
        Ok(page < self.built_count().await? / self.config.page_size)
    }

    /// The filter of the epoch that is still open.
    pub async fn pending(&self) -> Result<PendingFilterResponse, Error> {
        if !self.pending {
            return Err(Error::FilterNotAvailable);
        }

        let epoch = self.open_epoch(unix_time());
        let elements = self.db.get_filter_elements(epoch).await?;

        Ok(PendingFilterResponse {
            start: self.epoch_start(epoch),
            data: cdk_common::util::hex::encode(encode(&elements, self.config.p)?),
        })
    }
}
