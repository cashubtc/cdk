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

    /// Records a fixed-amount BOLT11 payment override for an unpaid quote.
    ///
    /// A quote whose payments already cover its amount is returned unchanged.
    /// Quotes that can accept multiple payments and quotes with ambiguous
    /// accounting are rejected because the RPC does not identify a specific
    /// payment to record.
    async fn set_mint_quote_paid(&self, quote_id: &str) -> Result<MintQuote, Status> {
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

        if !mint_quote.payment_method.is_bolt11() {
            return Err(Status::failed_precondition(
                "Payment overrides are only supported for BOLT11 quotes",
            ));
        }

        let amount = mint_quote.amount.clone().ok_or_else(|| {
            Status::failed_precondition("Amountless quotes cannot be marked paid")
        })?;

        if amount.value() == 0 {
            return Err(Status::failed_precondition(
                "Zero-amount quotes cannot be marked paid",
            ));
        }

        let amount_paid = mint_quote.amount_paid();
        let amount_issued = mint_quote.amount_issued();

        // A quote whose recorded payments already cover its amount is satisfied,
        // including one the backend recorded as overpaid; report it unchanged.
        if amount_paid.value() >= amount.value() && amount_issued.value() <= amount_paid.value() {
            tx.commit()
                .await
                .map_err(|_| Status::internal("Could not commit db transaction".to_string()))?;
            return Ok(mint_quote.inner());
        }

        if amount_paid.value() != 0 || amount_issued.value() != 0 || !mint_quote.payments.is_empty()
        {
            return Err(Status::failed_precondition(
                "Quote has existing payment or issuance accounting",
            ));
        }

        let response = WaitPaymentResponse {
            payment_id: format!("mint-rpc-override:{}", mint_quote.id),
            payment_amount: amount,
            payment_identifier: mint_quote.request_lookup_id.clone(),
        };

        let should_notify = self
            .mint
            .pay_mint_quote(&mut tx, &mut mint_quote, response)
            .await
            .map_err(|_| Status::internal("Could not process payment".to_string()))?;

        tx.commit()
            .await
            .map_err(|_| Status::internal("Could not commit db transaction".to_string()))?;

        if should_notify {
            self.mint
                .pubsub_manager()
                .mint_quote_payment(&mint_quote, mint_quote.amount_paid());
        }

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

        let mint_quote = self.set_mint_quote_paid(&request.quote_id).await?;

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
    use std::sync::Arc;

    use bip39::Mnemonic;
    use cdk::amount::SplitTarget;
    use cdk::mint::{MintInput, MintQuote};
    use cdk::nuts::{CurrencyUnit, MintRequest, PreMintSecrets};
    use cdk::Amount;
    use cdk_common::payment::{PaymentIdentifier, WaitPaymentResponse};
    use cdk_common::{MintQuoteBolt11Request, PaymentMethod, QuoteId};
    use cdk_sqlite::mint::MintSqliteDatabase;
    use tokio::sync::Barrier;

    use super::super::test_utils::{
        build_test_rpc_server_with_sqlite_database, create_test_rpc_server,
        create_test_rpc_server_with_payment_delay, create_test_rpc_server_with_sqlite_database,
        UNKNOWN_QUOTE_ID,
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
    async fn test_quote_service_force_paid_rejects_additive_method_before_payment() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote_with_method(
            &server,
            Some(100),
            PaymentMethod::Custom("test".to_string()),
        )
        .await;

        let status = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
            }),
        )
        .await
        .expect_err("an additive payment method cannot be force-paid");

        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        record_test_payment(&server, &quote_id, 40, "later-backend-payment").await;

        let persisted = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.amount_paid().value(), 40);
        assert_eq!(persisted.amount_mintable().value(), 40);
        assert_eq!(persisted.payment_ids().len(), 1);
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

        let status = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
            }),
        )
        .await
        .expect_err("an override must not add value after a real payment");

        assert_eq!(status.code(), tonic::Code::FailedPrecondition);

        let persisted = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.amount_paid().value(), 40);
        assert_eq!(persisted.amount_mintable().value(), 40);
        assert_eq!(persisted.payment_ids().len(), 1);
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_rejects_partial_bolt11_payment() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;
        record_test_payment(&server, &quote_id, 40, "backend-partial-payment").await;

        let status = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
            }),
        )
        .await
        .expect_err("a partial payment must not be topped up by an override");

        assert_eq!(status.code(), tonic::Code::FailedPrecondition);

        let persisted = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.amount_paid().value(), 40);
        assert_eq!(persisted.amount_mintable().value(), 40);
        assert_eq!(persisted.payment_ids().len(), 1);
    }

    #[tokio::test]
    async fn test_quote_service_force_paid_rejects_amountless_quote() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id =
            create_test_mint_quote_with_method(&server, None, PaymentMethod::BOLT11).await;

        let status = QuoteService::update_mint_quote_state(
            &server,
            Request::new(crate::quote::UpdateMintQuoteStateRequest {
                quote_id: quote_id.clone(),
                state: crate::quote::MintQuoteState::Paid.into(),
            }),
        )
        .await
        .expect_err("an amountless quote cannot infer a payment amount");

        assert_eq!(status.code(), tonic::Code::FailedPrecondition);

        let persisted = server
            .mint
            .localstore()
            .get_mint_quote(&quote_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.amount_paid().value(), 0);
        assert!(persisted.payment_ids().is_empty());
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
            format!("mint-rpc-override:{quote_id}")
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
        let quote_id = create_test_mint_quote(&server, 100).await;

        // File-backed SQLite has a multi-connection pool. The guard holds both RPCs
        // until both have entered the mutation path, after which BEGIN IMMEDIATE
        // serializes them at SQLite's database writer lock rather than a pool slot.
        let first_server = server.clone();
        let first_quote_id = quote_id.clone();
        let first = tokio::spawn(async move {
            let response = QuoteService::update_mint_quote_state(
                &first_server,
                Request::new(crate::quote::UpdateMintQuoteStateRequest {
                    quote_id: first_quote_id,
                    state: crate::quote::MintQuoteState::Paid.into(),
                }),
            )
            .await;
            drop(first_server);
            response
        });
        let second_server = server.clone();
        let second_quote_id = quote_id.clone();
        let second = tokio::spawn(async move {
            let response = QuoteService::update_mint_quote_state(
                &second_server,
                Request::new(crate::quote::UpdateMintQuoteStateRequest {
                    quote_id: second_quote_id,
                    state: crate::quote::MintQuoteState::Paid.into(),
                }),
            )
            .await;
            drop(second_server);
            response
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
            }),
        )
        .await
        .expect_err("payment override should be disabled");

        assert_eq!(status.code(), tonic::Code::PermissionDenied);
        assert_eq!(status.message(), "Mint quote state override is disabled");
        assert_eq!(amount_paid(&server, &quote_id).await, 0);
    }
}
