//! In-memory database that is provided by the `cdk-sqlite` crate, mainly for testing purposes.
use std::collections::HashMap;

use cdk_common::database::{self, MintDatabase, MintKeysDatabase};
use cdk_common::mint::{self, MintKeySetInfo, MintQuote, Operation};
use cdk_common::nuts::{CurrencyUnit, Id, Proofs, State};
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

    let operation = Operation::new_swap(Default::default(), Default::default(), Default::default());

    if !pending_proofs.is_empty() {
        let mut proofs = tx.add_proofs(pending_proofs, None, &operation).await?;
        tx.update_proofs_state(&mut proofs, State::Pending).await?;
    }

    if !spent_proofs.is_empty() {
        let mut proofs = tx.add_proofs(spent_proofs, None, &operation).await?;
        tx.update_proofs_state(&mut proofs, State::Spent).await?;
    }
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::str::FromStr;

    use cdk_common::database::MintProofsDatabase;
    use cdk_common::nuts::{Id, Proof, State};
    use cdk_common::secret::Secret;
    use cdk_common::{Amount, MintInfo, SecretKey};

    use super::new_with_state;

    fn make_proof(keyset_id: Id, amount: u64) -> Proof {
        Proof {
            amount: Amount::from(amount),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        }
    }

    #[tokio::test]
    async fn new_with_state_restores_spent_proofs_as_spent() {
        let keyset_id = Id::from_str("00916bbf7ef91a36").expect("valid keyset id");
        let pending_proofs = vec![make_proof(keyset_id, 1)];
        let spent_proofs = vec![make_proof(keyset_id, 2), make_proof(keyset_id, 4)];
        let spent_ys = spent_proofs
            .iter()
            .map(|proof| proof.y().expect("valid proof y"))
            .collect::<Vec<_>>();

        let db = new_with_state(
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            pending_proofs,
            spent_proofs,
            MintInfo::default(),
        )
        .await
        .expect("valid db");

        let states = db.get_proofs_states(&spent_ys).await.expect("proof states");

        assert_eq!(states, vec![Some(State::Spent), Some(State::Spent)]);
    }

    #[tokio::test]
    async fn new_with_state_restores_pending_proofs_without_spent_proofs() {
        let keyset_id = Id::from_str("00916bbf7ef91a36").expect("valid keyset id");
        let pending_proofs = vec![make_proof(keyset_id, 1), make_proof(keyset_id, 2)];
        let pending_ys = pending_proofs
            .iter()
            .map(|proof| proof.y().expect("valid proof y"))
            .collect::<Vec<_>>();

        let db = new_with_state(
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            pending_proofs,
            vec![],
            MintInfo::default(),
        )
        .await
        .expect("valid db");

        let states = db
            .get_proofs_states(&pending_ys)
            .await
            .expect("proof states");

        assert_eq!(states, vec![Some(State::Pending), Some(State::Pending)]);
    }
}
