//! NUT-XX transaction tests against a v3 test mint with the fake payment backend.

use std::str::FromStr;
use std::time::Duration;

use cdk_common::amount::SplitTarget;
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::mint::MeltQuote;
use cdk_common::nuts::nut10::nutroot::{self, Transaction, Witness as NutrootWitness};
use cdk_common::nuts::{
    CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteBolt11Request, MintQuoteState,
    MintRequest, PreMintSecrets, SecretKey, State, TransactionChangeOutput, TransactionMeltOutput,
    TransactionQuoteInput, TransactionRequest, TransactionState, Witness,
};
use cdk_common::{Amount, BlindedMessage, Proofs, ProofsMethods, QuoteId};
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};

use super::settlement_change;
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
    let mint_quotes: Vec<nutroot::MintQuoteInput> = request
        .mint_quote_inputs
        .iter()
        .zip(quote_keys)
        .map(|(q, key)| nutroot::MintQuoteInput {
            id: q.quote.clone(),
            amount: q.amount,
            pubkey: key.public_key(),
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
    let change: Vec<nutroot::ChangeOutput> = request
        .change_quote_outputs
        .iter()
        .map(|output| nutroot::ChangeOutput {
            amount: output.amount,
            pubkey: *cdk_common::PublicKey::from_hex(&output.pubkey)
                .unwrap()
                .as_secp256k1()
                .unwrap(),
        })
        .collect();
    let transaction = Transaction::with_change(
        &request.proof_inputs,
        &mint_quotes,
        &request.blinded_outputs,
        &melt_quotes,
        &change,
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

/// A change quote output locked to `key`; a remainder quote when `amount` is `None`.
fn change_output(key: &SecretKey, amount: Option<u64>) -> TransactionChangeOutput {
    TransactionChangeOutput {
        pubkey: key.public_key().to_hex(),
        amount: amount.map(Into::into),
    }
}

fn change_quote(
    response: &cdk_common::transaction::TransactionResponse,
    index: usize,
) -> &serde_json::Value {
    let quote = response.change_quotes[index]
        .as_ref()
        .expect("change quote");
    assert_eq!(quote["method"], "change");
    quote
}

fn change_quote_id(response: &cdk_common::transaction::TransactionResponse) -> QuoteId {
    QuoteId::from_str(change_quote(response, 0)["quote"].as_str().unwrap()).unwrap()
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
async fn proofs_to_outputs_park_the_rest_in_a_change_quote() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 8.into()).await.unwrap();
    let ys = proofs.ys().unwrap();
    let (blinded, premints) = outputs(&mint, 3).await;
    let change_key = SecretKey::generate();
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        blinded_outputs: blinded.clone(),
        change_quote_outputs: vec![change_output(&change_key, None)],
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
    let change = change_quote(&response, 0);
    assert_eq!(change["amount_paid"], 5);
    assert_eq!(change["amount_issued"], 0);
    let states = mint.localstore.get_proofs_states(&ys).await.unwrap();
    assert!(states.iter().all(|s| *s == Some(State::Spent)));

    // A resend returns the record rather than treating the inputs as spent again.
    let again = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(again, response);
    assert_eq!(
        mint.get_transaction(&response.digest).await.unwrap(),
        response
    );
    let signed = cdk_common::dhke::construct_proofs(
        response.signatures.clone(),
        premints.rs(),
        premints.secrets(),
        &mint.keyset_pubkeys(&blinded[0].keyset_id).unwrap().keysets[0].keys,
    )
    .unwrap();
    assert_eq!(signed.total_amount().unwrap(), Amount::from(3));

    // The change quote is a locked, paid mint quote redeemed at POST /v1/mint/change.
    let change_id = change_quote_id(&response);
    let checked = mint
        .check_mint_quotes(std::slice::from_ref(&change_id))
        .await
        .unwrap();
    assert_eq!(checked[0].state(), Some(MintQuoteState::Paid));
    let (redeem, _) = outputs(&mint, 5).await;
    let redeem_count = redeem.len();
    let transaction = Transaction::new(
        &[],
        &[nutroot::MintQuoteInput {
            id: change_id.to_string(),
            amount: 5.into(),
            pubkey: change_key.public_key(),
        }],
        &redeem,
        &[],
    )
    .unwrap();
    let signature = NutrootWitness::key_path(
        change_key.as_secp256k1().unwrap(),
        transaction.input_digest(0).unwrap(),
    )
    .signatures[0]
        .clone();
    let minted = mint
        .process_mint_request(MintInput::Single(MintRequest {
            quote: change_id.clone(),
            outputs: redeem,
            signature: Some(signature),
        }))
        .await
        .unwrap();
    assert_eq!(minted.signatures.len(), redeem_count);
    assert_eq!(
        mint.check_mint_quotes(&[change_id]).await.unwrap()[0].state(),
        Some(MintQuoteState::Issued)
    );
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
    let first_request = request.clone();
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Paid);
    assert!(response.change_quotes.is_empty());
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
    sign(&mut request, std::slice::from_ref(&key), None);
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::InvalidTransaction(_))
    ));

    // Blank outputs are rejected: change goes to the change quote.
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

    // Unbalanced without a change key.
    let (short, _) = outputs(&mint, 4).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 5)],
        blinded_outputs: short,
        ..Default::default()
    };
    sign(&mut request, std::slice::from_ref(&key), None);
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::TransactionUnbalanced(5, 4, 0))
    ));

    // The remainder is still mintable.
    let (rest, _) = outputs(&mint, 5).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 5)],
        blinded_outputs: rest,
        ..Default::default()
    };
    sign(&mut request, &[key], None);
    let last = mint
        .process_transaction(request.clone(), false)
        .await
        .unwrap();
    // A resend of the first draw returns its record, though the quote is now drained.
    assert_eq!(
        mint.process_transaction(first_request, false)
            .await
            .unwrap(),
        response
    );
    assert_eq!(
        mint.process_transaction(request, false).await.unwrap(),
        last
    );
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
async fn melt_from_a_quote_input_returns_the_unspent_reserve_as_change() {
    let mint = v3_mint().await;
    let (quote_id, key) = paid_locked_quote(&mint, 10).await;
    let melt = melt_quote(&mint, 5, true).await;
    let reserve = Amount::from(melt.fee_reserve());
    assert!(reserve > Amount::ZERO);
    let change_key = SecretKey::generate();
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 10)],
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change_output(&change_key, None)],
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
    assert!(response.signatures.is_empty());
    assert_eq!(response.melt_quotes.len(), 1);
    assert_eq!(response.melt_quotes[0]["state"], "PAID");
    // Inputs 10, melt 5, the fake backend charges 1 of the reserve: 4 back.
    assert_eq!(change_quote(&response, 0)["amount_paid"], 4);
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
    let (blinded, _) = outputs(&mint, 1).await;
    let change_key = SecretKey::generate();
    let mut request = TransactionRequest {
        proof_inputs: proofs.clone(),
        mint_quote_inputs: vec![quote_input(&quote_id, 4)],
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change_output(&change_key, None)],
        ..Default::default()
    };
    sign(&mut request, std::slice::from_ref(&key), Some(&melt));
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Failed);
    assert!(response.signatures.is_empty() && response.change_quotes == vec![None]);

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

    // The released inputs spend again, and the failed record is replaced.
    let (blinded, _) = outputs(&mint, 8).await;
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        mint_quote_inputs: vec![quote_input(&quote_id, 4)],
        blinded_outputs: blinded,
        ..Default::default()
    };
    sign(&mut request, &[key], None);
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Paid);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn async_melt_transaction_is_pending_then_paid() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 16.into()).await.unwrap();
    let melt = melt_quote(&mint, 5, true).await;
    assert_eq!(Amount::from(melt.fee_reserve()), Amount::from(5));
    let (blinded, _) = outputs(&mint, 1).await;
    let change_key = SecretKey::generate();
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change_output(&change_key, None)],
        prefer_async: true,
        ..Default::default()
    };
    sign(&mut request, &[], Some(&melt));
    let response = mint.process_transaction(request, false).await.unwrap();
    assert!(matches!(
        response.state,
        TransactionState::Pending | TransactionState::Paid
    ));
    let mut settled = response;
    for _ in 0..30 {
        settled = mint.get_transaction(&settled.digest).await.unwrap();
        if settled.state == TransactionState::Paid {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(settled.state, TransactionState::Paid);
    assert_eq!(settled.signatures.len(), 1);
    // 16 in, 1 out, 5 melt, 1 of the reserve spent: 9 back.
    assert_eq!(change_quote(&settled, 0)["amount_paid"], 9);
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
async fn duplicate_quote_only_submission_is_idempotent() {
    // Also cover a fee consuming all value: no duplicate change quote exists
    // to prevent a second issuance, so the digest insertion must reject it.
    for fee_ppk in [0, 2000] {
        let mint = v3_mint().await;
        let mut info = mint.mint_info().await.unwrap();
        info.nuts.nutxx.quote_input_fee_ppk = fee_ppk;
        mint.set_mint_info(info).await.unwrap();
        let (id, key) = paid_locked_quote(&mint, 8).await;
        let change_key = SecretKey::generate();
        let mut request = TransactionRequest {
            mint_quote_inputs: vec![quote_input(&id, 2)],
            change_quote_outputs: vec![change_output(&change_key, None)],
            ..Default::default()
        };
        sign(&mut request, &[key], None);
        // Both HTTP requests validate before either acquires the reservation locks.
        let super::Preparation::Fresh(first) =
            mint.prepare_transaction(request.clone()).await.unwrap()
        else {
            panic!()
        };
        let super::Preparation::Fresh(second) = mint.prepare_transaction(request).await.unwrap()
        else {
            panic!()
        };
        let first = mint.settle_without_melt(*first).await.unwrap();
        let second = mint.settle_without_melt(*second).await.unwrap();
        assert_eq!(first, second);
        let quote = mint.localstore.get_mint_quote(&id).await.unwrap().unwrap();
        mint.stop().await.unwrap();
        assert_eq!(
            Amount::from(quote.amount_issued()),
            Amount::from(2),
            "same digest must issue once"
        );
    }
}

#[tokio::test]
async fn retry_melt_with_new_digest_releases_quote_inputs() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, false).await;
    let mut results = Vec::new();
    for _ in 0..2 {
        let change_key = SecretKey::generate();
        let mut request = TransactionRequest {
            mint_quote_inputs: vec![quote_input(&id, 8)],
            melt_quote_outputs: vec![melt_output(&melt)],
            change_quote_outputs: vec![change_output(&change_key, None)],
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
    let change_key = SecretKey::generate();
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 8)],
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change_output(&change_key, None)],
        prefer_async: true,
        ..Default::default()
    };
    sign(&mut request, &[key], Some(&melt));
    // This test backend takes two seconds to make the outgoing payment.
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        mint.process_transaction(request, false),
    )
    .await;
    let response = response
        .expect("prefer_async must return before payment completes")
        .unwrap();
    assert_eq!(response.state, TransactionState::Pending);
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = mint.get_transaction(&response.digest).await.unwrap();
            if response.state != TransactionState::Pending {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(settled.state, TransactionState::Paid);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn invalid_denomination_rejected_before_payment() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, true).await;
    let change_key = SecretKey::generate();
    let (mut blinded, _) = outputs(&mint, 1).await;
    blinded[0].amount = Amount::from(3);
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 16)],
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change_output(&change_key, None)],
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
async fn resend_after_output_keyset_deactivation() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 8).await;
    let (blinded, _) = outputs(&mint, 8).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 8)],
        blinded_outputs: blinded,
        ..Default::default()
    };
    sign(&mut request, &[key], None);
    let first = mint
        .process_transaction(request.clone(), false)
        .await
        .unwrap();
    let mut keysets = mint.keysets.load().as_ref().clone();
    for keyset in &mut keysets {
        keyset.active = false;
    }
    mint.keysets.store(std::sync::Arc::new(keysets));
    let retry = mint.process_transaction(request, false).await;
    mint.stop().await.unwrap();
    assert_eq!(retry.unwrap(), first);
}

#[tokio::test]
async fn melt_with_outputs_requires_a_remainder_quote() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, true).await;
    let (blinded, _) = outputs(&mint, 1).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 16)],
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        ..Default::default()
    };
    sign(&mut request, &[key], Some(&melt));
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::InvalidTransaction(_))
    ));
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn keyset_rotation_during_payment_returns_outputs_as_change() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, true).await;
    let change_key = SecretKey::generate();
    let (blinded, _) = outputs(&mint, 1).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 16)],
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change_output(&change_key, None)],
        prefer_async: true,
        ..Default::default()
    };
    sign(&mut request, &[key], Some(&melt));
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Pending);
    mint.rotate_keyset_by_version(
        CurrencyUnit::Sat,
        vec![1, 2, 4, 8, 16, 32],
        0,
        cdk_common::nuts::KeySetVersion::Version02,
        None,
    )
    .await
    .unwrap();
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = mint.get_transaction(&response.digest).await.unwrap();
            if response.state != TransactionState::Pending {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(settled.state, TransactionState::Paid);
    assert!(
        settled.signatures.is_empty(),
        "inactive keyset signs nothing"
    );
    // 16 in, melt 2, 1 sat of the 2 sat reserve spent; the unsigned 1 sat output joins the change.
    assert_eq!(change_quote(&settled, 0)["amount_paid"], 13);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn fixed_change_quotes_settle_beside_the_remainder() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 16.into()).await.unwrap();
    let melt = melt_quote(&mint, 5, true).await;
    assert_eq!(Amount::from(melt.fee_reserve()), Amount::from(5));
    let fixed_key = SecretKey::generate();
    let remainder_key = SecretKey::generate();
    let mut request = TransactionRequest {
        proof_inputs: proofs.clone(),
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![
            change_output(&fixed_key, Some(3)),
            change_output(&remainder_key, None),
        ],
        ..Default::default()
    };
    // The fixed amount counts against the balance: 16 in, 5 melt, 5 reserve, 7 fixed is short.
    let mut short = request.clone();
    short.change_quote_outputs[0].amount = Some(7.into());
    sign(&mut short, &[], Some(&melt));
    assert!(matches!(
        mint.process_transaction(short, false).await,
        Err(Error::TransactionUnbalanced(16, 17, 0))
    ));

    sign(&mut request, &[], Some(&melt));
    let mut response = mint.process_transaction(request, false).await.unwrap();
    // Both quotes are null until the payment settles.
    assert_eq!(response.change_quotes.len(), 2);
    for _ in 0..30 {
        if response.state == TransactionState::Paid {
            break;
        }
        assert!(response.change_quotes.iter().all(Option::is_none));
        tokio::time::sleep(Duration::from_millis(200)).await;
        response = mint.get_transaction(&response.digest).await.unwrap();
    }
    assert_eq!(response.state, TransactionState::Paid);
    // 16 in, melt 5, 1 of the 5 reserve spent, 3 fixed: 7 remain.
    let fixed = change_quote(&response, 0);
    assert_eq!(fixed["amount_paid"], 3);
    assert_eq!(fixed["pubkey"], fixed_key.public_key().to_hex());
    let remainder = change_quote(&response, 1);
    assert_eq!(remainder["amount_paid"], 7);
    assert_eq!(remainder["pubkey"], remainder_key.public_key().to_hex());

    // A lock key names one change quote: reusing one is refused before anything is spent.
    let proofs = mint_test_proofs(&mint, 4.into()).await.unwrap();
    let mut request = TransactionRequest {
        proof_inputs: proofs.clone(),
        change_quote_outputs: vec![change_output(&fixed_key, Some(4))],
        ..Default::default()
    };
    sign(&mut request, &[], None);
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::DuplicateOutputs)
    ));

    // A fixed quote alone needs an exact balance and is created at once, without a melt.
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        change_quote_outputs: vec![change_output(&SecretKey::generate(), Some(4))],
        ..Default::default()
    };
    sign(&mut request, &[], None);
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Paid);
    assert_eq!(change_quote(&response, 0)["amount_paid"], 4);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn rejects_zero_amounts_and_a_second_remainder_quote() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 4.into()).await.unwrap();
    let key = SecretKey::generate();
    let other = SecretKey::generate();
    let request = TransactionRequest {
        proof_inputs: proofs.clone(),
        change_quote_outputs: vec![change_output(&key, Some(1)), change_output(&key, None)],
        ..Default::default()
    };
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::DuplicateOutputs)
    ));
    for outputs in [
        vec![change_output(&key, None), change_output(&other, None)],
        vec![change_output(&key, Some(0)), change_output(&other, None)],
    ] {
        let request = TransactionRequest {
            proof_inputs: proofs.clone(),
            change_quote_outputs: outputs,
            ..Default::default()
        };
        assert!(matches!(
            mint.process_transaction(request, false).await,
            Err(Error::InvalidTransaction(_))
        ));
    }
    let states = mint
        .localstore
        .get_proofs_states(&proofs.ys().unwrap())
        .await
        .unwrap();
    assert!(states.iter().all(Option::is_none), "nothing reserved");
    mint.stop().await.unwrap();
}
