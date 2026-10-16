use super::*;

// Implements only the original transaction interface, as an external backend would.
struct LegacyTransaction {
    updates: usize,
}

#[async_trait]
impl ProofsTransaction for LegacyTransaction {
    type Err = Error;

    async fn update_proofs_state(
        &mut self,
        proofs: &mut Acquired<ProofsWithState>,
        new_state: State,
    ) -> Result<(), Error> {
        self.updates += 1;
        proofs.state = new_state;
        Ok(())
    }

    async fn add_proofs(
        &mut self,
        _proof: Proofs,
        _quote_id: Option<QuoteId>,
        _operation: &Operation,
    ) -> Result<Acquired<ProofsWithState>, Error> {
        unimplemented!()
    }

    async fn get_proofs(&mut self, _ys: &[PublicKey]) -> Result<Acquired<ProofsWithState>, Error> {
        unimplemented!()
    }

    async fn remove_proofs(
        &mut self,
        _ys: &[PublicKey],
        _quote_id: Option<QuoteId>,
    ) -> Result<(), Error> {
        unimplemented!()
    }

    async fn get_proof_ys_by_quote_id(
        &mut self,
        _quote_id: &QuoteId,
    ) -> Result<Vec<PublicKey>, Error> {
        unimplemented!()
    }

    async fn get_proof_ys_by_operation_id(
        &mut self,
        _operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Error> {
        unimplemented!()
    }
}

// Older proof readers need no new methods for ordinary swaps or NUT-07.
struct LegacyProofsDatabase;

#[async_trait]
impl ProofsDatabase for LegacyProofsDatabase {
    type Err = Error;

    async fn get_proofs_by_ys(&self, _ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Error> {
        unimplemented!()
    }

    async fn get_proof_ys_by_quote_id(&self, _quote_id: &QuoteId) -> Result<Vec<PublicKey>, Error> {
        unimplemented!()
    }

    async fn get_proofs_states(&self, _ys: &[PublicKey]) -> Result<Vec<Option<State>>, Error> {
        unimplemented!()
    }

    async fn get_proofs_by_keyset_id(
        &self,
        _keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Error> {
        unimplemented!()
    }

    async fn get_total_redeemed(&self) -> Result<HashMap<Id, Amount>, Error> {
        unimplemented!()
    }

    async fn get_proof_ys_by_operation_id(
        &self,
        _operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Error> {
        unimplemented!()
    }
}

#[tokio::test]
async fn legacy_backend_does_not_advertise_unpoisonable_swaps() {
    let db = LegacyProofsDatabase;
    assert!(!db.supports_unpoisonable_swaps());
    assert!(db.get_unpoisonable_r(&[]).await.unwrap().is_empty());
}

#[tokio::test]
async fn legacy_backend_delegates_ordinary_updates_and_rejects_r_without_mutation() {
    let mut tx = LegacyTransaction { updates: 0 };
    let mut proofs = ProofsWithState::new(vec![], State::Pending).into();
    assert!(tx
        .update_proofs_state_with_r(&mut proofs, State::Spent, Some([42; 32]))
        .await
        .is_err());
    assert_eq!(tx.updates, 0);
    assert_eq!(proofs.state, State::Pending);

    tx.update_proofs_state_with_r(&mut proofs, State::Spent, None)
        .await
        .unwrap();
    assert_eq!(tx.updates, 1);
    assert_eq!(proofs.state, State::Spent);
}
