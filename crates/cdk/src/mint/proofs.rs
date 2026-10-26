use std::collections::HashMap;

use cdk_common::database::mint::Acquired;
use cdk_common::database::{DynMintDatabase, DynMintTransaction};
use cdk_common::mint::ProofsWithState;
use cdk_common::nuts::nut14::HTLCWitness;
use cdk_common::nuts::SpendingConditions;
use cdk_common::state::{self, check_state_transition};
use cdk_common::{Error, State, Witness};

use crate::Mint;

/// The witness a `PENDING` pre-v3 proof shows (NUT-07): an HTLC with a `pubkeys` tag shows its preimage alone.
pub(crate) fn pending_legacy_witness(proof: &cdk_common::Proof) -> Option<Witness> {
    let Some(Witness::HTLCWitness(witness)) = &proof.witness else {
        return None;
    };
    let Ok(SpendingConditions::HTLCConditions { conditions, .. }) =
        SpendingConditions::try_from(&proof.secret)
    else {
        return None;
    };
    conditions?.pubkeys.filter(|keys| !keys.is_empty())?;
    Some(Witness::HTLCWitness(HTLCWitness {
        preimage: witness.preimage.clone(),
        signatures: None,
    }))
}

/// A proof state as check-state and NUT-17 publish it: v3 proofs show their spend record, pre-v3 ones
/// their stored witness once spent and [`pending_legacy_witness`] while pending (NUT-07).
pub(crate) fn published_state(
    y: cdk_common::PublicKey,
    state: State,
    proof: Option<&cdk_common::Proof>,
    record: Option<cdk_common::nuts::nut10::nutroot::SpendRecord>,
) -> cdk_common::ProofState {
    let mut result = cdk_common::ProofState {
        y,
        state,
        witness: None,
        input_digest: None,
    };
    if let Some(record) = record {
        record.apply_to(&mut result);
    } else if let Some(proof) = proof
        .filter(|proof| proof.keyset_id.get_version() != cdk_common::nuts::KeySetVersion::Version02)
    {
        result.witness = match state {
            State::Spent => proof.witness.clone(),
            State::Pending => pending_legacy_witness(proof),
            _ => None,
        };
    }
    result
}

/// [`published_state`] for each of `ys`, `None` where the mint has no state for it.
pub(crate) async fn published_states(
    db: &DynMintDatabase,
    ys: &[cdk_common::PublicKey],
) -> Result<Vec<Option<cdk_common::ProofState>>, Error> {
    let states = db.get_proofs_states(ys).await?;
    if states.len() != ys.len() {
        tracing::error!("Database did not return states for all proofs");
        return Err(Error::UnknownPaymentState);
    }
    let needing_witness: Vec<_> = ys
        .iter()
        .zip(&states)
        .filter(|(_, state)| matches!(state, Some(State::Spent | State::Pending)))
        .map(|(y, _)| *y)
        .collect();
    let proofs: HashMap<_, _> = if needing_witness.is_empty() {
        HashMap::new()
    } else {
        db.get_proofs_by_ys(&needing_witness)
            .await?
            .into_iter()
            .flatten()
            .filter_map(|proof| Some((proof.y().ok()?, proof)))
            .collect()
    };
    let records = db.get_proof_spends(ys).await?;
    Ok(ys
        .iter()
        .zip(states)
        .zip(records)
        .map(|((y, state), record)| {
            state.map(|state| published_state(*y, state, proofs.get(y), record))
        })
        .collect())
}

impl Mint {
    pub(crate) async fn record_nutroot_spends(
        tx: &mut DynMintTransaction,
        proofs: &[cdk_common::Proof],
        transaction: &cdk_common::nuts::nut10::nutroot::Transaction,
    ) -> Result<(), Error> {
        let mut records = vec![];
        for (index, proof) in proofs.iter().enumerate() {
            if proof.keyset_id.get_version() == cdk_common::nuts::KeySetVersion::Version02 {
                let record = cdk_common::nuts::nut10::nutroot::SpendRecord::new(
                    proof,
                    transaction,
                    index,
                    cdk_common::util::unix_time(),
                )
                .map_err(cdk_common::nuts::nut10::Error::from)?;
                records.push((proof.y()?, record));
            }
        }
        tx.set_proof_spends(&records).await?;
        Ok(())
    }

    /// States for `proofs` in `proof_state`, each with the witness NUT-07 lets it show.
    pub(crate) async fn proof_states(
        tx: &mut DynMintTransaction,
        proofs: &[cdk_common::Proof],
        proof_state: State,
    ) -> Result<Vec<cdk_common::ProofState>, Error> {
        let ys = proofs
            .iter()
            .map(|proof| proof.y())
            .collect::<Result<Vec<_>, _>>()?;
        let records = tx.get_proof_spends(&ys).await?;
        Ok(proofs
            .iter()
            .zip(ys)
            .zip(records)
            .map(|((proof, y), record)| published_state(y, proof_state, Some(proof), record))
            .collect())
    }

    /// Updates the state of proofs with validation and error handling.
    ///
    /// This method:
    /// 1. Validates the state transition is allowed via `check_state_transition`
    /// 2. Persists the new state to the database
    /// 3. Updates the `ProofsWithState.state` field on success
    ///
    /// # Errors
    ///
    /// - [`Error::UnexpectedProofState`] if the state transition is invalid
    /// - [`Error::TokenAlreadySpent`] if the database rejects the update (proofs already spent)
    pub async fn update_proofs_state(
        tx: &mut DynMintTransaction,
        proofs: &mut Acquired<ProofsWithState>,
        new_state: State,
    ) -> Result<(), Error> {
        check_state_transition(proofs.state, new_state).map_err(|err| match err {
            state::Error::AlreadySpent => Error::TokenAlreadySpent,
            state::Error::Pending => Error::TokenPending,
            _ => Error::UnexpectedProofState,
        })?;

        tx.update_proofs_state(proofs, new_state)
            .await
            .map_err(|err| match err {
                cdk_common::database::Error::AttemptUpdateSpentProof
                | cdk_common::database::Error::AttemptRemoveSpentProof => Error::TokenAlreadySpent,
                err => err.into(),
            })
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::mint::Operation;
    use cdk_common::nuts::ProofsMethods;
    use cdk_common::{Amount, Error, State};

    use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};
    use crate::Mint;

    /// Test successful transition from Unspent to Pending
    #[tokio::test]
    async fn test_update_proofs_state_unspent_to_pending() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(100)).await.unwrap();
        let ys = proofs.ys().unwrap();

        let db = mint.localstore();

        // Add proofs to the database first
        {
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(
                proofs.clone(),
                None,
                &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        let mut tx = db.begin_transaction().await.unwrap();
        let mut acquired = tx.get_proofs(&ys).await.unwrap();

        assert_eq!(acquired.state, State::Unspent);

        Mint::update_proofs_state(&mut tx, &mut acquired, State::Pending)
            .await
            .unwrap();

        assert_eq!(acquired.state, State::Pending);
        tx.commit().await.unwrap();

        // Verify state persisted to database
        let states = db.get_proofs_states(&ys).await.unwrap();
        assert!(states.iter().all(|s| *s == Some(State::Pending)));
    }

    /// Test successful transition from Pending to Spent
    #[tokio::test]
    async fn test_update_proofs_state_pending_to_spent() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(100)).await.unwrap();
        let ys = proofs.ys().unwrap();

        let db = mint.localstore();

        // Add proofs and transition to Pending
        {
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(
                proofs.clone(),
                None,
                &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        {
            let mut tx = db.begin_transaction().await.unwrap();
            let mut acquired = tx.get_proofs(&ys).await.unwrap();
            Mint::update_proofs_state(&mut tx, &mut acquired, State::Pending)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }

        // Now test Pending -> Spent transition
        let mut tx = db.begin_transaction().await.unwrap();
        let mut acquired = tx.get_proofs(&ys).await.unwrap();

        assert_eq!(acquired.state, State::Pending);

        Mint::update_proofs_state(&mut tx, &mut acquired, State::Spent)
            .await
            .unwrap();

        assert_eq!(acquired.state, State::Spent);
        tx.commit().await.unwrap();

        // Verify state persisted to database
        let states = db.get_proofs_states(&ys).await.unwrap();
        assert!(states.iter().all(|s| *s == Some(State::Spent)));
    }

    /// Test that update_proofs_state rejects same-state transition (Pending -> Pending)
    #[tokio::test]
    async fn test_update_proofs_state_rejects_same_state_transition() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(100)).await.unwrap();
        let ys = proofs.ys().unwrap();

        let db = mint.localstore();

        // Add proofs and transition to Pending
        {
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(
                proofs.clone(),
                None,
                &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        {
            let mut tx = db.begin_transaction().await.unwrap();
            let mut acquired = tx.get_proofs(&ys).await.unwrap();
            Mint::update_proofs_state(&mut tx, &mut acquired, State::Pending)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }

        // Try invalid transition: Pending -> Pending (same state)
        let mut tx = db.begin_transaction().await.unwrap();
        let mut acquired = tx.get_proofs(&ys).await.unwrap();

        assert_eq!(acquired.state, State::Pending);

        let result = Mint::update_proofs_state(&mut tx, &mut acquired, State::Pending).await;

        assert!(matches!(result, Err(Error::TokenPending)));
    }

    /// Test that update_proofs_state rejects invalid transition from Spent
    #[tokio::test]
    async fn test_update_proofs_state_invalid_transition_from_spent() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(100)).await.unwrap();
        let ys = proofs.ys().unwrap();

        let db = mint.localstore();

        // Add proofs and transition to Spent
        {
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(
                proofs.clone(),
                None,
                &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        {
            let mut tx = db.begin_transaction().await.unwrap();
            let mut acquired = tx.get_proofs(&ys).await.unwrap();
            Mint::update_proofs_state(&mut tx, &mut acquired, State::Pending)
                .await
                .unwrap();
            Mint::update_proofs_state(&mut tx, &mut acquired, State::Spent)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }

        // Try invalid transition: Spent -> Pending (not allowed)
        let mut tx = db.begin_transaction().await.unwrap();
        let mut acquired = tx.get_proofs(&ys).await.unwrap();

        assert_eq!(acquired.state, State::Spent);

        let result = Mint::update_proofs_state(&mut tx, &mut acquired, State::Pending).await;

        assert!(matches!(result, Err(Error::TokenAlreadySpent)));
    }

    /// Test that ProofsWithState.state is updated after successful update
    #[tokio::test]
    async fn test_update_proofs_state_updates_wrapper_state() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(100)).await.unwrap();
        let ys = proofs.ys().unwrap();

        let db = mint.localstore();

        // Add proofs to the database first
        {
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(
                proofs.clone(),
                None,
                &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        let mut tx = db.begin_transaction().await.unwrap();
        let mut acquired = tx.get_proofs(&ys).await.unwrap();

        // Before update
        assert_eq!(acquired.state, State::Unspent);

        // After update
        Mint::update_proofs_state(&mut tx, &mut acquired, State::Pending)
            .await
            .unwrap();

        // The wrapper's state field should be updated
        assert_eq!(
            acquired.state,
            State::Pending,
            "ProofsWithState.state should be updated after successful update_proofs_state"
        );

        tx.commit().await.unwrap();
    }

    fn htlc_proof(pubkeys: Option<Vec<cdk_common::PublicKey>>) -> cdk_common::Proof {
        use std::str::FromStr;

        use bitcoin::hashes::sha256::Hash as Sha256Hash;
        use bitcoin::hashes::Hash;
        use cdk_common::nuts::nut14::HTLCWitness;
        use cdk_common::nuts::{Conditions, SpendingConditions};

        let preimage = [42u8; 32];
        let conditions = SpendingConditions::HTLCConditions {
            data: Sha256Hash::hash(&preimage),
            conditions: Some(Conditions {
                pubkeys,
                ..Default::default()
            }),
        };
        cdk_common::Proof {
            amount: Amount::from(1),
            keyset_id: cdk_common::Id::from_str("00deadbeef123456").unwrap(),
            secret: conditions.try_into().unwrap(),
            c: cdk_common::SecretKey::generate().public_key(),
            witness: Some(cdk_common::Witness::HTLCWitness(HTLCWitness {
                preimage: cdk_common::util::hex::encode(preimage),
                signatures: Some(vec!["00".repeat(64)]),
            })),
            dleq: None,
            p2pk_e: None,
            spend_info: None,
        }
    }

    /// NUT-07: a pending HTLC with a pubkeys tag shows its preimage and nothing else.
    #[test]
    fn pending_htlc_with_pubkeys_shows_only_its_preimage() {
        let key = cdk_common::SecretKey::generate().public_key();
        let shown = super::pending_legacy_witness(&htlc_proof(Some(vec![key])));
        assert_eq!(
            shown,
            Some(cdk_common::Witness::HTLCWitness(
                cdk_common::nuts::nut14::HTLCWitness {
                    preimage: cdk_common::util::hex::encode([42u8; 32]),
                    signatures: None,
                }
            ))
        );
        assert_eq!(super::pending_legacy_witness(&htlc_proof(None)), None);
        assert_eq!(
            super::pending_legacy_witness(&htlc_proof(Some(vec![]))),
            None
        );
    }

    /// Check-state and NUT-17 events publish one rule for pre-v3 proofs.
    #[test]
    fn published_state_shows_pre_v3_witness_by_state() {
        let key = cdk_common::SecretKey::generate().public_key();
        let proof = htlc_proof(Some(vec![key]));
        let y = proof.y().unwrap();
        let shown = |state| super::published_state(y, state, Some(&proof), None).witness;
        assert_eq!(shown(State::Unspent), None);
        assert_eq!(shown(State::Pending), super::pending_legacy_witness(&proof));
        assert_eq!(shown(State::Spent), proof.witness);
    }
}
