//! Receive module for the wallet.
//!
//! This module provides functionality for receiving ecash tokens and proofs.

mod api;

use std::str::FromStr;

use tracing::instrument;

pub use self::api::{ReceiveReceipt, ReceiveRequest};
use crate::nuts::{Proofs, Token};
use crate::{ensure_cdk, Amount, Error, Wallet};

pub(crate) mod saga;

pub(crate) use cdk_common::wallet::ReceiveOptions;
use saga::ReceiveSaga;

impl Wallet {
    /// Receive proofs using the saga pattern
    ///
    /// This is the internal implementation that uses the saga pattern
    /// for proper crash recovery and compensation.
    #[instrument(skip_all)]
    pub(crate) async fn receive_proofs(
        &self,
        proofs: Proofs,
        opts: ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Amount, Error> {
        Ok(self
            .receive_proofs_with_operation(proofs, opts, memo, token)
            .await?
            .1)
    }

    pub(crate) async fn receive_proofs_with_operation(
        &self,
        proofs: Proofs,
        opts: ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<(uuid::Uuid, Amount), Error> {
        self.retry_on_inactive_keyset(|| async {
            let saga = ReceiveSaga::new(self);
            let saga = saga
                .prepare(proofs.clone(), opts.clone(), memo.clone(), token.clone())
                .await?;
            let saga = saga.execute().await?;
            Ok(saga.into_parts())
        })
        .await
    }

    pub(crate) async fn receive_token_with_operation(
        &self,
        encoded_token: &str,
        opts: ReceiveOptions,
    ) -> Result<(uuid::Uuid, Amount), Error> {
        let token = Token::from_str(encoded_token)?;

        let unit = token.unit().unwrap_or_default();

        ensure_cdk!(unit == self.unit, Error::UnsupportedUnit);

        let proofs = self.token_proofs(&token).await?;

        if let Token::TokenV3(token) = &token {
            ensure_cdk!(!token.is_multi_mint(), Error::MultiMintTokenNotSupported);
        }

        ensure_cdk!(self.mint_url == token.mint_url()?, Error::IncorrectMint);

        self.receive_proofs_with_operation(
            proofs,
            opts,
            token.memo().clone(),
            Some(encoded_token.to_string()),
        )
        .await
    }
}
