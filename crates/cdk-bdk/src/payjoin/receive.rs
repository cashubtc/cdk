use std::str::FromStr;

use bdk_wallet::bitcoin::{Address, FeeRate, Transaction};
use futures::{stream, StreamExt};
use payjoin::persist::{InMemoryPersister, SessionPersister};
use payjoin::receive::v2::{self, ReceiveSession, ReceiverBuilder, SessionEvent};
use payjoin::receive::InputPair;

use super::persist::{Persister, SessionRecord};
use super::{pj_error, MAX_CONCURRENT_SESSIONS};
use crate::storage::BDK_NAMESPACE;
use crate::{CdkBdk, Error};

impl CdkBdk {
    async fn fetch_payjoin_keys(
        &self,
        relay: &str,
        directory: &str,
    ) -> Result<payjoin::OhttpKeys, Error> {
        #[cfg(all(test, feature = "payjoin-regtest"))]
        if let Some(cert) = &self.payjoin_test_root {
            return payjoin::io::fetch_ohttp_keys_with_cert(relay, directory, cert)
                .await
                .map_err(pj_error);
        }
        payjoin::io::fetch_ohttp_keys(relay, directory)
            .await
            .map_err(pj_error)
    }

    pub(crate) async fn create_payjoin_receiver(
        &self,
        quote_id: &str,
        address: &str,
    ) -> Result<Option<serde_json::Value>, Error> {
        let Some(config) = &self.payjoin_config else {
            return Ok(None);
        };
        let Some(directory) = &config.directory_url else {
            return Ok(None);
        };
        let keys = self
            .fetch_payjoin_keys(&config.relay_url, directory)
            .await?;
        let address = Address::from_str(address)
            .map_err(pj_error)?
            .require_network(self.network)
            .map_err(pj_error)?;
        let persister =
            Persister::<SessionEvent>::new(self.storage.clone(), format!("receive_{quote_id}"));
        let memory = InMemoryPersister::<SessionEvent>::default();
        let receiver = ReceiverBuilder::new(address.clone(), directory.as_str(), keys)
            .map_err(pj_error)?
            .with_max_fee_rate(FeeRate::ZERO)
            .build()
            .save(&memory)
            .map_err(pj_error)?;
        let events = memory
            .load()
            .map_err(pj_error)?
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        persister
            .insert(&SessionRecord {
                events,
                closed: false,
                inputs: Vec::new(),
                expires_at: crate::util::unix_now() + 86400,
                completed: false,
                address: Some(address.to_string()),
                input_value_sat: 0,
            })
            .await?;
        let params = cdk_common::payjoin::payjoin_v2_from_bip77_endpoint(
            &receiver.pj_uri().extras().endpoint(),
        )
        .map_err(pj_error)?;
        Ok(Some(serde_json::json!({ "payjoin": params })))
    }

    pub(crate) async fn process_payjoin_receivers(&self) -> Result<(), Error> {
        let records = self.storage.payjoin_records().await?;
        stream::iter(records.into_iter().filter(|(_, record)| {
            record.address.is_some() && !record.completed && !record.events.is_empty()
        }))
        .for_each_concurrent(MAX_CONCURRENT_SESSIONS, |(id, _)| async move {
            if let Err(error) = self.process_payjoin_receiver(&id).await {
                tracing::warn!(session_id = %id, %error, "Payjoin receive session will be retried");
            }
        })
        .await;
        Ok(())
    }

    pub(super) async fn process_payjoin_receiver(&self, id: &str) -> Result<(), Error> {
        let persister = Persister::<SessionEvent>::new(self.storage.clone(), id.to_string());
        let (session, history) = match v2::replay_event_log_async(&persister).await {
            Ok(replayed) => replayed,
            Err(error) if error.is_expired() => {
                let fallback = error.expiry_fallback_tx().cloned();
                return self.reconcile_receiver_fallback(&persister, fallback).await;
            }
            Err(error) => return Err(pj_error(error)),
        };
        // Disabling new negotiations must not disable settlement or release
        // signed contributions before a competing transaction confirms.
        if self.payjoin_config.is_none() {
            return self
                .reconcile_receiver_fallback(&persister, history.fallback_tx())
                .await;
        }
        let mut record = persister.record().await?;
        let address = Address::from_str(
            record
                .address
                .as_deref()
                .ok_or_else(|| Error::Payjoin("Missing receive address".to_string()))?,
        )
        .map_err(pj_error)?
        .require_network(self.network)
        .map_err(pj_error)?;
        let expired = crate::util::unix_now() >= record.expires_at;
        let signed = record
            .events
            .iter()
            .any(|e| e.get("FinalizedProposal").is_some());
        let settled = {
            let wallet = self.wallet_with_db.lock().await;
            let fallback_confirmed = history.fallback_tx().is_some_and(|tx| {
                self.txid_has_required_confirmations(
                    &wallet.wallet,
                    &tx.compute_txid().to_string(),
                    "payjoin_receive",
                    id,
                )
            });
            let contribution_spent = !record.inputs.is_empty()
                && wallet.wallet.transactions().any(|tx| {
                    tx.tx_node
                        .tx
                        .input
                        .iter()
                        .any(|i| record.inputs.contains(&i.previous_output))
                        && self.txid_has_required_confirmations(
                            &wallet.wallet,
                            &tx.tx_node.txid.to_string(),
                            "payjoin_receive",
                            id,
                        )
                });
            fallback_confirmed || contribution_spent
        };
        if settled || (!signed && (expired || matches!(session, ReceiveSession::Closed(_)))) {
            record.completed = true;
            persister.write(&record).await?;
            return Ok(());
        }

        match session {
            ReceiveSession::Initialized(receiver) if expired => {
                receiver
                    .cancel()
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::Initialized(receiver) => {
                let (request, context) = receiver
                    .create_poll_request(self.payjoin_relay_url()?)
                    .map_err(pj_error)?;
                let response = self.payjoin_request(request).await?;
                receiver
                    .process_response(&response, context)
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::UncheckedOriginalPayload(receiver) => {
                let tx = receiver.extract_tx_to_check_broadcast_suitability();
                let suitable = self.payjoin_can_broadcast(tx).await?;
                receiver
                    .apply_broadcast_suitability(Some(FeeRate::BROADCAST_MIN), suitable)
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::MaybeInputsOwned(receiver) => {
                let outpoints = receiver
                    .inputs_owned_checklist()
                    .map(|item| *item.value())
                    .collect::<Vec<_>>();
                let scripts = self.payjoin_prevout_scripts(outpoints.clone()).await?;
                let wallet = self.wallet_with_db.lock().await;
                let owned = outpoints
                    .into_iter()
                    .zip(scripts)
                    .filter_map(|(outpoint, script)| {
                        wallet.wallet.is_mine(script).then_some(outpoint)
                    })
                    .collect::<std::collections::HashSet<_>>();
                let transition =
                    receiver.check_inputs_not_owned(&mut |outpoint| Ok(owned.contains(outpoint)));
                drop(wallet);
                transition.save_async(&persister).await.map_err(pj_error)?;
            }
            ReceiveSession::MaybeInputsSeen(receiver) => {
                // Reserve sender outpoints durably before progressing; replay by this
                // session is allowed, reuse by another session is rejected.
                let mut tx = self.storage.kv_store.begin_transaction().await?;
                let mut checklist = Vec::new();
                for item in receiver.inputs_seen_checklist() {
                    let key = item.value().to_string().replace(':', "-");
                    let owner = tx.kv_read(BDK_NAMESPACE, "payjoin_seen", &key).await?;
                    let known = match owner {
                        Some(owner) => owner != id.as_bytes(),
                        None => {
                            !tx.kv_write_if_absent(
                                BDK_NAMESPACE,
                                "payjoin_seen",
                                &key,
                                id.as_bytes(),
                            )
                            .await?
                        }
                    };
                    checklist.push(item.mark(known));
                }
                tx.commit().await?;
                receiver
                    .apply_inputs_seen_checklist(checklist)
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::OutputsUnknown(receiver) => {
                let script = address.script_pubkey();
                receiver
                    .identify_receiver_outputs(&mut |output| Ok(output == script.as_script()))
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::WantsOutputs(receiver) => {
                receiver
                    .commit_outputs()
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::WantsInputs(receiver) => {
                let wallet = self.wallet_with_db.lock().await;
                let reserved = self.storage.payjoin_reserved_inputs().await?;
                let candidates = wallet
                    .wallet
                    .list_unspent()
                    .filter(|u| {
                        !reserved.contains(&u.outpoint) || record.inputs.contains(&u.outpoint)
                    })
                    .filter(|u| record.inputs.is_empty() || record.inputs.contains(&u.outpoint))
                    .filter_map(|u| InputPair::new_p2wpkh(u.txout, u.outpoint).ok())
                    .collect::<Vec<_>>();
                let input = receiver
                    .try_preserving_privacy(candidates)
                    .map_err(pj_error)?;
                let outpoint = input.outpoint();
                let utxo = wallet
                    .wallet
                    .list_unspent()
                    .find(|u| u.outpoint == outpoint)
                    .ok_or(Error::NoSpendableUtxos)?;
                record.inputs = vec![outpoint];
                record.input_value_sat = utxo.txout.value.to_sat();
                // The reservation precedes the PDK transition and any signatures.
                persister.write(&record).await?;
                receiver
                    .contribute_inputs([input])
                    .map_err(pj_error)?
                    .commit_inputs()
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::WantsFeeRange(receiver) => {
                receiver
                    .apply_fee_range(Some(FeeRate::BROADCAST_MIN), Some(FeeRate::ZERO))
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::ProvisionalProposal(receiver) => {
                let wallet = self.wallet_with_db.lock().await;
                let mut psbt = receiver.psbt_to_sign();
                // Only receiver inputs can be finalized at this stage.
                self.sign_payjoin_psbt(&wallet.wallet, &mut psbt)
                    .map_err(pj_error)?;
                receiver
                    .finalize_signed_proposal(&psbt)
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::PayjoinProposal(receiver) => {
                // The contribution record is already durable before exposing signatures.
                let (request, context) = receiver
                    .create_post_request(self.payjoin_relay_url()?)
                    .map_err(pj_error)?;
                let response = self.payjoin_request(request).await?;
                receiver
                    .process_response(&response, context)
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::HasReplyableError(receiver) if !expired => {
                let (request, context) = receiver
                    .create_error_request(self.payjoin_relay_url()?)
                    .map_err(pj_error)?;
                let response = self.payjoin_request(request).await?;
                receiver
                    .process_error_response(&response, context)
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
            }
            ReceiveSession::Monitor(receiver) => {
                let wallet = self.wallet_with_db.lock().await;
                let transition = receiver.check_for_transaction(|txid| {
                    Ok(wallet
                        .wallet
                        .get_tx(txid)
                        .filter(|_| {
                            self.txid_has_required_confirmations(
                                &wallet.wallet,
                                &txid.to_string(),
                                "payjoin_receive",
                                id,
                            )
                        })
                        .map(|tx| tx.tx_node.tx.as_ref().clone()))
                });
                drop(wallet);
                transition.save_async(&persister).await.map_err(pj_error)?;
            }
            ReceiveSession::PendingFallback(receiver) => {
                let tx = receiver.fallback_tx().clone();
                self.broadcast_transaction_internal(tx)
                    .await
                    .map_err(|e| pj_error(e.message))?;
                // Retain reservations until the competing transaction confirms.
            }
            ReceiveSession::Closed(_) | ReceiveSession::HasReplyableError(_) => {
                // Signed proposals remain valid after protocol expiry. Never free
                // their inputs based only on a timeout or directory response.
            }
        }
        // Expiry permits broadcasting the original transaction, but does not
        // prove the receiver inputs are safe to spend again.
        if expired {
            if let Some(tx) = history.fallback_tx() {
                self.broadcast_transaction_internal(tx)
                    .await
                    .map_err(|e| pj_error(e.message))?;
            }
        }
        Ok(())
    }

    async fn reconcile_receiver_fallback(
        &self,
        persister: &Persister<SessionEvent>,
        fallback: Option<Transaction>,
    ) -> Result<(), Error> {
        let mut record = persister.record().await?;
        let signed = record
            .events
            .iter()
            .any(|e| e.get("FinalizedProposal").is_some());
        let wallet = self.wallet_with_db.lock().await;
        let settled = fallback.as_ref().is_some_and(|tx| {
            self.txid_has_required_confirmations(
                &wallet.wallet,
                &tx.compute_txid().to_string(),
                "payjoin_receive",
                &persister.id,
            )
        }) || wallet.wallet.transactions().any(|tx| {
            tx.tx_node
                .tx
                .input
                .iter()
                .any(|i| record.inputs.contains(&i.previous_output))
                && self.txid_has_required_confirmations(
                    &wallet.wallet,
                    &tx.tx_node.txid.to_string(),
                    "payjoin_receive",
                    &persister.id,
                )
        });
        drop(wallet);
        if !signed || settled {
            record.completed = true;
            persister.write(&record).await?;
        }
        if !settled {
            if let Some(tx) = fallback {
                self.broadcast_transaction_internal(tx)
                    .await
                    .map_err(|e| pj_error(e.message))?;
            }
        }
        Ok(())
    }

    async fn payjoin_prevout_scripts(
        &self,
        outpoints: Vec<bdk_wallet::bitcoin::OutPoint>,
    ) -> Result<Vec<bdk_wallet::bitcoin::ScriptBuf>, Error> {
        #[cfg(feature = "bitcoin-rpc")]
        if let crate::ChainSource::BitcoinRpc(config) = &self.chain_source {
            use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, RpcApi};
            let config = config.clone();
            return tokio::task::spawn_blocking(move || {
                let client = Client::new(
                    &format!("http://{}:{}", config.host, config.port),
                    Auth::UserPass(config.user, config.password),
                )
                .map_err(pj_error)?;
                outpoints
                    .into_iter()
                    .map(|outpoint| {
                        let output = client
                            .get_tx_out(&outpoint.txid, outpoint.vout, Some(true))
                            .map_err(pj_error)?
                            .ok_or_else(|| {
                                Error::Payjoin("Sender input is no longer unspent".to_string())
                            })?;
                        output.script_pub_key.script().map_err(pj_error)
                    })
                    .collect()
            })
            .await
            .map_err(pj_error)?;
        }
        let _ = outpoints;
        Err(Error::Payjoin(
            "Receiving requires Bitcoin RPC input validation".to_string(),
        ))
    }

    async fn payjoin_can_broadcast(&self, tx: Transaction) -> Result<bool, Error> {
        #[cfg(feature = "bitcoin-rpc")]
        if let crate::ChainSource::BitcoinRpc(config) = &self.chain_source {
            use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, RpcApi};
            let config = config.clone();
            return tokio::task::spawn_blocking(move || {
                let client = Client::new(
                    &format!("http://{}:{}", config.host, config.port),
                    Auth::UserPass(config.user, config.password),
                )
                .map_err(pj_error)?;
                let result = client.test_mempool_accept(&[&tx]).map_err(pj_error)?;
                Ok(result.first().is_some_and(|r| r.allowed))
            })
            .await
            .map_err(pj_error)?;
        }
        let _ = tx;
        Err(Error::Payjoin(
            "Receiving requires Bitcoin RPC transaction validation".to_string(),
        ))
    }
}
