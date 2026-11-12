//! NUT-XX transaction tests against a v3 test mint with the fake payment backend.

use std::str::FromStr;
use std::time::Duration;

use cdk_common::amount::SplitTarget;
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::mint::{MeltQuote, TransactionRecord};
use cdk_common::nuts::nut10::nutroot::{self, Transaction, Witness as NutrootWitness};
use cdk_common::nuts::{
    CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteBolt11Request, MintQuoteState,
    MintRequest, PreMintSecrets, SecretKey, State, TransactionMeltOutput, TransactionQuoteInput,
    TransactionRequest, TransactionState, Witness,
};
use cdk_common::{Amount, BlindedMessage, Proofs, ProofsMethods, QuoteId};
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};

use super::{settled_melt_state, settlement_change};
use crate::mint::{Mint, MintInput};
use crate::test_helpers::mint::{
    create_test_blinded_messages, create_test_mint_with_version, mint_test_proofs,
};
use crate::Error;

async fn v3_mint() -> Mint {
    create_test_mint_with_version(None).await.unwrap()
}

/// A paid mint quote locked to a fresh key.
async fn paid_locked_quote(mint: &Mint, amount: u64) -> (QuoteId, SecretKey) {
    let key = SecretKey::generate();
    let quote = mint
        .get_mint_quote(
            MintQuoteBolt11Request {
                amount: amount.into(),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: Some(key.public_key()),
            }
            .into(),
        )
        .await
        .unwrap();
    let id = QuoteId::from_str(&quote.quote().to_string()).unwrap();
    for _ in 0..30 {
        let state = mint
            .check_mint_quotes(std::slice::from_ref(&id))
            .await
            .unwrap()[0]
            .state();
        if state == Some(MintQuoteState::Paid) {
            return (id, key);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("mint quote never paid");
}

async fn melt_quote(mint: &Mint, amount_sat: u64, pays: bool) -> MeltQuote {
    let state = if pays {
        MeltQuoteState::Paid
    } else {
        MeltQuoteState::Failed
    };
    let description = FakeInvoiceDescription {
        pay_invoice_state: state,
        check_payment_state: state,
        pay_err: false,
        check_err: false,
    };
    let invoice = create_fake_invoice(
        amount_sat * 1000,
        serde_json::to_string(&description).unwrap(),
    );
    let response = mint
        .get_melt_quote(MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        }))
        .await
        .unwrap();
    mint.localstore
        .get_melt_quote(response.quote().unwrap())
        .await
        .unwrap()
        .unwrap()
}

async fn outputs(mint: &Mint, amount: u64) -> (Vec<BlindedMessage>, PreMintSecrets) {
    create_test_blinded_messages(mint, amount.into())
        .await
        .unwrap()
}

/// Build the transcript the mint will build and sign every input with its key.
fn sign(request: &mut TransactionRequest, quote_keys: &[SecretKey], melt: Option<&MeltQuote>) {
    sign_at(request, quote_keys, melt, &[]);
}

/// [`sign`] with each quote input bound at `issued` (0 where absent).
fn sign_at(
    request: &mut TransactionRequest,
    quote_keys: &[SecretKey],
    melt: Option<&MeltQuote>,
    issued: &[u64],
) {
    let mint_quotes: Vec<nutroot::MintQuoteInput> = request
        .mint_quote_inputs
        .iter()
        .zip(quote_keys)
        .enumerate()
        .map(|(i, (q, key))| nutroot::MintQuoteInput {
            id: q.quote.clone(),
            amount: q.amount,
            pubkey: key.public_key(),
            issued: issued.get(i).copied().unwrap_or(0).into(),
        })
        .collect();
    let melt_quotes: Vec<nutroot::Quote> = melt
        .map(|quote| {
            vec![nutroot::Quote {
                id: quote.id.to_string(),
                amount: Amount::from(quote.amount()) + Amount::from(quote.fee_reserve()),
            }]
        })
        .unwrap_or_default();
    let transaction = Transaction::new(
        &request.proof_inputs,
        &mint_quotes,
        &request.blinded_outputs,
        &melt_quotes,
    )
    .unwrap();
    for (index, proof) in request.proof_inputs.iter_mut().enumerate() {
        let key = proof
            .spend_info
            .as_ref()
            .unwrap()
            .key_path_key(&proof.secret.to_string(), None)
            .unwrap();
        proof.witness = Some(Witness::NutrootWitness(
            serde_json::to_string(&NutrootWitness::key_path(
                &key,
                transaction.input_digest(index).unwrap(),
            ))
            .unwrap(),
        ));
        proof.spend_info = None;
    }
    let offset = request.proof_inputs.len();
    for (index, (input, key)) in request
        .mint_quote_inputs
        .iter_mut()
        .zip(quote_keys)
        .enumerate()
    {
        input.witness = NutrootWitness::key_path(
            key.as_secp256k1().unwrap(),
            transaction.input_digest(offset + index).unwrap(),
        )
        .signatures[0]
            .clone();
    }
}

fn quote_input(id: &QuoteId, amount: u64) -> TransactionQuoteInput {
    TransactionQuoteInput {
        quote: id.to_string(),
        amount: amount.into(),
        witness: String::new(),
    }
}

fn melt_output(quote: &MeltQuote) -> TransactionMeltOutput {
    TransactionMeltOutput {
        quote: quote.id.to_string(),
        fee_reserve: quote.fee_reserve().into(),
        fee_index: None,
    }
}

/// Poll a melt transaction, as a wallet polls its melt quote, until it leaves `PENDING`.
async fn settled(mint: &Mint, melt: &MeltQuote) -> cdk_common::transaction::TransactionResponse {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = mint.melt_transaction_response(&melt.id).await.unwrap();
            if response.state != TransactionState::Pending {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap()
}

fn signed_amounts(response: &cdk_common::transaction::TransactionResponse) -> Vec<u64> {
    response
        .signatures
        .iter()
        .map(|sig| u64::from(sig.amount))
        .collect()
}

#[test]
fn change_is_the_excess_less_the_fee_capped_at_the_reserve() {
    assert_eq!(
        settlement_change(6.into(), 1.into(), 0.into()),
        Amount::from(6)
    );
    assert_eq!(
        settlement_change(6.into(), 1.into(), 1.into()),
        Amount::from(5)
    );
    // A backend fee over the reserve is the mint's loss, never negative change.
    assert_eq!(
        settlement_change(6.into(), 1.into(), 3.into()),
        Amount::from(5)
    );
    assert_eq!(
        settlement_change(0.into(), 1.into(), 1.into()),
        Amount::ZERO
    );
}

#[tokio::test]
async fn quote_inputs_count_against_max_inputs_before_any_lookup() {
    let mint = v3_mint().await;
    let (blinded, _) = outputs(&mint, 1).await;
    // Unknown quotes: the cap answers first, so none is loaded.
    let request = TransactionRequest {
        mint_quote_inputs: (0..=mint.max_inputs())
            .map(|_| quote_input(&QuoteId::new(), 1))
            .collect(),
        blinded_outputs: blinded,
        ..Default::default()
    };
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::MaxInputsExceeded { .. })
    ));
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn proofs_to_outputs_balance_exactly() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 8.into()).await.unwrap();
    let ys = proofs.ys().unwrap();
    // Inputs equal outputs exactly.
    let (short, _) = outputs(&mint, 3).await;
    let mut request = TransactionRequest {
        proof_inputs: proofs.clone(),
        blinded_outputs: short,
        ..Default::default()
    };
    sign(&mut request, &[], None);
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::TransactionUnbalanced(8, 3, 0))
    ));

    let (blinded, premints) = outputs(&mint, 8).await;
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        blinded_outputs: blinded.clone(),
        ..Default::default()
    };
    sign(&mut request, &[], None);
    let response = mint
        .process_transaction(request.clone(), false)
        .await
        .unwrap();
    assert_eq!(response.state, TransactionState::Paid);
    assert_eq!(response.signatures.len(), blinded.len());
    assert!(response.melt_quotes.is_empty());
    let states = mint.localstore.get_proofs_states(&ys).await.unwrap();
    assert!(states.iter().all(|s| *s == Some(State::Spent)));

    // A resend is refused: its inputs are spent.
    assert!(mint.process_transaction(request, false).await.is_err());
    let signed = cdk_common::dhke::construct_proofs(
        response.signatures.clone(),
        premints.rs(),
        premints.secrets(),
        &mint.keyset_pubkeys(&blinded[0].keyset_id).unwrap().keysets[0].keys,
    )
    .unwrap();
    assert_eq!(signed.total_amount().unwrap(), Amount::from(8));
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn quote_inputs_draw_partially_and_never_past_mintable() {
    let mint = v3_mint().await;
    let (quote_id, key) = paid_locked_quote(&mint, 8).await;
    let (first, _) = outputs(&mint, 3).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 3)],
        blinded_outputs: first,
        ..Default::default()
    };
    sign(&mut request, std::slice::from_ref(&key), None);
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Paid);
    let quote = mint
        .localstore
        .get_mint_quote(&quote_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        quote.amount_issued(),
        Amount::from(3).with_unit(CurrencyUnit::Sat)
    );
    assert_eq!(quote.state(), MintQuoteState::Paid);

    // Overdraw: 6 of the 5 still mintable.
    let (too_many, _) = outputs(&mint, 6).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 6)],
        blinded_outputs: too_many,
        ..Default::default()
    };
    sign_at(&mut request, std::slice::from_ref(&key), None, &[3]);
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::InvalidTransaction(_))
    ));

    // Blank outputs are rejected without a melt.
    let (mut blank, _) = outputs(&mint, 5).await;
    blank[0].amount = Amount::ZERO;
    let request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 5)],
        blinded_outputs: blank,
        ..Default::default()
    };
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::InvalidTransaction(_))
    ));

    // Unbalanced.
    let (short, _) = outputs(&mint, 4).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 5)],
        blinded_outputs: short,
        ..Default::default()
    };
    sign_at(&mut request, std::slice::from_ref(&key), None, &[3]);
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::TransactionUnbalanced(5, 4, 0))
    ));

    // The remainder is still mintable, by a witness over the quote's current state: one signed
    // at the state the first draw consumed is refused (NUT-10 `issued`).
    let (rest, _) = outputs(&mint, 5).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 5)],
        blinded_outputs: rest,
        ..Default::default()
    };
    sign(&mut request, std::slice::from_ref(&key), None);
    assert!(matches!(
        mint.process_transaction(request.clone(), false).await,
        Err(Error::NUT10(_))
    ));
    sign_at(&mut request, &[key], None, &[3]);
    mint.process_transaction(request, false).await.unwrap();
    let quote = mint
        .localstore
        .get_mint_quote(&quote_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(quote.state(), MintQuoteState::Issued);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn melt_from_a_quote_input_returns_the_unspent_reserve_in_blanks() {
    let mint = v3_mint().await;
    let (quote_id, key) = paid_locked_quote(&mint, 10).await;
    let melt = melt_quote(&mint, 5, true).await;
    let reserve = Amount::from(melt.fee_reserve());
    assert!(reserve > Amount::ZERO);
    let (mut blanks, _) = outputs(&mint, 3).await;
    for blank in &mut blanks {
        blank.amount = Amount::ZERO;
    }
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 10)],
        blinded_outputs: blanks,
        melt_quote_outputs: vec![melt_output(&melt)],
        ..Default::default()
    };
    // The wrong reserve is rejected before anything is reserved.
    let mut wrong = request.clone();
    wrong.melt_quote_outputs[0].fee_reserve = reserve + Amount::ONE;
    sign(&mut wrong, std::slice::from_ref(&key), Some(&melt));
    assert!(matches!(
        mint.process_transaction(wrong, false).await,
        Err(Error::InvalidTransaction(_))
    ));

    sign(&mut request, &[key], Some(&melt));
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Paid);
    assert_eq!(response.melt_quotes.len(), 1);
    assert_eq!(response.melt_quotes[0]["state"], "PAID");
    // Inputs 10, melt 5, the fake backend charges 1 of the reserve: 4 back in one blank.
    assert_eq!(signed_amounts(&response), vec![4]);
    assert_eq!(
        response.melt_quotes[0]["change"],
        serde_json::to_value(&response.signatures).unwrap()
    );
    let quote = mint
        .localstore
        .get_mint_quote(&quote_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(quote.state(), MintQuoteState::Issued);
    let melt = mint
        .localstore
        .get_melt_quote(&melt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(melt.state, MeltQuoteState::Paid);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn failed_melt_releases_proofs_and_quote_inputs() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 4.into()).await.unwrap();
    let ys = proofs.ys().unwrap();
    let (quote_id, key) = paid_locked_quote(&mint, 4).await;
    let melt = melt_quote(&mint, 2, false).await;
    let (blinded, _) = outputs(&mint, 4).await;
    let mut request = TransactionRequest {
        proof_inputs: proofs.clone(),
        mint_quote_inputs: vec![quote_input(&quote_id, 4)],
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        ..Default::default()
    };
    sign(&mut request, std::slice::from_ref(&key), Some(&melt));
    let response = mint
        .process_transaction(request.clone(), false)
        .await
        .unwrap();
    assert_eq!(response.state, TransactionState::Failed);
    assert!(response.signatures.is_empty());

    let states = mint.localstore.get_proofs_states(&ys).await.unwrap();
    assert!(states.iter().all(Option::is_none), "proofs released");
    let quote = mint
        .localstore
        .get_mint_quote(&quote_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
    assert_eq!(quote.state(), MintQuoteState::Paid);
    let melt = mint
        .localstore
        .get_melt_quote(&melt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(melt.state, MeltQuoteState::Unpaid);

    // The released inputs spend again.
    let (blinded, _) = outputs(&mint, 8).await;
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        mint_quote_inputs: vec![quote_input(&quote_id, 4)],
        blinded_outputs: blinded,
        ..Default::default()
    };
    sign(&mut request, std::slice::from_ref(&key), None);
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Paid);
    drop(key);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn async_melt_transaction_is_pending_then_paid() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 16.into()).await.unwrap();
    let melt = melt_quote(&mint, 5, true).await;
    assert_eq!(Amount::from(melt.fee_reserve()), Amount::from(5));
    let (mut blinded, _) = outputs(&mint, 6).await;
    let (mut blank, _) = outputs(&mint, 1).await;
    blank[0].amount = Amount::ZERO;
    blinded.append(&mut blank);
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        prefer_async: true,
        ..Default::default()
    };
    sign(&mut request, &[], Some(&melt));
    let response = mint
        .process_transaction(request.clone(), false)
        .await
        .unwrap();
    assert!(matches!(
        response.state,
        TransactionState::Pending | TransactionState::Paid
    ));
    let settled = settled(&mint, &melt).await;
    assert_eq!(settled.state, TransactionState::Paid);
    // 16 in, 6 out, 5 melt, 1 of the 5 reserve spent: 4 back in the blank.
    assert_eq!(signed_amounts(&settled), vec![2, 4, 4]);
    // The melt quote records the transaction: its change is the outputs' signatures.
    let signatures = serde_json::to_value(&settled.signatures).unwrap();
    assert_eq!(settled.melt_quotes[0]["change"], signatures);
    let checked = mint.check_melt_quote(&melt.id).await.unwrap();
    let checked =
        serde_json::to_value(cdk_common::transaction::melt_quote_json(checked).unwrap()).unwrap();
    assert_eq!(checked["state"], "PAID");
    assert_eq!(checked["change"], signatures);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn rejects_missing_witnesses_and_duplicate_quotes() {
    let mint = v3_mint().await;
    let (quote_id, key) = paid_locked_quote(&mint, 4).await;
    let (blinded, _) = outputs(&mint, 4).await;
    let request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 4)],
        blinded_outputs: blinded.clone(),
        ..Default::default()
    };
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::SignatureMissingOrInvalid)
    ));
    let mut duplicate = quote_input(&quote_id, 2);
    duplicate.witness = "00".repeat(64);
    let request = TransactionRequest {
        mint_quote_inputs: vec![duplicate.clone(), duplicate],
        blinded_outputs: blinded.clone(),
        ..Default::default()
    };
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::DuplicateQuoteIds)
    ));
    // A witness by the wrong key is invalid, not missing.
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 4)],
        blinded_outputs: blinded,
        ..Default::default()
    };
    sign(&mut request, &[SecretKey::generate()], None);
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::NUT10(_))
    ));
    let proofs: Proofs = vec![];
    assert!(proofs.is_empty());
    drop(key);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn mint_quote_input_commits_the_amount_issued() {
    // A single quote spent through NUT-04 signs the outputs total, not the face amount.
    let mint = v3_mint().await;
    let (quote_id, key) = paid_locked_quote(&mint, 8).await;
    let (blinded, _) = outputs(&mint, 8).await;
    let transaction = Transaction::new(
        &[],
        &[nutroot::MintQuoteInput {
            id: quote_id.to_string(),
            amount: 8.into(),
            pubkey: key.public_key(),
            issued: Amount::ZERO,
        }],
        &blinded,
        &[],
    )
    .unwrap();
    let signature = NutrootWitness::key_path(
        key.as_secp256k1().unwrap(),
        transaction.input_digest(0).unwrap(),
    )
    .signatures[0]
        .clone();
    let split = SplitTarget::None;
    let _ = split;
    mint.process_mint_request(MintInput::Single(MintRequest {
        quote: quote_id,
        outputs: blinded,
        signature: Some(signature),
    }))
    .await
    .unwrap();
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn failed_melt_retries_with_its_quote_inputs() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, false).await;
    let mut results = Vec::new();
    for _ in 0..2 {
        let mut request = TransactionRequest {
            mint_quote_inputs: vec![quote_input(&id, 4)],
            melt_quote_outputs: vec![melt_output(&melt)],
            ..Default::default()
        };
        sign(&mut request, std::slice::from_ref(&key), Some(&melt));
        results.push(
            mint.process_transaction(request, false)
                .await
                .unwrap()
                .state,
        );
    }
    let quote = mint.localstore.get_mint_quote(&id).await.unwrap().unwrap();
    mint.stop().await.unwrap();
    assert_eq!(
        results,
        vec![TransactionState::Failed, TransactionState::Failed]
    );
    assert_eq!(Amount::from(quote.amount_issued()), Amount::ZERO);
}

#[tokio::test]
async fn prefer_async_returns_before_payment_completes() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, true).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 4)],
        melt_quote_outputs: vec![melt_output(&melt)],
        prefer_async: true,
        ..Default::default()
    };
    sign(&mut request, &[key], Some(&melt));
    // This test backend takes two seconds to make the outgoing payment.
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        mint.process_transaction(request.clone(), false),
    )
    .await;
    let response = response
        .expect("prefer_async must return before payment completes")
        .unwrap();
    assert_eq!(response.state, TransactionState::Pending);
    let settled = settled(&mint, &melt).await;
    assert_eq!(settled.state, TransactionState::Paid);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn invalid_denomination_rejected_before_payment() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, true).await;
    let (mut blinded, _) = outputs(&mint, 1).await;
    blinded[0].amount = Amount::from(3);
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 7)],
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        ..Default::default()
    };
    sign(&mut request, &[key], Some(&melt));
    let result = mint.process_transaction(request, false).await;
    let stored = mint
        .localstore
        .get_melt_quote(&melt.id)
        .await
        .unwrap()
        .unwrap();
    mint.stop().await.unwrap();
    assert!(matches!(result, Err(Error::InvalidTransaction(_))));
    assert_eq!(stored.state, MeltQuoteState::Unpaid);
    let quote = mint.localstore.get_mint_quote(&id).await.unwrap().unwrap();
    assert_eq!(Amount::from(quote.amount_issued()), Amount::ZERO);
}

#[tokio::test]
async fn outputs_on_a_keyset_expiring_before_the_melt_are_rejected() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, true).await;
    mint.rotate_keyset_by_version(
        CurrencyUnit::Sat,
        vec![1, 2, 4, 8, 16, 32],
        0,
        cdk_common::nuts::KeySetVersion::Version02,
        Some(melt.expiry - 1),
    )
    .await
    .unwrap();
    let (mut blinded, _) = outputs(&mint, 1).await;
    blinded[0].amount = Amount::ZERO;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 16)],
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        ..Default::default()
    };
    sign(&mut request, &[key], Some(&melt));
    let result = mint.process_transaction(request, false).await;
    let stored = mint
        .localstore
        .get_melt_quote(&melt.id)
        .await
        .unwrap()
        .unwrap();
    mint.stop().await.unwrap();
    assert!(
        matches!(&result, Err(Error::InvalidTransaction(reason)) if reason.contains("expires before the melt quote")),
        "{result:?}"
    );
    assert_eq!(stored.state, MeltQuoteState::Unpaid);
}

#[tokio::test]
async fn blank_outputs_take_the_change() {
    let mint = v3_mint().await;
    // 27 in, 1 fixed, melt 13, 1 sat of the 13 sat reserve spent: change 12, imprinted as 8 and 4
    // into the earliest blanks in request order, wherever the fixed output sits. What the blanks
    // cannot hold stays with the mint.
    for (blank_count, fixed_at, signed) in [
        (3, 1, vec![8, 1, 4]),
        (3, 3, vec![8, 4, 1]),
        (1, 1, vec![8, 1]),
    ] {
        let (id, key) = paid_locked_quote(&mint, 27).await;
        let melt = melt_quote(&mint, 13, true).await;
        assert_eq!(Amount::from(melt.fee_reserve()), Amount::from(13));
        let (mut blinded, _) = outputs(&mint, 7).await;
        blinded.truncate(blank_count);
        for output in &mut blinded {
            output.amount = Amount::ZERO;
        }
        let (fixed, _) = outputs(&mint, 1).await;
        blinded.insert(fixed_at, fixed[0].clone());
        let mut request = TransactionRequest {
            mint_quote_inputs: vec![quote_input(&id, 27)],
            blinded_outputs: blinded,
            melt_quote_outputs: vec![melt_output(&melt)],
            ..Default::default()
        };
        sign(&mut request, &[key], Some(&melt));
        mint.process_transaction(request.clone(), false)
            .await
            .unwrap();
        let response = settled(&mint, &melt).await;
        assert_eq!(response.state, TransactionState::Paid);
        assert_eq!(signed_amounts(&response), signed);
        assert_eq!(
            response.melt_quotes[0]["change"],
            serde_json::to_value(&response.signatures).unwrap()
        );
    }
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn blank_outputs_take_what_the_keyset_can_represent() {
    let mint = v3_mint().await;
    // Configured out of order: the fill still takes the largest amount that fits.
    mint.rotate_keyset_by_version(
        CurrencyUnit::Sat,
        vec![2, 8, 4, 32, 16],
        0,
        cdk_common::nuts::KeySetVersion::Version02,
        None,
    )
    .await
    .unwrap();
    let (id, key) = paid_locked_quote(&mint, 26).await;
    let melt = melt_quote(&mint, 12, true).await;
    let (mut blinded, _) = outputs(&mint, 7).await;
    for output in &mut blinded {
        output.amount = Amount::ZERO;
    }
    let (fixed, _) = outputs(&mint, 2).await;
    blinded.extend(fixed);
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 26)],
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        ..Default::default()
    };
    sign(&mut request, &[key], Some(&melt));
    mint.process_transaction(request.clone(), false)
        .await
        .unwrap();
    let response = settled(&mint, &melt).await;
    assert_eq!(response.state, TransactionState::Paid);
    // 26 in, 2 fixed, melt 12, 1 sat of the 12 sat reserve spent: change 11, the blanks take 8
    // and 2, and the 1 the keyset cannot represent stays with the mint.
    assert_eq!(signed_amounts(&response), vec![8, 2, 2]);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn melt_quote_reads_pending_until_its_transaction_settles() {
    let mint = v3_mint().await;
    let mut quote = melt_quote(&mint, 2, true).await;
    let mut tx = mint.localstore.begin_transaction().await.unwrap();
    tx.add_transaction(&TransactionRecord {
        melt_quote_id: quote.id.clone(),
        state: TransactionState::Pending,
        unit: CurrencyUnit::Sat,
        quote_inputs: vec![],
        excess: Amount::ZERO,
        created_time: 0,
    })
    .await
    .unwrap();
    tx.commit().await.unwrap();
    quote.state = MeltQuoteState::Paid;
    assert!(settled_melt_state(&mint.localstore, &mut quote)
        .await
        .unwrap());
    assert_eq!(quote.state, MeltQuoteState::Pending);

    let mut tx = mint.localstore.begin_transaction().await.unwrap();
    let mut record = tx
        .get_transaction_by_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap();
    tx.update_transaction(&mut record, TransactionState::Paid)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    quote.state = MeltQuoteState::Paid;
    assert!(settled_melt_state(&mint.localstore, &mut quote)
        .await
        .unwrap());
    assert_eq!(quote.state, MeltQuoteState::Paid);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn a_quote_drawn_by_a_pending_transaction_takes_no_other_draw() {
    let mint = v3_mint().await;
    let (quote_id, key) = paid_locked_quote(&mint, 8).await;
    let melt = melt_quote(&mint, 2, true).await;
    let mut tx = mint.localstore.begin_transaction().await.unwrap();
    tx.add_transaction(&TransactionRecord {
        melt_quote_id: melt.id.clone(),
        state: TransactionState::Pending,
        unit: CurrencyUnit::Sat,
        quote_inputs: vec![(quote_id.clone(), Amount::from(4))],
        excess: Amount::ZERO,
        created_time: 0,
    })
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Another transaction, and a NUT-04 mint, both refused with 20005.
    let (blinded, _) = outputs(&mint, 2).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 2)],
        blinded_outputs: blinded,
        ..Default::default()
    };
    sign(&mut request, std::slice::from_ref(&key), None);
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::PendingQuote)
    ));
    let (full, _) = outputs(&mint, 8).await;
    let transaction = Transaction::new(
        &[],
        &[nutroot::MintQuoteInput {
            id: quote_id.to_string(),
            amount: 8.into(),
            pubkey: key.public_key(),
            issued: Amount::ZERO,
        }],
        &full,
        &[],
    )
    .unwrap();
    let signature = NutrootWitness::key_path(
        key.as_secp256k1().unwrap(),
        transaction.input_digest(0).unwrap(),
    )
    .signatures[0]
        .clone();
    assert!(matches!(
        mint.process_mint_request(MintInput::Single(MintRequest {
            quote: quote_id.clone(),
            outputs: full,
            signature: Some(signature),
        }))
        .await,
        Err(Error::PendingQuote)
    ));
    // Nothing is issued while the transaction is pending.
    let quote = mint
        .localstore
        .get_mint_quote(&quote_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn custom_method_melt_check_carries_change_once_settled() {
    let mint = v3_mint().await;
    let mut quote = MeltQuote::new(
        None,
        cdk_common::mint::MeltPaymentRequest::Custom {
            method: "test_method".to_owned(),
            request: "custom-payment".to_owned(),
        },
        CurrencyUnit::Sat,
        Amount::new(21, CurrencyUnit::Sat),
        Amount::new(2, CurrencyUnit::Sat),
        0,
        None,
        None,
        cdk_common::PaymentMethod::Custom("test_method".to_owned()),
        None,
        Some(1),
    );
    quote.state = MeltQuoteState::Paid;
    quote.payment_proof = Some("payment-proof".to_owned());
    let signature = cdk_common::BlindSignature {
        amount: 2.into(),
        keyset_id: "009a1f293253e41e".parse().unwrap(),
        c: SecretKey::generate().public_key(),
        dleq: None,
    };
    let record = TransactionRecord {
        melt_quote_id: quote.id.clone(),
        state: TransactionState::Pending,
        unit: CurrencyUnit::Sat,
        quote_inputs: vec![],
        excess: Amount::ZERO,
        created_time: 0,
    };
    let mut tx = mint.localstore.begin_transaction().await.unwrap();
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.add_transaction(&record).await.unwrap();
    tx.commit().await.unwrap();

    // Paid but unsettled: the check route reads PENDING.
    let checked =
        cdk_common::transaction::melt_quote_json(mint.check_melt_quote(&quote.id).await.unwrap())
            .unwrap();
    assert_eq!(checked["state"], "PENDING");

    let mut tx = mint.localstore.begin_transaction().await.unwrap();
    tx.add_blind_signatures(
        &[SecretKey::generate().public_key()],
        std::slice::from_ref(&signature),
        Some(quote.id.clone()),
    )
    .await
    .unwrap();
    let mut stored = tx
        .get_transaction_by_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap();
    tx.update_transaction(&mut stored, TransactionState::Paid)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let checked =
        cdk_common::transaction::melt_quote_json(mint.check_melt_quote(&quote.id).await.unwrap())
            .unwrap();
    assert_eq!(checked["state"], "PAID");
    assert_eq!(
        checked["change"],
        serde_json::to_value(vec![signature]).unwrap()
    );
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn unfinished_melt_settlement_reads_pending_on_check() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 16.into()).await.unwrap();
    let melt = melt_quote(&mint, 5, true).await;
    let (blinded, _) = outputs(&mint, 6).await;
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        prefer_async: true,
        ..Default::default()
    };
    sign(&mut request, &[], Some(&melt));
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Pending);
    // A rotation while the payment is in flight inactivates the outputs' keyset, which the
    // signatory then refuses, so the settlement step fails after the melt is paid.
    mint.rotate_keyset_by_version(
        CurrencyUnit::Sat,
        vec![1, 2, 4, 8, 16, 32],
        0,
        cdk_common::nuts::KeySetVersion::Version02,
        None,
    )
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let stored = mint.localstore.get_melt_quote(&melt.id).await.unwrap();
            if stored.is_some_and(|quote| quote.state == MeltQuoteState::Paid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let saga = mint
        .localstore
        .get_melt_saga_by_quote_id(&melt.id)
        .await
        .unwrap()
        .expect("saga awaiting its settlement");
    assert!(saga.finalization_data.is_some());

    // The check retries the settlement, and a failed retry still reads PENDING.
    for _ in 0..2 {
        let checked = cdk_common::transaction::melt_quote_json(
            mint.check_melt_quote(&melt.id).await.unwrap(),
        )
        .unwrap();
        assert_eq!(checked["state"], "PENDING");
    }
    mint.stop().await.unwrap();
}
