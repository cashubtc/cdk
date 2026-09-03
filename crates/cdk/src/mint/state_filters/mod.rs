//! Compact state filters

use cdk_common::database::mint::StateFilterConfig;
use cdk_common::database::DynMintTransaction;
use cdk_common::nuts::state_filters::{
    FilterElement, FilterKind, Settings, DEFAULT_EPOCH, DEFAULT_P, DEFAULT_PAGE_SIZE,
};
use cdk_common::util::unix_time;
use cdk_common::{Error, MeltQuoteState, PublicKey, State};

mod service;

pub use service::StateFilterService;

/// How long after an epoch ends before its filter is built.
///
/// A transaction that commits on the boundary takes its epoch from the same
/// clock as the builder, so the grace period keeps a straggler from landing an
/// element in an epoch that has already been sealed.
pub const BUILD_GRACE_SECONDS: u64 = 30;

/// A recorder shared between the mint and anything constructed before the
/// filter service exists, such as the subscription manager.
pub type SharedStateFilters = std::sync::Arc<arc_swap::ArcSwap<StateFilters>>;

/// How a mint should publish its filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateFilterOptions {
    /// Epoch duration in seconds
    pub epoch_seconds: u64,
    /// Golomb-Rice parameter
    pub p: u8,
    /// Number of filters on a full page
    pub page_size: u64,
    /// Kinds the filters cover
    pub kinds: Vec<FilterKind>,
    /// Whether to serve the filter of the epoch that is still open
    pub pending: bool,
}

impl Default for StateFilterOptions {
    fn default() -> Self {
        Self {
            epoch_seconds: DEFAULT_EPOCH,
            p: DEFAULT_P,
            page_size: DEFAULT_PAGE_SIZE,
            kinds: vec![
                FilterKind::ProofState,
                FilterKind::MintQuote,
                FilterKind::MeltQuote,
            ],
            pending: true,
        }
    }
}

/// Records filter elements as part of the transaction that changes state.
///
/// A mint that does not publish filters holds a disabled recorder, and every
/// method is then a no-op.
#[derive(Debug, Clone, Default)]
pub struct StateFilters {
    config: Option<StateFilterConfig>,
    kinds: Vec<FilterKind>,
}

impl StateFilters {
    /// A recorder that captures nothing.
    pub fn disabled() -> Self {
        Self::default()
    }

    /// A recorder for a mint that publishes filters.
    pub fn new(config: StateFilterConfig, kinds: Vec<FilterKind>) -> Self {
        Self {
            config: Some(config),
            kinds,
        }
    }

    /// Whether this mint publishes filters.
    pub fn enabled(&self) -> bool {
        self.config.is_some()
    }

    /// The stored parameters, if filters are enabled.
    pub fn config(&self) -> Option<&StateFilterConfig> {
        self.config.as_ref()
    }

    /// Mint info settings describing what these filters cover.
    pub fn settings(&self) -> Settings {
        if self.config.is_none() {
            return Settings::default();
        }
        Settings::new(self.kinds.clone())
    }

    fn covers(&self, kind: FilterKind) -> Option<&StateFilterConfig> {
        let config = self.config.as_ref()?;
        self.kinds.contains(&kind).then_some(config)
    }

    fn open_epoch(config: &StateFilterConfig, now: u64) -> u64 {
        now.saturating_sub(config.genesis) / config.epoch_seconds
    }

    async fn record(
        tx: &mut DynMintTransaction,
        config: &StateFilterConfig,
        elements: Vec<FilterElement>,
    ) -> Result<(), Error> {
        if elements.is_empty() {
            return Ok(());
        }
        let epoch = Self::open_epoch(config, unix_time());
        tx.add_filter_elements(epoch, &elements).await?;
        Ok(())
    }

    /// Record that a set of proofs moved to `state`.
    pub async fn record_proof_states(
        &self,
        tx: &mut DynMintTransaction,
        ys: &[PublicKey],
        state: State,
    ) -> Result<(), Error> {
        let Some(config) = self.covers(FilterKind::ProofState) else {
            return Ok(());
        };

        let elements = ys
            .iter()
            .map(|y| FilterElement::proof_state(y, state))
            .collect::<Result<Vec<_>, _>>()?;

        Self::record(tx, config, elements).await
    }

    /// Record that a mint quote changed.
    pub async fn record_mint_quote(
        &self,
        tx: &mut DynMintTransaction,
        quote_id: &str,
    ) -> Result<(), Error> {
        let Some(config) = self.covers(FilterKind::MintQuote) else {
            return Ok(());
        };

        Self::record(tx, config, vec![FilterElement::mint_quote(quote_id)]).await
    }

    /// Record that a melt quote moved to `state`.
    ///
    /// `MeltQuoteState::Unknown` has no filter representation and is skipped.
    pub async fn record_melt_quote(
        &self,
        tx: &mut DynMintTransaction,
        quote_id: &str,
        state: MeltQuoteState,
    ) -> Result<(), Error> {
        let Some(config) = self.covers(FilterKind::MeltQuote) else {
            return Ok(());
        };

        if state == MeltQuoteState::Unknown {
            return Ok(());
        }

        Self::record(
            tx,
            config,
            vec![FilterElement::melt_quote(quote_id, state)?],
        )
        .await
    }
}

#[cfg(test)]
mod tests;
