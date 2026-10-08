//! File-backed test payment state, injected only into disposable fake backends.
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use cdk_common::amount::Amount;
use cdk_common::nuts::{CurrencyUnit, MeltQuoteState};
use cdk_common::payment::{self, MakePaymentResponse, PaymentIdentifier, WaitPaymentResponse};
use lightning_invoice::Bolt11Invoice;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Payment {
    amount: u64,
    unit: CurrencyUnit,
    outcome: String,
}

fn path(kind: &str, id: &PaymentIdentifier) -> Option<PathBuf> {
    let id = id.to_string();
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    std::env::var_os("CDK_UPGRADE_PAYMENT_CONTROL")
        .map(|root| PathBuf::from(root).join(format!("{kind}-{id}.json")))
}

fn error<E>(error: E) -> payment::Error
where
    E: std::fmt::Display,
{
    payment::Error::Custom(format!("upgrade payment controller: {error}"))
}

fn write(path: &std::path::Path, payment: &Payment) -> Result<(), payment::Error> {
    fs::create_dir_all(path.parent().expect("control directory")).map_err(error)?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec(payment).map_err(error)?).map_err(error)?;
    fs::rename(temporary, path).map_err(error)
}

pub(crate) fn hold_incoming(
    request: &str,
    id: &PaymentIdentifier,
    amount: &Amount<CurrencyUnit>,
) -> Result<bool, payment::Error> {
    let invoice = match request.parse::<Bolt11Invoice>() {
        Ok(invoice) if invoice.description().to_string() == "upgrade-unpaid" => invoice,
        _ => return Ok(false),
    };
    let _ = invoice;
    let Some(path) = path("incoming", id) else {
        return Ok(false);
    };
    write(
        &path,
        &Payment {
            amount: amount.value(),
            unit: amount.unit().clone(),
            outcome: "PAID".to_owned(),
        },
    )?;
    Ok(true)
}

pub(crate) fn incoming(
    id: &PaymentIdentifier,
) -> Result<Option<Vec<WaitPaymentResponse>>, payment::Error> {
    let Some(path) = path("incoming", id).filter(|path| path.exists()) else {
        return Ok(None);
    };
    if !path
        .parent()
        .expect("control directory")
        .join("release-incoming")
        .exists()
    {
        return Ok(Some(Vec::new()));
    }
    let record: Payment = serde_json::from_slice(&fs::read(path).map_err(error)?).map_err(error)?;
    Ok(Some(vec![WaitPaymentResponse {
        payment_identifier: id.clone(),
        payment_amount: Amount::new(record.amount, record.unit),
        payment_id: id.to_string(),
    }]))
}

pub(crate) fn start_incoming(sender: tokio::sync::mpsc::Sender<WaitPaymentResponse>) {
    let Some(root) = std::env::var_os("CDK_UPGRADE_PAYMENT_CONTROL").map(PathBuf::from) else {
        return;
    };
    tokio::spawn(async move {
        let mut sent = BTreeSet::new();
        loop {
            if sender.is_closed() {
                return;
            }
            if root.join("release-incoming").exists() {
                let entries = match fs::read_dir(&root) {
                    Ok(entries) => entries,
                    Err(_) => return,
                };
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let Some(hash) = name
                        .strip_prefix("incoming-")
                        .and_then(|name| name.strip_suffix(".json"))
                    else {
                        continue;
                    };
                    if sent.contains(hash) {
                        continue;
                    }
                    let id = match PaymentIdentifier::new("payment_hash", hash) {
                        Ok(id) => id,
                        Err(_) => continue,
                    };
                    match incoming(&id) {
                        Ok(Some(payments)) => {
                            for payment in payments {
                                if sender.send(payment).await.is_err() {
                                    return;
                                }
                            }
                            sent.insert(hash.to_owned());
                        }
                        _ => continue,
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
}

pub(crate) fn make_outgoing(
    invoice: &Bolt11Invoice,
    unit: &CurrencyUnit,
) -> Result<Option<MakePaymentResponse>, payment::Error> {
    let outcome = match invoice.description().to_string().as_str() {
        "upgrade-pending-paid" => "PAID",
        "upgrade-pending-failed" => "FAILED",
        _ => return Ok(None),
    };
    let id = PaymentIdentifier::PaymentHash(*invoice.payment_hash().as_ref());
    let Some(path) = path("outgoing", &id) else {
        return Ok(None);
    };
    let mut calls = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.with_extension("calls"))
        .map_err(error)?;
    writeln!(calls, "dispatch").map_err(error)?;
    if path.exists() {
        return Err(payment::Error::Custom(
            "pending payment dispatched twice".to_owned(),
        ));
    }
    write(
        &path,
        &Payment {
            amount: invoice
                .amount_milli_satoshis()
                .ok_or_else(|| payment::Error::Custom("missing amount".to_owned()))?
                / 1000
                + 1,
            unit: unit.clone(),
            outcome: outcome.to_owned(),
        },
    )?;
    outgoing(&id)
}

pub(crate) fn outgoing(
    id: &PaymentIdentifier,
) -> Result<Option<MakePaymentResponse>, payment::Error> {
    let Some(path) = path("outgoing", id).filter(|path| path.exists()) else {
        return Ok(None);
    };
    let record: Payment =
        serde_json::from_slice(&fs::read(&path).map_err(error)?).map_err(error)?;
    let status = match path
        .parent()
        .expect("control directory")
        .join("release-outgoing")
        .exists()
    {
        false => MeltQuoteState::Pending,
        true if record.outcome == "PAID" => MeltQuoteState::Paid,
        true => MeltQuoteState::Failed,
    };
    Ok(Some(MakePaymentResponse {
        payment_lookup_id: id.clone(),
        payment_proof: match status {
            MeltQuoteState::Paid => Some("upgrade-fixture".to_owned()),
            _ => None,
        },
        status,
        total_spent: Amount::new(
            match status {
                MeltQuoteState::Paid => record.amount,
                _ => 0,
            },
            record.unit,
        ),
    }))
}
