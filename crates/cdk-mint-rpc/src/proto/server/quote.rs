//! Quote administration service.

use cdk::mint::MintQuote;
use cdk::nuts::MintQuoteState;
use cdk::types::QuoteTTL;
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
    /// credited beyond their requested amount. Amountless quotes require an
    /// explicit amount. Quotes that can receive multiple payments require the
    /// backend payment ID represented by the override so only that payment is
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
        if requested_payment_id.is_some_and(str::is_empty) {
            return Err(Status::failed_precondition(
                "Payment override ID must not be empty",
            ));
        }
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
                    tx.commit().await.map_err(|_| {
                        Status::internal("Could not commit db transaction".to_string())
                    })?;
                    return Ok(mint_quote.inner());
                }

                let remaining = quote_amount.value() - amount_paid.value();
                let amount = requested_amount.unwrap_or(remaining);
                if amount == 0 {
                    return Err(Status::failed_precondition(
                        "Payment override amount must be greater than zero",
                    ));
                }
                if amount > remaining {
                    return Err(Status::failed_precondition(
                        "Payment override would exceed the quote amount",
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
                if amount == 0 {
                    return Err(Status::failed_precondition(
                        "Payment override amount must be greater than zero",
                    ));
                }

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

        mint_quote
            .add_payment(payment_amount, override_id, None)
            .map_err(|_| Status::internal("Could not process payment".to_string()))?;
        tx.update_mint_quote(&mut mint_quote)
            .await
            .map_err(|_| Status::internal("Could not persist payment".to_string()))?;

        tx.commit()
            .await
            .map_err(|_| Status::internal("Could not commit db transaction".to_string()))?;

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
    use std::fs::remove_file;
    use std::path::PathBuf;
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
    use cdk_sqlite::mint::MintSqliteDatabase;
    use tokio::sync::Barrier;

    use super::super::test_utils::{
        build_test_rpc_server_with_database, build_test_rpc_server_with_sqlite_database,
        create_test_rpc_server, create_test_rpc_server_with_payment_delay,
        create_test_rpc_server_with_sqlite_database, UNKNOWN_QUOTE_ID,
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

    fn file_backed_test_database_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cdk-mint-rpc-{test_name}-{}.sqlite",
            QuoteId::new()
        ))
    }

    async fn assert_mint_released_database(db: &Arc<MintSqliteDatabase>, instance: &str) {
        // Dropping the mint aborts its embedded signatory task, which releases its
        // database handle once a runtime worker processes the abort. Yielding on
        // this thread does not bound how long that takes, so wait on a deadline.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if Arc::strong_count(db) == 1 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

        assert_eq!(
            Arc::strong_count(db),
            1,
            "{instance} retained a database handle"
        );
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

    async fn assert_concurrent_payment_overrides_pay_once(server: &MintRPCServer) {
        let quote_id = create_test_mint_quote(server, 100).await;
        let first_server = server.clone();
        let first_quote_id = quote_id.clone();
        let first = tokio::spawn(async move {
            QuoteService::update_mint_quote_state(
                &first_server,
                Request::new(crate::quote::UpdateMintQuoteStateRequest {
                    quote_id: first_quote_id,
                    state: crate::quote::MintQuoteState::Paid.into(),
                    amount: None,
                    payment_id: None,
                }),
            )
            .await
        });
        let second_server = server.clone();
        let second_quote_id = quote_id.clone();
        let second = tokio::spawn(async move {
            QuoteService::update_mint_quote_state(
                &second_server,
                Request::new(crate::quote::UpdateMintQuoteStateRequest {
                    quote_id: second_quote_id,
                    state: crate::quote::MintQuoteState::Paid.into(),
                    amount: None,
                    payment_id: None,
                }),
            )
            .await
        });

        let (first, second) = tokio::join!(first, second);
        assert_eq!(
            first.unwrap().unwrap().into_inner().state(),
            crate::quote::MintQuoteState::Paid
        );
        assert_eq!(
            second.unwrap().unwrap().into_inner().state(),
            crate::quote::MintQuoteState::Paid
        );

        let quote = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.amount_issued().value(), 0);
        assert_eq!(quote.amount_mintable().value(), 100);
        assert_eq!(quote.payments.len(), 1);
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

    /// Creates a fixed-amount custom mint quote and returns its id.
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

    /// Records a payment through the mint's normal durable accounting path.
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
    async fn test_quote_service_force_paid_supports_fixed_bolt12_before_backend_payment() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id =
            create_test_mint_quote_with_method(&server, Some(100), PaymentMethod::BOLT12).await;

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
        .expect_err("a multi-payment quote requires a payment ID");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);

        let response = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: None,
                payment_id: Some("bolt12-payment".to_string()),
            }),
        )
        .await
        .expect("a fixed-amount BOLT12 quote can be force-paid")
        .into_inner();

        assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);
        record_test_payment(&server, &quote_id, 100, "bolt12-payment").await;
        record_test_payment(&server, &quote_id, 25, "distinct-bolt12-payment").await;

        let persisted = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.amount_paid().value(), 125);
        assert_eq!(persisted.amount_mintable().value(), 125);
        assert_eq!(persisted.payment_ids().len(), 2);
        assert!(persisted.has_manual_payment_override());
        assert!(persisted.has_manual_payment_override_for("bolt12-payment"));
        assert_eq!(persisted.payments[1].payment_id, "distinct-bolt12-payment");
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_after_partial_payment_does_not_over_credit() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote_with_method(
            &server,
            Some(100),
            PaymentMethod::Custom("test".to_string()),
        )
        .await;
        record_test_payment(&server, &quote_id, 40, "backend-partial-payment").await;

        let response = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: None,
                payment_id: Some("manual-remainder-payment".to_string()),
            }),
        )
        .await
        .expect("the override should credit only the unpaid remainder")
        .into_inner();

        assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);

        let persisted = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.amount_paid().value(), 100);
        assert_eq!(persisted.amount_mintable().value(), 100);
        assert_eq!(persisted.payment_ids().len(), 2);
        assert_eq!(persisted.payments[0].amount.value(), 40);
        assert_eq!(persisted.payments[1].amount.value(), 60);
        assert!(persisted.has_manual_payment_override());
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_rejects_amount_above_fixed_quote_remainder() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote_with_method(
            &server,
            Some(100),
            PaymentMethod::Custom("test".to_string()),
        )
        .await;
        record_test_payment(&server, &quote_id, 40, "backend-partial-payment").await;

        let status = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: Some(61),
                payment_id: Some("manual-remainder-payment".to_string()),
            }),
        )
        .await
        .expect_err("the override must not exceed the fixed quote amount");

        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        let persisted = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.amount_paid().value(), 40);
        assert_eq!(persisted.payment_ids().len(), 1);
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_tops_up_partial_bolt11_payment() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;
        record_test_payment(&server, &quote_id, 40, "backend-partial-payment").await;

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
        .expect("the override should credit only the unpaid remainder")
        .into_inner();

        assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);

        let persisted = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.amount_paid().value(), 100);
        assert_eq!(persisted.amount_mintable().value(), 100);
        assert_eq!(persisted.payment_ids().len(), 2);
        assert_eq!(persisted.payments[1].amount.value(), 60);
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_amountless_onchain_quote_requires_amount() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id =
            create_test_mint_quote_with_method(&server, None, PaymentMethod::from("onchain")).await;

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
        .expect_err("an amountless quote cannot infer a payment amount");

        assert_eq!(status.code(), tonic::Code::FailedPrecondition);

        let response = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: Some(75),
                payment_id: Some("onchain-payment".to_string()),
            }),
        )
        .await
        .expect("an explicit amount can force-pay an on-chain quote")
        .into_inner();

        assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);
        record_test_payment(&server, &quote_id, 75, "onchain-payment").await;
        record_test_payment(&server, &quote_id, 25, "later-onchain-payment").await;

        let persisted = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.amount_paid().value(), 100);
        assert_eq!(persisted.amount_mintable().value(), 100);
        assert_eq!(persisted.payment_ids().len(), 2);
        assert!(persisted.has_manual_payment_override());
        assert!(persisted.has_manual_payment_override_for("onchain-payment"));
        assert_eq!(persisted.payments[1].payment_id, "later-onchain-payment");
    }

    #[tokio::test]
    async fn test_quote_service_amountless_backend_payment_then_override_is_idempotent() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id =
            create_test_mint_quote_with_method(&server, None, PaymentMethod::from("onchain")).await;
        record_test_payment(&server, &quote_id, 75, "onchain-payment").await;

        let response = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: Some(75),
                payment_id: Some("onchain-payment".to_string()),
            }),
        )
        .await
        .expect("the existing backend payment should make the override a no-op")
        .into_inner();

        assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);
        let persisted = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.amount_paid().value(), 75);
        assert_eq!(persisted.payment_ids().len(), 1);
        assert!(!persisted.has_manual_payment_override());
    }

    #[tokio::test]
    async fn test_quote_service_backend_paid_quote_is_idempotent() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;
        record_test_payment(&server, &quote_id, 100, "backend-payment").await;

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

        let quote = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.payments.len(), 1);
    }

    #[tokio::test]
    async fn test_quote_service_overpaid_bolt11_quote_is_idempotent() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;
        record_test_payment(&server, &quote_id, 110, "backend-overpayment").await;

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
        .expect("an overpaid quote is already satisfied")
        .into_inner();

        assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);

        let quote = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quote.amount_paid().value(), 110);
        assert_eq!(quote.amount_issued().value(), 0);
        assert_eq!(quote.payments.len(), 1);
        assert_eq!(quote.payments[0].payment_id, "backend-overpayment");
    }

    #[tokio::test]
    async fn test_quote_service_overpaid_issued_bolt11_quote_is_idempotent() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;
        record_test_payment(&server, &quote_id, 110, "backend-overpayment").await;
        issue_test_mint_quote(&server, &quote_id, 100).await;

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
        .expect("an overpaid, issued quote is already satisfied")
        .into_inner();

        // BOLT11 issues the quote amount, so the overpayment leaves it Paid
        assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);

        let quote = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quote.amount_paid().value(), 110);
        assert_eq!(quote.amount_issued().value(), 100);
        assert_eq!(quote.payments.len(), 1);
    }

    #[tokio::test]
    async fn test_quote_service_backend_paid_issued_quote_is_idempotent() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;
        record_test_payment(&server, &quote_id, 100, "backend-payment").await;
        issue_test_mint_quote(&server, &quote_id, 100).await;

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
        .expect("an issued quote is already satisfied")
        .into_inner();

        assert_eq!(response.state(), crate::quote::MintQuoteState::Issued);

        let quote = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.amount_issued().value(), 100);
        assert_eq!(quote.amount_mintable().value(), 0);
        assert_eq!(quote.payments.len(), 1);
        assert_eq!(quote.payments[0].payment_id, "backend-payment");
    }

    #[tokio::test]
    async fn test_quote_service_operator_first_backend_event_is_idempotent() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;

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

        record_test_payment(&server, &quote_id, 100, "backend-payment").await;

        let quote = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.payments.len(), 1);
    }

    #[tokio::test]
    async fn test_quote_service_payment_override_persists_across_application_restart() {
        let path = file_backed_test_database_path("override-restart");
        let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");

        let quote_id = {
            let db = Arc::new(MintSqliteDatabase::new(&path).await.unwrap());
            let server = create_test_rpc_server_with_sqlite_database(db.clone(), &seed, 3600)
                .await
                .with_mint_quote_payment_override(true);
            let quote_id = create_test_mint_quote(&server, 100).await;

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

            server.mint.stop().await.unwrap();
            drop(server);
            assert_mint_released_database(&db, "the first application instance").await;

            quote_id
        };

        // This is an application-instance restart in the same test process: every
        // first-instance Mint/database Arc was dropped and a new pool reopens the file.
        let db = Arc::new(MintSqliteDatabase::new(&path).await.unwrap());
        let restarted_server = create_test_rpc_server_with_sqlite_database(db.clone(), &seed, 3600)
            .await
            .with_mint_quote_payment_override(true);

        let response = QuoteService::update_mint_quote_state(
            &restarted_server,
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
        assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);

        let quote = restarted_server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.amount_issued().value(), 0);
        assert_eq!(quote.amount_mintable().value(), 100);
        assert_eq!(quote.payments.len(), 1);
        assert_eq!(
            quote.payments[0].payment_id,
            format!("cdk-payment-override:{quote_id}")
        );

        restarted_server.mint.stop().await.unwrap();
        drop(restarted_server);
        assert_mint_released_database(&db, "the restarted application instance").await;
        drop(db);
        remove_file(path).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_quote_service_concurrent_payment_overrides_pay_once() {
        let path = file_backed_test_database_path("concurrent-overrides");
        let db = Arc::new(MintSqliteDatabase::new(&path).await.unwrap());
        let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
        let barrier = Arc::new(Barrier::new(2));
        let server = build_test_rpc_server_with_sqlite_database(db.clone(), &seed, 3600)
            .await
            .with_mint_quote_payment_override(true)
            .with_mutation_guard(Arc::new(BarrierMutationGuard {
                barrier: barrier.clone(),
            }));
        // File-backed SQLite has a multi-connection pool. The guard holds both RPCs
        // until both have entered the mutation path, after which BEGIN IMMEDIATE
        // serializes them at SQLite's database writer lock rather than a pool slot.
        assert_concurrent_payment_overrides_pay_once(&server).await;
        assert_eq!(Arc::strong_count(&server.mint), 1);

        drop(server);
        assert_mint_released_database(&db, "the concurrent override test mint").await;
        drop(db);
        remove_file(path).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_quote_service_override_racing_backend_payment_credits_once() {
        let path = file_backed_test_database_path("override-backend-race");
        let db = Arc::new(MintSqliteDatabase::new(&path).await.unwrap());
        let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
        let barrier = Arc::new(Barrier::new(2));
        let server = build_test_rpc_server_with_sqlite_database(db.clone(), &seed, 3600)
            .await
            .with_mint_quote_payment_override(true)
            .with_mutation_guard(Arc::new(BarrierMutationGuard {
                barrier: barrier.clone(),
            }));
        let quote_id = create_test_mint_quote(&server, 100).await;
        let quote = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        let backend_payment = WaitPaymentResponse {
            payment_id: "backend-payment".to_string(),
            payment_amount: Amount::new(100, CurrencyUnit::Sat),
            payment_identifier: quote.request_lookup_id,
        };

        // The barrier synchronizes the operator RPC entry with the backend event.
        // Whichever transaction obtains SQLite's writer lock first, the other must
        // reload the committed quote before applying the BOLT11 paid-state guard.
        let operator_server = server.clone();
        let operator_quote_id = quote_id.clone();
        let operator = tokio::spawn(async move {
            let response = QuoteService::update_mint_quote_state(
                &operator_server,
                Request::new(crate::quote::UpdateMintQuoteStateRequest {
                    quote_id: operator_quote_id,
                    state: crate::quote::MintQuoteState::Paid.into(),
                    amount: None,
                    payment_id: None,
                }),
            )
            .await;
            drop(operator_server);
            response
        });
        let backend_mint = server.mint.clone();
        let backend = tokio::spawn(async move {
            barrier.wait().await;
            let result = backend_mint
                .pay_mint_quote_for_request_id(backend_payment)
                .await;
            drop(backend_mint);
            result
        });

        let (operator, backend) = tokio::join!(operator, backend);
        assert_eq!(
            operator.unwrap().unwrap().into_inner().state(),
            crate::quote::MintQuoteState::Paid
        );
        backend.unwrap().unwrap();
        assert_eq!(Arc::strong_count(&server.mint), 1);

        let quote = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.amount_issued().value(), 0);
        assert_eq!(quote.amount_mintable().value(), 100);
        assert_eq!(quote.payments.len(), 1);

        drop(server);
        assert_mint_released_database(&db, "the backend race test mint").await;
        drop(db);
        remove_file(path).unwrap();
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
        let quote = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        let backend_payment = WaitPaymentResponse {
            payment_id: "postgres-backend-payment".to_string(),
            payment_amount: Amount::new(100, CurrencyUnit::Sat),
            payment_identifier: quote.request_lookup_id,
        };

        let operator_server = server.clone();
        let operator_quote_id = quote_id.clone();
        let operator = tokio::spawn(async move {
            QuoteService::update_mint_quote_state(
                &operator_server,
                Request::new(crate::quote::UpdateMintQuoteStateRequest {
                    quote_id: operator_quote_id,
                    state: crate::quote::MintQuoteState::Paid.into(),
                    amount: None,
                    payment_id: None,
                }),
            )
            .await
        });
        let backend_mint = server.mint.clone();
        let backend = tokio::spawn(async move {
            barrier.wait().await;
            backend_mint
                .pay_mint_quote_for_request_id(backend_payment)
                .await
        });

        let (operator, backend) = tokio::join!(operator, backend);
        assert_eq!(
            operator.unwrap().unwrap().into_inner().state(),
            crate::quote::MintQuoteState::Paid
        );
        backend.unwrap().unwrap();

        let quote = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.amount_issued().value(), 0);
        assert_eq!(quote.amount_mintable().value(), 100);
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
