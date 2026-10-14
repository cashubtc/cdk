//! NUT-XX transaction tests against a v3 test mint with the fake payment backend.

use std::str::FromStr;
use std::time::Duration;

use cdk_common::amount::SplitTarget;
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::mint::{MeltQuote, TransactionRecord};
use cdk_common::nuts::nut10::nutroot::{self, Transaction, Witness as NutrootWitness};
use cdk_common::nuts::{
    CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteBolt11Request, MintQuoteState,
    MintRequest, PreMintSecrets, SecretKey, State, TransactionChangeOutput, TransactionMeltOutput,
    TransactionQuoteInput, TransactionRequest, TransactionState, Witness,
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
    let section = nutroot::input_section(&request.proof_inputs, &mint_quotes).unwrap();
    let change: Vec<nutroot::ChangeOutput> = request
        .change_quote_outputs
        .iter()
        .enumerate()
        .map(|(k, output)| nutroot::ChangeOutput {
            amount: output.amount,
            pubkey: *cdk_common::PublicKey::from_hex(&output.pubkey)
                .unwrap()
                .as_secp256k1()
                .unwrap(),
            id: match output.id_preimage {
                Some(_) => derived_id(output).to_string(),
                None => nutroot::input_bound_quote_id(&section, k as u32),
            },
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

/// A change quote output locked to `key` with a fresh id; a remainder quote when `amount` is `None`.
fn change_output(key: &SecretKey, amount: Option<u64>) -> TransactionChangeOutput {
    TransactionChangeOutput {
        id_preimage: Some(SecretKey::generate().to_secret_hex()),
        pubkey: key.public_key().to_hex(),
        amount: amount.map(Into::into),
    }
}

/// The derived id a change quote output takes.
fn derived_id(output: &TransactionChangeOutput) -> QuoteId {
    let preimage: [u8; 32] = cdk_common::util::hex::decode(output.id_preimage.as_ref().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    QuoteId::from_str(&nutroot::derived_quote_id(&preimage)).unwrap()
}

/// Poll a melt transaction, as a wallet polls its melt quote, until it leaves `PENDING`.
async fn settled(
    mint: &Mint,
    melt: &MeltQuote,
    request: &TransactionRequest,
) -> cdk_common::transaction::TransactionResponse {
    let ids: Vec<QuoteId> = request
        .change_quote_outputs
        .iter()
        .map(derived_id)
        .collect();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = mint
                .melt_transaction_response(&melt.id, &ids)
                .await
                .unwrap();
            if response.state != TransactionState::Pending {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap()
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
    assert_eq!(change["request"], "");
    assert_eq!(change["expiry"], serde_json::Value::Null);
    assert_eq!(
        change["quote"],
        derived_id(&request.change_quote_outputs[0]).to_string()
    );
    let id_preimage = request.change_quote_outputs[0]
        .id_preimage
        .as_ref()
        .unwrap();
    assert!(!serde_json::to_string(&response)
        .unwrap()
        .contains(id_preimage));
    let states = mint.localstore.get_proofs_states(&ys).await.unwrap();
    assert!(states.iter().all(|s| *s == Some(State::Spent)));

    // A resend is refused: its inputs are spent and its change quote id is claimed.
    assert!(mint.process_transaction(request, false).await.is_err());
    assert!(mint
        .localstore
        .get_mint_quote_by_request("")
        .await
        .unwrap()
        .is_none());
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
            issued: Amount::ZERO,
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
async fn change_quote_without_id_preimage_takes_an_input_bound_id() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 8.into()).await.unwrap();
    let (blinded, _) = outputs(&mint, 2).await;
    let remainder_key = SecretKey::generate();
    let remainder = TransactionChangeOutput {
        id_preimage: None,
        ..change_output(&remainder_key, None)
    };
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        blinded_outputs: blinded,
        change_quote_outputs: vec![change_output(&SecretKey::generate(), Some(3)), remainder],
        ..Default::default()
    };
    // k counts every change quote output, so the remainder is at 1.
    let section = nutroot::input_section(&request.proof_inputs, &[]).unwrap();
    let id = QuoteId::from_str(&nutroot::input_bound_quote_id(&section, 1)).unwrap();
    assert_ne!(id.to_string(), nutroot::input_bound_quote_id(&section, 0));
    sign(&mut request, &[], None);

    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Paid);
    assert_eq!(change_quote(&response, 0)["amount_paid"], 3);
    let quote = change_quote(&response, 1);
    assert_eq!(quote["quote"], id.to_string());
    assert_eq!(quote["amount_paid"], 3);
    let checked = mint
        .check_mint_quotes(std::slice::from_ref(&id))
        .await
        .unwrap();
    assert_eq!(checked[0].state(), Some(MintQuoteState::Paid));
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

    // Unbalanced without a change key.
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
    assert_eq!(response.melt_quotes[0]["change"], serde_json::json!([]));
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
    let response = mint
        .process_transaction(request.clone(), false)
        .await
        .unwrap();
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

    // The released inputs and change quote id spend again; a zero remainder creates no quote.
    let change = request.change_quote_outputs.clone();
    let (blinded, _) = outputs(&mint, 8).await;
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        mint_quote_inputs: vec![quote_input(&quote_id, 4)],
        blinded_outputs: blinded,
        change_quote_outputs: change.clone(),
        ..Default::default()
    };
    sign(&mut request, std::slice::from_ref(&key), None);
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Paid);
    assert_eq!(response.change_quotes, vec![None]);
    // Fetching the uncreated remainder answers quote not found (20010).
    let err = mint
        .check_mint_quotes(&[derived_id(&change[0])])
        .await
        .unwrap_err();
    assert!(matches!(err, Error::UnknownQuote));
    assert_eq!(
        cdk_common::error::ErrorResponse::from(err).code.to_code(),
        20010
    );

    // The uncreated remainder's id stays claimed.
    let (quote_id, quote_key) = paid_locked_quote(&mint, 2).await;
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 2)],
        change_quote_outputs: change,
        ..Default::default()
    };
    sign(&mut request, &[quote_key], None);
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::DuplicateOutputs)
    ));
    drop(key);
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
    let response = mint
        .process_transaction(request.clone(), false)
        .await
        .unwrap();
    assert!(matches!(
        response.state,
        TransactionState::Pending | TransactionState::Paid
    ));
    let settled = settled(&mint, &melt, &request).await;
    assert_eq!(settled.state, TransactionState::Paid);
    assert_eq!(settled.signatures.len(), 1);
    // 16 in, 1 out, 5 melt, 1 of the reserve spent: 9 back.
    assert_eq!(change_quote(&settled, 0)["amount_paid"], 9);
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
async fn nut04_mint_refuses_a_witness_over_a_stale_issued() {
    // A change quote of 8 (bolt11 NUT-04 mints only the full amount here), then a transaction
    // draws 3 from it, so a NUT-04 mint of the rest must sign it at issued 3.
    let mint = v3_mint().await;
    let (funding, funding_key) = paid_locked_quote(&mint, 8).await;
    let key = SecretKey::generate();
    let change = change_output(&key, None);
    let quote_id = derived_id(&change);
    let mut fund = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&funding, 8)],
        change_quote_outputs: vec![change],
        ..Default::default()
    };
    sign(&mut fund, &[funding_key], None);
    let prepared = mint.prepare_transaction(fund).await.unwrap();
    mint.settle_without_melt(prepared).await.unwrap();
    let mut draw = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&quote_id, 3)],
        change_quote_outputs: vec![change_output(&SecretKey::generate(), Some(3))],
        ..Default::default()
    };
    sign(&mut draw, std::slice::from_ref(&key), None);
    let prepared = mint.prepare_transaction(draw).await.unwrap();
    mint.settle_without_melt(prepared).await.unwrap();
    let (blinded, _) = outputs(&mint, 5).await;
    let request_at = |issued: u64| {
        let transaction = Transaction::new(
            &[],
            &[nutroot::MintQuoteInput {
                id: quote_id.to_string(),
                amount: 5.into(),
                pubkey: key.public_key(),
                issued: issued.into(),
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
        MintInput::Single(MintRequest {
            quote: quote_id.clone(),
            outputs: blinded.clone(),
            signature: Some(signature),
        })
    };
    assert!(mint.process_mint_request(request_at(0)).await.is_err());
    mint.process_mint_request(request_at(3)).await.unwrap();
    let quote = mint
        .localstore
        .get_mint_quote(&quote_id)
        .await
        .unwrap()
        .unwrap();
    mint.stop().await.unwrap();
    assert_eq!(Amount::from(quote.amount_issued()), Amount::from(8));
}

#[tokio::test]
async fn change_quote_ids_are_claimed_at_acceptance() {
    // Also cover a fee consuming all value, which leaves a zero remainder.
    for fee_ppk in [0, 2000] {
        let mint = v3_mint().await;
        let mut info = mint.mint_info().await.unwrap();
        info.nuts.nutxx.quote_input_fee_ppk = fee_ppk;
        mint.set_mint_info(info).await.unwrap();
        let (id, key) = paid_locked_quote(&mint, 8).await;
        let (other_id, other_key) = paid_locked_quote(&mint, 8).await;
        let change_key = SecretKey::generate();
        let mut request = TransactionRequest {
            mint_quote_inputs: vec![quote_input(&id, 2)],
            change_quote_outputs: vec![change_output(&change_key, None)],
            ..Default::default()
        };
        // The same change quote id, drawn from another quote.
        let mut other = TransactionRequest {
            mint_quote_inputs: vec![quote_input(&other_id, 2)],
            ..request.clone()
        };
        sign(&mut request, &[key], None);
        sign(&mut other, &[other_key], None);
        // Both requests validate before either claims its change quote id.
        let first = mint.prepare_transaction(request).await.unwrap();
        let second = mint.prepare_transaction(other).await.unwrap();
        assert_eq!(
            mint.settle_without_melt(first).await.unwrap().state,
            TransactionState::Paid
        );
        assert!(matches!(
            mint.settle_without_melt(second).await,
            Err(Error::DuplicateOutputs)
        ));
        let quote = mint.localstore.get_mint_quote(&id).await.unwrap().unwrap();
        mint.stop().await.unwrap();
        assert_eq!(Amount::from(quote.amount_issued()), Amount::from(2));
    }
}

#[tokio::test]
async fn failed_melt_retries_with_its_quote_inputs_and_change_id() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, false).await;
    let change = change_output(&SecretKey::generate(), None);
    let mut results = Vec::new();
    for _ in 0..2 {
        let mut request = TransactionRequest {
            mint_quote_inputs: vec![quote_input(&id, 8)],
            melt_quote_outputs: vec![melt_output(&melt)],
            change_quote_outputs: vec![change.clone()],
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
        mint.process_transaction(request.clone(), false),
    )
    .await;
    let response = response
        .expect("prefer_async must return before payment completes")
        .unwrap();
    assert_eq!(response.state, TransactionState::Pending);
    assert_eq!(response.change_quotes, vec![None]);
    let settled = settled(&mint, &melt, &request).await;
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
async fn melt_with_outputs_requires_a_remainder_quote() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, true).await;
    let (blinded, _) = outputs(&mint, 1).await;
    let (mut blank, _) = outputs(&mint, 1).await;
    blank[0].amount = Amount::ZERO;
    for blinded in [blinded, blank] {
        let mut request = TransactionRequest {
            mint_quote_inputs: vec![quote_input(&id, 16)],
            blinded_outputs: blinded,
            melt_quote_outputs: vec![melt_output(&melt)],
            ..Default::default()
        };
        sign(&mut request, std::slice::from_ref(&key), Some(&melt));
        assert!(matches!(
            mint.process_transaction(request, false).await,
            Err(Error::InvalidTransaction(_))
        ));
    }
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn keyset_rotation_during_payment_returns_outputs_as_change() {
    let mint = v3_mint().await;
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, true).await;
    let change_key = SecretKey::generate();
    // A blank output beside the fixed one is not signed either.
    let (mut blinded, _) = outputs(&mint, 1).await;
    let (mut blank, _) = outputs(&mint, 1).await;
    blank[0].amount = Amount::ZERO;
    blinded.append(&mut blank);
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 16)],
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change_output(&change_key, None)],
        prefer_async: true,
        ..Default::default()
    };
    sign(&mut request, &[key], Some(&melt));
    let response = mint
        .process_transaction(request.clone(), false)
        .await
        .unwrap();
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
    let settled = settled(&mint, &melt, &request).await;
    assert_eq!(settled.state, TransactionState::Paid);
    assert!(
        settled.signatures.is_empty(),
        "inactive keyset signs nothing"
    );
    // The melt quote lists the outputs' signatures, here none, in every view.
    assert_eq!(settled.melt_quotes[0]["change"], serde_json::json!([]));
    let checked =
        cdk_common::transaction::melt_quote_json(mint.check_melt_quote(&melt.id).await.unwrap())
            .unwrap();
    assert_eq!(checked["change"], serde_json::json!([]));
    // 16 in, melt 2, 1 sat of the 2 sat reserve spent; the unsigned 1 sat output joins the change.
    assert_eq!(change_quote(&settled, 0)["amount_paid"], 13);
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn blank_outputs_take_the_change_before_the_remainder_quote() {
    let mint = v3_mint().await;
    // 16 in, 1 fixed, melt 2, 1 sat of the 2 sat reserve spent: change 12, imprinted as 8 and 4
    // into the earliest blanks in request order, wherever the fixed output sits.
    for (blank_count, fixed_at, signed, remainder) in [
        (3, 1, vec![8, 1, 4], None),
        (3, 3, vec![8, 4, 1], None),
        (1, 1, vec![8, 1], Some(4)),
    ] {
        let (id, key) = paid_locked_quote(&mint, 16).await;
        let melt = melt_quote(&mint, 2, true).await;
        let (mut blinded, _) = outputs(&mint, 7).await;
        blinded.truncate(blank_count);
        for output in &mut blinded {
            output.amount = Amount::ZERO;
        }
        let (fixed, _) = outputs(&mint, 1).await;
        blinded.insert(fixed_at, fixed[0].clone());
        let mut request = TransactionRequest {
            mint_quote_inputs: vec![quote_input(&id, 16)],
            blinded_outputs: blinded,
            melt_quote_outputs: vec![melt_output(&melt)],
            change_quote_outputs: vec![change_output(&SecretKey::generate(), None)],
            ..Default::default()
        };
        sign(&mut request, &[key], Some(&melt));
        mint.process_transaction(request.clone(), false)
            .await
            .unwrap();
        let response = settled(&mint, &melt, &request).await;
        assert_eq!(response.state, TransactionState::Paid);
        let amounts: Vec<u64> = response
            .signatures
            .iter()
            .map(|sig| u64::from(sig.amount))
            .collect();
        assert_eq!(amounts, signed);
        assert_eq!(
            response.melt_quotes[0]["change"],
            serde_json::to_value(&response.signatures).unwrap()
        );
        match remainder {
            Some(amount) => assert_eq!(change_quote(&response, 0)["amount_paid"], amount),
            // A zero remainder creates no quote.
            None => assert!(response.change_quotes[0].is_none()),
        }
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
    let (id, key) = paid_locked_quote(&mint, 16).await;
    let melt = melt_quote(&mint, 2, true).await;
    let (mut blinded, _) = outputs(&mint, 7).await;
    for output in &mut blinded {
        output.amount = Amount::ZERO;
    }
    let (fixed, _) = outputs(&mint, 2).await;
    blinded.extend(fixed);
    let mut request = TransactionRequest {
        mint_quote_inputs: vec![quote_input(&id, 16)],
        blinded_outputs: blinded,
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change_output(&SecretKey::generate(), None)],
        ..Default::default()
    };
    sign(&mut request, &[key], Some(&melt));
    mint.process_transaction(request.clone(), false)
        .await
        .unwrap();
    let response = settled(&mint, &melt, &request).await;
    assert_eq!(response.state, TransactionState::Paid);
    // 16 in, 2 fixed, melt 2, 1 sat of the 2 sat reserve spent: change 11, the blanks take 8 and 2.
    let amounts: Vec<u64> = response
        .signatures
        .iter()
        .map(|sig| u64::from(sig.amount))
        .collect();
    assert_eq!(amounts, vec![8, 2, 2]);
    assert_eq!(change_quote(&response, 0)["amount_paid"], 1);
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
    let response = mint
        .process_transaction(request.clone(), false)
        .await
        .unwrap();
    // Both quotes are null until the payment settles.
    assert_eq!(response.change_quotes.len(), 2);
    if response.state == TransactionState::Pending {
        assert!(response.change_quotes.iter().all(Option::is_none));
    }
    let response = settled(&mint, &melt, &request).await;
    assert_eq!(response.state, TransactionState::Paid);
    // 16 in, melt 5, 1 of the 5 reserve spent, 3 fixed: 7 remain.
    let fixed = change_quote(&response, 0);
    assert_eq!(fixed["amount_paid"], 3);
    assert_eq!(fixed["pubkey"], fixed_key.public_key().to_hex());
    let remainder = change_quote(&response, 1);
    assert_eq!(remainder["amount_paid"], 7);
    assert_eq!(remainder["pubkey"], remainder_key.public_key().to_hex());

    // A change quote id names one quote: naming a claimed one is refused before anything is spent.
    let claimed = request.change_quote_outputs[0].clone();
    let proofs = mint_test_proofs(&mint, 4.into()).await.unwrap();
    let mut request = TransactionRequest {
        proof_inputs: proofs.clone(),
        change_quote_outputs: vec![TransactionChangeOutput {
            amount: Some(4.into()),
            ..claimed
        }],
        ..Default::default()
    };
    sign(&mut request, &[], None);
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::DuplicateOutputs)
    ));

    // A fixed quote alone needs an exact balance and is created at once, without a melt; its
    // lock key may repeat under a fresh id.
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        change_quote_outputs: vec![change_output(&fixed_key, Some(4))],
        ..Default::default()
    };
    sign(&mut request, &[], None);
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Paid);
    assert_eq!(change_quote(&response, 0)["amount_paid"], 4);
    assert_eq!(
        change_quote(&response, 0)["pubkey"],
        fixed_key.public_key().to_hex()
    );
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn rejects_zero_amounts_and_a_second_remainder_quote() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 4.into()).await.unwrap();
    let key = SecretKey::generate();
    let other = SecretKey::generate();
    let fixed = change_output(&key, Some(1));
    let repeated = TransactionChangeOutput {
        amount: None,
        ..fixed.clone()
    };
    let request = TransactionRequest {
        proof_inputs: proofs.clone(),
        change_quote_outputs: vec![fixed, repeated],
        ..Default::default()
    };
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::DuplicateOutputs)
    ));
    let mut short_preimage = change_output(&key, None);
    short_preimage.id_preimage.as_mut().unwrap().truncate(62);
    let mut not_hex = change_output(&key, None);
    not_hex
        .id_preimage
        .as_mut()
        .unwrap()
        .replace_range(..2, "zz");
    for outputs in [
        vec![change_output(&key, None), change_output(&other, None)],
        vec![change_output(&key, Some(0)), change_output(&other, None)],
        vec![short_preimage],
        vec![not_hex],
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
        change_outputs: vec![],
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
        change_outputs: vec![],
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
async fn change_mint_quote_subscription_waits_for_the_quote() {
    use cdk_common::nut17::{Kind, Params};
    use cdk_common::NotificationPayload;

    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 8.into()).await.unwrap();
    let change = change_output(&SecretKey::generate(), None);
    let id = derived_id(&change);
    // Accepted for an id the mint does not hold yet, with no initial state.
    let mut subscription = mint
        .pubsub_manager()
        .subscribe(Params {
            kind: Kind::Custom("change_mint_quote".to_owned()),
            filters: vec![id.to_string()],
            id: std::sync::Arc::new(cdk_common::subscription::SubId::from("change")),
        })
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), subscription.recv())
            .await
            .is_err()
    );

    let mut request = TransactionRequest {
        proof_inputs: proofs,
        change_quote_outputs: vec![change],
        ..Default::default()
    };
    sign(&mut request, &[], None);
    mint.process_transaction(request, false).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), subscription.recv())
        .await
        .unwrap()
        .unwrap();
    match event.inner() {
        NotificationPayload::CustomMintQuoteResponse(method, response) => {
            assert_eq!(method, "change");
            assert_eq!(response.quote, id);
            assert_eq!(response.amount_paid, Amount::from(8));
        }
        payload => panic!("unexpected payload: {payload:?}"),
    }
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
        change_outputs: vec![],
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
async fn melt_transaction_change_quote_event_follows_settlement() {
    use cdk_common::nut17::{Kind, Params};
    use cdk_common::NotificationPayload;

    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 16.into()).await.unwrap();
    let melt = melt_quote(&mint, 5, true).await;
    let change = change_output(&SecretKey::generate(), None);
    let id = derived_id(&change);
    let mut subscription = mint
        .pubsub_manager()
        .subscribe(Params {
            kind: Kind::Custom("change_mint_quote".to_owned()),
            filters: vec![id.to_string()],
            id: std::sync::Arc::new(cdk_common::subscription::SubId::from("melt-change")),
        })
        .unwrap();
    let mut melt_subscription = mint
        .pubsub_manager()
        .subscribe(Params {
            kind: Kind::Bolt11MeltQuote,
            filters: vec![melt.id.to_string()],
            id: std::sync::Arc::new(cdk_common::subscription::SubId::from("melt")),
        })
        .unwrap();
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change],
        ..Default::default()
    };
    sign(&mut request, &[], Some(&melt));
    let pending = tokio::spawn({
        let mint = mint.clone();
        async move { mint.process_transaction(request, false).await }
    });
    let event = tokio::time::timeout(Duration::from_secs(10), subscription.recv())
        .await
        .unwrap()
        .unwrap();
    // The event is announced once the settlement has committed.
    let record = mint
        .localstore
        .get_transaction(&melt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.state, TransactionState::Paid);
    match event.inner() {
        NotificationPayload::CustomMintQuoteResponse(method, response) => {
            assert_eq!(method, "change");
            assert_eq!(response.quote, id);
            // 16 in, 5 melt, 1 of the 5 reserve spent: 10 back.
            assert_eq!(response.amount_paid, Amount::from(10));
        }
        payload => panic!("unexpected payload: {payload:?}"),
    }
    assert_eq!(
        pending.await.unwrap().unwrap().state,
        TransactionState::Paid
    );
    // The PAID melt quote notification lists the outputs' signatures, here none.
    let paid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = melt_subscription.recv().await.unwrap();
            if let NotificationPayload::MeltQuoteBolt11Response(response) = event.inner() {
                if response.state == MeltQuoteState::Paid {
                    break response.clone();
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(paid.change, Some(vec![]));
    mint.stop().await.unwrap();
}

/// Insert a mint quote under a change quote id.
async fn add_quote_under(mint: &Mint, id: &QuoteId) {
    let mut tx = mint.localstore.begin_transaction().await.unwrap();
    tx.add_mint_quote(super::change_quote(
        &CurrencyUnit::Sat,
        id,
        Amount::ONE,
        SecretKey::generate().public_key(),
    ))
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn change_quote_id_naming_a_mint_quote_is_refused_before_payment() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 16.into()).await.unwrap();
    let melt = melt_quote(&mint, 5, true).await;
    let change = change_output(&SecretKey::generate(), None);
    add_quote_under(&mint, &derived_id(&change)).await;

    let mut request = TransactionRequest {
        proof_inputs: proofs.clone(),
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change],
        ..Default::default()
    };
    sign(&mut request, &[], Some(&melt));
    assert!(matches!(
        mint.process_transaction(request, false).await,
        Err(Error::DuplicateOutputs)
    ));
    let stored = mint
        .localstore
        .get_melt_quote(&melt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, MeltQuoteState::Unpaid);
    let states = mint
        .localstore
        .get_proofs_states(&proofs.ys().unwrap())
        .await
        .unwrap();
    assert!(states.iter().all(Option::is_none), "nothing reserved");
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn unfinished_melt_settlement_reads_pending_on_check() {
    let mint = v3_mint().await;
    let proofs = mint_test_proofs(&mint, 16.into()).await.unwrap();
    let melt = melt_quote(&mint, 5, true).await;
    let change = change_output(&SecretKey::generate(), None);
    let id = derived_id(&change);
    let mut request = TransactionRequest {
        proof_inputs: proofs,
        melt_quote_outputs: vec![melt_output(&melt)],
        change_quote_outputs: vec![change],
        prefer_async: true,
        ..Default::default()
    };
    sign(&mut request, &[], Some(&melt));
    let response = mint.process_transaction(request, false).await.unwrap();
    assert_eq!(response.state, TransactionState::Pending);
    // A quote created under the change id while the payment is in flight makes the settlement
    // step fail after the melt is paid.
    add_quote_under(&mint, &id).await;

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
