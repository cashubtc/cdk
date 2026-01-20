//! NpubCash Proof Stream
//!
//! This stream continuously polls NpubCash for new paid quotes and yields them as proofs.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use cdk_common::amount::SplitTarget;
use cdk_common::MintQuoteState;
use futures::Stream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::nuts::{Proofs, SpendingConditions};
use crate::wallet::multi_mint_wallet::MultiMintWallet;
use crate::wallet::types::MintQuote;
use crate::wallet::Wallet;

/// Stream that continuously polls NpubCash and yields proofs as payments arrive
#[allow(missing_debug_implementations)]
pub struct NpubCashProofStream {
    rx: mpsc::Receiver<Result<(MintQuote, Proofs), Error>>,
    cancel: CancellationToken,
}

impl NpubCashProofStream {
    /// Create a new NpubCash proof stream
    pub fn new(
        wallet: MultiMintWallet,
        poll_interval: Duration,
        split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(32);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(poll_interval);

            loop {
                tokio::select! {
                    _ = cancel_clone.cancelled() => {
                        break;
                    }
                    _ = interval.tick() => {
                        match wallet.sync_npubcash_quotes().await {
                            Ok(quotes) => {
                                for quote in quotes {
                                    if matches!(quote.state, MintQuoteState::Paid) {
                                        let quote_id = quote.id.clone();
                                        let mint_url = quote.mint_url.clone();
                                        tracing::info!("Minting NpubCash quote {}...", quote_id);

                                        let result = async {
                                            // Get wallet for this quote's mint
                                            let wallet_instance = wallet.get_wallet(&mint_url).await.ok_or(Error::UnknownMint {
                                                mint_url: mint_url.to_string(),
                                            })?;

                                            let proofs = wallet_instance
                                                .mint(&quote_id, split_target.clone(), spending_conditions.clone())
                                                .await?;

                                            Ok((quote.clone(), proofs))
                                        }.await;

                                        if tx.send(result).await.is_err() {
                                            return; // Receiver dropped
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Error syncing NpubCash quotes: {}", e);
                                // Optional: Send error to stream? Or just log and retry?
                                // Logging is safer to keep stream alive.
                            }
                        }
                    }
                }
            }
        });

        Self { rx, cancel }
    }
}

impl Stream for NpubCashProofStream {
    type Item = Result<(MintQuote, Proofs), Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

impl Drop for NpubCashProofStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Stream that continuously polls NpubCash and yields proofs for a single Wallet
#[allow(missing_debug_implementations)]
pub struct WalletNpubCashProofStream {
    rx: mpsc::Receiver<Result<(MintQuote, Proofs), Error>>,
    cancel: CancellationToken,
}

impl WalletNpubCashProofStream {
    /// Create a new NpubCash proof stream for a single wallet
    pub fn new(
        wallet: Wallet,
        poll_interval: Duration,
        split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(32);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(poll_interval);

            loop {
                tokio::select! {
                    _ = cancel_clone.cancelled() => {
                        break;
                    }
                    _ = interval.tick() => {
                        match wallet.sync_npubcash_quotes().await {
                            Ok(quotes) => {
                                for quote in quotes {
                                    if matches!(quote.state, MintQuoteState::Paid) {
                                        let quote_id = quote.id.clone();
                                        let mint_url = quote.mint_url.clone();

                                        // Safety check: ensure the quote is for this wallet's mint
                                        if mint_url != wallet.mint_url {
                                            tracing::debug!("Skipping quote {} for different mint {}", quote_id, mint_url);
                                            continue;
                                        }

                                        tracing::info!("Minting NpubCash quote {}...", quote_id);

                                        let result = async {
                                            let proofs = wallet
                                                .mint(&quote_id, split_target.clone(), spending_conditions.clone())
                                                .await?;
                                            Ok((quote.clone(), proofs))
                                        }.await;

                                        if tx.send(result).await.is_err() {
                                            return; // Receiver dropped
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Error syncing NpubCash quotes: {}", e);
                            }
                        }
                    }
                }
            }
        });

        Self { rx, cancel }
    }
}

impl Stream for WalletNpubCashProofStream {
    type Item = Result<(MintQuote, Proofs), Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

impl Drop for WalletNpubCashProofStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
