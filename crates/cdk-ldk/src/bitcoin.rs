use bitcoin::{BlockHash, Transaction};
use bitcoincore_rpc::{json::EstimateMode, RpcApi};
use lightning::chain::chaininterface::{BroadcasterInterface, ConfirmationTarget, FeeEstimator};
use lightning_block_sync::{
    gossip::UtxoSource, AsyncBlockSourceResult, BlockData, BlockHeaderData, BlockSource,
};

use crate::Error;

pub struct BitcoinClient {
    client: bitcoincore_rpc::Client,
}

impl BitcoinClient {
    pub fn new(url: &str, user: &str, pass: &str) -> Result<Self, Error> {
        let client = bitcoincore_rpc::Client::new(
            url,
            bitcoincore_rpc::Auth::UserPass(user.to_string(), pass.to_string()),
        )?;
        Ok(Self { client })
    }
}

impl FeeEstimator for BitcoinClient {
    fn get_est_sat_per_1000_weight(&self, confirmation_target: ConfirmationTarget) -> u32 {
        let blocks = match confirmation_target {
            ConfirmationTarget::OnChainSweep => 6,
            ConfirmationTarget::MinAllowedAnchorChannelRemoteFee => todo!(),
            ConfirmationTarget::MinAllowedNonAnchorChannelRemoteFee => todo!(),
            ConfirmationTarget::AnchorChannelFee => todo!(),
            ConfirmationTarget::NonAnchorChannelFee => todo!(),
            ConfirmationTarget::ChannelCloseMinimum => todo!(),
            ConfirmationTarget::OutputSpendingFee => todo!(),
        };
        // LDK will wrap this to the minimum fee rate
        match self
            .client
            .estimate_smart_fee(blocks, Some(EstimateMode::Economical))
        {
            Ok(res) => {
                let amount = res.fee_rate.unwrap_or_default();
                (amount.to_sat() / 4) as u32
            }
            Err(e) => {
                tracing::error!("Failed to estimate fee: {}", e);
                0
            }
        }
    }
}

impl BroadcasterInterface for BitcoinClient {
    fn broadcast_transactions(&self, txs: &[&Transaction]) {
        for tx in txs {
            let txid = tx.txid();
            tracing::debug!("Broadcasting transaction: {}", txid);
            match self.client.send_raw_transaction(*tx) {
                Ok(_) => tracing::info!("Transaction broadcasted: {}", txid),
                Err(_) => tracing::error!("Failed to broadcast transaction: {}", txid),
            }
        }
    }
}

impl BlockSource for BitcoinClient {
    fn get_header<'a>(
        &'a self,
        header_hash: &'a BlockHash,
        height_hint: Option<u32>,
    ) -> AsyncBlockSourceResult<'a, BlockHeaderData> {
        todo!()
    }

    fn get_block<'a>(
        &'a self,
        header_hash: &'a BlockHash,
    ) -> AsyncBlockSourceResult<'a, BlockData> {
        todo!()
    }

    fn get_best_block<'a>(&'a self) -> AsyncBlockSourceResult<(BlockHash, Option<u32>)> {
        todo!()
    }
}

impl UtxoSource for BitcoinClient {
    fn get_block_hash_by_height<'a>(
        &'a self,
        block_height: u32,
    ) -> AsyncBlockSourceResult<'a, BlockHash> {
        todo!()
    }

    fn is_output_unspent<'a>(
        &'a self,
        outpoint: bitcoin::OutPoint,
    ) -> AsyncBlockSourceResult<'a, bool> {
        todo!()
    }
}
