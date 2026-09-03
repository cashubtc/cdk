//! Following mint state through compact state filters
//!
//! Testing a candidate against a filter is local, so the wallet learns "no"
//! without telling the mint anything. A match is only probably real, so it is
//! always confirmed through the identifying endpoint before anything acts on
//! it.

use cdk_common::nuts::state_filters::{
    DecodedFilter, FilterElement, FilterKind, GetFiltersInfoResponse,
};
use cdk_common::nuts::{ProofState, Proofs, ProofsMethods};
use cdk_common::{Error, MeltQuoteState, PublicKey, State};
use tracing::instrument;

use crate::Wallet;

mod cursor;

pub use cursor::Cursor;

/// What a sweep found, before confirmation.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Matches {
    /// Proofs whose `SPENT` or `PENDING` element matched
    pub proofs: Vec<PublicKey>,
    /// Mint quotes that changed
    pub mint_quotes: Vec<String>,
    /// Melt quotes that changed
    pub melt_quotes: Vec<String>,
}

impl Matches {
    /// Whether anything matched.
    pub fn is_empty(&self) -> bool {
        self.proofs.is_empty() && self.mint_quotes.is_empty() && self.melt_quotes.is_empty()
    }
}

/// The elements a wallet is watching for, built once per sweep.
///
/// Only the position of an element depends on the filter it is tested against,
/// so the digests are computed once and reused across the whole history.
#[derive(Debug, Default)]
struct Candidates {
    proofs: Vec<(FilterElement, PublicKey)>,
    mint_quotes: Vec<(FilterElement, String)>,
    melt_quotes: Vec<(FilterElement, String)>,
}

impl Candidates {
    fn is_empty(&self) -> bool {
        self.proofs.is_empty() && self.mint_quotes.is_empty() && self.melt_quotes.is_empty()
    }

    fn test(&self, filter: &DecodedFilter, found: &mut Matches) {
        for (element, y) in &self.proofs {
            if filter.contains(element) && !found.proofs.contains(y) {
                found.proofs.push(*y);
            }
        }
        for (element, id) in &self.mint_quotes {
            if filter.contains(element) && !found.mint_quotes.contains(id) {
                found.mint_quotes.push(id.clone());
            }
        }
        for (element, id) in &self.melt_quotes {
            if filter.contains(element) && !found.melt_quotes.contains(id) {
                found.melt_quotes.push(id.clone());
            }
        }
    }
}

impl Wallet {
    /// The mint's filter parameters, if it publishes filters.
    pub async fn state_filters_info(&self) -> Result<Option<GetFiltersInfoResponse>, Error> {
        let advertised = self
            .fetch_mint_info()
            .await?
            .is_some_and(|info| info.nuts.state_filters.supported);

        if !advertised {
            return Ok(None);
        }

        match self.client.get_filters_info().await {
            Ok(info) => Ok(Some(info)),
            Err(Error::FilterNotAvailable) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Build the elements this wallet is watching for.
    async fn state_filter_candidates(&self, kinds: &[FilterKind]) -> Result<Candidates, Error> {
        let mut candidates = Candidates::default();

        if kinds.contains(&FilterKind::ProofState) {
            let proofs = self
                .localstore
                .get_proofs(
                    Some(self.mint_url.clone()),
                    Some(self.unit.clone()),
                    Some(vec![
                        State::Unspent,
                        State::Pending,
                        State::Reserved,
                        State::PendingSpent,
                    ]),
                    None,
                )
                .await?;

            for proof in proofs {
                for state in [State::Spent, State::Pending] {
                    candidates
                        .proofs
                        .push((FilterElement::proof_state(&proof.y, state)?, proof.y));
                }
            }
        }

        if kinds.contains(&FilterKind::MintQuote) {
            for quote in self.localstore.get_unissued_mint_quotes().await? {
                if quote.mint_url != self.mint_url {
                    continue;
                }
                candidates
                    .mint_quotes
                    .push((FilterElement::mint_quote(&quote.id), quote.id));
            }
        }

        if kinds.contains(&FilterKind::MeltQuote) {
            for quote in self.localstore.get_melt_quotes().await? {
                if quote.mint_url.as_ref() != Some(&self.mint_url) {
                    continue;
                }
                if matches!(quote.state, MeltQuoteState::Paid) {
                    continue;
                }
                for state in [MeltQuoteState::Paid, MeltQuoteState::Pending] {
                    candidates.melt_quotes.push((
                        FilterElement::melt_quote(&quote.id, state)?,
                        quote.id.clone(),
                    ));
                }
            }
        }

        Ok(candidates)
    }

    /// Fetch new filters and test this wallet's objects against them.
    ///
    /// Resumes from the stored cursor, so a wallet that keeps up pays for one
    /// page per epoch and skips the epochs it already tested when the current
    /// page is fetched again. Returns what matched, without confirming it.
    #[instrument(skip(self))]
    pub async fn sweep_state_filters(&self) -> Result<Matches, Error> {
        let Some(info) = self.state_filters_info().await? else {
            return Ok(Matches::default());
        };

        let candidates = self.state_filter_candidates(&info.kinds).await?;
        if candidates.is_empty() {
            return Ok(Matches::default());
        }

        let cursor = self.state_filter_cursor().await?;
        let mut found = Matches::default();

        let first = cursor.next_page.max(info.first_page);
        for page in first..=info.current_page {
            let response = self.client.get_filters(page).await?;

            for filter in response.filters {
                if filter.start < cursor.next_start {
                    continue;
                }
                candidates.test(&filter.decode(info.p)?, &mut found);
            }
        }

        self.advance_state_filter_cursor(&info).await?;

        Ok(found)
    }

    /// Test this wallet's objects against the filter of the open epoch.
    ///
    /// The pending filter is rebuilt for every request and its `n` changes with
    /// it, so nothing about it can be cached.
    #[instrument(skip(self))]
    pub async fn sweep_pending_state_filter(&self) -> Result<Matches, Error> {
        let Some(info) = self.state_filters_info().await? else {
            return Ok(Matches::default());
        };
        if !info.pending {
            return Ok(Matches::default());
        }

        let candidates = self.state_filter_candidates(&info.kinds).await?;
        if candidates.is_empty() {
            return Ok(Matches::default());
        }

        let pending = match self.client.get_filters_pending().await {
            Ok(pending) => pending,
            Err(Error::FilterNotAvailable) => return Ok(Matches::default()),
            Err(err) => return Err(err),
        };

        let mut found = Matches::default();
        candidates.test(&pending.decode(info.p)?, &mut found);

        Ok(found)
    }

    /// Confirm matches through the identifying endpoints and apply them.
    ///
    /// A filter is never authoritative: a match may be spurious, so nothing
    /// irreversible happens until the mint has answered for the named object.
    #[instrument(skip(self, matches))]
    pub async fn confirm_state_filter_matches(&self, matches: &Matches) -> Result<(), Error> {
        if !matches.proofs.is_empty() {
            let proofs = self
                .localstore
                .get_proofs_by_ys(matches.proofs.clone())
                .await?
                .into_iter()
                .map(|info| info.proof)
                .collect::<Vec<_>>();

            if !proofs.is_empty() {
                self.check_proofs_spent(proofs).await?;
            }
        }

        for quote_id in &matches.mint_quotes {
            if let Some(mut quote) = self.localstore.get_mint_quote(quote_id).await? {
                if let Err(err) = self.check_mint_quote_state(&mut quote).await {
                    tracing::warn!("Could not confirm mint quote {quote_id}: {err}");
                }
            }
        }

        for quote_id in &matches.melt_quotes {
            if let Err(err) = self.check_melt_quote_status(quote_id).await {
                tracing::warn!("Could not confirm melt quote {quote_id}: {err}");
            }
        }

        Ok(())
    }

    /// Which of these proofs the mint's whole published history mentions.
    ///
    /// Every closed page is swept and then the open epoch, because a spend that
    /// just happened lives only in the pending filter.
    ///
    /// Returns `None` when filters cannot answer the question: absence of a
    /// match means unspent only if the wallet covers every epoch the proof
    /// could have been spent in, so a mint that does not publish filters, does
    /// not cover proof states, has pruned its history, or withholds the open
    /// epoch all fall back to NUT-07.
    pub async fn state_filter_proof_candidates(
        &self,
        ys: &[PublicKey],
    ) -> Result<Option<Vec<PublicKey>>, Error> {
        let Some(info) = self.state_filters_info().await? else {
            return Ok(None);
        };

        if !info.kinds.contains(&FilterKind::ProofState) || info.first_page > 0 || !info.pending {
            return Ok(None);
        }

        let mut elements = Vec::with_capacity(ys.len() * 2);
        for y in ys {
            for state in [State::Spent, State::Pending] {
                elements.push((FilterElement::proof_state(y, state)?, *y));
            }
        }

        let mut matched: Vec<PublicKey> = Vec::new();
        let test = |decoded: &DecodedFilter, matched: &mut Vec<PublicKey>| {
            for (element, y) in &elements {
                if decoded.contains(element) && !matched.contains(y) {
                    matched.push(*y);
                }
            }
        };

        for page in info.first_page..=info.current_page {
            for filter in self.client.get_filters(page).await?.filters {
                test(&filter.decode(info.p)?, &mut matched);
            }
        }

        let pending = self.client.get_filters_pending().await?;
        test(&pending.decode(info.p)?, &mut matched);

        Ok(Some(matched))
    }

    /// The state of each proof, learned through filters when the mint offers
    /// them and through NUT-07 otherwise.
    ///
    /// This is what a seed restore uses instead of sending every recovered `Y`
    /// to the mint. Only the proofs that matched a filter are ever named, and
    /// on a fresh seed with nothing spent that is none of them.
    pub(crate) async fn proof_states_preferring_filters(
        &self,
        proofs: &Proofs,
    ) -> Result<Vec<ProofState>, Error> {
        let ys = proofs.ys()?;

        let Some(matched) = self.state_filter_proof_candidates(&ys).await? else {
            return self.check_proofs_spent(proofs.clone()).await;
        };

        if matched.is_empty() {
            return Ok(ys
                .into_iter()
                .map(|y| ProofState::from((y, State::Unspent)))
                .collect());
        }

        let named: Proofs = proofs
            .iter()
            .zip(ys.iter())
            .filter(|(_, y)| matched.contains(y))
            .map(|(proof, _)| proof.clone())
            .collect();

        tracing::debug!(
            "Filters narrowed {} restored proofs down to {} worth naming",
            ys.len(),
            named.len()
        );

        let confirmed = self.check_proofs_spent(named).await?;

        Ok(ys
            .into_iter()
            .map(|y| {
                confirmed
                    .iter()
                    .find(|state| state.y == y)
                    .cloned()
                    .unwrap_or_else(|| ProofState::from((y, State::Unspent)))
            })
            .collect())
    }

    /// Which mint quotes the mint's published history says changed.
    ///
    /// Only the quotes that matched are worth naming, so a wallet whose quotes
    /// are all quiet names none of them. Returns `None` when filters cannot
    /// answer, so callers fall back to checking every quote.
    pub async fn state_filter_quote_candidates(&self) -> Result<Option<Vec<String>>, Error> {
        let Some(info) = self.state_filters_info().await? else {
            return Ok(None);
        };

        if !info.kinds.contains(&FilterKind::MintQuote) || info.first_page > 0 || !info.pending {
            return Ok(None);
        }

        let quotes = self.localstore.get_unissued_mint_quotes().await?;
        let elements: Vec<(FilterElement, String)> = quotes
            .into_iter()
            .filter(|quote| quote.mint_url == self.mint_url)
            .map(|quote| (FilterElement::mint_quote(&quote.id), quote.id))
            .collect();

        let mut matched: Vec<String> = Vec::new();
        for page in info.first_page..=info.current_page {
            for filter in self.client.get_filters(page).await?.filters {
                let decoded = filter.decode(info.p)?;
                for (element, id) in &elements {
                    if decoded.contains(element) && !matched.contains(id) {
                        matched.push(id.clone());
                    }
                }
            }
        }

        let pending = self.client.get_filters_pending().await?;
        let decoded = pending.decode(info.p)?;
        for (element, id) in &elements {
            if decoded.contains(element) && !matched.contains(id) {
                matched.push(id.clone());
            }
        }

        Ok(Some(matched))
    }

    /// Sweep the published filters and confirm anything that matched.
    ///
    /// This is the whole loop: almost every sweep confirms nothing and so tells
    /// the mint nothing.
    #[instrument(skip(self))]
    pub async fn sync_state_filters(&self) -> Result<Matches, Error> {
        let matches = self.sweep_state_filters().await?;
        self.confirm_state_filter_matches(&matches).await?;
        Ok(matches)
    }
}
