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

    /// Records an operator-confirmed payment under its backend payment ID.
    ///
    /// BOLT11 defaults to its unpaid remainder. The recorded amount is final:
    /// subsequent backend reports for this ID are deduplicated.
    async fn set_mint_quote_paid(
        &self,
        quote_id: &str,
        requested_amount: Option<u64>,
        requested_payment_id: Option<&str>,
    ) -> Result<MintQuote, Status> {
        self.ensure_mint_quote_state_override_allowed()?;
        let quote_id = quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote id"))?;
        if requested_amount == Some(0) {
            return Err(Status::invalid_argument("Payment amount must be nonzero"));
        }
        let payment_id = requested_payment_id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Status::invalid_argument("A backend payment ID is required"))?;

        let localstore = self.mint.localstore();
        let mut tx = localstore
            .begin_transaction()
            .await
            .map_err(|_| Status::internal("Could not start db transaction"))?;
        let mut quote = tx
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::internal("Could not get quote in transaction"))?
            .ok_or_else(|| Status::invalid_argument("Could not find quote"))?;

        if requested_amount.is_none() && !quote.payment_method.is_bolt11() {
            return Err(Status::invalid_argument(
                "An explicit payment amount is required for this method",
            ));
        }

        if quote
            .payments
            .iter()
            .any(|payment| payment.payment_id == payment_id)
        {
            return Err(Status::already_exists("Payment ID is already recorded"));
        }

        let amount = match requested_amount {
            Some(amount) => amount,
            None => quote
                .amount
                .as_ref()
                .ok_or_else(|| Status::failed_precondition("BOLT11 quote has no amount"))?
                .value()
                .saturating_sub(quote.amount_paid().value()),
        };
        if amount == 0 {
            return Err(Status::failed_precondition("Quote is already fully paid"));
        }

        // BOLT12 offers can receive multiple distinct payments even when the
        // offer specifies an amount; that amount is not a lifetime credit cap.
        if !quote.payment_method.is_bolt12() {
            if let Some(quote_amount) = &quote.amount {
                let remaining = quote_amount
                    .value()
                    .saturating_sub(quote.amount_paid().value());
                if amount > remaining {
                    return Err(Status::failed_precondition(
                        "Payment override would exceed the quote amount",
                    ));
                }
                if quote.payment_method.is_bolt11() && amount != remaining {
                    return Err(Status::failed_precondition(
                        "BOLT11 payment override must equal the unpaid remainder",
                    ));
                }
            }
        }

        let payment_amount = cdk::Amount::new(amount, quote.unit.clone());
        quote
            .add_payment(payment_amount.clone(), payment_id.to_owned(), None)
            .map_err(|err| Status::failed_precondition(err.to_string()))?;
        tx.update_mint_quote(&mut quote)
            .await
            .map_err(|err| match err {
                cdk_common::database::Error::Duplicate => {
                    Status::already_exists("Payment ID is already recorded")
                }
                _ => Status::internal("Could not record payment"),
            })?;
        tx.commit()
            .await
            .map_err(|_| Status::internal("Could not commit db transaction"))?;

        tracing::info!(
            quote_id = %quote.id,
            payment_id,
            amount = %payment_amount,
            "mint quote payment override committed",
        );
        self.mint
            .pubsub_manager()
            .mint_quote_payment(&quote, quote.amount_paid());
        Ok(quote.inner())
    }

    /// Releases the melt execution lock on a melt quote, regardless of the
    /// held token
    ///
    /// Returns whether a lock was held and released. The quote's state is
    /// never changed.
    async fn force_unlock_melt_quote(&self, quote_id: &str) -> Result<bool, Status> {
        let quote_id = quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote id".to_string()))?;

        let localstore = self.mint.localstore();

        localstore
            .get_melt_quote(&quote_id)
            .await
            .map_err(|_| Status::invalid_argument("Could not find quote".to_string()))?
            .ok_or(Status::invalid_argument("Could not find quote".to_string()))?;

        let mut tx = localstore
            .begin_transaction()
            .await
            .map_err(|_| Status::internal("Could not start db transaction".to_string()))?;

        let was_locked = tx
            .force_unlock_melt_quote(&quote_id)
            .await
            .map_err(|_| Status::internal("Could not unlock melt quote".to_string()))?;

        tx.commit()
            .await
            .map_err(|_| Status::internal("Could not commit db transaction".to_string()))?;

        if was_locked {
            tracing::warn!(
                "Operator released the melt execution lock on melt quote {}",
                quote_id
            );
        }

        Ok(was_locked)
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

    /// Releases the melt execution lock on a melt quote
    async fn unlock_melt_quote(
        &self,
        request: Request<crate::quote::UnlockMeltQuoteRequest>,
    ) -> Result<Response<crate::quote::UnlockMeltQuoteResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let request = request.into_inner();

        let was_locked = self.force_unlock_melt_quote(&request.quote_id).await?;

        Ok(Response::new(crate::quote::UnlockMeltQuoteResponse {
            quote_id: request.quote_id,
            was_locked,
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
            async move {
                force_paid(
                    &server,
                    &quote_id,
                    Some(100),
                    Some("postgres-backend-payment"),
                )
                .await
            }
        });
        let second = tokio::spawn({
            let server = server.clone();
            let quote_id = quote_id.clone();
            async move {
                force_paid(
                    &server,
                    &quote_id,
                    Some(100),
                    Some("postgres-backend-payment"),
                )
                .await
            }
        });

        let (first, second) = tokio::join!(first, second);
        let results = [first.unwrap(), second.unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        for result in results {
            match result {
                Ok(state) => assert_eq!(state, crate::quote::MintQuoteState::Paid),
                Err(status) => assert_eq!(status.code(), tonic::Code::AlreadyExists),
            }
        }

        let quote = get_quote(server, &quote_id).await;
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.amount_issued().value(), 0);
        assert_eq!(quote.payments.len(), 1);
    }

    #[tokio::test]
    async fn test_quote_service_rejects_zero_amount_and_requires_payment_id() {
        let server = create_override_test_server().await;
        let quote_id = create_test_mint_quote(&server, 100).await;
        for (amount, payment_id) in [
            (Some(0), Some("payment")),
            (Some(100), None),
            (Some(100), Some("")),
        ] {
            let error = force_paid(&server, &quote_id, amount, payment_id)
                .await
                .unwrap_err();
            assert_eq!(error.code(), tonic::Code::InvalidArgument);
        }
        assert!(get_quote(&server, &quote_id).await.payments.is_empty());
    }

    #[tokio::test]
    async fn test_quote_service_bolt11_defaults_to_unpaid_remainder() {
        for already_paid in [0, 40] {
            let server = create_override_test_server().await;
            let quote_id = create_test_mint_quote(&server, 100).await;
            if already_paid > 0 {
                record_test_payment(&server, &quote_id, already_paid, "earlier").await;
            }
            force_paid(&server, &quote_id, None, Some("payment"))
                .await
                .unwrap();
            let error = force_paid(&server, &quote_id, None, Some("payment"))
                .await
                .unwrap_err();
            assert_eq!(error.code(), tonic::Code::AlreadyExists);
            let quote = get_quote(&server, &quote_id).await;
            assert_eq!(quote.amount_paid().value(), 100);
            let payment = quote
                .payments
                .iter()
                .find(|p| p.payment_id == "payment")
                .unwrap();
            assert_eq!(payment.amount.value(), 100 - already_paid);
            let error = force_paid(&server, &quote_id, None, Some("another"))
                .await
                .unwrap_err();
            assert_eq!(error.code(), tonic::Code::FailedPrecondition);
            issue_test_mint_quote(&server, &quote_id, 100).await;
            assert_eq!(
                force_paid(&server, &quote_id, None, Some("payment"))
                    .await
                    .unwrap_err()
                    .code(),
                tonic::Code::AlreadyExists,
            );
        }
    }

    #[tokio::test]
    async fn test_quote_service_other_methods_require_explicit_amount() {
        for method in [
            PaymentMethod::BOLT12,
            PaymentMethod::from("onchain"),
            PaymentMethod::Custom("test".to_owned()),
        ] {
            for amount in [None, Some(100)] {
                let server = create_override_test_server().await;
                let quote_id =
                    create_test_mint_quote_with_method(&server, amount, method.clone()).await;
                let error = force_paid(&server, &quote_id, None, Some("payment"))
                    .await
                    .unwrap_err();
                assert_eq!(error.code(), tonic::Code::InvalidArgument);
                assert!(get_quote(&server, &quote_id).await.payments.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn test_quote_service_rejects_duplicate_credits_and_deduplicates_backend_reports() {
        for (amount, method) in [
            (Some(100), PaymentMethod::BOLT11),
            (Some(100), PaymentMethod::BOLT12),
            (None, PaymentMethod::from("onchain")),
        ] {
            let server = create_override_test_server().await;
            let quote_id = create_test_mint_quote_with_method(&server, amount, method).await;
            force_paid(&server, &quote_id, Some(100), Some("payment"))
                .await
                .unwrap();
            for retry_amount in [100, 101] {
                let error = force_paid(&server, &quote_id, Some(retry_amount), Some("payment"))
                    .await
                    .unwrap_err();
                assert_eq!(error.code(), tonic::Code::AlreadyExists);
            }
            // The operator's amount remains authoritative, even if a later
            // backend report differs. Both exact and differing reports are retries.
            for backend_amount in [100, 150] {
                record_test_payment(&server, &quote_id, backend_amount, "payment").await;
            }
            let quote = get_quote(&server, &quote_id).await;
            assert_eq!(quote.amount_paid().value(), 100);
            assert_eq!(recorded_payments(&quote), vec![("payment".to_owned(), 100)]);
        }
    }

    async fn assert_cross_quote_duplicate_payment_rejected(server: &MintRPCServer) {
        let first_id = create_test_mint_quote(server, 100).await;
        let second_id = create_test_mint_quote(server, 100).await;
        force_paid(server, &first_id, None, Some("shared-payment"))
            .await
            .unwrap();

        let error = force_paid(server, &second_id, None, Some("shared-payment"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::AlreadyExists);

        let first = get_quote(server, &first_id).await;
        assert_eq!(first.amount_paid().value(), 100);
        assert_eq!(
            recorded_payments(&first),
            vec![("shared-payment".to_owned(), 100)]
        );
        let second = get_quote(server, &second_id).await;
        assert_eq!(second.state(), MintQuoteState::Unpaid);
        assert_eq!(second.amount_paid().value(), 0);
        assert!(second.payments.is_empty());
    }

    #[tokio::test]
    async fn test_quote_service_cross_quote_duplicate_payment_rejected() {
        let server = create_override_test_server().await;
        assert_cross_quote_duplicate_payment_rejected(&server).await;
    }

    #[tokio::test]
    async fn test_quote_service_postgres_cross_quote_duplicate_payment_rejected() {
        let Some(db) = postgres_test_database("cross_quote_duplicate").await else {
            return;
        };
        let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
        let server = build_test_rpc_server_with_database(db, &seed, 3600)
            .await
            .with_mint_quote_payment_override(true);
        assert_cross_quote_duplicate_payment_rejected(&server).await;
    }

    #[tokio::test]
    async fn test_quote_service_backend_payment_before_override_rejects_duplicate_id() {
        let server = create_override_test_server().await;
        let quote_id =
            create_test_mint_quote_with_method(&server, None, PaymentMethod::BOLT12).await;
        record_test_payment(&server, &quote_id, 100, "payment").await;
        for amount in [100, 75] {
            let error = force_paid(&server, &quote_id, Some(amount), Some("payment"))
                .await
                .unwrap_err();
            assert_eq!(error.code(), tonic::Code::AlreadyExists);
        }
        let quote = get_quote(&server, &quote_id).await;
        assert_eq!(quote.amount_paid().value(), 100);
        assert_eq!(quote.payments.len(), 1);
    }

    #[tokio::test]
    async fn test_quote_service_fixed_quotes_enforce_unpaid_remainder() {
        for method in [
            PaymentMethod::BOLT11,
            PaymentMethod::Custom("test".to_owned()),
        ] {
            let server = create_override_test_server().await;
            let quote_id =
                create_test_mint_quote_with_method(&server, Some(100), method.clone()).await;
            record_test_payment(&server, &quote_id, 40, "first").await;
            let error = force_paid(&server, &quote_id, Some(61), Some("second"))
                .await
                .unwrap_err();
            assert_eq!(error.code(), tonic::Code::FailedPrecondition);
            if method.is_bolt11() {
                let error = force_paid(&server, &quote_id, Some(59), Some("second"))
                    .await
                    .unwrap_err();
                assert_eq!(error.code(), tonic::Code::FailedPrecondition);
            }
            force_paid(&server, &quote_id, Some(60), Some("second"))
                .await
                .unwrap();
            let error = force_paid(&server, &quote_id, Some(1), Some("third"))
                .await
                .unwrap_err();
            assert_eq!(error.code(), tonic::Code::FailedPrecondition);
            assert_eq!(amount_paid(&server, &quote_id).await, 100);
        }
    }

    #[tokio::test]
    async fn test_quote_service_reusable_quotes_accept_distinct_payments() {
        for (amount, method) in [
            (Some(100), PaymentMethod::BOLT12),
            (None, PaymentMethod::from("onchain")),
        ] {
            let server = create_override_test_server().await;
            let quote_id = create_test_mint_quote_with_method(&server, amount, method).await;
            force_paid(&server, &quote_id, Some(100), Some("first"))
                .await
                .unwrap();
            // Model issuance before the next payment to cover a reused quote.
            let store = server.mint.localstore();
            let mut tx = store.begin_transaction().await.unwrap();
            let mut quote = tx
                .get_mint_quote(&quote_id.parse().unwrap())
                .await
                .unwrap()
                .unwrap();
            quote
                .add_issuance(Amount::new(100, CurrencyUnit::Sat))
                .unwrap();
            tx.update_mint_quote(&mut quote).await.unwrap();
            tx.commit().await.unwrap();
            force_paid(&server, &quote_id, Some(100), Some("second"))
                .await
                .unwrap();
            record_test_payment(&server, &quote_id, 100, "second").await;
            record_test_payment(&server, &quote_id, 50, "third").await;
            let quote = get_quote(&server, &quote_id).await;
            assert_eq!(quote.amount_paid().value(), 250);
            assert_eq!(quote.amount_issued().value(), 100);
            assert_eq!(quote.amount_mintable().value(), 150);
            assert_eq!(quote.payments.len(), 3);
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
            async move {
                force_paid(
                    &server,
                    &quote_id,
                    Some(100),
                    Some("postgres-backend-payment"),
                )
                .await
            }
        });
        let backend = tokio::spawn({
            let mint = server.mint.clone();
            async move {
                barrier.wait().await;
                mint.pay_mint_quote_for_request_id(backend_payment).await
            }
        });

        let (operator, backend) = tokio::join!(operator, backend);
        match operator.unwrap() {
            Ok(state) => assert_eq!(state, crate::quote::MintQuoteState::Paid),
            Err(status) => assert_eq!(status.code(), tonic::Code::AlreadyExists),
        }
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
    async fn test_quote_service_repeated_override_of_issued_quote_is_rejected() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 32).await;

        QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: Some(32),
                payment_id: Some("payment".to_owned()),
            }),
        )
        .await
        .unwrap();
        issue_test_mint_quote(&server, &quote_id, 32).await;

        // A repeated override fails without changing the issued quote.
        let status = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
                amount: Some(32),
                payment_id: Some("payment".to_owned()),
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(status.code(), tonic::Code::AlreadyExists);
        let quote = get_quote(&server, &quote_id).await;
        assert_eq!(quote.state(), MintQuoteState::Issued);
        assert_eq!(quote.amount_paid().value(), 32);
        assert_eq!(quote.amount_issued().value(), 32);
        assert_eq!(quote.payments.len(), 1);
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
                amount: Some(100),
                payment_id: Some("payment".to_owned()),
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
                amount: Some(100),
                payment_id: Some("payment".to_owned()),
            }),
        )
        .await
        .expect_err("payment override should be disabled");

        assert_eq!(status.code(), tonic::Code::PermissionDenied);
        assert_eq!(status.message(), "Mint quote state override is disabled");
        assert_eq!(amount_paid(&server, &quote_id).await, 0);
    }

    /// Creates a melt quote and arms its melt execution lock, returning the
    /// quote id
    async fn create_locked_melt_quote(server: &MintRPCServer) -> String {
        let invoice = cdk_fake_wallet::create_fake_invoice(100_000, String::new());
        let response = server
            .mint
            .get_melt_quote(
                cdk_common::MeltQuoteBolt11Request {
                    request: invoice,
                    unit: CurrencyUnit::Sat,
                    options: None,
                }
                .into(),
            )
            .await
            .unwrap();
        let quote_id = response.quote().expect("single-quote method").clone();

        let localstore = server.mint.localstore();
        let mut tx = localstore.begin_transaction().await.unwrap();
        let mut quote = tx.get_melt_quote(&quote_id).await.unwrap().unwrap();
        quote.melt_lock = "test-lock-token".to_string();
        tx.update_melt_quote_state(&mut quote, cdk::nuts::MeltQuoteState::Pending, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        quote_id.to_string()
    }

    #[tokio::test]
    async fn test_quote_service_unlock_melt_quote_releases_lock() {
        let server = create_test_rpc_server().await;
        let quote_id = create_locked_melt_quote(&server).await;

        let response = QuoteService::unlock_melt_quote(
            &server,
            Request::new(crate::quote::UnlockMeltQuoteRequest {
                quote_id: quote_id.clone(),
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.quote_id, quote_id);
        assert!(response.was_locked);

        // The quote's state is untouched; only the lock was released.
        let quote = server
            .mint
            .localstore()
            .get_melt_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quote.state, cdk::nuts::MeltQuoteState::Pending);
        assert!(!quote.is_locked());

        // Unlocking an unlocked quote reports that no lock was held.
        let response = QuoteService::unlock_melt_quote(
            &server,
            Request::new(crate::quote::UnlockMeltQuoteRequest {
                quote_id: quote_id.clone(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(!response.was_locked);
    }

    #[tokio::test]
    async fn test_quote_service_unlock_melt_quote_unknown_quote() {
        let server = create_test_rpc_server().await;

        let status = QuoteService::unlock_melt_quote(
            &server,
            Request::new(crate::quote::UnlockMeltQuoteRequest {
                quote_id: UNKNOWN_QUOTE_ID.to_string(),
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(status.message(), "Could not find quote");
    }

    #[tokio::test]
    async fn test_quote_service_unlock_melt_quote_invalid_quote_id() {
        let server = create_test_rpc_server().await;

        let status = QuoteService::unlock_melt_quote(
            &server,
            Request::new(crate::quote::UnlockMeltQuoteRequest {
                quote_id: "not-a-quote-id".to_string(),
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(status.message(), "Invalid quote id");
    }
}
