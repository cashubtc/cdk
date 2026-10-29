//! Quote administration service.

use cdk::mint::MintQuote;
use cdk::nuts::MintQuoteState;
use cdk::types::QuoteTTL;
use cdk_common::payment::WaitPaymentResponse;
use tonic::{Request, Response, Status};

use super::MintRPCServer;
use crate::quote::quote_service_server::QuoteService;

impl MintRPCServer {
    fn ensure_mint_quote_state_override_allowed(&self) -> Result<(), Status> {
        if !self.allow_mint_quote_payment_override {
            return Err(Status::permission_denied(
                "Mint quote state override is disabled",
            ));
        }

        Ok(())
    }

    /// Returns the mint's quote time-to-live settings
    async fn quote_ttl(&self) -> Result<QuoteTTL, Status> {
        self.mint
            .quote_ttl()
            .await
            .map_err(|err| Status::internal(err.to_string()))
    }

    /// Updates the mint's quote time-to-live settings, keeping the current
    /// value of any setting that is not given
    ///
    /// Returns the settings in effect after the update.
    async fn set_quote_ttl(
        &self,
        mint_ttl: Option<u64>,
        melt_ttl: Option<u64>,
    ) -> Result<QuoteTTL, Status> {
        let current_ttl = self.quote_ttl().await?;

        let quote_ttl = QuoteTTL {
            mint_ttl: mint_ttl.unwrap_or(current_ttl.mint_ttl),
            melt_ttl: melt_ttl.unwrap_or(current_ttl.melt_ttl),
        };

        self.mint
            .set_quote_ttl(quote_ttl)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(quote_ttl)
    }

    /// Records a manual payment override for a mint quote.
    ///
    /// Fixed-amount quotes default to the unpaid remainder and cannot be
    /// credited beyond their requested amount. An explicit BOLT11 amount must
    /// equal the full unpaid remainder. Amountless quotes require an explicit
    /// amount. Quotes that can receive multiple payments require the backend
    /// payment ID represented by the override so only that payment is
    /// deduplicated.
    async fn set_mint_quote_paid(
        &self,
        quote_id: &str,
        requested_amount: Option<u64>,
        requested_payment_id: Option<&str>,
    ) -> Result<MintQuote, Status> {
        self.ensure_mint_quote_state_override_allowed()?;

        let quote_id = quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote id".to_string()))?;
        if requested_payment_id.is_some_and(str::is_empty) {
            return Err(Status::invalid_argument(
                "Payment override ID must not be empty",
            ));
        }
        if requested_amount == Some(0) {
            return Err(Status::invalid_argument(
                "Payment override amount must be greater than zero",
            ));
        }

        let localstore = self.mint.localstore();
        let mut tx = localstore
            .begin_transaction()
            .await
            .map_err(|_| Status::internal("Could not start db transaction".to_string()))?;

        let mut mint_quote = tx
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::internal("Could not get quote in transaction".to_string()))?
            .ok_or(Status::invalid_argument("Could not find quote".to_string()))?;

        let amount_paid = mint_quote.amount_paid();
        let amount_issued = mint_quote.amount_issued();
        if mint_quote.amount.is_none() && requested_amount.is_none() {
            return Err(Status::failed_precondition(
                "Amountless quotes require a payment override amount",
            ));
        }
        if mint_quote.amount.is_none() && requested_payment_id.is_none() {
            return Err(Status::failed_precondition(
                "Amountless quotes require a backend payment ID",
            ));
        }
        if !mint_quote.payment_method.is_bolt11() && requested_payment_id.is_none() {
            return Err(Status::failed_precondition(
                "This payment method requires a backend payment ID",
            ));
        }

        let override_id = mint_quote.manual_payment_override_id(requested_payment_id);

        if let Some(payment) = mint_quote
            .payments
            .iter()
            .find(|payment| payment.payment_id == override_id)
        {
            if requested_amount.is_some_and(|amount| amount != payment.amount.value()) {
                return Err(Status::failed_precondition(
                    "Quote already has a payment override with a different amount",
                ));
            }

            tx.commit()
                .await
                .map_err(|_| Status::internal("Could not commit db transaction".to_string()))?;
            return Ok(mint_quote.inner());
        }

        if let Some(payment_id) = requested_payment_id {
            if let Some(payment) = mint_quote
                .payments
                .iter()
                .find(|payment| payment.payment_id == payment_id)
            {
                if requested_amount.is_some_and(|amount| amount != payment.amount.value()) {
                    return Err(Status::failed_precondition(
                        "Recorded backend payment has a different amount",
                    ));
                }

                tx.commit()
                    .await
                    .map_err(|_| Status::internal("Could not commit db transaction".to_string()))?;
                return Ok(mint_quote.inner());
            }
        }

        let payment_amount = match mint_quote.amount.as_ref() {
            Some(quote_amount) => {
                if quote_amount.value() == 0 {
                    return Err(Status::failed_precondition(
                        "Zero-amount quotes cannot be marked paid",
                    ));
                }

                // A fixed-amount quote whose recorded payments already cover its
                // amount is satisfied, including an overpaid quote.
                if amount_paid.value() >= quote_amount.value() {
                    if requested_amount.is_some() || requested_payment_id.is_some() {
                        return Err(Status::failed_precondition(
                            "Quote is already satisfied and cannot credit another payment",
                        ));
                    }

                    tx.commit().await.map_err(|_| {
                        Status::internal("Could not commit db transaction".to_string())
                    })?;
                    return Ok(mint_quote.inner());
                }

                let remaining = quote_amount.value() - amount_paid.value();
                let amount = requested_amount.unwrap_or(remaining);
                if amount > remaining {
                    return Err(Status::failed_precondition(
                        "Payment override would exceed the quote amount",
                    ));
                }
                if mint_quote.payment_method.is_bolt11() && amount != remaining {
                    return Err(Status::failed_precondition(
                        "BOLT11 payment override must equal the unpaid remainder",
                    ));
                }

                cdk::Amount::new(amount, mint_quote.unit.clone())
            }
            None => {
                let amount = requested_amount.ok_or_else(|| {
                    Status::failed_precondition(
                        "Amountless quotes require a payment override amount",
                    )
                })?;
                cdk::Amount::new(amount, mint_quote.unit.clone())
            }
        };

        let resulting_amount_paid = amount_paid
            .value()
            .checked_add(payment_amount.value())
            .ok_or_else(|| Status::internal("Payment amount overflow"))?;
        if amount_issued.value() > resulting_amount_paid {
            return Err(Status::failed_precondition(
                "Payment override would leave the quote over-issued",
            ));
        }

        let previous_state = mint_quote.state();
        let payment = WaitPaymentResponse {
            payment_identifier: mint_quote.request_lookup_id.clone(),
            payment_amount: payment_amount.clone(),
            payment_id: override_id.clone(),
        };
        let recorded = self
            .mint
            .pay_mint_quote(&mut tx, &mut mint_quote, payment)
            .await
            .map_err(|_| Status::internal("Could not process payment".to_string()))?;
        if !recorded {
            return Err(Status::internal("Payment override was not recorded"));
        }

        tx.commit()
            .await
            .map_err(|_| Status::internal("Could not commit db transaction".to_string()))?;

        tracing::info!(
            quote_id = %mint_quote.id,
            method = %mint_quote.payment_method,
            unit = %mint_quote.unit,
            payment_id = %override_id,
            amount = %payment_amount,
            previous_state = %previous_state,
            new_state = %mint_quote.state(),
            amount_paid = %mint_quote.amount_paid(),
            amount_issued = %mint_quote.amount_issued(),
            "mint quote payment override committed",
        );

        self.mint
            .pubsub_manager()
            .mint_quote_payment(&mint_quote, mint_quote.amount_paid());

        Ok(mint_quote.inner())
    }
}

#[tonic::async_trait]
impl QuoteService for MintRPCServer {
    /// Gets the mint's quote time-to-live settings
    async fn get_quote_ttl(
        &self,
        _request: Request<crate::quote::GetQuoteTtlRequest>,
    ) -> Result<Response<crate::quote::GetQuoteTtlResponse>, Status> {
        let ttl = self.quote_ttl().await?;

        Ok(Response::new(crate::quote::GetQuoteTtlResponse {
            mint_ttl: ttl.mint_ttl,
            melt_ttl: ttl.melt_ttl,
        }))
    }

    /// Updates the mint's quote time-to-live settings
    async fn update_quote_ttl(
        &self,
        request: Request<crate::quote::UpdateQuoteTtlRequest>,
    ) -> Result<Response<crate::quote::UpdateQuoteTtlResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let request = request.into_inner();

        let ttl = self
            .set_quote_ttl(request.mint_ttl, request.melt_ttl)
            .await?;

        Ok(Response::new(crate::quote::UpdateQuoteTtlResponse {
            mint_ttl: ttl.mint_ttl,
            melt_ttl: ttl.melt_ttl,
        }))
    }

    /// Force-marks a mint quote as paid
    async fn update_mint_quote_state(
        &self,
        request: Request<crate::quote::UpdateMintQuoteStateRequest>,
    ) -> Result<Response<crate::quote::UpdateMintQuoteStateResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let request = request.into_inner();

        match request.state() {
            crate::quote::MintQuoteState::Paid => (),
            crate::quote::MintQuoteState::Unpaid => {
                return Err(Status::invalid_argument(
                    "Cannot unpay a quote: payments cannot be retracted".to_string(),
                ));
            }
            crate::quote::MintQuoteState::Issued => {
                return Err(Status::invalid_argument(
                    "Cannot issue a quote: no signatures would back the issuance".to_string(),
                ));
            }
            crate::quote::MintQuoteState::Unspecified => {
                return Err(Status::invalid_argument(
                    "Quote state is required".to_string(),
                ));
            }
        }

        let mint_quote = self
            .set_mint_quote_paid(
                &request.quote_id,
                request.amount,
                request.payment_id.as_deref(),
            )
            .await?;

        Ok(Response::new(crate::quote::UpdateMintQuoteStateResponse {
            quote_id: mint_quote.id.to_string(),
            state: crate::quote::MintQuoteState::from(mint_quote.state()).into(),
        }))
    }
}

impl From<MintQuoteState> for crate::quote::MintQuoteState {
    fn from(state: MintQuoteState) -> Self {
        match state {
            MintQuoteState::Unpaid => Self::Unpaid,
            MintQuoteState::Paid => Self::Paid,
            MintQuoteState::Issued => Self::Issued,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use bip39::Mnemonic;
    use cdk::amount::SplitTarget;
    use cdk::mint::{MintInput, MintQuote};
    use cdk::nuts::{CurrencyUnit, MintRequest, PreMintSecrets};
    use cdk::Amount;
    use cdk_common::payment::{PaymentIdentifier, WaitPaymentResponse};
    use cdk_common::{MintQuoteBolt11Request, PaymentMethod, QuoteId};
    use cdk_postgres::MintPgDatabase;
    use tokio::sync::Barrier;

    use super::super::test_utils::{
        build_test_rpc_server_with_database, create_test_rpc_server,
        create_test_rpc_server_with_payment_delay, UNKNOWN_QUOTE_ID,
    };
    use super::super::{MintMutationGuard, MintMutationGuardError};
    use super::*;

    #[derive(Debug)]
    struct BarrierMutationGuard {
        barrier: Arc<Barrier>,
    }

    #[tonic::async_trait]
    impl MintMutationGuard for BarrierMutationGuard {
        async fn check(&self) -> Result<(), MintMutationGuardError> {
            self.barrier.wait().await;
            Ok(())
        }
    }

    async fn postgres_test_database(test_name: &str) -> Option<Arc<MintPgDatabase>> {
        let require_postgres = std::env::var("CDK_REQUIRE_POSTGRES_TESTS").is_ok();
        let base_url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL"))
            .ok();
        if base_url.is_none() && !require_postgres {
            eprintln!(
                "skipping PostgreSQL {test_name} test: set CDK_MINTD_DATABASE_URL or PG_DB_URL"
            );
            return None;
        }

        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let schema = format!(
            "mint_rpc_{}_{}_{}",
            test_name,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let base_url = base_url.unwrap_or_else(|| {
            "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                .to_owned()
        });
        Some(Arc::new(
            MintPgDatabase::new(format!("{base_url} schema={schema}").as_str())
                .await
                .expect("PostgreSQL test database"),
        ))
    }

    #[tokio::test]
    async fn test_quote_service_get_quote_ttl() {
        let server = create_test_rpc_server().await;

        let response =
            QuoteService::get_quote_ttl(&server, Request::new(crate::quote::GetQuoteTtlRequest {}))
                .await
                .unwrap();

        let response = response.into_inner();
        assert_eq!(response.mint_ttl, 10000);
        assert_eq!(response.melt_ttl, 10000);
    }

    #[tokio::test]
    async fn test_quote_service_update_quote_ttl_keeps_omitted_setting() {
        let server = create_test_rpc_server().await;

        let response = QuoteService::update_quote_ttl(
            &server,
            Request::new(crate::quote::UpdateQuoteTtlRequest {
                mint_ttl: Some(60),
                melt_ttl: None,
            }),
        )
        .await
        .unwrap();

        let response = response.into_inner();
        assert_eq!(response.mint_ttl, 60);
        assert_eq!(response.melt_ttl, 10000);

        let persisted =
            QuoteService::get_quote_ttl(&server, Request::new(crate::quote::GetQuoteTtlRequest {}))
                .await
                .unwrap()
                .into_inner();
        assert_eq!(persisted.mint_ttl, 60);
        assert_eq!(persisted.melt_ttl, 10000);
    }

    /// Creates a bolt11 mint quote for `amount` sats and returns its id
    async fn create_test_mint_quote(server: &MintRPCServer, amount: u64) -> String {
        let response = server
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: amount.into(),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap();

        response.quote().to_string()
    }

    /// Creates a mint quote for `payment_method`, amountless when `amount` is
    /// `None`, and returns its id.
    async fn create_test_mint_quote_with_method(
        server: &MintRPCServer,
        amount: Option<u64>,
        payment_method: PaymentMethod,
    ) -> String {
        let quote_id = QuoteId::new();
        let request_lookup_id = PaymentIdentifier::CustomId(format!("lookup-{quote_id}"));
        let now = cdk::util::unix_time();
        let quote = MintQuote::new(
            Some(quote_id.clone()),
            format!("custom:{quote_id}"),
            CurrencyUnit::Sat,
            amount.map(|value| Amount::new(value, CurrencyUnit::Sat)),
            now + 3_600,
            request_lookup_id,
            None,
            Amount::new(0, CurrencyUnit::Sat),
            Amount::new(0, CurrencyUnit::Sat),
            payment_method,
            now,
            now,
            Vec::new(),
            Vec::new(),
            None,
        );

        let mut tx = server.mint.localstore().begin_transaction().await.unwrap();
        tx.add_mint_quote(quote).await.unwrap();
        tx.commit().await.unwrap();

        quote_id.to_string()
    }

    /// Records a backend payment through the mint's payment notification path.
    async fn record_test_payment(
        server: &MintRPCServer,
        quote_id: &str,
        amount: u64,
        payment_id: &str,
    ) {
        let mut tx = server.mint.localstore().begin_transaction().await.unwrap();
        let mut quote = tx
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        let payment_identifier = quote.request_lookup_id.clone();
        server
            .mint
            .pay_mint_quote(
                &mut tx,
                &mut quote,
                WaitPaymentResponse {
                    payment_id: payment_id.to_string(),
                    payment_amount: Amount::new(amount, CurrencyUnit::Sat),
                    payment_identifier,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// Issues the full paid amount of a quote, leaving it in the issued state
    async fn issue_test_mint_quote(server: &MintRPCServer, quote_id: &str, amount: u64) {
        let keyset_id = *server
            .mint
            .get_active_keysets()
            .get(&CurrencyUnit::Sat)
            .unwrap();
        let keys = server
            .mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect());
        let premint = PreMintSecrets::random(
            keyset_id,
            Amount::from(amount),
            &SplitTarget::None,
            &fees.into(),
        )
        .unwrap();

        server
            .mint
            .process_mint_request(MintInput::Single(MintRequest {
                quote: quote_id.parse().unwrap(),
                outputs: premint.blinded_messages().to_vec(),
                signature: None,
            }))
            .await
            .unwrap();
    }

    /// Returns the amount recorded as paid against a quote
    async fn amount_paid(server: &MintRPCServer, quote_id: &str) -> u64 {
        server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .amount_paid()
            .value()
    }

    /// Builds a server that permits payment overrides and whose backend never
    /// reports a payment of its own.
    async fn create_override_test_server() -> MintRPCServer {
        create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true)
    }

    /// Requests a payment override and returns the reported quote state.
    async fn force_paid(
        server: &MintRPCServer,
        quote_id: &str,
        amount: Option<u64>,
        payment_id: Option<&str>,
    ) -> Result<crate::quote::MintQuoteState, Status> {
        QuoteService::update_mint_quote_state(
            server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.to_owned(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount,
                payment_id: payment_id.map(str::to_owned),
            }),
        )
        .await
        .map(|response| response.into_inner().state())
    }

    async fn get_quote(server: &MintRPCServer, quote_id: &str) -> MintQuote {
        server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap()
    }

    /// Recorded payments as `(payment_id, amount)`, in recording order.
    fn recorded_payments(quote: &MintQuote) -> Vec<(String, u64)> {
        quote
            .payments
            .iter()
            .map(|payment| (payment.payment_id.clone(), payment.amount.value()))
            .collect()
    }

    async fn assert_concurrent_payment_overrides_pay_once(server: &MintRPCServer) {
        let quote_id = create_test_mint_quote(server, 100).await;
        let first = tokio::spawn({
            let server = server.clone();
            let quote_id = quote_id.clone();
            async move { force_paid(&server, &quote_id, None, None).await }
        });
        let second = tokio::spawn({
            let server = server.clone();
            let quote_id = quote_id.clone();
            async move { force_paid(&server, &quote_id, None, None).await }
        });

        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.unwrap().unwrap(), crate::quote::MintQuoteState::Paid);
        assert_eq!(second.unwrap().unwrap(), crate::quote::MintQuoteState::Paid);

        let quote = get_quote(server, &quote_id).await;
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.amount_issued().value(), 0);
        assert_eq!(quote.payments.len(), 1);
    }

    #[tokio::test]
    async fn test_quote_service_update_mint_quote_state_marks_quote_paid() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;

        assert_eq!(amount_paid(&server, &quote_id).await, 0);

        let response = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: None,
                payment_id: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.quote_id, quote_id);
        assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);
        assert_eq!(amount_paid(&server, &quote_id).await, 100);
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_rejects_invalid_arguments() {
        let server = create_override_test_server().await;
        let quote_id = create_test_mint_quote(&server, 100).await;

        for (amount, payment_id, expected) in [
            (
                Some(0),
                None,
                "Payment override amount must be greater than zero",
            ),
            (None, Some(""), "Payment override ID must not be empty"),
        ] {
            let status = force_paid(&server, &quote_id, amount, payment_id)
                .await
                .expect_err("invalid override arguments are rejected");
            assert_eq!(status.code(), tonic::Code::InvalidArgument);
            assert_eq!(status.message(), expected);
        }
        assert!(get_quote(&server, &quote_id).await.payments.is_empty());
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_bolt11_requires_full_remainder() {
        let server = create_override_test_server().await;
        let quote_id = create_test_mint_quote(&server, 100).await;

        let status = force_paid(&server, &quote_id, Some(60), None)
            .await
            .expect_err("a BOLT11 override must satisfy the unpaid remainder");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert!(get_quote(&server, &quote_id).await.payments.is_empty());

        assert_eq!(
            force_paid(&server, &quote_id, Some(100), None)
                .await
                .unwrap(),
            crate::quote::MintQuoteState::Paid
        );
        let quote = get_quote(&server, &quote_id).await;
        assert_eq!(
            recorded_payments(&quote),
            vec![(quote.manual_payment_override_id(None), 100)]
        );
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_multi_payment_quote_deduplicates_backend_payment() {
        let server = create_override_test_server().await;
        let quote_id =
            create_test_mint_quote_with_method(&server, Some(100), PaymentMethod::BOLT12).await;

        let status = force_paid(&server, &quote_id, None, None)
            .await
            .expect_err("a multi-payment quote requires a payment ID");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);

        assert_eq!(
            force_paid(&server, &quote_id, None, Some("bolt12-payment"))
                .await
                .unwrap(),
            crate::quote::MintQuoteState::Paid
        );
        record_test_payment(&server, &quote_id, 100, "bolt12-payment").await;
        record_test_payment(&server, &quote_id, 25, "distinct-bolt12-payment").await;

        let quote = get_quote(&server, &quote_id).await;
        assert_eq!(quote.amount_paid().value(), 125);
        assert_eq!(
            recorded_payments(&quote),
            vec![
                (
                    quote.manual_payment_override_id(Some("bolt12-payment")),
                    100
                ),
                ("distinct-bolt12-payment".to_owned(), 25),
            ]
        );
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_amountless_quote_requires_amount_and_payment_id() {
        let server = create_override_test_server().await;
        let quote_id =
            create_test_mint_quote_with_method(&server, None, PaymentMethod::from("onchain")).await;

        for (amount, payment_id) in [(None, Some("onchain-payment")), (Some(75), None)] {
            let status = force_paid(&server, &quote_id, amount, payment_id)
                .await
                .expect_err("an amountless override needs an amount and a payment ID");
            assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        }

        assert_eq!(
            force_paid(&server, &quote_id, Some(75), Some("onchain-payment"))
                .await
                .unwrap(),
            crate::quote::MintQuoteState::Paid
        );
        record_test_payment(&server, &quote_id, 75, "onchain-payment").await;
        record_test_payment(&server, &quote_id, 25, "later-onchain-payment").await;

        let quote = get_quote(&server, &quote_id).await;
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(
            recorded_payments(&quote),
            vec![
                (
                    quote.manual_payment_override_id(Some("onchain-payment")),
                    75
                ),
                ("later-onchain-payment".to_owned(), 25),
            ]
        );
    }

    #[tokio::test]
    async fn test_quote_service_backend_amount_is_authoritative_over_override() {
        let server = create_override_test_server().await;

        // A larger backend payment credits the difference once, even if redelivered.
        let quote_id =
            create_test_mint_quote_with_method(&server, None, PaymentMethod::from("onchain")).await;
        force_paid(&server, &quote_id, Some(75), Some("onchain-payment"))
            .await
            .unwrap();
        record_test_payment(&server, &quote_id, 750, "onchain-payment").await;
        record_test_payment(&server, &quote_id, 750, "onchain-payment").await;

        let quote = get_quote(&server, &quote_id).await;
        assert_eq!(quote.amount_paid().value(), 750);
        assert_eq!(
            recorded_payments(&quote),
            vec![
                (
                    quote.manual_payment_override_id(Some("onchain-payment")),
                    75
                ),
                ("onchain-payment".to_owned(), 675),
            ]
        );

        // A smaller backend payment cannot retract what the override credited.
        let quote_id =
            create_test_mint_quote_with_method(&server, None, PaymentMethod::from("onchain")).await;
        force_paid(&server, &quote_id, Some(75), Some("onchain-payment"))
            .await
            .unwrap();
        record_test_payment(&server, &quote_id, 50, "onchain-payment").await;

        let quote = get_quote(&server, &quote_id).await;
        assert_eq!(quote.amount_paid().value(), 75);
        assert_eq!(quote.payments.len(), 1);
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_credits_at_most_fixed_quote_remainder() {
        let server = create_override_test_server().await;
        let quote_id = create_test_mint_quote_with_method(
            &server,
            Some(100),
            PaymentMethod::Custom("test".to_owned()),
        )
        .await;
        record_test_payment(&server, &quote_id, 40, "backend-partial-payment").await;

        let status = force_paid(&server, &quote_id, Some(61), Some("manual-payment"))
            .await
            .expect_err("the override must not exceed the fixed quote amount");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);

        assert_eq!(
            force_paid(&server, &quote_id, None, Some("manual-payment"))
                .await
                .unwrap(),
            crate::quote::MintQuoteState::Paid
        );
        let quote = get_quote(&server, &quote_id).await;
        assert_eq!(
            recorded_payments(&quote),
            vec![
                ("backend-partial-payment".to_owned(), 40),
                (quote.manual_payment_override_id(Some("manual-payment")), 60),
            ]
        );
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_supports_partial_custom_payment() {
        let server = create_override_test_server().await;
        let quote_id = create_test_mint_quote_with_method(
            &server,
            Some(100),
            PaymentMethod::Custom("test".to_owned()),
        )
        .await;

        force_paid(&server, &quote_id, Some(60), Some("partial-custom-payment"))
            .await
            .expect("custom methods support partial payments");
        record_test_payment(&server, &quote_id, 40, "distinct-custom-payment").await;

        let quote = get_quote(&server, &quote_id).await;
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(
            recorded_payments(&quote),
            vec![
                (
                    quote.manual_payment_override_id(Some("partial-custom-payment")),
                    60
                ),
                ("distinct-custom-payment".to_owned(), 40),
            ]
        );
    }

    #[tokio::test]
    async fn test_quote_service_matching_payment_retries_are_idempotent() {
        let server = create_override_test_server().await;

        // A retry naming an already recorded backend payment changes nothing.
        let amountless_quote_id =
            create_test_mint_quote_with_method(&server, None, PaymentMethod::from("onchain")).await;
        record_test_payment(&server, &amountless_quote_id, 75, "amountless-payment").await;
        let fixed_quote_id = create_test_mint_quote_with_method(
            &server,
            Some(100),
            PaymentMethod::Custom("test".to_owned()),
        )
        .await;
        record_test_payment(&server, &fixed_quote_id, 100, "fixed-payment").await;
        for (quote_id, amount, payment_id) in [
            (&amountless_quote_id, 75, "amountless-payment"),
            (&fixed_quote_id, 100, "fixed-payment"),
        ] {
            force_paid(&server, quote_id, Some(amount), Some(payment_id))
                .await
                .expect("a matching backend payment retry is idempotent");
            assert_eq!(
                recorded_payments(&get_quote(&server, quote_id).await),
                vec![(payment_id.to_owned(), amount)]
            );
        }

        // Repeating the same override changes nothing.
        let override_quote_id = create_test_mint_quote_with_method(
            &server,
            Some(100),
            PaymentMethod::Custom("test".to_owned()),
        )
        .await;
        for _ in 0..2 {
            force_paid(
                &server,
                &override_quote_id,
                Some(100),
                Some("override-payment"),
            )
            .await
            .expect("a matching manual override retry is idempotent");
        }
        let quote = get_quote(&server, &override_quote_id).await;
        assert_eq!(
            recorded_payments(&quote),
            vec![(
                quote.manual_payment_override_id(Some("override-payment")),
                100
            )]
        );
    }

    #[tokio::test]
    async fn test_quote_service_satisfied_quote_rejects_new_explicit_payment() {
        let server = create_override_test_server().await;
        let quote_id = create_test_mint_quote(&server, 100).await;
        record_test_payment(&server, &quote_id, 100, "backend-payment").await;

        let status = force_paid(&server, &quote_id, Some(50), None)
            .await
            .expect_err("a satisfied quote cannot credit a new explicit payment");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert_eq!(
            recorded_payments(&get_quote(&server, &quote_id).await),
            vec![("backend-payment".to_owned(), 100)]
        );
    }

    #[tokio::test]
    async fn test_quote_service_satisfied_quote_without_arguments_is_idempotent() {
        use crate::quote::MintQuoteState::{Issued, Paid};

        // (case, backend payment or `None` for an earlier override, issue the quote, state)
        for (case, backend_payment, issue, expected_state) in [
            ("override paid", None, false, Paid),
            ("backend paid", Some(100), false, Paid),
            ("backend overpaid", Some(110), false, Paid),
            ("backend paid and issued", Some(100), true, Issued),
            // BOLT11 issues the quote amount, so the overpayment leaves it Paid
            ("backend overpaid and issued", Some(110), true, Paid),
        ] {
            let server = create_override_test_server().await;
            let quote_id = create_test_mint_quote(&server, 100).await;
            match backend_payment {
                Some(amount) => {
                    record_test_payment(&server, &quote_id, amount, "backend-payment").await;
                }
                None => {
                    force_paid(&server, &quote_id, None, None).await.unwrap();
                }
            }
            if issue {
                issue_test_mint_quote(&server, &quote_id, 100).await;
            }
            let before = get_quote(&server, &quote_id).await;

            assert_eq!(
                force_paid(&server, &quote_id, None, None).await.unwrap(),
                expected_state,
                "{case}"
            );
            let after = get_quote(&server, &quote_id).await;
            assert_eq!(after.amount_paid(), before.amount_paid(), "{case}");
            assert_eq!(after.amount_issued(), before.amount_issued(), "{case}");
            assert_eq!(
                recorded_payments(&after),
                recorded_payments(&before),
                "{case}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_quote_service_postgres_override_racing_backend_payment_credits_once() {
        let Some(db) = postgres_test_database("override_backend_race").await else {
            return;
        };
        let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
        let barrier = Arc::new(Barrier::new(2));
        let server = build_test_rpc_server_with_database(db, &seed, 3600)
            .await
            .with_mint_quote_payment_override(true)
            .with_mutation_guard(Arc::new(BarrierMutationGuard {
                barrier: barrier.clone(),
            }));
        let quote_id = create_test_mint_quote(&server, 100).await;
        let backend_payment = WaitPaymentResponse {
            payment_id: "postgres-backend-payment".to_string(),
            payment_amount: Amount::new(100, CurrencyUnit::Sat),
            payment_identifier: get_quote(&server, &quote_id).await.request_lookup_id,
        };

        // The barrier releases the operator RPC and the backend event together.
        let operator = tokio::spawn({
            let server = server.clone();
            let quote_id = quote_id.clone();
            async move { force_paid(&server, &quote_id, None, None).await }
        });
        let backend = tokio::spawn({
            let mint = server.mint.clone();
            async move {
                barrier.wait().await;
                mint.pay_mint_quote_for_request_id(backend_payment).await
            }
        });

        let (operator, backend) = tokio::join!(operator, backend);
        assert_eq!(
            operator.unwrap().unwrap(),
            crate::quote::MintQuoteState::Paid
        );
        backend.unwrap().unwrap();

        let quote = get_quote(&server, &quote_id).await;
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.amount_issued().value(), 0);
        assert_eq!(quote.payments.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_quote_service_postgres_concurrent_payment_overrides_pay_once() {
        let Some(db) = postgres_test_database("concurrent_overrides").await else {
            return;
        };
        let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
        let barrier = Arc::new(Barrier::new(2));
        let server = build_test_rpc_server_with_database(db, &seed, 3600)
            .await
            .with_mint_quote_payment_override(true)
            .with_mutation_guard(Arc::new(BarrierMutationGuard { barrier }));

        assert_concurrent_payment_overrides_pay_once(&server).await;
    }

    #[tokio::test]
    async fn test_quote_service_update_mint_quote_state_reports_state_of_issued_quote() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 32).await;

        QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: None,
                payment_id: None,
            }),
        )
        .await
        .unwrap();
        issue_test_mint_quote(&server, &quote_id, 32).await;

        // The request asks for Paid; an already-issued quote stays Issued
        let response = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: None,
                payment_id: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.state(), crate::quote::MintQuoteState::Issued);
        assert_eq!(amount_paid(&server, &quote_id).await, 32);
    }

    #[tokio::test]
    async fn test_quote_service_update_mint_quote_state_rejects_unsupported_states() {
        let server = create_test_rpc_server().await;

        // An unsupported state is rejected before the quote is looked up
        for (state, expected) in [
            (
                crate::quote::MintQuoteState::Unpaid,
                "Cannot unpay a quote: payments cannot be retracted",
            ),
            (
                crate::quote::MintQuoteState::Issued,
                "Cannot issue a quote: no signatures would back the issuance",
            ),
            (
                crate::quote::MintQuoteState::Unspecified,
                "Quote state is required",
            ),
        ] {
            let status = QuoteService::update_mint_quote_state(
                &server,
                Request::new(crate::quote::UpdateMintQuoteStateRequest {
                    quote_id: UNKNOWN_QUOTE_ID.to_string(),
                    state: state.into(),
                    amount: None,
                    payment_id: None,
                }),
            )
            .await
            .unwrap_err();

            assert_eq!(status.code(), tonic::Code::InvalidArgument);
            assert_eq!(
                status.message(),
                expected,
                "unexpected rejection for {state:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_quote_service_update_mint_quote_state_unknown_quote() {
        let server = create_test_rpc_server()
            .await
            .with_mint_quote_payment_override(true);

        let status = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: UNKNOWN_QUOTE_ID.to_string(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: None,
                payment_id: None,
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(status.message(), "Could not find quote");
    }

    #[tokio::test]
    async fn test_mint_quote_state_overrides_are_disabled_by_default() {
        let server = create_test_rpc_server_with_payment_delay(3600).await;
        let quote_id = create_test_mint_quote(&server, 100).await;

        let status = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: None,
                payment_id: None,
            }),
        )
        .await
        .expect_err("payment override should be disabled");

        assert_eq!(status.code(), tonic::Code::PermissionDenied);
        assert_eq!(status.message(), "Mint quote state override is disabled");
        assert_eq!(amount_paid(&server, &quote_id).await, 0);
    }
}
