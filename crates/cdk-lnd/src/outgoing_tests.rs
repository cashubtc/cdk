//! Exercise the real gRPC clients against a controllable streaming LND server.

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cdk_common::payment::{Bolt11OutgoingPaymentOptions, MintPayment};
use cdk_common::QuoteId;
use futures::StreamExt;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Bytes, Frame, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use prost::Message;
use tokio::sync::{watch, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::lnrpc::payment::PaymentStatus;

#[derive(Clone)]
struct MockLnd {
    payment: watch::Sender<Option<lnrpc::Payment>>,
    sends: Arc<AtomicUsize>,
    tracks: Arc<Mutex<Vec<routerrpc::TrackPaymentRequest>>>,
    silent: Arc<AtomicBool>,
    send_gate: Arc<Semaphore>,
}

impl MockLnd {
    fn set_status(&self, status: PaymentStatus) {
        self.payment.send_replace(Some(lnrpc::Payment {
            status: status as i32,
            value_msat: 10_000,
            fee_msat: 501,
            payment_preimage: if status == PaymentStatus::Succeeded {
                "preimage".to_string()
            } else {
                String::new()
            },
            ..Default::default()
        }));
    }

    async fn serve(&self, request: http::Request<Incoming>) -> http::Response<tonic::body::Body> {
        let path = request.uri().path().to_owned();
        let body = request.into_body().collect().await.unwrap().to_bytes();
        match path.as_str() {
            "/routerrpc.Router/SendPaymentV2" => {
                let request = routerrpc::SendPaymentRequest::decode(&body[5..]).unwrap();
                assert!(!request.cancelable);
                self.sends.fetch_add(1, Ordering::SeqCst);
                let _permit = self.send_gate.acquire().await.unwrap();
                self.set_status(PaymentStatus::Initiated);
            }
            "/routerrpc.Router/TrackPaymentV2" => {
                let request = routerrpc::TrackPaymentRequest::decode(&body[5..]).unwrap();
                assert!(!request.no_inflight_updates);
                self.tracks.lock().await.push(request);
                if self.payment.borrow().is_none() {
                    return http::Response::builder()
                        .header("content-type", "application/grpc")
                        .header("grpc-status", "5")
                        .body(tonic::body::Body::empty())
                        .unwrap();
                }
            }
            "/lnrpc.Lightning/SubscribeInvoices" => {
                return http::Response::builder()
                    .header("content-type", "application/grpc")
                    .body(tonic::body::Body::new(StreamBody::new(
                        futures::stream::pending::<Result<Frame<Bytes>, Infallible>>(),
                    )))
                    .unwrap();
            }
            _ => panic!("unexpected RPC {path}"),
        }
        let silent = self.silent.load(Ordering::SeqCst);
        let updates = futures::stream::unfold(
            (self.payment.subscribe(), true),
            move |(mut receiver, first)| async move {
                if silent {
                    futures::future::pending::<()>().await;
                }
                if !first && receiver.changed().await.is_err() {
                    return None;
                }
                let update = receiver.borrow_and_update().clone();
                let frame = match update {
                    Some(update) => {
                        let bytes = update.encode_to_vec();
                        let mut frame = vec![0];
                        frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                        frame.extend_from_slice(&bytes);
                        Frame::data(Bytes::from(frame))
                    }
                    None => {
                        let mut trailers = http::HeaderMap::new();
                        trailers.insert("grpc-status", "14".parse().unwrap());
                        Frame::trailers(trailers)
                    }
                };
                Some((Ok::<_, Infallible>(frame), (receiver, false)))
            },
        );
        http::Response::builder()
            .header("content-type", "application/grpc")
            .body(tonic::body::Body::new(StreamBody::new(updates)))
            .unwrap()
    }
}

async fn fixture() -> (Lnd, MockLnd, tokio_util::sync::DropGuard) {
    let state = MockLnd {
        payment: watch::channel(None).0,
        sends: Arc::new(AtomicUsize::new(0)),
        tracks: Arc::new(Mutex::new(Vec::new())),
        silent: Arc::new(AtomicBool::new(false)),
        send_gate: Arc::new(Semaphore::new(1)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let server_state = state.clone();
    tokio::spawn(async move {
        loop {
            let connection = tokio::select! {
                _ = task_cancel.cancelled() => break,
                connection = listener.accept() => connection.unwrap().0,
            };
            let state = server_state.clone();
            let cancel = task_cancel.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request| {
                    let state = state.clone();
                    async move { Ok::<_, Infallible>(state.serve(request).await) }
                });
                let builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
                tokio::select! {
                    _ = cancel.cancelled() => {},
                    _ = builder.serve_connection(TokioIo::new(connection), service) => {},
                }
            });
        }
    });
    let backend = Lnd {
        _address: address.clone(),
        _cert_file: PathBuf::new(),
        _macaroon_file: PathBuf::new(),
        lnd_client: client::Client::for_test(address.parse().unwrap()),
        fee_reserve: FeeReserve {
            min_fee_reserve: 0.into(),
            percent_fee_reserve: 0.0,
        },
        kv_store: Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap()),
        wait_invoice_cancel_token: CancellationToken::new(),
        wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
        settings: SettingsResponse {
            unit: "msat".to_string(),
            bolt11: None,
            bolt12: None,
            onchain: None,
            custom: Default::default(),
        },
        unit: CurrencyUnit::Msat,
        outgoing_tracking: Arc::new(outgoing::Tracking::default()),
    };
    (backend, state, cancel.drop_guard())
}

fn options(quote_id: QuoteId) -> OutgoingPaymentOptions {
    OutgoingPaymentOptions::Bolt11(Box::new(Bolt11OutgoingPaymentOptions {
        bolt11: cdk_fake_wallet::create_fake_invoice(10_000, "hold test".to_owned()),
        max_fee_amount: None,
        timeout_secs: None,
        melt_options: None,
        quote_id,
    }))
}

async fn next_event(events: &mut Pin<Box<dyn Stream<Item = Event> + Send>>) -> Event {
    tokio::time::timeout(Duration::from_secs(8), events.next())
        .await
        .expect("event without wallet polling")
        .expect("event stream alive")
}

#[tokio::test]
async fn pending_dispatch_and_status_complete_through_event_stream() {
    let (backend, state, _server) = fixture().await;
    let mut events = backend.wait_payment_event().await.unwrap();
    let quote_id = QuoteId::new();
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        backend.make_payment(&CurrencyUnit::Sat, options(quote_id.clone())),
    )
    .await
    .expect("hold dispatch must return promptly")
    .unwrap();
    assert_eq!(response.status, MeltQuoteState::Pending);
    assert!(response.payment_proof.is_none());
    state.set_status(PaymentStatus::InFlight);
    let checked = tokio::time::timeout(
        Duration::from_secs(3),
        backend.check_outgoing_payment(&response.payment_lookup_id),
    )
    .await
    .expect("hold status must return promptly")
    .unwrap();
    assert_eq!(checked.status, MeltQuoteState::Pending);
    state.set_status(PaymentStatus::Succeeded);
    match next_event(&mut events).await {
        Event::PaymentSuccessful {
            quote_id: actual,
            details,
        } => {
            assert_eq!(actual, quote_id);
            assert_eq!(details.status, MeltQuoteState::Paid);
            assert_eq!(details.payment_lookup_id, response.payment_lookup_id);
            assert_eq!(details.total_spent, Amount::new(10_501, CurrencyUnit::Msat));
            assert_eq!(details.payment_proof.as_deref(), Some("preimage"));
        }
        other => panic!("unexpected event {other:?}"),
    }
    assert_eq!(state.sends.load(Ordering::SeqCst), 1);
    backend.cancel_payment_event_stream();
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn pending_failure_is_reported_and_status_is_terminal() {
    let (backend, state, _server) = fixture().await;
    let mut events = backend.wait_payment_event().await.unwrap();
    let quote_id = QuoteId::new();
    let response = backend
        .make_payment(&CurrencyUnit::Sat, options(quote_id.clone()))
        .await
        .unwrap();
    assert_eq!(response.status, MeltQuoteState::Pending);
    state.set_status(PaymentStatus::Failed);
    assert!(matches!(next_event(&mut events).await,
        Event::PaymentFailed { quote_id: actual, .. } if actual == quote_id));
    let checked = backend
        .check_outgoing_payment(&response.payment_lookup_id)
        .await
        .unwrap();
    assert_eq!(checked.status, MeltQuoteState::Failed);
    assert_eq!(checked.total_spent.value(), 0);
    assert!(checked.payment_proof.is_none());
}

#[tokio::test]
async fn subscription_reconnect_replays_unacknowledged_event() {
    let (backend, state, _server) = fixture().await;
    let quote_id = QuoteId::new();
    backend
        .make_payment(&CurrencyUnit::Sat, options(quote_id.clone()))
        .await
        .unwrap();
    state.set_status(PaymentStatus::Succeeded);
    let mut events = backend.wait_payment_event().await.unwrap();
    assert!(matches!(next_event(&mut events).await,
        Event::PaymentSuccessful { quote_id: actual, .. } if actual == quote_id));
    drop(events); // Disconnect before the consumer acknowledges delivery.
    let mut events = backend.wait_payment_event().await.unwrap();
    assert!(matches!(next_event(&mut events).await,
        Event::PaymentSuccessful { quote_id: actual, .. } if actual == quote_id));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), events.next())
            .await
            .is_err()
    );
    drop(events);
    let mut events = backend.wait_payment_event().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), events.next())
            .await
            .is_err()
    );
    assert_eq!(state.sends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn transport_failure_reconnects_without_failing_or_resending_payment() {
    let (backend, state, _server) = fixture().await;
    let mut events = backend.wait_payment_event().await.unwrap();
    let quote_id = QuoteId::new();
    backend
        .make_payment(&CurrencyUnit::Sat, options(quote_id.clone()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while state.tracks.lock().await.len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    state.payment.send_replace(None); // Break the active gRPC stream.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), events.next())
            .await
            .is_err()
    );
    state.set_status(PaymentStatus::Succeeded);
    let event = tokio::time::timeout(Duration::from_secs(8), events.next())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event, Event::PaymentSuccessful { quote_id: actual, .. } if actual == quote_id)
    );
    assert!(state.tracks.lock().await.len() >= 3);
    assert_eq!(state.sends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unknown_and_timed_out_lookups_never_report_failure() {
    let (backend, state, _server) = fixture().await;
    let id = PaymentIdentifier::PaymentHash([1; 32]);
    assert_eq!(
        backend.check_outgoing_payment(&id).await.unwrap().status,
        MeltQuoteState::Unknown
    );
    state.set_status(PaymentStatus::InFlight);
    state.silent.store(true, Ordering::SeqCst);
    let check = backend.check_outgoing_payment(&id);
    tokio::pin!(check);
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut check)
        .await
        .is_err());
    tokio::time::pause();
    tokio::time::advance(outgoing::RPC_TIMEOUT).await;
    assert!(check.await.is_err());
}

#[tokio::test]
async fn retry_does_not_emit_previous_failure_while_dispatching() {
    let (backend, state, _server) = fixture().await;
    state.set_status(PaymentStatus::Failed);
    let gate = state.send_gate.acquire().await.unwrap();
    let quote_id = QuoteId::new();
    let task_backend = backend.clone();
    let task_quote = quote_id.clone();
    let dispatch = tokio::spawn(async move {
        task_backend
            .make_payment(&CurrencyUnit::Sat, options(task_quote))
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while state.sends.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Start the watcher while LND still exposes the previous failed attempt.
    let mut events = backend.wait_payment_event().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), events.next())
            .await
            .is_err()
    );
    drop(gate);
    assert_eq!(
        dispatch.await.unwrap().unwrap().status,
        MeltQuoteState::Pending
    );
    state.set_status(PaymentStatus::Succeeded);
    assert!(matches!(next_event(&mut events).await,
        Event::PaymentSuccessful { quote_id: actual, .. } if actual == quote_id));
}

#[tokio::test]
async fn ambiguous_dispatch_keeps_tracking_for_later_success() {
    let (backend, state, _server) = fixture().await;
    state.silent.store(true, Ordering::SeqCst);
    let quote_id = QuoteId::new();
    let task_backend = backend.clone();
    let task_quote = quote_id.clone();
    let dispatch = tokio::spawn(async move {
        task_backend
            .make_payment(&CurrencyUnit::Sat, options(task_quote))
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while state.sends.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::pause();
    tokio::time::advance(outgoing::RPC_TIMEOUT).await;
    assert!(dispatch.await.unwrap().is_err());
    tokio::time::resume();
    state.silent.store(false, Ordering::SeqCst);
    state.set_status(PaymentStatus::Succeeded);
    let mut events = backend.wait_payment_event().await.unwrap();
    assert!(matches!(next_event(&mut events).await,
        Event::PaymentSuccessful { quote_id: actual, .. } if actual == quote_id));
    assert_eq!(state.sends.load(Ordering::SeqCst), 1);
}
