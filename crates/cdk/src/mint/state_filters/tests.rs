use cdk_common::amount::SplitTarget;
use cdk_common::database::mint::StateFilterConfig;
use cdk_common::nuts::{
    CurrencyUnit, DecodedFilter, FilterElement, FilterKind, PreMintSecrets, ProofsMethods,
    SwapRequest,
};
use cdk_common::util::unix_time;
use cdk_common::{Amount, MeltQuoteState, State};

use super::{StateFilterOptions, StateFilterService};
use crate::test_helpers::mint::{create_test_mint_with_state_filters, mint_test_proofs};
use crate::Mint;

fn options() -> StateFilterOptions {
    StateFilterOptions {
        epoch_seconds: 3600,
        p: 28,
        page_size: 4,
        kinds: vec![
            FilterKind::ProofState,
            FilterKind::MintQuote,
            FilterKind::MeltQuote,
        ],
        pending: true,
    }
}

async fn filter_mint() -> Mint {
    create_test_mint_with_state_filters(Some(options()))
        .await
        .expect("mint with filters")
}

/// The open epoch's filter, which reflects every element captured so far.
async fn pending(mint: &Mint) -> DecodedFilter {
    let service = mint.state_filter_service().expect("filters enabled");
    let response = service.pending().await.expect("pending filter");
    response.decode(options().p).expect("decodes")
}

async fn swap(mint: &Mint, amount: Amount) -> cdk_common::Proofs {
    let proofs = mint_test_proofs(mint, amount).await.expect("proofs");

    let keyset_id = *mint
        .get_active_keysets()
        .get(&CurrencyUnit::Sat)
        .expect("active keyset");
    let keys = mint
        .keyset_pubkeys(&keyset_id)
        .expect("keys")
        .keysets
        .first()
        .expect("keyset")
        .keys
        .clone();
    let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect());

    let premint = PreMintSecrets::random(keyset_id, amount, &SplitTarget::None, &fees.into())
        .expect("premint");

    mint.process_swap_request(SwapRequest::new(proofs.clone(), premint.blinded_messages()))
        .await
        .expect("swap");

    proofs
}

#[tokio::test]
async fn a_swap_publishes_the_spent_inputs() {
    let mint = filter_mint().await;
    let spent = swap(&mint, Amount::from(64)).await;

    let filter = pending(&mint).await;

    for y in spent.ys().expect("ys") {
        let element = FilterElement::proof_state(&y, State::Spent).expect("supported");
        assert!(
            filter.contains(&element),
            "spent proof {y} is missing from the open epoch"
        );
    }
}

#[tokio::test]
async fn a_swap_publishes_the_pending_transition_too() {
    let mint = filter_mint().await;
    let spent = swap(&mint, Amount::from(64)).await;

    let filter = pending(&mint).await;

    for y in spent.ys().expect("ys") {
        let element = FilterElement::proof_state(&y, State::Pending).expect("supported");
        assert!(
            filter.contains(&element),
            "the pending transition of {y} is missing"
        );
    }
}

#[tokio::test]
async fn an_untouched_proof_is_absent() {
    let mint = filter_mint().await;
    swap(&mint, Amount::from(64)).await;

    let untouched = mint_test_proofs(&mint, Amount::from(16))
        .await
        .expect("proofs");

    let filter = pending(&mint).await;

    for y in untouched.ys().expect("ys") {
        let element = FilterElement::proof_state(&y, State::Spent).expect("supported");
        assert!(!filter.contains(&element), "unspent proof {y} matched");
    }
}

#[tokio::test]
async fn a_mint_quote_payment_is_published() {
    let mint = filter_mint().await;

    // Minting drives the quote from unpaid through paid and issued.
    mint_test_proofs(&mint, Amount::from(32))
        .await
        .expect("proofs");

    let quotes = mint.localstore().get_mint_quotes().await.expect("quotes");
    let quote = quotes.first().expect("one quote");

    let filter = pending(&mint).await;
    assert!(
        filter.contains(&FilterElement::mint_quote(&quote.id.to_string())),
        "mint quote {} is missing from the open epoch",
        quote.id
    );
}

#[tokio::test]
async fn a_mint_that_does_not_publish_records_nothing() {
    let mint = create_test_mint_with_state_filters(None)
        .await
        .expect("mint");

    assert!(mint.state_filter_service().is_none());
    assert!(!mint.state_filters().enabled());

    swap(&mint, Amount::from(64)).await;

    let elements = mint
        .localstore()
        .get_filter_elements(0)
        .await
        .expect("elements");
    assert!(elements.is_empty(), "a disabled mint captured elements");
}

#[tokio::test]
async fn only_advertised_kinds_are_captured() {
    let mut options = options();
    options.kinds = vec![FilterKind::MeltQuote];

    let mint = create_test_mint_with_state_filters(Some(options))
        .await
        .expect("mint");

    let spent = swap(&mint, Amount::from(64)).await;

    let filter = pending(&mint).await;
    for y in spent.ys().expect("ys") {
        let element = FilterElement::proof_state(&y, State::Spent).expect("supported");
        assert!(
            !filter.contains(&element),
            "proof states were captured by a mint that does not advertise them"
        );
    }
}

/// A mint that was down for several epochs still publishes a gapless history.
#[tokio::test]
async fn downtime_leaves_no_gap_in_the_history() {
    let db: cdk_common::database::DynMintDatabase =
        std::sync::Arc::new(cdk_sqlite::mint::memory::empty().await.expect("db"));
    let epoch_seconds = 3600;
    let now = unix_time();

    let genesis = now - (now % epoch_seconds) - epoch_seconds * 10;
    {
        let mut tx = db.begin_transaction().await.expect("tx");
        tx.set_state_filter_config(&StateFilterConfig {
            genesis,
            epoch_seconds,
            p: 28,
            page_size: 4,
        })
        .await
        .expect("config");
        tx.commit().await.expect("commit");
    }

    let service = StateFilterService::new(db.clone(), epoch_seconds, 28, 4, vec![], true)
        .await
        .expect("service");

    let built = service.build_due_epochs().await.expect("build");
    assert_eq!(built, 10, "every closed epoch is built");

    let info = service.info().await.expect("info");
    assert_eq!(info.first_page, 0);
    assert_eq!(info.current_page, 2, "ten filters at four per page");
    assert_eq!(info.current_page_count, 2);
    assert_eq!(info.earliest_start, genesis);
    assert_eq!(info.latest_end, genesis + epoch_seconds * 10);

    let mut expected_start = genesis;
    for page in 0..=info.current_page {
        for filter in service.page(page).await.expect("page").filters {
            assert_eq!(filter.start, expected_start, "epochs must be contiguous");
            assert_eq!(filter.end, expected_start + epoch_seconds);
            assert!(filter.data.is_empty(), "an empty epoch has an empty filter");
            expected_start = filter.end;
        }
    }
    assert_eq!(expected_start, info.latest_end);
}

#[tokio::test]
async fn a_page_above_the_current_one_is_rejected() {
    let mint = filter_mint().await;
    let service = mint.state_filter_service().expect("filters enabled");

    let info = service.info().await.expect("info");
    assert!(service.page(info.current_page).await.is_ok());

    assert!(matches!(
        service.page(info.current_page + 1).await,
        Err(crate::Error::FilterPageOutOfRange)
    ));
}

#[tokio::test]
async fn a_complete_page_is_immutable_but_the_current_one_is_not() {
    let db: cdk_common::database::DynMintDatabase =
        std::sync::Arc::new(cdk_sqlite::mint::memory::empty().await.expect("db"));
    let epoch_seconds = 3600;
    let now = unix_time();
    let genesis = now - (now % epoch_seconds) - epoch_seconds * 6;

    {
        let mut tx = db.begin_transaction().await.expect("tx");
        tx.set_state_filter_config(&StateFilterConfig {
            genesis,
            epoch_seconds,
            p: 28,
            page_size: 4,
        })
        .await
        .expect("config");
        tx.commit().await.expect("commit");
    }

    let service = StateFilterService::new(db, epoch_seconds, 28, 4, vec![], true)
        .await
        .expect("service");
    service.build_due_epochs().await.expect("build");

    assert!(service.page_is_complete(0).await.expect("page 0"));
    assert!(!service.page_is_complete(1).await.expect("page 1"));
}

#[tokio::test]
async fn changing_the_parameters_is_refused() {
    let db: cdk_common::database::DynMintDatabase =
        std::sync::Arc::new(cdk_sqlite::mint::memory::empty().await.expect("db"));

    StateFilterService::new(db.clone(), 3600, 28, 50, vec![], true)
        .await
        .expect("first run");

    for (epoch, p, page_size) in [(1800, 28, 50), (3600, 24, 50), (3600, 28, 25)] {
        let result = StateFilterService::new(db.clone(), epoch, p, page_size, vec![], true).await;
        assert!(
            matches!(result, Err(crate::Error::StateFilterConfig(_))),
            "changing to (epoch {epoch}, p {p}, page size {page_size}) should be refused"
        );
    }

    assert!(StateFilterService::new(db, 3600, 28, 50, vec![], true)
        .await
        .is_ok());
}

#[tokio::test]
async fn a_failed_melt_quote_publishes_unpaid() {
    let element = FilterElement::melt_quote("quote-id", MeltQuoteState::Failed).expect("supported");

    assert_eq!(
        element,
        FilterElement::melt_quote("quote-id", MeltQuoteState::Unpaid).expect("supported")
    );
}
