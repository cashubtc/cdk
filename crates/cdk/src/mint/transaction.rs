//! NUT-XX transactions: proofs and paid mint quotes in; blinded messages, one
//! melt quote and txout quotes out.
//!
//! A transaction without a melt settles in one database transaction. One with
//! a melt runs the melt saga: quote inputs are reserved, fixed txout quote ids
//! claimed and the record stored in the saga's setup transaction, and the shared
//! melt finalization and rollback paths settle or fail the record by its melt quote id.

use std::collections::HashSet;
use std::str::FromStr;

use cdk_common::database::mint::MeltRequestInfo;
use cdk_common::database::{self, DynMintDatabase, DynMintTransaction};
use cdk_common::mint::{MeltQuote, MintQuote, Operation, OperationKind, TransactionRecord};
use cdk_common::nuts::nut10::nutroot::{self, Witness};
use cdk_common::nuts::{
    KeySetVersion, MeltQuoteState, MintQuoteState, SpendingConditionVerification, TransactionState,
};
use cdk_common::payment::PaymentIdentifier;
use cdk_common::transaction::{melt_quote_json, mint_quote_json, TransactionResponse};
use cdk_common::util::unix_time;
use cdk_common::{
    Amount, BlindSignature, BlindedMessage, CurrencyUnit, MeltRequest, PaymentMethod, Proofs,
    ProofsMethods, PublicKey, QuoteId, State, TransactionRequest,
};
use tracing::instrument;

use super::melt::shared::{
    begin_melt_cleanup_transaction, MeltChangeResult, MeltCleanupTransaction,
};
use super::verification::Verification;
use super::{Error, Mint, MintQuoteResponse};

/// Everything the melt saga needs to reserve a transaction alongside its melt.
#[derive(Debug)]
pub(crate) struct TransactionSetup {
    pub record: TransactionRecord,
    /// Each quote input's `amount_issued` as its transcript binds it.
    pub quote_issued: Vec<Amount>,
    /// Present when a v3 proof input needs its spend recorded.
    pub transcript: Option<nutroot::Transaction>,
    pub inputs_amount: Amount<CurrencyUnit>,
    pub inputs_fee: Amount<CurrencyUnit>,
    pub outputs_amount: Amount<CurrencyUnit>,
}

/// Validated request, ready to reserve.
struct Prepared {
    request: TransactionRequest,
    unit: CurrencyUnit,
    transcript: nutroot::Transaction,
    quote_inputs: Vec<(QuoteId, Amount)>,
    quote_issued: Vec<Amount>,
    quote_outputs: Vec<QuoteOutput>,
    melt: Option<MeltQuote>,
    inputs_amount: Amount,
    outputs_amount: Amount,
    fee: Amount,
    excess: Amount,
}

fn invalid(reason: &str) -> Error {
    Error::InvalidTransaction(reason.to_owned())
}

/// Parse a quote input witness: a bare key-path signature or a JSON script-path witness.
pub(crate) fn parse_quote_witness(raw: &str) -> Result<Witness, Error> {
    if raw.len() > 4096 {
        return Err(Error::SignatureMissingOrInvalid);
    }
    if raw.len() == 128 {
        return Ok(Witness {
            signatures: vec![raw.to_owned()],
            leaf: None,
            control: None,
            preimage: None,
        });
    }
    serde_json::from_str(raw).map_err(|_| Error::SignatureMissingOrInvalid)
}

/// A mint quote output: its id, lock key and fixed amount.
type QuoteOutput = (QuoteId, PublicKey, Amount);

/// Parse a fixed txout quote's `id_preimage`: exactly 32 bytes of hex.
fn parse_id_preimage(raw: &str) -> Result<[u8; 32], Error> {
    (raw.len() == 64)
        .then(|| cdk_common::util::hex::decode(raw).ok())
        .flatten()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| invalid("txout quote id_preimage must be 32 bytes of hex"))
}

/// Change on settlement: the excess less what the payment cost, the cost capped at the reserve.
///
/// The balance rule keeps `excess >= fee_reserve`, so this never underflows; a
/// backend fee over the reserve is the mint's loss, as in NUT-08.
pub(crate) fn settlement_change(excess: Amount, fee_reserve: Amount, fee_paid: Amount) -> Amount {
    let charged = fee_paid.min(fee_reserve);
    match excess.checked_sub(charged) {
        Some(change) => change,
        None => {
            tracing::error!(
                excess = %excess,
                fee_reserve = %fee_reserve,
                fee_paid = %fee_paid,
                "transaction excess below the charged fee; returning no change",
            );
            Amount::ZERO
        }
    }
}

fn txout_quote(unit: &CurrencyUnit, id: &QuoteId, amount: Amount, pubkey: PublicKey) -> MintQuote {
    let now = unix_time();
    MintQuote::new(
        Some(id.clone()),
        String::new(),
        unit.clone(),
        Some(amount.with_unit(unit.clone())),
        0,
        PaymentIdentifier::CustomId(txout_payment_id(id)),
        Some(pubkey),
        Amount::ZERO.with_unit(unit.clone()),
        Amount::ZERO.with_unit(unit.clone()),
        PaymentMethod::Custom(cdk_common::nuts::nutxx::TXOUT_METHOD.to_owned()),
        now,
        now,
        vec![],
        vec![],
        None,
    )
}

/// Lookup and payment id of a txout quote.
fn txout_payment_id(id: &QuoteId) -> String {
    format!("txout:{id}")
}

/// Claim a transaction's fixed txout quote ids at acceptance; a claimed id refuses the transaction.
pub(crate) async fn claim_txout_quote_ids(
    tx: &mut DynMintTransaction,
    outputs: &[QuoteOutput],
    melt_quote_id: Option<&QuoteId>,
) -> Result<(), Error> {
    let ids: Vec<QuoteId> = outputs.iter().map(|(id, _, _)| id.clone()).collect();
    tx.claim_mint_quote_ids(&ids, melt_quote_id)
        .await
        .map_err(|err| match err {
            database::Error::Duplicate => Error::DuplicateOutputs,
            err => Error::Database(err),
        })
}

/// Create each fixed txout quote, paid in full on creation.
async fn create_txout_quotes(
    tx: &mut DynMintTransaction,
    unit: &CurrencyUnit,
    outputs: &[QuoteOutput],
) -> Result<Vec<MintQuote>, Error> {
    let mut created = Vec::with_capacity(outputs.len());
    for (id, pubkey, amount) in outputs {
        let mut quote = tx
            .add_mint_quote(txout_quote(unit, id, *amount, *pubkey))
            .await?;
        quote.add_payment(amount.with_unit(unit.clone()), txout_payment_id(id), None)?;
        tx.update_mint_quote(&mut quote).await?;
        created.push(quote.inner());
    }
    Ok(created)
}

/// Txout quote responses in output order, `None` where no quote exists yet.
fn txout_quote_responses(
    quotes: Vec<Option<MintQuote>>,
) -> Result<Vec<Option<serde_json::Value>>, Error> {
    quotes
        .into_iter()
        .map(|quote| {
            quote
                .map(|quote| mint_quote_json(MintQuoteResponse::<QuoteId>::try_from(quote)?))
                .transpose()
        })
        .collect()
}

/// A melt quote paying a transaction reads `PENDING` until the whole transaction settles.
///
/// Returns whether the quote pays a NUT-XX transaction.
pub(crate) async fn settled_melt_state(
    db: &DynMintDatabase,
    quote: &mut MeltQuote,
) -> Result<bool, Error> {
    let state = db
        .get_transaction(&quote.id)
        .await?
        .map(|record| record.state);
    if quote.state == MeltQuoteState::Paid && state == Some(TransactionState::Pending) {
        quote.state = MeltQuoteState::Pending;
    }
    Ok(state.is_some_and(|state| state != TransactionState::Failed))
}

/// A melt quote's `change`; a PAID quote paying a transaction lists its outputs' signatures, even none.
pub(crate) fn melt_change(
    pays_transaction: bool,
    quote: &MeltQuote,
    signatures: Vec<BlindSignature>,
) -> Option<Vec<BlindSignature>> {
    (!signatures.is_empty() || (pays_transaction && quote.state == MeltQuoteState::Paid))
        .then_some(signatures)
}

/// Refuse a draw on a quote that a pending transaction draws on (NUT-XX); call under the quote locks.
pub(crate) async fn refuse_pending_quotes(
    tx: &mut DynMintTransaction,
    ids: &[QuoteId],
) -> Result<(), Error> {
    if ids.is_empty() {
        return Ok(());
    }
    // ponytail: scans every pending transaction; a quote-input index if many melts are in flight at once.
    let pending = tx.get_pending_transactions().await?;
    if pending
        .iter()
        .flat_map(|record| &record.quote_inputs)
        .any(|(id, _)| ids.contains(id))
    {
        return Err(Error::PendingQuote);
    }
    Ok(())
}

/// Check each quote input under the quote row locks, and issue it when `issue` is set.
///
/// A melt transaction issues only at settlement ([`issue_quote_inputs`]); until then a pending
/// quote takes no other draw, so `amount_issued` is always the settled value a transcript binds.
pub(crate) async fn reserve_quote_inputs(
    tx: &mut DynMintTransaction,
    unit: &CurrencyUnit,
    quote_inputs: &[(QuoteId, Amount)],
    quote_issued: &[Amount],
    issue: bool,
) -> Result<Vec<MintQuote>, Error> {
    if quote_inputs.is_empty() {
        return Ok(vec![]);
    }
    let ids: Vec<QuoteId> = quote_inputs.iter().map(|(id, _)| id.clone()).collect();
    let quotes = tx.get_mint_quotes_by_ids(&ids).await?;
    refuse_pending_quotes(tx, &ids).await?;
    let mut issued = Vec::with_capacity(ids.len());
    for (((_, amount), bound), quote) in quote_inputs.iter().zip(quote_issued).zip(quotes) {
        let mut quote = quote.ok_or(Error::UnknownQuote)?;
        match quote.state() {
            MintQuoteState::Paid => {}
            MintQuoteState::Unpaid => return Err(Error::UnpaidQuote),
            MintQuoteState::Issued => return Err(Error::IssuedQuote),
        }
        // The witness signed this state; a draw settled since makes it stale.
        if Amount::from(quote.amount_issued()) != *bound {
            return Err(Error::SignatureMissingOrInvalid);
        }
        let amount = amount.with_unit(unit.clone());
        if amount > quote.amount_mintable() {
            return Err(invalid("quote input exceeds the quote's mintable amount"));
        }
        if issue {
            quote.add_issuance(amount)?;
            tx.update_mint_quote(&mut quote).await?;
        }
        issued.push(quote.inner());
    }
    Ok(issued)
}

/// Issue a settled melt transaction's quote inputs.
async fn issue_quote_inputs(
    tx: &mut DynMintTransaction,
    record: &TransactionRecord,
) -> Result<Vec<MintQuote>, Error> {
    if record.quote_inputs.is_empty() {
        return Ok(vec![]);
    }
    let ids: Vec<QuoteId> = record
        .quote_inputs
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    let quotes = tx.get_mint_quotes_by_ids(&ids).await?;
    let mut issued = Vec::with_capacity(ids.len());
    for ((_, amount), quote) in record.quote_inputs.iter().zip(quotes) {
        let mut quote = quote.ok_or(Error::UnknownQuote)?;
        quote.add_issuance(amount.with_unit(record.unit.clone()))?;
        tx.update_mint_quote(&mut quote).await?;
        issued.push(quote.inner());
    }
    Ok(issued)
}

/// Sign the reserved outputs, create the txout quotes and mark the record paid.
///
/// Runs in the melt finalization's second transaction, in place of the change
/// signing of a plain melt. Blank outputs take the change as in NUT-08. The melt quote is already paid.
pub(crate) async fn settle_with_melt(
    mint: &Mint,
    db: &DynMintDatabase,
    quote: &MeltQuote,
    melt_request_info: &MeltRequestInfo,
    total_spent: Amount<CurrencyUnit>,
) -> Result<MeltChangeResult, Error> {
    // The change is imprinted into blank outputs before signing, outside the cleanup transaction.
    let Some(excess) = db.get_transaction(&quote.id).await?.map(|r| r.excess) else {
        return Ok(MeltChangeResult::AlreadyCompleted);
    };
    let fee_paid: Amount = total_spent
        .checked_sub(&quote.amount())
        .map(Into::into)
        .unwrap_or(Amount::ZERO);

    let mut outputs = melt_request_info.change_outputs.clone();
    let change = settlement_change(excess, quote.fee_reserve().into(), fee_paid);
    if outputs.iter().any(|o| o.amount == Amount::ZERO) {
        let mut left = u64::from(change);
        // Each blank in request order takes the largest amount of its own keyset (active or not)
        // that fits; one left at zero is not signed (NUT-08), and what is left stays with the mint.
        outputs.retain_mut(|output| {
            if output.amount != Amount::ZERO {
                return true;
            }
            let denominations = mint
                .get_keyset_info(&output.keyset_id)
                .map(|keyset| keyset.amounts)
                .unwrap_or_default();
            match denominations.iter().filter(|&&d| d > 0 && d <= left).max() {
                Some(&amount) => {
                    output.amount = Amount::from(amount);
                    left -= amount;
                    true
                }
                None => false,
            }
        });
    }
    let signatures = if outputs.is_empty() {
        vec![]
    } else {
        mint.blind_sign(outputs.clone()).await?
    };

    let mut tx = match begin_melt_cleanup_transaction(db, &quote.id).await? {
        MeltCleanupTransaction::Ready(tx) => tx,
        MeltCleanupTransaction::AlreadyCompleted => return Ok(MeltChangeResult::AlreadyCompleted),
    };
    let Some(mut record) = tx.get_transaction_by_melt_quote(&quote.id).await? else {
        tx.rollback().await?;
        return Ok(MeltChangeResult::AlreadyCompleted);
    };
    if record.state != TransactionState::Pending {
        tx.rollback().await?;
        return Ok(MeltChangeResult::AlreadyCompleted);
    }
    if !outputs.is_empty() {
        let secrets: Vec<PublicKey> = outputs.iter().map(|o| o.blinded_secret).collect();
        tx.add_blind_signatures(&secrets, &signatures, Some(quote.id.clone()))
            .await?;
    }

    issue_quote_inputs(&mut tx, &record).await?;
    let created = create_txout_quotes(&mut tx, &record.unit, &record.mint_quote_outputs).await?;
    tx.update_transaction(&mut record, TransactionState::Paid)
        .await?;
    tracing::info!(
        melt_quote_id = %quote.id,
        change = %change,
        "transaction settled with melt",
    );
    Ok(MeltChangeResult::Ready {
        change_sigs: Some(signatures),
        txout_quotes: created,
        tx,
    })
}

impl Mint {
    /// Run a NUT-XX transaction, waiting for a melt to settle unless `respond_async`.
    #[instrument(skip_all)]
    pub async fn process_transaction(
        &self,
        request: TransactionRequest,
        respond_async: bool,
    ) -> Result<TransactionResponse, Error> {
        let prepared = self.prepare_transaction(request).await?;
        let Some(melt) = prepared.melt.clone() else {
            return self.settle_without_melt(prepared).await;
        };
        let respond_async = respond_async || prepared.request.prefer_async;
        let quote_ids: Vec<QuoteId> = prepared
            .quote_outputs
            .iter()
            .map(|(id, _, _)| id.clone())
            .collect();
        let pending = self.melt_transaction(prepared, melt.clone()).await?;
        if !respond_async {
            // The outcome is read from the melt quote; a failed payment is FAILED there.
            if let Err(err) = pending.await {
                tracing::info!(melt_quote_id = %melt.id, "transaction melt did not settle: {err}");
            }
        }
        self.melt_transaction_response(&melt.id, &quote_ids).await
    }

    /// The response for a transaction with a melt, read from its melt quote and txout quotes.
    async fn melt_transaction_response(
        &self,
        melt_quote_id: &QuoteId,
        quote_ids: &[QuoteId],
    ) -> Result<TransactionResponse, Error> {
        let mut quote = self
            .localstore
            .get_melt_quote(melt_quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;
        settled_melt_state(&self.localstore, &mut quote).await?;
        let state = match quote.state {
            MeltQuoteState::Paid => TransactionState::Paid,
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => TransactionState::Failed,
            MeltQuoteState::Pending | MeltQuoteState::Unknown => TransactionState::Pending,
        };
        let (signatures, mint_quotes) = if state == TransactionState::Paid {
            let quotes = if quote_ids.is_empty() {
                vec![]
            } else {
                self.localstore.get_mint_quotes_by_ids(quote_ids).await?
            };
            (
                self.localstore
                    .get_blind_signatures_for_quote(melt_quote_id)
                    .await?,
                txout_quote_responses(quotes)?,
            )
        } else {
            (vec![], vec![None; quote_ids.len()])
        };
        let change = (state == TransactionState::Paid).then(|| signatures.clone());
        Ok(TransactionResponse {
            state,
            signatures,
            melt_quotes: vec![melt_quote_json(quote.into_response(change))?],
            mint_quotes,
        })
    }

    /// Validate the request against mint and quote state and build its transcript.
    async fn prepare_transaction(&self, request: TransactionRequest) -> Result<Prepared, Error> {
        let proofs = &request.proof_inputs;
        let outputs = &request.blinded_outputs;
        if proofs.is_empty() && request.mint_quote_inputs.is_empty() {
            return Err(invalid("transaction requires at least one input"));
        }
        let mut quote_requests = Vec::with_capacity(request.mint_quote_outputs.len());
        for output in &request.mint_quote_outputs {
            let id_preimage = output
                .id_preimage
                .as_deref()
                .map(parse_id_preimage)
                .transpose()?;
            let pubkey = PublicKey::from_hex(&output.pubkey)
                .ok()
                .filter(|key| key.as_secp256k1().is_ok())
                .ok_or_else(|| {
                    invalid("mint quote output pubkey is not a compressed secp256k1 key")
                })?;
            if output.amount == Amount::ZERO {
                return Err(invalid("txout quote amount must be positive"));
            }
            quote_requests.push((id_preimage, pubkey, output.amount));
        }
        let has_fixed = !quote_requests.is_empty();
        if outputs.is_empty() && request.melt_quote_outputs.is_empty() && !has_fixed {
            return Err(invalid("transaction requires at least one output"));
        }
        // Quote inputs count with proofs, and both caps apply before any quote is loaded.
        let inputs_count = proofs.len() + request.mint_quote_inputs.len();
        if inputs_count > self.max_inputs {
            return Err(Error::MaxInputsExceeded {
                actual: inputs_count,
                max: self.max_inputs,
            });
        }
        // Txout quotes count with blinded outputs: each is a claim and a quote row.
        let outputs_count = outputs.len() + request.mint_quote_outputs.len();
        if outputs_count > self.max_outputs {
            return Err(Error::MaxOutputsExceeded {
                actual: outputs_count,
                max: self.max_outputs,
            });
        }
        if request.melt_quote_outputs.is_empty() && outputs.iter().any(|o| o.amount == Amount::ZERO)
        {
            return Err(invalid(
                "blank outputs are allowed only beside a melt quote",
            ));
        }
        if request.melt_quote_outputs.len() > 1 {
            return Err(invalid("multi-melt is not supported"));
        }
        let quote_ids: HashSet<&str> = request
            .mint_quote_inputs
            .iter()
            .map(|q| q.quote.as_str())
            .collect();
        if quote_ids.len() != request.mint_quote_inputs.len() {
            return Err(Error::DuplicateQuoteIds);
        }
        if request.has_at_least_one_sig_all()? {
            return Err(Error::SigAllUsedInMelt);
        }
        let melt = match request.melt_quote_outputs.first() {
            None => None,
            Some(output) => {
                let quote_id = QuoteId::from_str(&output.quote).map_err(|_| Error::UnknownQuote)?;
                let quote = self
                    .localstore
                    .get_melt_quote(&quote_id)
                    .await?
                    .ok_or(Error::UnknownQuote)?;
                let reserve = match (output.fee_index, quote.fee_options().is_empty()) {
                    (Some(index), false) => quote
                        .fee_options()
                        .iter()
                        .find(|option| option.fee_index == index)
                        .map(|option| option.fee_reserve)
                        .ok_or(Error::OnchainFeeIndexNotFound { index })?,
                    (None, true) => quote.fee_reserve().into(),
                    (Some(_), true) => {
                        return Err(invalid(
                            "fee_index applies only to quotes offering fee_options",
                        ))
                    }
                    (None, false) => return Err(invalid("quote requires a fee_index")),
                };
                if reserve != output.fee_reserve {
                    return Err(invalid("melt fee_reserve does not match the quote"));
                }
                Some(quote)
            }
        };

        let mut quote_inputs = Vec::with_capacity(request.mint_quote_inputs.len());
        for input in &request.mint_quote_inputs {
            let quote_id = QuoteId::from_str(&input.quote).map_err(|_| Error::UnknownQuote)?;
            if input.amount == Amount::ZERO {
                return Err(invalid("quote input amount must be positive"));
            }
            quote_inputs.push((quote_id, input.amount));
        }

        let melt_quotes: Vec<nutroot::Quote> = match &melt {
            Some(quote) => vec![nutroot::Quote {
                id: quote.id.to_string(),
                amount: quote
                    .amount()
                    .checked_add(
                        &request.melt_quote_outputs[0]
                            .fee_reserve
                            .with_unit(quote.unit.clone()),
                    )?
                    .into(),
            }],
            None => vec![],
        };
        // The transcript commits each quote input's lock key, so the keys are read before the
        // digest; the quotes are loaded again below with their state checks.
        let mut transcript_quotes = Vec::with_capacity(quote_inputs.len());
        let mut quote_issued = Vec::with_capacity(quote_inputs.len());
        for (id, amount) in &quote_inputs {
            let quote = self
                .localstore
                .get_mint_quote(id)
                .await?
                .ok_or(Error::UnknownQuote)?;
            // Value held in a txout quote cannot hop to another one (NUT-XX).
            if has_fixed && quote.payment_method.is_txout() {
                return Err(invalid(
                    "a txout quote input cannot pay into a fixed txout quote",
                ));
            }
            transcript_quotes.push(nutroot::MintQuoteInput {
                id: id.to_string(),
                amount: *amount,
                pubkey: quote
                    .pubkey
                    .ok_or_else(|| invalid("quote inputs must be locked"))?,
                issued: quote.amount_issued().into(),
            });
            quote_issued.push(quote.amount_issued().into());
        }
        // Ids resolve once quote inputs load: an input-bound id hashes their issued amounts too.
        let section = if quote_requests
            .iter()
            .any(|(preimage, _, _)| preimage.is_none())
        {
            nutroot::input_section(proofs, &transcript_quotes)
                .map_err(cdk_common::nuts::nut10::Error::from)?
        } else {
            vec![]
        };
        let mut quote_outputs: Vec<QuoteOutput> = Vec::with_capacity(quote_requests.len());
        let mut transcript_quote_outputs = Vec::with_capacity(quote_requests.len());
        let mut ids = HashSet::new();
        for (k, (id_preimage, pubkey, amount)) in quote_requests.into_iter().enumerate() {
            // A fixed txout quote is named by its preimage or, without one, its inputs.
            let id = match id_preimage {
                Some(preimage) => nutroot::derived_quote_id(&preimage),
                None => nutroot::input_bound_quote_id(
                    &section,
                    u32::try_from(k).map_err(|_| invalid("too many mint quote outputs"))?,
                ),
            };
            let quote_id = QuoteId::BASE64(id.clone());
            if !ids.insert(quote_id.clone()) {
                return Err(Error::DuplicateOutputs);
            }
            transcript_quote_outputs.push(nutroot::MintQuoteOutput {
                amount,
                pubkey: *pubkey.as_secp256k1()?,
                id,
            });
            quote_outputs.push((quote_id, pubkey, amount));
        }
        let transcript = nutroot::Transaction::with_quote_outputs(
            proofs,
            &transcript_quotes,
            outputs,
            &melt_quotes,
            &transcript_quote_outputs,
        )
        .map_err(cdk_common::nuts::nut10::Error::from)?;

        let mut units: HashSet<CurrencyUnit> = HashSet::new();
        let inputs_verification = if proofs.is_empty() {
            None
        } else {
            let verification = self.verify_inputs(proofs).await?;
            units.insert(verification.amount.unit().clone());
            Some(verification)
        };
        if !outputs.is_empty() {
            units.insert(self.verify_outputs(outputs)?.amount.unit().clone());
        }

        if let Some(quote) = &melt {
            units.insert(quote.unit.clone());
        }
        // Fixed outputs must be signable before any irreversible payment.
        for output in outputs {
            let keyset = self
                .get_keyset_info(&output.keyset_id)
                .ok_or(Error::UnknownKeySet)?;
            if output.amount != Amount::ZERO && !keyset.amounts.contains(&u64::from(output.amount))
            {
                return Err(invalid(
                    "output denomination is not supported by its keyset",
                ));
            }
            match output.keyset_id.get_version() {
                KeySetVersion::Version02 => {
                    output.blinded_secret.as_bls_g1()?;
                }
                _ => {
                    output.blinded_secret.as_secp256k1()?;
                }
            }
        }
        // Outputs beside a melt need keysets that outlive the melt quote (NUT-XX).
        if let Some(quote) = &melt {
            for output in outputs {
                let final_expiry = self
                    .get_keyset_info(&output.keyset_id)
                    .and_then(|keyset| keyset.final_expiry);
                if final_expiry.is_some_and(|expiry| expiry < quote.expiry) {
                    return Err(invalid("an output's keyset expires before the melt quote"));
                }
            }
        }

        if let Some(quote) = &melt {
            match quote.state {
                MeltQuoteState::Unpaid | MeltQuoteState::Failed => {}
                MeltQuoteState::Pending => return Err(Error::PendingQuote),
                MeltQuoteState::Paid => return Err(Error::PaidQuote),
                MeltQuoteState::Unknown => return Err(Error::UnknownPaymentState),
            }
        }
        let mut mint_quotes = Vec::with_capacity(quote_inputs.len());
        for (quote_id, amount) in &quote_inputs {
            let mut quote = self
                .localstore
                .get_mint_quote(quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;
            self.check_mint_quote_paid(&mut quote).await?;
            if quote.pubkey.is_none() {
                return Err(invalid("quote inputs must be locked"));
            }
            match quote.state() {
                MintQuoteState::Paid => {}
                MintQuoteState::Unpaid => return Err(Error::UnpaidQuote),
                MintQuoteState::Issued => return Err(Error::IssuedQuote),
            }
            if amount.with_unit(quote.unit.clone()) > quote.amount_mintable() {
                return Err(invalid("quote input exceeds the quote's mintable amount"));
            }
            // A txout quote redeems once and in full (NUT-XX).
            if quote.payment_method.is_txout()
                && amount.with_unit(quote.unit.clone()) != quote.amount_mintable()
            {
                return Err(invalid("a txout quote is drawn only in full"));
            }
            units.insert(quote.unit.clone());
            mint_quotes.push(quote);
        }
        if units.len() != 1 {
            return Err(Error::UnitMismatch);
        }
        let unit = units.into_iter().next().ok_or(Error::Internal)?;

        // Pre-v3 proofs keep their own rules; v3 inputs sign their input digest.
        request.verify_inputs_with_transaction(Some(&transcript))?;
        let now = unix_time();
        for (index, (input, quote)) in request
            .mint_quote_inputs
            .iter()
            .zip(&mint_quotes)
            .enumerate()
        {
            let pubkey = quote.pubkey.ok_or(Error::SignatureMissingOrInvalid)?;
            parse_quote_witness(&input.witness)?
                .verify_in(
                    &pubkey.to_string(),
                    transcript
                        .input_digest(proofs.len() + index)
                        .map_err(cdk_common::nuts::nut10::Error::from)?,
                    transcript.txout_keys(),
                    now,
                )
                .map_err(cdk_common::nuts::nut10::Error::from)?;
        }

        // A quote input prices as its minimal split, inside NUT-02's single rounding.
        let quote_input_fee_ppk = self.mint_info().await?.nuts.nutxx.quote_input_fee_ppk;
        let mut ppk: u64 = 0;
        for proof in proofs {
            let keyset = self
                .get_keyset_info(&proof.keyset_id)
                .ok_or(Error::UnknownKeySet)?;
            ppk = ppk
                .checked_add(keyset.input_fee_ppk)
                .ok_or(Error::AmountOverflow)?;
        }
        for (_, amount) in &quote_inputs {
            ppk = ppk
                .checked_add(
                    u64::from(u64::from(*amount).count_ones())
                        .checked_mul(quote_input_fee_ppk)
                        .ok_or(Error::AmountOverflow)?,
                )
                .ok_or(Error::AmountOverflow)?;
        }
        let fee = Amount::from(ppk.checked_add(999).ok_or(Error::AmountOverflow)? / 1000);

        let proofs_amount: Amount = inputs_verification
            .map(|v| v.amount.into())
            .unwrap_or(Amount::ZERO);
        let quotes_amount = Amount::try_sum(quote_inputs.iter().map(|(_, amount)| *amount))?;
        let inputs_amount = proofs_amount
            .checked_add(quotes_amount)
            .ok_or(Error::AmountOverflow)?;
        // Fixed txout quotes are outputs with a known amount.
        let outputs_amount = Amount::try_sum(
            outputs
                .iter()
                .map(|o| o.amount)
                .chain(quote_outputs.iter().map(|(_, _, amount)| *amount)),
        )?;
        let melt_amount: Amount = melt
            .as_ref()
            .map(|q| q.amount().into())
            .unwrap_or(Amount::ZERO);
        let melt_reserve = request
            .melt_quote_outputs
            .first()
            .map(|o| o.fee_reserve)
            .unwrap_or(Amount::ZERO);
        let required = outputs_amount
            .checked_add(melt_amount)
            .and_then(|a| a.checked_add(melt_reserve))
            .and_then(|a| a.checked_add(fee))
            .ok_or(Error::AmountOverflow)?;
        if inputs_amount != required {
            return Err(Error::TransactionUnbalanced(
                inputs_amount.into(),
                outputs_amount
                    .checked_add(melt_amount)
                    .and_then(|a| a.checked_add(melt_reserve))
                    .unwrap_or(Amount::ZERO)
                    .into(),
                fee.into(),
            ));
        }
        let excess = inputs_amount
            .checked_sub(fee)
            .and_then(|a| a.checked_sub(outputs_amount))
            .and_then(|a| a.checked_sub(melt_amount))
            .ok_or(Error::AmountOverflow)?;

        Ok(Prepared {
            request,
            unit,
            transcript,
            quote_inputs,
            quote_issued,
            quote_outputs,
            melt,
            inputs_amount,
            outputs_amount,
            fee,
            excess,
        })
    }

    /// Settle a transaction with no melt in one database transaction.
    async fn settle_without_melt(&self, prepared: Prepared) -> Result<TransactionResponse, Error> {
        let proofs: Proofs = prepared.request.proof_inputs.clone();
        let outputs: Vec<BlindedMessage> = prepared.request.blinded_outputs.clone();
        let signatures = if outputs.is_empty() {
            vec![]
        } else {
            self.blind_sign(outputs.clone()).await?
        };
        let fee_breakdown = self.get_proofs_fee(&proofs).await?;
        let operation = Operation::new(
            uuid::Uuid::now_v7(),
            OperationKind::Transaction,
            prepared.outputs_amount,
            prepared.inputs_amount,
            prepared.fee,
            None,
            None,
        );
        let ys = proofs.ys()?;

        let mut tx = self.localstore.begin_transaction().await?;
        let result: Result<(Vec<MintQuote>, Vec<MintQuote>), Error> = async {
            let issued = reserve_quote_inputs(
                &mut tx,
                &prepared.unit,
                &prepared.quote_inputs,
                &prepared.quote_issued,
                true,
            )
            .await?;
            if !proofs.is_empty() {
                let mut stored = tx
                    .add_proofs(proofs.clone(), None, &operation)
                    .await
                    .map_err(|err| match err {
                        database::Error::Duplicate => Error::TokenPending,
                        database::Error::AttemptUpdateSpentProof => Error::TokenAlreadySpent,
                        err => Error::Database(err),
                    })?;
                if proofs
                    .iter()
                    .any(|p| p.keyset_id.get_version() == KeySetVersion::Version02)
                {
                    Mint::record_nutroot_spends(&mut tx, &proofs, &prepared.transcript).await?;
                }
                Mint::update_proofs_state(&mut tx, &mut stored, State::Spent).await?;
            }
            // Same lock order as the melt saga: mint quotes, proofs, then txout quote ids.
            claim_txout_quote_ids(&mut tx, &prepared.quote_outputs, None).await?;
            if !outputs.is_empty() {
                let secrets: Vec<PublicKey> = outputs.iter().map(|o| o.blinded_secret).collect();
                tx.add_blind_signatures(&secrets, &signatures, None).await?;
            }
            // Without a melt every mint quote output is a fixed txout quote.
            let created =
                create_txout_quotes(&mut tx, &prepared.unit, &prepared.quote_outputs).await?;
            tx.add_completed_operation(&operation, &fee_breakdown.per_keyset)
                .await?;
            Ok((issued, created))
        }
        .await;
        let (issued, created) = match result {
            Ok(value) => value,
            Err(err) => {
                tx.rollback().await?;
                return Err(err);
            }
        };
        let states = if proofs.is_empty() {
            vec![]
        } else {
            Mint::proof_states(&mut tx, &proofs, State::Spent).await?
        };
        tx.commit().await?;

        for state in states {
            self.pubsub_manager.proof_state(state);
        }
        for quote in &issued {
            self.pubsub_manager
                .mint_quote_issue(quote, quote.amount_issued());
        }
        for quote in &created {
            self.pubsub_manager
                .mint_quote_payment(quote, quote.amount_paid());
        }
        tracing::info!(
            proof_count = proofs.len(),
            quote_input_count = prepared.quote_inputs.len(),
            output_count = outputs.len(),
            txout_quote_count = created.len(),
            "transaction settled",
        );
        Ok(TransactionResponse {
            state: TransactionState::Paid,
            signatures,
            melt_quotes: vec![],
            mint_quotes: txout_quote_responses(created.into_iter().map(Some).collect())?,
        })
    }

    /// Reserve a transaction with a melt through the melt saga.
    async fn melt_transaction(
        &self,
        prepared: Prepared,
        melt: MeltQuote,
    ) -> Result<super::melt::PendingMelt, Error> {
        let mut melt_request = MeltRequest::new(
            melt.id.clone(),
            prepared.request.proof_inputs.clone(),
            Some(prepared.request.blinded_outputs.clone()),
        );
        if let Some(index) = prepared.request.melt_quote_outputs[0].fee_index {
            melt_request = melt_request.fee_index(index);
        }
        let setup = TransactionSetup {
            record: TransactionRecord {
                melt_quote_id: melt.id.clone(),
                state: TransactionState::Pending,
                unit: prepared.unit.clone(),
                quote_inputs: prepared.quote_inputs.clone(),
                mint_quote_outputs: prepared.quote_outputs.clone(),
                excess: prepared.excess,
                created_time: unix_time(),
            },
            quote_issued: prepared.quote_issued.clone(),
            transcript: prepared
                .request
                .proof_inputs
                .iter()
                .any(|p| p.keyset_id.get_version() == KeySetVersion::Version02)
                .then_some(prepared.transcript),
            inputs_amount: prepared.inputs_amount.with_unit(prepared.unit.clone()),
            inputs_fee: prepared.fee.with_unit(prepared.unit.clone()),
            outputs_amount: prepared.outputs_amount.with_unit(prepared.unit.clone()),
        };
        let verification = Verification {
            amount: setup.inputs_amount.clone(),
        };
        self.melt_inner(&melt_request, verification, melt, Some(setup))
            .await
    }
}

#[cfg(test)]
mod tests;
