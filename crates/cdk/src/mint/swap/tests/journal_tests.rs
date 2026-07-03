//! End-to-end checks that a real mint SWAP emits the expected append-only
//! journal events, orchestrated by the mint layer.

use std::sync::Arc;

use cdk_common::database::event_log::{Delta, Entity, Event, Snapshot};
use cdk_common::nuts::{State, SwapRequest};
use cdk_common::Amount;

use crate::test_helpers::mint::{
    create_test_blinded_messages, create_test_mint_with_db, mint_test_proofs, read_journal,
};

/// A real swap writes, for each input proof, a creation snapshot followed by the
/// Pending (setup) and Spent (finalize) state deltas.
#[tokio::test]
async fn swap_journals_proof_snapshot_and_state_transitions() {
    let file = format!(
        "{}/cdk_journal_swap_test.sqlite",
        std::env::temp_dir().display()
    );
    let _ = std::fs::remove_file(&file);

    let db = Arc::new(
        cdk_sqlite::mint::MintSqliteDatabase::new(file.as_str())
            .await
            .expect("file-backed mint db"),
    );
    let mint = create_test_mint_with_db(db).await.expect("build mint");

    let amount = Amount::from(64);
    let input_proofs = mint_test_proofs(&mint, amount).await.expect("mint proofs");
    let (outputs, _pre) = create_test_blinded_messages(&mint, amount)
        .await
        .expect("blinded outputs");

    mint.process_swap_request(SwapRequest::new(input_proofs.clone(), outputs))
        .await
        .expect("swap");

    let events = read_journal(&file);

    for proof in &input_proofs {
        let record = proof.y().expect("proof y").to_hex();
        let for_record: Vec<&Event> = events
            .iter()
            .filter(|(entity, r, _)| *entity == Entity::Proof && *r == record)
            .map(|(_, _, e)| e)
            .collect();

        assert!(
            matches!(
                for_record.first(),
                Some(Event::Snapshot(s)) if matches!(s.as_ref(), Snapshot::Proof(_))
            ),
            "first journal event for {record} must be a proof snapshot, got {for_record:?}"
        );
        assert!(
            for_record
                .iter()
                .any(|e| matches!(e, Event::Delta(Delta::ProofState(State::Pending)))),
            "missing Pending state delta for {record}"
        );
        assert!(
            for_record
                .iter()
                .any(|e| matches!(e, Event::Delta(Delta::ProofState(State::Spent)))),
            "missing Spent state delta for {record}"
        );
    }

    let _ = std::fs::remove_file(&file);
}
