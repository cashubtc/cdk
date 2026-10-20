//! NUT-XX transactions: proofs and paid mint quotes in; blinded messages and one
//! melt quote out.
//!
//! A transaction without a melt settles in one database transaction. One with
//! a melt runs the melt saga: quote inputs are reserved and the record stored in
//! the saga's setup transaction, and the shared melt finalization and rollback
//! paths settle or fail the record by its melt quote id.

use std::collections::HashSet;
use std::str::FromStr;

use cdk_common::database::mint::MeltRequestInfo;
use cdk_common::database::{self, DynMintDatabase, DynMintTransaction};
use cdk_common::mint::{MeltQuote, MintQuote, Operation, OperationKind, TransactionRecord};
use cdk_common::nuts::nut10::nutroot::{self, Witness};
use cdk_common::nuts::{
    KeySetVersion, MeltQuoteState, MintQuoteState, SpendingConditionVerification, TransactionState,
};
use cdk_common::transaction::{melt_quote_json, TransactionResponse};
use cdk_common::util::unix_time;
use cdk_common::{
    Amount, BlindSignature, BlindedMessage, CurrencyUnit, MeltRequest, Proofs, ProofsMethods,
    PublicKey, QuoteId, State, TransactionRequest,
};
use tracing::instrument;

use super::melt::shared::{
    begin_melt_cleanup_transaction, MeltChangeResult, MeltCleanupTransaction,
};
use super::verification::Verification;
use super::{Error, Mint};

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

/// Sign the reserved outputs and mark the record paid.
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
        // The outputs' own keyset, active or not.
        let mut denominations = outputs
            .first()
            .and_then(|output| mint.get_keyset_info(&output.keyset_id))
            .map(|keyset| keyset.amounts)
            .unwrap_or_default();
        denominations.sort_unstable_by(|a, b| b.cmp(a));
        let mut left = u64::from(change);
        // Each blank in request order takes the largest amount that fits; one left at zero is not
        // signed (NUT-08), and what is left stays with the mint.
        outputs.retain_mut(|output| {
            if output.amount != Amount::ZERO {
                return true;
            }
            match denominations.iter().find(|&&d| d > 0 && d <= left) {
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
    tx.update_transaction(&mut record, TransactionState::Paid)
        .await?;
    tracing::info!(
        melt_quote_id = %quote.id,
        change = %change,
        "transaction settled with melt",
    );
    Ok(MeltChangeResult::Ready {
        change_sigs: Some(signatures),
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
        let pending = self.melt_transaction(prepared, melt.clone()).await?;
        if !respond_async {
            // The outcome is read from the melt quote; a failed payment is FAILED there.
            if let Err(err) = pending.await {
                tracing::info!(melt_quote_id = %melt.id, "transaction melt did not settle: {err}");
            }
        }
        self.melt_transaction_response(&melt.id).await
    }

    /// The response for a transaction with a melt, read from its melt quote.
    async fn melt_transaction_response(
        &self,
        melt_quote_id: &QuoteId,
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
        let signatures = if state == TransactionState::Paid {
            self.localstore
                .get_blind_signatures_for_quote(melt_quote_id)
                .await?
        } else {
            vec![]
        };
        let change = (state == TransactionState::Paid).then(|| signatures.clone());
        Ok(TransactionResponse {
            state,
            signatures,
            melt_quotes: vec![melt_quote_json(quote.into_response(change))?],
        })
    }

    /// Validate the request against mint and quote state and build its transcript.
    async fn prepare_transaction(&self, request: TransactionRequest) -> Result<Prepared, Error> {
        let proofs = &request.proof_inputs;
        let outputs = &request.blinded_outputs;
        if proofs.is_empty() && request.mint_quote_inputs.is_empty() {
            return Err(invalid("transaction requires at least one input"));
        }
        if outputs.is_empty() && request.melt_quote_outputs.is_empty() {
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
        if outputs.len() > self.max_outputs {
            return Err(Error::MaxOutputsExceeded {
                actual: outputs.len(),
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
        let transcript =
            nutroot::Transaction::new(proofs, &transcript_quotes, outputs, &melt_quotes)
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
            let keyset = outputs[0].keyset_id;
            if outputs.iter().any(|o| o.keyset_id != keyset) {
                return Err(invalid("outputs must share one keyset"));
            }
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
        // Outputs beside a melt need a keyset that outlives the melt quote (NUT-XX).
        if let (Some(quote), Some(output)) = (&melt, outputs.first()) {
            let final_expiry = self
                .get_keyset_info(&output.keyset_id)
                .and_then(|keyset| keyset.final_expiry);
            if final_expiry.is_some_and(|expiry| expiry < quote.expiry) {
                return Err(invalid("outputs' keyset expires before the melt quote"));
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
                .verify(
                    &pubkey.to_string(),
                    transcript
                        .input_digest(proofs.len() + index)
                        .map_err(cdk_common::nuts::nut10::Error::from)?,
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
        let outputs_amount = Amount::try_sum(outputs.iter().map(|o| o.amount))?;
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
        let result: Result<Vec<MintQuote>, Error> = async {
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
            if !outputs.is_empty() {
                let secrets: Vec<PublicKey> = outputs.iter().map(|o| o.blinded_secret).collect();
                tx.add_blind_signatures(&secrets, &signatures, None).await?;
            }
            tx.add_completed_operation(&operation, &fee_breakdown.per_keyset)
                .await?;
            Ok(issued)
        }
        .await;
        let issued = match result {
            Ok(value) => value,
            Err(err) => {
                tx.rollback().await?;
                return Err(err);
            }
        };
        let states = if ys.is_empty() {
            vec![]
        } else {
            Mint::proof_states(&mut tx, &ys, State::Spent).await?
        };
        tx.commit().await?;

        for state in states {
            self.pubsub_manager.proof_state(state);
        }
        for quote in &issued {
            self.pubsub_manager
                .mint_quote_issue(quote, quote.amount_issued());
        }
        tracing::info!(
            proof_count = proofs.len(),
            quote_input_count = prepared.quote_inputs.len(),
            output_count = outputs.len(),
            "transaction settled",
        );
        Ok(TransactionResponse {
            state: TransactionState::Paid,
            signatures,
            melt_quotes: vec![],
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
