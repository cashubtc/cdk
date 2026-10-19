use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use cdk::federation::{
    federation_wallet_signing_attempt_at, FederationAddressAllocation, FederationAddressKeychain,
    FederationAllocatedAddressRef, FederationChainObservationState,
    FederationChainSourceDescriptor, FederationChainSourceScope, FederationDepositConfirmation,
    FederationDepositObservation, FederationFrostApplicationConfig,
    FederationFrostRootPublicConfig, FederationFrostWalletConfig,
    FederationOnchainConfirmationPolicy, FederationOnchainPaymentBatch,
    FederationOnchainPaymentIntent, FederationOnchainPaymentIntentRecord,
    FederationSignedTransaction, FederationTransactionChangeOutput, FederationTransactionInput,
    FederationTransactionProposal, FederationTransactionProposalId,
    FederationTransactionRecipientOutput, FederationTransactionScope, FederationTransactionState,
    FederationWalletLivenessSchedule,
};
use cdk_bitcoin_wallet::{
    ChainSource, EsploraConfig, FederatedBdkWallet, FederatedWalletConfig, FederatedWalletKeychain,
    FederatedWalletScope,
};
use cdk_common::bitcoin::hashes::Hash;
use cdk_common::bitcoin::{
    absolute, blockdata::constants::genesis_block, transaction, Amount as BitcoinAmount, Block,
    FeeRate, Network as BitcoinNetwork, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut,
    Txid, Witness,
};
use cdk_frost::{MemoryNonceStore, ParticipantKeyManager, SecretEncryptionKey};

use super::super::frost_signing::{
    FederationFrostSigningCoordinator, FederationFrostSigningRuntime,
};
use super::*;

#[derive(Default)]
struct BitcoinBackend {
    fee_rate: AtomicUsize,
    reject: AtomicBool,
    transactions: Mutex<Vec<Transaction>>,
    mempool: Mutex<BTreeSet<Txid>>,
}

async fn bitcoin_backend() -> (
    Arc<BitcoinBackend>,
    ChainSource,
    tokio::task::JoinHandle<()>,
) {
    let backend = Arc::new(BitcoinBackend::default());
    backend.fee_rate.store(2, Ordering::SeqCst);
    let genesis = genesis_block(BitcoinNetwork::Regtest);
    let blocks = serde_json::json!([{
        "id": genesis.block_hash(), "height": 0, "version": 1,
        "timestamp": genesis.header.time, "tx_count": 1,
        "size": 285, "weight": 1140, "merkle_root": genesis.header.merkle_root,
        "previousblockhash": null, "mediantime": genesis.header.time,
        "nonce": genesis.header.nonce, "bits": genesis.header.bits.to_consensus(),
        "difficulty": 1.0
    }]);
    let router = Router::new()
        .route("/blocks", get(move || async move { axum::Json(blocks) }))
        .route(
            "/fee-estimates",
            get(|State(backend): State<Arc<BitcoinBackend>>| async move {
                let rate = backend.fee_rate.load(Ordering::SeqCst);
                if rate == 0 {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                axum::Json(serde_json::json!({"1": rate, "6": rate, "144": rate})).into_response()
            }),
        )
        .route(
            "/tx",
            post(
                |State(backend): State<Arc<BitcoinBackend>>, body: String| async move {
                    let transaction: Transaction =
                        cdk_common::bitcoin::consensus::encode::deserialize_hex(&body)
                            .expect("broadcast transaction bytes");
                    let txid = transaction.compute_txid();
                    backend
                        .transactions
                        .lock()
                        .expect("broadcasts")
                        .push(transaction);
                    if backend.reject.load(Ordering::SeqCst) {
                        return (StatusCode::BAD_REQUEST, "insufficient fee".to_string());
                    }
                    match backend.mempool.lock().expect("mempool").insert(txid) {
                        true => (StatusCode::OK, txid.to_string()),
                        false => (
                            StatusCode::BAD_REQUEST,
                            "transaction already in mempool".to_string(),
                        ),
                    }
                },
            ),
        )
        .with_state(backend.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock Bitcoin backend");
    let source = ChainSource::Esplora(EsploraConfig {
        url: format!("http://{}", listener.local_addr().expect("mock address")),
        parallel_requests: 1,
        request_timeout_secs: 2,
        max_retries: 0,
    });
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("mock Bitcoin server");
    });
    (backend, source, server)
}

struct RbfFederation {
    states: Vec<MintState>,
    wallets: Vec<FederatedBdkWallet>,
    transport: RouterFederationTransportRegistry,
    backend: Arc<BitcoinBackend>,
    server: tokio::task::JoinHandle<()>,
    _directories: Vec<tempfile::TempDir>,
}

impl Drop for RbfFederation {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl RbfFederation {
    async fn new() -> (Self, FederationTransactionProposal) {
        let (mut config, first, second, third) = transport_config_three();
        let roots = super::frost_signing::frost_roots(config.federation_id.to_bytes(), 3, 2);
        let public = roots[0].public();
        let root = FederationFrostRootPublicConfig {
            version: 2,
            federation_id: config.federation_id,
            setup_authorization: config.setup_authorization,
            ceremony_id: FederationDkgCeremonyId::from_bytes([42; 32]),
            root_key_epoch: 1,
            dkg_protocol_version: public.dkg_protocol_version(),
            ciphersuite_version: 1,
            threshold: 2,
            participant_mapping: config
                .members
                .keys()
                .map(|member| (*member, member.get()))
                .collect(),
            verification_shares: public
                .verifying_shares()
                .iter()
                .map(|(member, share)| (MemberId::new(member.get()).expect("member"), *share))
                .collect(),
            untweaked_group_key: public.group_key(),
            transcript_binding: public.transcript_binding(),
            canonical_public_package: public.encode(),
        };
        let application = FederationFrostApplicationConfig::new_cdk_onchain(
            &config,
            &root,
            1,
            BitcoinNetwork::Regtest,
            FederationOnchainConfirmationPolicy::new(1).expect("confirmations"),
            1,
        )
        .expect("application");
        config.frost_wallet = Some(
            FederationFrostWalletConfig::new(
                &config,
                root,
                Some(application.clone()),
                Default::default(),
            )
            .expect("wallet config"),
        );
        let scope = FederationTransactionScope {
            root_key_epoch: 1,
            application_id: application.application_id,
            application_key_epoch: 1,
            network: BitcoinNetwork::Regtest,
            policy_version: 1,
        };
        let (backend, source, server) = bitcoin_backend().await;
        let transport = RouterFederationTransportRegistry::default();
        let mut states = Vec::new();
        let mut wallets = Vec::new();
        let mut directories = Vec::new();
        for (private, root) in [first, second, third].into_iter().zip(roots) {
            let mut state =
                federation_route_state_for_private(config.clone(), private.clone()).await;
            let mut manager = ParticipantKeyManager::with_nonce_store(
                root,
                SecretEncryptionKey::new([private.member_id.get() as u8; 32]).expect("key"),
                MemoryNonceStore::default(),
            )
            .expect("FROST manager");
            manager
                .register_application(
                    cdk::federation::FederationCdkOnchainFrostAuthorizationAdapter::new(
                        &application,
                    )
                    .expect("authorization adapter"),
                )
                .expect("register application");
            let signing = FederationFrostSigningRuntime::for_test(
                BTreeMap::from([(1, Arc::new(tokio::sync::Mutex::new(manager)))]),
                [],
            );
            state.federation.set_frost_signing_for_test(Some(signing));
            let directory = tempfile::tempdir().expect("member wallet directory");
            let wallet_scope = FederatedWalletScope::new(
                config.federation_id.to_bytes(),
                application.application_id.to_bytes(),
                1,
                1,
                application.descriptor_commitment,
            )
            .expect("wallet scope");
            let wallet = FederatedBdkWallet::open(
                FederatedWalletConfig::new(
                    wallet_scope,
                    &application.external_descriptor,
                    &application.internal_descriptor,
                    BitcoinNetwork::Regtest,
                )
                .expect("descriptors"),
                source.clone(),
                directory.path(),
            )
            .expect("BDK wallet");
            wallet.sync_once().await.expect("initial BDK sync");
            wallet
                .reconcile_revealed_counters(1, 2)
                .await
                .expect("reveal allocations");
            state
                .federation
                .components
                .wallet
                .onchain_wallets
                .push(wallet.clone());
            transport.insert(
                config
                    .member(private.member_id)
                    .expect("member")
                    .federation_api_url
                    .clone(),
                Router::new()
                    .nest("/api/federation/v1", create_federation_router())
                    .with_state(state.clone()),
            );
            wallets.push(wallet);
            states.push(state);
            directories.push(directory);
        }

        let mut chain = FederationChainObservationState::new();
        let mut selected = Vec::new();
        for (keychain, index) in [
            (FederationAddressKeychain::External, 0),
            (FederationAddressKeychain::Internal, 1),
        ] {
            let output = TxOut {
                value: BitcoinAmount::from_sat(50_000),
                script_pubkey: config
                    .frost_wallet
                    .as_ref()
                    .expect("wallet")
                    .derive_cdk_onchain_script(
                        &config,
                        application.application_id,
                        1,
                        keychain,
                        index,
                    )
                    .expect("script"),
            };
            selected.push((keychain, index, output));
        }
        let funding = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([99; 32]), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: selected
                .iter()
                .map(|(_, _, output)| output.clone())
                .collect(),
        };
        let mut inputs = Vec::new();
        for (vout, (keychain, index, output)) in selected.into_iter().enumerate() {
            let allocation = FederationAddressAllocation {
                quote: format!("rbf-deposit-{vout}"),
                keychain,
                derivation_index: index,
                operation_id: FederationOperationId::from_bytes([vout as u8 + 10; 32]),
                operation_index: vout as u64 + 1,
            };
            let allocation_ref = FederationAllocatedAddressRef {
                application_id: application.application_id,
                application_key_epoch: 1,
                quote: allocation.quote.clone(),
                keychain,
                derivation_index: index,
            };
            let outpoint = OutPoint::new(funding.compute_txid(), vout as u32);
            for observer in [member_id(1), member_id(2)] {
                chain
                    .apply_deposit(
                        &FederationDepositObservation {
                            allocation: allocation_ref.clone(),
                            outpoint,
                            txout: output.clone(),
                            confirmation: Some(FederationDepositConfirmation {
                                block_hash: genesis_block(BitcoinNetwork::Regtest).block_hash(),
                                block_height: 0,
                                confirmation_depth: 1,
                            }),
                            observation_epoch: 0,
                            observer,
                            source: FederationChainSourceDescriptor::new(
                                FederationChainSourceScope::Independent,
                                format!("rbf-source-{observer}"),
                            )
                            .expect("source"),
                        },
                        &allocation,
                        &output.script_pubkey,
                        2,
                        1,
                        FederationOperationId::from_bytes(
                            [observer.get() as u8 + vout as u8 + 20; 32],
                        ),
                        10 + vout as u64,
                    )
                    .expect("confirmed deposit");
            }
            assert_eq!(
                chain.utxo(outpoint).expect("deposit").state,
                cdk::federation::FederationWalletUtxoState::Spendable
            );
            inputs.push(FederationTransactionInput {
                outpoint,
                previous_output: output,
                allocation: allocation_ref,
            });
        }
        for wallet in &wallets {
            wallet
                .engine()
                .apply_accepted_transaction(funding.clone(), 1)
                .await
                .expect("apply funding");
        }
        let recipient = ScriptBuf::from_bytes([vec![0, 32], vec![7; 32]].concat());
        let intent = FederationOnchainPaymentIntent::new(
            "rbf-payment".to_string(),
            FederationOperationId::from_bytes([55; 32]),
            recipient.clone(),
            10_000,
            10_000,
            0,
            1,
            1,
            scope,
        )
        .expect("payment intent");
        let batch = FederationOnchainPaymentBatch::new(vec![intent.clone()], 1).expect("batch");
        let change_script = wallets[0]
            .peek_federated_script(FederatedWalletKeychain::Internal, 0)
            .await;
        let psbt = wallets[0]
            .engine()
            .build_federated_psbt(
                &[(recipient.clone(), 10_000)],
                FeeRate::from_sat_per_vb(2).expect("fee rate"),
                change_script.clone(),
                &inputs
                    .iter()
                    .map(|input| input.outpoint)
                    .collect::<Vec<_>>(),
            )
            .await
            .expect("build original through BDK");
        let fee = psbt.fee().expect("fee").to_sat();
        let recipient_index = psbt
            .unsigned_tx
            .output
            .iter()
            .position(|output| output.script_pubkey == recipient)
            .expect("recipient");
        let change_index = psbt
            .unsigned_tx
            .output
            .iter()
            .position(|output| output.script_pubkey == change_script)
            .expect("change");
        let proposal = FederationTransactionProposal::new(
            &batch,
            member_id(1),
            psbt.unsigned_tx.clone(),
            psbt.unsigned_tx
                .input
                .iter()
                .map(|input| {
                    inputs
                        .iter()
                        .find(|selected| selected.outpoint == input.previous_output)
                        .expect("selected input")
                        .clone()
                })
                .collect(),
            vec![FederationTransactionRecipientOutput {
                intent_id: intent.intent_id().expect("intent id"),
                output_index: recipient_index as u32,
                output: psbt.unsigned_tx.output[recipient_index].clone(),
                fee_contribution_sat: fee,
            }],
            Some(FederationTransactionChangeOutput {
                derivation_index: 0,
                output_index: change_index as u32,
                output: psbt.unsigned_tx.output[change_index].clone(),
            }),
            fee,
            None,
        )
        .expect("proposal");
        let mut transactions = FederationTransactionState::new();
        transactions
            .add_pending_intent(
                FederationOnchainPaymentIntentRecord::pending(intent).expect("intent record"),
            )
            .expect("pending intent");
        for state in &states {
            let mut tx = state
                .mint
                .localstore()
                .begin_transaction()
                .await
                .expect("seed transaction");
            tx.kv_write(
                "federation",
                "transaction",
                "state_v1",
                &transactions.to_bytes().expect("transactions"),
            )
            .await
            .expect("seed intents");
            tx.kv_write(
                "federation",
                "chain_observation",
                "state_v1",
                &chain.to_bytes().expect("chain"),
            )
            .await
            .expect("seed deposits");
            tx.commit().await.expect("commit fixture");
        }
        let change_allocation = states[0]
            .mint
            .federation_config()
            .expect("runtime")
            .address_allocation_operation(
                "rbf-change",
                application.application_id,
                1,
                FederationAddressKeychain::Internal,
            )
            .expect("change allocation");
        for state in &states {
            state
                .submit_federation_envelope(change_allocation.clone())
                .await
                .expect("submit change allocation")
                .ensure_accepted()
                .expect("allocate change");
        }
        (
            Self {
                states,
                wallets,
                transport,
                backend,
                server,
                _directories: directories,
            },
            proposal,
        )
    }

    async fn replicate(&self, source: usize) {
        let entries = self.states[source]
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal");
        for (index, state) in self.states.iter().enumerate() {
            if index == source {
                continue;
            }
            let retained = state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("replica journal");
            assert!(retained.len() <= entries.len());
            for (local, source) in retained.iter().zip(&entries) {
                assert_eq!(local.operation_id, source.operation_id);
            }
            for entry in entries.iter().skip(retained.len()) {
                state
                    .submit_federation_envelope(entry.envelope.clone())
                    .await
                    .expect("replay operation")
                    .ensure_accepted()
                    .expect("accepted replay");
            }
        }
    }

    async fn sign(&self, proposal: &FederationTransactionProposal) -> FederationSignedTransaction {
        let state = &self.states[0];
        let runtime = state.mint.federation_config().expect("runtime");
        let config = runtime.public_config();
        let application = config
            .frost_wallet
            .as_ref()
            .expect("wallet")
            .cdk_onchain_application
            .as_ref()
            .expect("application");
        let public = application
            .derived_public_package(config, &config.frost_wallet.as_ref().expect("wallet").root)
            .expect("public package");
        let client = FederationRestPeerClient::new(
            self.transport.clone(),
            config,
            runtime.private_config().clone(),
        )
        .expect("client");
        let attempt = federation_wallet_signing_attempt_at(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_millis(),
        );
        let plan = FederationWalletLivenessSchedule::new(
            proposal.proposal_id().expect("proposal id").to_bytes(),
            config.members.keys().copied(),
            2,
        )
        .expect("schedule")
        .signing_plan(attempt)
        .expect("signing plan");
        let signed = FederationFrostSigningCoordinator::new(&client, public)
            .with_local_signer(
                runtime.member_id(),
                state.mint.as_ref(),
                state.federation.frost_signing().expect("signing runtime"),
            )
            .sign_accepted_transaction(
                state,
                &self.wallets[0],
                proposal.proposal_id().expect("proposal id"),
                attempt,
                plan.signer_set
                    .iter()
                    .map(|member| cdk_frost::MemberId::new(member.get()).expect("signer"))
                    .collect(),
            )
            .await
            .expect("threshold sign and finalize through BDK");
        self.replicate(0).await;
        signed
    }

    async fn broadcast(
        &self,
        member: usize,
        proposal_id: FederationTransactionProposalId,
    ) -> usize {
        let state = &self.states[member];
        let advanced = state
            .federation
            .transaction_broadcaster
            .run_proposal_now(state, proposal_id)
            .await
            .expect("broadcast round");
        self.replicate(member).await;
        advanced
    }

    async fn propose_replacement(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis();
        for (index, state) in self.states.iter().enumerate() {
            let runtime = state.mint.federation_config().expect("runtime");
            let client = FederationRestPeerClient::new(
                self.transport.clone(),
                runtime.public_config(),
                runtime.private_config().clone(),
            )
            .expect("client");
            state
                .federation
                .wallet_liveness
                .run_round(state, &client, now)
                .await
                .expect("liveness round");
            self.replicate(index).await;
            // Only run the proposer phase; signing is driven explicitly below.
            let transactions = self.states[0]
                .mint
                .federation_transaction_state()
                .await
                .expect("transactions");
            if transactions.accepted_transactions().len() > 1 {
                break;
            }
        }
    }
}

#[tokio::test]
async fn rbf_replacement_finalizes_broadcasts_and_confirms_through_bdk() {
    assert_rbf_lifecycle(false).await;
}

#[tokio::test]
async fn rbf_original_can_confirm_after_replacement_broadcast_through_bdk() {
    assert_rbf_lifecycle(true).await;
}

async fn assert_rbf_lifecycle(original_wins: bool) {
    let (federation, original) = RbfFederation::new().await;
    let original_id = original.proposal_id().expect("original id");
    let runtime = federation.states[0]
        .mint
        .federation_config()
        .expect("runtime");
    federation.states[0]
        .submit_federation_envelope(
            runtime
                .transaction_proposal_operation(original.clone())
                .expect("proposal operation"),
        )
        .await
        .expect("submit original")
        .ensure_accepted()
        .expect("accept original");
    federation.replicate(0).await;
    let signed_original = federation.sign(&original).await;
    federation.broadcast(0, original_id).await; // Persist authorization.
    for _ in 0..5 {
        federation.broadcast(0, original_id).await;
    }
    let transactions = federation.states[0]
        .mint
        .federation_transaction_state()
        .await
        .expect("broadcast state");
    let broadcast = transactions
        .accepted_transaction(original_id)
        .expect("original")
        .broadcast
        .as_ref()
        .expect("broadcast record");
    assert_eq!(
        broadcast
            .first_success
            .as_ref()
            .expect("first success")
            .observation
            .result,
        cdk::federation::FederationTransactionBroadcastResult::Accepted
    );
    assert_eq!(broadcast.observations.len(), 1);
    assert_eq!(broadcast.observations[0].observation.attempt, 5);
    assert_eq!(
        broadcast.observations[0].observation.result,
        cdk::federation::FederationTransactionBroadcastResult::AlreadyKnown
    );
    federation.propose_replacement().await;
    assert_eq!(
        federation.states[0]
            .mint
            .federation_transaction_state()
            .await
            .expect("transactions")
            .accepted_transactions()
            .len(),
        1,
        "healthy rebroadcast attempts must not trigger RBF"
    );
    federation.backend.fee_rate.store(0, Ordering::SeqCst);
    federation.propose_replacement().await;
    assert_eq!(
        federation.states[0]
            .mint
            .federation_transaction_state()
            .await
            .expect("transactions")
            .accepted_transactions()
            .len(),
        1,
        "an unavailable estimator must not trigger RBF with a fallback"
    );
    federation.backend.fee_rate.store(8, Ordering::SeqCst);
    federation.propose_replacement().await;
    let transactions = federation.states[0]
        .mint
        .federation_transaction_state()
        .await
        .expect("transactions");
    let replacement = transactions
        .accepted_transactions()
        .values()
        .find(|accepted| accepted.proposal.replacement_of == Some(original_id))
        .expect("fee increase proposes RBF")
        .proposal
        .clone();
    let replacement_id = replacement.proposal_id().expect("replacement id");
    assert_eq!(replacement.selected_inputs, original.selected_inputs);
    assert_eq!(
        replacement.recipient_outputs[0].output,
        original.recipient_outputs[0].output
    );
    for input in &original.selected_inputs {
        assert_eq!(transactions.reservation(input.outpoint), Some(original_id));
        for wallet in &federation.wallets {
            assert!(wallet
                .engine()
                .list_unspent()
                .await
                .iter()
                .all(|utxo| utxo.outpoint != input.outpoint));
        }
    }
    assert_eq!(
        federation.broadcast(0, original_id).await,
        1,
        "rebroadcast original while replacement is unsigned"
    );
    let signed_replacement = federation.sign(&replacement).await;
    assert_eq!(signed_replacement.input_signatures.len(), 2);
    assert_ne!(
        signed_original.input_signatures[0].session_id,
        signed_replacement.input_signatures[0].session_id
    );
    federation.broadcast(0, replacement_id).await; // Persist authorization.
    assert_eq!(
        federation.broadcast(0, original_id).await,
        1,
        "authorization alone must not stop the original"
    );
    federation.backend.reject.store(true, Ordering::SeqCst);
    federation.broadcast(0, replacement_id).await;
    federation.backend.reject.store(false, Ordering::SeqCst);
    let transactions = federation.states[0]
        .mint
        .federation_transaction_state()
        .await
        .expect("rejected replacement state");
    let broadcast = transactions
        .accepted_transaction(replacement_id)
        .expect("replacement")
        .broadcast
        .as_ref()
        .expect("broadcast record");
    assert!(broadcast.first_success.is_none());
    assert_eq!(
        broadcast
            .observations
            .last()
            .expect("rejection")
            .observation
            .result,
        cdk::federation::FederationTransactionBroadcastResult::Rejected
    );
    assert_eq!(
        federation.broadcast(0, original_id).await,
        1,
        "failed replacement broadcast must retain original"
    );
    assert_eq!(federation.broadcast(0, replacement_id).await, 1);
    let before = federation
        .backend
        .transactions
        .lock()
        .expect("broadcasts")
        .len();
    assert_eq!(
        federation.broadcast(0, original_id).await,
        0,
        "successful replacement stops original rebroadcast"
    );
    assert_eq!(
        federation
            .backend
            .transactions
            .lock()
            .expect("broadcasts")
            .len(),
        before
    );

    let (winner_id, winner, loser_id) = match original_wins {
        true => (original_id, &signed_original, replacement_id),
        false => (replacement_id, &signed_replacement, original_id),
    };
    let genesis = genesis_block(BitcoinNetwork::Regtest);
    let mut block = Block {
        header: genesis.header,
        txdata: vec![winner.transaction().expect("winner bytes")],
    };
    block.header.prev_blockhash = genesis.block_hash();
    block.header.time += 1;
    block.header.merkle_root = block.compute_merkle_root().expect("merkle root");
    for wallet in &federation.wallets {
        let mut storage = wallet.engine().wallet_with_db.lock().await;
        storage
            .wallet
            .apply_block(&block, 1)
            .expect("confirm winner in BDK");
        storage.persist().expect("persist confirmation");
    }
    federation.broadcast(0, winner_id).await;
    federation.broadcast(1, winner_id).await;
    for state in &federation.states {
        let transactions = state
            .mint
            .federation_transaction_state()
            .await
            .expect("settled state");
        assert_eq!(
            transactions
                .accepted_transaction(winner_id)
                .expect("winner")
                .state,
            cdk::federation::FederationAcceptedTransactionState::Confirmed
        );
        assert_eq!(
            transactions
                .accepted_transaction(loser_id)
                .expect("loser")
                .state,
            cdk::federation::FederationAcceptedTransactionState::Abandoned
        );
        assert_eq!(
            transactions
                .completed_payment_for_quote("rbf-payment")
                .expect("receipt")
                .expect("completed payment")
                .txid,
            winner.txid
        );
        assert_eq!(
            transactions.lineage_head(original.batch_id),
            Some(winner_id)
        );
    }
    assert_eq!(
        federation.broadcast(0, loser_id).await,
        0,
        "terminal loser must remain inactive"
    );
}
