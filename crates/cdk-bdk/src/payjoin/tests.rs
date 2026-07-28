use std::str::FromStr;

use bdk_wallet::bitcoin::absolute::LockTime;
use bdk_wallet::bitcoin::{
    transaction, Amount as BitcoinAmount, Network, Psbt, ScriptBuf, TxOut, Txid,
};
use bdk_wallet::keys::bip39::Mnemonic;
use cdk_common::common::FeeReserve;

use super::*;

async fn build_test_backend() -> (CdkBdk, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mnemonic = Mnemonic::from_str(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .expect("mnemonic");
    let kv = cdk_sqlite::mint::memory::empty()
        .await
        .expect("in-memory kv store");
    let fee_reserve = FeeReserve {
        min_fee_reserve: Amount::new(1, CurrencyUnit::Sat).into(),
        percent_fee_reserve: 0.02,
    };
    let backend = CdkBdk::new(
        mnemonic,
        Network::Regtest,
        crate::ChainSource::Esplora(crate::EsploraConfig {
            url: "http://127.0.0.1:1".to_string(),
            parallel_requests: 1,
        }),
        tmp.path().to_string_lossy().into_owned(),
        fee_reserve,
        Arc::new(kv),
        None,
        1,
        0,
        546,
        60,
        Some(1),
        None,
        None,
    )
    .expect("build CdkBdk test instance");

    (backend, tmp)
}

fn test_psbt_with_outputs(outputs: Vec<TxOut>) -> Psbt {
    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(
                Txid::from_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                    .expect("valid txid"),
                0,
            ),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Default::default(),
        }],
        output: outputs,
    };
    Psbt::from_unsigned_tx(tx).expect("valid test psbt")
}

#[test]
fn amountless_payjoin_receive_session_cap_comes_from_original_receiver_outputs() {
    let events = vec![
        ::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vec![1]),
        ::payjoin::receive::v2::SessionEvent::CommittedOutputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(8_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: BitcoinAmount::from_sat(3_000),
                script_pubkey: ScriptBuf::new(),
            },
        ]),
    ];
    let mut record = crate::storage::PayjoinReceiveSessionRecord {
        quote_id: "quote-1".to_string(),
        fallback_address: "bcrt1qfallback".to_string(),
        amount_sat: 0,
        proposal_receiver_outpoints: Vec::new(),
        proposal_tx_bytes: None,
        cut_through: None,
        expires_at: 1_700_000_000,
        events,
        closed: false,
    };

    update_payjoin_receive_credit_cap(&mut record);

    assert_eq!(record.amount_sat, 3_000);
}

#[test]
fn payjoin_receive_session_records_proposal_receiver_outpoints() {
    let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
    let other_script = ScriptBuf::from_bytes(vec![0x6a]);
    let psbt = test_psbt_with_outputs(vec![
        TxOut {
            value: BitcoinAmount::from_sat(8_000),
            script_pubkey: other_script,
        },
        TxOut {
            value: BitcoinAmount::from_sat(3_000),
            script_pubkey: fallback_script.clone(),
        },
    ]);
    let expected_outpoint = OutPoint::new(psbt.unsigned_tx.compute_txid(), 1).to_string();
    let mut record = crate::storage::PayjoinReceiveSessionRecord {
        quote_id: "quote-1".to_string(),
        fallback_address: "bcrt1qfallback".to_string(),
        amount_sat: 3_000,
        proposal_receiver_outpoints: Vec::new(),
        proposal_tx_bytes: None,
        cut_through: None,
        expires_at: 1_700_000_000,
        events: Vec::new(),
        closed: false,
    };

    update_payjoin_receive_proposal_receiver_outpoints(&mut record, &psbt, &fallback_script);

    assert_eq!(record.proposal_receiver_outpoints, vec![expected_outpoint]);
}

#[test]
fn payjoin_receiver_credit_sums_final_receiver_outputs() {
    let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
    let other_script = ScriptBuf::from_bytes(vec![0x6a]);
    let psbt = test_psbt_with_outputs(vec![
        TxOut {
            value: BitcoinAmount::from_sat(2_000),
            script_pubkey: fallback_script.clone(),
        },
        TxOut {
            value: BitcoinAmount::from_sat(9_000),
            script_pubkey: other_script,
        },
        TxOut {
            value: BitcoinAmount::from_sat(3_000),
            script_pubkey: fallback_script.clone(),
        },
    ]);

    assert_eq!(
        payjoin_receiver_output_amount(&psbt, &fallback_script).expect("sum outputs"),
        5_000
    );
}

#[test]
fn payjoin_receiver_credit_accepts_unreduced_receiver_output() {
    let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
    let psbt = test_psbt_with_outputs(vec![TxOut {
        value: BitcoinAmount::from_sat(5_000),
        script_pubkey: fallback_script.clone(),
    }]);

    ensure_payjoin_receiver_credit(&psbt, &fallback_script, 5_000)
        .expect("sender-funded payjoin keeps receiver output whole");
}

#[test]
fn payjoin_receiver_credit_rejects_reduced_receiver_output() {
    let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
    let psbt = test_psbt_with_outputs(vec![TxOut {
        value: BitcoinAmount::from_sat(4_999),
        script_pubkey: fallback_script.clone(),
    }]);

    let err = ensure_payjoin_receiver_credit(&psbt, &fallback_script, 5_000)
        .expect_err("receiver output below original amount must be rejected");

    assert!(err.to_string().contains("below original amount"));
}

#[test]
fn payjoin_send_payment_output_accepts_exact_output() {
    let payment_script = ScriptBuf::from_bytes(vec![0x51]);
    let other_script = ScriptBuf::from_bytes(vec![0x6a]);
    let psbt = test_psbt_with_outputs(vec![
        TxOut {
            value: BitcoinAmount::from_sat(9_000),
            script_pubkey: other_script,
        },
        TxOut {
            value: BitcoinAmount::from_sat(10_000),
            script_pubkey: payment_script.clone(),
        },
    ]);

    let outpoint = require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
        .expect("payment output is present");

    assert_eq!(outpoint.vout, 1);
}

#[test]
fn payjoin_send_payment_output_accepts_larger_output() {
    let payment_script = ScriptBuf::from_bytes(vec![0x51]);
    let psbt = test_psbt_with_outputs(vec![TxOut {
        value: BitcoinAmount::from_sat(12_000),
        script_pubkey: payment_script.clone(),
    }]);

    let outpoint = require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
        .expect("larger payment output is present");

    assert_eq!(outpoint.vout, 0);
}

#[test]
fn payjoin_send_payment_output_rejects_smaller_single_output() {
    let payment_script = ScriptBuf::from_bytes(vec![0x51]);
    let other_script = ScriptBuf::from_bytes(vec![0x6a]);
    let psbt = test_psbt_with_outputs(vec![
        TxOut {
            value: BitcoinAmount::from_sat(9_999),
            script_pubkey: payment_script.clone(),
        },
        TxOut {
            value: BitcoinAmount::from_sat(10_000),
            script_pubkey: other_script,
        },
    ]);

    let err = require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
        .expect_err("altered payment output must be rejected");

    assert!(err.to_string().contains("missing payment output"));
}

#[test]
fn payjoin_send_payment_output_rejects_split_only_outputs() {
    let payment_script = ScriptBuf::from_bytes(vec![0x51]);
    let psbt = test_psbt_with_outputs(vec![
        TxOut {
            value: BitcoinAmount::from_sat(6_000),
            script_pubkey: payment_script.clone(),
        },
        TxOut {
            value: BitcoinAmount::from_sat(4_000),
            script_pubkey: payment_script.clone(),
        },
    ]);

    let err = require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
        .expect_err("split-only receiver outputs are unsupported");

    assert!(err.to_string().contains("missing payment output"));
}

#[test]
fn payjoin_send_validation_accepts_net_spend_within_cap() {
    let payment_script = ScriptBuf::from_bytes(vec![0x51]);
    let psbt = test_psbt_with_outputs(vec![TxOut {
        value: BitcoinAmount::from_sat(10_000),
        script_pubkey: payment_script.clone(),
    }]);

    let validation = validate_payjoin_send_transaction(
        &psbt.unsigned_tx,
        &payment_script,
        10_000,
        1_000,
        12_000,
        1_000,
    )
    .expect("net spend at cap is accepted");

    assert_eq!(validation.fee_contribution_sat, 1_000);
}

#[test]
fn payjoin_send_validation_accepts_larger_receiver_output_with_local_fee_cap() {
    let payment_script = ScriptBuf::from_bytes(vec![0x51]);
    let psbt = test_psbt_with_outputs(vec![TxOut {
        value: BitcoinAmount::from_sat(12_000),
        script_pubkey: payment_script.clone(),
    }]);

    let validation = validate_payjoin_send_transaction(
        &psbt.unsigned_tx,
        &payment_script,
        10_000,
        1_000,
        20_000,
        9_500,
    )
    .expect("receiver-funded larger output is accepted when mint spend is capped");

    assert_eq!(validation.fee_contribution_sat, 500);
}

#[test]
fn payjoin_send_validation_rejects_net_spend_above_cap() {
    let payment_script = ScriptBuf::from_bytes(vec![0x51]);
    let psbt = test_psbt_with_outputs(vec![TxOut {
        value: BitcoinAmount::from_sat(10_000),
        script_pubkey: payment_script.clone(),
    }]);

    let err = validate_payjoin_send_transaction(
        &psbt.unsigned_tx,
        &payment_script,
        10_000,
        1_000,
        12_001,
        1_000,
    )
    .expect_err("net spend above amount plus max fee is rejected");

    assert!(err.to_string().contains("exceeding cap"));
}

#[test]
fn payjoin_send_validation_rejects_net_spend_below_payment_amount() {
    let payment_script = ScriptBuf::from_bytes(vec![0x51]);
    let psbt = test_psbt_with_outputs(vec![TxOut {
        value: BitcoinAmount::from_sat(10_000),
        script_pubkey: payment_script.clone(),
    }]);

    let err = validate_payjoin_send_transaction(
        &psbt.unsigned_tx,
        &payment_script,
        10_000,
        1_000,
        9_999,
        0,
    )
    .expect_err("mint net spend below quote cannot produce fee contribution");

    assert!(err.to_string().contains("below payment amount"));
}

#[test]
fn payjoin_original_receiver_output_amount_sums_all_receiver_outputs() {
    let events = vec![
        ::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vec![0, 2]),
        ::payjoin::receive::v2::SessionEvent::CommittedOutputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(21_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: BitcoinAmount::from_sat(99_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: BitcoinAmount::from_sat(34_000),
                script_pubkey: ScriptBuf::new(),
            },
        ]),
    ];

    assert_eq!(
        payjoin_original_receiver_output_amount_from_events(&events),
        Some(55_000)
    );
}

#[test]
fn ordinary_payjoin_rejects_multiple_receiver_outputs() {
    let events = vec![::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vec![0, 2])];

    let error = ensure_ordinary_payjoin_has_single_receiver_output(&events)
        .expect_err("multiple outputs must not use per-outpoint receive accounting");

    assert!(error.to_string().contains("exactly one receiver output"));
}

#[test]
fn payjoin_receive_amount_missing_events_returns_none() {
    let events = vec![::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vec![0])];

    assert_eq!(
        payjoin_original_receiver_output_amount_from_events(&events),
        None
    );
}

#[test]
fn payjoin_original_input_outpoints_come_from_retrieved_payload_event() {
    let first_outpoint = OutPoint::new(
        Txid::from_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .expect("valid txid"),
        0,
    );
    let second_outpoint = OutPoint::new(
        Txid::from_str("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .expect("valid txid"),
        1,
    );
    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: first_outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Default::default(),
            },
            TxIn {
                previous_output: second_outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Default::default(),
            },
        ],
        output: vec![TxOut {
            value: BitcoinAmount::from_sat(1_000),
            script_pubkey: ScriptBuf::new(),
        }],
    };
    let psbt = Psbt::from_unsigned_tx(tx).expect("valid unsigned psbt");
    let event = serde_json::json!({
        "RetrievedOriginalPayload": {
            "original": {
                "psbt": psbt,
                "params": {
                    "v": 2,
                    "output_substitution": "Enabled",
                    "additional_fee_contribution": null,
                    "min_fee_rate": 250
                }
            },
            "reply_key": null
        }
    });
    let event = serde_json::from_value(event).expect("deserialize Payjoin session event");

    assert_eq!(
        payjoin_original_input_outpoints_from_events(&[event])
            .expect("extract original input outpoints"),
        vec![first_outpoint, second_outpoint]
    );
}

#[test]
fn payjoin_receive_session_expires_at_deadline() {
    let record = crate::storage::PayjoinReceiveSessionRecord {
        quote_id: "quote-1".to_string(),
        fallback_address: "bcrt1qfallback".to_string(),
        amount_sat: 1_000,
        proposal_receiver_outpoints: Vec::new(),
        proposal_tx_bytes: None,
        cut_through: None,
        expires_at: 100,
        events: Vec::new(),
        closed: false,
    };

    assert!(!record.is_expired(99));
    assert!(record.is_expired(100));
}

#[test]
fn payjoin_receive_session_prunes_at_retention_deadline() {
    let record = crate::storage::PayjoinReceiveSessionRecord {
        quote_id: "quote-1".to_string(),
        fallback_address: "bcrt1qfallback".to_string(),
        amount_sat: 1_000,
        proposal_receiver_outpoints: Vec::new(),
        proposal_tx_bytes: None,
        cut_through: None,
        expires_at: 100,
        events: Vec::new(),
        closed: true,
    };
    let retention_edge = 100 + PAYJOIN_RECEIVE_SESSION_RETENTION_SECS;

    assert!(!record.should_prune(retention_edge - 1, PAYJOIN_RECEIVE_SESSION_RETENTION_SECS));
    assert!(record.should_prune(retention_edge, PAYJOIN_RECEIVE_SESSION_RETENTION_SECS));
}

#[tokio::test]
async fn payjoin_receive_credit_cap_outlives_unresolved_proposal() {
    use bdk_wallet::bitcoin::hashes::Hash;
    use bdk_wallet::bitcoin::{Address, WPubkeyHash};

    use crate::receive::receive_intent::record::{ReceiveIntentRecord, ReceiveIntentState};

    let db = cdk_sqlite::mint::memory::empty()
        .await
        .expect("in-memory db");
    let storage = crate::storage::BdkStorage::new(Arc::new(db));

    let fallback_script = ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([7u8; 20]));
    let fallback_address = Address::from_script(&fallback_script, Network::Regtest)
        .expect("valid script")
        .to_string();

    let original_tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(
                Txid::from_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                    .expect("valid txid"),
                0,
            ),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Default::default(),
        }],
        output: vec![TxOut {
            value: BitcoinAmount::from_sat(1_000),
            script_pubkey: fallback_script,
        }],
    };
    let original_txid = original_tx.compute_txid();
    let mut psbt = Psbt::from_unsigned_tx(original_tx).expect("valid unsigned psbt");
    // check_broadcast_suitability computes the psbt fee rate, which needs
    // input UTXO data.
    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: BitcoinAmount::from_sat(2_000),
        script_pubkey: ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([9u8; 20])),
    });
    let event = serde_json::json!({
        "RetrievedOriginalPayload": {
            "original": {
                "psbt": psbt,
                "params": {
                    "v": 2,
                    "output_substitution": "Enabled",
                    "additional_fee_contribution": null,
                    "min_fee_rate": 250
                }
            },
            "reply_key": null
        }
    });
    let events = vec![serde_json::from_value(event).expect("deserialize session event")];

    let proposal_outpoint =
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc:0".to_string();
    let record = crate::storage::PayjoinReceiveSessionRecord {
        quote_id: "quote-1".to_string(),
        fallback_address,
        amount_sat: 1_000,
        proposal_receiver_outpoints: vec![proposal_outpoint.clone()],
        proposal_tx_bytes: None,
        cut_through: None,
        expires_at: 100,
        events,
        closed: true,
    };

    // A session that never signed a proposal has no cap worth keeping.
    let mut no_proposal = record.clone();
    no_proposal.proposal_receiver_outpoints.clear();
    assert!(
        payjoin_receive_credit_cap_resolved(&storage, Network::Regtest, &no_proposal)
            .await
            .expect("resolve")
    );

    // The signed proposal is still broadcastable: keep the cap.
    assert!(
        !payjoin_receive_credit_cap_resolved(&storage, Network::Regtest, &record)
            .await
            .expect("resolve")
    );

    // Once a proposal receiver outpoint was detected the cap was applied.
    let detect = |outpoint: String, txid: String| ReceiveIntentRecord {
        intent_id: Uuid::new_v4(),
        quote_id: "quote-1".to_string(),
        state: ReceiveIntentState::Detected {
            address: record.fallback_address.clone(),
            txid,
            outpoint,
            amount_sat: 1_000,
            block_height: 1,
            created_at: 0,
        },
    };
    let proposal_txid = proposal_outpoint
        .split_once(':')
        .expect("outpoint format")
        .0
        .to_string();
    storage
        .create_receive_intent_if_absent(&detect(proposal_outpoint.clone(), proposal_txid))
        .await
        .expect("create intent");
    assert!(
        payjoin_receive_credit_cap_resolved(&storage, Network::Regtest, &record)
            .await
            .expect("resolve")
    );

    // A settled original also resolves the cap: the proposal conflicts
    // with it on the sender inputs and can never confirm.
    let db = cdk_sqlite::mint::memory::empty()
        .await
        .expect("in-memory db");
    let storage = crate::storage::BdkStorage::new(Arc::new(db));
    storage
        .create_receive_intent_if_absent(&detect(
            OutPoint::new(original_txid, 0).to_string(),
            original_txid.to_string(),
        ))
        .await
        .expect("create intent");
    assert!(
        payjoin_receive_credit_cap_resolved(&storage, Network::Regtest, &record)
            .await
            .expect("resolve")
    );
}

#[tokio::test]
async fn cut_through_receive_reuses_reserved_intent_before_exposure() {
    let (backend, _tmp) = build_test_backend().await;
    let receive_quote_id = "receive-quote".to_string();
    let send_intent_id = Uuid::new_v4();
    let intent = crate::send::payment_intent::record::SendIntentRecord {
        intent_id: send_intent_id,
        quote_id: "send-quote".to_string(),
        address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
        amount_sat: 40_000,
        max_fee_amount_sat: 1_000,
        tier: PaymentTier::Immediate,
        metadata: PaymentMetadata::default(),
        state: crate::send::payment_intent::record::SendIntentState::Pending {
            created_at: 1_700_000_000,
        },
    };
    backend
        .storage
        .create_send_intent_if_absent(&intent)
        .await
        .expect("store pending intent");
    let reservation_id = Uuid::new_v4();
    backend
        .storage
        .reserve_pending_send_intent_for_cut_through(
            &send_intent_id,
            reservation_id,
            &receive_quote_id,
            50_000,
        )
        .await
        .expect("reserve intent")
        .expect("reservation");

    let existing = backend
        .reserved_cut_through_candidate(&receive_quote_id, 50_000)
        .await
        .expect("load existing reservation");

    let (intent_record, reused_reservation_id) = existing.expect("reusable reservation");
    assert_eq!(reused_reservation_id, reservation_id);
    assert_eq!(intent_record.intent_id, send_intent_id);
    assert!(backend
        .storage
        .get_pending_send_intents()
        .await
        .expect("pending intents")
        .is_empty());
}

#[tokio::test]
async fn cut_through_receive_abandons_mismatched_reserved_intent_before_fallback() {
    let (backend, _tmp) = build_test_backend().await;
    let receive_quote_id = "receive-quote".to_string();
    let send_intent_id = Uuid::new_v4();
    let intent = crate::send::payment_intent::record::SendIntentRecord {
        intent_id: send_intent_id,
        quote_id: "send-quote".to_string(),
        address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
        amount_sat: 40_000,
        max_fee_amount_sat: 1_000,
        tier: PaymentTier::Immediate,
        metadata: PaymentMetadata::default(),
        state: crate::send::payment_intent::record::SendIntentState::Pending {
            created_at: 1_700_000_000,
        },
    };
    backend
        .storage
        .create_send_intent_if_absent(&intent)
        .await
        .expect("store pending intent");
    let reservation_id = Uuid::new_v4();
    backend
        .storage
        .reserve_pending_send_intent_for_cut_through(
            &send_intent_id,
            reservation_id,
            &receive_quote_id,
            60_000,
        )
        .await
        .expect("reserve intent")
        .expect("reservation");

    let existing = backend
        .reserved_cut_through_candidate(&receive_quote_id, 50_000)
        .await
        .expect("load existing reservation");

    assert!(existing.is_none());
    assert!(matches!(
        backend
            .storage
            .get_send_intent(&send_intent_id)
            .await
            .expect("load intent")
            .expect("intent")
            .state,
        crate::send::payment_intent::record::SendIntentState::Pending { .. }
    ));
}

#[tokio::test]
async fn payjoin_proposal_replay_detects_exposed_cut_through_settlement() {
    let (backend, _tmp) = build_test_backend().await;
    let quote_id = "receive-quote".to_string();
    let mut proposal_psbt = test_psbt_with_outputs(vec![TxOut {
        value: BitcoinAmount::from_sat(40_000),
        script_pubkey: ScriptBuf::new(),
    }]);
    proposal_psbt.inputs[0].witness_utxo = Some(TxOut {
        value: BitcoinAmount::from_sat(41_000),
        script_pubkey: ScriptBuf::new(),
    });
    proposal_psbt.inputs[0].final_script_witness = Some(Default::default());
    let proposal_tx = proposal_psbt
        .clone()
        .extract_tx()
        .expect("test proposal extracts");
    let session = crate::storage::PayjoinReceiveSessionRecord {
        quote_id: quote_id.clone(),
        fallback_address: "bcrt1qaddr".to_string(),
        amount_sat: 50_000,
        proposal_receiver_outpoints: Vec::new(),
        proposal_tx_bytes: Some(consensus::serialize(&proposal_tx)),
        cut_through: Some(crate::storage::PayjoinCutThroughProgress::Active {
            reservation_id: Uuid::new_v4(),
            send_intent_id: Uuid::new_v4(),
            proposal_txid: proposal_tx.compute_txid().to_string(),
        }),
        expires_at: 1_700_000_001,
        events: Vec::new(),
        closed: false,
    };
    backend
        .storage
        .put_payjoin_receive_session(&session)
        .await
        .expect("store exposed settlement");

    assert!(backend
        .exposed_cut_through_for_proposal(&quote_id, &proposal_psbt)
        .await
        .expect("lookup exposed settlement"));
    assert!(!backend
        .exposed_cut_through_for_proposal("other-quote", &proposal_psbt)
        .await
        .expect("lookup unrelated quote"));
}

#[test]
fn builds_payjoin_endpoint_from_normalized_fields() {
    let payjoin = PayjoinV2::new(
        "https://payjoin.example/pj".to_string(),
        "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ",
        "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG",
        1_720_547_781,
    )
    .expect("valid Payjoin keys");

    assert_eq!(
            cdk_common::payjoin::payjoin_v2_to_bip77_endpoint(&payjoin)
                .expect("endpoint builds"),
            "https://payjoin.example/pj#EX1C4UC6ES-OH1QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ-RK1QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG"
        );
}
