//! Payment Stream
//!
//! This future Stream will wait events for a Mint Quote be paid. If it is for a repeatable payment
//! method it will stay open until the caller stops polling, cancels it, or wraps it in a timeout.
//!
//! Bolt11 will emit a single event.
use std::sync::Arc;
use std::task::Poll;

use cdk_common::database::wallet::Database as WalletDatabase;
use cdk_common::{database, Amount, Error, MeltQuoteState, MintQuoteState, NotificationPayload};
use futures::future::join_all;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, Stream, StreamExt};
use tokio_util::sync::CancellationToken;

use super::RecvFuture;
use crate::event::MintEvent;
use crate::wallet::issue::{apply_accounting_mint_quote_update, apply_mint_quote_response};
use crate::wallet::subscription::ActiveSubscription;
use crate::{Wallet, WalletSubscription};

type PaymentValue = (String, Option<Amount>);

struct ClassifiedPayment {
    value: PaymentValue,
    finalize: bool,
}

struct NextPayment {
    payment: Option<ClassifiedPayment>,
    subscriptions: Vec<ActiveSubscription>,
}

async fn apply_mint_quote_notification(
    localstore: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    event: &MintEvent<String>,
) -> bool {
    match event.inner() {
        NotificationPayload::MintQuoteBolt11Response(info) => {
            let quote_id = info.quote.clone();
            if let Ok(Some(mut quote)) = localstore.get_mint_quote(&quote_id).await {
                let applied = apply_mint_quote_response(
                    &mut quote,
                    &cdk_common::MintQuoteResponse::Bolt11(info.clone()),
                );
                if applied {
                    if let Err(e) = localstore.add_mint_quote(quote).await {
                        tracing::warn!("Failed to update quote state: {}", e);
                    }
                }
                return applied;
            }
        }
        NotificationPayload::MintQuoteBolt12Response(info) => {
            let quote_id = info.quote.clone();
            if let Ok(Some(mut quote)) = localstore.get_mint_quote(&quote_id).await {
                let applied = apply_accounting_mint_quote_update(
                    &mut quote,
                    info.amount_paid,
                    info.amount_issued,
                    info.updated_at,
                );
                if applied {
                    if let Err(e) = localstore.add_mint_quote(quote).await {
                        tracing::warn!("Failed to update quote state: {}", e);
                    }
                }
                return applied;
            }
        }
        NotificationPayload::MintQuoteOnchainResponse(info) => {
            let quote_id = info.quote.clone();
            if let Ok(Some(mut quote)) = localstore.get_mint_quote(&quote_id).await {
                let applied = apply_accounting_mint_quote_update(
                    &mut quote,
                    info.amount_paid,
                    info.amount_issued,
                    info.updated_at,
                );
                if applied {
                    if let Err(e) = localstore.add_mint_quote(quote).await {
                        tracing::warn!("Failed to update quote state: {}", e);
                    }
                }
                return applied;
            }
        }
        NotificationPayload::CustomMintQuoteResponse(_, info) => {
            let quote_id = info.quote.clone();
            if let Ok(Some(mut quote)) = localstore.get_mint_quote(&quote_id).await {
                let applied = apply_accounting_mint_quote_update(
                    &mut quote,
                    info.amount_paid,
                    info.amount_issued,
                    info.updated_at,
                );
                if applied {
                    if let Err(e) = localstore.add_mint_quote(quote).await {
                        tracing::warn!("Failed to update quote state: {}", e);
                    }
                }
                return applied;
            }
        }
        _ => (),
    }

    true
}

/// PaymentWaiter
#[allow(missing_debug_implementations)]
pub struct PaymentStream<'a> {
    wallet: &'a Wallet,
    filters: Option<Vec<WalletSubscription>>,
    is_finalized: bool,
    active_subscriptions: Option<Vec<ActiveSubscription>>,

    cancel_token: CancellationToken,

    subscriber_future: Option<RecvFuture<'a, Vec<ActiveSubscription>>>,
    payment_future: Option<RecvFuture<'a, NextPayment>>,
    cancellation_future: Option<RecvFuture<'a, ()>>,
}

impl<'a> PaymentStream<'a> {
    /// Creates a new instance of the
    pub fn new(wallet: &'a Wallet, filters: Vec<WalletSubscription>) -> Self {
        Self {
            wallet,
            filters: Some(filters),
            is_finalized: false,
            active_subscriptions: None,
            cancel_token: CancellationToken::new(),
            subscriber_future: None,
            payment_future: None,
            cancellation_future: None,
        }
    }

    /// Get cancellation token
    pub fn get_cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Starts or continues async subscription setup.
    ///
    /// Returns `Poll::Pending` while subscriptions are being created. Once setup completes,
    /// stores the active subscriptions and returns `Poll::Ready(())`.
    fn poll_init_subscription(&mut self, cx: &mut std::task::Context<'_>) -> Poll<()> {
        if let Some(filters) = self.filters.take() {
            let wallet = self.wallet;
            self.subscriber_future = Some(Box::pin(async move {
                let results = join_all(filters.into_iter().map(|w| wallet.subscribe(w))).await;
                results
                    .into_iter()
                    .filter_map(|r| match r {
                        Ok(sub) => Some(sub),
                        Err(e) => {
                            tracing::warn!("Failed to create subscription: {}", e);
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            }));
        }

        let Some(mut subscriber_future) = self.subscriber_future.take() else {
            return Poll::Ready(());
        };

        match subscriber_future.poll_unpin(cx) {
            Poll::Pending => {
                self.subscriber_future = Some(subscriber_future);
                Poll::Pending
            }
            Poll::Ready(active_subscriptions) => {
                self.active_subscriptions = Some(active_subscriptions);
                Poll::Ready(())
            }
        }
    }

    /// Checks if the stream has been externally cancelled
    fn poll_cancel(&mut self, cx: &mut std::task::Context<'_>) -> bool {
        let mut cancellation_future = self.cancellation_future.take().unwrap_or_else(|| {
            let cancel_token = self.cancel_token.clone();
            Box::pin(async move { cancel_token.cancelled().await })
        });

        if cancellation_future.poll_unpin(cx).is_ready() {
            self.subscriber_future = None;
            self.payment_future = None;
            self.active_subscriptions = None;
            true
        } else {
            self.cancellation_future = Some(cancellation_future);
            false
        }
    }

    fn poll_payment(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<PaymentValue, Error>>> {
        if self.payment_future.is_none() {
            let Some(active_subscriptions) = self.active_subscriptions.take() else {
                return Poll::Ready(Some(Err(Error::Internal)));
            };
            let wallet = self.wallet;
            self.payment_future = Some(Box::pin(async move {
                recv_next_payment(wallet, active_subscriptions).await
            }));
        }

        let Some(mut payment_future) = self.payment_future.take() else {
            return Poll::Ready(Some(Err(Error::Internal)));
        };
        match payment_future.poll_unpin(cx) {
            Poll::Pending => {
                self.payment_future = Some(payment_future);
                Poll::Pending
            }
            Poll::Ready(next_payment) => {
                let Some(payment) = next_payment.payment else {
                    self.is_finalized = true;
                    return Poll::Ready(None);
                };

                if payment.finalize {
                    self.is_finalized = true;
                } else {
                    self.active_subscriptions = Some(next_payment.subscriptions);
                }

                Poll::Ready(Some(Ok(payment.value)))
            }
        }
    }
}

impl Drop for PaymentStream<'_> {
    fn drop(&mut self) {
        self.cancel_token.cancel();
    }
}

impl Stream for PaymentStream<'_> {
    type Item = Result<PaymentValue, Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.is_finalized {
            // end of stream
            return Poll::Ready(None);
        }

        if this.poll_cancel(cx) {
            this.is_finalized = true;
            return Poll::Ready(None);
        }

        if this.poll_init_subscription(cx).is_pending() {
            return Poll::Pending;
        }

        this.poll_payment(cx)
    }
}

async fn recv_next_payment(
    wallet: &Wallet,
    mut subscriptions: Vec<ActiveSubscription>,
) -> NextPayment {
    loop {
        let Some(notification) = recv_payment_notification(&mut subscriptions).await else {
            return NextPayment {
                payment: None,
                subscriptions,
            };
        };

        tracing::debug!("Receive payment notification {:?}", notification);
        let Some(payment) = handle_payment_notification(wallet, notification).await else {
            continue;
        };

        return NextPayment {
            payment: Some(payment),
            subscriptions,
        };
    }
}

async fn recv_payment_notification(
    subscriptions: &mut [ActiveSubscription],
) -> Option<MintEvent<String>> {
    let mut futures: FuturesUnordered<_> = subscriptions.iter_mut().map(|sub| sub.recv()).collect();

    while let Some(res) = futures.next().await {
        if res.is_some() {
            return res;
        }
    }

    None
}

async fn handle_payment_notification(
    wallet: &Wallet,
    notification: MintEvent<String>,
) -> Option<ClassifiedPayment> {
    if !apply_mint_quote_notification(&wallet.localstore, &notification).await {
        return None;
    }

    classify_payment_notification(notification.into_inner())
}

fn classify_payment_notification(
    notification: NotificationPayload<String>,
) -> Option<ClassifiedPayment> {
    match notification {
        NotificationPayload::MintQuoteBolt11Response(info)
            if info.state == MintQuoteState::Paid =>
        {
            Some(ClassifiedPayment {
                value: (info.quote, None),
                finalize: true,
            })
        }
        NotificationPayload::MintQuoteBolt12Response(info) => {
            positive_unissued_amount(info.quote, info.amount_paid, info.amount_issued)
        }
        NotificationPayload::MintQuoteOnchainResponse(info) => {
            positive_unissued_amount(info.quote, info.amount_paid, info.amount_issued)
        }
        NotificationPayload::CustomMintQuoteResponse(_, info) => {
            positive_unissued_amount(info.quote, info.amount_paid, info.amount_issued)
        }
        NotificationPayload::MeltQuoteBolt11Response(info)
            if info.state == MeltQuoteState::Paid =>
        {
            Some(ClassifiedPayment {
                value: (info.quote, None),
                finalize: true,
            })
        }
        NotificationPayload::MeltQuoteBolt12Response(info)
            if info.state == MeltQuoteState::Paid =>
        {
            Some(ClassifiedPayment {
                value: (info.quote, None),
                finalize: true,
            })
        }
        NotificationPayload::MeltQuoteOnchainResponse(info)
            if info.state == MeltQuoteState::Paid =>
        {
            Some(ClassifiedPayment {
                value: (info.quote, None),
                finalize: true,
            })
        }
        NotificationPayload::CustomMeltQuoteResponse(_, info)
            if info.state == MeltQuoteState::Paid =>
        {
            Some(ClassifiedPayment {
                value: (info.quote, None),
                finalize: true,
            })
        }
        _ => None,
    }
}

fn positive_unissued_amount(
    quote: String,
    amount_paid: Amount,
    amount_issued: Amount,
) -> Option<ClassifiedPayment> {
    let to_be_issued = amount_paid.checked_sub(amount_issued)?;
    if to_be_issued > Amount::ZERO {
        Some(ClassifiedPayment {
            value: (quote, Some(to_be_issued)),
            finalize: false,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::str::FromStr;

    use cdk_common::mint_url::MintUrl;
    use cdk_common::{
        Amount, CurrencyUnit, MeltQuoteBolt11Response, MeltQuoteBolt12Response,
        MeltQuoteCustomResponse, MeltQuoteOnchainResponse, MeltQuoteState, MintQuoteBolt11Response,
        MintQuoteBolt12Response, MintQuoteCustomResponse, MintQuoteOnchainResponse, MintQuoteState,
        NotificationPayload, PaymentMethod,
    };

    use super::{classify_payment_notification, handle_payment_notification, ClassifiedPayment};
    use crate::event::MintEvent;
    use crate::nuts::nut30::MeltQuoteOnchainFeeOption;
    use crate::nuts::SecretKey;
    use crate::wallet::test_utils::{create_test_db, create_test_wallet};
    use crate::wallet::MintQuote;

    #[test]
    fn mint_bolt11_paid_emits_and_finalizes() {
        let payment = classify_payment_notification(NotificationPayload::MintQuoteBolt11Response(
            mint_bolt11_response("bolt11_quote", MintQuoteState::Paid),
        ))
        .expect("paid bolt11 quote should emit");

        assert_eq!(payment.value, ("bolt11_quote".to_string(), None));
        assert!(payment.finalize);
    }

    #[test]
    fn mint_bolt12_positive_unissued_amount_emits_and_remains_open() {
        let payment = classify_payment_notification(NotificationPayload::MintQuoteBolt12Response(
            mint_bolt12_response("bolt12_quote", 150, 100),
        ))
        .expect("positive unissued amount should emit");

        assert_eq!(
            payment.value,
            ("bolt12_quote".to_string(), Some(Amount::from(50u64)))
        );
        assert!(!payment.finalize);
    }

    #[test]
    fn mint_onchain_and_custom_positive_unissued_amount_emit_and_remain_open() {
        let pubkey = SecretKey::generate().public_key();

        let onchain = classify_payment_notification(NotificationPayload::MintQuoteOnchainResponse(
            MintQuoteOnchainResponse::<String> {
                quote: "onchain_quote".to_string(),
                request: "test_request".to_string(),
                unit: CurrencyUnit::Sat,
                expiry: None,
                pubkey,
                amount_paid: Amount::from(101u64),
                amount_issued: Amount::from(100u64),
                updated_at: 0,
            },
        ))
        .expect("positive unissued onchain amount should emit");

        assert_eq!(
            onchain.value,
            ("onchain_quote".to_string(), Some(Amount::from(1u64)))
        );
        assert!(!onchain.finalize);

        let custom = classify_payment_notification(NotificationPayload::CustomMintQuoteResponse(
            "custom".to_string(),
            MintQuoteCustomResponse::<String> {
                quote: "custom_quote".to_string(),
                request: "test_request".to_string(),
                amount: None,
                amount_paid: Amount::from(125u64),
                amount_issued: Amount::from(100u64),
                updated_at: 0,
                unit: Some(CurrencyUnit::Sat),
                expiry: None,
                pubkey: Some(pubkey),
                extra: serde_json::Value::Null,
            },
        ))
        .expect("positive unissued custom amount should emit");

        assert_eq!(
            custom.value,
            ("custom_quote".to_string(), Some(Amount::from(25u64)))
        );
        assert!(!custom.finalize);
    }

    #[test]
    fn melt_paid_notifications_emit_and_finalize() {
        let bolt11 = classify_payment_notification(NotificationPayload::MeltQuoteBolt11Response(
            melt_bolt11_response("melt_bolt11", MeltQuoteState::Paid),
        ))
        .expect("paid bolt11 melt quote should emit");
        let bolt12 = classify_payment_notification(NotificationPayload::MeltQuoteBolt12Response(
            melt_bolt12_response("melt_bolt12", MeltQuoteState::Paid),
        ))
        .expect("paid bolt12 melt quote should emit");
        let onchain = classify_payment_notification(NotificationPayload::MeltQuoteOnchainResponse(
            melt_onchain_response("melt_onchain", MeltQuoteState::Paid),
        ))
        .expect("paid onchain melt quote should emit");
        let custom = classify_payment_notification(NotificationPayload::CustomMeltQuoteResponse(
            "custom".to_string(),
            melt_custom_response("melt_custom", MeltQuoteState::Paid),
        ))
        .expect("paid custom melt quote should emit");

        for payment in [bolt11, bolt12, onchain, custom] {
            assert_eq!(payment.value.1, None);
            assert!(payment.finalize);
        }
    }

    #[test]
    fn non_terminal_and_unexpected_events_are_ignored() {
        assert!(
            classify_payment_notification(NotificationPayload::MintQuoteBolt11Response(
                mint_bolt11_response("unpaid_mint", MintQuoteState::Unpaid),
            ))
            .is_none()
        );
        assert!(
            classify_payment_notification(NotificationPayload::MeltQuoteBolt11Response(
                melt_bolt11_response("pending_melt", MeltQuoteState::Pending),
            ))
            .is_none()
        );
        assert!(
            classify_payment_notification(NotificationPayload::MintQuoteBolt12Response(
                mint_bolt12_response("issued_bolt12", 100, 100),
            ))
            .is_none()
        );
    }

    #[tokio::test]
    async fn driver_ignores_many_queued_events_before_valid_event_without_recursion() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db).await;
        let mut notifications = VecDeque::new();

        for _ in 0..10_000 {
            notifications.push_back(MintEvent::new(
                NotificationPayload::MintQuoteBolt11Response(mint_bolt11_response(
                    "ignored",
                    MintQuoteState::Unpaid,
                )),
            ));
        }

        notifications.push_back(MintEvent::new(
            NotificationPayload::MintQuoteBolt11Response(mint_bolt11_response(
                "paid",
                MintQuoteState::Paid,
            )),
        ));

        let payment = recv_next_test_payment(&wallet, notifications)
            .await
            .expect("valid event should still emit after ignored events");

        assert_eq!(payment.value, ("paid".to_string(), None));
        assert!(payment.finalize);
    }

    #[test]
    fn mint_quote_notification_underflow_does_not_panic() {
        let pubkey = SecretKey::generate().public_key();

        assert!(
            classify_payment_notification(NotificationPayload::MintQuoteBolt12Response(
                mint_bolt12_response("bolt12_quote", 50, 100),
            ))
            .is_none()
        );
        assert!(
            classify_payment_notification(NotificationPayload::MintQuoteOnchainResponse(
                MintQuoteOnchainResponse::<String> {
                    quote: "onchain_quote".to_string(),
                    request: "test_request".to_string(),
                    unit: CurrencyUnit::Sat,
                    expiry: None,
                    pubkey,
                    amount_paid: Amount::from(50u64),
                    amount_issued: Amount::from(100u64),
                    updated_at: 0,
                },
            ))
            .is_none()
        );
        assert!(
            classify_payment_notification(NotificationPayload::CustomMintQuoteResponse(
                "custom".to_string(),
                MintQuoteCustomResponse::<String> {
                    quote: "custom_quote".to_string(),
                    request: "test_request".to_string(),
                    amount: None,
                    amount_paid: Amount::from(50u64),
                    amount_issued: Amount::from(100u64),
                    updated_at: 0,
                    unit: Some(CurrencyUnit::Sat),
                    expiry: None,
                    pubkey: Some(pubkey),
                    extra: serde_json::Value::Null,
                },
            ))
            .is_none()
        );
    }

    fn mint_bolt11_response(quote: &str, state: MintQuoteState) -> MintQuoteBolt11Response<String> {
        MintQuoteBolt11Response {
            quote: quote.to_string(),
            request: "test_request".to_string(),
            amount: Some(Amount::from(100u64)),
            unit: Some(CurrencyUnit::Sat),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state,
            expiry: None,
            pubkey: None,
        }
    }

    fn mint_bolt12_response(
        quote: &str,
        amount_paid: u64,
        amount_issued: u64,
    ) -> MintQuoteBolt12Response<String> {
        MintQuoteBolt12Response {
            quote: quote.to_string(),
            request: "test_request".to_string(),
            amount: None,
            unit: CurrencyUnit::Sat,
            expiry: None,
            pubkey: SecretKey::generate().public_key(),
            amount_paid: Amount::from(amount_paid),
            amount_issued: Amount::from(amount_issued),
            updated_at: 0,
        }
    }

    fn melt_bolt11_response(quote: &str, state: MeltQuoteState) -> MeltQuoteBolt11Response<String> {
        MeltQuoteBolt11Response {
            quote: quote.to_string(),
            amount: Amount::from(100u64),
            fee_reserve: Amount::from(1u64),
            state,
            expiry: 1234,
            payment_preimage: None,
            change: None,
            request: Some("test_request".to_string()),
            unit: Some(CurrencyUnit::Sat),
        }
    }

    fn melt_bolt12_response(quote: &str, state: MeltQuoteState) -> MeltQuoteBolt12Response<String> {
        melt_bolt11_response(quote, state)
    }

    fn melt_onchain_response(
        quote: &str,
        state: MeltQuoteState,
    ) -> MeltQuoteOnchainResponse<String> {
        MeltQuoteOnchainResponse {
            quote: quote.to_string(),
            amount: Amount::from(100u64),
            unit: CurrencyUnit::Sat,
            state,
            expiry: 1234,
            request: "test_request".to_string(),
            fee_options: vec![MeltQuoteOnchainFeeOption {
                fee_index: 0,
                fee_reserve: Amount::from(1u64),
                estimated_blocks: 1,
            }],
            selected_fee_index: None,
            outpoint: None,
            change: None,
        }
    }

    fn melt_custom_response(quote: &str, state: MeltQuoteState) -> MeltQuoteCustomResponse<String> {
        MeltQuoteCustomResponse {
            quote: quote.to_string(),
            amount: Amount::from(100u64),
            fee_reserve: Some(Amount::from(1u64)),
            state,
            expiry: 1234,
            payment_preimage: None,
            change: None,
            request: Some("test_request".to_string()),
            unit: Some(CurrencyUnit::Sat),
            extra: serde_json::Value::Null,
        }
    }

    async fn recv_next_test_payment(
        wallet: &crate::Wallet,
        mut notifications: VecDeque<MintEvent<String>>,
    ) -> Option<ClassifiedPayment> {
        loop {
            let notification = notifications.pop_front()?;
            if let Some(payment) = handle_payment_notification(wallet, notification).await {
                return Some(payment);
            }
        }
    }

    #[tokio::test]
    async fn stale_mint_quote_notification_is_not_emitted() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db.clone()).await;
        let pubkey = SecretKey::generate().public_key();
        let quote_id = "custom_quote".to_string();
        let mut quote = MintQuote::new(
            quote_id.clone(),
            MintUrl::from_str("https://mint.example.com").expect("valid mint URL"),
            PaymentMethod::Custom("custom".to_string()),
            Some(Amount::from(200)),
            CurrencyUnit::Sat,
            "test_request".to_string(),
            1_700_000_000,
            None,
        );
        quote.amount_paid = Amount::from(150);
        quote.amount_issued = Amount::from(100);
        quote.updated_at = 10;
        quote.update_state_from_amounts();
        db.add_mint_quote(quote)
            .await
            .expect("mint quote should be stored");
        assert!(
            db.get_mint_quote(&quote_id)
                .await
                .expect("mint quote lookup should succeed")
                .is_some(),
            "mint quote should be readable before polling"
        );

        let event = MintEvent::new(NotificationPayload::CustomMintQuoteResponse(
            "custom".to_string(),
            MintQuoteCustomResponse::<String> {
                quote: quote_id.clone(),
                request: "test_request".to_string(),
                amount: None,
                amount_paid: Amount::from(120),
                amount_issued: Amount::from(100),
                updated_at: 11,
                unit: Some(CurrencyUnit::Sat),
                expiry: None,
                pubkey: Some(pubkey),
                extra: serde_json::Value::Null,
            },
        ));

        assert!(!super::apply_mint_quote_notification(&wallet.localstore, &event).await);

        let stored_quote = db
            .get_mint_quote(&quote_id)
            .await
            .expect("mint quote lookup should succeed")
            .expect("mint quote should exist");
        assert_eq!(stored_quote.amount_paid, Amount::from(150));
        assert_eq!(stored_quote.amount_issued, Amount::from(100));
        assert_eq!(stored_quote.updated_at, 10);
    }
}
