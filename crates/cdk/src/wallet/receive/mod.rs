//! Receive module for the wallet.
//!
//! This module provides functionality for receiving ecash tokens and proofs.

use std::str::FromStr;

use cdk_common::wallet::{TransactionDirection, TransactionId};
use tracing::instrument;

use crate::nuts::{Proofs, Token};
use crate::{ensure_cdk, Amount, Error, Wallet};

pub(crate) mod saga;

pub use cdk_common::wallet::ReceiveOptions;
use saga::ReceiveSaga;

impl Wallet {
    /// Receive proofs using the saga pattern
    ///
    /// This is the internal implementation that uses the saga pattern
    /// for proper crash recovery and compensation.
    #[instrument(skip_all)]
    pub async fn receive_proofs(
        &self,
        proofs: Proofs,
        opts: ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Amount, Error> {
        self.retry_on_inactive_keyset(|| async {
            let transaction_id = TransactionId::from_proofs(proofs.clone())?;
            if let Some(transaction) = self.get_transaction(transaction_id).await? {
                if transaction.direction == TransactionDirection::Incoming
                    && transaction.status == cdk_common::wallet::TransactionStatus::Completed
                {
                    return Ok(transaction.amount);
                }
            }

            let saga = ReceiveSaga::new(self);
            let saga = saga
                .prepare(proofs.clone(), opts.clone(), memo.clone(), token.clone())
                .await?;
            let saga = saga.execute().await?;
            Ok(saga.into_amount())
        })
        .await
    }

    /// Receive
    /// # Synopsis
    /// ```rust, no_run
    ///  use std::sync::Arc;
    ///
    ///  use cdk::amount::SplitTarget;
    ///  use cdk_sqlite::wallet::memory;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::{ReceiveOptions, Wallet};
    ///  use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///  let seed = random::<[u8; 64]>();
    ///  let mint_url = "https://testnut.cashudevkit.org";
    ///  let unit = CurrencyUnit::Sat;
    ///
    ///  let localstore = memory::empty().await?;
    ///  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
    ///  let token = "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJhbW91bnQiOjEsInNlY3JldCI6ImI0ZjVlNDAxMDJhMzhiYjg3NDNiOTkwMzU5MTU1MGYyZGEzZTQxNWEzMzU0OTUyN2M2MmM5ZDc5MGVmYjM3MDUiLCJDIjoiMDIzYmU1M2U4YzYwNTMwZWVhOWIzOTQzZmRhMWEyY2U3MWM3YjNmMGNmMGRjNmQ4NDZmYTc2NWFhZjc3OWZhODFkIiwiaWQiOiIwMDlhMWYyOTMyNTNlNDFlIn1dLCJtaW50IjoiaHR0cHM6Ly90ZXN0bnV0LmNhc2h1LnNwYWNlIn1dLCJ1bml0Ijoic2F0In0=";
    ///  let amount_receive = wallet.receive(token, ReceiveOptions::default()).await?;
    ///  Ok(())
    /// }
    /// ```
    #[instrument(skip_all)]
    pub async fn receive(
        &self,
        encoded_token: &str,
        opts: ReceiveOptions,
    ) -> Result<Amount, Error> {
        let token = Token::from_str(encoded_token)?;

        let unit = token.unit().unwrap_or_default();

        ensure_cdk!(unit == self.unit, Error::UnsupportedUnit);

        let proofs = self.token_proofs(&token).await?;

        if let Token::TokenV3(token) = &token {
            ensure_cdk!(!token.is_multi_mint(), Error::MultiMintTokenNotSupported);
        }

        ensure_cdk!(self.mint_url == token.mint_url()?, Error::IncorrectMint);

        let amount = self
            .receive_proofs(
                proofs,
                opts,
                token.memo().clone(),
                Some(encoded_token.to_string()),
            )
            .await?;

        Ok(amount)
    }

    /// Receive
    /// # Synopsis
    /// ```rust, no_run
    ///  use std::sync::Arc;
    ///
    ///  use cdk::amount::SplitTarget;
    ///  use cdk_sqlite::wallet::memory;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::{ReceiveOptions, Wallet};
    ///  use cdk::util::hex;
    ///  use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///  let seed = random::<[u8; 64]>();
    ///  let mint_url = "https://testnut.cashudevkit.org";
    ///  let unit = CurrencyUnit::Sat;
    ///
    ///  let localstore = memory::empty().await?;
    ///  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
    ///  let token_raw = hex::decode("6372617742a4617481a261694800ad268c4d1f5826617081a3616101617378403961366462623834376264323332626137366462306466313937323136623239643362386363313435353363643237383237666331636339343266656462346561635821038618543ffb6b8695df4ad4babcde92a34a96bdcd97dcee0d7ccf98d4721267926164695468616e6b20796f75616d75687474703a2f2f6c6f63616c686f73743a33333338617563736174").unwrap();
    ///  let amount_receive = wallet.receive_raw(&token_raw, ReceiveOptions::default()).await?;
    ///  Ok(())
    /// }
    /// ```
    #[instrument(skip_all)]
    pub async fn receive_raw(
        &self,
        binary_token: &Vec<u8>,
        opts: ReceiveOptions,
    ) -> Result<Amount, Error> {
        let token_str = Token::try_from(binary_token)?.to_string();
        self.receive(token_str.as_str(), opts).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cdk_common::util::unix_time;
    use cdk_common::wallet::{Transaction, TransactionDirection};

    use super::*;
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet, test_keyset_id, test_mint_url, test_proof_info,
    };

    #[tokio::test]
    async fn receive_proofs_returns_existing_legacy_incoming_transaction() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db.clone()).await;
        let proof = test_proof_info(test_keyset_id(), 100, test_mint_url());
        let expected = Amount::from(99);
        db.add_transaction(Transaction {
            status: cdk_common::wallet::TransactionStatus::Completed,
            mint_url: test_mint_url(),
            direction: TransactionDirection::Incoming,
            amount: expected,
            fee: Amount::from(1),
            unit: crate::nuts::CurrencyUnit::Sat,
            ys: vec![proof.y],
            timestamp: unix_time(),
            memo: None,
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: None,
        })
        .await
        .expect("existing receive transaction");

        let amount = wallet
            .receive_proofs(vec![proof.proof], ReceiveOptions::default(), None, None)
            .await
            .expect("idempotent receive result");

        assert_eq!(amount, expected);
    }
}
