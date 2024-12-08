#[cfg(test)]
mod integration_tests_pure {
    use std::assert_eq;
    use std::collections::HashMap;
    use std::fmt::{Debug, Formatter};
    use std::str::FromStr;
    use std::sync::Arc;

    use async_trait::async_trait;
    use cdk::amount::SplitTarget;
    use cdk::cdk_database::mint_memory::MintMemoryDatabase;
    use cdk::cdk_database::WalletMemoryDatabase;
    use cdk::nuts::{
        CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeySet, KeysetResponse,
        MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintBolt11Request,
        MintBolt11Response, MintInfo, MintQuoteBolt11Request, MintQuoteBolt11Response,
        MintQuoteState, Nuts, RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
    };
    use cdk::types::QuoteTTL;
    use cdk::util::unix_time;
    use cdk::wallet::client::MintConnector;
    use cdk::{Amount, Error, Mint, Wallet};
    use cdk_integration_tests::create_backends_fake_wallet;
    use rand::random;
    use tokio::sync::Notify;
    use uuid::Uuid;

    struct DirectMintConnection {
        mint: Arc<Mint>,
    }

    impl Debug for DirectMintConnection {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "DirectMintConnection {{ mint_info: {:?} }}",
                self.mint.mint_info
            )
        }
    }

    /// Implements the generic [MintConnector] (i.e. use the interface that expects to communicate
    /// to a generic mint, where we don't know that quote ID's are [Uuid]s) for [DirectMintConnection],
    /// where we know we're dealing with a mint that uses [Uuid]s for quotes.
    /// Convert the requests and responses between the [String] and [Uuid] variants as necessary.
    #[async_trait]
    impl MintConnector for DirectMintConnection {
        async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
            self.mint.pubkeys().await.map(|pks| pks.keysets)
        }

        async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
            self.mint
                .keyset(&keyset_id)
                .await
                .and_then(|res| res.ok_or(Error::UnknownKeySet))
        }

        async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
            self.mint.keysets().await
        }

        async fn post_mint_quote(
            &self,
            request: MintQuoteBolt11Request,
        ) -> Result<MintQuoteBolt11Response<String>, Error> {
            self.mint
                .get_mint_bolt11_quote(request)
                .await
                .map(Into::into)
        }

        async fn get_mint_quote_status(
            &self,
            quote_id: &str,
        ) -> Result<MintQuoteBolt11Response<String>, Error> {
            let quote_id_uuid = Uuid::from_str(quote_id).unwrap();
            self.mint
                .check_mint_quote(&quote_id_uuid)
                .await
                .map(Into::into)
        }

        async fn post_mint(
            &self,
            request: MintBolt11Request<String>,
        ) -> Result<MintBolt11Response, Error> {
            let request_uuid = request.try_into().unwrap();
            self.mint.process_mint_request(request_uuid).await
        }

        async fn post_melt_quote(
            &self,
            request: MeltQuoteBolt11Request,
        ) -> Result<MeltQuoteBolt11Response<String>, Error> {
            self.mint
                .get_melt_bolt11_quote(&request)
                .await
                .map(Into::into)
        }

        async fn get_melt_quote_status(
            &self,
            quote_id: &str,
        ) -> Result<MeltQuoteBolt11Response<String>, Error> {
            let quote_id_uuid = Uuid::from_str(quote_id).unwrap();
            self.mint
                .check_melt_quote(&quote_id_uuid)
                .await
                .map(Into::into)
        }

        async fn post_melt(
            &self,
            request: MeltBolt11Request<String>,
        ) -> Result<MeltQuoteBolt11Response<String>, Error> {
            let request_uuid = request.try_into().unwrap();
            self.mint.melt_bolt11(&request_uuid).await.map(Into::into)
        }

        async fn post_swap(&self, swap_request: SwapRequest) -> Result<SwapResponse, Error> {
            self.mint.process_swap_request(swap_request).await
        }

        async fn get_mint_info(&self) -> Result<MintInfo, Error> {
            Ok(self.mint.mint_info().clone().time(unix_time()))
        }

        async fn post_check_state(
            &self,
            request: CheckStateRequest,
        ) -> Result<CheckStateResponse, Error> {
            self.mint.check_state(&request).await
        }

        async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
            self.mint.restore(request).await
        }
    }

    fn get_mint_connector(mint: Arc<Mint>) -> DirectMintConnection {
        DirectMintConnection { mint }
    }

    async fn create_and_start_test_mint() -> anyhow::Result<Arc<Mint>> {
        let fee: u64 = 0;
        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::Sat, (fee, 32));

        let nuts = Nuts::new()
            .nut07(true)
            .nut08(true)
            .nut09(true)
            .nut10(true)
            .nut11(true)
            .nut12(true)
            .nut14(true);

        let mint_info = MintInfo::new().nuts(nuts);

        let quote_ttl = QuoteTTL::new(10000, 10000);

        let mint_url = "http://aaa";

        let seed = random::<[u8; 32]>();
        let mint: Mint = Mint::new(
            mint_url,
            &seed,
            mint_info,
            quote_ttl,
            Arc::new(MintMemoryDatabase::default()),
            create_backends_fake_wallet(),
            supported_units,
            HashMap::new(),
        )
        .await?;

        let mint_arc = Arc::new(mint);

        let mint_arc_clone = Arc::clone(&mint_arc);
        let shutdown = Arc::new(Notify::new());
        tokio::spawn({
            let shutdown = Arc::clone(&shutdown);
            async move { mint_arc_clone.wait_for_paid_invoices(shutdown).await }
        });

        Ok(mint_arc)
    }

    fn create_test_wallet_for_mint(mint: Arc<Mint>) -> anyhow::Result<Arc<Wallet>> {
        let connector = get_mint_connector(mint);

        let seed = random::<[u8; 32]>();
        let mint_url = connector.mint.mint_url.to_string();
        let unit = CurrencyUnit::Sat;

        let localstore = WalletMemoryDatabase::default();
        let mut wallet = Wallet::new(&mint_url, unit, Arc::new(localstore), &seed, None)?;

        wallet.set_client(Arc::from(connector));

        Ok(Arc::new(wallet))
    }

    /// Creates a mint quote for the given amount and checks its state in a loop. Returns when
    /// amount is minted.
    async fn receive(wallet: Arc<Wallet>, amount: u64) -> anyhow::Result<Amount> {
        let desired_amount = Amount::from(amount);
        let quote = wallet.mint_quote(desired_amount, None).await?;

        loop {
            let status = wallet.mint_quote_state(&quote.id).await?;
            if status.state == MintQuoteState::Paid {
                break;
            }
        }

        wallet
            .mint(&quote.id, SplitTarget::default(), None)
            .await
            .map_err(Into::into)
    }

    mod nut03 {
        use cdk::nuts::nut00::ProofsMethods;
        use cdk::wallet::SendKind;

        use crate::integration_tests_pure::*;

        #[tokio::test]
        async fn test_swap_to_send() -> anyhow::Result<()> {
            let mint_bob = create_and_start_test_mint().await?;
            let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())?;

            // Alice gets 64 sats
            receive(wallet_alice.clone(), 64).await?;
            let balance_alice = wallet_alice.total_balance().await?;
            assert_eq!(Amount::from(64), balance_alice);

            // Alice wants to send 40 sats, which internally swaps
            let token = wallet_alice
                .send(
                    Amount::from(40),
                    None,
                    None,
                    &SplitTarget::None,
                    &SendKind::OnlineExact,
                    false,
                )
                .await?;
            assert_eq!(Amount::from(40), token.proofs().total_amount()?);
            assert_eq!(Amount::from(24), wallet_alice.total_balance().await?);

            // Alice sends cashu, Carol receives
            let wallet_carol = create_test_wallet_for_mint(mint_bob.clone())?;
            let received_amount = wallet_carol
                .receive_proofs(token.proofs(), SplitTarget::None, &[], &[])
                .await?;

            assert_eq!(Amount::from(40), received_amount);
            assert_eq!(Amount::from(40), wallet_carol.total_balance().await?);

            Ok(())
        }
    }
}
