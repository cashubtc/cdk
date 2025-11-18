//! In-memory database that is provided by the `cdk-sqlite` crate, mainly for testing purposes.
use std::collections::HashMap;

use cdk_common::database::{self, MintDatabase, MintKeysDatabase};
use cdk_common::mint::{self, MintKeySetInfo, MintQuote, Operation};
use cdk_common::nuts::{CurrencyUnit, Id, Proofs};
use cdk_common::MintInfo;

use super::MintSqliteDatabase;

const CDK_MINT_PRIMARY_NAMESPACE: &str = "cdk_mint";
const CDK_MINT_CONFIG_SECONDARY_NAMESPACE: &str = "config";
const CDK_MINT_CONFIG_KV_KEY: &str = "mint_info";

/// Creates a new in-memory [`MintSqliteDatabase`] instance
pub async fn empty() -> Result<MintSqliteDatabase, database::Error> {
    #[cfg(not(feature = "sqlcipher"))]
    let path = ":memory:";
    #[cfg(feature = "sqlcipher")]
    let path = (":memory:", "memory");

    MintSqliteDatabase::new(path).await
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
    mint_info: MintInfo,
) -> Result<MintSqliteDatabase, database::Error> {
    let db = empty().await?;
    let mut tx = MintKeysDatabase::begin_transaction(&db).await?;

    for active_keyset in active_keysets {
        tx.set_active_keyset(active_keyset.0, active_keyset.1)
            .await?;
    }

    for keyset in keysets {
        tx.add_keyset_info(keyset).await?;
    }
    tx.commit().await?;

    let mut tx = MintDatabase::begin_transaction(&db).await?;

    for quote in mint_quotes {
        tx.add_mint_quote(quote).await?;
    }

    for quote in melt_quotes {
        tx.add_melt_quote(quote).await?;
    }

    tx.add_proofs(pending_proofs, None, &Operation::new_swap())
        .await?;
    tx.add_proofs(spent_proofs, None, &Operation::new_swap())
        .await?;
    let mint_info_bytes = serde_json::to_vec(&mint_info)?;
    tx.kv_write(
        CDK_MINT_PRIMARY_NAMESPACE,
        CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
        CDK_MINT_CONFIG_KV_KEY,
        &mint_info_bytes,
    )
    .await?;
    tx.commit().await?;

    Ok(db)
}
