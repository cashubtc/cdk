use cdk_common::database::{Acquired, DynMintTransaction};
use cdk_common::mint::ProofsWithState;
use cdk_common::state::{self, check_state_transition};
use cdk_common::{Error, State};

use crate::Mint;

impl Mint {
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
}
