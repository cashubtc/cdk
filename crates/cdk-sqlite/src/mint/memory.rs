//! In-memory database that is provided by the `cdk-sqlite` crate, mainly for testing purposes.
use std::collections::HashMap;

use cdk_common::common::PaymentProcessorKey;
use cdk_common::database::{
    self, MintDatabase, MintKeysDatabase, MintProofsDatabase, MintQuotesDatabase,
};
use cdk_common::mint::{self, MintKeySetInfo, MintQuote};
use cdk_common::nuts::{CurrencyUnit, Id, MeltBolt11Request, Proofs};
use cdk_common::MintInfo;
use uuid::Uuid;

use super::MintSqliteDatabase;

/// Creates a new in-memory [`MintSqliteDatabase`] instance
pub async fn empty() -> Result<MintSqliteDatabase, database::Error> {
    #[cfg(not(feature = "sqlcipher"))]
    let db = MintSqliteDatabase::new(":memory:").await?;
    #[cfg(feature = "sqlcipher")]
    let db = MintSqliteDatabase::new(":memory:", "memory".to_string()).await?;
    db.migrate().await;
    Ok(db)
}

/// Creates a new in-memory [`MintSqliteDatabase`] instance with the given state
#[allow(clippy::too_many_arguments)]
pub async fn new_with_state(
    active_keysets: HashMap<CurrencyUnit, Id>,
    keysets: Vec<MintKeySetInfo>,
    mint_quotes: Vec<MintQuote>,
    melt_quotes: Vec<mint::MeltQuote>,
    pending_proofs: Proofs,
    spent_proofs: Proofs,
    melt_request: Vec<(MeltBolt11Request<Uuid>, PaymentProcessorKey)>,
    mint_info: MintInfo,
) -> Result<MintSqliteDatabase, database::Error> {
    let db = empty().await?;

    for active_keyset in active_keysets {
        db.set_active_keyset(active_keyset.0, active_keyset.1)
            .await?;
    }

    for keyset in keysets {
        db.add_keyset_info(keyset).await?;
    }

    for quote in mint_quotes {
        db.add_mint_quote(quote).await?;
    }

    for quote in melt_quotes {
        db.add_melt_quote(quote).await?;
    }

    db.add_proofs(pending_proofs, None).await?;
    db.add_proofs(spent_proofs, None).await?;

    for (melt_request, ln_key) in melt_request {
        db.add_melt_request(melt_request, ln_key).await?;
    }

    db.set_mint_info(mint_info).await?;

    Ok(db)
}
