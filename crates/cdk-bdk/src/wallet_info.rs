//! Read-only wallet information for operator interfaces.

use std::collections::HashMap;

use bdk_wallet::bitcoin::Address;
use bdk_wallet::chain::ChainPosition;
use bdk_wallet::KeychainKind;

use crate::{CdkBdk, Error};

/// BDK wallet balance split by confirmation status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletBalance {
    /// Bitcoin network used by the wallet.
    pub network: String,
    /// Height of the wallet's latest local chain checkpoint.
    pub synced_height: u32,
    /// Confirmed, spendable balance in satoshis.
    pub confirmed_sat: u64,
    /// Unconfirmed wallet-created outputs in satoshis.
    pub trusted_pending_sat: u64,
    /// Unconfirmed externally-created outputs in satoshis.
    pub untrusted_pending_sat: u64,
    /// Immature coinbase outputs in satoshis.
    pub immature_sat: u64,
    /// Confirmed plus trusted-pending balance in satoshis.
    pub trusted_spendable_sat: u64,
    /// Total wallet balance in satoshis.
    pub total_sat: u64,
}

/// A transaction relevant to the BDK wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletTransaction {
    /// Transaction ID.
    pub txid: String,
    /// Inputs spent by this transaction in input order.
    pub inputs: Vec<WalletTransactionInput>,
    /// Non-change payment outputs in transaction output order.
    pub outputs: Vec<WalletTransactionOutput>,
    /// Value received by wallet scripts in satoshis.
    pub received_sat: u64,
    /// Value spent from wallet inputs in satoshis.
    pub sent_sat: u64,
    /// Transaction fee in satoshis, when all previous outputs are known.
    pub fee_sat: Option<u64>,
    /// Net effect on the wallet balance in satoshis.
    pub balance_delta_sat: i64,
    /// Confirmation block height, when confirmed.
    pub confirmation_height: Option<u32>,
    /// Confirmation block timestamp, when confirmed.
    pub confirmation_time: Option<u64>,
    /// First-seen timestamp, when known for an unconfirmed transaction.
    pub first_seen: Option<u64>,
}

/// An input spent by a wallet transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletTransactionInput {
    /// Transaction ID containing the previous output.
    pub txid: String,
    /// Index of the previous output.
    pub vout: u32,
    /// Value of the previous output in satoshis, when known.
    pub amount_sat: Option<u64>,
    /// Address of the previous output, when it belongs to the wallet.
    pub address: Option<String>,
}

/// A payment output belonging to a wallet transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletTransactionOutput {
    /// Transaction output index.
    pub vout: u32,
    /// Bitcoin address in network-specific display encoding.
    pub address: String,
    /// Output value in satoshis.
    pub amount_sat: u64,
    /// Quote associated with this output, when managed by the payment backend.
    pub quote_id: Option<String>,
}

/// BDK keychain containing a revealed address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WalletKeychain {
    /// Address intended for incoming payments.
    External,
    /// Internal change address.
    Internal,
}

/// An address revealed by the BDK wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletAddress {
    /// Bitcoin address.
    pub address: String,
    /// Descriptor keychain.
    pub keychain: WalletKeychain,
    /// Child derivation index.
    pub derivation_index: u32,
    /// Whether the address has appeared in a wallet transaction.
    pub used: bool,
    /// Total current unspent balance in satoshis.
    pub balance_sat: u64,
    /// Confirmed current unspent balance in satoshis.
    pub confirmed_balance_sat: u64,
}

/// A page of wallet records and the total number available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletPage<T> {
    /// Records in this page.
    pub items: Vec<T>,
    /// Total records before pagination.
    pub total: u64,
}

impl CdkBdk {
    /// Returns the wallet balance split by confirmation status.
    pub async fn wallet_balance(&self) -> WalletBalance {
        let wallet_with_db = self.wallet_with_db.lock().await;
        let balance = wallet_with_db.wallet.balance();

        WalletBalance {
            network: self.network.to_string(),
            synced_height: wallet_with_db.wallet.latest_checkpoint().height(),
            confirmed_sat: balance.confirmed.to_sat(),
            trusted_pending_sat: balance.trusted_pending.to_sat(),
            untrusted_pending_sat: balance.untrusted_pending.to_sat(),
            immature_sat: balance.immature.to_sat(),
            trusted_spendable_sat: balance.trusted_spendable().to_sat(),
            total_sat: balance.total().to_sat(),
        }
    }

    /// Returns relevant wallet transactions, newest first.
    pub async fn wallet_transactions(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<WalletPage<WalletTransaction>, Error> {
        self.storage.ensure_send_outpoint_quote_id_index().await?;
        let wallet_with_db = self.wallet_with_db.lock().await;
        let wallet = &wallet_with_db.wallet;
        let transactions = wallet.transactions_sort_by(|left, right| {
            right
                .chain_position
                .cmp(&left.chain_position)
                .then_with(|| right.tx_node.txid.cmp(&left.tx_node.txid))
        });
        let total = u64::try_from(transactions.len())
            .map_err(|_| Error::Wallet("Transaction count exceeds u64".to_string()))?;

        let mut items = transactions
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|transaction| {
                let tx = &transaction.tx_node.tx;
                let (sent, received) = wallet.sent_and_received(tx);
                let received_sat = received.to_sat();
                let sent_sat = sent.to_sat();
                let received_signed = i64::try_from(received_sat)
                    .map_err(|_| Error::Wallet("Received value exceeds i64".to_string()))?;
                let sent_signed = i64::try_from(sent_sat)
                    .map_err(|_| Error::Wallet("Sent value exceeds i64".to_string()))?;
                let is_outgoing = sent_sat > 0;
                let inputs = tx
                    .input
                    .iter()
                    .map(|input| {
                        let outpoint = input.previous_output;
                        let previous_output = wallet.tx_graph().get_txout(outpoint);
                        let amount_sat = previous_output.map(|output| output.value.to_sat());
                        let address = previous_output
                            .filter(|output| wallet.is_mine(output.script_pubkey.clone()))
                            .and_then(|output| {
                                Address::from_script(output.script_pubkey.as_script(), self.network)
                                    .ok()
                            })
                            .map(|address| address.to_string());

                        WalletTransactionInput {
                            txid: outpoint.txid.to_string(),
                            vout: outpoint.vout,
                            amount_sat,
                            address,
                        }
                    })
                    .collect();
                let outputs = tx
                    .output
                    .iter()
                    .enumerate()
                    .filter(|(_, output)| {
                        let is_wallet_output = wallet.is_mine(output.script_pubkey.clone());
                        is_outgoing != is_wallet_output
                    })
                    .filter_map(|(vout, output)| {
                        Address::from_script(output.script_pubkey.as_script(), self.network)
                            .ok()
                            .map(|address| (vout, output, address))
                    })
                    .map(|(vout, output, address)| {
                        let vout = u32::try_from(vout).map_err(|_| {
                            Error::Wallet("Transaction output index exceeds u32".to_string())
                        })?;
                        let address = address.to_string();
                        Ok(WalletTransactionOutput {
                            vout,
                            address,
                            amount_sat: output.value.to_sat(),
                            quote_id: None,
                        })
                    })
                    .collect::<Result<Vec<_>, Error>>()?;

                let (confirmation_height, confirmation_time, first_seen) =
                    match transaction.chain_position {
                        ChainPosition::Confirmed { anchor, .. } => (
                            Some(anchor.block_id.height),
                            Some(anchor.confirmation_time),
                            None,
                        ),
                        ChainPosition::Unconfirmed { first_seen, .. } => (None, None, first_seen),
                    };

                Ok(WalletTransaction {
                    txid: transaction.tx_node.txid.to_string(),
                    inputs,
                    outputs,
                    received_sat,
                    sent_sat,
                    fee_sat: wallet.calculate_fee(tx).ok().map(|fee| fee.to_sat()),
                    balance_delta_sat: received_signed - sent_signed,
                    confirmation_height,
                    confirmation_time,
                    first_seen,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        drop(wallet_with_db);

        for transaction in &mut items {
            for output in &mut transaction.outputs {
                output.quote_id = match transaction.sent_sat > 0 {
                    true => {
                        let outpoint = format!("{}:{}", transaction.txid, output.vout);
                        self.storage
                            .get_quote_id_by_send_outpoint(&outpoint)
                            .await?
                    }
                    false => {
                        self.storage
                            .get_quote_id_by_receive_address(&output.address)
                            .await?
                    }
                };
            }
        }

        Ok(WalletPage { items, total })
    }

    /// Returns revealed external and internal addresses in derivation order.
    pub async fn wallet_addresses(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<WalletPage<WalletAddress>, Error> {
        let wallet_with_db = self.wallet_with_db.lock().await;
        let wallet = &wallet_with_db.wallet;
        let mut balances = HashMap::<(KeychainKind, u32), (u64, u64)>::new();

        for output in wallet.list_unspent() {
            let entry = balances
                .entry((output.keychain, output.derivation_index))
                .or_default();
            entry.0 = entry
                .0
                .checked_add(output.txout.value.to_sat())
                .ok_or_else(|| Error::Wallet("Address balance overflow".to_string()))?;
            if output.chain_position.is_confirmed() {
                entry.1 = entry
                    .1
                    .checked_add(output.txout.value.to_sat())
                    .ok_or_else(|| Error::Wallet("Address balance overflow".to_string()))?;
            }
        }

        let keychains = [
            (KeychainKind::External, WalletKeychain::External),
            (KeychainKind::Internal, WalletKeychain::Internal),
        ];
        let total = keychains.iter().try_fold(0_u64, |total, (keychain, _)| {
            let keychain_total =
                u64::try_from(wallet.spk_index().revealed_keychain_spks(*keychain).count())
                    .map_err(|_| Error::Wallet("Address count exceeds u64".to_string()))?;
            total
                .checked_add(keychain_total)
                .ok_or_else(|| Error::Wallet("Address count exceeds u64".to_string()))
        })?;

        let items = keychains
            .into_iter()
            .flat_map(|(keychain, wallet_keychain)| {
                wallet.spk_index().revealed_keychain_spks(keychain).map(
                    move |(derivation_index, script)| {
                        (keychain, wallet_keychain, derivation_index, script)
                    },
                )
            })
            .skip(offset)
            .take(limit)
            .map(|(keychain, wallet_keychain, derivation_index, script)| {
                let address = Address::from_script(&script, self.network)
                    .map_err(|err| Error::Wallet(err.to_string()))?;
                let (balance_sat, confirmed_balance_sat) = balances
                    .get(&(keychain, derivation_index))
                    .copied()
                    .unwrap_or_default();

                Ok(WalletAddress {
                    address: address.to_string(),
                    keychain: wallet_keychain,
                    derivation_index,
                    used: wallet.spk_index().is_used(keychain, derivation_index),
                    balance_sat,
                    confirmed_balance_sat,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;

        Ok(WalletPage { items, total })
    }
}
