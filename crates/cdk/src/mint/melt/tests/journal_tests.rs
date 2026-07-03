//! End-to-end checks that a real mint MELT emits the expected append-only
//! journal events, orchestrated by the mint layer.

use std::str::FromStr;
use std::sync::Arc;

use cdk_common::database::event_log::{Delta, Entity, Event, Snapshot};
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::{
    Amount, Bolt11Invoice, CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MeltRequest,
};

use crate::test_helpers::mint::{create_test_mint_with_db, mint_test_proofs, read_journal};

/// A real melt writes a full melt-quote snapshot at creation, then the
/// Unpaid -> Pending -> Paid state deltas and the payment proof, so the quote's
/// state can be replayed from the journal.
#[tokio::test]
async fn melt_journals_quote_snapshot_and_state_transitions() {
    let file = format!(
        "{}/cdk_journal_melt_test.sqlite",
        std::env::temp_dir().display()
    );
    let _ = std::fs::remove_file(&file);

    let db = Arc::new(
        cdk_sqlite::mint::MintSqliteDatabase::new(file.as_str())
            .await
            .expect("file-backed mint db"),
    );
    let mint = create_test_mint_with_db(db).await.expect("build mint");

    // Fund proofs to spend on the melt (invoice below is 10 sat).
    let input_proofs = mint_test_proofs(&mint, Amount::from(64))
        .await
        .expect("mint proofs");

    // A fake invoice the FakeWallet backend settles as Paid.
    let bolt11 = Bolt11Invoice::from_str("lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq").expect("invoice");

    let melt_quote = mint
        .get_melt_quote(MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: bolt11,
            unit: CurrencyUnit::Sat,
            options: None,
        }))
        .await
        .expect("melt quote");
    let quote_id = melt_quote.quote().expect("quote id").clone();

    let melt_request = MeltRequest::new(quote_id.clone(), input_proofs, None);
    mint.melt(&melt_request)
        .await
        .expect("melt started")
        .await
        .expect("melt finished");

    let events = read_journal(&file);
    let record = quote_id.to_string();
    let for_record: Vec<&Event> = events
        .iter()
        .filter(|(entity, r, _)| *entity == Entity::MeltQuote && *r == record)
        .map(|(_, _, e)| e)
        .collect();

    // Creation snapshot first, then the state transitions.
    assert!(
        matches!(
            for_record.first(),
            Some(Event::Snapshot(s)) if matches!(s.as_ref(), Snapshot::MeltQuote(_))
        ),
        "first melt_quote event must be a snapshot, got {for_record:?}"
    );
    assert!(
        for_record.iter().any(|e| matches!(
            e,
            Event::Delta(Delta::MeltQuoteState(MeltQuoteState::Pending))
        )),
        "missing Pending state delta for {record}"
    );
    assert!(
        for_record
            .iter()
            .any(|e| matches!(e, Event::Delta(Delta::MeltQuoteState(MeltQuoteState::Paid)))),
        "missing Paid state delta for {record}"
    );
    assert!(
        for_record
            .iter()
            .any(|e| matches!(e, Event::Delta(Delta::MeltQuotePaymentProof(_)))),
        "missing payment proof delta for {record}"
    );

    // Replay: snapshot's initial state plus ordered state deltas => Paid.
    let mut replayed = match for_record.first() {
        Some(Event::Snapshot(s)) => match s.as_ref() {
            Snapshot::MeltQuote(q) => q.state,
            other => panic!("first event must be a melt quote snapshot, got {other:?}"),
        },
        other => panic!("first event must be a snapshot, got {other:?}"),
    };
    for event in &for_record {
        if let Event::Delta(Delta::MeltQuoteState(state)) = event {
            replayed = *state;
        }
    }
    assert_eq!(replayed, MeltQuoteState::Paid);

    let _ = std::fs::remove_file(&file);
}
