//! Outgoing payment monitoring and bounded status lookups.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdk_common::nuts::{CurrencyUnit, MeltQuoteState};
use cdk_common::payment::{Event, MakePaymentResponse, PaymentIdentifier};
use cdk_common::{Amount, QuoteId};
use futures::stream::FuturesUnordered;
use futures::{Stream, StreamExt};
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

use crate::lnrpc::payment::PaymentStatus;
use crate::{client, lnrpc, routerrpc, Error};

pub(crate) const RPC_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
// Periodically reattach even a silent stream so a half-open connection cannot
// leave a settled payment unobserved indefinitely.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
struct TrackedPayment {
    quote_id: QuoteId,
    payment_hash: [u8; 32],
}

#[derive(Debug, Default)]
pub(crate) struct Tracking {
    changed: Notify,
    payments: Mutex<HashMap<String, TrackedPayment>>,
    dispatching: Mutex<HashSet<String>>,
}

// Prevent a live retry from observing the previous attempt's failure before
// SendPaymentV2 has acknowledged the new attempt.
pub(crate) struct DispatchGuard {
    key: String,
    tracking: Arc<Tracking>,
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        self.tracking
            .dispatching
            .lock()
            .expect("dispatch lock poisoned")
            .remove(&self.key);
        self.tracking.changed.notify_one();
    }
}

pub(crate) fn register(
    tracking: &Arc<Tracking>,
    quote_id: QuoteId,
    payment_hash: [u8; 32],
) -> DispatchGuard {
    let key = QuoteId::new().to_string();
    tracking
        .dispatching
        .lock()
        .expect("dispatch lock poisoned")
        .insert(key.clone());
    // Each attempt has its own key: acknowledging an old terminal event must
    // never remove tracking for a concurrent retry of the same quote/hash.
    tracking
        .payments
        .lock()
        .expect("tracking lock poisoned")
        .insert(
            key.clone(),
            TrackedPayment {
                quote_id,
                payment_hash,
            },
        );
    DispatchGuard {
        key,
        tracking: tracking.clone(),
    }
}

pub(crate) fn payment_response(
    payment: lnrpc::Payment,
    payment_lookup_id: PaymentIdentifier,
    unit: &CurrencyUnit,
) -> Result<MakePaymentResponse, Error> {
    let status = match payment.status() {
        PaymentStatus::Initiated | PaymentStatus::InFlight => MeltQuoteState::Pending,
        PaymentStatus::Succeeded => MeltQuoteState::Paid,
        PaymentStatus::Failed => MeltQuoteState::Failed,
        #[allow(deprecated)]
        PaymentStatus::Unknown => MeltQuoteState::Unknown,
    };
    let total_spent = if status == MeltQuoteState::Paid {
        let msat = crate::lnrpc_payment_total_spent(&payment)?.value();
        crate::msat_total_spent_for_unit(msat, unit)?
    } else {
        Amount::new(0, unit.clone())
    };
    let payment_proof = (status == MeltQuoteState::Paid && !payment.payment_preimage.is_empty())
        .then_some(payment.payment_preimage);
    Ok(MakePaymentResponse {
        payment_lookup_id,
        payment_proof,
        status,
        total_spent,
    })
}

pub(crate) async fn check_payment(
    mut client: client::Client,
    identifier: &PaymentIdentifier,
    unit: &CurrencyUnit,
) -> Result<MakePaymentResponse, Error> {
    let hash =
        cdk_common::util::hex::decode(identifier.to_string()).map_err(|_| Error::InvalidHash)?;
    let result = tokio::time::timeout(RPC_TIMEOUT, async {
        let mut stream = client
            .router()
            .track_payment_v2(routerrpc::TrackPaymentRequest {
                payment_hash: hash,
                no_inflight_updates: false,
            })
            .await?
            .into_inner();
        stream.message().await
    })
    .await
    .map_err(|_| Error::UnknownPaymentStatus)?;
    match result {
        Ok(Some(payment)) => payment_response(payment, identifier.clone(), unit),
        Err(err) if err.code() == tonic::Code::NotFound => Ok(MakePaymentResponse {
            payment_lookup_id: identifier.clone(),
            payment_proof: None,
            status: MeltQuoteState::Unknown,
            total_spent: Amount::new(0, unit.clone()),
        }),
        Ok(None) => Err(Error::UnknownPaymentStatus),
        Err(err) => Err(Error::LndError(err)),
    }
}

// A terminal event remains registered until the consumer polls again. Dropping
// the subscription while delivering an event therefore replays it on reconnect
// within the same backend instance.
pub(crate) fn events(
    client: client::Client,
    tracking: Arc<Tracking>,
    cancel: CancellationToken,
) -> Pin<Box<dyn Stream<Item = Event> + Send>> {
    let (sender, receiver) = mpsc::channel(32);
    let task_cancel = cancel.clone();
    let task_tracking = tracking.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = task_cancel.cancelled() => {},
            _ = monitor(client, task_tracking, sender) => {}
        }
    });
    Box::pin(futures::stream::unfold(
        (receiver, tracking, None::<String>, cancel.drop_guard()),
        |(mut receiver, tracking, previous, guard)| async move {
            if let Some(key) = previous {
                tracking
                    .payments
                    .lock()
                    .expect("tracking lock poisoned")
                    .remove(&key);
            }
            let (key, event) = receiver.recv().await?;
            Some((event, (receiver, tracking, Some(key), guard)))
        },
    ))
}

async fn monitor(
    client: client::Client,
    tracking: Arc<Tracking>,
    sender: mpsc::Sender<(String, Event)>,
) {
    let mut active = HashSet::new();
    let mut payments = FuturesUnordered::new();
    let mut refresh = tokio::time::interval(RETRY_INTERVAL);
    loop {
        tokio::select! {
            _ = tracking.changed.notified() => {},
            _ = refresh.tick() => {},
            Some(event) = payments.next(), if !payments.is_empty() => {
                if sender.send(event).await.is_err() {
                    return;
                }
                continue;
            }
        }
        let registered = tracking
            .payments
            .lock()
            .expect("tracking lock poisoned")
            .clone();
        active.retain(|key| registered.contains_key(key));
        for (key, payment) in registered {
            if active.contains(&key)
                || tracking
                    .dispatching
                    .lock()
                    .expect("dispatch lock poisoned")
                    .contains(&key)
            {
                continue;
            }
            payments.push(watch(client.clone(), key.clone(), payment));
            active.insert(key);
        }
    }
}

async fn watch(
    mut client: client::Client,
    key: String,
    payment: TrackedPayment,
) -> (String, Event) {
    loop {
        let result = tokio::time::timeout(
            RPC_TIMEOUT,
            client
                .router()
                .track_payment_v2(routerrpc::TrackPaymentRequest {
                    payment_hash: payment.payment_hash.to_vec(),
                    no_inflight_updates: false,
                }),
        )
        .await;
        if let Ok(Ok(response)) = result {
            let mut stream = response.into_inner();
            while let Ok(Ok(Some(update))) =
                tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.message()).await
            {
                let response = match payment_response(
                    update,
                    PaymentIdentifier::PaymentHash(payment.payment_hash),
                    &CurrencyUnit::Msat,
                ) {
                    Ok(response) => response,
                    Err(err) => {
                        tracing::warn!(
                            "Invalid LND payment update for {}: {err}",
                            payment.quote_id
                        );
                        break;
                    }
                };
                let event = match response.status {
                    MeltQuoteState::Paid => Event::PaymentSuccessful {
                        quote_id: payment.quote_id,
                        details: response,
                    },
                    MeltQuoteState::Failed => Event::PaymentFailed {
                        quote_id: payment.quote_id,
                        reason: "LND reported terminal payment failure".to_string(),
                    },
                    _ => continue,
                };
                return (key, event);
            }
        }
        // Transport errors, missing payments, and silence are indeterminate.
        // Reattach rather than release funds or initiate another payment.
        tracing::debug!("Reconnecting LND payment monitor for {}", payment.quote_id);
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
}
