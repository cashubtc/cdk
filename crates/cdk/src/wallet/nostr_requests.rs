//! Durable Nostr payment requests and recovery of their receive operations.
//!
//! Use one repository (or its clones) per database while processing requests.
//! Its lock serializes receipt, recovery, and cancellation; the wallet KV API
//! does not provide cross-process compare-and-swap operations.

use std::str::FromStr;
use std::time::Duration;

use cdk_common::wallet::{TransactionDirection, TransactionId, TransactionStatus};
use nostr::prelude::{Filter, Keys, Kind, SecretKey, UnwrappedGift};
use nostr_sdk::prelude::{Client, RelayCapabilities, SignerAuthenticator};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use super::payment_request::payment_request_amount_for_wallet;
use super::{NostrWaitInfo, ReceiveOptions, WalletRepository};
use crate::mint_url::MintUrl;
use crate::nuts::{CurrencyUnit, PaymentRequest, PaymentRequestPayload, ProofsMethods, Token};
use crate::{Amount, Error};

const NAMESPACE: &str = "cdk";
const REQUESTS: &str = "nostr_requests";

/// Durable state of a Nostr payment request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NostrRequestStatus {
    /// Waiting for a matching payment.
    Pending,
    /// A receive operation must finish or be recovered before another attempt.
    Receiving,
    /// Payment was redeemed and recorded successfully.
    Completed,
    /// Cancelled locally; future payments will not be accepted.
    Cancelled,
}

/// Public view of a stored request, without its Nostr secret key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NostrRequest {
    /// Original request. Its payment ID identifies the stored record.
    pub request: PaymentRequest,
    /// Current durable status.
    pub status: NostrRequestStatus,
    /// Amount successfully received, net of mint input fees.
    pub received: Option<Amount>,
    /// Receive saga ID, retained after completion for transaction correlation.
    pub receive_operation_id: Option<Uuid>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredRequest {
    version: u8,
    public: NostrRequest,
    // Hex encoding is not encryption. This has the wallet database's protection.
    secret_key: Option<String>,
    relays: Vec<String>,
    trusted_mints: Vec<MintUrl>,
    receive_mint: Option<MintUrl>,
    receive_unit: Option<CurrencyUnit>,
    // Snapshot the net minimum, including applicable method fees, before redemption.
    #[serde(default)]
    required_net: Option<Amount>,
}

impl StoredRequest {
    fn info(&self) -> Result<NostrWaitInfo, Error> {
        let secret = self
            .secret_key
            .as_deref()
            .ok_or(Error::InvalidPaymentRequest)?;
        let keys =
            Keys::new(SecretKey::from_str(secret).map_err(|_| Error::InvalidPaymentRequest)?);
        Ok(NostrWaitInfo {
            pubkey: keys.public_key(),
            keys,
            relays: self.relays.clone(),
            request: self.public.request.clone(),
            trusted_mints: self.trusted_mints.clone(),
        })
    }

    fn id(&self) -> Result<&str, Error> {
        self.public
            .request
            .payment_id
            .as_deref()
            .ok_or(Error::InvalidPaymentRequest)
    }

    fn validate_received_amount(&self, amount: Amount) -> Result<(), Error> {
        self.info()?.validate_amount(amount)?;
        let required = match self.required_net {
            Some(required) => required,
            // Older attempts without method restrictions only required the base amount.
            None if self.public.request.supported_methods.is_empty() => {
                self.public.request.amount.unwrap_or(Amount::ZERO)
            }
            None => return Err(Error::InvalidPaymentRequest),
        };
        if amount < required {
            return Err(Error::InvalidPaymentRequest);
        }
        Ok(())
    }

    fn complete(&mut self, amount: Amount) -> Result<(), Error> {
        self.validate_received_amount(amount)?;
        self.public.status = NostrRequestStatus::Completed;
        self.public.received = Some(amount);
        self.secret_key = None;
        Ok(())
    }
}

impl WalletRepository {
    pub(super) async fn persist_nostr_request(&self, info: &NostrWaitInfo) -> Result<(), Error> {
        let _guard = self.nostr_request_lock.lock().await;
        let record = StoredRequest {
            version: 1,
            public: NostrRequest {
                request: info.request.clone(),
                status: NostrRequestStatus::Pending,
                received: None,
                receive_operation_id: None,
            },
            secret_key: Some(info.keys.secret_key().to_secret_hex()),
            relays: info.relays.clone(),
            trusted_mints: info.trusted_mints.clone(),
            receive_mint: None,
            receive_unit: None,
            required_net: None,
        };
        if self
            .localstore
            .kv_read(NAMESPACE, REQUESTS, record.id()?)
            .await?
            .is_some()
        {
            return Err(Error::InvalidPaymentRequest);
        }
        self.save_nostr_request(&record).await
    }

    async fn save_nostr_request(&self, record: &StoredRequest) -> Result<(), Error> {
        self.localstore
            .kv_write(
                NAMESPACE,
                REQUESTS,
                record.id()?,
                &serde_json::to_vec(record)?,
            )
            .await?;
        Ok(())
    }

    async fn read_nostr_request(&self, id: &str) -> Result<Option<StoredRequest>, Error> {
        let Some(bytes) = self.localstore.kv_read(NAMESPACE, REQUESTS, id).await? else {
            return Ok(None);
        };
        let record: StoredRequest = serde_json::from_slice(&bytes)?;
        if record.version != 1 || record.id()? != id {
            return Err(Error::InvalidPaymentRequest);
        }
        Ok(Some(record))
    }

    /// Read a request's saved status without making network requests.
    #[instrument(skip_all)]
    pub async fn get_nostr_request(&self, id: &str) -> Result<Option<NostrRequest>, Error> {
        Ok(self
            .read_nostr_request(id)
            .await?
            .map(|record| record.public))
    }

    /// List saved requests, including completed and cancelled requests, without networking.
    /// Pending and receiving requests can be checked or waited on after restarting.
    #[instrument(skip_all)]
    pub async fn list_nostr_requests(&self) -> Result<Vec<NostrRequest>, Error> {
        let mut requests = Vec::new();
        for id in self.localstore.kv_list(NAMESPACE, REQUESTS).await? {
            if let Some(request) = self.get_nostr_request(&id).await? {
                requests.push(request);
            }
        }
        Ok(requests)
    }

    // Called only while holding nostr_request_lock, never concurrently with receipt.
    async fn reconcile_nostr_request(&self, record: &mut StoredRequest) -> Result<(), Error> {
        if record.public.status != NostrRequestStatus::Receiving {
            return Ok(());
        }
        let operation_id = record
            .public
            .receive_operation_id
            .ok_or(Error::InvalidPaymentRequest)?;
        let mint = record
            .receive_mint
            .clone()
            .ok_or(Error::InvalidPaymentRequest)?;
        let unit = record
            .receive_unit
            .clone()
            .ok_or(Error::InvalidPaymentRequest)?;
        let transaction_id = TransactionId::from_saga_id(operation_id);
        let mut transaction = self.localstore.get_transaction(transaction_id).await?;
        // A completed transaction is authoritative even if saving the request failed.
        if !transaction
            .as_ref()
            .is_some_and(|tx| tx.status == TransactionStatus::Completed)
        {
            if let Some(saga) = self.localstore.get_saga(&operation_id).await? {
                let wallet = self
                    .get_or_create_wallet(mint.clone(), unit.clone(), None)
                    .await?;
                wallet.resume_receive_saga(&saga).await?;
                transaction = self.localstore.get_transaction(transaction_id).await?;
                if self.localstore.get_saga(&operation_id).await?.is_some() {
                    return Ok(());
                }
            }
        }
        match transaction {
            Some(tx) if tx.status == TransactionStatus::Completed => {
                if tx.mint_url != mint
                    || tx.unit != unit
                    || tx.direction != TransactionDirection::Incoming
                {
                    return Err(Error::InvalidPaymentRequest);
                }
                record.complete(tx.amount)?;
            }
            Some(tx) if tx.status == TransactionStatus::Pending => return Ok(()),
            _ => {
                // No successful or uncertain operation remains. A new payload may be tried.
                record.public.status = NostrRequestStatus::Pending;
                record.public.receive_operation_id = None;
                record.receive_mint = None;
                record.receive_unit = None;
                record.required_net = None;
            }
        }
        self.save_nostr_request(record).await
    }

    async fn recover_nostr_request(&self, id: &str) -> Result<StoredRequest, Error> {
        let _guard = self.nostr_request_lock.lock().await;
        let mut record = self
            .read_nostr_request(id)
            .await?
            .ok_or(Error::InvalidPaymentRequest)?;
        self.reconcile_nostr_request(&mut record).await?;
        Ok(record)
    }

    /// Validate and receive a payload using the saved request constraints.
    ///
    /// Repeated calls after completion return the original receipt without spending
    /// the supplied proofs. An uncertain receive must be recovered before retrying.
    /// Use one repository or its clones per database when processing requests.
    #[instrument(skip_all)]
    pub async fn receive_nostr_request(
        &self,
        id: &str,
        payload: PaymentRequestPayload,
    ) -> Result<Amount, Error> {
        let _guard = self.nostr_request_lock.lock().await;
        let mut record = self
            .read_nostr_request(id)
            .await?
            .ok_or(Error::InvalidPaymentRequest)?;
        self.reconcile_nostr_request(&mut record).await?;
        match record.public.status {
            NostrRequestStatus::Completed => {
                return record.public.received.ok_or(Error::InvalidPaymentRequest)
            }
            NostrRequestStatus::Cancelled => return Err(Error::PaymentFailed),
            NostrRequestStatus::Receiving => return Err(Error::PaymentPending),
            NostrRequestStatus::Pending => {}
        }
        let info = record.info()?;
        info.validate_payload(&payload)?;
        let wallet = self
            .get_or_create_wallet(payload.mint.clone(), payload.unit.clone(), None)
            .await?;
        record.required_net = Some(
            payment_request_amount_for_wallet(
                info.request.amount.unwrap_or(Amount::ZERO),
                &info.request,
                &wallet,
                &payload.unit,
            )
            .await?,
        );
        let fee = wallet.get_proofs_fee(&payload.proofs).await?.total;
        let net = payload
            .proofs
            .total_amount()?
            .checked_sub(fee)
            .ok_or(Error::InvalidPaymentRequest)?;
        record.validate_received_amount(net)?;
        let operation_id = Uuid::now_v7();
        record.public.status = NostrRequestStatus::Receiving;
        record.public.receive_operation_id = Some(operation_id);
        record.receive_mint = Some(payload.mint.clone());
        record.receive_unit = Some(payload.unit.clone());
        // This write must succeed before the receive saga can contact the mint.
        self.save_nostr_request(&record).await?;
        let token = Token::new(payload.mint, payload.proofs, payload.memo, payload.unit);
        let received = wallet
            .receive_with_operation_id(
                &token.to_string(),
                ReceiveOptions::default(),
                Some(operation_id),
            )
            .await?;
        record.complete(received)?;
        self.save_nostr_request(&record).await?;
        Ok(received)
    }

    /// Cancel a pending request and discard its Nostr secret key.
    ///
    /// An in-progress receipt is reconciled first. Returns `PaymentPending` if
    /// its outcome is uncertain, or `RequestAlreadyPaid` if it completed.
    /// This stops local acceptance; it cannot retract a request already shared.
    #[instrument(skip_all)]
    pub async fn cancel_nostr_request(&self, id: &str) -> Result<(), Error> {
        let _guard = self.nostr_request_lock.lock().await;
        let mut record = self
            .read_nostr_request(id)
            .await?
            .ok_or(Error::InvalidPaymentRequest)?;
        self.reconcile_nostr_request(&mut record).await?;
        match record.public.status {
            NostrRequestStatus::Completed => return Err(Error::RequestAlreadyPaid),
            NostrRequestStatus::Receiving => return Err(Error::PaymentPending),
            NostrRequestStatus::Cancelled => return Ok(()),
            NostrRequestStatus::Pending => {}
        }
        record.public.status = NostrRequestStatus::Cancelled;
        record.secret_key = None;
        self.save_nostr_request(&record).await
    }

    /// Recover an interrupted receipt and check relay history for a pending request.
    ///
    /// Fetches events retained by the configured relays, including payments sent
    /// while offline. A pending result means no matching redeemable payment was
    /// found. Relay retention is required; local persistence cannot guarantee delivery.
    #[instrument(skip_all)]
    pub async fn check_nostr_request(&self, id: &str) -> Result<NostrRequest, Error> {
        let record = self.recover_nostr_request(id).await?;
        if record.public.status != NostrRequestStatus::Pending {
            return Ok(record.public);
        }
        let info = record.info()?;
        let client = Client::builder()
            .authenticator(SignerAuthenticator::new(info.keys.clone()))
            .build();
        for relay in &info.relays {
            client
                .add_relay(relay)
                .capabilities(RelayCapabilities::READ)
                .await
                .map_err(|_| Error::InvalidPaymentRequest)?;
        }
        client.connect().await;
        let events = client
            .fetch_events(Filter::new().pubkey(info.pubkey).kind(Kind::GiftWrap))
            .timeout(Duration::from_secs(10))
            .await;
        client.disconnect().await;
        let events = events.map_err(|_| Error::PaymentPending)?;
        for event in events {
            let Ok(unwrapped) = UnwrappedGift::from_gift_wrap(&info.keys, &event) else {
                continue;
            };
            let Ok(payload) =
                serde_json::from_str::<PaymentRequestPayload>(&unwrapped.rumor.content)
            else {
                continue;
            };
            // Reject malformed/mismatched payloads without touching the database or mint.
            if info.validate_payload(&payload).is_err() {
                continue;
            }
            match self.receive_nostr_request(id, payload).await {
                Ok(_) => break,
                Err(error) => {
                    tracing::debug!(%error, "Nostr payment was not completed");
                    let record = self.recover_nostr_request(id).await?;
                    if record.public.status != NostrRequestStatus::Pending {
                        return Ok(record.public);
                    }
                }
            }
        }
        self.get_nostr_request(id)
            .await?
            .ok_or(Error::InvalidPaymentRequest)
    }

    /// Resume a saved request by ID until it is paid or cancelled.
    /// Dropping the future pauses listening and leaves the request available to resume.
    #[cfg(not(target_arch = "wasm32"))]
    #[instrument(skip_all)]
    pub async fn wait_for_nostr_request(&self, id: &str) -> Result<Amount, Error> {
        use futures::StreamExt;

        use super::streams::nostr::NostrPaymentEventStream;

        let record = self.recover_nostr_request(id).await?;
        if let Some(result) = terminal_result(&record.public) {
            return result;
        }
        let info = record.info()?;
        // The subscription has no `since` filter, so retained offline events are included.
        let mut stream =
            NostrPaymentEventStream::new(info.keys.clone(), info.relays.clone(), info.pubkey);
        let cancel = stream.cancel_token();
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let record = self.recover_nostr_request(id).await?;
                    if let Some(result) = terminal_result(&record.public) {
                        cancel.cancel();
                        return result;
                    }
                }
                item = stream.next() => {
                    match item {
                        Some(Ok(payload)) => match self.receive_nostr_request(id, payload).await {
                            Ok(amount) => { cancel.cancel(); return Ok(amount); }
                            Err(error) => tracing::debug!(%error, "Nostr payment was not completed"),
                        },
                        Some(Err(error)) => tracing::debug!(%error, "Invalid Nostr event"),
                        None => return Err(Error::PaymentPending),
                    }
                }
            }
        }
    }

    /// Resume a saved request by ID, periodically checking relay history on WASM.
    /// Dropping the future leaves the request available to resume.
    #[cfg(target_arch = "wasm32")]
    pub async fn wait_for_nostr_request(&self, id: &str) -> Result<Amount, Error> {
        loop {
            let request = self.check_nostr_request(id).await?;
            if let Some(result) = terminal_result(&request) {
                return result;
            }
            gloo_timers::future::TimeoutFuture::new(2_000).await;
        }
    }
}

fn terminal_result(request: &NostrRequest) -> Option<Result<Amount, Error>> {
    match request.status {
        NostrRequestStatus::Completed => Some(request.received.ok_or(Error::InvalidPaymentRequest)),
        NostrRequestStatus::Cancelled => Some(Err(Error::PaymentFailed)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::database::{Error as DatabaseError, WalletDatabase};

    use super::*;
    use crate::nuts::{BlindSignature, SwapResponse};
    use crate::wallet::test_utils::{
        create_test_db, test_keyset_id, test_mint_url, test_proof, MockMintConnector,
    };
    use crate::wallet::{CreateRequestParams, WalletConfig, WalletRepositoryBuilder};

    type TestDatabase = Arc<dyn WalletDatabase<DatabaseError> + Send + Sync>;

    async fn repository(db: TestDatabase) -> WalletRepository {
        WalletRepositoryBuilder::new()
            .localstore(db)
            .seed([0; 64])
            .build()
            .await
            .unwrap()
    }

    fn params() -> CreateRequestParams {
        CreateRequestParams {
            amount: Some(1),
            transport: "nostr".to_string(),
            nostr_relays: Some(vec!["wss://relay.example".to_string()]),
            mints: Some(vec![test_mint_url().to_string()]),
            ..Default::default()
        }
    }

    fn payload(info: &NostrWaitInfo) -> PaymentRequestPayload {
        PaymentRequestPayload {
            id: info.request.payment_id.clone(),
            memo: None,
            mint: test_mint_url(),
            unit: CurrencyUnit::Sat,
            proofs: vec![test_proof(test_keyset_id(), 1)],
        }
    }

    fn swap_response() -> SwapResponse {
        SwapResponse {
            signatures: vec![BlindSignature {
                amount: Amount::from(1),
                keyset_id: test_keyset_id(),
                c: crate::nuts::SecretKey::generate().public_key(),
                dleq: None,
            }],
        }
    }

    async fn add_mock_wallet(repo: &WalletRepository) -> Arc<MockMintConnector> {
        let connector = Arc::new(MockMintConnector::new());
        for keyset in connector.keysets.lock().unwrap().iter_mut() {
            keyset.input_fee_ppk = 0;
        }
        connector.set_post_swap_response(Ok(swap_response()));
        repo.create_wallet(
            test_mint_url(),
            CurrencyUnit::Sat,
            Some(WalletConfig {
                mint_connector: Some(connector.clone()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        connector
    }

    // Serve one connection, optionally replaying an event retained while offline.
    async fn serve_relay(
        listener: tokio::net::TcpListener,
        gift: Option<nostr::prelude::Event>,
        mut subscribed: Option<tokio::sync::oneshot::Sender<()>>,
    ) {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        while let Some(Ok(message)) = ws.next().await {
            if let Message::Text(text) = message {
                let request: serde_json::Value = serde_json::from_str(&text).unwrap();
                if request[0] == "REQ" {
                    if let Some(gift) = &gift {
                        assert!(request[2].get("since").is_none());
                        ws.send(Message::Text(
                            serde_json::json!(["EVENT", request[1], gift])
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                    }
                    ws.send(Message::Text(
                        serde_json::json!(["EOSE", request[1]]).to_string().into(),
                    ))
                    .await
                    .unwrap();
                    if let Some(subscribed) = subscribed.take() {
                        subscribed.send(()).unwrap();
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn method_requirements_are_checked_before_redemption() {
        // Listed mints waive mf, but must still support a requested melt method.
        for (listed, amount, method, method_fee, input_fee, accepted) in [
            (false, Some(1_u64), "bolt11", 1, 0, false),
            (true, Some(1), "bolt11", 100, 0, true),
            (true, Some(1), "unsupported", 0, 0, false),
            (false, None, "bolt11", 1, 0, true),
            (false, None, "bolt11", 2, 0, false),
            (false, None, "bolt11", 1, 1000, false),
        ] {
            let repo = repository(create_test_db().await).await;
            let (_, info) = repo.create_request(params()).await.unwrap();
            let info = info.unwrap();
            let id = info.request.payment_id.as_deref().unwrap();
            let connector = add_mock_wallet(&repo).await;
            for keyset in connector.keysets.lock().unwrap().iter_mut() {
                keyset.input_fee_ppk = input_fee;
            }
            let mut record = repo.read_nostr_request(id).await.unwrap().unwrap();
            record.public.request.amount = amount.map(Amount::from);
            record.public.request.supported_methods = vec![crate::nuts::SupportedMethod {
                method: method.to_string(),
                fee: Some(Amount::from(method_fee)),
            }];
            if !listed {
                record.public.request.mints.clear();
            }
            repo.save_nostr_request(&record).await.unwrap();
            let result = repo.receive_nostr_request(id, payload(&info)).await;
            assert_eq!(
                result.is_ok(),
                accepted,
                "{listed} {amount:?} {method} {method_fee} {input_fee}"
            );
            assert_eq!(
                connector.captured_swap_requests().len(),
                usize::from(accepted)
            );
            if !accepted {
                assert_eq!(
                    repo.get_nostr_request(id).await.unwrap().unwrap().status,
                    NostrRequestStatus::Pending
                );
            }
        }
    }

    #[tokio::test]
    async fn method_fee_minimum_survives_restart_and_checks_recovered_amount() {
        let db = create_test_db().await;
        let repo = repository(db.clone()).await;
        let (_, info) = repo.create_request(params()).await.unwrap();
        let info = info.unwrap();
        let id = info.request.payment_id.as_deref().unwrap();
        let connector = add_mock_wallet(&repo).await;
        let mut record = repo.read_nostr_request(id).await.unwrap().unwrap();
        record.public.request.amount = None;
        record.public.request.mints.clear();
        record.public.request.supported_methods = vec![
            crate::nuts::SupportedMethod {
                method: "bolt11".to_string(),
                fee: Some(Amount::from(3)),
            },
            crate::nuts::SupportedMethod {
                method: "bolt12".to_string(),
                fee: Some(Amount::from(1)),
            },
        ];
        repo.save_nostr_request(&record).await.unwrap();
        repo.receive_nostr_request(id, payload(&info))
            .await
            .unwrap();
        let completed = repo.read_nostr_request(id).await.unwrap().unwrap();
        assert_eq!(completed.required_net, Some(Amount::from(1)));
        record.public.status = NostrRequestStatus::Receiving;
        record.public.receive_operation_id = completed.public.receive_operation_id;
        record.receive_mint = completed.receive_mint;
        record.receive_unit = completed.receive_unit;
        record.required_net = Some(Amount::from(2));
        repo.save_nostr_request(&record).await.unwrap();
        drop(repo);

        // Recovery must enforce the saved minimum, without fetching mint info.
        let reopened = repository(db).await;
        assert!(reopened.check_nostr_request(id).await.is_err());
        record.required_net = completed.required_net;
        reopened.save_nostr_request(&record).await.unwrap();
        let recovered = reopened.check_nostr_request(id).await.unwrap();
        assert_eq!(recovered.status, NostrRequestStatus::Completed);
        assert_eq!(recovered.received, Some(Amount::from(1)));
        assert_eq!(connector.captured_swap_requests().len(), 1);
    }

    #[tokio::test]
    async fn request_survives_database_reopen_without_any_wallets() {
        let directory = std::env::temp_dir().join(format!("cdk-nostr-request-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("wallet.sqlite");
        let db = Arc::new(
            cdk_sqlite::wallet::WalletSqliteDatabase::new(path.clone())
                .await
                .unwrap(),
        );
        let repo = repository(db.clone()).await;
        let (request, info) = repo
            .create_request(CreateRequestParams {
                hash: Some("00".repeat(32)),
                ..params()
            })
            .await
            .unwrap();
        let info = info.unwrap();
        let id = request.payment_id.as_deref().unwrap();
        assert!(repo.get_wallets().await.is_empty());
        assert_eq!(repo.list_nostr_requests().await.unwrap().len(), 1);
        drop(repo);
        drop(db);

        let db = Arc::new(
            cdk_sqlite::wallet::WalletSqliteDatabase::new(path)
                .await
                .unwrap(),
        );
        let reopened = repository(db.clone()).await;
        let record = reopened.read_nostr_request(id).await.unwrap().unwrap();
        let restored = record.info().unwrap();
        assert_eq!(restored.keys.public_key(), info.keys.public_key());
        assert_eq!(restored.keys.secret_key(), info.keys.secret_key());
        assert_eq!(restored.request, request);
        assert_eq!(restored.trusted_mints, info.trusted_mints);
        assert_eq!(restored.relays, info.relays);
        assert_eq!(record.public.status, NostrRequestStatus::Pending);
        let public = serde_json::to_string(&record.public).unwrap();
        assert!(!public.contains(&info.keys.secret_key().to_secret_hex()));
        assert!(reopened.get_wallets().await.is_empty());
        drop(reopened);
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn concurrent_receipts_redeem_once_and_keep_a_completed_record() {
        let repo = repository(create_test_db().await).await;
        let (_, info) = repo.create_request(params()).await.unwrap();
        let info = info.unwrap();
        let connector = add_mock_wallet(&repo).await;
        let clone = repo.clone();
        let (first, second) = tokio::join!(
            repo.receive_nostr_payment(&info, payload(&info)),
            clone.receive_nostr_payment(&info, payload(&info)),
        );
        assert_eq!(first.unwrap(), Amount::from(1));
        assert_eq!(second.unwrap(), Amount::from(1));
        assert_eq!(connector.captured_swap_requests().len(), 1);
        let id = info.request.payment_id.as_deref().unwrap();
        let record = repo.read_nostr_request(id).await.unwrap().unwrap();
        assert_eq!(record.public.status, NostrRequestStatus::Completed);
        assert!(record.secret_key.is_none());
        let tx = repo
            .localstore
            .get_transaction(TransactionId::from_saga_id(
                record.public.receive_operation_id.unwrap(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tx.status, TransactionStatus::Completed);
        assert_eq!(tx.amount, record.public.received.unwrap());
        assert!(matches!(
            repo.cancel_nostr_request(id).await,
            Err(Error::RequestAlreadyPaid)
        ));
    }

    #[tokio::test]
    async fn restart_reconciles_redemption_before_request_completion_write() {
        let db = create_test_db().await;
        let repo = repository(db.clone()).await;
        let (_, info) = repo.create_request(params()).await.unwrap();
        let info = info.unwrap();
        let id = info.request.payment_id.as_deref().unwrap();
        let connector = add_mock_wallet(&repo).await;
        let mut before = repo.read_nostr_request(id).await.unwrap().unwrap();
        repo.receive_nostr_payment(&info, payload(&info))
            .await
            .unwrap();
        let completed = repo.get_nostr_request(id).await.unwrap().unwrap();
        // Leave the actual successful transaction/proofs intact, but model a crash
        // immediately before the final request KV write.
        before.public.status = NostrRequestStatus::Receiving;
        before.public.receive_operation_id = completed.receive_operation_id;
        before.receive_mint = Some(test_mint_url());
        before.receive_unit = Some(CurrencyUnit::Sat);
        repo.save_nostr_request(&before).await.unwrap();
        drop(repo);

        let reopened = repository(db).await;
        let recovered = reopened.check_nostr_request(id).await.unwrap();
        assert_eq!(recovered.status, NostrRequestStatus::Completed);
        assert_eq!(recovered.received, Some(Amount::from(1)));
        assert_eq!(
            reopened.wait_for_nostr_request(id).await.unwrap(),
            Amount::from(1)
        );
        assert_eq!(
            reopened
                .receive_nostr_payment(&info, payload(&info))
                .await
                .unwrap(),
            Amount::from(1)
        );
        assert_eq!(connector.captured_swap_requests().len(), 1);
    }

    #[tokio::test]
    async fn restart_resumes_the_original_ambiguous_receive_saga() {
        let db = create_test_db().await;
        let repo = repository(db.clone()).await;
        let (_, info) = repo.create_request(params()).await.unwrap();
        let info = info.unwrap();
        let id = info.request.payment_id.as_deref().unwrap();
        let connector = add_mock_wallet(&repo).await;
        connector.set_post_swap_response(Err(Error::HttpError(None, "disconnected".to_string())));
        assert!(repo
            .receive_nostr_payment(&info, payload(&info))
            .await
            .is_err());
        let receiving = repo.get_nostr_request(id).await.unwrap().unwrap();
        assert_eq!(receiving.status, NostrRequestStatus::Receiving);
        let operation_id = receiving.receive_operation_id.unwrap();
        assert!(db.get_saga(&operation_id).await.unwrap().is_some());
        drop(repo);

        let reopened = repository(db.clone()).await;
        let replay = add_mock_wallet(&reopened).await;
        let recovered = reopened.check_nostr_request(id).await.unwrap();
        assert_eq!(recovered.status, NostrRequestStatus::Completed);
        assert_eq!(recovered.receive_operation_id, Some(operation_id));
        assert!(db.get_saga(&operation_id).await.unwrap().is_none());
        assert_eq!(
            connector.captured_swap_requests()[0],
            replay.captured_swap_requests()[0]
        );
    }

    #[tokio::test]
    async fn cancellation_is_durable_and_idempotent() {
        let db = create_test_db().await;
        let repo = repository(db.clone()).await;
        let (_, info) = repo.create_request(params()).await.unwrap();
        let info = info.unwrap();
        let id = info.request.payment_id.as_deref().unwrap();
        repo.cancel_nostr_request(id).await.unwrap();
        drop(repo);
        let reopened = repository(db).await;
        reopened.cancel_nostr_request(id).await.unwrap();
        assert!(reopened
            .read_nostr_request(id)
            .await
            .unwrap()
            .unwrap()
            .secret_key
            .is_none());
        assert_eq!(
            reopened.check_nostr_request(id).await.unwrap().status,
            NostrRequestStatus::Cancelled
        );
        assert!(reopened.wait_for_nostr_request(id).await.is_err());
        assert!(reopened
            .receive_nostr_payment(&info, payload(&info))
            .await
            .is_err());
        assert!(reopened.get_wallets().await.is_empty());
    }

    #[tokio::test]
    async fn stopping_listener_preserves_request_unless_explicitly_cancelled() {
        for cancel_request in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let relay_url = format!("ws://{}", listener.local_addr().unwrap());
            let (subscribed, ready) = tokio::sync::oneshot::channel();
            let relay = tokio::spawn(serve_relay(listener, None, Some(subscribed)));
            let repo = repository(create_test_db().await).await;
            let (request, _) = repo
                .create_request(CreateRequestParams {
                    nostr_relays: Some(vec![relay_url]),
                    ..params()
                })
                .await
                .unwrap();
            let id = request.payment_id.unwrap();
            let waiter = repo.clone();
            let wait_id = id.clone();
            let waiting =
                tokio::spawn(async move { waiter.wait_for_nostr_request(&wait_id).await });
            tokio::time::timeout(Duration::from_secs(10), ready)
                .await
                .unwrap()
                .unwrap();
            if cancel_request {
                repo.cancel_nostr_request(&id).await.unwrap();
                assert!(tokio::time::timeout(Duration::from_secs(5), waiting)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err());
                assert_eq!(
                    repo.get_nostr_request(&id).await.unwrap().unwrap().status,
                    NostrRequestStatus::Cancelled
                );
            } else {
                waiting.abort();
                assert!(waiting.await.unwrap_err().is_cancelled());
                let saved = repo.read_nostr_request(&id).await.unwrap().unwrap();
                assert_eq!(saved.public.status, NostrRequestStatus::Pending);
                assert!(saved.secret_key.is_some());
            }
            tokio::time::timeout(Duration::from_secs(5), relay)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn offline_payment_is_fetched_and_received_after_restart() {
        use nostr::prelude::{FinalizeEvent, PrivateDirectMessageBuilder};

        for wait in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let relay_url = format!("ws://{}", listener.local_addr().unwrap());
            let db = create_test_db().await;
            let repo = repository(db.clone()).await;
            let (_, info) = repo
                .create_request(CreateRequestParams {
                    nostr_relays: Some(vec![relay_url]),
                    ..params()
                })
                .await
                .unwrap();
            let info = info.unwrap();
            let id = info.request.payment_id.clone().unwrap();
            drop(repo);
            // The relay has a gift wrap sent while the receiver is offline.
            let gift = PrivateDirectMessageBuilder::new(
                info.pubkey,
                serde_json::to_string(&payload(&info)).unwrap(),
            )
            .finalize(&Keys::generate())
            .unwrap();
            let relay = tokio::spawn(serve_relay(listener, Some(gift), None));
            let reopened = repository(db).await;
            let connector = add_mock_wallet(&reopened).await;
            let result = if wait {
                assert_eq!(
                    tokio::time::timeout(
                        Duration::from_secs(15),
                        reopened.wait_for_nostr_request(&id)
                    )
                    .await
                    .unwrap()
                    .unwrap(),
                    Amount::from(1)
                );
                reopened.get_nostr_request(&id).await.unwrap().unwrap()
            } else {
                tokio::time::timeout(Duration::from_secs(15), reopened.check_nostr_request(&id))
                    .await
                    .unwrap()
                    .unwrap()
            };
            assert_eq!(result.status, NostrRequestStatus::Completed);
            assert_eq!(result.received, Some(Amount::from(1)));
            assert_eq!(connector.captured_swap_requests().len(), 1);
            tokio::time::timeout(Duration::from_secs(2), relay)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
