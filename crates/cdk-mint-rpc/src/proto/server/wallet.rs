//! On-chain wallet management service.

use tonic::{Request, Response, Status};

use super::{page_limit, MintRPCServer};
use crate::wallet::wallet_service_server::WalletService;
use crate::{DynWalletInfoProvider, WalletAddressPage, WalletTransactionPage};

const DEFAULT_TRANSACTION_LIMIT: u32 = 20;
const MAX_TRANSACTION_LIMIT: u32 = 100;
const DEFAULT_ADDRESS_LIMIT: u32 = 100;
const MAX_ADDRESS_LIMIT: u32 = 1_000;

impl MintRPCServer {
    fn wallet_info_provider(&self) -> Result<&DynWalletInfoProvider, Status> {
        self.wallet_info_provider.as_ref().ok_or_else(|| {
            Status::failed_precondition("No on-chain wallet information provider is configured")
        })
    }
}

#[tonic::async_trait]
impl WalletService for MintRPCServer {
    /// Creates an on-chain address for operator deposits.
    async fn create_deposit_address(
        &self,
        _request: Request<crate::wallet::CreateDepositAddressRequest>,
    ) -> Result<Response<crate::wallet::CreateDepositAddressResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let address = self
            .wallet_info_provider()?
            .create_deposit_address()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(crate::wallet::CreateDepositAddressResponse {
            address,
        }))
    }

    /// Gets the on-chain wallet balance.
    async fn get_balance(
        &self,
        _request: Request<crate::wallet::GetBalanceRequest>,
    ) -> Result<Response<crate::wallet::GetBalanceResponse>, Status> {
        let balance = self
            .wallet_info_provider()?
            .get_balance()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(balance))
    }

    /// Lists on-chain wallet transactions.
    async fn list_transactions(
        &self,
        request: Request<crate::wallet::ListTransactionsRequest>,
    ) -> Result<Response<crate::wallet::ListTransactionsResponse>, Status> {
        let request = request.into_inner();
        let limit = page_limit(
            request.limit,
            DEFAULT_TRANSACTION_LIMIT,
            MAX_TRANSACTION_LIMIT,
        )?;

        let WalletTransactionPage {
            transactions,
            total,
        } = self
            .wallet_info_provider()?
            .list_transactions(request.offset as usize, limit)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(crate::wallet::ListTransactionsResponse {
            transactions,
            total,
        }))
    }

    /// Lists addresses revealed by the on-chain wallet.
    async fn list_addresses(
        &self,
        request: Request<crate::wallet::ListAddressesRequest>,
    ) -> Result<Response<crate::wallet::ListAddressesResponse>, Status> {
        let request = request.into_inner();
        let limit = page_limit(request.limit, DEFAULT_ADDRESS_LIMIT, MAX_ADDRESS_LIMIT)?;

        let WalletAddressPage { addresses, total } = self
            .wallet_info_provider()?
            .list_addresses(request.offset as usize, limit)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(crate::wallet::ListAddressesResponse {
            addresses,
            total,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::test_utils::create_test_rpc_server;
    use super::*;

    struct TestWalletInfoProvider;

    #[async_trait::async_trait]
    impl crate::WalletInfoProvider for TestWalletInfoProvider {
        async fn create_deposit_address(&self) -> Result<String, crate::WalletInfoError> {
            Ok("bcrt1qoperatordeposit".to_string())
        }

        async fn get_balance(
            &self,
        ) -> Result<crate::wallet::GetBalanceResponse, crate::WalletInfoError> {
            Ok(crate::wallet::GetBalanceResponse {
                confirmed_sat: 21,
                trusted_pending_sat: 2,
                untrusted_pending_sat: 3,
                immature_sat: 4,
                trusted_spendable_sat: 23,
                total_sat: 30,
                network: "regtest".to_string(),
                synced_height: 123,
            })
        }

        async fn list_transactions(
            &self,
            offset: usize,
            limit: usize,
        ) -> Result<crate::WalletTransactionPage, crate::WalletInfoError> {
            Ok(crate::WalletTransactionPage {
                transactions: vec![crate::wallet::WalletTransaction {
                    txid: format!("{offset}:{limit}"),
                    inputs: vec![crate::wallet::WalletTransactionInput {
                        txid: "previous-txid".to_string(),
                        vout: 1,
                        amount_sat: Some(42_000),
                        address: Some("bcrt1qinput".to_string()),
                    }],
                    outputs: vec![crate::wallet::WalletTransactionOutput {
                        vout: 2,
                        address: "bcrt1qoutput".to_string(),
                        amount_sat: 21_000,
                        quote_id: Some("quote-id".to_string()),
                    }],
                    ..Default::default()
                }],
                total: 7,
            })
        }

        async fn list_addresses(
            &self,
            offset: usize,
            limit: usize,
        ) -> Result<crate::WalletAddressPage, crate::WalletInfoError> {
            Ok(crate::WalletAddressPage {
                addresses: vec![crate::wallet::WalletAddress {
                    address: format!("{offset}:{limit}"),
                    ..Default::default()
                }],
                total: 9,
            })
        }
    }

    #[tokio::test]
    async fn wallet_service_requires_a_configured_provider() {
        let server = create_test_rpc_server().await;

        let error = server
            .get_balance(Request::new(crate::wallet::GetBalanceRequest {}))
            .await
            .expect_err("wallet provider should be required");

        assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    }

    #[tokio::test]
    async fn wallet_service_returns_provider_data_and_applies_page_defaults() {
        let server = create_test_rpc_server()
            .await
            .with_wallet_info_provider(Arc::new(TestWalletInfoProvider));

        let deposit_address = server
            .create_deposit_address(Request::new(crate::wallet::CreateDepositAddressRequest {}))
            .await
            .expect("create deposit address")
            .into_inner();
        assert_eq!(deposit_address.address, "bcrt1qoperatordeposit");

        let balance = server
            .get_balance(Request::new(crate::wallet::GetBalanceRequest {}))
            .await
            .expect("get balance")
            .into_inner();
        assert_eq!(balance.confirmed_sat, 21);
        assert_eq!(balance.total_sat, 30);
        assert_eq!(balance.network, "regtest");
        assert_eq!(balance.synced_height, 123);

        let transactions = server
            .list_transactions(Request::new(crate::wallet::ListTransactionsRequest {
                limit: 0,
                offset: 2,
            }))
            .await
            .expect("list transactions")
            .into_inner();
        assert_eq!(transactions.total, 7);
        assert_eq!(transactions.transactions[0].txid, "2:20");
        let input = &transactions.transactions[0].inputs[0];
        assert_eq!(input.txid, "previous-txid");
        assert_eq!(input.vout, 1);
        assert_eq!(input.amount_sat, Some(42_000));
        assert_eq!(input.address.as_deref(), Some("bcrt1qinput"));
        let output = &transactions.transactions[0].outputs[0];
        assert_eq!(output.vout, 2);
        assert_eq!(output.address, "bcrt1qoutput");
        assert_eq!(output.amount_sat, 21_000);
        assert_eq!(output.quote_id.as_deref(), Some("quote-id"));

        let addresses = server
            .list_addresses(Request::new(crate::wallet::ListAddressesRequest {
                limit: 3,
                offset: 4,
            }))
            .await
            .expect("list addresses")
            .into_inner();
        assert_eq!(addresses.total, 9);
        assert_eq!(addresses.addresses[0].address, "4:3");
    }

    #[test]
    fn wallet_service_rejects_oversized_pages() {
        let error = page_limit(101, DEFAULT_TRANSACTION_LIMIT, MAX_TRANSACTION_LIMIT)
            .expect_err("oversized page should fail");

        assert_eq!(error.code(), tonic::Code::InvalidArgument);
    }
}
