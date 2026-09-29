//! NUT-XX transactions: proofs and paid mint quotes in; blinded messages, one
//! melt quote and a change quote out.
//!
//! A transaction without a melt settles in one database transaction. One with
//! a melt runs the melt saga: quote inputs are issued and the record stored in
//! the saga's setup transaction, and the shared melt finalization and rollback
//! paths settle or fail the record by its melt quote id.

use std::collections::HashSet;
use std::str::FromStr;

use cdk_common::database::mint::MeltRequestInfo;
use cdk_common::database::{DynMintDatabase, DynMintTransaction};
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
use super::subscription::PubSubManager;
use super::verification::Verification;
use super::{Error, Mint, MintQuoteResponse};
use crate::MeltQuoteResponse;

/// Everything the melt saga needs to reserve a transaction alongside its melt.
#[derive(Debug)]
pub(crate) struct TransactionSetup {
    pub record: TransactionRecord,
    /// Present when a v3 proof input needs its spend recorded.
    pub transcript: Option<nutroot::Transaction>,
    pub inputs_amount: Amount<CurrencyUnit>,
    pub inputs_fee: Amount<CurrencyUnit>,
    pub outputs_amount: Amount<CurrencyUnit>,
}

/// Outcome of validation: a record already held, or a request ready to reserve.
enum Preparation {
    Existing(String),
    Fresh(Box<Prepared>),
}

/// Validated request, ready to reserve.
struct Prepared {
    request: TransactionRequest,
    digest: String,
    unit: CurrencyUnit,
    transcript: nutroot::Transaction,
    quote_inputs: Vec<(QuoteId, Amount)>,
    change_pubkey: Option<PublicKey>,
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

/// Where a transaction's signatures are filed: under its melt quote, else its digest.
fn signature_key(record: &TransactionRecord) -> QuoteId {
    record
        .melt_quote_id
        .clone()
        .unwrap_or_else(|| QuoteId::BASE64(record.digest.clone()))
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

fn change_quote(record: &TransactionRecord, change: Amount, pubkey: PublicKey) -> MintQuote {
    let now = unix_time();
    MintQuote::new(
        Some(QuoteId::new()),
        record.digest.clone(),
        record.unit.clone(),
        Some(change.with_unit(record.unit.clone())),
        0,
        PaymentIdentifier::CustomId(format!("change:{}", record.digest)),
        Some(pubkey),
        Amount::ZERO.with_unit(record.unit.clone()),
        Amount::ZERO.with_unit(record.unit.clone()),
        PaymentMethod::Custom(cdk_common::nuts::nutxx::CHANGE_METHOD.to_owned()),
        now,
        now,
        vec![],
        vec![],
        None,
    )
}

/// Issue each quote input's amount, under the quote row locks.
pub(crate) async fn reserve_quote_inputs(
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
        match quote.state() {
            MintQuoteState::Paid => {}
            MintQuoteState::Unpaid => return Err(Error::UnpaidQuote),
            MintQuoteState::Issued => return Err(Error::IssuedQuote),
        }
        let amount = amount.with_unit(record.unit.clone());
        if amount > quote.amount_mintable() {
            return Err(invalid("quote input exceeds the quote's mintable amount"));
        }
        quote.add_issuance(amount)?;
        tx.update_mint_quote(&mut quote).await?;
        issued.push(quote.inner());
    }
    Ok(issued)
}

/// Return each quote input's amount after a failed melt.
pub(crate) async fn release_quote_inputs(
    tx: &mut DynMintTransaction,
    record: &TransactionRecord,
) -> Result<(), Error> {
    if record.quote_inputs.is_empty() {
        return Ok(());
    }
    let ids: Vec<QuoteId> = record
        .quote_inputs
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    let quotes = tx.get_mint_quotes_by_ids(&ids).await?;
    for ((_, amount), quote) in record.quote_inputs.iter().zip(quotes) {
        let mut quote = quote.ok_or(Error::UnknownQuote)?;
        tx.remove_mint_quote_issuance(&mut quote, *amount).await?;
    }
    Ok(())
}

/// Sign the reserved outputs, create the change quote and mark the record paid.
///
/// Runs in the melt finalization's second transaction, in place of the blank
/// change signing of a plain melt. The melt quote is already paid.
pub(crate) async fn settle_with_melt(
    mint: &Mint,
    db: &DynMintDatabase,
    pubsub: &PubSubManager,
    quote: &MeltQuote,
    melt_request_info: &MeltRequestInfo,
    total_spent: Amount<CurrencyUnit>,
) -> Result<MeltChangeResult, Error> {
    // Outputs whose keyset went inactive during the payment are not signed (NUT-02);
    // their value returns through the change quote instead.
    let mut outputs = melt_request_info.change_outputs.clone();
    let mut unsigned_amount = Amount::ZERO;
    let signable = outputs.first().is_none_or(|output| {
        mint.get_keyset_info(&output.keyset_id)
            .is_some_and(|keyset| keyset.active && !keyset.is_expired())
    });
    if !signable {
        unsigned_amount = Amount::try_sum(outputs.iter().map(|o| o.amount))?;
        tracing::warn!(
            melt_quote_id = %quote.id,
            unsigned_amount = %unsigned_amount,
            "transaction outputs lost their active keyset during payment; returning their value as change",
        );
        outputs.clear();
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

    let fee_paid: Amount = total_spent
        .checked_sub(&quote.amount())
        .map(Into::into)
        .unwrap_or(Amount::ZERO);
    let excess = record
        .excess
        .checked_add(unsigned_amount)
        .ok_or(Error::AmountOverflow)?;
    let change = settlement_change(excess, quote.fee_reserve().into(), fee_paid);
    let mut change_quote_id = None;
    let mut created = None;
    if let (Some(pubkey), true) = (record.change_pubkey, change > Amount::ZERO) {
        let mut created_quote = tx
            .add_mint_quote(change_quote(&record, change, pubkey))
            .await?;
        created_quote.add_payment(
            change.with_unit(record.unit.clone()),
            record.digest.clone(),
            None,
        )?;
        tx.update_mint_quote(&mut created_quote).await?;
        change_quote_id = Some(created_quote.id.clone());
        created = Some(created_quote.inner());
    }
    tx.update_transaction(&mut record, TransactionState::Paid, change_quote_id)
        .await?;
    if let Some(quote) = created {
        pubsub.mint_quote_payment(&quote, quote.amount_paid());
    }
    tracing::info!(
        digest = %record.digest,
        melt_quote_id = %quote.id,
        change = %change,
        "transaction settled with melt",
    );
    Ok(MeltChangeResult::Ready {
        change_sigs: (!signatures.is_empty()).then_some(signatures),
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
        let prepared = match self.prepare_transaction(request).await? {
            // A resend of an accepted transaction returns its record.
            Preparation::Existing(digest) => return self.get_transaction(&digest).await,
            Preparation::Fresh(prepared) => *prepared,
        };
        match prepared.melt.clone() {
            None => self.settle_without_melt(prepared).await,
            Some(melt) => {
                let respond_async = respond_async || prepared.request.prefer_async;
                let digest = prepared.digest.clone();
                let pending = match self.melt_transaction(prepared, melt).await {
                    Ok(pending) => pending,
                    Err(err) => return self.transaction_or_error(&digest, err).await,
                };
                if !respond_async {
                    // The outcome is read from the record; a failed payment is FAILED there.
                    if let Err(err) = pending.await {
                        tracing::info!(digest = %digest, "transaction melt did not settle: {err}");
                    }
                }
                self.get_transaction(&digest).await
            }
        }
    }

    // Reservation may lose a race to the same request. Only return a committed
    // accepted record, never mask a failure with an older FAILED attempt.
    async fn transaction_or_error(
        &self,
        digest: &str,
        err: Error,
    ) -> Result<TransactionResponse, Error> {
        if let Some(record) = self.localstore.get_transaction(digest).await? {
            if record.state != TransactionState::Failed {
                return self.get_transaction(digest).await;
            }
        }
        Err(err)
    }

    /// The record for a transaction digest; a pending melt is reconciled first.
    #[instrument(skip_all)]
    pub async fn get_transaction(&self, digest: &str) -> Result<TransactionResponse, Error> {
        let mut record = self
            .localstore
            .get_transaction(digest)
            .await?
            .ok_or(Error::TransactionNotFound)?;
        if record.state == TransactionState::Pending {
            if let Some(melt_quote_id) = &record.melt_quote_id {
                let quote_lock = self.melt_quote_lock(melt_quote_id).await;
                // Live dispatch owns recovery; return PENDING without waiting for it.
                if let Ok(guard) = quote_lock.try_lock_owned() {
                    let mut quote = self
                        .localstore
                        .get_melt_quote(melt_quote_id)
                        .await?
                        .ok_or(Error::UnknownQuote)?;
                    self.handle_pending_melt_quote_locked(&mut quote, guard)
                        .await?;
                }
                record = self
                    .localstore
                    .get_transaction(digest)
                    .await?
                    .ok_or(Error::TransactionNotFound)?;
            }
        }
        let signatures: Vec<BlindSignature> = if record.state == TransactionState::Paid {
            self.localstore
                .get_blind_signatures_for_quote(&signature_key(&record))
                .await?
        } else {
            vec![]
        };
        let mut melt_quotes = vec![];
        if let Some(melt_quote_id) = &record.melt_quote_id {
            let quote = self
                .localstore
                .get_melt_quote(melt_quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;
            melt_quotes.push(melt_quote_json(MeltQuoteResponse::<QuoteId>::from(quote))?);
        }
        let mut change_quote = None;
        if let Some(change_quote_id) = &record.change_quote_id {
            let quote = self
                .localstore
                .get_mint_quote(change_quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;
            change_quote = Some(mint_quote_json(MintQuoteResponse::<QuoteId>::try_from(
                quote,
            )?)?);
        }
        Ok(TransactionResponse {
            digest: record.digest,
            state: record.state,
            signatures,
            melt_quotes,
            change_quote,
        })
    }

    /// Validate the request against mint and quote state and build its transcript.
    async fn prepare_transaction(&self, request: TransactionRequest) -> Result<Preparation, Error> {
        let proofs = &request.proof_inputs;
        let outputs = &request.blinded_outputs;
        if proofs.is_empty() && request.mint_quote_inputs.is_empty() {
            return Err(invalid("transaction requires at least one input"));
        }
        if outputs.is_empty()
            && request.melt_quote_outputs.is_empty()
            && request.change_pubkey.is_none()
        {
            return Err(invalid("transaction requires at least one output"));
        }
        if outputs.iter().any(|o| o.amount == Amount::ZERO) {
            return Err(invalid("blank outputs are not allowed: use a change quote"));
        }
        if request.melt_quote_outputs.len() > 1 {
            return Err(invalid("multi-melt is not supported"));
        }
        // Outputs cannot be signed if their keyset rotates while the payment is in
        // flight; the change quote is where their value goes then.
        if !request.melt_quote_outputs.is_empty()
            && !outputs.is_empty()
            && request.change_pubkey.is_none()
        {
            return Err(invalid(
                "a melt with blinded outputs requires a change_pubkey",
            ));
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
        let change_pubkey = request
            .change_pubkey
            .as_deref()
            .map(PublicKey::from_hex)
            .transpose()
            .map_err(|_| invalid("change_pubkey is not a compressed secp256k1 key"))?;
        if let Some(key) = &change_pubkey {
            key.as_secp256k1()
                .map_err(|_| invalid("change_pubkey is not a compressed secp256k1 key"))?;
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
            });
        }
        let secp_change_key = change_pubkey
            .as_ref()
            .map(|key| key.as_secp256k1().copied())
            .transpose()
            .map_err(|_| invalid("change_pubkey is not a compressed secp256k1 key"))?;
        let transcript = nutroot::Transaction::with_change(
            proofs,
            &transcript_quotes,
            outputs,
            &melt_quotes,
            secp_change_key.as_ref(),
        )
        .map_err(cdk_common::nuts::nut10::Error::from)?;
        let digest = cdk_common::util::hex::encode(transcript.digest());

        // Held as PENDING or PAID: the inputs are not spent again, the record is returned.
        if let Some(record) = self.localstore.get_transaction(&digest).await? {
            if record.state != TransactionState::Failed {
                return Ok(Preparation::Existing(digest));
            }
        }

        let retry_digest = digest.clone();
        let result = async {
            let proofs = &request.proof_inputs;
            let outputs = &request.blinded_outputs;
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
                if !keyset.amounts.contains(&u64::from(output.amount)) {
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
            if inputs_amount < required || (change_pubkey.is_none() && inputs_amount != required) {
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

            Ok(Preparation::Fresh(Box::new(Prepared {
                request,
                digest,
                unit,
                transcript,
                quote_inputs,
                change_pubkey,
                melt,
                inputs_amount,
                outputs_amount,
                fee,
                excess,
            })))
        }
        .await;
        if result.is_err() {
            // Another request may have committed after our initial digest lookup
            // but before validation observed its issued quotes or spent inputs.
            if let Some(record) = self.localstore.get_transaction(&retry_digest).await? {
                if record.state != TransactionState::Failed {
                    return Ok(Preparation::Existing(retry_digest));
                }
            }
        }
        result
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
        let mut record = TransactionRecord {
            digest: prepared.digest.clone(),
            state: TransactionState::Paid,
            unit: prepared.unit.clone(),
            melt_quote_id: None,
            quote_inputs: prepared.quote_inputs.clone(),
            change_pubkey: prepared.change_pubkey,
            excess: prepared.excess,
            change_quote_id: None,
            operation_id: *operation.id(),
            created_time: unix_time(),
        };
        let change = settlement_change(prepared.excess, Amount::ZERO, Amount::ZERO);
        let ys = proofs.ys()?;

        let mut tx = self.localstore.begin_transaction().await?;
        let result: Result<(Vec<MintQuote>, Option<MintQuote>), Error> = async {
            let issued = reserve_quote_inputs(&mut tx, &record).await?;
            if !proofs.is_empty() {
                let mut stored = tx
                    .add_proofs(proofs.clone(), None, &operation)
                    .await
                    .map_err(|err| match err {
                        cdk_common::database::Error::Duplicate => Error::TokenPending,
                        cdk_common::database::Error::AttemptUpdateSpentProof => {
                            Error::TokenAlreadySpent
                        }
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
                tx.add_blind_signatures(&secrets, &signatures, Some(signature_key(&record)))
                    .await?;
            }
            let mut created = None;
            if let (Some(pubkey), true) = (record.change_pubkey, change > Amount::ZERO) {
                let mut quote = tx
                    .add_mint_quote(change_quote(&record, change, pubkey))
                    .await?;
                quote.add_payment(
                    change.with_unit(record.unit.clone()),
                    record.digest.clone(),
                    None,
                )?;
                tx.update_mint_quote(&mut quote).await?;
                record.change_quote_id = Some(quote.id.clone());
                created = Some(quote.inner());
            }
            tx.add_transaction(&record).await?;
            tx.add_completed_operation(&operation, &fee_breakdown.per_keyset)
                .await?;
            Ok((issued, created))
        }
        .await;
        let (issued, created) = match result {
            Ok(value) => value,
            Err(err) => {
                tx.rollback().await?;
                return self.transaction_or_error(&record.digest, err).await;
            }
        };
        let states = if ys.is_empty() {
            vec![]
        } else {
            Mint::spent_proof_states(&mut tx, &ys).await?
        };
        tx.commit().await?;

        for state in states {
            self.pubsub_manager.proof_state(state);
        }
        for quote in &issued {
            self.pubsub_manager
                .mint_quote_issue(quote, quote.amount_issued());
        }
        if let Some(quote) = &created {
            self.pubsub_manager
                .mint_quote_payment(quote, quote.amount_paid());
        }
        tracing::info!(
            digest = %record.digest,
            proof_count = proofs.len(),
            quote_input_count = record.quote_inputs.len(),
            output_count = outputs.len(),
            change = %change,
            "transaction settled",
        );
        self.get_transaction(&record.digest).await
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
                digest: prepared.digest.clone(),
                state: TransactionState::Pending,
                unit: prepared.unit.clone(),
                melt_quote_id: Some(melt.id.clone()),
                quote_inputs: prepared.quote_inputs.clone(),
                change_pubkey: prepared.change_pubkey,
                excess: prepared.excess,
                change_quote_id: None,
                operation_id: uuid::Uuid::nil(),
                created_time: unix_time(),
            },
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
