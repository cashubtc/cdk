use bitcoin::{block::Header, string::FromHexStr, BlockHash, CompactTarget, Transaction, Work};
use bitcoincore_rpc::{json::EstimateMode, RpcApi};
use lightning::chain::chaininterface::{BroadcasterInterface, ConfirmationTarget, FeeEstimator};
use lightning_block_sync::{
    gossip::UtxoSource, AsyncBlockSourceResult, BlockData, BlockHeaderData, BlockSource,
    BlockSourceError,
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
            ConfirmationTarget::MinAllowedAnchorChannelRemoteFee => 6,
            ConfirmationTarget::MinAllowedNonAnchorChannelRemoteFee => 6,
            ConfirmationTarget::AnchorChannelFee => 6,
            ConfirmationTarget::NonAnchorChannelFee => 6,
            ConfirmationTarget::ChannelCloseMinimum => 6,
            ConfirmationTarget::OutputSpendingFee => 6,
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
        _height_hint: Option<u32>,
    ) -> AsyncBlockSourceResult<'a, BlockHeaderData> {
        Box::pin(async move {
            let res = self
                .client
                .get_block_header_info(header_hash)
                .map_err(|e| match e {
                    bitcoincore_rpc::Error::JsonRpc(e) => match e {
                        bitcoincore_rpc::jsonrpc::Error::Transport(e) => {
                            BlockSourceError::transient(e)
                        }
                        e => BlockSourceError::persistent(e),
                    },
                    bitcoincore_rpc::Error::Io(e) => BlockSourceError::transient(e),
                    e => BlockSourceError::persistent(e),
                })?;
            Ok(BlockHeaderData {
                header: Header {
                    version: res.version,
                    prev_blockhash: res
                        .previous_block_hash
                        .ok_or(BlockSourceError::persistent("no previous block hash"))?,
                    merkle_root: res.merkle_root,
                    time: res.time as u32,
                    bits: CompactTarget::from_hex_str_no_prefix(&res.bits)
                        .map_err(|e| BlockSourceError::persistent(e))?,
                    nonce: res.nonce,
                },
                height: res.height as u32,
                chainwork: Work::from_be_bytes(
                    res.chainwork
                        .try_into()
                        .map_err(|_| BlockSourceError::persistent("invalid work"))?,
                ),
            })
        })
    }

    fn get_block<'a>(
        &'a self,
        header_hash: &'a BlockHash,
    ) -> AsyncBlockSourceResult<'a, BlockData> {
        Box::pin(async move {
            let res = self
                .client
                .get_block(header_hash)
                .map_err(|e| BlockSourceError::persistent(e))?;
            Ok(BlockData::FullBlock(res))
        })
    }

    fn get_best_block<'a>(&'a self) -> AsyncBlockSourceResult<(BlockHash, Option<u32>)> {
        Box::pin(async move {
            let block_hash = self
                .client
                .get_best_block_hash()
                .map_err(|e| BlockSourceError::persistent(e))?;
            Ok((block_hash, None))
        })
    }
}

impl UtxoSource for BitcoinClient {
    fn get_block_hash_by_height<'a>(
        &'a self,
        block_height: u32,
    ) -> AsyncBlockSourceResult<'a, BlockHash> {
        Box::pin(async move {
            let block_hash = self
                .client
                .get_block_hash(block_height as u64)
                .map_err(|e| BlockSourceError::persistent(e))?;
            Ok(block_hash)
        })
    }

    fn is_output_unspent<'a>(
        &'a self,
        outpoint: bitcoin::OutPoint,
    ) -> AsyncBlockSourceResult<'a, bool> {
        Box::pin(async move {
            let res = self
                .client
                .get_tx_out(&outpoint.txid, outpoint.vout as u32, None)
                .map_err(|e| BlockSourceError::persistent(e))?;
            Ok(res.is_some())
        })
    }
}
