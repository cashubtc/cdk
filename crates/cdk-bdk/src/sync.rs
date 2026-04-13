use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client};
use bdk_bitcoind_rpc::{Emitter, NO_EXPECTED_MEMPOOL_TXS};
use bdk_esplora::esplora_client::Builder;
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::WalletEvent;
use tokio::time::{interval, Duration};

use crate::error::Error;
use crate::{CdkBdk, ChainSource};

impl CdkBdk {
    pub(crate) async fn sync_wallet(&self) -> Result<(), Error> {
        match &self.chain_source {
            ChainSource::BitcoinRpc(rpc_config) => {
                let mut sync_interval = interval(Duration::from_secs(self.sync_interval_secs));
                let mut startup_reconciliation_pending = true;

                tracing::info!("Starting continuous block monitoring...");
                loop {
                    tokio::select! {
                        _ = self.events_cancel_token.cancelled() => {
                            tracing::info!("Wallet sync cancelled via cancel token");
                            break;
                        }
                        _ = sync_interval.tick() => {
                            let mut found_blocks = vec![];

                            {
                                let rpc_client: Client = Client::new(
                                    &format!("http://{}:{}", rpc_config.host, rpc_config.port),
                                    Auth::UserPass(
                                        rpc_config.user.clone(),
                                        rpc_config.password.clone(),
                                    ),
                                )?;

                                let mut wallet_with_db =
                                    self.wallet_with_db.lock().await;
                                let wallet_tip =
                                    wallet_with_db.wallet.latest_checkpoint();

                                let mut emitter = Emitter::new(
                                    &rpc_client,
                                    wallet_tip.clone(),
                                    wallet_tip.height(),
                                    NO_EXPECTED_MEMPOOL_TXS,
                                );

                                while let Some(block) = emitter.next_block()? {
                                    found_blocks.push(block.block_height());

                                    wallet_with_db
                                        .wallet
                                        .apply_block_connected_to(
                                            &block.block,
                                            block.block_height(),
                                            block.connected_to(),
                                        )
                                        .map_err(|e| Error::Wallet(e.to_string()))?;
                                }

                                if !found_blocks.is_empty() {
                                    wallet_with_db.persist()?;
                                    let checkpoint =
                                        wallet_with_db.wallet.latest_checkpoint();

                                    tracing::info!(
                                        "New block {} at height {}",
                                        checkpoint.block_id().hash,
                                        checkpoint.block_id().height
                                    );
                                }
                            }

                            if !found_blocks.is_empty() {
                                for block in &found_blocks {
                                    tracing::debug!("Scanning wallet outputs after block {}", block);
                                }
                            }

                            if startup_reconciliation_pending || !found_blocks.is_empty() {
                                self.scan_for_new_payments().await?;
                                self.check_receive_saga_confirmations().await?;
                                self.check_send_saga_confirmations().await?;
                                startup_reconciliation_pending = false;
                            }
                        }
                    }
                }
            }
            ChainSource::Esplora {
                url,
                parallel_requests,
            } => {
                let mut sync_interval = interval(Duration::from_secs(self.sync_interval_secs));

                tracing::info!("Starting esplora block sync...");
                loop {
                    tokio::select! {
                        _ = self.events_cancel_token.cancelled() => {
                            tracing::info!("Wallet sync cancelled via cancel token");
                            break;
                        }
                        _ = sync_interval.tick() => {
                            let client = Builder::new(url)
                                .build_async()
                                .map_err(|e| Error::Esplora(e.to_string()))?;

                            let mut wallet_with_db = self.wallet_with_db.lock().await;

                            let sync_request = wallet_with_db.wallet.start_sync_with_revealed_spks();

                            let sync_update = client
                                .sync(sync_request, *parallel_requests)
                                .await
                                .map_err(|e| Error::Esplora(e.to_string()))?;

                            tracing::debug!(
                                parallel_requests = *parallel_requests,
                                "Applying esplora wallet sync update"
                            );
                            let events = wallet_with_db
                                .wallet
                                .apply_update_events(sync_update)
                                .map_err(|e| Error::Wallet(e.to_string()))?;
                            wallet_with_db.persist()?;
                            let checkpoint = wallet_with_db.wallet.latest_checkpoint();

                            tracing::info!(
                                "Esplora synced to block {} at height {}",
                                checkpoint.block_id().hash,
                                checkpoint.block_id().height
                            );

                            let has_relevant_events = events.iter().any(|e| matches!(
                                e,
                                WalletEvent::TxConfirmed { .. } | WalletEvent::ChainTipChanged { .. }
                            ));

                            if has_relevant_events {
                                drop(wallet_with_db);

                                self.scan_for_new_payments().await?;
                                self.check_receive_saga_confirmations().await?;
                                self.check_send_saga_confirmations().await?;
                            }
                        }
                    }
                }
            }
        };

        Ok(())
    }
}
