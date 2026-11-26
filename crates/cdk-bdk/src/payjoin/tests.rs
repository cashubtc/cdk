use std::str::FromStr;
use std::time::Duration;

use bdk_wallet::bitcoin::{
    absolute, transaction, Address, Amount, FeeRate, Network, OutPoint, Psbt, Transaction, TxIn,
    TxOut,
};
use payjoin::persist::{AsyncSessionPersister, InMemoryPersister};
use payjoin::receive::v2::ReceiverBuilder;
use uuid::Uuid;

use super::*;
use crate::send::payment_intent::record::SendIntentState;
use crate::storage::BDK_NAMESPACE;
use crate::types::{PaymentMetadata, PaymentTier};

mod faults;
#[cfg(feature = "payjoin-regtest")]
mod live;

const ADDRESS: &str = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";

async fn backend() -> (CdkBdk, tempfile::TempDir) {
    let (backend, temp) = crate::tests::build_test_instance_with_tempdir(5).await;
    crate::tests::fund_backend_wallet_transactions(&backend, &[100_000]).await;
    let backend = backend
        .with_payjoin(PayjoinConfig {
            relay_url: "https://127.0.0.1:1".to_string(),
            directory_url: None,
        })
        .unwrap();
    (backend, temp)
}

fn ohttp_keys() -> payjoin::OhttpKeys {
    let secp = bdk_wallet::bitcoin::secp256k1::Secp256k1::new();
    let secret = bdk_wallet::bitcoin::secp256k1::SecretKey::from_slice(&[5; 32]).unwrap();
    let public = bdk_wallet::bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &secret);
    let mut bytes = vec![1];
    bytes.extend(public.serialize());
    payjoin::OhttpKeys::try_from(bytes.as_slice()).unwrap()
}

fn destination(expiration: Duration) -> cdk_common::nuts::nut31::PayjoinV2 {
    let address = Address::from_str(ADDRESS)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    let memory = InMemoryPersister::default();
    let receiver = ReceiverBuilder::new(address, "https://directory.example", ohttp_keys())
        .unwrap()
        .with_expiration(expiration)
        .build()
        .save(&memory)
        .unwrap();
    cdk_common::payjoin::payjoin_v2_from_bip77_endpoint(&receiver.pj_uri().extras().endpoint())
        .unwrap()
}

async fn intent(backend: &CdkBdk, params: cdk_common::nuts::nut31::PayjoinV2) -> SendIntentRecord {
    intent_at_address(backend, params, ADDRESS).await
}

async fn intent_at_address(
    backend: &CdkBdk,
    params: cdk_common::nuts::nut31::PayjoinV2,
    address: &str,
) -> SendIntentRecord {
    let record = SendIntentRecord {
        intent_id: Uuid::new_v4(),
        quote_id: Uuid::new_v4().to_string(),
        address: address.to_string(),
        amount_sat: 10_000,
        max_fee_amount_sat: 5_000,
        tier: PaymentTier::Immediate,
        metadata: PaymentMetadata {
            payjoin: Some(params),
            ..Default::default()
        },
        state: SendIntentState::Pending {
            created_at: crate::util::unix_now(),
        },
    };
    backend
        .storage
        .create_send_intent_if_absent(&record)
        .await
        .unwrap();
    record
}

async fn persist_sender(
    backend: &CdkBdk,
    intent: &SendIntentRecord,
) -> (Persister<SessionEvent>, Psbt) {
    let mut wallet = backend.wallet_with_db.lock().await;
    let address = Address::from_str(&intent.address)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    let mut builder = wallet.wallet.build_tx();
    builder
        .add_recipient(address.clone(), Amount::from_sat(intent.amount_sat))
        .fee_rate(FeeRate::BROADCAST_MIN);
    let mut psbt = builder.finish().unwrap();
    wallet.persist().unwrap();
    assert!(backend
        .sign_payjoin_psbt(&wallet.wallet, &mut psbt)
        .unwrap());
    let endpoint = cdk_common::payjoin::payjoin_v2_to_bip77_endpoint(
        intent.metadata.payjoin.as_ref().unwrap(),
    )
    .unwrap();
    let payjoin::uri::PjParam::V2(param) = payjoin::uri::PjParam::parse(endpoint).unwrap() else {
        panic!("v2")
    };
    let memory = InMemoryPersister::default();
    SenderBuilder::from_parts(
        psbt.clone(),
        &param,
        &address,
        Some(Amount::from_sat(intent.amount_sat)),
    )
    .always_disable_output_substitution()
    .build_with_additional_fee(Amount::from_sat(5_000), None, FeeRate::BROADCAST_MIN, true)
    .unwrap()
    .save(&memory)
    .unwrap();
    let events = memory
        .load()
        .unwrap()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    let persister = Persister::new(backend.storage.clone(), intent.intent_id.to_string());
    persister
        .insert(&SessionRecord {
            events,
            closed: false,
            inputs: psbt
                .unsigned_tx
                .input
                .iter()
                .map(|i| i.previous_output)
                .collect(),
            expires_at: intent.metadata.payjoin.as_ref().unwrap().expires_at,
            completed: false,
            address: None,
            input_value_sat: 0,
        })
        .await
        .unwrap();
    (persister, psbt)
}

#[tokio::test]
async fn network_failure_replays_same_session_and_keeps_inputs_reserved() {
    let (backend, _temp) = backend().await;
    let intent = intent(&backend, destination(Duration::from_secs(600))).await;
    assert!(backend.process_payjoin_send(&intent).await.is_err());
    let persister =
        Persister::<SessionEvent>::new(backend.storage.clone(), intent.intent_id.to_string());
    let before = persister.record().await.unwrap();
    assert!(!before.events.is_empty());
    assert!(!backend
        .storage
        .payjoin_reserved_inputs()
        .await
        .unwrap()
        .is_empty());
    assert!(backend.process_payjoin_send(&intent).await.is_err());
    assert_eq!(before.events, persister.record().await.unwrap().events);
    assert!(matches!(
        backend
            .storage
            .get_send_intent(&intent.intent_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SendIntentState::Pending { .. }
    ));
    let (state, _) = v2::replay_event_log_async(&persister).await.unwrap();
    let SendSession::WithReplyKey(sender) = state else {
        panic!("expected sender")
    };
    let (first, _) = sender
        .create_v2_post_request("https://relay.example")
        .unwrap();
    let (second, _) = sender
        .create_v2_post_request("https://relay.example")
        .unwrap();
    assert_ne!(
        first.body, second.body,
        "OHTTP ciphertext must be fresh for retries"
    );
    assert!(
        backend
            .estimate_onchain_fee_reserve(ADDRESS, 1_000, PaymentTier::Immediate)
            .await
            .is_err(),
        "reserved input cannot fund another quote"
    );
}

#[tokio::test]
async fn expired_session_recovers_exact_fallback_without_refunding() {
    let (backend, _temp) = backend().await;
    let intent = intent(&backend, destination(Duration::ZERO)).await;
    let (persister, psbt) = persist_sender(&backend, &intent).await;
    assert!(v2::replay_event_log_async(&persister)
        .await
        .unwrap_err()
        .is_expired());
    backend.process_payjoin_send(&intent).await.unwrap();
    let batch = backend
        .storage
        .get_send_batch(&intent.intent_id)
        .await
        .unwrap()
        .unwrap();
    let bytes = match batch.state {
        SendBatchState::Signed { tx_bytes, .. } | SendBatchState::Broadcast { tx_bytes, .. } => {
            tx_bytes
        }
        _ => panic!("fallback must be durable"),
    };
    assert_eq!(
        bytes,
        bdk_wallet::bitcoin::consensus::serialize(&psbt.extract_tx().unwrap())
    );
    let after = backend
        .storage
        .get_send_intent(&intent.intent_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        after.state,
        SendIntentState::AwaitingConfirmation { .. }
    ));
    backend.recover_send_saga().await.unwrap();
    assert!(matches!(
        backend
            .storage
            .get_send_intent(&intent.intent_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SendIntentState::AwaitingConfirmation { .. }
    ));
}

#[tokio::test]
async fn persisted_success_recovers_after_close_marker_was_not_written() {
    let (backend, _temp) = backend().await;
    let intent = intent(&backend, destination(Duration::from_secs(600))).await;
    let (persister, psbt) = persist_sender(&backend, &intent).await;
    persister
        .save_event(SessionEvent::PostedOriginalPsbt())
        .await
        .unwrap();
    persister
        .save_event(SessionEvent::Closed(SessionOutcome::Success(psbt)))
        .await
        .unwrap();
    assert!(!persister.record().await.unwrap().closed);
    backend.process_payjoin_send(&intent).await.unwrap();
    assert!(backend
        .storage
        .get_send_batch(&intent.intent_id)
        .await
        .unwrap()
        .is_some());
    assert!(matches!(
        backend
            .storage
            .get_send_intent(&intent.intent_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SendIntentState::AwaitingConfirmation { .. }
    ));
}

#[tokio::test]
async fn invalid_or_unfunded_unstarted_payment_can_fail_without_reservations() {
    let (backend, _temp) = backend().await;
    let intent = intent(&backend, destination(Duration::ZERO)).await;
    let error = backend.process_payjoin_send(&intent).await.unwrap_err();
    backend
        .fail_unstarted_payjoin(&intent, &error)
        .await
        .unwrap();
    assert!(matches!(
        backend
            .storage
            .get_send_intent(&intent.intent_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SendIntentState::Failed { .. }
    ));
    assert!(backend
        .storage
        .payjoin_reserved_inputs()
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn corrupted_session_fails_closed_in_coin_selection() {
    let (backend, _temp) = backend().await;
    let mut tx = backend.storage.kv_store.begin_transaction().await.unwrap();
    tx.kv_write(BDK_NAMESPACE, persist::NAMESPACE, "broken", b"invalid")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(backend.storage.payjoin_reserved_inputs().await.is_err());
}

#[test]
fn receiving_credits_only_net_value_even_with_duplicate_outputs_and_changed_txid() {
    let input = OutPoint::null();
    let record = SessionRecord {
        events: Vec::new(),
        closed: true,
        completed: true,
        inputs: vec![input],
        input_value_sat: 20_000,
        expires_at: 0,
        address: Some(ADDRESS.to_string()),
    };
    let records = vec![("receive_test".to_string(), record)];
    let mut tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: input,
            ..Default::default()
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(15_000),
                script_pubkey: Default::default(),
            },
            TxOut {
                value: Amount::from_sat(14_900),
                script_pubkey: Default::default(),
            },
        ],
    };
    assert_eq!(net_received_amount(&tx, 0, ADDRESS, &records), 0);
    assert_eq!(net_received_amount(&tx, 1, ADDRESS, &records), 9_900);
    tx.input[0].script_sig = bdk_wallet::bitcoin::ScriptBuf::from_bytes(vec![1, 1]);
    assert_eq!(net_received_amount(&tx, 1, ADDRESS, &records), 9_900);
    tx.input.clear();
    assert_eq!(
        net_received_amount(&tx, 0, ADDRESS, &records),
        15_000,
        "fallback does not spend our contribution"
    );
    assert_eq!(net_received_amount(&tx, 1, ADDRESS, &records), 14_900);
}

#[tokio::test]
async fn reopening_both_databases_restores_the_original_transaction_and_reservation() {
    assert_reopened_sender_recovers(false).await;
}

#[tokio::test]
async fn reopening_without_payjoin_configuration_recovers_the_original_transaction() {
    assert_reopened_sender_recovers(true).await;
}

async fn assert_reopened_sender_recovers(disabled: bool) {
    use std::sync::Arc;

    use bdk_wallet::keys::bip39::Mnemonic;

    use crate::{BdkStorage, ChainSource, EsploraConfig};
    let (mut backend, temp) = backend().await;
    let path = temp.path().join("sessions.sqlite");
    backend.storage = BdkStorage::new(Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(path.to_str().unwrap())
            .await
            .unwrap(),
    ));
    let intent = intent(&backend, destination(Duration::from_secs(600))).await;
    let (persister, original) = persist_sender(&backend, &intent).await;
    let expected = original.extract_tx().unwrap();
    let fee_reserve = backend.fee_reserve.clone();
    drop(persister);
    drop(backend);
    let storage = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(path.to_str().unwrap())
            .await
            .unwrap(),
    );
    let mut reopened = CdkBdk::new(
        Mnemonic::from_str("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap(),
        Network::Regtest, ChainSource::Esplora(EsploraConfig { url: "http://127.0.0.1:1".to_string(), parallel_requests: 1 }),
        temp.path().to_str().unwrap().to_string(), fee_reserve, storage, None, 1, 0, 1, 60, None, None,
    ).unwrap().with_payjoin(PayjoinConfig { relay_url: "https://127.0.0.1:1".to_string(), directory_url: None }).unwrap();
    if disabled {
        reopened.payjoin_config = None;
    }
    let persister =
        Persister::<SessionEvent>::new(reopened.storage.clone(), intent.intent_id.to_string());
    let (_, history) = v2::replay_event_log_async(&persister).await.unwrap();
    assert_eq!(history.fallback_tx(), expected);
    assert_eq!(
        reopened.storage.payjoin_reserved_inputs().await.unwrap(),
        expected
            .input
            .iter()
            .map(|i| i.previous_output)
            .collect::<Vec<_>>()
    );
    persister
        .save_event(SessionEvent::Closed(SessionOutcome::Aborted))
        .await
        .unwrap();
    reopened.process_payjoin_send(&intent).await.unwrap();
    assert!(matches!(
        reopened
            .storage
            .get_send_intent(&intent.intent_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SendIntentState::AwaitingConfirmation { .. }
    ));
}

#[derive(Clone)]
struct FailingWrites(
    std::sync::Arc<
        dyn cdk_common::database::KVStore<Err = cdk_common::database::Error> + Send + Sync,
    >,
);

#[async_trait::async_trait]
impl cdk_common::database::KVStoreDatabase for FailingWrites {
    type Err = cdk_common::database::Error;
    async fn kv_read(
        &self,
        primary: &str,
        secondary: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Self::Err> {
        self.0.kv_read(primary, secondary, key).await
    }
    async fn kv_list(&self, primary: &str, secondary: &str) -> Result<Vec<String>, Self::Err> {
        self.0.kv_list(primary, secondary).await
    }
}

#[async_trait::async_trait]
impl cdk_common::database::KVStore for FailingWrites {
    async fn begin_transaction(
        &self,
    ) -> Result<
        Box<dyn cdk_common::database::KVStoreTransaction<Self::Err> + Send + Sync>,
        cdk_common::database::Error,
    > {
        Err(cdk_common::database::Error::Locked)
    }
}

#[tokio::test]
async fn persistence_failure_does_not_expose_a_session_or_emit_a_refund_event() {
    let (mut backend, _temp) = backend().await;
    let intent = intent(&backend, destination(Duration::from_secs(600))).await;
    let storage = backend.storage.clone();
    backend.storage =
        crate::BdkStorage::new(std::sync::Arc::new(FailingWrites(storage.kv_store.clone())));
    let mut events = backend.payment_sender.subscribe();
    let error = backend.process_payjoin_send(&intent).await.unwrap_err();
    backend
        .fail_unstarted_payjoin(&intent, &error)
        .await
        .unwrap();
    assert!(events.try_recv().is_err());
    assert!(storage.payjoin_records().await.unwrap().is_empty());
    assert!(matches!(
        storage
            .get_send_intent(&intent.intent_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SendIntentState::Pending { .. }
    ));
    backend.storage = storage;
    assert!(backend.process_payjoin_send(&intent).await.is_err());
    assert_eq!(backend.storage.payjoin_records().await.unwrap().len(), 1);
}

#[test]
fn metadata_keeps_nested_payjoin_and_legacy_entries() {
    let params = destination(Duration::from_secs(600));
    let json = serde_json::json!({"payjoin": params}).to_string();
    assert_eq!(
        PaymentMetadata::from_optional_json(Some(&json)).payjoin,
        Some(params)
    );
    assert_eq!(
        PaymentMetadata::from_optional_json(Some(r#"{"label":"test"}"#))
            .entries
            .get("label"),
        Some(&"test".to_string())
    );
    assert_eq!(
        PaymentMetadata::from_optional_json(Some(r#"{"entries":{"label":"test"}}"#))
            .entries
            .get("label"),
        Some(&"test".to_string())
    );
}

#[tokio::test]
async fn receiver_signing_and_fallback_confirmation_recover_both_wallets() {
    assert_confirmation_recovers_both_wallets(true, false).await;
}

#[tokio::test]
async fn receiver_signing_and_payjoin_confirmation_recover_both_wallets() {
    assert_confirmation_recovers_both_wallets(false, false).await;
}

#[tokio::test]
async fn disabled_payjoin_retains_signed_inputs_until_fallback_confirmation() {
    assert_confirmation_recovers_both_wallets(true, true).await;
}

#[tokio::test]
async fn disabled_payjoin_recovers_a_saved_success_and_receiver_confirmation() {
    assert_confirmation_recovers_both_wallets(false, true).await;
}

async fn assert_confirmation_recovers_both_wallets(confirm_fallback: bool, disabled: bool) {
    use std::sync::Arc;

    use bdk_wallet::keys::bip39::Mnemonic;
    use payjoin::receive::v2::{self as receive, ReceiveSession, SessionEvent as ReceiveEvent};

    use crate::{ChainSource, EsploraConfig};
    let (mut sender, _sender_temp) = backend().await;
    let receiver_temp = tempfile::tempdir().unwrap();
    let mut receiver = CdkBdk::new(
        Mnemonic::from_str(
            "legal winner thank year wave sausage worth useful legal winner thank yellow",
        )
        .unwrap(),
        Network::Regtest,
        ChainSource::Esplora(EsploraConfig {
            url: "http://127.0.0.1:1".to_string(),
            parallel_requests: 1,
        }),
        receiver_temp.path().to_str().unwrap().to_string(),
        sender.fee_reserve.clone(),
        Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap()),
        None,
        1,
        0,
        1,
        60,
        None,
        None,
    )
    .unwrap()
    .with_payjoin(PayjoinConfig {
        relay_url: "https://127.0.0.1:1".to_string(),
        directory_url: None,
    })
    .unwrap();
    crate::tests::fund_backend_wallet_transactions(&receiver, &[50_000]).await;
    let address = {
        let mut wallet = receiver.wallet_with_db.lock().await;
        let address = wallet
            .wallet
            .reveal_next_address(bdk_wallet::KeychainKind::External)
            .address;
        wallet.persist().unwrap();
        address
    };
    let memory = InMemoryPersister::default();
    let initial = ReceiverBuilder::new(address.clone(), "https://directory.example", ohttp_keys())
        .unwrap()
        .with_expiration(Duration::from_secs(600))
        .build()
        .save(&memory)
        .unwrap();
    let params =
        cdk_common::payjoin::payjoin_v2_from_bip77_endpoint(&initial.pj_uri().extras().endpoint())
            .unwrap();
    let persister =
        Persister::<ReceiveEvent>::new(receiver.storage.clone(), "receive_test".to_string());
    persister
        .insert(&SessionRecord {
            events: memory
                .load()
                .unwrap()
                .map(|e| serde_json::to_value(e).unwrap())
                .collect(),
            closed: false,
            inputs: Vec::new(),
            expires_at: params.expires_at,
            completed: false,
            address: Some(address.to_string()),
            input_value_sat: 0,
        })
        .await
        .unwrap();
    let intent = intent_at_address(&sender, params, &address.to_string()).await;
    let receive_quote_id = Uuid::new_v4().to_string();
    assert!(receiver
        .storage
        .track_receive_address(&address.to_string(), &receive_quote_id)
        .await
        .unwrap());
    let (sender_persister, original) = persist_sender(&sender, &intent).await;
    let change_index = original
        .unsigned_tx
        .output
        .iter()
        .position(|o| o.script_pubkey != address.script_pubkey())
        .unwrap();
    // Start after transport and Bitcoin Core validation. The local state machine
    // below exercises the real PDK contribution, fee checks and signing paths.
    let payload: payjoin::receive::OriginalPayload = serde_json::from_value(serde_json::json!({
        "psbt": original,
        "params": {"v": 2, "output_substitution": "Disabled", "additional_fee_contribution": [5000, change_index], "min_fee_rate": 250}
    })).unwrap();
    persister
        .save_event(ReceiveEvent::RetrievedOriginalPayload {
            original: payload,
            reply_key: None,
        })
        .await
        .unwrap();
    persister
        .save_event(ReceiveEvent::CheckedBroadcastSuitability())
        .await
        .unwrap();
    persister
        .save_event(ReceiveEvent::CheckedInputsNotOwned())
        .await
        .unwrap();
    persister
        .save_event(ReceiveEvent::CheckedNoInputsSeenBefore())
        .await
        .unwrap();
    // Identify outputs, commit outputs, contribute inputs, calculate fees, sign.
    for _ in 0..5 {
        receiver
            .process_payjoin_receiver(&persister.id)
            .await
            .unwrap();
    }
    let (state, _) = receive::replay_event_log_async(&persister).await.unwrap();
    let ReceiveSession::PayjoinProposal(proposal) = state else {
        panic!("expected signed receiver proposal")
    };
    let record = persister.record().await.unwrap();
    assert_eq!(record.inputs.len(), 1);
    assert_eq!(record.input_value_sat, 50_000);
    let mut psbt = proposal.psbt().clone();
    let wallet = sender.wallet_with_db.lock().await;
    assert!(sender.sign_payjoin_psbt(&wallet.wallet, &mut psbt).unwrap());
    drop(wallet);
    let tx = psbt.extract_tx().unwrap();
    let vout = tx
        .output
        .iter()
        .position(|o| o.script_pubkey == address.script_pubkey())
        .unwrap() as u32;
    assert_eq!(
        net_received_amount(
            &tx,
            vout,
            &address.to_string(),
            &receiver.storage.payjoin_records().await.unwrap()
        ),
        10_000
    );
    assert_eq!(
        receiver.storage.payjoin_reserved_inputs().await.unwrap(),
        record.inputs
    );

    // A receiver may have sent its signatures even if it never saved the POST
    // response. Expiry cannot release those inputs.
    if disabled {
        sender.payjoin_config = None;
        receiver.payjoin_config = None;
    } else {
        let mut expired = record;
        expired.events[0]["Created"]["expiration"] = serde_json::json!(500_000_001);
        expired.expires_at = 500_000_001;
        persister.write(&expired).await.unwrap();
    }
    assert!(receiver
        .process_payjoin_receiver(&persister.id)
        .await
        .is_err()); // fallback broadcast is offline
    assert!(!persister.record().await.unwrap().completed);
    assert!(!receiver
        .storage
        .payjoin_reserved_inputs()
        .await
        .unwrap()
        .is_empty());

    sender_persister
        .save_event(SessionEvent::PostedOriginalPsbt())
        .await
        .unwrap();
    sender_persister
        .save_event(SessionEvent::Closed(SessionOutcome::Success(
            proposal.psbt().clone(),
        )))
        .await
        .unwrap();
    sender.process_payjoin_send(&intent).await.unwrap();
    let fallback = original.extract_tx().unwrap();
    assert_ne!(fallback.compute_txid(), tx.compute_txid());
    let confirmed_tx = if confirm_fallback { fallback } else { tx };
    let mut block = bdk_wallet::bitcoin::blockdata::constants::genesis_block(Network::Regtest);
    block.header.prev_blockhash = block.block_hash();
    block.header.time += 1;
    block.txdata = vec![confirmed_tx.clone()];
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    for backend in [&sender, &receiver] {
        let mut wallet = backend.wallet_with_db.lock().await;
        wallet.wallet.apply_block(&block, 1).unwrap();
        wallet.persist().unwrap();
    }
    sender.check_send_saga_confirmations().await.unwrap();
    sender.reconcile_payjoin_sends().await.unwrap();
    let receipt = sender
        .storage
        .get_finalized_intent(&intent.intent_id)
        .await
        .unwrap()
        .unwrap();
    assert!(receipt
        .outpoint
        .starts_with(&confirmed_tx.compute_txid().to_string()));
    assert!(sender
        .storage
        .payjoin_reserved_inputs()
        .await
        .unwrap()
        .is_empty());
    receiver
        .process_payjoin_receiver(&persister.id)
        .await
        .unwrap();
    assert!(persister.record().await.unwrap().completed);
    assert!(receiver
        .storage
        .payjoin_reserved_inputs()
        .await
        .unwrap()
        .is_empty());

    // Completed session records must still deduct the receiver's contribution
    // when deposit accounting runs, including repeated scans after recovery.
    for _ in 0..2 {
        receiver.scan_for_new_payments().await.unwrap();
        receiver.check_receive_saga_confirmations().await.unwrap();
        let receipts = receiver
            .storage
            .get_finalized_receive_intents_by_quote_id(&receive_quote_id)
            .await
            .unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].amount_sat, 10_000);
        assert!(receipts[0]
            .outpoint
            .starts_with(&confirmed_tx.compute_txid().to_string()));
    }
}

async fn persist_idle_receiver(
    backend: &CdkBdk,
    id: &str,
) -> Persister<payjoin::receive::v2::SessionEvent> {
    let address = Address::from_str(ADDRESS)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    let memory = InMemoryPersister::default();
    ReceiverBuilder::new(address.clone(), "https://directory.example", ohttp_keys())
        .unwrap()
        .with_expiration(Duration::from_secs(600))
        .build()
        .save(&memory)
        .unwrap();
    let persister = Persister::new(backend.storage.clone(), id.to_string());
    persister
        .insert(&SessionRecord {
            events: memory
                .load()
                .unwrap()
                .map(|e| serde_json::to_value(e).unwrap())
                .collect(),
            closed: false,
            inputs: Vec::new(),
            expires_at: crate::util::unix_now() + 600,
            completed: false,
            address: Some(address.to_string()),
            input_value_sat: 0,
        })
        .await
        .unwrap();
    persister
}

#[tokio::test]
async fn disabled_payjoin_recovers_pending_senders_without_a_relay() {
    for (expired, posted) in [(false, false), (false, true), (true, false)] {
        let (mut backend, _temp) = backend().await;
        let duration = if expired {
            Duration::ZERO
        } else {
            Duration::from_secs(600)
        };
        let intent = intent(&backend, destination(duration)).await;
        let (persister, original) = persist_sender(&backend, &intent).await;
        if posted {
            persister
                .save_event(SessionEvent::PostedOriginalPsbt())
                .await
                .unwrap();
        }
        backend.payjoin_config = None;
        backend.process_payjoin_sessions().await.unwrap();
        let batch = backend
            .storage
            .get_send_batch(&intent.intent_id)
            .await
            .unwrap()
            .unwrap();
        let SendBatchState::Broadcast { tx_bytes, .. } = batch.state else {
            panic!("persisted sender must reach fallback broadcast without a relay")
        };
        assert_eq!(
            tx_bytes,
            bdk_wallet::bitcoin::consensus::serialize(&original.extract_tx().unwrap())
        );
        assert!(matches!(
            backend
                .storage
                .get_send_intent(&intent.intent_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            SendIntentState::AwaitingConfirmation { .. }
        ));
    }
}

#[tokio::test]
async fn disabled_payjoin_completes_unsigned_receivers_and_rejects_new_sends() {
    let (mut backend, _temp) = backend().await;
    let receiver = persist_idle_receiver(&backend, "receive_disabled").await;
    let intent = intent(&backend, destination(Duration::from_secs(600))).await;
    backend.payjoin_config = None;
    backend.process_payjoin_sessions().await.unwrap();
    assert!(receiver.record().await.unwrap().completed);
    assert!(matches!(
        backend
            .storage
            .get_send_intent(&intent.intent_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SendIntentState::Failed { .. }
    ));
    assert!(backend
        .storage
        .get_send_batch(&intent.intent_id)
        .await
        .unwrap()
        .is_none());
    assert!(backend
        .storage
        .payjoin_reserved_inputs()
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn idle_receivers_are_bounded_and_do_not_block_sender_ticks() {
    let (mut backend, _temp) = backend().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    backend.payjoin_config.as_mut().unwrap().relay_url =
        format!("https://{}", listener.local_addr().unwrap());
    backend.batch_config.poll_interval = Duration::from_millis(10);
    for index in 0..MAX_CONCURRENT_SESSIONS + 1 {
        persist_idle_receiver(&backend, &format!("receive_{index}")).await;
    }
    let (accepted, mut connections) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            // Keep sockets open without completing TLS, so requests stay pending.
            if accepted.send(socket).is_err() {
                break;
            }
        }
    });
    let intent = intent(&backend, destination(Duration::ZERO)).await;
    persist_sender(&backend, &intent).await;
    backend
        .storage
        .update_send_intent(
            &intent.intent_id,
            &SendIntentState::Failed {
                reason: "held until receiver polls start".to_string(),
                failed_at: crate::util::unix_now(),
                created_at: crate::util::unix_now(),
            },
        )
        .await
        .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker_backend = backend.clone();
    let worker_cancel = cancel.clone();
    let worker =
        tokio::spawn(async move { worker_backend.run_payjoin_processor(worker_cancel).await });
    let mut sockets = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        for _ in 0..MAX_CONCURRENT_SESSIONS {
            sockets.push(connections.recv().await.unwrap());
        }
        // Queue after receiver polling starts: this requires a subsequent sender
        // tick, rather than merely concurrent execution within the first pass.
        backend
            .storage
            .update_send_intent(&intent.intent_id, &intent.state)
            .await
            .unwrap();
        loop {
            if matches!(
                backend
                    .storage
                    .get_send_intent(&intent.intent_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .state,
                SendIntentState::AwaitingConfirmation { .. }
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            connections.try_recv().is_err(),
            "receiver concurrency exceeded its bound"
        );
    })
    .await
    .expect("idle receiver requests must not block sender progress");
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    server.abort();
    let _ = server.await;
}
