//! Quote administration service.

use cdk::mint::MintQuote;
use cdk::nuts::{MeltQuoteState, MintQuoteState};
use cdk::types::QuoteTTL;
use cdk::Amount;
use cdk_common::payment::WaitPaymentResponse;
use tonic::{Request, Response, Status};

use super::{page_limit, MintRPCServer};
use crate::quote::quote_service_server::QuoteService;

const DEFAULT_MELT_QUOTE_LIMIT: u32 = 100;
const MAX_MELT_QUOTE_LIMIT: u32 = 1_000;

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

    /// Records a payment against a mint quote as though the payment backend
    /// had reported it, marking the quote paid
    ///
    /// Returns the quote as it stands after the update.
    async fn set_mint_quote_paid(&self, quote_id: &str) -> Result<MintQuote, Status> {
        self.ensure_mint_quote_state_override_allowed()?;

        let quote_id = quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote id".to_string()))?;

        let mint_quote = self
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::invalid_argument("Could not find quote".to_string()))?
            .ok_or(Status::invalid_argument("Could not find quote".to_string()))?;

        // Create a dummy payment response
        let response = WaitPaymentResponse {
            payment_id: mint_quote.request_lookup_id.to_string(),
            payment_amount: mint_quote.clone().amount.unwrap_or(cdk::Amount::new(
                mint_quote.amount_paid().value(),
                mint_quote.unit.clone(),
            )),
            payment_identifier: mint_quote.request_lookup_id.clone(),
        };

        let localstore = self.mint.localstore();
        let mut tx = localstore
            .begin_transaction()
            .await
            .map_err(|_| Status::internal("Could not start db transaction".to_string()))?;

        // Re-fetch the mint quote within the transaction to lock it
        let mut mint_quote = tx
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::internal("Could not get quote in transaction".to_string()))?
            .ok_or(Status::invalid_argument(
                "Quote not found in transaction".to_string(),
            ))?;

        let should_notify = self
            .mint
            .pay_mint_quote(&mut tx, &mut mint_quote, response)
            .await
            .map_err(|_| Status::internal("Could not process payment".to_string()))?;

        tx.commit()
            .await
            .map_err(|_| Status::internal("Could not commit db transaction".to_string()))?;

        // Publish notification AFTER transaction commits
        if should_notify {
            self.mint
                .pubsub_manager()
                .mint_quote_payment(&mint_quote, mint_quote.amount_paid());
        }

        self.mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::invalid_argument("Could not find quote".to_string()))?
            .ok_or(Status::invalid_argument("Could not find quote".to_string()))
    }
}

#[tonic::async_trait]
impl QuoteService for MintRPCServer {
    #[tracing::instrument(skip_all)]
    async fn resolve_melt_quote(
        &self,
        request: Request<crate::quote::ResolveMeltQuoteRequest>,
    ) -> Result<Response<crate::quote::ResolveMeltQuoteResponse>, Status> {
        use cdk::mint::{MeltQuoteResolution, MeltQuoteResolutionAction, MeltQuoteResolutionError};
        use cdk_common::mint::MeltFinalizationData;
        use cdk_common::payment::PaymentIdentifier;

        use crate::quote::resolve_melt_quote_request::Resolution;

        self.ensure_mutation_allowed().await?;
        let request = request.into_inner();
        if request.quote_id.is_empty() {
            return Err(Status::invalid_argument("quote_id is required"));
        }
        let quote_id = request
            .quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote_id"))?;
        let operation_id = request
            .operation_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid operation_id"))?;
        let expected_saga_state = request
            .expected_saga_state
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid expected_saga_state"))?;
        let action = match request.resolution {
            Some(Resolution::Finalize(payment)) => {
                if payment.unit.is_empty() || payment.payment_lookup_id.is_empty() {
                    return Err(Status::invalid_argument(
                        "unit and payment_lookup_id are required",
                    ));
                }
                let unit = payment
                    .unit
                    .parse()
                    .map_err(|_| Status::invalid_argument("Invalid unit"))?;
                let payment_lookup_id = PaymentIdentifier::new(
                    &payment.payment_lookup_id_kind,
                    &payment.payment_lookup_id,
                )
                .map_err(|_| {
                    Status::invalid_argument("Invalid payment lookup identifier or kind")
                })?;
                MeltQuoteResolutionAction::Finalize(MeltFinalizationData {
                    total_spent: Amount::new(payment.total_spent, unit),
                    payment_lookup_id,
                    payment_proof: payment.payment_proof,
                })
            }
            Some(Resolution::Compensate(compensation)) => MeltQuoteResolutionAction::Compensate {
                payment_failure_confirmed: compensation.payment_failure_confirmed,
            },
            None => return Err(Status::invalid_argument("A resolution action is required")),
        };
        let quote = self
            .mint
            .resolve_melt_quote(MeltQuoteResolution {
                quote_id,
                operation_id,
                expected_saga_state,
                reason: request.reason,
                action,
            })
            .await
            .map_err(|error| match error {
                MeltQuoteResolutionError::InvalidRequest(reason) => {
                    Status::invalid_argument(reason)
                }
                MeltQuoteResolutionError::Conflict(reason) => Status::failed_precondition(reason),
                MeltQuoteResolutionError::UnknownQuote => Status::not_found("Unknown melt quote"),
                error => Status::internal(error.to_string()),
            })?;
        Ok(Response::new(crate::quote::ResolveMeltQuoteResponse {
            quote_id: quote.id.to_string(),
            operation_id: operation_id.to_string(),
            state: crate::quote::MeltQuoteState::from(quote.state).into(),
        }))
    }

    /// Inspects stored melt quotes without initiating payment recovery.
    #[tracing::instrument(skip_all)]
    async fn list_melt_quotes(
        &self,
        request: Request<crate::quote::ListMeltQuotesRequest>,
    ) -> Result<Response<crate::quote::ListMeltQuotesResponse>, Status> {
        let request = request.into_inner();
        let state_filter = request
            .state
            .map(|state| {
                use crate::quote::MeltQuoteState as RpcState;

                match RpcState::try_from(state) {
                    Ok(RpcState::Unpaid) => Ok(MeltQuoteState::Unpaid),
                    Ok(RpcState::Pending) => Ok(MeltQuoteState::Pending),
                    Ok(RpcState::Paid) => Ok(MeltQuoteState::Paid),
                    Ok(RpcState::Unknown) => Ok(MeltQuoteState::Unknown),
                    Ok(RpcState::Failed) => Ok(MeltQuoteState::Failed),
                    Ok(RpcState::Unspecified) | Err(_) => {
                        Err(Status::invalid_argument("Invalid melt quote state"))
                    }
                }
            })
            .transpose()?;
        let limit = page_limit(
            request.limit,
            DEFAULT_MELT_QUOTE_LIMIT,
            MAX_MELT_QUOTE_LIMIT,
        )?;
        for (name, value) in [
            ("quote_id", &request.quote_id),
            ("request_lookup_id", &request.request_lookup_id),
            ("payment_request", &request.payment_request),
        ] {
            if value.as_ref().is_some_and(|value| value.is_empty()) {
                return Err(Status::invalid_argument(format!(
                    "{name} must not be empty"
                )));
            }
        }

        let mut quotes = match request.quote_id {
            Some(id) => {
                let id = id
                    .parse()
                    .map_err(|_| Status::invalid_argument("Invalid quote_id"))?;
                self.mint
                    .localstore()
                    .get_melt_quote(&id)
                    .await
                    .map_err(|err| Status::internal(err.to_string()))?
                    .into_iter()
                    .collect::<Vec<_>>()
            }
            None => self
                .mint
                .melt_quotes()
                .await
                .map_err(|err| Status::internal(err.to_string()))?,
        };
        quotes.retain(|quote| {
            state_filter.is_none_or(|state| quote.state == state)
                && request
                    .payment_request
                    .as_ref()
                    .is_none_or(|filter| quote.request.to_string() == *filter)
                && request.request_lookup_id.as_ref().is_none_or(|filter| {
                    quote
                        .request_lookup_id
                        .as_ref()
                        .is_some_and(|id| id.to_string() == *filter)
                })
        });
        quotes.sort_unstable_by(|a, b| {
            a.created_time
                .cmp(&b.created_time)
                .then_with(|| a.id.cmp(&b.id))
        });
        let total = quotes.len() as u64;
        let mut results = Vec::new();
        let db = self.mint.localstore();
        for quote in quotes.into_iter().skip(request.offset as usize).take(limit) {
            let saga = db
                .get_melt_saga_by_quote_id(&quote.id)
                .await
                .map_err(|err| Status::internal(err.to_string()))?
                .map(|saga| crate::quote::MeltQuoteSaga {
                    operation_id: saga.operation_id.to_string(),
                    state: saga.state.state().to_owned(),
                    created_at: saga.created_at,
                    updated_at: saga.updated_at,
                });
            results.push(crate::quote::MeltQuoteInfo {
                quote_id: quote.id.to_string(),
                amount: quote.amount().value(),
                fee_reserve: quote.fee_reserve().value(),
                unit: quote.unit.to_string(),
                payment_method: quote.payment_method.to_string(),
                created_time: quote.created_time,
                expiry: quote.expiry,
                request_lookup_id: quote.request_lookup_id.as_ref().map(ToString::to_string),
                request_lookup_id_kind: quote.request_lookup_id.as_ref().map(|id| id.kind()),
                state: crate::quote::MeltQuoteState::from(quote.state).into(),
                request: quote.request.to_string(),
                paid_time: quote.paid_time,
                payment_proof: quote.payment_proof,
                saga,
            });
        }

        Ok(Response::new(crate::quote::ListMeltQuotesResponse {
            quotes: results,
            total,
        }))
    }

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

impl From<MeltQuoteState> for crate::quote::MeltQuoteState {
    fn from(state: MeltQuoteState) -> Self {
        match state {
            MeltQuoteState::Unpaid => Self::Unpaid,
            MeltQuoteState::Pending => Self::Pending,
            MeltQuoteState::Paid => Self::Paid,
            MeltQuoteState::Unknown => Self::Unknown,
            MeltQuoteState::Failed => Self::Failed,
        }
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
    use std::sync::Arc;

    use cdk::amount::SplitTarget;
    use cdk::mint::MintInput;
    use cdk::nuts::{CurrencyUnit, MintRequest, PaymentMethod, PreMintSecrets};
    use cdk::Amount;
    use cdk_common::MintQuoteBolt11Request;
    use tonic::Code;

    use super::super::test_utils::{
        create_test_rpc_server, create_test_rpc_server_with_payment_delay, RejectingMutationGuard,
        UNKNOWN_QUOTE_ID,
    };
    use super::*;

    async fn store_test_melt_quote(
        server: &MintRPCServer,
        state: MeltQuoteState,
        lookup_id: Option<cdk_common::payment::PaymentIdentifier>,
        created_time: u64,
    ) -> String {
        let mut quote = cdk::mint::MeltQuote::new(
            None,
            cdk_common::mint::MeltPaymentRequest::Custom {
                method: "test".to_owned(),
                request: "test payment".to_owned(),
            },
            CurrencyUnit::Msat,
            Amount::new(1200, CurrencyUnit::Msat),
            Amount::new(100, CurrencyUnit::Msat),
            1,
            lookup_id,
            None,
            PaymentMethod::Custom("test".to_owned()),
            None,
            None,
        );
        quote.state = state;
        if state == MeltQuoteState::Paid {
            quote.paid_time = Some(created_time + 1);
            quote.payment_proof = Some("test proof".to_owned());
        }
        quote.created_time = created_time;
        let id = quote.id.to_string();
        let db = server.mint.localstore();
        let mut tx = db.begin_transaction().await.unwrap();
        tx.add_melt_quote(quote).await.unwrap();
        tx.commit().await.unwrap();
        id
    }

    #[tokio::test]
    async fn test_list_melt_quotes_is_read_only_and_includes_expired_quotes() {
        let mut server = create_test_rpc_server().await;
        server.mutation_guard = Some(Arc::new(RejectingMutationGuard));
        let pending_id = store_test_melt_quote(&server, MeltQuoteState::Pending, None, 10).await;
        for state in [MeltQuoteState::Unpaid, MeltQuoteState::Paid] {
            store_test_melt_quote(&server, state, None, 0).await;
        }

        // There is no processor for this custom method. Inspection must work
        // without trying to check the payment or requiring mutation permission.
        let response = QuoteService::list_melt_quotes(
            &server,
            Request::new(crate::quote::ListMeltQuotesRequest {
                state: Some(crate::quote::MeltQuoteState::Pending.into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(response.total, 1);
        assert_eq!(response.quotes.len(), 1);
        let quote = &response.quotes[0];
        assert_eq!(quote.quote_id, pending_id);
        assert_eq!(quote.amount, 1200);
        assert_eq!(quote.fee_reserve, 100);
        assert_eq!(quote.unit, "msat");
        assert_eq!(quote.payment_method, "test");
        assert_eq!(quote.created_time, 10);
        assert_eq!(quote.expiry, 1);
        assert_eq!(quote.request_lookup_id, None);
        assert_eq!(quote.request_lookup_id_kind, None);
        assert_eq!(quote.state(), crate::quote::MeltQuoteState::Pending);
        assert_eq!(quote.request, "test payment");
        assert_eq!(quote.saga, None);
        assert_eq!(
            server
                .mint
                .localstore()
                .get_melt_quote(&pending_id.parse().unwrap())
                .await
                .unwrap()
                .unwrap()
                .state,
            MeltQuoteState::Pending
        );
    }

    #[tokio::test]
    async fn test_list_melt_quotes_filters_and_pagination() {
        use cdk_common::payment::PaymentIdentifier;

        use crate::quote::ListMeltQuotesRequest;

        let server = create_test_rpc_server().await;
        let hash = PaymentIdentifier::PaymentHash([0xab; 32]);
        let lookup_id = hash.to_string();
        let first = store_test_melt_quote(&server, MeltQuoteState::Pending, Some(hash), 10).await;
        let second = store_test_melt_quote(
            &server,
            MeltQuoteState::Pending,
            Some(PaymentIdentifier::CustomId("custom-lookup".to_owned())),
            20,
        )
        .await;
        let paid = store_test_melt_quote(
            &server,
            MeltQuoteState::Paid,
            Some(PaymentIdentifier::CustomId("paid-lookup".to_owned())),
            0,
        )
        .await;
        store_test_melt_quote(&server, MeltQuoteState::Pending, None, 30).await;
        // Unpaid attempts may share an identifier with a pending quote.
        store_test_melt_quote(
            &server,
            MeltQuoteState::Unpaid,
            Some(PaymentIdentifier::PaymentHash([0xab; 32])),
            0,
        )
        .await;

        for (quote_id, filter, offset, expected, total) in [
            (None, Some(lookup_id.clone()), 0, Some(first.clone()), 1),
            (None, Some(lookup_id.clone()), 1, None, 1),
            (None, None, 1, Some(second.clone()), 3),
            (None, None, u32::MAX, None, 3),
            (
                None,
                Some("custom-lookup".to_owned()),
                0,
                Some(second.clone()),
                1,
            ),
            (Some(second.clone()), None, 0, Some(second.clone()), 1),
            (
                Some(first.clone()),
                Some(lookup_id.clone()),
                0,
                Some(first.clone()),
                1,
            ),
            (Some(first.clone()), Some("other".to_owned()), 0, None, 0),
            (None, Some(lookup_id.to_uppercase()), 0, None, 0),
            (Some(paid), None, 0, None, 0),
            (Some(UNKNOWN_QUOTE_ID.to_owned()), None, 0, None, 0),
        ] {
            let response = QuoteService::list_melt_quotes(
                &server,
                Request::new(ListMeltQuotesRequest {
                    quote_id,
                    request_lookup_id: filter,
                    limit: 1,
                    offset,
                    state: Some(crate::quote::MeltQuoteState::Pending.into()),
                    payment_request: None,
                }),
            )
            .await
            .unwrap()
            .into_inner();
            assert_eq!(response.total, total);
            assert_eq!(
                response
                    .quotes
                    .iter()
                    .map(|quote| quote.quote_id.clone())
                    .collect::<Vec<_>>(),
                expected.into_iter().collect::<Vec<_>>()
            );
            if let Some(quote) = response.quotes.first() {
                let (id, kind) = match quote.quote_id == first {
                    true => (lookup_id.as_str(), "payment_hash"),
                    false => ("custom-lookup", "custom"),
                };
                assert_eq!(quote.request_lookup_id.as_deref(), Some(id));
                assert_eq!(quote.request_lookup_id_kind.as_deref(), Some(kind));
            }
        }
    }

    #[tokio::test]
    async fn test_list_melt_quotes_default_limit_and_stable_order() {
        let server = create_test_rpc_server().await;
        let mut ids = Vec::new();
        for _ in 0..101 {
            ids.push(store_test_melt_quote(&server, MeltQuoteState::Pending, None, 10).await);
        }
        ids.sort();
        let response = QuoteService::list_melt_quotes(
            &server,
            Request::new(crate::quote::ListMeltQuotesRequest::default()),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(response.total, 101);
        assert_eq!(
            response
                .quotes
                .iter()
                .map(|quote| quote.quote_id.clone())
                .collect::<Vec<_>>(),
            ids[..100]
        );
        let response = QuoteService::list_melt_quotes(
            &server,
            Request::new(crate::quote::ListMeltQuotesRequest {
                limit: 1000,
                offset: 100,
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(response.total, 101);
        assert_eq!(response.quotes.len(), 1);
        assert_eq!(response.quotes[0].quote_id, ids[100]);
    }

    #[tokio::test]
    async fn test_reported_melt_lookup_returns_current_state_and_recovery_details() {
        use cdk_common::mint::{MeltSagaState, Saga};
        use cdk_common::payment::PaymentIdentifier;

        use crate::quote::ListMeltQuotesRequest;

        let server = create_test_rpc_server().await;
        let pending = store_test_melt_quote(
            &server,
            MeltQuoteState::Pending,
            Some(PaymentIdentifier::CustomId("reported".to_owned())),
            10,
        )
        .await;
        let unpaid = store_test_melt_quote(
            &server,
            MeltQuoteState::Unpaid,
            Some(PaymentIdentifier::CustomId("reported".to_owned())),
            20,
        )
        .await;
        let paid = store_test_melt_quote(&server, MeltQuoteState::Paid, None, 30).await;
        let mut saga = Saga::new_melt(
            UNKNOWN_QUOTE_ID.parse().unwrap(),
            MeltSagaState::PaymentPending,
            pending.clone(),
        );
        saga.created_at = 11;
        saga.updated_at = 12;
        let db = server.mint.localstore();
        let mut tx = db.begin_transaction().await.unwrap();
        tx.add_saga(&saga).await.unwrap();
        tx.commit().await.unwrap();
        let saga = db
            .get_melt_saga_by_quote_id(&pending.parse().unwrap())
            .await
            .unwrap()
            .unwrap();

        let response = QuoteService::list_melt_quotes(
            &server,
            Request::new(ListMeltQuotesRequest {
                request_lookup_id: Some("reported".to_owned()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(response.total, 2);
        assert_eq!(response.quotes[0].quote_id, pending);
        assert_eq!(
            response.quotes[0].state(),
            crate::quote::MeltQuoteState::Pending
        );
        let recovery = response.quotes[0].saga.as_ref().unwrap();
        assert_eq!(recovery.operation_id, saga.operation_id.to_string());
        assert_eq!(recovery.state, "payment_pending");
        assert_eq!(recovery.created_at, 11);
        assert_eq!(recovery.updated_at, saga.updated_at);
        assert_eq!(response.quotes[1].quote_id, unpaid);
        assert_eq!(
            response.quotes[1].state(),
            crate::quote::MeltQuoteState::Unpaid
        );
        assert!(response.quotes[1].saga.is_none());
        assert_eq!(
            db.get_melt_saga_by_quote_id(&pending.parse().unwrap())
                .await
                .unwrap(),
            Some(saga)
        );

        // A report of a pending quote must still find it after it has settled.
        let response = QuoteService::list_melt_quotes(
            &server,
            Request::new(ListMeltQuotesRequest {
                quote_id: Some(paid.clone()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(response.total, 1);
        let quote = &response.quotes[0];
        assert_eq!(quote.quote_id, paid);
        assert_eq!(quote.state(), crate::quote::MeltQuoteState::Paid);
        assert_eq!(quote.paid_time, Some(31));
        assert_eq!(quote.payment_proof.as_deref(), Some("test proof"));
        assert!(quote.saga.is_none());
    }

    #[tokio::test]
    async fn test_payment_request_lookup_returns_attempts_and_combines_filters() {
        use cdk_common::mint::MeltPaymentRequest;
        use cdk_common::payment::PaymentIdentifier;

        use crate::quote::ListMeltQuotesRequest;

        let server = create_test_rpc_server().await;
        let db = server.mint.localstore();
        for (index, request) in [
            MeltPaymentRequest::Bolt11 {
                bolt11: cdk_fake_wallet::create_fake_invoice(1200, "reported payment".to_owned()),
            },
            MeltPaymentRequest::Onchain {
                address: "bcrt1qreportedaddress".to_owned(),
            },
            MeltPaymentRequest::Custom {
                method: "test".to_owned(),
                request: "case-sensitive custom request".to_owned(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let payment_request = request.to_string();
            let lookup_id = format!("attempt-{index}");
            let mut pending = cdk::mint::MeltQuote::new(
                None,
                request,
                CurrencyUnit::Msat,
                Amount::new(1200, CurrencyUnit::Msat),
                Amount::new(100, CurrencyUnit::Msat),
                1,
                Some(PaymentIdentifier::CustomId(lookup_id.clone())),
                None,
                PaymentMethod::Custom("test".to_owned()),
                None,
                None,
            );
            pending.state = MeltQuoteState::Pending;
            pending.created_time = 10;
            let pending_id = pending.id.to_string();
            let mut unpaid = pending.clone();
            unpaid.id = Default::default();
            unpaid.state = MeltQuoteState::Unpaid;
            unpaid.created_time = 20;
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_melt_quote(pending).await.unwrap();
            tx.add_melt_quote(unpaid).await.unwrap();
            tx.commit().await.unwrap();

            for (filter, quote_id, lookup, state, expected) in [
                (payment_request.clone(), None, None, None, 2),
                (
                    payment_request.clone(),
                    None,
                    Some(lookup_id.clone()),
                    None,
                    2,
                ),
                (
                    payment_request.clone(),
                    Some(pending_id.clone()),
                    Some(lookup_id.clone()),
                    Some(crate::quote::MeltQuoteState::Pending as i32),
                    1,
                ),
                (
                    payment_request.clone(),
                    None,
                    None,
                    Some(crate::quote::MeltQuoteState::Unpaid as i32),
                    1,
                ),
                (
                    payment_request.clone(),
                    None,
                    None,
                    Some(crate::quote::MeltQuoteState::Paid as i32),
                    0,
                ),
                (
                    payment_request.clone(),
                    None,
                    Some("unrelated".to_owned()),
                    None,
                    0,
                ),
                (
                    "unrelated".to_owned(),
                    Some(pending_id.clone()),
                    None,
                    None,
                    0,
                ),
                (payment_request.to_uppercase(), None, None, None, 0),
            ] {
                let response = QuoteService::list_melt_quotes(
                    &server,
                    Request::new(ListMeltQuotesRequest {
                        payment_request: Some(filter),
                        quote_id,
                        request_lookup_id: lookup,
                        state,
                        ..Default::default()
                    }),
                )
                .await
                .unwrap()
                .into_inner();
                assert_eq!(response.total, expected);
                assert_eq!(response.quotes.len() as u64, expected);
                for quote in response.quotes {
                    assert_eq!(quote.request, payment_request);
                }
            }
        }
    }

    #[tokio::test]
    async fn test_list_melt_quotes_rejects_invalid_filters_and_limit() {
        use crate::quote::ListMeltQuotesRequest;

        let server = create_test_rpc_server().await;
        for request in [
            ListMeltQuotesRequest {
                quote_id: Some(String::new()),
                ..Default::default()
            },
            ListMeltQuotesRequest {
                quote_id: Some("invalid!".to_owned()),
                ..Default::default()
            },
            ListMeltQuotesRequest {
                request_lookup_id: Some(String::new()),
                ..Default::default()
            },
            ListMeltQuotesRequest {
                limit: 1001,
                ..Default::default()
            },
            ListMeltQuotesRequest {
                payment_request: Some(String::new()),
                ..Default::default()
            },
            ListMeltQuotesRequest {
                state: Some(0),
                ..Default::default()
            },
            ListMeltQuotesRequest {
                state: Some(999),
                ..Default::default()
            },
        ] {
            let status = QuoteService::list_melt_quotes(&server, Request::new(request))
                .await
                .unwrap_err();
            assert_eq!(status.code(), Code::InvalidArgument);
        }
    }

    #[tokio::test]
    async fn test_resolve_melt_quote_validates_operator_input_and_mutation_guard() {
        use crate::quote::resolve_melt_quote_request::Resolution;
        use crate::quote::{CompensateMeltQuote, FinalizeMeltQuote, ResolveMeltQuoteRequest};

        let mut server = create_test_rpc_server().await;
        let valid = ResolveMeltQuoteRequest {
            quote_id: UNKNOWN_QUOTE_ID.to_owned(),
            operation_id: UNKNOWN_QUOTE_ID.to_owned(),
            expected_saga_state: "payment_pending".to_owned(),
            reason: "Backend confirms permanent failure".to_owned(),
            resolution: Some(Resolution::Compensate(CompensateMeltQuote {
                payment_failure_confirmed: true,
            })),
        };
        for request in [
            ResolveMeltQuoteRequest {
                quote_id: String::new(),
                ..valid.clone()
            },
            ResolveMeltQuoteRequest {
                operation_id: "invalid".to_owned(),
                ..valid.clone()
            },
            ResolveMeltQuoteRequest {
                expected_saga_state: "invalid".to_owned(),
                ..valid.clone()
            },
            ResolveMeltQuoteRequest {
                reason: " ".to_owned(),
                ..valid.clone()
            },
            ResolveMeltQuoteRequest {
                reason: "x".repeat(4097),
                ..valid.clone()
            },
            ResolveMeltQuoteRequest {
                resolution: None,
                ..valid.clone()
            },
            ResolveMeltQuoteRequest {
                resolution: Some(Resolution::Compensate(CompensateMeltQuote {
                    payment_failure_confirmed: false,
                })),
                ..valid.clone()
            },
            ResolveMeltQuoteRequest {
                resolution: Some(Resolution::Finalize(FinalizeMeltQuote {
                    total_spent: 100,
                    unit: "sat".to_owned(),
                    payment_lookup_id: "not-a-hash".to_owned(),
                    payment_lookup_id_kind: "payment_hash".to_owned(),
                    payment_proof: None,
                })),
                ..valid.clone()
            },
        ] {
            let status = QuoteService::resolve_melt_quote(&server, Request::new(request))
                .await
                .unwrap_err();
            assert_eq!(status.code(), Code::InvalidArgument);
        }
        let status = QuoteService::resolve_melt_quote(&server, Request::new(valid.clone()))
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::NotFound);
        server.mutation_guard = Some(Arc::new(RejectingMutationGuard));
        let status = QuoteService::resolve_melt_quote(&server, Request::new(valid))
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::FailedPrecondition);
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
    async fn test_quote_service_update_mint_quote_state_paid_twice_pays_once() {
        let server = create_test_rpc_server_with_payment_delay(3600)
            .await
            .with_mint_quote_payment_override(true);
        let quote_id = create_test_mint_quote(&server, 100).await;

        for _ in 0..2 {
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

            assert_eq!(response.state(), crate::quote::MintQuoteState::Paid);
        }

        assert_eq!(amount_paid(&server, &quote_id).await, 100);
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
