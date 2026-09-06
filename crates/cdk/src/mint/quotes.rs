use cdk_common::database::mint::Acquired;
use cdk_common::database::DynMintTransaction;
use cdk_common::mint::{MeltQuote, MintQuote};
use cdk_common::{Error, MeltQuoteState};

use super::state_filters::StateFilters;
use crate::Mint;

impl Mint {
    /// Persists a mint quote and publishes the change to the filters.
    ///
    /// A mint quote element carries no state, because the quote's accounting
    /// fields would disclose amounts. A match means only that the quote
    /// changed, so nothing is published when it did not.
    pub async fn update_mint_quote(
        tx: &mut DynMintTransaction,
        filters: &StateFilters,
        quote: &mut Acquired<MintQuote>,
    ) -> Result<(), Error> {
        let changed = quote.has_changes();
        let quote_id = quote.id.to_string();

        tx.update_mint_quote(quote).await?;

        if changed {
            filters.record_mint_quote(tx, &quote_id).await?;
        }

        Ok(())
    }

    /// Moves a melt quote to a new state and publishes it to the filters.
    ///
    /// Returns the state the quote was in before the update.
    pub async fn update_melt_quote_state(
        tx: &mut DynMintTransaction,
        filters: &StateFilters,
        quote: &mut Acquired<MeltQuote>,
        new_state: MeltQuoteState,
        payment_proof: Option<String>,
    ) -> Result<MeltQuoteState, Error> {
        let quote_id = quote.id.to_string();

        let previous = tx
            .update_melt_quote_state(quote, new_state, payment_proof)
            .await?;

        filters.record_melt_quote(tx, &quote_id, new_state).await?;

        Ok(previous)
    }
}
