use super::*;
pub(super) fn find_payment_outpoint(
    tx: &Transaction,
    payment_script: &Script,
    amount_sat: u64,
) -> Option<OutPoint> {
    // The payment proof records one outpoint, so require one receiver-script
    // output to cover the full quote. A proposal that only pays the quote via
    // multiple smaller outputs is valid-looking value-wise but not representable
    // by the current proof model.
    tx.output
        .iter()
        .enumerate()
        .find(|(_, output)| {
            output.script_pubkey.as_script() == payment_script
                && output.value.to_sat() >= amount_sat
        })
        .map(|(vout, _)| OutPoint::new(tx.compute_txid(), vout as u32))
}

pub(super) fn require_payjoin_send_payment_output(
    tx: &Transaction,
    payment_script: &Script,
    amount_sat: u64,
) -> Result<OutPoint, Error> {
    find_payment_outpoint(tx, payment_script, amount_sat).ok_or_else(|| {
        Error::Payjoin(format!(
            "Payjoin transaction missing payment output for {} sats",
            amount_sat
        ))
    })
}

/// Validate a signed Payjoin send by local wallet accounting.
///
/// A receiver may contribute inputs and increase the receiver output, so the
/// proposal's total transaction fee is not the mint's fee contribution. The
/// relevant budget is the mint wallet's net spend (`sent - received`), which
/// must stay within `amount_sat + max_fee_sat`. The recorded fee contribution is
/// therefore `mint_net_spend_sat - amount_sat`.
pub(super) fn validate_payjoin_send_transaction(
    tx: &Transaction,
    payment_script: &Script,
    amount_sat: u64,
    max_fee_sat: u64,
    sent_sat: u64,
    received_sat: u64,
) -> Result<PayjoinSendValidation, Error> {
    let payment_outpoint = require_payjoin_send_payment_output(tx, payment_script, amount_sat)?;
    let mint_net_spend_sat = sent_sat.checked_sub(received_sat).ok_or_else(|| {
        Error::Payjoin(format!(
            "Payjoin transaction wallet receive amount {} exceeds sent amount {}",
            received_sat, sent_sat
        ))
    })?;
    let max_net_spend_sat = amount_sat.checked_add(max_fee_sat).ok_or_else(|| {
        Error::Payjoin(format!(
            "Payjoin spend cap overflow for amount {} and max fee {}",
            amount_sat, max_fee_sat
        ))
    })?;
    if mint_net_spend_sat > max_net_spend_sat {
        return Err(Error::Payjoin(format!(
            "Payjoin transaction spends {} sats from mint wallet, exceeding cap {}",
            mint_net_spend_sat, max_net_spend_sat
        )));
    }
    let fee_contribution_sat = mint_net_spend_sat.checked_sub(amount_sat).ok_or_else(|| {
        Error::Payjoin(format!(
            "Payjoin transaction mint net spend {} is below payment amount {}",
            mint_net_spend_sat, amount_sat
        ))
    })?;

    Ok(PayjoinSendValidation {
        payment_outpoint,
        fee_contribution_sat,
    })
}
